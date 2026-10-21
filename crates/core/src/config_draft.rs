//! Port of configDraft.ts.
//!
//! Pure helpers for the UI's config editor. Keep fs/Git code out of this module so UI code can use it without
//! pulling in persistence code.
//!
//! `formatConfigJson` is `JSON.stringify(JSON.parse(text), null, 2)`, so it reproduces what that round trip does to
//! a document rather than what serde_json would: numbers become JS doubles (`1e400` -> `null`, `-0` -> `0`, JS
//! number formatting), duplicate keys keep their first position with the last value, integer-like keys move to the
//! front in ascending order (V8's element ordering), and lone surrogates in strings survive as `\udxxx` escapes.
//! Parsing and printing are iterative over a flat arena, so deep nesting cannot overflow the stack.
//!
//! The one limit is nesting depth ([`MAX_STRINGIFY_DEPTH`]): `JSON.parse` accepts any depth, but V8's
//! `JSON.stringify` recurses per level and throws `RangeError: Maximum call stack size exceeded` past a few thousand
//! levels (the exact depth depends on the stack in use; about 6170 under Node 24 with the default stack). Printing
//! is also quadratic in depth (each level is indented), so an unbounded depth would mean gigabytes of output.

use indexmap::IndexMap;

use crate::error::{DhError, Result, json_parse_message};

/// TS: configDraft.ts MAX_CONFIG_BYTES.
pub const MAX_CONFIG_BYTES: usize = 64 * 1024;

/// TS: configDraft.ts MAX_CONFIG_LABEL.
pub const MAX_CONFIG_LABEL: &str = "64 KiB";

/// The deepest value (the root is at depth 1) [`format_config_json`] prints; deeper input fails as V8's recursive
/// `JSON.stringify` does (see the module docs; V8's own limit depends on its stack, this one is fixed).
pub const MAX_STRINGIFY_DEPTH: usize = 6144;

/// V8's RangeError message for a stack overflow.
const STACK_OVERFLOW_MESSAGE: &str = "Maximum call stack size exceeded";

/// TS: configDraft.ts formatConfigJson (`JSON.stringify(JSON.parse(text), null, 2)` plus a newline; the parse error
/// is JSON.parse's message, see `DhError::json_parse`).
///
/// Formatting only needs valid JSON; saving performs the full settings validation.
pub fn format_config_json(text: &str) -> Result<String> {
    if let Some(message) = json_parse_message(text) {
        return Err(DhError::msg(message));
    }
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut parser = Parser {
        units: &units,
        pos: 0,
        nodes: Vec::new(),
        too_deep: false,
    };
    let root = parser.document().ok_or_else(|| {
        if parser.too_deep {
            DhError::msg(STACK_OVERFLOW_MESSAGE)
        } else {
            DhError::json_parse(text)
        }
    })?;
    let mut out = String::new();
    stringify(&parser.nodes, root, &mut out);
    out.push('\n');
    Ok(out)
}

/// A parsed JSON value; children are indices into the arena.
enum Node {
    Null,
    Bool(bool),
    Number(f64),
    String(Vec<u16>),
    Array(Vec<usize>),
    Object(IndexMap<Vec<u16>, usize>),
}

/// An open container while parsing.
enum Frame {
    Array(Vec<usize>),
    Object(IndexMap<Vec<u16>, usize>, Vec<u16>),
}

struct Parser<'a> {
    units: &'a [u16],
    pos: usize,
    nodes: Vec<Node>,
    /// Set when [`Parser::document`] stopped at a value deeper than [`MAX_STRINGIFY_DEPTH`].
    too_deep: bool,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u16> {
        self.units.get(self.pos).copied()
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(0x20 | 0x09 | 0x0a | 0x0d)) {
            self.pos += 1;
        }
    }

    fn eat(&mut self, unit: u8) -> bool {
        self.skip_whitespace();
        if self.peek() == Some(u16::from(unit)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn push(&mut self, node: Node) -> usize {
        self.nodes.push(node);
        self.nodes.len() - 1
    }

    fn literal(&mut self, word: &str, node: Node) -> Option<usize> {
        let end = self.pos + word.len();
        let matches = self
            .units
            .get(self.pos..end)
            .is_some_and(|units| units.iter().copied().eq(word.encode_utf16()));
        if !matches {
            return None;
        }
        self.pos = end;
        Some(self.push(node))
    }

    /// The whole text: one value and nothing after it but whitespace.
    fn document(&mut self) -> Option<usize> {
        let mut stack: Vec<Frame> = Vec::new();
        loop {
            // A value starts here (or a container opens).
            if stack.len() >= MAX_STRINGIFY_DEPTH {
                self.too_deep = true;
                return None;
            }
            self.skip_whitespace();
            let mut value = match self.peek()? {
                0x7b => {
                    self.pos += 1;
                    if self.eat(b'}') {
                        self.push(Node::Object(IndexMap::new()))
                    } else {
                        let key = self.key()?;
                        stack.push(Frame::Object(IndexMap::new(), key));
                        continue;
                    }
                }
                0x5b => {
                    self.pos += 1;
                    if self.eat(b']') {
                        self.push(Node::Array(Vec::new()))
                    } else {
                        stack.push(Frame::Array(Vec::new()));
                        continue;
                    }
                }
                0x22 => {
                    let string = self.string()?;
                    self.push(Node::String(string))
                }
                0x74 => self.literal("true", Node::Bool(true))?,
                0x66 => self.literal("false", Node::Bool(false))?,
                0x6e => self.literal("null", Node::Null)?,
                _ => {
                    let number = self.number()?;
                    self.push(Node::Number(number))
                }
            };
            // Attach the value, closing every container it completes.
            loop {
                match stack.last_mut() {
                    None => {
                        self.skip_whitespace();
                        return (self.pos == self.units.len()).then_some(value);
                    }
                    Some(Frame::Array(items)) => {
                        items.push(value);
                        if self.eat(b',') {
                            break;
                        }
                        if !self.eat(b']') {
                            return None;
                        }
                        let Some(Frame::Array(items)) = stack.pop() else {
                            return None;
                        };
                        value = self.push(Node::Array(items));
                    }
                    Some(Frame::Object(entries, key)) => {
                        // A repeated key keeps its first position and takes the last value.
                        entries.insert(std::mem::take(key), value);
                        if self.eat(b',') {
                            let next = self.key()?;
                            if let Some(Frame::Object(_, key)) = stack.last_mut() {
                                *key = next;
                            }
                            break;
                        }
                        if !self.eat(b'}') {
                            return None;
                        }
                        let Some(Frame::Object(entries, _)) = stack.pop() else {
                            return None;
                        };
                        value = self.push(Node::Object(entries));
                    }
                }
            }
        }
    }

    /// `"key" :`
    fn key(&mut self) -> Option<Vec<u16>> {
        self.skip_whitespace();
        let key = self.string()?;
        self.eat(b':').then_some(key)
    }

    /// A string literal at `pos`, as UTF-16 (escapes may produce lone surrogates).
    fn string(&mut self) -> Option<Vec<u16>> {
        if self.peek()? != 0x22 {
            return None;
        }
        self.pos += 1;
        let mut out = Vec::new();
        loop {
            let unit = self.peek()?;
            self.pos += 1;
            match unit {
                0x22 => return Some(out),
                0x5c => {
                    let escape = self.peek()?;
                    self.pos += 1;
                    let decoded = match escape {
                        0x22 | 0x5c | 0x2f => escape,
                        0x62 => 0x08,
                        0x66 => 0x0c,
                        0x6e => 0x0a,
                        0x72 => 0x0d,
                        0x74 => 0x09,
                        0x75 => {
                            let digits = self.units.get(self.pos..self.pos + 4)?;
                            let digits = String::from_utf16(digits).ok()?;
                            self.pos += 4;
                            u16::from_str_radix(&digits, 16).ok()?
                        }
                        _ => return None,
                    };
                    out.push(decoded);
                }
                0x00..=0x1f => return None,
                _ => out.push(unit),
            }
        }
    }

    /// `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?` as a double (overflow to ±Infinity, as JS).
    fn number(&mut self) -> Option<f64> {
        let start = self.pos;
        let digits = |parser: &mut Self| {
            let from = parser.pos;
            while matches!(parser.peek(), Some(0x30..=0x39)) {
                parser.pos += 1;
            }
            parser.pos > from
        };
        if self.peek() == Some(0x2d) {
            self.pos += 1;
        }
        if self.peek() == Some(0x30) {
            self.pos += 1;
        } else if !digits(self) {
            return None;
        }
        if self.peek() == Some(0x2e) {
            self.pos += 1;
            if !digits(self) {
                return None;
            }
        }
        if matches!(self.peek(), Some(0x65 | 0x45)) {
            self.pos += 1;
            if matches!(self.peek(), Some(0x2b | 0x2d)) {
                self.pos += 1;
            }
            if !digits(self) {
                return None;
            }
        }
        String::from_utf16(&self.units[start..self.pos]).ok()?.parse().ok()
    }
}

/// An array index key (`0` .. `2^32 - 2` in canonical form): V8 keeps these first, ascending.
fn array_index(key: &[u16]) -> Option<u32> {
    let text = String::from_utf16(key).ok()?;
    if text.is_empty() || (text.len() > 1 && text.starts_with('0')) || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse::<u32>().ok().filter(|index| *index != u32::MAX)
}

/// The keys of an object in JS property order.
fn ordered_entries(entries: &IndexMap<Vec<u16>, usize>) -> Vec<(&[u16], usize)> {
    let mut indexed: Vec<(u32, &[u16], usize)> = Vec::new();
    let mut named: Vec<(&[u16], usize)> = Vec::new();
    for (key, value) in entries {
        match array_index(key) {
            Some(index) => indexed.push((index, key, *value)),
            None => named.push((key, *value)),
        }
    }
    indexed.sort_by_key(|(index, _, _)| *index);
    indexed
        .into_iter()
        .map(|(_, key, value)| (key, value))
        .chain(named)
        .collect()
}

/// `JSON.stringify` of a string given as UTF-16 (lone surrogates escaped, as well-formed JSON.stringify does).
fn quote(units: &[u16], out: &mut String) {
    out.push('"');
    for decoded in char::decode_utf16(units.iter().copied()) {
        match decoded {
            Ok('"') => out.push_str("\\\""),
            Ok('\\') => out.push_str("\\\\"),
            Ok('\u{08}') => out.push_str("\\b"),
            Ok('\u{0c}') => out.push_str("\\f"),
            Ok('\n') => out.push_str("\\n"),
            Ok('\r') => out.push_str("\\r"),
            Ok('\t') => out.push_str("\\t"),
            Ok(character) if u32::from(character) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", u32::from(character)));
            }
            Ok(character) => out.push(character),
            Err(error) => out.push_str(&format!("\\u{:04x}", error.unpaired_surrogate())),
        }
    }
    out.push('"');
}

/// `JSON.stringify` of a number: JS `Number.prototype.toString`, `null` for NaN/±Infinity.
fn js_number(value: f64) -> String {
    if !value.is_finite() {
        return String::from("null");
    }
    if value == 0.0 {
        return String::from("0");
    }
    // Shortest round-trip digits and the decimal exponent, from Rust's `{:e}` (same shortest digits as JS).
    let formatted = format!("{:e}", value.abs());
    let (mantissa, exponent) = formatted.split_once('e').unwrap_or((&formatted, "0"));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let k = i64::try_from(digits.len()).unwrap_or(i64::MAX);
    let n = exponent.parse::<i64>().unwrap_or(0) + 1;
    let sign = if value < 0.0 { "-" } else { "" };
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat(usize::try_from(n - k).unwrap_or(0)))
    } else if 0 < n && n <= 21 {
        let (int, frac) = digits.split_at(usize::try_from(n).unwrap_or(0));
        format!("{int}.{frac}")
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat(usize::try_from(-n).unwrap_or(0)))
    } else {
        let e = n - 1;
        let e_sign = if e < 0 { '-' } else { '+' };
        let (first, rest) = digits.split_at(1);
        let rest = if rest.is_empty() {
            String::new()
        } else {
            format!(".{rest}")
        };
        format!("{first}{rest}e{e_sign}{}", e.abs())
    };
    format!("{sign}{body}")
}

/// Printing work: a value to write at a depth, or text (closing brackets, separators) to emit.
enum Step<'a> {
    Value(usize, usize),
    Text(String),
    Key(&'a [u16]),
}

/// `JSON.stringify(value, null, 2)`.
fn stringify(nodes: &[Node], root: usize, out: &mut String) {
    let indent = |depth: usize| format!("\n{}", "  ".repeat(depth));
    let mut steps = vec![Step::Value(root, 0)];
    while let Some(step) = steps.pop() {
        let (id, depth) = match step {
            Step::Text(text) => {
                out.push_str(&text);
                continue;
            }
            Step::Key(key) => {
                quote(key, out);
                out.push_str(": ");
                continue;
            }
            Step::Value(id, depth) => (id, depth),
        };
        let Some(node) = nodes.get(id) else {
            out.push_str("null");
            continue;
        };
        match node {
            Node::Null => out.push_str("null"),
            Node::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Node::Number(value) => out.push_str(&js_number(*value)),
            Node::String(units) => quote(units, out),
            Node::Array(items) if items.is_empty() => out.push_str("[]"),
            Node::Object(entries) if entries.is_empty() => out.push_str("{}"),
            Node::Array(items) => {
                out.push('[');
                steps.push(Step::Text(format!("{}]", indent(depth))));
                for (index, item) in items.iter().enumerate().rev() {
                    steps.push(Step::Value(*item, depth + 1));
                    let separator = if index == 0 { "" } else { "," };
                    steps.push(Step::Text(format!("{separator}{}", indent(depth + 1))));
                }
            }
            Node::Object(entries) => {
                out.push('{');
                steps.push(Step::Text(format!("{}}}", indent(depth))));
                for (index, (key, value)) in ordered_entries(entries).into_iter().enumerate().rev() {
                    steps.push(Step::Value(value, depth + 1));
                    steps.push(Step::Key(key));
                    let separator = if index == 0 { "" } else { "," };
                    steps.push(Step::Text(format!("{separator}{}", indent(depth + 1))));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use deckhand_testkit::{Case, replay};

    use super::*;

    #[test]
    fn formats_with_js_semantics() {
        let ordered = format_config_json(r#"{"b":1,"a":2,"1":3,"0":4,"b":5}"#).unwrap();
        assert_eq!(ordered, "{\n  \"0\": 4,\n  \"1\": 3,\n  \"b\": 5,\n  \"a\": 2\n}\n");
        let numbers = format_config_json("[1e21, 1e-7, 1e400, -0, 123456789.123456789]").unwrap();
        assert_eq!(
            numbers,
            "[\n  1e+21,\n  1e-7,\n  null,\n  0,\n  123456789.12345679\n]\n"
        );
        assert_eq!(format_config_json(r#""\ud800""#).unwrap(), "\"\\ud800\"\n");
        assert_eq!(
            format_config_json("").unwrap_err().to_string(),
            "Unexpected end of JSON input"
        );
    }

    #[test]
    fn deep_nesting_does_not_overflow() {
        let nested = |depth: usize| format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        assert!(format_config_json(&nested(5000)).is_ok());
        assert!(format_config_json(&nested(MAX_STRINGIFY_DEPTH)).is_ok());
        // Valid JSON (JSON.parse accepts any depth) that JSON.stringify cannot print: V8's RangeError, not a crash.
        for depth in [MAX_STRINGIFY_DEPTH + 1, 100_000, 1_000_000] {
            assert_eq!(
                format_config_json(&nested(depth)).unwrap_err().to_string(),
                "Maximum call stack size exceeded"
            );
        }
        let objects = |depth: usize| format!("{}1{}", "{\"a\":".repeat(depth), "}".repeat(depth));
        assert!(format_config_json(&objects(MAX_STRINGIFY_DEPTH - 1)).is_ok());
        assert!(format_config_json(&objects(MAX_STRINGIFY_DEPTH)).is_err());
        // Bad JSON deep inside still gets JSON.parse's message.
        assert_eq!(
            format_config_json(&format!("{}x", "[".repeat(100_000)))
                .unwrap_err()
                .to_string(),
            "Unexpected token 'x', ...\"[[[[[[[[[[x\" is not valid JSON"
        );
    }

    #[test]
    fn format_config_json_parity() {
        replay(
            "configDraft/formatConfigJson",
            |case: &Case| -> Result<String, String> {
                format_config_json(case.arg_str(0)?).map_err(|error| error.to_string())
            },
        );
    }
}
