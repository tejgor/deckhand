//! Port of sidebarModel.ts.
//!
//! Pure layout of the session sidebar (rendered by the sidebar pane): rows, header and the selected session's
//! details. Widths are TS `.length` (UTF-16 units). Times are milliseconds since the epoch.

use std::collections::HashSet;

use serde::Serialize;
use serde_with::skip_serializing_none;

use crate::agent_versions::InstalledVersions;
use crate::display::Color;
use crate::session_features::{SessionFilter, SessionFilterCounts};
use crate::session_order::SessionLike;
use deckhand_protocol::data::SessionRecord;

/// TS: sidebarModel.ts RowRole (what a row segment is; the sidebar pane maps roles to colors).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum RowRole {
    #[serde(rename = "cursor")]
    Cursor,
    #[serde(rename = "gutter")]
    Gutter,
    #[serde(rename = "number")]
    Number,
    #[serde(rename = "tree")]
    Tree,
    #[serde(rename = "status")]
    Status,
    #[serde(rename = "title")]
    Title,
    #[serde(rename = "gap")]
    Gap,
    #[serde(rename = "dev")]
    Dev,
    #[serde(rename = "archived")]
    Archived,
    #[serde(rename = "cleanup")]
    Cleanup,
    #[serde(rename = "merged")]
    Merged,
    #[serde(rename = "done")]
    Done,
    #[serde(rename = "count")]
    Count,
    #[serde(rename = "outdated")]
    Outdated,
    #[serde(rename = "agent")]
    Agent,
}

/// TS: sidebarModel.ts RowPart.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RowPart {
    pub text: String,
    pub role: RowRole,
}

/// TS: sidebarModel.ts SidebarRow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SidebarRow {
    pub id: String,
    pub parts: Vec<RowPart>,
    pub selected: bool,
    /// Archived rows outside the archived view.
    pub dimmed: bool,
    /// Marked done (D) and not dimmed: its title is muted, its markers stay readable.
    pub done: bool,
    /// statusColor of the session (status glyph, tree glyphs and title).
    pub color: Color,
}

/// TS: sidebarModel.ts GUTTER_MARKER.
pub const GUTTER_MARKER: &str = "╎";

/// TS: sidebarModel.ts DONE_MARKER.
pub const DONE_MARKER: &str = "☑";

/// TS: sidebarModel.ts OUTDATED_MARKER.
pub const OUTDATED_MARKER: &str = "↑";

/// TS: sidebarModel.ts MIN_TITLE (title columns below which suffix markers are dropped).
pub const MIN_TITLE: usize = 4;

/// TS: sidebarModel.ts DROP_ORDER (suffix markers dropped first to last; the agent stays).
pub const DROP_ORDER: [RowRole; 7] = [
    RowRole::Outdated,
    RowRole::Count,
    RowRole::Merged,
    RowRole::Done,
    RowRole::Archived,
    RowRole::Cleanup,
    RowRole::Dev,
];

/// TS: sidebarModel.ts SidebarRowsInput.
#[derive(Clone, Copy)]
pub struct SidebarRowsInput<'a> {
    /// The rows on screen, in order.
    pub rows: &'a [&'a SessionRecord],
    pub all_sessions: &'a [&'a SessionRecord],
    /// Row number of `rows[0]` (positions in the whole list, as the numeric jump keys use).
    pub first_number: usize,
    /// Digits of the largest number in the list.
    pub number_width: usize,
    pub selected_id: Option<&'a str>,
    /// Row width, the cursor column included.
    pub width: usize,
    pub spinner_frame: &'a str,
    pub filter: SessionFilter,
    pub collapsed_session_ids: Option<&'a HashSet<String>>,
    pub hidden_session_ids: Option<&'a HashSet<String>>,
    /// Installed agent versions: a running session launched with an older one gets ↑.
    pub installed_versions: Option<&'a InstalledVersions>,
}

/// TS: sidebarModel.ts rowDimmed (archived rows, except in the archived view).
pub fn row_dimmed(session: &SessionRecord, filter: SessionFilter) -> bool {
    let _ = (session, filter);
    todo!("P8")
}

/// TS: sidebarModel.ts sidebarRows.
pub fn sidebar_rows(input: &SidebarRowsInput<'_>) -> Vec<SidebarRow> {
    let _ = input;
    todo!("P8")
}

/// TS: sidebarModel.ts SidebarHeader.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SidebarHeader {
    pub title: String,
    /// Filter and/or search with the count, or the plain count in the default view.
    pub label: String,
    /// `! N` when non-archived sessions need attention, else empty.
    pub attention: String,
    /// A non-default filter or a search is active.
    pub highlighted: bool,
}

/// TS: sidebarModel.ts sidebarHeader argument.
#[derive(Clone, Copy)]
pub struct SidebarHeaderInput<'a> {
    pub width: usize,
    pub filter: SessionFilter,
    pub query: &'a str,
    pub shown: usize,
    pub total: usize,
    pub all_sessions: &'a [&'a SessionRecord],
}

/// TS: sidebarModel.ts sidebarHeader.
pub fn sidebar_header(input: &SidebarHeaderInput<'_>) -> SidebarHeader {
    let _ = input;
    todo!("P8")
}

/// TS: sidebarModel.ts FilterMenuLine (`› a  active      12`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FilterMenuLine {
    pub filter: SessionFilter,
    pub selected: bool,
    pub cursor: String,
    pub key: String,
    pub label: String,
    pub count: String,
}

/// TS: sidebarModel.ts filterMenuLines.
pub fn filter_menu_lines(selected: SessionFilter, counts: &SessionFilterCounts, width: usize) -> Vec<FilterMenuLine> {
    let _ = (selected, counts, width);
    todo!("P8")
}

/// TS: sidebarModel.ts DetailPart.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DetailPart {
    pub text: String,
    pub color: Option<Color>,
    pub dim: Option<bool>,
}

/// TS: sidebarModel.ts DetailLine.
pub type DetailLine = Vec<DetailPart>;

/// TS: sidebarModel.ts statusWords.
pub fn status_words(session: &SessionRecord) -> &'static str {
    let _ = session;
    todo!("P8")
}

/// TS: sidebarModel.ts statusSince.
pub fn status_since(session: &SessionRecord) -> Option<&str> {
    let _ = session;
    todo!("P8")
}

/// TS: sidebarModel.ts formatAge (`now`, `12m`, `3h`, `2d`).
pub fn format_age(ms: i64) -> String {
    let _ = ms;
    todo!("P8")
}

/// TS: sidebarModel.ts msUntilAgeChanges.
pub fn ms_until_age_changes(ms: i64) -> i64 {
    let _ = ms;
    todo!("P8")
}

/// TS: sidebarModel.ts locationText (`width`: TS `Infinity` by default; `now`: for the done marker's age).
pub fn location_text<S: SessionLike>(
    session: &SessionRecord,
    all_sessions: &[S],
    width: Option<usize>,
    now_ms: Option<i64>,
) -> String {
    let _ = (session, all_sessions, width, now_ms);
    todo!("P8")
}

/// TS: sidebarModel.ts sessionDetails `task` argument (the task it works on, and how many more are open).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetailTask {
    pub title: String,
    pub done: bool,
    pub more: Option<usize>,
}

/// TS: sidebarModel.ts sessionDetails.
pub fn session_details<S: SessionLike>(
    session: Option<&SessionRecord>,
    all_sessions: &[S],
    width: usize,
    free_rows: usize,
    now_ms: i64,
    installed_versions: Option<&InstalledVersions>,
    task: Option<&DetailTask>,
) -> Vec<DetailLine> {
    let _ = (
        session,
        all_sessions,
        width,
        free_rows,
        now_ms,
        installed_versions,
        task,
    );
    todo!("P8")
}

/// TS: sidebarModel.ts attentionReasonLines.
pub fn attention_reason_lines(session: &SessionRecord, width: usize, rows: usize) -> Vec<DetailLine> {
    let _ = (session, width, rows);
    todo!("P8")
}
