//! Port of preview.tsx: the Preview tab (the agent's screen, or what a session that is not running leaves: setup
//! output, its last preview, a hint).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_protocol::data::{PreviewRecord, SessionRecord, SessionSetup};

use crate::primitives::Row;

/// TS: preview.tsx PreviewPaneProps (width and height are the area's).
#[derive(Clone, Copy, Debug)]
pub struct PreviewPaneProps<'a> {
    pub session: Option<&'a SessionRecord>,
    pub preview: &'a PreviewRecord,
    pub spinner_frame: &'a str,
    pub focused: bool,
}

/// TS: preview.tsx setupMessage: the setup's header line and the tail of its output in `rows` rows, as plain text.
pub fn setup_message(setup: &SessionSetup, rows: usize) -> String {
    let _ = (setup, rows);
    todo!("U1")
}

/// TS: preview.tsx fallbackMessage: what the body shows (the preview, or why there is none).
pub fn fallback_message(session: Option<&SessionRecord>, preview: &PreviewRecord, rows: usize) -> String {
    let _ = (session, preview, rows);
    todo!("U1")
}

/// TS: preview.tsx worktreeBadge (`worktree` / `attached`).
pub fn worktree_badge(session: &SessionRecord) -> Option<&'static str> {
    let _ = session;
    todo!("U1")
}

/// TS: preview.tsx PreviewPane's rows at `width` × `height`: the path/badge header, then the body.
pub fn preview_rows(props: &PreviewPaneProps<'_>, width: usize, height: usize) -> Vec<Row> {
    let _ = (props, width, height);
    todo!("U1")
}

/// TS: preview.tsx PreviewPane.
pub fn render_preview_pane(props: &PreviewPaneProps<'_>, buf: &mut Buffer, area: Rect) {
    let _ = (props, buf, area);
    todo!("U1")
}
