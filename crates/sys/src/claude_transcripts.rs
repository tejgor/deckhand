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
        .filter_map(|line| match serde_json::from_str::<Value>(line) {
            Ok(Value::Object(record)) => Some(record),
            _ => None,
        })
        .collect()
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
