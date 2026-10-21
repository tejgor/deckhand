//! Port of textEditor.ts.
//!
//! The text editor shared by the config editor, the notes editor and the task editor. Cursors and row bounds are
//! byte offsets on char boundaries (TS: UTF-16 indices; parity fixtures convert, see the unit's `utf16_fields`).
//! Moving by one "character" moves by one code point, as the TS does by surrogate pair; columns count code points.
//!
//! Offsets past the end behave as in JS (one past-the-end unit is one byte here): slices clamp, `previous` steps back
//! by one. Nothing here panics on an offset off a char boundary; such an offset rounds down to its char.

use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::config_draft::{MAX_CONFIG_BYTES, MAX_CONFIG_LABEL};
use crate::jsstr::js_len;
use crate::key::Key;

/// TS: textEditor.ts EditorState. `message`: why the last edit was not applied (over the limit).
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorState {
    pub text: String,
    /// Byte offset into `text`, on a char boundary.
    pub cursor: usize,
    pub select_all: Option<bool>,
    pub message: Option<String>,
}

/// `text.slice(start, end)` on byte offsets: clamped to the text, rounded down to char boundaries, empty when
/// `start >= end`.
fn slice(text: &str, start: usize, end: usize) -> &str {
    let floor = |mut index: usize| {
        index = index.min(text.len());
        while !text.is_char_boundary(index) {
            index -= 1;
        }
        index
    };
    let (start, end) = (floor(start), floor(end));
    if start >= end { "" } else { &text[start..end] }
}

fn to_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// TS: textEditor.ts previous (the code point before `cursor`).
pub fn previous(text: &str, cursor: usize) -> usize {
    if cursor > text.len() {
        return cursor - 1;
    }
    match slice(text, 0, cursor).chars().next_back() {
        Some(character) => slice(text, 0, cursor).len() - character.len_utf8(),
        None => 0,
    }
}

/// TS: textEditor.ts next (the code point after `cursor`).
pub fn next(text: &str, cursor: usize) -> usize {
    match text.get(cursor..).and_then(|after| after.chars().next()) {
        Some(character) => cursor + character.len_utf8(),
        None => text.len().min(cursor.saturating_add(1)),
    }
}

/// TS: textEditor.ts lineStart.
pub fn line_start(text: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    slice(text, 0, cursor).rfind('\n').map_or(0, |index| index + 1)
}

/// TS: textEditor.ts lineEnd.
pub fn line_end(text: &str, cursor: usize) -> usize {
    text.get(cursor..)
        .and_then(|after| after.find('\n'))
        .map_or(text.len(), |index| cursor + index)
}

/// TS: textEditor.ts vertical (Up/Down by line, keeping the column in code points).
fn vertical(text: &str, cursor: usize, direction: isize) -> usize {
    let start = line_start(text, cursor);
    let column = slice(text, start, cursor).chars().count();
    let target_start = if direction < 0 {
        if start > 0 { line_start(text, start - 1) } else { start }
    } else if line_end(text, cursor) < text.len() {
        line_end(text, cursor) + 1
    } else {
        start
    };
    let target = slice(text, target_start, line_end(text, target_start));
    target_start + target.chars().take(column).map(char::len_utf8).sum::<usize>()
}

/// `/[\p{L}\p{N}_]/u.test(character)`.
fn is_word_character(character: &str) -> bool {
    static WORD: OnceLock<Regex> = OnceLock::new();
    WORD.get_or_init(|| Regex::new(r"[\p{L}\p{N}_]").expect("valid regex"))
        .is_match(character)
}

/// TS: textEditor.ts wordLeft (Alt/Ctrl+←): skips non-word characters (a newline included), then the word.
pub fn word_left(text: &str, cursor: usize) -> usize {
    let mut position = cursor;
    while position > 0 && !is_word_character(slice(text, previous(text, position), position)) {
        position = previous(text, position);
    }
    while position > 0 && is_word_character(slice(text, previous(text, position), position)) {
        position = previous(text, position);
    }
    position
}

/// TS: textEditor.ts wordRight (Alt/Ctrl+→).
pub fn word_right(text: &str, cursor: usize) -> usize {
    let mut position = cursor;
    while position < text.len() && !is_word_character(slice(text, position, next(text, position))) {
        position = next(text, position);
    }
    while position < text.len() && is_word_character(slice(text, position, next(text, position))) {
        position = next(text, position);
    }
    position
}

/// TS: textEditor.ts EditOptions (the defaults are the JSON config editor's).
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditOptions {
    /// Ctrl+A selects everything (default); otherwise Ctrl+A/Ctrl+E go to the line's start/end.
    pub select_all: Option<bool>,
    /// Text limit in UTF-16 units; the default is the config editor's byte limit (MAX_CONFIG_BYTES).
    pub max_chars: Option<usize>,
    pub limit_message: Option<String>,
    /// Tab inserts two spaces (default), or nothing.
    pub tab: Option<bool>,
    /// Width for Up/Down by visual (soft-wrapped) rows, and PageUp/PageDown by `page_rows` of them.
    pub wrap_width: Option<usize>,
    pub page_rows: Option<usize>,
}

/// TS: textEditor.ts cleanInsertedText (terminal sequences and controls other than newline and tab dropped).
pub fn clean_inserted_text(input: &str) -> String {
    static PATTERNS: OnceLock<[Regex; 3]> = OnceLock::new();
    let [osc, csi, controls] = PATTERNS.get_or_init(|| {
        [
            Regex::new(r"\x1b\][^\x07]*(?:\x07|\x1b\\)").expect("valid regex"),
            Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").expect("valid regex"),
            Regex::new(r"[\x00-\x08\x0b\x0c\x0e-\x1f\x7f-\x{9f}]").expect("valid regex"),
        ]
    });
    let text = osc.replace_all(input, "");
    let text = csi.replace_all(&text, "");
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    controls.replace_all(&text, "").into_owned()
}

/// TS: textEditor.ts replaceRange (`start..end` replaced within the limit, else the state with a `message`).
pub fn replace_range(
    state: &EditorState,
    start: usize,
    end: usize,
    inserted: &str,
    options: &EditOptions,
) -> EditorState {
    let mut changed = String::with_capacity(state.text.len() + inserted.len());
    changed.push_str(slice(&state.text, 0, start));
    changed.push_str(inserted);
    changed.push_str(slice(&state.text, end, state.text.len()));
    let over = match options.max_chars {
        Some(max_chars) => {
            let length = js_len(&changed);
            length > max_chars && length > js_len(&state.text)
        }
        None => changed.len() > MAX_CONFIG_BYTES,
    };
    if over {
        let message = options
            .limit_message
            .clone()
            .unwrap_or_else(|| format!("Draft exceeds {MAX_CONFIG_LABEL}; paste/edit was not applied."));
        return EditorState {
            message: Some(message),
            ..state.clone()
        };
    }
    EditorState {
        text: changed,
        cursor: start + inserted.len(),
        select_all: None,
        message: None,
    }
}

/// TS: textEditor.ts editText (one keypress: Ink's `input` and `key`).
pub fn edit_text(state: &EditorState, input: &str, key: &Key, options: &EditOptions) -> EditorState {
    let text = &state.text;
    let cursor = state.cursor;
    let select_all = state.select_all == Some(true);
    let select_all_key = options.select_all.unwrap_or(true);
    let word = key.meta || key.ctrl;
    let moved = |cursor: usize| EditorState {
        text: text.clone(),
        cursor,
        select_all: None,
        message: None,
    };
    if select_all_key && key.ctrl && input == "a" {
        return EditorState {
            select_all: Some(true),
            ..moved(text.len())
        };
    }
    if !select_all_key && key.ctrl && (input == "a" || input == "e") {
        return moved(if input == "a" {
            line_start(text, cursor)
        } else {
            line_end(text, cursor)
        });
    }
    if key.left_arrow {
        return moved(if select_all {
            0
        } else if word {
            word_left(text, cursor)
        } else {
            previous(text, cursor)
        });
    }
    if key.right_arrow {
        return moved(if select_all {
            text.len()
        } else if word {
            word_right(text, cursor)
        } else {
            next(text, cursor)
        });
    }
    // Alt+B / Alt+F: what macOS terminals send for Option+←/→ (Esc+ mode).
    if key.meta && !key.ctrl && (input == "b" || input == "f") {
        return moved(if input == "b" {
            word_left(text, cursor)
        } else {
            word_right(text, cursor)
        });
    }
    if key.home {
        return moved(if key.ctrl { 0 } else { line_start(text, cursor) });
    }
    if key.end {
        return moved(if key.ctrl { text.len() } else { line_end(text, cursor) });
    }
    // `options.wrapWidth ? ...`: 0 counts as unset.
    let wrap_width = options.wrap_width.filter(|width| *width != 0);
    if key.up_arrow || key.down_arrow {
        let direction = if key.up_arrow { -1 } else { 1 };
        return moved(match wrap_width {
            Some(width) => move_visual(text, cursor, width, direction, 1),
            None => vertical(text, cursor, direction),
        });
    }
    if let (true, Some(width)) = (key.page_up || key.page_down, wrap_width) {
        let direction = if key.page_up { -1 } else { 1 };
        return moved(move_visual(
            text,
            cursor,
            width,
            direction,
            options.page_rows.unwrap_or(1).max(1),
        ));
    }
    // Word deletion: Alt+Backspace, Ctrl+Backspace (where terminals send it), Ctrl+W.
    if (key.backspace && word) || (key.ctrl && input == "w") {
        if select_all {
            return replace_range(state, 0, text.len(), "", options);
        }
        return replace_range(state, word_left(text, cursor), cursor, "", options);
    }
    if key.delete && key.meta {
        return replace_range(state, cursor, word_right(text, cursor), "", options);
    }
    if key.ctrl || key.meta || key.escape || key.page_down || key.page_up {
        return state.clone();
    }
    let mut start = if select_all { 0 } else { cursor };
    let mut end = if select_all { text.len() } else { cursor };
    let inserted = if key.backspace {
        start = if select_all { 0 } else { previous(text, cursor) };
        String::new()
    } else if key.delete {
        end = if select_all { text.len() } else { next(text, cursor) };
        String::new()
    } else if key.return_ {
        String::from("\n")
    } else if key.tab {
        if options.tab == Some(false) {
            return state.clone();
        }
        String::from("  ")
    } else {
        clean_inserted_text(input)
    };
    if inserted.is_empty() && !key.backspace && !key.delete {
        return state.clone();
    }
    replace_range(state, start, end, &inserted, options)
}

/// TS: textEditor.ts EditorLine (a row split around the cursor's character).
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorLine {
    pub before: String,
    pub cursor: Option<String>,
    pub after: String,
}

/// TS: textEditor.ts editorLines / wrappedEditorLines result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorLines {
    pub lines: Vec<EditorLine>,
    pub cursor_row: usize,
}

/// A row without the cursor.
fn plain_line(content: &str) -> EditorLine {
    EditorLine {
        before: content.to_owned(),
        cursor: None,
        after: String::new(),
    }
}

/// The row holding the cursor at byte `offset`, split around the character there (a space past the end).
fn cursor_line(content: &str, offset: usize) -> EditorLine {
    let character = slice(content, offset, next(content, offset));
    EditorLine {
        before: slice(content, 0, offset).to_owned(),
        cursor: Some(if character.is_empty() { " " } else { character }.to_owned()),
        after: slice(content, offset.saturating_add(character.len()), content.len()).to_owned(),
    }
}

/// TS: textEditor.ts editorLines (hard-wrapped at `width` code points).
pub fn editor_lines(state: &EditorState, width: usize) -> EditorLines {
    let width = width.max(1);
    let cursor = to_i64(state.cursor);
    let mut lines = Vec::new();
    let (mut position, mut cursor_row) = (0_i64, 0);
    for line in state.text.split('\n') {
        let characters: Vec<char> = line.chars().collect();
        // A trailing space gives the cursor somewhere to sit at an exact wrap boundary.
        let segments = (characters.len() + 1).div_ceil(width).max(1);
        let mut consumed = 0_i64;
        for segment in 0..segments {
            let from = segment.saturating_mul(width).min(characters.len());
            let to = (segment + 1).saturating_mul(width).min(characters.len());
            let content: String = characters[from..to].iter().collect();
            let offset = cursor - position - consumed;
            let length = to_i64(content.len());
            let has_cursor = offset >= 0 && offset <= length && (offset < length || segment == segments - 1);
            if has_cursor {
                cursor_row = lines.len();
                lines.push(cursor_line(&content, usize::try_from(offset).unwrap_or(0)));
            } else {
                lines.push(plain_line(&content));
            }
            consumed += length;
        }
        position += to_i64(line.len()) + 1;
    }
    EditorLines { lines, cursor_row }
}

/// TS: textEditor.ts WrapRow: `text[start..end]` (byte offsets); `last`: the last row of its line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WrapRow {
    pub start: usize,
    pub end: usize,
    pub last: bool,
}

/// TS: textEditor.ts wrapRows (word-wrapped at `width` code points; `cursor_room`: an extra row after a line that
/// exactly fills its last row).
///
/// A line breaks after its last space that fits, else mid-word. `last` marks the last row of its line, where the
/// cursor may sit at `end`; elsewhere `end` is the next row's start, so the cursor there shows on the next row.
pub fn wrap_rows(text: &str, width: usize, cursor_room: bool) -> Vec<WrapRow> {
    let columns = width.max(1);
    let mut rows = Vec::new();
    let mut line_offset = 0;
    for line in text.split('\n') {
        let characters: Vec<char> = line.chars().collect();
        // Byte offset of each code point (and of the end).
        let mut offsets = Vec::with_capacity(characters.len() + 1);
        let mut total = 0;
        offsets.push(0);
        for character in &characters {
            total += character.len_utf8();
            offsets.push(total);
        }
        let mut from = 0;
        while characters.len() - from > columns {
            let mut cut = from + columns;
            if let Some(index) = (from + 1..from + columns).rev().find(|index| characters[*index] == ' ') {
                cut = index + 1;
            }
            rows.push(WrapRow {
                start: line_offset + offsets[from],
                end: line_offset + offsets[cut],
                last: false,
            });
            from = cut;
        }
        let start = line_offset + offsets[from];
        let end = line_offset + total;
        if cursor_room && characters.len() - from == columns && !characters.is_empty() {
            rows.push(WrapRow {
                start,
                end,
                last: false,
            });
            rows.push(WrapRow {
                start: end,
                end,
                last: true,
            });
        } else {
            rows.push(WrapRow { start, end, last: true });
        }
        line_offset += line.len() + 1;
    }
    rows
}

/// TS: textEditor.ts cursorRowIndex (the row showing `cursor`).
pub fn cursor_row_index(rows: &[WrapRow], cursor: usize) -> usize {
    rows.iter()
        .position(|row| cursor >= row.start && (cursor < row.end || (cursor == row.end && row.last)))
        .unwrap_or(rows.len().saturating_sub(1))
}

/// TS: textEditor.ts moveVisual (`direction`: -1 up, 1 down; `count` rows, keeping the column where the target row
/// has it).
pub fn move_visual(text: &str, cursor: usize, width: usize, direction: isize, count: usize) -> usize {
    let rows = wrap_rows(text, width, true);
    let from = cursor_row_index(&rows, cursor);
    // `from + direction * count`, as the TS computes it for any direction.
    let step = to_i64(count).saturating_mul(i64::try_from(direction).unwrap_or(0));
    let target = to_i64(from).saturating_add(step).min(to_i64(rows.len()) - 1).max(0);
    let target = usize::try_from(target).unwrap_or(0);
    let (Some(from_row), Some(row)) = (rows.get(from), rows.get(target)) else {
        return if direction < 0 { 0 } else { text.len() };
    };
    if target == from {
        return if direction < 0 { 0 } else { text.len() };
    }
    let column = slice(text, from_row.start, cursor).chars().count();
    let characters = slice(text, row.start, row.end);
    let count = characters.chars().count();
    // A row's end is the next row's start unless it ends its line.
    let room = if row.last { count } else { count.saturating_sub(1) };
    row.start
        + characters
            .chars()
            .take(column.min(room))
            .map(char::len_utf8)
            .sum::<usize>()
}

/// TS: textEditor.ts wrappedEditorLines (the soft-wrapped editor view: every row, the cursor's split around it).
pub fn wrapped_editor_lines(state: &EditorState, width: usize) -> EditorLines {
    let rows = wrap_rows(&state.text, width, true);
    let cursor_row = cursor_row_index(&rows, state.cursor);
    let lines = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let content = slice(&state.text, row.start, row.end);
            if index == cursor_row {
                cursor_line(content, state.cursor.saturating_sub(row.start))
            } else {
                plain_line(content)
            }
        })
        .collect();
    EditorLines { lines, cursor_row }
}

/// scrollTopFor on JS-like signed numbers.
fn scroll_top(previous_top: i64, cursor_row: i64, rows: i64, line_count: i64) -> i64 {
    let mut top = previous_top;
    if cursor_row < top {
        top = cursor_row;
    } else if cursor_row >= top.saturating_add(rows) {
        top = cursor_row - rows + 1;
    }
    top.min(line_count.saturating_sub(rows).max(0)).max(0)
}

/// TS: textEditor.ts scrollTopFor (the first visible row, moved only as far as needed to keep the cursor's row).
pub fn scroll_top_for(previous_top: usize, cursor_row: usize, rows: usize, line_count: usize) -> usize {
    let top = scroll_top(
        to_i64(previous_top),
        to_i64(cursor_row),
        to_i64(rows),
        to_i64(line_count),
    );
    usize::try_from(top).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use deckhand_testkit::strategies::unicode_text;
    use deckhand_testkit::{Case, replay};
    use proptest::prelude::*;
    use serde_json::{Value, json};

    use super::*;
    use crate::terminal_keys::normalize_terminal_key;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        #[test]
        fn character_steps_visit_each_unicode_scalar_boundary(text in unicode_text()) {
            let boundaries: Vec<_> = text.char_indices().map(|(byte, _)| byte)
                .chain(std::iter::once(text.len())).collect();
            for (index, &cursor) in boundaries.iter().enumerate() {
                prop_assert_eq!(previous(&text, cursor), boundaries[index.saturating_sub(1)]);
                prop_assert_eq!(next(&text, cursor), boundaries[(index + 1).min(boundaries.len() - 1)]);
            }
        }

        #[test]
        fn word_jumps_preserve_boundaries_and_progress_toward_endpoints(text in unicode_text()) {
            for cursor in text.char_indices().map(|(byte, _)| byte).chain(std::iter::once(text.len())) {
                let left = word_left(&text, cursor);
                let right = word_right(&text, cursor);
                prop_assert!(text.is_char_boundary(left));
                prop_assert!(text.is_char_boundary(right));
                prop_assert!(left <= cursor);
                prop_assert_eq!(left == cursor, cursor == 0);
                prop_assert!(right >= cursor && right <= text.len());
                prop_assert_eq!(right == cursor, cursor == text.len());
            }
        }

        #[test]
        fn line_and_visual_navigation_keep_unicode_cursors_valid(
            text in unicode_text(), width in 1usize..24, rows in 1usize..8,
        ) {
            for cursor in text.char_indices().map(|(byte, _)| byte).chain(std::iter::once(text.len())) {
                for moved in [
                    line_start(&text, cursor), line_end(&text, cursor),
                    vertical(&text, cursor, -1), vertical(&text, cursor, 1),
                    move_visual(&text, cursor, width, -1, rows),
                    move_visual(&text, cursor, width, 1, rows),
                ] {
                    prop_assert!(moved <= text.len());
                    prop_assert!(text.is_char_boundary(moved));
                }
            }
        }
    }

    fn key(value: Value) -> Key {
        serde_json::from_value(value).expect("a Key")
    }

    fn st(text: &str, cursor: usize) -> EditorState {
        EditorState {
            text: text.to_owned(),
            cursor,
            ..EditorState::default()
        }
    }

    fn edit(state: &EditorState, input: &str, keys: Value) -> EditorState {
        edit_text(state, input, &key(keys), &EditOptions::default())
    }

    fn notes_options() -> EditOptions {
        EditOptions {
            select_all: Some(false),
            max_chars: Some(20),
            limit_message: Some(String::from("too long")),
            tab: Some(false),
            ..EditOptions::default()
        }
    }

    // covers: B-P11-01 editor inserts, selects/replaces, deletes and indents
    #[test]
    fn inserts_selects_deletes_indents() {
        let mut state = st("abcd", 2);
        state = edit(&state, "X", json!({}));
        assert_eq!(state.text, "abXcd");
        state = edit(&state, "", json!({"backspace": true}));
        assert_eq!(state.text, "abcd");
        state = edit(&state, "", json!({"delete": true}));
        assert_eq!(state.text, "abd");
        state = edit(&state, "a", json!({"ctrl": true}));
        assert_eq!(state.select_all, Some(true));
        state = edit(&state, "{}", json!({}));
        assert_eq!(state.text, "{}");
        assert_eq!(state.cursor, 2);
        state = edit(&state, "", json!({"return": true}));
        state = edit(&state, "", json!({"tab": true}));
        assert_eq!(state.text, "{}\n  ");
    }

    // covers: B-P11-02 arrows and home/end navigate multiline text and Unicode without splitting surrogate pairs
    #[test]
    fn arrows_and_home_end() {
        let mut state = st("abc\nxy\nlong", 2);
        state = edit(&state, "", json!({"downArrow": true}));
        assert_eq!(state.cursor, 6);
        state = edit(&state, "", json!({"downArrow": true}));
        assert_eq!(state.cursor, 9);
        state = edit(&state, "", json!({"home": true}));
        assert_eq!(state.cursor, 7);
        state = edit(&state, "", json!({"end": true}));
        assert_eq!(state.cursor, 11);
        state = edit(&state, "", json!({"home": true, "ctrl": true}));
        assert_eq!(state.cursor, 0);
        // TS UTF-16 offsets 3 / 1 / 3 are bytes 5 / 1 / 5 here.
        state = st("a😀b", 5);
        state = edit(&state, "", json!({"leftArrow": true}));
        assert_eq!(state.cursor, 1);
        state = edit(&state, "", json!({"rightArrow": true}));
        assert_eq!(state.cursor, 5);
        state = edit(&state, "", json!({"backspace": true}));
        assert_eq!(state.text, "ab");
    }

    // covers: B-P11-03 paste normalizes newlines and strips terminal control sequences; oversized paste is not applied
    #[test]
    fn paste_normalizes_and_limit() {
        let state = edit(&st("", 0), "\x1b[200~{\r\n}\x1b[201~", json!({}));
        assert_eq!(state.text, "{\n}");
        let limited = edit(&state, &"x".repeat(65536), json!({}));
        assert_eq!(limited.text, state.text);
        assert!(limited.message.unwrap_or_default().contains("64 KiB"));
    }

    // covers: B-P11-04 rendering keeps the cursor visible on wrapped boundaries and empty lines
    #[test]
    fn rendering_keeps_cursor_visible() {
        let wrapped = editor_lines(&st("abcd", 4), 4);
        assert_eq!(wrapped.cursor_row, 1);
        assert_eq!(wrapped.lines[1].cursor.as_deref(), Some(" "));
        let middle = editor_lines(&st("abcd", 2), 4);
        assert_eq!(middle.lines[0].before, "ab");
        assert_eq!(middle.lines[0].cursor.as_deref(), Some("c"));
        assert_eq!(middle.lines[0].after, "d");
        let empty = editor_lines(&st("\n", 0), 10);
        assert_eq!(empty.cursor_row, 0);
        assert_eq!(empty.lines[0].cursor.as_deref(), Some(" "));
    }

    // covers: B-P11-05 word jumps (Alt/Ctrl+arrows, Alt+B/F), word deletion, and line start/end on Ctrl+A/E without select-all
    #[test]
    fn word_jumps_and_deletion() {
        let text = "fix the  login-bug\nnext";
        assert_eq!(word_left(text, 18), 15);
        assert_eq!(word_left(text, 15), 9);
        assert_eq!(word_left(text, 9), 4);
        assert_eq!(word_left(text, 0), 0);
        assert_eq!(word_right(text, 0), 3);
        assert_eq!(word_right(text, 3), 7);
        assert_eq!(word_right(text, 18), 23);
        let mut state = st(text, 18);
        state = edit(&state, "", json!({"leftArrow": true, "meta": true}));
        assert_eq!(state.cursor, 15);
        state = edit(&state, "", json!({"leftArrow": true, "ctrl": true}));
        assert_eq!(state.cursor, 9);
        state = edit(&state, "f", json!({"meta": true}));
        assert_eq!(state.cursor, 14);
        state = edit(&state, "b", json!({"meta": true}));
        assert_eq!(state.cursor, 9);
        state = edit(&state, "", json!({"rightArrow": true, "ctrl": true}));
        assert_eq!(state.cursor, 14);
        // Alt+Backspace (ESC DEL) and Ctrl+W delete the word before the cursor; Alt+Delete the one after.
        let alt_backspace = normalize_terminal_key(key(json!({"delete": true, "meta": true})), "\x1b\x7f");
        let deleted = st("fix the  -bug\nnext", 9);
        assert_eq!(edit_text(&state, "", &alt_backspace, &EditOptions::default()), deleted);
        assert_eq!(edit(&state, "w", json!({"ctrl": true})), deleted);
        assert_eq!(edit(&st(text, 9), "", json!({"delete": true, "meta": true})), deleted);
        // Notes options: Ctrl+A/Ctrl+E go to the line's start/end; Tab is left to the caller.
        let notes = notes_options();
        assert_eq!(
            edit_text(&st(text, 21), "a", &key(json!({"ctrl": true})), &notes),
            st(text, 19)
        );
        assert_eq!(
            edit_text(&st(text, 2), "e", &key(json!({"ctrl": true})), &notes),
            st(text, 18)
        );
        assert_eq!(edit_text(&st("a", 1), "", &key(json!({"tab": true})), &notes).text, "a");
    }

    // covers: B-P11-06 insertions and deletions anywhere; the character limit refuses growth but allows shrinking
    #[test]
    fn insertions_deletions_and_limit() {
        let notes = notes_options();
        let mut state = edit_text(&st("hello world", 5), ",", &Key::NONE, &notes);
        assert_eq!(state, st("hello, world", 6));
        state = edit_text(
            &EditorState { cursor: 0, ..state },
            "",
            &key(json!({"delete": true})),
            &notes,
        );
        assert_eq!(state, st("ello, world", 0));
        state = edit_text(
            &EditorState { cursor: 4, ..state },
            "",
            &key(json!({"return": true})),
            &notes,
        );
        assert_eq!(state, st("ello\n, world", 5));
        state = edit_text(&state, "x\r\ny", &Key::NONE, &notes);
        assert_eq!(state.text, "ello\nx\ny, world");
        assert_eq!(
            edit_text(&state, "0123456789", &Key::NONE, &notes).message.as_deref(),
            Some("too long")
        );
        let long = st(&"x".repeat(30), 30);
        assert_eq!(
            edit_text(&long, "", &key(json!({"backspace": true})), &notes)
                .text
                .len(),
            29
        );
    }

    // covers: B-P11-07 soft wrap: words break at spaces, long words mid-word, the cursor gets a row at an exact fit; Up/Down move by visual rows
    #[test]
    fn soft_wrap() {
        let text = "the quick brown fox\nabcdefghij";
        let rows: Vec<&str> = wrap_rows(text, 10, true)
            .iter()
            .map(|row| &text[row.start..row.end])
            .collect();
        assert_eq!(rows, ["the quick ", "brown fox", "abcdefghij", ""]);
        let exact = WrapRow {
            start: 0,
            end: 10,
            last: true,
        };
        assert_eq!(wrap_rows("abcdefghij", 10, false), [exact]);
        let view = wrapped_editor_lines(&st(text, 10), 10);
        // The cursor at a wrap boundary shows at the start of the next row.
        assert_eq!(view.cursor_row, 1);
        let expected = EditorLine {
            before: String::new(),
            cursor: Some(String::from("b")),
            after: String::from("rown fox"),
        };
        assert_eq!(view.lines[1], expected);
        assert_eq!(wrapped_editor_lines(&st(text, text.len()), 10).cursor_row, 3);
        assert_eq!(cursor_row_index(&wrap_rows(text, 10, true), 19), 1);
        // Up/Down keep the column within the wrapped paragraph instead of jumping a whole line.
        assert_eq!(move_visual(text, 2, 10, 1, 1), 12);
        assert_eq!(move_visual(text, 12, 10, 1, 1), 22);
        assert_eq!(move_visual(text, 22, 10, -1, 1), 12);
        assert_eq!(move_visual(text, 4, 10, -1, 1), 0);
        assert_eq!(move_visual(text, 25, 10, 1, 1), text.len());
        // A wrapped row's end is the next row's start: Down from column 4 onto a full 5-column row stays on that row.
        assert_eq!(move_visual("aaaa bbbb\ncccccccccccc", 9, 5, 1, 1), 14);
        // The cursor after a line that fills its row sits on an extra row (column 0), so Down goes to column 0.
        assert_eq!(move_visual("aaaaa\nbbbbbbb", 5, 5, 1, 1), 6);
        let wrap = EditOptions {
            wrap_width: Some(10),
            ..EditOptions::default()
        };
        assert_eq!(
            edit_text(&st(text, 2), "", &key(json!({"downArrow": true})), &wrap).cursor,
            12
        );
        let page = EditOptions {
            wrap_width: Some(10),
            page_rows: Some(2),
            ..EditOptions::default()
        };
        assert_eq!(
            edit_text(&st(text, 2), "", &key(json!({"pageDown": true})), &page).cursor,
            22
        );
    }

    // covers: B-P11-08 scrolling follows the cursor: only as far as needed, and never past the end
    #[test]
    fn scrolling_follows_cursor() {
        assert_eq!(scroll_top_for(0, 3, 5, 20), 0);
        assert_eq!(scroll_top_for(0, 7, 5, 20), 3);
        assert_eq!(scroll_top_for(6, 2, 5, 20), 2);
        assert_eq!(scroll_top_for(18, 19, 5, 20), 15);
        assert_eq!(scroll_top_for(4, 1, 5, 3), 0);
    }

    #[test]
    fn untrusted_offsets_do_not_panic() {
        let text = "a😀\nb";
        for cursor in 0..12 {
            let state = st(text, cursor);
            let _ = (
                previous(text, cursor),
                next(text, cursor),
                line_start(text, cursor),
                line_end(text, cursor),
            );
            let _ = (
                word_left(text, cursor),
                word_right(text, cursor),
                vertical(text, cursor, 1),
            );
            let _ = (
                editor_lines(&state, 1),
                wrapped_editor_lines(&state, 1),
                move_visual(text, cursor, 1, -1, 2),
            );
            for keys in [
                json!({"backspace": true}),
                json!({"upArrow": true}),
                json!({"delete": true}),
            ] {
                let _ = edit(&state, "x", keys);
            }
            let _ = replace_range(&state, cursor, 2, "z", &EditOptions::default());
        }
    }

    // Parity replays (fixtures/cases/textEditor).

    fn options(case: &Case, index: usize) -> Result<EditOptions, String> {
        if case.arg_is_undefined(index) {
            Ok(EditOptions::default())
        } else {
            case.arg_as(index)
        }
    }

    #[test]
    fn offsets_parity() {
        for (fixture, port) in [
            ("textEditor/lineStart", line_start as fn(&str, usize) -> usize),
            ("textEditor/lineEnd", line_end),
            ("textEditor/wordLeft", word_left),
            ("textEditor/wordRight", word_right),
        ] {
            replay(fixture, |case: &Case| -> Result<usize, String> {
                Ok(port(case.arg_str(0)?, case.arg_usize(1)?))
            });
        }
    }

    #[test]
    fn clean_inserted_text_parity() {
        replay(
            "textEditor/cleanInsertedText",
            |case: &Case| -> Result<String, String> { Ok(clean_inserted_text(case.arg_str(0)?)) },
        );
    }

    #[test]
    fn replace_range_parity() {
        replay(
            "textEditor/replaceRange",
            |case: &Case| -> Result<EditorState, String> {
                Ok(replace_range(
                    &case.arg_as(0)?,
                    case.arg_usize(1)?,
                    case.arg_usize(2)?,
                    case.arg_str(3)?,
                    &options(case, 4)?,
                ))
            },
        );
    }

    #[test]
    fn edit_text_parity() {
        replay("textEditor/editText", |case: &Case| -> Result<EditorState, String> {
            Ok(edit_text(
                &case.arg_as(0)?,
                case.arg_str(1)?,
                &case.arg_as(2)?,
                &options(case, 3)?,
            ))
        });
    }

    #[test]
    fn editor_lines_parity() {
        for (fixture, port) in [
            (
                "textEditor/editorLines",
                editor_lines as fn(&EditorState, usize) -> EditorLines,
            ),
            ("textEditor/wrappedEditorLines", wrapped_editor_lines),
        ] {
            replay(fixture, |case: &Case| -> Result<EditorLines, String> {
                Ok(port(&case.arg_as(0)?, case.arg_usize(1)?))
            });
        }
    }

    /// A JS width as wrapRows uses it (`Math.max(1, Math.floor(width))`).
    fn js_width(width: f64) -> usize {
        // Truncation is the point: the value is already floored and clamped.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let width = width.floor().clamp(1.0, 1e9) as usize;
        width
    }

    #[test]
    fn wrap_rows_parity() {
        replay("textEditor/wrapRows", |case: &Case| -> Result<Vec<WrapRow>, String> {
            let room = !case.arg_is_undefined(2) && case.arg_bool(2)?;
            Ok(wrap_rows(case.arg_str(0)?, js_width(case.arg_f64(1)?), room))
        });
    }

    #[test]
    fn cursor_row_index_parity() {
        replay("textEditor/cursorRowIndex", |case: &Case| -> Result<usize, String> {
            let rows: Vec<WrapRow> = case.arg_as(0)?;
            Ok(cursor_row_index(&rows, case.arg_usize(1)?))
        });
    }

    #[test]
    fn move_visual_parity() {
        replay("textEditor/moveVisual", |case: &Case| -> Result<usize, String> {
            let count = if case.arg_is_undefined(4) {
                1
            } else {
                case.arg_usize(4)?
            };
            let direction = isize::try_from(case.arg_i64(3)?).map_err(|error| error.to_string())?;
            let width = js_width(case.arg_f64(2)?);
            Ok(move_visual(
                case.arg_str(0)?,
                case.arg_usize(1)?,
                width,
                direction,
                count,
            ))
        });
    }

    #[test]
    fn scroll_top_for_parity() {
        replay("textEditor/scrollTopFor", |case: &Case| -> Result<i64, String> {
            Ok(scroll_top(
                case.arg_i64(0)?,
                case.arg_i64(1)?,
                case.arg_i64(2)?,
                case.arg_i64(3)?,
            ))
        });
    }
}
