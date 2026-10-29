//! The session list (header, the filter menu, numbered rows that window around the selection with `↑ N more` lines, and
//! the selected session's details block). Row contents come from deckhand_core::sidebar_model; this lays them out and
//! styles them.

use std::collections::HashSet;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use deckhand_core::agent_versions::InstalledVersions;
use deckhand_core::display::{THEME as INK_THEME, display_session_title, truncate};
use deckhand_core::session_features::{SESSION_FILTERS, SessionFilterCounts, session_needs_attention};
use deckhand_core::session_sections::{SessionSection, session_section};
use deckhand_core::sidebar_model::{
    DetailLine, DetailTask, RowPart, RowRole, SidebarHeaderInput, SidebarRow, SidebarRowsInput, filter_menu_lines,
    section_parents, session_details_with_parent, sidebar_header, sidebar_rows,
};
use deckhand_protocol::data::SessionRecord;
use deckhand_protocol::records::SessionFilter;

use crate::primitives::{
    Border, Column, Row, Seg, TextStyle, Wrap, bordered, inset, space_between, string_width, truncate_row, with_base,
};
use crate::theme::{THEME, color};

/// TS: sidebar.tsx DETAIL_ROWS: the most rows the details block can use (9 in the TS; not in the TS: one more for a
/// sub-session's parent).
pub const DETAIL_ROWS: usize = 10;

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
    /// Not in the TS: the state view: `sessions` are in sections ([`deckhand_core::session_sections`]), each under a
    /// header.
    pub sectioned: bool,
    /// Not in the TS: keys go to the selected session's agent or shell: everything but its row is dimmed, so the list
    /// reads as not listening.
    pub typing: bool,
    /// Not in the TS: the state view's Exited section is folded (`c`), holding this many exited sessions; `sessions`
    /// lists at most the selected one of them.
    pub folded_exited: Option<usize>,
}

/// TS: sidebar.tsx visibleSessions: the index range of the rows shown, `available_rows` around the selection.
pub fn visible_range(count: usize, selected_index: usize, available_rows: usize) -> std::ops::Range<usize> {
    if available_rows == 0 || count <= available_rows {
        return 0..count;
    }
    let half = available_rows / 2;
    let max_start = count.saturating_sub(available_rows);
    let start = selected_index.saturating_sub(half).min(max_start);
    start..start + available_rows
}

/// Not in the TS: one line of the list: a section's header (with how many sessions it lists), the line naming the
/// parent a run of sub-sessions sits under in the state view (the index of the first of them), or a session (its
/// index).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListLine {
    Header(SessionSection, usize),
    Parent(usize),
    Session(usize),
}

/// The list's lines: the sessions, each section headed in the state view (`sessions` are already grouped). A folded
/// Exited section (`folded_exited`, its whole count) keeps its header, last, even with none of its sessions listed.
/// In the state view each run of sessions its section lists apart from their parent (`parents`, from
/// [`section_parents`]) is headed by a line naming it.
pub fn list_lines(
    sessions: &[&SessionRecord],
    sectioned: bool,
    folded_exited: Option<usize>,
    parents: &[Option<&SessionRecord>],
) -> Vec<ListLine> {
    let mut lines = Vec::with_capacity(sessions.len() + SessionSection::ALL.len());
    let mut current = None;
    let mut under: Option<&str> = None;
    for (index, session) in sessions.iter().enumerate() {
        let section = session_section(session);
        if sectioned && current != Some(section) {
            current = Some(section);
            under = None;
            let count = match folded_exited {
                Some(total) if section == SessionSection::Exited => total,
                _ => sessions[index..]
                    .iter()
                    .take_while(|session| session_section(session) == section)
                    .count(),
            };
            lines.push(ListLine::Header(section, count));
        }
        if sectioned {
            let parent = parents.get(index).copied().flatten().map(|parent| parent.id.as_str());
            if parent.is_some() && parent != under {
                lines.push(ListLine::Parent(index));
            }
            under = parent;
        }
        lines.push(ListLine::Session(index));
    }
    if let Some(total) = folded_exited.filter(|total| sectioned && *total > 0)
        && current != Some(SessionSection::Exited)
    {
        lines.push(ListLine::Header(SessionSection::Exited, total));
    }
    lines
}

/// The lines a line sits under, in order: its section's header (unless it is one) and, for a session listed apart from
/// its parent, the line naming the parent. Each is the last of its kind before it.
fn heads_of(lines: &[ListLine], at: usize, parents: &[Option<&SessionRecord>]) -> Vec<ListLine> {
    let before = &lines[..at];
    let mut heads = Vec::new();
    if !matches!(lines[at], ListLine::Header(..)) {
        heads.extend(before.iter().rev().find(|line| matches!(line, ListLine::Header(..))));
    }
    if let ListLine::Session(index) = lines[at]
        && parents.get(index).copied().flatten().is_some()
    {
        heads.extend(before.iter().rev().find(|line| matches!(line, ListLine::Parent(..))));
    }
    heads
}

/// The lines shown in `range`. A window that starts inside a section shows its header first and, inside a run of
/// sub-sessions listed apart from their parent, the line naming it, in place of the window's first lines (only while
/// the selection, kept mid-window, stays shown).
fn window_lines(
    lines: &[ListLine],
    parents: &[Option<&SessionRecord>],
    range: std::ops::Range<usize>,
) -> Vec<ListLine> {
    let mut shown = lines[range.clone()].to_vec();
    if range.start == 0 {
        return shown;
    }
    // The first line kept below the heads it needs, which take the places of the lines before it.
    let kept =
        (0..=2).find(|&skip| range.len() > 2 * skip && heads_of(lines, range.start + skip, parents).len() == skip);
    if let Some(skip) = kept.filter(|&skip| skip > 0) {
        shown.splice(0..skip, heads_of(lines, range.start + skip, parents));
    }
    shown
}

/// The line naming the parent a run of sub-sessions sits under in the state view: muted, where a top-level row's
/// status glyph goes (its sub-sessions' `↳` one level in), then a dotted rule to the edge.
fn parent_line_row(parent: &SessionRecord, all_sessions: &[&SessionRecord], number_width: usize, width: usize) -> Row {
    let indent = " ".repeat(number_width + 3);
    let title = display_session_title(parent, all_sessions);
    let used = indent.chars().count() + string_width(&title) + 1;
    let row = vec![
        Seg::plain(indent),
        Seg::new(title, TextStyle::fg(THEME.muted)),
        Seg::new(
            format!(" {}", "┄".repeat(width.saturating_sub(used))),
            TextStyle::fg(THEME.muted).dim(true),
        ),
    ];
    truncate_row(&row, width, Wrap::TruncateEnd)
}

/// A section's header row: its name in the section's colour, the count, then a rule to the edge. Exited, which `c`
/// folds, says which it is: `▾` open, `▸` folded.
fn section_header_row(section: SessionSection, count: usize, folded: bool, width: usize) -> Row {
    let tone = match section {
        SessionSection::NeedsYou => THEME.warn,
        SessionSection::Unread => THEME.accent,
        SessionSection::Working => THEME.success,
        SessionSection::Idle | SessionSection::Exited => THEME.muted,
    };
    let label = format!("  {} ", section.label());
    let count = match section {
        SessionSection::Exited => format!("{count} {}", if folded { "▸" } else { "▾" }),
        _ => count.to_string(),
    };
    let used = label.chars().count() + count.chars().count() + 1;
    let row = vec![
        Seg::new(label, TextStyle::fg(tone).bold(true)),
        Seg::new(count, TextStyle::fg(THEME.muted)),
        Seg::new(
            format!(" {}", "─".repeat(width.saturating_sub(used))),
            TextStyle::fg(THEME.muted).dim(true),
        ),
    ];
    truncate_row(&row, width, Wrap::TruncateEnd)
}

/// The row with every segment dimmed.
fn dimmed(row: Row) -> Row {
    row.into_iter()
        .map(|seg| Seg {
            style: seg.style.dim(true),
            text: seg.text,
        })
        .collect()
}

/// TS: sidebar.tsx emptyMessage: the title and hint of an empty list.
pub fn empty_message(all_sessions: usize, loaded: bool) -> (&'static str, &'static str) {
    if !loaded && all_sessions == 0 {
        return ("Loading sessions…", "");
    }
    if all_sessions > 0 {
        return ("No sessions match this view.", "f filter · / search");
    }
    ("No sessions yet.", "Press n to create.")
}

/// A part's style as Ink color names (`color`, `dimColor`), before they become ratatui colors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PartStyleSpec {
    pub color: Option<&'static str>,
    pub dim: bool,
}

/// TS: sidebar.tsx partStyle, with the colors still Ink names (a row's `color` can be any name).
pub fn part_style_spec(part: &RowPart, row: &SidebarRow) -> PartStyleSpec {
    let spec = |color, dim| PartStyleSpec {
        color: Some(color),
        dim,
    };
    // The selected row is one inverse highlight; a dimmed row is muted throughout, ▣ kept readable.
    if row.selected {
        return PartStyleSpec::default();
    }
    if row.dimmed {
        let readable = matches!(part.role, RowRole::Archived | RowRole::Done);
        return spec(INK_THEME.muted, !readable);
    }
    // Done: the title (and tree) muted, not dim; the status glyph keeps its color, so a signal still shows.
    if row.done && matches!(part.role, RowRole::Title | RowRole::Tree) {
        return spec(INK_THEME.muted, false);
    }
    match part.role {
        RowRole::Gutter => spec(INK_THEME.active, true),
        RowRole::Number | RowRole::Archived | RowRole::Merged | RowRole::Count | RowRole::Outdated => {
            spec(INK_THEME.muted, true)
        }
        RowRole::Done | RowRole::Agent | RowRole::DevShared | RowRole::Parent => spec(INK_THEME.muted, false),
        RowRole::Dev => spec(INK_THEME.success, false),
        RowRole::Cleanup => spec(INK_THEME.error, false),
        RowRole::Tree | RowRole::Status | RowRole::Title => spec(row.color, false),
        RowRole::Cursor | RowRole::Gap => PartStyleSpec::default(),
    }
}

/// TS: sidebar.tsx partStyle: a row part's style (the selected row is one inverse highlight; dimmed rows muted; not in
/// the TS: an unread reply's title bold).
pub fn part_style(part: &RowPart, row: &SidebarRow) -> TextStyle {
    let spec = part_style_spec(part, row);
    let mut style = TextStyle::default().color(spec.color.and_then(color));
    if spec.dim {
        style = style.dim(true);
    }
    if row.unread && part.role == RowRole::Title {
        style = style.bold(true);
    }
    style
}

/// TS: sidebar.tsx moreText: `↑ 12 more` / `↓ 3 more · ! 1` for the sessions out of view on one side.
pub fn more_text(arrow: &str, hidden: &[&SessionRecord]) -> String {
    if hidden.is_empty() {
        return String::new();
    }
    let waiting = hidden
        .iter()
        .filter(|session| session.archived_at.as_deref().is_none_or(str::is_empty) && session_needs_attention(session))
        .count();
    let tail = if waiting > 0 {
        format!(" · ! {waiting}")
    } else {
        String::new()
    };
    format!("{arrow} {} more{tail}", hidden.len())
}

/// The sidebar's rows split into its parts (TS: the budgeting in Sidebar's body). Rows are relative to the area
/// inside the border and the right padding, and carry the one-column left margin as a leading space where TS has
/// `marginLeft={1}`.
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
    /// The details block's lines, at the bottom.
    pub details: Vec<DetailLine>,
}

/// `<Box marginLeft={1}>` around a row.
fn with_margin(mut row: Row) -> Row {
    row.insert(0, Seg::plain(" "));
    row
}

fn text_row(text: String, color: Option<Color>, dim: bool) -> Row {
    let mut style = TextStyle::default().color(color);
    if dim {
        style = style.dim(true);
    }
    vec![Seg::new(text, style)]
}

/// TS: sidebar.tsx MoreLine: the `↑ N more` line, warn-colored when it counts sessions that need you.
fn more_line(arrow: &str, hidden: &[&SessionRecord], width: usize) -> Row {
    let text = more_text(arrow, hidden);
    let waiting = text.contains('!');
    let text = if text.is_empty() { " ".to_owned() } else { text };
    let tone = if waiting { THEME.warn } else { THEME.muted };
    with_margin(truncate_row(
        &text_row(text, Some(tone), false),
        width,
        Wrap::TruncateEnd,
    ))
}

/// TS: sidebar.tsx Sidebar's layout at `width` × `height`.
pub fn sidebar_layout(props: &SidebarProps<'_>, width: usize, height: usize) -> SidebarLayout {
    let sessions = props.sessions;
    let all_sessions = props.all_sessions;
    let selected_index = sessions
        .iter()
        .position(|session| Some(session.id.as_str()) == props.selected_id)
        .unwrap_or(0);
    let content_width = width.saturating_sub(4).max(1);
    // Rows start in the left padding column: it holds the cursor (›) and the shared-workspace marker.
    let row_width = content_width + 1;
    // The filter menu sits under the header, a rule below it, and takes its rows from the list.
    let menu_rows = if props.filter_menu.is_some() {
        SESSION_FILTERS.len() + 1
    } else {
        0
    };
    let rows_for_sessions = height.saturating_sub(3 + menu_rows).max(1);
    // A list longer than the sidebar keeps rows for the selected session's details (and scrolls in the rest);
    // a short one leaves them whatever it does not use.
    let selected = sessions
        .iter()
        .find(|session| Some(session.id.as_str()) == props.selected_id)
        .copied();
    let task_for = |session: &SessionRecord| props.task_of.and_then(|task_of| task_of(session));
    let task = selected.and_then(task_for);
    // As many rows as the largest details block of any listed session needs (at most DETAIL_ROWS), so the list keeps
    // its size while the selection moves; sessions with less leave the rest blank.
    let parents = if props.sectioned {
        section_parents(sessions, all_sessions, props.filter)
    } else {
        Vec::new()
    };
    let lines = list_lines(sessions, props.sectioned, props.folded_exited, &parents);
    let detail_rows = if lines.len() + DETAIL_ROWS > rows_for_sessions && rows_for_sessions >= DETAIL_MIN_SIDEBAR_ROWS {
        sessions
            .iter()
            .map(|session| {
                let task = task_for(session);
                session_details_with_parent(
                    Some(session),
                    all_sessions,
                    content_width,
                    DETAIL_ROWS,
                    props.now,
                    props.installed_versions,
                    task.as_ref(),
                )
                .len()
            })
            .max()
            .unwrap_or(0)
    } else {
        0
    };
    // A list that scrolls gets a line above and below it saying how many sessions are out of view (and how many of
    // those need you), each only while some are: at the top of the list the rows take the line above's place, at the
    // bottom the line below's, so the list and the "more" lines always fill the same rows.
    let free_rows = rows_for_sessions.saturating_sub(detail_rows);
    let scrolls = lines.len() > free_rows && free_rows >= 5;
    let selected_line = lines
        .iter()
        .position(|line| *line == ListLine::Session(selected_index))
        .unwrap_or(0);
    let range = if scrolls {
        let middle = visible_range(lines.len(), selected_line, free_rows - 2);
        if middle.start == 0 {
            0..free_rows - 1
        } else if middle.end == lines.len() {
            lines.len() - (free_rows - 1)..lines.len()
        } else {
            middle
        }
    } else {
        visible_range(lines.len(), selected_line, free_rows)
    };
    let more_above = range.start > 0;
    let more_below = range.end < lines.len();
    let more_rows = usize::from(more_above) + usize::from(more_below);
    let shown = window_lines(&lines, &parents, range);
    // The sessions shown are one run of the list; the headers between them are not sessions.
    let shown_sessions = || {
        shown.iter().filter_map(|line| match line {
            ListLine::Session(index) => Some(*index),
            ListLine::Header(..) | ListLine::Parent(..) => None,
        })
    };
    let visible_start = shown_sessions().next().unwrap_or(0);
    let visible_end = shown_sessions().next_back().map_or(visible_start, |index| index + 1);
    let visible = &sessions[visible_start..visible_end];
    let (empty_title, empty_hint) = empty_message(all_sessions.len(), props.loaded);
    let header = sidebar_header(&SidebarHeaderInput {
        width: content_width,
        filter: props.filter,
        query: props.query,
        shown: sessions.len(),
        total: all_sessions.len(),
        all_sessions,
    });
    let no_ids = HashSet::new();
    let number_width = sessions.len().max(1).to_string().len();
    let session_rows = sidebar_rows(&SidebarRowsInput {
        rows: visible,
        all_sessions,
        first_number: visible_start + 1,
        number_width,
        selected_id: props.selected_id,
        width: row_width,
        spinner_frame: props.spinner_frame,
        filter: props.filter,
        collapsed_session_ids: Some(props.collapsed_session_ids.unwrap_or(&no_ids)),
        hidden_session_ids: Some(props.hidden_session_ids.unwrap_or(&no_ids)),
        installed_versions: props.installed_versions,
        shared_dev: true,
        sectioned: props.sectioned,
        parent_names: true,
    });
    // The details take the rows the list leaves free (the ones kept for them when it scrolls).
    let details = if sessions.is_empty() {
        Vec::new()
    } else {
        session_details_with_parent(
            selected,
            all_sessions,
            content_width,
            rows_for_sessions.saturating_sub(shown.len() + more_rows),
            props.now,
            props.installed_versions,
            task.as_ref(),
        )
    };

    let header_left = vec![Seg::new(header.title.clone(), TextStyle::fg(THEME.accent).bold(true))];
    let mut header_right = vec![Seg::new(
        header.label.clone(),
        TextStyle::fg(if header.highlighted { THEME.active } else { THEME.muted }),
    )];
    for (count, colour) in [(&header.attention, THEME.warn), (&header.unread, THEME.accent)] {
        if !count.is_empty() {
            header_right.push(Seg::new(" · ", TextStyle::fg(THEME.muted)));
            header_right.push(Seg::new(count.clone(), TextStyle::fg(colour).bold(true)));
        }
    }
    // While keys go to the selected session's agent or shell, everything but its row is dimmed.
    let dim_if = |row: Row| if props.typing { dimmed(row) } else { row };
    let header_row = dim_if(with_margin(space_between(&header_left, &header_right, content_width)));

    let mut filter_menu = Vec::new();
    if let Some(menu) = props.filter_menu {
        for line in filter_menu_lines(menu.selected, &menu.counts, row_width) {
            let base = TextStyle {
                color: if line.selected { Some(THEME.active) } else { None },
                inverse: Some(line.selected),
                bold: Some(line.selected),
                ..TextStyle::default()
            };
            let key_color = if line.selected { None } else { Some(THEME.accent) };
            let count_color = if line.selected { None } else { Some(THEME.muted) };
            let row = vec![
                Seg::plain(format!("{} ", line.cursor)),
                Seg::new(line.key.clone(), TextStyle::default().color(key_color)),
                Seg::plain(format!("  {}", line.label)),
                Seg::new(line.count.clone(), TextStyle::default().color(count_color)),
            ];
            filter_menu.push(truncate_row(&with_base(row, base), row_width, Wrap::TruncateEnd));
        }
        filter_menu.push(with_margin(text_row(
            "─".repeat(content_width),
            Some(THEME.muted),
            true,
        )));
    }

    let rows: Vec<Row> = if sessions.is_empty() {
        vec![
            Vec::new(),
            with_margin(text_row(truncate(empty_title, content_width), Some(THEME.muted), false)),
            with_margin(text_row(
                truncate(if empty_hint.is_empty() { " " } else { empty_hint }, content_width),
                Some(THEME.active),
                false,
            )),
        ]
    } else {
        let mut session_rows = session_rows.iter();
        shown
            .iter()
            .filter_map(|line| match line {
                ListLine::Header(section, count) => Some(dim_if(section_header_row(
                    *section,
                    *count,
                    props.folded_exited.is_some(),
                    row_width,
                ))),
                ListLine::Parent(index) => parents
                    .get(*index)
                    .copied()
                    .flatten()
                    .map(|parent| dim_if(parent_line_row(parent, all_sessions, number_width, row_width))),
                ListLine::Session(_) => session_rows.next().map(|row| {
                    let base = TextStyle {
                        color: if row.selected { Some(THEME.active) } else { None },
                        inverse: Some(row.selected),
                        bold: Some(row.selected),
                        ..TextStyle::default()
                    };
                    let segs = row
                        .parts
                        .iter()
                        .map(|part| Seg::new(part.text.clone(), part_style(part, row)))
                        .collect();
                    let row_segs = truncate_row(&with_base(segs, base), row_width, Wrap::TruncateEnd);
                    if row.selected { row_segs } else { dim_if(row_segs) }
                }),
            })
            .collect()
    };
    let above =
        (!sessions.is_empty() && more_above).then(|| dim_if(more_line("↑", &sessions[..visible_start], content_width)));
    let below =
        (!sessions.is_empty() && more_below).then(|| dim_if(more_line("↓", &sessions[visible_end..], content_width)));
    SidebarLayout {
        header: header_row,
        filter_menu,
        above,
        rows,
        below,
        details,
    }
}

/// The details block's lines as rows (`marginLeft={1}`, each `<Text wrap="truncate-end">` of colored parts).
fn detail_row_list(details: &[DetailLine], width: usize) -> Vec<Row> {
    details
        .iter()
        .map(|line| {
            let segs: Row = line
                .iter()
                .map(|part| {
                    let mut style = TextStyle::default().color(part.color.and_then(color));
                    if part.dim == Some(true) {
                        style = style.dim(true);
                    }
                    Seg::new(part.text.clone(), style)
                })
                .collect();
            with_margin(truncate_row(&segs, width, Wrap::TruncateEnd))
        })
        .collect()
}

/// TS: sidebar.tsx Sidebar.
pub fn render_sidebar(props: &SidebarProps<'_>, buf: &mut Buffer, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    // A round border and one column of padding on the right.
    let inner = bordered(buf, area, &Border::round(THEME.border), 0);
    let inner = inset(inner, 0, 1, 0, 0);
    let width = usize::from(area.width);
    let layout = sidebar_layout(props, width, usize::from(area.height));
    let content_width = width.saturating_sub(4).max(1);
    let mut column = Column::new();
    column.push(layout.header);
    column.extend(layout.filter_menu);
    column.extend(layout.above);
    column.extend(layout.rows);
    column.extend(layout.below);
    if !layout.details.is_empty() {
        column.grow();
        let details = detail_row_list(&layout.details, content_width);
        column.extend(
            details
                .into_iter()
                .map(|row| if props.typing { dimmed(row) } else { row }),
        );
    }
    column.draw(buf, inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::string_width;
    use deckhand_core::jsstr::date;
    use deckhand_core::session_order::sort_sessions_for_sidebar;
    use deckhand_core::sidebar_model::{filter_menu_lines, session_details};
    use deckhand_testkit::{Case, replay, tags};
    use serde_json::{Value, json};

    fn now() -> i64 {
        date::parse("2026-10-05T12:00:00.000Z").unwrap_or_default()
    }

    fn ago(minutes: i64) -> String {
        date::to_iso_string(now() - minutes * 60_000).unwrap_or_default()
    }

    /// tests/sidebarModel.test.ts `session()`: a running claude session with the given fields (null removes one).
    fn session(id: &str, order: usize, fields: Value) -> SessionRecord {
        let mut base = json!({
            "id": id, "title": id, "program": "claude", "command": "claude", "cwd": "/repo", "repoRoot": "/repo",
            "launchWorktreeRoot": "/repo", "worktree": {"mode": "none"}, "status": "running", "agentStatus": "idle",
            "agentStatusUpdatedAt": ago(5), "createdAt": ago(300), "updatedAt": ago(5), "sidebarOrder": order,
        });
        if let (Some(base), Some(fields)) = (base.as_object_mut(), fields.as_object()) {
            for (key, value) in fields {
                if value.is_null() {
                    base.remove(key);
                } else {
                    base.insert(key.clone(), value.clone());
                }
            }
        }
        serde_json::from_value(base).expect("a valid session record")
    }

    /// `{...base, ...extra}`.
    fn merged(base: Value, extra: &Value) -> Value {
        let mut base = base;
        if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                base.insert(key.clone(), value.clone());
            }
        }
        base
    }

    /// The seven-session fixture of tests/sidebarModel.test.ts, in sidebar order.
    fn fixture() -> Vec<SessionRecord> {
        let auth = json!({"mode": "managed", "id": "w1", "path": "/wt/auth", "branch": "feat/auth"});
        let in_auth =
            json!({"cwd": "/wt/auth", "launchWorktreeRoot": "/wt/auth", "worktree": {"mode": "none", "id": "w1"}});
        let list = vec![
            session(
                "a",
                0,
                json!({"title": "auth refactor", "cwd": "/wt/auth", "worktree": auth, "agentStatus": "active",
                    "attention": {"state": "working", "event": "PreToolUse", "at": ago(1)}, "devRunning": true}),
            ),
            session(
                "b",
                1,
                merged(
                    json!({"title": "auth refactor / write tests", "program": "pi", "parentSessionId": "a",
                        "subSessionKind": "clean", "devRunning": true, "agentStatusUpdatedAt": ago(7)}),
                    &in_auth,
                ),
            ),
            session(
                "c",
                2,
                merged(
                    json!({"title": "auth refactor / try alt approach", "parentSessionId": "a",
                        "subSessionKind": "forked", "devRunning": true, "status": "exited",
                        "exitReason": "completed", "agentStatusUpdatedAt": ago(42)}),
                    &in_auth,
                ),
            ),
            session(
                "d",
                3,
                json!({"title": "fix flaky checkout e2e test on CI", "program": "codex",
                    "attention": {"state": "needs-input", "event": "PermissionRequest", "at": ago(12)}}),
            ),
            session(
                "e",
                4,
                json!({"title": "migrate billing webhooks to v2", "cwd": "/wt/billing",
                    "worktree": {"mode": "managed", "id": "w2", "path": "/wt/billing", "branch": "feat/billing"},
                    "status": "exited", "exitReason": "failed", "agentStatusUpdatedAt": ago(190)}),
            ),
            session(
                "f",
                5,
                json!({"title": "docs cleanup", "cwd": "/wt/docs",
                    "worktree": {"mode": "managed", "id": "w3", "path": "/wt/docs", "branch": "docs/cleanup",
                        "mergedAt": ago(3000)},
                    "status": "exited", "exitReason": "completed", "archivedAt": ago(2900),
                    "agentStatusUpdatedAt": ago(3100)}),
            ),
            session(
                "g",
                6,
                json!({"title": "bump deps", "program": "pi", "status": "starting", "agentStatus": null,
                    "requestedWorktreeMode": "new", "worktree": null}),
            ),
        ];
        sort_sessions_for_sidebar(&list).into_iter().cloned().collect()
    }

    fn many(count: usize) -> Vec<SessionRecord> {
        (0..count)
            .map(|index| {
                session(
                    &format!("s{index}"),
                    index,
                    json!({"title": format!("session number {index}")}),
                )
            })
            .collect()
    }

    fn refs(sessions: &[SessionRecord]) -> Vec<&SessionRecord> {
        sessions.iter().collect()
    }

    fn props<'a>(sessions: &'a [&'a SessionRecord], selected_id: &'a str, filter: SessionFilter) -> SidebarProps<'a> {
        SidebarProps {
            sessions,
            all_sessions: sessions,
            selected_id: Some(selected_id),
            spinner_frame: "⠋",
            collapsed_session_ids: None,
            hidden_session_ids: None,
            loaded: true,
            filter,
            query: "",
            now: now(),
            installed_versions: None,
            task_of: None,
            filter_menu: None,
            sectioned: false,
            typing: false,
            folded_exited: None,
        }
    }

    /// The buffer as lines of text (wide characters take their continuation cells with them).
    fn buffer_lines(buf: &Buffer) -> Vec<String> {
        let area = buf.area;
        (area.top()..area.bottom())
            .map(|y| {
                let mut line = String::new();
                let mut x = area.left();
                while x < area.right() {
                    let symbol = buf[(x, y)].symbol();
                    line.push_str(symbol);
                    x += u16::try_from(string_width(symbol).max(1)).unwrap_or(1);
                }
                line
            })
            .collect()
    }

    fn render(props: &SidebarProps<'_>, width: u16, height: u16) -> Vec<String> {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        render_sidebar(props, &mut buf, area);
        buffer_lines(&buf)
    }

    /// `/^│.\s*\d+ /`: a numbered session row.
    fn is_list_row(line: &str) -> bool {
        let mut chars = line.chars();
        if chars.next() != Some('│') || chars.next().is_none() {
            return false;
        }
        let rest = chars.as_str().trim_start();
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        digits > 0 && rest[digits..].starts_with(' ')
    }

    fn role_of(name: &str) -> Result<RowRole, String> {
        Ok(match name {
            "cursor" => RowRole::Cursor,
            "gutter" => RowRole::Gutter,
            "number" => RowRole::Number,
            "tree" => RowRole::Tree,
            "status" => RowRole::Status,
            "title" => RowRole::Title,
            "gap" => RowRole::Gap,
            "dev" => RowRole::Dev,
            "archived" => RowRole::Archived,
            "cleanup" => RowRole::Cleanup,
            "merged" => RowRole::Merged,
            "done" => RowRole::Done,
            "count" => RowRole::Count,
            "outdated" => RowRole::Outdated,
            "agent" => RowRole::Agent,
            other => return Err(format!("unknown role {other}")),
        })
    }

    #[test]
    fn part_style_parity() {
        replay("sidebar/partStyle", |case: &Case| -> Result<Value, String> {
            let part = tags::untag(case.arg(0));
            let row = tags::untag(case.arg(1));
            let text = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
            let flag = |key: &str| row.get(key).and_then(Value::as_bool).unwrap_or(false);
            let part = RowPart {
                text: text(&part, "text").ok_or("part text")?,
                role: role_of(&text(&part, "role").ok_or("part role")?)?,
            };
            // The row's color is any Ink color name (fixtures mutate it), so it is leaked to the static the row holds.
            let color: &'static str = Box::leak(text(&row, "color").ok_or("row color")?.into_boxed_str());
            let row = SidebarRow {
                id: text(&row, "id").ok_or("row id")?,
                parts: Vec::new(),
                selected: flag("selected"),
                dimmed: flag("dimmed"),
                done: flag("done"),
                unread: flag("unread"),
                color,
            };
            let spec = part_style_spec(&part, &row);
            let mut style = serde_json::Map::new();
            if let Some(color) = spec.color {
                style.insert("color".to_owned(), json!(color));
            }
            if spec.dim {
                style.insert("dimColor".to_owned(), json!(true));
            }
            Ok(Value::Object(style))
        });
    }

    #[test]
    fn more_text_parity() {
        replay("sidebar/moreText", |case: &Case| -> Result<String, String> {
            // Fixtures mutate `program` and `exitReason` to unknown strings, which a Rust record cannot hold; moreText
            // reads neither (an unknown exitReason is no different from none to sessionNeedsAttention).
            let mut sessions = tags::untag(case.arg(1));
            for entry in sessions
                .as_array_mut()
                .into_iter()
                .flatten()
                .filter_map(Value::as_object_mut)
            {
                let known = |key: &str, names: &[&str], entry: &serde_json::Map<String, Value>| {
                    entry
                        .get(key)
                        .and_then(Value::as_str)
                        .is_some_and(|name| names.contains(&name))
                };
                if !known("program", &["claude", "pi", "codex"], entry) {
                    entry.insert("program".to_owned(), json!("claude"));
                }
                if !known("exitReason", &["stopped", "failed", "completed", "interrupted"], entry) {
                    entry.remove("exitReason");
                }
            }
            let hidden: Vec<SessionRecord> = serde_json::from_value(sessions).map_err(|error| error.to_string())?;
            Ok(more_text(case.arg_str(0)?, &refs(&hidden)))
        });
    }

    #[test]
    fn part_style_maps_ink_names_to_ratatui_colors() {
        let part = RowPart {
            text: "x".into(),
            role: RowRole::Number,
        };
        let row = SidebarRow {
            id: "a".into(),
            parts: Vec::new(),
            selected: false,
            dimmed: false,
            done: false,
            unread: false,
            color: "red",
        };
        let style = part_style(&part, &row);
        assert_eq!(style.color, Some(THEME.muted));
        assert_eq!(style.dim, Some(true));
        let title = RowPart {
            text: "x".into(),
            role: RowRole::Title,
        };
        assert_eq!(part_style(&title, &row).color, Some(Color::Red));
        let selected = SidebarRow { selected: true, ..row };
        assert_eq!(part_style(&title, &selected), TextStyle::default());
    }

    #[test]
    fn windows_the_list_around_the_selection() {
        assert_eq!(visible_range(5, 2, 10), 0..5);
        assert_eq!(visible_range(40, 0, 15), 0..15);
        assert_eq!(visible_range(40, 20, 15), 13..28);
        assert_eq!(visible_range(40, 39, 15), 25..40);
        assert_eq!(visible_range(40, 3, 0), 0..40);
    }

    #[test]
    fn empty_lists_explain_themselves() {
        assert_eq!(empty_message(0, false), ("Loading sessions…", ""));
        assert_eq!(empty_message(0, true), ("No sessions yet.", "Press n to create."));
        assert_eq!(
            empty_message(3, true),
            ("No sessions match this view.", "f filter · / search")
        );
        let none: Vec<&SessionRecord> = Vec::new();
        let lines = render(&props(&none, "x", SessionFilter::Active), 34, 8);
        assert_eq!(lines[0], "╭────────────────────────────────╮");
        assert_eq!(lines[1], "│ Sessions                     0 │");
        assert_eq!(lines[3], "│ No sessions yet.               │");
        assert_eq!(lines[4], "│ Press n to create.             │");
    }

    #[test]
    fn sidebar_renders_the_cursor_in_the_padding_column_and_pins_the_details_to_the_bottom() {
        let all = fixture();
        let sessions = refs(&all);
        let lines = render(&props(&sessions, "d", SessionFilter::All), 34, 16);
        assert_eq!(
            lines,
            [
                "╭────────────────────────────────╮",
                "│ Sessions         all 7/7 · ! 2 │",
                "│  1 ▾ ⠋ auth refactor       ▶ ✶ │",
                // Not in the TS: its sub-sessions share the worktree's running Dev (a muted ▶).
                "│  2   ↳ ● write tests       ▶ π │",
                "│  3   ⑂ ○ try alt approach  ▶ ✶ │",
                "│› 4 ? fix flaky checkout e2e… ◇ │",
                "│  5 ! migrate billing webhoo… ✶ │",
                "│  6 ○ docs cleanup        ▣ ✓ ✶ │",
                "│  7 ⠋ bump deps               π │",
                "│                                │",
                "│ ────────────────────────────── │",
                "│ fix flaky checkout e2e test on │",
                "│ CI                             │",
                "│ ◇ codex · needs input · 12m    │",
                "│ main checkout                  │",
                "╰────────────────────────────────╯",
            ]
        );
    }

    #[test]
    fn the_state_view_heads_each_section_and_nests_only_within_one() {
        let all = fixture();
        let everyone = refs(&all);
        let ordered: Vec<&SessionRecord> = deckhand_core::session_sections::order_by_section(&everyone)
            .into_iter()
            .copied()
            .collect();
        let mut props = props(&ordered, "d", SessionFilter::All);
        props.all_sessions = &everyone;
        props.sectioned = true;
        let lines = render(&props, 34, 26);
        assert_eq!(
            lines[..17],
            [
                "╭────────────────────────────────╮",
                "│ Sessions         all 7/7 · ! 2 │",
                "│  Needs you 1 ───────────────── │",
                "│› 1 ? fix flaky checkout e2e… ◇ │",
                "│  Working 2 ─────────────────── │",
                // a's sub-sessions are in other sections: no ▾ on a, and in each section a line naming it, its
                // sub-session one level in under it.
                "│  2 ⠋ auth refactor         ▶ ✶ │",
                "│  3 ⠋ bump deps               π │",
                "│  Idle 1 ────────────────────── │",
                "│    auth refactor ┄┄┄┄┄┄┄┄┄┄┄┄┄ │",
                "│  4   ↳ ● write tests       ▶ π │",
                "│  Exited 3 ▾ ────────────────── │",
                "│    auth refactor ┄┄┄┄┄┄┄┄┄┄┄┄┄ │",
                "│  5   ⑂ ○ try alt approach  ▶ ✶ │",
                "│  6 ! migrate billing webhoo… ✶ │",
                "│  7 ○ docs cleanup        ▣ ✓ ✶ │",
                "│                                │",
                "│                                │",
            ]
        );
        // The selected sub-session's details name its parent and how it is doing.
        props.selected_id = Some("b");
        let details = render(&props, 34, 24);
        assert!(
            details
                .iter()
                .any(|line| line.contains("↳ sub-session of auth refactor")),
            "{details:#?}"
        );
        props.selected_id = Some("d");
        // Folded (c): the header keeps the whole count, last, with no exited row left to list.
        let running: Vec<&SessionRecord> = ordered
            .iter()
            .copied()
            .filter(|session| session.status != deckhand_protocol::data::SessionStatus::Exited)
            .collect();
        props.sessions = &running;
        props.folded_exited = Some(3);
        let folded = render(&props, 34, 18);
        assert_eq!(folded[10], "│  Exited 3 ▸ ────────────────── │", "{folded:#?}");
        assert!(!folded.iter().any(|line| line.contains("docs cleanup")));
    }

    #[test]
    fn a_scrolled_state_view_keeps_the_selected_sessions_header_on_top() {
        let list = many(40);
        let sessions = refs(&list);
        let mut props = props(&sessions, "s20", SessionFilter::All);
        props.sectioned = true;
        let lines = render(&props, 34, 24);
        let list_rows: Vec<&String> = lines.iter().filter(|line| is_list_row(line)).collect();
        // The window starts inside the Idle section: its header replaces the first session shown.
        assert!(lines[3].contains("Idle 40 ───"), "{lines:#?}");
        assert!(list_rows.iter().any(|line| line.contains("› 21 ● session number 20")));
        let has = |text: &str| lines.iter().any(|line| line.contains(text));
        // Of 15 rows the header takes one, so the session it replaced is counted above.
        assert!(lines[4].contains("15 ● session number 14"), "{lines:#?}");
        assert!(has("↑ 14 more"), "{lines:#?}");
        assert!(has("↓ 12 more"), "{lines:#?}");
    }

    #[test]
    fn a_scrolled_state_view_keeps_the_line_naming_the_parent_on_top() {
        let mut list = vec![session(
            "p",
            0,
            json!({"title": "big parent", "agentStatus": "active",
                "attention": {"state": "working", "event": "PreToolUse", "at": ago(1)}}),
        )];
        list.extend((0..30).map(|index| {
            session(
                &format!("k{index}"),
                index + 1,
                json!({"title": format!("child {index}"), "parentSessionId": "p", "subSessionKind": "clean"}),
            )
        }));
        let everyone = refs(&list);
        let ordered: Vec<&SessionRecord> = deckhand_core::session_sections::order_by_section(&everyone)
            .into_iter()
            .copied()
            .collect();
        let mut props = props(&ordered, "k15", SessionFilter::All);
        props.all_sessions = &everyone;
        props.sectioned = true;
        let lines = render(&props, 34, 24);
        // Under the "↑" line, the Idle header and the line naming the children's parent, then the children.
        assert!(lines[2].contains("↑ "), "{lines:#?}");
        assert!(lines[3].contains("Idle 30 ───"), "{lines:#?}");
        assert_eq!(lines[4], "│     big parent ┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄ │", "{lines:#?}");
        assert!(lines[5].contains("↳ ● child"), "{lines:#?}");
        assert!(
            lines.iter().any(|line| line.contains("› 17   ↳ ● child 15")),
            "{lines:#?}"
        );
    }

    #[test]
    fn sidebar_with_more_sessions_than_fit_keeps_rows_for_the_details_and_scrolls_the_list() {
        let list = many(40);
        let sessions = refs(&list);
        let lines = render(&props(&sessions, "s20", SessionFilter::All), 34, 24);
        let list_rows: Vec<&String> = lines.iter().filter(|line| is_list_row(line)).collect();
        // 24 rows: border, header and border leave 21; the details keep the four they use, the two "more" lines two.
        assert_eq!(list_rows.len(), 15, "{lines:#?}");
        assert!(
            list_rows.iter().any(|line| line.contains("› 21 ● session number 20")),
            "the selection stays in view"
        );
        let has = |text: &str| lines.iter().any(|line| line.contains(text));
        assert!(has("↑ 13 more"), "sessions above are counted");
        assert!(has("↓ 12 more"), "sessions below are counted");
        assert!(has("──────"), "the details rule is shown");
        assert!(has("✶ claude · idle"), "the details state line is shown");
        // A short list still leaves the details only what it does not use.
        let few = refs(&list[..3]);
        let short = render(&props(&few, "s1", SessionFilter::All), 34, 24);
        assert_eq!(short.iter().filter(|line| is_list_row(line)).count(), 3);
        assert!(
            !short.iter().any(|line| line.contains(" more")),
            "a list that fits has no \"more\" lines"
        );
        // Hidden sessions that need you are counted on their side.
        let waiting = session(
            "w",
            99,
            json!({"attention": {"state": "needs-input", "event": "PermissionRequest", "at": ago(1)}}),
        );
        assert_eq!(more_text("↓", &[&list[0], &waiting]), "↓ 2 more · ! 1");
        assert_eq!(more_text("↑", &[]), "");
    }

    #[test]
    fn a_scrolling_list_keeps_the_same_rows_whichever_session_is_selected() {
        let list = many(40);
        let sessions = refs(&list);
        let task_of = |entry: &SessionRecord| {
            (entry.id == "s21").then(|| DetailTask {
                title: "Fix the footer".to_owned(),
                done: false,
                more: None,
            })
        };
        let render_with = |selected: &str| {
            let mut props = props(&sessions, selected, SessionFilter::All);
            props.task_of = Some(&task_of);
            render(&props, 34, 24)
        };
        let with_task = render_with("s21");
        let without = render_with("s20");
        let shape = |lines: &[String]| {
            (
                lines.iter().filter(|line| is_list_row(line)).count(),
                lines.iter().position(|line| line.contains("↓ ")),
            )
        };
        assert_eq!(shape(&without), shape(&with_task));
        // The details sit at the bottom, as under a list that fits: the session with a task shows it, the other
        // leaves a blank row between the "more" line and its rule.
        let last_detail = |lines: &[String]| lines[lines.len() - 2].clone();
        assert!(last_detail(&with_task).contains("◆ Fix the footer"), "{with_task:#?}");
        assert!(last_detail(&without).contains("main checkout"), "{without:#?}");
        assert!(!without.iter().any(|line| line.contains('◆')));
        let rule = |lines: &[String]| lines.iter().position(|line| line.starts_with("│ ──────"));
        assert_eq!(rule(&without), rule(&with_task).map(|rule| rule + 1));
    }

    #[test]
    fn a_scrolling_list_shows_a_more_line_only_on_a_side_with_sessions_out_of_view() {
        let list = many(40);
        let sessions = refs(&list);
        let lines_for = |selected: &str| render(&props(&sessions, selected, SessionFilter::All), 34, 24);
        let rows = |lines: &[String]| lines.iter().filter(|line| is_list_row(line)).count();
        // At the top no blank line stands in for "↑": the first session sits under the header, and the row the
        // line would take lists one more session.
        let top = lines_for("s0");
        assert!(top[2].contains(" 1 ● session number 0"), "{top:#?}");
        assert!(!top.iter().any(|line| line.contains('↑')), "{top:#?}");
        assert!(top.iter().any(|line| line.contains("↓ ")), "{top:#?}");
        // At the bottom, likewise for "↓".
        let bottom = lines_for("s39");
        assert!(bottom.iter().any(|line| line.contains("↑ ")), "{bottom:#?}");
        assert!(!bottom.iter().any(|line| line.contains('↓')), "{bottom:#?}");
        // In between both, and the list and its "more" lines always fill the same rows.
        let middle = lines_for("s20");
        assert_eq!(rows(&top), rows(&middle) + 1);
        assert_eq!(rows(&bottom), rows(&middle) + 1);
    }

    #[test]
    fn sidebar_with_the_filter_menu_open_lists_every_filter_with_its_key_and_count() {
        let list = many(3);
        let sessions = refs(&list);
        let counts = SessionFilterCounts {
            active: 3,
            running: 3,
            attention: 0,
            exited: 0,
            archived: 0,
            all: 12,
        };
        let mut props = props(&sessions, "s1", SessionFilter::Running);
        props.filter_menu = Some(FilterMenu {
            selected: SessionFilter::Running,
            counts,
        });
        let lines = render(&props, 34, 24);
        assert_eq!(
            lines[2..9],
            [
                "│  a  active                   3 │",
                "│› r  running                  3 │",
                "│  !  attention                0 │",
                "│  e  exited                   0 │",
                "│  A  archived                 0 │",
                "│  *  all                     12 │",
                "│ ────────────────────────────── │",
            ]
        );
        assert!(lines[9].contains("1 ● session number 0"), "{}", lines[9]);
        // Narrow: the label is cut before the count.
        let narrow: Vec<String> = filter_menu_lines(SessionFilter::Active, &counts, 9)
            .iter()
            .map(|line| format!("{} {}  {}{}", line.cursor, line.key, line.label, line.count))
            .collect();
        assert_eq!(narrow[4..], ["  A  a… 0", "  *  a 12"]);
    }

    #[test]
    fn sidebar_marks_outdated_running_sessions_and_the_details_name_both_versions() {
        // tests/agentVersions.test.ts builds its sessions 3 minutes old.
        let aged = |id: &str, order: usize, fields: Value| {
            session(
                id,
                order,
                merged(json!({"agentStatusUpdatedAt": ago(3), "updatedAt": ago(3)}), &fields),
            )
        };
        let all = vec![
            aged("old", 0, json!({"title": "old build", "agentVersion": "2.1.287"})),
            aged("new", 1, json!({"title": "new build", "agentVersion": "2.1.290"})),
            aged(
                "gone",
                2,
                json!({"title": "exited", "agentVersion": "2.1.287", "status": "exited", "exitReason": "completed"}),
            ),
            aged(
                "pi",
                3,
                json!({"title": "pi one", "program": "pi", "agentVersion": "1.0.2"}),
            ),
        ];
        let sessions = refs(&all);
        let installed: InstalledVersions = serde_json::from_value(json!({"claude": "2.1.290"})).expect("versions");
        let rows_of = |rows: &[&SessionRecord],
                       everyone: &[&SessionRecord],
                       width: usize,
                       collapsed: Option<&HashSet<String>>,
                       versions: Option<&InstalledVersions>| {
            sidebar_rows(&SidebarRowsInput {
                rows,
                all_sessions: everyone,
                first_number: 1,
                number_width: 1,
                selected_id: None,
                width,
                spinner_frame: "⠋",
                filter: SessionFilter::All,
                collapsed_session_ids: collapsed,
                hidden_session_ids: None,
                installed_versions: versions,
                shared_dev: false,
                sectioned: false,
                parent_names: false,
            })
        };
        let texts = |rows: Vec<SidebarRow>| -> Vec<String> {
            rows.iter()
                .map(|row| row.parts.iter().map(|part| part.text.as_str()).collect())
                .collect()
        };
        assert_eq!(
            texts(rows_of(&sessions, &sessions, 31, None, Some(&installed))),
            [
                "  1 ● old build             ↑ ✶",
                "  2 ● new build               ✶",
                "  3 ○ exited                  ✶",
                // No installed version known for pi: never marked.
                "  4 ● pi one                  π",
            ]
        );
        let marked = rows_of(&sessions[..1], &sessions, 31, None, Some(&installed));
        let roles: Vec<RowRole> = marked[0]
            .parts
            .iter()
            .filter(|part| !part.text.trim().is_empty())
            .map(|part| part.role)
            .collect();
        assert_eq!(roles[roles.len() - 2..], [RowRole::Outdated, RowRole::Agent]);
        // Short of room, ↑ goes before +N (and the rest); the agent glyph stays.
        let parent = aged("p", 4, json!({"title": "parent session", "agentVersion": "2.1.287"}));
        let child = aged("k", 5, json!({"parentSessionId": "p", "subSessionKind": "clean"}));
        let pair = [&parent, &child];
        let collapsed: HashSet<String> = ["p".to_owned()].into_iter().collect();
        let narrow_row =
            |width: usize| texts(rows_of(&pair[..1], &pair, width, Some(&collapsed), Some(&installed)))[0].clone();
        assert_eq!(narrow_row(30), "  1 ▸ ● parent session  +1 ↑ ✶");
        assert_eq!(narrow_row(17), "  1 ▸ ● par… +1 ✶");
        // Without versions nothing changes.
        assert_eq!(
            texts(rows_of(&sessions[..1], &sessions, 31, None, None))[0],
            "  1 ● old build               ✶"
        );

        let details = |id: &str, width: usize| -> Vec<String> {
            let found = all.iter().find(|item| item.id == id);
            session_details(found, &sessions, width, 10, now(), Some(&installed), None)
                .iter()
                .map(|line| line.iter().map(|part| part.text.as_str()).collect())
                .collect()
        };
        let state = |id: &str, width: usize| details(id, width).get(2).cloned().unwrap_or_default();
        assert_eq!(state("old", 48), "✶ claude 2.1.287 · 2.1.290 installed · idle · 3m");
        assert_eq!(state("old", 40), "✶ claude 2.1.287 → 2.1.290 · idle · 3m");
        assert_eq!(state("old", 31), "✶ 2.1.287 → 2.1.290 · idle · 3m");
        assert_eq!(state("old", 30), "✶ 2.1.287 → 2.1.290 · idle");
        // Too narrow for versions: the plain line.
        assert_eq!(state("old", 20), "✶ claude · idle · 3m");
        assert_eq!(state("old", 18), "✶ idle · 3m");
        assert_eq!(state("new", 48), "✶ claude · idle · 3m");
        assert_eq!(state("gone", 48), "✶ claude · exited · 3m");

        // Rendered at the default sidebar width (34), the outdated session selected.
        let mut props = props(&sessions, "old", SessionFilter::All);
        props.installed_versions = Some(&installed);
        assert_eq!(
            render(&props, 34, 11),
            [
                "╭────────────────────────────────╮",
                "│ Sessions               all 4/4 │",
                "│› 1 ● old build             ↑ ✶ │",
                "│╎ 2 ● new build               ✶ │",
                "│╎ 3 ○ exited                  ✶ │",
                "│╎ 4 ● pi one                  π │",
                "│ ────────────────────────────── │",
                "│ old build                      │",
                "│ ✶ 2.1.287 → 2.1.290 · idle     │",
                "│ main checkout ╎3               │",
                "╰────────────────────────────────╯",
            ]
        );
    }

    #[test]
    fn tiny_areas_draw_without_panicking() {
        let list = many(2);
        let sessions = refs(&list);
        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(area);
        render_sidebar(&props(&sessions, "s0", SessionFilter::All), &mut buf, area);
        assert_eq!(buf.area, area);
        for (width, height) in [(1, 1), (2, 2), (5, 3), (8, 40)] {
            let _ = render(&props(&sessions, "s0", SessionFilter::All), width, height);
        }
    }
}
