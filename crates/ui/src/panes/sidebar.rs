//! Port of sidebar.tsx: the session list (header, the filter menu, numbered rows that window around the selection
//! with `↑ N more` lines, and the selected session's details block). Row contents come from
//! deckhand_core::sidebar_model; this lays them out and styles them.

use std::collections::HashSet;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_core::agent_versions::InstalledVersions;
use deckhand_core::session_features::SessionFilterCounts;
use deckhand_core::sidebar_model::{DetailLine, DetailTask, RowPart, SidebarRow};
use deckhand_protocol::data::SessionRecord;
use deckhand_protocol::records::SessionFilter;

use crate::primitives::{Row, TextStyle};

/// TS: sidebar.tsx DETAIL_ROWS: the most rows the details block can use.
pub const DETAIL_ROWS: usize = 9;

/// TS: sidebar.tsx DETAIL_MIN_SIDEBAR_ROWS: a long list keeps rows for the details only in a sidebar this tall.
pub const DETAIL_MIN_SIDEBAR_ROWS: usize = 21;

/// TS: sidebar.tsx SidebarProps `filterMenu`: the open filter menu (f), its highlighted filter and row counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilterMenu {
    pub selected: SessionFilter,
    pub counts: SessionFilterCounts,
}

/// TS: sidebar.tsx SidebarProps `taskOf`: the task a session works on, for the details block.
pub type TaskOf<'a> = &'a dyn Fn(&SessionRecord) -> Option<DetailTask>;

/// TS: sidebar.tsx SidebarProps (width and height are the area's).
#[derive(Clone, Copy)]
pub struct SidebarProps<'a> {
    /// The listed sessions (filtered, collapsed), in order.
    pub sessions: &'a [&'a SessionRecord],
    /// Every session of the checkout (defaults to `sessions` in TS).
    pub all_sessions: &'a [&'a SessionRecord],
    pub selected_id: Option<&'a str>,
    pub spinner_frame: &'a str,
    /// Empty in filtered/search views, where collapse state is not applied.
    pub collapsed_session_ids: Option<&'a HashSet<String>>,
    pub hidden_session_ids: Option<&'a HashSet<String>>,
    pub loaded: bool,
    pub filter: SessionFilter,
    pub query: &'a str,
    /// Clock for the details block's age (ms since the epoch).
    pub now: i64,
    pub installed_versions: Option<&'a InstalledVersions>,
    /// The task a session works on, for the details block.
    pub task_of: Option<TaskOf<'a>>,
    pub filter_menu: Option<FilterMenu>,
}

/// TS: sidebar.tsx visibleSessions: the index range of the rows shown, `available_rows` around the selection.
pub fn visible_range(count: usize, selected_index: usize, available_rows: usize) -> std::ops::Range<usize> {
    let _ = (count, selected_index, available_rows);
    todo!("U2")
}

/// TS: sidebar.tsx emptyMessage: the title and hint of an empty list.
pub fn empty_message(all_sessions: usize, loaded: bool) -> (&'static str, &'static str) {
    let _ = (all_sessions, loaded);
    todo!("U2")
}

/// TS: sidebar.tsx partStyle: a row part's style (the selected row is one inverse highlight; dimmed rows muted).
pub fn part_style(part: &RowPart, row: &SidebarRow) -> TextStyle {
    let _ = (part, row);
    todo!("U2")
}

/// TS: sidebar.tsx moreText: `↑ 12 more` / `↓ 3 more · ! 1` for the sessions out of view on one side.
pub fn more_text(arrow: &str, hidden: &[&SessionRecord]) -> String {
    let _ = (arrow, hidden);
    todo!("U2")
}

/// The sidebar's rows split into its parts (TS: the budgeting in Sidebar's body).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SidebarLayout {
    /// The header row (title, then the filter/search label and `! N`).
    pub header: Row,
    /// The filter menu's lines and its rule, when open.
    pub filter_menu: Vec<Row>,
    /// The `↑ N more` line when the list scrolls.
    pub above: Option<Row>,
    /// The session rows (or the empty message).
    pub rows: Vec<Row>,
    /// The `↓ N more` line when the list scrolls.
    pub below: Option<Row>,
    /// The details block's lines.
    pub details: Vec<DetailLine>,
    /// Whether the details follow the list directly (scrolling) or sit at the bottom.
    pub details_follow_list: bool,
}

/// TS: sidebar.tsx Sidebar's layout at `width` × `height`.
pub fn sidebar_layout(props: &SidebarProps<'_>, width: usize, height: usize) -> SidebarLayout {
    let _ = (props, width, height);
    todo!("U2")
}

/// TS: sidebar.tsx Sidebar.
pub fn render_sidebar(props: &SidebarProps<'_>, buf: &mut Buffer, area: Rect) {
    let _ = (props, buf, area);
    todo!("U2")
}
