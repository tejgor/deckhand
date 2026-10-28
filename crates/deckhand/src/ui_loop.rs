//! The UI loop and its hand-off to attach or `$EDITOR`.
//!
//! The UI process loop: run the ratatui app until it exits with a [`UiExitResult`], then either quit, hand the
//! terminal to an attached session pane (`deckhand_client::attach::attach_session`) or to the terminal editor for a
//! note, and come back to the UI. The UI preferences (`UiState`) live across those round trips in this loop and are
//! saved debounced while the UI runs and once more when it exits. The Terminal tab's view (shell or last action) and
//! the tasks whose steps the Tasks view hides survive attach/detach, not restarts.

use std::cell::{Cell, RefCell};
use std::io::{IsTerminal, Write};
use std::rc::Rc;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use deckhand_client::attach::{AttachSessionOptions, attach_session};
use deckhand_client::attach_status::StatusScope;
use deckhand_client::client::request;
use deckhand_client::term_state::reset_terminal_state;
use deckhand_core::display::{ThemeMode, theme_mode};
use deckhand_core::error::DhError;
use deckhand_protocol::UiState;
use deckhand_protocol::data::{AttachTarget, EditorCommand, RightPaneTab, UiExitResult};
use deckhand_protocol::records::SessionView;
use deckhand_protocol::wire::{Request, SetReplyRead};
use deckhand_sys::async_util::Timer;
use deckhand_sys::desktop::{is_vim_family, terminal_editor_args};
use deckhand_sys::git::ensure_git_repo;
use deckhand_sys::notes_vim::{NOTE_TITLE_ENV, write_notes_status_vim_script, write_notes_vim_script};
use deckhand_sys::storage::load_app_config;
use deckhand_sys::ui_state::{load_ui_state, save_ui_state};
use deckhand_ui::app::event_loop::{SavedTermios, capture_startup_termios, stdin_fd};
use deckhand_ui::app::state::{AppCallbacks, AppProps};

/// TS: cli.ts `terminalView` (`'shell' | 'action'`): what the Terminal tab shows, kept across attach round trips (the
/// UI's own type).
pub use deckhand_ui::panes::terminal::TerminalView;

/// TS: cli.ts scheduleSave's debounce of UI state saves.
pub const UI_STATE_SAVE_DEBOUNCE: Duration = Duration::from_millis(250);
/// TS: cli.ts the pause after a failed attach or editor launch, so its message can be read.
pub const HAND_OFF_ERROR_PAUSE: Duration = Duration::from_millis(1500);
/// TS: cli.ts clearTerminalScreen's sequence (leave every alternate screen, clear, home).
pub const CLEAR_TERMINAL_SEQUENCE: &str = "\x1b[?47l\x1b[?1047l\x1b[?1049l\x1b[2J\x1b[H";
/// TS: cli.ts enterAlternateScreen's sequence.
pub const ENTER_ALTERNATE_SCREEN_SEQUENCE: &str = "\x1b[?1049h\x1b[2J\x1b[H";
/// TS: cli.ts leaveAlternateScreen's sequence.
pub const LEAVE_ALTERNATE_SCREEN_SEQUENCE: &str = "\x1b[?1049l";

/// TTY only: reset terminal modes, then `sequence`.
fn write_screen_sequence(sequence: &str) {
    let mut stdout = std::io::stdout();
    if !stdout.is_terminal() {
        return;
    }
    reset_terminal_state();
    // Best effort, as a failed `process.stdout.write` is not reported either.
    let _ = stdout.write_all(sequence.as_bytes());
    let _ = stdout.flush();
}

/// TS: cli.ts clearTerminalScreen (TTY only: reset terminal modes, then [`CLEAR_TERMINAL_SEQUENCE`]).
pub fn clear_terminal_screen() {
    write_screen_sequence(CLEAR_TERMINAL_SEQUENCE);
}

/// TS: cli.ts enterAlternateScreen (TTY only: reset terminal modes, then [`ENTER_ALTERNATE_SCREEN_SEQUENCE`]).
pub fn enter_alternate_screen() {
    write_screen_sequence(ENTER_ALTERNATE_SCREEN_SEQUENCE);
}

/// TS: cli.ts leaveAlternateScreen (TTY only: reset terminal modes, then [`LEAVE_ALTERNATE_SCREEN_SEQUENCE`]).
pub fn leave_alternate_screen() {
    write_screen_sequence(LEAVE_ALTERNATE_SCREEN_SEQUENCE);
}

/// TS: cli.ts scheduleSave: the UI state is saved 250 ms after the last change (a pending save is replaced).
#[derive(Clone)]
struct SaveScheduler {
    repo_root: Rc<str>,
    ui_state: Rc<RefCell<UiState>>,
    timer: Rc<RefCell<Option<Timer>>>,
}

impl SaveScheduler {
    fn schedule(&self) {
        let repo_root = Rc::clone(&self.repo_root);
        let ui_state = Rc::clone(&self.ui_state);
        let timer = Timer::after(UI_STATE_SAVE_DEBOUNCE, move || {
            let snapshot = ui_state.borrow().clone();
            // `void saveUiState(...).catch(() => {})`: the save runs on; its failure is ignored.
            drop(save_ui_state(&repo_root, &snapshot));
        });
        let previous = self.timer.borrow_mut().replace(timer);
        drop(previous);
    }

    /// TS `clearTimeout(saveTimer)`.
    fn cancel(&self) {
        let pending = self.timer.borrow_mut().take();
        drop(pending);
    }

    /// Updates the UI state, then schedules its save.
    fn update(&self, change: impl FnOnce(&mut UiState)) {
        change(&mut self.ui_state.borrow_mut());
        self.schedule();
    }
}

/// What the UI keeps across attach round trips, not restarts (nor saved in `UiState`).
#[derive(Default)]
pub struct RoundTrip {
    /// The Terminal tab's view (TS: cli.ts `terminalView`).
    pub terminal_view: Rc<Cell<TerminalView>>,
    /// Not in the TS: the tasks whose steps the Tasks view hides.
    pub collapsed_task_steps: Rc<RefCell<Vec<String>>>,
}

/// TS: cli.ts the App props: `ui_state`'s initial values, and callbacks that update it and schedule its save.
fn app_props(
    repo_root: &str,
    ui_state: &Rc<RefCell<UiState>>,
    round_trip: &RoundTrip,
    saver: &SaveScheduler,
) -> AppProps {
    let state = ui_state.borrow();
    let on_visibility = saver.clone();
    let on_selected = saver.clone();
    let on_active = saver.clone();
    let on_session_tab = saver.clone();
    let on_collapsed = saver.clone();
    let on_hidden = saver.clone();
    let on_view = saver.clone();
    let on_exited = saver.clone();
    let view = Rc::clone(&round_trip.terminal_view);
    let collapsed_steps = Rc::clone(&round_trip.collapsed_task_steps);
    AppProps {
        repo_root: repo_root.to_owned(),
        cwd: repo_root.to_owned(),
        initial_selected_id: state.selected_id.clone(),
        initial_active_tab: state.active_tab,
        initial_session_tabs: Some(state.session_tabs.clone()),
        initial_collapsed_session_ids: Some(state.collapsed_session_ids.clone()),
        initial_hidden_exited_session_ids: Some(state.hidden_exited_session_ids.clone()),
        initial_session_filter: Some(state.session_filter),
        initial_session_query: Some(state.session_query.clone()),
        initial_session_view: state.session_view,
        initial_exited_collapsed: state.exited_collapsed,
        initial_terminal_view: Some(round_trip.terminal_view.get()),
        initial_dev_process: None,
        initial_collapsed_task_steps: Some(round_trip.collapsed_task_steps.borrow().clone()),
        callbacks: AppCallbacks {
            on_terminal_view_change: Some(Box::new(move |next| view.set(next))),
            on_session_visibility_change: Some(Box::new(move |filter, query| {
                on_visibility.update(|state| {
                    state.session_filter = filter;
                    state.session_query = query.to_owned();
                });
            })),
            on_session_view_change: Some(Box::new(move |view| {
                // The default (your own order) is not written.
                on_view.update(|state| state.session_view = (view != SessionView::Order).then_some(view));
            })),
            on_exited_collapsed_change: Some(Box::new(move |collapsed| {
                on_exited.update(|state| state.exited_collapsed = collapsed.then_some(true));
            })),
            on_selected_id_change: Some(Box::new(move |session_id| {
                on_selected.update(|state| state.selected_id = session_id.map(str::to_owned));
            })),
            on_active_tab_change: Some(Box::new(move |tab| {
                on_active.update(|state| state.active_tab = Some(tab));
            })),
            on_session_tab_change: Some(Box::new(move |session_id, tab| {
                on_session_tab.update(|state| {
                    state.session_tabs.insert(session_id.to_owned(), tab);
                });
            })),
            on_collapsed_session_ids_change: Some(Box::new(move |session_ids| {
                on_collapsed.update(|state| state.collapsed_session_ids = session_ids.to_vec());
            })),
            on_hidden_exited_session_ids_change: Some(Box::new(move |session_ids| {
                on_hidden.update(|state| state.hidden_exited_session_ids = session_ids.to_vec());
            })),
            on_collapsed_task_steps_change: Some(Box::new(move |task_ids| {
                *collapsed_steps.borrow_mut() = task_ids.to_vec();
            })),
        },
    }
}

/// TS: cli.ts runUi: enters the alternate screen, runs the app for `repo_root` with `ui_state`'s initial values and
/// change callbacks (each updates `ui_state` and schedules a debounced save), and on exit tears it down, restores the
/// terminal before any fallible I/O, then saves `ui_state` (failure: `deckhand: could not save UI state: <message>`
/// on stderr). `None`: the app ended without a result (treated as quit).
pub async fn run_ui(
    repo_root: &str,
    ui_state: &Rc<RefCell<UiState>>,
    round_trip: &RoundTrip,
    dev_process: Option<String>,
) -> Result<Option<UiExitResult>, DhError> {
    let saver = SaveScheduler {
        repo_root: Rc::from(repo_root),
        ui_state: Rc::clone(ui_state),
        timer: Rc::default(),
    };
    let mut props = app_props(repo_root, ui_state, round_trip, &saver);
    props.initial_dev_process = dev_process;
    deckhand_ui::theme::set_mode(configured_theme_mode().await);
    enter_alternate_screen();
    let result = deckhand_ui::app::run(props).await;
    saver.cancel();
    // Restore the terminal before any I/O that can fail.
    leave_alternate_screen();
    clear_terminal_screen();
    let snapshot = ui_state.borrow().clone();
    if let Err(error) = save_ui_state(repo_root, &snapshot).await {
        write_stderr(&format!("deckhand: could not save UI state: {error}\n"));
    }
    result.map_err(|error| DhError::msg(error.to_string()))
}

/// The theme the `theme` setting of config.json chooses (an unreadable config: terminal).
/// Read on every round trip, so an edited setting applies the next time the UI starts or comes back from an attach.
async fn configured_theme_mode() -> ThemeMode {
    let setting = load_app_config().await.ok().and_then(|config| config.theme);
    theme_mode(setting.as_deref())
}

/// The UI process's hold on its terminal, from startup until the UI loop ends (however it ends): saves stdin's startup
/// termios, and puts it back on the way out as Node's ResetStdio does at exit. Raw-mode entries only restore what they
/// found, which is raw when `$EDITOR` left the tty that way.
///
/// It is also the unwind guard for a panic in the UI or the attach proxy, whose terminal TS restores in `finally`
/// blocks: the panic hook ([`install_panic_hook`]) restores the terminal before the report prints, and dropping this
/// during the unwind puts the startup termios back once more after the raw-mode guards have restored theirs.
struct UiTerminal {
    saved: Option<SavedTermios>,
}

/// The termios the panic hook restores while a [`UiTerminal`] is held (taken by the first panic).
static PANIC_RESTORE: Mutex<Option<SavedTermios>> = Mutex::new(None);

impl UiTerminal {
    fn start(fd: std::os::fd::RawFd) -> UiTerminal {
        capture_startup_termios(fd);
        let saved = SavedTermios::capture(fd);
        install_panic_hook();
        *PANIC_RESTORE.lock().unwrap_or_else(PoisonError::into_inner) = saved;
        UiTerminal { saved }
    }
}

impl Drop for UiTerminal {
    fn drop(&mut self) {
        PANIC_RESTORE.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(saved) = &self.saved {
            saved.restore();
        }
    }
}

/// TS: what runUi's and attach's `finally` blocks undo, done before a panic's report prints: the startup termios,
/// then (TTY only) the modes the UI or the attached pane turned on, the alternate screen and the screen itself, as
/// `leaveAlternateScreen(); clearTerminalScreen()` do. Only while the UI loop holds the terminal, and once: the screen
/// is not cleared again under a second report.
fn install_panic_hook() {
    let report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // `try_lock`: a panic while the lock is held must still print its report.
        let saved = PANIC_RESTORE.try_lock().ok().and_then(|mut saved| saved.take());
        if let Some(saved) = saved {
            saved.restore();
            leave_alternate_screen();
            clear_terminal_screen();
        }
        report(info);
    }));
}

/// TS: cli.ts main's UI branch: loads the UI state of the cwd's repository (`ensure_git_repo`), then runs the UI and
/// performs each hand-off until it quits.
pub async fn run_ui_loop() -> Result<(), DhError> {
    let _terminal = UiTerminal::start(stdin_fd());
    let repo_root = ensure_git_repo(None).await?;
    let ui_state = Rc::new(RefCell::new(load_ui_state(&repo_root).await));
    let round_trip = RoundTrip::default();
    // The Dev process last attached, selected again when the UI comes back.
    let mut dev_process: Option<String> = None;
    loop {
        // TS: runUi resolves the repository again on every round trip.
        let repo_root = ensure_git_repo(None).await?;
        let result = run_ui(&repo_root, &ui_state, &round_trip, dev_process.clone()).await?;
        match result {
            None | Some(UiExitResult::Quit) => return Ok(()),
            Some(UiExitResult::Attach {
                session_id,
                target,
                title,
                process,
                shell,
                shell_label,
                status_title,
                ..
            }) => {
                if process.is_some() {
                    dev_process.clone_from(&process);
                }
                let title = title.as_deref();
                let pane = AttachPane {
                    process,
                    shell: shell.map(|shell| (shell, shell_label.unwrap_or_default())),
                };
                hand_off_attach(&ui_state, &repo_root, &session_id, target, title, status_title, pane).await;
            }
            Some(UiExitResult::EditNote {
                session_id,
                file,
                editor,
                title,
            }) => hand_off_editor(&ui_state, &session_id, &file, &editor, title.as_deref()).await,
        }
    }
}

/// TS: cli.ts the tab an attach target selects (`terminal`/`action` → terminal, `git`, `dev`, else preview).
pub fn attach_tab(target: AttachTarget) -> RightPaneTab {
    match target {
        AttachTarget::Terminal | AttachTarget::Action => RightPaneTab::Terminal,
        AttachTarget::Git => RightPaneTab::Git,
        AttachTarget::Dev => RightPaneTab::Dev,
        AttachTarget::Agent => RightPaneTab::Preview,
    }
}

/// TS: cli.ts before an attach: select the session and the target's tab, remembered for the session too.
pub fn record_attach(ui_state: &mut UiState, session_id: &str, target: AttachTarget) {
    let tab = attach_tab(target);
    ui_state.selected_id = Some(session_id.to_owned());
    ui_state.active_tab = Some(tab);
    ui_state.session_tabs.insert(session_id.to_owned(), tab);
}

/// TS: cli.ts after attaching to the agent: its reply counts as read now. The daemon records it (`set-reply-read`),
/// so every UI and the status line agree; a failure leaves the reply unread, which `U` or the next attach fixes.
async fn record_reply_read(session_id: &str) {
    let message = Request::SetReplyRead(SetReplyRead {
        request_id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.to_owned(),
        read: true,
    });
    drop(request::<()>(&message, None).await);
}

/// TS: cli.ts before editing a note: select the session and its Notes tab.
pub fn record_edit_note(ui_state: &mut UiState, session_id: &str) {
    ui_state.selected_id = Some(session_id.to_owned());
    ui_state.active_tab = Some(RightPaneTab::Notes);
    ui_state.session_tabs.insert(session_id.to_owned(), RightPaneTab::Notes);
}

/// Writes `text` to stderr (best effort).
fn write_stderr(text: &str) {
    let mut stderr = std::io::stderr();
    let _ = stderr.write_all(text.as_bytes());
    let _ = stderr.flush();
}

/// Not in the TS: which part of a workspace pane an attach is on: one of Dev's named processes, or one of the
/// Terminal's shells (by id, with its status-line label).
#[derive(Clone, Debug, Default)]
pub struct AttachPane {
    pub process: Option<String>,
    pub shell: Option<(i64, String)>,
}

/// TS: cli.ts the `attach` hand-off: records the selection, clears the screen, attaches with the configured scroll
/// sensitivity; only attaching to the agent marks its reply read (in the daemon). A failure prints `\nattach failed: <message>\n`
/// on stderr and pauses [`HAND_OFF_ERROR_PAUSE`]. Clears the screen again on the way back. The status line shows
/// `status_title` (the full title, with a sub-session's parent) and counts the sessions the UI of `repo_root` lists,
/// with its read replies.
pub async fn hand_off_attach(
    ui_state: &Rc<RefCell<UiState>>,
    repo_root: &str,
    session_id: &str,
    target: AttachTarget,
    title: Option<&str>,
    status_title: Option<String>,
    pane: AttachPane,
) {
    record_attach(&mut ui_state.borrow_mut(), session_id, target);
    clear_terminal_screen();
    let status_scope = StatusScope {
        root: Some(repo_root.to_owned()),
    };
    let attached = async {
        let config = load_app_config().await?;
        let options = AttachSessionOptions {
            title: title.map(str::to_owned),
            scroll_sensitivity: config.scroll_speed.map(|value| value.0),
            theme: theme_mode(config.theme.as_deref()),
            dev_process: pane.process,
            shell: pane.shell,
            status_title,
            status_scope,
        };
        attach_session(session_id, Some(target), options).await
    }
    .await;
    match attached {
        Ok(()) => {
            // Only attaching to the agent reads its reply; selecting the session or its other tabs does not.
            if target == AttachTarget::Agent {
                record_reply_read(session_id).await;
            }
        }
        Err(error) => {
            write_stderr(&format!("\nattach failed: {error}\n"));
            tokio::time::sleep(HAND_OFF_ERROR_PAUSE).await;
        }
    }
    clear_terminal_screen();
}

/// Node's message for a `spawnSync` that could not start (`spawnSync <command> ENOENT`).
fn spawn_error_message(command: &str, error: &std::io::Error) -> String {
    match error.raw_os_error() {
        Some(code) => format!("spawnSync {command} {:?}", nix::errno::Errno::from_raw(code)),
        None => error.to_string(),
    }
}

/// TS: cli.ts the `edit-note` hand-off: records the selection, clears the screen, writes the notes vim script for a
/// vim-family editor (best effort), runs the editor in the foreground (it owns the terminal and stdin until it
/// quits). Only a failed launch is reported: `\n<command> failed: <message>; the note is <file>\n` on stderr and a
/// [`HAND_OFF_ERROR_PAUSE`]. Clears the screen again on the way back. Not in the TS: a vim-family editor also gets
/// Deckhand's status bar, with `title` (the session's) on it.
pub async fn hand_off_editor(
    ui_state: &Rc<RefCell<UiState>>,
    session_id: &str,
    file: &str,
    editor: &EditorCommand,
    title: Option<&str>,
) {
    record_edit_note(&mut ui_state.borrow_mut(), session_id);
    clear_terminal_screen();
    // Without the scripts vim still edits the note; only the Deckhand keys or the status bar are missing.
    let (vim_script, status_script) = if is_vim_family(editor) {
        let keys = write_notes_vim_script().await.ok();
        let status = write_notes_status_vim_script(configured_theme_mode().await).await.ok();
        let path = |path: std::path::PathBuf| path.to_string_lossy().into_owned();
        (keys.map(path), status.map(path))
    } else {
        (None, None)
    };
    let status = status_script.as_deref().map(|script| EditorStatus {
        script,
        title: title.unwrap_or(session_id),
    });
    // The daemon's watcher picks up the saved file, so the Notes tab shows it when the UI comes back.
    if let Err(error) = run_terminal_editor(editor, file, vim_script.as_deref(), status) {
        write_stderr(&format!(
            "\n{} failed: {}; the note is {file}\n",
            editor.command,
            spawn_error_message(&editor.command, &error)
        ));
        tokio::time::sleep(HAND_OFF_ERROR_PAUSE).await;
    }
    clear_terminal_screen();
}

/// Not in the TS: the status bar for a vim-family editor: the script sourced after the note keys' (`-S`), and the
/// session title it shows (in [`NOTE_TITLE_ENV`]).
#[derive(Clone, Copy, Debug)]
pub struct EditorStatus<'a> {
    pub script: &'a str,
    pub title: &'a str,
}

/// TS: cli.ts `spawnSync(editor.command, terminalEditorArgs(...), {stdio: 'inherit'})`: blocks until the editor
/// quits. Errors only when it cannot be launched; its exit status is ignored (vim exits non-zero after any error
/// message seen while editing).
pub fn run_terminal_editor(
    editor: &EditorCommand,
    file: &str,
    vim_script: Option<&str>,
    status: Option<EditorStatus<'_>>,
) -> std::io::Result<()> {
    let mut command = std::process::Command::new(&editor.command);
    command.args(terminal_editor_args(editor, file, vim_script));
    if let Some(status) = status {
        command.arg("-S").arg(status.script).env(NOTE_TITLE_ENV, status.title);
    }
    command.status().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use deckhand_protocol::data::JsNumber;

    /// Held by the tests that hold a [`UiTerminal`]: [`PANIC_RESTORE`] and the panic hook are process-wide, so under
    /// `cargo test` (tests in parallel threads) one test's drop took the termios the other's panic was to restore.
    static UI_TERMINAL: Mutex<()> = Mutex::new(());

    /// Whether the terminal on `fd` is in canonical (cooked) mode with echo.
    fn is_cooked(fd: std::os::fd::RawFd) -> bool {
        let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: tcgetattr fills the termios on success, which the assert checks before it is read.
        let termios = unsafe {
            assert_eq!(libc::tcgetattr(fd, termios.as_mut_ptr()), 0);
            termios.assume_init()
        };
        let cooked = (libc::ICANON | libc::ECHO) as libc::tcflag_t;
        termios.c_lflag & cooked == cooked
    }

    #[test]
    fn leaving_the_ui_loop_restores_the_startup_termios() {
        use deckhand_ui::app::event_loop::RawMode;
        use std::os::fd::AsRawFd;
        let _serial = UI_TERMINAL.lock().unwrap_or_else(PoisonError::into_inner);
        let pty = nix::pty::openpty(None, None).unwrap_or_else(|error| panic!("{error}"));
        let fd = pty.slave.as_raw_fd();
        {
            let _terminal = UiTerminal::start(fd);
            // A UI round trip, then $EDITOR quits leaving the tty raw and the UI's next raw-mode entry saves that.
            drop(RawMode::enable(fd).unwrap_or_else(|error| panic!("{error}")));
            std::mem::forget(RawMode::enable(fd).unwrap_or_else(|error| panic!("{error}")));
            drop(RawMode::enable(fd).unwrap_or_else(|error| panic!("{error}")));
            assert!(!is_cooked(fd));
        }
        assert!(is_cooked(fd));
    }

    #[test]
    fn a_panic_restores_the_terminal_before_its_report_prints() {
        use deckhand_ui::app::event_loop::RawMode;
        use std::os::fd::AsRawFd;
        use std::sync::Arc;
        let _serial = UI_TERMINAL.lock().unwrap_or_else(PoisonError::into_inner);
        let pty = nix::pty::openpty(None, None).unwrap_or_else(|error| panic!("{error}"));
        let fd = pty.slave.as_raw_fd();
        // Stands in for the default hook that prints the report: records the tty's mode at that moment.
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        std::panic::set_hook(Box::new(move |_| {
            if let Ok(mut seen) = record.lock() {
                seen.push(is_cooked(fd));
            }
        }));
        let outcome = std::panic::catch_unwind(|| {
            let _terminal = UiTerminal::start(fd);
            // The UI (or the attach proxy) holds the tty raw when it panics; the editor had left it raw too.
            std::mem::forget(RawMode::enable(fd).unwrap_or_else(|error| panic!("{error}")));
            let _raw = RawMode::enable(fd).unwrap_or_else(|error| panic!("{error}"));
            panic!("boom");
        });
        drop(std::panic::take_hook());
        assert!(outcome.is_err());
        assert_eq!(seen.lock().map(|seen| seen.clone()).ok(), Some(vec![true]));
        // Unwinding the raw-mode guards does not leave it raw again.
        assert!(is_cooked(fd));
        // Outside the UI the hook leaves the terminal alone.
        std::mem::forget(RawMode::enable(fd).unwrap_or_else(|error| panic!("{error}")));
        let record = Arc::clone(&seen);
        std::panic::set_hook(Box::new(move |_| {
            if let Ok(mut seen) = record.lock() {
                seen.push(is_cooked(fd));
            }
        }));
        install_panic_hook();
        let outcome = std::panic::catch_unwind(|| panic!("outside"));
        drop(std::panic::take_hook());
        assert!(outcome.is_err());
        assert_eq!(seen.lock().map(|seen| seen.clone()).ok(), Some(vec![true, false]));
    }

    #[test]
    fn attach_targets_select_their_tab() {
        assert_eq!(attach_tab(AttachTarget::Agent), RightPaneTab::Preview);
        assert_eq!(attach_tab(AttachTarget::Terminal), RightPaneTab::Terminal);
        assert_eq!(attach_tab(AttachTarget::Action), RightPaneTab::Terminal);
        assert_eq!(attach_tab(AttachTarget::Git), RightPaneTab::Git);
        assert_eq!(attach_tab(AttachTarget::Dev), RightPaneTab::Dev);
    }

    #[test]
    fn hand_offs_record_the_selection() {
        let mut state = UiState::default();
        record_attach(&mut state, "a", AttachTarget::Action);
        assert_eq!(state.selected_id.as_deref(), Some("a"));
        assert_eq!(state.active_tab, Some(RightPaneTab::Terminal));
        assert_eq!(state.session_tabs.get("a"), Some(&RightPaneTab::Terminal));
        record_edit_note(&mut state, "b");
        assert_eq!(state.selected_id.as_deref(), Some("b"));
        assert_eq!(state.active_tab, Some(RightPaneTab::Notes));
        assert_eq!(state.session_tabs.get("b"), Some(&RightPaneTab::Notes));
    }

    #[test]
    fn a_missing_editor_fails_like_spawn_sync() {
        let editor = EditorCommand {
            command: "/nonexistent/deckhand-editor".to_owned(),
            args: Vec::new(),
            label: "deckhand-editor".to_owned(),
        };
        let error = run_terminal_editor(&editor, "/tmp/note.md", None, None).err();
        let message = error.map(|error| spawn_error_message(&editor.command, &error));
        assert_eq!(
            message.as_deref(),
            Some("spawnSync /nonexistent/deckhand-editor ENOENT")
        );
        let ok = EditorCommand {
            command: "true".to_owned(),
            args: Vec::new(),
            label: "true".to_owned(),
        };
        assert!(run_terminal_editor(&ok, "/tmp/note.md", None, None).is_ok());
    }

    #[test]
    fn app_props_carry_the_ui_state_and_save_its_changes() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| panic!("{error}"));
        tokio::task::LocalSet::new().block_on(&runtime, async {
            let ui_state = Rc::new(RefCell::new(UiState::default()));
            ui_state.borrow_mut().selected_id = Some("a".to_owned());
            ui_state.borrow_mut().sidebar_width = Some(JsNumber(30.7));
            let round_trip = RoundTrip::default();
            round_trip.terminal_view.set(TerminalView::Action);
            round_trip.collapsed_task_steps.borrow_mut().push("t1".to_owned());
            let saver = SaveScheduler {
                repo_root: Rc::from("/repo"),
                ui_state: Rc::clone(&ui_state),
                timer: Rc::default(),
            };
            let props = app_props("/repo", &ui_state, &round_trip, &saver);
            assert_eq!(props.initial_selected_id.as_deref(), Some("a"));
            assert_eq!(props.initial_terminal_view, Some(TerminalView::Action));
            assert_eq!(props.initial_collapsed_task_steps, Some(vec!["t1".to_owned()]));
            if let Some(listener) = &props.callbacks.on_active_tab_change {
                listener(RightPaneTab::Dev);
            }
            if let Some(listener) = &props.callbacks.on_terminal_view_change {
                listener(TerminalView::Shell(0));
            }
            if let Some(listener) = &props.callbacks.on_collapsed_task_steps_change {
                listener(&["t2".to_owned()]);
            }
            assert_eq!(ui_state.borrow().active_tab, Some(RightPaneTab::Dev));
            assert_eq!(round_trip.terminal_view.get(), TerminalView::Shell(0));
            assert_eq!(*round_trip.collapsed_task_steps.borrow(), ["t2"]);
            assert!(saver.timer.borrow().as_ref().is_some_and(Timer::is_pending));
            saver.cancel();
            assert!(saver.timer.borrow().is_none());
        });
    }
}
