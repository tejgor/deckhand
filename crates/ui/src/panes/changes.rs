//! Port of changesPane.tsx: the Git tab's rendering. The Changes list of the session's workspace (browse: the list
//! alone, read-only; focus: a selectable list plus the selected entry's diff, side by side when wide enough, stacked
//! otherwise). State and keys live in flows/changes.rs; lazygit is only attached (o).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use deckhand_core::changes_model::{
    ChangeRow, DiffLineKind, LineCounts, change_group_title, change_label, change_rows, classify_diff,
    line_counts_text, total_changes,
};
use deckhand_core::display::{
    DISPLAY_CONTROL_PATTERN, DetailPart, SELECTION_MARKER, compact_path, fit_parts, menu_window_start, truncate,
};
use deckhand_core::jsstr::{js_len, js_trim};
use deckhand_core::workspace::workspace_pane_unavailable;
use deckhand_protocol::data::{ChangeDiff, ChangeEntry, ChangeGroup, ChangesRecord, SessionRecord};

use crate::primitives::{Row, Seg, TextStyle, Wrap, draw_rows, fit_row, space_between, truncate_row, with_base};
use crate::theme::THEME;

/// TS: changesPane.tsx SIDE_BY_SIDE_WIDTH: side by side from this many columns of pane width.
pub const SIDE_BY_SIDE_WIDTH: usize = 100;

/// TS: changesPane.tsx ChangesLayout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChangesLayout {
    pub side: bool,
    pub list_width: usize,
    pub list_rows: usize,
    pub diff_width: usize,
    pub diff_rows: usize,
}

/// TS: changesPane.tsx changesLayout: how focus mode splits the pane (after its header row) between the list and
/// the diff (whose title takes a row).
pub fn changes_layout(width: usize, height: usize, list_length: usize) -> ChangesLayout {
    let body = height.saturating_sub(1).max(2);
    if width >= SIDE_BY_SIDE_WIDTH {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let wanted = (width as f64 * 0.42).floor() as usize;
        let list_width = wanted.clamp(36, 64);
        return ChangesLayout {
            side: true,
            list_width,
            list_rows: body,
            diff_width: width.saturating_sub(list_width + 3).max(10),
            diff_rows: body.saturating_sub(1).max(1),
        };
    }
    // Stacked: the list takes what it needs up to 40% (at least 3 rows), a rule, then the diff.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let forty = (body as f64 * 0.4).floor() as usize;
    let list_rows = body.min(3).max(list_length.min(forty));
    ChangesLayout {
        side: false,
        list_width: width,
        list_rows,
        diff_width: width,
        diff_rows: body.saturating_sub(list_rows + 2).max(1),
    }
}

/// TS: changesPane.tsx STATUS_COLORS (by porcelain status letter).
pub fn status_color(status: &str) -> Option<Color> {
    match status {
        "M" | "T" => Some(THEME.warn),
        "A" | "?" => Some(THEME.success),
        "D" | "U" => Some(THEME.error),
        "R" | "C" => Some(THEME.active),
        _ => None,
    }
}

/// TS: changesPane.tsx DIFF_COLORS.
pub fn diff_color(kind: DiffLineKind) -> Option<Color> {
    match kind {
        DiffLineKind::Add => Some(THEME.success),
        DiffLineKind::Del => Some(THEME.error),
        DiffLineKind::Hunk => Some(THEME.active),
        DiffLineKind::Meta => Some(THEME.muted),
        DiffLineKind::Context => None,
    }
}

/// TS: changesPane.tsx visible: tabs as four spaces, controls as `?`.
pub fn visible(text: &str) -> String {
    DISPLAY_CONTROL_PATTERN
        .replace_all(&text.replace('\t', "    "), "?")
        .into_owned()
}

/// A part of a row with its (ratatui) color; `fit_parts` keeps the order, so colors travel by index.
struct Piece {
    part: DetailPart,
    color: Option<Color>,
}

fn piece(text: String, color: Option<Color>, bold: bool) -> Piece {
    Piece {
        part: DetailPart {
            text,
            bold: bold.then_some(true),
            ..DetailPart::default()
        },
        color,
    }
}

/// TS: changesPane.tsx EntryRow: status, name, directory and line counts of one entry.
pub fn entry_row(entry: &ChangeEntry, selected: bool, width: usize) -> Row {
    let inner = width.saturating_sub(2).max(1);
    let label = change_label(entry);
    let counts = line_counts_text(&LineCounts::of(entry));
    let tail: Vec<Piece> = if counts.is_empty() {
        Vec::new()
    } else if entry.binary == Some(true) {
        vec![piece(counts.clone(), Some(THEME.muted), false)]
    } else {
        vec![
            piece(format!("+{}", entry.additions.unwrap_or(0)), Some(THEME.success), false),
            piece(format!(" −{}", entry.deletions.unwrap_or(0)), Some(THEME.error), false),
        ]
    };
    let tail_length = js_len(&counts);
    let shown_tail = if tail_length + 2 <= inner / 2 { tail } else { Vec::new() };
    let room = inner.saturating_sub(if shown_tail.is_empty() { 0 } else { tail_length + 1 });
    let mut pieces = vec![
        piece(format!("{} ", entry.status), status_color(&entry.status), true),
        piece(visible(&label.name), None, false),
    ];
    if !label.dir.is_empty() {
        pieces.push(piece(format!("  {}", visible(&label.dir)), Some(THEME.muted), false));
    }
    let parts: Vec<DetailPart> = pieces.iter().map(|piece| piece.part.clone()).collect();
    let body = fit_parts(&parts, room);
    let used: usize = body.iter().map(|part| js_len(&part.text)).sum();
    let base = TextStyle {
        color: selected.then_some(THEME.active),
        bold: selected.then_some(true),
        inverse: selected.then_some(true),
        ..TextStyle::default()
    };
    let colored = |text: String, color: Option<Color>, bold: bool| {
        Seg::new(
            text,
            TextStyle {
                color: if selected { None } else { color },
                bold: bold.then_some(true),
                ..TextStyle::default()
            },
        )
    };
    let mut row: Row = vec![Seg::plain(format!(
        "{} ",
        if selected { SELECTION_MARKER } else { " " }
    ))];
    for (part, source) in body.into_iter().zip(&pieces) {
        row.push(colored(part.text, source.color, part.bold == Some(true)));
    }
    let padding = (room + usize::from(!shown_tail.is_empty())).saturating_sub(used);
    row.push(Seg::plain(" ".repeat(padding)));
    for piece in shown_tail {
        row.push(colored(piece.part.text, piece.color, false));
    }
    truncate_row(&with_base(row, base), width, Wrap::TruncateEnd)
}

/// TS: changesPane.tsx ListRow (`selected`: the selected entry's index, none in browse).
pub fn list_row(row: &ChangeRow, selected: Option<usize>, width: usize) -> Row {
    match row {
        ChangeRow::Entry { entry, index } => entry_row(entry, selected == Some(*index), width),
        ChangeRow::More { count, .. } => {
            let line = vec![Seg::new(
                truncate(&format!("    +{count} more"), width),
                TextStyle::fg(THEME.muted),
            )];
            truncate_row(&line, width, Wrap::TruncateEnd)
        }
        ChangeRow::Header { group, count } => {
            let title = change_group_title(*group);
            let color = if *group == ChangeGroup::Conflicts {
                THEME.error
            } else {
                THEME.accent_soft
            };
            let line = vec![
                Seg::new(
                    truncate(title, width.saturating_sub(6).max(1)),
                    TextStyle::fg(color).bold(true),
                ),
                Seg::new(format!(" {count}"), TextStyle::fg(THEME.muted)),
            ];
            truncate_row(&line, width, Wrap::TruncateEnd)
        }
    }
}

/// A row of the windowed list: a change row or a `↑/↓ N more` marker.
#[derive(Clone, Debug, PartialEq)]
pub enum ListWindowRow {
    Row(ChangeRow),
    Marker(String),
}

/// TS: changesPane.tsx listWindow: the rows shown in `height` lines: from the top (browse), or around the selection
/// with ↑/↓ more markers (focus).
pub fn list_window(rows: &[ChangeRow], selected: Option<usize>, height: usize, focused: bool) -> Vec<ListWindowRow> {
    let wrap = |slice: &[ChangeRow]| slice.iter().cloned().map(ListWindowRow::Row).collect::<Vec<_>>();
    if rows.len() <= height {
        return wrap(rows);
    }
    if !focused {
        let mut shown = wrap(&rows[..height.saturating_sub(1)]);
        shown.push(ListWindowRow::Marker(format!(
            "  ↓ {} more · v to browse",
            rows.len() - height + 1
        )));
        return shown;
    }
    let at = rows
        .iter()
        .position(|row| matches!(row, ChangeRow::Entry { index, .. } if Some(*index) == selected))
        .unwrap_or(0);
    if height < 3 {
        let start = menu_window_start(at, rows.len(), height);
        return wrap(&rows[start.min(rows.len())..(start + height).min(rows.len())]);
    }
    let visible_rows = height - 2;
    // Keep the selection's group header in view when it fits.
    let start = menu_window_start(at, rows.len(), visible_rows);
    let anchored = if at == start && start > 0 && matches!(rows.get(at - 1), Some(ChangeRow::Header { .. })) {
        start - 1
    } else {
        start
    };
    let shown = &rows[anchored.min(rows.len())..(anchored + visible_rows).min(rows.len())];
    let below = rows.len() - anchored - shown.len();
    let mut window = vec![ListWindowRow::Marker(if anchored > 0 {
        format!("  ↑ {anchored} more")
    } else {
        " ".to_owned()
    })];
    window.extend(wrap(shown));
    window.push(ListWindowRow::Marker(if below > 0 {
        format!("  ↓ {below} more")
    } else {
        " ".to_owned()
    }));
    window
}

/// TS: changesPane.tsx ChangesList's rows.
pub fn changes_list(
    rows: &[ChangeRow],
    selected: Option<usize>,
    width: usize,
    height: usize,
    focused: bool,
) -> Vec<Row> {
    let selected = if focused { selected } else { None };
    list_window(rows, selected, height, focused)
        .iter()
        .map(|row| match row {
            ListWindowRow::Marker(text) => {
                let line = vec![Seg::new(truncate(text, width), TextStyle::fg(THEME.muted))];
                truncate_row(&line, width, Wrap::TruncateEnd)
            }
            ListWindowRow::Row(row) => list_row(row, selected, width),
        })
        .collect()
}

/// TS: changesPane.tsx DiffState: the diff shown for the entry with `key`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiffState {
    pub key: String,
    pub diff: Option<ChangeDiff>,
    pub error: Option<String>,
}

/// TS: changesPane.tsx diffLines's kinds: a classified diff line, or a note.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiffRowKind {
    Line(DiffLineKind),
    Note,
}

/// TS: changesPane.tsx diffLines's entries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffRow {
    pub kind: DiffRowKind,
    pub text: String,
}

fn note(text: &str) -> DiffRow {
    DiffRow {
        kind: DiffRowKind::Note,
        text: text.to_owned(),
    }
}

/// TS: changesPane.tsx diffLines: the diff's lines for scrolling (classified, the cut notice included).
pub fn diff_lines(diff: Option<&ChangeDiff>) -> Vec<DiffRow> {
    let Some(diff) = diff else { return Vec::new() };
    if diff.binary {
        return vec![note("Binary file; no preview.")];
    }
    if js_trim(&diff.text).is_empty() {
        return vec![note("No textual changes (mode, directory or empty file).")];
    }
    let mut lines: Vec<DiffRow> = classify_diff(&diff.text)
        .into_iter()
        .map(|line| DiffRow {
            kind: DiffRowKind::Line(line.kind),
            text: line.text,
        })
        .collect();
    if diff.truncated {
        lines.push(note(
            "… preview cut here; open the file (enter) or lazygit (o) for the rest",
        ));
    }
    lines
}

/// TS: changesPane.tsx DiffView's rows (title with position, then the visible diff lines or a message).
pub fn diff_view(
    entry: Option<&ChangeEntry>,
    state: Option<&DiffState>,
    scroll: usize,
    width: usize,
    height: usize,
) -> Vec<Row> {
    let lines = state.map_or_else(Vec::new, |state| diff_lines(state.diff.as_ref()));
    let rows = height.saturating_sub(1).max(1);
    let start = scroll.min(lines.len().saturating_sub(rows));
    let position = if lines.len() > rows {
        format!("{}-{}/{}", start + 1, lines.len().min(start + rows), lines.len())
    } else {
        String::new()
    };
    let title = entry.map_or_else(
        || "No selection".to_owned(),
        |entry| {
            let group = match entry.group {
                ChangeGroup::Staged => "staged",
                ChangeGroup::Untracked => "untracked",
                ChangeGroup::Conflicts => "conflict",
                ChangeGroup::Unstaged => "unstaged",
            };
            format!("{group} · {}", visible(&entry.path))
        },
    );
    let error = state
        .and_then(|state| state.error.as_deref())
        .filter(|error| !error.is_empty());
    let message = if entry.is_none() {
        Some("Select a file to preview its diff.".to_owned())
    } else if let Some(error) = error {
        Some(format!("Diff unavailable: {error}"))
    } else if state.is_none_or(|state| state.diff.is_none()) {
        Some("Loading diff…".to_owned())
    } else {
        None
    };
    let position_len = js_len(&position);
    let head = truncate(
        &title,
        width
            .saturating_sub(if position.is_empty() { 0 } else { position_len + 2 })
            .max(1),
    );
    let mut title_row = vec![Seg::new(head.clone(), TextStyle::fg(THEME.muted).bold(true))];
    if !position.is_empty() {
        let gap = width.saturating_sub(js_len(&head) + position_len).max(2);
        title_row.push(Seg::new(
            format!("{}{position}", " ".repeat(gap)),
            TextStyle::fg(THEME.muted),
        ));
    }
    let mut out = vec![truncate_row(&title_row, width, Wrap::TruncateEnd)];
    if let Some(message) = message {
        let color = if error.is_some() { THEME.error } else { THEME.muted };
        let line = vec![Seg::new(truncate(&message, width), TextStyle::fg(color))];
        out.push(truncate_row(&line, width, Wrap::TruncateEnd));
        return out;
    }
    for line in lines.iter().skip(start).take(rows) {
        let (color, dim) = match line.kind {
            DiffRowKind::Note => (Some(THEME.muted), false),
            DiffRowKind::Line(kind) => (diff_color(kind), kind == DiffLineKind::Meta),
        };
        let mut text = truncate(&visible(&line.text), width);
        if text.is_empty() {
            text.push(' ');
        }
        let style = TextStyle {
            color,
            dim: dim.then_some(true),
            ..TextStyle::default()
        };
        out.push(truncate_row(&[Seg::new(text, style)], width, Wrap::TruncateEnd));
    }
    out
}

/// TS: changesPane.tsx summary: the header's counts and their color.
pub fn summary(changes: &ChangesRecord) -> (String, Color) {
    if changes.error.as_deref().is_some_and(|error| !error.is_empty()) {
        return ("○ error".to_owned(), THEME.error);
    }
    if !changes.loaded {
        return ("…".to_owned(), THEME.muted);
    }
    let counts = &changes.counts;
    if total_changes(counts) == 0 {
        return ("● clean".to_owned(), THEME.success);
    }
    let parts: Vec<String> = [
        (counts.conflicts, "conflicted"),
        (counts.staged, "staged"),
        (counts.unstaged, "changed"),
        (counts.untracked, "untracked"),
    ]
    .into_iter()
    .filter(|(count, _)| *count != 0)
    .map(|(count, label)| format!("{count} {label}"))
    .collect();
    (
        parts.join(" · "),
        if counts.conflicts != 0 {
            THEME.error
        } else {
            THEME.muted
        },
    )
}

/// TS: changesPane.tsx fallbackMessage: why no list is shown (and its color), if so.
pub fn fallback_message(session: Option<&SessionRecord>, changes: &ChangesRecord) -> Option<(String, Option<Color>)> {
    let Some(session) = session else {
        return Some(("No session selected.".to_owned(), None));
    };
    if let Some(unavailable) =
        workspace_pane_unavailable(session, changes.session_id.as_deref(), changes.workspace.as_deref())
    {
        return Some((format!("Git is unavailable: {unavailable}."), None));
    }
    if changes.session_id.as_deref() != Some(session.id.as_str()) || !changes.loaded {
        return Some(("Reading changes…".to_owned(), None));
    }
    if let Some(error) = changes.error.as_deref().filter(|error| !error.is_empty()) {
        return Some((format!("Git status failed: {error}"), Some(THEME.error)));
    }
    if total_changes(&changes.counts) == 0 {
        return Some((
            "No changes. o opens lazygit for history, branches and commits.".to_owned(),
            None,
        ));
    }
    None
}

/// TS: changesPane.tsx ChangesPane props (width and height are the area's).
#[derive(Clone, Copy, Debug)]
pub struct ChangesPaneProps<'a> {
    pub session: Option<&'a SessionRecord>,
    pub changes: &'a ChangesRecord,
    pub focused: bool,
    /// The selected entry (TS -1: none).
    pub selected: Option<usize>,
    pub diff: Option<&'a DiffState>,
    pub diff_scroll: usize,
}

/// The part of `area` at (`dx`, `dy`) of the given size, clipped to the area.
fn sub_area(area: Rect, dx: usize, dy: usize, width: usize, height: usize) -> Rect {
    let clip = |value: usize, limit: u16| u16::try_from(value).unwrap_or(u16::MAX).min(limit);
    let x = clip(dx, area.width);
    let y = clip(dy, area.height);
    Rect::new(
        area.x + x,
        area.y + y,
        clip(width, area.width - x),
        clip(height, area.height - y),
    )
}

/// TS: changesPane.tsx ChangesPane's header: the workspace (compact path and branch) and the status counts.
fn header_row(props: &ChangesPaneProps<'_>, width: usize) -> Row {
    let ChangesPaneProps { session, changes, .. } = *props;
    let ready = changes.session_id.as_deref() == session.map(|session| session.id.as_str())
        && changes
            .workspace
            .as_deref()
            .is_some_and(|workspace| !workspace.is_empty());
    let status = if ready {
        summary(changes)
    } else {
        (String::new(), THEME.muted)
    };
    let branch = match changes.branch.as_deref() {
        Some(branch) if ready && !branch.is_empty() => format!(" · {branch}"),
        _ => String::new(),
    };
    let path_budget = width.saturating_sub(js_len(&status.0) + 1).max(8);
    let workspace = changes
        .workspace
        .as_deref()
        .or_else(|| session.and_then(|session| session.worktree.as_ref().and_then(|worktree| worktree.path.as_deref())))
        .or_else(|| session.map(|session| session.cwd.as_str()))
        .unwrap_or("");
    let where_ = truncate(
        &format!(
            "{}{branch}",
            compact_path(workspace, path_budget.saturating_sub(js_len(&branch)).max(8))
        ),
        path_budget,
    );
    let left = vec![Seg::new(where_, TextStyle::fg(THEME.muted))];
    let right = truncate_row(&[Seg::new(status.0, TextStyle::fg(status.1))], width, Wrap::TruncateEnd);
    space_between(&left, &right, width)
}

/// TS: changesPane.tsx ChangesPane.
pub fn render_changes_pane(props: &ChangesPaneProps<'_>, buf: &mut Buffer, area: Rect) {
    let width = usize::from(area.width);
    let height = usize::from(area.height);
    let ChangesPaneProps {
        session,
        changes,
        focused,
        selected,
        diff,
        diff_scroll,
    } = *props;
    draw_rows(buf, area, &[header_row(props, width)]);
    let below = sub_area(area, 0, 1, width, height.saturating_sub(1));
    if let Some((text, color)) = fallback_message(session, changes) {
        let line = vec![Seg::new(text, TextStyle::default().color(color))];
        draw_rows(buf, below, &fit_row(&line, width, Wrap::Wrap));
        return;
    }
    let rows = change_rows(changes);
    let body = height.saturating_sub(1).max(1);
    if !focused {
        draw_rows(buf, below, &changes_list(&rows, None, width, body, false));
        return;
    }
    let layout = changes_layout(width, height, rows.len());
    let entry = selected.and_then(|index| changes.entries.get(index));
    let diff_rows = diff_view(entry, diff, diff_scroll, layout.diff_width, layout.diff_rows + 1);
    let list = changes_list(&rows, selected, layout.list_width, layout.list_rows, true);
    if layout.side {
        draw_rows(buf, sub_area(area, 0, 1, layout.list_width, body), &list);
        let separator = vec![vec![Seg::new(" │ ", TextStyle::fg(THEME.border))]; body];
        draw_rows(buf, sub_area(area, layout.list_width, 1, 3, body), &separator);
        draw_rows(
            buf,
            sub_area(area, layout.list_width + 3, 1, layout.diff_width, body),
            &diff_rows,
        );
    } else {
        let list_rows = list.len();
        draw_rows(buf, sub_area(area, 0, 1, width, list_rows), &list);
        let rule = vec![Seg::new("─".repeat(width.max(1)), TextStyle::fg(THEME.border))];
        draw_rows(buf, sub_area(area, 0, 1 + list_rows, width, 1), &[rule]);
        draw_rows(
            buf,
            sub_area(area, 0, 2 + list_rows, width, height.saturating_sub(2 + list_rows)),
            &diff_rows,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::row_text;
    use deckhand_parity::{Case, replay};
    use serde_json::{Value, json};

    fn entry(group: &str, status: &str, path: &str, extra: Value) -> ChangeEntry {
        let mut full = json!({"group": group, "path": path, "status": status});
        if let (Some(full), Some(extra)) = (full.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                full.insert(key.clone(), value.clone());
            }
        }
        serde_json::from_value(full).expect("entry")
    }

    fn record(entries: Vec<ChangeEntry>) -> ChangesRecord {
        let count = |group: ChangeGroup| {
            i64::try_from(entries.iter().filter(|entry| entry.group == group).count()).unwrap_or(0)
        };
        let counts = serde_json::from_value(json!({
            "conflicts": count(ChangeGroup::Conflicts), "staged": count(ChangeGroup::Staged),
            "unstaged": count(ChangeGroup::Unstaged), "untracked": count(ChangeGroup::Untracked),
        }))
        .expect("counts");
        ChangesRecord {
            loaded: true,
            entries,
            counts,
            omitted: serde_json::from_value(json!({"conflicts": 0, "staged": 0, "unstaged": 0, "untracked": 0}))
                .expect("omitted"),
            branch: Some("main".to_owned()),
            error: None,
            session_id: Some("s1".to_owned()),
            workspace: Some("/work/wt".to_owned()),
        }
    }

    fn session() -> SessionRecord {
        serde_json::from_value(json!({
            "id": "s1", "title": "t", "program": "claude", "command": "claude", "cwd": "/work/repo",
            "repoRoot": "/work/repo", "status": "running", "createdAt": "", "updatedAt": "",
            "worktree": {"mode": "managed", "path": "/work/wt"},
        }))
        .expect("session")
    }

    fn lines(buf: &Buffer, area: Rect) -> Vec<String> {
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    fn draw(props: &ChangesPaneProps<'_>, width: u16, height: u16) -> Vec<String> {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        render_changes_pane(props, &mut buf, area);
        lines(&buf, area)
    }

    #[test]
    fn layout_splits_side_by_side_from_100_columns() {
        let side = changes_layout(120, 24, 5);
        assert_eq!(
            side,
            ChangesLayout {
                side: true,
                list_width: 50,
                list_rows: 23,
                diff_width: 67,
                diff_rows: 22
            }
        );
        let stacked = changes_layout(80, 24, 100);
        assert_eq!(
            stacked,
            ChangesLayout {
                side: false,
                list_width: 80,
                list_rows: 9,
                diff_width: 80,
                diff_rows: 12
            }
        );
        assert_eq!(changes_layout(0, 0, 0).list_rows, 2);
    }

    #[test]
    fn entry_rows_show_status_name_directory_and_counts() {
        let modified = entry("unstaged", "M", "src/app.ts", json!({"additions": 12, "deletions": 3}));
        assert_eq!(
            row_text(&entry_row(&modified, false, 40)),
            "  M app.ts  src                   +12 −3"
        );
        assert_eq!(
            row_text(&entry_row(&modified, true, 40)),
            "❯ M app.ts  src                   +12 −3"
        );
        // Counts that do not fit in half the row are dropped.
        assert_eq!(row_text(&entry_row(&modified, false, 12)), "  M app.ts …");
        let binary = entry("untracked", "?", "a.bin", json!({"binary": true}));
        assert_eq!(
            row_text(&entry_row(&binary, false, 30)),
            "  ? a.bin                  bin"
        );
        let selected = entry_row(&modified, true, 40);
        assert_eq!(selected[0].style.inverse, Some(true));
        // The selected row inherits its color from the row; the parts set none of their own.
        assert_eq!(selected[1].style.color, Some(THEME.active));
        let plain = entry_row(&modified, false, 40);
        assert_eq!(plain[1].style.color, Some(THEME.warn));
        assert_eq!(plain[1].style.bold, Some(true));
    }

    #[test]
    fn the_pane_shows_the_list_in_browse_and_the_diff_in_focus() {
        let entries = vec![
            entry("staged", "A", "new.txt", json!({"additions": 2, "deletions": 0})),
            entry("unstaged", "M", "src/a.ts", json!({"additions": 1, "deletions": 1})),
        ];
        let changes = record(entries);
        let session = session();
        let diff = DiffState {
            key: "k".to_owned(),
            diff: Some(ChangeDiff {
                group: ChangeGroup::Unstaged,
                path: "src/a.ts".to_owned(),
                text: "@@ -1 +1 @@\n-a\n+b\n".to_owned(),
                truncated: false,
                binary: false,
                first_line: 1,
            }),
            error: None,
        };
        let mut props = ChangesPaneProps {
            session: Some(&session),
            changes: &changes,
            focused: false,
            selected: None,
            diff: Some(&diff),
            diff_scroll: 0,
        };
        let browse = draw(&props, 40, 8);
        assert_eq!(browse[0], "/work/wt · main     1 staged · 1 changed");
        assert_eq!(browse[1], "Staged Changes 1");
        assert_eq!(browse[2], format!("  A new.txt{}+2 −0", " ".repeat(24)));
        assert_eq!(browse[3], "Changes 1");
        props.focused = true;
        props.selected = Some(1);
        let stacked = draw(&props, 40, 12);
        assert_eq!(stacked[4], format!("❯ M a.ts  src{}+1 −1", " ".repeat(22)));
        assert_eq!(stacked[5], "─".repeat(40));
        assert_eq!(stacked[6], "unstaged · src/a.ts");
        assert_eq!(stacked[7], "@@ -1 +1 @@");
        assert_eq!(stacked[8], "-a");
        assert_eq!(stacked[9], "+b");
        let side = draw(&props, 110, 6);
        assert!(side[3].contains(" │ -a"), "{side:?}");
        assert!(side[4].contains(" │ +b"), "{side:?}");
    }

    #[test]
    fn why_no_list_is_shown() {
        let session = session();
        let mut changes = record(Vec::new());
        assert_eq!(
            fallback_message(None, &changes).map(|found| found.0).as_deref(),
            Some("No session selected.")
        );
        assert_eq!(
            fallback_message(Some(&session), &changes)
                .map(|found| found.0)
                .as_deref(),
            Some("No changes. o opens lazygit for history, branches and commits.")
        );
        changes.loaded = false;
        assert_eq!(
            fallback_message(Some(&session), &changes)
                .map(|found| found.0)
                .as_deref(),
            Some("Reading changes…")
        );
        changes.loaded = true;
        changes.error = Some("boom".to_owned());
        assert_eq!(
            fallback_message(Some(&session), &changes),
            Some(("Git status failed: boom".to_owned(), Some(THEME.error)))
        );
        assert_eq!(summary(&changes), ("○ error".to_owned(), THEME.error));
        let props = ChangesPaneProps {
            session: Some(&session),
            changes: &changes,
            focused: false,
            selected: None,
            diff: None,
            diff_scroll: 0,
        };
        let drawn = draw(&props, 50, 3);
        assert_eq!(drawn[1], "Git status failed: boom");
    }

    #[test]
    fn the_list_window_follows_the_selection_with_markers() {
        let entries: Vec<ChangeEntry> = (0..10)
            .map(|index| entry("unstaged", "M", &format!("f{index}.txt"), json!({})))
            .collect();
        let changes = record(entries);
        let rows = change_rows(&changes);
        assert_eq!(rows.len(), 11);
        let browse = list_window(&rows, None, 4, false);
        assert_eq!(browse.len(), 4);
        assert_eq!(browse[3], ListWindowRow::Marker("  ↓ 8 more · v to browse".to_owned()));
        let focused = list_window(&rows, Some(5), 5, true);
        assert_eq!(focused.len(), 5);
        assert!(matches!(&focused[0], ListWindowRow::Marker(text) if text.starts_with("  ↑ ")));
        assert!(matches!(&focused[4], ListWindowRow::Marker(text) if text.starts_with("  ↓ ")));
        // The group header stays in view when the selection is its first entry.
        let first = list_window(&rows, Some(0), 5, true);
        assert!(matches!(&first[1], ListWindowRow::Row(ChangeRow::Header { .. })));
        assert_eq!(list_window(&rows, Some(7), 2, true).len(), 2);
        assert_eq!(list_window(&rows, Some(7), 0, true).len(), 0);
    }

    #[test]
    fn diff_view_scrolls_and_explains_missing_diffs() {
        let item = entry("conflicts", "U", "a.txt", json!({}));
        let text: String = (0..30).map(|index| format!("+line {index}\n")).collect();
        let state = DiffState {
            key: "k".to_owned(),
            diff: Some(ChangeDiff {
                group: ChangeGroup::Conflicts,
                path: "a.txt".to_owned(),
                text: format!("@@ -0,0 +1,30 @@\n{text}"),
                truncated: true,
                binary: false,
                first_line: 1,
            }),
            error: None,
        };
        let shown = diff_view(Some(&item), Some(&state), 5, 40, 6);
        assert_eq!(row_text(&shown[0]), "conflict · a.txt                 6-10/32");
        assert_eq!(shown.len(), 6);
        assert_eq!(row_text(&shown[1]), "+line 4");
        // Scrolled past the end: pinned to the last rows, the cut notice among them.
        let end = diff_view(Some(&item), Some(&state), 999, 80, 3);
        assert!(row_text(&end[0]).ends_with("31-32/32"));
        assert_eq!(
            row_text(&end[2]),
            "… preview cut here; open the file (enter) or lazygit (o) for the rest"
        );
        let failed = DiffState {
            error: Some("nope".to_owned()),
            ..DiffState::default()
        };
        let message = diff_view(Some(&item), Some(&failed), 0, 40, 4);
        assert_eq!(row_text(&message[1]), "Diff unavailable: nope");
        assert_eq!(message[1][0].style.color, Some(THEME.error));
        assert_eq!(row_text(&diff_view(Some(&item), None, 0, 40, 4)[1]), "Loading diff…");
        assert_eq!(row_text(&diff_view(None, None, 0, 40, 4)[0]), "No selection");
        assert_eq!(
            row_text(&diff_view(None, None, 0, 40, 4)[1]),
            "Select a file to preview its diff."
        );
    }

    #[test]
    fn visible_replaces_tabs_and_controls() {
        assert_eq!(visible("a\tb\u{1}c\u{9f}d\ne"), "a    b?c?d\ne");
    }

    #[test]
    fn colors_by_status_and_diff_kind() {
        assert_eq!(status_color("M"), Some(THEME.warn));
        assert_eq!(status_color("?"), Some(THEME.success));
        assert_eq!(status_color("U"), Some(THEME.error));
        assert_eq!(status_color("R"), Some(THEME.active));
        assert_eq!(status_color("X"), None);
        assert_eq!(diff_color(DiffLineKind::Context), None);
        assert_eq!(diff_color(DiffLineKind::Meta), Some(THEME.muted));
    }

    // ── Parity replays ──────────────────────────────────────────────────────────────────────────────────────────

    #[test]
    fn changes_layout_parity() {
        replay("U4", "changesPane/changesLayout", |case: &Case| {
            // Negative sizes (the fast-check cases) have no usize: a negative height lays out as 0 does, and a negative
            // width is passed through to the stacked widths, which are the width itself.
            let count = |index: usize| -> Result<usize, String> { Ok(case.arg_f64(index)?.max(0.0) as usize) };
            let width = case.arg_f64(0)?;
            let layout = changes_layout(count(0)?, count(1)?, count(2)?);
            let mut value = json!({
                "side": layout.side, "listWidth": layout.list_width, "listRows": layout.list_rows,
                "diffWidth": layout.diff_width, "diffRows": layout.diff_rows,
            });
            if width < 0.0 {
                value["listWidth"] = json!(width);
                value["diffWidth"] = json!(width);
            }
            Ok::<_, String>(value)
        });
    }

    #[test]
    fn diff_lines_parity() {
        replay("U4", "changesPane/diffLines", |case: &Case| {
            let diff = if case.arg_is_undefined(0) {
                None
            } else {
                let raw = case.arg(0);
                Some(ChangeDiff {
                    group: ChangeGroup::Unstaged,
                    path: String::new(),
                    text: raw
                        .get("text")
                        .and_then(Value::as_str)
                        .ok_or("diff without text")?
                        .to_owned(),
                    truncated: raw.get("truncated").and_then(Value::as_bool).unwrap_or(false),
                    binary: raw.get("binary").and_then(Value::as_bool).unwrap_or(false),
                    first_line: 1,
                })
            };
            let rows: Vec<Value> = diff_lines(diff.as_ref())
                .into_iter()
                .map(|row| {
                    let kind = match row.kind {
                        DiffRowKind::Note => json!("note"),
                        DiffRowKind::Line(kind) => json!(kind),
                    };
                    json!({"kind": kind, "text": row.text})
                })
                .collect();
            Ok::<_, String>(rows)
        });
    }
}
