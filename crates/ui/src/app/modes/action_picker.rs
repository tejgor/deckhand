//! Port of app.tsx's run list (r, pick-action): the worktree's Dev command first (when the session has a
//! workspace), then the repository's actions; untrusted repository ones are reviewed when chosen. x stops the
//! worktree's running action. ActionPickerPane renders it.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_core::key::Input;
use deckhand_protocol::data::{ActionRecord, ProjectInfo};

use crate::app::state::App;
use crate::keymap::Keymap;

/// pick-action keys.
pub static KEYMAP: Keymap = &[];

/// TS: r in browse: the run list of the selected session's repository (the running action watched first).
pub fn open_run_list(app: &mut App) {
    let _ = app;
    todo!("A5")
}

/// TS: the pick-action branch of the useTerminalInput handler.
pub fn handle_pick_action(app: &mut App, input: &Input) {
    let _ = (app, input);
    todo!("A5")
}

/// TS: ActionPickerPane `dev`: the Dev row and whether it runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DevRow {
    pub running: bool,
}

/// TS: app.tsx ActionPickerPane props (width and height are the area's).
#[derive(Clone, Copy, Debug)]
pub struct ActionPickerPaneProps<'a> {
    pub project: Option<&'a ProjectInfo>,
    /// The worktree's last action, when it belongs to the picker's session.
    pub running: Option<&'a ActionRecord>,
    pub dev: Option<DevRow>,
    pub selected_index: usize,
}

/// TS: app.tsx ActionPickerPane (a MenuPane).
pub fn render_action_picker_pane(props: &ActionPickerPaneProps<'_>, buf: &mut Buffer, area: Rect) {
    let _ = (props, buf, area);
    todo!("A5")
}
