//! Port of sidebarModel.ts.
//!
//! Pure layout of the session sidebar (rendered by the sidebar pane): rows, header and the selected session's
//! details. Widths are TS `.length` (UTF-16 units). Times are milliseconds since the epoch.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use serde_with::skip_serializing_none;

use crate::agent_versions::{InstalledVersions, session_outdated};
use crate::display::{
    Color, THEME, display_session_title, program_glyph, status_color, status_glyph, truncate, wrap_words,
};
use crate::jsstr::{date, js_len, js_pad_end, js_pad_start, js_slice, js_trim, path};
use crate::notes::open_checklist_text;
use crate::session_features::{
    SESSION_FILTERS, SessionFilter, SessionFilterCounts, filter_session_list, session_filter_key,
    session_needs_attention,
};
use crate::session_order::{
    SessionLike, count_hidden_session_descendants, count_session_descendants, session_depth, session_has_children,
};
use crate::workspace::workspace_key;
use deckhand_protocol::data::{
    AgentActivityStatus, AttentionReasonKind, AttentionState, ExitReason, SessionRecord, SessionStatus,
    SessionWorktreeMode, SubSessionKind,
};

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
    /// Not in the TS: `▶` on the other rows that share the workspace whose Dev runs (muted).
    #[serde(rename = "dev-shared")]
    DevShared,
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
pub const DROP_ORDER: [RowRole; 8] = [
    RowRole::DevShared,
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
    /// Not in the TS: `▶` on every row whose workspace's Dev runs, muted after the first (the TS marks the first
    /// only, which is easy to miss with many sessions in one checkout).
    pub shared_dev: bool,
}

/// UTF-16 length as a JS number (widths here go negative).
fn jlen(text: &str) -> i64 {
    i64::try_from(js_len(text)).unwrap_or(i64::MAX)
}

fn jnum(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// `truncate(text, width)` with a JS width (zero or less: nothing).
fn cut(text: &str, width: i64) -> String {
    usize::try_from(width).map_or_else(|_| String::new(), |width| truncate(text, width))
}

/// A JS-truthy optional string (set and not empty).
fn truthy(value: Option<&String>) -> bool {
    value.is_some_and(|text| !text.is_empty())
}

fn is_merged(session: &SessionRecord) -> bool {
    truthy(
        session
            .worktree
            .as_ref()
            .and_then(|worktree| worktree.merged_at.as_ref()),
    ) || truthy(session.merged_at.as_ref())
}

/// TS: sidebarModel.ts rowDimmed (archived rows, except in the archived view, which lists nothing else).
pub fn row_dimmed(session: &SessionRecord, filter: SessionFilter) -> bool {
    filter != SessionFilter::Archived && truthy(session.archived_at.as_ref())
}

/// The installed version of the session's agent (`installedVersions[session.program]`).
fn installed_for<'a>(installed_versions: Option<&'a InstalledVersions>, session: &SessionRecord) -> Option<&'a str> {
    installed_versions
        .and_then(|versions| versions.get(&session.program))
        .map(String::as_str)
}

fn part(text: impl Into<String>, role: RowRole) -> RowPart {
    RowPart {
        text: text.into(),
        role,
    }
}

fn parts_width(parts: &[RowPart]) -> i64 {
    parts.iter().map(|part| jlen(&part.text)).sum()
}

/// TS: sidebarModel.ts sidebarRows.
pub fn sidebar_rows(input: &SidebarRowsInput<'_>) -> Vec<SidebarRow> {
    let empty = HashSet::new();
    let collapsed_ids = input.collapsed_session_ids.unwrap_or(&empty);
    let hidden_ids = input.hidden_session_ids.unwrap_or(&empty);
    let width = jnum(input.width);
    // `new Map(rows.map(...))`: the last row with an id wins.
    let mut keys: HashMap<&str, Option<String>> = HashMap::new();
    for session in input.rows {
        keys.insert(session.id.as_str(), workspace_key(session));
    }
    let key_of = |id: &str| keys.get(id).cloned().flatten();
    let selected_key = input
        .selected_id
        .filter(|selected| input.rows.iter().any(|session| session.id == *selected))
        .and_then(key_of);
    // Dev belongs to the workspace: ▶ on its first row on screen (and, with `shared_dev`, muted on the others).
    let mut dev_shown: HashSet<String> = HashSet::new();
    // ▾ ▸ and +N count the sub-sessions this view lists (an archived one is gone from the active view); depth and
    // titles still follow every ancestor.
    let listed = filter_session_list(input.all_sessions, input.filter, "", None);
    let mut out = Vec::with_capacity(input.rows.len());
    for (index, &session) in input.rows.iter().enumerate() {
        let selected = input.selected_id == Some(session.id.as_str());
        let key = key_of(&session.id);
        let shares_selected = !selected && selected_key.is_some() && key == selected_key;
        // Indented under the parents the view lists (an archived sub-session of a live session is top level there).
        let depth = session_depth(session, &listed);
        let has_children = session_has_children(&session.id, &listed);
        let collapsed = collapsed_ids.contains(&session.id);
        let child_count = if collapsed && has_children {
            count_session_descendants(&session.id, &listed)
        } else {
            count_hidden_session_descendants(&session.id, &listed, hidden_ids)
        };
        let mut dev = None;
        if session.dev_running == Some(true) {
            let first = key.as_ref().is_none_or(|key| !dev_shown.contains(key));
            if let Some(key) = &key {
                dev_shown.insert(key.clone());
            }
            dev = if first {
                Some(RowRole::Dev)
            } else {
                input.shared_dev.then_some(RowRole::DevShared)
            };
        }
        let tree = format!(
            "{}{}{}",
            "  ".repeat(depth.min(4)),
            if has_children {
                if collapsed { "▸ " } else { "▾ " }
            } else {
                ""
            },
            match session.sub_session_kind {
                Some(SubSessionKind::Forked) => "⑂ ",
                Some(SubSessionKind::Clean) => "↳ ",
                None => "",
            }
        );
        let mut prefix = vec![
            if selected {
                part("›", RowRole::Cursor)
            } else if shares_selected {
                part(GUTTER_MARKER, RowRole::Gutter)
            } else {
                part(" ", RowRole::Cursor)
            },
            part(
                format!(
                    " {} ",
                    js_pad_start(&(input.first_number + index).to_string(), input.number_width, " ")
                ),
                RowRole::Number,
            ),
        ];
        if !tree.is_empty() {
            prefix.push(part(tree, RowRole::Tree));
        }
        prefix.push(part(
            format!("{} ", status_glyph(session, input.spinner_frame)),
            RowRole::Status,
        ));
        let mut suffix = Vec::new();
        if let Some(role) = dev {
            suffix.push(part("▶", role));
        }
        if truthy(session.archived_at.as_ref()) {
            suffix.push(part("▣", RowRole::Archived));
        }
        if truthy(session.cleanup_error.as_ref()) {
            suffix.push(part("!", RowRole::Cleanup));
        }
        if is_merged(session) {
            suffix.push(part("✓", RowRole::Merged));
        }
        if truthy(session.done_at.as_ref()) {
            suffix.push(part(DONE_MARKER, RowRole::Done));
        }
        if child_count > 0 {
            suffix.push(part(format!("+{child_count}"), RowRole::Count));
        }
        if session_outdated(session, installed_for(input.installed_versions, session)) {
            suffix.push(part(OUTDATED_MARKER, RowRole::Outdated));
        }
        suffix.push(part(program_glyph(session.program), RowRole::Agent));
        let suffix_width = |suffix: &[RowPart]| parts_width(suffix) + jnum(suffix.len()) - 1;
        let prefix_width = parts_width(&prefix);
        let full_title = display_session_title(session, input.all_sessions);
        let min_title = jnum(MIN_TITLE).min(jlen(&full_title));
        for role in DROP_ORDER {
            if width - prefix_width - 1 - suffix_width(&suffix) >= min_title {
                break;
            }
            if let Some(at) = suffix.iter().position(|part| part.role == role) {
                suffix.remove(at);
            }
        }
        let title = cut(&full_title, (width - prefix_width - 1 - suffix_width(&suffix)).max(0));
        let gap = (width - prefix_width - jlen(&title) - suffix_width(&suffix)).max(1);
        let mut parts = prefix;
        parts.push(part(title, RowRole::Title));
        parts.push(part(" ".repeat(usize::try_from(gap).unwrap_or(1)), RowRole::Gap));
        for (at, item) in suffix.into_iter().enumerate() {
            if at > 0 {
                parts.push(part(" ", RowRole::Gap));
            }
            parts.push(item);
        }
        let dimmed = !selected && row_dimmed(session, input.filter);
        out.push(SidebarRow {
            id: session.id.clone(),
            parts: fit_row(parts, width),
            selected,
            dimmed,
            done: !selected && !dimmed && truthy(session.done_at.as_ref()),
            color: status_color(session),
        });
    }
    out
}

/// A part with text (row parts and detail parts are cut the same way).
trait TextPart: Clone {
    fn text(&self) -> &str;
    fn with_text(&self, text: String) -> Self;
}

impl TextPart for RowPart {
    fn text(&self) -> &str {
        &self.text
    }
    fn with_text(&self, text: String) -> Self {
        RowPart { text, role: self.role }
    }
}

impl TextPart for DetailPart {
    fn text(&self) -> &str {
        &self.text
    }
    fn with_text(&self, text: String) -> Self {
        DetailPart { text, ..self.clone() }
    }
}

/// Parts cut to `width` columns in total, in order.
fn cut_parts<T: TextPart>(parts: &[T], width: i64) -> Vec<T> {
    let mut shown = Vec::new();
    let mut room = width.max(0);
    for item in parts {
        if room <= 0 {
            break;
        }
        let text = if jlen(item.text()) > room {
            cut(item.text(), room)
        } else {
            item.text().to_owned()
        };
        room -= jlen(&text);
        if !text.is_empty() {
            shown.push(item.with_text(text));
        }
    }
    shown
}

/// A row exactly `width` columns wide (cutting only bites when the prefix alone is too wide).
fn fit_row(parts: Vec<RowPart>, width: i64) -> Vec<RowPart> {
    let mut shown = cut_parts(&parts, width);
    let room = width - parts_width(&shown);
    if room > 0 {
        shown.push(part(" ".repeat(usize::try_from(room).unwrap_or(0)), RowRole::Gap));
    }
    shown
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

/// TS: sidebarModel.ts sidebarHeader: the attention count wins over the label (count over filter/search), which wins
/// over the title.
pub fn sidebar_header(input: &SidebarHeaderInput<'_>) -> SidebarHeader {
    let needing = input
        .all_sessions
        .iter()
        .filter(|session| !truthy(session.archived_at.as_ref()) && session_needs_attention(session))
        .count();
    let attention = if needing > 0 {
        format!("! {needing}")
    } else {
        String::new()
    };
    let search = js_trim(input.query);
    let highlighted = input.filter != SessionFilter::Active || !search.is_empty();
    let tag = [
        if input.filter != SessionFilter::Active {
            input.filter.as_str().to_owned()
        } else {
            String::new()
        },
        if search.is_empty() {
            String::new()
        } else {
            format!("/{search}")
        },
    ]
    .into_iter()
    .filter(|item| !item.is_empty())
    .collect::<Vec<_>>()
    .join(" ");
    let count = if highlighted {
        format!("{}/{}", input.shown, input.total)
    } else {
        input.shown.to_string()
    };
    let room = jnum(input.width) - if attention.is_empty() { 0 } else { jlen(&attention) + 3 };
    let mut label = if tag.is_empty() {
        count.clone()
    } else {
        format!("{tag} {count}")
    };
    if jlen(&label) > room {
        // Cut the filter/search, keep the count; below three columns of tag, drop it.
        let tag_room = room - jlen(&count) - 1;
        label = if !tag.is_empty() && tag_room >= 3 {
            format!("{} {count}", cut(&tag, tag_room))
        } else {
            count
        };
    }
    // The title goes first when the label needs its columns.
    let title = if room - jlen(&label) > jlen("Sessions") {
        "Sessions"
    } else {
        ""
    };
    SidebarHeader {
        title: title.to_owned(),
        label,
        attention,
        highlighted,
    }
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

/// TS: sidebarModel.ts filterMenuLines (at `width` columns, the session rows' width: the label is cut before the
/// count).
pub fn filter_menu_lines(selected: SessionFilter, counts: &SessionFilterCounts, width: usize) -> Vec<FilterMenuLine> {
    SESSION_FILTERS
        .into_iter()
        .map(|filter| {
            let count = counts.get(filter).to_string();
            // `› a  ` before the label, one column before the count.
            let room = jnum(width) - 5 - jlen(&count) - 1;
            let name = filter.as_str();
            let label = if room >= jlen(name) {
                js_pad_end(name, usize::try_from(room).unwrap_or(0), " ")
            } else {
                cut(name, room.max(0))
            };
            FilterMenuLine {
                filter,
                selected: filter == selected,
                cursor: if filter == selected { "›" } else { " " }.to_owned(),
                key: session_filter_key(filter).to_owned(),
                label,
                count: if room >= 0 { format!(" {count}") } else { String::new() },
            }
        })
        .collect()
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

fn detail(text: impl Into<String>, color: Color) -> DetailPart {
    DetailPart {
        text: text.into(),
        color: Some(color),
        dim: None,
    }
}

/// TS: sidebarModel.ts statusWords (the session's state in words, from the same inputs as statusGlyph).
pub fn status_words(session: &SessionRecord) -> &'static str {
    if session.status == SessionStatus::Running {
        match session.attention.as_ref().map(|attention| attention.state) {
            Some(AttentionState::NeedsInput) => return "needs input",
            Some(AttentionState::ResponseEnded) if session.reply_read != Some(true) => return "response ended",
            Some(AttentionState::Failed) => return "failed",
            Some(AttentionState::Limited) => return "rate-limited",
            Some(AttentionState::Working) => return "working",
            _ => {}
        }
    }
    match session.status {
        SessionStatus::Exited => match session.exit_reason {
            Some(ExitReason::Failed) => "exited (failed)",
            Some(ExitReason::Interrupted) => "interrupted",
            _ => "exited",
        },
        SessionStatus::Starting => "starting",
        SessionStatus::Running => match session.agent_status {
            Some(AgentActivityStatus::Active) => "working",
            Some(AgentActivityStatus::Idle) => "idle",
            _ => "running",
        },
    }
}

/// TS: sidebarModel.ts statusSince (when the state statusWords describes began: the attention signal while running,
/// else the last activity change, set on exit too).
pub fn status_since(session: &SessionRecord) -> Option<&str> {
    if session.status == SessionStatus::Running
        && let Some(attention) = &session.attention
        && attention.state != AttentionState::Unknown
    {
        return Some(&attention.at);
    }
    Some(
        session
            .agent_status_updated_at
            .as_deref()
            .unwrap_or(&session.updated_at),
    )
}

const MINUTE: f64 = 60_000.0;
const HOUR: f64 = 60.0 * MINUTE;
const DAY: f64 = 24.0 * HOUR;

/// `String(n)` for the whole numbers `Math.floor` gives (`Infinity` included).
fn js_whole_number(value: f64) -> String {
    if value.is_nan() {
        "NaN".to_owned()
    } else if value == f64::INFINITY {
        "Infinity".to_owned()
    } else if value == f64::NEG_INFINITY {
        "-Infinity".to_owned()
    } else if value.abs() < 1e21 {
        format!("{value:.0}")
    } else {
        // `1e+21` and up: JS writes the exponent form.
        let text = format!("{value:e}");
        text.replacen('e', "e+", 1)
    }
}

/// TS: sidebarModel.ts formatAge (`now`, `12m`, `3h`, `2d`). A JS number: NaN (an unreadable time) is `now`.
pub fn format_age(ms: f64) -> String {
    if ms.is_nan() || ms < MINUTE {
        return "now".to_owned();
    }
    if ms < HOUR {
        return format!("{}m", js_whole_number((ms / MINUTE).floor()));
    }
    if ms < DAY {
        return format!("{}h", js_whole_number((ms / HOUR).floor()));
    }
    format!("{}d", js_whole_number((ms / DAY).floor()))
}

/// TS: sidebarModel.ts msUntilAgeChanges (milliseconds until [`format_age`] changes, so a clock can re-render exactly
/// then).
pub fn ms_until_age_changes(ms: f64) -> f64 {
    if ms.is_nan() || ms < 0.0 {
        return MINUTE;
    }
    let unit = if ms < HOUR {
        MINUTE
    } else if ms < DAY {
        HOUR
    } else {
        DAY
    };
    unit - (ms % unit) + 50.0
}

fn session_branch<'a>(session: &'a SessionRecord, sharing: &[&'a SessionRecord]) -> Option<&'a str> {
    let own = session
        .worktree
        .as_ref()
        .and_then(|worktree| worktree.branch.as_deref())
        .filter(|branch| !branch.is_empty());
    own.or_else(|| {
        sharing
            .iter()
            .find_map(|other| {
                other.worktree.as_ref().filter(|worktree| {
                    worktree.branch.as_deref().is_some_and(|branch| !branch.is_empty())
                        && worktree.is_main != Some(true)
                })
            })
            .and_then(|worktree| worktree.branch.as_deref())
    })
}

/// `☑2d` (`☑` alone within a minute, without a clock or a readable time): the done marker with its age.
fn done_glyph(session: &SessionRecord, now_ms: Option<i64>) -> String {
    let at = session
        .done_at
        .as_deref()
        .filter(|at| !at.is_empty())
        .and_then(date::parse);
    let age = match (now_ms, at) {
        (Some(now), Some(at)) => format_age(now as f64 - at as f64),
        _ => "now".to_owned(),
    };
    if age == "now" {
        DONE_MARKER.to_owned()
    } else {
        format!("{DONE_MARKER}{age}")
    }
}

/// The branch (or place) keeps at least this many columns before markers are dropped.
const MIN_WHERE: i64 = 8;

/// TS: sidebarModel.ts locationText (`width`: TS `Infinity` by default; `now`: for the done marker's age).
///
/// Where the session runs, then its workspace markers as the sidebar's glyphs: `╎N` N other sessions share the
/// worktree, `▶` Dev runs, `✓` merged, `☑2d` done (and when), `▣` archived. A long branch is cut to keep the markers;
/// only below MIN_WHERE columns are markers dropped from the end, ` …` marking that.
pub fn location_text<S: SessionLike>(
    session: &SessionRecord,
    all_sessions: &[S],
    width: Option<usize>,
    now_ms: Option<i64>,
) -> String {
    let width = width.map_or(i64::MAX, jnum);
    let key = workspace_key(session);
    let sharing: Vec<&SessionRecord> = match &key {
        None => Vec::new(),
        Some(key) => all_sessions
            .iter()
            .map(SessionLike::session)
            .filter(|other| other.id != session.id && workspace_key(other).as_ref() == Some(key))
            .collect(),
    };
    let sharers = sharing
        .iter()
        .filter(|other| !truthy(other.archived_at.as_ref()))
        .count();
    let worktree = session.worktree.as_ref();
    let place = if worktree.is_some_and(|worktree| truthy(worktree.deleted_at.as_ref())) {
        "worktree deleted".to_owned()
    } else if let Some(key) = &key {
        let main_checkout = worktree.is_some_and(|worktree| worktree.is_main == Some(true))
            || (!truthy(worktree.and_then(|worktree| worktree.id.as_ref()))
                && worktree.is_none_or(|worktree| worktree.mode == SessionWorktreeMode::None));
        if main_checkout {
            "main checkout".to_owned()
        } else {
            match session_branch(session, &sharing) {
                Some(branch) => format!("⎇ {branch}"),
                None => format!("worktree {}", path::basename(key)),
            }
        }
    } else {
        "preparing worktree".to_owned()
    };
    let markers: Vec<String> = [
        if sharers > 0 {
            format!("{GUTTER_MARKER}{sharers}")
        } else {
            String::new()
        },
        if session.dev_running == Some(true) {
            "▶".to_owned()
        } else {
            String::new()
        },
        if is_merged(session) {
            "✓".to_owned()
        } else {
            String::new()
        },
        if truthy(session.done_at.as_ref()) {
            done_glyph(session, now_ms)
        } else {
            String::new()
        },
        if truthy(session.archived_at.as_ref()) {
            "▣".to_owned()
        } else {
            String::new()
        },
    ]
    .into_iter()
    .filter(|marker| !marker.is_empty())
    .collect();
    for shown in (0..=markers.len()).rev() {
        let cut_markers = shown < markers.len();
        if shown == 0 {
            return if cut_markers && jlen(&place).saturating_add(2) <= width {
                format!("{place} …")
            } else {
                cut(&place, width)
            };
        }
        let mut tail: Vec<&str> = markers[..shown].iter().map(String::as_str).collect();
        if cut_markers {
            tail.push("…");
        }
        let tail = tail.join(" ");
        let room = width.saturating_sub(jlen(&tail)).saturating_sub(1);
        if room >= MIN_WHERE.min(jlen(&place)) {
            return format!("{} {tail}", cut(&place, room));
        }
    }
    cut(&place, width)
}

/// Title lines: wrapped, at most `max`, the last one ending in … when cut.
fn title_lines(title: &str, width: usize, max: usize) -> Vec<String> {
    let mut lines = wrap_words(title, width);
    if lines.len() <= max {
        return lines;
    }
    // The rest is wider than `width` (it wrapped), so truncate ends it in ….
    let keep = max.saturating_sub(1);
    let rest = lines.split_off(keep).join(" ");
    lines.push(truncate(&rest, width));
    lines
}

/// TS: sidebarModel.ts sessionDetails `task` argument (the task it works on, and how many more are open).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetailTask {
    pub title: String,
    pub done: bool,
    pub more: Option<usize>,
}

/// TS: sidebarModel.ts sessionDetails.
///
/// The selected session's details in the `free_rows` rows the list leaves: a separator, the full title (two lines
/// with five free rows, else one), `agent · state · age`, why it waits (two lines when the location still fits after
/// them), where it runs, the task it works on, and open checklist items of its notes. Fewer rows drop the checklist
/// line first, then the location, and fewer than three hide it.
pub fn session_details<S: SessionLike>(
    session: Option<&SessionRecord>,
    all_sessions: &[S],
    width: usize,
    free_rows: usize,
    now_ms: i64,
    installed_versions: Option<&InstalledVersions>,
    task: Option<&DetailTask>,
) -> Vec<DetailLine> {
    let Some(session) = session else {
        return Vec::new();
    };
    if free_rows < 3 || width < 1 {
        return Vec::new();
    }
    let mut title = display_session_title(session, all_sessions);
    if title.is_empty() {
        title = "(untitled)".to_owned();
    }
    let titles = title_lines(&title, width, if free_rows >= 5 { 2 } else { 1 });
    let since_ms = status_since(session)
        .filter(|since| !since.is_empty())
        .and_then(date::parse);
    let age = since_ms.map_or_else(String::new, |since| format_age(now_ms as f64 - since as f64));
    let state = status_words(session);
    let mut lines: Vec<DetailLine> = vec![vec![detail("─".repeat(width), THEME.border)]];
    lines.extend(titles.into_iter().map(|text| vec![detail(text, THEME.accent_soft)]));
    lines.push(state_line(
        session,
        state,
        &age,
        width,
        installed_for(installed_versions, session),
    ));
    let reason = attention_reason_lines(session, width, free_rows.saturating_sub(lines.len()));
    lines.extend(reason);
    let room = |lines: &Vec<DetailLine>| free_rows.saturating_sub(lines.len());
    if room(&lines) >= 1 {
        lines.push(vec![detail(
            location_text(session, all_sessions, Some(width), Some(now_ms)),
            THEME.muted,
        )]);
    }
    // The task it works on (b), and how many more are open on its work: kept longer than the checklist line below.
    if let Some(task) = task
        && room(&lines) >= 1
    {
        let tail = format!(
            "{}{}",
            if task.done { " ✓" } else { "" },
            match task.more {
                Some(more) if more > 0 => format!(" +{more}"),
                _ => String::new(),
            }
        );
        let head = cut(&format!("◆ {}", task.title), (jnum(width) - jlen(&tail)).max(1));
        lines.push(vec![detail(
            js_slice(&format!("{head}{tail}"), 0, Some(jnum(width))),
            THEME.accent_soft,
        )]);
    }
    // Open checklist items of its notes and its worktree's: the first line to go when rows are short.
    if let Some(checklist) = open_checklist_text(session, width).filter(|text| !text.is_empty())
        && room(&lines) >= 1
    {
        lines.push(vec![detail(checklist, THEME.muted)]);
    }
    lines
}

/// TS: sidebarModel.ts attentionReasonLines: what a running session waits on, from its agent signal's reason (`asks:
/// …`, `plan ready: …`, `wants to run: …`, `said: …`, a failure): up to two lines when `rows` leaves one for the
/// location after them, else one.
pub fn attention_reason_lines(session: &SessionRecord, width: usize, rows: usize) -> Vec<DetailLine> {
    let reason = if session.status == SessionStatus::Running {
        session
            .attention
            .as_ref()
            .and_then(|attention| attention.reason.as_ref())
    } else {
        None
    };
    let Some(reason) = reason else {
        return Vec::new();
    };
    if rows < 1 || width < 1 {
        return Vec::new();
    }
    let color = match reason.kind {
        AttentionReasonKind::Message => THEME.muted,
        AttentionReasonKind::Failure => status_color(session),
        _ => THEME.warn,
    };
    title_lines(&reason.text, width, if rows >= 3 { 2 } else { 1 })
        .into_iter()
        .map(|text| vec![detail(text, color)])
        .collect()
}

/// `<glyph> <agent> · <state> · <age>`; when too wide, the agent's name goes first (its glyph stays), then the age.
/// An outdated session names both versions first (`claude 2.1.287 · 2.1.290 installed`, then `2.1.287 → 2.1.290`).
fn state_line(session: &SessionRecord, state: &str, age: &str, width: usize, installed: Option<&str>) -> DetailLine {
    let glyph = program_glyph(session.program);
    let program = session.program.as_str();
    let sep = DetailPart {
        text: " · ".into(),
        color: Some(THEME.muted),
        dim: Some(true),
    };
    let state_text = detail(state, status_color(session));
    let age_parts: Vec<DetailPart> = if age.is_empty() {
        Vec::new()
    } else {
        vec![sep.clone(), detail(age, THEME.muted)]
    };
    let with_age = |mut line: DetailLine| {
        line.extend(age_parts.iter().cloned());
        line
    };
    let mut candidates: Vec<DetailLine> = Vec::new();
    if session_outdated(session, installed) {
        let launched = session.agent_version.as_deref().unwrap_or("");
        let installed = installed.unwrap_or("");
        candidates.extend([
            with_age(vec![
                detail(format!("{glyph} {program} {launched}"), THEME.muted),
                sep.clone(),
                detail(format!("{installed} installed"), THEME.muted),
                sep.clone(),
                state_text.clone(),
            ]),
            with_age(vec![
                detail(format!("{glyph} {program} {launched} → {installed}"), THEME.muted),
                sep.clone(),
                state_text.clone(),
            ]),
            with_age(vec![
                detail(format!("{glyph} {launched} → {installed}"), THEME.muted),
                sep.clone(),
                state_text.clone(),
            ]),
            vec![
                detail(format!("{glyph} {launched} → {installed}"), THEME.muted),
                sep.clone(),
                state_text.clone(),
            ],
        ]);
    }
    candidates.extend([
        with_age(vec![
            detail(format!("{glyph} {program}"), THEME.muted),
            sep.clone(),
            state_text.clone(),
        ]),
        with_age(vec![detail(format!("{glyph} "), THEME.muted), state_text.clone()]),
        vec![detail(format!("{glyph} "), THEME.muted), state_text],
    ]);
    let line_width = |line: &DetailLine| line.iter().map(|part| jlen(&part.text)).sum::<i64>();
    let width = jnum(width);
    let chosen = candidates
        .iter()
        .find(|line| line_width(line) <= width)
        .or(candidates.last())
        .cloned()
        .unwrap_or_default();
    cut_parts(&chosen, width)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_features::tests::{partial_session, sessions_arg};
    use deckhand_parity::{Case, Outcome, replay, tags};
    use serde_json::Value;

    const UNIT: &str = "P8";

    /// A width argument: every width at or below zero lays out the same in TS (nothing fits), so negatives are 0.
    fn width_of(value: &Value) -> Result<usize, String> {
        let number = value.as_f64().ok_or("width is not a number")?;
        Ok(if number <= 0.0 { 0 } else { number as usize })
    }

    fn arg_width(case: &Case, index: usize) -> Result<usize, String> {
        width_of(case.arg(index))
    }

    fn refs(sessions: &[SessionRecord]) -> Vec<&SessionRecord> {
        sessions.iter().collect()
    }

    fn installed_of(value: Option<&Value>) -> Result<Option<InstalledVersions>, String> {
        value
            .map(|value| serde_json::from_value(value.clone()).map_err(|error| error.to_string()))
            .transpose()
    }

    fn id_set(value: Option<&Value>) -> Result<Option<HashSet<String>>, String> {
        value
            .map(|value| serde_json::from_value(value.clone()).map_err(|error| error.to_string()))
            .transpose()
    }

    #[test]
    fn row_dimmed_parity() {
        replay(UNIT, "sidebarModel/rowDimmed", |case: &Case| {
            Ok::<_, String>(row_dimmed(&partial_session(case.arg(0))?, case.arg_as(1)?))
        });
    }

    #[test]
    fn status_words_parity() {
        replay(UNIT, "sidebarModel/statusWords", |case: &Case| {
            Ok::<_, String>(status_words(&partial_session(case.arg(0))?))
        });
    }

    #[test]
    fn status_since_parity() {
        replay(UNIT, "sidebarModel/statusSince", |case: &Case| -> Outcome {
            match partial_session(case.arg(0)) {
                Ok(session) => match status_since(&session) {
                    Some(since) => Outcome::ret(since),
                    None => Outcome::ret(tags::undefined()),
                },
                Err(error) => Outcome::throws(error),
            }
        });
    }

    #[test]
    fn format_age_parity() {
        replay(UNIT, "sidebarModel/formatAge", |case: &Case| {
            Ok::<_, String>(format_age(case.arg_f64(0)?))
        });
    }

    #[test]
    fn ms_until_age_changes_parity() {
        replay(UNIT, "sidebarModel/msUntilAgeChanges", |case: &Case| {
            Ok::<_, String>(tags::number(ms_until_age_changes(case.arg_f64(0)?)))
        });
    }

    #[test]
    fn filter_menu_lines_parity() {
        replay(UNIT, "sidebarModel/filterMenuLines", |case: &Case| {
            Ok::<_, String>(filter_menu_lines(
                case.arg_as(0)?,
                &case.arg_as(1)?,
                arg_width(case, 2)?,
            ))
        });
    }

    #[test]
    fn sidebar_header_parity() {
        replay(
            UNIT,
            "sidebarModel/sidebarHeader",
            |case: &Case| -> Result<SidebarHeader, String> {
                let input = tags::untag(case.arg(0));
                let field = |key: &str| input.get(key).cloned().unwrap_or(Value::Null);
                let all = sessions_arg(&field("allSessions"))?;
                let all = refs(&all);
                let count = |key: &str| -> Result<usize, String> {
                    serde_json::from_value(field(key)).map_err(|error| format!("{key}: {error}"))
                };
                Ok(sidebar_header(&SidebarHeaderInput {
                    width: width_of(&field("width"))?,
                    filter: serde_json::from_value(field("filter")).map_err(|error| error.to_string())?,
                    query: input.get("query").and_then(Value::as_str).ok_or("query")?,
                    shown: count("shown")?,
                    total: count("total")?,
                    all_sessions: &all,
                }))
            },
        );
    }

    #[test]
    fn sidebar_rows_parity() {
        replay(
            UNIT,
            "sidebarModel/sidebarRows",
            |case: &Case| -> Result<Vec<SidebarRow>, String> {
                let input = tags::untag(case.arg(0));
                let field = |key: &str| input.get(key).cloned().unwrap_or(Value::Null);
                let rows = sessions_arg(&field("rows"))?;
                let all = sessions_arg(&field("allSessions"))?;
                let (rows, all) = (refs(&rows), refs(&all));
                let number = |key: &str| -> Result<usize, String> {
                    serde_json::from_value(field(key)).map_err(|error| format!("{key}: {error}"))
                };
                let collapsed = id_set(input.get("collapsedSessionIds"))?;
                let hidden = id_set(input.get("hiddenSessionIds"))?;
                let installed = installed_of(input.get("installedVersions"))?;
                Ok(sidebar_rows(&SidebarRowsInput {
                    rows: &rows,
                    all_sessions: &all,
                    first_number: number("firstNumber")?,
                    number_width: width_of(&field("numberWidth"))?,
                    selected_id: input.get("selectedId").and_then(Value::as_str),
                    width: width_of(&field("width"))?,
                    spinner_frame: input
                        .get("spinnerFrame")
                        .and_then(Value::as_str)
                        .ok_or("spinnerFrame")?,
                    filter: serde_json::from_value(field("filter")).map_err(|error| error.to_string())?,
                    collapsed_session_ids: collapsed.as_ref(),
                    hidden_session_ids: hidden.as_ref(),
                    installed_versions: installed.as_ref(),
                    shared_dev: false,
                }))
            },
        );
    }

    #[test]
    fn location_text_parity() {
        replay(
            UNIT,
            "sidebarModel/locationText",
            |case: &Case| -> Result<String, String> {
                let session = partial_session(case.arg(0))?;
                let all = sessions_arg(&tags::untag(case.arg(1)))?;
                let width = if case.arg_is_undefined(2) {
                    None
                } else {
                    let width = case.arg_f64(2)?;
                    if width == f64::INFINITY {
                        None
                    } else {
                        Some(width_of(&serde_json::json!(width))?)
                    }
                };
                let now = if case.arg_is_undefined(3) {
                    None
                } else {
                    Some(case.arg_i64(3)?)
                };
                Ok(location_text(&session, &all, width, now))
            },
        );
    }

    #[test]
    fn session_details_parity() {
        replay(
            UNIT,
            "sidebarModel/sessionDetails",
            |case: &Case| -> Result<Vec<DetailLine>, String> {
                let session = if case.arg_is_undefined(0) {
                    None
                } else {
                    Some(partial_session(case.arg(0))?)
                };
                let all = sessions_arg(&tags::untag(case.arg(1)))?;
                let installed = if case.arg_is_undefined(5) {
                    None
                } else {
                    installed_of(Some(&tags::untag(case.arg(5))))?
                };
                let task = if case.arg_is_undefined(6) {
                    None
                } else {
                    let task = tags::untag(case.arg(6));
                    Some(DetailTask {
                        title: task
                            .get("title")
                            .and_then(Value::as_str)
                            .ok_or("task title")?
                            .to_owned(),
                        // A missing `done` is falsy, as in TS.
                        done: task.get("done").and_then(Value::as_bool).unwrap_or(false),
                        more: task.get("more").map(width_of).transpose()?,
                    })
                };
                Ok(session_details(
                    session.as_ref(),
                    &all,
                    arg_width(case, 2)?,
                    arg_width(case, 3)?,
                    case.arg_i64(4)?,
                    installed.as_ref(),
                    task.as_ref(),
                ))
            },
        );
    }

    // --- tests/sidebarModel.test.ts -----------------------------------------------------------------------------

    use crate::session_order::sort_sessions_for_sidebar;
    use serde_json::json;
    use std::sync::atomic::{AtomicI64, Ordering};

    const NOW: i64 = 1_791_201_600_000; // 2026-10-05T12:00:00.000Z
    static ORDER: AtomicI64 = AtomicI64::new(0);

    fn ago(minutes: i64) -> String {
        date::to_iso_string(NOW - minutes * 60_000).unwrap_or_default()
    }

    /// tests/sidebarModel.test.ts `session(id, fields)`: a main-checkout claude session, idle for five minutes.
    fn sess(id: &str, fields: Value) -> SessionRecord {
        let mut base = json!({
            "id": id, "title": id, "program": "claude", "command": "claude", "cwd": "/repo", "repoRoot": "/repo",
            "launchWorktreeRoot": "/repo", "worktree": {"mode": "none"}, "status": "running", "agentStatus": "idle",
            "agentStatusUpdatedAt": ago(5), "createdAt": ago(300), "updatedAt": ago(5),
            "sidebarOrder": ORDER.fetch_add(1, Ordering::Relaxed),
        });
        if let (Some(base), Some(fields)) = (base.as_object_mut(), fields.as_object()) {
            base.extend(fields.clone());
        }
        partial_session(&base).expect("a session literal")
    }

    fn auth() -> Value {
        json!({"mode": "managed", "id": "w1", "path": "/wt/auth", "branch": "feat/auth"})
    }

    /// `{...inAuth, ...fields}`: a session in the auth worktree without its own record of it.
    fn in_auth(fields: Value) -> Value {
        let mut value =
            json!({"cwd": "/wt/auth", "launchWorktreeRoot": "/wt/auth", "worktree": {"mode": "none", "id": "w1"}});
        if let (Some(value), Some(fields)) = (value.as_object_mut(), fields.as_object()) {
            value.extend(fields.clone());
        }
        value
    }

    /// A parent with a clean and a forked child in one worktree running Dev, a codex session waiting for input, a
    /// failed exit, an archived merged session and one still starting.
    fn fixture() -> Vec<SessionRecord> {
        let list = vec![
            sess(
                "a",
                json!({"title": "auth refactor", "cwd": "/wt/auth", "worktree": auth(), "agentStatus": "active", "attention": {"state": "working", "event": "PreToolUse", "at": ago(1)}, "devRunning": true}),
            ),
            sess(
                "b",
                in_auth(
                    json!({"title": "auth refactor / write tests", "program": "pi", "parentSessionId": "a", "subSessionKind": "clean", "devRunning": true, "agentStatusUpdatedAt": ago(7)}),
                ),
            ),
            sess(
                "c",
                in_auth(
                    json!({"title": "auth refactor / try alt approach", "parentSessionId": "a", "subSessionKind": "forked", "devRunning": true, "status": "exited", "exitReason": "completed", "agentStatusUpdatedAt": ago(42)}),
                ),
            ),
            sess(
                "d",
                json!({"title": "fix flaky checkout e2e test on CI", "program": "codex", "attention": {"state": "needs-input", "event": "PermissionRequest", "at": ago(12)}}),
            ),
            sess(
                "e",
                json!({"title": "migrate billing webhooks to v2", "cwd": "/wt/billing", "worktree": {"mode": "managed", "id": "w2", "path": "/wt/billing", "branch": "feat/billing"}, "status": "exited", "exitReason": "failed", "agentStatusUpdatedAt": ago(190)}),
            ),
            sess(
                "f",
                json!({"title": "docs cleanup", "cwd": "/wt/docs", "worktree": {"mode": "managed", "id": "w3", "path": "/wt/docs", "branch": "docs/cleanup", "mergedAt": ago(3000)}, "status": "exited", "exitReason": "completed", "archivedAt": ago(2900), "agentStatusUpdatedAt": ago(3100)}),
            ),
            sess(
                "g",
                json!({"title": "bump deps", "program": "pi", "status": "starting", "agentStatus": null, "requestedWorktreeMode": "new", "worktree": null}),
            ),
        ];
        sort_sessions_for_sidebar(&list).into_iter().cloned().collect()
    }

    /// tests/sidebarModel.test.ts `rows(input)` overrides.
    #[derive(Default)]
    struct Opts<'a> {
        all: Option<&'a [&'a SessionRecord]>,
        selected: Option<&'a str>,
        width: Option<usize>,
        first: Option<usize>,
        number_width: Option<usize>,
        filter: Option<SessionFilter>,
        collapsed: Option<HashSet<String>>,
        shared_dev: bool,
    }

    fn rows_of(rows: &[&SessionRecord], opts: Opts<'_>) -> Vec<SidebarRow> {
        sidebar_rows(&SidebarRowsInput {
            rows,
            all_sessions: opts.all.unwrap_or(rows),
            first_number: opts.first.unwrap_or(1),
            number_width: opts.number_width.unwrap_or_else(|| rows.len().to_string().len()),
            selected_id: opts.selected,
            width: opts.width.unwrap_or(45),
            spinner_frame: "⠋",
            filter: opts.filter.unwrap_or(SessionFilter::All),
            collapsed_session_ids: opts.collapsed.as_ref(),
            hidden_session_ids: None,
            installed_versions: None,
            shared_dev: opts.shared_dev,
        })
    }

    fn row_text(row: &SidebarRow) -> String {
        row.parts.iter().map(|part| part.text.as_str()).collect()
    }

    fn texts(rows: &[&SessionRecord], opts: Opts<'_>) -> Vec<String> {
        rows_of(rows, opts).iter().map(row_text).collect()
    }

    fn ids(values: &[&str]) -> HashSet<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn line_texts(lines: &[DetailLine]) -> Vec<String> {
        lines
            .iter()
            .map(|line| line.iter().map(|part| part.text.as_str()).collect())
            .collect()
    }

    // covers: B-P8-01 sidebar rows: unbracketed numbers, tree and status before the title, markers and the agent last; exact width
    #[test]
    fn sidebar_rows_layout() {
        let list = fixture();
        let all = refs(&list);
        assert_eq!(
            texts(
                &all,
                Opts {
                    selected: Some("d"),
                    ..Opts::default()
                }
            ),
            [
                "  1 ▾ ⠋ auth refactor                     ▶ ✶",
                "  2   ↳ ● write tests                       π",
                "  3   ⑂ ○ try alt approach                  ✶",
                "› 4 ? fix flaky checkout e2e test on CI     ◇",
                "  5 ! migrate billing webhooks to v2        ✶",
                "  6 ○ docs cleanup                      ▣ ✓ ✶",
                "  7 ⠋ bump deps                             π",
            ]
        );
        // Narrow (24 columns: 21 with the cursor column): titles are cut, markers stay.
        let narrow = texts(
            &all,
            Opts {
                selected: Some("d"),
                width: Some(21),
                ..Opts::default()
            },
        );
        assert_eq!(
            narrow,
            [
                "  1 ▾ ⠋ auth ref… ▶ ✶",
                "  2   ↳ ● write te… π",
                "  3   ⑂ ○ try alt … ✶",
                "› 4 ? fix flaky ch… ◇",
                "  5 ! migrate bill… ✶",
                "  6 ○ docs cle… ▣ ✓ ✶",
                "  7 ⠋ bump deps     π",
            ]
        );
        for row in &narrow {
            assert_eq!(js_len(row), 21);
        }
        // Numbers are positions in the whole list, right-aligned to the widest; the parts carry roles for styling.
        let one = [all[3]];
        let rows = rows_of(
            &one,
            Opts {
                first: Some(12),
                number_width: Some(2),
                selected: Some("x"),
                ..Opts::default()
            },
        );
        assert_eq!(row_text(&rows[0]), "  12 ? fix flaky checkout e2e test on CI    ◇");
        let roles: Vec<RowRole> = rows[0]
            .parts
            .iter()
            .filter(|part| !js_trim(&part.text).is_empty())
            .map(|part| part.role)
            .collect();
        assert_eq!(
            roles,
            [RowRole::Number, RowRole::Status, RowRole::Title, RowRole::Agent]
        );
        // Deep nesting at a narrow width gives up the count, then other markers, before the title gets under 4 columns.
        let mut deep = vec![sess("p0", json!({"cleanupError": "x"}))];
        for level in 1..=5 {
            deep.push(sess(
                &format!("p{level}"),
                json!({"parentSessionId": format!("p{}", level - 1), "subSessionKind": "clean", "mergedAt": ago(1)}),
            ));
        }
        let deep = refs(&deep);
        let collapsed = texts(
            &deep,
            Opts {
                width: Some(21),
                collapsed: Some(ids(&["p2", "p4"])),
                ..Opts::default()
            },
        );
        assert_eq!(collapsed[0], "  1 ▾ ● p0        ! ✶");
        // p2 (collapsed, merged) drops its +3 count; at depth 4 only the agent stays and the title gets what is left.
        assert_eq!(collapsed[2], "  3     ▸ ↳ ● p2  ✓ ✶");
        assert_eq!(collapsed[4], "  5         ▸ ↳ ● p ✶");
    }

    // covers: B-P8-02 sidebar rows: ▶ once per workspace, on its first row on screen; rows sharing the selected workspace get ╎
    #[test]
    fn sidebar_rows_dev_and_gutter() {
        let list = fixture();
        let all = refs(&list);
        let mark = |row: &SidebarRow| {
            format!(
                "{}{}",
                row.parts[0].text,
                if row.parts.iter().any(|part| part.role == RowRole::Dev) {
                    "▶"
                } else {
                    ""
                }
            )
        };
        let marks = |selected: &str, rows: &[&SessionRecord]| -> Vec<String> {
            rows_of(
                rows,
                Opts {
                    selected: Some(selected),
                    all: Some(&all),
                    ..Opts::default()
                },
            )
            .iter()
            .map(mark)
            .collect()
        };
        assert_eq!(marks("b", &all), ["╎▶", "›", "╎", " ", " ", " ", " "]);
        // Scrolled past the parent: the first row of the workspace on screen carries ▶.
        assert_eq!(marks("c", &all[1..4]), ["╎▶", "›", " "]);
        // Main-checkout sessions share /repo; a session without a workspace keeps its own ▶.
        let main_a = sess("m1", json!({"devRunning": true}));
        let main_b = sess("m2", json!({"devRunning": true}));
        let loose = sess(
            "m3",
            json!({"devRunning": true, "requestedWorktreeMode": "new", "worktree": null}),
        );
        let loose2 = sess(
            "m4",
            json!({"devRunning": true, "worktree": {"mode": "managed", "path": "/wt/x", "deletedAt": ago(1)}}),
        );
        let mains = [&main_a, &main_b, &loose, &loose2];
        let got: Vec<String> = rows_of(
            &mains,
            Opts {
                selected: Some("m2"),
                ..Opts::default()
            },
        )
        .iter()
        .map(mark)
        .collect();
        assert_eq!(got, ["╎▶", "›", " ▶", " ▶"]);
        // No marker without a selection on screen, and none for an unshared workspace.
        assert_eq!(marks("none", &all), [" ▶", " ", " ", " ", " ", " ", " "]);
        assert_eq!(marks("e", &all), [" ▶", " ", " ", " ", "›", " ", " "]);
    }

    // Not in the TS: with shared_dev, every row sharing a running Dev shows ▶, muted after the first.
    #[test]
    fn shared_dev_marks_every_row_of_the_workspace() {
        let main_a = sess("m1", json!({"devRunning": true}));
        let main_b = sess("m2", json!({"devRunning": true}));
        let main_c = sess("m3", json!({"devRunning": false}));
        let rows = [&main_a, &main_b, &main_c];
        let roles: Vec<Option<RowRole>> = rows_of(
            &rows,
            Opts {
                shared_dev: true,
                ..Opts::default()
            },
        )
        .iter()
        .map(|row| row.parts.iter().find(|part| part.text == "▶").map(|part| part.role))
        .collect();
        assert_eq!(roles, [Some(RowRole::Dev), Some(RowRole::DevShared), None]);
        // The muted ones go first when the row is too narrow.
        assert_eq!(DROP_ORDER[0], RowRole::DevShared);
    }

    // covers: B-P8-03 sidebar rows: archived rows dimmed, except in the archived view, which lists only archived ones (a sub-session of a live session at top level)
    #[test]
    fn sidebar_rows_archived_dimming() {
        let parent = sess("parent", json!({"title": "parent"}));
        let archived_child = sess(
            "child",
            json!({"parentSessionId": "parent", "subSessionKind": "clean", "archivedAt": ago(1)}),
        );
        let archived = sess("old", json!({"archivedAt": ago(1)}));
        let list = [&parent, &archived_child, &archived];
        let dimmed = |filter: SessionFilter, selected: Option<&str>| -> Vec<bool> {
            rows_of(
                &list,
                Opts {
                    filter: Some(filter),
                    selected,
                    ..Opts::default()
                },
            )
            .iter()
            .map(|row| row.dimmed)
            .collect()
        };
        assert_eq!(dimmed(SessionFilter::All, None), [false, true, true]);
        // The archived view: no live parent brought along, nothing dimmed, the child not indented (↳ still says what it
        // is).
        let archived_view: Vec<&SessionRecord> = filter_session_list(&list, SessionFilter::Archived, "", None)
            .into_iter()
            .copied()
            .collect();
        assert_eq!(
            archived_view.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
            ["child", "old"]
        );
        let shown = rows_of(
            &archived_view,
            Opts {
                all: Some(&list),
                filter: Some(SessionFilter::Archived),
                ..Opts::default()
            },
        );
        assert_eq!(shown.iter().map(|row| row.dimmed).collect::<Vec<_>>(), [false, false]);
        let tree_text = |row: &SidebarRow| {
            row.parts
                .iter()
                .find(|part| part.role == RowRole::Tree)
                .map(|part| part.text.clone())
        };
        assert_eq!(tree_text(&shown[0]).as_deref(), Some("↳ "));
        // An archived parent keeps its archived child under it.
        let archived_parent = SessionRecord {
            archived_at: Some(ago(2)),
            ..parent.clone()
        };
        let both = [&archived_parent, &archived_child];
        let both_view: Vec<&SessionRecord> = filter_session_list(&both, SessionFilter::Archived, "", None)
            .into_iter()
            .copied()
            .collect();
        let both_rows = rows_of(
            &both_view,
            Opts {
                all: Some(&both),
                filter: Some(SessionFilter::Archived),
                ..Opts::default()
            },
        );
        assert_eq!(tree_text(&both_rows[1]).as_deref(), Some("  ↳ "));
        // The selected row is never dimmed; ▣ stays.
        assert_eq!(dimmed(SessionFilter::All, Some("old")), [false, true, false]);
        assert!(
            rows_of(
                &list,
                Opts {
                    filter: Some(SessionFilter::All),
                    ..Opts::default()
                }
            )[2]
            .parts
            .iter()
            .any(|part| part.text == "▣")
        );
    }

    // covers: B-P8-04 sidebar rows: an archived sub-session (gone from the active view) gives its parent no ▾ ▸, +N or ╎N; the archived view still does
    #[test]
    fn sidebar_rows_archived_sub_session() {
        let parent = sess("parent", in_auth(json!({"title": "parent", "worktree": auth()})));
        let child = sess(
            "child",
            in_auth(
                json!({"parentSessionId": "parent", "subSessionKind": "forked", "worktree": auth(), "status": "exited", "archivedAt": ago(1)}),
            ),
        );
        let all = [&parent, &child];
        let tree = |filter: SessionFilter, collapsed: &[&str]| -> Vec<String> {
            rows_of(
                &[&parent],
                Opts {
                    all: Some(&all),
                    filter: Some(filter),
                    collapsed: Some(ids(collapsed)),
                    ..Opts::default()
                },
            )[0]
            .parts
            .iter()
            .filter(|part| matches!(part.role, RowRole::Tree | RowRole::Count))
            .map(|part| part.text.clone())
            .collect()
        };
        assert!(tree(SessionFilter::Active, &[]).is_empty());
        assert!(tree(SessionFilter::Active, &["parent"]).is_empty());
        assert_eq!(tree(SessionFilter::Archived, &[]), ["▾ "]);
        assert_eq!(tree(SessionFilter::All, &["parent"]), ["▸ ", "+1"]);
        assert_eq!(location_text(&parent, &all, None, None), "⎇ feat/auth");
        let unarchived = SessionRecord {
            archived_at: None,
            ..child.clone()
        };
        assert_eq!(
            location_text(&parent, &[&parent, &unarchived], None, None),
            "⎇ feat/auth ╎1"
        );
    }

    // covers: B-P8-05 sidebar header: plain count by default, filter/search with counts otherwise, attention count in front of the title
    #[test]
    fn sidebar_header_counts() {
        let list = fixture();
        let all = refs(&list);
        let header = |width: usize, filter: SessionFilter, query: &str, shown: usize, sessions: &[&SessionRecord]| {
            sidebar_header(&SidebarHeaderInput {
                width,
                filter,
                query,
                shown,
                total: 7,
                all_sessions: sessions,
            })
        };
        let expect = |title: &str, label: &str, attention: &str, highlighted: bool| SidebarHeader {
            title: title.into(),
            label: label.into(),
            attention: attention.into(),
            highlighted,
        };
        // d needs input and e failed; an archived session needing attention is not counted.
        assert_eq!(
            header(30, SessionFilter::Active, "", 6, &all),
            expect("Sessions", "6", "! 2", false)
        );
        let archived_e = SessionRecord {
            archived_at: Some(ago(1)),
            ..all[4].clone()
        };
        assert_eq!(
            header(30, SessionFilter::Active, "", 6, &[all[0], &archived_e]),
            expect("Sessions", "6", "", false)
        );
        assert_eq!(
            header(30, SessionFilter::Archived, "", 1, &all),
            expect("Sessions", "archived 1/7", "! 2", true)
        );
        assert_eq!(
            header(30, SessionFilter::Active, "auth", 3, &all),
            expect("Sessions", "/auth 3/7", "! 2", true)
        );
        assert_eq!(
            header(30, SessionFilter::Attention, " auth ", 2, &[]),
            expect("Sessions", "attention /auth 2/7", "", true)
        );
        // Narrow: the title goes first, then the filter/search is cut; the count and attention stay.
        assert_eq!(
            header(20, SessionFilter::Archived, "", 1, &all),
            expect("", "archived 1/7", "! 2", true)
        );
        assert_eq!(
            header(20, SessionFilter::Active, "checkout flake", 1, &all),
            expect("", "/checkout… 1/7", "! 2", true)
        );
        assert_eq!(
            header(12, SessionFilter::Active, "checkout flake", 1, &all),
            expect("", "1/7", "! 2", true)
        );
    }

    // covers: B-P8-06 sidebar details: title, agent · state · age, location; shrinks with fewer free rows
    #[test]
    fn sidebar_details_shrink() {
        let list = fixture();
        let all = refs(&list);
        let by_id = |id: &str| *all.iter().find(|item| item.id == id).expect("listed");
        let details_in = |id: Option<&str>, width: usize, free: usize, sessions: &[&SessionRecord]| {
            let session = id.and_then(|id| sessions.iter().find(|item| item.id == id).copied());
            line_texts(&session_details(session, sessions, width, free, NOW, None, None))
        };
        let details = |id: &str, width: usize, free: usize| details_in(Some(id), width, free, &all);
        let rule = "─".repeat(30);
        assert_eq!(
            details("d", 30, 10),
            [
                rule.as_str(),
                "fix flaky checkout e2e test on",
                "CI",
                "◇ codex · needs input · 12m",
                "main checkout"
            ]
        );
        // Title to one line with four free rows, the location dropped with three, nothing below that or without a
        // selection.
        assert_eq!(
            details("d", 30, 4),
            [
                rule.as_str(),
                "fix flaky checkout e2e test o…",
                "◇ codex · needs input · 12m",
                "main checkout"
            ]
        );
        assert_eq!(
            details("d", 30, 3),
            [
                rule.as_str(),
                "fix flaky checkout e2e test o…",
                "◇ codex · needs input · 12m"
            ]
        );
        assert!(details("d", 30, 2).is_empty());
        assert!(details_in(None, 30, 10, &all).is_empty());
        // More than two lines of title: the second ends in ….
        assert_eq!(details("d", 12, 10)[1..3], ["fix flaky", "checkout e2…"]);
        // Worktree sessions: the branch (a sub-session without its own takes the workspace's), who shares it, Dev.
        assert_eq!(details("b", 48, 10)[2..], ["π pi · idle · 7m", "⎇ feat/auth ╎2 ▶"]);
        assert_eq!(
            details("c", 48, 10)[2..],
            ["✶ claude · exited · 42m", "⎇ feat/auth ╎2 ▶"]
        );
        assert_eq!(
            details("e", 48, 10)[2..],
            ["✶ claude · exited (failed) · 3h", "⎇ feat/billing"]
        );
        assert_eq!(
            details("f", 48, 10)[2..],
            ["✶ claude · exited · 2d", "⎇ docs/cleanup ✓ ▣"]
        );
        assert_eq!(
            details("g", 48, 10)[2..],
            ["π pi · starting · 5m", "preparing worktree"]
        );
        // Narrow: the agent's name goes before the age, the age before the state.
        assert_eq!(details("e", 20, 10)[3..], ["✶ exited (failed)", "⎇ feat/billing"]);
        assert_eq!(details("c", 20, 10)[2..], ["✶ exited · 42m", "⎇ feat/auth ╎2 ▶"]);
        // Markers are glyphs, kept by cutting a long branch; only below 8 columns for it are they dropped (… marks that
        // when it fits).
        assert_eq!(location_text(by_id("b"), &all, Some(13), None), "⎇ feat/… ╎2 ▶");
        assert_eq!(location_text(by_id("b"), &all, Some(12), None), "⎇ feat/auth");
        assert_eq!(location_text(by_id("b"), &all, Some(10), None), "⎇ feat/au…");
        let mut long_tree = auth();
        long_tree["id"] = json!("w8");
        long_tree["path"] = json!("/wt/long");
        long_tree["branch"] = json!("feature/a-very-long-branch");
        let long = sess("long", json!({"worktree": long_tree, "archivedAt": ago(1)}));
        assert_eq!(
            location_text::<&SessionRecord>(&long, &[], Some(20), None),
            "⎇ feature/a-very-… ▣"
        );
        let mut deleted_tree = auth();
        deleted_tree["id"] = json!("w9");
        deleted_tree["deletedAt"] = json!(ago(1));
        let deleted = sess(
            "z",
            json!({"title": "gone", "worktree": deleted_tree, "status": "exited", "exitReason": "interrupted"}),
        );
        let mut with_deleted = all.clone();
        with_deleted.push(&deleted);
        assert_eq!(
            details_in(Some("z"), 48, 10, &with_deleted)[2..],
            ["✶ claude · interrupted · 5m", "worktree deleted"]
        );
        let d2 = sess("d2", json!({}));
        let mut with_d2 = all.clone();
        with_d2.push(&d2);
        assert_eq!(location_text(by_id("d"), &with_d2, None, None), "main checkout ╎1");
        let main = sess(
            "main",
            json!({"worktree": {"mode": "attached", "path": "/repo", "isMain": true, "branch": "main"}}),
        );
        assert_eq!(location_text::<&SessionRecord>(&main, &[], None, None), "main checkout");
        let no_branch = sess(
            "nb",
            json!({"cwd": "/wt/x", "launchWorktreeRoot": "/wt/x", "worktree": {"mode": "none", "id": "w7"}}),
        );
        assert_eq!(
            location_text::<&SessionRecord>(&no_branch, &[], None, None),
            "worktree x"
        );
    }

    // covers: B-P8-07 sidebar details: why a running session waits, under its state; two lines while the location still fits after them
    #[test]
    fn sidebar_details_attention_reason() {
        let asking = sess(
            "q",
            json!({"title": "auth", "attention": {"state": "needs-input", "event": "PreToolUse", "at": ago(2), "reason": {"kind": "question", "text": "asks: Should sessions live in Redis or Postgres?", "options": ["Redis", "Postgres"]}}}),
        );
        let details = |item: &SessionRecord, free: usize| {
            line_texts(&session_details(Some(item), &[item], 30, free, NOW, None, None))
        };
        let rule = "─".repeat(30);
        assert_eq!(
            details(&asking, 10),
            [
                rule.as_str(),
                "auth",
                "✶ claude · needs input · 2m",
                "asks: Should sessions live in",
                "Redis or Postgres?",
                "main checkout"
            ]
        );
        // Short of rows: one line, cut; then the location goes.
        assert_eq!(
            details(&asking, 5),
            [
                rule.as_str(),
                "auth",
                "✶ claude · needs input · 2m",
                "asks: Should sessions live in…",
                "main checkout"
            ]
        );
        assert_eq!(
            details(&asking, 4),
            [
                rule.as_str(),
                "auth",
                "✶ claude · needs input · 2m",
                "asks: Should sessions live in…"
            ]
        );
        let lines = session_details(Some(&asking), &[&asking], 30, 10, NOW, None, None);
        assert_eq!(lines[3][0].color, Some("yellow"));
        // A reply's last line is muted; an exited session's leftover reason is not shown.
        let said = sess(
            "m",
            json!({"title": "auth", "attention": {"state": "response-ended", "event": "Stop", "at": ago(2), "reason": {"kind": "message", "text": "said: Shall I push?"}}}),
        );
        assert_eq!(details(&said, 10)[3..4], ["said: Shall I push?"]);
        assert_eq!(
            session_details(Some(&said), &[&said], 30, 10, NOW, None, None)[3][0].color,
            Some("gray")
        );
        let exited = SessionRecord {
            status: SessionStatus::Exited,
            exit_reason: Some(ExitReason::Stopped),
            ..said.clone()
        };
        assert_eq!(details(&exited, 10)[3..], ["main checkout"]);
    }

    // covers: B-P8-08 sidebar details: state words and their timestamps follow the status glyph inputs; compact ages
    #[test]
    fn sidebar_details_state_words_and_ages() {
        let words = |fields: Value| status_words(&sess("s", fields));
        assert_eq!(words(json!({})), "idle");
        assert_eq!(words(json!({"agentStatus": "active"})), "working");
        assert_eq!(words(json!({"agentStatus": "unknown"})), "running");
        assert_eq!(
            words(
                json!({"attention": {"state": "unknown", "event": "SessionStart", "at": ago(1)}, "agentStatus": null})
            ),
            "running"
        );
        assert_eq!(
            words(json!({"attention": {"state": "response-ended", "event": "Stop", "at": ago(1)}})),
            "response ended"
        );
        assert_eq!(
            words(json!({"attention": {"state": "failed", "event": "StopFailure", "at": ago(1)}})),
            "failed"
        );
        assert_eq!(
            words(json!({"attention": {"state": "limited", "event": "StopFailure", "at": ago(1)}})),
            "rate-limited"
        );
        assert_eq!(words(json!({"status": "starting"})), "starting");
        assert_eq!(
            words(
                json!({"status": "exited", "exitReason": "stopped", "attention": {"state": "needs-input", "event": "x", "at": ago(1)}})
            ),
            "exited"
        );
        let since = |fields: Value| status_since(&sess("s", fields)).map(str::to_owned);
        assert_eq!(
            since(json!({"attention": {"state": "needs-input", "event": "x", "at": ago(12)}})),
            Some(ago(12))
        );
        assert_eq!(
            since(json!({"status": "exited", "attention": {"state": "needs-input", "event": "x", "at": ago(12)}})),
            Some(ago(5))
        );
        assert_eq!(
            since(json!({"agentStatusUpdatedAt": null, "updatedAt": ago(3)})),
            Some(ago(3))
        );
        let ages: Vec<String> = [
            0.0,
            59_999.0,
            60_000.0,
            59.0 * 60_000.0,
            3_600_000.0,
            23.0 * 3_600_000.0 + 1.0,
            86_400_000.0 * 3.0,
            -5.0,
            f64::NAN,
        ]
        .into_iter()
        .map(format_age)
        .collect();
        assert_eq!(ages, ["now", "now", "1m", "59m", "1h", "23h", "3d", "now", "now"]);
        // The next change of the shown age: the next minute, hour or day boundary.
        assert_eq!(ms_until_age_changes(30_000.0), 30_050.0);
        assert_eq!(ms_until_age_changes(90.0 * 60_000.0), 30.0 * 60_000.0 + 50.0);
        assert_eq!(ms_until_age_changes(-1.0), 60_000.0);
    }

    // covers: B-P8-09 sidebar done marker: ☑ after ✓ in the suffix, dropped after ✓ when narrow; done rows slightly dimmed; details say done 2d ago
    #[test]
    fn sidebar_done_marker() {
        // A worktree session merged and done, a main-checkout session only done, an archived done one, and a plain one.
        let mut both_tree = auth();
        both_tree["mergedAt"] = json!(ago(10));
        let both = sess(
            "both",
            json!({"title": "ship auth", "cwd": "/wt/auth", "worktree": both_tree, "doneAt": ago(3000), "status": "exited", "exitReason": "completed"}),
        );
        let main = sess("main", json!({"title": "tidy readme", "doneAt": ago(30)}));
        let archived = sess(
            "old",
            json!({"title": "old idea", "doneAt": ago(60), "archivedAt": ago(50), "status": "exited"}),
        );
        let plain_row = sess("plain", json!({"title": "plain"}));
        let list = [&both, &main, &archived, &plain_row];
        assert_eq!(
            texts(
                &list,
                Opts {
                    width: Some(34),
                    ..Opts::default()
                }
            ),
            [
                "  1 ○ ship auth              ✓ ☑ ✶",
                "  2 ● tidy readme              ☑ ✶",
                "  3 ○ old idea               ▣ ☑ ✶",
                "  4 ● plain                      ✶",
            ]
        );
        assert!(
            rows_of(&[&main], Opts::default())[0]
                .parts
                .iter()
                .any(|part| part.role == RowRole::Done && part.text == "☑")
        );
        // Narrow: ✓ goes first, then ☑, then ▣; the agent glyph stays.
        assert_eq!(
            texts(
                &[&both],
                Opts {
                    width: Some(14),
                    ..Opts::default()
                }
            ),
            ["  1 ○ shi… ☑ ✶"]
        );
        assert_eq!(
            texts(
                &[&both],
                Opts {
                    width: Some(12),
                    ..Opts::default()
                }
            ),
            ["  1 ○ shi… ✶"]
        );
        assert_eq!(
            texts(
                &[&archived],
                Opts {
                    width: Some(14),
                    ..Opts::default()
                }
            ),
            ["  1 ○ old… ▣ ✶"]
        );
        // Dimming: done rows are `done` (muted title), never archive-dimmed for it; archived rules win, the selected row
        // is neither.
        let flags = |filter: SessionFilter, selected: Option<&str>| -> Vec<String> {
            rows_of(
                &list,
                Opts {
                    filter: Some(filter),
                    selected,
                    ..Opts::default()
                },
            )
            .iter()
            .map(|row| {
                let flag = format!(
                    "{}{}",
                    if row.dimmed { "dim" } else { "" },
                    if row.done { "done" } else { "" }
                );
                if flag.is_empty() { "-".to_owned() } else { flag }
            })
            .collect()
        };
        assert_eq!(flags(SessionFilter::All, None), ["done", "done", "dim", "-"]);
        // The archived view dims nothing (it lists only archived sessions).
        assert_eq!(flags(SessionFilter::Archived, None), ["done", "done", "done", "-"]);
        assert_eq!(flags(SessionFilter::All, Some("main")), ["done", "-", "dim", "-"]);
        // Details: ☑ with its age after ✓, before ▣; ☑ alone within a minute or without a time.
        let detail = |item: &SessionRecord| {
            line_texts(&session_details(Some(item), &[item], 48, 10, NOW, None, None))
                .pop()
                .unwrap_or_default()
        };
        assert_eq!(detail(&both), "⎇ feat/auth ✓ ☑2d");
        assert_eq!(detail(&main), "main checkout ☑30m");
        assert_eq!(detail(&archived), "main checkout ☑1h ▣");
        let fresh = sess("x", json!({"doneAt": ago(0)}));
        assert_eq!(
            location_text::<&SessionRecord>(&fresh, &[], None, Some(NOW)),
            "main checkout ☑"
        );
        assert_eq!(location_text(&main, &[&main], None, None), "main checkout ☑");
        assert_eq!(location_text(&both, &[&both], Some(14), Some(NOW)), "⎇ feat/… ✓ ☑2d");
        // The row styles (partStyle) and the rendered Sidebar are the sidebar pane's (U2); the model's part: the done
        // row's flags and its roles.
        let rows = rows_of(
            &list,
            Opts {
                filter: Some(SessionFilter::All),
                ..Opts::default()
            },
        );
        assert!(rows[1].done && !rows[1].dimmed);
        assert_eq!(rows[1].color, "green");
        assert!(rows[2].dimmed && !rows[2].done);
    }

    #[test]
    fn attention_reason_lines_parity() {
        replay(UNIT, "sidebarModel/attentionReasonLines", |case: &Case| {
            Ok::<_, String>(attention_reason_lines(
                &partial_session(case.arg(0))?,
                arg_width(case, 1)?,
                arg_width(case, 2)?,
            ))
        });
    }

    // covers: B-P8-14 sidebar details: ☐ open items of the session and its worktree, shortened to fit, the first line to go
    #[test]
    fn details_show_open_checklist_items() {
        let with_notes = partial_session(&serde_json::json!({
            "id": "s", "title": "task", "program": "claude", "command": "claude", "cwd": "/repo", "repoRoot": "/repo",
            "status": "running", "agentStatus": "idle",
            "createdAt": "1970-01-01T00:00:00.000Z", "updatedAt": "1970-01-01T00:00:00.000Z",
            "notes": "- [ ] a",
            "sharedNotes": {"kind": "worktree", "id": "w", "path": "/n/w.md", "revision": "r",
                "text": "- [ ] b\n- [ ] c\n- [x] d"},
        }))
        .expect("a session literal");
        let details = |free: usize| -> Vec<String> {
            session_details(Some(&with_notes), &[&with_notes], 30, free, 0, None, None)
                .iter()
                .map(|line| line.iter().map(|part| part.text.as_str()).collect())
                .collect()
        };
        let rule = "─".repeat(30);
        let full: Vec<String> = vec![
            rule.clone(),
            "task".into(),
            "✶ claude · idle · now".into(),
            "main checkout".into(),
            "☐ 3 open (2 worktree)".into(),
        ];
        assert_eq!(details(5), full);
        assert_eq!(details(4), full[..4].to_vec());
    }
}
