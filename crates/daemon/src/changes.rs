//! The daemon's Git tab: changes, staging, commits.
//!
//! The Git tab's Changes view of one workspace: no worker or PTY, just Git run by the daemon (changesGit.ts). A
//! workspace is polled while at least one client watches one of its sessions, pushing only changed records; every
//! Git run for it (status, diff, stage) is serialized through one queue. Also the workspace summary (branch, ahead,
//! PR; cached briefly) and creating a pull request.

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::rc::{Rc, Weak};
use std::time::Duration;

use deckhand_core::changes_model::{empty_changes, find_change};
use deckhand_core::error::{DhError, Result};
use deckhand_protocol::data::{
    ChangeDiff, ChangeStageResult, ChangesRecord, CreatePrResult, SessionRecord, SessionWorktreeMode,
    SessionWorktreeRecord, StageMode, WorkspaceSummary,
};
use deckhand_protocol::events::{ChangesUpdated, ServerMessage};
use deckhand_protocol::wire::{ChangeStage, ChangesDiff, CreatePr, WatchChanges, WorkspaceSummaryRequest};
use deckhand_sys::async_util::{Interval, KeyedQueue, SharedTask, Task, spawn};
use deckhand_sys::changes_git::{
    ChangesSnapshot, StageTarget, UntrackedCounts, apply_stage, read_change_diff, read_changes,
};
use deckhand_sys::workspace_git::{CreatePrOptions, create_pull_request, get_workspace_summary};
use futures::FutureExt;
use indexmap::{IndexMap, IndexSet};
use tokio::time::Instant;

use crate::daemon::Daemon;
use crate::dispatch::as_given;
use crate::server::{ClientHandle, Connection, send_message};
use crate::workspaces::WorkspacePane;

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

/// A watch as the daemon and its in-flight Git runs share it (TS: the object itself). A read that outlives its watch
/// (stopped meanwhile) still updates and publishes through the one it started with, as the TS closure does.
pub type SharedWatch = Rc<RefCell<ChangesWatch>>;

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
    pub watches: HashMap<String, SharedWatch>,
    /// TS ChangesWatch `queue`: per workspace key, Git runs one at a time.
    pub queue: KeyedQueue<String>,
    /// TS `summaries`, by `<sessionId>\0<'pr' or ''>`.
    pub summaries: IndexMap<String, SummaryEntry>,
}

/// TS `emptyChanges({loaded: true, error: errorMessage(error).split('\n')[0]})` as a snapshot: a failed status read.
fn failed_snapshot(error: &DhError) -> ChangesSnapshot {
    let message = error.to_string();
    let first = message.split('\n').next().unwrap_or_default().to_owned();
    ChangesSnapshot {
        record: ChangesRecord {
            loaded: true,
            error: Some(first),
            ..empty_changes()
        },
        all: Vec::new(),
        has_head: false,
    }
}

/// `{...record, sessionId, workspace}`.
fn stamped(record: ChangesRecord, session_id: &str, key: &str) -> ChangesRecord {
    ChangesRecord {
        session_id: Some(session_id.to_owned()),
        workspace: Some(key.to_owned()),
        ..record
    }
}

/// The `summaries` slot of a session: `${session.id}\0${includePr ? 'pr' : ''}`.
fn summary_slot(session_id: &str, include_pr: bool) -> String {
    format!("{session_id}\0{}", if include_pr { "pr" } else { "" })
}

/// `JSON.stringify([session.cwd, session.worktree?.baseRef])` (an absent base is `null`).
fn summary_key(session: &SessionRecord) -> String {
    let base_ref = session.worktree.as_ref().and_then(|worktree| worktree.base_ref.clone());
    serde_json::to_string(&(session.cwd.as_str(), base_ref)).unwrap_or_default()
}

/// create-pr's precondition: a worktree (not `none`) on a branch, not deleted.
fn pr_worktree(session: &SessionRecord) -> Result<&SessionWorktreeRecord> {
    match &session.worktree {
        Some(worktree)
            if worktree.mode != SessionWorktreeMode::None
                && worktree.branch.as_deref().is_some_and(|branch| !branch.is_empty())
                && worktree.deleted_at.as_deref().is_none_or(str::is_empty) =>
        {
            Ok(worktree)
        }
        _ => Err(DhError::msg("Create PR needs a session in a worktree on a branch")),
    }
}

/// Clears `running` when a status read ends, however it ends (TS `finally`).
struct RunningGuard(SharedWatch);

impl Drop for RunningGuard {
    fn drop(&mut self) {
        if let Ok(mut watch) = self.0.try_borrow_mut() {
            watch.running = false;
        }
    }
}

/// `readChanges(key, watch.untracked)`: the watch's untracked line counts are lent to the read (every read of a
/// workspace runs in its exclusive section, so no other read uses them meanwhile).
async fn read_with_cache(key: &str, watch: &SharedWatch) -> Result<ChangesSnapshot> {
    let mut cache = std::mem::take(&mut watch.borrow_mut().untracked);
    let read = read_changes(key, &mut cache).await;
    watch.borrow_mut().untracked = cache;
    read
}

impl Daemon {
    /// TS: InkDaemon.changesWatch: the workspace's watch, created when missing.
    pub fn changes_watch(&self, key: &str) -> SharedWatch {
        let mut changes = self.changes.borrow_mut();
        Rc::clone(changes.watches.entry(key.to_owned()).or_insert_with(|| {
            Rc::new(RefCell::new(ChangesWatch {
                key: key.to_owned(),
                ..ChangesWatch::default()
            }))
        }))
    }

    /// TS: InkDaemon.changesExclusive: runs `run` after every Git run queued for the workspace (failures do not stop
    /// the queue).
    pub fn changes_exclusive<T, F, Fut>(&self, key: &str, run: F) -> Task<T>
    where
        T: 'static,
        F: FnOnce() -> Fut + 'static,
        Fut: Future<Output = T> + 'static,
    {
        let queue = self.changes.borrow().queue.clone();
        queue.run(key.to_owned(), run)
    }

    /// TS: InkDaemon.refreshChanges: a fresh status read (shared with one queued and not started), pushed to watchers
    /// when it changed; a failed read is an empty record carrying the error's first line.
    pub fn refresh_changes(&self, key: &str) -> SharedTask<ChangesSnapshot> {
        let watch = self.changes_watch(key);
        let pending = watch.borrow().queued.clone();
        if let Some(queued) = pending {
            return queued;
        }
        let daemon = self.clone();
        let task_watch = Rc::clone(&watch);
        let task_key = key.to_owned();
        let queued = self
            .changes_exclusive(key, move || async move {
                // TS `if (watch.queued === queued)`: nothing replaces a queued read before it starts (callers share
                // it), so the one queued is this one.
                {
                    let mut current = task_watch.borrow_mut();
                    current.queued = None;
                    current.running = true;
                }
                let _running = RunningGuard(Rc::clone(&task_watch));
                let read = match daemon.assert_workspace_available(&task_key) {
                    Ok(()) => read_with_cache(&task_key, &task_watch).await,
                    Err(error) => Err(error),
                };
                let snapshot = read.unwrap_or_else(|error| failed_snapshot(&error));
                task_watch.borrow_mut().last = Some(ChangesSnapshotAt {
                    snapshot: snapshot.clone(),
                    at: Instant::now(),
                });
                daemon.publish_watch(&task_watch, &snapshot.record);
                snapshot
            })
            .boxed_local()
            .shared();
        watch.borrow_mut().queued = Some(queued.clone());
        queued
    }

    /// TS: InkDaemon.publishChanges: pushes `changes-updated` to the workspace's watchers when the record changed.
    pub fn publish_changes(&self, key: &str, record: &ChangesRecord) {
        let watch = self.changes_watch(key);
        self.publish_watch(&watch, record);
    }

    /// [`Daemon::publish_changes`] through a given watch (TS publishChanges takes the watch object).
    fn publish_watch(&self, watch: &SharedWatch, record: &ChangesRecord) {
        let json = serde_json::to_string(record).unwrap_or_default();
        let key = {
            let mut watch = watch.borrow_mut();
            if watch.published.as_deref() == Some(json.as_str()) {
                return;
            }
            watch.published = Some(json);
            watch.key.clone()
        };
        for (client, session) in self.changes_viewers() {
            if self.workspace_key_of(&session).as_deref() == Some(key.as_str()) {
                let changes = stamped(record.clone(), &session.id, &key);
                send_message(&client, &ServerMessage::ChangesUpdated(ChangesUpdated { changes }));
            }
        }
    }

    /// Every client watching Changes with the session it watches (when that session exists), in client order.
    fn changes_viewers(&self) -> Vec<(ClientHandle, SessionRecord)> {
        let watching: Vec<(ClientHandle, String)> = self
            .server
            .borrow()
            .clients
            .values()
            .filter_map(|client| {
                let watched = client.subscription.watched_changes_session_id.as_ref()?;
                (!watched.is_empty()).then(|| (client.handle.clone(), watched.clone()))
            })
            .collect();
        let sessions = self.sessions.borrow();
        watching
            .into_iter()
            .filter_map(|(client, id)| sessions.sessions.get(&id).map(|session| (client, (**session).clone())))
            .collect()
    }

    /// TS: InkDaemon.watchChanges: the record for a viewer of the session (no `workspace` while it has none).
    pub async fn watch_changes(&self, session_id: Option<&str>) -> Result<ChangesRecord> {
        let Some(session_id) = session_id.filter(|id| !id.is_empty()) else {
            return Ok(empty_changes());
        };
        let session = self.sessions.borrow().sessions.get(session_id).cloned();
        let Some(session) = session else {
            return Ok(ChangesRecord {
                session_id: Some(session_id.to_owned()),
                ..empty_changes()
            });
        };
        // No `workspace` on the record tells the pane there is none yet (or any more).
        let Some(key) = self.workspace_key_of(&session) else {
            return Ok(ChangesRecord {
                loaded: true,
                session_id: Some(session_id.to_owned()),
                ..empty_changes()
            });
        };
        let snapshot = self.refresh_changes(&key).await;
        Ok(stamped(snapshot.record, session_id, &key))
    }

    /// TS: InkDaemon.syncChangeWatches: polls exactly the workspaces some client watches.
    pub fn sync_change_watches(&self) {
        let mut watched: IndexSet<String> = IndexSet::new();
        for (_, session) in self.changes_viewers() {
            if let Some(key) = self.workspace_key_of(&session) {
                watched.insert(key);
            }
        }
        let known: Vec<String> = self.changes.borrow().watches.keys().cloned().collect();
        for key in known {
            if !watched.contains(&key) {
                self.stop_changes_watch(&key);
            }
        }
        if self.lifecycle.borrow().shutting_down {
            return;
        }
        for key in watched {
            let watch = self.changes_watch(&key);
            if watch.borrow().timer.is_some() {
                continue;
            }
            let daemon = self.downgrade();
            let weak: Weak<RefCell<ChangesWatch>> = Rc::downgrade(&watch);
            let poll_key = key.clone();
            let timer = Interval::every(CHANGES_POLL, move || {
                let (Some(daemon), Some(watch)) = (daemon.upgrade(), weak.upgrade()) else {
                    return;
                };
                let idle = {
                    let watch = watch.borrow();
                    watch.queued.is_none() && !watch.running
                };
                if idle {
                    drop(daemon.refresh_changes(&poll_key));
                }
            });
            watch.borrow_mut().timer = Some(timer);
        }
    }

    /// TS: InkDaemon.stopChangesWatch.
    pub fn stop_changes_watch(&self, key: &str) {
        let removed = self.changes.borrow_mut().watches.remove(key);
        let Some(watch) = removed else {
            return;
        };
        let timer = watch.borrow_mut().timer.take();
        if let Some(mut timer) = timer {
            timer.clear();
        }
    }

    /// TS: handleRequest `watch-changes`.
    pub async fn handle_watch_changes(&self, conn: &Connection, request: &WatchChanges) -> Result<ChangesRecord> {
        let session_id = request.session_id.clone();
        self.get_client(&conn.client, |client| client.watched_changes_session_id = session_id);
        self.sync_change_watches();
        self.watch_changes(request.session_id.as_deref()).await
    }

    /// TS: handleRequest `changes-diff` (`<path> is not in the current changes`, or the status read's error).
    pub async fn handle_changes_diff(&self, request: &ChangesDiff) -> Result<ChangeDiff> {
        let (_, key) = self.require_workspace(&request.session_id, WorkspacePane::Git)?;
        let watch = self.changes_watch(&key);
        let fresh = {
            let watch = watch.borrow();
            watch
                .last
                .as_ref()
                .filter(|last| last.at.elapsed() < CHANGES_FRESH)
                .map(|last| last.snapshot.clone())
        };
        let recent = match fresh {
            Some(snapshot) => snapshot,
            None => self.refresh_changes(&key).await,
        };
        if let Some(error) = recent.record.error.as_deref().filter(|error| !error.is_empty()) {
            return Err(DhError::msg(error));
        }
        // TS findChange compares with `===`: a group or path of another type never matches.
        let found = match (request.group.known(), request.path.known()) {
            (Some(group), Some(path)) => find_change(&recent.all, *group, path).cloned(),
            _ => None,
        };
        let Some(entry) = found else {
            return Err(DhError::msg(format!(
                "{} is not in the current changes",
                request.path.js_string()
            )));
        };
        let diff_key = key.clone();
        let diff = self
            .changes_exclusive(&key, move || async move { read_change_diff(&diff_key, &entry).await })
            .await?;
        self.sync_change_watches();
        Ok(diff)
    }

    /// TS: handleRequest `change-stage`: stages or unstages against a status read in the same exclusive section,
    /// answering the result with the refreshed changes.
    pub async fn handle_change_stage(&self, request: &ChangeStage) -> Result<ChangeStageResult> {
        let (session, key) = self.require_workspace(&request.session_id, WorkspacePane::Git)?;
        let watch = self.changes_watch(&key);
        // TS: `message.path !== undefined && message.group ? {group, path} : undefined`. A target is matched by its
        // changeKey (`${group}\0${path}`), so a path of another type is compared as its string and a group that is
        // not one never matches.
        let target = (!request.path.is_absent() && request.group.truthy())
            .then(|| (request.group.known().copied(), as_given(&request.path)));
        // applyStage tests `mode === 'stage'` for a target and `mode === 'unstage'` without one: any other mode
        // unstages the one and stages everything.
        let mode = request.mode.known().copied().unwrap_or(if target.is_some() {
            StageMode::Unstage
        } else {
            StageMode::Stage
        });
        let result = async {
            let daemon = self.clone();
            let stage_key = key.clone();
            let outcome = self
                .changes_exclusive(&key, move || async move {
                    daemon.assert_workspace_available(&stage_key)?;
                    // Validated against a status read inside the same exclusive section, so the paths are current.
                    let current = read_with_cache(&stage_key, &watch).await?;
                    let target = match target {
                        Some((Some(group), path)) => Some(StageTarget { group, path }),
                        Some((None, path)) => {
                            return Err(DhError::msg(format!(
                                "{path} is not among the {} changes; refresh and try again",
                                if mode == StageMode::Stage { "unstaged" } else { "staged" }
                            )));
                        }
                        None => None,
                    };
                    apply_stage(&stage_key, &current, mode, target.as_ref()).await
                })
                .await?;
            let snapshot = self.refresh_changes(&key).await;
            Ok(ChangeStageResult {
                changed: outcome.changed,
                skipped_conflicts: outcome.skipped_conflicts,
                changes: stamped(snapshot.record, &session.id, &key),
            })
        }
        .await;
        self.sync_change_watches();
        result
    }

    /// TS: handleRequest `workspace-summary`: cached per session for SUMMARY_TTL; a fresh one also runs merge
    /// detection on it in the background.
    pub async fn handle_workspace_summary(&self, request: &WorkspaceSummaryRequest) -> Result<WorkspaceSummary> {
        let session = self.require_session(&request.session_id)?;
        let include_pr = request.include_pr == Some(true);
        let now = Instant::now();
        self.prune_summaries(now);
        let slot = summary_slot(&session.id, include_pr);
        let key = summary_key(&session);
        let cached = self.changes.borrow().summaries.get(&slot).cloned();
        let cached = match cached {
            Some(entry) if entry.key == key && now.saturating_duration_since(entry.at) <= SUMMARY_TTL => entry,
            _ => {
                let cwd = session.cwd.clone();
                let base_ref = session.worktree.as_ref().and_then(|worktree| worktree.base_ref.clone());
                let result =
                    spawn(async move { get_workspace_summary(&cwd, base_ref.as_deref(), include_pr, None).await })
                        .boxed_local()
                        .shared();
                let entry = SummaryEntry { key, at: now, result };
                self.changes.borrow_mut().summaries.insert(slot.clone(), entry.clone());
                self.follow_summary(slot, session.id.clone(), entry.result.clone());
                entry
            }
        };
        cached.result.await
    }

    /// Drops summaries older than 15 TTLs, and those of sessions that are gone.
    fn prune_summaries(&self, now: Instant) {
        let entries: Vec<(String, Instant)> = self
            .changes
            .borrow()
            .summaries
            .iter()
            .map(|(id, entry)| (id.clone(), entry.at))
            .collect();
        for (id, at) in entries {
            let owner = id.split('\0').next().unwrap_or_default();
            let gone = !self.sessions.borrow().sessions.contains_key(owner);
            if now.saturating_duration_since(at) > SUMMARY_TTL * 15 || gone {
                self.changes.borrow_mut().summaries.shift_remove(&id);
            }
        }
    }

    /// A failed summary is forgotten (unless replaced meanwhile); a fresh one runs merge detection.
    fn follow_summary(&self, slot: String, session_id: String, result: SharedTask<Result<WorkspaceSummary>>) {
        let daemon = self.clone();
        drop(spawn(async move {
            match result.clone().await {
                Err(_) => {
                    let mut changes = daemon.changes.borrow_mut();
                    if changes
                        .summaries
                        .get(&slot)
                        .is_some_and(|held| held.result.ptr_eq(&result))
                    {
                        changes.summaries.shift_remove(&slot);
                    }
                }
                // A fresh look at the workspace (and its PR) also tells whether it was merged elsewhere.
                Ok(summary) => {
                    if let Err(error) = daemon.detect_merge_from_summary(&session_id, &summary).await {
                        daemon.log(&format!("merge detection failed: {error}")).await;
                    }
                }
            }
        }));
    }

    /// TS: handleRequest `create-pr` (`Create PR needs a session in a worktree on a branch`): pushes and opens the PR
    /// form via gh; the session's cached summaries are dropped either way.
    pub async fn handle_create_pr(&self, request: &CreatePr) -> Result<CreatePrResult> {
        // Outward-facing: the UI confirms first. Pushes (never forced) and opens GitHub's PR form via gh.
        let session = self.require_session(&request.session_id)?;
        let worktree = pr_worktree(&session)?;
        let options = CreatePrOptions {
            base_ref: worktree.base_ref.clone(),
            expected_branch: request.branch.clone(),
        };
        let result = create_pull_request(&session.cwd, &options).await;
        let prefix = format!("{}\0", session.id);
        self.changes
            .borrow_mut()
            .summaries
            .retain(|slot, _| !slot.starts_with(&prefix));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::{local, session, test_client, test_daemon};
    use deckhand_core::changes_model::LineCounts;
    use deckhand_sys::changes_git::UntrackedCount;
    use serde_json::json;

    fn record_json(record: &ChangesRecord) -> String {
        serde_json::to_string(record).unwrap()
    }

    #[test]
    fn timings_match_the_ts() {
        assert_eq!(CHANGES_POLL, Duration::from_millis(2000));
        assert_eq!(CHANGES_FRESH, Duration::from_millis(2500));
        assert_eq!(SUMMARY_TTL, Duration::from_millis(4000));
    }

    #[test]
    fn a_failed_read_is_an_empty_loaded_record_with_the_first_error_line() {
        let snapshot = failed_snapshot(&DhError::msg("fatal: not a git repository\nhint: more"));
        assert_eq!(
            record_json(&snapshot.record),
            r#"{"loaded":true,"entries":[],"counts":{"conflicts":0,"staged":0,"unstaged":0,"untracked":0},"omitted":{"conflicts":0,"staged":0,"unstaged":0,"untracked":0},"error":"fatal: not a git repository"}"#
        );
        assert!(snapshot.all.is_empty() && !snapshot.has_head);
        let stamped = stamped(snapshot.record, "s1", "/repo");
        assert!(
            record_json(&stamped)
                .ends_with(r#""error":"fatal: not a git repository","sessionId":"s1","workspace":"/repo"}"#)
        );
    }

    #[test]
    fn summary_slots_and_keys_match_the_ts_strings() {
        assert_eq!(summary_slot("s1", true), "s1\0pr");
        assert_eq!(summary_slot("s1", false), "s1\0");
        let mut plain = session("s1", "/repo", "/repo/wt");
        assert_eq!(summary_key(&plain), r#"["/repo/wt",null]"#);
        let mut worktree = SessionWorktreeRecord::new(SessionWorktreeMode::Managed);
        worktree.base_ref = Some("origin/main".into());
        plain.worktree = Some(worktree);
        assert_eq!(summary_key(&plain), r#"["/repo/wt","origin/main"]"#);
    }

    #[test]
    fn create_pr_needs_a_live_worktree_on_a_branch() {
        let message = "Create PR needs a session in a worktree on a branch";
        let mut record = session("s1", "/repo", "/repo/wt");
        assert_eq!(pr_worktree(&record).unwrap_err().to_string(), message);
        let mut worktree = SessionWorktreeRecord::new(SessionWorktreeMode::Managed);
        record.worktree = Some(worktree.clone());
        assert_eq!(pr_worktree(&record).unwrap_err().to_string(), message);
        worktree.branch = Some(String::new());
        record.worktree = Some(worktree.clone());
        assert_eq!(pr_worktree(&record).unwrap_err().to_string(), message);
        worktree.branch = Some("feature".into());
        record.worktree = Some(worktree.clone());
        assert!(pr_worktree(&record).is_ok());
        worktree.deleted_at = Some("2026-01-01T00:00:00.000Z".into());
        record.worktree = Some(worktree.clone());
        assert_eq!(pr_worktree(&record).unwrap_err().to_string(), message);
        worktree.deleted_at = None;
        worktree.mode = SessionWorktreeMode::None;
        record.worktree = Some(worktree);
        assert_eq!(pr_worktree(&record).unwrap_err().to_string(), message);
    }

    #[test]
    fn git_runs_of_a_workspace_are_serialized_and_survive_failures() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = test_daemon(dir.path());
        local(async {
            let log = Rc::new(RefCell::new(Vec::new()));
            let first_log = Rc::clone(&log);
            let first = daemon.changes_exclusive("/w", move || async move {
                first_log.borrow_mut().push("first start");
                tokio::time::sleep(Duration::from_millis(20)).await;
                first_log.borrow_mut().push("first end");
                Err::<(), _>(DhError::msg("boom"))
            });
            let second_log = Rc::clone(&log);
            let second = daemon.changes_exclusive("/w", move || async move {
                second_log.borrow_mut().push("second");
                7
            });
            let other_log = Rc::clone(&log);
            let other = daemon.changes_exclusive("/other", move || async move {
                other_log.borrow_mut().push("other");
            });
            assert_eq!(first.await.unwrap_err().to_string(), "boom");
            assert_eq!(second.await, 7);
            other.await;
            assert_eq!(*log.borrow(), vec!["first start", "other", "first end", "second"]);
        });
    }

    #[test]
    fn a_queued_status_read_is_shared_until_it_starts() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = test_daemon(dir.path());
        local(async {
            let first = daemon.refresh_changes("/w");
            let second = daemon.refresh_changes("/w");
            assert!(first.ptr_eq(&second));
            let watch = daemon.changes_watch("/w");
            assert!(
                watch
                    .borrow()
                    .queued
                    .as_ref()
                    .is_some_and(|queued| queued.ptr_eq(&first))
            );
            assert_eq!(watch.borrow().key, "/w");
            assert!(!watch.borrow().running);
            // Not run: the LocalSet ends before the read starts.
        });
    }

    #[test]
    fn a_read_lends_the_untracked_counts_and_gets_them_back() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        let missing = missing.to_str().unwrap();
        let watch: SharedWatch = Rc::new(RefCell::new(ChangesWatch::default()));
        let counts = LineCounts {
            additions: None,
            deletions: None,
            binary: Some(true),
        };
        watch.borrow_mut().untracked.insert(
            "a.bin".into(),
            UntrackedCount {
                size: 1,
                mtime_ms: 2.0,
                counts,
            },
        );
        local(async {
            let error = read_with_cache(missing, &watch).await.unwrap_err();
            assert_eq!(error.to_string(), "Worktree directory is missing");
        });
        assert!(watch.borrow().untracked.contains_key("a.bin"));
    }

    #[test]
    fn a_viewer_without_a_session_gets_the_empty_record() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = test_daemon(dir.path());
        local(async {
            let empty = r#"{"loaded":false,"entries":[],"counts":{"conflicts":0,"staged":0,"unstaged":0,"untracked":0},"omitted":{"conflicts":0,"staged":0,"unstaged":0,"untracked":0}}"#;
            assert_eq!(record_json(&daemon.watch_changes(None).await.unwrap()), empty);
            assert_eq!(record_json(&daemon.watch_changes(Some("")).await.unwrap()), empty);
            let unknown = daemon.watch_changes(Some("gone")).await.unwrap();
            let counts = json!({"conflicts": 0, "staged": 0, "unstaged": 0, "untracked": 0});
            assert_eq!(
                serde_json::to_value(&unknown).unwrap(),
                json!({"loaded": false, "entries": [], "counts": counts, "omitted": counts, "sessionId": "gone"})
            );
        });
    }

    #[test]
    fn watch_changes_records_what_the_client_watches_and_stops_unwatched_polls() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = test_daemon(dir.path());
        let (client, mut rx) = test_client(1);
        let conn = Connection::new(client);
        local(async {
            // A watch nobody looks at any more (its poll running) is stopped by the next sync.
            let stale = daemon.changes_watch("/old");
            stale.borrow_mut().timer = Some(Interval::every(CHANGES_POLL, || {}));
            let request = WatchChanges {
                request_id: "r1".into(),
                session_id: Some("gone".into()),
            };
            let record = daemon.handle_watch_changes(&conn, &request).await.unwrap();
            assert_eq!(record.session_id.as_deref(), Some("gone"));
            assert!(record.workspace.is_none());
            let watched = daemon.get_client(&conn.client, |client| client.watched_changes_session_id.clone());
            assert_eq!(watched.as_deref(), Some("gone"));
            assert!(daemon.changes.borrow().watches.is_empty());
            assert!(stale.borrow().timer.is_none());
            let request = WatchChanges {
                request_id: "r2".into(),
                session_id: None,
            };
            daemon.handle_watch_changes(&conn, &request).await.unwrap();
            assert!(daemon.get_client(&conn.client, |client| client.watched_changes_session_id.is_none()));
        });
        // The answer is the dispatcher's to send: nothing was pushed.
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn records_are_published_only_when_they_change() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = test_daemon(dir.path());
        let (client, mut rx) = test_client(1);
        // Watching a session that is gone: nobody to push to.
        daemon.get_client(&client, |subscription| {
            subscription.watched_changes_session_id = Some("gone".into());
        });
        let first = failed_snapshot(&DhError::msg("first"));
        daemon.publish_changes("/w", &first.record);
        let published = daemon.changes_watch("/w").borrow().published.clone();
        assert_eq!(published, Some(record_json(&first.record)));
        let next = failed_snapshot(&DhError::msg("second"));
        daemon.publish_changes("/w", &next.record);
        let published = daemon.changes_watch("/w").borrow().published.clone();
        assert_eq!(published, Some(record_json(&next.record)));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn stopping_a_watch_forgets_it_and_clears_its_poll() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = test_daemon(dir.path());
        local(async {
            daemon.stop_changes_watch("/none");
            let watch = daemon.changes_watch("/w");
            watch.borrow_mut().timer = Some(Interval::every(CHANGES_POLL, || {}));
            daemon.stop_changes_watch("/w");
            assert!(watch.borrow().timer.is_none());
            assert!(!daemon.changes.borrow().watches.contains_key("/w"));
            // A new watch for the workspace starts afresh.
            assert!(!Rc::ptr_eq(&daemon.changes_watch("/w"), &watch));
        });
    }
}
