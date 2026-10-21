//! Port of Ink 6's stdin keypress pipeline (ink/build/input-parser.js, parse-keypress.js, hooks/use-input.js) and of
//! useTerminalInput.ts.
//!
//! Ink reads stdin with `setEncoding('utf8')` (Node's `StringDecoder`), splits what arrives into one event per key
//! (input-parser.js: CSI/SS3 sequences, `ESC`+code point, runs of plain text, a trailing lone `ESC` held back until
//! the next tick), parses each event (parse-keypress.js, a copy of enquirer's keypress parser plus the kitty keyboard
//! protocol) and maps it to the `(input, key)` pair `useInput` handlers receive (use-input.js). Plain text typed or
//! pasted in one chunk is one event (`input` is all of it); bracketed-paste markers arrive as `[200~` / `[201~`.
//!
//! Deckhand never enables the kitty protocol (Ink only does with the `kittyKeyboard` render option), but Ink parses
//! kitty sequences whenever a terminal sends them, so this does too. Vectors recorded from Ink itself live in
//! `tests/data/keypress-vectors.json`.

use std::sync::LazyLock;

use regex::Regex;

use deckhand_core::key::{Input, Key, KeyEventType};
use deckhand_core::terminal_keys::normalize_terminal_key;
use deckhand_term::utf8::Utf8Decoder;

const ESC: char = '\u{1b}';
const ESC_STR: &str = "\u{1b}";

// ---------------------------------------------------------------------------------------------------------------
// input-parser.js

fn is_csi_parameter_byte(byte: u32) -> bool {
    (0x30..=0x3f).contains(&byte)
}

fn is_csi_intermediate_byte(byte: u32) -> bool {
    (0x20..=0x2f).contains(&byte)
}

fn is_csi_final_byte(byte: u32) -> bool {
    (0x40..=0x7e).contains(&byte)
}

/// What a control sequence starting at an `ESC` turned out to be.
enum Sequence {
    /// More input is needed to tell.
    Pending,
    /// Not a CSI/SS3 sequence.
    Invalid,
    /// The sequence ends before byte offset `next`.
    Complete { next: usize },
}

/// TS: input-parser.js parseCsiSequence (`start`: the `ESC`, `prefix`: how many `ESC`s precede the `[`).
fn parse_csi_sequence(input: &str, start: usize, prefix: usize) -> Sequence {
    let payload_start = start + prefix + 1;
    for (offset, character) in input[payload_start..].char_indices() {
        let byte = character as u32;
        if is_csi_parameter_byte(byte) || is_csi_intermediate_byte(byte) {
            continue;
        }
        // Legacy terminal function-key sequences like ESC[[A and ESC[[5~.
        if byte == 0x5b && offset == 0 {
            continue;
        }
        if is_csi_final_byte(byte) {
            return Sequence::Complete {
                next: payload_start + offset + 1,
            };
        }
        return Sequence::Invalid;
    }
    Sequence::Pending
}

/// TS: input-parser.js parseSs3Sequence.
fn parse_ss3_sequence(input: &str, start: usize, prefix: usize) -> Sequence {
    let final_start = start + prefix + 1;
    match input[final_start..].chars().next() {
        None => Sequence::Pending,
        Some(character) if is_csi_final_byte(character as u32) => Sequence::Complete { next: final_start + 1 },
        Some(_) => Sequence::Invalid,
    }
}

/// TS: input-parser.js parseControlSequence.
fn parse_control_sequence(input: &str, start: usize, prefix: usize) -> Sequence {
    match input.as_bytes().get(start + prefix) {
        None => Sequence::Pending,
        Some(b'[') => parse_csi_sequence(input, start, prefix),
        Some(b'O') => parse_ss3_sequence(input, start, prefix),
        Some(_) => Sequence::Invalid,
    }
}

/// TS: input-parser.js parseKeypresses's result: the events, and what is held for the next chunk.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedKeypresses {
    pub events: Vec<String>,
    pub pending: String,
}

/// TS: input-parser.js parseKeypresses: one event per key; plain text between escapes is one event.
pub fn parse_keypresses(input: &str) -> ParsedKeypresses {
    let mut events = Vec::new();
    let mut index = 0;
    let pending_from = |events: Vec<String>, start: usize| ParsedKeypresses {
        events,
        pending: input[start..].to_string(),
    };
    while index < input.len() {
        let Some(found) = input[index..].find(ESC) else {
            events.push(input[index..].to_string());
            return ParsedKeypresses {
                events,
                pending: String::new(),
            };
        };
        let escape = index + found;
        if escape > index {
            events.push(input[index..escape].to_string());
        }
        if escape == input.len() - 1 {
            return pending_from(events, escape);
        }
        match parse_control_sequence(input, escape, 1) {
            Sequence::Pending => return pending_from(events, escape),
            Sequence::Complete { next } => {
                events.push(input[escape..next].to_string());
                index = next;
                continue;
            }
            Sequence::Invalid => {}
        }
        if input.as_bytes().get(escape + 1) == Some(&0x1b) {
            if escape + 2 >= input.len() {
                return pending_from(events, escape);
            }
            match parse_control_sequence(input, escape, 2) {
                Sequence::Pending => return pending_from(events, escape),
                Sequence::Complete { next } => {
                    events.push(input[escape..next].to_string());
                    index = next;
                }
                Sequence::Invalid => {
                    events.push(input[escape..escape + 2].to_string());
                    index = escape + 2;
                }
            }
            continue;
        }
        // TS parseEscapedCodePoint: ESC and the code point after it.
        let next_len = input[escape + 1..].chars().next().map_or(0, char::len_utf8);
        let next = escape + 1 + next_len;
        events.push(input[escape..next].to_string());
        index = next;
    }
    ParsedKeypresses {
        events,
        pending: String::new(),
    }
}

/// TS: input-parser.js createInputParser: keeps an unfinished escape sequence across chunks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputParser {
    pending: String,
}

impl InputParser {
    /// TS: createInputParser.
    pub fn new() -> Self {
        Self::default()
    }

    /// TS: push: the events `chunk` completes (prefixed by what was held).
    pub fn push(&mut self, chunk: &str) -> Vec<String> {
        let mut input = std::mem::take(&mut self.pending);
        input.push_str(chunk);
        let parsed = parse_keypresses(&input);
        self.pending = parsed.pending;
        parsed.events
    }

    /// TS: hasPendingEscape.
    pub fn has_pending_escape(&self) -> bool {
        self.pending.starts_with(ESC)
    }

    /// TS: flushPendingEscape: the held escape as an event of its own (Ink does this on the next tick when no more
    /// input arrived).
    pub fn flush_pending_escape(&mut self) -> Option<String> {
        if !self.pending.starts_with(ESC) {
            return None;
        }
        Some(std::mem::take(&mut self.pending))
    }

    /// TS: reset.
    pub fn reset(&mut self) {
        self.pending.clear();
    }
}

// ---------------------------------------------------------------------------------------------------------------
// parse-keypress.js

/// TS: parse-keypress.js metaKeyCodeRe.
static META_KEY_CODE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\x1b([a-zA-Z0-9])$").expect("metaKeyCodeRe is a valid regex"));

/// TS: parse-keypress.js fnKeyRe (not anchored at the end, as in TS; `\d` is ASCII there).
static FN_KEY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\x1b+(O|N|\[|\[\[)(?:([0-9]+)(?:;([0-9]+))?([~^$])|(?:1;)?([0-9]+)?([a-zA-Z]))")
        .expect("fnKeyRe is a valid regex")
});

/// TS: parse-keypress.js kittyKeyRe.
static KITTY_KEY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\x1b\[([0-9]+)(?:;([0-9]+)(?::([0-9]+))?(?:;([0-9:]+))?)?u$").expect("kittyKeyRe is a valid regex")
});

/// TS: parse-keypress.js kittySpecialKeyRe.
static KITTY_SPECIAL_KEY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\x1b\[([0-9]+);([0-9]+):([0-9]+)([A-Za-z~])$").expect("kittySpecialKeyRe is a valid regex")
});

/// TS: parse-keypress.js keyName (code → key name).
const KEY_NAMES: &[(&str, &str)] = &[
    ("OP", "f1"),
    ("OQ", "f2"),
    ("OR", "f3"),
    ("OS", "f4"),
    ("[11~", "f1"),
    ("[12~", "f2"),
    ("[13~", "f3"),
    ("[14~", "f4"),
    ("[[A", "f1"),
    ("[[B", "f2"),
    ("[[C", "f3"),
    ("[[D", "f4"),
    ("[[E", "f5"),
    ("[15~", "f5"),
    ("[17~", "f6"),
    ("[18~", "f7"),
    ("[19~", "f8"),
    ("[20~", "f9"),
    ("[21~", "f10"),
    ("[23~", "f11"),
    ("[24~", "f12"),
    ("[A", "up"),
    ("[B", "down"),
    ("[C", "right"),
    ("[D", "left"),
    ("[E", "clear"),
    ("[F", "end"),
    ("[H", "home"),
    ("OA", "up"),
    ("OB", "down"),
    ("OC", "right"),
    ("OD", "left"),
    ("OE", "clear"),
    ("OF", "end"),
    ("OH", "home"),
    ("[1~", "home"),
    ("[2~", "insert"),
    ("[3~", "delete"),
    ("[4~", "end"),
    ("[5~", "pageup"),
    ("[6~", "pagedown"),
    ("[[5~", "pageup"),
    ("[[6~", "pagedown"),
    ("[7~", "home"),
    ("[8~", "end"),
    ("[a", "up"),
    ("[b", "down"),
    ("[c", "right"),
    ("[d", "left"),
    ("[e", "clear"),
    ("[2$", "insert"),
    ("[3$", "delete"),
    ("[5$", "pageup"),
    ("[6$", "pagedown"),
    ("[7$", "home"),
    ("[8$", "end"),
    ("Oa", "up"),
    ("Ob", "down"),
    ("Oc", "right"),
    ("Od", "left"),
    ("Oe", "clear"),
    ("[2^", "insert"),
    ("[3^", "delete"),
    ("[5^", "pageup"),
    ("[6^", "pagedown"),
    ("[7^", "home"),
    ("[8^", "end"),
    ("[Z", "tab"),
];

fn key_name(code: &str) -> Option<&'static str> {
    KEY_NAMES
        .iter()
        .find(|(entry, _)| *entry == code)
        .map(|(_, name)| *name)
}

/// TS: parse-keypress.js nonAlphanumericKeys (every keyName value, then `backspace`): use-input reports these with an
/// empty `input`.
pub fn is_non_alphanumeric_key(name: &str) -> bool {
    name == "backspace" || KEY_NAMES.iter().any(|(_, entry)| *entry == name)
}

/// TS: parse-keypress.js isShiftKey.
fn is_shift_key(code: &str) -> bool {
    [
        "[a", "[b", "[c", "[d", "[e", "[2$", "[3$", "[5$", "[6$", "[7$", "[8$", "[Z",
    ]
    .contains(&code)
}

/// TS: parse-keypress.js isCtrlKey.
fn is_ctrl_key(code: &str) -> bool {
    ["Oa", "Ob", "Oc", "Od", "Oe", "[2^", "[3^", "[5^", "[6^", "[7^", "[8^"].contains(&code)
}

/// TS: parse-keypress.js kittySpecialLetterKeys.
fn kitty_special_letter_key(terminator: &str) -> Option<&'static str> {
    Some(match terminator {
        "A" => "up",
        "B" => "down",
        "C" => "right",
        "D" => "left",
        "E" => "clear",
        "F" => "end",
        "H" => "home",
        "P" => "f1",
        "Q" => "f2",
        "R" => "f3",
        "S" => "f4",
        _ => return None,
    })
}

/// TS: parse-keypress.js kittySpecialNumberKeys.
fn kitty_special_number_key(number: f64) -> Option<&'static str> {
    Some(match number as i64 {
        _ if number.fract() != 0.0 || !number.is_finite() => return None,
        2 => "insert",
        3 => "delete",
        5 => "pageup",
        6 => "pagedown",
        7 => "home",
        8 => "end",
        11 => "f1",
        12 => "f2",
        13 => "f3",
        14 => "f4",
        15 => "f5",
        17 => "f6",
        18 => "f7",
        19 => "f8",
        20 => "f9",
        21 => "f10",
        23 => "f11",
        24 => "f12",
        _ => return None,
    })
}

/// TS: parse-keypress.js kittyCodepointNames.
fn kitty_codepoint_name(codepoint: u32) -> Option<String> {
    let name = match codepoint {
        27 => "escape",
        9 => "tab",
        127 => "delete",
        8 => "backspace",
        57358 => "capslock",
        57359 => "scrolllock",
        57360 => "numlock",
        57361 => "printscreen",
        57362 => "pause",
        57363 => "menu",
        57376..=57398 => return Some(format!("f{}", codepoint - 57376 + 13)),
        57399..=57408 => return Some(format!("kp{}", codepoint - 57399)),
        57409 => "kpdecimal",
        57410 => "kpdivide",
        57411 => "kpmultiply",
        57412 => "kpsubtract",
        57413 => "kpadd",
        57414 => "kpenter",
        57415 => "kpequal",
        57416 => "kpseparator",
        57417 => "kpleft",
        57418 => "kpright",
        57419 => "kpup",
        57420 => "kpdown",
        57421 => "kppageup",
        57422 => "kppagedown",
        57423 => "kphome",
        57424 => "kpend",
        57425 => "kpinsert",
        57426 => "kpdelete",
        57427 => "kpbegin",
        57428 => "mediaplay",
        57429 => "mediapause",
        57430 => "mediaplaypause",
        57431 => "mediareverse",
        57432 => "mediastop",
        57433 => "mediafastforward",
        57434 => "mediarewind",
        57435 => "mediatracknext",
        57436 => "mediatrackprevious",
        57437 => "mediarecord",
        57438 => "lowervolume",
        57439 => "raisevolume",
        57440 => "mutevolume",
        57441 => "leftshift",
        57442 => "leftcontrol",
        57443 => "leftalt",
        57444 => "leftsuper",
        57445 => "lefthyper",
        57446 => "leftmeta",
        57447 => "rightshift",
        57448 => "rightcontrol",
        57449 => "rightalt",
        57450 => "rightsuper",
        57451 => "righthyper",
        57452 => "rightmeta",
        57453 => "isoLevel3Shift",
        57454 => "isoLevel5Shift",
        _ => return None,
    };
    Some(name.to_string())
}

/// TS `Number(digits)` / `parseInt(digits, 10)` for an all-digit string.
fn js_number(digits: &str) -> f64 {
    digits.parse::<f64>().unwrap_or(f64::NAN)
}

/// JS ToInt32 (what `&` applies to its operands).
fn to_int32(value: f64) -> i32 {
    if !value.is_finite() {
        return 0;
    }
    let truncated = value.trunc();
    let modulo = truncated.rem_euclid(4_294_967_296.0);
    let wrapped = if modulo >= 2_147_483_648.0 {
        modulo - 4_294_967_296.0
    } else {
        modulo
    };
    wrapped as i32
}

/// TS: parse-keypress.js isValidCodepoint.
fn is_valid_codepoint(codepoint: f64) -> bool {
    (0.0..=1_114_111.0).contains(&codepoint) && !(55_296.0..=57_343.0).contains(&codepoint)
}

/// TS: parse-keypress.js safeFromCodePoint.
fn safe_from_code_point(codepoint: f64) -> String {
    if is_valid_codepoint(codepoint) {
        char::from_u32(codepoint as u32).map_or_else(|| "?".to_string(), String::from)
    } else {
        "?".to_string()
    }
}

/// TS: parse-keypress.js resolveEventType.
fn resolve_event_type(value: f64) -> KeyEventType {
    if value == 3.0 {
        KeyEventType::Release
    } else if value == 2.0 {
        KeyEventType::Repeat
    } else {
        KeyEventType::Press
    }
}

/// TS: kitty-keyboard.js kittyModifiers, as parse-keypress.js parseKittyModifiers reads them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct KittyModifiers {
    ctrl: bool,
    shift: bool,
    meta: bool,
    option: bool,
    super_: bool,
    hyper: bool,
    caps_lock: bool,
    num_lock: bool,
}

/// TS: parse-keypress.js parseKittyModifiers.
fn parse_kitty_modifiers(modifiers: f64) -> KittyModifiers {
    let bits = to_int32(modifiers);
    KittyModifiers {
        ctrl: bits & 4 != 0,
        shift: bits & 1 != 0,
        meta: bits & 32 != 0,
        option: bits & 2 != 0,
        super_: bits & 8 != 0,
        hyper: bits & 16 != 0,
        caps_lock: bits & 64 != 0,
        num_lock: bits & 128 != 0,
    }
}

/// TS: parse-keypress.js's keypress object (`ParsedKey`). `name` is empty where TS leaves it `''` or `undefined`
/// (an unknown escape sequence); nothing downstream tells those apart.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Keypress {
    pub name: String,
    pub ctrl: bool,
    pub meta: bool,
    pub shift: bool,
    /// Option/Alt as a doubled escape (`ESC ESC [A`) or `ESC CR`, or the kitty alt modifier.
    pub option: bool,
    pub sequence: String,
    pub raw: Option<String>,
    /// The key code of an escape sequence (`[A`, `[3~`, ...).
    pub code: Option<String>,
    pub super_: bool,
    pub hyper: bool,
    pub caps_lock: bool,
    pub num_lock: bool,
    pub event_type: Option<KeyEventType>,
    pub is_kitty_protocol: bool,
    pub is_printable: bool,
    /// Kitty: the text the key produces.
    pub text: Option<String>,
}

impl Keypress {
    fn with_kitty_modifiers(mut self, modifiers: KittyModifiers) -> Self {
        self.ctrl = modifiers.ctrl;
        self.shift = modifiers.shift;
        self.meta = modifiers.meta;
        self.option = modifiers.option;
        self.super_ = modifiers.super_;
        self.hyper = modifiers.hyper;
        self.caps_lock = modifiers.caps_lock;
        self.num_lock = modifiers.num_lock;
        self
    }
}

/// TS: parse-keypress.js parseKittyKeypress (CSI codepoint ; modifiers [: event type] [; text] u).
fn parse_kitty_keypress(s: &str) -> Option<Keypress> {
    let captures = KITTY_KEY_RE.captures(s)?;
    let codepoint = js_number(captures.get(1)?.as_str());
    let modifiers = captures
        .get(2)
        .map_or(0.0, |value| (js_number(value.as_str()) - 1.0).max(0.0));
    let event_type = captures.get(3).map_or(1.0, |value| js_number(value.as_str()));
    if !is_valid_codepoint(codepoint) {
        return None;
    }
    let mut text = captures.get(4).map(|field| {
        field
            .as_str()
            .split(':')
            .map(|part| safe_from_code_point(if part.is_empty() { f64::NAN } else { js_number(part) }))
            .collect::<String>()
    });
    let (name, is_printable) = if codepoint == 32.0 {
        ("space".to_string(), true)
    } else if codepoint == 13.0 {
        ("return".to_string(), true)
    } else if let Some(name) = kitty_codepoint_name(codepoint as u32) {
        (name, false)
    } else if (1.0..=26.0).contains(&codepoint) {
        // Ctrl+letter comes as codepoint 1-26.
        (safe_from_code_point(codepoint + 96.0), false)
    } else {
        (safe_from_code_point(codepoint).to_lowercase(), true)
    };
    if is_printable && text.as_deref().is_none_or(str::is_empty) {
        text = Some(safe_from_code_point(codepoint));
    }
    Some(
        Keypress {
            name,
            event_type: Some(resolve_event_type(event_type)),
            sequence: s.to_string(),
            raw: Some(s.to_string()),
            is_kitty_protocol: true,
            is_printable,
            text,
            ..Keypress::default()
        }
        .with_kitty_modifiers(parse_kitty_modifiers(modifiers)),
    )
}

/// TS: parse-keypress.js parseKittySpecialKey (legacy CSI keys with a `:event type` field).
fn parse_kitty_special_key(s: &str) -> Option<Keypress> {
    let captures = KITTY_SPECIAL_KEY_RE.captures(s)?;
    let number = js_number(captures.get(1)?.as_str());
    let modifiers = (js_number(captures.get(2)?.as_str()) - 1.0).max(0.0);
    let event_type = js_number(captures.get(3)?.as_str());
    let terminator = captures.get(4)?.as_str();
    let name = if terminator == "~" {
        kitty_special_number_key(number)
    } else {
        kitty_special_letter_key(terminator)
    }?;
    Some(
        Keypress {
            name: name.to_string(),
            event_type: Some(resolve_event_type(event_type)),
            sequence: s.to_string(),
            raw: Some(s.to_string()),
            is_kitty_protocol: true,
            is_printable: false,
            ..Keypress::default()
        }
        .with_kitty_modifiers(parse_kitty_modifiers(modifiers)),
    )
}

/// UTF-16 length of `s` (TS `s.length`).
fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// TS: parse-keypress.js parseKeypress for one input event (a string).
pub fn parse_keypress(s: &str) -> Keypress {
    if let Some(kitty) = parse_kitty_keypress(s) {
        return kitty;
    }
    if let Some(special) = parse_kitty_special_key(s) {
        return special;
    }
    // Matched the kitty CSI-u pattern but was rejected (an invalid codepoint): a safe empty keypress.
    if KITTY_KEY_RE.is_match(s) {
        return Keypress {
            sequence: s.to_string(),
            raw: Some(s.to_string()),
            is_kitty_protocol: true,
            ..Keypress::default()
        };
    }
    let mut key = Keypress {
        sequence: s.to_string(),
        raw: Some(s.to_string()),
        ..Keypress::default()
    };
    let single = utf16_len(s) == 1;
    let first = s.chars().next();
    if s == "\r" || s == "\u{1b}\r" {
        // Carriage return (or Option+Return on macOS).
        key.raw = None;
        key.name = "return".to_string();
        key.option = s.len() == 2;
    } else if s == "\n" {
        key.name = "enter".to_string();
    } else if s == "\t" {
        key.name = "tab".to_string();
    } else if s == "\u{8}" || s == "\u{1b}\u{8}" {
        key.name = "backspace".to_string();
        key.meta = s.starts_with(ESC);
    } else if s == "\u{7f}" || s == "\u{1b}\u{7f}" {
        key.name = "delete".to_string();
        key.meta = s.starts_with(ESC);
    } else if s == ESC_STR || s == "\u{1b}\u{1b}" {
        key.name = "escape".to_string();
        key.meta = s.len() == 2;
    } else if s == " " || s == "\u{1b} " {
        key.name = "space".to_string();
        key.meta = s.len() == 2;
    } else if let (true, Some(character @ '\u{0}'..='\u{1a}')) = (single, first) {
        // Ctrl+letter.
        key.name = char::from_u32(character as u32 + 'a' as u32 - 1).map_or_else(String::new, String::from);
        key.ctrl = true;
    } else if let (true, Some(character @ '0'..='9')) = (single, first) {
        let _ = character;
        key.name = "number".to_string();
    } else if let (true, Some(character @ 'a'..='z')) = (single, first) {
        key.name = character.to_string();
    } else if let (true, Some(character @ 'A'..='Z')) = (single, first) {
        key.name = character.to_ascii_lowercase().to_string();
        key.shift = true;
    } else if let Some(captures) = META_KEY_CODE_RE.captures(s) {
        // Meta+character.
        key.meta = true;
        key.shift = captures
            .get(1)
            .is_some_and(|letter| letter.as_str().chars().all(|c| c.is_ascii_uppercase()));
    } else if let Some(captures) = FN_KEY_RE.captures(s) {
        let mut code_points = s.chars();
        if code_points.next() == Some(ESC) && code_points.next() == Some(ESC) {
            key.option = true;
        }
        // The key code without the leading escapes, the modifier and any meaningless "1;".
        let code: String = [1, 2, 4, 6]
            .iter()
            .filter_map(|index| captures.get(*index))
            .map(|part| part.as_str())
            .filter(|part| !part.is_empty())
            .collect();
        let modifier_text = captures
            .get(3)
            .or_else(|| captures.get(5))
            .map(|part| part.as_str())
            .filter(|part| !part.is_empty());
        let modifier = to_int32(modifier_text.map_or(1.0, js_number) - 1.0);
        key.ctrl = modifier & 4 != 0;
        key.meta = modifier & 10 != 0;
        key.shift = modifier & 1 != 0;
        key.name = key_name(&code).unwrap_or_default().to_string();
        key.shift = is_shift_key(&code) || key.shift;
        key.ctrl = is_ctrl_key(&code) || key.ctrl;
        key.code = Some(code);
    }
    key
}

// ---------------------------------------------------------------------------------------------------------------
// hooks/use-input.js

/// TS: hooks/use-input.js handleData: the `(input, key)` a `useInput` handler gets for one event.
pub fn input_for_event(event: &str) -> Input {
    let keypress = parse_keypress(event);
    let name = keypress.name.as_str();
    let mut key = Key {
        up_arrow: name == "up",
        down_arrow: name == "down",
        left_arrow: name == "left",
        right_arrow: name == "right",
        page_down: name == "pagedown",
        page_up: name == "pageup",
        home: name == "home",
        end: name == "end",
        return_: name == "return",
        escape: name == "escape",
        ctrl: keypress.ctrl,
        shift: keypress.shift,
        tab: name == "tab",
        backspace: name == "backspace",
        delete: name == "delete",
        // parseKeypress reports ESC ESC [A (meta + up) as option, not meta.
        meta: keypress.meta || name == "escape" || keypress.option,
        super_: keypress.super_,
        hyper: keypress.hyper,
        caps_lock: keypress.caps_lock,
        num_lock: keypress.num_lock,
        event_type: keypress.event_type,
    };
    let mut input = if keypress.is_kitty_protocol {
        if keypress.is_printable {
            keypress.text.clone().unwrap_or_else(|| keypress.name.clone())
        } else if keypress.ctrl && utf16_len(&keypress.name) == 1 {
            // Ctrl+letter via codepoints 1-26: not text, but the letter flows through.
            keypress.name.clone()
        } else {
            String::new()
        }
    } else if keypress.ctrl {
        keypress.name.clone()
    } else {
        keypress.sequence.clone()
    };
    if !keypress.is_kitty_protocol && is_non_alphanumeric_key(name) {
        input.clear();
    }
    // Meta left over after parseKeypress.
    if let Some(rest) = input.strip_prefix(ESC) {
        input = rest.to_string();
    }
    if utf16_len(&input) == 1 && input.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
        key.shift = true;
    }
    Input { input, key }
}

/// One Ink `input` event: the framed sequence (what useTerminalInput captures) and what `useInput` makes of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyEvent {
    pub sequence: String,
    pub input: Input,
}

impl KeyEvent {
    /// The event for one framed sequence.
    pub fn from_sequence(sequence: String) -> Self {
        let input = input_for_event(&sequence);
        KeyEvent { sequence, input }
    }
}

/// One read from stdin: its text (what a raw `stdin.on('data')` listener sees, after Ink's `setEncoding('utf8')`)
/// and the key events it completed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StdinChunk {
    pub text: String,
    pub events: Vec<KeyEvent>,
}

/// Raw stdin bytes → Ink input events: Node's UTF-8 `StringDecoder` then Ink's input parser (components/App.js
/// handleReadable). The caller flushes a held escape when no more input is immediately available (Ink's
/// `setImmediate`).
#[derive(Clone, Debug, Default)]
pub struct StdinParser {
    decoder: Utf8Decoder,
    parser: InputParser,
}

impl StdinParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decodes `bytes` (an incomplete UTF-8 tail is held) and parses the text.
    pub fn push(&mut self, bytes: &[u8]) -> StdinChunk {
        let text = self.decoder.write(bytes);
        let events = self
            .parser
            .push(&text)
            .into_iter()
            .map(KeyEvent::from_sequence)
            .collect();
        StdinChunk { text, events }
    }

    /// TS: App.js hasPendingEscape.
    pub fn has_pending_escape(&self) -> bool {
        self.parser.has_pending_escape()
    }

    /// TS: App.js schedulePendingInputFlush's callback: the held escape as its own event.
    pub fn flush_pending_escape(&mut self) -> Option<KeyEvent> {
        self.parser.flush_pending_escape().map(KeyEvent::from_sequence)
    }

    /// TS: App.js disableRawMode → inputParser.reset (the decoder keeps a held byte, as Node's stream does).
    pub fn reset(&mut self) {
        self.parser.reset();
    }
}

// ---------------------------------------------------------------------------------------------------------------
// useTerminalInput.ts

/// TS: useTerminalInput.ts useTerminalInput's handler: key releases (kitty) are dropped; the key is normalised with
/// the framed sequence (Backspace vs Delete, Alt+Backspace) before the app sees it.
pub fn terminal_input(event: &KeyEvent) -> Option<Input> {
    if event.input.key.event_type == Some(KeyEventType::Release) {
        return None;
    }
    Some(Input {
        input: event.input.input.clone(),
        key: normalize_terminal_key(event.input.key, &event.sequence),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(input: &str) -> Vec<String> {
        parse_keypresses(input).events
    }

    #[test]
    fn splits_keys_and_keeps_text_runs_whole() {
        assert_eq!(events("abc"), ["abc"]);
        assert_eq!(events("a\u{1b}[Ab"), ["a", "\u{1b}[A", "b"]);
        assert_eq!(events("\u{1b}[A\u{1b}[B"), ["\u{1b}[A", "\u{1b}[B"]);
        assert_eq!(events("\u{1b}bx"), ["\u{1b}b", "x"]);
        assert_eq!(events("\u{1b}😀x"), ["\u{1b}😀", "x"]);
        assert_eq!(parse_keypresses("x\u{1b}").pending, "\u{1b}");
        assert_eq!(parse_keypresses("\u{1b}[1;").pending, "\u{1b}[1;");
        assert_eq!(parse_keypresses("\u{1b}\u{1b}").pending, "\u{1b}\u{1b}");
    }

    #[test]
    fn a_held_escape_waits_for_the_next_chunk_or_the_flush() {
        let mut parser = InputParser::new();
        assert!(parser.push("\u{1b}").is_empty());
        assert!(parser.has_pending_escape());
        assert_eq!(parser.push("[A"), ["\u{1b}[A"]);
        assert!(!parser.has_pending_escape());
        assert!(parser.push("\u{1b}").is_empty());
        assert_eq!(parser.flush_pending_escape().as_deref(), Some("\u{1b}"));
        assert_eq!(parser.flush_pending_escape(), None);
        assert!(parser.push("\u{1b}[").is_empty());
        parser.reset();
        assert_eq!(parser.push("A"), ["A"]);
    }

    #[test]
    fn to_int32_wraps_like_js() {
        assert_eq!(to_int32(-1.0), -1);
        assert_eq!(to_int32(4_294_967_297.0), 1);
        assert_eq!(to_int32(2_147_483_648.0), -2_147_483_648);
        assert_eq!(to_int32(f64::NAN), 0);
    }

    #[test]
    fn modifier_zero_is_every_modifier_as_in_js() {
        // `("0" || 1) - 1` is -1 in JS: every bit set.
        let key = input_for_event("\u{1b}[1;0A").key;
        assert!(key.up_arrow && key.ctrl && key.meta && key.shift);
    }

    #[test]
    fn stdin_parser_reports_the_decoded_text_and_events() {
        let mut parser = StdinParser::new();
        let chunk = parser.push(&[0xe6, 0x97]);
        assert_eq!(chunk, StdinChunk::default());
        let chunk = parser.push(&[0xa5, b'x', 0x1b]);
        assert_eq!(chunk.text, "日x\u{1b}");
        assert_eq!(chunk.events.len(), 1);
        assert_eq!(chunk.events[0].sequence, "日x");
        assert_eq!(chunk.events[0].input, Input::text("日x"));
        assert!(parser.has_pending_escape());
        let escape = parser.flush_pending_escape().expect("held escape");
        assert!(escape.input.key.escape && escape.input.key.meta);
        assert_eq!(escape.input.input, "");
    }

    #[test]
    fn terminal_input_drops_key_releases() {
        let release = KeyEvent::from_sequence("\u{1b}[97;1:3u".to_string());
        assert_eq!(release.input.key.event_type, Some(KeyEventType::Release));
        assert_eq!(terminal_input(&release), None);
    }
}
