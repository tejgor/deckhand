//! Port of app.tsx's browse keys (`if (mode === 'browse')`): the session list and the tabbed right pane. Selecting,
//! tabs, the lifecycle keys (stop, resume, archive, done, merge), opening what a tab shows, and the doors to every
//! other screen.

use deckhand_core::key::Input;
use deckhand_protocol::data::{MoveDirection, RestartMode};

use crate::app::state::{App, AppCallback};
use crate::keymap::Keymap;

/// The browse view's keys (keysDocumented: every one in the ? guide).
pub static KEYMAP: Keymap = &[];

/// TS: the browse branch of the useTerminalInput handler.
pub fn handle_browse(app: &mut App, input: &Input) {
    let _ = (app, input);
    todo!("A2")
}

/// TS: Tab / ] next tab, Shift+Tab / [ previous (`step` 1 or RIGHT_TABS.length - 1).
pub fn cycle_tab(app: &mut App, step: usize) {
    let _ = (app, step);
    todo!("A2")
}

/// TS `confirmNumericSelection`: select visible session number `value`, or say there is none.
pub fn confirm_numeric_selection(app: &mut App, value: &str) {
    let _ = (app, value);
    todo!("A2")
}

/// TS `moveSelection` (j/k, wrapping).
pub fn move_selection(app: &mut App, delta: isize) {
    let _ = (app, delta);
    todo!("A2")
}

/// TS `toggleSelectedCollapse` (c): hide exited children → collapse → expand.
pub fn toggle_selected_collapse(app: &mut App) {
    let _ = app;
    todo!("A2")
}

/// TS `reorderSelected` (J / K): move the session among its siblings.
pub fn reorder_selected(app: &mut App, direction: MoveDirection) {
    let _ = (app, direction);
    todo!("A2")
}

/// TS `resizeSidebar` (< / >).
pub fn resize_sidebar(app: &mut App, delta: i64) {
    let _ = (app, delta);
    todo!("A2")
}

/// TS `stepIntoPane` (→ / l): scroll the preview, browse the changes, edit the notes.
pub fn step_into_pane(app: &mut App) {
    let _ = app;
    todo!("A2")
}

/// TS `runDev`: start (after reviewing an untrusted repository devCommand) or stop the worktree's Dev; `back` is
/// where Esc from the review returns.
pub fn run_dev(app: &mut App, back: Option<AppCallback>) {
    let _ = (app, back);
    todo!("A2")
}

/// TS `toggleDevSelected`: start or stop the selected workspace's Dev.
pub fn toggle_dev_selected(app: &mut App) {
    let _ = app;
    todo!("A2")
}

/// TS `openSelectedInEditor` (E off Notes): the session's folder in Cursor / VS Code.
pub fn open_selected_in_editor(app: &mut App) {
    let _ = app;
    todo!("A2")
}

/// TS `toggleSelectedDone` (Space).
pub fn toggle_selected_done(app: &mut App) {
    let _ = app;
    todo!("A2")
}

/// TS `markSelectedMerged` (M).
pub fn mark_selected_merged(app: &mut App) {
    let _ = app;
    todo!("A2")
}

/// TS `restartSelected` (s / S; `project_fingerprint`: the reviewed-and-skipped config).
pub fn restart_selected(app: &mut App, mode: RestartMode, project_fingerprint: Option<String>) {
    let _ = (app, mode, project_fingerprint);
    todo!("A2")
}

/// TS `refreshSessions` (R).
pub fn refresh_sessions(app: &mut App) {
    let _ = app;
    todo!("A2")
}

/// TS: A: archive or unarchive the selected session.
pub fn toggle_archive(app: &mut App) {
    let _ = app;
    todo!("A2")
}

/// TS: !: the next session that needs attention (blocked ones first, then unread replies).
pub fn next_attention(app: &mut App) {
    let _ = app;
    todo!("A2")
}

/// TS: d (and d d): the Dev tab, a quick double-tap starts or stops Dev.
pub fn dev_tap(app: &mut App, input: &str) {
    let _ = (app, input);
    todo!("A2")
}

/// TS: Enter / o: open what the tab shows full screen (on Notes, Enter edits them here).
pub fn open_selected(app: &mut App, input: &Input) {
    let _ = (app, input);
    todo!("A2")
}

/// TS: Backspace on an exited session: archive it, or on an archived one ask to remove it for good.
pub fn archive_or_remove(app: &mut App) {
    let _ = app;
    todo!("A2")
}
