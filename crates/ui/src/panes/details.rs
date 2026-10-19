//! Port of detailsPane.tsx: a scrollable text pane with one hint line (the trust review, the workspace overview,
//! the DELETE override, the discard prompt). Text is shown exactly: tabs as four spaces, controls and invisible
//! characters (bidi overrides, zero-width) escaped as `<U+XXXX>`, rows wrapped by code points.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_core::display::{DISPLAY_CONTROL_PATTERN, HintPart, fit_hint, truncate};
use deckhand_core::jsstr::js_len;
use deckhand_core::key::Input;

use crate::primitives::{Border, Column, Row, Seg, TextStyle, Wrap, bordered, fit_row, truncate_row};
use crate::theme::THEME;

/// TS: detailsPane.tsx DETAILS_CHROME_ROWS: border (2) + title + hint rows around the body.
pub const DETAILS_CHROME_ROWS: usize = 4;

/// TS: detailsPane.tsx DETAILS_CHROME_COLUMNS: border (2) + paddingX (2).
pub const DETAILS_CHROME_COLUMNS: usize = 4;

/// TS: detailsPane.tsx INVISIBLE_PATTERN's characters (bidi controls, zero-width, line separators, BOM).
pub fn is_invisible(c: char) -> bool {
    matches!(
        c as u32,
        0x061C | 0x180E | 0x200B..=0x200F | 0x2028..=0x202E | 0x2060..=0x2064 | 0x2066..=0x2069 | 0xFEFF
    )
}

/// TS: detailsPane.tsx codePointEscape (`<U+200B>`).
pub fn code_point_escape(c: char) -> String {
    format!("<U+{:04X}>", c as u32)
}

/// TS: detailsPane.tsx visibleDetailText: never silently drop characters (details panes include the trust review).
pub fn visible_detail_text(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '\t' {
            result.push_str("    ");
        } else if is_invisible(c) || DISPLAY_CONTROL_PATTERN.is_match(c.encode_utf8(&mut [0; 4])) {
            result.push_str(&code_point_escape(c));
        } else {
            result.push(c);
        }
    }
    result
}

/// TS: detailsPane.tsx detailLines: wrapped by code points at `width` so surrogate pairs are never split; an empty
/// line is one space.
pub fn detail_lines(text: &str, width: usize) -> Vec<String> {
    let columns = width.max(1);
    let mut lines = Vec::new();
    for line in visible_detail_text(text).split('\n') {
        let characters: Vec<char> = line.chars().collect();
        if characters.is_empty() {
            lines.push(" ".to_owned());
            continue;
        }
        lines.extend(characters.chunks(columns).map(|chunk| chunk.iter().collect::<String>()));
    }
    lines
}

/// TS: detailsPane.tsx DetailsViewport.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DetailsViewport {
    pub lines: Vec<String>,
    pub rows: usize,
    pub max: usize,
}

/// TS: detailsPane.tsx detailsViewport (the pane's `width` × `height`, chrome included).
pub fn details_viewport(text: &str, width: usize, height: usize) -> DetailsViewport {
    let lines = detail_lines(text, width.saturating_sub(DETAILS_CHROME_COLUMNS).max(1));
    let rows = height.saturating_sub(DETAILS_CHROME_ROWS).max(1);
    let max = lines.len().saturating_sub(rows);
    DetailsViewport { lines, rows, max }
}

/// TS: detailsPane.tsx clampScroll with JS numbers (the fixtures include negative and fractional ones).
fn clamp_scroll_f64(scroll: f64, max: f64) -> f64 {
    0f64.max(max.min(scroll))
}

/// TS: detailsPane.tsx clampScroll.
pub fn clamp_scroll(scroll: usize, max: usize) -> usize {
    clamp_scroll_f64(scroll as f64, max as f64) as usize
}

/// TS: detailsPane.tsx scrollDetails: ↑↓ j k, PgUp/PgDn, Home/End; `None` when the key does not scroll (callers
/// handle it). `rows` and `max` are the viewport's.
pub fn scroll_details(scroll: usize, input: &Input, rows: usize, max: usize) -> Option<usize> {
    let key = &input.key;
    let current = clamp_scroll(scroll, max);
    let plain = |letter: &str| input.input == letter && !key.ctrl && !key.meta;
    if key.home {
        return Some(0);
    }
    if key.end {
        return Some(max);
    }
    if key.page_up {
        return Some(clamp_scroll(current.saturating_sub(rows), max));
    }
    if key.page_down {
        return Some(clamp_scroll(current.saturating_add(rows), max));
    }
    if key.up_arrow || plain("k") {
        return Some(clamp_scroll(current.saturating_sub(1), max));
    }
    if key.down_arrow || plain("j") {
        return Some(clamp_scroll(current.saturating_add(1), max));
    }
    None
}

/// TS: detailsPane.tsx DetailsPane props (`footer`: a plain string is one part).
#[derive(Clone, Copy, Debug)]
pub struct DetailsPaneProps<'a> {
    pub title: &'a str,
    pub text: &'a str,
    pub footer: &'a [HintPart],
    pub scroll: usize,
}

/// The pane's rows inside its border: the title (with the scroll position at its right), the visible body lines,
/// then, against the bottom, the hint line. `width` × `height` is the whole pane, chrome included.
pub fn details_pane_column(props: &DetailsPaneProps<'_>, width: usize, height: usize) -> Column {
    let DetailsViewport { lines, rows, max } = details_viewport(props.text, width, height);
    let start = clamp_scroll(props.scroll, max);
    let inner = width.saturating_sub(DETAILS_CHROME_COLUMNS).max(1);
    let scrollable = lines.len() > rows;
    // The position sits at the right of the title; the hint names the scroll keys unless the footer already does.
    let position = if scrollable {
        format!("{}/{}", lines.len().min(start + rows), lines.len())
    } else {
        String::new()
    };
    let mut parts = props.footer.to_vec();
    if scrollable && !parts.iter().any(|part| part.text.contains("scroll")) {
        parts.push(HintPart {
            text: "↑↓/PgUp/PgDn scroll".to_owned(),
            short: Some("↑↓ scroll".to_owned()),
            drop: Some(1),
        });
    }
    let hint = fit_hint(&parts, inner, " • ");
    let reserved = if position.is_empty() { 0 } else { position.len() + 2 };
    let heading = truncate(props.title, inner.saturating_sub(reserved).max(1));
    let mut title: Row = vec![Seg::new(heading.clone(), TextStyle::fg(THEME.accent).bold(true))];
    if !position.is_empty() {
        let gap = inner.saturating_sub(js_len(&heading) + position.len()).max(2);
        title.push(Seg::new(
            format!("{}{position}", " ".repeat(gap)),
            TextStyle::fg(THEME.muted),
        ));
    }
    let mut column = Column::new();
    column.push(truncate_row(&title, inner, Wrap::TruncateEnd));
    // A wide character can make a code-point-wrapped line wrap again; the body never takes the hint's row.
    let body = lines
        .iter()
        .skip(start)
        .take(rows)
        .flat_map(|line| fit_row(&[Seg::plain(line.clone())], inner, Wrap::Wrap))
        .take(rows);
    column.extend(body);
    column.grow();
    column.push(truncate_row(
        &[Seg::new(hint, TextStyle::fg(THEME.muted))],
        inner,
        Wrap::TruncateEnd,
    ));
    column
}

/// TS: detailsPane.tsx DetailsPane.
pub fn render_details_pane(props: &DetailsPaneProps<'_>, buf: &mut Buffer, area: Rect) {
    let content = bordered(buf, area, &Border::round(THEME.border_active), 1);
    details_pane_column(props, usize::from(area.width), usize::from(area.height)).draw(buf, content);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::row_text;
    use deckhand_core::key::Key;
    use deckhand_parity::{Case, replay, tags};
    use serde_json::json;

    fn width_arg(case: &Case, index: usize) -> Result<usize, String> {
        // Negative widths and heights behave as zero (every use clamps them up to one).
        Ok(usize::try_from(case.arg_i64(index)?.max(0)).unwrap_or(0))
    }

    #[test]
    fn visible_detail_text_parity() {
        replay(
            "U3",
            "detailsPane/visibleDetailText",
            |case: &Case| -> Result<String, String> { Ok(visible_detail_text(case.arg_str(0)?)) },
        );
    }

    #[test]
    fn detail_lines_parity() {
        replay(
            "U3",
            "detailsPane/detailLines",
            |case: &Case| -> Result<Vec<String>, String> { Ok(detail_lines(case.arg_str(0)?, width_arg(case, 1)?)) },
        );
    }

    #[test]
    fn details_viewport_parity() {
        replay(
            "U3",
            "detailsPane/detailsViewport",
            |case: &Case| -> Result<serde_json::Value, String> {
                let viewport = details_viewport(case.arg_str(0)?, width_arg(case, 1)?, width_arg(case, 2)?);
                Ok(json!({"lines": viewport.lines, "rows": viewport.rows, "max": viewport.max}))
            },
        );
    }

    #[test]
    fn clamp_scroll_parity() {
        replay("U3", "detailsPane/clampScroll", |case: &Case| -> Result<f64, String> {
            Ok(clamp_scroll_f64(case.arg_f64(0)?, case.arg_f64(1)?))
        });
    }

    #[test]
    fn scroll_details_parity() {
        replay(
            "U3",
            "detailsPane/scrollDetails",
            |case: &Case| -> Result<serde_json::Value, String> {
                let scroll = width_arg(case, 0)?;
                let key: Key = case.arg_as(2)?;
                let viewport: serde_json::Value = case.arg_as(3)?;
                let count = |name: &str| viewport.get(name).and_then(|value| value.as_u64()).unwrap_or(0) as usize;
                let input = Input::key(case.arg_str(1)?, key);
                Ok(match scroll_details(scroll, &input, count("rows"), count("max")) {
                    Some(next) => json!(next),
                    None => tags::undefined(),
                })
            },
        );
    }

    #[test]
    fn invisible_characters_are_shown_as_code_points() {
        assert_eq!(
            visible_detail_text("a\u{202E}b\u{200B}\tc\u{1}"),
            "a<U+202E>b<U+200B>    c<U+0001>"
        );
        assert_eq!(visible_detail_text("keep\nnewline"), "keep\nnewline");
    }

    #[test]
    fn lines_wrap_by_code_points_and_empty_lines_are_a_space() {
        assert_eq!(detail_lines("😀😀😀\n\nab", 2), vec!["😀😀", "😀", " ", "ab"]);
        assert_eq!(detail_lines("abc", 0), vec!["a", "b", "c"]);
    }

    #[test]
    fn pane_shows_the_position_and_the_scroll_hint_only_when_it_scrolls() {
        let text = (0..10).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
        let footer = [HintPart::from("Esc back")];
        let props = DetailsPaneProps {
            title: "Trust",
            text: &text,
            footer: &footer,
            scroll: 3,
        };
        let column = details_pane_column(&props, 30, 8);
        let title = row_text(&column.top[0]);
        assert!(title.starts_with("Trust") && title.ends_with("7/10"), "{title:?}");
        assert_eq!(column.top.len(), 1 + 4);
        assert_eq!(row_text(&column.top[1]), "line 3");
        assert!(row_text(&column.bottom[0]).contains("scroll"));
        let short = DetailsPaneProps { text: "one", ..props };
        let column = details_pane_column(&short, 30, 8);
        assert_eq!(row_text(&column.top[0]), "Trust");
        assert_eq!(row_text(&column.bottom[0]), "Esc back");
    }

    #[test]
    fn pane_renders_inside_its_border() {
        let footer = [HintPart::from("Esc back")];
        let props = DetailsPaneProps {
            title: "T",
            text: "hello",
            footer: &footer,
            scroll: 0,
        };
        let area = Rect::new(0, 0, 20, 6);
        let mut buf = Buffer::empty(area);
        render_details_pane(&props, &mut buf, area);
        let line = |y: u16| (0..20).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>();
        assert!(line(0).starts_with('╭'));
        assert!(line(1).contains(" T"));
        assert!(line(2).contains(" hello"));
        assert!(line(4).contains("Esc back"));
        assert!(line(5).starts_with('╰'));
    }
}
