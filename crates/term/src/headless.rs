//! Port of HeadlessTerminal trait (stands in for @xterm/headless).
//!
//! The surface deckhand reads from `@xterm/headless`: TerminalPreview (src/terminalPreview.ts) writes decoded PTY
//! output, resizes, and reads `buffer.active` (`baseY`, `getLine(y).translateToString(true)`, `cursorX/Y`); the
//! goldens in fixtures/term additionally record the buffer type, wrapped lines, per-cell attributes, wide
//! cells, scrollback, modes and the title. Implementations: [`crate::alacritty::AlacrittyTerminal`].
//!
//! Line indices (`y`) are absolute buffer lines as in xterm: `0..length()`, scrollback first, the viewport starting
//! at `base_y()`. Columns are cells.
//!
//! [`ScreenSnapshot`] serialises to (documented contract, compared by T1 against the H0c goldens' `screen`):
//!
//! ```json
//! {"cols": 80, "rows": 24, "lines": ["$ ls", "a  b", "$ "], "cursor": {"x": 2, "y": 2}, "alt": false,
//!  "runs": {"1": [[0, 1, "p16:4", "d", "b"]]}}
//! ```
//!
//! `lines` has exactly `rows` entries (viewport, right-trimmed); `runs` maps a viewport row to its non-default
//! attribute runs `[x, cells, fg, bg, flags]` (rows without any are omitted). Colours are `"d"` (default),
//! `"p16:N"` (SGR 30–37/90–97 and backgrounds), `"p256:N"` (38;5;N) or `"rgb:rrggbb"`; flags are the letters
//! `b`old `d`im `i`talic `u`nderline blin`k` `r`everse `h`idden `s`trikethrough `o`verline, in that order.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// TS: @xterm/headless ITerminalOptions (cols, rows, scrollback)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalOptions {
    pub cols: u16,
    pub rows: u16,
    /// Lines kept above the viewport in the normal buffer.
    pub scrollback: usize,
}

/// TS: @xterm/headless IBuffer.type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BufferKind {
    #[serde(rename = "normal")]
    Normal,
    #[serde(rename = "alternate")]
    Alternate,
}

/// TS: @xterm/headless IBuffer.cursorX / cursorY (viewport-relative)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CursorPos {
    pub x: u16,
    pub y: u16,
}

/// TS: @xterm/headless IBufferCell getFgColorMode/getFgColor (and Bg), as the goldens in fixtures/term encode it
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    /// Terminal default colour (`d`).
    #[default]
    Default,
    /// One of the 16 ANSI colours (`p16:N`).
    P16(u8),
    /// One of the 256 palette colours set with `38;5;N`/`48;5;N` (`p256:N`).
    P256(u8),
    /// Truecolor (`rgb:rrggbb`).
    Rgb(u8, u8, u8),
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Color::Default => f.write_str("d"),
            Color::P16(index) => write!(f, "p16:{index}"),
            Color::P256(index) => write!(f, "p256:{index}"),
            Color::Rgb(r, g, b) => write!(f, "rgb:{r:02x}{g:02x}{b:02x}"),
        }
    }
}

impl FromStr for Color {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || format!("invalid colour {text:?}");
        if text == "d" {
            return Ok(Color::Default);
        }
        if let Some(index) = text.strip_prefix("p16:") {
            return index
                .parse()
                .ok()
                .filter(|index| *index < 16)
                .map(Color::P16)
                .ok_or_else(invalid);
        }
        if let Some(index) = text.strip_prefix("p256:") {
            return index.parse().map(Color::P256).map_err(|_| invalid());
        }
        if let Some(hex) = text.strip_prefix("rgb:")
            && hex.len() == 6
            && let Ok(value) = u32::from_str_radix(hex, 16)
        {
            let [_, r, g, b] = value.to_be_bytes();
            return Ok(Color::Rgb(r, g, b));
        }
        Err(invalid())
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// TS: @xterm/headless IBufferCell isBold()/isDim()/... as the goldens in fixtures/term encode them
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct CellFlags(u16);

impl CellFlags {
    pub const BOLD: CellFlags = CellFlags(1);
    pub const DIM: CellFlags = CellFlags(1 << 1);
    pub const ITALIC: CellFlags = CellFlags(1 << 2);
    pub const UNDERLINE: CellFlags = CellFlags(1 << 3);
    pub const BLINK: CellFlags = CellFlags(1 << 4);
    pub const INVERSE: CellFlags = CellFlags(1 << 5);
    pub const INVISIBLE: CellFlags = CellFlags(1 << 6);
    pub const STRIKETHROUGH: CellFlags = CellFlags(1 << 7);
    pub const OVERLINE: CellFlags = CellFlags(1 << 8);

    /// Golden letter for each flag, in serialisation order.
    const LETTERS: [(CellFlags, char); 9] = [
        (CellFlags::BOLD, 'b'),
        (CellFlags::DIM, 'd'),
        (CellFlags::ITALIC, 'i'),
        (CellFlags::UNDERLINE, 'u'),
        (CellFlags::BLINK, 'k'),
        (CellFlags::INVERSE, 'r'),
        (CellFlags::INVISIBLE, 'h'),
        (CellFlags::STRIKETHROUGH, 's'),
        (CellFlags::OVERLINE, 'o'),
    ];

    /// No flags set.
    pub const fn empty() -> Self {
        CellFlags(0)
    }

    pub const fn bits(self) -> u16 {
        self.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains(self, other: CellFlags) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn insert(&mut self, other: CellFlags) {
        self.0 |= other.0;
    }

    pub fn set(&mut self, other: CellFlags, on: bool) {
        if on {
            self.0 |= other.0;
        } else {
            self.0 &= !other.0;
        }
    }
}

impl std::ops::BitOr for CellFlags {
    type Output = CellFlags;

    fn bitor(self, other: CellFlags) -> CellFlags {
        CellFlags(self.0 | other.0)
    }
}

impl fmt::Display for CellFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (flag, letter) in CellFlags::LETTERS {
            if self.contains(flag) {
                write!(f, "{letter}")?;
            }
        }
        Ok(())
    }
}

impl FromStr for CellFlags {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut flags = CellFlags::empty();
        for letter in text.chars() {
            let (flag, _) = CellFlags::LETTERS
                .iter()
                .find(|(_, known)| *known == letter)
                .ok_or_else(|| format!("invalid cell flag {letter:?} in {text:?}"))?;
            flags.insert(*flag);
        }
        Ok(flags)
    }
}

impl Serialize for CellFlags {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for CellFlags {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// The attributes compared when building runs (fg, bg, flags).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellAttrs {
    pub fg: Color,
    pub bg: Color,
    pub flags: CellFlags,
}

impl CellAttrs {
    /// Default colours and no flags: cells like this are outside every run.
    pub fn is_default(&self) -> bool {
        self.fg == Color::Default && self.bg == Color::Default && self.flags.is_empty()
    }
}

/// TS: @xterm/headless IBufferCell (getChars, getWidth, colours, flags)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CellInfo {
    /// The cell's text (`""` for an empty cell or the trailing half of a wide character).
    pub chars: String,
    /// 1 for a normal cell, 2 for a wide character, 0 for the cell after a wide character.
    pub width: u8,
    pub attrs: CellAttrs,
}

/// One run of equal non-default attributes on a line; JSON `[x, cells, fg, bg, flags]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    into = "(u16, u16, Color, Color, CellFlags)",
    from = "(u16, u16, Color, Color, CellFlags)"
)]
pub struct AttrRun {
    pub x: u16,
    pub cells: u16,
    pub attrs: CellAttrs,
}

impl From<AttrRun> for (u16, u16, Color, Color, CellFlags) {
    fn from(run: AttrRun) -> Self {
        (run.x, run.cells, run.attrs.fg, run.attrs.bg, run.attrs.flags)
    }
}

impl From<(u16, u16, Color, Color, CellFlags)> for AttrRun {
    fn from((x, cells, fg, bg, flags): (u16, u16, Color, Color, CellFlags)) -> Self {
        AttrRun {
            x,
            cells,
            attrs: CellAttrs { fg, bg, flags },
        }
    }
}

/// TS: @xterm/headless IModes.mouseTrackingMode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MouseTrackingMode {
    #[default]
    #[serde(rename = "none")]
    None,
    #[serde(rename = "x10")]
    X10,
    #[serde(rename = "vt200")]
    Vt200,
    #[serde(rename = "drag")]
    Drag,
    #[serde(rename = "any")]
    Any,
}

/// TS: @xterm/headless IModes (key order as xterm's `terminal.modes`)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct XtermModes {
    pub application_cursor_keys_mode: bool,
    pub application_keypad_mode: bool,
    pub bracketed_paste_mode: bool,
    pub insert_mode: bool,
    pub mouse_tracking_mode: MouseTrackingMode,
    pub origin_mode: bool,
    pub reverse_wraparound_mode: bool,
    pub send_focus_mode: bool,
    pub wraparound_mode: bool,
}

impl Default for XtermModes {
    /// A fresh terminal: everything off except auto-wrap.
    fn default() -> Self {
        XtermModes {
            application_cursor_keys_mode: false,
            application_keypad_mode: false,
            bracketed_paste_mode: false,
            insert_mode: false,
            mouse_tracking_mode: MouseTrackingMode::None,
            origin_mode: false,
            reverse_wraparound_mode: false,
            send_focus_mode: false,
            wraparound_mode: true,
        }
    }
}

/// The visible screen: see the module docs for the JSON shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenSnapshot {
    pub cols: u16,
    pub rows: u16,
    /// Viewport lines, `translateToString(true)`; exactly `rows` entries.
    pub lines: Vec<String>,
    pub cursor: CursorPos,
    /// The alternate buffer is active.
    pub alt: bool,
    /// Viewport row → its non-default attribute runs (rows without runs are absent).
    pub runs: BTreeMap<u16, Vec<AttrRun>>,
}

impl ScreenSnapshot {
    /// The documented JSON text.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// TS: @xterm/headless Terminal — the headless emulator surface deckhand uses.
pub trait HeadlessTerminal {
    /// TS: new Terminal({cols, rows, scrollback, allowProposedApi: true})
    fn new(options: TerminalOptions) -> Self
    where
        Self: Sized;

    /// TS: Terminal.write(data) — parses synchronously. Callers feed text already decoded by
    /// [`crate::utf8::Utf8Decoder`] (node-pty's decoding); the bytes are that text's UTF-8.
    fn write(&mut self, data: &[u8]);

    /// TS: Terminal.resize(cols, rows) — reflows like xterm; both are at least 1.
    fn resize(&mut self, cols: u16, rows: u16);

    /// TS: Terminal.cols
    fn cols(&self) -> u16;

    /// TS: Terminal.rows
    fn rows(&self) -> u16;

    /// TS: Terminal.buffer.active.type
    fn buffer_kind(&self) -> BufferKind;

    /// TS: Terminal.buffer.active.baseY — lines above the viewport (always 0 in the alternate buffer).
    fn base_y(&self) -> usize;

    /// TS: Terminal.buffer.active.length — scrollback plus viewport lines.
    fn length(&self) -> usize;

    /// TS: IBuffer.getLine(y)?.translateToString(trimRight) — `None` past the end of the buffer.
    fn line_text(&self, y: usize, trim_right: bool) -> Option<String>;

    /// TS: IBuffer.getLine(y)?.isWrapped — the line continues the previous one (soft wrap).
    fn line_is_wrapped(&self, y: usize) -> bool;

    /// TS: IBuffer.getLine(y)?.getCell(x) — `None` outside the buffer.
    fn cell(&self, y: usize, x: u16) -> Option<CellInfo>;

    /// TS: IBuffer.cursorX / cursorY
    fn cursor(&self) -> CursorPos;

    /// TS: Terminal.modes
    fn modes(&self) -> XtermModes;

    /// TS: Terminal.onTitleChange — the last title set (OSC 0/2), `None` if never set.
    fn title(&self) -> Option<String>;

    /// TS: Terminal.write(string)
    fn write_str(&mut self, data: &str) {
        self.write(data.as_bytes());
    }

    /// The alternate screen is active (`buffer.active.type === 'alternate'`).
    fn is_alt_screen(&self) -> bool {
        self.buffer_kind() == BufferKind::Alternate
    }

    /// TS: IBuffer.viewportY — headless terminals never scroll by user input, so the viewport sits at `baseY`.
    fn viewport_y(&self) -> usize {
        self.base_y()
    }

    /// The `rows` viewport lines (`getLine(baseY + y)`), missing lines as `""`.
    fn viewport_lines(&self, trim_right: bool) -> Vec<String> {
        let base = self.base_y();
        (0..usize::from(self.rows()))
            .map(|y| self.line_text(base + y, trim_right).unwrap_or_default())
            .collect()
    }

    /// The scrollback lines above the viewport, oldest first, right-trimmed.
    fn scrollback_lines(&self) -> Vec<String> {
        (0..self.base_y())
            .map(|y| self.line_text(y, true).unwrap_or_default())
            .collect()
    }

    /// Non-default attribute runs of line `y`, merged as the goldens in fixtures/term record them.
    fn line_runs(&self, y: usize) -> Vec<AttrRun> {
        let mut runs: Vec<AttrRun> = Vec::new();
        for x in 0..self.cols() {
            let Some(cell) = self.cell(y, x) else { break };
            if cell.attrs.is_default() {
                continue;
            }
            match runs.last_mut() {
                Some(last) if last.x + last.cells == x && last.attrs == cell.attrs => last.cells += 1,
                _ => runs.push(AttrRun {
                    x,
                    cells: 1,
                    attrs: cell.attrs,
                }),
            }
        }
        runs
    }

    /// Columns of line `y` holding a wide (2-cell) character.
    fn wide_cells(&self, y: usize) -> Vec<u16> {
        (0..self.cols())
            .filter(|x| self.cell(y, *x).is_some_and(|cell| cell.width == 2))
            .collect()
    }

    /// The visible screen as a [`ScreenSnapshot`].
    fn snapshot(&self) -> ScreenSnapshot {
        let base = self.base_y();
        let mut runs = BTreeMap::new();
        for row in 0..self.rows() {
            let line_runs = self.line_runs(base + usize::from(row));
            if !line_runs.is_empty() {
                runs.insert(row, line_runs);
            }
        }
        ScreenSnapshot {
            cols: self.cols(),
            rows: self.rows(),
            lines: self.viewport_lines(true),
            cursor: self.cursor(),
            alt: self.is_alt_screen(),
            runs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal emulator for exercising the provided methods: printable text, CR, LF and scrolling only.
    struct GridTerminal {
        cols: u16,
        rows: u16,
        lines: Vec<Vec<CellInfo>>,
        wrapped: Vec<bool>,
        cursor: CursorPos,
        pen: CellAttrs,
        alt: bool,
    }

    impl GridTerminal {
        fn blank(cols: u16) -> Vec<CellInfo> {
            vec![
                CellInfo {
                    chars: String::new(),
                    width: 1,
                    attrs: CellAttrs::default()
                };
                usize::from(cols)
            ]
        }

        fn line_index(&self) -> usize {
            self.base_y() + usize::from(self.cursor.y)
        }

        fn line_feed(&mut self) {
            if self.cursor.y + 1 < self.rows {
                self.cursor.y += 1;
            } else {
                self.lines.push(Self::blank(self.cols));
                self.wrapped.push(false);
            }
        }
    }

    impl HeadlessTerminal for GridTerminal {
        fn new(options: TerminalOptions) -> Self {
            GridTerminal {
                cols: options.cols,
                rows: options.rows,
                lines: (0..options.rows).map(|_| Self::blank(options.cols)).collect(),
                wrapped: vec![false; usize::from(options.rows)],
                cursor: CursorPos::default(),
                pen: CellAttrs::default(),
                alt: false,
            }
        }

        fn write(&mut self, data: &[u8]) {
            for ch in String::from_utf8_lossy(data).chars() {
                match ch {
                    '\r' => self.cursor.x = 0,
                    '\n' => self.line_feed(),
                    _ => {
                        if self.cursor.x >= self.cols {
                            self.cursor.x = 0;
                            self.line_feed();
                            let index = self.line_index();
                            self.wrapped[index] = true;
                        }
                        let index = self.line_index();
                        let x = usize::from(self.cursor.x);
                        self.lines[index][x] = CellInfo {
                            chars: ch.to_string(),
                            width: 1,
                            attrs: self.pen,
                        };
                        self.cursor.x += 1;
                    }
                }
            }
        }

        fn resize(&mut self, cols: u16, rows: u16) {
            self.cols = cols;
            self.rows = rows;
        }

        fn cols(&self) -> u16 {
            self.cols
        }

        fn rows(&self) -> u16 {
            self.rows
        }

        fn buffer_kind(&self) -> BufferKind {
            if self.alt {
                BufferKind::Alternate
            } else {
                BufferKind::Normal
            }
        }

        fn base_y(&self) -> usize {
            self.lines.len().saturating_sub(usize::from(self.rows))
        }

        fn length(&self) -> usize {
            self.lines.len()
        }

        fn line_text(&self, y: usize, trim_right: bool) -> Option<String> {
            let line = self.lines.get(y)?;
            let text: String = line
                .iter()
                .map(|cell| {
                    if cell.chars.is_empty() {
                        " "
                    } else {
                        cell.chars.as_str()
                    }
                })
                .collect();
            Some(if trim_right { text.trim_end().to_string() } else { text })
        }

        fn line_is_wrapped(&self, y: usize) -> bool {
            self.wrapped.get(y).copied().unwrap_or(false)
        }

        fn cell(&self, y: usize, x: u16) -> Option<CellInfo> {
            self.lines.get(y)?.get(usize::from(x)).cloned()
        }

        fn cursor(&self) -> CursorPos {
            self.cursor
        }

        fn modes(&self) -> XtermModes {
            XtermModes::default()
        }

        fn title(&self) -> Option<String> {
            None
        }
    }

    fn grid(cols: u16, rows: u16) -> GridTerminal {
        GridTerminal::new(TerminalOptions {
            cols,
            rows,
            scrollback: 100,
        })
    }

    #[test]
    fn colors_round_trip_through_golden_encoding() {
        for (color, text) in [
            (Color::Default, "d"),
            (Color::P16(9), "p16:9"),
            (Color::P256(200), "p256:200"),
            (Color::Rgb(0x0a, 0xbc, 0xff), "rgb:0abcff"),
        ] {
            assert_eq!(color.to_string(), text);
            assert_eq!(text.parse::<Color>(), Ok(color));
        }
        assert!("p16:16".parse::<Color>().is_err());
        assert!("rgb:12345".parse::<Color>().is_err());
        assert!("x".parse::<Color>().is_err());
    }

    #[test]
    fn flags_serialise_in_golden_letter_order() {
        let flags = CellFlags::OVERLINE | CellFlags::BOLD | CellFlags::INVERSE | CellFlags::UNDERLINE;
        assert_eq!(flags.to_string(), "buro");
        assert_eq!("buro".parse::<CellFlags>(), Ok(flags));
        assert_eq!(CellFlags::empty().to_string(), "");
        assert!("z".parse::<CellFlags>().is_err());
        let mut toggled = CellFlags::empty();
        toggled.set(CellFlags::DIM, true);
        assert!(toggled.contains(CellFlags::DIM));
        toggled.set(CellFlags::DIM, false);
        assert!(toggled.is_empty());
    }

    #[test]
    fn attr_run_is_a_json_tuple() {
        let run = AttrRun {
            x: 3,
            cells: 2,
            attrs: CellAttrs {
                fg: Color::P16(1),
                bg: Color::Default,
                flags: CellFlags::BOLD,
            },
        };
        let json = serde_json::to_string(&run).unwrap();
        assert_eq!(json, r#"[3,2,"p16:1","d","b"]"#);
        assert_eq!(serde_json::from_str::<AttrRun>(&json).unwrap(), run);
    }

    #[test]
    fn modes_serialise_like_xterm() {
        let json = serde_json::to_string(&XtermModes::default()).unwrap();
        assert_eq!(
            json,
            r#"{"applicationCursorKeysMode":false,"applicationKeypadMode":false,"bracketedPasteMode":false,"insertMode":false,"mouseTrackingMode":"none","originMode":false,"reverseWraparoundMode":false,"sendFocusMode":false,"wraparoundMode":true}"#
        );
    }

    #[test]
    fn runs_merge_adjacent_equal_attributes_and_skip_defaults() {
        let mut term = grid(10, 2);
        term.write_str("ab");
        term.pen = CellAttrs {
            fg: Color::P16(2),
            ..CellAttrs::default()
        };
        term.write_str("cde");
        term.pen = CellAttrs {
            fg: Color::P16(2),
            bg: Color::Rgb(1, 2, 3),
            flags: CellFlags::ITALIC,
        };
        term.write_str("f");
        term.pen = CellAttrs::default();
        term.write_str("g");
        term.pen = CellAttrs {
            fg: Color::P16(2),
            ..CellAttrs::default()
        };
        term.write_str("h");
        let runs = term.line_runs(0);
        assert_eq!(
            serde_json::to_string(&runs).unwrap(),
            r#"[[2,3,"p16:2","d",""],[5,1,"p16:2","rgb:010203","i"],[7,1,"p16:2","d",""]]"#
        );
    }

    #[test]
    fn snapshot_has_documented_shape() {
        let mut term = grid(6, 3);
        term.write_str("one\r\ntwo\r\n");
        term.pen = CellAttrs {
            flags: CellFlags::BOLD,
            ..CellAttrs::default()
        };
        term.write_str("$ ");
        let snapshot = term.snapshot();
        assert_eq!(
            snapshot.to_json(),
            r#"{"cols":6,"rows":3,"lines":["one","two","$"],"cursor":{"x":2,"y":2},"alt":false,"runs":{"2":[[0,2,"d","d","b"]]}}"#
        );
        let parsed: ScreenSnapshot = serde_json::from_str(&snapshot.to_json()).unwrap();
        assert_eq!(parsed, snapshot);
    }

    #[test]
    fn scrollback_and_viewport_split_at_base_y() {
        let mut term = grid(4, 2);
        term.write_str("a\r\nb\r\nc\r\nd");
        assert_eq!(term.base_y(), 2);
        assert_eq!(term.viewport_y(), 2);
        assert_eq!(term.length(), 4);
        assert_eq!(term.scrollback_lines(), ["a", "b"]);
        assert_eq!(term.viewport_lines(true), ["c", "d"]);
        assert_eq!(term.viewport_lines(false), ["c   ", "d   "]);
        assert!(!term.is_alt_screen());
        assert_eq!(term.line_text(9, true), None);
    }

    #[test]
    fn wrapped_lines_and_wide_cells_are_reported() {
        let mut term = grid(3, 3);
        term.write_str("abcd");
        assert!(!term.line_is_wrapped(0));
        assert!(term.line_is_wrapped(1));
        term.lines[2][1].width = 2;
        assert_eq!(term.wide_cells(2), [1]);
        assert!(term.wide_cells(0).is_empty());
    }

    #[test]
    fn missing_viewport_lines_are_empty_strings() {
        let mut term = grid(3, 2);
        term.lines.truncate(1);
        assert_eq!(term.viewport_lines(true), ["", ""]);
        term.alt = true;
        assert!(term.snapshot().alt);
    }
}
