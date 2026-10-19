//! Port of textEditor.ts.
//!
//! The text editor shared by the config editor, the notes editor and the task editor. Cursors and row bounds are
//! byte offsets on char boundaries (TS: UTF-16 indices; parity fixtures convert, see the unit's `utf16_fields`).
//! Moving by one "character" moves by one code point, as the TS does by surrogate pair; columns count code points.

use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

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

/// TS: textEditor.ts previous (the code point before `cursor`).
pub fn previous(text: &str, cursor: usize) -> usize {
    let _ = (text, cursor);
    todo!("P11")
}

/// TS: textEditor.ts next (the code point after `cursor`).
pub fn next(text: &str, cursor: usize) -> usize {
    let _ = (text, cursor);
    todo!("P11")
}

/// TS: textEditor.ts lineStart.
pub fn line_start(text: &str, cursor: usize) -> usize {
    let _ = (text, cursor);
    todo!("P11")
}

/// TS: textEditor.ts lineEnd.
pub fn line_end(text: &str, cursor: usize) -> usize {
    let _ = (text, cursor);
    todo!("P11")
}

/// TS: textEditor.ts wordLeft (Alt/Ctrl+←).
pub fn word_left(text: &str, cursor: usize) -> usize {
    let _ = (text, cursor);
    todo!("P11")
}

/// TS: textEditor.ts wordRight (Alt/Ctrl+→).
pub fn word_right(text: &str, cursor: usize) -> usize {
    let _ = (text, cursor);
    todo!("P11")
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
    let _ = input;
    todo!("P11")
}

/// TS: textEditor.ts replaceRange (`start..end` replaced within the limit, else the state with a `message`).
pub fn replace_range(
    state: &EditorState,
    start: usize,
    end: usize,
    inserted: &str,
    options: &EditOptions,
) -> EditorState {
    let _ = (state, start, end, inserted, options);
    todo!("P11")
}

/// TS: textEditor.ts editText (one keypress: Ink's `input` and `key`).
pub fn edit_text(state: &EditorState, input: &str, key: &Key, options: &EditOptions) -> EditorState {
    let _ = (state, input, key, options);
    todo!("P11")
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

/// TS: textEditor.ts editorLines (hard-wrapped at `width` code points).
pub fn editor_lines(state: &EditorState, width: usize) -> EditorLines {
    let _ = (state, width);
    todo!("P11")
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
pub fn wrap_rows(text: &str, width: usize, cursor_room: bool) -> Vec<WrapRow> {
    let _ = (text, width, cursor_room);
    todo!("P11")
}

/// TS: textEditor.ts cursorRowIndex.
pub fn cursor_row_index(rows: &[WrapRow], cursor: usize) -> usize {
    let _ = (rows, cursor);
    todo!("P11")
}

/// TS: textEditor.ts moveVisual (`direction`: -1 up, 1 down; `count` rows, keeping the column).
pub fn move_visual(text: &str, cursor: usize, width: usize, direction: isize, count: usize) -> usize {
    let _ = (text, cursor, width, direction, count);
    todo!("P11")
}

/// TS: textEditor.ts wrappedEditorLines (the soft-wrapped editor view).
pub fn wrapped_editor_lines(state: &EditorState, width: usize) -> EditorLines {
    let _ = (state, width);
    todo!("P11")
}

/// TS: textEditor.ts scrollTopFor (the first visible row, moved only as far as needed to keep the cursor's row).
pub fn scroll_top_for(previous_top: usize, cursor_row: usize, rows: usize, line_count: usize) -> usize {
    let _ = (previous_top, cursor_row, rows, line_count);
    todo!("P11")
}
