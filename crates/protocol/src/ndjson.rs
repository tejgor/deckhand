//! Port of client.ts / daemon.ts (newline-delimited JSON framing).
//!
//! Every message is one `JSON.stringify` line ending in `\n` ([`encode`]). Readers decode the byte stream as UTF-8
//! incrementally (Node's StringDecoder: a sequence split across chunks is joined, invalid bytes become U+FFFD), split
//! on `\n`, trim each line (JS `trim`) and skip empty ones ([`LineDecoder`]). The daemon also bounds a request line at
//! [`MAX_REQUEST_LINE`] UTF-16 units: an oversized line is skipped and reported once with an excerpt of its head and
//! tail, from which the daemon answers the request ID it can find ([`oversized_request_id`]). What a reader does with
//! a line that is not JSON is its own policy (the daemon closes the connection, the client fails), so
//! [`parse_line`] only reports it.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// daemon.ts MAX_REQUEST_LINE, in UTF-16 code units (JS string length).
pub const MAX_REQUEST_LINE: usize = 256_000;
/// How much of an oversized line's head and tail the daemon keeps (UTF-16 units).
pub const OVERSIZED_EXCERPT: usize = 4096;

#[derive(Debug, thiserror::Error)]
pub enum NdjsonError {
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// daemon.ts: a line that parses but is not an object with a string `type`.
    #[error("invalid request")]
    NotAMessage,
}

/// One message as a line: `` `${JSON.stringify(message)}\n` ``.
pub fn encode<T: Serialize + ?Sized>(message: &T) -> serde_json::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(message)?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// [`encode`] as a string.
pub fn encode_line<T: Serialize + ?Sized>(message: &T) -> serde_json::Result<String> {
    let mut line = serde_json::to_string(message)?;
    line.push('\n');
    Ok(line)
}

/// Parses a (trimmed, non-empty) line.
pub fn parse_line<T: DeserializeOwned>(line: &str) -> Result<T, NdjsonError> {
    Ok(serde_json::from_str(line)?)
}

/// daemon.ts's acceptance test for a request line: JSON, an object, with a string `type`.
pub fn parse_message(line: &str) -> Result<Value, NdjsonError> {
    let value: Value = serde_json::from_str(line)?;
    match value.as_object().and_then(|object| object.get("type")) {
        Some(Value::String(_)) => Ok(value),
        _ => Err(NdjsonError::NotAMessage),
    }
}

/// What a [`LineDecoder`] yields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    /// A complete, trimmed, non-empty line.
    Line(String),
    /// A line over the limit was dropped: its first and last [`OVERSIZED_EXCERPT`] units, joined by `\n`.
    Oversized { excerpt: String },
}

/// JS `String.prototype.trim`'s whitespace (WhiteSpace and LineTerminator), which differs from Rust's `trim` in
/// U+0085 (not trimmed by JS) and U+FEFF (trimmed by JS).
fn is_js_whitespace(ch: char) -> bool {
    matches!(
        ch,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    )
}

/// JS `trim()`.
pub fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_whitespace)
}

/// JS string length (UTF-16 units).
fn js_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The first `units` UTF-16 units (never splitting a character).
fn js_head(text: &str, units: usize) -> &str {
    let mut used = 0;
    for (index, ch) in text.char_indices() {
        used += ch.len_utf16();
        if used > units {
            return &text[..index];
        }
    }
    text
}

/// The last `units` UTF-16 units, JS `slice(-units)` (never splitting a character).
fn js_tail(text: &str, units: usize) -> &str {
    let mut used = 0;
    for (index, ch) in text.char_indices().rev() {
        used += ch.len_utf16();
        if used > units {
            return &text[index + ch.len_utf8()..];
        }
    }
    text
}

/// Incremental UTF-8 decoding with Node StringDecoder semantics.
#[derive(Debug, Default)]
struct Utf8Stream {
    pending: Vec<u8>,
}

impl Utf8Stream {
    fn write(&mut self, chunk: &[u8], out: &mut String) {
        self.pending.extend_from_slice(chunk);
        let mut rest: &[u8] = &self.pending;
        let mut consumed = 0;
        loop {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    out.push_str(text);
                    consumed += rest.len();
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    // Safe: from_utf8 vouched for the first `valid` bytes.
                    out.push_str(std::str::from_utf8(&rest[..valid]).unwrap_or_default());
                    match error.error_len() {
                        Some(bad) => {
                            out.push('\u{FFFD}');
                            consumed += valid + bad;
                            rest = &rest[valid + bad..];
                        }
                        // An incomplete sequence at the end: wait for the next chunk.
                        None => {
                            consumed += valid;
                            break;
                        }
                    }
                }
            }
        }
        self.pending.drain(..consumed);
    }
}

/// Splits a byte stream into NDJSON lines (see the module docs).
#[derive(Debug)]
pub struct LineDecoder {
    utf8: Utf8Stream,
    buffer: String,
    /// The length of `buffer` known to hold no newline (it was searched by an earlier chunk).
    scanned: usize,
    max_line: Option<usize>,
    oversized_head: Option<String>,
}

impl Default for LineDecoder {
    fn default() -> Self {
        LineDecoder::new()
    }
}

impl LineDecoder {
    /// client.ts attachJsonParser: no line limit.
    pub fn new() -> Self {
        LineDecoder {
            utf8: Utf8Stream::default(),
            buffer: String::new(),
            scanned: 0,
            max_line: None,
            oversized_head: None,
        }
    }

    /// daemon.ts: lines over `max_line` UTF-16 units are dropped and reported as [`Frame::Oversized`].
    pub fn with_max_line(max_line: usize) -> Self {
        LineDecoder {
            max_line: Some(max_line),
            ..LineDecoder::new()
        }
    }

    /// Bytes buffered for an incomplete line (decoded) or character (not yet decoded).
    pub fn buffered(&self) -> usize {
        self.buffer.len() + self.utf8.pending.len()
    }

    /// Feeds one chunk; returns the frames it completed, in order.
    ///
    /// Linear in the chunk: lines are cut from the buffer at a moving offset and the consumed prefix is dropped once,
    /// and the search for the next newline starts where the previous chunk's search ended.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<Frame> {
        self.utf8.write(chunk, &mut self.buffer);
        let mut frames = Vec::new();
        // The buffer before `start` is consumed.
        let mut start = 0;
        loop {
            let from = start.max(self.scanned);
            let newline = self.buffer[from..].find('\n').map(|index| from + index);
            if let Some(head) = &self.oversized_head {
                let Some(newline) = newline else {
                    self.buffer = js_tail(&self.buffer[start..], OVERSIZED_EXCERPT).to_owned();
                    break;
                };
                let excerpt = format!("{head}\n{}", js_tail(&self.buffer[start..newline], OVERSIZED_EXCERPT));
                frames.push(Frame::Oversized { excerpt });
                self.oversized_head = None;
                start = newline + 1;
                continue;
            }
            let Some(newline) = newline else {
                let rest = &self.buffer[start..];
                // Bytes bound UTF-16 units from above, so counting is only needed past the limit in bytes.
                if let Some(max) = self.max_line
                    && rest.len() > max
                    && js_len(rest) > max
                {
                    self.oversized_head = Some(js_head(rest, OVERSIZED_EXCERPT).to_owned());
                    self.buffer = js_tail(rest, OVERSIZED_EXCERPT).to_owned();
                } else {
                    self.buffer.drain(..start);
                }
                break;
            };
            let line = js_trim(&self.buffer[start..newline]).to_owned();
            start = newline + 1;
            if line.is_empty() {
                continue;
            }
            if let Some(max) = self.max_line
                && line.len() > max
                && js_len(&line) > max
            {
                let excerpt = format!(
                    "{}\n{}",
                    js_head(&line, OVERSIZED_EXCERPT),
                    js_tail(&line, OVERSIZED_EXCERPT)
                );
                frames.push(Frame::Oversized { excerpt });
                continue;
            }
            frames.push(Frame::Line(line));
        }
        // What is left has no newline: the next chunk's search starts after it.
        self.scanned = self.buffer.len();
        frames
    }
}

/// daemon.ts rejectOversizedRequest: the request ID in an oversized line's excerpt, matched by
/// `/"requestId":"([\w-]{1,128})"/` (the first match).
pub fn oversized_request_id(excerpt: &str) -> Option<&str> {
    const KEY: &str = "\"requestId\":\"";
    let mut from = 0;
    while let Some(found) = excerpt[from..].find(KEY) {
        let start = from + found + KEY.len();
        let rest = &excerpt[start..];
        let len = rest
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
            .count();
        if (1..=128).contains(&len) && rest.as_bytes().get(len) == Some(&b'"') {
            return Some(&rest[..len]);
        }
        from = start;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(frames: Vec<Frame>) -> Vec<String> {
        frames
            .into_iter()
            .map(|frame| match frame {
                Frame::Line(line) => line,
                Frame::Oversized { excerpt } => format!("<oversized {}>", excerpt.len()),
            })
            .collect()
    }

    #[test]
    fn encodes_one_value_per_line() {
        assert_eq!(
            encode(&serde_json::json!({"type": "ping", "requestId": "r"})).unwrap(),
            b"{\"type\":\"ping\",\"requestId\":\"r\"}\n"
        );
        assert_eq!(encode_line(&"a\nb").unwrap(), "\"a\\nb\"\n");
    }

    #[test]
    fn joins_partial_lines_across_chunks() {
        let mut decoder = LineDecoder::new();
        assert!(decoder.push(b"{\"type\":").is_empty());
        assert_eq!(lines(decoder.push(b"\"ping\"}\n{\"a\"")), vec![r#"{"type":"ping"}"#]);
        assert_eq!(
            lines(decoder.push(b":1}\n\n  \r\n {\"b\":2} \r\n")),
            vec![r#"{"a":1}"#, r#"{"b":2}"#]
        );
        assert_eq!(decoder.buffered(), 0);
    }

    #[test]
    fn joins_utf8_sequences_split_across_chunks() {
        let text = "{\"data\":\"\u{1F600}é\"}\n".as_bytes();
        let mut decoder = LineDecoder::new();
        let mut out = Vec::new();
        for byte in text {
            out.extend(lines(decoder.push(std::slice::from_ref(byte))));
        }
        assert_eq!(out, vec!["{\"data\":\"\u{1F600}é\"}"]);
    }

    #[test]
    fn replaces_invalid_utf8_like_string_decoder() {
        let mut decoder = LineDecoder::new();
        assert_eq!(lines(decoder.push(b"a\xffb\xe2\x82c\n")), vec!["a\u{FFFD}b\u{FFFD}c"]);
    }

    #[test]
    fn trims_like_js() {
        assert_eq!(js_trim("\u{FEFF} x \u{3000}"), "x");
        assert_eq!(js_trim("\u{85}x"), "\u{85}x");
    }

    #[test]
    fn invalid_lines_are_reported_by_the_parser() {
        assert!(matches!(parse_message("{nope"), Err(NdjsonError::Json(_))));
        assert!(matches!(parse_message("[1]"), Err(NdjsonError::NotAMessage)));
        assert!(matches!(parse_message(r#"{"type":1}"#), Err(NdjsonError::NotAMessage)));
        assert!(parse_message(r#"{"type":"list","requestId":"x"}"#).is_ok());
    }

    #[test]
    fn drops_oversized_lines_with_an_excerpt() {
        let mut decoder = LineDecoder::with_max_line(64);
        let long = format!(
            "{{\"type\":\"x\",\"requestId\":\"abc-1\",\"data\":\"{}\"}}",
            "z".repeat(100)
        );
        let frames = decoder.push(format!("{long}\n{{\"type\":\"ok\"}}\n").as_bytes());
        assert_eq!(frames.len(), 2);
        let Frame::Oversized { excerpt } = &frames[0] else {
            panic!("expected an oversized frame")
        };
        assert_eq!(oversized_request_id(excerpt), Some("abc-1"));
        assert_eq!(frames[1], Frame::Line(r#"{"type":"ok"}"#.into()));
    }

    #[test]
    fn drops_an_oversized_line_still_arriving() {
        let mut decoder = LineDecoder::with_max_line(16);
        assert!(decoder.push(b"{\"requestId\":\"r1\",").is_empty());
        assert!(decoder.push(&[b'y'; 50]).is_empty());
        let frames = decoder.push(b"}\n{\"type\":\"a\"}\n");
        let Frame::Oversized { excerpt } = &frames[0] else {
            panic!("expected an oversized frame")
        };
        assert_eq!(oversized_request_id(excerpt), Some("r1"));
        assert!(excerpt.ends_with("y}"));
        assert_eq!(frames[1], Frame::Line(r#"{"type":"a"}"#.into()));
    }

    #[test]
    fn an_oversized_line_after_complete_ones_in_the_same_chunk() {
        let mut decoder = LineDecoder::with_max_line(16);
        let mut chunk = b"{\"a\":1}\n\n{\"requestId\":\"r2\",".to_vec();
        chunk.extend([b'y'; 40]);
        assert_eq!(decoder.push(&chunk), vec![Frame::Line(r#"{"a":1}"#.into())]);
        assert!(decoder.push(b"yy\nz").len() == 1);
        assert_eq!(lines(decoder.push(b"\n")), vec!["z"]);
        assert_eq!(decoder.buffered(), 0);
    }

    /// A chunk of many lines, or a long line arriving in many chunks, is decoded in time linear in its size (each byte
    /// is scanned and moved a bounded number of times), not one buffer shift per line or one rescan per chunk.
    #[test]
    fn decodes_in_linear_time() {
        let started = std::time::Instant::now();
        let mut decoder = LineDecoder::with_max_line(MAX_REQUEST_LINE);
        let chunk = "{\"a\":1}\n".repeat(400_000);
        assert_eq!(decoder.push(chunk.as_bytes()).len(), 400_000);
        assert_eq!(decoder.buffered(), 0);
        let mut client = LineDecoder::new();
        for _ in 0..40_000 {
            assert!(client.push(&[b'x'; 100]).is_empty());
        }
        assert_eq!(lines(client.push(b"\n")), vec!["x".repeat(4_000_000)]);
        let elapsed = started.elapsed();
        assert!(elapsed < std::time::Duration::from_secs(5), "took {elapsed:?}");
    }

    #[test]
    fn request_id_pattern_matches_the_ts_regex() {
        assert_eq!(
            oversized_request_id(r#"x"requestId":"""requestId":"ok_1-2""#),
            Some("ok_1-2")
        );
        assert_eq!(oversized_request_id(r#""requestId":"a b""#), None);
        assert_eq!(
            oversized_request_id(&format!("\"requestId\":\"{}\"", "a".repeat(129))),
            None
        );
    }
}
