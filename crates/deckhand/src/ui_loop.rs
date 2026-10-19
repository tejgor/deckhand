//! Port of cli.ts (ratatui <-> attach/$EDITOR hand-off).
//!
//! The UI process loop: run the ratatui app until it exits with a [`UiExitResult`], then either quit, hand the
//! terminal to an attached session pane (`deckhand_client::attach::attach_session`) or to the terminal editor for a
//! note, and come back to the UI. The UI preferences (`UiState`) live across those round trips in this loop and are
//! saved debounced while the UI runs and once more when it exits. The Terminal tab's view (shell or last action)
//! survives attach/detach, not restarts.

use std::cell::{Cell, RefCell};
use std::io::{IsTerminal, Write};
use std::rc::Rc;
use std::time::Duration;

use deckhand_client::attach::{AttachSessionOptions, attach_session};
use deckhand_client::term_state::reset_terminal_state;
use deckhand_core::error::DhError;
use deckhand_core::jsstr::date;
use deckhand_protocol::UiState;
use deckhand_protocol::data::{AttachTarget, EditorCommand, JsNumber, ProgramKey, RightPaneTab, UiExitResult};
use deckhand_sys::async_util::Timer;
use deckhand_sys::desktop::{is_vim_family, terminal_editor_args};
use deckhand_sys::git::ensure_git_repo;
use deckhand_sys::notes_vim::write_notes_vim_script;
use deckhand_sys::storage::load_app_config;
use deckhand_sys::ui_state::{load_ui_state, save_ui_state};
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

/// TS: cli.ts the App props: `ui_state`'s initial values, and callbacks that update it and schedule its save.
fn app_props(
    repo_root: &str,
    ui_state: &Rc<RefCell<UiState>>,
    terminal_view: &Rc<Cell<TerminalView>>,
    saver: &SaveScheduler,
) -> AppProps {
    let state = ui_state.borrow();
    let on_visibility = saver.clone();
    let on_selected = saver.clone();
    let on_active = saver.clone();
    let on_session_tab = saver.clone();
    let on_width = saver.clone();
    let on_collapsed = saver.clone();
    let on_hidden = saver.clone();
    let view = Rc::clone(terminal_view);
    AppProps {
        repo_root: repo_root.to_owned(),
        cwd: repo_root.to_owned(),
        initial_selected_id: state.selected_id.clone(),
        initial_active_tab: state.active_tab,
        initial_sidebar_width: state
            .sidebar_width
            .map(|width| width.0)
            .filter(|width| width.is_finite())
            .map(|width| width.floor() as i64),
        initial_session_tabs: Some(state.session_tabs.clone()),
        initial_collapsed_session_ids: Some(state.collapsed_session_ids.clone()),
        initial_hidden_exited_session_ids: Some(state.hidden_exited_session_ids.clone()),
        read_replies: Some(state.read_replies.clone()),
        initial_session_filter: Some(state.session_filter),
        initial_session_query: Some(state.session_query.clone()),
        initial_terminal_view: Some(terminal_view.get()),
        callbacks: AppCallbacks {
            on_terminal_view_change: Some(Box::new(move |next| view.set(next))),
            on_session_visibility_change: Some(Box::new(move |filter, query| {
                on_visibility.update(|state| {
                    state.session_filter = filter;
                    state.session_query = query.to_owned();
                });
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
            on_sidebar_width_change: Some(Box::new(move |width| {
                on_width.update(|state| state.sidebar_width = Some(JsNumber(width as f64)));
            })),
            on_collapsed_session_ids_change: Some(Box::new(move |session_ids| {
                on_collapsed.update(|state| state.collapsed_session_ids = session_ids.to_vec());
            })),
            on_hidden_exited_session_ids_change: Some(Box::new(move |session_ids| {
                on_hidden.update(|state| state.hidden_exited_session_ids = session_ids.to_vec());
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
    terminal_view: &Rc<Cell<TerminalView>>,
) -> Result<Option<UiExitResult>, DhError> {
    let saver = SaveScheduler {
        repo_root: Rc::from(repo_root),
        ui_state: Rc::clone(ui_state),
        timer: Rc::default(),
    };
    let props = app_props(repo_root, ui_state, terminal_view, &saver);
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

/// TS: cli.ts main's UI branch: loads the UI state of the cwd's repository (`ensure_git_repo`), then runs the UI and
/// performs each hand-off until it quits.
pub async fn run_ui_loop() -> Result<(), DhError> {
    let repo_root = ensure_git_repo(None).await?;
    let ui_state = Rc::new(RefCell::new(load_ui_state(&repo_root).await));
    let terminal_view = Rc::new(Cell::new(TerminalView::Shell));
    loop {
        // TS: runUi resolves the repository again on every round trip.
        let repo_root = ensure_git_repo(None).await?;
        let result = run_ui(&repo_root, &ui_state, &terminal_view).await?;
        match result {
            None | Some(UiExitResult::Quit) => return Ok(()),
            Some(UiExitResult::Attach {
                session_id,
                target,
                title,
                program,
                ..
            }) => hand_off_attach(&ui_state, &session_id, target, title.as_deref(), program).await,
            Some(UiExitResult::EditNote {
                session_id,
                file,
                editor,
            }) => hand_off_editor(&ui_state, &session_id, &file, &editor).await,
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

/// TS: cli.ts after attaching to the agent: its reply counts as read at `now_iso` (the entry is moved to the end, as
/// `delete` then assign does).
pub fn record_reply_read(ui_state: &mut UiState, session_id: &str, now_iso: String) {
    ui_state.read_replies.shift_remove(session_id);
    ui_state.read_replies.insert(session_id.to_owned(), now_iso);
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

/// TS: cli.ts the `attach` hand-off: records the selection, clears the screen, attaches with the configured scroll
/// sensitivity; only attaching to the agent marks its reply read. A failure prints `\nattach failed: <message>\n`
/// on stderr and pauses [`HAND_OFF_ERROR_PAUSE`]. Clears the screen again on the way back.
pub async fn hand_off_attach(
    ui_state: &Rc<RefCell<UiState>>,
    session_id: &str,
    target: AttachTarget,
    title: Option<&str>,
    program: Option<ProgramKey>,
) {
    record_attach(&mut ui_state.borrow_mut(), session_id, target);
    clear_terminal_screen();
    let attached = async {
        let config = load_app_config().await?;
        let options = AttachSessionOptions {
            title: title.map(str::to_owned),
            program,
            scroll_sensitivity: config.attach_scroll_sensitivity.map(|value| value.0),
        };
        attach_session(session_id, Some(target), options).await
    }
    .await;
    match attached {
        Ok(()) => {
            // Only attaching to the agent reads its reply; selecting the session or its other tabs does not.
            if target == AttachTarget::Agent {
                let now = date::to_iso_string(date::now_ms()).unwrap_or_default();
                record_reply_read(&mut ui_state.borrow_mut(), session_id, now);
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
/// [`HAND_OFF_ERROR_PAUSE`]. Clears the screen again on the way back.
pub async fn hand_off_editor(ui_state: &Rc<RefCell<UiState>>, session_id: &str, file: &str, editor: &EditorCommand) {
    record_edit_note(&mut ui_state.borrow_mut(), session_id);
    clear_terminal_screen();
    // Without the script vim still edits the note; only the Deckhand keys are missing.
    let vim_script = if is_vim_family(editor) {
        write_notes_vim_script()
            .await
            .ok()
            .map(|path| path.to_string_lossy().into_owned())
    } else {
        None
    };
    // The daemon's watcher picks up the saved file, so the Notes tab shows it when the UI comes back.
    if let Err(error) = run_terminal_editor(editor, file, vim_script.as_deref()) {
        write_stderr(&format!(
            "\n{} failed: {}; the note is {file}\n",
            editor.command,
            spawn_error_message(&editor.command, &error)
        ));
        tokio::time::sleep(HAND_OFF_ERROR_PAUSE).await;
    }
    clear_terminal_screen();
}

/// TS: cli.ts `spawnSync(editor.command, terminalEditorArgs(...), {stdio: 'inherit'})`: blocks until the editor
/// quits. Errors only when it cannot be launched; its exit status is ignored (vim exits non-zero after any error
/// message seen while editing).
pub fn run_terminal_editor(editor: &EditorCommand, file: &str, vim_script: Option<&str>) -> std::io::Result<()> {
    std::process::Command::new(&editor.command)
        .args(terminal_editor_args(editor, file, vim_script))
        .status()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        record_reply_read(&mut state, "a", "2026-01-01T00:00:00.000Z".to_owned());
        record_reply_read(&mut state, "b", "2026-01-02T00:00:00.000Z".to_owned());
        record_reply_read(&mut state, "a", "2026-01-03T00:00:00.000Z".to_owned());
        let order: Vec<&str> = state.read_replies.keys().map(String::as_str).collect();
        assert_eq!(order, ["b", "a"]);
        assert_eq!(
            state.read_replies.get("a").map(String::as_str),
            Some("2026-01-03T00:00:00.000Z")
        );
    }

    #[test]
    fn a_missing_editor_fails_like_spawn_sync() {
        let editor = EditorCommand {
            command: "/nonexistent/deckhand-editor".to_owned(),
            args: Vec::new(),
            label: "deckhand-editor".to_owned(),
        };
        let error = run_terminal_editor(&editor, "/tmp/note.md", None).err();
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
        assert!(run_terminal_editor(&ok, "/tmp/note.md", None).is_ok());
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
            let view = Rc::new(Cell::new(TerminalView::Action));
            let saver = SaveScheduler {
                repo_root: Rc::from("/repo"),
                ui_state: Rc::clone(&ui_state),
                timer: Rc::default(),
            };
            let props = app_props("/repo", &ui_state, &view, &saver);
            assert_eq!(props.initial_selected_id.as_deref(), Some("a"));
            assert_eq!(props.initial_sidebar_width, Some(30));
            assert_eq!(props.initial_terminal_view, Some(TerminalView::Action));
            if let Some(listener) = &props.callbacks.on_active_tab_change {
                listener(RightPaneTab::Dev);
            }
            if let Some(listener) = &props.callbacks.on_terminal_view_change {
                listener(TerminalView::Shell);
            }
            assert_eq!(ui_state.borrow().active_tab, Some(RightPaneTab::Dev));
            assert_eq!(view.get(), TerminalView::Shell);
            assert!(saver.timer.borrow().as_ref().is_some_and(Timer::is_pending));
            saver.cancel();
            assert!(saver.timer.borrow().is_none());
        });
    }
}
