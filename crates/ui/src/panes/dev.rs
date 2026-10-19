//! Port of devPane.tsx: the Dev tab (the workspace's Dev command, shared by every session in the worktree).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_protocol::data::{DevRecord, SessionRecord};

use crate::primitives::Row;

/// TS: devPane.tsx fallbackMessage: the Dev output, or why there is none.
pub fn fallback_message(session: Option<&SessionRecord>, dev: &DevRecord) -> String {
    let _ = (session, dev);
    todo!("U1")
}

/// TS: devPane.tsx DevPaneProps (width and height are the area's).
#[derive(Clone, Copy, Debug)]
pub struct DevPaneProps<'a> {
    pub session: Option<&'a SessionRecord>,
    pub dev: &'a DevRecord,
}

/// TS: devPane.tsx DevPane's rows: the path · command / status header, then the body.
pub fn dev_rows(props: &DevPaneProps<'_>, width: usize, height: usize) -> Vec<Row> {
    let _ = (props, width, height);
    todo!("U1")
}

/// TS: devPane.tsx DevPane.
pub fn render_dev_pane(props: &DevPaneProps<'_>, buf: &mut Buffer, area: Rect) {
    let _ = (props, buf, area);
    todo!("U1")
}
