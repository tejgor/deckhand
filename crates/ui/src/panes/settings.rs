//! Port of settingsPane.tsx: the Settings screen (C). The grid (one row per setting, one column per layer, the
//! cursor a cell), the Actions list and the link picker of one layer, each with a details box (or an inline edit
//! control) under it and one hint line. State and keys live in flows/settings.rs.

use indexmap::IndexMap;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use deckhand_core::display::{DetailLine, DetailPart, HintPart};
use deckhand_core::settings_model::{ChoiceOption, GridCell, GridRow, SettingId};
use deckhand_core::text_editor::EditorState;
use deckhand_protocol::data::{ConfigTargetKind, SettingsInfo, WorktreeCandidate, WorktreeCandidates};

use crate::primitives::Row;

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

/// TS: settingsPane.tsx visible: controls as `?`, newlines and tabs as spaces.
pub fn visible(text: &str) -> String {
    let _ = text;
    todo!("U5")
}

/// TS: settingsPane.tsx layerShort (`global defaults` / `this repo`).
pub fn layer_short(kind: ConfigTargetKind) -> &'static str {
    let _ = kind;
    todo!("U5")
}

/// TS: settingsPane.tsx LAYER_COLOR.
pub fn layer_color(kind: ConfigTargetKind) -> Color {
    let _ = kind;
    todo!("U5")
}

/// TS: settingsPane.tsx tail: paths keep their distinctive tail (`…` in front); `width` <= 1 cuts.
pub fn tail(text: &str, width: usize) -> String {
    let _ = (text, width);
    todo!("U5")
}

/// TS: settingsPane.tsx partsLine: a nowrap detail line of styled parts.
pub fn parts_line(parts: Vec<DetailPart>) -> DetailLine {
    let _ = parts;
    todo!("U5")
}

/// TS: settingsPane.tsx repositoryState: the header's right side (deckhand.json's trust state) and its color.
pub fn repository_state(info: &SettingsInfo) -> (String, Color) {
    let _ = info;
    todo!("U5")
}

/// TS: settingsPane.tsx headerLines: invalid-config notices under the title.
pub fn header_lines(info: &SettingsInfo) -> Vec<Notice> {
    let _ = info;
    todo!("U5")
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
    let _ = (lines, selected, rows, scroll);
    todo!("U5")
}

/// TS: settingsPane.tsx TWO_COLUMN_WIDTH: both layers' cells side by side from this inner width.
pub const TWO_COLUMN_WIDTH: usize = 64;

/// TS: settingsPane.tsx MAX_CELL.
pub const MAX_CELL: usize = 40;

/// TS: settingsPane.tsx LABEL_WIDTH: the longest setting label.
pub fn label_width() -> usize {
    todo!("U5")
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
    let _ = inner;
    todo!("U5")
}

/// TS: settingsPane.tsx cellParts: a cell's styled parts and whether its value was cut.
pub fn cell_parts(cell: &GridCell, width: usize, path: bool) -> (Vec<DetailPart>, bool) {
    let _ = (cell, width, path);
    todo!("U5")
}

/// TS: settingsPane.tsx gridRowElement: one grid row (label, then one or both cells).
pub fn grid_row(row: &GridRow, selected: bool, column: ConfigTargetKind, layout: GridLayout) -> Row {
    let _ = (row, selected, column, layout);
    todo!("U5")
}

/// TS: settingsPane.tsx ColumnHeader.
pub fn column_header(column: ConfigTargetKind, layout: GridLayout) -> Row {
    let _ = (column, layout);
    todo!("U5")
}

/// TS: settingsPane.tsx gridLines.
pub fn grid_lines(grid: &[GridRow], selected: usize, column: ConfigTargetKind, layout: GridLayout) -> Vec<ListLine> {
    let _ = (grid, selected, column, layout);
    todo!("U5")
}

/// TS: settingsPane.tsx gridDetails: the selected cell's details box lines.
pub fn grid_details(
    info: &SettingsInfo,
    row: &GridRow,
    column: ConfigTargetKind,
    layout: GridLayout,
    width: usize,
) -> Vec<DetailLine> {
    let _ = (info, row, column, layout, width);
    todo!("U5")
}

/// TS: settingsPane.tsx actionLines: the Actions view's lines and the value column's width.
pub fn action_lines(
    info: &SettingsInfo,
    column: ConfigTargetKind,
    selected: usize,
    inner: usize,
) -> (Vec<ListLine>, usize) {
    let _ = (info, column, selected, inner);
    todo!("U5")
}

/// TS: settingsPane.tsx actionDetails.
pub fn action_details(
    info: &SettingsInfo,
    column: ConfigTargetKind,
    selected: usize,
    value_width: usize,
) -> Vec<DetailLine> {
    let _ = (info, column, selected, value_width);
    todo!("U5")
}

/// TS: settingsPane.tsx formatSize (`12 KB`, `1.2 MB`, `?` while unknown).
pub fn format_size(kib: Option<Option<i64>>, kind: Option<deckhand_protocol::data::CandidateKind>) -> String {
    let _ = (kib, kind);
    todo!("U5")
}

/// TS: settingsPane.tsx candidateAction: what saving would do with a candidate.
pub fn candidate_action(candidate: &WorktreeCandidate) -> String {
    let _ = candidate;
    todo!("U5")
}

/// TS: settingsPane.tsx globalLinkLines: the global link picker's lines.
pub fn global_link_lines(state: &LinksState, inner: usize) -> Vec<ListLine> {
    let _ = (state, inner);
    todo!("U5")
}

/// TS: settingsPane.tsx linkLines: the link picker's lines (sizes per path; `home` for `~`).
pub fn link_lines(
    state: &LinksState,
    sizes: &IndexMap<String, Option<i64>>,
    home: Option<&str>,
    inner: usize,
) -> Vec<ListLine> {
    let _ = (state, sizes, home, inner);
    todo!("U5")
}

/// TS: settingsPane.tsx InputLine: a one-line text input with its cursor (or the placeholder).
pub fn input_line(state: &EditorState, width: usize, placeholder: Option<&str>) -> Row {
    let _ = (state, width, placeholder);
    todo!("U5")
}

/// TS: settingsPane.tsx wrapChunks: `text` in `width`-column chunks.
pub fn wrap_chunks(text: &str, width: usize) -> Vec<String> {
    let _ = (text, width);
    todo!("U5")
}

/// TS: settingsPane.tsx editTitle.
pub fn edit_title(edit: &SettingsEdit, width: usize) -> String {
    let _ = (edit, width);
    todo!("U5")
}

/// TS: settingsPane.tsx editRows: rows the edit control needs inside its box.
pub fn edit_rows(edit: &SettingsEdit, help: Option<&[DetailLine]>, inner: usize) -> usize {
    let _ = (edit, help, inner);
    todo!("U5")
}

/// TS: settingsPane.tsx EditBox's rows (`bordered`: inside a box, else taking the body).
pub fn edit_box(
    edit: &SettingsEdit,
    help: Option<&[DetailLine]>,
    width: usize,
    rows: usize,
    bordered: bool,
) -> Vec<Row> {
    let _ = (edit, help, width, rows, bordered);
    todo!("U5")
}

/// TS: settingsPane.tsx editHint.
pub fn edit_hint(edit: &SettingsEdit) -> Vec<HintPart> {
    let _ = edit;
    todo!("U5")
}

/// TS: settingsPane.tsx infoRows: rows the details box needs.
pub fn info_rows(lines: &[DetailLine], width: usize) -> usize {
    let _ = (lines, width);
    todo!("U5")
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

/// TS: settingsPane.tsx SettingsPane.
pub fn render_settings_pane(props: &SettingsPaneProps<'_>, scroll: &mut SettingsScroll, buf: &mut Buffer, area: Rect) {
    let _ = (props, scroll, buf, area);
    todo!("U5")
}
