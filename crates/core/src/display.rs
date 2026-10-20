//! Port of ui.ts and the pure helpers of menu.tsx (wrapWords, fitParts, fitHint, menuWindowStart, ...).
//!
//! Widths here are TS `.length` (UTF-16 units), as the TS measures them (see `jsstr`); the ratatui renderers in
//! `deckhand-ui` draw what these produce. Colors are Ink color names (ANSI named colors, so the palette follows the
//! terminal's theme).

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::jsstr::{js_collapse_whitespace, js_len, js_slice, js_split_whitespace, js_trim};
use crate::session_order::SessionLike;
use deckhand_protocol::data::{
    AgentActivityStatus, AttentionState, ExitReason, ProgramKey, SessionRecord, SessionStatus, SetupState,
};

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

/// A 24-bit color.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// Deckhand's own colors (docs/brand/deckhand-design-system.html, "Terminal Native, dusk").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// The page; everything sits on it.
    pub ground: Rgb,
    /// Panels and cards.
    pub surface: Rgb,
    /// Inside a terminal window: the UI's ground.
    pub term: Rgb,
    /// Boxes inside the terminal.
    pub box_fill: Rgb,
    /// Title bars, tab strips.
    pub chrome: Rgb,
    /// Headlines, body text.
    pub ink: Rgb,
    /// Secondary text.
    pub muted: Rgb,
    /// Rules, borders, hints.
    pub dim: Rgb,
    /// Thin rules in chrome.
    pub rule: Rgb,
    /// Box and window outlines.
    pub edge: Rgb,
    /// Grid dots.
    pub faint: Rgb,
    /// Text on fills (the selection bar, chips).
    pub ink_dark: Rgb,
    /// Brand: logo, accents, cursor, focused borders.
    pub magenta: Rgb,
    /// Brand titles in the product.
    pub magenta_bright: Rgb,
    /// Active / selected.
    pub cyan: Rgb,
    /// Success: working, done, additions.
    pub green: Rgb,
    /// Attention and annotation.
    pub yellow: Rgb,
    /// Error: failures, deletions.
    pub red: Rgb,
    /// Third-party marks.
    pub orange: Rgb,
}

/// The dusk palette's tokens.
pub const DUSK: Palette = Palette {
    ground: Rgb(0x4a, 0x52, 0x60),
    surface: Rgb(0x3c, 0x43, 0x4f),
    term: Rgb(0x36, 0x3c, 0x46),
    box_fill: Rgb(0x3f, 0x46, 0x51),
    chrome: Rgb(0x42, 0x49, 0x55),
    ink: Rgb(0xf4, 0xf2, 0xec),
    muted: Rgb(0xc3, 0xc8, 0xd0),
    dim: Rgb(0x84, 0x8c, 0x99),
    rule: Rgb(0x59, 0x61, 0x6e),
    edge: Rgb(0x9a, 0xa2, 0xae),
    faint: Rgb(0x55, 0x5d, 0x6b),
    ink_dark: Rgb(0x2a, 0x2f, 0x38),
    magenta: Rgb(0xf7, 0xa8, 0xf0),
    magenta_bright: Rgb(0xfb, 0xcd, 0xf6),
    cyan: Rgb(0x86, 0xe2, 0xee),
    green: Rgb(0xb8, 0xe6, 0x9c),
    yellow: Rgb(0xf6, 0xd4, 0x7e),
    red: Rgb(0xff, 0xa2, 0x9b),
    orange: Rgb(0xff, 0xb9, 0x8f),
};

/// How Deckhand colors its own UI.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ThemeMode {
    /// The dusk palette in 24-bit color, on its own background.
    Dusk,
    /// The 16 ANSI named colors, on the terminal's background, so the palette follows the terminal's theme.
    #[default]
    Terminal,
}

/// The theme for the `theme` setting of config.json: dusk only when it is `"dusk"` (an opt-in, e.g. for recording
/// demos in the brand's colors), otherwise terminal.
pub fn theme_mode(setting: Option<&str>) -> ThemeMode {
    match setting {
        Some("dusk") => ThemeMode::Dusk,
        _ => ThemeMode::Terminal,
    }
}

/// TS: ui.ts errorMessage (`error.message`, else `String(error)`).
pub fn error_message(error: &dyn std::fmt::Display) -> String {
    error.to_string()
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
    let text = TERMINAL_ESCAPE_PATTERN.replace_all(text, "");
    let text = DISPLAY_CONTROL_PATTERN.replace_all(&text, "");
    text.replace(['\t', '\n'], "")
}

/// TS: ui.ts plainTerminalText (captured output as plain text: what a carriage return leaves visible per line).
pub fn plain_terminal_text(text: &str) -> String {
    TERMINAL_ESCAPE_PATTERN
        .replace_all(text, "")
        .split('\n')
        .map(|line| {
            let trimmed = line.trim_end_matches('\r');
            let visible = trimmed.rfind('\r').map_or(trimmed, |at| &trimmed[at + 1..]);
            DISPLAY_CONTROL_PATTERN.replace_all(visible, "").replace('\t', "  ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A Rust width as the JS number it stands for.
fn js_width(width: usize) -> i64 {
    i64::try_from(width).unwrap_or(i64::MAX)
}

/// A non-negative JS result as a Rust index.
fn to_usize(value: i64) -> usize {
    usize::try_from(value.max(0)).unwrap_or(usize::MAX)
}

/// TS: ui.ts truncate (`width` UTF-16 units, the last one `…` when cut).
pub fn truncate(text: &str, width: usize) -> String {
    truncate_js(text, js_width(width))
}

/// [`truncate`] with a JS width (zero or less: nothing).
fn truncate_js(text: &str, width: i64) -> String {
    if width <= 0 {
        return String::new();
    }
    if js_width(js_len(text)) <= width {
        return text.to_owned();
    }
    if width == 1 {
        js_slice(text, 0, Some(1))
    } else {
        format!("{}…", js_slice(text, 0, Some(width - 1)))
    }
}

/// TS: ui.ts fitLines (`height` lines cut to `width`; blank rows as a space).
pub fn fit_lines(text: &str, width: usize, height: usize) -> Vec<String> {
    fit_lines_js(text, js_width(width), height)
}

fn fit_lines_js(text: &str, width: i64, height: usize) -> Vec<String> {
    let mut lines: Vec<String> = text
        .split('\n')
        .take(height)
        .map(|line| truncate_js(line, width))
        .collect();
    lines.resize(height, String::new());
    // Ink can collapse empty <Text> nodes. Render blank terminal rows as a space so full-screen TUIs keep their
    // vertical positioning in previews.
    for line in &mut lines {
        if line.is_empty() {
            line.push(' ');
        }
    }
    lines
}

/// TS: ui.ts compactPath (`~` for `$HOME`, then `…/<last two>` when too wide).
pub fn compact_path(path: &str, width: usize) -> String {
    let home = std::env::var_os("HOME");
    compact_path_home(path, home.as_deref().and_then(|home| home.to_str()), width)
}

/// [`compact_path`] with an explicit `$HOME` (`None` or empty: no `~`).
fn compact_path_home(path: &str, home: Option<&str>, width: usize) -> String {
    let display = match home {
        Some(home) if !home.is_empty() && path.starts_with(home) => format!("~{}", &path[home.len()..]),
        _ => path.to_owned(),
    };
    if js_len(&display) <= width {
        return display;
    }
    let parts: Vec<&str> = display.split('/').filter(|part| !part.is_empty()).collect();
    if parts.len() <= 2 {
        return truncate(&display, width);
    }
    let compact = format!("…/{}", parts[parts.len() - 2..].join("/"));
    truncate(&compact, width)
}

/// TS: ui.ts programGlyph.
pub fn program_glyph(program: ProgramKey) -> &'static str {
    match program {
        ProgramKey::Claude => "✶",
        ProgramKey::Pi => "π",
        ProgramKey::Codex => "◇",
    }
}

/// TS: ui.ts statusGlyph (`spinner_frame` while working or starting).
pub fn status_glyph(session: &SessionRecord, spinner_frame: &str) -> String {
    let attention = session.attention.as_ref().map(|attention| attention.state);
    if session.status == SessionStatus::Running {
        match attention {
            Some(AttentionState::NeedsInput) => return "?".to_owned(),
            Some(AttentionState::ResponseEnded) if session.reply_read != Some(true) => return "◆".to_owned(),
            Some(AttentionState::Failed) => return "!".to_owned(),
            Some(AttentionState::Limited) => return "⌛".to_owned(),
            Some(AttentionState::Working) => return spinner_frame.to_owned(),
            _ => {}
        }
    }
    if session.status == SessionStatus::Exited
        && matches!(session.exit_reason, Some(ExitReason::Failed | ExitReason::Interrupted))
    {
        return "!".to_owned();
    }
    match session.status {
        SessionStatus::Starting => spinner_frame.to_owned(),
        SessionStatus::Running => match session.agent_status {
            Some(AgentActivityStatus::Active) => spinner_frame.to_owned(),
            Some(AgentActivityStatus::Idle) => "●".to_owned(),
            _ => "◌".to_owned(),
        },
        SessionStatus::Exited => "○".to_owned(),
    }
}

/// TS: ui.ts statusColor.
pub fn status_color(session: &SessionRecord) -> Color {
    let attention = session.attention.as_ref().map(|attention| attention.state);
    if attention == Some(AttentionState::Failed)
        || session.cleanup_error.as_deref().is_some_and(|error| !error.is_empty())
        || session.exit_reason == Some(ExitReason::Failed)
    {
        return THEME.error;
    }
    if matches!(attention, Some(AttentionState::NeedsInput | AttentionState::Limited))
        || session.exit_reason == Some(ExitReason::Interrupted)
    {
        return THEME.warn;
    }
    match session.status {
        SessionStatus::Starting => THEME.warn,
        SessionStatus::Running => match session.agent_status {
            None | Some(AgentActivityStatus::Unknown) => THEME.warn,
            Some(_) => THEME.success,
        },
        SessionStatus::Exited => THEME.muted,
    }
}

/// TS: ui.ts displaySessionTitle (whitespace collapsed; a sub-session's parent prefix removed; ` / ` → `/`).
pub fn display_session_title<S: SessionLike>(session: &SessionRecord, sessions: &[S]) -> String {
    let parent = session
        .parent_session_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .and_then(|id| {
            sessions
                .iter()
                .map(SessionLike::session)
                .find(|candidate| candidate.id == id)
        });
    let parent_prefix = parent.map(|parent| format!("{} / ", js_collapse_whitespace(js_trim(&parent.title))));
    let normalized = js_collapse_whitespace(js_trim(&session.title));
    let local = match parent_prefix.as_deref() {
        Some(prefix) if normalized.starts_with(prefix) => &normalized[prefix.len()..],
        _ => normalized.as_str(),
    };
    // TS: `.replace(/\s+\/\s+/g, '/')`. Whitespace runs are single spaces by now, so `\s+` is exactly one space.
    local.replace(" / ", "/")
}

/// TS: ui.ts statusLabel.
pub fn status_label(session: Option<&SessionRecord>) -> String {
    let Some(session) = session else {
        return "—".to_owned();
    };
    match session.status {
        SessionStatus::Exited => return session.exit_reason.map_or("exited", ExitReason::as_str).to_owned(),
        SessionStatus::Starting => {
            let preparing = session
                .setup
                .as_ref()
                .is_some_and(|setup| setup.state == SetupState::Running);
            return if preparing { "preparing workspace" } else { "starting" }.to_owned();
        }
        SessionStatus::Running => {}
    }
    if let Some(attention) = &session.attention
        && attention.state != AttentionState::Unknown
        && session.reply_read != Some(true)
    {
        return match attention.state {
            AttentionState::ResponseEnded => "response ended".to_owned(),
            state => state.as_str().to_owned(),
        };
    }
    session
        .agent_status
        .map_or("running", AgentActivityStatus::as_str)
        .to_owned()
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
    to_usize(menu_window_start_js(
        js_width(selected),
        js_width(count),
        js_width(rows),
    ))
}

fn menu_window_start_js(selected: i64, count: i64, rows: i64) -> i64 {
    if count <= rows {
        return 0;
    }
    (count - rows).min(selected - rows.div_euclid(2)).max(0)
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
    let current = current.map_or(-1, js_width);
    to_usize(scrolled_list_top_js(
        js_width(top),
        current,
        js_width(rows),
        js_width(total),
        selectable,
    ))
}

fn scrolled_list_top_js(mut top: i64, current: i64, rows: i64, total: i64, selectable: &dyn Fn(usize) -> bool) -> i64 {
    if current >= 0 {
        if current < top {
            let mut first = current;
            while first > 0 && !selectable(to_usize(first - 1)) {
                first -= 1;
            }
            top = first.max(current - rows + 1);
        }
        if current >= top + rows {
            top = current - rows + 1;
        }
    }
    top.min((total - rows).max(0)).max(0)
}

/// TS: menu.tsx wrapWords (word-wrapped to `width` UTF-16 units, long words cut; at least one line).
pub fn wrap_words(text: &str, width: usize) -> Vec<String> {
    wrap_words_js(text, js_width(width))
}

fn wrap_words_js(text: &str, width: i64) -> Vec<String> {
    let columns = width.max(1);
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in js_split_whitespace(text) {
        if line.is_empty() {
            word.clone_into(&mut line);
        } else if js_width(js_len(&line)) + 1 + js_width(js_len(word)) <= columns {
            line.push(' ');
            line.push_str(word);
        } else {
            lines.push(std::mem::replace(&mut line, word.to_owned()));
        }
        while js_width(js_len(&line)) > columns {
            lines.push(js_slice(&line, 0, Some(columns)));
            line = js_slice(&line, columns, None);
        }
    }
    if !line.is_empty() {
        lines.push(line);
    } else if lines.is_empty() {
        lines.push(String::new());
    }
    lines
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
    fit_parts_js(parts, js_width(width))
}

fn fit_parts_js(parts: &[DetailPart], width: i64) -> Vec<DetailPart> {
    let mut shown = Vec::new();
    let mut room = width.max(0);
    for part in parts {
        if room <= 0 {
            break;
        }
        let text = if js_width(js_len(&part.text)) > room {
            truncate_js(&part.text, room)
        } else {
            part.text.clone()
        };
        room -= js_width(js_len(&text));
        shown.push(DetailPart { text, ..part.clone() });
    }
    shown
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
        HintPart {
            text: text.to_owned(),
            short: None,
            drop: None,
        }
    }
}

/// TS: menu.tsx fitHint (full texts, then short forms, then dropping parts; `separator` defaults to ` • `).
pub fn fit_hint(parts: &[HintPart], width: usize, separator: &str) -> String {
    fit_hint_js(parts, js_width(width), separator)
}

fn fit_hint_js(parts: &[HintPart], width: i64, separator: &str) -> String {
    // (order, part): among parts with the same `drop`, the later one goes first.
    let mut current: Vec<(usize, &HintPart)> = parts.iter().enumerate().collect();
    let join = |list: &[(usize, &HintPart)], short: bool| {
        list.iter()
            .map(|(_, part)| {
                if short {
                    part.short.as_deref().unwrap_or(&part.text)
                } else {
                    part.text.as_str()
                }
            })
            .collect::<Vec<_>>()
            .join(separator)
    };
    let full = join(&current, false);
    if js_width(js_len(&full)) <= width {
        return full;
    }
    let drop = |part: &HintPart| part.drop.unwrap_or(0);
    while current.len() > 1 && js_width(js_len(&join(&current, true))) > width {
        let mut victim = 0;
        for (index, &(order, part)) in current.iter().enumerate() {
            let (worst_order, worst) = current[victim];
            if drop(part) > drop(worst) || (drop(part) == drop(worst) && order > worst_order) {
                victim = index;
            }
        }
        current.remove(victim);
    }
    truncate_js(&join(&current, true), width)
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
    menu_details_rows_js(lines, js_width(width))
}

fn menu_details_rows_js(lines: &[DetailLine], width: i64) -> usize {
    let inner = (width - 4).max(1);
    let body: usize = lines
        .iter()
        .map(|line| {
            if line.nowrap == Some(true) || line.parts.is_some() {
                1
            } else {
                wrap_words_js(&line.text, inner).len()
            }
        })
        .sum();
    3 + body
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

    // -----------------------------------------------------------------------------------------------------------
    // Parity replays (fixtures/parity/ui, fixtures/parity/menu)

    use deckhand_parity::{Case, JsValue, replay, tags};
    use serde_json::Value;

    /// An Ink color name from a fixture (the fixtures use a handful of names; tests may leak them).
    fn color_of(value: Option<&Value>) -> Option<Color> {
        value
            .and_then(Value::as_str)
            .map(|name| &*Box::leak(name.to_owned().into_boxed_str()))
    }

    /// A string that may be tagged `$utf16` (a lone surrogate; only its presence matters to the callers).
    fn text_of(value: Option<&Value>) -> String {
        match value.map(tags::decode) {
            Some(Ok(JsValue::String(text))) => text,
            Some(Ok(JsValue::Utf16(units))) => String::from_utf16_lossy(&units),
            _ => String::new(),
        }
    }

    fn detail_part(value: &Value) -> DetailPart {
        DetailPart {
            text: text_of(value.get("text")),
            color: color_of(value.get("color")),
            bold: value.get("bold").and_then(Value::as_bool),
            italic: value.get("italic").and_then(Value::as_bool),
        }
    }

    fn detail_parts(case: &Case, index: usize) -> Result<Vec<DetailPart>, String> {
        let parts = case.arg(index).as_array().ok_or("parts: expected an array")?;
        Ok(parts.iter().map(detail_part).collect())
    }

    /// `String(value)` for the values errorMessage is called with (an Error shows its message).
    struct JsString(JsValue);

    impl std::fmt::Display for JsString {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match &self.0 {
                JsValue::Error { message, .. } => f.write_str(message),
                JsValue::String(text) => f.write_str(text),
                JsValue::Undefined => f.write_str("undefined"),
                JsValue::Null => f.write_str("null"),
                JsValue::Bool(value) => write!(f, "{value}"),
                JsValue::Number(n) if n.is_nan() => f.write_str("NaN"),
                JsValue::Number(n) if n.is_infinite() => f.write_str(if *n > 0.0 { "Infinity" } else { "-Infinity" }),
                JsValue::Number(n) if n.abs() >= 1e21 => {
                    let text = format!("{n:e}");
                    f.write_str(&text.replace('e', "e+").replace("e+-", "e-"))
                }
                JsValue::Number(n) => write!(f, "{n}"),
                other => write!(f, "{other:?}"),
            }
        }
    }

    #[test]
    fn theme_parity() {
        replay("P12", "ui/THEME", |_case: &Case| -> Result<Theme, String> { Ok(THEME) });
    }

    #[test]
    fn dusk_is_opt_in() {
        assert_eq!(theme_mode(Some("dusk")), ThemeMode::Dusk);
        assert_eq!(theme_mode(Some("terminal")), ThemeMode::Terminal);
        assert_eq!(theme_mode(Some("auto")), ThemeMode::Terminal);
        assert_eq!(theme_mode(None), ThemeMode::Terminal);
    }

    #[test]
    fn display_control_pattern_parity() {
        // The JS pattern is recorded as its source; the Rust one must match exactly the same code points.
        replay(
            "P12",
            "ui/DISPLAY_CONTROL_PATTERN",
            |_case: &Case| -> Result<Value, String> {
                for code in 0u32..0x3000 {
                    let Some(ch) = char::from_u32(code) else { continue };
                    let js = matches!(code, 0..=0x08 | 0x0B..=0x1F | 0x7F..=0x9F);
                    if DISPLAY_CONTROL_PATTERN.is_match(ch.encode_utf8(&mut [0; 4])) != js {
                        return Err(format!("U+{code:04X} differs"));
                    }
                }
                Ok(serde_json::json!({"$regexp": "/[\\u0000-\\u0008\\u000B-\\u001F\\u007F-\\u009F]/g"}))
            },
        );
    }

    #[test]
    fn error_message_parity() {
        replay("P12", "ui/errorMessage", |case: &Case| -> Result<String, String> {
            Ok(error_message(&JsString(tags::decode(case.arg(0))?)))
        });
    }

    #[test]
    fn strip_terminal_controls_parity() {
        replay(
            "P12",
            "ui/stripTerminalControls",
            |case: &Case| -> Result<String, String> { Ok(strip_terminal_controls(case.arg_str(0)?)) },
        );
    }

    #[test]
    fn plain_terminal_text_parity() {
        replay("P12", "ui/plainTerminalText", |case: &Case| -> Result<String, String> {
            Ok(plain_terminal_text(case.arg_str(0)?))
        });
    }

    #[test]
    fn truncate_parity() {
        replay("P12", "ui/truncate", |case: &Case| -> Result<String, String> {
            Ok(truncate_js(case.arg_str(0)?, case.arg_i64(1)?))
        });
    }

    #[test]
    fn fit_lines_parity() {
        replay("P12", "ui/fitLines", |case: &Case| -> Result<Vec<String>, String> {
            Ok(fit_lines_js(case.arg_str(0)?, case.arg_i64(1)?, case.arg_usize(2)?))
        });
    }

    #[test]
    fn compact_path_parity() {
        replay("P12", "ui/compactPath", |case: &Case| -> Result<String, String> {
            Ok(compact_path_home(case.arg_str(0)?, None, case.arg_usize(1)?))
        });
    }

    #[test]
    fn compact_path_home_parity() {
        // [home, path, width]: compactPath with HOME set to `home`.
        replay("P12", "ui/compactPathHome", |case: &Case| -> Result<String, String> {
            Ok(compact_path_home(
                case.arg_str(1)?,
                Some(case.arg_str(0)?),
                case.arg_usize(2)?,
            ))
        });
    }

    #[test]
    fn program_glyph_parity() {
        replay("P12", "ui/programGlyph", |case: &Case| -> Result<&str, String> {
            Ok(program_glyph(case.arg_as(0)?))
        });
    }

    #[test]
    fn status_glyph_parity() {
        replay("P12", "ui/statusGlyph", |case: &Case| -> Result<String, String> {
            Ok(status_glyph(&case.arg_as(0)?, case.arg_str(1)?))
        });
    }

    #[test]
    fn status_color_parity() {
        replay("P12", "ui/statusColor", |case: &Case| -> Result<Color, String> {
            Ok(status_color(&case.arg_as(0)?))
        });
    }

    #[test]
    fn status_label_parity() {
        replay("P12", "ui/statusLabel", |case: &Case| -> Result<String, String> {
            let session: Option<SessionRecord> = if case.arg_is_undefined(0) {
                None
            } else {
                Some(case.arg_as(0)?)
            };
            Ok(status_label(session.as_ref()))
        });
    }

    #[test]
    fn display_session_title_parity() {
        replay(
            "P12",
            "ui/displaySessionTitle",
            |case: &Case| -> Result<String, String> {
                let sessions: Vec<SessionRecord> = case.arg_as(1)?;
                Ok(display_session_title(&case.arg_as(0)?, &sessions))
            },
        );
    }

    #[test]
    fn selection_marker_parity() {
        replay("P12", "menu/SELECTION_MARKER", |_case: &Case| -> Result<&str, String> {
            Ok(SELECTION_MARKER)
        });
    }

    #[test]
    fn menu_window_start_parity() {
        replay("P12", "menu/menuWindowStart", |case: &Case| -> Result<i64, String> {
            Ok(menu_window_start_js(
                case.arg_i64(0)?,
                case.arg_i64(1)?,
                case.arg_i64(2)?,
            ))
        });
    }

    #[test]
    fn scrolled_list_top_parity() {
        replay("P12", "menu/scrolledListTop", |case: &Case| -> Result<i64, String> {
            let selectable: fn(usize) -> bool = match case.fn_name(4) {
                Some("all") => |_| true,
                Some("none") => |_| false,
                Some("even") => |index| index % 2 == 0,
                Some("notEvery4") => |index| index % 4 != 0,
                Some("notEvery3") => |index| index % 3 != 0,
                Some("notFirstTwo") => |index| index > 1,
                Some("notEvery5") => |index| index % 5 != 0,
                Some("headings03") => |index| index != 0 && index != 3,
                other => return Err(format!("unknown predicate {other:?}")),
            };
            Ok(scrolled_list_top_js(
                case.arg_i64(0)?,
                case.arg_i64(1)?,
                case.arg_i64(2)?,
                case.arg_i64(3)?,
                &selectable,
            ))
        });
    }

    #[test]
    fn wrap_words_parity() {
        replay("P12", "menu/wrapWords", |case: &Case| -> Result<Vec<String>, String> {
            Ok(wrap_words_js(case.arg_str(0)?, case.arg_i64(1)?))
        });
    }

    #[test]
    fn fit_parts_parity() {
        replay(
            "P12",
            "menu/fitParts",
            |case: &Case| -> Result<Vec<DetailPart>, String> {
                Ok(fit_parts_js(&detail_parts(case, 0)?, case.arg_i64(1)?))
            },
        );
    }

    #[test]
    fn fit_hint_parity() {
        replay("P12", "menu/fitHint", |case: &Case| -> Result<String, String> {
            let parts = case.arg(0).as_array().ok_or("parts: expected an array")?;
            let parts = parts
                .iter()
                .map(|part| match part.as_str() {
                    Some(text) => Ok(HintPart::from(text)),
                    None => serde_json::from_value(part.clone()).map_err(|error| error.to_string()),
                })
                .collect::<Result<Vec<HintPart>, String>>()?;
            let separator = if case.arg_is_undefined(2) {
                HINT_SEPARATOR
            } else {
                case.arg_str(2)?
            };
            Ok(fit_hint_js(&parts, case.arg_i64(1)?, separator))
        });
    }

    #[test]
    fn menu_details_rows_parity() {
        replay("P12", "menu/menuDetailsRows", |case: &Case| -> Result<usize, String> {
            let lines = case.arg(0).as_array().ok_or("lines: expected an array")?;
            let lines: Vec<DetailLine> = lines
                .iter()
                .map(|line| DetailLine {
                    text: text_of(line.get("text")),
                    color: color_of(line.get("color")),
                    nowrap: line.get("nowrap").and_then(Value::as_bool),
                    parts: line
                        .get("parts")
                        .and_then(Value::as_array)
                        .map(|parts| parts.iter().map(detail_part).collect()),
                })
                .collect();
            Ok(menu_details_rows_js(&lines, case.arg_i64(1)?))
        });
    }

    #[test]
    fn usize_wrappers_match_the_js_versions() {
        assert_eq!(truncate("hello world", 5), "hell…");
        assert_eq!(truncate("😀😀", 3), "😀…");
        assert_eq!(fit_lines("a\n\nbcdef", 3, 4), ["a", " ", "bc…", " "]);
        assert_eq!(menu_window_start(9, 20, 5), 7);
        assert_eq!(
            scrolled_list_top(5, Some(4), 4, 10, &|index| index != 0 && index != 3),
            3
        );
        assert_eq!(scrolled_list_top(5, None, 4, 7, &|_| true), 3);
        assert_eq!(wrap_words("one two three", 7), ["one two", "three"]);
        let parts = [
            DetailPart {
                text: "abc".into(),
                ..DetailPart::default()
            },
            DetailPart {
                text: "def".into(),
                ..DetailPart::default()
            },
        ];
        assert_eq!(
            fit_parts(&parts, 4)
                .iter()
                .map(|part| part.text.as_str())
                .collect::<Vec<_>>(),
            ["abc", "d"]
        );
        let hint = [
            HintPart {
                text: "move up".into(),
                short: Some("mv".into()),
                drop: Some(1),
            },
            HintPart::from("esc back"),
        ];
        assert_eq!(fit_hint(&hint, 13, HINT_SEPARATOR), "mv • esc back");
        assert_eq!(fit_hint(&hint, 9, HINT_SEPARATOR), "esc back");
        assert_eq!(
            menu_details_rows(
                &[DetailLine {
                    text: "aaa bbb".into(),
                    ..DetailLine::default()
                }],
                7
            ),
            5
        );
    }
}
