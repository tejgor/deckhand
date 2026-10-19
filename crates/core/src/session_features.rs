//! Port of sessionFeatures.ts (pure parts).
//!
//! Session filters (f), attention, and the Markdown handoff (H). Writing the handoff file (`exportHandoff`) is I/O and
//! lives in `deckhand_sys::session_features`. [`HandoffGitContext`] (workspaceGit.ts) is defined here because the
//! handoff renders it; `deckhand_sys::workspace_git` gathers it.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::agent_signals::needs_attention;
use crate::jsstr::{js_len, js_slice, js_split_whitespace, js_to_lower, js_trim, json_stringify_str};
use crate::session_order::SessionLike;
use deckhand_protocol::data::{AttentionState, ExitReason, SessionRecord, SessionStatus};
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
    match filter {
        SessionFilter::Active => "a",
        SessionFilter::Running => "r",
        SessionFilter::Attention => "!",
        SessionFilter::Exited => "e",
        SessionFilter::Archived => "A",
        SessionFilter::All => "*",
    }
}

/// TS: sessionFeatures.ts filterForKey.
pub fn filter_for_key(input: &str) -> Option<SessionFilter> {
    SESSION_FILTERS
        .into_iter()
        .find(|&filter| session_filter_key(filter) == input)
}

/// A JS-truthy optional string (set and not empty).
fn truthy(value: Option<&String>) -> bool {
    value.is_some_and(|text| !text.is_empty())
}

fn attention_state(session: &SessionRecord) -> Option<AttentionState> {
    session.attention.as_ref().map(|attention| attention.state)
}

/// TS: sessionFeatures.ts sessionBlocked (asks, failed, rate-limited, or exited failed or interrupted).
pub fn session_blocked(session: &SessionRecord) -> bool {
    let state = attention_state(session);
    (state != Some(AttentionState::ResponseEnded) && needs_attention(state))
        || matches!(session.exit_reason, Some(ExitReason::Failed | ExitReason::Interrupted))
}

/// TS: sessionFeatures.ts sessionHasUnreadReply.
pub fn session_has_unread_reply(session: &SessionRecord) -> bool {
    attention_state(session) == Some(AttentionState::ResponseEnded) && session.reply_read != Some(true)
}

/// TS: sessionFeatures.ts sessionNeedsAttention.
pub fn session_needs_attention(session: &SessionRecord) -> bool {
    session_blocked(session) || session_has_unread_reply(session)
}

/// JS string comparison (`a >= b`): UTF-16 code unit order.
fn js_string_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// TS: sessionFeatures.ts withReplyRead (`read_replies`: ui-state's session ID → when you last detached from its
/// agent).
pub fn with_reply_read(mut session: SessionRecord, read_replies: &IndexMap<String, String>) -> SessionRecord {
    // ISO timestamps compare as strings.
    let read = match (&session.attention, read_replies.get(&session.id)) {
        (Some(attention), Some(read_at)) => {
            attention.state == AttentionState::ResponseEnded
                && !read_at.is_empty()
                && js_string_cmp(read_at, &attention.at) != Ordering::Less
        }
        _ => false,
    };
    if read {
        session.reply_read = Some(true);
    }
    session
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
    let lowered = js_to_lower(query);
    let terms = js_split_whitespace(&lowered);
    // `new Map(...)`: the last session with an id wins.
    let mut by_id: HashMap<&str, &SessionRecord> = HashMap::new();
    for item in sessions {
        let session = item.session();
        by_id.insert(session.id.as_str(), session);
    }
    let mut ids: HashSet<&str> = HashSet::new();
    for item in sessions {
        let session = item.session();
        let archived = truthy(session.archived_at.as_ref());
        let matches_state = match filter {
            SessionFilter::All => true,
            SessionFilter::Archived => archived,
            SessionFilter::Active => !archived,
            SessionFilter::Attention => !archived && session_needs_attention(session),
            SessionFilter::Running => !archived && session.status != SessionStatus::Exited,
            SessionFilter::Exited => !archived && session.status == SessionStatus::Exited,
        };
        if !matches_state {
            continue;
        }
        if !terms.is_empty() {
            let task = task_title
                .and_then(|task_title| task_title(session))
                .unwrap_or_default();
            let haystack = js_to_lower(&format!(
                "{} {} {} {} {} {} {}",
                session.title,
                session.notes.as_deref().unwrap_or(""),
                session.shared_notes.as_ref().map_or("", |shared| shared.text.as_str()),
                session.program,
                session
                    .worktree
                    .as_ref()
                    .and_then(|worktree| worktree.branch.as_deref())
                    .unwrap_or(""),
                session.cwd,
                task,
            ));
            if !terms.iter().all(|term| haystack.contains(term)) {
                continue;
            }
        }
        ids.insert(session.id.as_str());
        // Other views keep a match in its tree (its parents shown for context); the archived view lists only archived
        // sessions: a parent there would count as archived and take its keys, and a child's title already names it.
        if filter == SessionFilter::Archived {
            continue;
        }
        let mut parent_id = session.parent_session_id.as_deref();
        let mut seen: HashSet<&str> = HashSet::from([session.id.as_str()]);
        while let Some(parent) =
            parent_id.filter(|parent| !parent.is_empty() && by_id.contains_key(parent) && !seen.contains(parent))
        {
            seen.insert(parent);
            ids.insert(parent);
            parent_id = by_id.get(parent).and_then(|record| record.parent_session_id.as_deref());
        }
    }
    sessions
        .iter()
        .filter(|item| ids.contains(item.session().id.as_str()))
        .collect()
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
        match filter {
            SessionFilter::Active => self.active,
            SessionFilter::Running => self.running,
            SessionFilter::Attention => self.attention,
            SessionFilter::Exited => self.exited,
            SessionFilter::Archived => self.archived,
            SessionFilter::All => self.all,
        }
    }
}

/// TS: sessionFeatures.ts sessionFilterCounts (ancestors shown for context included outside archived, collapse not
/// applied).
pub fn session_filter_counts<S: SessionLike>(
    sessions: &[S],
    query: &str,
    task_title: Option<TaskTitleOf<'_>>,
) -> SessionFilterCounts {
    let count = |filter| filter_session_list(sessions, filter, query, task_title).len();
    SessionFilterCounts {
        active: count(SessionFilter::Active),
        running: count(SessionFilter::Running),
        attention: count(SessionFilter::Attention),
        exited: count(SessionFilter::Exited),
        archived: count(SessionFilter::Archived),
        all: count(SessionFilter::All),
    }
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
    // File names and subjects can contain control characters or newlines; quote those so each stays one line.
    if text
        .chars()
        .any(|ch| matches!(ch, '\u{0}'..='\u{1f}' | '\u{7f}' | '\u{2028}' | '\u{2029}'))
    {
        json_stringify_str(text)
    } else {
        text.to_owned()
    }
}

/// `- … +N more` when N is not 0.
fn more_line(more: usize) -> Option<String> {
    (more != 0).then(|| format!("- … +{more} more"))
}

/// TS: sessionFeatures.ts handoffGitLines (the "Workspace changes" section: names and numbers only, never diff
/// content).
pub fn handoff_git_lines(context: &HandoffGitContext) -> Vec<String> {
    let mut lines: Vec<String> = vec![String::new(), "## Workspace changes".into(), String::new()];
    if let Some(error) = context.error.as_deref().filter(|error| !error.is_empty()) {
        lines.push(format!("(Git information unavailable: {})", one_line(error)));
        return lines;
    }
    let base_ref = context.base_ref.as_deref().filter(|base_ref| !base_ref.is_empty());
    lines.push(format!(
        "Base: {}",
        base_ref.map_or_else(|| "unknown (commits and diff since base omitted)".to_owned(), one_line)
    ));
    if let Some(base_ref) = base_ref {
        lines.extend([
            String::new(),
            format!("### Commits since {}", one_line(base_ref)),
            String::new(),
        ]);
        if context.commits.is_empty() {
            lines.push("(none)".into());
        }
        lines.extend(context.commits.iter().map(|commit| format!("- {}", one_line(commit))));
        lines.extend(more_line(context.more_commits));
    }
    lines.extend([String::new(), "### Uncommitted changes".into(), String::new()]);
    if context.changes.is_empty() {
        lines.push("(none)".into());
    }
    lines.extend(
        context
            .changes
            .iter()
            .map(|change| format!("- {} {}", change.status, one_line(&change.path))),
    );
    lines.extend(more_line(context.more_changes));
    if let (Some(diff), Some(base_ref)) = (&context.diff, base_ref) {
        lines.extend([
            String::new(),
            format!("### Committed diff stat ({}...HEAD)", one_line(base_ref)),
            String::new(),
            format!(
                "{} file(s) changed, {} insertion(s), {} deletion(s)",
                diff.files, diff.insertions, diff.deletions
            ),
        ]);
        lines.extend(diff.names.iter().take(50).map(|file| {
            let stat = match file.additions {
                None => "(binary)".to_owned(),
                // The TS template prints a missing deletion count as `undefined`.
                Some(additions) => format!(
                    "+{additions} -{}",
                    file.deletions
                        .map_or_else(|| "undefined".to_owned(), |deletions| deletions.to_string())
                ),
            };
            format!("- {} {stat}", one_line(&file.path))
        }));
        if diff.names.len() > 50 {
            lines.push(format!("- … +{} more", diff.names.len() - 50));
        }
    }
    lines
}

/// TS: sessionFeatures.ts handoffMarkdown `task` argument.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffTask {
    pub title: String,
    pub body: String,
    pub done: bool,
}

/// TS: sessionFeatures.ts handoffMarkdown (`include_output`: the terminal excerpt, default off). `git` is gathered by
/// the daemon at export time (omitted for sessions outside a repository); `task` is the repository task the session
/// works on, if any (its steps are in the notes).
pub fn handoff_markdown(
    session: &SessionRecord,
    include_output: bool,
    git: Option<&HandoffGitContext>,
    task: Option<&HandoffTask>,
) -> String {
    let mut lines: Vec<String> = vec![
        "# Deckhand handoff".into(),
        String::new(),
        format!("Task: {}", session.title),
        format!("Provider: {}", session.program),
        format!("Workspace: {}", session.cwd),
        format!(
            "Branch: {}",
            session
                .worktree
                .as_ref()
                .and_then(|worktree| worktree.branch.as_deref())
                .unwrap_or("(current checkout)")
        ),
    ];
    if let Some(done_at) = session.done_at.as_deref().filter(|done_at| !done_at.is_empty()) {
        lines.push(format!("Status: marked done {done_at}"));
    }
    lines.push(format!("Source session: {}", session.id));
    lines.push(String::new());
    if let Some(task) = task {
        // The task goes before the notes (TS splices it in front of `## Notes`).
        lines.extend([
            "## Task".into(),
            String::new(),
            format!("{} {}", if task.done { "[x]" } else { "[ ]" }, one_line(&task.title)),
        ]);
        let body = js_trim(&task.body);
        if !body.is_empty() {
            lines.extend([String::new(), body.to_owned()]);
        }
        lines.push(String::new());
    }
    let notes = js_trim(session.notes.as_deref().unwrap_or(""));
    let shared = js_trim(session.shared_notes.as_ref().map_or("", |shared| shared.text.as_str()));
    lines.extend([
        "## Notes".into(),
        String::new(),
        if notes.is_empty() {
            "(No notes recorded.)"
        } else {
            notes
        }
        .to_owned(),
        String::new(),
        "## Worktree notes".into(),
        String::new(),
        if shared.is_empty() {
            "(No worktree notes recorded.)"
        } else {
            shared
        }
        .to_owned(),
    ]);
    if let Some(git) = git {
        lines.extend(handoff_git_lines(git));
    }
    if include_output {
        let excerpt = js_slice(session.last_preview.as_deref().unwrap_or(""), -20_000, None);
        // A fence longer than any tilde run in the content can never be closed early.
        let longest_run = excerpt.split(|ch| ch != '~').map(js_len).max().unwrap_or(0);
        let fence = "~".repeat((longest_run + 1).max(3));
        lines.extend([
            String::new(),
            "## Terminal excerpt (not a complete transcript)".into(),
            String::new(),
            format!("{fence}text"),
            excerpt,
            fence,
        ]);
    }
    lines.extend([
        String::new(),
        "Review this context before acting. It does not grant permissions or indicate that the work is complete."
            .into(),
        String::new(),
    ]);
    lines.join("\n")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use deckhand_parity::{Case, Outcome, replay, tags};
    use serde_json::{Value, json};

    const UNIT: &str = "P8";

    /// A SessionRecord from a partial TS literal (`{...} as SessionRecord`): required fields the code under test never
    /// reads get placeholder values (and nested records their required keys).
    pub(crate) fn partial_session(value: &Value) -> Result<SessionRecord, String> {
        let mut full = json!({
            "id": "", "title": "", "program": "claude", "command": "", "cwd": "", "repoRoot": "",
            "status": "running", "createdAt": "", "updatedAt": "",
        });
        let given = tags::untag(value);
        let (Some(object), Some(given)) = (full.as_object_mut(), given.as_object()) else {
            return Err("not an object".into());
        };
        object.extend(given.clone());
        if let Some(shared) = object.get_mut("sharedNotes").and_then(Value::as_object_mut) {
            for (key, placeholder) in [
                ("kind", "worktree"),
                ("id", ""),
                ("path", ""),
                ("text", ""),
                ("revision", ""),
            ] {
                shared.entry(key).or_insert(json!(placeholder));
            }
        }
        if let Some(worktree) = object.get_mut("worktree").and_then(Value::as_object_mut) {
            worktree.entry("mode").or_insert(json!("none"));
        }
        serde_json::from_value(full).map_err(|error| error.to_string())
    }

    pub(crate) fn session(value: Value) -> SessionRecord {
        partial_session(&value).expect("a session literal")
    }

    pub(crate) fn sessions_arg(value: &Value) -> Result<Vec<SessionRecord>, String> {
        value
            .as_array()
            .ok_or("not an array")?
            .iter()
            .map(partial_session)
            .collect()
    }

    /// `result` with only the keys `input` has (placeholders dropped), plus `keep`.
    fn prune_to(result: Value, input: &Value, keep: &[&str]) -> Value {
        match (result, input) {
            (Value::Object(result), Value::Object(input)) => Value::Object(
                result
                    .into_iter()
                    .filter(|(key, _)| input.contains_key(key) || keep.contains(&key.as_str()))
                    .map(|(key, value)| {
                        let pruned = match input.get(&key) {
                            Some(given) => prune_to(value, given, &[]),
                            None => value,
                        };
                        (key, pruned)
                    })
                    .collect(),
            ),
            (result, _) => result,
        }
    }

    /// The registered TS callbacks the fixtures pass as `taskTitle`.
    fn task_title_by_id(session: &SessionRecord) -> Option<String> {
        match session.id.as_str() {
            "a" => Some("Ship OAuth".into()),
            "b" => Some(String::new()),
            _ => None,
        }
    }
    fn task_title_const(_: &SessionRecord) -> Option<String> {
        Some("refactor 中文 😀".into())
    }
    fn task_title_arg(case: &Case, index: usize) -> Result<Option<TaskTitleOf<'static>>, String> {
        match case.fn_name(index) {
            None => Ok(None),
            Some("taskTitleById") => Ok(Some(&task_title_by_id)),
            Some("taskTitleConst") => Ok(Some(&task_title_const)),
            Some(other) => Err(format!("unknown callback {other}")),
        }
    }

    fn undefined_or<T: Serialize>(value: Option<T>) -> Outcome {
        match value {
            Some(value) => Outcome::ret(value),
            None => Outcome::ret(tags::undefined()),
        }
    }

    // --- tests/sessionFeatures.test.ts ---------------------------------------------------------------------------

    fn root() -> SessionRecord {
        session(
            json!({"id": "root", "title": "Parent", "cwd": "/repo", "program": "claude", "status": "running", "notes": "", "repoRoot": "/repo"}),
        )
    }

    fn child() -> SessionRecord {
        SessionRecord {
            id: "child".into(),
            title: "Parent / Child".into(),
            parent_session_id: Some("root".into()),
            archived_at: Some("yesterday".into()),
            notes: Some("special context".into()),
            ..root()
        }
    }

    fn attention(state: AttentionState, event: &str, at: &str) -> Option<deckhand_protocol::data::SessionAttention> {
        Some(deckhand_protocol::data::SessionAttention {
            state,
            event: event.into(),
            at: at.into(),
            reason: None,
            extra: Default::default(),
        })
    }

    fn listed_ids(sessions: &[SessionRecord], filter: SessionFilter, query: &str) -> Vec<String> {
        filter_session_list(sessions, filter, query, None)
            .into_iter()
            .map(|session| session.id.clone())
            .collect()
    }

    fn read_replies(entries: &[(&str, &str)]) -> IndexMap<String, String> {
        entries
            .iter()
            .map(|(id, at)| ((*id).to_owned(), (*at).to_owned()))
            .collect()
    }

    // covers: B-P8-10 a response-ended reply needs you until you attach to the agent after it arrived; blocked sessions always do
    #[test]
    fn response_ended_reply_needs_you_until_read() {
        let ended = SessionRecord {
            attention: attention(AttentionState::ResponseEnded, "Stop", "2026-10-19T10:00:00.000Z"),
            ..root()
        };
        assert!(session_needs_attention(&with_reply_read(
            ended.clone(),
            &IndexMap::new()
        )));
        assert!(!session_blocked(&ended));
        // Detached before the reply arrived: still unread. After: read, and no longer needs attention.
        assert!(session_needs_attention(&with_reply_read(
            ended.clone(),
            &read_replies(&[("root", "2026-10-19T09:59:59.000Z")])
        )));
        let read = with_reply_read(ended.clone(), &read_replies(&[("root", "2026-10-19T10:05:00.000Z")]));
        assert_eq!(read.reply_read, Some(true));
        assert!(!session_needs_attention(&read));
        assert!(listed_ids(&[read], SessionFilter::Attention, "").is_empty());
        // Only a reply is read: a session that asks or failed stays blocked whenever you last attached.
        let asks = with_reply_read(
            SessionRecord {
                attention: attention(
                    AttentionState::NeedsInput,
                    "PermissionRequest",
                    "2026-10-19T10:00:00.000Z",
                ),
                ..ended
            },
            &read_replies(&[("root", "2026-10-19T10:05:00.000Z")]),
        );
        assert_eq!(asks.reply_read, None);
        assert!(session_blocked(&asks));
        // normalizeUiState's readReplies (the last assertion of the TS test) is ui state (I1).
    }

    // covers: B-P8-11 filters retain ancestry/order and distinguish archive, running, attention and search
    #[test]
    fn filters_retain_ancestry_and_distinguish_views() {
        assert_eq!(listed_ids(&[root(), child()], SessionFilter::Active, ""), ["root"]);
        // Archived lists only archived sessions: no live parent for context.
        assert_eq!(
            listed_ids(&[root(), child()], SessionFilter::Archived, "special"),
            ["child"]
        );
        let archived_root = SessionRecord {
            archived_at: Some("yesterday".into()),
            ..root()
        };
        assert_eq!(
            listed_ids(&[archived_root, child()], SessionFilter::Archived, ""),
            ["root", "child"]
        );
        assert!(listed_ids(&[root(), child()], SessionFilter::All, "not found").is_empty());
        let asking = SessionRecord {
            attention: attention(AttentionState::NeedsInput, "PermissionRequest", "now"),
            ..root()
        };
        assert_eq!(listed_ids(&[asking], SessionFilter::Attention, ""), ["root"]);
        let interrupted = SessionRecord {
            status: SessionStatus::Exited,
            exit_reason: Some(ExitReason::Interrupted),
            ..root()
        };
        assert_eq!(listed_ids(&[interrupted], SessionFilter::Attention, ""), ["root"]);
    }

    // covers: B-P8-12 the filter menu picks each filter by one key and counts the rows each would list
    #[test]
    fn filter_menu_keys_and_counts() {
        let picked: Vec<Option<SessionFilter>> = SESSION_FILTERS
            .iter()
            .map(|filter| {
                let key = match filter {
                    SessionFilter::Archived => "A".to_owned(),
                    SessionFilter::Attention => "!".to_owned(),
                    SessionFilter::All => "*".to_owned(),
                    other => other.as_str()[..1].to_owned(),
                };
                filter_for_key(&key)
            })
            .collect();
        assert_eq!(picked, SESSION_FILTERS.map(Some));
        assert_eq!(filter_for_key("x"), None);
        assert_eq!(filter_for_key("f"), None);
        let exited = SessionRecord {
            id: "gone".into(),
            status: SessionStatus::Exited,
            exit_reason: Some(ExitReason::Failed),
            ..root()
        };
        let list = [root(), child(), exited];
        // The archived view lists only the archived child (no parent for context); search narrows every count.
        assert_eq!(
            session_filter_counts(&list, "", None),
            SessionFilterCounts {
                active: 2,
                running: 1,
                attention: 1,
                exited: 1,
                archived: 1,
                all: 3
            }
        );
        assert_eq!(
            session_filter_counts(&list, "special", None),
            SessionFilterCounts {
                active: 0,
                running: 0,
                attention: 0,
                exited: 0,
                archived: 1,
                all: 2
            }
        );
    }

    // covers: B-P8-13 handoffs include notes by default, and clearly label optional terminal excerpts
    #[test]
    fn handoffs_include_notes_and_label_excerpts() {
        let with_notes = SessionRecord {
            notes: Some("Implement the feature".into()),
            last_preview: Some("sensitive terminal output".into()),
            ..root()
        };
        let plain = handoff_markdown(&with_notes, false, None, None);
        assert!(plain.contains("Implement the feature"));
        assert!(!plain.contains("sensitive terminal output"));
        assert!(handoff_markdown(&with_notes, true, None, None).contains("not a complete transcript"));
        // The done marker (D) is stated; search ignores it.
        let done = SessionRecord {
            done_at: Some("2026-10-05T12:00:00.000Z".into()),
            ..with_notes.clone()
        };
        assert!(
            handoff_markdown(&done, false, None, None).contains("\nStatus: marked done 2026-10-05T12:00:00.000Z\n")
        );
        assert!(!plain.contains("Status: marked done"));
        // The work's task comes first (its steps are in the notes).
        let task = HandoffTask {
            title: "Add OAuth".into(),
            body: "with PKCE".into(),
            done: false,
        };
        assert!(
            handoff_markdown(&with_notes, false, None, Some(&task))
                .contains("## Task\n\n[ ] Add OAuth\n\nwith PKCE\n\n## Notes")
        );
        assert!(!plain.contains("## Task"));
        let done_root = SessionRecord {
            done_at: Some("2026-10-05T12:00:00.000Z".into()),
            ..root()
        };
        assert!(listed_ids(&[done_root], SessionFilter::Active, "done").is_empty());
    }

    #[test]
    fn filter_for_key_parity() {
        replay(UNIT, "sessionFeatures-P8/filterForKey", |case: &Case| -> Outcome {
            match case.arg_str(0) {
                Ok(input) => undefined_or(filter_for_key(input)),
                Err(error) => Outcome::throws(error),
            }
        });
    }

    #[test]
    fn session_blocked_parity() {
        replay(UNIT, "sessionFeatures-P8/sessionBlocked", |case: &Case| {
            Ok::<_, String>(session_blocked(&partial_session(case.arg(0))?))
        });
    }

    #[test]
    fn session_has_unread_reply_parity() {
        replay(UNIT, "sessionFeatures-P8/sessionHasUnreadReply", |case: &Case| {
            Ok::<_, String>(session_has_unread_reply(&partial_session(case.arg(0))?))
        });
    }

    #[test]
    fn session_needs_attention_parity() {
        replay(UNIT, "sessionFeatures-P8/sessionNeedsAttention", |case: &Case| {
            Ok::<_, String>(session_needs_attention(&partial_session(case.arg(0))?))
        });
    }

    #[test]
    fn with_reply_read_parity() {
        replay(
            UNIT,
            "sessionFeatures-P8/withReplyRead",
            |case: &Case| -> Result<Value, String> {
                let input = tags::untag(case.arg(0));
                let read_replies: IndexMap<String, String> = if case.arg_is_undefined(1) {
                    IndexMap::new()
                } else {
                    case.arg_as(1)?
                };
                let result = with_reply_read(partial_session(&input)?, &read_replies);
                let value = serde_json::to_value(result).map_err(|error| error.to_string())?;
                Ok(prune_to(value, &input, &["replyRead"]))
            },
        );
    }

    #[test]
    fn filter_session_list_parity() {
        replay(
            UNIT,
            "sessionFeatures-P8/filterSessionList",
            |case: &Case| -> Result<Value, String> {
                let input = tags::untag(case.arg(0));
                let sessions = sessions_arg(&input)?;
                let listed =
                    filter_session_list(&sessions, case.arg_as(1)?, case.arg_str(2)?, task_title_arg(case, 3)?);
                // The sessions come back as given: the input literals of the listed ones, in order.
                let given = input.as_array().cloned().unwrap_or_default();
                Ok(Value::Array(
                    listed
                        .into_iter()
                        .filter_map(|listed| {
                            let at = sessions.iter().position(|item| std::ptr::eq(item, listed))?;
                            given.get(at).cloned()
                        })
                        .collect(),
                ))
            },
        );
    }

    #[test]
    fn session_filter_counts_parity() {
        replay(UNIT, "sessionFeatures-P8/sessionFilterCounts", |case: &Case| {
            let sessions = sessions_arg(&tags::untag(case.arg(0)))?;
            Ok::<_, String>(session_filter_counts(
                &sessions,
                case.arg_str(1)?,
                task_title_arg(case, 2)?,
            ))
        });
    }

    #[test]
    fn handoff_git_lines_parity() {
        replay(UNIT, "sessionFeatures-P8/handoffGitLines", |case: &Case| {
            Ok::<_, String>(handoff_git_lines(&case.arg_as(0)?))
        });
    }

    #[test]
    fn handoff_markdown_parity() {
        replay(UNIT, "sessionFeatures-P8/handoffMarkdown", |case: &Case| {
            let session = partial_session(case.arg(0))?;
            let include_output = !case.arg_is_undefined(1) && case.arg_bool(1)?;
            let git: Option<HandoffGitContext> = if case.arg_is_undefined(2) {
                None
            } else {
                Some(case.arg_as(2)?)
            };
            let task = if case.arg_is_undefined(3) {
                None
            } else {
                let task = tags::untag(case.arg(3));
                let text = |key: &str| {
                    task.get(key)
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .ok_or(format!("task {key}"))
                };
                Some(HandoffTask {
                    title: text("title")?,
                    body: text("body")?,
                    // A missing `done` is falsy, as in TS.
                    done: task.get("done").and_then(Value::as_bool).unwrap_or(false),
                })
            };
            Ok::<_, String>(handoff_markdown(&session, include_output, git.as_ref(), task.as_ref()))
        });
    }
}
