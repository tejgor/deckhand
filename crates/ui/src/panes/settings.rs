//! Port of settingsPane.tsx: the Settings screen (C). The grid (one row per setting, one column per layer, the
//! cursor a cell), the Actions list and the link picker of one layer, each with a details box (or an inline edit
//! control) under it and one hint line. State and keys live in flows/settings.rs.

use std::sync::LazyLock;

use indexmap::IndexMap;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use regex::Regex;

use deckhand_core::display::{
    DISPLAY_CONTROL_PATTERN, DetailLine, DetailPart, HintPart, RowStatus, SELECTION_MARKER, THEME as INK, fit_hint,
    truncate, wrap_words,
};
use deckhand_core::jsstr::{js_len, js_pad_end, js_pad_start, js_slice};
use deckhand_core::settings_model::{
    ACTIONS_INTRO, COLUMNS, ChoiceOption, GridCell, GridRow, MAX_ACTIONS, NEEDS_TRUST_DETAIL, SETTINGS, SettingControl,
    SettingId, SettingSection, action_count, cell_detail, column_titles, home_path, layer_actions, layer_name,
    settings_grid,
};
use deckhand_core::text_editor::EditorState;
use deckhand_protocol::data::{
    CandidateConfigured, CandidateKind, CandidateSuggestion, ConfigTargetKind, RepositoryState, SettingsInfo,
    WorktreeCandidate, WorktreeCandidates,
};

use crate::panes::menu::{SelectableRowProps, detail_text, selectable_cell, selectable_row};
use crate::primitives::{Border, Column, Row, Seg, TextStyle, Wrap, bordered, draw_rows, truncate_row};
use crate::theme::{THEME, color};

/// TS: settingsPane.tsx SettingsView.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SettingsView {
    #[default]
    Main,
    Actions,
    Links,
}

/// TS: settingsPane.tsx SettingsEdit `step` (adding an action: its name first, then its command).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ActionStep {
    Name,
    Command,
}

/// TS: settingsPane.tsx SettingsEdit: the inline edit control under the list.
#[derive(Clone, Debug, PartialEq)]
pub enum SettingsEdit {
    Choice {
        id: SettingId,
        label: String,
        target: ConfigTargetKind,
        options: Vec<ChoiceOption>,
        index: usize,
        /// The option this layer stores (◉), if any (TS -1: none).
        current: Option<usize>,
    },
    Text {
        id: SettingId,
        label: String,
        target: ConfigTargetKind,
        state: EditorState,
        error: Option<String>,
        entry: Option<String>,
        step: Option<ActionStep>,
        /// Shown while the input is empty: what applies without a value here.
        placeholder: Option<String>,
    },
    Clear {
        id: SettingId,
        label: String,
        target: ConfigTargetKind,
        entry: Option<String>,
    },
    DiscardLinks,
}

/// TS: settingsPane.tsx LinksState: the link picker of one layer.
#[derive(Clone, Debug, PartialEq)]
pub struct LinksState {
    pub data: WorktreeCandidates,
    pub links: IndexMap<String, bool>,
    pub initial: IndexMap<String, bool>,
    pub target: ConfigTargetKind,
    pub row: usize,
}

/// TS: settingsPane.tsx Notice.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub error: bool,
}

/// `Math.max(0, value)` as a width.
fn width_of(value: i64) -> usize {
    usize::try_from(value.max(0)).unwrap_or(0)
}

/// A Rust length as the JS number it stands for.
fn num(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// An unstyled detail part.
fn plain_part(text: impl Into<String>) -> DetailPart {
    DetailPart {
        text: text.into(),
        ..DetailPart::default()
    }
}

/// A detail part in an Ink color.
fn colored_part(text: impl Into<String>, shade: &'static str) -> DetailPart {
    DetailPart {
        text: text.into(),
        color: Some(shade),
        ..DetailPart::default()
    }
}

/// The Ink name of a theme color (the inverse of [`color`] for the ANSI palette).
fn ink_name(value: Color) -> Option<&'static str> {
    Some(match value {
        Color::Black => "black",
        Color::Red => "red",
        Color::Green => "green",
        Color::Yellow => "yellow",
        Color::Blue => "blue",
        Color::Magenta => "magenta",
        Color::Cyan => "cyan",
        Color::Gray => "white",
        Color::DarkGray => "gray",
        Color::LightRed => "redBright",
        Color::LightGreen => "greenBright",
        Color::LightYellow => "yellowBright",
        Color::LightBlue => "blueBright",
        Color::LightMagenta => "magentaBright",
        Color::LightCyan => "cyanBright",
        Color::White => "whiteBright",
        _ => return None,
    })
}

/// A hint part with a short form and a drop priority.
fn hint_part(text: &str, short: Option<&str>, drop: Option<i32>) -> HintPart {
    HintPart {
        text: text.to_owned(),
        short: short.map(str::to_owned),
        drop,
    }
}

/// TS: settingsPane.tsx visible: controls as `?`, newlines and tabs as spaces.
pub fn visible(text: &str) -> String {
    DISPLAY_CONTROL_PATTERN
        .replace_all(text, "?")
        .replace(['\n', '\t'], " ")
}

/// TS: settingsPane.tsx layerShort (`global defaults` / `this repo`).
pub fn layer_short(kind: ConfigTargetKind) -> &'static str {
    match kind {
        ConfigTargetKind::Global => "global defaults",
        ConfigTargetKind::Repository => "this repo",
    }
}

/// TS: settingsPane.tsx LAYER_COLOR.
pub fn layer_color(kind: ConfigTargetKind) -> Color {
    match kind {
        ConfigTargetKind::Global => THEME.active,
        ConfigTargetKind::Repository => THEME.success,
    }
}

/// LAYER_COLOR as the Ink color name the detail parts carry.
fn layer_color_name(kind: ConfigTargetKind) -> &'static str {
    match kind {
        ConfigTargetKind::Global => INK.active,
        ConfigTargetKind::Repository => INK.success,
    }
}

/// TS: settingsPane.tsx tail: paths keep their distinctive tail (`…` in front); `width` <= 1 cuts.
pub fn tail(text: &str, width: usize) -> String {
    let length = js_len(text);
    if width <= 1 {
        js_slice(text, 0, Some(num(width)))
    } else if length <= width {
        text.to_owned()
    } else {
        format!("…{}", js_slice(text, num(length) - num(width) + 1, None))
    }
}

/// TS: settingsPane.tsx partsLength.
fn parts_length(parts: &[DetailPart]) -> usize {
    parts.iter().map(|part| js_len(&part.text)).sum()
}

/// TS: settingsPane.tsx partsLine: a nowrap detail line of styled parts.
pub fn parts_line(parts: Vec<DetailPart>) -> DetailLine {
    DetailLine {
        text: parts.iter().map(|part| part.text.as_str()).collect(),
        color: None,
        nowrap: Some(true),
        parts: Some(parts),
    }
}

/// TS: settingsPane.tsx repositoryState: the header's right side (deckhand.json's trust state) and its color.
pub fn repository_state(info: &SettingsInfo) -> (String, Color) {
    let hook = info.needs_review && info.repository.state != RepositoryState::Untrusted;
    let (text, shade) = match info.repository.state {
        RepositoryState::Trusted => {
            if hook {
                ("deckhand.json: trusted ✓ · hook needs review", THEME.warn)
            } else {
                ("deckhand.json: trusted ✓", THEME.success)
            }
        }
        RepositoryState::Untrusted => ("deckhand.json: needs trust", THEME.warn),
        RepositoryState::Absent => {
            if hook {
                ("creation hook needs review", THEME.warn)
            } else {
                ("deckhand.json: not present", THEME.muted)
            }
        }
        RepositoryState::Invalid => ("deckhand.json: invalid", THEME.error),
        RepositoryState::Bare => ("bare repo: global defaults only", THEME.muted),
        RepositoryState::None => ("not a Git repo: global defaults only", THEME.warn),
    };
    (text.to_owned(), shade)
}

/// The repair hint's reason inside a global config error: `/: (.*?)\. Repair it/` (JS `.` stops at line breaks).
static GLOBAL_REASON: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r": ([^\n\r  ]*?)\. Repair it").expect("GLOBAL_REASON is a valid regex"));

/// TS: settingsPane.tsx headerLines: invalid-config notices under the title.
pub fn header_lines(info: &SettingsInfo) -> Vec<Notice> {
    let mut lines = Vec::new();
    if info.repository.state == RepositoryState::Invalid {
        lines.push(Notice {
            text: format!(
                "deckhand.json invalid: {} (→ This repo, then e repairs it)",
                info.repository.error.as_deref().unwrap_or("unknown error")
            ),
            error: true,
        });
    }
    if let Some(error) = info.global_error.as_deref().filter(|error| !error.is_empty()) {
        let reason = GLOBAL_REASON
            .captures(error)
            .and_then(|captures| captures.get(1))
            .map_or(error, |reason| reason.as_str());
        lines.push(Notice {
            text: format!("Global defaults invalid: {reason} (← Global, then e repairs it)"),
            error: true,
        });
    }
    lines
}

/// TS: settingsPane.tsx ListLine: one line of the scrolled list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListLine {
    pub text: String,
    /// Styled segments of `text`.
    pub parts: Option<Vec<DetailPart>>,
    /// Rendered as is (a grid row).
    pub element: Option<Row>,
    /// The selectable row this line belongs to.
    pub row: Option<usize>,
    pub head: bool,
    pub selected: bool,
    pub color: Option<Color>,
    pub bold: bool,
    pub status: Option<(String, Option<Color>)>,
    pub status_width: Option<usize>,
    /// Explanation above the list, dropped when the list must scroll.
    pub intro: bool,
    /// A section heading: shown above its first row when it fits, never the scroll anchor.
    pub section: bool,
}

/// TS: settingsPane.tsx windowLines: keeps the selected row in view (its section heading too when it fits), with
/// ↑/↓ more markers; `scroll` persists the first shown line between renders.
pub fn window_lines(lines: &[ListLine], selected: usize, rows: usize, scroll: &mut usize) -> Vec<ListLine> {
    if lines.len() <= rows {
        *scroll = 0;
        return lines.to_vec();
    }
    let total = num(lines.len());
    let anchor = |line: &ListLine| line.row == Some(selected) && !line.section;
    let head = lines.iter().position(anchor).unwrap_or(0);
    let mut last = head;
    while lines.get(last + 1).is_some_and(anchor) {
        last += 1;
    }
    let current = num(*scroll);
    let place = |room: usize| -> usize {
        let section_above = head > 0 && lines.get(head - 1).is_some_and(|line| line.section);
        let top = if section_above && last - head + 2 <= room {
            head - 1
        } else {
            head
        };
        let mut start = current;
        if num(top) < start {
            start = num(top);
        } else if num(last) >= start + num(room) {
            start = num(head).min(num(last) - num(room) + 1);
        }
        width_of(start.min(total - num(room)))
    };
    let marker = |text: String| ListLine {
        text,
        color: Some(THEME.muted),
        ..ListLine::default()
    };
    // Too short for a marker beside the rows (two markers need a third row): rows only.
    if rows < 3 {
        let room = rows.max(1);
        *scroll = place(room);
        return lines.iter().skip(*scroll).take(room).cloned().collect();
    }
    // One marker when the window touches either end, otherwise both.
    let start = place(rows - 1);
    if start == 0 {
        *scroll = 0;
        let mut shown: Vec<ListLine> = lines.iter().take(rows - 1).cloned().collect();
        shown.push(marker(format!("  ↓ {} more", lines.len() + 1 - rows)));
        return shown;
    }
    if start + rows > lines.len() {
        *scroll = start;
        let mut shown = vec![marker(format!("  ↑ {start} more"))];
        shown.extend(lines.iter().skip(start).cloned());
        return shown;
    }
    let room = (rows - 2).max(1);
    let start = place(room);
    *scroll = start;
    let below = lines.len().saturating_sub(start + room);
    let mut shown = vec![marker(if start > 0 {
        format!("  ↑ {start} more")
    } else {
        " ".to_owned()
    })];
    shown.extend(lines.iter().skip(start).take(room).cloned());
    shown.push(marker(if below > 0 {
        format!("  ↓ {below} more")
    } else {
        " ".to_owned()
    }));
    shown
}

/// TS: settingsPane.tsx TWO_COLUMN_WIDTH: both layers' cells side by side from this inner width.
pub const TWO_COLUMN_WIDTH: usize = 64;

/// TS: settingsPane.tsx MAX_CELL.
pub const MAX_CELL: usize = 40;

/// TS: settingsPane.tsx LABEL_WIDTH: the longest setting label.
pub fn label_width() -> usize {
    SETTINGS.iter().map(|def| js_len(def.label)).max().unwrap_or(0)
}

/// TS: settingsPane.tsx GridLayout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GridLayout {
    pub two: bool,
    pub label: usize,
    pub cell: usize,
}

/// TS: settingsPane.tsx gridLayout.
pub fn grid_layout(inner: usize) -> GridLayout {
    let label = label_width().min((inner / 3).max(8));
    let two = inner >= TWO_COLUMN_WIDTH;
    let room = width_of(num(inner) - num(label) - 4).max(4);
    GridLayout {
        two,
        label,
        cell: MAX_CELL.min(if two { (room - 1) / 2 } else { room }),
    }
}

/// TS: settingsPane.tsx cellParts: a cell's styled parts and whether its value was cut.
pub fn cell_parts(cell: &GridCell, width: usize, path: bool) -> (Vec<DetailPart>, bool) {
    let lead = if cell.effective {
        DetailPart {
            text: "● ".to_owned(),
            color: Some(INK.active),
            bold: Some(true),
            italic: None,
        }
    } else {
        plain_part("  ")
    };
    let room = width.saturating_sub(2).max(1);
    let text = visible(&cell.text);
    let needs_trust = cell.needs_trust == Some(true);
    let warning = cell.warning.as_deref().filter(|warning| !warning.is_empty());
    let warn = needs_trust || warning.is_some();
    let (long, short) = if needs_trust {
        (" ⚠ needs trust".to_owned(), " ⚠")
    } else if let Some(warning) = warning {
        (format!(" ⚠ {warning}"), " ⚠")
    } else if cell.built_in == Some(true) {
        (" (built-in)".to_owned(), " (built-in)")
    } else if cell.legacy == Some(true) {
        (" (legacy)".to_owned(), " (legacy)")
    } else {
        (String::new(), "")
    };
    // The long form when the value fits beside it. Otherwise ⚠ always stays (cutting the value); "(built-in)" goes when
    // the value alone fits (its style and the details line still say so), else stays while the value keeps 8 columns.
    let text_length = js_len(&text);
    let suffix: &str = if text_length + js_len(&long) <= room {
        &long
    } else if warn {
        short
    } else if text_length <= room || room < js_len(short) + 8 {
        ""
    } else {
        short
    };
    let keep = room.saturating_sub(js_len(suffix)).max(1);
    let body = if path { tail(&text, keep) } else { truncate(&text, keep) };
    let cut = js_len(&body) < text_length;
    let mut style = plain_part("");
    if needs_trust {
        style.color = Some(INK.warn);
    } else if cell.built_in == Some(true) || cell.legacy == Some(true) || cell.global_only == Some(true) {
        style.color = Some(INK.muted);
        style.italic = Some(true);
    } else if cell.set && cell.effective {
        style.bold = Some(true);
    } else {
        style.color = Some(INK.muted);
    }
    let mut parts = vec![lead, DetailPart { text: body, ..style }];
    if !suffix.is_empty() {
        parts.push(if warn {
            colored_part(suffix, INK.warn)
        } else {
            DetailPart {
                text: suffix.to_owned(),
                color: Some(INK.muted),
                bold: None,
                italic: Some(true),
            }
        });
    }
    (parts, cut)
}

/// TS: settingsPane.tsx isPath.
fn is_path(row: &GridRow) -> bool {
    row.def.id == SettingId::WorktreeLocation
}

/// TS: settingsPane.tsx gridRowElement: one grid row (label, then one or both cells), cut by the caller to the pane.
pub fn grid_row(row: &GridRow, selected: bool, column: ConfigTargetKind, layout: GridLayout) -> Row {
    let columns: &[ConfigTargetKind] = if layout.two { &COLUMNS } else { &[column] };
    let mut segs = vec![
        Seg::new(
            if selected { SELECTION_MARKER } else { " " },
            TextStyle::fg(THEME.active).bold(true),
        ),
        Seg::plain(" "),
        Seg::new(
            js_pad_end(&truncate(row.def.label, layout.label), layout.label, " "),
            TextStyle::default().bold(selected),
        ),
        Seg::plain("  "),
    ];
    for (index, kind) in columns.iter().enumerate() {
        if index > 0 {
            segs.push(Seg::plain(" "));
        }
        let (parts, _) = cell_parts(row.cells.get(*kind), layout.cell, is_path(row));
        segs.extend(selectable_cell(selected && *kind == column, &parts, layout.cell, None));
    }
    segs
}

/// TS: settingsPane.tsx ColumnHeader: the column headings (aligned with the values), or `◂ Global | This repo ▸` when
/// only the selected column shows. Cut by the caller to the pane.
pub fn column_header(column: ConfigTargetKind, layout: GridLayout) -> Row {
    let style = |kind: ConfigTargetKind| {
        if kind == column {
            TextStyle::fg(THEME.accent).bold(true)
        } else {
            TextStyle::fg(THEME.muted)
        }
    };
    let mut segs = vec![Seg::plain(" ".repeat(layout.label + 4))];
    if layout.two {
        for (index, kind) in COLUMNS.iter().enumerate() {
            if index > 0 {
                segs.push(Seg::plain(" "));
            }
            let titles = column_titles(*kind);
            let title = titles
                .iter()
                .copied()
                .find(|title| js_len(title) + 2 <= layout.cell)
                .or_else(|| titles.last().copied())
                .unwrap_or_default();
            let cut = truncate(&format!("  {title}"), layout.cell);
            segs.push(Seg::new(
                js_pad_end(&cut, if index > 0 { 0 } else { layout.cell }, " "),
                style(*kind),
            ));
        }
        return segs;
    }
    let muted = TextStyle::fg(THEME.muted);
    segs.push(Seg::new("◂ ", muted));
    segs.push(Seg::new("Global", style(ConfigTargetKind::Global)));
    segs.push(Seg::new(" | ", muted));
    segs.push(Seg::new("This repo", style(ConfigTargetKind::Repository)));
    segs.push(Seg::new(" ▸", muted));
    segs
}

/// The section heading text of a setting.
fn section_name(section: SettingSection) -> &'static str {
    match section {
        SettingSection::General => "General",
        SettingSection::Commands => "Commands",
        SettingSection::Worktrees => "Worktrees",
        SettingSection::Agents => "Agents",
    }
}

/// TS: settingsPane.tsx gridLines.
pub fn grid_lines(grid: &[GridRow], selected: usize, column: ConfigTargetKind, layout: GridLayout) -> Vec<ListLine> {
    let mut lines = Vec::new();
    for (index, row) in grid.iter().enumerate() {
        if index == 0 || grid[index - 1].def.section != row.def.section {
            lines.push(ListLine {
                text: format!(" {}", section_name(row.def.section)),
                section: true,
                bold: true,
                color: Some(THEME.accent_soft),
                ..ListLine::default()
            });
        }
        lines.push(ListLine {
            text: row.def.label.to_owned(),
            row: Some(index),
            element: Some(grid_row(row, index == selected, column, layout)),
            ..ListLine::default()
        });
    }
    lines
}

/// The config file path of a layer (`''` when the layer has none).
fn target_path(info: &SettingsInfo, kind: ConfigTargetKind) -> &str {
    let document = match kind {
        ConfigTargetKind::Global => info.targets.global.as_ref(),
        ConfigTargetKind::Repository => info.targets.repository.as_ref(),
    };
    document.map_or("", |document| document.path.as_str())
}

/// TS: settingsPane.tsx gridDetails: the details box of the selected cell, one line: "<Layer> · <file> · <how it
/// relates to the other layer>" (the file is cut from the front, or dropped when the rest needs the room, then the
/// line wraps), plus the full value when the cell cut it, a template, or a note (hook state, ignored while a hook is
/// active).
pub fn grid_details(
    info: &SettingsInfo,
    row: &GridRow,
    column: ConfigTargetKind,
    layout: GridLayout,
    width: usize,
) -> Vec<DetailLine> {
    let detail = cell_detail(info, row, column);
    let shade = if detail.error == Some(true) {
        Some(INK.error)
    } else if detail.warn == Some(true) {
        Some(INK.warn)
    } else {
        None
    };
    // "Built-in default" becomes "Built-in" when that keeps the explanation on one line (the cell says built-in too).
    let head_text = if detail.layer.is_none() && js_len(&detail.head) + js_len(&detail.relation) + 3 > width {
        "Built-in".to_owned()
    } else {
        detail.head.clone()
    };
    let head = DetailPart {
        text: head_text.clone(),
        color: Some(detail.layer.map_or(INK.muted, layer_color_name)),
        bold: Some(true),
        italic: None,
    };
    let relation = DetailPart {
        text: detail.relation.clone(),
        color: shade,
        ..DetailPart::default()
    };
    let home = info
        .vars
        .as_ref()
        .map(|vars| vars.home.clone())
        .or_else(|| std::env::var("HOME").ok());
    let file = detail
        .layer
        .map(|layer| home_path(target_path(info, layer), home.as_deref()))
        .unwrap_or_default();
    let used = parts_length(&[head.clone(), relation.clone()]);
    let room = num(width) - num(used) - 6;
    let mut lines: Vec<DetailLine> = if !file.is_empty() && room >= num(js_len(&file).min(14)) {
        vec![parts_line(vec![
            head,
            plain_part(" · "),
            plain_part(tail(&file, width_of(room))),
            plain_part(" · "),
            relation,
        ])]
    } else if used + 3 <= width {
        vec![parts_line(vec![head, plain_part(" · "), relation])]
    } else {
        vec![DetailLine {
            text: format!("{head_text} · {}", detail.relation),
            color: shade,
            ..DetailLine::default()
        }]
    };
    let cell = row.cells.get(column);
    let cut = cell_parts(cell, layout.cell, is_path(row)).1 || (row.def.control == SettingControl::Links && cell.set);
    let template = cell.template.as_deref().filter(|template| !template.is_empty());
    let note = row.note.as_deref().filter(|note| !note.is_empty());
    let full = cell.full.as_deref().filter(|full| !full.is_empty());
    let extra: Option<String> = match template {
        Some(template) if template != cell.text => Some(format!("Template: {template}")),
        _ => {
            if cut && full.is_some() {
                full.map(str::to_owned)
            } else {
                note.map(str::to_owned)
            }
        }
    };
    // A second line only while the first did not wrap: the box stays at two lines.
    if let Some(extra) = extra {
        let unwrapped = lines[0].parts.is_some() || wrap_words(&lines[0].text, width).len() == 1;
        if unwrapped {
            let is_note = note == Some(extra.as_str());
            lines.push(DetailLine {
                text: visible(&extra),
                color: Some(if is_note && row.warn == Some(true) {
                    INK.warn
                } else {
                    INK.muted
                }),
                nowrap: Some(true),
                parts: None,
            });
        }
    }
    lines
}

/// TS: settingsPane.tsx columns: label/value/status rows: marker (2) + label + gap + value + gap + status.
struct ListColumns {
    label_width: usize,
    value_width: usize,
    status_width: usize,
}

/// TS: settingsPane.tsx columns.
fn list_columns(inner: usize, labels: &[&str], statuses: &[&str]) -> ListColumns {
    let status_width = statuses
        .iter()
        .map(|status| js_len(status))
        .max()
        .unwrap_or(0)
        .min(inner / 3);
    let label_width = labels
        .iter()
        .map(|label| js_len(label))
        .max()
        .unwrap_or(0)
        .max(4)
        .min((inner / 3).max(8));
    let value_width = width_of(num(inner) - 2 - 1 - num(label_width) - 2 - 2 - num(status_width)).max(6);
    ListColumns {
        label_width,
        value_width,
        status_width,
    }
}

/// TS: settingsPane.tsx valueLine's options.
#[derive(Clone, Copy)]
struct ValueStyle {
    selected: bool,
    muted: bool,
    status_color: Color,
}

/// TS: settingsPane.tsx valueLine.
fn value_line(
    layout: &ListColumns,
    row: Option<usize>,
    label: &str,
    value: &str,
    status: Option<&str>,
    style: ValueStyle,
) -> ListLine {
    let text = format!(
        " {}  {}",
        js_pad_end(&truncate(&visible(label), layout.label_width), layout.label_width, " "),
        js_pad_end(&truncate(&visible(value), layout.value_width), layout.value_width, " ")
    );
    ListLine {
        text,
        row,
        head: row.is_some(),
        selected: style.selected,
        color: style.muted.then_some(THEME.muted),
        status: Some((status.unwrap_or("").to_owned(), Some(style.status_color))),
        status_width: Some(layout.status_width),
        ..ListLine::default()
    }
}

/// TS: settingsPane.tsx actionLines: the Actions view's lines and the value column's width.
pub fn action_lines(
    info: &SettingsInfo,
    column: ConfigTargetKind,
    selected: usize,
    inner: usize,
) -> (Vec<ListLine>, usize) {
    let actions = layer_actions(info, column);
    let (own, context) = (&actions.own, &actions.context);
    let all: Vec<_> = own.iter().chain(context.iter()).collect();
    let names: Vec<&str> = all.iter().map(|action| action.name.as_str()).collect();
    let statuses: Vec<&str> = all
        .iter()
        .map(|action| action.status.as_deref().unwrap_or(""))
        .collect();
    let layout = list_columns(inner, &names, &statuses);
    // What actions are, above the list (it scrolls away with a long list).
    let mut lines: Vec<ListLine> = wrap_words(ACTIONS_INTRO, inner.saturating_sub(1).max(1))
        .into_iter()
        .map(|text| ListLine {
            text: format!(" {text}"),
            color: Some(THEME.muted),
            intro: true,
            ..ListLine::default()
        })
        .collect();
    for (index, action) in own.iter().enumerate() {
        let style = ValueStyle {
            selected: index == selected,
            muted: false,
            status_color: if action.needs_trust == Some(true) {
                THEME.warn
            } else {
                THEME.muted
            },
        };
        lines.push(value_line(
            &layout,
            Some(index),
            &action.name,
            &action.command,
            action.status.as_deref(),
            style,
        ));
    }
    lines.push(ListLine {
        text: " + Add action".to_owned(),
        row: Some(own.len()),
        head: true,
        color: Some(THEME.active),
        selected: selected == own.len(),
        ..ListLine::default()
    });
    if !context.is_empty() {
        lines.push(ListLine {
            text: if column == ConfigTargetKind::Repository {
                " Also in effect here, from global defaults:"
            } else {
                " This repo also has:"
            }
            .to_owned(),
            color: Some(THEME.muted),
            ..ListLine::default()
        });
        let style = ValueStyle {
            selected: false,
            muted: true,
            status_color: THEME.muted,
        };
        for action in context {
            lines.push(value_line(
                &layout,
                None,
                &action.name,
                &action.command,
                action.status.as_deref(),
                style,
            ));
        }
    }
    (lines, layout.value_width)
}

/// TS: settingsPane.tsx actionDetails.
pub fn action_details(
    info: &SettingsInfo,
    column: ConfigTargetKind,
    selected: usize,
    value_width: usize,
) -> Vec<DetailLine> {
    let actions = layer_actions(info, column);
    let Some(action) = actions.own.get(selected) else {
        let mut lines = vec![DetailLine {
            text: "a (or Enter) adds an action: first its name, then its command.".to_owned(),
            ..DetailLine::default()
        }];
        if action_count(&info.targets, column) >= MAX_ACTIONS {
            lines.push(DetailLine {
                text: format!(
                    "{} has {MAX_ACTIONS} actions, the most allowed: remove one (x) to add another.",
                    layer_short(column)
                ),
                color: Some(INK.warn),
                ..DetailLine::default()
            });
        }
        return lines;
    };
    let needs_trust = action.needs_trust == Some(true);
    let status = action.status.as_deref().filter(|status| !status.is_empty());
    let relation = if needs_trust {
        NEEDS_TRUST_DETAIL.to_owned()
    } else if status == Some("overrides global") {
        format!("overrides the global {} action", action.name)
    } else if status.is_some() {
        format!("this repo's own {} action wins there", action.name)
    } else {
        "run it with e on a session".to_owned()
    };
    let mut lines = vec![DetailLine {
        text: format!("{} · {relation}", layer_name(column)),
        color: needs_trust.then_some(INK.warn),
        ..DetailLine::default()
    }];
    if js_len(&action.command) > value_width {
        lines.push(DetailLine {
            text: action.command.clone(),
            color: Some(INK.active),
            nowrap: Some(true),
            parts: None,
        });
    }
    lines
}

/// TS: settingsPane.tsx formatSize (`12 KB`, `1.2 MB`, `?` while unknown). `kib`: `None` is undefined (still being
/// measured), `Some(None)` is null (unknown).
pub fn format_size(kib: Option<Option<i64>>, kind: Option<CandidateKind>) -> String {
    if kind == Some(CandidateKind::Symlink) {
        return String::new();
    }
    let kib = match kib {
        None => return "…".to_owned(),
        Some(None) => return "?".to_owned(),
        Some(Some(kib)) => i128::from(kib),
    };
    if kib < 1024 {
        return format!("{kib} KB");
    }
    // `toFixed(1)` and `Math.round` round halves up; the sizes are exact multiples of 1/1024, so integer arithmetic
    // finds the same tenth the decimal expansion does.
    let tenths = |unit: i128| {
        let n = (kib * 20 + unit) / (2 * unit);
        format!("{}.{}", n / 10, n % 10)
    };
    if kib < 1024 * 1024 {
        if kib < 10 * 1024 {
            format!("{} MB", tenths(1024))
        } else {
            format!("{} MB", (kib * 2 + 1024) / 2048)
        }
    } else {
        format!("{} GB", tenths(1024 * 1024))
    }
}

/// TS: settingsPane.tsx candidateAction: what saving would do with a candidate.
pub fn candidate_action(candidate: &WorktreeCandidate) -> String {
    let layers = candidate.layers.as_deref().unwrap_or_default();
    if candidate.added == Some(true) {
        "added here (Enter saves)".to_owned()
    } else if candidate.missing == Some(true) {
        "missing in checkout".to_owned()
    } else if candidate.configured == Some(CandidateConfigured::Files) {
        format!("files → {}", candidate.source.as_deref().unwrap_or("undefined"))
    // In this repo's picker: a global entry links here too, whatever this list says.
    } else if candidate.configured.is_some() && layers == [ConfigTargetKind::Global] {
        "linked by Global (every repo)".to_owned()
    } else if candidate.configured.is_some() {
        if layers.is_empty() {
            "configured".to_owned()
        } else {
            let names: Vec<&str> = layers
                .iter()
                .map(|layer| match layer {
                    ConfigTargetKind::Global => "global",
                    ConfigTargetKind::Repository => "repo",
                })
                .collect();
            format!("configured ({})", names.join(", "))
        }
    } else if !candidate.reason.is_empty() {
        let prefix = if candidate.suggestion == CandidateSuggestion::Link {
            "suggested: "
        } else {
            ""
        };
        format!("{prefix}{}", candidate.reason)
    } else {
        String::new()
    }
}

/// Narrow rows drop the "suggested: " prefix ([link]/[skip] already shows the suggestion); the details box has it.
fn fit_action(text: &str, width: usize) -> String {
    if js_len(text) > width {
        text.strip_prefix("suggested: ").unwrap_or(text).to_owned()
    } else {
        text.to_owned()
    }
}

/// "in 3 of 4 repos" for the Global picker's rows.
fn repo_count(candidate: &WorktreeCandidate, total: usize) -> String {
    if candidate.configured == Some(CandidateConfigured::Files) {
        format!("files → {}", candidate.source.as_deref().unwrap_or("undefined"))
    } else if candidate.added == Some(true) {
        "added here".to_owned()
    } else {
        format!(
            "in {} of {total} repo{}",
            candidate.repos.as_ref().map_or(0, Vec::len),
            if total == 1 { "" } else { "s" }
        )
    }
}

/// A candidate's path with `/` after a directory.
fn candidate_name(candidate: &WorktreeCandidate) -> String {
    format!(
        "{}{}",
        candidate.path,
        if candidate.kind == CandidateKind::Dir { "/" } else { "" }
    )
}

/// TS: settingsPane.tsx globalLinkLines: the global link picker's lines. Global: the global entries, then the paths
/// several of your repositories share; a count instead of a size.
pub fn global_link_lines(state: &LinksState, inner: usize) -> Vec<ListLine> {
    let data = &state.data;
    let candidates: Vec<&WorktreeCandidate> = data.candidates.iter().map(|candidate| &**candidate).collect();
    let repos: &[String] = data.repos.as_deref().unwrap_or_default();
    let plural = |count: usize| if count == 1 { "" } else { "s" };
    let list = if repos.is_empty() {
        "none readable".to_owned()
    } else {
        repos.join(", ")
    };
    let mut lines = vec![ListLine {
        text: format!(
            "Links in every repository's new worktrees · your {} repo{}: {list}",
            repos.len(),
            plural(repos.len())
        ),
        color: Some(THEME.muted),
        intro: true,
        ..ListLine::default()
    }];
    if let Some(error) = data.candidates_error.as_deref().filter(|error| !error.is_empty()) {
        lines.push(ListLine {
            text: format!("Could not list your repositories' files: {error}"),
            color: Some(THEME.error),
            ..ListLine::default()
        });
    }
    let linked = candidates
        .iter()
        .filter(|candidate| candidate.configured.is_some() || candidate.added == Some(true))
        .count();
    let count_width = candidates
        .iter()
        .map(|candidate| js_len(&repo_count(candidate, repos.len())))
        .max()
        .unwrap_or(0)
        .clamp(10, 24);
    let longest = candidates
        .iter()
        .map(|candidate| js_len(&candidate.path) + 1)
        .max()
        .unwrap_or(0)
        .max(8);
    let name_width = width_of(num(longest).min(num(inner) - 2 - 7 - 2 - num(count_width) - 2 - 10)).max(8);
    let row = |lines: &mut Vec<ListLine>, candidate: &WorktreeCandidate, index: usize| {
        let on = candidate.configured == Some(CandidateConfigured::Files)
            || state.links.get(&candidate.path).copied().unwrap_or(false);
        let reason = if candidate.configured.is_some() || candidate.added == Some(true) || candidate.reason.is_empty() {
            String::new()
        } else {
            format!(
                "{}{}",
                if candidate.suggestion == CandidateSuggestion::Link {
                    "suggested: "
                } else {
                    ""
                },
                candidate.reason
            )
        };
        let text = format!(
            "[{}] {}  {}  {reason}",
            if on { "link" } else { "skip" },
            js_pad_end(
                &truncate(&visible(&candidate_name(candidate)), name_width),
                name_width,
                " "
            ),
            js_pad_end(
                &truncate(&repo_count(candidate, repos.len()), count_width),
                count_width,
                " "
            ),
        );
        lines.push(ListLine {
            text,
            row: Some(index),
            head: true,
            color: Some(if on { THEME.success } else { THEME.muted }),
            selected: index == state.row,
            ..ListLine::default()
        });
    };
    let section = |text: &str| ListLine {
        text: text.to_owned(),
        color: Some(THEME.accent_soft),
        bold: true,
        section: true,
        ..ListLine::default()
    };
    let note = |text: String| ListLine {
        text,
        color: Some(THEME.muted),
        ..ListLine::default()
    };
    lines.push(section("Linked in every repo"));
    if linked == 0 {
        lines.push(note(
            "  Nothing yet: space links a path below, a adds one by name.".to_owned(),
        ));
    }
    for (index, candidate) in candidates.iter().take(linked).enumerate() {
        row(&mut lines, candidate, index);
    }
    lines.push(section("Found in several of your repos"));
    if candidates.len() == linked {
        lines.push(note(if repos.len() < 2 {
            "  Deckhand knows only this repository; its own files belong in the This repo column.".to_owned()
        } else {
            format!(
                "  No untracked or ignored path is in two of your {} repos (build output and caches are left out).",
                repos.len()
            )
        }));
    }
    for (index, candidate) in candidates.iter().skip(linked).enumerate() {
        row(&mut lines, candidate, linked + index);
    }
    if data.more_candidates != 0 {
        lines.push(note(format!("  +{} more (a adds one by name)", data.more_candidates)));
    }
    lines
}

/// TS: settingsPane.tsx linkLines: the link picker's lines (sizes per path; `home` for `~`).
pub fn link_lines(
    state: &LinksState,
    sizes: &IndexMap<String, Option<i64>>,
    home: Option<&str>,
    inner: usize,
) -> Vec<ListLine> {
    if state.data.repos.is_some() {
        return global_link_lines(state, inner);
    }
    let data = &state.data;
    let candidates: Vec<&WorktreeCandidate> = data.candidates.iter().map(|candidate| &**candidate).collect();
    let mut lines = Vec::new();
    if let Some(error) = data.candidates_error.as_deref().filter(|error| !error.is_empty()) {
        lines.push(ListLine {
            text: format!("Could not list untracked/ignored files: {error}"),
            color: Some(THEME.error),
            ..ListLine::default()
        });
    } else if candidates.is_empty() {
        lines.push(ListLine {
            text: "No untracked or ignored entries in the main checkout.".to_owned(),
            color: Some(THEME.muted),
            ..ListLine::default()
        });
    }
    // Marker (2), "[link] " (7), name, gap, size, gap, then the reason in the rest.
    let size_width = 7;
    let fixed = 2 + 7 + 2 + size_width + 2;
    let names: Vec<String> = candidates
        .iter()
        .map(|candidate| {
            let arrow = match candidate.target.as_deref().filter(|target| !target.is_empty()) {
                Some(target) => format!(" → {}", tail(&home_path(target, home), 30)),
                None if candidate.kind == CandidateKind::Symlink => " → (broken link)".to_owned(),
                None => String::new(),
            };
            format!("{}{arrow}", candidate_name(candidate))
        })
        .collect();
    let longest = names.iter().map(|name| js_len(name)).max().unwrap_or(0).max(8);
    let name_width = width_of(num(longest).min(num(inner) - num(fixed) - 12)).max(8);
    let action_width = width_of(num(inner) - num(fixed) - num(name_width)).max(1);
    for (index, (candidate, name)) in candidates.iter().zip(&names).enumerate() {
        let linked = candidate.configured == Some(CandidateConfigured::Files)
            || state.links.get(&candidate.path).copied().unwrap_or(false);
        // `sizes[path] ?? (missing ? null : undefined)`: a null size is replaced too.
        let size = match sizes.get(&candidate.path) {
            Some(Some(kib)) => Some(Some(*kib)),
            _ => candidate.missing.filter(|missing| *missing).map(|_| None),
        };
        let text = format!(
            "[{}] {}  {}  {}",
            if linked { "link" } else { "skip" },
            js_pad_end(&truncate(&visible(name), name_width), name_width, " "),
            js_pad_start(&format_size(size, Some(candidate.kind)), size_width, " "),
            truncate(
                &visible(&fit_action(&candidate_action(candidate), action_width)),
                action_width
            ),
        );
        lines.push(ListLine {
            text,
            row: Some(index),
            head: true,
            color: Some(if linked { THEME.success } else { THEME.muted }),
            selected: index == state.row,
            ..ListLine::default()
        });
    }
    if data.more_candidates != 0 {
        lines.push(ListLine {
            text: format!("  +{} more (edit raw JSON)", data.more_candidates),
            color: Some(THEME.muted),
            ..ListLine::default()
        });
    }
    lines
}

// Edit controls ---------------------------------------------------------------------------------------------------

/// A single-line `<Text wrap="truncate-end">` of one styled run.
fn text_row(text: &str, style: TextStyle, width: usize) -> Row {
    truncate_row(&[Seg::new(text, style)], width, Wrap::TruncateEnd)
}

/// A single-line text in an Ink color name (`None`: the terminal's own).
fn ink_style(shade: Option<&str>) -> TextStyle {
    TextStyle::default().color(shade.and_then(color))
}

/// TS: settingsPane.tsx InputLine: a one-line text input with its cursor (or the placeholder). Text on one row: the
/// cursor is shown inverse and the view scrolls horizontally to keep it visible.
pub fn input_line(state: &EditorState, width: usize, placeholder: Option<&str>) -> Row {
    let characters: Vec<char> = state.text.chars().collect();
    let mut end = state.cursor.min(state.text.len());
    while !state.text.is_char_boundary(end) {
        end -= 1;
    }
    let cursor = state.text[..end].chars().count();
    let room = width.saturating_sub(3).max(2);
    let from = if cursor < room { 0 } else { cursor - room + 1 };
    let window = |from: usize, to: usize| -> String {
        characters
            .get(from.min(characters.len())..to.min(characters.len()))
            .unwrap_or_default()
            .iter()
            .collect()
    };
    let at = cursor - from;
    let lead = Seg::new("› ", TextStyle::fg(THEME.accent));
    let row = if state.select_all == Some(true) && !characters.is_empty() {
        // Ctrl+A: the whole text is selected (typing replaces it).
        vec![
            lead,
            Seg::new(visible(&window(0, room)), TextStyle::default().inverse(true)),
        ]
    } else if let (true, Some(placeholder)) = (characters.is_empty(), placeholder.filter(|text| !text.is_empty())) {
        vec![
            lead,
            Seg::new(" ", TextStyle::default().inverse(true).bold(true)),
            Seg::new(
                truncate(&format!(" {}", visible(placeholder)), room.saturating_sub(1)),
                TextStyle::fg(THEME.muted).italic(true),
            ),
        ]
    } else {
        let at_char = window(from + at, from + at + 1);
        vec![
            lead,
            Seg::plain(visible(&window(from, from + at))),
            Seg::new(
                visible(if at_char.is_empty() { " " } else { &at_char }),
                TextStyle::default().inverse(true).bold(true),
            ),
            Seg::plain(visible(&window(from + at + 1, from + room))),
        ]
    };
    truncate_row(&row, width, Wrap::TruncateEnd)
}

/// TS: settingsPane.tsx wrapChunks: help prose wrapped at its " · " separators where it can (so an example or rule is
/// not split), else at words.
pub fn wrap_chunks(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for chunk in text.split(" · ") {
        let joined = if line.is_empty() {
            chunk.to_owned()
        } else {
            format!("{line} · {chunk}")
        };
        if js_len(&joined) <= width {
            line = joined;
            continue;
        }
        if !line.is_empty() {
            lines.push(line);
        }
        let mut words = wrap_words(chunk, width);
        line = words.pop().unwrap_or_default();
        lines.extend(words);
    }
    let mut chunked = lines;
    if !line.is_empty() {
        chunked.push(line);
    }
    let words = wrap_words(text, width);
    if chunked.len() <= words.len() { chunked } else { words }
}

/// TS: settingsPane.tsx editMessage: a text edit's messages: its error (wrapped, up to 3 lines), then the help lines
/// (prose wraps, paths and previews are cut).
fn edit_message(error: Option<&str>, help: Option<&[DetailLine]>, inner: usize) -> Vec<DetailLine> {
    let mut lines: Vec<DetailLine> = Vec::new();
    if let Some(error) = error.filter(|error| !error.is_empty()) {
        lines.extend(
            wrap_words(&visible(error), inner)
                .into_iter()
                .take(3)
                .map(|text| DetailLine {
                    text,
                    color: Some(INK.error),
                    ..DetailLine::default()
                }),
        );
    }
    for line in help.unwrap_or_default() {
        // Wrapped help continues indented, so each rule or example list reads as one item.
        let texts: Vec<String> = if line.nowrap == Some(true) {
            vec![truncate(&visible(&line.text), inner)]
        } else {
            wrap_chunks(&visible(&line.text), inner.saturating_sub(2).max(1))
                .into_iter()
                .enumerate()
                .map(|(index, text)| if index > 0 { format!("  {text}") } else { text })
                .collect()
        };
        lines.extend(texts.into_iter().map(|text| DetailLine {
            text,
            color: Some(line.color.unwrap_or(INK.muted)),
            ..DetailLine::default()
        }));
    }
    if lines.is_empty() {
        lines.push(DetailLine {
            text: " ".to_owned(),
            ..DetailLine::default()
        });
    }
    lines
}

/// TS: settingsPane.tsx editTitle: the title of an edit: "<Setting> · <Layer>"; adding an action is two numbered
/// steps (shortened to keep the name).
pub fn edit_title(edit: &SettingsEdit, width: usize) -> String {
    match edit {
        SettingsEdit::DiscardLinks => "Discard link changes?".to_owned(),
        SettingsEdit::Clear { label, target, .. } => format!("Clear {label} in {}?", layer_short(*target)),
        SettingsEdit::Choice { label, target, .. } => format!("{label} · {}", layer_name(*target)),
        SettingsEdit::Text {
            id,
            label,
            target,
            entry,
            step,
            ..
        } => {
            if *id != SettingId::Actions {
                return format!("{label} · {}", layer_name(*target));
            }
            let name = visible(entry.as_deref().unwrap_or(""));
            let titles = match step {
                Some(ActionStep::Name) => vec!["New action · step 1 of 2: name".to_owned()],
                Some(ActionStep::Command) => vec![
                    format!("New action · step 2 of 2: command for {name}"),
                    format!("Step 2 of 2: command for {name}"),
                    format!("2/2: command for {name}"),
                ],
                None => vec![format!("Action {name}: command"), format!("{name}: command")],
            };
            titles
                .iter()
                .find(|title| js_len(title) <= width)
                .or(titles.last())
                .cloned()
                .unwrap_or_default()
        }
    }
}

/// TS: settingsPane.tsx WARNING_INDENT: an option's warning sits on its own lines under it (aligned with the label,
/// at most two), so it never cuts the path.
const WARNING_INDENT: usize = 4;

/// TS: settingsPane.tsx warningLines.
fn warning_lines(option: &ChoiceOption, inner: usize) -> Vec<String> {
    let Some(warning) = option.warning.as_deref().filter(|warning| !warning.is_empty()) else {
        return Vec::new();
    };
    let room = inner.saturating_sub(WARNING_INDENT).max(1);
    let lines = wrap_words(&visible(warning), room);
    let lines = if lines.len() > 2 {
        vec![lines[0].clone(), truncate(&lines[1..].join(" "), room)]
    } else {
        lines
    };
    let indent = " ".repeat(WARNING_INDENT.min(inner.saturating_sub(room)));
    lines.into_iter().map(|text| format!("{indent}{text}")).collect()
}

/// TS: settingsPane.tsx editRows: rows the edit control needs inside its box.
pub fn edit_rows(edit: &SettingsEdit, help: Option<&[DetailLine]>, inner: usize) -> usize {
    match edit {
        SettingsEdit::Choice { options, .. } => options
            .iter()
            .map(|option| 1 + warning_lines(option, inner).len())
            .sum(),
        SettingsEdit::Text { error, .. } => 1 + edit_message(error.as_deref(), help, inner).len(),
        _ => 1 + help.map_or(0, <[DetailLine]>::len),
    }
}

/// TS: settingsPane.tsx EditBox's rows (`bordered`: inside a box, else taking the body): the title, then the control,
/// cut to `rows` (less the border).
pub fn edit_box(
    edit: &SettingsEdit,
    help: Option<&[DetailLine]>,
    width: usize,
    rows: usize,
    bordered: bool,
) -> Vec<Row> {
    let border_rows = if bordered { 2 } else { 0 };
    let inner = width.saturating_sub(if bordered { 4 } else { 0 }).max(1);
    let mut content: Vec<Row> = Vec::new();
    match edit {
        SettingsEdit::Choice {
            options,
            index,
            current,
            ..
        } => {
            let label_width = options.iter().map(|option| js_len(&option.label)).max().unwrap_or(0);
            // Paths keep their tail (the end says where worktrees go); everything else is cut at the end.
            let room = width_of(num(inner) - 2 - 2 - num(label_width) - 2).max(1);
            let blocks: Vec<Vec<Row>> = options
                .iter()
                .enumerate()
                .map(|(position, option)| {
                    let detail = option
                        .detail
                        .as_deref()
                        .filter(|detail| !detail.is_empty())
                        .map(|detail| {
                            if option.path == Some(true) {
                                tail(&visible(detail), room)
                            } else {
                                truncate(&visible(detail), room)
                            }
                        });
                    let text = format!(
                        "{} {}{}",
                        if Some(position) == *current { '◉' } else { '○' },
                        js_pad_end(&option.label, label_width, " "),
                        detail.map_or_else(String::new, |detail| format!("  {detail}"))
                    );
                    let mut block = selectable_row(
                        &SelectableRowProps {
                            selected: position == *index,
                            text: &text,
                            color: (Some(position) == *current).then_some(THEME.active),
                            ..SelectableRowProps::default()
                        },
                        inner,
                    );
                    block.extend(
                        warning_lines(option, inner)
                            .iter()
                            .map(|text| text_row(text, TextStyle::fg(THEME.warn), inner)),
                    );
                    block
                })
                .collect();
            // When the options do not all fit, show a window that keeps the highlighted one (and its warning) visible.
            let free = rows.saturating_sub(border_rows + 1).max(1);
            let size = |from: usize, to: usize| -> usize {
                blocks
                    .iter()
                    .skip(from)
                    .take((to + 1).saturating_sub(from))
                    .map(Vec::len)
                    .sum()
            };
            let mut start = 0;
            while start < *index && size(start, *index) > free {
                start += 1;
            }
            let mut used = 0;
            for block in blocks.iter().skip(start) {
                if used > 0 && used + block.len() > free {
                    break;
                }
                content.extend(block.iter().cloned());
                used += block.len();
            }
        }
        SettingsEdit::Text {
            state,
            error,
            placeholder,
            ..
        } => {
            content.push(input_line(state, inner, placeholder.as_deref()));
            for line in edit_message(error.as_deref(), help, inner) {
                content.push(text_row(
                    &line.text,
                    ink_style(Some(line.color.unwrap_or(INK.muted))),
                    inner,
                ));
            }
        }
        SettingsEdit::Clear { target, .. } => {
            let text = if *target == ConfigTargetKind::Repository {
                "Removes it from deckhand.json; global defaults (or the built-in default) apply instead."
            } else {
                "Removes it from global defaults; the built-in default applies unless deckhand.json sets it."
            };
            content.extend(confirm_rows(text, help, inner));
        }
        SettingsEdit::DiscardLinks => {
            content.extend(confirm_rows(
                "Nothing has been saved. Enter discards your link changes.",
                help,
                inner,
            ));
        }
    }
    let mut body = vec![text_row(
        &truncate(&visible(&edit_title(edit, inner)), inner),
        TextStyle::fg(THEME.accent).bold(true),
        inner,
    )];
    body.extend(content);
    body.truncate(rows.saturating_sub(border_rows).max(1));
    body
}

/// The text of a confirmation, then its help lines (warnings unless they bring their own color).
fn confirm_rows(text: &str, help: Option<&[DetailLine]>, inner: usize) -> Vec<Row> {
    let mut rows = vec![text_row(&truncate(text, inner), TextStyle::fg(THEME.muted), inner)];
    for line in help.unwrap_or_default() {
        rows.push(text_row(
            &truncate(&visible(&line.text), inner),
            ink_style(Some(line.color.unwrap_or(INK.warn))),
            inner,
        ));
    }
    rows
}

/// TS: settingsPane.tsx editHint.
pub fn edit_hint(edit: &SettingsEdit) -> Vec<HintPart> {
    match edit {
        SettingsEdit::Choice { .. } => vec![HintPart::from("enter save"), HintPart::from("esc cancel")],
        SettingsEdit::Text { step, .. } => vec![
            hint_part("←→/home/end move", Some("←→ move"), Some(1)),
            HintPart::from(if *step == Some(ActionStep::Name) {
                "enter next"
            } else {
                "enter save"
            }),
            HintPart::from("esc cancel"),
        ],
        SettingsEdit::Clear { .. } => vec![HintPart::from("enter clear"), HintPart::from("esc keep")],
        SettingsEdit::DiscardLinks => vec![HintPart::from("enter discard"), HintPart::from("esc keep editing")],
    }
}

/// TS: settingsPane.tsx infoRows: rows a details box (border, no title) needs for `lines` at `width`.
pub fn info_rows(lines: &[DetailLine], width: usize) -> usize {
    let inner = width.saturating_sub(4).max(1);
    2 + lines
        .iter()
        .map(|line| {
            if line.nowrap == Some(true) || line.parts.is_some() {
                1
            } else {
                wrap_words(&line.text, inner).len()
            }
        })
        .sum::<usize>()
}

/// TS: settingsPane.tsx InfoBox's content rows: wrapped prose, cut values, at most `rows` rows (border included).
fn info_box(lines: &[DetailLine], width: usize, rows: usize) -> Vec<Row> {
    let inner = width.saturating_sub(4).max(1);
    let wrapped: Vec<DetailLine> = lines
        .iter()
        .flat_map(|line| {
            if line.parts.is_some() || line.nowrap == Some(true) {
                vec![line.clone()]
            } else {
                wrap_words(&line.text, inner)
                    .into_iter()
                    .map(|text| DetailLine {
                        text,
                        color: line.color,
                        ..DetailLine::default()
                    })
                    .collect()
            }
        })
        .collect();
    wrapped
        .iter()
        .take(rows.saturating_sub(2).max(1))
        .map(|line| detail_text(line, inner, None))
        .collect()
}

/// TS: settingsPane.tsx SettingsPane's per-view scroll (`useRef<Record<SettingsView, number>>`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SettingsScroll {
    pub main: usize,
    pub actions: usize,
    pub links: usize,
}

/// TS: settingsPane.tsx SettingsPaneProps (width and height are the area's).
#[derive(Clone, Copy, Debug)]
pub struct SettingsPaneProps<'a> {
    pub info: &'a SettingsInfo,
    pub view: SettingsView,
    pub row: usize,
    pub action_row: usize,
    /// The selected column: the layer the cursor's cell (and the sub-editors) edit.
    pub column: ConfigTargetKind,
    pub edit: Option<&'a SettingsEdit>,
    /// The edit control's help lines (computed by the flow).
    pub edit_help: Option<&'a [DetailLine]>,
    pub links: Option<&'a LinksState>,
    pub sizes: &'a IndexMap<String, Option<i64>>,
    pub notice: Option<&'a Notice>,
}

/// Global: which of your repositories a link would reach.
fn reach(candidate: &WorktreeCandidate, repos: &[String]) -> String {
    let having: &[String] = candidate.repos.as_deref().unwrap_or_default();
    let without: Vec<&str> = repos
        .iter()
        .filter(|repo| !having.contains(repo))
        .map(String::as_str)
        .collect();
    let first = if having.is_empty() {
        "in none of your repos yet".to_owned()
    } else {
        format!("in {}", having.join(", "))
    };
    let second = if !having.is_empty() && !without.is_empty() {
        format!(" · not in {} (skipped there)", without.join(", "))
    } else {
        String::new()
    };
    format!("{first}{second}")
}

/// The details of the link picker: the current candidate and any unsaved changes.
fn link_details(links: &LinksState) -> Vec<DetailLine> {
    let mut details = Vec::new();
    if let Some(current) = links.data.candidates.get(links.row) {
        let what = if current.configured == Some(CandidateConfigured::Files) {
            format!(
                "copied from {} by worktree.files (edit in raw JSON: E)",
                current.source.as_deref().unwrap_or("undefined")
            )
        } else if let (Some(repos), true) = (links.data.repos.as_deref(), current.added != Some(true)) {
            reach(current, repos)
        } else {
            let action = candidate_action(current);
            if !action.is_empty() {
                action
            } else if links.links.get(&current.path).copied().unwrap_or(false) {
                "linked".to_owned()
            } else {
                "not linked".to_owned()
            }
        };
        details.push(DetailLine {
            text: format!("{}: {what}", candidate_name(current)),
            ..DetailLine::default()
        });
    }
    if links.links.iter().ne(links.initial.iter()) {
        details.push(DetailLine {
            text: "Unsaved changes: Enter saves them, Esc discards.".to_owned(),
            color: Some(INK.warn),
            ..DetailLine::default()
        });
    }
    details
}

/// What sits under the list: the edit control, the details box or its single line.
enum Bottom {
    Edit { rows: Vec<Row>, bordered: bool },
    Info(Vec<Row>),
    Line(Row),
}

/// TS: settingsPane.tsx SettingsPane.
pub fn render_settings_pane(props: &SettingsPaneProps<'_>, scroll: &mut SettingsScroll, buf: &mut Buffer, area: Rect) {
    let info = props.info;
    let width = usize::from(area.width);
    let height = usize::from(area.height);
    let inner = width.saturating_sub(4).max(1);
    let header = header_lines(info);
    let layout = grid_layout(inner);
    let main = props.view == SettingsView::Main;
    // Border (2), title, header lines, the column headings (grid) and the hint line are fixed; the list and the box
    // below share the rest.
    let body = height
        .saturating_sub(2 + 1 + header.len() + usize::from(main) + 1)
        .max(1);
    let links = props.links.filter(|_| props.view == SettingsView::Links);
    let layer = links.map_or(props.column, |links| links.target);
    let layer_title = layer_name(layer);
    let title = match props.view {
        SettingsView::Main => format!("Settings · {}", info.repo),
        SettingsView::Actions => format!("Settings › Actions · {layer_title}"),
        SettingsView::Links => format!("Settings › Linked items · {layer_title}"),
    };
    let (state_text, state_color) = if main {
        repository_state(info)
    } else {
        (format!("saves to {}", layer_short(layer)), layer_color(layer))
    };
    let heading = truncate(
        &visible(&title),
        width_of(num(inner) - num(js_len(&state_text)) - 2).max(8),
    );
    let right = truncate(&state_text, width_of(num(inner) - num(js_len(&heading)) - 2));

    let selected;
    let mut lines: Vec<ListLine>;
    let mut details: Vec<DetailLine> = Vec::new();
    let mut hint: Vec<HintPart>;
    let scroll_slot: &mut usize;
    match props.view {
        SettingsView::Links if props.links.is_some() => {
            let Some(links) = props.links else { return };
            let home = info.vars.as_ref().map(|vars| vars.home.as_str());
            lines = link_lines(links, props.sizes, home, inner);
            selected = links.row;
            hint = vec![
                HintPart::from("space link/skip"),
                hint_part("a add a path", Some("a add"), None),
                HintPart::from("enter save"),
                hint_part("E raw JSON", Some("E JSON"), Some(1)),
                HintPart::from("esc cancel"),
            ];
            details = link_details(links);
            scroll_slot = &mut scroll.links;
        }
        SettingsView::Actions => {
            let (action_lines, value_width) = action_lines(info, props.column, props.action_row, inner);
            lines = action_lines;
            selected = props.action_row;
            details = action_details(info, props.column, props.action_row, value_width);
            hint = vec![
                HintPart::from("enter edit"),
                HintPart::from("a add"),
                hint_part("x remove", None, Some(1)),
                hint_part("E raw JSON", Some("E JSON"), Some(2)),
                hint_part("esc back", Some("esc"), None),
            ];
            scroll_slot = &mut scroll.actions;
        }
        _ => {
            let grid = settings_grid(info);
            let current = props.row.min(grid.len().saturating_sub(1));
            lines = grid_lines(&grid, current, props.column, layout);
            selected = current;
            if let Some(row) = grid.get(current) {
                details = grid_details(info, row, props.column, layout, inner.saturating_sub(4));
            }
            hint = vec![
                hint_part("←→ global/repo", Some("←→ column"), None),
                HintPart::from("enter edit"),
                hint_part("x clear", None, Some(3)),
                hint_part("E JSON", None, Some(1)),
                hint_part("T trust", None, Some(2)),
                HintPart::from("esc"),
            ];
            scroll_slot = &mut scroll.main;
        }
    }
    if let Some(notice) = props.notice {
        details.insert(
            0,
            DetailLine {
                text: notice.text.clone(),
                color: Some(if notice.error { INK.error } else { INK.warn }),
                ..DetailLine::default()
            },
        );
    }
    if details.is_empty() {
        details.push(DetailLine {
            text: " ".to_owned(),
            ..DetailLine::default()
        });
    }
    if let Some(edit) = props.edit {
        hint = edit_hint(edit);
    }

    // The edit control (or details box) sits under the list. When it does not fit beside at least one list row, the
    // edit control takes the whole body and the details box becomes one line.
    let list;
    let bottom;
    if let Some(edit) = props.edit {
        let needed = 2 + 1 + edit_rows(edit, props.edit_help, inner.saturating_sub(4));
        let bordered = body > needed;
        list = if bordered {
            lines.len().min(body - needed).max(1)
        } else {
            0
        };
        bottom = Bottom::Edit {
            rows: edit_box(
                edit,
                props.edit_help,
                inner,
                if bordered { needed } else { body },
                bordered,
            ),
            bordered,
        };
    } else {
        // The details box takes what it needs (a notice can make it three lines) while the list keeps three rows;
        // else one line.
        let rows = info_rows(&details, inner).min(2 + if props.notice.is_some() { 3 } else { 2 });
        if body >= rows + 3 {
            list = body - rows;
            bottom = Bottom::Info(info_box(&details, inner, rows));
        } else {
            list = body.saturating_sub(1).max(1);
            bottom = Bottom::Line(detail_text(&details[0], inner, None));
        }
    }
    if lines.len() > list {
        lines.retain(|line| !line.intro);
    }
    let shown = if list > 0 {
        window_lines(&lines, selected, list, scroll_slot)
    } else {
        Vec::new()
    };

    let content = bordered(buf, area, &Border::round(THEME.border_active), 1);
    let mut column = Column::new();
    let mut title_row = vec![Seg::new(&heading, TextStyle::fg(THEME.accent).bold(true))];
    if !right.is_empty() {
        let gap = width_of(num(inner) - num(js_len(&heading)) - num(js_len(&right))).max(2);
        title_row.push(Seg::new(
            format!("{}{right}", " ".repeat(gap)),
            TextStyle::fg(state_color),
        ));
    }
    column.push(truncate_row(&title_row, inner, Wrap::TruncateEnd));
    for line in &header {
        column.push(text_row(
            &truncate(&visible(&line.text), inner),
            TextStyle::fg(THEME.error),
            inner,
        ));
    }
    if main {
        column.push(truncate_row(
            &column_header(props.column, layout),
            inner,
            Wrap::TruncateEnd,
        ));
    }
    for line in &shown {
        if let Some(element) = &line.element {
            column.push(truncate_row(element, inner, Wrap::TruncateEnd));
        } else if line.head || line.status.is_some() {
            let status = line.status.as_ref().map(|(text, shade)| RowStatus {
                text: text.clone(),
                color: shade.and_then(ink_name),
            });
            let text = visible(&line.text);
            column.extend(selectable_row(
                &SelectableRowProps {
                    selected: line.selected,
                    text: &text,
                    parts: line.parts.as_deref(),
                    color: line.color,
                    status: status.as_ref(),
                    status_width: line.status_width,
                    ..SelectableRowProps::default()
                },
                inner,
            ));
        } else {
            let text = truncate(&visible(&line.text), inner);
            let style = TextStyle::default().color(line.color).bold(line.bold);
            column.push(text_row(if text.is_empty() { " " } else { &text }, style, inner));
        }
    }
    // The box is drawn over blank rows kept for it.
    let mut box_at = None;
    match &bottom {
        Bottom::Line(row) => column.push(row.clone()),
        Bottom::Edit { rows, bordered: false } => column.extend(rows.iter().cloned()),
        Bottom::Edit { rows, bordered: true } | Bottom::Info(rows) => {
            box_at = Some(column.top.len());
            column.extend(std::iter::repeat_n(Vec::new(), rows.len() + 2));
        }
    }
    column.grow();
    let hint_text = fit_hint(&hint, inner, " • ");
    column.push(text_row(&hint_text, TextStyle::fg(THEME.muted), inner));
    column.draw(buf, content);
    if let Some(offset) = box_at {
        let (rows, border) = match &bottom {
            Bottom::Edit { rows, .. } => (rows, Border::round(THEME.border_active)),
            Bottom::Info(rows) => (rows, Border::round(THEME.border)),
            Bottom::Line(_) => return,
        };
        let y = u32::from(content.y) + u32::try_from(offset).unwrap_or(u32::MAX);
        if let Ok(y) = u16::try_from(y)
            && y < content.bottom()
        {
            let wanted = u16::try_from(rows.len() + 2).unwrap_or(u16::MAX);
            let box_area = Rect::new(content.x, y, content.width, wanted.min(content.bottom() - y));
            let inside = bordered(buf, box_area, &border, 1);
            draw_rows(buf, inside, rows);
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Shared by the pane replays: a buffer as the styled lines parity/gen/*.gen.ts record.

    use ratatui::buffer::Buffer;
    use ratatui::style::Modifier;
    use unicode_width::UnicodeWidthStr;

    use super::ink_name;

    /// The buffer as styled lines: the text with `⟦style⟧` wherever the style changes (foreground name, `bold`, `dim`,
    /// `italic`, `inverse`). A space shows only its inverse, otherwise it continues the style before it, and trailing
    /// spaces without inverse are cut.
    pub fn styled_lines(buf: &Buffer) -> Vec<String> {
        let area = buf.area;
        (area.top()..area.bottom())
            .map(|y| {
                let mut cells: Vec<(String, String)> = Vec::new();
                let mut skip = 0;
                for x in area.left()..area.right() {
                    if skip > 0 {
                        skip -= 1;
                        continue;
                    }
                    let cell = &buf[(x, y)];
                    let symbol = if cell.symbol().is_empty() { " " } else { cell.symbol() };
                    skip = UnicodeWidthStr::width(symbol).saturating_sub(1);
                    let modifier = cell.modifier;
                    let inverse = modifier.contains(Modifier::REVERSED);
                    let previous = cells.last().map_or("", |(_, style)| style.as_str());
                    let style = if symbol != " " {
                        let flags = [
                            ink_name(cell.fg).unwrap_or(""),
                            if modifier.contains(Modifier::BOLD) { "bold" } else { "" },
                            if modifier.contains(Modifier::DIM) { "dim" } else { "" },
                            if modifier.contains(Modifier::ITALIC) {
                                "italic"
                            } else {
                                ""
                            },
                            if inverse { "inverse" } else { "" },
                        ];
                        flags
                            .iter()
                            .filter(|flag| !flag.is_empty())
                            .copied()
                            .collect::<Vec<_>>()
                            .join(" ")
                    } else if inverse {
                        "inverse".to_owned()
                    } else if previous.ends_with("inverse") {
                        String::new()
                    } else {
                        previous.to_owned()
                    };
                    cells.push((symbol.to_owned(), style));
                }
                while cells
                    .last()
                    .is_some_and(|(symbol, style)| symbol == " " && !style.ends_with("inverse"))
                {
                    cells.pop();
                }
                let mut out = String::new();
                let mut previous = String::new();
                for (symbol, style) in cells {
                    if style != previous {
                        out.push_str(&format!("⟦{style}⟧"));
                        previous = style;
                    }
                    out.push_str(&symbol);
                }
                out
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::styled_lines;
    use super::*;
    use deckhand_parity::{Case, replay};
    use serde_json::Value;

    /// An Ink color name as the static name the detail lines carry.
    fn static_color(name: &str) -> Option<&'static str> {
        [
            INK.accent,
            INK.accent_soft,
            INK.active,
            INK.muted,
            INK.success,
            INK.warn,
            INK.error,
        ]
        .into_iter()
        .find(|candidate| *candidate == name)
    }

    fn detail_lines(value: &Value) -> Vec<DetailLine> {
        value
            .as_array()
            .map(|lines| {
                lines
                    .iter()
                    .map(|line| DetailLine {
                        text: line["text"].as_str().unwrap_or_default().to_owned(),
                        color: line["color"].as_str().and_then(static_color),
                        nowrap: line["nowrap"].as_bool().filter(|nowrap| *nowrap),
                        parts: None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn kind(value: &Value) -> Result<ConfigTargetKind, String> {
        serde_json::from_value(value.clone()).map_err(|error| format!("target: {error}"))
    }

    fn edit(value: &Value) -> Result<Option<SettingsEdit>, String> {
        if value.is_null() {
            return Ok(None);
        }
        let id = || serde_json::from_value::<SettingId>(value["id"].clone()).map_err(|error| error.to_string());
        let label = || value["label"].as_str().unwrap_or_default().to_owned();
        let text = |key: &str| value[key].as_str().map(str::to_owned);
        Ok(Some(match value["kind"].as_str() {
            Some("choice") => SettingsEdit::Choice {
                id: id()?,
                label: label(),
                target: kind(&value["target"])?,
                options: serde_json::from_value(value["options"].clone()).map_err(|error| error.to_string())?,
                index: value["index"].as_u64().map_or(0, |index| index as usize),
                current: value["current"]
                    .as_i64()
                    .and_then(|current| usize::try_from(current).ok()),
            },
            Some("text") => SettingsEdit::Text {
                id: id()?,
                label: label(),
                target: kind(&value["target"])?,
                state: serde_json::from_value(value["state"].clone()).map_err(|error| error.to_string())?,
                error: text("error"),
                entry: text("entry"),
                step: match value["step"].as_str() {
                    Some("name") => Some(ActionStep::Name),
                    Some("command") => Some(ActionStep::Command),
                    _ => None,
                },
                placeholder: text("placeholder"),
            },
            Some("clear") => SettingsEdit::Clear {
                id: id()?,
                label: label(),
                target: kind(&value["target"])?,
                entry: text("entry"),
            },
            Some("discard-links") => SettingsEdit::DiscardLinks,
            other => return Err(format!("unknown edit kind {other:?}")),
        }))
    }

    fn links(value: &Value) -> Result<Option<LinksState>, String> {
        if value.is_null() {
            return Ok(None);
        }
        fn field<T: serde::de::DeserializeOwned>(value: &Value, key: &str) -> Result<T, String> {
            serde_json::from_value(value[key].clone()).map_err(|error| format!("{key}: {error}"))
        }
        Ok(Some(LinksState {
            data: field(value, "data")?,
            links: field(value, "links")?,
            initial: field(value, "initial")?,
            target: kind(&value["target"])?,
            row: value["row"].as_u64().map_or(0, |row| row as usize),
        }))
    }

    fn render(case: &Case) -> Result<Vec<String>, String> {
        let input: Value = case.arg_as(0)?;
        let info: SettingsInfo =
            serde_json::from_value(input["info"].clone()).map_err(|error| format!("info: {error}"))?;
        let width = input["width"].as_u64().map_or(80, |width| width as u16);
        let height = input["height"].as_u64().map_or(24, |height| height as u16);
        let view = match input["view"].as_str() {
            Some("actions") => SettingsView::Actions,
            Some("links") => SettingsView::Links,
            _ => SettingsView::Main,
        };
        let edit = edit(&input["edit"])?;
        let help = input.get("editHelp").map(detail_lines);
        let links = links(&input["links"])?;
        let sizes: IndexMap<String, Option<i64>> =
            serde_json::from_value(input["sizes"].clone()).map_err(|error| format!("sizes: {error}"))?;
        let notice = input["notice"].as_object().map(|notice| Notice {
            text: notice["text"].as_str().unwrap_or_default().to_owned(),
            error: notice.get("error").and_then(Value::as_bool).unwrap_or(false),
        });
        let props = SettingsPaneProps {
            info: &info,
            view,
            row: input["row"].as_u64().map_or(0, |row| row as usize),
            action_row: input["actionRow"].as_u64().map_or(0, |row| row as usize),
            column: kind(&input["column"])?,
            edit: edit.as_ref(),
            edit_help: help.as_deref(),
            links: links.as_ref(),
            sizes: &sizes,
            notice: notice.as_ref(),
        };
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        render_settings_pane(&props, &mut SettingsScroll::default(), &mut buf, area);
        Ok(styled_lines(&buf))
    }

    #[test]
    fn settings_pane_render_parity() {
        replay("U5", "settingsPane/render", render);
    }
}

#[cfg(test)]
mod helper_tests {
    //! The renders are replayed against Ink (fixtures); these cover what a fresh render cannot: the scroll position kept
    //! between renders, and the pure helpers' edges.

    use super::*;

    fn lines(count: usize) -> Vec<ListLine> {
        (0..count)
            .map(|index| ListLine {
                text: format!("line {index}"),
                row: Some(index),
                ..ListLine::default()
            })
            .collect()
    }

    fn texts(lines: &[ListLine]) -> Vec<&str> {
        lines.iter().map(|line| line.text.as_str()).collect()
    }

    #[test]
    fn window_lines_keeps_the_selected_row_in_view_with_markers() {
        let all = lines(10);
        let mut scroll = 0;
        let shown = window_lines(&all, 7, 5, &mut scroll);
        assert_eq!(
            texts(&shown),
            ["  ↑ 5 more", "line 5", "line 6", "line 7", "  ↓ 2 more"]
        );
        assert_eq!(scroll, 5);
        // The window stays while the selection moves inside it.
        let shown = window_lines(&all, 6, 5, &mut scroll);
        assert_eq!(texts(&shown)[1], "line 5");
        assert_eq!(scroll, 5);
        // Moving above it scrolls up just far enough.
        let shown = window_lines(&all, 3, 5, &mut scroll);
        assert_eq!(
            texts(&shown),
            ["  ↑ 3 more", "line 3", "line 4", "line 5", "  ↓ 4 more"]
        );
        // At the top there is one marker; at the end too.
        let shown = window_lines(&all, 0, 5, &mut scroll);
        assert_eq!(texts(&shown), ["line 0", "line 1", "line 2", "line 3", "  ↓ 6 more"]);
        let shown = window_lines(&all, 9, 5, &mut scroll);
        assert_eq!(texts(&shown), ["  ↑ 6 more", "line 6", "line 7", "line 8", "line 9"]);
        // Everything fits: no markers, scroll reset.
        let shown = window_lines(&all, 9, 10, &mut scroll);
        assert_eq!(shown.len(), 10);
        assert_eq!(scroll, 0);
        // Too short for markers: rows only.
        let shown = window_lines(&all, 8, 2, &mut scroll);
        assert_eq!(texts(&shown), ["line 7", "line 8"]);
    }

    #[test]
    fn window_lines_shows_the_section_heading_above_the_first_row_when_it_fits() {
        let mut all = lines(10);
        all.insert(
            5,
            ListLine {
                text: "Section".to_owned(),
                section: true,
                ..ListLine::default()
            },
        );
        let mut scroll = 0;
        // Row 5 is at index 6; its heading is index 5.
        let shown = window_lines(&all, 5, 5, &mut scroll);
        assert!(texts(&shown).contains(&"Section"), "{:?}", texts(&shown));
        assert!(texts(&shown).contains(&"line 5"));
    }

    #[test]
    fn format_size_rounds_halves_up_like_to_fixed() {
        assert_eq!(format_size(None, Some(CandidateKind::Dir)), "…");
        assert_eq!(format_size(Some(None), Some(CandidateKind::Dir)), "?");
        assert_eq!(format_size(Some(Some(5)), Some(CandidateKind::Symlink)), "");
        let size = |kib| format_size(Some(Some(kib)), Some(CandidateKind::File));
        assert_eq!(size(0), "0 KB");
        assert_eq!(size(1023), "1023 KB");
        assert_eq!(size(1024), "1.0 MB");
        assert_eq!(size(1280), "1.3 MB");
        assert_eq!(size(10239), "10.0 MB");
        assert_eq!(size(10240), "10 MB");
        assert_eq!(size(10752), "11 MB");
        assert_eq!(size(1_048_575), "1024 MB");
        assert_eq!(size(1_048_576), "1.0 GB");
        assert_eq!(size(1_310_720), "1.3 GB");
    }

    #[test]
    fn tail_keeps_the_end_of_a_path() {
        assert_eq!(tail("/a/b/c/d", 20), "/a/b/c/d");
        assert_eq!(tail("/a/b/c/d", 5), "…/c/d");
        assert_eq!(tail("/a/b/c/d", 1), "/");
        assert_eq!(tail("/a/b/c/d", 0), "");
    }

    #[test]
    fn visible_makes_controls_visible_and_flattens_newlines_and_tabs() {
        assert_eq!(visible("a\tb\nc\u{1b}d\u{7f}"), "a b c?d?");
    }

    #[test]
    fn wrap_chunks_breaks_at_separators_before_words() {
        assert_eq!(wrap_chunks("one two · three four", 100), ["one two · three four"]);
        assert_eq!(
            wrap_chunks("alpha beta · gamma delta", 12),
            ["alpha beta", "gamma delta"]
        );
    }

    #[test]
    fn grid_layout_follows_the_inner_width() {
        let wide = grid_layout(100);
        assert!(wide.two);
        assert_eq!(wide.label, label_width());
        assert_eq!(wide.cell, (100 - wide.label - 4 - 1) / 2);
        let narrow = grid_layout(40);
        assert!(!narrow.two);
        assert_eq!(narrow.cell, 40 - narrow.label - 4);
        assert_eq!(
            grid_layout(0),
            GridLayout {
                two: false,
                label: 8,
                cell: 4
            }
        );
    }

    #[test]
    fn input_line_scrolls_to_keep_the_cursor_and_survives_an_inner_cursor() {
        let state = EditorState {
            text: "abcdefghij".to_owned(),
            cursor: 10,
            select_all: None,
            message: None,
        };
        let row = input_line(&state, 8, None);
        assert_eq!(crate::primitives::row_text(&row), "› ghij ");
        // A cursor inside a multi-byte char does not panic.
        let state = EditorState {
            text: "é".to_owned(),
            cursor: 1,
            select_all: None,
            message: None,
        };
        let _ = input_line(&state, 8, Some("hint"));
    }
}
