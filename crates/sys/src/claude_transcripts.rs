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

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use deckhand_core::error::Result;

/// TS: claudeTranscripts.ts HEAD_BYTES.
pub const HEAD_BYTES: usize = 64 * 1024;
/// TS: claudeTranscripts.ts TAIL_BYTES.
pub const TAIL_BYTES: usize = 64 * 1024;
/// TS: claudeTranscripts.ts MAX_CANDIDATES.
pub const MAX_CANDIDATES: usize = 200;

/// TS: claudeTranscripts.ts claudeProjectsDir (`$CLAUDE_CONFIG_DIR/projects`, else `~/.claude/projects`).
/// `claude_config_dir`: the `CLAUDE_CONFIG_DIR` value (TS reads it from `env`, default `process.env`).
pub fn claude_projects_dir(claude_config_dir: Option<&str>) -> PathBuf {
    let _ = claude_config_dir;
    todo!("I5")
}

/// TS: claudeTranscripts.ts claudeProjectKey (the resolved cwd with every character but an ASCII letter or digit
/// replaced by `-`, per UTF-16 unit as in TS).
pub fn claude_project_key(cwd: &str) -> String {
    let _ = cwd;
    todo!("I5")
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
    let _ = (file, start, length);
    todo!("I5")
}

/// TS: claudeTranscripts.ts records (the JSON objects among `text`'s lines; lines cut by the byte range skipped).
pub fn records(text: &str) -> Vec<serde_json::Map<String, serde_json::Value>> {
    let _ = text;
    todo!("I5")
}

/// TS: claudeTranscripts.ts timeOf (`Date.parse(record.timestamp)` when finite).
pub fn time_of(record: &serde_json::Map<String, serde_json::Value>) -> Option<f64> {
    let _ = record;
    todo!("I5")
}

/// TS: claudeTranscripts.ts readHead.
pub async fn read_head(file: &Path) -> Result<TranscriptHead> {
    let _ = file;
    todo!("I5")
}

/// TS: claudeTranscripts.ts lastActivity (when the conversation last had a message; 0 when none).
pub async fn last_activity(file: &Path, size: u64) -> Result<f64> {
    let _ = (file, size);
    todo!("I5")
}

/// TS: claudeTranscripts.ts findTranscript (under each `cwd_hints` project first, then in every project).
pub async fn find_transcript(projects_dir: &Path, id: &str, cwd_hints: &[String]) -> Option<PathBuf> {
    let _ = (projects_dir, id, cwd_hints);
    todo!("I5")
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

/// TS: claudeTranscripts.ts latestClaudeConversation (the conversation `id` continued in, when it differs: among `id`
/// and its successors carrying its title or continuing one of them, the one with the latest message).
pub async fn latest_claude_conversation(id: &str, options: &LatestConversationOptions) -> Result<Option<String>> {
    let _ = (id, options);
    todo!("I5")
}
