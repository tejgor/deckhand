//! Port of daemon.ts (Git tab: changes, staging, commits).
//!
//! The Git tab's Changes view of one workspace: no worker or PTY, just Git run by the daemon (changesGit.ts). A
//! workspace is polled while at least one client watches one of its sessions, pushing only changed records; every
//! Git run for it (status, diff, stage) is serialized through one queue. Also the workspace summary (branch, ahead,
//! PR; cached briefly) and creating a pull request.

use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use deckhand_core::error::Result;
use deckhand_protocol::data::{ChangeDiff, ChangeStageResult, ChangesRecord, CreatePrResult, WorkspaceSummary};
use deckhand_protocol::wire::{ChangeStage, ChangesDiff, CreatePr, WatchChanges, WorkspaceSummaryRequest};
use deckhand_sys::async_util::{Interval, KeyedQueue, SharedTask, Task};
use deckhand_sys::changes_git::{ChangesSnapshot, UntrackedCounts};
use indexmap::IndexMap;
use tokio::time::Instant;

use crate::daemon::Daemon;
use crate::server::Connection;

/// TS: daemon.ts CHANGES_POLL_MS.
pub const CHANGES_POLL: Duration = Duration::from_millis(2000);

/// TS: daemon.ts CHANGES_FRESH_MS: a diff reuses the last status read when it is this recent.
pub const CHANGES_FRESH: Duration = Duration::from_millis(2500);

/// TS: daemon.ts SUMMARY_TTL_MS: a workspace summary is reused this long (entries are dropped after 15 times it).
pub const SUMMARY_TTL: Duration = Duration::from_millis(4000);

/// TS: ChangesWatch `last`.
#[derive(Clone, Debug)]
pub struct ChangesSnapshotAt {
    pub snapshot: ChangesSnapshot,
    pub at: Instant,
}

/// TS: daemon.ts ChangesWatch (its `queue` is [`ChangesState::queue`] under the same key).
#[derive(Default)]
pub struct ChangesWatch {
    pub key: String,
    /// The poll, while some client watches.
    pub timer: Option<Interval>,
    /// A status read queued and not started yet: callers share it.
    pub queued: Option<SharedTask<ChangesSnapshot>>,
    pub running: bool,
    pub last: Option<ChangesSnapshotAt>,
    /// The last record pushed to watchers (JSON), so polls push only changes.
    pub published: Option<String>,
    pub untracked: UntrackedCounts,
}

/// TS: `summaries` entry: a summary in flight or done, for one session (and whether it includes the PR).
#[derive(Clone)]
pub struct SummaryEntry {
    /// `JSON.stringify([cwd, baseRef])`: a different workspace or base reads again.
    pub key: String,
    pub at: Instant,
    pub result: SharedTask<Result<WorkspaceSummary>>,
}

/// The Git tab's part of the daemon state.
#[derive(Default)]
pub struct ChangesState {
    /// TS `changeWatches` by workspace key.
    pub watches: HashMap<String, ChangesWatch>,
    /// TS ChangesWatch `queue`: per workspace key, Git runs one at a time.
    pub queue: KeyedQueue<String>,
    /// TS `summaries`, by `<sessionId>\0<'pr' or ''>`.
    pub summaries: IndexMap<String, SummaryEntry>,
}

impl Daemon {
    /// TS: InkDaemon.changesWatch: the workspace's watch, created when missing.
    pub fn changes_watch(&self, key: &str) {
        let _ = key;
        todo!("D7")
    }

    /// TS: InkDaemon.changesExclusive: runs `run` after every Git run queued for the workspace (failures do not stop
    /// the queue).
    pub fn changes_exclusive<T, F, Fut>(&self, key: &str, run: F) -> Task<T>
    where
        T: 'static,
        F: FnOnce() -> Fut + 'static,
        Fut: Future<Output = T> + 'static,
    {
        let _ = (key, run);
        todo!("D7")
    }

    /// TS: InkDaemon.refreshChanges: a fresh status read (shared with one queued and not started), pushed to watchers
    /// when it changed; a failed read is an empty record carrying the error's first line.
    pub fn refresh_changes(&self, key: &str) -> SharedTask<ChangesSnapshot> {
        let _ = key;
        todo!("D7")
    }

    /// TS: InkDaemon.publishChanges: pushes `changes-updated` to the workspace's watchers when the record changed.
    pub fn publish_changes(&self, key: &str, record: &ChangesRecord) {
        let _ = (key, record);
        todo!("D7")
    }

    /// TS: InkDaemon.watchChanges: the record for a viewer of the session (no `workspace` while it has none).
    pub async fn watch_changes(&self, session_id: Option<&str>) -> Result<ChangesRecord> {
        let _ = session_id;
        todo!("D7")
    }

    /// TS: InkDaemon.syncChangeWatches: polls exactly the workspaces some client watches.
    pub fn sync_change_watches(&self) {
        todo!("D7")
    }

    /// TS: InkDaemon.stopChangesWatch.
    pub fn stop_changes_watch(&self, key: &str) {
        let _ = key;
        todo!("D7")
    }

    /// TS: handleRequest `watch-changes`.
    pub async fn handle_watch_changes(&self, conn: &Connection, request: &WatchChanges) -> Result<ChangesRecord> {
        let _ = (conn, request);
        todo!("D7")
    }

    /// TS: handleRequest `changes-diff` (`<path> is not in the current changes`, or the status read's error).
    pub async fn handle_changes_diff(&self, request: &ChangesDiff) -> Result<ChangeDiff> {
        let _ = request;
        todo!("D7")
    }

    /// TS: handleRequest `change-stage`: stages or unstages against a status read in the same exclusive section,
    /// answering the result with the refreshed changes.
    pub async fn handle_change_stage(&self, request: &ChangeStage) -> Result<ChangeStageResult> {
        let _ = request;
        todo!("D7")
    }

    /// TS: handleRequest `workspace-summary`: cached per session for SUMMARY_TTL; a fresh one also runs merge
    /// detection on it in the background.
    pub async fn handle_workspace_summary(&self, request: &WorkspaceSummaryRequest) -> Result<WorkspaceSummary> {
        let _ = request;
        todo!("D7")
    }

    /// TS: handleRequest `create-pr` (`Create PR needs a session in a worktree on a branch`): pushes and opens the PR
    /// form via gh; the session's cached summaries are dropped either way.
    pub async fn handle_create_pr(&self, request: &CreatePr) -> Result<CreatePrResult> {
        let _ = request;
        todo!("D7")
    }
}
