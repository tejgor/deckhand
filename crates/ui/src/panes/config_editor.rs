//! Port of configEditorPane.tsx: the raw JSON editor of a config layer (E in Settings): one row per editor line, the
//! cursor inverse, scrolled only as far as needed to keep it on screen.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_core::display::{DISPLAY_CONTROL_PATTERN, HintPart, compact_path, fit_hint, truncate};
use deckhand_core::text_editor::{EditorState, editor_lines};
use deckhand_protocol::data::{ConfigTargetKind, ProjectConfigDocument};

use crate::primitives::{Border, Column, Row, Seg, TextStyle, Wrap, bordered, fit_row, truncate_row};
use crate::theme::THEME;

/// TS: configEditorPane.tsx visible: CR and tab as spaces, other controls and line separators `?`.
pub fn visible(text: &str) -> String {
    DISPLAY_CONTROL_PATTERN
        .replace_all(&text.replace('\r', " "), "?")
        .replace(['\u{2028}', '\u{2029}'], "?")
        .replace('\t', " ")
}

/// TS: configEditorPane.tsx scrollTopFor: scroll only as far as needed to keep the cursor row on screen.
pub fn scroll_top_for(previous: usize, cursor_row: usize, rows: usize, line_count: usize) -> usize {
    let mut top = previous;
    if cursor_row < top {
        top = cursor_row;
    } else if cursor_row >= top + rows {
        top = cursor_row + 1 - rows;
    }
    top.min(line_count.saturating_sub(rows))
}

/// TS: configEditorPane.tsx ConfigEditorPane props (width and height are the area's).
#[derive(Clone, Copy, Debug)]
pub struct ConfigEditorPaneProps<'a> {
    pub document: &'a ProjectConfigDocument,
    pub state: &'a EditorState,
    pub error: Option<&'a str>,
}

/// The text before `cursor` (a byte offset, rounded down to a char boundary).
fn before_cursor(state: &EditorState) -> &str {
    let mut end = state.cursor.min(state.text.len());
    while !state.text.is_char_boundary(end) {
        end -= 1;
    }
    &state.text[..end]
}

/// TS: configEditorPane.tsx ConfigEditorPane (`scroll_top`: its `useRef` scroll position, kept between renders).
pub fn render_config_editor_pane(
    props: &ConfigEditorPaneProps<'_>,
    scroll_top: &mut usize,
    buf: &mut Buffer,
    area: Rect,
) {
    let (document, state) = (props.document, props.state);
    let inner = usize::from(area.width).saturating_sub(4).max(1);
    let rows = usize::from(area.height).saturating_sub(6).max(1);
    let editor = editor_lines(state, inner);
    *scroll_top = scroll_top_for(*scroll_top, editor.cursor_row, rows, editor.lines.len());
    let start = *scroll_top;
    let dirty = state.text != document.raw;
    let line_number = before_cursor(state).split('\n').count();
    let selected = state.select_all == Some(true);

    let content = bordered(buf, area, &Border::round(THEME.border_active), 1);
    let mut column = Column::new();
    let kind = match document.kind {
        ConfigTargetKind::Global => "Global defaults · config.json \"defaults\"",
        ConfigTargetKind::Repository => "Repository config · deckhand.json",
    };
    let status = if !document.exists {
        " · new"
    } else if dirty {
        " · unsaved"
    } else {
        ""
    };
    column.push(truncate_row(
        &[Seg::new(
            truncate(&format!("{kind}{status}"), inner),
            TextStyle::fg(THEME.accent).bold(true),
        )],
        inner,
        Wrap::TruncateEnd,
    ));
    column.push(truncate_row(
        &[Seg::new(
            compact_path(&visible(&document.path), inner),
            TextStyle::fg(THEME.muted),
        )],
        inner,
        Wrap::TruncateEnd,
    ));
    for index in 0..rows {
        let line = editor.lines.get(index + start);
        let base = TextStyle::default().inverse(selected);
        // Ink drops an empty row, so a blank line renders as a space.
        let mut row: Row = Vec::new();
        match line {
            Some(line) if !line.before.is_empty() || line.cursor.is_some() || !line.after.is_empty() => {
                row.push(Seg::new(visible(&line.before), base));
            }
            _ => row.push(Seg::new(" ", base)),
        }
        if let Some(cursor) = line.and_then(|line| line.cursor.as_deref()) {
            row.push(Seg::new(
                visible(cursor),
                base.patch(TextStyle::default().inverse(true).bold(true)),
            ));
        }
        if let Some(line) = line {
            row.push(Seg::new(visible(&line.after), base));
        }
        // Ink wraps a row whose wide characters pass the width (the editor wraps by code points).
        column.extend(fit_row(&row, inner, Wrap::Wrap));
    }
    let problem = props.error.map(str::to_owned).or_else(|| state.message.clone());
    let note = match document.kind {
        ConfigTargetKind::Repository => {
            "Save validates JSON/schema and never runs anything; a trusted file stays trusted."
        }
        ConfigTargetKind::Global => "Save validates JSON/schema; it never runs commands.",
    };
    let shown = problem.as_deref().unwrap_or(note);
    let failing =
        props.error.is_some_and(|error| !error.is_empty()) || state.message.as_ref().is_some_and(|m| !m.is_empty());
    column.push(truncate_row(
        &[
            Seg::new(
                format!("Line {line_number}{} · ", if selected { " · all selected" } else { "" }),
                TextStyle::fg(THEME.muted),
            ),
            Seg::new(
                visible(shown),
                TextStyle::fg(if failing { THEME.error } else { THEME.muted }),
            ),
        ],
        inner,
        Wrap::TruncateEnd,
    ));
    let hint = [
        HintPart::from("Ctrl+S save"),
        HintPart::from("Ctrl+F format"),
        HintPart {
            text: "Ctrl+A select all".to_owned(),
            short: None,
            drop: Some(1),
        },
        HintPart {
            text: "arrows/home/end edit".to_owned(),
            short: None,
            drop: Some(3),
        },
        HintPart {
            text: "enter newline".to_owned(),
            short: None,
            drop: Some(2),
        },
        HintPart::from("esc cancel"),
    ];
    column.push(truncate_row(
        &[Seg::new(fit_hint(&hint, inner, " • "), TextStyle::fg(THEME.muted))],
        inner,
        Wrap::TruncateEnd,
    ));
    column.draw(buf, content);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panes::settings::test_support::styled_lines;
    use deckhand_parity::{Case, replay};
    use serde_json::Value;

    fn render(case: &Case) -> Result<Vec<String>, String> {
        let input: Value = case.arg_as(0)?;
        let document: ProjectConfigDocument =
            serde_json::from_value(input["document"].clone()).map_err(|error| format!("document: {error}"))?;
        let state: EditorState =
            serde_json::from_value(input["state"].clone()).map_err(|error| format!("state: {error}"))?;
        let width = input["width"].as_u64().map_or(80, |width| width as u16);
        let height = input["height"].as_u64().map_or(24, |height| height as u16);
        let props = ConfigEditorPaneProps {
            document: &document,
            state: &state,
            error: input["error"].as_str(),
        };
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        render_config_editor_pane(&props, &mut 0, &mut buf, area);
        Ok(styled_lines(&buf))
    }

    #[test]
    fn config_editor_pane_render_parity() {
        replay("U5", "configEditorPane/render", render);
    }

    #[test]
    fn visible_replaces_cr_tab_controls_and_line_separators() {
        assert_eq!(visible("a\rb\tc\u{1}d\u{7f}e\u{2028}f\u{2029}g"), "a b c?d?e?f?g");
        // A newline is kept (the editor lines never carry one).
        assert_eq!(visible("a\nb"), "a\nb");
    }

    #[test]
    fn scroll_top_for_scrolls_only_as_far_as_needed() {
        // Cursor above the window: it becomes the top.
        assert_eq!(scroll_top_for(10, 4, 5, 100), 4);
        // Cursor below: the cursor row is the last one shown.
        assert_eq!(scroll_top_for(0, 7, 5, 100), 3);
        // Inside: unchanged.
        assert_eq!(scroll_top_for(3, 5, 5, 100), 3);
        // Never past the last full window, never negative.
        assert_eq!(scroll_top_for(50, 99, 5, 100), 95);
        assert_eq!(scroll_top_for(9, 0, 5, 3), 0);
    }

    #[test]
    fn the_cursor_may_sit_inside_a_multibyte_char_without_panicking() {
        let document = ProjectConfigDocument {
            kind: ConfigTargetKind::Repository,
            root: "/r".to_owned(),
            path: "/r/deckhand.json".to_owned(),
            raw: String::new(),
            revision: None,
            exists: true,
        };
        let state = EditorState {
            text: "é😀".to_owned(),
            cursor: 2,
            select_all: None,
            message: None,
        };
        let props = ConfigEditorPaneProps {
            document: &document,
            state: &state,
            error: None,
        };
        let area = Rect::new(0, 0, 30, 10);
        render_config_editor_pane(&props, &mut 0, &mut Buffer::empty(area), area);
    }
}
