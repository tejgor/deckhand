//! `DhError`, Deckhand's error type.
//!
//! TS code throws `new Error(message)` and tests (and users) see `error.message`, so [`DhError`]'s `Display` is
//! exactly that message. Some TS errors also carry a Node-style `code` (`ENOENT` from fs, or one set with
//! `Object.assign(error, {code})`), which callers branch on; [`DhError::code`] keeps it.
//!
//! I/O errors: Node's fs messages are `<CODE>: <libuv description>, <syscall> '<path>'` (for example
//! `ENOENT: no such file or directory, open '/x/deckhand.json'`). A `std::io::Error` knows neither the syscall nor the
//! path, so:
//! - [`DhError::io`] / [`DhError::io_dest`] build the exact Node message: use them wherever the TS let an fs error's
//!   message through (it reaches the user, or a test asserts on it);
//! - `From<std::io::Error>` (for `?`) keeps the code and writes `<CODE>: <description>` without the syscall part;
//!   errors without an errno (e.g. a custom `io::Error`) keep their own text and get no code.
//!
//! JSON: [`DhError::json_parse`] reproduces V8's `JSON.parse` SyntaxError messages (validated against Node in
//! `tests/data/json_parse_errors.json`); `From<serde_json::Error>` keeps serde's own text for the cases where the TS
//! message never surfaces.

/// `Result` with [`DhError`].
pub type Result<T, E = DhError> = std::result::Result<T, E>;

/// A TS `Error`: `Display` is its `message`, verbatim.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DhError {
    /// `new Error(message)`.
    #[error("{0}")]
    Msg(String),
    /// An error with a Node-style `code` (a system error, or `Object.assign(new Error(message), {code})`).
    #[error("{message}")]
    Coded { code: String, message: String },
}

impl DhError {
    /// `new Error(message)`.
    pub fn msg(message: impl Into<String>) -> Self {
        DhError::Msg(message.into())
    }

    /// `Object.assign(new Error(message), {code})`.
    pub fn with_code(code: impl Into<String>, message: impl Into<String>) -> Self {
        DhError::Coded {
            code: code.into(),
            message: message.into(),
        }
    }

    /// `error.message`.
    pub fn message(&self) -> &str {
        match self {
            DhError::Msg(message) => message,
            DhError::Coded { message, .. } => message,
        }
    }

    /// `error.code`, when it has one.
    pub fn code(&self) -> Option<&str> {
        match self {
            DhError::Msg(_) => None,
            DhError::Coded { code, .. } => Some(code),
        }
    }

    /// `error.code === code`.
    pub fn is_code(&self, code: &str) -> bool {
        self.code() == Some(code)
    }

    /// A missing file or directory (`error.code === 'ENOENT'`).
    pub fn is_not_found(&self) -> bool {
        self.is_code("ENOENT")
    }

    /// `new Error(`${prefix}${error.message}`)`: a new error (without the code, as in TS) whose message is prefixed.
    pub fn prefixed(&self, prefix: &str) -> Self {
        DhError::Msg(format!("{prefix}{}", self.message()))
    }

    /// Node's fs error for `syscall` on `path`: `ENOENT: no such file or directory, open '/x'`.
    pub fn io(error: &std::io::Error, syscall: &str, path: &str) -> Self {
        match errno_code(error) {
            Some(code) => DhError::Coded {
                code: code.to_owned(),
                message: format!("{code}: {}, {syscall} '{path}'", errno_description(code)),
            },
            None => DhError::Msg(error.to_string()),
        }
    }

    /// Node's fs error for a two-path syscall (`rename`, `link`, `symlink`, `copyfile`): `EXDEV: cross-device link not
    /// permitted, rename '/a' -> '/b'`.
    pub fn io_dest(error: &std::io::Error, syscall: &str, path: &str, dest: &str) -> Self {
        match errno_code(error) {
            Some(code) => DhError::Coded {
                code: code.to_owned(),
                message: format!("{code}: {}, {syscall} '{path}' -> '{dest}'", errno_description(code)),
            },
            None => DhError::Msg(error.to_string()),
        }
    }

    /// The SyntaxError `JSON.parse(text)` throws, for text serde_json rejected. Should V8 accept `text` after all,
    /// the message is V8's for an unexpected end (never reached for real inputs: the grammars agree).
    pub fn json_parse(text: &str) -> Self {
        DhError::Msg(json_parse_message(text).unwrap_or_else(|| String::from("Unexpected end of JSON input")))
    }
}

impl From<std::io::Error> for DhError {
    fn from(error: std::io::Error) -> Self {
        match errno_code(&error) {
            Some(code) => DhError::Coded {
                code: code.to_owned(),
                message: format!("{code}: {}", errno_description(code)),
            },
            None => DhError::Msg(error.to_string()),
        }
    }
}

impl From<serde_json::Error> for DhError {
    fn from(error: serde_json::Error) -> Self {
        DhError::Msg(error.to_string())
    }
}

/// The Node error code (`ENOENT`, ...) of an OS error.
pub fn errno_code(error: &std::io::Error) -> Option<&'static str> {
    let errno = error.raw_os_error()?;
    ERRNO_CODES
        .iter()
        .find(|(number, _)| *number == errno)
        .map(|(_, code)| *code)
}

/// libuv's description of an error code (`uv_strerror`), as Node puts it in fs error messages.
pub fn errno_description(code: &str) -> &'static str {
    match code {
        "E2BIG" => "argument list too long",
        "EACCES" => "permission denied",
        "EADDRINUSE" => "address already in use",
        "EADDRNOTAVAIL" => "address not available",
        "EAGAIN" => "resource temporarily unavailable",
        "EBADF" => "bad file descriptor",
        "EBUSY" => "resource busy or locked",
        "ECONNREFUSED" => "connection refused",
        "ECONNRESET" => "connection reset by peer",
        "EEXIST" => "file already exists",
        "EFBIG" => "file too large",
        "EINTR" => "interrupted system call",
        "EINVAL" => "invalid argument",
        "EIO" => "i/o error",
        "EISDIR" => "illegal operation on a directory",
        "ELOOP" => "too many symbolic links encountered",
        "EMFILE" => "too many open files",
        "EMLINK" => "too many links",
        "ENAMETOOLONG" => "name too long",
        "ENFILE" => "file table overflow",
        "ENODEV" => "no such device",
        "ENOENT" => "no such file or directory",
        "ENOMEM" => "not enough memory",
        "ENOSPC" => "no space left on device",
        "ENOSYS" => "function not implemented",
        "ENOTCONN" => "socket is not connected",
        "ENOTDIR" => "not a directory",
        "ENOTEMPTY" => "directory not empty",
        "ENOTSOCK" => "socket operation on non-socket",
        "ENOTSUP" => "operation not supported on socket",
        "ENXIO" => "no such device or address",
        "EPERM" => "operation not permitted",
        "EPIPE" => "broken pipe",
        "ERANGE" => "result too large",
        "EROFS" => "read-only file system",
        "ESPIPE" => "invalid seek",
        "ESRCH" => "no such process",
        "ETIMEDOUT" => "connection timed out",
        "ETXTBSY" => "text file is busy",
        "EXDEV" => "cross-device link not permitted",
        _ => "unknown error",
    }
}

/// errno numbers per platform (deckhand runs on macOS and Linux).
#[cfg(target_os = "macos")]
const ERRNO_CODES: &[(i32, &str)] = &[
    (1, "EPERM"),
    (2, "ENOENT"),
    (3, "ESRCH"),
    (4, "EINTR"),
    (5, "EIO"),
    (6, "ENXIO"),
    (7, "E2BIG"),
    (9, "EBADF"),
    (12, "ENOMEM"),
    (13, "EACCES"),
    (16, "EBUSY"),
    (17, "EEXIST"),
    (18, "EXDEV"),
    (19, "ENODEV"),
    (20, "ENOTDIR"),
    (21, "EISDIR"),
    (22, "EINVAL"),
    (23, "ENFILE"),
    (24, "EMFILE"),
    (26, "ETXTBSY"),
    (27, "EFBIG"),
    (28, "ENOSPC"),
    (29, "ESPIPE"),
    (30, "EROFS"),
    (31, "EMLINK"),
    (32, "EPIPE"),
    (34, "ERANGE"),
    (35, "EAGAIN"),
    (38, "ENOTSOCK"),
    (45, "ENOTSUP"),
    (48, "EADDRINUSE"),
    (49, "EADDRNOTAVAIL"),
    (54, "ECONNRESET"),
    (57, "ENOTCONN"),
    (60, "ETIMEDOUT"),
    (61, "ECONNREFUSED"),
    (62, "ELOOP"),
    (63, "ENAMETOOLONG"),
    (66, "ENOTEMPTY"),
    (78, "ENOSYS"),
];

/// errno numbers per platform (deckhand runs on macOS and Linux).
#[cfg(not(target_os = "macos"))]
const ERRNO_CODES: &[(i32, &str)] = &[
    (1, "EPERM"),
    (2, "ENOENT"),
    (3, "ESRCH"),
    (4, "EINTR"),
    (5, "EIO"),
    (6, "ENXIO"),
    (7, "E2BIG"),
    (9, "EBADF"),
    (11, "EAGAIN"),
    (12, "ENOMEM"),
    (13, "EACCES"),
    (16, "EBUSY"),
    (17, "EEXIST"),
    (18, "EXDEV"),
    (19, "ENODEV"),
    (20, "ENOTDIR"),
    (21, "EISDIR"),
    (22, "EINVAL"),
    (23, "ENFILE"),
    (24, "EMFILE"),
    (26, "ETXTBSY"),
    (27, "EFBIG"),
    (28, "ENOSPC"),
    (29, "ESPIPE"),
    (30, "EROFS"),
    (31, "EMLINK"),
    (32, "EPIPE"),
    (34, "ERANGE"),
    (36, "ENAMETOOLONG"),
    (38, "ENOSYS"),
    (39, "ENOTEMPTY"),
    (40, "ELOOP"),
    (88, "ENOTSOCK"),
    (95, "ENOTSUP"),
    (98, "EADDRINUSE"),
    (99, "EADDRNOTAVAIL"),
    (104, "ECONNRESET"),
    (107, "ENOTCONN"),
    (110, "ETIMEDOUT"),
    (111, "ECONNREFUSED"),
];

// ---------------------------------------------------------------------------------------------------------------
// JSON.parse messages (V8's json-parser.cc)

/// V8's longest context on each side of an unexpected token, and the source length from which it cuts context.
const MAX_CONTEXT: usize = 10;
const MIN_LENGTH_FOR_CONTEXT: usize = MAX_CONTEXT * 2 + 1;

/// The message `JSON.parse(text)` throws, or `None` when V8 accepts `text`.
pub fn json_parse_message(text: &str) -> Option<String> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut parser = JsonScanner { units: &units, pos: 0 };
    parser.document().err().map(|error| error.message(&units))
}

/// Why V8 stopped, and where (a UTF-16 index).
#[derive(Clone, Copy, Debug)]
enum JsonFault {
    /// An unexpected token with no more specific message (`Unexpected token 'x', ...`, `Unexpected number`, ...).
    Token(usize),
    /// A specific message (template with `%` for the position).
    At(&'static str, usize),
}

impl JsonFault {
    fn message(self, units: &[u16]) -> String {
        match self {
            JsonFault::At(template, pos) => positioned(template, units, pos),
            JsonFault::Token(pos) => {
                let Some(&unit) = units.get(pos) else {
                    return String::from("Unexpected end of JSON input");
                };
                if unit == u16::from(b'-') || (u16::from(b'0')..=u16::from(b'9')).contains(&unit) {
                    return positioned("Unexpected number in JSON at position %", units, pos);
                }
                if unit == u16::from(b'"') {
                    return positioned("Unexpected string in JSON at position %", units, pos);
                }
                let source = String::from_utf16_lossy(units);
                if matches!(source.as_str(), "undefined" | "NaN" | "Infinity" | "[object Object]") {
                    return format!("\"{source}\" is not valid JSON");
                }
                let token = String::from_utf16_lossy(&[unit]);
                let len = units.len();
                if len < MIN_LENGTH_FOR_CONTEXT {
                    return format!("Unexpected token '{token}', \"{source}\" is not valid JSON");
                }
                let slice = |from: usize, to: usize| String::from_utf16_lossy(&units[from..to.min(len)]);
                if pos < MAX_CONTEXT {
                    format!(
                        "Unexpected token '{token}', \"{}\"... is not valid JSON",
                        slice(0, pos + MAX_CONTEXT)
                    )
                } else if pos < len - MAX_CONTEXT {
                    let context = slice(pos - MAX_CONTEXT, pos + MAX_CONTEXT);
                    format!("Unexpected token '{token}', ...\"{context}\"... is not valid JSON")
                } else {
                    format!(
                        "Unexpected token '{token}', ...\"{}\" is not valid JSON",
                        slice(pos - MAX_CONTEXT, len)
                    )
                }
            }
        }
    }
}

/// `template` with `%` replaced by the position, then V8's ` (line L column C)`; `\r`, `\n` and `\r\n` end lines.
fn positioned(template: &str, units: &[u16], pos: usize) -> String {
    let (cr, lf) = (u16::from(b'\r'), u16::from(b'\n'));
    let end = pos.min(units.len());
    let mut line = 1;
    let mut line_start = 0;
    let mut index = 0;
    while index < end {
        // `\r\n` counts as one line break.
        if units[index] == cr && index + 1 < end && units[index + 1] == lf {
            index += 1;
        }
        if units[index] == cr || units[index] == lf {
            line += 1;
            line_start = index + 1;
        }
        index += 1;
    }
    let column = 1 + end.saturating_sub(line_start);
    format!(
        "{} (line {line} column {column})",
        template.replace('%', &pos.to_string())
    )
}

struct JsonScanner<'a> {
    units: &'a [u16],
    pos: usize,
}

type Scan = std::result::Result<(), JsonFault>;

/// A container the scanner is inside of.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Container {
    Object,
    Array,
}

impl JsonScanner<'_> {
    fn peek(&self) -> Option<u16> {
        self.units.get(self.pos).copied()
    }

    fn is(&self, ch: u8) -> bool {
        self.peek() == Some(u16::from(ch))
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(0x20 | 0x09 | 0x0A | 0x0D)) {
            self.pos += 1;
        }
    }

    fn digit(&self) -> bool {
        matches!(self.peek(), Some(unit) if (0x30..=0x39).contains(&unit))
    }

    fn document(&mut self) -> Scan {
        self.skip_whitespace();
        self.value()?;
        self.skip_whitespace();
        if self.peek().is_some() {
            return Err(JsonFault::At(
                "Unexpected non-whitespace character after JSON at position %",
                self.pos,
            ));
        }
        Ok(())
    }

    /// One value, iteratively: open containers live on an explicit stack, so any nesting depth is scanned (as V8's
    /// JSON.parse does) without growing the call stack.
    fn value(&mut self) -> Scan {
        let mut open: Vec<Container> = Vec::new();
        'value: loop {
            self.skip_whitespace();
            match self.peek().and_then(|unit| u8::try_from(unit).ok()) {
                Some(b'"') => self.string()?,
                Some(b'-' | b'0'..=b'9') => self.number()?,
                Some(b't') => self.literal("true")?,
                Some(b'f') => self.literal("false")?,
                Some(b'n') => self.literal("null")?,
                Some(b'{') => {
                    self.pos += 1;
                    self.skip_whitespace();
                    if self.is(b'}') {
                        self.pos += 1;
                    } else {
                        self.property_name(true)?;
                        open.push(Container::Object);
                        continue 'value;
                    }
                }
                Some(b'[') => {
                    self.pos += 1;
                    self.skip_whitespace();
                    if self.is(b']') {
                        self.pos += 1;
                    } else {
                        open.push(Container::Array);
                        continue 'value;
                    }
                }
                _ => return Err(JsonFault::Token(self.pos)),
            }
            // A value ended: continue its container, closing every container it completes.
            while let Some(&container) = open.last() {
                self.skip_whitespace();
                if self.is(b',') {
                    self.pos += 1;
                    if container == Container::Object {
                        self.property_name(false)?;
                    }
                    continue 'value;
                }
                let (close, message) = match container {
                    Container::Object => (b'}', "Expected ',' or '}' after property value in JSON at position %"),
                    Container::Array => (b']', "Expected ',' or ']' after array element in JSON at position %"),
                };
                if !self.is(close) {
                    return Err(JsonFault::At(message, self.pos));
                }
                self.pos += 1;
                open.pop();
            }
            return Ok(());
        }
    }

    /// `"name" :` inside an object (`first`: right after `{`, where V8 also accepts `}`).
    fn property_name(&mut self, first: bool) -> Scan {
        self.skip_whitespace();
        if !self.is(b'"') {
            let message = if first {
                "Expected property name or '}' in JSON at position %"
            } else {
                "Expected double-quoted property name in JSON at position %"
            };
            return Err(JsonFault::At(message, self.pos));
        }
        self.string()?;
        self.skip_whitespace();
        if !self.is(b':') {
            return Err(JsonFault::At(
                "Expected ':' after property name in JSON at position %",
                self.pos,
            ));
        }
        self.pos += 1;
        Ok(())
    }

    fn literal(&mut self, word: &str) -> Scan {
        for expected in word.bytes() {
            if self.peek() != Some(u16::from(expected)) {
                return Err(JsonFault::Token(self.pos));
            }
            self.pos += 1;
        }
        Ok(())
    }

    fn number(&mut self) -> Scan {
        if self.is(b'-') {
            self.pos += 1;
            if !self.digit() {
                return Err(JsonFault::At(
                    "No number after minus sign in JSON at position %",
                    self.pos,
                ));
            }
        }
        if self.is(b'0') {
            self.pos += 1;
            if self.digit() {
                return Err(JsonFault::Token(self.pos));
            }
        } else {
            while self.digit() {
                self.pos += 1;
            }
        }
        if self.is(b'.') {
            self.pos += 1;
            if !self.digit() {
                return Err(JsonFault::At(
                    "Unterminated fractional number in JSON at position %",
                    self.pos,
                ));
            }
            while self.digit() {
                self.pos += 1;
            }
        }
        if self.is(b'e') || self.is(b'E') {
            self.pos += 1;
            if self.is(b'+') || self.is(b'-') {
                self.pos += 1;
            }
            if !self.digit() {
                return Err(JsonFault::At(
                    "Exponent part is missing a number in JSON at position %",
                    self.pos,
                ));
            }
            while self.digit() {
                self.pos += 1;
            }
        }
        Ok(())
    }

    fn string(&mut self) -> Scan {
        self.pos += 1;
        loop {
            let Some(unit) = self.peek() else {
                return Err(JsonFault::At("Unterminated string in JSON at position %", self.pos));
            };
            if unit == u16::from(b'"') {
                self.pos += 1;
                return Ok(());
            }
            if unit < 0x20 {
                return Err(JsonFault::At(
                    "Bad control character in string literal in JSON at position %",
                    self.pos,
                ));
            }
            if unit != u16::from(b'\\') {
                self.pos += 1;
                continue;
            }
            self.pos += 1;
            match self.peek().and_then(|unit| u8::try_from(unit).ok()) {
                None if self.peek().is_none() => return Err(JsonFault::Token(self.pos)),
                Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => self.pos += 1,
                Some(b'u') => {
                    self.pos += 1;
                    for _ in 0..4 {
                        match self.peek() {
                            Some(unit) if char::from_u32(u32::from(unit)).is_some_and(|ch| ch.is_ascii_hexdigit()) => {
                                self.pos += 1;
                            }
                            // The end of the text too.
                            _ => return Err(JsonFault::At("Bad Unicode escape in JSON at position %", self.pos)),
                        }
                    }
                }
                _ => return Err(JsonFault::At("Bad escaped character in JSON at position %", self.pos)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_the_message() {
        let error = DhError::msg("Unknown worktree setting: x");
        assert_eq!(error.to_string(), "Unknown worktree setting: x");
        assert_eq!(error.code(), None);
        let coded = DhError::with_code("ELOOP", "deckhand.json must not be a symlink");
        assert_eq!(coded.to_string(), "deckhand.json must not be a symlink");
        assert!(coded.is_code("ELOOP"));
        assert_eq!(
            coded.prefixed("/x: ").to_string(),
            "/x: deckhand.json must not be a symlink"
        );
        assert_eq!(coded.prefixed("/x: ").code(), None);
        // Messages keep every character: emoji, CJK, surrogate pairs.
        assert_eq!(DhError::msg("😀 日本 👍🏽").to_string(), "😀 日本 👍🏽");
    }

    #[test]
    fn io_errors_read_like_node() {
        let missing = std::io::Error::from_raw_os_error(2);
        let error = DhError::io(&missing, "open", "/tmp/x/deckhand.json");
        assert_eq!(
            error.to_string(),
            "ENOENT: no such file or directory, open '/tmp/x/deckhand.json'"
        );
        assert!(error.is_not_found());
        let error: DhError = std::io::Error::from_raw_os_error(13).into();
        assert_eq!(error.to_string(), "EACCES: permission denied");
        assert!(error.is_code("EACCES"));
        let exists = std::io::Error::from_raw_os_error(17);
        assert_eq!(
            DhError::io_dest(&exists, "link", "/a/日本", "/b/😀").to_string(),
            "EEXIST: file already exists, link '/a/日本' -> '/b/😀'"
        );
        let custom = std::io::Error::other("worker went away");
        let error: DhError = custom.into();
        assert_eq!(error.to_string(), "worker went away");
        assert_eq!(error.code(), None);
    }

    #[test]
    fn real_io_errors_carry_their_code() {
        let path = "/definitely/not/here/😀";
        let error = std::fs::read(path)
            .map_err(|error| DhError::io(&error, "open", path))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "ENOENT: no such file or directory, open '/definitely/not/here/😀'"
        );
    }

    #[test]
    fn serde_errors_convert() {
        let error: DhError = serde_json::from_str::<serde_json::Value>("{").unwrap_err().into();
        assert!(!error.to_string().is_empty());
        assert_eq!(
            DhError::json_parse("{").to_string(),
            "Expected property name or '}' in JSON at position 1 (line 1 column 2)"
        );
    }

    #[test]
    fn json_parse_message_scans_any_depth() {
        // V8's JSON.parse is iterative: it accepts (and reports errors in) arbitrarily deep input.
        let depth = 1_000_000;
        assert_eq!(
            json_parse_message(&format!("{}{}", "[".repeat(depth), "]".repeat(depth))),
            None
        );
        let objects = format!("{}1{}", "{\"a\":".repeat(depth), "}".repeat(depth));
        assert_eq!(json_parse_message(&objects), None);
        let mixed = format!("{}null{}", "[{\"k\":".repeat(depth), "}]".repeat(depth));
        assert_eq!(json_parse_message(&mixed), None);
        assert_eq!(
            json_parse_message(&format!("{}x", "[".repeat(depth))).as_deref(),
            Some("Unexpected token 'x', ...\"[[[[[[[[[[x\" is not valid JSON")
        );
        assert_eq!(
            json_parse_message(&"[".repeat(depth)).as_deref(),
            Some("Unexpected end of JSON input")
        );
        assert_eq!(
            json_parse_message(&format!("{}{}", "[".repeat(depth), "]".repeat(depth - 1))).as_deref(),
            Some("Expected ',' or ']' after array element in JSON at position 1999999 (line 1 column 2000000)")
        );
    }

    #[test]
    fn json_parse_messages_replay_v8() {
        let cases: Vec<(String, Option<String>)> =
            serde_json::from_str(include_str!("../tests/data/json_parse_errors.json")).expect("fixture parses");
        let mut failures = Vec::new();
        for (text, expected) in &cases {
            let got = json_parse_message(text);
            if &got != expected {
                failures.push(format!("{text:?}\n  node: {expected:?}\n  rust: {got:?}"));
            }
            // serde_json and V8 agree on what is valid JSON.
            let serde_ok = serde_json::from_str::<serde_json::Value>(text).is_ok();
            if serde_ok != expected.is_none() && !text.contains("\\ud83d") {
                failures.push(format!(
                    "{text:?}: serde_json accepts={serde_ok}, node accepts={}",
                    expected.is_none()
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "{} mismatches:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}
