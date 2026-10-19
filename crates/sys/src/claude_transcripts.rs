//! Port of claudeTranscripts.ts.
//!
//! Claude moves to a new conversation ID without exiting: accepting a plan with "clear context", /clear, /resume,
//! /branch. Hooks report the new ID while Deckhand runs, but a hook can be missed; restart then reads Claude's
//! transcripts to find the conversation the session continued in.
//!
//! Transcripts are `<config>/projects/<encoded cwd>/<session id>.jsonl`, one JSON record per line. A conversation
//! started by accepting a plan keeps the planning conversation's `custom-title` record (Deckhand's `--name`), and its
//! first message's `parentUuid` is a record of the planning conversation. Reads are bounded (the first and last 64 KiB
//! of a file, at most 200 candidates).

use std::collections::{HashMap, HashSet};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use deckhand_core::error::{DhError, Result};
use deckhand_core::jsstr::{date, path as jspath};
use serde_json::{Map, Value};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::paths::home_dir;

/// TS: claudeTranscripts.ts HEAD_BYTES.
pub const HEAD_BYTES: usize = 64 * 1024;
/// TS: claudeTranscripts.ts TAIL_BYTES.
pub const TAIL_BYTES: usize = 64 * 1024;
/// TS: claudeTranscripts.ts MAX_CANDIDATES.
pub const MAX_CANDIDATES: usize = 200;

/// Node `path.join(base, ...parts)` (lexical normalisation included); a non-UTF-8 base is joined as is.
fn join(base: &Path, parts: &[&str]) -> PathBuf {
    match base.to_str() {
        Some(base) => {
            let mut all = vec![base];
            all.extend_from_slice(parts);
            PathBuf::from(jspath::join(&all))
        }
        None => parts.iter().fold(base.to_path_buf(), |path, part| path.join(part)),
    }
}

/// TS: claudeTranscripts.ts claudeProjectsDir (`$CLAUDE_CONFIG_DIR/projects`, else `~/.claude/projects`).
/// `claude_config_dir`: the `CLAUDE_CONFIG_DIR` value (TS reads it from `env`, default `process.env`).
pub fn claude_projects_dir(claude_config_dir: Option<&str>) -> PathBuf {
    match claude_config_dir {
        Some(dir) if !dir.is_empty() => PathBuf::from(jspath::join(&[&jspath::resolve(dir), "projects"])),
        _ => join(&home_dir(), &[".claude", "projects"]),
    }
}

/// TS: claudeTranscripts.ts claudeProjectKey (the resolved cwd with every character but an ASCII letter or digit
/// replaced by `-`, per UTF-16 unit as in TS).
pub fn claude_project_key(cwd: &str) -> String {
    jspath::resolve(cwd)
        .chars()
        .flat_map(|ch| {
            let units = if ch.is_ascii_alphanumeric() { 0 } else { ch.len_utf16() };
            let keep = (units == 0).then_some(ch);
            keep.into_iter().chain(std::iter::repeat_n('-', units))
        })
        .collect()
}

/// TS: claudeTranscripts.ts TranscriptHead.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TranscriptHead {
    /// The first `custom-title` (the launch `--name`).
    pub title: Option<String>,
    /// `parentUuid` of the first message: set when the conversation continues another one.
    pub parent_uuid: Option<String>,
    /// The first message's time (epoch ms).
    pub started_at: Option<f64>,
}

/// TS: claudeTranscripts.ts readRange (UTF-8, lossy).
pub async fn read_range(file: &Path, start: u64, length: usize) -> Result<String> {
    let mut handle = tokio::fs::File::open(file)
        .await
        .map_err(|error| DhError::io(&error, "open", &file.to_string_lossy()))?;
    handle.seek(std::io::SeekFrom::Start(start)).await?;
    let mut buffer = vec![0u8; length];
    let mut filled = 0;
    while filled < length {
        let read = handle.read(&mut buffer[filled..]).await?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    buffer.truncate(filled);
    Ok(String::from_utf8_lossy(&buffer).into_owned())
}

/// TS: claudeTranscripts.ts records (the JSON objects among `text`'s lines; lines cut by the byte range skipped).
pub fn records(text: &str) -> Vec<Map<String, Value>> {
    // TS also keeps arrays (typeof 'object'), but no array has the fields read here.
    text.split('\n')
        .filter_map(|line| match js_json_parse(line) {
            Some(Value::Object(record)) => Some(record),
            _ => None,
        })
        .collect()
}

/// A container `js_json_parse` is filling: an array, or an object and the key of the value being parsed.
enum Open {
    Array(Vec<Value>),
    Object(Map<String, Value>, String),
}

/// Containers nested deeper than this are kept as `null` by `js_json_parse`: only top-level fields are read, and a
/// `Value` that deep would overflow the stack when dropped (its `Drop` recurses).
const KEPT_DEPTH: usize = 128;

/// `JSON.parse(text)` as V8 accepts it, where serde_json would refuse: nesting of any depth (parsed without
/// recursion; below [`KEPT_DEPTH`] kept as `null`), numbers out of f64 range (±Infinity in JS; `null` here, as no
/// field read is a number) and lone surrogate escapes (U+FFFD, the jsstr rule). A duplicate key keeps its first
/// position and its last value. `None` where JSON.parse throws.
fn js_json_parse(text: &str) -> Option<Value> {
    let mut scan = JsonScan { text, pos: 0 };
    let mut open: Vec<Open> = Vec::new();
    loop {
        scan.skip_ws();
        let mut value = match scan.peek()? {
            b'{' => {
                scan.pos += 1;
                scan.skip_ws();
                if scan.eat(b'}') {
                    Value::Object(Map::new())
                } else {
                    open.push(Open::Object(Map::new(), scan.key()?));
                    continue;
                }
            }
            b'[' => {
                scan.pos += 1;
                scan.skip_ws();
                if scan.eat(b']') {
                    Value::Array(Vec::new())
                } else {
                    open.push(Open::Array(Vec::new()));
                    continue;
                }
            }
            b'"' => Value::String(scan.string()?),
            b't' => scan.literal("true", Value::Bool(true))?,
            b'f' => scan.literal("false", Value::Bool(false))?,
            b'n' => scan.literal("null", Value::Null)?,
            b'-' | b'0'..=b'9' => scan.number()?,
            _ => return None,
        };
        // Close every container `value` completes; stop at a `,` (the next value follows).
        loop {
            match open.last_mut() {
                None => {
                    scan.skip_ws();
                    return (scan.pos == text.len()).then_some(value);
                }
                Some(Open::Array(items)) => {
                    items.push(value);
                    scan.skip_ws();
                    if scan.eat(b',') {
                        break;
                    }
                    if !scan.eat(b']') {
                        return None;
                    }
                    let Some(Open::Array(items)) = open.pop() else {
                        return None;
                    };
                    value = if open.len() < KEPT_DEPTH {
                        Value::Array(items)
                    } else {
                        Value::Null
                    };
                }
                Some(Open::Object(map, key)) => {
                    map.insert(std::mem::take(key), value);
                    scan.skip_ws();
                    if scan.eat(b',') {
                        scan.skip_ws();
                        *key = scan.key()?;
                        break;
                    }
                    if !scan.eat(b'}') {
                        return None;
                    }
                    let Some(Open::Object(map, _)) = open.pop() else {
                        return None;
                    };
                    value = if open.len() < KEPT_DEPTH {
                        Value::Object(map)
                    } else {
                        Value::Null
                    };
                }
            }
        }
    }
}

struct JsonScan<'a> {
    text: &'a str,
    pos: usize,
}

impl JsonScan<'_> {
    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.pos).copied()
    }

    fn eat(&mut self, byte: u8) -> bool {
        let found = self.peek() == Some(byte);
        if found {
            self.pos += 1;
        }
        found
    }

    /// JSON whitespace: space, tab, LF, CR (not the wider JS set).
    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Option<Value> {
        let found = self.text.get(self.pos..)?.starts_with(word);
        self.pos += word.len();
        found.then_some(value)
    }

    /// A property name and its `:`.
    fn key(&mut self) -> Option<String> {
        if self.peek() != Some(b'"') {
            return None;
        }
        let key = self.string()?;
        self.skip_ws();
        self.eat(b':').then_some(key)
    }

    fn digits(&mut self) -> usize {
        let start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        self.pos - start
    }

    /// `-? (0 | [1-9][0-9]*) (. [0-9]+)? ([eE] [+-]? [0-9]+)?`
    fn number(&mut self) -> Option<Value> {
        let start = self.pos;
        self.eat(b'-');
        if self.eat(b'0') {
            if matches!(self.peek(), Some(b'0'..=b'9')) {
                return None;
            }
        } else if self.digits() == 0 {
            return None;
        }
        let mut integer = true;
        if self.eat(b'.') {
            integer = false;
            if self.digits() == 0 {
                return None;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            integer = false;
            self.pos += 1;
            if !self.eat(b'+') {
                self.eat(b'-');
            }
            if self.digits() == 0 {
                return None;
            }
        }
        let literal = self.text.get(start..self.pos)?;
        if integer
            && literal != "-0"
            && let Ok(whole) = literal.parse::<i64>()
        {
            return Some(Value::from(whole));
        }
        let float = literal.parse::<f64>().ok()?;
        Some(serde_json::Number::from_f64(float).map_or(Value::Null, Value::Number))
    }

    /// Four hex digits of a `\u` escape.
    fn hex4(&mut self) -> Option<u16> {
        let digits = self.text.get(self.pos..self.pos + 4)?;
        if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        self.pos += 4;
        u16::from_str_radix(digits, 16).ok()
    }

    /// A string literal (at its opening quote).
    fn string(&mut self) -> Option<String> {
        self.pos += 1;
        let mut out = String::new();
        let mut run = self.pos;
        loop {
            match self.peek()? {
                b'"' => {
                    out.push_str(self.text.get(run..self.pos)?);
                    self.pos += 1;
                    return Some(out);
                }
                b'\\' => {
                    out.push_str(self.text.get(run..self.pos)?);
                    self.pos += 1;
                    let escape = self.peek()?;
                    self.pos += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let unit = self.hex4()?;
                            out.push(self.unit_char(unit));
                        }
                        _ => return None,
                    }
                    run = self.pos;
                }
                0..=0x1f => return None,
                _ => self.pos += 1,
            }
        }
    }

    /// The character of a `\u` escape: a high surrogate pairs with an immediately following low surrogate escape;
    /// a surrogate left alone is U+FFFD.
    fn unit_char(&mut self, unit: u16) -> char {
        if (0xD800..0xDC00).contains(&unit) && self.text.get(self.pos..self.pos + 2) == Some("\\u") {
            let back = self.pos;
            self.pos += 2;
            if let Some(low) = self.hex4().filter(|low| (0xDC00..0xE000).contains(low)) {
                let code = 0x10000 + ((u32::from(unit) - 0xD800) << 10) + (u32::from(low) - 0xDC00);
                return char::from_u32(code).unwrap_or('\u{FFFD}');
            }
            self.pos = back;
        }
        char::from_u32(u32::from(unit)).unwrap_or('\u{FFFD}')
    }
}

/// TS: claudeTranscripts.ts timeOf (`Date.parse(record.timestamp)` when finite).
pub fn time_of(record: &Map<String, Value>) -> Option<f64> {
    let text = record.get("timestamp")?.as_str()?;
    date::parse(text).map(|ms| ms as f64)
}

fn type_of(record: &Map<String, Value>) -> Option<&str> {
    record.get("type").and_then(Value::as_str)
}

fn is_message(record: &Map<String, Value>) -> bool {
    matches!(type_of(record), Some("user" | "assistant"))
}

/// TS: claudeTranscripts.ts readHead.
pub async fn read_head(file: &Path) -> Result<TranscriptHead> {
    let mut head = TranscriptHead::default();
    let mut saw_message = false;
    for record in records(&read_range(file, 0, HEAD_BYTES).await?) {
        if type_of(&record) == Some("custom-title")
            && head.title.is_none()
            && let Some(Value::String(title)) = record.get("customTitle")
        {
            head.title = Some(title.clone());
        }
        if is_message(&record) && !saw_message {
            saw_message = true;
            if let Some(Value::String(parent)) = record.get("parentUuid") {
                head.parent_uuid = Some(parent.clone());
            }
            head.started_at = time_of(&record);
        }
    }
    Ok(head)
}

/// TS: claudeTranscripts.ts lastActivity (when the conversation last had a message; 0 when none).
/// Not metadata, which Claude re-appends to idle transcripts.
pub async fn last_activity(file: &Path, size: u64) -> Result<f64> {
    let start = size.saturating_sub(TAIL_BYTES as u64);
    let found = records(&read_range(file, start, TAIL_BYTES).await?)
        .iter()
        .filter(|record| is_message(record))
        .filter_map(time_of)
        .fold(None, |max: Option<f64>, at| Some(max.map_or(at, |max| max.max(at))));
    Ok(found.unwrap_or(0.0))
}

/// `fs.stat(file).then(stat => stat.isFile(), () => false)`.
async fn is_file(file: &Path) -> bool {
    tokio::fs::metadata(file).await.is_ok_and(|stat| stat.is_file())
}

/// `fs.readdir(dir)`: names sorted bytewise, as libuv's scandir returns them.
async fn read_dir_names(dir: &Path) -> std::io::Result<Vec<String>> {
    let mut entries = tokio::fs::read_dir(dir).await?;
    let mut names = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        names.push(entry.file_name());
    }
    names.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    Ok(names
        .into_iter()
        .map(|name| name.to_string_lossy().into_owned())
        .collect())
}

/// TS: claudeTranscripts.ts findTranscript (under each `cwd_hints` project first, then in every project).
pub async fn find_transcript(projects_dir: &Path, id: &str, cwd_hints: &[String]) -> Option<PathBuf> {
    let name = format!("{id}.jsonl");
    for cwd in cwd_hints {
        let file = join(projects_dir, &[&claude_project_key(cwd), &name]);
        if is_file(&file).await {
            return Some(file);
        }
    }
    // Long paths are shortened in the key, and a session may have moved: look in every project.
    let dirs = read_dir_names(projects_dir).await.ok()?;
    for dir in dirs {
        let file = join(projects_dir, &[&dir, &name]);
        if is_file(&file).await {
            return Some(file);
        }
    }
    None
}

/// TS: claudeTranscripts.ts LatestConversationOptions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LatestConversationOptions {
    /// Default [`claude_projects_dir`] of the environment.
    pub projects_dir: Option<PathBuf>,
    /// Directories the conversation may be stored under, tried before every project.
    pub cwd_hints: Vec<String>,
    /// Conversation IDs that belong to other sessions (e.g. a fork's): never followed.
    pub exclude: HashSet<String>,
}

/// Node's `stat.*Ms`: milliseconds since the epoch, fractional.
fn system_ms(time: SystemTime) -> f64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => after.as_secs_f64() * 1000.0,
        Err(before) => -before.duration().as_secs_f64() * 1000.0,
    }
}

fn mtime_ms(stat: &std::fs::Metadata) -> f64 {
    stat.mtime() as f64 * 1000.0 + stat.mtime_nsec() as f64 / 1_000_000.0
}

/// `stat.birthtimeMs` (0 where the platform has none, as Node reports).
fn birthtime_ms(stat: &std::fs::Metadata) -> f64 {
    stat.created().map(system_ms).unwrap_or(0.0)
}

struct Member {
    id: String,
    file: PathBuf,
    size: u64,
}

/// TS: claudeTranscripts.ts latestClaudeConversation (the conversation `id` continued in, when it differs: among `id`
/// and its successors carrying its title or continuing one of them, the one with the latest message).
/// The latest message wins, not the latest start, so resuming the earlier conversation and working there keeps it.
pub async fn latest_claude_conversation(id: &str, options: &LatestConversationOptions) -> Result<Option<String>> {
    let projects_dir = match &options.projects_dir {
        Some(dir) => dir.clone(),
        None => {
            let config = std::env::var_os("CLAUDE_CONFIG_DIR").map(|dir| dir.to_string_lossy().into_owned());
            claude_projects_dir(config.as_deref())
        }
    };
    let Some(stored) = find_transcript(&projects_dir, id, &options.cwd_hints).await else {
        return Ok(None);
    };
    let dir = match stored.to_str() {
        Some(text) => PathBuf::from(jspath::dirname(text)),
        None => stored.parent().map(Path::to_path_buf).unwrap_or_default(),
    };
    let stored_stat = tokio::fs::metadata(&stored)
        .await
        .map_err(|error| DhError::io(&error, "stat", &stored.to_string_lossy()))?;
    let stored_head = read_head(&stored).await?;
    let since = stored_head.started_at.unwrap_or_else(|| birthtime_ms(&stored_stat));
    let Ok(names) = read_dir_names(&dir).await else {
        return Ok(None);
    };

    struct Candidate {
        id: String,
        file: PathBuf,
        size: u64,
        mtime_ms: f64,
    }
    let mut candidates = Vec::new();
    for name in names {
        let Some(other) = name.strip_suffix(".jsonl") else {
            continue;
        };
        if other.is_empty() || other == id || options.exclude.contains(other) {
            continue;
        }
        let file = join(&dir, &[&name]);
        // A successor is written after the stored conversation began.
        if let Ok(stat) = tokio::fs::metadata(&file).await
            && stat.is_file()
            && mtime_ms(&stat) >= since
        {
            candidates.push(Candidate {
                id: other.to_string(),
                file,
                size: stat.len(),
                mtime_ms: mtime_ms(&stat),
            });
        }
    }
    candidates.sort_by(|a, b| b.mtime_ms.total_cmp(&a.mtime_ms));
    candidates.truncate(MAX_CANDIDATES);
    let mut heads = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let head = read_head(&candidate.file).await.unwrap_or_default();
        heads.push((candidate, head));
    }
    // Oldest first: a conversation's parent was started before it, so one pass follows a chain of plan acceptances.
    let mut successors: Vec<(Candidate, f64, TranscriptHead)> = heads
        .into_iter()
        .filter_map(|(candidate, head)| {
            let started = head.started_at.filter(|started| *started > since)?;
            Some((candidate, started, head))
        })
        .collect();
    successors.sort_by(|a, b| a.1.total_cmp(&b.1));

    let mut family = vec![Member {
        id: id.to_string(),
        file: stored.clone(),
        size: stored_stat.len(),
    }];
    let mut family_text: HashMap<PathBuf, String> = HashMap::new();
    let stored_title = stored_head.title.as_deref().filter(|title| !title.is_empty());
    for (candidate, _, head) in successors {
        let same_title = stored_title.is_some() && head.title.as_deref() == stored_title;
        let mut continues = false;
        if !same_title && let Some(parent) = head.parent_uuid.as_deref().filter(|parent| !parent.is_empty()) {
            let needle = format!("\"uuid\":\"{parent}\"");
            for member in &family {
                if !family_text.contains_key(&member.file) {
                    let text = tokio::fs::read(&member.file)
                        .await
                        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                        .unwrap_or_default();
                    family_text.insert(member.file.clone(), text);
                }
                if family_text.get(&member.file).is_some_and(|text| text.contains(&needle)) {
                    continues = true;
                    break;
                }
            }
        }
        if same_title || continues {
            family.push(Member {
                id: candidate.id,
                file: candidate.file,
                size: candidate.size,
            });
        }
    }
    if family.len() == 1 {
        return Ok(None);
    }
    let mut latest = 0;
    let mut latest_at = last_activity(&family[0].file, family[0].size).await.unwrap_or(0.0);
    for (index, member) in family.iter().enumerate().skip(1) {
        let at = last_activity(&member.file, member.size).await.unwrap_or(0.0);
        if at > latest_at {
            latest = index;
            latest_at = at;
        }
    }
    let latest_id = &family[latest].id;
    Ok((latest_id != id).then(|| latest_id.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use deckhand_parity::{Case, DirOp, Replay, replay, tags};

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
    }

    #[test]
    fn claude_project_key_parity() {
        replay(
            "I5",
            "claudeTranscripts/claudeProjectKey",
            |case: &Case| -> Result<String, String> { Ok(claude_project_key(case.arg_str(0)?)) },
        );
    }

    #[test]
    fn claude_projects_dir_parity() {
        replay(
            "I5",
            "claudeTranscripts/claudeProjectsDir",
            |case: &Case| -> Result<String, String> {
                let dir = case.arg(0).get("CLAUDE_CONFIG_DIR").and_then(Value::as_str);
                Ok(claude_projects_dir(dir).to_string_lossy().into_owned())
            },
        );
    }

    fn options_of(op: &DirOp) -> std::result::Result<LatestConversationOptions, String> {
        let value = tags::untag(op.arg(1));
        let strings = |key: &str| -> Vec<String> {
            value
                .get(key)
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default()
        };
        Ok(LatestConversationOptions {
            projects_dir: value.get("projectsDir").and_then(Value::as_str).map(PathBuf::from),
            cwd_hints: strings("cwdHints"),
            exclude: strings("exclude").into_iter().collect(),
        })
    }

    #[test]
    fn latest_claude_conversation_parity() {
        let manifest = deckhand_parity::manifest::Manifest::load("I5").expect("manifest");
        let cases: Vec<String> = manifest
            .fixtures
            .iter()
            .filter(|name| {
                name.starts_with("claudeTranscripts/")
                    && !matches!(
                        name.as_str(),
                        "claudeTranscripts/claudeProjectKey" | "claudeTranscripts/claudeProjectsDir"
                    )
            })
            .cloned()
            .collect();
        assert!(!cases.is_empty());
        let mut failed = Vec::new();
        for fixture in &cases {
            let report =
                Replay::new("I5", fixture).run_dir(|_root, op: &DirOp| -> std::result::Result<Value, String> {
                    if op.fn_name != "latestClaudeConversation" {
                        return Err(format!("unexpected fn {}", op.fn_name));
                    }
                    let id = op.arg_str(0)?.to_string();
                    let options = options_of(op)?;
                    let found = runtime()
                        .block_on(latest_claude_conversation(&id, &options))
                        .map_err(|error| error.to_string())?;
                    Ok(found.map(Value::String).unwrap_or_else(tags::undefined))
                });
            if !report.ok {
                failed.push(report.summary());
            }
        }
        assert!(failed.is_empty(), "{}", failed.join("\n"));
    }

    #[test]
    fn records_skip_cut_lines_and_non_objects() {
        let found = records("not json\n42\nnull\n[1]\n{\"a\":1}\n{\"b\":\n {\"c\":2} \r");
        assert_eq!(found.len(), 2);
        assert_eq!(found[1].get("c"), Some(&Value::from(2)));
    }

    #[test]
    fn records_keep_lines_json_parse_accepts() {
        // JSON.parse keeps a lone surrogate (here U+FFFD), any nesting depth and out-of-range numbers.
        let lone = records(r#"{"type":"user","message":"cut \ud83d","low":"\ude00x","pair":"😀"}"#);
        assert_eq!(lone.len(), 1, "lone surrogate line dropped");
        assert_eq!(lone[0].get("message"), Some(&Value::from("cut \u{FFFD}")));
        assert_eq!(lone[0].get("low"), Some(&Value::from("\u{FFFD}x")));
        assert_eq!(lone[0].get("pair"), Some(&Value::from("\u{1F600}")));
        let deep = format!(r#"{{"type":"user","m":{}1{}}}"#, "[".repeat(20_000), "]".repeat(20_000));
        assert_eq!(records(&deep).len(), 1, "deep line dropped");
        assert_eq!(
            records(&deep[..deep.len() - 1]).len(),
            0,
            "a deep line cut by the byte range"
        );
        assert_eq!(
            records(r#"{"type":"user","n":1e400,"m":-1e400}"#).len(),
            1,
            "out-of-range number dropped"
        );
        // Duplicate keys: the last value, at the first key's position.
        let dup = records(r#"{"a":1,"b":2,"a":3}"#);
        assert_eq!(dup[0].keys().collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(dup[0].get("a"), Some(&Value::from(3)));
    }

    #[test]
    fn json_parse_accepts_what_v8_accepts() {
        let cases: Vec<(String, Option<String>)> =
            serde_json::from_str(include_str!("../../core/tests/data/json_parse_errors.json")).expect("fixture");
        let mut wrong = Vec::new();
        for (text, message) in &cases {
            let parsed = js_json_parse(text);
            if parsed.is_some() != message.is_none() {
                wrong.push(format!("{text:?}: parsed {parsed:?}, V8 {message:?}"));
            }
            // Where serde_json also accepts, both agree on the value.
            if let (Some(parsed), Ok(serde)) = (&parsed, serde_json::from_str::<Value>(text)) {
                assert_eq!(parsed, &serde, "{text:?}");
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    /// Review findings 1 and 2: a message line holding a lone surrogate escape (an emoji cut in half) still counts
    /// for the head and the last activity.
    #[tokio::test(flavor = "current_thread")]
    async fn lone_surrogate_message_lines_still_count() {
        let temp = tempfile::Builder::new()
            .prefix("claude-projects-")
            .tempdir()
            .expect("tempdir");
        let projects = temp.path().to_path_buf();
        let cwd = "/work/repo";
        let dir = projects.join(claude_project_key(cwd));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let write = |id: &str, text: &str| std::fs::write(dir.join(format!("{id}.jsonl")), text).expect("write");
        let latest = |id: &str| {
            let options = LatestConversationOptions {
                projects_dir: Some(projects.clone()),
                cwd_hints: vec![cwd.to_string()],
                exclude: HashSet::new(),
            };
            let id = id.to_string();
            async move { latest_claude_conversation(&id, &options).await.expect("latest") }
        };
        let title = r#"{"type":"custom-title","customTitle":"T"}"#;
        // Finding 1: the last message (the one that makes impl the latest) holds a cut emoji.
        write(
            "plan",
            &format!(
                "{title}\n{}\n{}\n",
                r#"{"type":"user","uuid":"plan-1","timestamp":"2026-01-01T12:00:00.000Z"}"#,
                r#"{"type":"assistant","uuid":"plan-2","parentUuid":"plan-1","timestamp":"2026-01-01T12:20:00.000Z"}"#
            ),
        );
        write(
            "impl",
            &format!(
                "{title}\n{}\n{}\n",
                r#"{"type":"user","uuid":"impl-1","parentUuid":"plan-2","timestamp":"2026-01-01T12:11:00.000Z"}"#,
                r#"{"type":"assistant","uuid":"impl-2","parentUuid":"impl-1","x":"cut \ud83d","timestamp":"2026-01-01T12:30:00.000Z"}"#
            ),
        );
        assert_eq!(latest("plan").await, Some("impl".to_string()));
        // Finding 2: no shared title; the first message (whose parentUuid links to plan) holds a cut emoji.
        write(
            "plan",
            &format!(
                "{}\n{}\n",
                r#"{"type":"user","uuid":"plan-1","timestamp":"2026-01-01T12:00:00.000Z"}"#,
                r#"{"type":"assistant","uuid":"plan-2","parentUuid":"plan-1","timestamp":"2026-01-01T12:10:00.000Z"}"#
            ),
        );
        write(
            "impl",
            &format!(
                "{}\n{}\n",
                r#"{"type":"user","uuid":"impl-1","parentUuid":"plan-2","message":{"content":"cut emoji \ud83d"},"timestamp":"2026-01-01T12:11:00.000Z"}"#,
                r#"{"type":"assistant","uuid":"impl-2","parentUuid":"impl-1","timestamp":"2026-01-01T12:30:00.000Z"}"#
            ),
        );
        assert_eq!(latest("plan").await, Some("impl".to_string()));
    }

    // covers: B-I5-03 restart follows the conversation a Claude session continued in, from its transcripts
    #[tokio::test(flavor = "current_thread")]
    async fn restart_follows_the_conversation_a_claude_session_continued_in() {
        fn line(record: Value) -> String {
            format!("{record}\n")
        }
        fn at(minutes: i64) -> String {
            date::to_iso_string(1_767_268_800_000 + minutes * 60_000).expect("iso")
        }
        /// A transcript as Claude writes it: the launch title first (unless none), then messages; `from` is the
        /// parent of the first.
        fn transcript(title: Option<&str>, start: i64, end: i64, prefix: &str, from: Option<&str>) -> String {
            let mut text = title
                .map(|title| line(serde_json::json!({"type": "custom-title", "customTitle": title})))
                .unwrap_or_default();
            text += &line(
                serde_json::json!({"type": "user", "uuid": format!("{prefix}-1"), "parentUuid": from, "timestamp": at(start)}),
            );
            text += &line(
                serde_json::json!({"type": "assistant", "uuid": format!("{prefix}-2"), "parentUuid": format!("{prefix}-1"), "timestamp": at(end)}),
            );
            text
        }

        let temp = tempfile::Builder::new()
            .prefix("claude-projects-")
            .tempdir()
            .expect("tempdir");
        let projects = temp.path().to_path_buf();
        let cwd = "/work/repo";
        let dir = projects.join(claude_project_key(cwd));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let write = |id: &str, text: String| std::fs::write(dir.join(format!("{id}.jsonl")), text).expect("write");
        let latest = |id: &str, exclude: &[&str]| {
            let options = LatestConversationOptions {
                projects_dir: Some(projects.clone()),
                cwd_hints: vec![cwd.to_string()],
                exclude: exclude.iter().map(|id| id.to_string()).collect(),
            };
            let id = id.to_string();
            async move { latest_claude_conversation(&id, &options).await.expect("latest") }
        };
        let some = |id: &str| Some(id.to_string());
        assert_eq!(
            claude_project_key("/Users/me/.deckhand/worktrees/a_b"),
            "-Users-me--deckhand-worktrees-a-b"
        );

        // Unknown conversation, or no successor: nothing to change.
        assert_eq!(latest("missing", &[]).await, None);
        write("plan", transcript(Some("dh-task-1234"), 0, 10, "plan", None));
        assert_eq!(latest("plan", &[]).await, None);

        // Accepting the plan with "clear context": same title, first message continues the plan. Then accepted again.
        write("impl", transcript(Some("dh-task-1234"), 11, 30, "impl", Some("plan-2")));
        assert_eq!(latest("plan", &[]).await, some("impl"));
        write(
            "impl2",
            transcript(Some("dh-task-1234"), 31, 40, "impl2", Some("impl-2")),
        );
        assert_eq!(latest("plan", &[]).await, some("impl2"));
        assert_eq!(latest("impl", &[]).await, some("impl2"));
        assert_eq!(latest("impl2", &[]).await, None); // Never back to an earlier conversation.

        // Linked by the parent message alone (the title was renamed), transitively; unrelated conversations are not
        // followed.
        write("other", transcript(Some("dh-other-9999"), 41, 50, "other", None));
        write("a", transcript(None, 0, 5, "a", None));
        write("b", transcript(Some("renamed"), 6, 8, "b", Some("a-2")));
        write("c", transcript(None, 9, 12, "c", Some("b-2")));
        assert_eq!(latest("a", &[]).await, some("c"));

        // A conversation resumed and worked in after its successor stays the one to resume (latest message wins).
        write("plan", transcript(Some("dh-task-1234"), 0, 55, "plan", None));
        assert_eq!(latest("plan", &[]).await, None);

        // Another session's conversation (a Deckhand fork with the copied title) is never followed.
        write("x", transcript(Some("dh-x-0000"), 0, 5, "x", None));
        write("fork", transcript(Some("dh-x-0000"), 6, 20, "fork", Some("x-2")));
        assert_eq!(latest("x", &["fork"]).await, None);
        assert_eq!(latest("x", &[]).await, some("fork"));

        // Found in another project when the cwd hint does not match (e.g. a path Claude shortened).
        let elsewhere = LatestConversationOptions {
            projects_dir: Some(projects.clone()),
            cwd_hints: vec!["/elsewhere".into()],
            exclude: HashSet::new(),
        };
        assert_eq!(
            latest_claude_conversation("impl", &elsewhere).await.expect("latest"),
            some("impl2")
        );
        let no_hints = LatestConversationOptions {
            projects_dir: Some(projects.clone()),
            ..Default::default()
        };
        assert_eq!(
            latest_claude_conversation("a", &no_hints).await.expect("latest"),
            some("c")
        );
    }
}
