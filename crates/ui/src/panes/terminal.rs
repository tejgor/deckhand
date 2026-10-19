//! Port of terminalPane.tsx: the Terminal tab (the workspace's shell, or the last action run there; v switches).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use deckhand_protocol::data::{ActionRecord, SessionRecord, TerminalRecord};

use crate::primitives::Row;

/// TS: terminalPane.tsx TerminalView: what the Terminal tab shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TerminalView {
    #[default]
    Shell,
    Action,
}

/// TS: terminalPane.tsx hasAction: the selected session's workspace has an action to show (running, or finished with
/// its output kept).
pub fn has_action(session: Option<&SessionRecord>, action: &ActionRecord) -> bool {
    let _ = (session, action);
    todo!("U1")
}

/// TS: terminalPane.tsx actionStatus's result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionStatus {
    pub text: String,
    pub color: Color,
}

/// TS: terminalPane.tsx actionStatus: running, or how it ended (`● running`, `✓ exit 0`, `✗ signal 9`, ...).
pub fn action_status(action: &ActionRecord) -> ActionStatus {
    let _ = action;
    todo!("U1")
}

/// TS: terminalPane.tsx fallbackMessage: the shell's output, or why there is none.
pub fn fallback_message(session: Option<&SessionRecord>, terminal: &TerminalRecord) -> String {
    let _ = (session, terminal);
    todo!("U1")
}

/// TS: terminalPane.tsx TerminalPaneProps (width and height are the area's).
#[derive(Clone, Copy, Debug)]
pub struct TerminalPaneProps<'a> {
    pub session: Option<&'a SessionRecord>,
    pub terminal: &'a TerminalRecord,
    pub action: &'a ActionRecord,
    pub view: TerminalView,
}

/// TS: terminalPane.tsx TerminalPane's rows: the path / view header, then the body.
pub fn terminal_rows(props: &TerminalPaneProps<'_>, width: usize, height: usize) -> Vec<Row> {
    let _ = (props, width, height);
    todo!("U1")
}

/// TS: terminalPane.tsx TerminalPane.
pub fn render_terminal_pane(props: &TerminalPaneProps<'_>, buf: &mut Buffer, area: Rect) {
    let _ = (props, buf, area);
    todo!("U1")
}
