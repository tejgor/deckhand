//! Wire data types: sessions, panes, worktrees, agent signals, project config and its documents, settings, changes,
//! tasks.
//!
//! Field order is the order the TS code that builds each object writes its keys (object literal order, spreads
//! included), which is what `JSON.stringify` puts on the wire; it often differs from the interface's declaration
//! order. Records whose key order depends on their history are carried as [`Ordered`] by the messages that hold them.
//! `?: T | null` fields are `Option<Option<T>>` (absent / null / value).

use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use serde_with::skip_serializing_none;

use crate::order::Ordered;
use crate::screen::PreviewScreen;

/// Declares a string union: an enum whose variants (de)serialise as the given strings.
macro_rules! string_union {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident = $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name {
            $($(#[$vmeta])* #[serde(rename = $text)] $variant,)+
        }
        impl $name {
            /// Every value, in the TS declaration order.
            pub const ALL: &'static [$name] = &[$($name::$variant),+];
            /// The wire string.
            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $text,)+ }
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.as_str()) }
        }
    };
}
pub(crate) use string_union;

/// A JS number that may be fractional: serialised the way `JSON.stringify` writes it (integral values without `.0`).
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Default)]
pub struct JsNumber(pub f64);

impl JsNumber {
    /// `JSON.stringify` of the number (non-finite values are written as `null`, as JS does).
    pub fn to_js_string(self) -> String {
        let n = self.0;
        if !n.is_finite() {
            return "null".to_owned();
        }
        if n == 0.0 {
            return "0".to_owned();
        }
        if n.fract() == 0.0 && n.abs() < 1e21 {
            return format!("{n:.0}");
        }
        // Shortest round-trip digits with a signed exponent, as JS writes them.
        serde_json::Number::from_f64(n)
            .map(|number| number.to_string())
            .unwrap_or_default()
    }
}

impl Serialize for JsNumber {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let n = self.0;
        if n.is_finite() && n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_992.0 {
            // Exact: |n| <= 2^53.
            #[allow(clippy::cast_possible_truncation)]
            return serializer.serialize_i64(n as i64);
        }
        if !n.is_finite() {
            return serializer.serialize_unit();
        }
        serializer.serialize_f64(n)
    }
}

impl<'de> Deserialize<'de> for JsNumber {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        f64::deserialize(deserializer).map(JsNumber)
    }
}

string_union!(ProgramKey { Claude = "claude", Pi = "pi", Codex = "codex" });
string_union!(SessionStatus { Starting = "starting", Running = "running", Exited = "exited" });
string_union!(AgentActivityStatus { Unknown = "unknown", Active = "active", Idle = "idle" });
string_union!(WorktreeMode { None = "none", New = "new", Existing = "existing" });
string_union!(SessionWorktreeMode { None = "none", Managed = "managed", Attached = "attached" });
string_union!(AttachTarget { Agent = "agent", Terminal = "terminal", Git = "git", Dev = "dev", Action = "action" });
string_union!(RightPaneTab { Preview = "preview", Terminal = "terminal", Git = "git", Dev = "dev", Notes = "notes" });
string_union!(WorktreeMergeMode { Merge = "merge", Squash = "squash" });
string_union!(RestartMode { Resume = "resume", Fresh = "fresh" });
string_union!(WorktreeOrigin { Created = "created", Existing = "existing", Selected = "selected" });
string_union!(WorktreeCreator {
    /// A repository creation script made it (worktrees from before Deckhand stopped running those; still read).
    Script = "script",
    Fallback = "fallback",
    Picker = "picker",
});
string_union!(MergeDetected { Ancestor = "ancestor", Pr = "pr" });
string_union!(SubSessionKind { Clean = "clean", Forked = "forked" });
string_union!(AgentSessionRefKind { Name = "name", Path = "path", Id = "id" });
string_union!(
    /// agentSignals.ts AttentionState.
    AttentionState {
        Unknown = "unknown",
        Working = "working",
        NeedsInput = "needs-input",
        ResponseEnded = "response-ended",
        Failed = "failed",
        Limited = "limited",
    }
);
string_union!(AttentionReasonKind {
    Question = "question",
    Plan = "plan",
    Permission = "permission",
    Message = "message",
    Failure = "failure",
    Notice = "notice",
});
string_union!(ExitReason { Stopped = "stopped", Failed = "failed", Completed = "completed", Interrupted = "interrupted" });
string_union!(SetupState {
    Pending = "pending",
    Running = "running",
    Failed = "failed",
    Cancelled = "cancelled",
    Complete = "complete",
});
string_union!(SharedNoteKind { Worktree = "worktree", Repo = "repo" });
string_union!(BranchFrom { Current = "current", Default = "default", Origin = "origin" });
string_union!(TargetOperation {
    Merge = "merge",
    Rebase = "rebase",
    CherryPick = "cherry-pick",
    Revert = "revert",
    Conflicts = "conflicts",
});
string_union!(WorktreeMissing { Prunable = "prunable", Unlisted = "unlisted" });
string_union!(WorktreeInUse { This = "this", Other = "other" });
string_union!(WorktreeRemoved { Deleted = "deleted", Pruned = "pruned", Forgotten = "forgotten" });
string_union!(NoteSection { Session = "session", Shared = "shared" });
string_union!(StageMode { Stage = "stage", Unstage = "unstage" });
string_union!(MoveDirection { Up = "up", Down = "down" });
string_union!(ResolveMergeAction { Keep = "keep", Abort = "abort" });
string_union!(ConfigTargetKind { Global = "global", Repository = "repository" });
string_union!(SaveTrust { Kept = "kept", Created = "created", Unreviewed = "unreviewed" });
string_union!(SettingSource {
    Repo = "repo",
    Global = "global",
    LegacyDevCommand = "legacy dev_command",
    BuiltInDefault = "built-in default",
    NotSet = "not set",
});
string_union!(RepositoryState {
    Trusted = "trusted",
    Untrusted = "untrusted",
    Absent = "absent",
    Invalid = "invalid",
    Bare = "bare",
    None = "none",
});
string_union!(CodexHookState { Missing = "missing", Other = "other" });
string_union!(CandidateKind { Dir = "dir", File = "file", Symlink = "symlink" });
string_union!(CandidateSuggestion { Link = "link", Skip = "skip" });
string_union!(CandidateConfigured { Symlink = "symlink", Files = "files" });
string_union!(ChangeGroup {
    Conflicts = "conflicts",
    Staged = "staged",
    Unstaged = "unstaged",
    Untracked = "untracked",
    /// Not in the TS: a file the branch's commits changed since its base.
    Committed = "committed",
});
string_union!(CleanupFileState { Changed = "changed", Untracked = "untracked", Ignored = "ignored" });
string_union!(PrChecks { Passing = "passing", Pending = "pending", Failing = "failing", Unknown = "unknown" });
string_union!(TaskAuto { Merge = "merge", Done = "done" });

// ---------------------------------------------------------------------------------------------------------------
// Sessions (types.ts)

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentSessionRef {
    pub provider: ProgramKey,
    pub kind: AgentSessionRefKind,
    pub value: String,
    /// Keys this version does not know (kept, as the TS spreads keep them).
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// Worktree settings links applied when the worktree was created (worktreeLinks.ts LinkResult).
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WorktreeLinks {
    pub linked: Vec<String>,
    pub notes: Vec<String>,
    /// Keys this version does not know (kept, as the TS spreads keep them).
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// types.ts SessionWorktreeRecord. Order: creation literal (git.ts), then baseRef and id as the daemon adds them,
/// then the projected markers in WORKTREE_MARKERS order.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionWorktreeRecord {
    pub mode: SessionWorktreeMode,
    pub path: Option<String>,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub is_main: Option<bool>,
    pub origin: Option<WorktreeOrigin>,
    pub creator: Option<WorktreeCreator>,
    pub name: Option<String>,
    pub links: Option<WorktreeLinks>,
    pub base_ref: Option<String>,
    pub id: Option<String>,
    pub merged_at: Option<String>,
    pub merge_mode: Option<WorktreeMergeMode>,
    pub merge_target_branch: Option<String>,
    pub merge_source_ref: Option<String>,
    pub merge_source_sha: Option<String>,
    pub merge_marked_manually: Option<bool>,
    pub merge_detected: Option<MergeDetected>,
    pub deleted_at: Option<String>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

impl SessionWorktreeRecord {
    pub fn new(mode: SessionWorktreeMode) -> Self {
        SessionWorktreeRecord {
            mode,
            path: None,
            branch: None,
            head: None,
            is_main: None,
            origin: None,
            creator: None,
            name: None,
            links: None,
            base_ref: None,
            id: None,
            merged_at: None,
            merge_mode: None,
            merge_target_branch: None,
            merge_source_ref: None,
            merge_source_sha: None,
            merge_marked_manually: None,
            merge_detected: None,
            deleted_at: None,
            extra: IndexMap::new(),
        }
    }
}

/// The merge and deletion markers of a worktree (types.ts WorktreeMarkers), in WORKTREE_MARKERS order.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeMarkers {
    pub merged_at: Option<String>,
    pub merge_mode: Option<WorktreeMergeMode>,
    pub merge_target_branch: Option<String>,
    pub merge_source_ref: Option<String>,
    pub merge_source_sha: Option<String>,
    pub merge_marked_manually: Option<bool>,
    pub merge_detected: Option<MergeDetected>,
    pub deleted_at: Option<String>,
}

/// agentSignals.ts AttentionReason: held in memory only, never persisted.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AttentionReason {
    pub kind: AttentionReasonKind,
    pub text: String,
    pub options: Option<Vec<String>>,
}

/// SessionRecord.attention: the latest agent signal.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionAttention {
    pub state: AttentionState,
    pub event: String,
    pub at: String,
    pub reason: Option<AttentionReason>,
    /// Keys this version does not know (kept, as the TS spreads keep them).
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// SessionRecord.setup: the setup command's progress.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSetup {
    pub command: String,
    pub state: SetupState,
    pub output: String,
    #[serde(default, with = "::serde_with::rust::double_option")]
    pub exit_code: Option<Option<i64>>,
    /// Keys this version does not know (kept, as the TS spreads keep them).
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// A note file as the daemon last read it (types.ts NoteFile).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteFile {
    pub path: String,
    pub revision: String,
    pub too_large: Option<bool>,
}

/// A worktree's (or the main checkout's) note shared by its sessions (types.ts SharedNote).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedNote {
    pub kind: SharedNoteKind,
    pub id: String,
    pub path: String,
    pub text: String,
    pub revision: String,
    pub too_large: Option<bool>,
    pub read_only: Option<bool>,
}

/// types.ts SessionRecord. Order: the create literal (daemon.ts createSession), then keys in the order a session's
/// life usually adds them. Its real key order is history-dependent: messages carry it as `Ordered<SessionRecord>`.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    pub id: String,
    pub launch_id: Option<String>,
    pub title: String,
    pub program: ProgramKey,
    pub command: String,
    pub args: Option<Vec<String>>,
    pub cwd: String,
    pub repo_root: String,
    pub launch_cwd: Option<String>,
    pub launch_worktree_root: Option<String>,
    pub worktree: Option<SessionWorktreeRecord>,
    pub requested_worktree_mode: Option<WorktreeMode>,
    pub handoff_path: Option<String>,
    pub start_prompt: Option<String>,
    pub status: SessionStatus,
    pub agent_status: Option<AgentActivityStatus>,
    pub agent_status_updated_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub last_preview: Option<String>,
    pub parent_session_id: Option<String>,
    pub sub_session_kind: Option<SubSessionKind>,
    pub forked_from_session_id: Option<String>,
    pub forked_from_agent_session_ref: Option<AgentSessionRef>,
    /// Any JS number (TS writes back what it read; sessionOrder.ts orderValue reads a non-finite one as 0).
    pub sidebar_order: Option<JsNumber>,
    pub notes: Option<String>,
    pub notes_file: Option<NoteFile>,
    pub shared_notes: Option<SharedNote>,
    pub agent_session_ref: Option<AgentSessionRef>,
    pub pid: Option<i64>,
    pub agent_version: Option<String>,
    pub agent_started_at: Option<String>,
    pub attention: Option<SessionAttention>,
    pub dev_running: Option<bool>,
    pub setup: Option<SessionSetup>,
    pub exit_reason: Option<ExitReason>,
    #[serde(default, with = "::serde_with::rust::double_option")]
    pub exit_code: Option<Option<i64>>,
    #[serde(default, with = "::serde_with::rust::double_option")]
    pub exit_signal: Option<Option<i64>>,
    pub cleanup_error: Option<String>,
    pub done_at: Option<String>,
    pub archived_at: Option<String>,
    pub merged_at: Option<String>,
    pub merge_target_branch: Option<String>,
    pub merge_source_ref: Option<String>,
    pub merge_marked_manually: Option<bool>,
    /// Not in the TS: when you last read the session's reply (detached from its agent, or `U`); the reply that ended
    /// its last turn is read when this is not before `attention.at`. `U` clears it to mark the reply unread.
    pub reply_read_at: Option<String>,
    /// UI only: never sent by the daemon (derived from `reply_read_at` by `with_reply_read`).
    pub reply_read: Option<bool>,
    /// Not in the TS: a sub-session's path (`parent / child`), projected by the daemon and never saved. The daemon
    /// holds every parent; a client may not (a checkout's list leaves out parents that work elsewhere).
    pub path_title: Option<String>,
    /// Keys this version does not know (kept, as the TS spreads keep them).
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// types.ts WorktreeInfoRecord (`list-worktrees`).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeInfoRecord {
    pub path: String,
    pub branch: String,
    pub head: String,
    pub is_main: bool,
}

/// One running session a worktree deletion would stop.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionTitle {
    pub id: String,
    pub title: String,
}

/// workspaceGit.ts CleanupFile.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CleanupFile {
    pub path: String,
    pub state: CleanupFileState,
}

/// workspaceGit.ts CleanupInspection (inspectWorkspaceCleanup literal order).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupInspection {
    pub safe: bool,
    pub reasons: Vec<String>,
    pub dirty_files: i64,
    pub untracked_files: i64,
    pub ignored_files: i64,
    pub files: Option<Vec<CleanupFile>>,
    pub unpublished_commits: Option<i64>,
    pub unmerged_commits: Option<i64>,
}

/// types.ts SessionCleanupInspection: `{...inspection, safe, structuralBlockers}`.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCleanupInspection {
    #[serde(flatten)]
    pub inspection: CleanupInspection,
    pub structural_blockers: Vec<String>,
}

/// types.ts WorktreeCleanupInspection: the session inspection plus the sessions deleting it stops.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorktreeCleanupInspection {
    #[serde(flatten)]
    pub inspection: SessionCleanupInspection,
    pub running: Vec<SessionTitle>,
}

/// types.ts WorktreeOverviewEntry (daemon.ts worktreeOverview literal order).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeOverviewEntry {
    pub path: String,
    pub branch: String,
    pub head: String,
    pub is_main: bool,
    pub locked: Option<bool>,
    pub missing: Option<WorktreeMissing>,
    pub record_id: Option<String>,
    pub created_at: Option<String>,
    pub markers: Option<WorktreeMarkers>,
    pub session_ids: Vec<String>,
    pub running_ids: Vec<String>,
    pub in_use: Option<WorktreeInUse>,
    pub error: Option<String>,
    pub last_commit_at: Option<String>,
    pub ahead_of_default: Option<i64>,
    pub inspection: Option<CleanupInspection>,
}

/// types.ts WorktreeOverview.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeOverview {
    pub main_root: Option<String>,
    pub default_branch: Option<String>,
    pub entries: Vec<WorktreeOverviewEntry>,
    pub unchecked: Option<i64>,
    pub checked_at: String,
}

/// types.ts WorktreeDeleteResult.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeDeleteResult {
    pub removed: WorktreeRemoved,
    pub branch_deleted: Option<String>,
    pub stopped: i64,
    pub archived: i64,
    pub sent_items: Option<i64>,
}

/// WorktreeMergeResult.committed: the worktree's changes committed before merging.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommittedChanges {
    pub files: i64,
    pub sha: String,
}

/// types.ts WorktreeMergeResult (git.ts mergeWorktreeIntoCurrent). Its literals place `committed` and `reason`
/// differently per outcome, so messages carry it as `Ordered<WorktreeMergeResult>`.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeMergeResult {
    pub mode: WorktreeMergeMode,
    pub source_ref: String,
    pub source_sha: Option<String>,
    pub target_branch: String,
    pub skipped: Option<bool>,
    pub conflicted: Option<bool>,
    pub conflicts: Option<Vec<String>>,
    pub conflict_count: Option<i64>,
    pub committed: Option<CommittedChanges>,
    pub reason: Option<String>,
    pub stdout: String,
    pub stderr: String,
    pub sent_items: Option<i64>,
}

/// MergePreview.diff.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DiffStat {
    pub files: i64,
    pub insertions: i64,
    pub deletions: i64,
}

/// MergePreview.overlap.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MergeOverlap {
    pub committed: Vec<String>,
    pub uncommitted: Vec<String>,
}

/// A commit a merge would bring in (not in the TS): its short hash, commit time (Unix seconds) and subject. A bare
/// subject, as the TS sent, still reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MergeCommit {
    Detail { hash: String, time: i64, subject: String },
    Subject(String),
}

impl MergeCommit {
    pub fn subject(&self) -> &str {
        match self {
            MergeCommit::Detail { subject, .. } | MergeCommit::Subject(subject) => subject,
        }
    }
}

/// types.ts MergePreview (workspaceGit.ts getMergePreview literal order).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergePreview {
    pub source_ref: String,
    pub source_sha: Option<String>,
    pub target_root: String,
    pub target_branch: Option<String>,
    pub target_is_main: bool,
    pub default_branch: Option<String>,
    pub commit_count: i64,
    pub commits: Vec<MergeCommit>,
    pub diff: DiffStat,
    pub uncommitted: i64,
    pub overlap: MergeOverlap,
    pub in_progress: Option<TargetOperation>,
}

/// types.ts TasksDoc.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksDoc {
    pub key: String,
    pub path: String,
    pub text: String,
    pub revision: String,
    pub too_large: Option<bool>,
}

/// types.ts BranchList.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchList {
    pub branches: Vec<String>,
    pub current: Option<String>,
    pub default_branch: Option<String>,
    pub branch_from: BranchFrom,
}

/// types.ts NoteSaveResult.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoteSaveResult {
    pub saved: bool,
    pub session: Ordered<SessionRecord>,
}

/// `promote-note-item` result.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PromoteNoteItemResult {
    pub session: Ordered<SessionRecord>,
    pub tasks: TasksDoc,
}

/// `send-open-items` result.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SendOpenItemsResult {
    pub session: Ordered<SessionRecord>,
    pub tasks: TasksDoc,
    pub sent: i64,
}

/// types.ts AgentVersionInfo (agentVersionCheck.ts snapshot literal order).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentVersionInfo {
    pub program: ProgramKey,
    pub installed: Option<String>,
    pub latest: Option<String>,
    pub path: Option<String>,
    pub checked_at: Option<String>,
    pub error: Option<String>,
    pub updating: Option<bool>,
    pub running: i64,
    pub outdated: i64,
}

/// types.ts AgentVersions: `Record<ProgramKey, AgentVersionInfo>` built in AGENT_PROGRAMS order.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentVersions {
    pub claude: AgentVersionInfo,
    pub pi: AgentVersionInfo,
    pub codex: AgentVersionInfo,
}

/// types.ts AgentUpdateResult: `{program, ...outcome, versions}`.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUpdateResult {
    pub program: ProgramKey,
    pub ok: bool,
    #[serialize_always]
    pub exit_code: Option<i64>,
    pub command: String,
    pub output: String,
    pub before: Option<String>,
    pub after: Option<String>,
    pub versions: AgentVersions,
}

/// types.ts PreviewRecord (session worker snapshot literal order; `...scrollInfo` last, then `screen`).
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRecord {
    pub session_id: Option<String>,
    pub content: String,
    pub live: bool,
    pub status: Option<SessionStatus>,
    pub agent_status: Option<AgentActivityStatus>,
    pub scroll_offset: Option<i64>,
    pub max_scroll_offset: Option<i64>,
    /// Not in the TS: the same rows as `content` with their colours, the cursor and the input modes, only while a UI
    /// types to the agent in its Preview.
    pub screen: Option<PreviewScreen>,
}

/// The record shared by the workspace panes (types.ts TerminalRecord, GitRecord, DevRecord, ActionRecord): one shape
/// serves every pane, as in the daemon (`WorkspacePaneRecord`). Order: the worker's `record()` literal, then the
/// `sessionId`/`workspace` stamp the daemon appends; other literals differ, so messages carry `Ordered<PaneRecord>`.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaneRecord {
    pub content: String,
    pub live: bool,
    pub cwd: Option<String>,
    pub command: Option<String>,
    pub name: Option<String>,
    #[serde(default, with = "::serde_with::rust::double_option")]
    pub exit_code: Option<Option<i64>>,
    #[serde(default, with = "::serde_with::rust::double_option")]
    pub exit_signal: Option<Option<i64>>,
    /// Not in the TS: Dev's named processes (`devCommand` as a map), one PTY each, in the configured order. Absent for a
    /// single Dev command, whose output is the record's own `content`.
    pub processes: Option<Vec<DevProcessRecord>>,
    /// Not in the TS: the Terminal's shells, oldest first, while there is more than one. The first (id 0) is also
    /// the record's own `content` and `live`. Absent with a single shell.
    pub shells: Option<Vec<ShellRecord>>,
    /// Not in the TS: the first shell's styled screen, while a UI types into it.
    pub screen: Option<PreviewScreen>,
    pub session_id: Option<String>,
    pub workspace: Option<String>,
}

/// Not in the TS: one shell of a [`PaneRecord`] (the Terminal tab). Ids never change while it runs; the UI labels
/// shells by position. An extra shell that exits is gone; the first starts again on the next view.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellRecord {
    pub id: i64,
    pub content: String,
    pub live: bool,
    /// Its styled screen, while a UI types into it.
    pub screen: Option<PreviewScreen>,
}

/// Not in the TS: one named Dev process of a [`PaneRecord`]. A configured process that is not running has no
/// output and `live: false`.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevProcessRecord {
    pub name: String,
    pub content: String,
    pub live: bool,
    pub command: Option<String>,
    pub exit_code: Option<i64>,
    pub exit_signal: Option<i64>,
}

pub type TerminalRecord = PaneRecord;
pub type GitRecord = PaneRecord;
pub type DevRecord = PaneRecord;
pub type ActionRecord = PaneRecord;

/// The bracketed-paste state an attach mirrors.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalModes {
    pub bracketed_paste: bool,
}

/// What a worker answers to `attach`: its snapshot (a PreviewRecord for the agent, a PaneRecord for a workspace pane)
/// plus, while the PTY exists, `{terminalModes, initialFrame}`.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachSnapshot {
    pub session_id: Option<String>,
    pub content: String,
    pub live: bool,
    pub cwd: Option<String>,
    pub command: Option<String>,
    pub name: Option<String>,
    #[serde(default, with = "::serde_with::rust::double_option")]
    pub exit_code: Option<Option<i64>>,
    #[serde(default, with = "::serde_with::rust::double_option")]
    pub exit_signal: Option<Option<i64>>,
    pub status: Option<SessionStatus>,
    pub agent_status: Option<AgentActivityStatus>,
    pub scroll_offset: Option<i64>,
    pub max_scroll_offset: Option<i64>,
    pub terminal_modes: Option<TerminalModes>,
    pub initial_frame: Option<String>,
}

/// The daemon's answer to `attach` / `attach-<pane>`: `{...session, ...attachData}`. The snapshot's keys override the
/// session's in place (its `cwd` lands in the session's slot), so it is carried as `Ordered<AttachResult>`.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachResult {
    /// The session, with the snapshot's `cwd`, `command` (a Dev or action pane's), `status`, `agentStatus`,
    /// `exitCode` and `exitSignal` overriding its own, as the spread does.
    #[serde(flatten)]
    pub session: SessionRecord,
    /// The agent snapshot's session ID.
    pub session_id: Option<String>,
    pub content: String,
    pub live: bool,
    pub name: Option<String>,
    pub scroll_offset: Option<i64>,
    pub max_scroll_offset: Option<i64>,
    pub terminal_modes: Option<TerminalModes>,
    pub initial_frame: Option<String>,
}

/// types.ts CreateSessionInput.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionInput {
    pub title: String,
    pub program: ProgramKey,
    pub cwd: String,
    pub repo_root: String,
    pub cols: i64,
    pub rows: i64,
    pub worktree_mode: Option<WorktreeMode>,
    pub existing_worktree_path: Option<String>,
    pub parent_session_id: Option<String>,
    pub sub_session_kind: Option<SubSessionKind>,
    pub handoff_from_session_id: Option<String>,
    pub base_branch: Option<String>,
    pub task_id: Option<String>,
    pub project_fingerprint: Option<String>,
    /// Not in the TS: the session a top-level fork (`subSessionKind: "forked"`, no parent) copies the conversation of.
    pub fork_from_session_id: Option<String>,
}

// ---------------------------------------------------------------------------------------------------------------
// Project configuration (projectConfig.ts, projectConfigDocument.ts, worktreeLinks.ts)

/// worktreeLinks.ts WorktreeSettings (deckhand.json `worktree`).
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeSettings {
    pub location: Option<String>,
    pub symlink: Option<Vec<String>>,
    pub files: Option<IndexMap<String, String>>,
    pub branch_from: Option<BranchFrom>,
    pub branch_name: Option<String>,
}

/// projectConfig.ts ProjectConfig (a validated deckhand.json). Its key order is the file's, so it travels as
/// `Ordered<ProjectConfig>`.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectConfig {
    pub default_agent: Option<ProgramKey>,
    pub default_workspace: Option<WorktreeMode>,
    /// A command, or (not in the TS) named commands that run side by side.
    pub dev_command: Option<DevCommand>,
    pub setup_command: Option<String>,
    pub actions: Option<IndexMap<String, String>>,
    pub worktree: Option<WorktreeSettings>,
}

impl ProjectConfig {
    /// `devCommand` when it is one command.
    pub fn dev_command_text(&self) -> Option<&str> {
        self.dev_command.as_ref().and_then(DevCommand::text)
    }

    /// `devCommand` when it names several processes (not in the TS).
    pub fn dev_processes(&self) -> Option<&IndexMap<String, String>> {
        self.dev_command.as_ref().and_then(DevCommand::processes)
    }
}

/// `devCommand`: one command, or (not in the TS) a name → command map of processes that run side by side.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DevCommand {
    Command(String),
    Processes(IndexMap<String, String>),
}

impl DevCommand {
    /// The one command.
    pub fn text(&self) -> Option<&str> {
        match self {
            DevCommand::Command(command) => Some(command),
            DevCommand::Processes(_) => None,
        }
    }

    /// The named processes.
    pub fn processes(&self) -> Option<&IndexMap<String, String>> {
        match self {
            DevCommand::Command(_) => None,
            DevCommand::Processes(processes) => Some(processes),
        }
    }
}

/// projectConfig.ts LoadedProject.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedProject {
    pub root: String,
    pub trust_root: String,
    pub path: Option<String>,
    pub config: Ordered<ProjectConfig>,
    pub exists: bool,
    pub fingerprint: String,
}

/// types.ts ProjectInfo: `{...project, trusted, needsReview, effective}`.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    #[serde(flatten)]
    pub project: LoadedProject,
    pub trusted: bool,
    pub needs_review: bool,
    pub effective: Ordered<ProjectConfig>,
}

/// projectConfigDocument.ts ProjectConfigDocument.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectConfigDocument {
    pub kind: ConfigTargetKind,
    pub root: String,
    pub path: String,
    pub raw: String,
    /// `string | null`: always written.
    #[serialize_always]
    pub revision: Option<String>,
    pub exists: bool,
}

/// projectConfigDocument.ts SavedConfigDocument.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedConfigDocument {
    #[serde(flatten)]
    pub document: ProjectConfigDocument,
    pub trust: Option<SaveTrust>,
}

/// projectConfigDocument.ts ConfigTargets.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigTargets {
    pub global: Option<ProjectConfigDocument>,
    pub global_error: Option<String>,
    pub repository: Option<ProjectConfigDocument>,
    pub repository_error: Option<String>,
}

/// A setting's raw value: a string, or a boolean (an on/off choice).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StringOrBool {
    String(String),
    Bool(bool),
}

/// SettingRow.pending: the untrusted repository value.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingSetting {
    pub value: String,
    pub raw: StringOrBool,
    /// Always `true` when present.
    pub same: Option<bool>,
}

/// projectConfig.ts SettingRow (explainSettings literal order: `note` and `pending` are assigned after).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SettingRow {
    pub key: String,
    pub entry: Option<String>,
    pub value: String,
    pub source: SettingSource,
    pub raw: Option<StringOrBool>,
    pub note: Option<String>,
    pub pending: Option<PendingSetting>,
}

/// worktreeLinks.ts TemplateVars.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateVars {
    pub name: String,
    pub repo: String,
    pub repo_parent: String,
    pub repo_root: String,
    pub home: String,
}

/// SettingsInfo.repository.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RepositoryInfo {
    pub state: RepositoryState,
    pub path: Option<String>,
    pub error: Option<String>,
}

/// settingsInfo.ts CodexHookStatus.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexHookStatus {
    pub state: CodexHookState,
    pub file: String,
    pub file_exists: bool,
    pub command: String,
}

/// Not in the TS: whether macOS lets Deckhand.app show notifications.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NotificationPermission {
    Allowed,
    /// Turned off in System Settings › Notifications.
    Blocked,
    /// The permission prompt was never answered (or has not been shown yet).
    NotAsked,
}

/// settingsInfo.ts SettingsInfo (readSettingsInfo literal order).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsInfo {
    pub cwd: String,
    pub repo: String,
    pub repository: RepositoryInfo,
    pub needs_review: bool,
    pub global_error: Option<String>,
    pub rows: Vec<SettingRow>,
    pub targets: ConfigTargets,
    pub vars: Option<TemplateVars>,
    pub default_location: String,
    pub inside_ignored: bool,
    pub default_branch: Option<String>,
    pub origin_branch: Option<String>,
    pub user: String,
    pub agent_hooks: Option<bool>,
    pub notifications: bool,
    pub codex_hooks: Option<CodexHookStatus>,
    /// Not in the TS: macOS's answer for Deckhand.app, read while Notifications are on (`None` elsewhere).
    pub notification_permission: Option<NotificationPermission>,
    /// Not in the TS: `scroll_speed` as stored in config.json (`None`: unset, the default).
    pub scroll_speed: Option<f64>,
}

/// settingsInfo.ts WorktreeCandidate. Its literals differ in where `source`, `layers`, `repos` and `missing` go, so
/// candidates travel as `Ordered<WorktreeCandidate>`.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorktreeCandidate {
    pub path: String,
    pub kind: CandidateKind,
    pub target: Option<String>,
    pub ignored: bool,
    pub suggestion: CandidateSuggestion,
    pub reason: String,
    pub configured: Option<CandidateConfigured>,
    pub source: Option<String>,
    pub layers: Option<Vec<ConfigTargetKind>>,
    pub repos: Option<Vec<String>>,
    pub missing: Option<bool>,
    pub added: Option<bool>,
}

/// settingsInfo.ts WorktreeCandidates (`worktree-candidates`, `global-link-candidates`).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeCandidates {
    pub checkout: String,
    pub candidates: Vec<Ordered<WorktreeCandidate>>,
    pub more_candidates: i64,
    pub repos: Option<Vec<String>>,
    pub candidates_error: Option<String>,
}

/// `worktree-candidate-sizes`: KiB per path, `null` when unknown.
pub type CandidateSizes = IndexMap<String, Option<i64>>;

// ---------------------------------------------------------------------------------------------------------------
// Git views (workspaceGit.ts, changesModel.ts)

/// workspaceGit.ts PullRequestInfo.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestInfo {
    pub number: i64,
    pub url: String,
    pub state: String,
    pub checks: PrChecks,
    pub head_sha: Option<String>,
    pub base_branch: Option<String>,
}

/// workspaceGit.ts WorkspaceSummary (getWorkspaceSummary literal and assignment order).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSummary {
    pub cwd: String,
    pub branch: String,
    pub head: String,
    pub base_ref: Option<String>,
    pub changed_files: i64,
    pub untracked_files: i64,
    pub additions: i64,
    pub deletions: i64,
    pub ahead: Option<i64>,
    pub behind: Option<i64>,
    pub pr: Option<PullRequestInfo>,
    pub pr_error: Option<String>,
    pub commits_ahead_of_base: Option<i64>,
    pub push_remote: Option<String>,
}

/// workspaceGit.ts CreatePrResult.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreatePrResult {
    pub branch: String,
    pub remote: String,
    pub base: Option<String>,
    pub existing: bool,
}

/// changesModel.ts ChangeEntry (changesGit.ts literal order; line counts last).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeEntry {
    pub group: ChangeGroup,
    pub path: String,
    pub orig_path: Option<String>,
    pub status: String,
    pub conflict: Option<String>,
    pub additions: Option<i64>,
    pub deletions: Option<i64>,
    pub binary: Option<bool>,
}

/// changesModel.ts ChangeCounts: `Record<ChangeGroup, number>` in CHANGE_GROUPS order.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ChangeCounts {
    pub conflicts: i64,
    pub staged: i64,
    pub unstaged: i64,
    pub untracked: i64,
    /// Not in the TS (left out while 0): files the branch's commits changed since its base.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub committed: i64,
}

fn is_zero(count: &i64) -> bool {
    *count == 0
}

/// changesModel.ts ChangesRecord: `emptyChanges` order, `...fields` appended, then the daemon's stamp.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangesRecord {
    pub loaded: bool,
    pub entries: Vec<ChangeEntry>,
    pub counts: ChangeCounts,
    pub omitted: ChangeCounts,
    pub branch: Option<String>,
    pub error: Option<String>,
    pub session_id: Option<String>,
    pub workspace: Option<String>,
}

/// changesModel.ts ChangeDiff.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeDiff {
    pub group: ChangeGroup,
    pub path: String,
    pub text: String,
    pub truncated: bool,
    pub binary: bool,
    pub first_line: i64,
}

/// `change-stage` result: `{...applyStage(), changes}`.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeStageResult {
    pub changed: i64,
    pub skipped_conflicts: i64,
    pub changes: ChangesRecord,
}

// ---------------------------------------------------------------------------------------------------------------
// Tasks (tasks.ts)

/// tasks.ts TaskLink: `{wt}` (a worktree record) or `{s}` (a session).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TaskLink {
    Worktree { wt: String },
    Session { s: String },
}

/// tasks.ts TaskMeta.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TaskMeta {
    pub t: Option<String>,
    pub wt: Option<String>,
    pub s: Option<String>,
    pub done: Option<String>,
    pub auto: Option<TaskAuto>,
    pub tried: Option<String>,
    pub from: Option<String>,
    pub added: Option<String>,
}

/// tasks.ts Task.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub done: bool,
    pub body: String,
    pub meta: TaskMeta,
}

/// tasks.ts TaskOp: one change to a task list (internally tagged on `type`).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TaskOp {
    #[serde(rename = "add")]
    Add {
        title: String,
        body: Option<String>,
        link: Option<TaskLink>,
        id: Option<String>,
        from: Option<String>,
    },
    #[serde(rename = "edit")]
    Edit { id: String, title: String, body: String },
    #[serde(rename = "toggle")]
    Toggle { id: String },
    #[serde(rename = "remove")]
    Remove { id: String },
    #[serde(rename = "move")]
    Move {
        id: String,
        target: String,
        after: Option<bool>,
    },
    #[serde(rename = "link")]
    Link { id: String, link: TaskLink },
    #[serde(rename = "assign")]
    Assign { id: String, link: Option<TaskLink> },
    #[serde(rename = "tick-linked")]
    TickLinked { link: TaskLink, auto: TaskAuto },
    #[serde(rename = "reopen-linked")]
    ReopenLinked { link: TaskLink, auto: TaskAuto },
    #[serde(rename = "abandon-linked")]
    AbandonLinked { link: TaskLink, tried: Option<String> },
}

// ---------------------------------------------------------------------------------------------------------------
// Small results

/// `{ok: true}` (kill, remove, stop-dev, agent-hook, ...).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OkResult {
    pub ok: bool,
}

impl Default for OkResult {
    fn default() -> Self {
        OkResult { ok: true }
    }
}

/// `ping`: protocol version, the daemon's home and channel, and the daemon's build version and pid. The last two
/// came without a protocol bump: a client treats them as optional, since an older daemon of the same protocol
/// leaves them out.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PingResult {
    pub ok: bool,
    pub version: Option<u32>,
    pub home: Option<String>,
    pub channel: Option<String>,
    /// The daemon's [`crate::BUILD_VERSION`].
    pub build: Option<String>,
    /// The daemon's process id.
    pub pid: Option<u32>,
}

/// types.ts UiExitResult: what the UI process exits with (never on the wire; kept with the other shared types).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum UiExitResult {
    #[serde(rename = "quit")]
    Quit,
    #[serde(rename = "attach")]
    Attach {
        #[serde(rename = "sessionId")]
        session_id: String,
        target: AttachTarget,
        title: Option<String>,
        cwd: Option<String>,
        program: Option<ProgramKey>,
        /// Not in the TS: the Dev process to attach to (`devCommand` as a map).
        process: Option<String>,
        /// Not in the TS: the Terminal's shell to attach to (by id) and its label (`shell 2`); absent is the first.
        shell: Option<i64>,
        #[serde(rename = "shellLabel")]
        shell_label: Option<String>,
        /// Not in the TS: the session's path (a sub-session's `parent / child`, built from its parents' names), for the
        /// status line under the attach.
        #[serde(rename = "statusTitle")]
        status_title: Option<String>,
    },
    #[serde(rename = "edit-note")]
    EditNote {
        #[serde(rename = "sessionId")]
        session_id: String,
        file: String,
        editor: EditorCommand,
        /// Not in the TS: the session's title, for the status bar over the note.
        title: Option<String>,
    },
}

/// UiExitResult `edit-note`: the terminal editor to run.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EditorCommand {
    pub command: String,
    pub args: Vec<String>,
    pub label: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_numbers_print_like_json_stringify() {
        assert_eq!(serde_json::to_string(&JsNumber(30.0)).unwrap(), "30");
        assert_eq!(serde_json::to_string(&JsNumber(1.5)).unwrap(), "1.5");
        assert_eq!(serde_json::to_string(&JsNumber(-0.0)).unwrap(), "0");
        assert_eq!(serde_json::to_string(&JsNumber(f64::NAN)).unwrap(), "null");
        assert_eq!(JsNumber(1e21).to_js_string(), "1e+21");
        assert_eq!(JsNumber(1e20).to_js_string(), "100000000000000000000");
        assert_eq!(JsNumber(1.5e-7).to_js_string(), "1.5e-7");
    }

    #[test]
    fn nullable_fields_distinguish_absent_and_null() {
        let setup: SessionSetup =
            serde_json::from_str(r#"{"command":"x","state":"failed","output":"","exitCode":null}"#).unwrap();
        assert_eq!(setup.exit_code, Some(None));
        assert_eq!(
            serde_json::to_string(&setup).unwrap(),
            r#"{"command":"x","state":"failed","output":"","exitCode":null}"#
        );
        let setup: SessionSetup = serde_json::from_str(r#"{"command":"x","state":"running","output":""}"#).unwrap();
        assert_eq!(setup.exit_code, None);
        assert_eq!(
            serde_json::to_string(&setup).unwrap(),
            r#"{"command":"x","state":"running","output":""}"#
        );
    }

    #[test]
    fn task_links_are_untagged() {
        let op: TaskOp = serde_json::from_str(r#"{"type":"link","id":"a","link":{"s":"b"}}"#).unwrap();
        assert_eq!(
            op,
            TaskOp::Link {
                id: "a".into(),
                link: TaskLink::Session { s: "b".into() }
            }
        );
        assert_eq!(
            serde_json::to_string(&op).unwrap(),
            r#"{"type":"link","id":"a","link":{"s":"b"}}"#
        );
    }
}
