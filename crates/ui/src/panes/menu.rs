//! Port of menu.tsx (the components; the pure helpers are in deckhand_core::display).
//!
//! The one selection style for every picker (a marker plus a full-width inverse, bold bar), picker lists that window
//! around the selection, the details box and the boxed picker screen (MenuPane). Components become pure row
//! builders (`*_rows`, unit-testable) plus `render_*` functions that draw them.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use deckhand_core::display::{
    DetailLine, DetailPart, HintPart, MenuItem, MenuPaneLine, RowStatus, SELECTION_MARKER, fit_hint, fit_parts,
    menu_details_rows as menu_details_row_count, menu_window_start, truncate, wrap_words,
};
use deckhand_core::jsstr::{js_len, js_pad_end};

use crate::primitives::{
    Border, Column, Row, Seg, TextStyle, Wrap, bordered, draw_rows, fit_row, row_width, truncate_row, with_base,
};
use crate::theme::{THEME, color};

/// An Ink color name as a ratatui color (unknown names draw uncolored, as chalk leaves them).
fn ink(name: Option<&str>) -> Option<Color> {
    name.and_then(color)
}

/// A segment of a nested `<Text>`: `color` unless selected, the part's own bold and italic.
fn part_seg(part: &DetailPart, selected: bool) -> Seg {
    let mut style = TextStyle::default().color(if selected { None } else { ink(part.color) });
    style.bold = part.bold;
    style.italic = part.italic;
    Seg::new(part.text.clone(), style)
}

/// A single-line `<Text wrap="truncate-end">` of `segs` under `base`, cut to `width` columns.
fn truncated_line(segs: Row, base: TextStyle, width: usize) -> Row {
    truncate_row(&with_base(segs, base), width, Wrap::TruncateEnd)
}

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
    let inner = width.saturating_sub(2).max(1);
    let selected = props.selected;
    let selected_color = props.selected_color.unwrap_or(THEME.active);
    let base = TextStyle {
        color: if selected { Some(selected_color) } else { props.color },
        inverse: Some(selected),
        bold: Some(selected),
        dim: Some(!selected && props.dim),
        ..TextStyle::default()
    };
    let marker = if selected { SELECTION_MARKER } else { " " };
    if props.wrap && props.status.is_none() && js_len(props.text) > inner {
        return wrap_words(props.text, inner)
            .iter()
            .enumerate()
            .flat_map(|(index, line)| {
                let lead = if index == 0 { marker } else { " " };
                let row = with_base(
                    vec![Seg::plain(format!("{lead} {}", js_pad_end(line, inner, " ")))],
                    base,
                );
                // A wide character can push the padded line past the box: Ink wraps it.
                if row_width(&row) > width {
                    fit_row(&row, width, Wrap::Wrap)
                } else {
                    vec![row]
                }
            })
            .collect();
    }
    let status_text = props.status.map_or("", |status| status.text.as_str());
    let tail = if status_text.is_empty() {
        String::new()
    } else if let Some(status_width) = props.status_width {
        let room = status_width.min(inner - 1);
        js_pad_end(&truncate(status_text, room), room, " ")
    } else {
        truncate(status_text, inner / 2)
    };
    let tail_len = js_len(&tail);
    let reserved = if tail.is_empty() { 0 } else { tail_len + 2 };
    let body = truncate(props.text, inner.saturating_sub(reserved).max(1));
    let body_len = js_len(&body);
    let min_gap = if tail.is_empty() { 0 } else { 2 };
    let gap = " ".repeat(inner.saturating_sub(body_len + tail_len).max(min_gap));
    let mut segs = vec![Seg::plain(format!("{marker} "))];
    match props.parts {
        Some(parts) => segs.extend(fit_parts(parts, body_len).iter().map(|part| part_seg(part, selected))),
        None => segs.push(Seg::plain(body)),
    }
    segs.push(Seg::plain(gap));
    if !tail.is_empty() {
        let status_color = if selected {
            None
        } else {
            ink(props.status.and_then(|status| status.color))
        };
        segs.push(Seg::new(tail, TextStyle::default().color(status_color)));
    }
    vec![truncated_line(segs, base, width)]
}

/// TS: menu.tsx SelectableCell: one grid cell, `parts` cut and padded to `width`; selected, the inverse bold bar.
pub fn selectable_cell(selected: bool, parts: &[DetailPart], width: usize, selected_color: Option<Color>) -> Row {
    let shown = fit_parts(parts, width);
    let used: usize = shown.iter().map(|part| js_len(&part.text)).sum();
    let base = TextStyle {
        color: if selected {
            Some(selected_color.unwrap_or(THEME.active))
        } else {
            None
        },
        inverse: Some(selected),
        bold: Some(selected),
        ..TextStyle::default()
    };
    let mut segs: Row = shown.iter().map(|part| part_seg(part, selected)).collect();
    segs.push(Seg::plain(" ".repeat(width.saturating_sub(used))));
    with_base(segs, base)
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
    let column = props.label_width.unwrap_or_else(|| {
        let longest = props.items.iter().map(|item| js_len(&item.label)).max().unwrap_or(0);
        longest.min(8.max(width / 3))
    });
    let limit = props.rows.map_or(props.items.len(), |rows| rows.max(1));
    // The ↑/↓ more markers take a row each, so they appear only when at least one item row remains beside them.
    let windowed = props.items.len() > limit && limit >= 3;
    let visible_rows = if windowed { limit - 2 } else { limit };
    let start = menu_window_start(props.selected, props.items.len(), visible_rows);
    let marker = |text: String| vec![Seg::new(text, TextStyle::fg(THEME.muted))];
    let mut rows = Vec::new();
    if windowed {
        rows.push(marker(if start > 0 {
            format!("  ↑ {start} more")
        } else {
            " ".to_owned()
        }));
    }
    for (offset, item) in props.items.iter().skip(start).take(visible_rows).enumerate() {
        // A description cut to a few words reads worse than none (the details box has the full text).
        let status_cost = match &item.status {
            Some(status) if !status.text.is_empty() => js_len(&status.text) as i64 + 2,
            _ => 0,
        };
        let room = width as i64 - 2 - column as i64 - 2 - status_cost;
        let label = match item.description.as_deref() {
            Some(description) if !description.is_empty() && (js_len(description) as i64 <= room || room >= 24) => {
                format!(
                    "{}  {description}",
                    js_pad_end(&truncate(&item.label, column), column, " ")
                )
            }
            _ => item.label.clone(),
        };
        rows.extend(selectable_row(
            &SelectableRowProps {
                selected: start + offset == props.selected,
                text: &label,
                parts: None,
                color: ink(item.color),
                selected_color: ink(item.selected_color),
                dim: item.dim.unwrap_or(false),
                status: item.status.as_ref(),
                status_width: None,
                wrap: false,
            },
            width,
        ));
    }
    if windowed {
        let hidden = props.items.len().saturating_sub(start + visible_rows);
        rows.push(marker(if hidden > 0 {
            format!("  ↓ {hidden} more")
        } else {
            " ".to_owned()
        }));
    }
    rows
}

/// TS: menu.tsx DetailText: one detail line, plain or its colored segments, cut to `width`.
pub fn detail_text(line: &DetailLine, width: usize, color: Option<Color>) -> Row {
    let text_color = match line.color {
        Some(name) => ink(Some(name)),
        None => color.or(Some(THEME.muted)),
    };
    let base = TextStyle::default().color(text_color);
    let segs: Row = match &line.parts {
        None => vec![Seg::plain(truncate(&line.text, width))],
        Some(parts) => fit_parts(parts, width)
            .iter()
            .map(|part| part_seg(part, false))
            .collect(),
    };
    truncated_line(segs, base, width)
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
    let inner = width.saturating_sub(4).max(1);
    let mut wrapped: Vec<DetailLine> = Vec::new();
    for line in props.lines {
        if line.parts.is_some() {
            wrapped.push(line.clone());
            continue;
        }
        let texts = if line.nowrap == Some(true) {
            vec![truncate(&line.text, inner)]
        } else {
            wrap_words(&line.text, inner)
        };
        wrapped.extend(texts.into_iter().map(|text| DetailLine {
            text,
            color: line.color,
            nowrap: None,
            parts: None,
        }));
    }
    let shown = props
        .rows
        .map_or(wrapped.len(), |rows| rows.saturating_sub(3).min(wrapped.len()));
    let title = vec![Seg::plain(truncate(props.title, inner))];
    let mut rows = vec![truncated_line(
        title,
        TextStyle::fg(THEME.accent_soft).bold(true),
        inner,
    )];
    rows.extend(wrapped.iter().take(shown).map(|line| detail_text(line, inner, None)));
    rows
}

/// TS: menu.tsx MenuDetails: the titled, bordered details box.
pub fn render_menu_details(props: &MenuDetailsProps<'_>, buf: &mut Buffer, area: Rect) {
    let rows = menu_details_rows(props, usize::from(area.width));
    // The box is as tall as its content (Ink sizes it), never past the area.
    let height = u16::try_from(rows.len() + 2).unwrap_or(u16::MAX).min(area.height);
    let box_area = Rect::new(area.x, area.y, area.width, height);
    let inner = bordered(buf, box_area, &Border::round(THEME.border), 1);
    draw_rows(buf, inner, &rows);
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
    // Border (2), title and hint rows are fixed.
    let free = height.saturating_sub(4).max(1);
    let head = subtitle.min(free - 1);
    // A blank row separates the subtitle from the menu when the menu keeps at least one row.
    let spacer = usize::from(head > 0 && free - head > 1);
    let menu_rows = items.max(1).min(free.saturating_sub(head + spacer)).max(1);
    // A blank row, the details box border (2) and its title, then as many lines as fit.
    let details_rows = if has_details {
        free.saturating_sub(head + spacer + menu_rows + 1)
    } else {
        0
    };
    MenuPaneBudget {
        head,
        spacer,
        menu_rows,
        details_rows,
        show_details: has_details && details_rows >= 4,
    }
}

/// TS: menu.tsx MenuPane: a boxed picker: title, subtitle, the menu, a details box and one hint line.
pub fn render_menu_pane(props: &MenuPaneProps<'_>, buf: &mut Buffer, area: Rect) {
    let inner = usize::from(area.width).saturating_sub(4).max(1);
    let budget = menu_pane_budget(
        props.subtitle.len(),
        props.items.len(),
        props.details.is_some(),
        usize::from(area.height),
    );
    let content = bordered(buf, area, &Border::round(THEME.border_active), 1);
    let mut column = Column::new();
    let title = vec![Seg::plain(truncate(props.title, inner))];
    column.push(truncated_line(title, TextStyle::fg(THEME.accent).bold(true), inner));
    for line in props.subtitle.iter().take(budget.head) {
        let text = vec![Seg::plain(truncate(&line.text, inner))];
        let line_color = match line.color {
            Some(name) => ink(Some(name)),
            None => Some(THEME.muted),
        };
        column.push(truncated_line(text, TextStyle::default().color(line_color), inner));
    }
    if budget.spacer > 0 {
        column.blank();
    }
    if props.items.is_empty() {
        let text = vec![Seg::plain(truncate(props.empty.unwrap_or("Nothing to choose."), inner))];
        column.push(truncated_line(text, TextStyle::fg(THEME.muted), inner));
    } else {
        column.extend(menu_list(
            &MenuListProps {
                items: props.items,
                selected: props.selected,
                rows: Some(budget.menu_rows),
                label_width: props.label_width,
            },
            inner,
        ));
    }
    let top_rows = column.top.len();
    column.grow();
    let hint = fit_hint(props.hint, inner, " • ");
    column.push(truncated_line(
        vec![Seg::plain(hint)],
        TextStyle::fg(THEME.muted),
        inner,
    ));
    column.draw(buf, content);
    if let (true, Some(details)) = (budget.show_details, props.details) {
        // A blank row, then the box, capped to the rows the budget left for it.
        let y = usize::from(content.y) + top_rows + 1;
        if let Ok(y) = u16::try_from(y)
            && y < content.bottom()
        {
            let cap = menu_details_row_count(&details.lines, inner).min(budget.details_rows);
            let box_props = MenuDetailsProps {
                title: &details.title,
                lines: &details.lines,
                rows: Some(cap),
            };
            let height = content.bottom() - y;
            render_menu_details(&box_props, buf, Rect::new(content.x, y, content.width, height));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::row_text;
    use deckhand_core::display::{fit_hint, menu_window_start, scrolled_list_top, wrap_words};
    use std::collections::HashSet;

    fn texts(rows: &[Row]) -> Vec<String> {
        rows.iter().map(|row| row_text(row)).collect()
    }

    fn hint(text: &str, short: Option<&str>, drop: Option<i32>) -> HintPart {
        HintPart {
            text: text.to_owned(),
            short: short.map(str::to_owned),
            drop,
        }
    }

    // covers: B-U1-01 fitHint keeps one line: full texts, then short forms, then drops the least important parts
    #[test]
    fn fit_hint_keeps_one_line() {
        let parts = [
            hint("↑↓ move", None, Some(2)),
            hint("enter/e open the file that sets it", Some("enter open"), None),
            "esc back".into(),
        ];
        assert_eq!(
            fit_hint(&parts, 80, " • "),
            "↑↓ move • enter/e open the file that sets it • esc back"
        );
        assert_eq!(fit_hint(&parts, 31, " • "), "↑↓ move • enter open • esc back");
        assert_eq!(fit_hint(&parts, 22, " • "), "enter open • esc back");
        assert!(js_len(&fit_hint(&parts, 5, " • ")) <= 5);
    }

    // covers: B-U1-02 fitHint can join with another separator
    #[test]
    fn fit_hint_can_join_with_another_separator() {
        let parts = [
            HintPart::from("o attach"),
            hint("f filter", None, Some(2)),
            HintPart::from("? help"),
        ];
        assert_eq!(fit_hint(&parts, 80, " • "), "o attach • f filter • ? help");
        assert_eq!(fit_hint(&parts, 20, " • "), "o attach • ? help");
    }

    // covers: B-U1-03 menuWindowStart keeps the selection visible
    #[test]
    fn menu_window_start_keeps_the_selection_visible() {
        assert_eq!(menu_window_start(0, 3, 5), 0);
        assert_eq!(menu_window_start(9, 10, 4), 6);
        assert_eq!(menu_window_start(5, 10, 4), 3);
    }

    // covers: B-U1-04 wrapWords wraps at spaces and cuts words longer than the width
    #[test]
    fn wrap_words_wraps_at_spaces_and_cuts_long_words() {
        assert_eq!(
            wrap_words("Applies to every repository and never needs trust.", 20),
            ["Applies to every", "repository and never", "needs trust."]
        );
        assert_eq!(wrap_words("abcdefgh", 3), ["abc", "def", "gh"]);
        assert_eq!(wrap_words("", 5), [""]);
    }

    // covers: B-U1-05 scrolledListTop: moving up onto a row shows the headings right above it, keeping the selection visible
    #[test]
    fn scrolled_list_top_shows_headings_above_the_selection() {
        // Rows 0 and 1 are headings (a group's name and its note's name), then selectable lines; 5 rows on screen.
        let headings: HashSet<usize> = [0, 1, 6].into_iter().collect();
        let selectable = |index: usize| !headings.contains(&index);
        // Scrolled down, then back up onto the first line: both headings come back (the reported bug kept only row 2).
        assert_eq!(scrolled_list_top(4, Some(2), 5, 12, &selectable), 0);
        // Up onto the first line of the second group: its heading (6) shows too.
        assert_eq!(scrolled_list_top(8, Some(7), 5, 12, &selectable), 6);
        // Never so far that the selection leaves the screen.
        assert_eq!(scrolled_list_top(4, Some(2), 2, 12, &selectable), 1);
        // Moving down scrolls just enough; the end of the list is not passed; no selection keeps the position.
        assert_eq!(scrolled_list_top(0, Some(7), 5, 12, &selectable), 3);
        assert_eq!(scrolled_list_top(20, Some(11), 5, 12, &selectable), 7);
        assert_eq!(scrolled_list_top(3, None, 5, 12, &selectable), 3);
        assert_eq!(scrolled_list_top(3, Some(1), 5, 3, &selectable), 0);
    }

    fn item(label: &str, description: Option<&str>) -> MenuItem {
        MenuItem {
            key: label.to_owned(),
            label: label.to_owned(),
            description: description.map(str::to_owned),
            ..MenuItem::default()
        }
    }

    #[test]
    fn selectable_row_marks_pads_and_right_aligns_the_status() {
        let status = RowStatus {
            text: "ok".to_owned(),
            color: Some("green"),
        };
        let props = SelectableRowProps {
            selected: true,
            text: "Open",
            status: Some(&status),
            ..SelectableRowProps::default()
        };
        let rows = selectable_row(&props, 12);
        assert_eq!(texts(&rows), ["❯ Open    ok"]);
        assert_eq!(rows[0][0].style.inverse, Some(true));
        let rows = selectable_row(
            &SelectableRowProps {
                selected: false,
                ..props
            },
            12,
        );
        assert_eq!(texts(&rows), ["  Open    ok"]);
        assert_eq!(rows[0].last().and_then(|seg| seg.style.color), Some(Color::Green));
        let cut = selectable_row(
            &SelectableRowProps {
                text: "a long label here",
                ..props
            },
            12,
        );
        assert_eq!(texts(&cut), ["❯ a lon…  ok"]);
    }

    #[test]
    fn selectable_row_wraps_onto_aligned_continuation_rows() {
        let props = SelectableRowProps {
            selected: true,
            text: "one two three four",
            wrap: true,
            ..SelectableRowProps::default()
        };
        assert_eq!(texts(&selectable_row(&props, 12)), ["❯ one two   ", "  three four"]);
    }

    #[test]
    fn selectable_cell_pads_to_the_cell() {
        let parts = [DetailPart {
            text: "abc".to_owned(),
            ..DetailPart::default()
        }];
        assert_eq!(row_text(&selectable_cell(true, &parts, 6, None)), "abc   ");
        assert_eq!(row_text(&selectable_cell(false, &parts, 2, None)), "a…");
    }

    #[test]
    fn menu_list_aligns_labels_and_windows_around_the_selection() {
        let items: Vec<MenuItem> = (0..10)
            .map(|index| item(&format!("item{index}"), Some("desc")))
            .collect();
        let rows = menu_list(
            &MenuListProps {
                items: &items,
                selected: 5,
                rows: Some(5),
                label_width: None,
            },
            30,
        );
        let lines = texts(&rows);
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], "  ↑ 4 more");
        assert_eq!(lines[1].trim_end(), "  item4  desc");
        assert_eq!(lines[2].trim_end(), "❯ item5  desc");
        assert_eq!(lines[4], "  ↓ 3 more");
    }

    #[test]
    fn menu_pane_budget_shrinks_details_first() {
        let tall = menu_pane_budget(1, 3, true, 30);
        assert_eq!(
            tall,
            MenuPaneBudget {
                head: 1,
                spacer: 1,
                menu_rows: 3,
                details_rows: 26 - 1 - 1 - 3 - 1,
                show_details: true,
            }
        );
        let short = menu_pane_budget(2, 9, true, 8);
        assert!(!short.show_details);
        assert_eq!(short.head, 2.min(3));
        assert_eq!(menu_pane_budget(0, 0, false, 0).menu_rows, 1);
    }

    #[test]
    fn menu_details_wrap_prose_and_cut_nowrap_lines() {
        let lines = [
            DetailLine {
                text: "alpha beta gamma delta".to_owned(),
                ..DetailLine::default()
            },
            DetailLine {
                text: "/very/long/path/value".to_owned(),
                nowrap: Some(true),
                ..DetailLine::default()
            },
        ];
        let props = MenuDetailsProps {
            title: "Title",
            lines: &lines,
            rows: None,
        };
        assert_eq!(
            texts(&menu_details_rows(&props, 16)),
            ["Title", "alpha beta", "gamma delta", "/very/long/…"]
        );
        let capped = MenuDetailsProps { rows: Some(5), ..props };
        assert_eq!(
            texts(&menu_details_rows(&capped, 16)),
            ["Title", "alpha beta", "gamma delta"]
        );
    }

    #[test]
    fn menu_pane_draws_title_menu_details_and_hint() {
        let items = [item("One", Some("first")), item("Two", None)];
        let details = MenuPaneDetails {
            title: "About".to_owned(),
            lines: vec![DetailLine {
                text: "Some words".to_owned(),
                ..DetailLine::default()
            }],
        };
        let hint = [HintPart::from("enter ok"), HintPart::from("esc back")];
        let props = MenuPaneProps {
            title: "Pick",
            subtitle: &[],
            items: &items,
            selected: 1,
            details: Some(&details),
            empty: None,
            hint: &hint,
            label_width: None,
        };
        let area = Rect::new(0, 0, 30, 12);
        let mut buf = Buffer::empty(area);
        render_menu_pane(&props, &mut buf, area);
        let line = |y: u16| -> String { (0..area.width).map(|x| buf[(x, y)].symbol().to_string()).collect() };
        assert!(line(1).starts_with("│ Pick"));
        assert!(line(2).contains("  One  first"));
        assert!(line(3).contains("❯ Two"));
        assert!(line(5).contains("╭"));
        assert!(line(6).contains("About"));
        assert!(line(7).contains("Some words"));
        assert!(line(10).contains("enter ok • esc back"));
        let mut empty = Buffer::empty(area);
        render_menu_pane(&MenuPaneProps { items: &[], ..props }, &mut empty, area);
        let empty_line: String = (0..area.width).map(|x| empty[(x, 2)].symbol().to_string()).collect();
        assert!(empty_line.contains("Nothing to choose."));
    }
}
