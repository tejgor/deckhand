//! Port of JS string semantics: UTF-16 length/slice, localeCompare.
//!
//! TS strings are UTF-16; Rust strings are UTF-8. Everything here takes and returns UTF-16 code-unit indices where
//! the JS original does, so a ported `text.slice(0, width - 1)` is `js_slice(text, 0, Some(width - 1))`.
//!
//! Lone surrogates: a JS slice can cut a surrogate pair in half; a Rust `String` cannot hold the half. Wherever that
//! happens the half becomes U+FFFD, which is what Node writes for a lone surrogate when it encodes the string as UTF-8
//! (to a terminal, a file or a socket), and which keeps the UTF-16 length unchanged (one unit either way).
//!
//! Also here, because ported code needs them everywhere and they are part of the JS runtime's semantics rather than
//! any one module's: [`path`] (Node's `path.posix`) and [`date`] (`Date.parse` / `toISOString`).

use std::cmp::Ordering;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------------------------------------------
// Lengths and indices

/// `text.length`: UTF-16 code units.
pub fn js_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// The byte offset of UTF-16 index `index` (clamped to the end). An index inside a surrogate pair rounds down to the
/// pair's start, so the result is always a char boundary.
pub fn utf16_to_byte(text: &str, index: usize) -> usize {
    let mut units = 0;
    for (byte, ch) in text.char_indices() {
        let next = units + ch.len_utf16();
        if next > index {
            return byte;
        }
        units = next;
    }
    text.len()
}

/// The UTF-16 index of byte offset `byte` (clamped to the end; a non-boundary offset rounds down to its char).
pub fn byte_to_utf16(text: &str, byte: usize) -> usize {
    let mut units = 0;
    for (start, ch) in text.char_indices() {
        if start + ch.len_utf8() > byte {
            return units;
        }
        units += ch.len_utf16();
    }
    units
}

/// The UTF-16 code unit at `index` (`text.charCodeAt(index)`), or `None` past the end (JS: NaN).
pub fn js_code_unit_at(text: &str, index: usize) -> Option<u16> {
    text.encode_utf16().nth(index)
}

/// `text[index]` as a string (a lone half of a surrogate pair becomes U+FFFD), or `None` past the end.
pub fn js_char_at(text: &str, index: usize) -> Option<String> {
    js_code_unit_at(text, index).map(|unit| String::from_utf16_lossy(&[unit]))
}

/// Resolves a relative index the way `slice` does: negative counts from the end, then clamped to `0..=len`.
fn relative_index(len: usize, index: i64) -> usize {
    let len_i = i64::try_from(len).unwrap_or(i64::MAX);
    let resolved = if index < 0 {
        (len_i + index).max(0)
    } else {
        index.min(len_i)
    };
    usize::try_from(resolved).unwrap_or(0)
}

/// `text.slice(start, end)` with UTF-16 indices (negative ones count from the end).
pub fn js_slice(text: &str, start: i64, end: Option<i64>) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    let from = relative_index(units.len(), start);
    let to = end.map_or(units.len(), |end| relative_index(units.len(), end));
    if from >= to {
        return String::new();
    }
    String::from_utf16_lossy(&units[from..to])
}

/// `text.substring(start, end)` with UTF-16 indices: negative ones are 0, and the two are swapped when reversed.
pub fn js_substring(text: &str, start: i64, end: Option<i64>) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    let clamp = |index: i64| usize::try_from(index.max(0)).unwrap_or(0).min(units.len());
    let a = clamp(start);
    let b = end.map_or(units.len(), clamp);
    let (from, to) = if a <= b { (a, b) } else { (b, a) };
    String::from_utf16_lossy(&units[from..to])
}

/// `text.indexOf(search, from)` as a UTF-16 index.
pub fn js_index_of(text: &str, search: &str, from: usize) -> Option<usize> {
    let mut start = utf16_to_byte(text, from);
    // A `from` inside a surrogate pair skips the pair: no needle can start at its low half.
    if byte_to_utf16(text, start) < from {
        start += text[start..].chars().next().map_or(0, char::len_utf8);
    }
    text.get(start..)
        .and_then(|rest| rest.find(search))
        .map(|at| byte_to_utf16(text, start + at))
}

/// `text.lastIndexOf(search)` as a UTF-16 index.
pub fn js_last_index_of(text: &str, search: &str) -> Option<usize> {
    text.rfind(search).map(|at| byte_to_utf16(text, at))
}

/// `text.padStart(width, fill)`: `fill` repeated (and cut) in front until the UTF-16 length is `width`.
pub fn js_pad_start(text: &str, width: usize, fill: &str) -> String {
    let pad = padding(js_len(text), width, fill);
    format!("{pad}{text}")
}

/// `text.padEnd(width, fill)`.
pub fn js_pad_end(text: &str, width: usize, fill: &str) -> String {
    let pad = padding(js_len(text), width, fill);
    format!("{text}{pad}")
}

fn padding(len: usize, width: usize, fill: &str) -> String {
    if len >= width || fill.is_empty() {
        return String::new();
    }
    let missing = width - len;
    let fill_len = js_len(fill);
    let repeated = fill.repeat(missing / fill_len + 1);
    js_slice(&repeated, 0, Some(i64::try_from(missing).unwrap_or(i64::MAX)))
}

// ---------------------------------------------------------------------------------------------------------------
// Case and whitespace

/// `text.toLowerCase()`. Rust's `to_lowercase` applies the same full Unicode mapping, final sigma included.
pub fn js_to_lower(text: &str) -> String {
    text.to_lowercase()
}

/// `text.toUpperCase()` (full mapping: `ß` → `SS`, as in JS).
pub fn js_to_upper(text: &str) -> String {
    text.to_uppercase()
}

/// JS whitespace (`\s`, `trim`): WhiteSpace plus LineTerminator. Unlike `char::is_whitespace` it includes U+FEFF and
/// excludes U+0085 (NEL).
pub fn is_js_whitespace(ch: char) -> bool {
    matches!(
        ch,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    )
}

/// `text.trim()`.
pub fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_whitespace)
}

/// `text.trimStart()`.
pub fn js_trim_start(text: &str) -> &str {
    text.trim_start_matches(is_js_whitespace)
}

/// `text.trimEnd()`.
pub fn js_trim_end(text: &str) -> &str {
    text.trim_end_matches(is_js_whitespace)
}

/// `text.split(/\s+/).filter(Boolean)`: the whitespace-separated words.
pub fn js_split_whitespace(text: &str) -> Vec<&str> {
    text.split(is_js_whitespace).filter(|word| !word.is_empty()).collect()
}

/// `text.replace(/\s+/g, ' ')`: every run of JS whitespace as one space.
pub fn js_collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for ch in text.chars() {
        if is_js_whitespace(ch) {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(ch);
            in_space = false;
        }
    }
    out
}

/// `JSON.stringify(text)` for a string (JS and serde_json escape the same characters, with lowercase hex).
pub fn json_stringify_str(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| String::from("\"\""))
}

// ---------------------------------------------------------------------------------------------------------------
// localeCompare

/// `a.localeCompare(b)` with Node's default collator (ICU root/en-US: sensitivity `variant`, punctuation not ignored,
/// not numeric): primary weights (base letters, with punctuation and symbols before digits before letters), then
/// accents, then case and width (lowercase first). Strings equal at all three levels compare equal, as in JS.
///
/// The weights are measured from Node by `tests/data/gen_collation.mjs` for Latin, Greek, Cyrillic, the other
/// scripts below U+2000, punctuation, symbols, kana, BMP Han (ICU's radical-stroke order), fullwidth forms and emoji;
/// Hangul syllables decompose to their jamo. Anything else (Han beyond the BMP included, which ICU interleaves with
/// BMP Han) sorts after the table by code point, as UCA's implicit weights do. Thai and other scripts with
/// contractions or reordering are compared character by character.
pub fn locale_compare(a: &str, b: &str) -> Ordering {
    if a == b {
        return Ordering::Equal;
    }
    let left = collation_elements(a);
    let right = collation_elements(b);
    let primary = |ces: &[Ce]| {
        ces.iter()
            .filter(|ce| ce.primary > 0)
            .map(|ce| ce.primary)
            .collect::<Vec<_>>()
    };
    let secondary = |ces: &[Ce]| {
        ces.iter()
            .filter(|ce| ce.secondary > 0)
            .map(|ce| ce.secondary)
            .collect::<Vec<_>>()
    };
    let tertiary = |ces: &[Ce]| {
        ces.iter()
            .filter(|ce| ce.primary > 0 || ce.secondary > 0)
            .map(|ce| ce.tertiary)
            .collect::<Vec<_>>()
    };
    primary(&left)
        .cmp(&primary(&right))
        .then_with(|| secondary(&left).cmp(&secondary(&right)))
        .then_with(|| tertiary(&left).cmp(&tertiary(&right)))
}

/// `a.localeCompare(b)` as JS returns it: -1, 0 or 1.
pub fn locale_compare_sign(a: &str, b: &str) -> i32 {
    match locale_compare(a, b) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// One collation element: weights per level (0: ignorable at that level).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Ce {
    primary: u32,
    secondary: u32,
    tertiary: u32,
}

/// The secondary weight of every primary-bearing element (marks weigh more).
const COMMON: u32 = 1;

#[derive(Clone, Debug)]
enum Entry {
    /// A run of code points whose primary weight grows by one per code point. Tertiary weights are ranks within the
    /// character's primary class: strings equal at the first two levels only ever compare tertiaries within a class.
    Primary {
        primary: u32,
        extra: u32,
        tertiary: u32,
    },
    Mark(u32),
    Ignorable,
    Decomposition(Vec<u32>),
    /// `tertiary`: the first element's tertiary weight among its class (ﬃ sorts after ffi, before FFI).
    Expansion {
        extra: u32,
        tertiary: Option<u32>,
        parts: Vec<u32>,
    },
}

/// The table as code-point ranges sorted by start (runs keep it small: Han is mostly in code-point order), plus the
/// contractions and the canonical combining classes (as ranks) NFD reordering needs.
struct Table {
    ranges: Vec<(u32, u32, Entry)>,
    contractions: Vec<((u32, u32), u32)>,
    combining: Vec<(u32, u32)>,
}

impl Table {
    /// The entry for `cp` and its offset in its run.
    fn get(&self, cp: u32) -> Option<(&Entry, u32)> {
        let at = self.ranges.partition_point(|(start, _, _)| *start <= cp);
        let (start, end, entry) = self.ranges.get(at.checked_sub(1)?)?;
        (cp <= *end).then_some((entry, cp - start))
    }

    fn combining_class(&self, cp: u32) -> u32 {
        self.combining
            .binary_search_by_key(&cp, |(code, _)| *code)
            .map_or(0, |at| self.combining[at].1)
    }

    fn contraction(&self, first: u32, second: u32) -> Option<u32> {
        self.contractions
            .binary_search_by_key(&(first, second), |(pair, _)| *pair)
            .ok()
            .map(|at| self.contractions[at].1)
    }

    /// Canonical decomposition of `cp` (table decompositions and Hangul syllables), appended to `out`.
    fn decompose(&self, cp: u32, out: &mut Vec<u32>, depth: u8) {
        const S_BASE: u32 = 0xAC00;
        const S_COUNT: u32 = 11172;
        if (S_BASE..S_BASE + S_COUNT).contains(&cp) {
            let index = cp - S_BASE;
            out.push(0x1100 + index / 588);
            out.push(0x1161 + (index % 588) / 28);
            if !index.is_multiple_of(28) {
                out.push(0x11A7 + index % 28);
            }
            return;
        }
        match self.get(cp) {
            Some((Entry::Decomposition(parts), _)) if depth < 8 => {
                for part in parts {
                    self.decompose(*part, out, depth + 1);
                }
            }
            _ => out.push(cp),
        }
    }
}

fn collation_table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| parse_collation_table(include_str!("jsstr/collation.txt")))
}

fn parse_collation_table(data: &str) -> Table {
    let mut table = Table {
        ranges: Vec::new(),
        contractions: Vec::new(),
        combining: Vec::new(),
    };
    let hex = |text: &str| u32::from_str_radix(text, 16).ok();
    let num = |text: Option<&str>| text.and_then(|text| text.parse::<u32>().ok()).unwrap_or(0);
    let range = |text: &str| -> Option<(u32, u32)> {
        match text.split_once('-') {
            Some((start, end)) => Some((hex(start)?, hex(end)?)),
            None => hex(text).map(|cp| (cp, cp)),
        }
    };
    for line in data.lines() {
        let Some(kind) = line.chars().next() else { continue };
        let mut fields = line[kind.len_utf8()..].split(' ');
        let Some((start, end)) = fields.next().and_then(range) else {
            continue;
        };
        let entry = match kind {
            'P' => Entry::Primary {
                primary: num(fields.next()),
                extra: num(fields.next()),
                tertiary: num(fields.next()),
            },
            'M' => Entry::Mark(num(fields.next())),
            'I' => Entry::Ignorable,
            'D' => Entry::Decomposition(fields.filter_map(hex).collect()),
            'X' => Entry::Expansion {
                extra: num(fields.next()),
                tertiary: fields.next().and_then(|text| text.parse::<u32>().ok()),
                parts: fields.filter_map(hex).collect(),
            },
            'K' => {
                if let (Some(first), Some(second)) = (fields.next().and_then(hex), fields.next().and_then(hex)) {
                    table.contractions.push(((first, second), start));
                }
                continue;
            }
            'C' => {
                table.combining.push((start, num(fields.next())));
                continue;
            }
            _ => continue,
        };
        table.ranges.push((start, end, entry));
    }
    table.ranges.sort_by_key(|(start, _, _)| *start);
    table.contractions.sort();
    table.combining.sort();
    table
}

/// The collation elements of `text`: NFD (decomposition, then canonical reordering of combining marks), two-character
/// contractions, then each character's elements.
fn collation_elements(text: &str) -> Vec<Ce> {
    let table = collation_table();
    let mut code_points = Vec::with_capacity(text.len());
    for ch in text.chars() {
        table.decompose(u32::from(ch), &mut code_points, 0);
    }
    // Canonical ordering: a run of combining marks is stably sorted by combining class.
    let mut index = 0;
    while index < code_points.len() {
        if table.combining_class(code_points[index]) == 0 {
            index += 1;
            continue;
        }
        let start = index;
        while index < code_points.len() && table.combining_class(code_points[index]) != 0 {
            index += 1;
        }
        code_points[start..index].sort_by_key(|cp| table.combining_class(*cp));
    }
    let mut out = Vec::with_capacity(code_points.len());
    let mut index = 0;
    while index < code_points.len() {
        let cp = code_points[index];
        if let Some(contracted) = code_points.get(index + 1).and_then(|next| table.contraction(cp, *next)) {
            push_elements(table, contracted, &mut out, 0);
            index += 2;
        } else {
            push_elements(table, cp, &mut out, 0);
            index += 1;
        }
    }
    out
}

fn push_elements(table: &Table, cp: u32, out: &mut Vec<Ce>, depth: u8) {
    if depth > 8 {
        return;
    }
    match table.get(cp) {
        Some((
            Entry::Primary {
                primary,
                extra,
                tertiary,
            },
            offset,
        )) => {
            out.push(Ce {
                primary: primary + offset,
                secondary: COMMON,
                tertiary: *tertiary,
            });
            if *extra > 0 {
                out.push(Ce {
                    primary: 0,
                    secondary: *extra,
                    tertiary: 0,
                });
            }
        }
        Some((Entry::Mark(weight), _)) => out.push(Ce {
            primary: 0,
            secondary: *weight,
            tertiary: 0,
        }),
        Some((Entry::Ignorable, _)) => {}
        Some((Entry::Decomposition(parts), _)) => {
            for part in parts {
                push_elements(table, *part, out, depth + 1);
            }
        }
        Some((Entry::Expansion { extra, tertiary, parts }, _)) => {
            let start = out.len();
            for part in parts {
                push_elements(table, *part, out, depth + 1);
            }
            if let (Some(first), Some(tertiary)) = (out.get_mut(start), tertiary) {
                first.tertiary = *tertiary;
            }
            if *extra > 0 && out.len() > start {
                out.insert(
                    start + 1,
                    Ce {
                        primary: 0,
                        secondary: *extra,
                        tertiary: 0,
                    },
                );
            }
        }
        None => out.push(Ce {
            primary: implicit_primary(cp),
            secondary: COMMON,
            tertiary: 0,
        }),
    }
}

/// UCA implicit weights: Han (core, then extensions) and then everything else, by code point, after the table.
fn implicit_primary(cp: u32) -> u32 {
    const CORE_HAN_COMPAT: [u32; 12] = [
        0xFA0E, 0xFA0F, 0xFA11, 0xFA13, 0xFA14, 0xFA1F, 0xFA21, 0xFA23, 0xFA24, 0xFA27, 0xFA28, 0xFA29,
    ];
    let core = (0x4E00..=0x9FFF).contains(&cp) || CORE_HAN_COMPAT.contains(&cp);
    let extension = (0x3400..=0x4DBF).contains(&cp)
        || (0x20000..=0x2A6DF).contains(&cp)
        || (0x2A700..=0x2EE5F).contains(&cp)
        || (0x30000..=0x323AF).contains(&cp);
    let group = if core {
        0x0100_0000
    } else if extension {
        0x0140_0000
    } else {
        0x0180_0000
    };
    group + cp
}

// ---------------------------------------------------------------------------------------------------------------
// Node's path module (POSIX)

/// Port of Node's `path.posix` (deckhand only runs on POSIX). Byte-wise like Node's UTF-16 scan: `/` and `.` are ASCII,
/// so every cut lands on a char boundary.
pub mod path {
    /// `path.sep`.
    pub const SEP: &str = "/";

    /// `path.isAbsolute(path)`.
    pub fn is_absolute(path: &str) -> bool {
        path.starts_with('/')
    }

    /// Node's `normalizeString`: resolves `.` and `..` segments and repeated separators.
    fn normalize_string(path: &str, allow_above_root: bool) -> String {
        let bytes = path.as_bytes();
        let mut res = String::new();
        let mut last_segment_length = 0usize;
        let mut last_slash: isize = -1;
        let mut dots: isize = 0;
        let mut code = 0u8;
        let mut i = 0usize;
        while i <= bytes.len() {
            if i < bytes.len() {
                code = bytes[i];
            } else if code == b'/' {
                break;
            } else {
                code = b'/';
            }
            let at = isize::try_from(i).unwrap_or(isize::MAX);
            if code == b'/' {
                if last_slash == at - 1 || dots == 1 {
                    // A repeated separator or a `.` segment.
                } else if dots == 2 {
                    let ends_in_dots = res.len() >= 2 && last_segment_length == 2 && res.ends_with("..");
                    if !ends_in_dots {
                        if res.len() > 2 {
                            match res.rfind('/') {
                                None => {
                                    res.clear();
                                    last_segment_length = 0;
                                }
                                Some(index) => {
                                    res.truncate(index);
                                    // `res.length - 1 - res.lastIndexOf('/')` (lastIndexOf -1: the whole length).
                                    last_segment_length = res.rfind('/').map_or(res.len(), |at| res.len() - 1 - at);
                                }
                            }
                            last_slash = at;
                            dots = 0;
                            i += 1;
                            continue;
                        } else if !res.is_empty() {
                            res.clear();
                            last_segment_length = 0;
                            last_slash = at;
                            dots = 0;
                            i += 1;
                            continue;
                        }
                    }
                    if allow_above_root {
                        res.push_str(if res.is_empty() { ".." } else { "/.." });
                        last_segment_length = 2;
                    }
                } else {
                    let start = usize::try_from(last_slash + 1).unwrap_or(0);
                    let segment = path.get(start..i).unwrap_or("");
                    if !res.is_empty() {
                        res.push('/');
                    }
                    res.push_str(segment);
                    last_segment_length = i - start;
                }
                last_slash = at;
                dots = 0;
            } else if code == b'.' && dots != -1 {
                dots += 1;
            } else {
                dots = -1;
            }
            i += 1;
        }
        res
    }

    /// `path.normalize(path)`.
    pub fn normalize(path: &str) -> String {
        if path.is_empty() {
            return ".".into();
        }
        let absolute = path.starts_with('/');
        let trailing = path.ends_with('/');
        let mut out = normalize_string(path, !absolute);
        if out.is_empty() {
            if absolute {
                return "/".into();
            }
            return if trailing { "./".into() } else { ".".into() };
        }
        if trailing {
            out.push('/');
        }
        if absolute { format!("/{out}") } else { out }
    }

    /// `path.join(...parts)`.
    pub fn join(parts: &[&str]) -> String {
        let joined = parts
            .iter()
            .filter(|part| !part.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join("/");
        if joined.is_empty() {
            ".".into()
        } else {
            normalize(&joined)
        }
    }

    /// `path.resolve(...parts)`: relative parts resolve against `process.cwd()`.
    pub fn resolve_all(parts: &[&str]) -> String {
        let mut resolved = String::new();
        let mut absolute = false;
        for part in parts.iter().rev() {
            if part.is_empty() {
                continue;
            }
            resolved = format!("{part}/{resolved}");
            absolute = part.starts_with('/');
            if absolute {
                break;
            }
        }
        if !absolute {
            let cwd = std::env::current_dir()
                .map(|dir| dir.to_string_lossy().into_owned())
                .unwrap_or_default();
            resolved = format!("{cwd}/{resolved}");
            absolute = cwd.starts_with('/');
        }
        let normalized = normalize_string(&resolved, !absolute);
        if absolute {
            format!("/{normalized}")
        } else if normalized.is_empty() {
            ".".into()
        } else {
            normalized
        }
    }

    /// `path.resolve(path)`.
    pub fn resolve(path: &str) -> String {
        resolve_all(&[path])
    }

    /// `path.relative(from, to)`.
    pub fn relative(from: &str, to: &str) -> String {
        if from == to {
            return String::new();
        }
        let from = resolve(from);
        let to = resolve(to);
        if from == to {
            return String::new();
        }
        let (f, t) = (from.as_bytes(), to.as_bytes());
        let from_start = 1usize;
        let from_end = f.len();
        let from_len = from_end - from_start;
        let to_start = 1usize;
        let to_len = t.len() - to_start;
        let length = from_len.min(to_len);
        let mut last_common_sep: isize = -1;
        let mut i = 0usize;
        while i < length {
            let code = f[from_start + i];
            if code != t[to_start + i] {
                break;
            } else if code == b'/' {
                last_common_sep = isize::try_from(i).unwrap_or(isize::MAX);
            }
            i += 1;
        }
        if i == length {
            if to_len > length {
                if t[to_start + i] == b'/' {
                    return to.get(to_start + i + 1..).unwrap_or("").to_owned();
                }
                if i == 0 {
                    return to.get(to_start + i..).unwrap_or("").to_owned();
                }
            } else if from_len > length {
                if f[from_start + i] == b'/' {
                    last_common_sep = isize::try_from(i).unwrap_or(isize::MAX);
                } else if i == 0 {
                    last_common_sep = 0;
                }
            }
        }
        let mut out = String::new();
        let mut index = usize::try_from(isize::try_from(from_start).unwrap_or(0) + last_common_sep + 1).unwrap_or(0);
        while index <= from_end {
            if index == from_end || f[index] == b'/' {
                out.push_str(if out.is_empty() { ".." } else { "/.." });
            }
            index += 1;
        }
        let tail_start = usize::try_from(isize::try_from(to_start).unwrap_or(0) + last_common_sep).unwrap_or(0);
        format!("{out}{}", to.get(tail_start..).unwrap_or(""))
    }

    /// `path.dirname(path)`.
    pub fn dirname(path: &str) -> String {
        if path.is_empty() {
            return ".".into();
        }
        let bytes = path.as_bytes();
        let has_root = bytes[0] == b'/';
        let mut end: Option<usize> = None;
        let mut matched_slash = true;
        let mut i = bytes.len() - 1;
        while i >= 1 {
            if bytes[i] == b'/' {
                if !matched_slash {
                    end = Some(i);
                    break;
                }
            } else {
                matched_slash = false;
            }
            i -= 1;
        }
        match end {
            None => (if has_root { "/" } else { "." }).into(),
            Some(1) if has_root => "//".into(),
            Some(end) => path.get(..end).unwrap_or("").to_owned(),
        }
    }

    /// `path.basename(path)`.
    pub fn basename(path: &str) -> String {
        let bytes = path.as_bytes();
        let mut start = 0usize;
        let mut end: Option<usize> = None;
        let mut matched_slash = true;
        for i in (0..bytes.len()).rev() {
            if bytes[i] == b'/' {
                if !matched_slash {
                    start = i + 1;
                    break;
                }
            } else if end.is_none() {
                matched_slash = false;
                end = Some(i + 1);
            }
        }
        match end {
            None => String::new(),
            Some(end) => path.get(start..end).unwrap_or("").to_owned(),
        }
    }

    /// `path.basename(path, suffix)`: the base name without `suffix` (when it ends with it and is not just it).
    pub fn basename_without(path: &str, suffix: &str) -> String {
        if suffix.is_empty() || suffix.len() > path.len() {
            return basename(path);
        }
        if suffix == path {
            return String::new();
        }
        let bytes = path.as_bytes();
        let ext = suffix.as_bytes();
        let mut start = 0usize;
        let mut end: Option<usize> = None;
        let mut matched_slash = true;
        let mut ext_index: isize = isize::try_from(ext.len()).unwrap_or(0) - 1;
        let mut first_non_slash_end: Option<usize> = None;
        for i in (0..bytes.len()).rev() {
            let code = bytes[i];
            if code == b'/' {
                if !matched_slash {
                    start = i + 1;
                    break;
                }
            } else {
                if first_non_slash_end.is_none() {
                    matched_slash = false;
                    first_non_slash_end = Some(i + 1);
                }
                if ext_index >= 0 {
                    let expected = usize::try_from(ext_index).ok().and_then(|at| ext.get(at)).copied();
                    if Some(code) == expected {
                        ext_index -= 1;
                        if ext_index == -1 {
                            end = Some(i);
                        }
                    } else {
                        ext_index = -1;
                        end = first_non_slash_end;
                    }
                }
            }
        }
        let end = if Some(start) == end {
            first_non_slash_end.unwrap_or(0)
        } else {
            end.unwrap_or(bytes.len())
        };
        path.get(start..end).unwrap_or("").to_owned()
    }

    /// `path.extname(path)`.
    pub fn extname(path: &str) -> String {
        let bytes = path.as_bytes();
        let mut start_dot: Option<usize> = None;
        let mut start_part = 0usize;
        let mut end: Option<usize> = None;
        let mut matched_slash = true;
        let mut pre_dot_state = 0i8;
        for i in (0..bytes.len()).rev() {
            let code = bytes[i];
            if code == b'/' {
                if !matched_slash {
                    start_part = i + 1;
                    break;
                }
                continue;
            }
            if end.is_none() {
                matched_slash = false;
                end = Some(i + 1);
            }
            if code == b'.' {
                if start_dot.is_none() {
                    start_dot = Some(i);
                } else if pre_dot_state != 1 {
                    pre_dot_state = 1;
                }
            } else if start_dot.is_some() {
                pre_dot_state = -1;
            }
        }
        match (start_dot, end) {
            (Some(dot), Some(end))
                if pre_dot_state != 0 && !(pre_dot_state == 1 && dot + 1 == end && dot == start_part + 1) =>
            {
                path.get(dot..end).unwrap_or("").to_owned()
            }
            _ => String::new(),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Date

/// `Date.parse` and `Date.prototype.toISOString` for the timestamps deckhand writes and reads: ISO 8601 as
/// `toISOString` writes it, the date-only and offset forms (`git log --format=%cI`), and a local date-time without an
/// offset. Times are milliseconds since the epoch (`Date.now()` / `getTime()`).
pub mod date {
    use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};

    /// `Date.now()`.
    pub fn now_ms() -> i64 {
        Utc::now().timestamp_millis()
    }

    /// `new Date(ms).toISOString()` (`YYYY-MM-DDTHH:mm:ss.sssZ`; `±YYYYYY` years outside 0..=9999). `None` where JS
    /// throws (an invalid date).
    pub fn to_iso_string(ms: i64) -> Option<String> {
        let time = DateTime::<Utc>::from_timestamp_millis(ms)?;
        let year = chrono::Datelike::year(&time);
        let rest = time.format("-%m-%dT%H:%M:%S%.3fZ");
        Some(if (0..=9999).contains(&year) {
            format!("{year:04}{rest}")
        } else if year < 0 {
            format!("-{:06}{rest}", -i64::from(year))
        } else {
            format!("+{year:06}{rest}")
        })
    }

    /// `Date.parse(text)`: milliseconds, or `None` for NaN.
    pub fn parse(text: &str) -> Option<i64> {
        let text = text.trim();
        let (date_part, time_part) = match text.find(['T', 't', ' ']) {
            Some(at) => (&text[..at], Some(&text[at + 1..])),
            None => (text, None),
        };
        let date = parse_date(date_part)?;
        let Some(time_part) = time_part else {
            // Date-only forms are UTC.
            return Some(Utc.from_utc_datetime(&date.and_time(NaiveTime::MIN)).timestamp_millis());
        };
        let (clock, offset) = split_offset(time_part)?;
        let time = parse_time(clock)?;
        let naive = NaiveDateTime::new(date, time);
        match offset {
            Some(minutes) => Some(Utc.from_utc_datetime(&naive).timestamp_millis() - i64::from(minutes) * 60_000),
            // A date-time without an offset is local time.
            None => Local
                .from_local_datetime(&naive)
                .earliest()
                .map(|local| local.timestamp_millis()),
        }
    }

    fn digits(text: &str, count: usize) -> Option<u32> {
        if text.len() == count && text.bytes().all(|byte| byte.is_ascii_digit()) {
            text.parse().ok()
        } else {
            None
        }
    }

    fn parse_date(text: &str) -> Option<NaiveDate> {
        let (sign, body) = match text.as_bytes().first() {
            Some(b'+') => (1, &text[1..]),
            Some(b'-') => (-1, &text[1..]),
            _ => (0, text),
        };
        let mut parts = body.split('-');
        let year_text = parts.next()?;
        let year = if sign == 0 {
            i32::try_from(digits(year_text, 4)?).ok()?
        } else {
            let value = i32::try_from(digits(year_text, 6)?).ok()?;
            if sign < 0 && value == 0 {
                return None;
            }
            value * sign
        };
        let month = match parts.next() {
            Some(month) => digits(month, 2)?,
            None => 1,
        };
        let day = match parts.next() {
            Some(day) => digits(day, 2)?,
            None => 1,
        };
        if parts.next().is_some() {
            return None;
        }
        NaiveDate::from_ymd_opt(year, month, day)
    }

    /// The clock part and its offset in minutes (`Z` → 0, none → `None`).
    fn split_offset(text: &str) -> Option<(&str, Option<i32>)> {
        if let Some(clock) = text.strip_suffix(['Z', 'z']) {
            return Some((clock, Some(0)));
        }
        if let Some(at) = text.rfind(['+', '-']) {
            let (clock, zone) = (&text[..at], &text[at..]);
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let zone = &zone[1..];
            let (hours, minutes) = match zone.split_once(':') {
                Some((hours, minutes)) => (digits(hours, 2)?, digits(minutes, 2)?),
                None if zone.len() == 4 && zone.is_ascii() => (digits(&zone[..2], 2)?, digits(&zone[2..], 2)?),
                None => return None,
            };
            if hours > 23 || minutes > 59 {
                return None;
            }
            let total = i32::try_from(hours * 60 + minutes).ok()?;
            return Some((clock, Some(sign * total)));
        }
        Some((text, None))
    }

    fn parse_time(text: &str) -> Option<NaiveTime> {
        let (clock, fraction) = match text.split_once('.') {
            Some((clock, fraction)) => (clock, Some(fraction)),
            None => (text, None),
        };
        let mut parts = clock.split(':');
        let hours = digits(parts.next()?, 2)?;
        let minutes = digits(parts.next()?, 2)?;
        let seconds = match parts.next() {
            Some(seconds) => digits(seconds, 2)?,
            None if fraction.is_none() => 0,
            None => return None,
        };
        if parts.next().is_some() {
            return None;
        }
        let millis = match fraction {
            // V8 reads any number of fraction digits and keeps milliseconds.
            Some(fraction) if !fraction.is_empty() && fraction.bytes().all(|byte| byte.is_ascii_digit()) => {
                let padded = format!("{fraction:0<3}");
                padded[..3].parse::<u32>().ok()?
            }
            Some(_) => return None,
            None => 0,
        };
        if hours == 24 && minutes == 0 && seconds == 0 && millis == 0 {
            // 24:00 is the end of the day: chrono has no such time, so callers get the next midnight's value.
            return NaiveTime::from_hms_milli_opt(23, 59, 59, 999)
                .map(|time| time + chrono::TimeDelta::milliseconds(1));
        }
        NaiveTime::from_hms_milli_opt(hours, minutes, seconds, millis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_parse_non_ascii_offset_is_nan_not_panic() {
        assert!(date::parse("2026-01-01T10:00+1\u{e9}1").is_none());
    }

    #[test]
    fn lengths_count_utf16_units() {
        assert_eq!(js_len(""), 0);
        assert_eq!(js_len("abc"), 3);
        assert_eq!(js_len("é"), 1);
        assert_eq!(js_len("e\u{301}"), 2);
        assert_eq!(js_len("😀"), 2);
        assert_eq!(js_len("日本"), 2);
        assert_eq!(js_len("👍🏽"), 4);
        assert_eq!(js_len("👨‍👩‍👧"), 8);
        assert_eq!(js_len("𠀀"), 2);
    }

    #[test]
    fn offsets_convert_between_utf16_and_bytes() {
        let text = "a😀日é";
        assert_eq!(utf16_to_byte(text, 0), 0);
        assert_eq!(utf16_to_byte(text, 1), 1);
        // Inside the surrogate pair: rounds down to the pair's start.
        assert_eq!(utf16_to_byte(text, 2), 1);
        assert_eq!(utf16_to_byte(text, 3), 5);
        assert_eq!(utf16_to_byte(text, 4), 8);
        assert_eq!(utf16_to_byte(text, 5), 10);
        assert_eq!(utf16_to_byte(text, 99), 10);
        assert_eq!(byte_to_utf16(text, 0), 0);
        assert_eq!(byte_to_utf16(text, 1), 1);
        assert_eq!(byte_to_utf16(text, 3), 1);
        assert_eq!(byte_to_utf16(text, 5), 3);
        assert_eq!(byte_to_utf16(text, 8), 4);
        assert_eq!(byte_to_utf16(text, 10), 5);
        assert_eq!(byte_to_utf16(text, 99), 5);
        for index in 0..=5 {
            let byte = utf16_to_byte(text, index);
            assert!(text.is_char_boundary(byte));
            assert!(byte_to_utf16(text, byte) <= index);
        }
    }

    #[test]
    fn code_units_and_chars() {
        assert_eq!(js_code_unit_at("a😀", 0), Some(0x61));
        assert_eq!(js_code_unit_at("a😀", 1), Some(0xD83D));
        assert_eq!(js_code_unit_at("a😀", 2), Some(0xDE00));
        assert_eq!(js_code_unit_at("a😀", 3), None);
        assert_eq!(js_char_at("a😀", 1).as_deref(), Some("\u{FFFD}"));
        assert_eq!(js_char_at("日", 0).as_deref(), Some("日"));
    }

    #[test]
    fn slice_takes_utf16_and_negative_indices() {
        assert_eq!(js_slice("hello", 1, Some(3)), "el");
        assert_eq!(js_slice("hello", -3, None), "llo");
        assert_eq!(js_slice("hello", 0, Some(-1)), "hell");
        assert_eq!(js_slice("hello", 3, Some(1)), "");
        assert_eq!(js_slice("hello", -99, Some(99)), "hello");
        assert_eq!(js_slice("日本語", 1, None), "本語");
        assert_eq!(js_slice("a😀b", 0, Some(3)), "a😀");
        // Cutting a pair leaves U+FFFD for the half (one unit, like the lone surrogate).
        assert_eq!(js_slice("a😀b", 0, Some(2)), "a\u{FFFD}");
        assert_eq!(js_slice("a😀b", 2, None), "\u{FFFD}b");
        assert_eq!(js_len(&js_slice("a😀b", 0, Some(2))), 2);
        assert_eq!(js_slice("👍🏽!", -1, None), "!");
        assert_eq!(js_slice("e\u{301}x", 1, None), "\u{301}x");
    }

    #[test]
    fn substring_clamps_and_swaps() {
        assert_eq!(js_substring("hello", 3, Some(1)), "el");
        assert_eq!(js_substring("hello", -2, Some(2)), "he");
        assert_eq!(js_substring("hello", 2, None), "llo");
        assert_eq!(js_substring("😀😁", 2, None), "😁");
        assert_eq!(js_substring("😀😁", 1, Some(3)), "\u{FFFD}\u{FFFD}");
        assert_eq!(js_substring("中文字", 0, Some(2)), "中文");
    }

    #[test]
    fn index_of_returns_utf16_positions() {
        assert_eq!(js_index_of("a😀b😀", "😀", 0), Some(1));
        assert_eq!(js_index_of("a😀b😀", "😀", 2), Some(4));
        assert_eq!(js_index_of("a😀b😀", "x", 0), None);
        assert_eq!(js_index_of("日本日本", "本", 2), Some(3));
        assert_eq!(js_last_index_of("a😀b😀", "😀"), Some(4));
        assert_eq!(js_last_index_of("é/é", "/"), Some(1));
    }

    #[test]
    fn padding_counts_utf16() {
        assert_eq!(js_pad_start("7", 3, " "), "  7");
        assert_eq!(js_pad_start("123", 2, " "), "123");
        assert_eq!(js_pad_end("ab", 5, "xy"), "abxyx");
        assert_eq!(js_pad_end("😀", 3, "."), "😀.");
        assert_eq!(js_pad_start("日", 3, "😀"), "😀日");
        assert_eq!(js_pad_end("x", 4, ""), "x");
    }

    #[test]
    fn case_mapping_matches_js() {
        assert_eq!(js_to_lower("ÀÉÎ ΑΣ"), "àéî ας");
        assert_eq!(js_to_upper("straße"), "STRASSE");
        assert_eq!(js_to_lower("İ"), "i\u{307}");
        assert_eq!(js_to_upper("ǆ"), "Ǆ");
        assert_eq!(js_to_lower("😀日本ABC"), "😀日本abc");
        assert_eq!(js_to_upper("ﬁ"), "FI");
    }

    #[test]
    fn trim_uses_the_js_whitespace_set() {
        assert_eq!(js_trim("\u{FEFF}\u{A0} a b \u{3000}\n"), "a b");
        assert_eq!(js_trim("\u{85}a\u{85}"), "\u{85}a\u{85}");
        assert_eq!(js_trim_start("  😀 "), "😀 ");
        assert_eq!(js_trim_end("  日\u{2028}"), "  日");
        assert_eq!(js_split_whitespace("  a\tb\u{3000}c\n "), vec!["a", "b", "c"]);
        assert!(js_split_whitespace(" \u{FEFF} ").is_empty());
        assert_eq!(js_collapse_whitespace("a \t b\u{A0}\u{A0}😀"), "a b 😀");
    }

    #[test]
    fn json_strings_escape_like_json_stringify() {
        assert_eq!(json_stringify_str("a\"b\\c\n\u{1}"), "\"a\\\"b\\\\c\\n\\u0001\"");
        assert_eq!(json_stringify_str("😀\u{2028}\u{7f}"), "\"😀\u{2028}\u{7f}\"");
    }

    #[test]
    fn locale_compare_basics() {
        use Ordering::*;
        assert_eq!(locale_compare("a", "b"), Less);
        assert_eq!(locale_compare("a", "A"), Less);
        assert_eq!(locale_compare("A", "b"), Less);
        assert_eq!(locale_compare("é", "f"), Less);
        assert_eq!(locale_compare("e\u{301}", "é"), Equal);
        assert_eq!(locale_compare("item 10", "item 9"), Less);
        assert_eq!(locale_compare("_", "a"), Less);
        assert_eq!(locale_compare("😀", "a"), Less);
        assert_eq!(locale_compare("日", "a"), Greater);
        assert_eq!(locale_compare("a\u{1}", "a"), Equal);
        assert_eq!(locale_compare_sign("b", "a"), 1);
        assert_eq!(locale_compare("한", "가"), Greater);
    }

    #[derive(serde::Deserialize)]
    struct LocaleFixture {
        strings: Vec<String>,
        sorted: Vec<String>,
        pairs: Vec<(usize, usize, i32)>,
    }

    #[test]
    fn locale_compare_replays_node() {
        let fixture: LocaleFixture =
            serde_json::from_str(include_str!("../tests/data/locale_compare.json")).expect("fixture parses");
        let mut failures = Vec::new();
        for (i, j, sign) in &fixture.pairs {
            let (a, b) = (&fixture.strings[*i], &fixture.strings[*j]);
            let got = locale_compare_sign(a, b);
            if got != *sign {
                failures.push(format!("{a:?} vs {b:?}: node {sign}, rust {got}"));
            }
        }
        assert!(
            failures.is_empty(),
            "{} mismatches:\n{}",
            failures.len(),
            failures.join("\n")
        );
        let mut sorted = fixture.strings.clone();
        sorted.sort_by(|a, b| locale_compare(a, b));
        assert_eq!(sorted, fixture.sorted);
    }

    #[test]
    fn known_divergences() {
        // Han beyond the BMP: ICU puts 𠀁 (radical 1) before 三; the table stops at the BMP, so it sorts after all Han.
        assert_eq!(locale_compare("𠀁", "三"), Ordering::Greater);
        // Still a total order among themselves, by code point.
        assert_eq!(locale_compare("𠀀", "𠀁"), Ordering::Less);
    }

    #[test]
    fn path_matches_node() {
        use path::*;
        assert_eq!(normalize(""), ".");
        assert_eq!(normalize("/a//b/../c/./d/"), "/a/c/d/");
        assert_eq!(normalize("a/../../b"), "../b");
        assert_eq!(normalize("./"), "./");
        assert_eq!(normalize("/.."), "/");
        assert_eq!(normalize("日本/./語/"), "日本/語/");
        assert_eq!(join(&["/a", "b", "../c", ""]), "/a/c");
        assert_eq!(join(&["", ""]), ".");
        assert_eq!(join(&["😀", "x"]), "😀/x");
        assert_eq!(resolve_all(&["/a/b", "../c", "/d", "e"]), "/d/e");
        assert_eq!(resolve("/a/b/"), "/a/b");
        assert_eq!(resolve("/"), "/");
        assert!(resolve("x").ends_with("/x"));
        assert_eq!(relative("/a/b/c", "/a/d"), "../../d");
        assert_eq!(relative("/a/b", "/a/b/c/d"), "c/d");
        assert_eq!(relative("/a/b", "/a/b"), "");
        assert_eq!(relative("/a/bc", "/a/b"), "../b");
        assert_eq!(relative("/", "/a"), "a");
        assert_eq!(relative("/a", "/"), "..");
        assert_eq!(relative("/日本/語", "/日本/😀"), "../😀");
        assert_eq!(dirname("/a/b/"), "/a");
        assert_eq!(dirname("/a"), "/");
        assert_eq!(dirname("a"), ".");
        assert_eq!(dirname("//a"), "//");
        assert_eq!(dirname(""), ".");
        assert_eq!(dirname("日本/語"), "日本");
        assert_eq!(basename("/a/b/"), "b");
        assert_eq!(basename("/"), "");
        assert_eq!(basename("😀.txt"), "😀.txt");
        assert_eq!(basename_without("/a/b.txt", ".txt"), "b");
        assert_eq!(basename_without("/a/b.txt", "b.txt"), "b.txt");
        assert_eq!(basename_without("/a/.txt", ".txt"), ".txt");
        assert_eq!(basename_without("日本.md", ".md"), "日本");
        assert_eq!(extname("a/b.c.d"), ".d");
        assert_eq!(extname(".gitignore"), "");
        assert_eq!(extname("a."), ".");
        assert_eq!(extname("a/b"), "");
        assert_eq!(extname("..."), ".");
        assert_eq!(extname("文書.日本"), ".日本");
        assert!(is_absolute("/x") && !is_absolute("x"));
    }

    #[test]
    fn dates_parse_and_print_like_js() {
        use date::*;
        assert_eq!(parse("2026-10-09T12:34:56.789Z"), Some(1_791_549_296_789));
        assert_eq!(parse("2026-10-09"), Some(1_791_504_000_000));
        assert_eq!(parse("2026-10-09T14:34:56+02:00"), Some(1_791_549_296_000));
        assert_eq!(parse("2026-10-09T12:34:56.7891Z"), Some(1_791_549_296_789));
        assert_eq!(parse("2026-10"), Some(1_790_812_800_000));
        assert_eq!(parse(""), None);
        assert_eq!(parse("not a date"), None);
        assert_eq!(parse("2026-13-01"), None);
        assert_eq!(
            to_iso_string(1_791_549_296_789).as_deref(),
            Some("2026-10-09T12:34:56.789Z")
        );
        assert_eq!(to_iso_string(0).as_deref(), Some("1970-01-01T00:00:00.000Z"));
        assert_eq!(to_iso_string(-1).as_deref(), Some("1969-12-31T23:59:59.999Z"));
        let now = now_ms();
        assert_eq!(parse(&to_iso_string(now).unwrap_or_default()), Some(now));
    }
}
