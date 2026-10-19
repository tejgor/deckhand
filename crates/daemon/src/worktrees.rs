//! Port of daemon.ts (worktree create/delete/links).
//!
//! Worktree records: one per incarnation of a linked worktree (persisted with the sessions), the only copy of its
//! merge/deleted markers, shared by every session in it. Deleting a worktree, from a kill (`x` with delete) or from W
//! (the worktree manager): structural blockers never give way; data-loss reasons only to an explicit override.

use std::collections::HashSet;
use std::future::Future;
use std::time::Duration;

use deckhand_core::error::Result;
use deckhand_core::worktree_records::WorktreeRecords;
use deckhand_protocol::Ordered;
use deckhand_protocol::data::{
    SessionCleanupInspection, SessionRecord, WorktreeCleanupInspection, WorktreeDeleteResult, WorktreeOverview,
};
use deckhand_protocol::records::WorktreeRecord;
use deckhand_protocol::wire::{CwdRequest, DeleteWorktree};
use deckhand_sys::git::WorktreeInfo;

use crate::daemon::Daemon;
use crate::server::ClientHandle;

/// TS: daemon.ts WORKTREE_OVERVIEW_MAX: at most this many linked worktrees get Git details per overview.
pub const WORKTREE_OVERVIEW_MAX: usize = 60;

/// TS: daemon.ts WORKTREE_OVERVIEW_CONCURRENCY: this many at a time.
pub const WORKTREE_OVERVIEW_CONCURRENCY: usize = 4;

/// TS: daemon.ts STOP_FOR_DELETE_MS: how long deleting a worktree waits for each session it stops to exit.
pub const STOP_FOR_DELETE: Duration = Duration::from_millis(10_000);

/// TS: daemon.ts WorktreeTarget: a worktree W acts on.
#[derive(Clone, Debug, PartialEq)]
pub struct WorktreeTarget {
    /// Its repository's checkout root (the one W runs in).
    pub root: String,
    /// The resolved worktree path.
    pub key: String,
    /// Git's entry (none: only Deckhand's record is left).
    pub item: Option<WorktreeInfo>,
    /// Its live record.
    pub record: Option<Ordered<WorktreeRecord>>,
    pub sessions: Vec<Ordered<SessionRecord>>,
}

/// TS: InkDaemon.joinWorktree result: the incarnation joined, and the live record it superseded (marked deleted).
#[derive(Clone, Debug, PartialEq)]
pub struct JoinedWorktree {
    pub id: String,
    pub superseded: Option<Ordered<WorktreeRecord>>,
}

/// TS: InkDaemon.inspectWorktree result.
#[derive(Clone, Debug, PartialEq)]
pub struct InspectedWorktree {
    pub inspection: WorktreeCleanupInspection,
    pub target: WorktreeTarget,
}

/// The worktrees' part of the daemon state.
#[derive(Default)]
pub struct WorktreesState {
    /// TS `worktrees`: linked worktree incarnations by ID (persisted with the sessions).
    pub records: WorktreeRecords,
    /// TS `cleanupWorktrees`: resolved paths of worktrees being deleted.
    pub cleanup: HashSet<String>,
}

/// TS: daemon.ts realpathOrResolve.
pub async fn realpath_or_resolve(target: &str) -> String {
    let _ = target;
    todo!("D10")
}

/// TS: daemon.ts isLinkedWorktreeRoot: the checkout root is a linked worktree (not the main checkout).
pub async fn is_linked_worktree_root(root: &str) -> bool {
    let _ = root;
    todo!("D10")
}

/// TS: daemon.ts forEachLimited: runs `work` over `items`, at most `limit` at a time.
pub async fn for_each_limited<T, F, Fut>(items: Vec<T>, limit: usize, work: F)
where
    F: Fn(T) -> Fut,
    Fut: Future<Output = ()>,
{
    let _ = (items, limit, work);
    todo!("D10")
}

impl Daemon {
    /// TS: InkDaemon.worktreeRecordOf: the record of the linked worktree incarnation the session runs in.
    pub fn worktree_record_of(&self, session: &SessionRecord) -> Option<Ordered<WorktreeRecord>> {
        let _ = session;
        todo!("D10")
    }

    /// TS: InkDaemon.saveWorktreeRecord: saves it and re-projects, persists and broadcasts every session of that
    /// incarnation, then syncs its tasks.
    pub async fn save_worktree_record(&self, record: Ordered<WorktreeRecord>) -> Result<()> {
        let _ = record;
        todo!("D10")
    }

    /// TS: InkDaemon.joinWorktree: the live incarnation at the path, or (created, or none) a new one; synchronous so
    /// the record cannot be dropped before the caller sets the session.
    pub fn join_worktree(&self, worktree_path: &str, created: bool, base_sha: Option<String>) -> JoinedWorktree {
        let _ = (worktree_path, created, base_sha);
        todo!("D10")
    }

    /// TS: InkDaemon.cleanupBlockers: reasons no override may bypass, for deleting the session's worktree.
    pub async fn cleanup_blockers(&self, session: &SessionRecord, delete_branch: bool) -> Vec<String> {
        let _ = (session, delete_branch);
        todo!("D10")
    }

    /// TS: InkDaemon.inspectSessionCleanup: what deleting the session's worktree would lose, plus its blockers.
    pub async fn inspect_session_cleanup(
        &self,
        session_id: &str,
        delete_branch: bool,
    ) -> Result<SessionCleanupInspection> {
        let _ = (session_id, delete_branch);
        todo!("D10")
    }

    /// TS: InkDaemon.assertCleanupAllowed (the blockers joined by `; `, or `Deletion blocked: <reasons>. An explicit
    /// data-loss override is required.`).
    pub async fn assert_cleanup_allowed(
        &self,
        session_id: &str,
        delete_branch: bool,
        allow_data_loss: bool,
    ) -> Result<()> {
        let _ = (session_id, delete_branch, allow_data_loss);
        todo!("D10")
    }

    /// TS: InkDaemon.updateWorktreeRecord: patches a record's own fields; `broadcast`: through saveWorktreeRecord.
    pub async fn update_worktree_record(
        &self,
        id: &str,
        patch: impl FnOnce(&mut Ordered<WorktreeRecord>),
        broadcast: bool,
    ) -> Result<()> {
        let _ = (id, patch, broadcast);
        todo!("D10")
    }

    /// TS: InkDaemon.repositorySessions: every session of the repository `root` belongs to.
    pub async fn repository_sessions(&self, root: &str) -> Vec<Ordered<SessionRecord>> {
        let _ = root;
        todo!("D10")
    }

    /// TS: InkDaemon.sessionsInWorktree: its live record's sessions and those whose workspace it is; `inside`: also
    /// any not exited whose directory is in it.
    pub async fn sessions_in_worktree(
        &self,
        worktree_path: &str,
        candidates: &[Ordered<SessionRecord>],
        inside: bool,
    ) -> Vec<Ordered<SessionRecord>> {
        let _ = (worktree_path, candidates, inside);
        todo!("D10")
    }

    /// TS: InkDaemon.worktreeOverview: every worktree of the repository with its sessions, markers and (bounded)
    /// what deleting it would lose.
    pub async fn worktree_overview(&self, cwd: &str, client: &ClientHandle) -> Result<WorktreeOverview> {
        let _ = (cwd, client);
        todo!("D10")
    }

    /// TS: InkDaemon.worktreeTarget (`Invalid worktree path`, `This worktree is not listed by Git any more; refresh
    /// (R)`).
    pub async fn worktree_target(&self, cwd: &str, worktree_path: &str) -> Result<WorktreeTarget> {
        let _ = (cwd, worktree_path);
        todo!("D10")
    }

    /// TS: InkDaemon.worktreeBlockers: reasons no DELETE may bypass, for a worktree deleted from W.
    pub async fn worktree_blockers(
        &self,
        target: &WorktreeTarget,
        delete_branch: bool,
        client: &ClientHandle,
        expected_branch: Option<&str>,
        deleting: bool,
    ) -> Vec<String> {
        let _ = (target, delete_branch, client, expected_branch, deleting);
        todo!("D10")
    }

    /// TS: InkDaemon.inspectWorktree.
    pub async fn inspect_worktree(
        &self,
        cwd: &str,
        worktree_path: &str,
        delete_branch: bool,
        client: &ClientHandle,
        expected_branch: Option<&str>,
        deleting: bool,
    ) -> Result<InspectedWorktree> {
        let _ = (cwd, worktree_path, delete_branch, client, expected_branch, deleting);
        todo!("D10")
    }

    /// TS: InkDaemon.deleteWorktree: checks, stops its running sessions when asked (and checks again), stops its
    /// workspace, removes it (and its branch), marks its record deleted and archives its sessions.
    pub async fn delete_worktree(
        &self,
        message: &DeleteWorktree,
        client: &ClientHandle,
    ) -> Result<WorktreeDeleteResult> {
        let _ = (message, client);
        todo!("D10")
    }

    /// TS: InkDaemon.stopForDeletion (`Session "<title>" did not stop; nothing was deleted`).
    pub async fn stop_for_deletion(&self, session_id: &str) -> Result<()> {
        let _ = session_id;
        todo!("D10")
    }

    /// TS: handleRequest `list-worktrees` (git.ts listWorktrees).
    pub async fn handle_list_worktrees(&self, request: &CwdRequest) -> Result<Vec<WorktreeInfo>> {
        let _ = request;
        todo!("D10")
    }
}
