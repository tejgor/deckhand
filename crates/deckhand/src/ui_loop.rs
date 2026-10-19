//! Port of cli.ts (ratatui <-> attach/$EDITOR hand-off).
//!
//! The UI process loop: run the ratatui app until it exits with a [`UiExitResult`], then either quit, hand the
//! terminal to an attached session pane (`deckhand_client::attach::attach_session`) or to the terminal editor for a
//! note, and come back to the UI. The UI preferences (`UiState`) live across those round trips in this loop and are
//! saved debounced while the UI runs and once more when it exits. The Terminal tab's view (shell or last action)
//! survives attach/detach, not restarts.

// Wave 0 skeleton: the binary does not reach these items until their unit wires them; drop this then.
#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use deckhand_core::error::DhError;
use deckhand_protocol::UiState;
use deckhand_protocol::data::{AttachTarget, EditorCommand, ProgramKey, RightPaneTab, UiExitResult};

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

/// TS: cli.ts `terminalView` (`'shell' | 'action'`): what the Terminal tab shows, kept across attach round trips.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminalView {
    #[default]
    Shell,
    Action,
}

/// TS: cli.ts clearTerminalScreen (TTY only: reset terminal modes, then [`CLEAR_TERMINAL_SEQUENCE`]).
pub fn clear_terminal_screen() {
    todo!("A1")
}

/// TS: cli.ts enterAlternateScreen (TTY only: reset terminal modes, then [`ENTER_ALTERNATE_SCREEN_SEQUENCE`]).
pub fn enter_alternate_screen() {
    todo!("A1")
}

/// TS: cli.ts leaveAlternateScreen (TTY only: reset terminal modes, then [`LEAVE_ALTERNATE_SCREEN_SEQUENCE`]).
pub fn leave_alternate_screen() {
    todo!("A1")
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
    let _ = (repo_root, ui_state, terminal_view);
    todo!("A1")
}

/// TS: cli.ts main's UI branch: loads the UI state of the cwd's repository (`ensure_git_repo`), then runs the UI and
/// performs each hand-off until it quits.
pub async fn run_ui_loop() -> Result<(), DhError> {
    todo!("A1")
}

/// TS: cli.ts the tab an attach target selects (`terminal`/`action` → terminal, `git`, `dev`, else preview).
pub fn attach_tab(target: AttachTarget) -> RightPaneTab {
    let _ = target;
    todo!("A1")
}

/// TS: cli.ts before an attach: select the session and the target's tab, remembered for the session too.
pub fn record_attach(ui_state: &mut UiState, session_id: &str, target: AttachTarget) {
    let _ = (ui_state, session_id, target);
    todo!("A1")
}

/// TS: cli.ts after attaching to the agent: its reply counts as read at `now_iso` (the entry is moved to the end, as
/// `delete` then assign does).
pub fn record_reply_read(ui_state: &mut UiState, session_id: &str, now_iso: String) {
    let _ = (ui_state, session_id, now_iso);
    todo!("A1")
}

/// TS: cli.ts before editing a note: select the session and its Notes tab.
pub fn record_edit_note(ui_state: &mut UiState, session_id: &str) {
    let _ = (ui_state, session_id);
    todo!("A1")
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
    let _ = (ui_state, session_id, target, title, program);
    todo!("A1")
}

/// TS: cli.ts the `edit-note` hand-off: records the selection, clears the screen, writes the notes vim script for a
/// vim-family editor (best effort), runs the editor in the foreground (it owns the terminal and stdin until it
/// quits). Only a failed launch is reported: `\n<command> failed: <message>; the note is <file>\n` on stderr and a
/// [`HAND_OFF_ERROR_PAUSE`]. Clears the screen again on the way back.
pub async fn hand_off_editor(ui_state: &Rc<RefCell<UiState>>, session_id: &str, file: &str, editor: &EditorCommand) {
    let _ = (ui_state, session_id, file, editor);
    todo!("A1")
}

/// TS: cli.ts `spawnSync(editor.command, terminalEditorArgs(...), {stdio: 'inherit'})`: blocks until the editor
/// quits. Errors only when it cannot be launched; its exit status is ignored (vim exits non-zero after any error
/// message seen while editing).
pub fn run_terminal_editor(editor: &EditorCommand, file: &str, vim_script: Option<&str>) -> std::io::Result<()> {
    let _ = (editor, file, vim_script);
    todo!("A1")
}
