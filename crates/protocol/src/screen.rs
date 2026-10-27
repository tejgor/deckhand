//! Not in the TS: the agent's screen with its colours, as an interactive Preview draws it.
//!
//! The cell attribute types are the ones the golden screens in fixtures/term encode, so a [`ScreenSegment`] uses the
//! same colour and flag strings as an attribute run there. The emulator (deckhand-term) re-exports them.
//!
//! A [`PreviewScreen`] serialises to:
//!
//! ```json
//! {"lines": [[[0, 2, "$ ", "d", "d", "b"]], []], "cursor": {"x": 2, "y": 0},
//!  "modes": {"bracketedPaste": true, "applicationCursorKeys": false, "mouseTracking": false}}
//! ```
//!
//! `lines` has one entry per visible row; each holds that row's segments `[x, cells, text, fg, bg, flags]` left to
//! right. Blank stretches in default colours have no segment. `cursor` is absent while the cursor is hidden or the
//! view is scrolled back.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::skip_serializing_none;

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

/// Text of one row in one set of attributes; JSON `[x, cells, text, fg, bg, flags]`.
///
/// `x` and `cells` are emulator columns: the text's own display width can differ (the emulator measures some
/// characters differently from the UI), so the UI places the text at `x` and fits it to `cells`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    into = "(u16, u16, String, Color, Color, CellFlags)",
    from = "(u16, u16, String, Color, Color, CellFlags)"
)]
pub struct ScreenSegment {
    pub x: u16,
    pub cells: u16,
    pub text: String,
    pub attrs: CellAttrs,
}

impl From<ScreenSegment> for (u16, u16, String, Color, Color, CellFlags) {
    fn from(segment: ScreenSegment) -> Self {
        let ScreenSegment { x, cells, text, attrs } = segment;
        (x, cells, text, attrs.fg, attrs.bg, attrs.flags)
    }
}

impl From<(u16, u16, String, Color, Color, CellFlags)> for ScreenSegment {
    fn from((x, cells, text, fg, bg, flags): (u16, u16, String, Color, Color, CellFlags)) -> Self {
        ScreenSegment {
            x,
            cells,
            text,
            attrs: CellAttrs { fg, bg, flags },
        }
    }
}

/// The terminal modes that change what a key sent to the agent should be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenModes {
    /// DECSET 2004: pasted text is wrapped in `\x1b[200~`…`\x1b[201~`.
    pub bracketed_paste: bool,
    /// DECCKM: arrow, Home and End keys use their `\x1bO` form.
    pub application_cursor_keys: bool,
    /// Any mouse reporting mode is on.
    pub mouse_tracking: bool,
}

/// The visible rows of an agent's screen with their attributes; see the module docs for the JSON shape.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewScreen {
    pub lines: Vec<Vec<ScreenSegment>>,
    /// Viewport-relative; absent while hidden or scrolled back.
    pub cursor: Option<CursorPos>,
    pub modes: ScreenModes,
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn segment_is_a_json_tuple() {
        let segment = ScreenSegment {
            x: 4,
            cells: 3,
            text: "a界".to_string(),
            attrs: CellAttrs {
                fg: Color::Rgb(0xd9, 0x77, 0x57),
                bg: Color::Default,
                flags: CellFlags::BOLD | CellFlags::INVERSE,
            },
        };
        let json = serde_json::to_string(&segment).unwrap();
        assert_eq!(json, r#"[4,3,"a界","rgb:d97757","d","br"]"#);
        assert_eq!(serde_json::from_str::<ScreenSegment>(&json).unwrap(), segment);
    }

    #[test]
    fn screen_has_documented_shape() {
        let screen = PreviewScreen {
            lines: vec![
                vec![ScreenSegment {
                    x: 0,
                    cells: 2,
                    text: "$ ".to_string(),
                    attrs: CellAttrs {
                        flags: CellFlags::BOLD,
                        ..CellAttrs::default()
                    },
                }],
                vec![],
            ],
            cursor: Some(CursorPos { x: 2, y: 0 }),
            modes: ScreenModes {
                bracketed_paste: true,
                ..ScreenModes::default()
            },
        };
        let json = serde_json::to_string(&screen).unwrap();
        assert_eq!(
            json,
            r#"{"lines":[[[0,2,"$ ","d","d","b"]],[]],"cursor":{"x":2,"y":0},"modes":{"bracketedPaste":true,"applicationCursorKeys":false,"mouseTracking":false}}"#
        );
        assert_eq!(serde_json::from_str::<PreviewScreen>(&json).unwrap(), screen);
        let hidden = PreviewScreen { cursor: None, ..screen };
        assert!(!serde_json::to_string(&hidden).unwrap().contains("cursor"));
    }
}
