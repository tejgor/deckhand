//! The ? guide (a topic list beside the selected topic, each a table of key → what it does with a few notes; / searches
//! every topic; narrow panes show "◂ topic ▸" in the title instead of the list).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use serde::Serialize;

use deckhand_core::display::{
    DetailPart, HINT_SEPARATOR, HintPart, SELECTION_MARKER, THEME as INK_THEME, fit_hint, fit_parts, truncate,
    wrap_words,
};
use deckhand_core::help::{HELP_TOPICS, HelpLine, HelpTopic};
use deckhand_core::jsstr::{js_len, js_pad_end, js_pad_start, js_to_lower, js_trim};
use deckhand_core::key::Input;

use crate::primitives::{
    Border, Row, Seg, TextStyle, Wrap, bordered, draw_border, draw_rows, inset, truncate_row, with_base,
};
use crate::theme::{THEME, color};

/// TS: helpPane.tsx LIST_WIDTH: from this inner width the topic list is shown beside the content.
pub const LIST_WIDTH: usize = 64;

/// TS: helpPane.tsx MAX_KEY_WIDTH: a longer key gets its own row instead of widening the key column.
pub const MAX_KEY_WIDTH: usize = 15;

/// TS: helpPane.tsx Row: one rendered line as styled parts.
pub type HelpRow = Vec<DetailPart>;

/// A topic with only the lines that matched a search (TS `HelpTopic` objects built by searchHelp).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FoundTopic {
    pub title: &'static str,
    pub lines: Vec<HelpLine>,
}

/// Whether a line matches the lowercased `needle`: keys and description together, or a note; headings never.
fn matches(line: &HelpLine, needle: &str) -> bool {
    match line {
        HelpLine::Heading { .. } => false,
        HelpLine::Note { note } => js_to_lower(note).contains(needle),
        HelpLine::Key { keys, text } => js_to_lower(&format!("{keys} {text}")).contains(needle),
    }
}

/// TS: helpPane.tsx searchHelp: the topics with their lines matching `query` (case-insensitive), in order; headings
/// are dropped. `topics` defaults to HELP_TOPICS in TS.
pub fn search_help(query: &str, topics: &[HelpTopic]) -> Vec<FoundTopic> {
    let needle = js_to_lower(js_trim(query));
    if needle.is_empty() {
        return Vec::new();
    }
    topics
        .iter()
        .map(|topic| {
            let title_matches = js_to_lower(topic.title).contains(&needle);
            FoundTopic {
                title: topic.title,
                lines: topic
                    .lines
                    .iter()
                    .filter(|line| {
                        matches(line, &needle) || (title_matches && !matches!(line, HelpLine::Heading { .. }))
                    })
                    .copied()
                    .collect(),
            }
        })
        .filter(|topic| !topic.lines.is_empty())
        .collect()
}

fn part(text: impl Into<String>, color: Option<&'static str>, bold: bool) -> DetailPart {
    DetailPart {
        text: text.into(),
        color,
        bold: if bold { Some(true) } else { None },
        italic: None,
    }
}

fn plain_part(text: impl Into<String>) -> DetailPart {
    part(text, None, false)
}

/// TS: helpPane.tsx helpRows: a topic's lines at `width` (aligned key column, wrapped descriptions and notes).
///
/// Keys sit in one aligned, highlighted column (a key too long for it gets its own row); descriptions and notes wrap
/// with a hanging indent; a blank row separates notes from key rows.
pub fn help_rows(lines: &[HelpLine], width: usize) -> Vec<HelpRow> {
    // Sized by the keys that fit the column; a longer one gets its own row instead of widening every row.
    let key_width = lines
        .iter()
        .filter_map(|line| match line {
            HelpLine::Key { keys, .. } => Some(js_len(keys)),
            _ => None,
        })
        .filter(|length| *length <= MAX_KEY_WIDTH)
        .max()
        .unwrap_or(0)
        .max(1);
    let text_width = width.saturating_sub(key_width + 2).max(10);
    let mut rows: Vec<HelpRow> = Vec::new();
    #[derive(PartialEq, Clone, Copy)]
    enum Kind {
        Key,
        Note,
        Heading,
    }
    let mut previous: Option<Kind> = None;
    for line in lines {
        let kind = match line {
            HelpLine::Key { .. } => Kind::Key,
            HelpLine::Note { .. } => Kind::Note,
            HelpLine::Heading { .. } => Kind::Heading,
        };
        if !rows.is_empty() && (kind == Kind::Heading || (previous != Some(Kind::Heading) && previous != Some(kind))) {
            rows.push(vec![plain_part("")]);
        }
        previous = Some(kind);
        match line {
            HelpLine::Heading { heading } => {
                rows.push(vec![part(*heading, Some(INK_THEME.accent_soft), true)]);
            }
            HelpLine::Note { note } => {
                for text in wrap_words(note, width) {
                    rows.push(vec![part(text, Some(INK_THEME.muted), false)]);
                }
            }
            HelpLine::Key { keys, text } => {
                let mut text = wrap_words(text, text_width).into_iter();
                let indent = " ".repeat(key_width + 2);
                let active = Some(INK_THEME.active);
                if js_len(keys) > key_width {
                    rows.push(vec![part(*keys, active, true)]);
                } else {
                    rows.push(vec![
                        part(js_pad_end(keys, key_width + 2, " "), active, true),
                        plain_part(text.next().unwrap_or_default()),
                    ]);
                }
                for rest in text {
                    rows.push(vec![plain_part(indent.clone()), plain_part(rest)]);
                }
            }
        }
    }
    rows
}

/// TS: helpPane.tsx searchRows: each matching topic's numbered title, then its matching lines.
pub fn search_rows(query: &str, width: usize) -> Vec<HelpRow> {
    let found = search_help(query, HELP_TOPICS);
    if found.is_empty() {
        let trimmed = js_trim(query);
        let text = if trimmed.is_empty() {
            "Type to search every topic.".to_owned()
        } else {
            format!("Nothing matches \"{trimmed}\".")
        };
        return vec![vec![part(text, Some(INK_THEME.muted), false)]];
    }
    let mut rows = Vec::new();
    for (index, topic) in found.iter().enumerate() {
        if index > 0 {
            rows.push(vec![plain_part("")]);
        }
        let number = HELP_TOPICS
            .iter()
            .position(|entry| entry.title == topic.title)
            .map_or(0, |at| at + 1);
        rows.push(vec![part(
            format!("{number}  {}", topic.title),
            Some(INK_THEME.accent),
            true,
        )]);
        rows.extend(help_rows(&topic.lines, width));
    }
    rows
}

/// TS: helpPane.tsx HelpPane's `onLayout`: the scroll range and page size of the last render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HelpViewport {
    pub max: usize,
    pub page: usize,
}

impl Default for HelpViewport {
    fn default() -> Self {
        HelpViewport { max: 0, page: 1 }
    }
}

/// What a key did to the guide (TS handleInput's `'close'`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelpOutcome {
    Close,
}

/// TS: helpPane.tsx HelpView / useHelp: the guide's state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HelpView {
    pub topic: usize,
    pub scroll: usize,
    /// `None`: browsing topics; `Some`: search results (typed while `typing`).
    pub query: Option<String>,
    pub typing: bool,
    /// The last render's scroll range and page size, so keys can clamp.
    pub view: HelpViewport,
}

impl HelpView {
    /// TS: HelpView.open: on the first topic (or `topic`), not searching.
    pub fn open(&mut self, topic: Option<usize>) {
        self.topic = topic.unwrap_or(0);
        self.scroll = 0;
        self.query = None;
        self.typing = false;
    }

    /// TS: useHelp pick: topic `index`, wrapping around.
    pub fn pick(&mut self, index: isize) {
        let count = isize::try_from(HELP_TOPICS.len()).unwrap_or(isize::MAX).max(1);
        self.topic = usize::try_from(index.rem_euclid(count)).unwrap_or(0);
        self.scroll = 0;
    }

    /// TS: useHelp scrollBy.
    pub fn scroll_by(&mut self, delta: isize) {
        let max = self.view.max;
        let current = self.scroll.min(max);
        self.scroll = current.saturating_add_signed(delta).min(max);
    }

    /// TS: HelpView.handleInput: keys while the guide is open; `Close` when it should close.
    pub fn handle_input(&mut self, input: &Input) -> Option<HelpOutcome> {
        let key = &input.key;
        let text = input.input.as_str();
        let page = isize::try_from(self.view.page).unwrap_or(isize::MAX);
        // Not in the TS: J/K page too (a Mac keyboard has no page keys), unless the search takes letters.
        if key.page_down || (matches!(text, " " | "J") && !self.typing) {
            self.scroll_by(page);
            return None;
        }
        if key.page_up || (text == "K" && !self.typing) {
            self.scroll_by(-page);
            return None;
        }
        if let Some(query) = self.query.as_mut() {
            if key.escape {
                self.query = None;
                self.typing = false;
                self.scroll = 0;
                return None;
            }
            // Not in the TS: Ctrl+P / Ctrl+N and Ctrl+K / Ctrl+J scroll too, as the sidebar's search moves.
            let (up, down) = crate::app::modes::typing_moves(input);
            if up {
                self.scroll_by(-1);
                return None;
            }
            if down {
                self.scroll_by(1);
                return None;
            }
            if self.typing {
                if key.return_ || key.tab {
                    self.typing = false;
                } else if key.backspace || key.delete {
                    // `query.slice(0, -1)` drops one UTF-16 unit; a Rust string drops the whole character.
                    query.pop();
                    self.scroll = 0;
                } else if !text.is_empty() && !key.ctrl && !key.meta && !text.chars().any(|ch| ch <= '\u{1f}') {
                    query.push_str(text);
                    self.scroll = 0;
                }
                return None;
            }
            match text {
                "k" => self.scroll_by(-1),
                "j" => self.scroll_by(1),
                "/" => self.typing = true,
                "?" => return Some(HelpOutcome::Close),
                _ => {}
            }
            return None;
        }
        if key.escape || text == "?" {
            return Some(HelpOutcome::Close);
        }
        if text == "/" {
            self.query = Some(String::new());
            self.typing = true;
            self.scroll = 0;
            return None;
        }
        let topic = isize::try_from(self.topic).unwrap_or(0);
        if key.up_arrow || key.left_arrow || text == "k" || text == "h" || (key.tab && key.shift) {
            self.pick(topic - 1);
            return None;
        }
        if key.down_arrow || key.right_arrow || text == "j" || text == "l" || key.tab {
            self.pick(topic + 1);
            return None;
        }
        if key.home || text == "g" {
            self.pick(0);
            return None;
        }
        if key.end || text == "G" {
            self.pick(isize::try_from(HELP_TOPICS.len()).unwrap_or(1) - 1);
            return None;
        }
        if let [digit @ b'1'..=b'9'] = text.as_bytes() {
            let number = usize::from(digit - b'0');
            if number <= HELP_TOPICS.len() {
                self.pick(isize::try_from(number).unwrap_or(1) - 1);
                return None;
            }
        }
        if text == "0" && HELP_TOPICS.len() >= 10 {
            self.pick(9);
        }
        None
    }

    /// TS: HelpView.render: draws the guide and records the viewport for the next key.
    pub fn render(&mut self, buf: &mut Buffer, area: Rect) {
        let props = HelpPaneProps {
            topic: self.topic,
            scroll: self.scroll,
            query: self.query.as_deref(),
            typing: self.typing,
        };
        self.view = render_help_pane(&props, buf, area);
    }
}

/// TS: helpPane.tsx HelpPane props (width and height are the area's).
#[derive(Clone, Copy, Debug)]
pub struct HelpPaneProps<'a> {
    pub topic: usize,
    pub scroll: usize,
    pub query: Option<&'a str>,
    pub typing: bool,
}

fn ink(name: Option<&str>) -> Option<Color> {
    name.and_then(color)
}

/// One `<Text wrap="truncate-end">` of `segs` under `base`, cut to `width` columns.
fn line(segs: Row, base: TextStyle, width: usize) -> Row {
    truncate_row(&with_base(segs, base), width, Wrap::TruncateEnd)
}

/// A help row as styled segments, cut to `width` (TS `fitParts`); an all-empty row becomes one space so it keeps its
/// height.
fn content_row(row: &HelpRow, width: usize) -> Row {
    let mut segs: Row = fit_parts(row, width)
        .iter()
        .map(|piece| {
            let mut style = TextStyle::default().color(ink(piece.color));
            style.bold = piece.bold;
            Seg::new(piece.text.clone(), style)
        })
        .collect();
    if !row.is_empty() && row.iter().all(|piece| piece.text.is_empty()) {
        segs.push(Seg::plain(" "));
    }
    line(segs, TextStyle::default(), width)
}

fn hint(text: &str, short: Option<&str>, drop: Option<i32>) -> HintPart {
    HintPart {
        text: text.to_owned(),
        short: short.map(str::to_owned),
        drop,
    }
}

/// TS: helpPane.tsx HelpPane: draws the guide; returns what `onLayout` reported.
pub fn render_help_pane(props: &HelpPaneProps<'_>, buf: &mut Buffer, area: Rect) -> HelpViewport {
    let width = usize::from(area.width);
    let height = usize::from(area.height);
    let inner = width.saturating_sub(4).max(1);
    let list = inner >= LIST_WIDTH;
    // Marker, a two-digit number, a space, the title, then the list's right border and one column of air.
    let list_width = if list {
        HELP_TOPICS.iter().map(|entry| js_len(entry.title)).max().unwrap_or(0) + 6
    } else {
        0
    };
    // The list's right border (1) and a gap (1) separate it from the content.
    let content_width = inner.saturating_sub(if list { list_width + 2 } else { 0 }).max(10);
    let topic = props.topic.min(HELP_TOPICS.len().saturating_sub(1));
    let current = &HELP_TOPICS[topic];
    let query = props.query;
    let searching = query.is_some();
    let rows = match query {
        Some(query) => search_rows(query, content_width),
        None => help_rows(current.lines, content_width),
    };
    // Border (2), title, hint; the search line when searching.
    let page = height.saturating_sub(4 + if searching { 2 } else { 0 }).max(1);
    let max = rows.len().saturating_sub(page);
    let start = props.scroll.min(max);
    let viewport = HelpViewport { max, page };

    let position = if max > 0 {
        format!(
            "{}{}/{}",
            if start + page < rows.len() { "↓ " } else { "" },
            rows.len().min(start + page),
            rows.len()
        )
    } else {
        String::new()
    };
    let title = if searching {
        "Help · search".to_owned()
    } else if list {
        format!("Help · {}", current.title)
    } else {
        format!("Help · ◂ {} ({}/{}) ▸", current.title, topic + 1, HELP_TOPICS.len())
    };
    let heading = truncate(
        &title,
        inner
            .saturating_sub(if position.is_empty() { 0 } else { js_len(&position) + 2 })
            .max(1),
    );
    let hints: Vec<HintPart> = if searching {
        if props.typing {
            vec![
                "type to search".into(),
                hint("↑↓ scroll", None, Some(1)),
                "enter done".into(),
                "esc clear".into(),
            ]
        } else {
            vec![
                hint("↑↓ J/K scroll", Some("↑↓ scroll"), None),
                "/ edit search".into(),
                "esc clear".into(),
                "? close".into(),
            ]
        }
    } else {
        let mut hints = vec![
            hint(if list { "↑↓ topic" } else { "↑↓/←→ topic" }, Some("↑↓ topic"), None),
            hint("1-9 jump", None, Some(2)),
        ];
        if max > 0 {
            hints.push(hint("J/K/space scroll", Some("J/K scroll"), Some(1)));
        }
        hints.push("/ search".into());
        hints.push("esc close".into());
        hints
    };

    let body = bordered(buf, area, &Border::round(THEME.border_active), 1);
    if body.width == 0 || body.height == 0 {
        return viewport;
    }
    // The title line: the heading, and the scroll position flush right.
    let mut title_row = vec![Seg::new(heading.clone(), TextStyle::fg(THEME.accent).bold(true))];
    if !position.is_empty() {
        let gap = inner.saturating_sub(js_len(&heading) + js_len(&position)).max(2);
        title_row.push(Seg::new(
            format!("{}{position}", " ".repeat(gap)),
            TextStyle::fg(THEME.muted),
        ));
    }
    draw_rows(buf, body, &[line(title_row, TextStyle::default(), inner)]);
    // The hint line at the bottom.
    let hint_text = fit_hint(&hints, inner, HINT_SEPARATOR);
    let hint_row = line(
        vec![Seg::new(hint_text, TextStyle::fg(THEME.muted))],
        TextStyle::default(),
        inner,
    );
    let hint_area = Rect::new(body.x, body.bottom().saturating_sub(1), body.width, 1);
    if body.height >= 2 {
        draw_rows(buf, hint_area, &[hint_row]);
    }
    // Between them: the topic list beside the content.
    let middle = inset(body, 0, 0, 1, 1);
    if middle.height == 0 {
        return viewport;
    }
    let mut content_area = middle;
    if list {
        let list_area = Rect::new(
            middle.x,
            middle.y,
            u16::try_from(list_width).unwrap_or(u16::MAX).min(middle.width),
            middle.height,
        );
        let list_border = Border {
            top: false,
            bottom: false,
            left: false,
            ..Border::single(THEME.border)
        };
        let list_inner = draw_border(buf, list_area, &list_border);
        let found: Option<Vec<&str>> = query.map(|query| {
            search_help(query, HELP_TOPICS)
                .iter()
                .map(|topic| topic.title)
                .collect()
        });
        let label_width = list_width.saturating_sub(6).max(1);
        let list_rows: Vec<Row> = HELP_TOPICS
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let selected = !searching && index == topic;
                let dim = found.as_ref().is_some_and(|found| !found.contains(&entry.title));
                let base = TextStyle {
                    color: if selected {
                        Some(THEME.active)
                    } else if dim {
                        Some(THEME.muted)
                    } else {
                        None
                    },
                    bold: Some(selected),
                    inverse: Some(selected),
                    ..TextStyle::default()
                };
                let text = format!(
                    "{}{} {}",
                    if selected { SELECTION_MARKER } else { " " },
                    js_pad_start(&(index + 1).to_string(), 2, " "),
                    js_pad_end(&truncate(entry.title, label_width), label_width, " ")
                );
                line(vec![Seg::plain(text)], base, usize::from(list_inner.width))
            })
            .collect();
        draw_rows(buf, list_inner, &list_rows);
        // marginRight={1}: the content starts one column after the list's border.
        let used = list_width + 1;
        content_area = inset(middle, u16::try_from(used).unwrap_or(u16::MAX), 0, 0, 0);
    }
    let content_area = Rect::new(
        content_area.x,
        content_area.y,
        content_area.width.min(u16::try_from(content_width).unwrap_or(u16::MAX)),
        content_area.height,
    );
    let mut lines: Vec<Row> = Vec::new();
    if let Some(query) = query {
        let shown = truncate(query, content_width.saturating_sub(4).max(1));
        let mut search = vec![Seg::new("/ ", TextStyle::fg(THEME.accent)), Seg::plain(shown)];
        if props.typing {
            search.push(Seg::new(" ", TextStyle::default().inverse(true)));
        }
        lines.push(line(search, TextStyle::default(), content_width));
        lines.push(vec![Seg::plain(" ")]);
    }
    lines.extend(
        rows.iter()
            .skip(start)
            .take(page)
            .map(|row| content_row(row, content_width)),
    );
    draw_rows(buf, content_area, &lines);
    viewport
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::string_width;
    use deckhand_core::key::Key;
    use regex::Regex;

    const SEARCH_TOPICS: &[HelpTopic] = &[
        HelpTopic {
            title: "Sessions",
            lines: &[
                HelpLine::Heading { heading: "Navigation" },
                HelpLine::Key {
                    keys: "n",
                    text: "Start a workspace",
                },
                HelpLine::Key {
                    keys: "Ctrl+]",
                    text: "Return to Deckhand",
                },
                HelpLine::Note {
                    note: "Saved conversations remain available.",
                },
            ],
        },
        HelpTopic {
            title: "Workspaces",
            lines: &[
                HelpLine::Heading { heading: "Start" },
                HelpLine::Key {
                    keys: "Space",
                    text: "Toggle a task",
                },
                HelpLine::Note {
                    note: "Start from an existing branch.",
                },
            ],
        },
    ];

    #[test]
    fn shift_j_and_k_page_and_g_and_shift_g_pick_the_first_and_last_topic() {
        // A Mac keyboard has no page or Home/End keys.
        let mut view = HelpView::default();
        view.open(Some(1));
        view.view.max = 30;
        view.view.page = 10;
        view.handle_input(&Input::text("J"));
        assert_eq!(view.scroll, 10);
        view.handle_input(&Input::text("K"));
        assert_eq!(view.scroll, 0);
        view.handle_input(&Input::text("G"));
        assert_eq!(view.topic, HELP_TOPICS.len() - 1);
        view.handle_input(&Input::text("g"));
        assert_eq!(view.topic, 0);
        // While a search is typed, letters are text.
        view.handle_input(&Input::text("/"));
        view.handle_input(&Input::text("J"));
        assert_eq!(view.query.as_deref(), Some("J"));
    }
    #[test]
    fn search_matches_keys_descriptions_and_notes_case_insensitively() {
        for (query, line) in [("CTRL+]", 2), (" deckHAND ", 2), ("\tSaVeD\n", 3)] {
            assert_eq!(
                search_help(query, SEARCH_TOPICS),
                [FoundTopic {
                    title: "Sessions",
                    lines: vec![SEARCH_TOPICS[0].lines[line]],
                }],
                "query {query:?}"
            );
        }
    }

    #[test]
    fn title_match_includes_the_topics_content_in_order_without_headings() {
        assert_eq!(
            search_help("\u{a0}SESSIONs\u{a0}", SEARCH_TOPICS),
            [FoundTopic {
                title: "Sessions",
                lines: SEARCH_TOPICS[0].lines[1..].to_vec(),
            }]
        );
    }

    #[test]
    fn search_preserves_topic_and_line_order_and_omits_unmatched_lines() {
        assert_eq!(
            search_help("start", SEARCH_TOPICS),
            [
                FoundTopic {
                    title: "Sessions",
                    lines: vec![SEARCH_TOPICS[0].lines[1]],
                },
                FoundTopic {
                    title: "Workspaces",
                    lines: vec![SEARCH_TOPICS[1].lines[2]],
                },
            ]
        );
    }

    #[test]
    fn search_omits_blank_queries_unmatched_topics_and_heading_only_matches() {
        for query in ["", " \t\n", "missing", "Navigation"] {
            assert!(search_help(query, SEARCH_TOPICS).is_empty(), "query {query:?}");
        }
        assert!(search_help("anything", &[]).is_empty());
        assert!(
            search_help(
                "match",
                &[
                    HelpTopic {
                        title: "Match with no lines",
                        lines: &[],
                    },
                    HelpTopic {
                        title: "Match with only a heading",
                        lines: &[HelpLine::Heading { heading: "Match" }],
                    },
                ]
            )
            .is_empty()
        );
    }

    #[test]
    fn search_matches_unicode_text_and_keys() {
        let topics = [HelpTopic {
            title: "Änderungen",
            lines: &[
                HelpLine::Key {
                    keys: "←",
                    text: "Zurück zur Übersicht 😀",
                },
                HelpLine::Note {
                    note: "日本語のガイド"
                },
            ],
        }];
        for query in ["←", "ÜBERSICHT", "😀"] {
            assert_eq!(search_help(query, &topics)[0].lines, [topics[0].lines[0]]);
        }
        assert_eq!(search_help("日本語", &topics)[0].lines, [topics[0].lines[1]]);
        assert_eq!(search_help("änderungen", &topics)[0].lines, topics[0].lines);
    }

    /// The pane drawn at `width` × `height`, as lines of text (wide characters take their continuation cells).
    fn pane(props: &HelpPaneProps<'_>, width: u16, height: u16) -> Vec<String> {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        render_help_pane(props, &mut buf, area);
        (0..height)
            .map(|y| {
                let mut line = String::new();
                let mut x = 0;
                while x < width {
                    let symbol = buf[(x, y)].symbol();
                    line.push_str(symbol);
                    x += u16::try_from(string_width(symbol).max(1)).unwrap_or(1);
                }
                line
            })
            .collect()
    }

    fn topic_props(topic: usize) -> HelpPaneProps<'static> {
        HelpPaneProps {
            topic,
            scroll: 0,
            query: None,
            typing: false,
        }
    }

    fn texts(rows: &[HelpRow]) -> Vec<String> {
        rows.iter()
            .map(|row| row.iter().map(|part| part.text.as_str()).collect())
            .collect()
    }

    #[test]
    fn help_topics_keys_in_one_aligned_column() {
        let rows = help_rows(
            &[
                HelpLine::Note { note: "About this." },
                HelpLine::Key {
                    keys: "n",
                    text: "one two three four",
                },
                HelpLine::Key {
                    keys: "Ctrl+]",
                    text: "back",
                },
                HelpLine::Heading { heading: "More" },
                HelpLine::Key {
                    keys: "a-very-long-key-name",
                    text: "x",
                },
            ],
            20,
        );
        assert_eq!(
            texts(&rows),
            [
                "About this.",
                "",
                "n       one two",
                "        three four",
                "Ctrl+]  back",
                "",
                "More",
                "a-very-long-key-name",
                "        x"
            ]
        );
        assert_eq!(rows[0][0].color, Some(INK_THEME.muted));
        assert_eq!(
            (rows[2][0].color, rows[2][0].bold),
            (Some(INK_THEME.active), Some(true))
        );
        assert_eq!((rows[2][1].color, rows[2][1].bold), (None, None));
        assert_eq!(
            (rows[6][0].color, rows[6][0].bold),
            (Some(INK_THEME.accent_soft), Some(true))
        );
        // Every topic fits in a short title list and has content.
        for topic in HELP_TOPICS {
            assert!(js_len(topic.title) <= 15, "{}", topic.title);
            assert!(!topic.lines.is_empty(), "{}", topic.title);
        }
    }

    #[test]
    fn notes_wrap_at_the_full_width_and_headings_start_a_group() {
        let rows = help_rows(
            &[
                HelpLine::Note {
                    note: "one two three four",
                },
                HelpLine::Note { note: "another note" },
                HelpLine::Heading { heading: "Next steps" },
                HelpLine::Note { note: "read this" },
                HelpLine::Key {
                    keys: "n",
                    text: "start",
                },
                HelpLine::Note { note: "then continue" },
            ],
            14,
        );
        assert_eq!(
            texts(&rows),
            [
                "one two three",
                "four",
                "another note",
                "",
                "Next steps",
                "read this",
                "",
                "n  start",
                "",
                "then continue",
            ]
        );
    }

    #[test]
    fn keys_longer_than_the_column_limit_get_their_own_row() {
        let rows = help_rows(
            &[
                HelpLine::Key {
                    keys: "Ctrl+Shift+PgUp",
                    text: "scroll up",
                },
                HelpLine::Key {
                    keys: "Ctrl+Shift+Right",
                    text: "scroll right",
                },
            ],
            40,
        );
        assert_eq!(
            texts(&rows),
            [
                "Ctrl+Shift+PgUp  scroll up",
                "Ctrl+Shift+Right",
                "                 scroll right",
            ]
        );
        assert_eq!(
            (rows[1][0].color, rows[1][0].bold),
            (Some(INK_THEME.active), Some(true))
        );
    }

    #[test]
    fn narrow_key_rows_keep_a_readable_hanging_indent() {
        let lines = [HelpLine::Key {
            keys: "n",
            text: "one two three",
        }];
        for width in [0, 1, 10, 13] {
            assert_eq!(
                texts(&help_rows(&lines, width)),
                ["n  one two", "   three"],
                "width {width}"
            );
        }
        assert!(help_rows(&[], 20).is_empty());
    }

    #[test]
    fn unicode_notes_wrap_without_losing_characters() {
        let rows = help_rows(
            &[HelpLine::Note {
                note: "café 😀 next"
            }],
            7,
        );
        assert_eq!(texts(&rows), ["café 😀", "next"]);
        let rows = help_rows(
            &[HelpLine::Key {
                keys: "←",
                text: "日本語 café 😀",
            }],
            40,
        );
        assert_eq!(texts(&rows), ["←  日本語 café 😀"]);
    }

    #[test]
    fn search_rows_prompt_for_a_query_and_report_trimmed_unmatched_text() {
        assert_eq!(texts(&search_rows(" \t", 60)), ["Type to search every topic."]);
        assert_eq!(
            texts(&search_rows("  no-such-help-term  ", 60)),
            ["Nothing matches \"no-such-help-term\"."]
        );
    }

    #[test]
    fn help_search_matches_across_topics_and_the_pane_adapts_to_its_width() {
        let titles = |query: &str| -> Vec<&str> {
            search_help(query, HELP_TOPICS)
                .iter()
                .map(|topic| topic.title)
                .collect()
        };
        assert_eq!(titles("lazygit"), ["Git & PRs"]);
        assert!(titles("codex").contains(&"Settings"));
        assert!(search_help("  ", HELP_TOPICS).is_empty());
        let wide = pane(&topic_props(1), 110, 24);
        assert!(wide[1].contains("Help · Sessions"), "{}", wide[1]);
        let selected = Regex::new(r"❯ 2 Sessions +│ +handoff \(after H\)").expect("regex");
        assert!(wide.iter().any(|line| selected.is_match(line)), "{wide:#?}");
        // Every topic fits a 24-row pane at 100 columns, so none needs scrolling there.
        let scrolling = Regex::new(r"J/K scroll|↓ \d").expect("regex");
        for (index, topic) in HELP_TOPICS.iter().enumerate() {
            let text = pane(&topic_props(index), 100, 24).join("\n");
            assert!(!scrolling.is_match(&text), "{}\n{text}", topic.title);
        }
        let narrow = pane(&topic_props(1), 56, 14);
        let heading = Regex::new(r"Help · ◂ Sessions \(2/11\) ▸ +↓ \d+/\d+").expect("regex");
        assert!(heading.is_match(&narrow[1]), "{}", narrow[1]);
        assert!(narrow.iter().any(|line| line.contains("J/K scroll")));
        let search = pane(
            &HelpPaneProps {
                query: Some("lazygit"),
                typing: true,
                ..topic_props(0)
            },
            110,
            24,
        )
        .join("\n");
        for text in [
            "Help · search",
            "/ lazygit",
            "9  Git & PRs",
            "On the Git tab",
            "enter done",
        ] {
            assert!(search.contains(text), "{text}\n{search}");
        }
        let nothing = pane(
            &HelpPaneProps {
                query: Some("zzz"),
                ..topic_props(0)
            },
            110,
            24,
        )
        .join("\n");
        assert!(nothing.contains("Nothing matches \"zzz\"."));
    }

    fn press(view: &mut HelpView, input: &str) -> Option<HelpOutcome> {
        view.handle_input(&Input::text(input))
    }

    fn special(view: &mut HelpView, key: Key) -> Option<HelpOutcome> {
        view.handle_input(&Input::key("", key))
    }

    #[test]
    fn browsing_picks_topics_with_arrows_digits_and_wraps() {
        let mut view = HelpView::default();
        view.open(None);
        assert_eq!(press(&mut view, "j"), None);
        assert_eq!(view.topic, 1);
        assert_eq!(press(&mut view, "k"), None);
        assert_eq!(press(&mut view, "k"), None);
        assert_eq!(view.topic, HELP_TOPICS.len() - 1, "wraps to the last topic");
        special(&mut view, Key { tab: true, ..Key::NONE });
        assert_eq!(view.topic, 0);
        special(
            &mut view,
            Key {
                tab: true,
                shift: true,
                ..Key::NONE
            },
        );
        assert_eq!(view.topic, HELP_TOPICS.len() - 1);
        press(&mut view, "3");
        assert_eq!(view.topic, 2);
        // A digit past the last topic does nothing.
        press(&mut view, "9");
        assert_eq!(view.topic, 8);
        special(&mut view, Key { end: true, ..Key::NONE });
        assert_eq!(view.topic, HELP_TOPICS.len() - 1);
        special(
            &mut view,
            Key {
                home: true,
                ..Key::NONE
            },
        );
        assert_eq!(view.topic, 0);
        press(&mut view, "0");
        assert_eq!(view.topic, 9, "0 jumps to the tenth topic");
        assert_eq!(press(&mut view, "?"), Some(HelpOutcome::Close));
        assert_eq!(
            special(
                &mut view,
                Key {
                    escape: true,
                    ..Key::NONE
                }
            ),
            Some(HelpOutcome::Close)
        );
    }

    #[test]
    fn searching_types_edits_scrolls_and_clears() {
        let mut view = HelpView::default();
        view.open(Some(2));
        assert_eq!(view.topic, 2);
        press(&mut view, "/");
        assert_eq!(view.query.as_deref(), Some(""));
        assert!(view.typing);
        // Space and k type while typing; they do not scroll or browse.
        for text in ["l", "a", " ", "k"] {
            assert_eq!(press(&mut view, text), None);
        }
        assert_eq!(view.query.as_deref(), Some("la k"));
        // Control characters and Ctrl chords are not text.
        press(&mut view, "\u{7}");
        view.handle_input(&Input::key(
            "x",
            Key {
                ctrl: true,
                ..Key::NONE
            },
        ));
        assert_eq!(view.query.as_deref(), Some("la k"));
        special(
            &mut view,
            Key {
                backspace: true,
                ..Key::NONE
            },
        );
        assert_eq!(view.query.as_deref(), Some("la "));
        // Ctrl+J (a bare line feed) and Ctrl+K scroll while typing, leaving the search alone.
        view.view = HelpViewport { max: 5, page: 3 };
        view.handle_input(&crate::input::input_for_event("\n"));
        assert_eq!(view.scroll, 1);
        view.handle_input(&crate::input::input_for_event("\x0b"));
        assert_eq!((view.scroll, view.query.as_deref()), (0, Some("la ")));
        // Enter ends typing; j and k then scroll, / edits again, ? closes.
        special(
            &mut view,
            Key {
                return_: true,
                ..Key::NONE
            },
        );
        assert!(!view.typing);
        view.view = HelpViewport { max: 5, page: 3 };
        press(&mut view, "j");
        assert_eq!(view.scroll, 1);
        press(&mut view, " ");
        assert_eq!(view.scroll, 4);
        special(
            &mut view,
            Key {
                page_down: true,
                ..Key::NONE
            },
        );
        assert_eq!(view.scroll, 5, "clamped to the range");
        special(
            &mut view,
            Key {
                page_up: true,
                ..Key::NONE
            },
        );
        assert_eq!(view.scroll, 2);
        press(&mut view, "/");
        assert!(view.typing);
        special(
            &mut view,
            Key {
                escape: true,
                ..Key::NONE
            },
        );
        assert_eq!((view.query.clone(), view.typing, view.scroll), (None, false, 0));
        // Searching and not typing: ? closes the guide.
        press(&mut view, "/");
        special(&mut view, Key { tab: true, ..Key::NONE });
        assert_eq!(press(&mut view, "?"), Some(HelpOutcome::Close));
    }

    #[test]
    fn rendering_records_the_viewport_for_the_next_key() {
        let mut view = HelpView::default();
        view.open(Some(1));
        let area = Rect::new(0, 0, 56, 14);
        let mut buf = Buffer::empty(area);
        view.render(&mut buf, area);
        assert!(view.view.max > 0);
        assert_eq!(view.view.page, 10);
        // A stale scroll past the end draws the last page.
        view.scroll = 10_000;
        view.render(&mut buf, area);
        let last = pane(
            &HelpPaneProps {
                scroll: 10_000,
                ..topic_props(1)
            },
            56,
            14,
        );
        assert!(
            last[1].contains(&format!("{0}/{0}", view.view.max + view.view.page)),
            "{}",
            last[1]
        );
    }

    #[test]
    fn tiny_panes_and_stale_topics_do_not_panic() {
        for (width, height) in [(0, 0), (1, 1), (5, 2), (12, 4), (64, 5), (200, 3)] {
            for topic in [0, 3, 999] {
                let _ = pane(&topic_props(topic), width, height);
                let _ = pane(
                    &HelpPaneProps {
                        query: Some("git"),
                        typing: true,
                        ..topic_props(topic)
                    },
                    width,
                    height,
                );
            }
        }
    }
}
