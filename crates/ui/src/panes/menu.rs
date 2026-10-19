//! Port of menu.tsx (the components; the pure helpers are in deckhand_core::display).
//!
//! The one selection style for every picker (a marker plus a full-width inverse, bold bar), picker lists that window
//! around the selection, the details box and the boxed picker screen (MenuPane). Components become pure row
//! builders (`*_rows`, unit-testable) plus `render_*` functions that draw them.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use deckhand_core::display::{DetailLine, DetailPart, HintPart, MenuItem, MenuPaneLine, RowStatus};

use crate::primitives::Row;

/// TS: menu.tsx SelectableRow props (`width` is the caller's; `selected_color` defaults to THEME.active).
#[derive(Clone, Copy, Debug, Default)]
pub struct SelectableRowProps<'a> {
    pub selected: bool,
    pub text: &'a str,
    /// Styled segments of `text` (their texts join to it), cut like it; colors apply unless selected.
    pub parts: Option<&'a [DetailPart]>,
    pub color: Option<Color>,
    pub selected_color: Option<Color>,
    pub dim: bool,
    pub status: Option<&'a RowStatus>,
    /// A fixed status column (left-aligned in it) instead of right-aligning the status.
    pub status_width: Option<usize>,
    /// Wrap long text onto aligned continuation rows instead of cutting it.
    pub wrap: bool,
}

/// TS: menu.tsx SelectableRow: one picker row (several when `wrap` and the text is long).
pub fn selectable_row(props: &SelectableRowProps<'_>, width: usize) -> Vec<Row> {
    let _ = (props, width);
    todo!("U1")
}

/// TS: menu.tsx SelectableCell: one grid cell, `parts` cut and padded to `width`; selected, the inverse bold bar.
pub fn selectable_cell(selected: bool, parts: &[DetailPart], width: usize, selected_color: Option<Color>) -> Row {
    let _ = (selected, parts, width, selected_color);
    todo!("U1")
}

/// TS: menu.tsx MenuList props.
#[derive(Clone, Copy, Debug, Default)]
pub struct MenuListProps<'a> {
    pub items: &'a [MenuItem],
    pub selected: usize,
    /// Only a window around the selection (with ↑/↓ more markers) when set.
    pub rows: Option<usize>,
    pub label_width: Option<usize>,
}

/// TS: menu.tsx MenuList: the aligned label column, then the description cut to fit.
pub fn menu_list(props: &MenuListProps<'_>, width: usize) -> Vec<Row> {
    let _ = (props, width);
    todo!("U1")
}

/// TS: menu.tsx DetailText: one detail line, plain or its colored segments, cut to `width`.
pub fn detail_text(line: &DetailLine, width: usize, color: Option<Color>) -> Row {
    let _ = (line, width, color);
    todo!("U1")
}

/// TS: menu.tsx MenuDetails props.
#[derive(Clone, Copy, Debug)]
pub struct MenuDetailsProps<'a> {
    pub title: &'a str,
    pub lines: &'a [DetailLine],
    /// The box's rows, border included (caps it).
    pub rows: Option<usize>,
}

/// TS: menu.tsx MenuDetails's content rows inside the border (title, then wrapped or cut lines).
pub fn menu_details_rows(props: &MenuDetailsProps<'_>, width: usize) -> Vec<Row> {
    let _ = (props, width);
    todo!("U1")
}

/// TS: menu.tsx MenuDetails: the titled, bordered details box.
pub fn render_menu_details(props: &MenuDetailsProps<'_>, buf: &mut Buffer, area: Rect) {
    let _ = (props, buf, area);
    todo!("U1")
}

/// TS: menu.tsx MenuPane `details`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuPaneDetails {
    pub title: String,
    pub lines: Vec<DetailLine>,
}

/// TS: menu.tsx MenuPane props.
#[derive(Clone, Copy, Debug)]
pub struct MenuPaneProps<'a> {
    pub title: &'a str,
    pub subtitle: &'a [MenuPaneLine],
    pub items: &'a [MenuItem],
    pub selected: usize,
    pub details: Option<&'a MenuPaneDetails>,
    /// Shown when there are no items (default "Nothing to choose.").
    pub empty: Option<&'a str>,
    pub hint: &'a [HintPart],
    pub label_width: Option<usize>,
}

/// How MenuPane budgets its rows (border, title and hint are fixed).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MenuPaneBudget {
    /// Subtitle lines shown.
    pub head: usize,
    /// A blank row between the subtitle and the menu.
    pub spacer: usize,
    pub menu_rows: usize,
    /// Rows left for the details box (shown only from 4).
    pub details_rows: usize,
    pub show_details: bool,
}

/// TS: menu.tsx MenuPane's row budget: the details box shrinks (or disappears) and the menu scrolls before anything
/// overflows `height`.
pub fn menu_pane_budget(subtitle: usize, items: usize, has_details: bool, height: usize) -> MenuPaneBudget {
    let _ = (subtitle, items, has_details, height);
    todo!("U1")
}

/// TS: menu.tsx MenuPane: a boxed picker: title, subtitle, the menu, a details box and one hint line.
pub fn render_menu_pane(props: &MenuPaneProps<'_>, buf: &mut Buffer, area: Rect) {
    let _ = (props, buf, area);
    todo!("U1")
}
