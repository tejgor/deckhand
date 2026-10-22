//! The daemon client: requests, ping, starting the daemon.
//!
//! One-shot requests open a fresh connection to the daemon socket, write one NDJSON request and wait for the response
//! with the same `requestId`. [`ensure_daemon_running`] pings the daemon and starts one when none answers, but never
//! replaces a live daemon that speaks another protocol version: it fails with a `kill <pid>` hint instead, because
//! killing it would take every running agent down with it. The long-lived subscription lives in [`crate::live`].

use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::UnixStream;

use deckhand_core::error::{DhError, errno_code};
use deckhand_core::jsstr::date;
use deckhand_protocol::ndjson::{Frame, LineDecoder};
use deckhand_protocol::{PROTOCOL_VERSION, Request, ServerMessage};
use deckhand_sys::paths;

/// TS: client.ts request default `timeoutMs` (15 s).
pub const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 15_000;

/// TS: client.ts waitForDaemon poll interval (150 ms between pings).
pub const DAEMON_POLL_INTERVAL: Duration = Duration::from_millis(150);

/// TS: client.ts ensureDaemonRunning: how long a freshly spawned daemon gets to answer a ping (5 s).
pub const DAEMON_START_DEADLINE_MS: u64 = 5_000;

/// TS: client.ts ensureDaemonRunning: how long a live daemon that failed one ping gets before we give up (2 s).
pub const DAEMON_RETRY_DEADLINE_MS: u64 = 2_000;

/// Read buffer size for daemon connections.
const READ_CHUNK: usize = 64 * 1024;

/// Which way a wire-logged message went (`{dir, msg}` in the wire log).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum WireDir {
    /// Client to daemon.
    #[serde(rename = "c2d")]
    C2d,
    /// Daemon to client.
    #[serde(rename = "d2c")]
    D2c,
}

// ---------------------------------------------------------------------------------------------------------------
// JS value helpers

/// `Number.prototype.toString()` for a finite double (JSON.stringify writes numbers this way): positional for decimal
/// exponents -7 < e < 21, else `<digits>e+<exp>` / `<digits>e-<exp>`; `-0` is `0`.
pub(crate) fn js_number(value: f64) -> String {
    if value.is_nan() {
        return String::from("NaN");
    }
    if value.is_infinite() {
        return String::from(if value > 0.0 { "Infinity" } else { "-Infinity" });
    }
    if value == 0.0 {
        return String::from("0");
    }
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((scientific.as_str(), "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    if (-7 < exponent) && (exponent < 21) {
        // Rust's Display is the shortest round-trip form, positional, without a trailing `.0`.
        return format!("{value}");
    }
    let sign = if value < 0.0 { "-" } else { "" };
    let exponent_sign = if exponent < 0 { '-' } else { '+' };
    format!("{sign}{mantissa}e{exponent_sign}{}", exponent.unsigned_abs())
}

/// `String(value)` for a JSON value.
pub(crate) fn js_string(value: &Value) -> String {
    match value {
        Value::Null => String::from("null"),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => match (number.as_i64(), number.as_u64(), number.as_f64()) {
            (Some(int), _, _) => int.to_string(),
            (_, Some(uint), _) => uint.to_string(),
            (_, _, Some(float)) => js_number(float),
            _ => number.to_string(),
        },
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                other => js_string(other),
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => String::from("[object Object]"),
    }
}

/// JS truthiness of a JSON value.
pub(crate) fn js_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(_) | Value::Object(_)) => true,
    }
}

/// A failed response's message: `payload.error || 'daemon request failed'`.
pub(crate) fn response_error(payload: &Value) -> DhError {
    let error = payload.get("error");
    if js_truthy(error) {
        DhError::msg(js_string(error.unwrap_or(&Value::Null)))
    } else {
        DhError::msg("daemon request failed")
    }
}

/// `payload.type === 'response' && payload.requestId === id`.
pub(crate) fn is_response_to(payload: &Value, id: &str) -> bool {
    payload.get("type").and_then(Value::as_str) == Some("response")
        && payload.get("requestId").and_then(Value::as_str) == Some(id)
}

/// serde_json's compact output with numbers written as JS writes them (`1e+21`, not `1e21`; `100`, not `100.0`).
struct JsFormatter;

impl serde_json::ser::Formatter for JsFormatter {
    fn write_f64<W: ?Sized + std::io::Write>(&mut self, writer: &mut W, value: f64) -> std::io::Result<()> {
        writer.write_all(js_number(value).as_bytes())
    }

    fn write_f32<W: ?Sized + std::io::Write>(&mut self, writer: &mut W, value: f32) -> std::io::Result<()> {
        writer.write_all(js_number(f64::from(value)).as_bytes())
    }
}

/// `JSON.stringify(value)` (compact, JS number formatting; strings escape as JS does).
pub fn json_stringify<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, DhError> {
    let mut out = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, JsFormatter);
    value.serialize(&mut serializer)?;
    Ok(out)
}

/// `` `${JSON.stringify(message)}\n` `` for any message (what [`frame_message`] writes, without the wire log).
pub fn frame<T: Serialize + ?Sized>(message: &T) -> Result<Vec<u8>, DhError> {
    let mut line = json_stringify(message)?;
    line.push(b'\n');
    Ok(line)
}

/// Node's message for a failed socket operation: `<syscall> <CODE>` (`read ECONNRESET`, `write EPIPE`).
pub(crate) fn socket_error(error: &std::io::Error, syscall: &str) -> DhError {
    match errno_code(error) {
        Some(code) => DhError::with_code(code, format!("{syscall} {code}")),
        None => DhError::msg(error.to_string()),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Connection and framing

/// TS: client.ts createConnection (connects to the daemon socket; a failure carries Node's message, e.g.
/// `connect ENOENT <socket path>`).
pub async fn create_connection() -> Result<UnixStream, DhError> {
    connect_at(&paths::socket_path()).await
}

/// [`create_connection`] to a given socket.
pub(crate) async fn connect_at(socket_path: &Path) -> Result<UnixStream, DhError> {
    UnixStream::connect(socket_path)
        .await
        .map_err(|error| match errno_code(&error) {
            Some(code) => DhError::with_code(code, format!("connect {code} {}", socket_path.display())),
            None => DhError::msg(error.to_string()),
        })
}

/// TS: client.ts wireLog. `DECKHAND_WIRE_LOG=<dir>` (test/porting aid, off by default): every message sent or received
/// is appended to `<dir>/client-<pid>.jsonl` as `{"dir":..,"msg":..}`; failures are ignored.
pub fn wire_log<T: Serialize + ?Sized>(dir: WireDir, msg: &T) {
    let Some(log_dir) = std::env::var_os("DECKHAND_WIRE_LOG") else {
        return;
    };
    if log_dir.is_empty() {
        return;
    }
    #[derive(Serialize)]
    struct Entry<'a, T: ?Sized> {
        dir: WireDir,
        msg: &'a T,
    }
    let Ok(line) = frame(&Entry { dir, msg }) else {
        return;
    };
    let path = PathBuf::from(log_dir).join(format!("client-{}.jsonl", std::process::id()));
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(&line);
    }
}

/// The line [`write_message`] writes (`JSON.stringify(message)` + `\n`), wire-logged as `c2d`. Lets a caller that
/// queues writes (the [`crate::live::LiveClient`] writer) send synchronously, as `socket.write` does in TS.
pub fn frame_message(message: &Request) -> Result<Vec<u8>, DhError> {
    wire_log(WireDir::C2d, message);
    frame(message)
}

/// TS: client.ts writeMessage (exported): wire-logs and writes one request line.
pub async fn write_message<W: AsyncWrite + Unpin>(writer: &mut W, message: &Request) -> Result<(), DhError> {
    let line = frame_message(message)?;
    writer
        .write_all(&line)
        .await
        .map_err(|error| socket_error(&error, "write"))
}

/// How deep [`json_parse`] nests arrays and objects (serde_json stops at 128). V8 has no such limit, but a `Value` is
/// dropped, cloned, serialized and converted to a [`ServerMessage`] recursively, so a much deeper one could overflow
/// the stack (an abort, where TS keeps going); at this depth all of that fits a 2 MiB thread stack in a debug build.
/// Deeper lines fail with V8's stack-overflow message instead; the daemon never sends anything close.
const MAX_JSON_DEPTH: usize = 512;

/// `JSON.parse(line)` as a `Value`. serde_json takes the common case; a line it rejects that V8 accepts (a lone
/// surrogate escape, a number beyond f64 such as `1e400`, nesting deeper than 128) goes through
/// [`parse_js_json_value`]; a line V8 rejects fails with V8's message ([`DhError::json_parse`]).
pub(crate) fn json_parse(line: &str) -> Result<Value, DhError> {
    if let Ok(value) = serde_json::from_str::<Value>(line) {
        return Ok(value);
    }
    // The iterative parser runs before the message scanner, which recurses: a line it rejects as invalid failed
    // within MAX_JSON_DEPTH levels, so the scanner never nests deeper than that either.
    match parse_js_json_value(line) {
        Ok(value) => Ok(value),
        Err(JsJsonError::TooDeep) => Err(DhError::msg("Maximum call stack size exceeded")),
        Err(JsJsonError::Invalid) => Err(DhError::json_parse(line)),
    }
}

/// Why [`parse_js_json_value`] gave up.
#[derive(Debug, PartialEq, Eq)]
enum JsJsonError {
    /// Nested deeper than [`MAX_JSON_DEPTH`].
    TooDeep,
    /// Not JSON (never for text V8 accepted).
    Invalid,
}

/// An array or object [`parse_js_json_value`] is inside of: its members so far (an object's with the key whose value
/// is being parsed).
enum JsJsonFrame {
    Array(Vec<Value>),
    Object(serde_json::Map<String, Value>, String),
}

/// `JSON.parse` for text serde_json rejects, iteratively (no recursion limit but [`MAX_JSON_DEPTH`]). Where a JS
/// value has no `Value` form it takes the one Node gives it on the way out: a lone surrogate becomes U+FFFD (as Node
/// encodes it to UTF-8; one UTF-16 unit either way), ±Infinity becomes `null` (as JSON.stringify writes it). A
/// repeated key keeps its first position and takes the last value, as a JS object assignment does.
fn parse_js_json_value(text: &str) -> Result<Value, JsJsonError> {
    let mut scanner = JsJsonScanner {
        bytes: text.as_bytes(),
        pos: 0,
    };
    let mut stack: Vec<JsJsonFrame> = Vec::new();
    loop {
        scanner.skip_whitespace();
        let mut value = match scanner.peek() {
            Some(b'[') => {
                scanner.pos += 1;
                scanner.skip_whitespace();
                if scanner.eat(b']') {
                    Value::Array(Vec::new())
                } else {
                    push_frame(&mut stack, JsJsonFrame::Array(Vec::new()))?;
                    continue;
                }
            }
            Some(b'{') => {
                scanner.pos += 1;
                scanner.skip_whitespace();
                if scanner.eat(b'}') {
                    Value::Object(serde_json::Map::new())
                } else {
                    let key = scanner.member_key()?;
                    push_frame(&mut stack, JsJsonFrame::Object(serde_json::Map::new(), key))?;
                    continue;
                }
            }
            Some(b'"') => Value::String(scanner.string()?),
            Some(b't') => scanner.literal("true", Value::Bool(true))?,
            Some(b'f') => scanner.literal("false", Value::Bool(false))?,
            Some(b'n') => scanner.literal("null", Value::Null)?,
            _ => scanner.number()?,
        };
        // Hand the finished value to its container, closing every container it finishes in turn.
        loop {
            scanner.skip_whitespace();
            match stack.last_mut() {
                None => {
                    return if scanner.pos == scanner.bytes.len() {
                        Ok(value)
                    } else {
                        Err(JsJsonError::Invalid)
                    };
                }
                Some(JsJsonFrame::Array(items)) => {
                    items.push(value);
                    if scanner.eat(b',') {
                        break;
                    }
                    if !scanner.eat(b']') {
                        return Err(JsJsonError::Invalid);
                    }
                    let Some(JsJsonFrame::Array(items)) = stack.pop() else {
                        return Err(JsJsonError::Invalid);
                    };
                    value = Value::Array(items);
                }
                Some(JsJsonFrame::Object(members, key)) => {
                    members.insert(std::mem::take(key), value);
                    if scanner.eat(b',') {
                        scanner.skip_whitespace();
                        *key = scanner.member_key()?;
                        break;
                    }
                    if !scanner.eat(b'}') {
                        return Err(JsJsonError::Invalid);
                    }
                    let Some(JsJsonFrame::Object(members, _)) = stack.pop() else {
                        return Err(JsJsonError::Invalid);
                    };
                    value = Value::Object(members);
                }
            }
        }
    }
}

fn push_frame(stack: &mut Vec<JsJsonFrame>, frame: JsJsonFrame) -> Result<(), JsJsonError> {
    if stack.len() >= MAX_JSON_DEPTH {
        return Err(JsJsonError::TooDeep);
    }
    stack.push(frame);
    Ok(())
}

/// The cursor of [`parse_js_json_value`] (a byte index into valid UTF-8; JSON's syntax is all ASCII).
struct JsJsonScanner<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl JsJsonScanner<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn eat(&mut self, byte: u8) -> bool {
        let matched = self.peek() == Some(byte);
        if matched {
            self.pos += 1;
        }
        matched
    }

    /// JSON whitespace (V8 skips exactly these four).
    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Result<Value, JsJsonError> {
        let end = self.pos + word.len();
        if self.bytes.get(self.pos..end) != Some(word.as_bytes()) {
            return Err(JsJsonError::Invalid);
        }
        self.pos = end;
        Ok(value)
    }

    /// `"key"` then `:`, leaving the cursor at the member's value.
    fn member_key(&mut self) -> Result<String, JsJsonError> {
        if self.peek() != Some(b'"') {
            return Err(JsJsonError::Invalid);
        }
        let key = self.string()?;
        self.skip_whitespace();
        if !self.eat(b':') {
            return Err(JsJsonError::Invalid);
        }
        Ok(key)
    }

    fn hex4(&mut self) -> Result<u16, JsJsonError> {
        let digits = self.bytes.get(self.pos..self.pos + 4).ok_or(JsJsonError::Invalid)?;
        let digits = std::str::from_utf8(digits).map_err(|_| JsJsonError::Invalid)?;
        if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(JsJsonError::Invalid);
        }
        let unit = u16::from_str_radix(digits, 16).map_err(|_| JsJsonError::Invalid)?;
        self.pos += 4;
        Ok(unit)
    }

    /// A string literal at the cursor (on its opening quote), as UTF-16 decoded lossily: a surrogate escape that is
    /// not half of a pair becomes U+FFFD.
    fn string(&mut self) -> Result<String, JsJsonError> {
        self.pos += 1;
        let mut units: Vec<u16> = Vec::new();
        loop {
            let start = self.pos;
            while matches!(self.peek(), Some(byte) if byte != b'"' && byte != b'\\' && byte >= 0x20) {
                self.pos += 1;
            }
            let run = self.bytes.get(start..self.pos).ok_or(JsJsonError::Invalid)?;
            let run = std::str::from_utf8(run).map_err(|_| JsJsonError::Invalid)?;
            units.extend(run.encode_utf16());
            match self.peek() {
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(String::from_utf16_lossy(&units));
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let escape = self.peek().ok_or(JsJsonError::Invalid)?;
                    self.pos += 1;
                    let unit = match escape {
                        b'"' => 0x22,
                        b'\\' => 0x5c,
                        b'/' => 0x2f,
                        b'b' => 0x08,
                        b'f' => 0x0c,
                        b'n' => 0x0a,
                        b'r' => 0x0d,
                        b't' => 0x09,
                        b'u' => self.hex4()?,
                        _ => return Err(JsJsonError::Invalid),
                    };
                    units.push(unit);
                }
                _ => return Err(JsJsonError::Invalid),
            }
        }
    }

    /// `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`: serde_json's number where it has one (exact integers), else
    /// the double JS reads, ±Infinity as `null`.
    fn number(&mut self) -> Result<Value, JsJsonError> {
        let start = self.pos;
        self.eat(b'-');
        let digits = |scanner: &mut Self| {
            let from = scanner.pos;
            while matches!(scanner.peek(), Some(b'0'..=b'9')) {
                scanner.pos += 1;
            }
            scanner.pos > from
        };
        // No leading zeros: a `0` ends the integer part (what follows it is then out of place).
        if !self.eat(b'0') && !digits(self) {
            return Err(JsJsonError::Invalid);
        }
        if self.eat(b'.') && !digits(self) {
            return Err(JsJsonError::Invalid);
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !digits(self) {
                return Err(JsJsonError::Invalid);
            }
        }
        let lexeme = self.bytes.get(start..self.pos).ok_or(JsJsonError::Invalid)?;
        let lexeme = std::str::from_utf8(lexeme).map_err(|_| JsJsonError::Invalid)?;
        if let Ok(number) = serde_json::from_str::<serde_json::Number>(lexeme) {
            return Ok(Value::Number(number));
        }
        let double: f64 = lexeme.parse().map_err(|_| JsJsonError::Invalid)?;
        Ok(serde_json::Number::from_f64(double).map_or(Value::Null, Value::Number))
    }
}

/// What one chunk fed to a [`JsonParser`] produced.
#[derive(Debug, Default)]
pub struct Parsed {
    /// The messages completed, in order (any JSON value, as `JSON.parse` returns it).
    pub messages: Vec<Value>,
    /// The `JSON.parse` error of the first line that was not JSON: the messages before it were delivered, the lines
    /// after it stay buffered for the next chunk (as the TS listener threw out of `handleData` mid-buffer).
    pub error: Option<DhError>,
}

/// TS: client.ts attachJsonParser (exported): the per-socket framing state. Feeding it a chunk replaces the TS `data`
/// listener; dropping it replaces the returned stop function (`decoder.end()` output is discarded there too).
#[derive(Debug, Default)]
pub struct JsonParser {
    decoder: LineDecoder,
    /// Complete lines not parsed yet (only after a line failed to parse).
    lines: VecDeque<String>,
}

impl JsonParser {
    /// A parser with no line limit (`StringDecoder('utf8')`, split on `\n`, lines trimmed, empty lines skipped).
    pub fn new() -> Self {
        JsonParser {
            decoder: LineDecoder::new(),
            lines: VecDeque::new(),
        }
    }

    /// Feeds one chunk and parses every complete line as JSON, each wire-logged as `d2c`; stops at the first line that
    /// is not JSON (see [`Parsed::error`]).
    pub fn push_values(&mut self, chunk: &[u8]) -> Parsed {
        for frame in self.decoder.push(chunk) {
            if let Frame::Line(line) = frame {
                self.lines.push_back(line);
            }
        }
        let mut parsed = Parsed::default();
        while let Some(line) = self.lines.pop_front() {
            match json_parse(&line) {
                Ok(message) => {
                    wire_log(WireDir::D2c, &message);
                    parsed.messages.push(message);
                }
                Err(error) => {
                    parsed.error = Some(error);
                    break;
                }
            }
        }
        parsed
    }

    /// Feeds one chunk; returns the messages it completed, in order, each wire-logged as `d2c`. A line that is not
    /// JSON fails with `JSON.parse`'s message ([`DhError::json_parse`]); a JSON line whose `type` no
    /// [`ServerMessage`] variant has is skipped, as every TS consumer ignores unknown types.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<ServerMessage>, DhError> {
        let parsed = self.push_values(chunk);
        if let Some(error) = parsed.error {
            return Err(error);
        }
        Ok(parsed.messages.into_iter().filter_map(to_server_message).collect())
    }
}

/// A parsed line as a [`ServerMessage`]; `None` for an unknown `type` (or a known one whose shape does not match).
pub(crate) fn to_server_message(message: Value) -> Option<ServerMessage> {
    let kind = message.get("type").and_then(Value::as_str)?;
    if !ServerMessage::TYPES.contains(&kind) {
        return None;
    }
    serde_json::from_value(message).ok()
}

/// Reads [`ServerMessage`]s off a socket with a [`JsonParser`] (the async face of `attachJsonParser` + `on('data')`).
#[derive(Debug)]
pub struct MessageReader<R> {
    reader: R,
    parser: JsonParser,
    queued: std::collections::VecDeque<ServerMessage>,
    /// A parse error to report once the messages before it were read.
    error: Option<DhError>,
}

impl<R: AsyncRead + Unpin> MessageReader<R> {
    /// Wraps the read half of a daemon connection.
    pub fn new(reader: R) -> Self {
        MessageReader {
            reader,
            parser: JsonParser::new(),
            queued: VecDeque::new(),
            error: None,
        }
    }

    /// The next message; `Ok(None)` once the daemon closed the connection.
    pub async fn next(&mut self) -> Result<Option<ServerMessage>, DhError> {
        let mut buffer = vec![0u8; READ_CHUNK];
        loop {
            if let Some(message) = self.queued.pop_front() {
                return Ok(Some(message));
            }
            if let Some(error) = self.error.take() {
                return Err(error);
            }
            let read = self
                .reader
                .read(&mut buffer)
                .await
                .map_err(|error| socket_error(&error, "read"))?;
            if read == 0 {
                return Ok(None);
            }
            let parsed = self.parser.push_values(buffer.get(..read).unwrap_or_default());
            self.queued
                .extend(parsed.messages.into_iter().filter_map(to_server_message));
            self.error = parsed.error;
        }
    }

    /// The read half back.
    pub fn into_inner(self) -> R {
        self.reader
    }
}

// ---------------------------------------------------------------------------------------------------------------
// One-shot requests

/// TS: client.ts request (exported). Sends one request on a fresh connection and resolves with the matching response's
/// `data`. Errors (verbatim TS messages): the response's `error` (or `daemon request failed`), `daemon request timed
/// out` after `timeout_ms` (default [`DEFAULT_REQUEST_TIMEOUT_MS`]), `daemon connection closed before response`, or the
/// socket's own error.
pub async fn request<T: DeserializeOwned>(message: &Request, timeout_ms: Option<u64>) -> Result<T, DhError> {
    request_at(&paths::socket_path(), message, timeout_ms).await
}

/// [`request`] on a given socket.
pub(crate) async fn request_at<T: DeserializeOwned>(
    socket_path: &Path,
    message: &Request,
    timeout_ms: Option<u64>,
) -> Result<T, DhError> {
    let data = request_value_at(socket_path, message, timeout_ms).await?;
    decode_data(data)
}

/// A response's `data` (absent: `null`, TS `undefined`) as `T`. TS casts whatever arrived (`payload.data as T`), so a
/// caller that ignores the data (`T` = `()`, as for `Promise<void>`) accepts any: a `T` that `null` decodes to is
/// the fallback when the data does not fit.
pub(crate) fn decode_data<T: DeserializeOwned>(data: Option<Value>) -> Result<T, DhError> {
    serde_json::from_value(data.unwrap_or(Value::Null)).or_else(|error| {
        serde_json::from_value(Value::Null)
            .map_err(|_| DhError::msg(format!("unexpected daemon response data: {error}")))
    })
}

/// [`request_at`] up to the response's raw `data`.
async fn request_value_at(
    socket_path: &Path,
    message: &Request,
    timeout_ms: Option<u64>,
) -> Result<Option<Value>, DhError> {
    let mut socket = connect_at(socket_path).await?;
    // The TS timer starts once connected, before the request is written.
    let timeout = Duration::from_millis(timeout_ms.unwrap_or(DEFAULT_REQUEST_TIMEOUT_MS));
    let outcome = tokio::time::timeout(timeout, exchange(&mut socket, message)).await;
    match outcome {
        Ok(Ok(data)) => {
            let _ = socket.shutdown().await;
            Ok(data)
        }
        Ok(Err(error)) => Err(error),
        Err(_) => Err(DhError::msg("daemon request timed out")),
    }
}

/// Writes `message` and reads until its response (or the connection ends).
async fn exchange(socket: &mut UnixStream, message: &Request) -> Result<Option<Value>, DhError> {
    let id = message.request_id().map(str::to_owned);
    write_message(socket, message).await?;
    let mut parser = JsonParser::new();
    let mut buffer = vec![0u8; READ_CHUNK];
    loop {
        let read = socket
            .read(&mut buffer)
            .await
            .map_err(|error| socket_error(&error, "read"))?;
        if read == 0 {
            return Err(DhError::msg("daemon connection closed before response"));
        }
        let parsed = parser.push_values(buffer.get(..read).unwrap_or_default());
        for payload in parsed.messages {
            let Some(id) = id.as_deref() else { continue };
            if !is_response_to(&payload, id) {
                continue;
            }
            if !js_truthy(payload.get("ok")) {
                return Err(response_error(&payload));
            }
            return Ok(payload.get("data").cloned());
        }
        if let Some(error) = parsed.error {
            return Err(error);
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Ping and daemon lifecycle

/// TS: client.ts ProtocolMismatchError vs any other ping failure.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PingError {
    /// TS: ProtocolMismatchError: `daemon protocol mismatch: expected v49, got <version>` (`undefined` when the daemon
    /// sent no version).
    #[error("{0}")]
    ProtocolMismatch(String),
    /// The ping request itself failed (no daemon, timeout, closed connection, ...).
    #[error(transparent)]
    Failed(#[from] DhError),
}

/// TS: client.ts ping: a `ping` request whose `version` must equal [`deckhand_protocol::PROTOCOL_VERSION`].
pub async fn ping() -> Result<(), PingError> {
    ping_at(&paths::socket_path()).await
}

/// [`ping`] on a given socket.
pub(crate) async fn ping_at(socket_path: &Path) -> Result<(), PingError> {
    let message = Request::Ping(deckhand_protocol::wire::RequestOnly {
        request_id: uuid::Uuid::new_v4().to_string(),
    });
    let data = request_value_at(socket_path, &message, None).await?;
    check_ping_version(data.as_ref())
}

/// `response.version !== PROTOCOL_VERSION` on a ping's `data` (`undefined`/`null` data throws as property access does).
fn check_ping_version(data: Option<&Value>) -> Result<(), PingError> {
    let version = match data {
        None => {
            return Err(PingError::Failed(DhError::msg(
                "Cannot read properties of undefined (reading 'version')",
            )));
        }
        Some(Value::Null) => {
            return Err(PingError::Failed(DhError::msg(
                "Cannot read properties of null (reading 'version')",
            )));
        }
        Some(Value::Object(object)) => object.get("version"),
        Some(_) => None,
    };
    let matches = version
        .and_then(Value::as_f64)
        .is_some_and(|version| version == f64::from(PROTOCOL_VERSION));
    if matches {
        return Ok(());
    }
    let got = version.map_or_else(|| String::from("undefined"), js_string);
    Err(PingError::ProtocolMismatch(format!(
        "daemon protocol mismatch: expected v{PROTOCOL_VERSION}, got {got}"
    )))
}

/// `Number.parseInt(text, 10)` when it is a positive integer that fits a pid.
fn parse_pid(raw: &str) -> Option<i32> {
    let text = deckhand_core::jsstr::js_trim(raw);
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let end = digits.bytes().take_while(u8::is_ascii_digit).count();
    let digits = &digits[..end];
    if negative || digits.is_empty() {
        return None;
    }
    // A pid too large for i32 is never a live process (TS: kill() throws, so not alive), the only use of the value.
    digits.parse::<i32>().ok().filter(|pid| *pid > 0)
}

/// TS: client.ts readDaemonPid: the positive integer in `daemon.pid` (`parseInt` semantics), if any.
pub async fn read_daemon_pid() -> Option<i32> {
    read_daemon_pid_at(&paths::daemon_pid_path()).await
}

async fn read_daemon_pid_at(pid_path: &Path) -> Option<i32> {
    let raw = tokio::fs::read(pid_path).await.ok()?;
    parse_pid(&String::from_utf8_lossy(&raw))
}

/// TS: client.ts isProcessAlive (`kill(pid, 0)` succeeds; `None`/0 is never alive).
pub fn is_process_alive(pid: Option<i32>) -> bool {
    match pid {
        Some(pid) if pid > 0 => nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok(),
        _ => false,
    }
}

/// TS: client.ts appendClientLog: appends `[<ISO time>] client <message>\n` to `daemon.log` (creating the config dir).
pub async fn append_client_log(message: &str) -> Result<(), DhError> {
    append_client_log_at(&paths::config_dir(), &paths::daemon_log_path(), message).await
}

async fn append_client_log_at(config_dir: &Path, log_path: &Path, message: &str) -> Result<(), DhError> {
    tokio::fs::create_dir_all(config_dir)
        .await
        .map_err(|error| DhError::io(&error, "mkdir", &config_dir.to_string_lossy()))?;
    let now = date::to_iso_string(date::now_ms()).unwrap_or_default();
    let line = format!("[{now}] client {message}\n");
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .await
        .map_err(|error| DhError::io(&error, "open", &log_path.to_string_lossy()))?;
    file.write_all(line.as_bytes()).await?;
    // tokio's File writes in the background: flush so the line is on disk when this returns.
    file.flush().await?;
    Ok(())
}

/// The daemon's working directory (TS getProjectRoot, the package root): the directory of the executable.
fn project_root(executable: &Path) -> PathBuf {
    executable
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(paths::current_dir)
}

/// TS: client.ts spawnDaemon: starts `<deckhand> --daemon` detached (own session), cwd the project root, env plus
/// `DECKHAND_DAEMON=1`, stdin ignored and stdout/stderr appended to `daemon.log`; never waited for.
pub fn spawn_daemon() -> Result<(), DhError> {
    let executable = paths::cli_entry_path()?;
    let log_path = paths::daemon_log_path();
    let open_log = || {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|error| DhError::io(&error, "open", &log_path.to_string_lossy()))
    };
    let stdout = open_log()?;
    let stderr = open_log()?;
    let options = deckhand_sys::proc::ExecOptions::default()
        .cwd(project_root(&executable))
        .env("DECKHAND_DAEMON", "1");
    let program = executable.to_string_lossy().into_owned();
    if tokio::runtime::Handle::try_current().is_ok() {
        // tokio reaps the child once it exits (TS `child.unref()`: nobody waits for it).
        let mut command = deckhand_sys::proc::command(&program, &["--daemon"], &options);
        command
            .stdin(std::process::Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .kill_on_drop(false);
        command.spawn().map_err(|error| spawn_error(&program, &error))?;
    } else {
        let mut command = std::process::Command::new(&program);
        command
            .arg("--daemon")
            .current_dir(project_root(&executable))
            .env("DECKHAND_DAEMON", "1")
            .stdin(std::process::Stdio::null())
            .stdout(stdout)
            .stderr(stderr);
        {
            use std::os::unix::process::CommandExt as _;
            // SAFETY: setsid is async-signal-safe and touches no memory of the parent.
            unsafe {
                command.pre_exec(|| nix::unistd::setsid().map(|_| ()).map_err(std::io::Error::from));
            }
        }
        command.spawn().map_err(|error| spawn_error(&program, &error))?;
    }
    Ok(())
}

/// Node's `spawn <program> <CODE>` error.
fn spawn_error(program: &str, error: &std::io::Error) -> DhError {
    match errno_code(error) {
        Some(code) => DhError::with_code(code, format!("spawn {program} {code}")),
        None => DhError::msg(error.to_string()),
    }
}

/// TS: client.ts waitForDaemon: pings every [`DAEMON_POLL_INTERVAL`] until one succeeds or `deadline_ms` passes, then
/// fails with `failed to start daemon: <last ping error>`.
pub async fn wait_for_daemon(deadline_ms: u64) -> Result<(), DhError> {
    wait_for_daemon_at(&paths::socket_path(), deadline_ms).await
}

async fn wait_for_daemon_at(socket_path: &Path, deadline_ms: u64) -> Result<(), DhError> {
    let deadline = Instant::now() + Duration::from_millis(deadline_ms);
    let mut last_error: Option<PingError> = None;
    while Instant::now() < deadline {
        match ping_at(socket_path).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                tokio::time::sleep(DAEMON_POLL_INTERVAL).await;
            }
        }
    }
    let described = last_error.map_or_else(|| String::from("undefined"), |error| error.to_string());
    Err(DhError::msg(format!("failed to start daemon: {described}")))
}

/// The files [`ensure_daemon_running`] works with (from `DECKHAND_HOME` in production; temp dirs in tests).
#[derive(Clone, Debug)]
pub(crate) struct DaemonFiles {
    pub(crate) config_dir: PathBuf,
    pub(crate) socket: PathBuf,
    pub(crate) pid: PathBuf,
    pub(crate) log: PathBuf,
}

/// [`append_client_log`] into `files`.
async fn log(files: &DaemonFiles, message: String) -> Result<(), DhError> {
    append_client_log_at(&files.config_dir, &files.log, &message).await
}

impl DaemonFiles {
    fn from_env() -> Self {
        DaemonFiles {
            config_dir: paths::config_dir(),
            socket: paths::socket_path(),
            pid: paths::daemon_pid_path(),
            log: paths::daemon_log_path(),
        }
    }
}

/// TS: client.ts ensureDaemonRunning (exported).
///
/// - Ping succeeds: done.
/// - Protocol mismatch and the pid in `daemon.pid` is alive: logs and fails with
///   `<mismatch message>; stop the old daemon first: kill <pid> (or kill $(cat <pid path>))`.
/// - Other failure with a live pid: retries for [`DAEMON_RETRY_DEADLINE_MS`], then fails with
///   `daemon pid <pid> is alive but did not respond; see <log path>: <error>`.
/// - Otherwise: logs, removes the stale socket, spawns a daemon and waits [`DAEMON_START_DEADLINE_MS`] for it.
pub async fn ensure_daemon_running() -> Result<(), DhError> {
    ensure_daemon_running_with(&DaemonFiles::from_env(), spawn_daemon).await
}

/// [`ensure_daemon_running`] with explicit files and spawner.
pub(crate) async fn ensure_daemon_running_with(
    files: &DaemonFiles,
    spawn: impl FnOnce() -> Result<(), DhError>,
) -> Result<(), DhError> {
    let initial_error = match ping_at(&files.socket).await {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    let pid = read_daemon_pid_at(&files.pid).await;
    if let (PingError::ProtocolMismatch(mismatch), Some(live_pid)) = (&initial_error, pid)
        && is_process_alive(pid)
    {
        let restart_hint = format!(
            "stop the old daemon first: kill {live_pid} (or kill $(cat {}))",
            files.pid.display()
        );
        log(
            files,
            format!(
                "refusing to replace live daemon pid {live_pid} after protocol mismatch: {mismatch}; {restart_hint}"
            ),
        )
        .await?;
        return Err(DhError::msg(format!("{mismatch}; {restart_hint}")));
    }

    if let Some(live_pid) = pid
        && is_process_alive(pid)
    {
        log(
            files,
            format!(
                "ping failed while daemon pid {live_pid} is still alive; retrying before replacement: {initial_error}"
            ),
        )
        .await?;
        return wait_for_daemon_at(&files.socket, DAEMON_RETRY_DEADLINE_MS)
            .await
            .map_err(|retry_error| {
                DhError::msg(format!(
                    "daemon pid {live_pid} is alive but did not respond; see {}: {retry_error}",
                    files.log.display()
                ))
            });
    }

    log(files, format!("starting daemon after ping failure: {initial_error}")).await?;
    // A stale socket from a dead daemon; failures are ignored.
    let _ = tokio::fs::remove_file(&files.socket).await;
    spawn()?;
    wait_for_daemon_at(&files.socket, DAEMON_START_DEADLINE_MS).await
}

/// TS: client.ts openPersistentConnection (exported): [`ensure_daemon_running`], then a new connection.
pub async fn open_persistent_connection() -> Result<UnixStream, DhError> {
    ensure_daemon_running().await?;
    create_connection().await
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use deckhand_testkit::{Case, replay, tags};
    use serde_json::json;

    use super::*;

    /// A fake daemon on `socket`: every request line goes to `reply`, whose text is written back verbatim (empty:
    /// nothing). `None` from `reply` closes the connection.
    fn serve(socket: &Path, reply: impl Fn(Value) -> Option<String> + Send + Sync + 'static) {
        let listener = UnixListener::bind(socket).unwrap();
        let reply = Arc::new(reply);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let reply = Arc::clone(&reply);
                std::thread::spawn(move || {
                    let mut writer = stream.try_clone().unwrap();
                    for line in BufReader::new(stream).lines() {
                        let Ok(line) = line else { return };
                        let request: Value = serde_json::from_str(&line).unwrap();
                        match reply(request) {
                            Some(text) => {
                                let _ = writer.write_all(text.as_bytes());
                            }
                            None => return,
                        }
                    }
                });
            }
        });
    }

    fn response(request: &Value, body: Value) -> String {
        let mut message = json!({"type": "response", "requestId": request["requestId"]});
        if let (Some(message), Some(body)) = (message.as_object_mut(), body.as_object()) {
            message.extend(body.clone());
        }
        format!("{message}\n")
    }

    fn ping_request() -> Request {
        Request::Ping(deckhand_protocol::wire::RequestOnly {
            request_id: String::from("r1"),
        })
    }

    fn files(dir: &Path) -> DaemonFiles {
        DaemonFiles {
            config_dir: dir.to_path_buf(),
            socket: dir.join("daemon.sock"),
            pid: dir.join("daemon.pid"),
            log: dir.join("daemon.log"),
        }
    }

    #[test]
    fn write_message_parity() {
        replay("client/writeMessage", |case: &Case| -> Result<String, String> {
            let message = tags::untag(case.arg(0));
            // Protocol messages go through the typed request (its key order is the wire order); anything else
            // through the same stringifier.
            let line = match serde_json::from_value::<Request>(message.clone()) {
                Ok(request) => frame_message(&request),
                Err(_) => frame(&message),
            }
            .map_err(|error| error.to_string())?;
            String::from_utf8(line).map_err(|error| error.to_string())
        });
    }

    #[test]
    fn attach_json_parser_parity() {
        replay("client/attachJsonParser", |case: &Case| -> Result<Value, String> {
            let chunks = case.arg(0).as_array().cloned().unwrap_or_default();
            let detach_after = if case.arg_is_undefined(1) {
                None
            } else {
                Some(case.arg_usize(1)?)
            };
            let mut parser = JsonParser::new();
            let mut messages = Vec::new();
            let mut error = None;
            let mut detached = false;
            for (index, chunk) in chunks.iter().enumerate() {
                let bytes = match (chunk.get("text"), chunk.get("bytes")) {
                    (Some(text), _) => tags::untag(text).as_str().unwrap_or_default().as_bytes().to_vec(),
                    (_, Some(bytes)) => tags::base64_decode(bytes["$bytes"].as_str().unwrap_or_default())?,
                    _ => return Err(format!("unknown chunk {chunk}")),
                };
                let parsed = parser.push_values(&bytes);
                messages.extend(parsed.messages);
                if let Some(thrown) = parsed.error {
                    error = Some(thrown.to_string());
                    break;
                }
                if detach_after == Some(index) {
                    detached = true;
                    break;
                }
            }
            let listeners = if detached { 0 } else { 1 };
            Ok(match error {
                Some(error) => json!({"messages": messages, "error": error, "listeners": listeners}),
                None => json!({"messages": messages, "listeners": listeners}),
            })
        });
    }

    #[test]
    fn numbers_are_written_as_js_writes_them() {
        assert_eq!(js_number(1e21), "1e+21");
        assert_eq!(js_number(1e20), "100000000000000000000");
        assert_eq!(js_number(-0.0), "0");
        assert_eq!(js_number(2.5), "2.5");
        assert_eq!(js_number(100.0), "100");
        assert_eq!(js_number(1e-7), "1e-7");
        assert_eq!(js_number(0.000001), "0.000001");
        assert_eq!(js_number(-1.5e300), "-1.5e+300");
        assert_eq!(
            String::from_utf8(frame(&json!({"n": [1.0, 1e21, 0.5]})).unwrap()).unwrap(),
            "{\"n\":[1,1e+21,0.5]}\n"
        );
    }

    #[test]
    fn js_string_matches_string_conversion() {
        assert_eq!(js_string(&json!(45)), "45");
        assert_eq!(js_string(&json!(4.5)), "4.5");
        assert_eq!(js_string(&json!(null)), "null");
        assert_eq!(js_string(&json!("46")), "46");
        assert_eq!(js_string(&json!([1, null, "a"])), "1,,a");
        assert_eq!(js_string(&json!({"a": 1})), "[object Object]");
    }

    #[test]
    fn pid_file_is_read_with_parse_int_semantics() {
        assert_eq!(parse_pid("123\n"), Some(123));
        assert_eq!(parse_pid("  42abc"), Some(42));
        assert_eq!(parse_pid("+7"), Some(7));
        assert_eq!(parse_pid("abc"), None);
        assert_eq!(parse_pid(""), None);
        assert_eq!(parse_pid("0"), None);
        assert_eq!(parse_pid("-5"), None);
        assert_eq!(parse_pid("99999999999"), None);
    }

    #[test]
    fn process_liveness_uses_signal_zero() {
        let own = i32::try_from(std::process::id()).unwrap();
        assert!(is_process_alive(Some(own)));
        assert!(!is_process_alive(None));
        assert!(!is_process_alive(Some(0)));
        assert!(!is_process_alive(Some(i32::MAX)));
    }

    #[test]
    fn ping_version_must_match_the_protocol() {
        assert_eq!(check_ping_version(Some(&json!({"ok": true, "version": 49}))), Ok(()));
        assert_eq!(
            check_ping_version(Some(&json!({"ok": true, "version": 45}))),
            Err(PingError::ProtocolMismatch(String::from(
                "daemon protocol mismatch: expected v49, got 45"
            )))
        );
        assert_eq!(
            check_ping_version(Some(&json!({"ok": true}))).unwrap_err().to_string(),
            "daemon protocol mismatch: expected v49, got undefined"
        );
        assert_eq!(
            check_ping_version(Some(&json!({"version": "49"})))
                .unwrap_err()
                .to_string(),
            "daemon protocol mismatch: expected v49, got 49"
        );
        assert!(matches!(
            check_ping_version(Some(&Value::Null)),
            Err(PingError::Failed(_))
        ));
    }

    #[test]
    fn parser_keeps_lines_after_a_bad_one_for_the_next_chunk() {
        let mut parser = JsonParser::new();
        let parsed = parser.push_values(b"{\"a\":1}\nnope\n{\"b\":2}\n");
        assert_eq!(parsed.messages, vec![json!({"a": 1})]);
        assert_eq!(
            parsed.error.unwrap().to_string(),
            "Unexpected token 'o', \"nope\" is not valid JSON"
        );
        let parsed = parser.push_values(b"{\"c\":3}\n");
        assert_eq!(parsed.messages, vec![json!({"b": 2}), json!({"c": 3})]);
        assert!(parsed.error.is_none());
    }

    #[test]
    fn parser_accepts_lone_surrogate_escapes_as_json_parse_does() {
        // JSON.stringify of a string cut through an emoji by UTF-16 index escapes the half; JSON.parse takes it back.
        let mut parser = JsonParser::new();
        let parsed = parser.push_values(
            b"{\"type\":\"session-removed\",\"sessionId\":\"ab\\ud83d\"}\n[\"\\udc00\\ud83d\\ude00\",\"\\ud83dx\\\\ud83d\"]\n",
        );
        assert!(parsed.error.is_none(), "{:?}", parsed.error);
        // The half is U+FFFD, as Node writes it (one UTF-16 unit either way).
        assert_eq!(
            parsed.messages,
            vec![
                json!({"type": "session-removed", "sessionId": "ab\u{fffd}"}),
                json!(["\u{fffd}\u{1f600}", "\u{fffd}x\\ud83d"]),
            ]
        );
        let messages = JsonParser::new()
            .push(b"{\"type\":\"session-removed\",\"sessionId\":\"ab\\ud83d\"}\n")
            .unwrap();
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn parser_accepts_numbers_beyond_f64_as_json_parse_does() {
        let mut parser = JsonParser::new();
        let parsed =
            parser.push_values(b"{\"type\":\"x\",\"n\":1e400,\"m\":[-1e400,1e-400,12345678901234567890123,7]}\n");
        assert!(parsed.error.is_none(), "{:?}", parsed.error);
        // ±Infinity has no JSON value: it is held as null, which is also what JSON.stringify (the wire log) writes.
        assert_eq!(
            parsed.messages,
            vec![json!({"type": "x", "n": null, "m": [null, 0.0, 1.2345678901234568e22, 7]})]
        );
    }

    #[test]
    fn parser_accepts_nesting_deeper_than_serde_json_does() {
        let depth = 500;
        let mut line = format!("{{\"type\":\"x\",\"deep\":{}{}}}", "[".repeat(depth), "]".repeat(depth));
        line.push_str("\n{\"a\":{\"b\":[1,{\"c\":[]},{}],\"a\":2},\"a\":3}\n");
        let mut parser = JsonParser::new();
        let parsed = parser.push_values(line.as_bytes());
        assert!(parsed.error.is_none(), "{:?}", parsed.error);
        assert_eq!(parsed.messages.len(), 2);
        let mut level = &parsed.messages[0]["deep"];
        let mut levels = 0;
        while let Some(items) = level.as_array() {
            levels += 1;
            match items.first() {
                Some(next) => level = next,
                None => break,
            }
        }
        assert_eq!(levels, depth);
        // A repeated key keeps its first position and takes the last value, as JSON.parse does.
        assert_eq!(parsed.messages[1], json!({"a": 3}));
        assert_eq!(serde_json::to_string(&parsed.messages[1]).unwrap(), "{\"a\":3}");
    }

    #[test]
    fn parser_depth_cap_keeps_values_safe_on_a_small_stack() {
        // At the cap a value still drops, clones, serializes (the wire log) and converts (the typed parser) on a
        // 2 MiB stack (a spawned thread's default) in a debug build; one level more fails with V8's stack-overflow
        // message, as does a deeper line that is not JSON at all (the recursive message scanner never sees it).
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(|| {
                for line in [
                    format!(
                        "{{\"type\":\"git-updated\",\"git\":{}{}}}",
                        "[".repeat(MAX_JSON_DEPTH - 1),
                        "]".repeat(MAX_JSON_DEPTH - 1)
                    ),
                    format!(
                        "{{\"type\":\"session-updated\",\"session\":{{\"id\":\"s\",\"x\":{}{}}}}}",
                        "[".repeat(MAX_JSON_DEPTH - 2),
                        "]".repeat(MAX_JSON_DEPTH - 2)
                    ),
                ] {
                    let value = json_parse(&line).unwrap();
                    let copy = value.clone();
                    assert_eq!(json_stringify(&copy).unwrap(), line.into_bytes());
                    drop(copy);
                    let _ = to_server_message(value);
                }
                let over = format!("{}0{}", "[".repeat(MAX_JSON_DEPTH + 1), "]".repeat(MAX_JSON_DEPTH + 1));
                assert_eq!(
                    json_parse(&over).unwrap_err().to_string(),
                    "Maximum call stack size exceeded"
                );
                let invalid = format!("{}x", "[".repeat(1_000_000));
                assert_eq!(
                    json_parse(&invalid).unwrap_err().to_string(),
                    "Maximum call stack size exceeded"
                );
                // Within the cap, an invalid line still gets JSON.parse's message.
                assert_eq!(
                    json_parse("[[1,]]").unwrap_err().to_string(),
                    "Unexpected token ']', \"[[1,]]\" is not valid JSON"
                );
                assert_eq!(
                    json_parse("[01]").unwrap_err().to_string(),
                    "Unexpected number in JSON at position 2 (line 1 column 3)"
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn parser_fallback_keeps_object_key_order_and_rejects_what_v8_rejects() {
        let mut parser = JsonParser::new();
        let parsed =
            parser.push_values(b" {\"z\":\"\\ud800\",\"a\":[true,false,null,-0.5e1,\"\\n\\u00e9\\\"\"]} \n[1e400,\n");
        assert_eq!(
            serde_json::to_string(&parsed.messages).unwrap(),
            "[{\"z\":\"\u{fffd}\",\"a\":[true,false,null,-5.0,\"\\n\u{e9}\\\"\"]}]"
        );
        assert_eq!(parsed.error.unwrap().to_string(), "Unexpected end of JSON input");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn request_resolves_through_lines_serde_json_rejects() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("daemon.sock");
        serve(&socket, |request| {
            Some(format!(
                "{{\"type\":\"session-removed\",\"sessionId\":\"ab\\ud83d\",\"n\":1e400}}\n{}",
                response(&request, json!({"ok": true, "data": {"x": 1}}))
                    .replace("\"x\":1", "\"x\":1,\"t\":\"\\ud83d\"")
            ))
        });
        let data: Value = request_at(&socket, &ping_request(), None).await.unwrap();
        assert_eq!(data, json!({"x": 1, "t": "\u{fffd}"}));
    }

    #[test]
    fn typed_parser_skips_unknown_message_types() {
        let mut parser = JsonParser::new();
        let messages = parser
            .push(b"{\"type\":\"nope\"}\n{\"type\":\"attached\",\"sessionId\":\"s\"}\n")
            .unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].type_name(), "attached");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn message_reader_reads_until_the_daemon_closes() {
        let (client, mut server) = UnixStream::pair().unwrap();
        server
            .write_all(b"{\"type\":\"detached\",\"sessionId\":\"a\"}\n{\"type\":\"output\",\"sessionId\":\"a\",")
            .await
            .unwrap();
        server.write_all(b"\"data\":\"x\"}\n").await.unwrap();
        drop(server);
        let mut reader = MessageReader::new(client);
        assert_eq!(reader.next().await.unwrap().unwrap().type_name(), "detached");
        assert_eq!(reader.next().await.unwrap().unwrap().type_name(), "output");
        assert!(reader.next().await.unwrap().is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn request_resolves_with_the_matching_response_data() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("daemon.sock");
        serve(&socket, |request| {
            Some(format!(
                "{}{}\n{}",
                response(&json!({"requestId": "other"}), json!({"ok": true, "data": 1})),
                json!({"type": "session-removed", "sessionId": "s"}),
                response(&request, json!({"ok": true, "data": {"x": 1}}))
            ))
        });
        let data: Value = request_at(&socket, &ping_request(), None).await.unwrap();
        assert_eq!(data, json!({"x": 1}));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn request_rejects_failed_responses_with_their_error() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("daemon.sock");
        serve(&socket, |request| {
            let body = if request["requestId"] == "r1" {
                json!({"ok": false, "error": "boom"})
            } else {
                json!({"ok": false, "error": ""})
            };
            Some(response(&request, body))
        });
        let error = request_at::<Value>(&socket, &ping_request(), None).await.unwrap_err();
        assert_eq!(error.to_string(), "boom");
        let other = Request::List(deckhand_protocol::wire::RequestOnly {
            request_id: String::from("r2"),
        });
        let error = request_at::<Value>(&socket, &other, None).await.unwrap_err();
        assert_eq!(error.to_string(), "daemon request failed");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn request_fails_when_the_daemon_closes_first() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("daemon.sock");
        serve(&socket, |_| None);
        let error = request_at::<Value>(&socket, &ping_request(), None).await.unwrap_err();
        assert_eq!(error.to_string(), "daemon connection closed before response");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn request_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("daemon.sock");
        serve(&socket, |_| Some(String::new()));
        let error = request_at::<Value>(&socket, &ping_request(), Some(100))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "daemon request timed out");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn request_without_a_daemon_reports_the_connect_error() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("daemon.sock");
        let error = request_at::<Value>(&socket, &ping_request(), None).await.unwrap_err();
        assert_eq!(error.to_string(), format!("connect ENOENT {}", socket.display()));
        assert!(error.is_not_found());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ensure_daemon_running_accepts_a_daemon_that_answers() {
        let dir = tempfile::tempdir().unwrap();
        let files = files(dir.path());
        serve(&files.socket, |request| {
            Some(response(
                &request,
                json!({"ok": true, "data": {"ok": true, "version": 49}}),
            ))
        });
        ensure_daemon_running_with(&files, || panic!("must not spawn"))
            .await
            .unwrap();
        assert!(!files.log.exists());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ensure_daemon_running_refuses_to_replace_a_live_daemon_of_another_protocol() {
        let dir = tempfile::tempdir().unwrap();
        let files = files(dir.path());
        let pid = std::process::id();
        std::fs::write(&files.pid, format!("{pid}\n")).unwrap();
        serve(&files.socket, |request| {
            Some(response(
                &request,
                json!({"ok": true, "data": {"ok": true, "version": 45}}),
            ))
        });
        let error = ensure_daemon_running_with(&files, || panic!("must not spawn"))
            .await
            .unwrap_err();
        let hint = format!(
            "stop the old daemon first: kill {pid} (or kill $(cat {}))",
            files.pid.display()
        );
        assert_eq!(
            error.to_string(),
            format!("daemon protocol mismatch: expected v49, got 45; {hint}")
        );
        let log = std::fs::read_to_string(&files.log).unwrap();
        assert!(log.starts_with('['), "{log}");
        assert!(
            log.ends_with(&format!(
                "] client refusing to replace live daemon pid {pid} after protocol mismatch: daemon protocol \
                 mismatch: expected v49, got 45; {hint}\n"
            )),
            "{log}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ensure_daemon_running_starts_a_daemon_when_none_answers() {
        let dir = tempfile::tempdir().unwrap();
        let files = files(dir.path());
        // A stale socket (its daemon is gone) and a pid that is not running.
        drop(UnixListener::bind(&files.socket).unwrap());
        std::fs::write(&files.pid, format!("{}\n", i32::MAX)).unwrap();
        let spawned = Arc::new(AtomicBool::new(false));
        let socket = files.socket.clone();
        let flag = Arc::clone(&spawned);
        ensure_daemon_running_with(&files, move || {
            assert!(!socket.exists(), "the stale socket is removed first");
            flag.store(true, Ordering::SeqCst);
            serve(&socket, |request| {
                Some(response(
                    &request,
                    json!({"ok": true, "data": {"ok": true, "version": 49}}),
                ))
            });
            Ok(())
        })
        .await
        .unwrap();
        assert!(spawned.load(Ordering::SeqCst));
        let log = std::fs::read_to_string(&files.log).unwrap();
        assert!(
            log.ends_with(&format!(
                "] client starting daemon after ping failure: connect ECONNREFUSED {}\n",
                files.socket.display()
            )),
            "{log}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ensure_daemon_running_gives_a_live_unresponsive_daemon_two_seconds() {
        let dir = tempfile::tempdir().unwrap();
        let files = files(dir.path());
        let pid = std::process::id();
        std::fs::write(&files.pid, pid.to_string()).unwrap();
        let started = Instant::now();
        let error = ensure_daemon_running_with(&files, || panic!("must not spawn"))
            .await
            .unwrap_err();
        assert!(started.elapsed() >= Duration::from_millis(DAEMON_RETRY_DEADLINE_MS));
        assert_eq!(
            error.to_string(),
            format!(
                "daemon pid {pid} is alive but did not respond; see {}: failed to start daemon: connect ENOENT {}",
                files.log.display(),
                files.socket.display()
            )
        );
        let log = std::fs::read_to_string(&files.log).unwrap();
        assert!(log.contains(&format!(
            "client ping failed while daemon pid {pid} is still alive; retrying before replacement: connect ENOENT"
        )));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn wait_for_daemon_reports_the_last_ping_error() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("daemon.sock");
        let error = wait_for_daemon_at(&socket, 200).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("failed to start daemon: connect ENOENT {}", socket.display())
        );
        let error = wait_for_daemon_at(&socket, 0).await.unwrap_err();
        assert_eq!(error.to_string(), "failed to start daemon: undefined");
    }
}
