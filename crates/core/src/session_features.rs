//! Port of sessionFeatures.ts (pure parts).
//!
//! Session filters (f), attention, and the Markdown handoff (H). Writing the handoff file (`exportHandoff`) is I/O and
//! lives in `deckhand_sys::session_features`. [`HandoffGitContext`] (workspaceGit.ts) is defined here because the
//! handoff renders it; `deckhand_sys::workspace_git` gathers it.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::session_order::SessionLike;
use deckhand_protocol::data::SessionRecord;
/// TS: sessionFeatures.ts SessionFilter (persisted in ui-state.json, so defined by deckhand-protocol).
pub use deckhand_protocol::records::SessionFilter;

/// TS: sessionFeatures.ts SESSION_FILTERS (the filter menu's order).
pub const SESSION_FILTERS: [SessionFilter; 6] = [
    SessionFilter::Active,
    SessionFilter::Running,
    SessionFilter::Attention,
    SessionFilter::Exited,
    SessionFilter::Archived,
    SessionFilter::All,
];

/// TS: sessionFeatures.ts SESSION_FILTER_KEYS.
pub const SESSION_FILTER_KEYS: [(SessionFilter, &str); 6] = [
    (SessionFilter::Active, "a"),
    (SessionFilter::Running, "r"),
    (SessionFilter::Attention, "!"),
    (SessionFilter::Exited, "e"),
    (SessionFilter::Archived, "A"),
    (SessionFilter::All, "*"),
];

/// TS: sessionFeatures.ts SESSION_FILTER_KEYS[filter].
pub fn session_filter_key(filter: SessionFilter) -> &'static str {
    let _ = filter;
    todo!("P8")
}

/// TS: sessionFeatures.ts filterForKey.
pub fn filter_for_key(input: &str) -> Option<SessionFilter> {
    let _ = input;
    todo!("P8")
}

/// TS: sessionFeatures.ts sessionBlocked (asks, failed, rate-limited, or exited failed or interrupted).
pub fn session_blocked(session: &SessionRecord) -> bool {
    let _ = session;
    todo!("P8")
}

/// TS: sessionFeatures.ts sessionHasUnreadReply.
pub fn session_has_unread_reply(session: &SessionRecord) -> bool {
    let _ = session;
    todo!("P8")
}

/// TS: sessionFeatures.ts sessionNeedsAttention.
pub fn session_needs_attention(session: &SessionRecord) -> bool {
    let _ = session;
    todo!("P8")
}

/// TS: sessionFeatures.ts withReplyRead (`read_replies`: ui-state's session ID → when you last detached from its
/// agent).
pub fn with_reply_read(session: SessionRecord, read_replies: &IndexMap<String, String>) -> SessionRecord {
    let _ = (session, read_replies);
    todo!("P8")
}

/// TS: sessionFeatures.ts filterSessionList `taskTitle`: the title of the task a session works on, if any.
pub type TaskTitleOf<'a> = &'a dyn Fn(&SessionRecord) -> Option<String>;

/// TS: sessionFeatures.ts filterSessionList (`task_title`: the task a session works on, so search finds it).
pub fn filter_session_list<'a, S: SessionLike>(
    sessions: &'a [S],
    filter: SessionFilter,
    query: &str,
    task_title: Option<TaskTitleOf<'_>>,
) -> Vec<&'a S> {
    let _ = (sessions, filter, query, task_title);
    todo!("P8")
}

/// TS: sessionFeatures.ts sessionFilterCounts result (`Record<SessionFilter, number>` in SESSION_FILTERS order).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionFilterCounts {
    pub active: usize,
    pub running: usize,
    pub attention: usize,
    pub exited: usize,
    pub archived: usize,
    pub all: usize,
}

impl SessionFilterCounts {
    /// `counts[filter]`.
    pub fn get(&self, filter: SessionFilter) -> usize {
        let _ = filter;
        todo!("P8")
    }
}

/// TS: sessionFeatures.ts sessionFilterCounts.
pub fn session_filter_counts<S: SessionLike>(
    sessions: &[S],
    query: &str,
    task_title: Option<TaskTitleOf<'_>>,
) -> SessionFilterCounts {
    let _ = (sessions, query, task_title);
    todo!("P8")
}

/// workspaceGit.ts HandoffGitContext `changes` item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffChange {
    pub status: String,
    pub path: String,
}

/// workspaceGit.ts HandoffGitContext `diff.names` item (`additions` absent: binary).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffDiffName {
    pub path: String,
    pub additions: Option<i64>,
    pub deletions: Option<i64>,
}

/// workspaceGit.ts HandoffGitContext `diff`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffDiff {
    pub files: i64,
    pub insertions: i64,
    pub deletions: i64,
    pub names: Vec<HandoffDiffName>,
}

/// workspaceGit.ts HandoffGitContext.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffGitContext {
    pub base_ref: Option<String>,
    pub commits: Vec<String>,
    pub more_commits: usize,
    pub changes: Vec<HandoffChange>,
    pub more_changes: usize,
    pub diff: Option<HandoffDiff>,
    pub error: Option<String>,
}

/// TS: sessionFeatures.ts oneLine (a string with controls or newlines JSON-quoted, so it stays one line).
pub fn one_line(text: &str) -> String {
    let _ = text;
    todo!("P8")
}

/// TS: sessionFeatures.ts handoffGitLines (the "Workspace changes" section).
pub fn handoff_git_lines(context: &HandoffGitContext) -> Vec<String> {
    let _ = context;
    todo!("P8")
}

/// TS: sessionFeatures.ts handoffMarkdown `task` argument.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffTask {
    pub title: String,
    pub body: String,
    pub done: bool,
}

/// TS: sessionFeatures.ts handoffMarkdown (`include_output`: the terminal excerpt, default off).
pub fn handoff_markdown(
    session: &SessionRecord,
    include_output: bool,
    git: Option<&HandoffGitContext>,
    task: Option<&HandoffTask>,
) -> String {
    let _ = (session, include_output, git, task);
    todo!("P8")
}
