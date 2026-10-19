//! Port of daemon.ts (merge flow).
//!
//! Merging a session's linked worktree into a target checkout (merge or squash, optionally committing first), keeping
//! or aborting a conflicted merge Deckhand started, marking a worktree merged by hand, and detecting merges done
//! outside Deckhand (the branch tip now in the default branch, or its PR merged). Merge markers live on the worktree
//! record, so every session of the worktree changes together.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use deckhand_core::error::Result;
use deckhand_protocol::Ordered;
use deckhand_protocol::data::{
    MergeDetected, MergePreview, ResolveMergeAction, SessionRecord, WorkspaceSummary, WorktreeMarkers,
    WorktreeMergeMode, WorktreeMergeResult,
};
use deckhand_sys::async_util::{Interval, SharedTask};
use deckhand_sys::git::ConflictedMerge;

use crate::daemon::Daemon;

/// TS: daemon.ts MERGE_DETECT_MS: merges done outside Deckhand are looked for this often, plus at start.
pub const MERGE_DETECT_INTERVAL: Duration = Duration::from_millis(5 * 60_000);

/// TS: daemon.ts MERGE_DETECT_MAX: at most this many worktree records are checked per run.
pub const MERGE_DETECT_MAX: usize = 50;

/// TS: daemon.ts PendingMerge: a conflicted merge Deckhand started and left in progress, until kept or aborted.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingMerge {
    /// TS `extends ConflictedMerge` (mode, target head, whether the index was clean).
    pub merge: ConflictedMerge,
    pub session_id: String,
    pub source_ref: String,
    pub source_sha: Option<String>,
    pub target_branch: String,
}

/// TS: InkDaemon.markDetectedMerge `found`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetectedMerge {
    pub target_branch: Option<String>,
    pub source_ref: String,
    pub source_sha: Option<String>,
    /// The branch tip the detection saw (a dismissed tip is not marked again).
    pub tip: String,
}

/// TS: InkDaemon.mergeSource result.
#[derive(Clone, Debug, PartialEq)]
pub struct MergeSource {
    pub session: Ordered<SessionRecord>,
    pub worktree_path: String,
}

/// The merge flow's part of the daemon state.
#[derive(Default)]
pub struct MergeState {
    /// TS `pendingMerges`: conflicted merges by target worktree root.
    pub pending: HashMap<String, PendingMerge>,
    /// TS `mergeDetectTimer`.
    pub timer: Option<Interval>,
    /// TS `mergeDetection`: the run in progress (one at a time; callers share it).
    pub detection: Option<SharedTask<()>>,
    /// TS `noBaseSha`: records whose starting commit could not be established (never checked).
    pub no_base_sha: HashSet<String>,
}

/// TS: daemon.ts defaultBranchRefs: `refs/heads/<default>`, plus `refs/remotes/origin/<default>` when fetched.
pub async fn default_branch_refs(repo_root: &str, default_branch: &str) -> Vec<String> {
    let _ = (repo_root, default_branch);
    todo!("D9")
}

impl Daemon {
    /// TS: InkDaemon.mergeSource (`session does not exist`, `session does not have a worktree to merge`, `cannot merge
    /// session because its worktree was deleted`).
    pub fn merge_source(&self, session_id: &str) -> Result<MergeSource> {
        let _ = session_id;
        todo!("D9")
    }

    /// TS: InkDaemon.mergePreview.
    pub async fn merge_preview(&self, session_id: &str, target_cwd: &str) -> Result<MergePreview> {
        let _ = (session_id, target_cwd);
        todo!("D9")
    }

    /// TS: InkDaemon.mergeSessionWorktree: merges (or records the conflict as pending, or the skip), marks the
    /// worktree merged and, when asked, sends its note's open items to the backlog.
    pub async fn merge_session_worktree(
        &self,
        session_id: &str,
        mode: WorktreeMergeMode,
        target_cwd: &str,
        commit_first: bool,
        send_open_items: bool,
    ) -> Result<Ordered<WorktreeMergeResult>> {
        let _ = (session_id, mode, target_cwd, commit_first, send_open_items);
        todo!("D9")
    }

    /// TS: InkDaemon.resolveMerge: `keep` leaves the conflicted merge in progress and marks the worktree merged;
    /// `abort` undoes it.
    pub async fn resolve_merge(
        &self,
        session_id: &str,
        target_cwd: &str,
        action: ResolveMergeAction,
    ) -> Result<Ordered<SessionRecord>> {
        let _ = (session_id, target_cwd, action);
        todo!("D9")
    }

    /// TS: InkDaemon.saveMergeMarkers: replaces the merge markers (`None`: clears them, remembering `dismissed_tip`)
    /// on the worktree record, else on the session.
    pub async fn save_merge_markers(
        &self,
        session_id: &str,
        markers: Option<WorktreeMarkers>,
        dismissed_tip: Option<String>,
    ) -> Result<Ordered<SessionRecord>> {
        let _ = (session_id, markers, dismissed_tip);
        todo!("D9")
    }

    /// TS: InkDaemon.markSessionMerged: toggles the manual merged marker (`session does not exist`, `cannot mark
    /// session merged because its worktree was deleted`, `Not in a worktree, so there is nothing to mark merged. Use
    /// Space to mark it done`, `target worktree is detached; checkout a branch before marking merged`).
    pub async fn mark_session_merged(&self, session_id: &str, target_cwd: &str) -> Result<Ordered<SessionRecord>> {
        let _ = (session_id, target_cwd);
        todo!("D9")
    }

    /// TS: InkDaemon.detectMerges: one detection run at a time (a caller during a run shares it); failures are logged.
    pub fn detect_merges(&self, only_ids: Option<HashSet<String>>) -> SharedTask<()> {
        let _ = only_ids;
        todo!("D9")
    }

    /// TS: InkDaemon.runMergeDetection: live, unmarked records Git still lists whose tip is in the default branch
    /// beyond their base (bounded by MERGE_DETECT_MAX).
    pub async fn run_merge_detection(&self, only_ids: Option<HashSet<String>>) -> Result<()> {
        let _ = only_ids;
        todo!("D9")
    }

    /// TS: InkDaemon.detectMergeFromSummary: a merged PR marks the worktree merged; otherwise ancestry is checked.
    pub async fn detect_merge_from_summary(&self, session_id: &str, summary: &WorkspaceSummary) -> Result<()> {
        let _ = (session_id, summary);
        todo!("D9")
    }

    /// TS: InkDaemon.markDetectedMerge: marks the record merged (unless marked, dismissed or deleted meanwhile).
    pub async fn mark_detected_merge(&self, record_id: &str, kind: MergeDetected, found: DetectedMerge) -> Result<()> {
        let _ = (record_id, kind, found);
        todo!("D9")
    }
}
