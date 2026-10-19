//! Port of ui.ts and the pure helpers of menu.tsx (wrapWords, fitParts, fitHint, menuWindowStart, ...).
//!
//! Widths here are TS `.length` (UTF-16 units), as the TS measures them (see `jsstr`); the ratatui renderers in
//! `deckhand-ui` draw what these produce. Colors are Ink color names (ANSI named colors, so the palette follows the
//! terminal's theme).

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::session_order::SessionLike;
use deckhand_protocol::data::{ProgramKey, SessionRecord};

/// An Ink color name (`magenta`, `gray`, ...).
pub type Color = &'static str;

/// TS: ui.ts THEME.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Theme {
    pub accent: Color,
    pub accent_soft: Color,
    pub active: Color,
    pub muted: Color,
    pub border: Color,
    pub border_active: Color,
    pub border_danger: Color,
    pub success: Color,
    pub warn: Color,
    pub error: Color,
}

/// TS: ui.ts THEME.
pub const THEME: Theme = Theme {
    accent: "magenta",
    accent_soft: "magentaBright",
    active: "cyan",
    muted: "gray",
    border: "gray",
    border_active: "magenta",
    border_danger: "red",
    success: "green",
    warn: "yellow",
    error: "red",
};

/// TS: ui.ts errorMessage (`error.message`, else `String(error)`).
pub fn error_message(error: &dyn std::fmt::Display) -> String {
    let _ = error;
    todo!("P12")
}

/// TS: ui.ts TERMINAL_ESCAPE_PATTERN (CSI, OSC and two-character escape sequences).
pub static TERMINAL_ESCAPE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\x1B(?:\[[0-?]*[ -/]*[@-~]|\][^\x07\x1B]*(?:\x07|\x1B\\)?|[@-Z\\-_])")
        .expect("TERMINAL_ESCAPE_PATTERN is a valid regex")
});

/// TS: ui.ts DISPLAY_CONTROL_PATTERN (C0/C1 controls except tab and newline).
pub static DISPLAY_CONTROL_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\x00-\x08\x0B-\x1F\x7F-\u{9F}]").expect("DISPLAY_CONTROL_PATTERN is a valid regex"));

/// TS: ui.ts stripTerminalControls (single-line typed input: escapes and every control removed).
pub fn strip_terminal_controls(text: &str) -> String {
    let _ = text;
    todo!("P12")
}

/// TS: ui.ts plainTerminalText (captured output as plain text: what a carriage return leaves visible per line).
pub fn plain_terminal_text(text: &str) -> String {
    let _ = text;
    todo!("P12")
}

/// TS: ui.ts truncate (`width` UTF-16 units, the last one `…` when cut).
pub fn truncate(text: &str, width: usize) -> String {
    let _ = (text, width);
    todo!("P12")
}

/// TS: ui.ts fitLines (`height` lines cut to `width`; blank rows as a space).
pub fn fit_lines(text: &str, width: usize, height: usize) -> Vec<String> {
    let _ = (text, width, height);
    todo!("P12")
}

/// TS: ui.ts compactPath (`~` for `$HOME`, then `…/<last two>` when too wide).
pub fn compact_path(path: &str, width: usize) -> String {
    let _ = (path, width);
    todo!("P12")
}

/// TS: ui.ts programGlyph.
pub fn program_glyph(program: ProgramKey) -> &'static str {
    let _ = program;
    todo!("P12")
}

/// TS: ui.ts statusGlyph (`spinner_frame` while working or starting).
pub fn status_glyph(session: &SessionRecord, spinner_frame: &str) -> String {
    let _ = (session, spinner_frame);
    todo!("P12")
}

/// TS: ui.ts statusColor.
pub fn status_color(session: &SessionRecord) -> Color {
    let _ = session;
    todo!("P12")
}

/// TS: ui.ts displaySessionTitle (whitespace collapsed; a sub-session's parent prefix removed; ` / ` → `/`).
pub fn display_session_title<S: SessionLike>(session: &SessionRecord, sessions: &[S]) -> String {
    let _ = (session, sessions);
    todo!("P12")
}

/// TS: ui.ts statusLabel.
pub fn status_label(session: Option<&SessionRecord>) -> String {
    let _ = session;
    todo!("P12")
}

// ---------------------------------------------------------------------------------------------------------------
// menu.tsx pure helpers (the components themselves are rendered by deckhand-ui)

/// TS: menu.tsx SELECTION_MARKER.
pub const SELECTION_MARKER: &str = "❯";

/// TS: menu.tsx RowStatus.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RowStatus {
    pub text: String,
    pub color: Option<Color>,
}

/// TS: menu.tsx MenuItem.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuItem {
    pub key: String,
    pub label: String,
    pub description: Option<String>,
    pub status: Option<RowStatus>,
    pub color: Option<Color>,
    pub selected_color: Option<Color>,
    pub dim: Option<bool>,
}

/// TS: menu.tsx menuWindowStart (the first visible index that keeps `selected` on screen within `rows`).
pub fn menu_window_start(selected: usize, count: usize, rows: usize) -> usize {
    let _ = (selected, count, rows);
    todo!("P12")
}

/// TS: menu.tsx scrolledListTop (`current`: the selection, `None` for none; `selectable`: whether a row index can
/// be selected).
pub fn scrolled_list_top(
    top: usize,
    current: Option<usize>,
    rows: usize,
    total: usize,
    selectable: &dyn Fn(usize) -> bool,
) -> usize {
    let _ = (top, current, rows, total, selectable);
    todo!("P12")
}

/// TS: menu.tsx wrapWords (word-wrapped to `width` UTF-16 units, long words cut; at least one line).
pub fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let _ = (text, width);
    todo!("P12")
}

/// TS: menu.tsx DetailPart.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DetailPart {
    pub text: String,
    pub color: Option<Color>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
}

/// TS: menu.tsx DetailLine (`nowrap`: cut instead of wrapped; `parts`: its colored segments).
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DetailLine {
    pub text: String,
    pub color: Option<Color>,
    pub nowrap: Option<bool>,
    pub parts: Option<Vec<DetailPart>>,
}

/// TS: menu.tsx fitParts (segments cut to `width` columns in total, in order).
pub fn fit_parts(parts: &[DetailPart], width: usize) -> Vec<DetailPart> {
    let _ = (parts, width);
    todo!("P12")
}

/// TS: menu.tsx HintPart (a plain string hint is a part with only `text`). Higher `drop` goes first.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HintPart {
    pub text: String,
    pub short: Option<String>,
    pub drop: Option<i32>,
}

impl From<&str> for HintPart {
    /// A plain hint (TS: a string in the parts list).
    fn from(text: &str) -> Self {
        let _ = text;
        todo!("P12")
    }
}

/// TS: menu.tsx fitHint (full texts, then short forms, then dropping parts; `separator` defaults to ` • `).
pub fn fit_hint(parts: &[HintPart], width: usize, separator: &str) -> String {
    let _ = (parts, width, separator);
    todo!("P12")
}

/// TS: menu.tsx fitHint's default separator.
pub const HINT_SEPARATOR: &str = " • ";

/// TS: menu.tsx MenuPaneLine.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct MenuPaneLine {
    pub text: String,
    pub color: Option<Color>,
}

/// TS: menu.tsx menuDetailsRows (rows a MenuDetails box needs, border and title included).
pub fn menu_details_rows(lines: &[DetailLine], width: usize) -> usize {
    let _ = (lines, width);
    todo!("P12")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_compile_and_match_like_js() {
        assert_eq!(
            TERMINAL_ESCAPE_PATTERN.replace_all("a\x1b[31mb\x1b]0;t\x07c\x1bMd", ""),
            "abcd"
        );
        assert_eq!(
            DISPLAY_CONTROL_PATTERN.replace_all("a\x01\tb\n\u{9f}😀", ""),
            "a\tb\n😀"
        );
    }
}
