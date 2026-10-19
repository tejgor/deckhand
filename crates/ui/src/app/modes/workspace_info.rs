//! Port of app.tsx's workspace overview (i): the worktree's summary, P fetches its PR's state, b opens the PR, c
//! asks to push and open GitHub's PR form (only Enter on that confirmation pushes), g the Git tab.

use deckhand_core::key::Input;

use crate::app::state::App;
use crate::keymap::Keymap;

/// workspace-info keys.
pub static KEYMAP: Keymap = &[];

/// TS: i in browse: the overview of the selected session's workspace, its summary requested.
pub fn open_workspace_info(app: &mut App) {
    let _ = app;
    todo!("A5")
}

/// TS: the workspace-info branch of the useTerminalInput handler.
pub fn handle_workspace_info(app: &mut App, input: &Input) {
    let _ = (app, input);
    todo!("A5")
}
