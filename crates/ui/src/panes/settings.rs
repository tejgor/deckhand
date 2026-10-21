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
    ACTIONS_INTRO, COLUMNS, ChoiceOption, GridCell, GridRow, NEEDS_TRUST_DETAIL, SETTINGS, SettingControl, SettingId,
    SettingSection, cell_detail, column_titles, entry_count, entry_noun, home_path, layer_entries, layer_name,
    max_entries, settings_grid,
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

/// Not in the TS: the two steps of adding or editing a Dev process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProcessStep {
    /// Its command.
    Command,
    /// Its name, suggested from the command (or kept, when editing).
    Name,
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
    /// Not in the TS: a Dev process (Settings › Dev command): its command, then its name.
    Process {
        target: ConfigTargetKind,
        step: ProcessStep,
        /// The process edited; none while adding one.
        name: Option<String>,
        /// The command from step 1, while the name is asked.
        command: Option<String>,
        state: EditorState,
        error: Option<String>,
    },
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

/// Not in the TS: what Dev processes are, above their list.
pub const DEV_PROCESSES_INTRO: &str = "What Dev runs (d d): one command, or several processes side by side, each in its own split of the Dev tab. One layer's Dev replaces the other's whole.";

/// TS: settingsPane.tsx actionLines: the Actions view's lines and the value column's width (`id`: Actions, or the
/// Dev processes).
pub fn action_lines(
    info: &SettingsInfo,
    id: SettingId,
    column: ConfigTargetKind,
    selected: usize,
    inner: usize,
) -> (Vec<ListLine>, usize) {
    let actions = layer_entries(info, id, column);
    let dev = id == SettingId::DevCommand;
    let (own, context) = (&actions.own, &actions.context);
    let all: Vec<_> = own.iter().chain(context.iter()).collect();
    let names: Vec<&str> = all.iter().map(|action| action.name.as_str()).collect();
    let statuses: Vec<&str> = all
        .iter()
        .map(|action| action.status.as_deref().unwrap_or(""))
        .collect();
    let layout = list_columns(inner, &names, &statuses);
    // What actions are, above the list (it scrolls away with a long list).
    let intro = if dev { DEV_PROCESSES_INTRO } else { ACTIONS_INTRO };
    let mut lines: Vec<ListLine> = wrap_words(intro, inner.saturating_sub(1).max(1))
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
        text: format!(" + Add {}", if dev { "a process" } else { "action" }),
        row: Some(own.len()),
        head: true,
        color: Some(THEME.active),
        selected: selected == own.len(),
        ..ListLine::default()
    });
    if !context.is_empty() {
        lines.push(ListLine {
            text: match (dev, column) {
                (true, ConfigTargetKind::Repository) => {
                    " In effect here until this repo sets its own Dev, from global defaults:"
                }
                (true, ConfigTargetKind::Global) => " This repo has its own Dev (it replaces these there):",
                (false, ConfigTargetKind::Repository) => " Also in effect here, from global defaults:",
                (false, ConfigTargetKind::Global) => " This repo also has:",
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

/// TS: settingsPane.tsx actionDetails (`id`: Actions, or the Dev processes).
pub fn action_details(
    info: &SettingsInfo,
    id: SettingId,
    column: ConfigTargetKind,
    selected: usize,
    value_width: usize,
) -> Vec<DetailLine> {
    let actions = layer_entries(info, id, column);
    let noun = entry_noun(id);
    let Some(action) = actions.own.get(selected) else {
        let text = if id != SettingId::DevCommand {
            format!("a (or Enter) adds an {noun}: first its name, then its command.")
        } else {
            "Enter adds a process: its command, then a name (suggested from the command).".to_owned()
        };
        let mut lines = vec![DetailLine {
            text,
            ..DetailLine::default()
        }];
        let max = max_entries(id);
        if entry_count(&info.targets, id, column) >= max {
            lines.push(DetailLine {
                text: format!(
                    "{} has {max} {}, the most allowed: remove one (x) to add another.",
                    layer_short(column),
                    if id == SettingId::DevCommand {
                        "Dev processes"
                    } else {
                        "actions"
                    }
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
    } else if id == SettingId::DevCommand {
        "d d starts it with the other Dev processes · Enter edits its command and name".to_owned()
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
        SettingsEdit::Process { step, name, .. } => {
            let what = match name {
                Some(name) => format!("Dev process {}", visible(name)),
                None => "New Dev process".to_owned(),
            };
            let step = match step {
                ProcessStep::Command => "step 1 of 2: command",
                ProcessStep::Name => "step 2 of 2: name",
            };
            let titles = [format!("{what} · {step}"), capitalize_first(step)];
            titles
                .iter()
                .find(|title| js_len(title) <= width)
                .or(titles.last())
                .cloned()
                .unwrap_or_default()
        }
        SettingsEdit::Text {
            id,
            label,
            target,
            entry,
            step,
            ..
        } => {
            // The Dev command's processes (not in the TS) are titled like actions; the Dev command itself is not.
            let process = *id == SettingId::DevCommand && (entry.is_some() || step.is_some());
            if *id != SettingId::Actions && !process {
                return format!("{label} · {}", layer_name(*target));
            }
            let noun = entry_noun(*id);
            let name = visible(entry.as_deref().unwrap_or(""));
            let titles = match step {
                Some(ActionStep::Name) => vec![format!("New {noun} · step 1 of 2: name")],
                Some(ActionStep::Command) => vec![
                    format!("New {noun} · step 2 of 2: command for {name}"),
                    format!("Step 2 of 2: command for {name}"),
                    format!("2/2: command for {name}"),
                ],
                None => vec![
                    format!("{}{name}: command", if process { "Dev process " } else { "Action " }),
                    format!("{name}: command"),
                ],
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

/// `text` with its first character upper-cased.
fn capitalize_first(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
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
        SettingsEdit::Text { error, .. } | SettingsEdit::Process { error, .. } => {
            1 + edit_message(error.as_deref(), help, inner).len()
        }
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
        SettingsEdit::Process { state, error, .. } => {
            content.push(input_line(state, inner, None));
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
        SettingsEdit::Process { step, .. } => vec![
            hint_part("←→/home/end move", Some("←→ move"), Some(1)),
            HintPart::from(match step {
                ProcessStep::Command => "enter next",
                ProcessStep::Name => "enter save",
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
/// Lines can carry untrusted names and paths (a link candidate, its source), so their text is made `visible`.
fn info_box(lines: &[DetailLine], width: usize, rows: usize) -> Vec<Row> {
    let inner = width.saturating_sub(4).max(1);
    let wrapped: Vec<DetailLine> = lines
        .iter()
        .flat_map(|line| {
            if line.parts.is_some() {
                vec![line.clone()]
            } else if line.nowrap == Some(true) {
                vec![DetailLine {
                    text: visible(&line.text),
                    ..line.clone()
                }]
            } else {
                wrap_words(&visible(&line.text), inner)
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
    /// Not in the TS: the list the Actions view shows (Actions, or the Dev command's processes).
    pub list: SettingId,
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
        SettingsView::Actions if props.list == SettingId::DevCommand => {
            format!("Settings › Dev command · {layer_title}")
        }
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
            let id = props.list;
            let (action_lines, value_width) = action_lines(info, id, props.column, props.action_row, inner);
            lines = action_lines;
            selected = props.action_row;
            details = action_details(info, id, props.column, props.action_row, value_width);
            hint = if id == SettingId::DevCommand {
                vec![
                    HintPart::from("enter edit"),
                    hint_part("x remove", None, Some(1)),
                    hint_part("E raw JSON", Some("E JSON"), Some(2)),
                    hint_part("esc back", Some("esc"), None),
                ]
            } else {
                vec![
                    HintPart::from("enter edit"),
                    HintPart::from("a add"),
                    hint_part("x remove", None, Some(1)),
                    hint_part("E raw JSON", Some("E JSON"), Some(2)),
                    hint_part("esc back", Some("esc"), None),
                ]
            };
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
    //! Styled terminal buffers for pane snapshots and rendering assertions.

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
mod helper_tests {
    //! Scroll position across renders and the pure helpers' edge cases.

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
        assert!(grid_layout(64).two);
        assert!(!grid_layout(63).two);
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

#[cfg(test)]
mod scenario_tests {
    //! Named settings scenarios built from current Rust configuration rules.

    use super::test_support::styled_lines;
    use super::*;
    use deckhand_core::project_config::{
        ExplainContext, UserSettings, explain_settings, parse_project_config, trust_project_config,
    };
    use deckhand_core::settings_model::{choice_options, current_choice, initial_links};
    use deckhand_protocol::data::{
        ConfigTargets, LoadedProject, ProjectConfig, ProjectConfigDocument, RepositoryInfo, TemplateVars,
    };
    use deckhand_protocol::order::Ordered;
    use deckhand_testkit::assert_snapshot;
    use regex::Regex;
    use serde_json::{Value, json};

    const REPOSITORY_CONFIG: &str = r#"{"devCommand":"npm run dev","actions":{"test":"npm test"},"worktree":{"location":"{repoParent}/worktrees/{name}","files":{"backend/.env":"{repoParent}/b.env"}}}"#;

    fn global_defaults() -> Value {
        json!({"defaultAgent": "pi", "worktree": {"symlink": ["shared"], "branchFrom": "origin"}})
    }

    // Pure data only: the same explain_settings path as the daemon, with fixed synthetic paths and no I/O.
    fn settings_info(repo_raw: Option<&str>, defaults: Value, trusted: bool) -> SettingsInfo {
        let vars = TemplateVars {
            name: "<name>".to_owned(),
            repo: "mono".to_owned(),
            repo_parent: "/dev".to_owned(),
            repo_root: "/dev/mono".to_owned(),
            home: "/home/me".to_owned(),
        };
        let project = LoadedProject {
            root: "/dev/mono".to_owned(),
            trust_root: "/dev/mono".to_owned(),
            path: Some("/dev/mono/deckhand.json".to_owned()),
            config: repo_raw.map_or_else(
                || Ordered::new(ProjectConfig::default()),
                |raw| parse_project_config(raw, None).expect("valid scenario config"),
            ),
            exists: repo_raw.is_some(),
            fingerprint: "scenario".to_owned(),
            creation_hook: None,
            disabled_hook: None,
        };
        let mut user = UserSettings {
            defaults: Some(defaults.clone()),
            ..UserSettings::default()
        };
        if trusted {
            user.trusted_projects = Some(trust_project_config("/dev/mono", "scenario", None));
        }
        let rows = explain_settings(
            Some(&project),
            &user,
            &ExplainContext {
                vars: Some(vars.clone()),
                user: Some("me".to_owned()),
                config_dir: "/state".to_owned(),
                location_config_dir: Some("/state".to_owned()),
                ..ExplainContext::default()
            },
        )
        .rows;
        let document = |kind, raw: Option<String>| ProjectConfigDocument {
            kind,
            root: "/dev/mono".to_owned(),
            path: if kind == ConfigTargetKind::Global {
                "/home/me/.deckhand/config.json".to_owned()
            } else {
                "/dev/mono/deckhand.json".to_owned()
            },
            revision: raw.as_ref().map(|_| "scenario revision".to_owned()),
            exists: raw.is_some(),
            raw: raw.unwrap_or_else(|| "{}".to_owned()),
        };
        SettingsInfo {
            cwd: "/dev/mono".to_owned(),
            repo: "mono".to_owned(),
            repository: RepositoryInfo {
                state: match (repo_raw.is_some(), trusted) {
                    (false, _) => RepositoryState::Absent,
                    (true, false) => RepositoryState::Untrusted,
                    (true, true) => RepositoryState::Trusted,
                },
                path: Some("/dev/mono/deckhand.json".to_owned()),
                error: None,
            },
            needs_review: repo_raw.is_some() && !trusted,
            global_error: None,
            rows,
            targets: ConfigTargets {
                global: Some(document(ConfigTargetKind::Global, Some(defaults.to_string()))),
                repository: Some(document(ConfigTargetKind::Repository, repo_raw.map(str::to_owned))),
                ..ConfigTargets::default()
            },
            hook_file: None,
            vars: Some(vars),
            default_location: "/state/worktrees/<name>".to_owned(),
            inside_ignored: false,
            default_branch: None,
            origin_branch: None,
            user: "me".to_owned(),
            agent_hooks: None,
            notifications: false,
            codex_hooks: None,
            notification_permission: None,
            scroll_speed: None,
        }
    }

    fn pending_info() -> SettingsInfo {
        settings_info(Some(REPOSITORY_CONFIG), global_defaults(), false)
    }

    struct Pane<'a> {
        info: &'a SettingsInfo,
        view: SettingsView,
        row: usize,
        action_row: usize,
        column: ConfigTargetKind,
        edit: Option<SettingsEdit>,
        help: Option<Vec<DetailLine>>,
        links: Option<LinksState>,
        sizes: IndexMap<String, Option<i64>>,
        notice: Option<Notice>,
        width: u16,
        height: u16,
    }

    impl<'a> Pane<'a> {
        fn new(info: &'a SettingsInfo) -> Self {
            Pane {
                info,
                view: SettingsView::Main,
                row: 2,
                action_row: 0,
                column: ConfigTargetKind::Repository,
                edit: None,
                help: None,
                links: None,
                sizes: IndexMap::new(),
                notice: None,
                width: 100,
                height: 30,
            }
        }

        fn buffer(&self) -> Buffer {
            let props = SettingsPaneProps {
                info: self.info,
                view: self.view,
                row: self.row,
                action_row: self.action_row,
                list: SettingId::Actions,
                column: self.column,
                edit: self.edit.as_ref(),
                edit_help: self.help.as_deref(),
                links: self.links.as_ref(),
                sizes: &self.sizes,
                notice: self.notice.as_ref(),
            };
            let area = Rect::new(0, 0, self.width, self.height);
            let mut buf = Buffer::empty(area);
            render_settings_pane(&props, &mut SettingsScroll::default(), &mut buf, area);
            buf
        }

        fn lines(&self) -> Vec<String> {
            let marker = Regex::new("⟦[^⟧]*⟧").expect("valid regex");
            styled_lines(&self.buffer())
                .iter()
                .map(|line| marker.replace_all(line, "").into_owned())
                .collect()
        }
    }

    fn any_matches(lines: &[String], pattern: &str) -> bool {
        let regex = Regex::new(pattern).expect("valid regex");
        lines.iter().any(|line| regex.is_match(line))
    }

    fn any_contains(lines: &[String], text: &str) -> bool {
        lines.iter().any(|line| line.contains(text))
    }

    fn line_with<'a>(lines: &'a [String], text: &str) -> &'a str {
        lines
            .iter()
            .find(|line| line.contains(text))
            .unwrap_or_else(|| panic!("no line has {text:?} in {lines:#?}"))
    }

    fn matches(line: &str, pattern: &str) -> bool {
        Regex::new(pattern).expect("valid regex").is_match(line)
    }

    fn text_cell<'a>(buffer: &'a Buffer, text: &str) -> &'a ratatui::buffer::Cell {
        let symbols: Vec<String> = text.chars().map(|ch| ch.to_string()).collect();
        &buffer
            .content
            .windows(symbols.len())
            .find(|cells| cells.iter().zip(&symbols).all(|(cell, symbol)| cell.symbol() == symbol))
            .unwrap_or_else(|| panic!("no buffer cell starts {text:?}"))[0]
    }

    fn text_edit(id: SettingId, label: &str, target: ConfigTargetKind) -> SettingsEdit {
        SettingsEdit::Text {
            id,
            label: label.to_owned(),
            target,
            state: EditorState::default(),
            error: None,
            entry: None,
            step: None,
            placeholder: None,
        }
    }

    #[test]
    fn wide_grid_shows_both_layers_and_marks_pending_repository_values() {
        let info = pending_info();
        let wide = Pane::new(&info).lines();
        assert!(any_matches(&wide, r"Global \(all repos\) +This repo"));
        assert!(matches(
            line_with(&wide, "Dev command"),
            r"❯ Dev command +● dev \(built-in\) +npm run dev ⚠ needs trust"
        ));
        assert!(any_contains(
            &wide,
            "This repo · Applies once trusted — you'll be asked the first time it runs (or press T)"
        ));
        assert!(!matches(&wide.join("\n"), r"Saves to|until trusted|pending"));
        assert_snapshot(
            "fixtures/ui/settings/pending-grid.txt",
            &styled_lines(&Pane::new(&info).buffer()).join("\n"),
        );
    }

    #[test]
    fn narrow_grid_shows_the_selected_column_and_its_details() {
        let info = pending_info();
        // Narrow: only the selected column, with the indicator; ←→ (the flow) switches it.
        let narrow = Pane {
            width: 55,
            ..Pane::new(&info)
        }
        .lines();
        assert!(any_contains(&narrow, "◂ Global | This repo ▸"));
        assert!(matches(
            line_with(&narrow, "Dev command"),
            r"Dev command +npm run dev ⚠"
        ));
        assert!(!matches(&narrow.join("\n"), r"dev \(built-in\)"));
        let global = Pane {
            width: 55,
            column: ConfigTargetKind::Global,
            ..Pane::new(&info)
        }
        .lines();
        assert!(matches(
            line_with(&global, "Dev command"),
            r"Dev command +● dev \(built-in\)"
        ));
        assert!(any_contains(&global, "Built-in · No Dev command is set, so d runs"));
        assert_snapshot(
            "fixtures/ui/settings/narrow-global-column.txt",
            &styled_lines(
                &Pane {
                    width: 55,
                    height: 22,
                    column: ConfigTargetKind::Global,
                    ..Pane::new(&info)
                }
                .buffer(),
            )
            .join("\n"),
        );
    }

    #[test]
    fn selection_highlights_only_the_selected_layers_cell() {
        use ratatui::style::Modifier;

        let info = pending_info();
        for (column, selected, other) in [
            (ConfigTargetKind::Repository, "npm run dev", "dev (built-in)"),
            (ConfigTargetKind::Global, "dev (built-in)", "npm run dev"),
        ] {
            let buffer = Pane {
                column,
                ..Pane::new(&info)
            }
            .buffer();
            assert!(text_cell(&buffer, selected).modifier.contains(Modifier::REVERSED));
            assert!(!text_cell(&buffer, other).modifier.contains(Modifier::REVERSED));
        }
    }

    #[test]
    fn location_choices_show_warnings_below_the_full_path() {
        let info = pending_info();
        let options = choice_options(&info, SettingId::WorktreeLocation, ConfigTargetKind::Repository);
        for index in [0, 2] {
            let lines = Pane {
                row: 5,
                edit: Some(SettingsEdit::Choice {
                    id: SettingId::WorktreeLocation,
                    label: "Location".to_owned(),
                    target: ConfigTargetKind::Repository,
                    options: options.clone(),
                    index,
                    current: Some(1),
                }),
                ..Pane::new(&info)
            }
            .lines();
            assert!(any_contains(&lines, "Location · This repo"));
            let inside = lines
                .iter()
                .position(|line| line.contains("inside repo"))
                .expect("the inside repo option is shown");
            assert!(matches(&lines[inside], r"inside repo +/dev/mono/\.worktrees/<name> "));
            assert!(!lines[inside].contains('⚠'));
            assert!(matches(
                &lines[inside + 1],
                r"^\W+⚠ \.worktrees/ is not gitignored — worktrees would show up as untracked files"
            ));
        }
    }

    #[test]
    fn empty_text_editor_shows_its_layer_and_inherited_placeholder() {
        let info = pending_info();
        // An empty text input shows what applies without a value here.
        let empty = |edit| {
            Pane {
                column: ConfigTargetKind::Global,
                edit: Some(edit),
                ..Pane::new(&info)
            }
            .lines()
        };
        let mut devcommand = text_edit(SettingId::DevCommand, "Dev command", ConfigTargetKind::Global);
        if let SettingsEdit::Text { placeholder, .. } = &mut devcommand {
            *placeholder = Some("built-in: dev".to_owned());
        }
        assert!(any_matches(&empty(devcommand.clone()), r"Dev command · Global"));
        assert!(any_matches(&empty(devcommand), r"› +built-in: dev"));
    }

    #[test]
    fn new_action_editor_shows_name_and_command_steps_with_live_validation() {
        let info = pending_info();
        // Adding an action: numbered steps, the rules and examples, a live error; the Actions list is one layer's.
        let mut name = text_edit(SettingId::Actions, "New action", ConfigTargetKind::Repository);
        if let SettingsEdit::Text { step, state, .. } = &mut name {
            *step = Some(ActionStep::Name);
            *state = EditorState {
                text: " bad".to_owned(),
                cursor: 4,
                ..EditorState::default()
            };
        }
        let help = |text: &str, color| DetailLine {
            text: text.to_owned(),
            color,
            ..DetailLine::default()
        };
        let adding = Pane {
            view: SettingsView::Actions,
            action_row: 1,
            edit: Some(name),
            help: Some(vec![
                help("can't start with a space", Some("red")),
                help(
                    "letters, numbers, spaces, _ . - · starts with a letter or number · up to 48 characters",
                    None,
                ),
                help("e.g. test · lint frontend · db.migrate", None),
            ]),
            width: 55,
            ..Pane::new(&info)
        }
        .lines()
        .join("\n");
        for text in [
            "Settings › Actions · This repo",
            "New action · step 1 of 2: name",
            "can't start with a space",
            "letters, numbers, spaces",
            "e.g. test · lint frontend · db.migrate",
        ] {
            assert!(adding.contains(text), "{text}\n{adding}");
        }
        let mut command = text_edit(SettingId::Actions, "Action lint", ConfigTargetKind::Repository);
        if let SettingsEdit::Text { step, entry, .. } = &mut command {
            *step = Some(ActionStep::Command);
            *entry = Some("lint".to_owned());
        }
        let command = Pane {
            view: SettingsView::Actions,
            action_row: 1,
            edit: Some(command),
            width: 55,
            ..Pane::new(&info)
        }
        .lines()
        .join("\n");
        assert!(
            command.contains("New action · step 2 of 2: command for lint"),
            "{command}"
        );
    }

    #[test]
    fn scroll_speed_editor_shows_presets_and_the_stored_global_value() {
        let mut info = pending_info();
        info.scroll_speed = Some(2.0);
        let options = choice_options(&info, SettingId::ScrollSpeed, ConfigTargetKind::Global);
        let selected = current_choice(&info, SettingId::ScrollSpeed, ConfigTargetKind::Global, &options);
        let pane = Pane {
            width: 80,
            height: 24,
            row: SETTINGS
                .iter()
                .position(|setting| setting.id == SettingId::ScrollSpeed)
                .unwrap(),
            column: ConfigTargetKind::Global,
            edit: Some(SettingsEdit::Choice {
                id: SettingId::ScrollSpeed,
                label: "Scroll speed".to_owned(),
                target: ConfigTargetKind::Global,
                options,
                index: selected.index.unwrap(),
                current: selected.current,
            }),
            ..Pane::new(&info)
        };
        let text = pane.lines().join("\n");
        for expected in [
            "Scroll speed · Global",
            "0.5",
            "◉ 2",
            "custom…",
            "enter save",
            "esc cancel",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in\n{text}");
        }
        assert_snapshot(
            "fixtures/ui/settings/scroll-speed-editor.txt",
            &styled_lines(&pane.buffer()).join("\n"),
        );
    }

    #[test]
    fn trusted_repository_command_is_effective_without_a_trust_warning() {
        let info = settings_info(
            Some(r#"{"devCommand":"npm run dev"}"#),
            json!({"devCommand": "make dev"}),
            true,
        );
        let text = Pane::new(&info).lines();
        assert!(any_contains(&text, "deckhand.json: trusted ✓"));
        assert!(matches(
            line_with(&text, "Dev command"),
            r"Dev command +make dev +● npm run dev"
        ));
        assert!(any_matches(&text, r"This repo .*overrides global"));
        assert!(!any_contains(&text, "needs trust"));
    }

    /// No cell of `buffer` holds a control character (an escape sequence or a bell would reach the terminal).
    fn assert_no_control_characters(buffer: &Buffer) {
        for cell in &buffer.content {
            assert!(
                !cell.symbol().chars().any(char::is_control),
                "control character in cell {:?}",
                cell.symbol()
            );
        }
    }

    #[test]
    fn untrusted_values_with_control_characters_render_harmlessly() {
        // A repository's deckhand.json is untrusted input: an escape sequence or a bell in it must show as `?`.
        let info = settings_info(Some(r#"{"devCommand":"a\u001b[2Jb\u0007c"}"#), global_defaults(), false);
        let pane = Pane::new(&info);
        assert_no_control_characters(&pane.buffer());
        let text = pane.lines();
        assert!(matches(line_with(&text, "Dev command"), r"a\?\[2Jb\?c"));

        // The link picker: a candidate's own path and the source named on its details line.
        let candidates = vec![WorktreeCandidate {
            configured: Some(CandidateConfigured::Files),
            source: Some("/dev/sh\u{7}ared.env".to_owned()),
            ..candidate(".e\u{1b}nv", CandidateKind::File, CandidateSuggestion::Skip, "")
        }];
        let links = initial_links(&candidates, false);
        let picker = Pane {
            view: SettingsView::Links,
            links: Some(LinksState {
                data: WorktreeCandidates {
                    checkout: "/dev/mono".to_owned(),
                    candidates: candidates.into_iter().map(Ordered::new).collect(),
                    more_candidates: 0,
                    repos: None,
                    candidates_error: None,
                },
                initial: links.clone(),
                links,
                target: ConfigTargetKind::Repository,
                row: 0,
            }),
            ..Pane::new(&info)
        };
        assert_no_control_characters(&picker.buffer());
        let text = picker.lines();
        assert!(any_contains(&text, "[link] .e?nv"));
        // Its details line: shown as `?` like everywhere else, not silently dropped by the terminal buffer.
        assert!(any_contains(&text, ".e?nv: copied from /dev/sh?ared.env"));
    }

    #[test]
    fn invalid_documents_show_layer_specific_repair_messages() {
        let mut info = pending_info();
        info.repository.state = RepositoryState::Invalid;
        info.repository.error = Some("expected a JSON object".to_owned());
        info.global_error = Some("defaults.devCommand must be a string".to_owned());
        let text = Pane::new(&info).lines().join("\n");
        for expected in [
            "deckhand.json: invalid",
            "deckhand.json invalid: expected a JSON object (→ This repo, then e repairs it)",
            "Global defaults invalid: defaults.devCommand must be a string (← Global, then e repairs it)",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in\n{text}");
        }
    }

    #[test]
    fn save_failure_stays_visible_with_selection_and_hints_in_a_short_pane() {
        use ratatui::style::Modifier;

        let info = pending_info();
        let pane = Pane {
            height: 12,
            notice: Some(Notice {
                text: "Could not save settings: configuration changed on disk".to_owned(),
                error: true,
            }),
            ..Pane::new(&info)
        };
        let text = pane.lines().join("\n");
        for expected in [
            "Could not save settings: configuration changed on disk",
            "❯ Dev command",
            "enter edit",
            "esc",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in\n{text}");
        }
        let buffer = pane.buffer();
        assert_eq!(text_cell(&buffer, "Could not save settings").fg, THEME.error);
        assert!(text_cell(&buffer, "npm run dev").modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn empty_actions_still_offer_adding_an_action_in_the_selected_layer() {
        let info = settings_info(None, json!({}), false);
        let text = Pane {
            view: SettingsView::Actions,
            column: ConfigTargetKind::Global,
            ..Pane::new(&info)
        }
        .lines()
        .join("\n");
        for expected in ["Settings › Actions · Global", "+ Add action", "a add", "enter edit"] {
            assert!(text.contains(expected), "missing {expected:?} in\n{text}");
        }
    }

    #[test]
    fn clearing_a_setting_explains_which_layer_will_apply_instead() {
        let info = pending_info();
        for (column, explanation) in [
            (
                ConfigTargetKind::Repository,
                "Removes it from deckhand.json; global defaults (or the built-in default) apply instead.",
            ),
            (
                ConfigTargetKind::Global,
                "Removes it from global defaults; the built-in default applies",
            ),
        ] {
            let text = Pane {
                column,
                edit: Some(SettingsEdit::Clear {
                    id: SettingId::DevCommand,
                    label: "Dev command".to_owned(),
                    target: column,
                    entry: None,
                }),
                ..Pane::new(&info)
            }
            .lines()
            .join("\n");
            for expected in [explanation, "enter clear", "esc keep"] {
                assert!(text.contains(expected), "missing {expected:?} in\n{text}");
            }
        }
    }

    #[test]
    fn discarding_link_changes_explains_that_they_have_not_been_saved() {
        let info = pending_info();
        let text = Pane {
            view: SettingsView::Links,
            links: Some(LinksState {
                data: WorktreeCandidates {
                    checkout: "/dev/mono".to_owned(),
                    candidates: Vec::new(),
                    more_candidates: 0,
                    repos: None,
                    candidates_error: None,
                },
                links: IndexMap::from([("local.env".to_owned(), true)]),
                initial: IndexMap::new(),
                target: ConfigTargetKind::Repository,
                row: 0,
            }),
            edit: Some(SettingsEdit::DiscardLinks),
            ..Pane::new(&info)
        }
        .lines()
        .join("\n");
        for expected in [
            "Nothing has been saved. Enter discards your link changes.",
            "enter discard",
            "esc keep editing",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in\n{text}");
        }
    }

    fn candidate(path: &str, kind: CandidateKind, suggestion: CandidateSuggestion, reason: &str) -> WorktreeCandidate {
        WorktreeCandidate {
            path: path.to_owned(),
            kind,
            target: None,
            ignored: true,
            suggestion,
            reason: reason.to_owned(),
            configured: None,
            source: None,
            layers: None,
            repos: None,
            missing: None,
            added: None,
        }
    }

    fn repos(names: &[&str]) -> Option<Vec<String>> {
        Some(names.iter().map(|name| (*name).to_owned()).collect())
    }

    #[test]
    fn repository_link_picker_keeps_copies_linked_and_shows_sizes_and_missing_entries() {
        let info = pending_info();
        let candidates = vec![
            WorktreeCandidate {
                configured: Some(CandidateConfigured::Files),
                source: Some("/dev/shared.env".to_owned()),
                ..candidate(".env", CandidateKind::File, CandidateSuggestion::Skip, "")
            },
            candidate(
                "node_modules",
                CandidateKind::Dir,
                CandidateSuggestion::Link,
                "dependencies",
            ),
            WorktreeCandidate {
                configured: Some(CandidateConfigured::Symlink),
                missing: Some(true),
                ..candidate("missing.cfg", CandidateKind::File, CandidateSuggestion::Skip, "")
            },
            WorktreeCandidate {
                configured: Some(CandidateConfigured::Symlink),
                ..candidate("broken-link", CandidateKind::Symlink, CandidateSuggestion::Skip, "")
            },
        ];
        let mut links = initial_links(&candidates, false);
        // Copied files stay linked even if the selection map says otherwise; this picker cannot remove a copy.
        links.insert(".env".to_owned(), false);
        let text = Pane {
            view: SettingsView::Links,
            links: Some(LinksState {
                data: WorktreeCandidates {
                    checkout: "/dev/mono".to_owned(),
                    candidates: candidates.into_iter().map(Ordered::new).collect(),
                    more_candidates: 0,
                    repos: None,
                    candidates_error: None,
                },
                initial: links.clone(),
                links,
                target: ConfigTargetKind::Repository,
                row: 0,
            }),
            sizes: IndexMap::from([
                (".env".to_owned(), Some(4)),
                ("node_modules".to_owned(), Some(1280)),
                ("missing.cfg".to_owned(), None),
            ]),
            ..Pane::new(&info)
        }
        .lines();
        assert!(any_contains(&text, "Settings › Linked items · This repo"));
        assert!(matches(
            line_with(&text, "[link] .env"),
            r"\[link\] \.env +4 KB +files → /dev/shared.env"
        ));
        assert!(matches(
            line_with(&text, "[skip] node_modules/"),
            r"\[skip\] node_modules/ +1\.3 MB +suggested: dependencies"
        ));
        assert!(matches(
            line_with(&text, "missing.cfg"),
            r"\[link\] missing\.cfg +\? +missing in checkout"
        ));
        assert!(any_contains(&text, "[link] broken-link → (broken link)"));
        assert!(any_contains(
            &text,
            ".env: copied from /dev/shared.env by worktree.files (edit in raw JSON: E)"
        ));
        assert!(any_contains(&text, "space link/skip"));
        assert!(any_contains(&text, "enter save"));
    }

    // covers: B-U5-02 Settings pane, Linked items · Global: the global entries and what your repos share, counted per repo; nothing suggested is preselected
    #[test]
    fn settings_pane_linked_items_global() {
        let info = settings_info(None, global_defaults(), false);
        let candidates = vec![
            WorktreeCandidate {
                configured: Some(CandidateConfigured::Symlink),
                layers: Some(vec![ConfigTargetKind::Global]),
                repos: repos(&["mono", "site"]),
                ..candidate("shared", CandidateKind::File, CandidateSuggestion::Skip, "")
            },
            WorktreeCandidate {
                repos: repos(&["mono", "site", "tools"]),
                ..candidate(
                    "node_modules",
                    CandidateKind::Dir,
                    CandidateSuggestion::Link,
                    "dependencies",
                )
            },
            WorktreeCandidate {
                repos: repos(&["mono", "tools"]),
                ..candidate("local.cfg", CandidateKind::File, CandidateSuggestion::Skip, "")
            },
        ];
        let links = initial_links(&candidates, false);
        let expected: IndexMap<String, bool> = [("shared", true), ("node_modules", false), ("local.cfg", false)]
            .into_iter()
            .map(|(path, on)| (path.to_owned(), on))
            .collect();
        assert_eq!(links, expected);
        let state = LinksState {
            data: WorktreeCandidates {
                checkout: "/dev/mono".to_owned(),
                candidates: candidates.into_iter().map(Ordered::new).collect(),
                more_candidates: 0,
                repos: repos(&["mono", "site", "tools"]),
                candidates_error: None,
            },
            links: links.clone(),
            initial: links,
            target: ConfigTargetKind::Global,
            row: 1,
        };
        let pane = |links: LinksState| Pane {
            view: SettingsView::Links,
            row: 0,
            column: ConfigTargetKind::Global,
            links: Some(links),
            height: 20,
            ..Pane::new(&info)
        };
        let text = pane(state.clone()).lines().join("\n");
        for expected in [
            "Settings › Linked items · Global",
            "your 3 repos: mono, site, tools",
            "Linked in every repo",
            "[link] shared",
            "in 2 of 3 repos",
            "Found in several of your repos",
            "[skip] node_modules/",
            "in 3 of 3 repos",
            "suggested: dependencies",
            "node_modules/: in mono, site, tools",
            "a add a path",
        ] {
            assert!(text.contains(expected), "{expected}\n{text}");
        }
        // The details name where a link would be skipped.
        let second = pane(LinksState {
            row: 2,
            ..state.clone()
        })
        .lines()
        .join("\n");
        assert!(
            second.contains("local.cfg: in mono, tools · not in site (skipped there)"),
            "{second}"
        );
        // Only this repository known: it says the This repo column is the place for its files.
        let mut data = state.data.clone();
        data.candidates = Vec::new();
        data.repos = repos(&["mono"]);
        let alone = pane(LinksState {
            data,
            links: IndexMap::new(),
            initial: IndexMap::new(),
            row: 0,
            ..state
        })
        .lines()
        .join("\n");
        for expected in ["Nothing yet", "Deckhand knows only this repository"] {
            assert!(alone.contains(expected), "{expected}\n{alone}");
        }
    }
}
