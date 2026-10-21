//! The Ink Box/Text behaviour the panes rely on, on ratatui: styled text segments (nested `<Text>` styles),
//! `wrap="truncate*"` (cli-truncate), `wrap="wrap"` (wrap-ansi `{hard: true, trim: false}`), text measuring
//! (string-width), `borderStyle` borders (render-border.js with cli-boxes) and padding.
//!
//! Panes are laid out by hand (they already budget their rows and columns in TS); these are the pieces they draw
//! with. Widths are terminal columns. Two width rules exist because Ink has two: string-width ([`string_width`],
//! what Ink and cli-truncate measure with) and slice-ansi's per-grapheme width ([`slice_columns`], what cli-truncate
//! cuts with). A wide character at a cut can therefore overflow by one column exactly as in Ink; drawing clips it.
//! Vectors recorded from Ink's packages live in `tests/data/text-vectors.json`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

use crate::theme::THEME;

/// cli-truncate's truncation character.
pub const ELLIPSIS: &str = "…";

// ---------------------------------------------------------------------------------------------------------------
// Measuring

fn is_regional_indicator(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}

fn is_extended_pictographic(c: char) -> bool {
    // The Extended_Pictographic blocks string-width counts in ZWJ sequences (approximated by range).
    matches!(c as u32, 0x00A9 | 0x00AE | 0x203C | 0x2049 | 0x2122 | 0x2139 | 0x2194..=0x2199 | 0x21A9..=0x21AA
        | 0x231A..=0x231B | 0x2328 | 0x23CF | 0x23E9..=0x23F3 | 0x23F8..=0x23FA | 0x24C2 | 0x25AA..=0x25AB
        | 0x25B6 | 0x25C0 | 0x25FB..=0x25FE | 0x2600..=0x27BF | 0x2934..=0x2935 | 0x2B05..=0x2B07
        | 0x2B1B..=0x2B1C | 0x2B50 | 0x2B55 | 0x3030 | 0x303D | 0x3297 | 0x3299 | 0x1F000..=0x1FAFF)
}

/// Default_Ignorable / Control / Format / Mark: what string-width skips (zero columns, never a cluster's base).
fn is_non_printing(c: char) -> bool {
    c.is_control()
        || matches!(c as u32, 0x00AD | 0x034F | 0x061C | 0x115F..=0x1160 | 0x17B4..=0x17B5 | 0x180B..=0x180F
            | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x206F | 0x3164 | 0xFE00..=0xFE0F | 0xFEFF | 0xFFA0
            | 0xFFF0..=0xFFF8 | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A | 0xE0000..=0xE0FFF)
        || (c.width() == Some(0) && !is_regional_indicator(c))
}

/// get-east-asian-width eastAsianWidth with ambiguous characters narrow: 2 for Wide/Fullwidth, else 1.
fn east_asian_width(c: char) -> usize {
    if c.width() == Some(2) { 2 } else { 1 }
}

/// string-width's emoji rule: an RGI emoji cluster (approximated: emoji presentation forms, modifiers, flags,
/// keycaps, tag sequences) or a ZWJ sequence of two or more pictographs.
fn is_double_width_emoji(cluster: &str) -> bool {
    let mut chars = cluster.chars();
    let Some(first) = chars.next() else { return false };
    if chars.next().is_none() {
        return false;
    }
    let regional = cluster.chars().filter(|c| is_regional_indicator(*c)).count();
    if regional == 2 {
        return true;
    }
    if cluster
        .chars()
        .any(|c| matches!(c as u32, 0xFE0F | 0x20E3 | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F))
    {
        return is_extended_pictographic(first) || first.is_ascii_digit() || first == '#' || first == '*';
    }
    cluster.contains('\u{200D}') && cluster.chars().filter(|c| is_extended_pictographic(*c)).count() >= 2
}

/// The columns string-width gives one grapheme cluster.
fn cluster_width(cluster: &str) -> usize {
    if cluster.chars().all(is_non_printing) {
        return 0;
    }
    if is_double_width_emoji(cluster) {
        return 2;
    }
    let mut visible = cluster.chars().skip_while(|c| is_non_printing(*c));
    let Some(base) = visible.next() else { return 0 };
    let trailing: usize = cluster
        .chars()
        .skip(1)
        .filter(|c| ('\u{FF00}'..='\u{FFEF}').contains(c))
        .map(east_asian_width)
        .sum();
    east_asian_width(base) + trailing
}

/// TS: string-width (what Ink measures text with): grapheme clusters, emoji double width, ambiguous narrow,
/// controls and marks zero.
pub fn string_width(text: &str) -> usize {
    if text.bytes().all(|byte| (0x20..=0x7e).contains(&byte)) {
        return text.len();
    }
    text.graphemes(true).map(cluster_width).sum()
}

/// The Emoji property (emoji-data.txt), as emoji-regex matches single characters: text-style emoji included, bare
/// `#`, `*` and digits not.
fn is_emoji(c: char) -> bool {
    matches!(c as u32, 0x00A9 | 0x00AE | 0x203C | 0x2049 | 0x2122 | 0x2139 | 0x2194..=0x2199 | 0x21A9..=0x21AA
        | 0x231A..=0x231B | 0x2328 | 0x23CF | 0x23E9..=0x23F3 | 0x23F8..=0x23FA | 0x24C2 | 0x25AA..=0x25AB
        | 0x25B6 | 0x25C0 | 0x25FB..=0x25FE | 0x2600..=0x2604 | 0x260E | 0x2611 | 0x2614..=0x2615 | 0x2618
        | 0x261D | 0x2620 | 0x2622..=0x2623 | 0x2626 | 0x262A | 0x262E..=0x262F | 0x2638..=0x263A | 0x2640
        | 0x2642 | 0x2648..=0x2653 | 0x265F..=0x2660 | 0x2663 | 0x2665..=0x2666 | 0x2668 | 0x267B
        | 0x267E..=0x267F | 0x2692..=0x2697 | 0x2699 | 0x269B..=0x269C | 0x26A0..=0x26A1 | 0x26A7
        | 0x26AA..=0x26AB | 0x26B0..=0x26B1 | 0x26BD..=0x26BE | 0x26C4..=0x26C5 | 0x26C8 | 0x26CE..=0x26CF
        | 0x26D1 | 0x26D3..=0x26D4 | 0x26E9..=0x26EA | 0x26F0..=0x26F5 | 0x26F7..=0x26FA | 0x26FD | 0x2702
        | 0x2705 | 0x2708..=0x270D | 0x270F | 0x2712 | 0x2714 | 0x2716 | 0x271D | 0x2721 | 0x2728
        | 0x2733..=0x2734 | 0x2744 | 0x2747 | 0x274C | 0x274E | 0x2753..=0x2755 | 0x2757 | 0x2763..=0x2764
        | 0x2795..=0x2797 | 0x27A1 | 0x27B0 | 0x27BF | 0x2934..=0x2935 | 0x2B05..=0x2B07 | 0x2B1B..=0x2B1C
        | 0x2B50 | 0x2B55 | 0x3030 | 0x303D | 0x3297 | 0x3299 | 0x1F004 | 0x1F0CF | 0x1F170..=0x1F171
        | 0x1F17E..=0x1F17F | 0x1F18E | 0x1F191..=0x1F19A | 0x1F201..=0x1F202 | 0x1F21A
        | 0x1F22F | 0x1F232..=0x1F23A | 0x1F250..=0x1F251 | 0x1F300..=0x1F321 | 0x1F324..=0x1F393
        | 0x1F396..=0x1F397 | 0x1F399..=0x1F39B | 0x1F39E..=0x1F3F0 | 0x1F3F3..=0x1F3F5 | 0x1F3F7..=0x1F4FD
        | 0x1F4FF..=0x1F53D | 0x1F549..=0x1F54E | 0x1F550..=0x1F567 | 0x1F56F..=0x1F570 | 0x1F573..=0x1F57A
        | 0x1F587 | 0x1F58A..=0x1F58D | 0x1F590 | 0x1F595..=0x1F596 | 0x1F5A4..=0x1F5A5 | 0x1F5A8
        | 0x1F5B1..=0x1F5B2 | 0x1F5BC | 0x1F5C2..=0x1F5C4 | 0x1F5D1..=0x1F5D3 | 0x1F5DC..=0x1F5DE | 0x1F5E1
        | 0x1F5E3 | 0x1F5E8 | 0x1F5EF | 0x1F5F3 | 0x1F5FA..=0x1F64F | 0x1F680..=0x1F6C5 | 0x1F6CB..=0x1F6D2
        | 0x1F6D5..=0x1F6D7 | 0x1F6DC..=0x1F6E5 | 0x1F6E9 | 0x1F6EB..=0x1F6EC | 0x1F6F0 | 0x1F6F3..=0x1F6FC
        | 0x1F7E0..=0x1F7EB | 0x1F7F0 | 0x1F90C..=0x1F93A | 0x1F93C..=0x1F945 | 0x1F947..=0x1F9FF
        | 0x1FA70..=0x1FA7C | 0x1FA80..=0x1FA89 | 0x1FA8F..=0x1FAC6 | 0x1FACE..=0x1FADC | 0x1FADF..=0x1FAE9
        | 0x1FAF0..=0x1FAF8)
}

/// string-width 7 for one grapheme cluster (what wrap-ansi 9 measures with): its first code point decides; an
/// emoji anywhere in the cluster (emoji-regex, text-style emoji included) is 2.
fn wrap_cluster_width(cluster: &str) -> usize {
    let Some(first) = cluster.chars().next() else { return 0 };
    let code = first as u32;
    let ignored = code <= 0x1F
        || (0x7F..=0x9F).contains(&code)
        || (0x200B..=0x200F).contains(&code)
        || code == 0xFEFF
        || (0x300..=0x36F).contains(&code)
        || (0x1AB0..=0x1AFF).contains(&code)
        || (0x1DC0..=0x1DFF).contains(&code)
        || (0x20D0..=0x20FF).contains(&code)
        || (0xFE20..=0xFE2F).contains(&code)
        || (0xFE00..=0xFE0F).contains(&code)
        || is_non_printing(first);
    if ignored {
        return 0;
    }
    let flag = cluster.chars().filter(|c| is_regional_indicator(*c)).count() >= 2;
    if flag || cluster.contains('\u{20E3}') || cluster.chars().any(is_emoji) {
        return 2;
    }
    east_asian_width(first)
}

/// TS: string-width 7.2 (wrap-ansi 9's own copy, older than Ink's): the width wrap="wrap" measures with.
pub fn wrap_width(text: &str) -> usize {
    text.graphemes(true).map(wrap_cluster_width).sum()
}

/// slice-ansi's getGraphemeWidth: 2 for a cluster with a wide character, a regional indicator or emoji style, else 1
/// (zero-width clusters included).
pub fn grapheme_width(cluster: &str) -> usize {
    let wide = cluster
        .chars()
        .any(|c| c.width() == Some(2) || is_regional_indicator(c) || matches!(c as u32, 0xFE0F | 0x20E3));
    if wide { 2 } else { 1 }
}

/// TS: slice-ansi `sliceAnsi(text, start, end)` for plain text: the clusters starting at column `start` or later and
/// before column `end` (a wide cluster starting at `end - 1` is kept).
pub fn slice_columns(text: &str, start: usize, end: usize) -> String {
    let mut position = 0;
    let mut result = String::new();
    let mut include = false;
    for cluster in text.graphemes(true) {
        if position >= end {
            break;
        }
        if !include && position >= start {
            include = true;
        }
        if include {
            result.push_str(cluster);
        }
        position += grapheme_width(cluster);
    }
    result
}

/// Ink `wrap` values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Wrap {
    /// `wrap="wrap"` (Ink's default): wrap-ansi, hard.
    #[default]
    Wrap,
    /// `wrap="truncate"` / `"truncate-end"`.
    TruncateEnd,
    /// `wrap="truncate-start"`.
    TruncateStart,
    /// `wrap="truncate-middle"`.
    TruncateMiddle,
}

/// The ranges (start column, end column) cli-truncate keeps, before and after the ellipsis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cut {
    Whole,
    Empty,
    Ellipsis,
    Keep {
        head: (usize, usize),
        tail: Option<(usize, usize)>,
    },
}

/// TS: cli-truncate's plan for `text` at `columns` (no space or preferTruncationOnSpace, as Ink calls it).
fn cut(text: &str, columns: usize, position: Wrap) -> Cut {
    if columns < 1 {
        return Cut::Empty;
    }
    let length = string_width(text);
    if length <= columns {
        return Cut::Whole;
    }
    if columns == 1 {
        return Cut::Ellipsis;
    }
    match position {
        Wrap::TruncateStart => Cut::Keep {
            head: (0, 0),
            tail: Some((length + 1 - columns, length)),
        },
        Wrap::TruncateMiddle => {
            let half = columns / 2;
            Cut::Keep {
                head: (0, half),
                tail: Some(((length + 1).saturating_sub(columns - half), length)),
            }
        }
        Wrap::Wrap | Wrap::TruncateEnd => Cut::Keep {
            head: (0, columns - 1),
            tail: None,
        },
    }
}

/// TS: cli-truncate `cliTruncate(text, columns, {position})` (Ink `wrap="truncate*"`).
pub fn truncate(text: &str, columns: usize, position: Wrap) -> String {
    match cut(text, columns, position) {
        Cut::Whole => text.to_string(),
        Cut::Empty => String::new(),
        Cut::Ellipsis => ELLIPSIS.to_string(),
        Cut::Keep { head, tail } => match position {
            Wrap::TruncateStart => {
                let (start, end) = tail.unwrap_or_default();
                format!("{ELLIPSIS}{}", slice_columns(text, start, end))
            }
            _ => {
                let mut result = slice_columns(text, head.0, head.1);
                result.push_str(ELLIPSIS);
                if let Some((start, end)) = tail {
                    result.push_str(&slice_columns(text, start, end));
                }
                result
            }
        },
    }
}

/// TS: Ink `wrap="truncate-end"` (cli-truncate at the end).
pub fn truncate_end(text: &str, columns: usize) -> String {
    truncate(text, columns, Wrap::TruncateEnd)
}

/// TS: wrap-ansi wrapWord.
fn wrap_word(rows: &mut Vec<String>, word: &str, columns: usize) {
    let characters: Vec<char> = word.chars().collect();
    let mut visible = rows.last().map_or(0, |row| wrap_width(row));
    for (index, character) in characters.iter().enumerate() {
        let mut buffer = [0; 4];
        let text = character.encode_utf8(&mut buffer);
        let length = wrap_width(text);
        if visible + length <= columns {
            if let Some(row) = rows.last_mut() {
                row.push(*character);
            }
        } else {
            rows.push(character.to_string());
            visible = 0;
        }
        visible += length;
        if visible == columns && index + 1 < characters.len() {
            rows.push(String::new());
            visible = 0;
        }
    }
    // A last row holding only invisible characters joins the row before it.
    if visible == 0
        && rows.last().is_some_and(|row| !row.is_empty())
        && rows.len() > 1
        && let Some(last) = rows.pop()
        && let Some(previous) = rows.last_mut()
    {
        previous.push_str(&last);
    }
}

/// TS: wrap-ansi exec for one line with `{hard: true, trim: false}`.
fn wrap_line(line: &str, columns: usize) -> Vec<String> {
    let words: Vec<&str> = line.split(' ').collect();
    let lengths: Vec<usize> = words.iter().map(|word| wrap_width(word)).collect();
    let mut rows = vec![String::new()];
    for (index, word) in words.iter().enumerate() {
        let mut row_length = rows.last().map_or(0, |row| wrap_width(row));
        if index != 0 {
            if row_length >= columns {
                rows.push(String::new());
                row_length = 0;
            }
            if let Some(row) = rows.last_mut() {
                row.push(' ');
            }
            row_length += 1;
        }
        let length = lengths[index];
        if length > columns {
            let remaining = columns as i64 - row_length as i64;
            let breaks_this_line = 1 + (length as i64 - remaining - 1).div_euclid(columns as i64);
            let breaks_next_line = (length as i64 - 1).div_euclid(columns as i64);
            if breaks_next_line < breaks_this_line {
                rows.push(String::new());
            }
            wrap_word(&mut rows, word, columns);
            continue;
        }
        if row_length + length > columns && row_length > 0 && length > 0 {
            rows.push(String::new());
        }
        if let Some(row) = rows.last_mut() {
            row.push_str(word);
        }
    }
    rows
}

/// TS: wrap-ansi `wrapAnsi(text, columns, {trim: false, hard: true})` (Ink `wrap="wrap"`), as lines. The lines
/// joined with `\n` are the text with CRLF as LF. wrap-ansi first NFC-normalises the text; that is not applied
/// here (no normalisation tables in the workspace), so decomposed accents stay decomposed.
pub fn wrap_text(text: &str, columns: usize) -> Vec<String> {
    let columns = columns.max(1);
    text.replace("\r\n", "\n")
        .split('\n')
        .flat_map(|line| wrap_line(line, columns))
        .collect()
}

// ---------------------------------------------------------------------------------------------------------------
// Styled text

/// Ink `<Text>` style props: `color`, `backgroundColor`, `bold`, `italic`, `underline`, `strikethrough`, `inverse`,
/// `dimColor`. Unset fields inherit from the enclosing Text ([`TextStyle::patch`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TextStyle {
    pub color: Option<Color>,
    pub background: Option<Color>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strikethrough: Option<bool>,
    pub inverse: Option<bool>,
    pub dim: Option<bool>,
}

impl TextStyle {
    /// A style with only a color.
    pub const fn fg(color: Color) -> Self {
        TextStyle {
            color: Some(color),
            background: None,
            bold: None,
            italic: None,
            underline: None,
            strikethrough: None,
            inverse: None,
            dim: None,
        }
    }

    /// `color` (or none: inherit).
    pub fn color(mut self, color: impl Into<Option<Color>>) -> Self {
        self.color = color.into();
        self
    }

    pub fn bold(mut self, on: bool) -> Self {
        self.bold = Some(on);
        self
    }

    pub fn italic(mut self, on: bool) -> Self {
        self.italic = Some(on);
        self
    }

    pub fn underline(mut self, on: bool) -> Self {
        self.underline = Some(on);
        self
    }

    pub fn inverse(mut self, on: bool) -> Self {
        self.inverse = Some(on);
        self
    }

    /// Ink `dimColor`.
    pub fn dim(mut self, on: bool) -> Self {
        self.dim = Some(on);
        self
    }

    /// A nested `<Text>`: its own props win, the rest are the parent's.
    pub fn patch(self, child: TextStyle) -> TextStyle {
        TextStyle {
            color: child.color.or(self.color),
            background: child.background.or(self.background),
            bold: child.bold.or(self.bold),
            italic: child.italic.or(self.italic),
            underline: child.underline.or(self.underline),
            strikethrough: child.strikethrough.or(self.strikethrough),
            inverse: child.inverse.or(self.inverse),
            dim: child.dim.or(self.dim),
        }
    }

    /// The ratatui style (chalk's SGR attributes as modifiers).
    pub fn to_style(self) -> Style {
        let mut style = Style::default();
        if let Some(color) = self.color {
            style = style.fg(color);
        }
        if let Some(color) = self.background {
            style = style.bg(color);
        }
        let flags = [
            (self.bold, Modifier::BOLD),
            (self.italic, Modifier::ITALIC),
            (self.underline, Modifier::UNDERLINED),
            (self.strikethrough, Modifier::CROSSED_OUT),
            (self.inverse, Modifier::REVERSED),
            (self.dim, Modifier::DIM),
        ];
        for (flag, modifier) in flags {
            if flag == Some(true) {
                style = style.add_modifier(modifier);
            }
        }
        style
    }
}

/// One styled piece of a row (a `<Text>` leaf).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Seg {
    pub text: String,
    pub style: TextStyle,
}

impl Seg {
    pub fn new(text: impl Into<String>, style: TextStyle) -> Self {
        Seg {
            text: text.into(),
            style,
        }
    }

    /// Unstyled text.
    pub fn plain(text: impl Into<String>) -> Self {
        Seg::new(text, TextStyle::default())
    }
}

/// One rendered line: its segments in order (a `<Text>` with nested `<Text>`s, flattened).
pub type Row = Vec<Seg>;

/// The row's plain text.
pub fn row_text(row: &[Seg]) -> String {
    row.iter().map(|seg| seg.text.as_str()).collect()
}

/// The row's width ([`string_width`] of its text).
pub fn row_width(row: &[Seg]) -> usize {
    string_width(&row_text(row))
}

/// `style` applied under every segment's own (the enclosing `<Text>` of a row).
pub fn with_base(row: Row, base: TextStyle) -> Row {
    row.into_iter()
        .map(|seg| Seg {
            text: seg.text,
            style: base.patch(seg.style),
        })
        .collect()
}

/// The segments covering byte range `start..end` of the row's text, styles kept.
fn slice_row(row: &[Seg], start: usize, end: usize) -> Row {
    let mut out = Vec::new();
    let mut offset = 0;
    for seg in row {
        let seg_start = offset;
        let seg_end = offset + seg.text.len();
        offset = seg_end;
        let from = start.max(seg_start);
        let to = end.min(seg_end);
        if from < to {
            out.push(Seg::new(&seg.text[from - seg_start..to - seg_start], seg.style));
        }
    }
    out
}

/// The byte offset in `text` where [`slice_columns`]`(text, start, end)` begins and ends.
fn slice_bounds(text: &str, start: usize, end: usize) -> (usize, usize) {
    let mut position = 0;
    let mut from = None;
    let mut to = text.len();
    for (offset, cluster) in text.grapheme_indices(true) {
        if position >= end {
            to = offset;
            break;
        }
        if from.is_none() && position >= start {
            from = Some(offset);
        }
        position += grapheme_width(cluster);
    }
    match from {
        Some(from) => (from, to.max(from)),
        None => (text.len(), text.len()),
    }
}

/// The style the ellipsis takes: cli-truncate puts it inside the trailing (end) or leading (start) style.
fn edge_style(row: &[Seg], from_end: bool) -> TextStyle {
    let mut segs = row.iter().filter(|seg| !seg.text.is_empty());
    let seg = if from_end { segs.next_back() } else { segs.next() };
    seg.map(|seg| seg.style).unwrap_or_default()
}

/// TS: Ink `wrap="truncate*"` on a styled `<Text>`: cli-truncate over the row's text, the ellipsis taking the style
/// next to it.
pub fn truncate_row(row: &[Seg], columns: usize, position: Wrap) -> Row {
    let text = row_text(row);
    match cut(&text, columns, position) {
        Cut::Whole => row.to_vec(),
        Cut::Empty => Vec::new(),
        Cut::Ellipsis => vec![Seg::new(ELLIPSIS, edge_style(row, true))],
        Cut::Keep { head, tail } => {
            if position == Wrap::TruncateStart {
                let (start, end) = tail.unwrap_or_default();
                let (from, to) = slice_bounds(&text, start, end);
                let kept = slice_row(row, from, to);
                let mut out = vec![Seg::new(ELLIPSIS, edge_style(&kept, false))];
                out.extend(kept);
                return out;
            }
            let (from, to) = slice_bounds(&text, head.0, head.1);
            let mut out = slice_row(row, from, to);
            let style = edge_style(&out, true);
            out.push(Seg::new(ELLIPSIS, style));
            if let Some((start, end)) = tail {
                let (from, to) = slice_bounds(&text, start, end);
                out.extend(slice_row(row, from, to));
            }
            out
        }
    }
}

/// TS: Ink `wrap="wrap"` on a styled `<Text>`: the row's text wrapped (wrap-ansi, hard), styles kept per character.
pub fn wrap_row(row: &[Seg], columns: usize) -> Vec<Row> {
    let text = row_text(row).replace("\r\n", "\n");
    let flat: Row = if text.len() == row_text(row).len() {
        row.to_vec()
    } else {
        row.iter()
            .map(|seg| Seg::new(seg.text.replace("\r\n", "\n"), seg.style))
            .collect()
    };
    let mut rows = Vec::new();
    let mut offset = 0;
    for line in wrap_text(&text, columns) {
        rows.push(slice_row(&flat, offset, offset + line.len()));
        offset += line.len();
        // The newline (an original one or one between wrapped rows) belongs to no row.
        if text[offset..].starts_with('\n') {
            offset += 1;
        }
    }
    rows
}

/// A row fitted to `columns` by Ink's rule for `wrap` (wrapped rows, or one truncated row).
pub fn fit_row(row: &[Seg], columns: usize, wrap: Wrap) -> Vec<Row> {
    match wrap {
        Wrap::Wrap => wrap_row(row, columns),
        _ => vec![truncate_row(row, columns, wrap)],
    }
}

/// The row as a ratatui line.
pub fn to_line(row: &[Seg]) -> Line<'static> {
    Line::from(
        row.iter()
            .map(|seg| Span::styled(seg.text.clone(), seg.style.to_style()))
            .collect::<Vec<_>>(),
    )
}

/// Draws `row` at (`x`, `y`), clipped to `width` columns and the buffer; returns the columns drawn.
pub fn draw_row(buf: &mut Buffer, x: u16, y: u16, width: u16, row: &[Seg]) -> u16 {
    let area = buf.area;
    if y < area.top() || y >= area.bottom() || x >= area.right() {
        return 0;
    }
    let limit = width.min(area.right() - x);
    let mut used: u16 = 0;
    for seg in row {
        if used >= limit {
            break;
        }
        let (next_x, _) = buf.set_stringn(x + used, y, &seg.text, usize::from(limit - used), seg.style.to_style());
        used = next_x - x;
    }
    used
}

/// Draws rows top to bottom from `area`'s top-left, each clipped to the area (Ink would overflow; panes size their
/// rows to fit). Returns the rows drawn.
pub fn draw_rows(buf: &mut Buffer, area: Rect, rows: &[Row]) -> u16 {
    let mut drawn = 0;
    for (index, row) in rows.iter().enumerate() {
        let Ok(offset) = u16::try_from(index) else { break };
        if offset >= area.height {
            break;
        }
        draw_row(buf, area.x, area.y + offset, area.width, row);
        drawn += 1;
    }
    drawn
}

/// TS: `<Box justifyContent="space-between" width={width}>` holding a left and a right `<Text>`: the left text at
/// the start (cut to leave the right one room), the right one flush right.
pub fn space_between(left: &[Seg], right: &[Seg], width: usize) -> Row {
    let right_width = row_width(right);
    let room = width.saturating_sub(right_width);
    let mut out = truncate_row(left, room, Wrap::TruncateEnd);
    let gap = width.saturating_sub(row_width(&out) + right_width);
    if gap > 0 {
        out.push(Seg::plain(" ".repeat(gap)));
    }
    out.extend(right.iter().cloned());
    out
}

/// `text` padded with spaces to `width` columns (TS `padEnd` by columns).
pub fn pad_end(text: &str, width: usize) -> String {
    let used = string_width(text);
    if used >= width {
        return text.to_string();
    }
    format!("{text}{}", " ".repeat(width - used))
}

// ---------------------------------------------------------------------------------------------------------------
// Boxes

/// cli-boxes border characters (Ink `borderStyle`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BoxChars {
    pub top_left: &'static str,
    pub top: &'static str,
    pub top_right: &'static str,
    pub right: &'static str,
    pub bottom_right: &'static str,
    pub bottom: &'static str,
    pub bottom_left: &'static str,
    pub left: &'static str,
}

/// cli-boxes `round` (every bordered pane but the help topic list).
pub const ROUND: BoxChars = BoxChars {
    top_left: "╭",
    top: "─",
    top_right: "╮",
    right: "│",
    bottom_right: "╯",
    bottom: "─",
    bottom_left: "╰",
    left: "│",
};

/// cli-boxes `single`.
pub const SINGLE: BoxChars = BoxChars {
    top_left: "┌",
    top: "─",
    top_right: "┐",
    right: "│",
    bottom_right: "┘",
    bottom: "─",
    bottom_left: "└",
    left: "│",
};

/// Ink Box border props: `borderStyle`, `borderColor`, `borderDimColor` and which sides show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Border {
    pub chars: BoxChars,
    pub color: Option<Color>,
    pub dim: bool,
    pub top: bool,
    pub bottom: bool,
    pub left: bool,
    pub right: bool,
}

impl Border {
    /// `borderStyle="round"` in `color`.
    pub const fn round(color: Color) -> Self {
        Border {
            chars: ROUND,
            color: Some(color),
            dim: false,
            top: true,
            bottom: true,
            left: true,
            right: true,
        }
    }

    /// `borderStyle="single"` in `color`.
    pub const fn single(color: Color) -> Self {
        Border {
            chars: SINGLE,
            color: Some(color),
            dim: false,
            top: true,
            bottom: true,
            left: true,
            right: true,
        }
    }

    fn style(&self) -> Style {
        let mut style = Style::default();
        if let Some(color) = self.color {
            style = style.fg(color);
        }
        if self.dim {
            style = style.add_modifier(Modifier::DIM);
        }
        style
    }

    /// The area inside the border.
    pub fn inner(&self, area: Rect) -> Rect {
        let left = u16::from(self.left);
        let right = u16::from(self.right);
        let top = u16::from(self.top);
        let bottom = u16::from(self.bottom);
        inset(area, left, right, top, bottom)
    }
}

/// TS: ink render-border.js: draws `border` around `area`; returns the area inside it.
pub fn draw_border(buf: &mut Buffer, area: Rect, border: &Border) -> Rect {
    if area.width == 0 || area.height == 0 {
        return border.inner(area);
    }
    let style = border.style();
    let chars = border.chars;
    let content_width = usize::from(area.width)
        .saturating_sub(usize::from(border.left))
        .saturating_sub(usize::from(border.right));
    let horizontal = |left: &str, fill: &str, right: &str| {
        format!(
            "{}{}{}",
            if border.left { left } else { "" },
            fill.repeat(content_width),
            if border.right { right } else { "" }
        )
    };
    let clip = buf.area.intersection(area);
    // Dusk mode fills the box (before its content draws, which keeps the background).
    if let Some(fill) = THEME.box_fill {
        buf.set_style(clip.intersection(border.inner(area)), Style::default().bg(fill));
    }
    let mut put = |x: u16, y: u16, text: &str| {
        if clip.contains((x, y).into()) {
            buf.set_stringn(x, y, text, usize::from(clip.right() - x), style);
        }
    };
    if border.top {
        put(area.x, area.y, &horizontal(chars.top_left, chars.top, chars.top_right));
    }
    if border.bottom && area.height > u16::from(border.top) {
        put(
            area.x,
            area.bottom() - 1,
            &horizontal(chars.bottom_left, chars.bottom, chars.bottom_right),
        );
    }
    let first = area.y + u16::from(border.top);
    let last = area.bottom().saturating_sub(u16::from(border.bottom));
    for y in first..last {
        if border.left {
            put(area.x, y, chars.left);
        }
        if border.right {
            put(area.right() - 1, y, chars.right);
        }
    }
    border.inner(area)
}

/// `area` shrunk by the given columns and rows (Ink padding/margins), never below zero.
pub fn inset(area: Rect, left: u16, right: u16, top: u16, bottom: u16) -> Rect {
    let x = area.x.saturating_add(left.min(area.width));
    let width = area.width.saturating_sub(left).saturating_sub(right);
    let y = area.y.saturating_add(top.min(area.height));
    let height = area.height.saturating_sub(top).saturating_sub(bottom);
    Rect::new(x, y, width, height)
}

/// TS: `<Box borderStyle=... paddingX={padding_x}>`: draws the border and returns the content area.
pub fn bordered(buf: &mut Buffer, area: Rect, border: &Border, padding_x: u16) -> Rect {
    let inner = draw_border(buf, area, border);
    inset(inner, padding_x, padding_x, 0, 0)
}

/// A column of rows to draw top to bottom (a `<Box flexDirection="column">` of single-line `<Text>`s), with Ink's
/// `<Box flexGrow={1} />` spacer: rows pushed after [`Column::grow`] are drawn at the bottom of the area.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Column {
    pub top: Vec<Row>,
    pub bottom: Vec<Row>,
    grown: bool,
}

impl Column {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a row (below the spacer once [`Column::grow`] was called).
    pub fn push(&mut self, row: Row) {
        if self.grown {
            self.bottom.push(row);
        } else {
            self.top.push(row);
        }
    }

    /// Appends rows.
    pub fn extend(&mut self, rows: impl IntoIterator<Item = Row>) {
        for row in rows {
            self.push(row);
        }
    }

    /// An empty row (Ink `<Box height={1} />` / `marginTop={1}`).
    pub fn blank(&mut self) {
        self.push(Vec::new());
    }

    /// TS `<Box flexGrow={1} />`: what follows sits at the bottom.
    pub fn grow(&mut self) {
        self.grown = true;
    }

    /// Draws the rows: the top ones from the top, the bottom ones against the bottom (the top rows win when they
    /// do not all fit, as Ink's flex shrinking does to a spacer first).
    pub fn draw(&self, buf: &mut Buffer, area: Rect) {
        let drawn = draw_rows(buf, area, &self.top);
        let free = area.height.saturating_sub(drawn);
        let count = u16::try_from(self.bottom.len()).unwrap_or(u16::MAX).min(free);
        let skip = self.bottom.len() - usize::from(count);
        let start = area.bottom() - count;
        draw_rows(buf, Rect::new(area.x, start, area.width, count), &self.bottom[skip..]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::THEME;

    fn buffer_lines(buf: &Buffer) -> Vec<String> {
        let area = buf.area;
        (area.top()..area.bottom())
            .map(|y| {
                (area.left()..area.right())
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn styles_nest_like_ink_text() {
        let parent = TextStyle::fg(THEME.active).inverse(true).bold(true);
        let child = TextStyle::default().color(THEME.muted);
        let style = parent.patch(child);
        assert_eq!(style.color, Some(THEME.muted));
        assert_eq!(style.inverse, Some(true));
        assert!(
            style
                .to_style()
                .add_modifier
                .contains(Modifier::REVERSED | Modifier::BOLD)
        );
        assert_eq!(parent.patch(TextStyle::default()).color, Some(THEME.active));
    }

    #[test]
    fn truncates_styled_rows_with_the_ellipsis_in_the_edge_style() {
        let red = TextStyle::fg(Color::Red);
        let row = vec![Seg::plain("abc"), Seg::new("defgh", red)];
        let cut = truncate_row(&row, 6, Wrap::TruncateEnd);
        assert_eq!(row_text(&cut), "abcde…");
        assert_eq!(cut.last().map(|seg| seg.style), Some(red));
        let cut = truncate_row(&row, 4, Wrap::TruncateStart);
        assert_eq!(row_text(&cut), "…fgh");
        assert_eq!(cut[0].style, red);
        assert_eq!(row_text(&truncate_row(&row, 8, Wrap::TruncateEnd)), "abcdefgh");
        assert_eq!(row_text(&truncate_row(&row, 1, Wrap::TruncateEnd)), "…");
        assert!(truncate_row(&row, 0, Wrap::TruncateEnd).is_empty());
    }

    #[test]
    fn wraps_styled_rows_keeping_styles() {
        let bold = TextStyle::default().bold(true);
        let row = vec![Seg::new("hello ", bold), Seg::plain("wide world")];
        let rows = wrap_row(&row, 6);
        let texts: Vec<String> = rows.iter().map(|row| row_text(row)).collect();
        assert_eq!(texts, ["hello ", "wide ", "world"]);
        assert_eq!(rows[0][0].style, bold);
        assert_eq!(rows[1][0].style, TextStyle::default());
    }

    #[test]
    fn space_between_puts_the_right_text_flush_right() {
        let row = space_between(&[Seg::plain("left side")], &[Seg::plain("right")], 12);
        assert_eq!(row_text(&row), "left s…right");
        assert_eq!(row_width(&row), 12);
        let row = space_between(&[Seg::plain("ab")], &[Seg::plain("cd")], 8);
        assert_eq!(row_text(&row), "ab    cd");
    }

    #[test]
    fn draws_round_borders_like_ink() {
        let area = Rect::new(0, 0, 6, 4);
        let mut buf = Buffer::empty(area);
        let inner = bordered(&mut buf, area, &Border::round(THEME.border), 1);
        assert_eq!(inner, Rect::new(2, 1, 2, 2));
        assert_eq!(buffer_lines(&buf), ["╭────╮", "│    │", "│    │", "╰────╯"]);
        assert_eq!(buf[(0, 0)].fg, THEME.border);

        let mut buf = Buffer::empty(area);
        let border = Border {
            top: false,
            bottom: false,
            left: false,
            ..Border::single(THEME.border)
        };
        let inner = draw_border(&mut buf, area, &border);
        assert_eq!(inner, Rect::new(0, 0, 5, 4));
        assert_eq!(buffer_lines(&buf), ["     │"; 4]);
    }

    #[test]
    fn columns_draw_the_grown_part_at_the_bottom() {
        let area = Rect::new(0, 0, 5, 4);
        let mut buf = Buffer::empty(area);
        let mut column = Column::new();
        column.push(vec![Seg::plain("top")]);
        column.grow();
        column.push(vec![Seg::plain("hint")]);
        column.draw(&mut buf, area);
        assert_eq!(buffer_lines(&buf), ["top  ", "     ", "     ", "hint "]);
    }

    #[test]
    fn draw_row_clips_wide_characters() {
        let area = Rect::new(0, 0, 3, 1);
        let mut buf = Buffer::empty(area);
        let used = draw_row(&mut buf, 0, 0, 3, &[Seg::plain("日本")]);
        assert_eq!(used, 2);
        assert_eq!(buf[(0, 0)].symbol(), "日");
        assert_eq!(buf[(2, 0)].symbol(), " ");
    }
}
