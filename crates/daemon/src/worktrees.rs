//! Port of daemon.ts (worktree create/delete/links).
//!
//! Worktree records: one per incarnation of a linked worktree (persisted with the sessions), the only copy of its
//! merge/deleted markers, shared by every session in it. Deleting a worktree, from a kill (`x` with delete) or from W
//! (the worktree manager): structural blockers never give way; data-loss reasons only to an explicit override.

use std::cell::RefCell;
use std::collections::HashSet;
use std::future::Future;
use std::time::{Duration, Instant};

use deckhand_core::error::{DhError, Result};
use deckhand_core::jsstr::{date, path};
use deckhand_core::session_scope::{is_path_inside, session_matches_scope};
use deckhand_core::worktree_records::{WorktreeRecords, live_worktree_record};
use deckhand_protocol::Ordered;
use deckhand_protocol::data::{
    CleanupInspection, SessionCleanupInspection, SessionRecord, SessionStatus, SessionTitle, SessionWorktreeMode,
    WorktreeCleanupInspection, WorktreeDeleteResult, WorktreeInUse, WorktreeMissing, WorktreeOverview,
    WorktreeOverviewEntry, WorktreeRemoved,
};
use deckhand_protocol::records::WorktreeRecord;
use deckhand_protocol::wire::{CwdRequest, DeleteWorktree};
use deckhand_sys::git::{
    BranchWhere, WorktreeInfo, delete_local_branch, find_repo_root, list_worktrees, prune_worktrees, remove_worktree,
    resolve_default_branch,
};
use deckhand_sys::workspace_git::{CleanupOptions, WorktreeActivity, inspect_workspace_cleanup, worktree_activity};
use indexmap::IndexMap;

use crate::daemon::Daemon;
use crate::merge::default_branch_refs;
use crate::server::ClientHandle;
use crate::sessions::KillOptions;

/// TS: daemon.ts WORKTREE_OVERVIEW_MAX: at most this many linked worktrees get Git details per overview.
pub const WORKTREE_OVERVIEW_MAX: usize = 60;

/// TS: daemon.ts WORKTREE_OVERVIEW_CONCURRENCY: this many at a time.
pub const WORKTREE_OVERVIEW_CONCURRENCY: usize = 4;

/// TS: daemon.ts STOP_FOR_DELETE_MS: how long deleting a worktree waits for each session it stops to exit.
pub const STOP_FOR_DELETE: Duration = Duration::from_millis(10_000);

/// How often stopForDeletion looks whether the session has exited.
const STOP_POLL: Duration = Duration::from_millis(100);

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

/// A JS-truthy optional string (set and not empty).
fn truthy(value: Option<&str>) -> bool {
    value.is_some_and(|text| !text.is_empty())
}

/// `new Date().toISOString()`.
fn now_iso() -> String {
    date::to_iso_string(date::now_ms()).unwrap_or_default()
}

/// The session's `worktree.id` (TS `session.worktree?.id`).
fn worktree_id(session: &SessionRecord) -> Option<&str> {
    session.worktree.as_ref().and_then(|worktree| worktree.id.as_deref())
}

/// The session's `worktree.branch`, when truthy.
fn worktree_branch(session: &SessionRecord) -> Option<&str> {
    session
        .worktree
        .as_ref()
        .and_then(|worktree| worktree.branch.as_deref())
        .filter(|branch| !branch.is_empty())
}

/// `{safe, reasons: [], dirtyFiles: 0, untrackedFiles: 0, ignoredFiles: 0}`.
fn empty_inspection(safe: bool, reasons: Vec<String>) -> CleanupInspection {
    CleanupInspection {
        safe,
        reasons,
        dirty_files: 0,
        untracked_files: 0,
        ignored_files: 0,
        files: None,
        unpublished_commits: None,
        unmerged_commits: None,
    }
}

/// `{...record, deletedAt: at}`.
fn marked_deleted(record: &Ordered<WorktreeRecord>, at: String) -> Ordered<WorktreeRecord> {
    let mut record = record.clone();
    record.touch("deletedAt");
    record.deleted_at = Some(at);
    record
}

/// TS: daemon.ts realpathOrResolve.
pub async fn realpath_or_resolve(target: &str) -> String {
    match tokio::fs::canonicalize(target).await {
        Ok(real) => real.to_string_lossy().into_owned(),
        Err(_) => path::resolve(target),
    }
}

/// TS: daemon.ts isLinkedWorktreeRoot: the checkout root is a linked worktree (not the main checkout).
///
/// Git reports both paths the same way.
pub async fn is_linked_worktree_root(root: &str) -> bool {
    let worktrees = list_worktrees(root).await.unwrap_or_default();
    let key = path::resolve(root);
    worktrees
        .iter()
        .any(|item| !item.is_main && path::resolve(&item.path) == key)
}

/// TS: daemon.ts forEachLimited: runs `work` over `items`, at most `limit` at a time.
pub async fn for_each_limited<T, F, Fut>(items: Vec<T>, limit: usize, work: F)
where
    F: Fn(T) -> Fut,
    Fut: Future<Output = ()>,
{
    let workers = limit.min(items.len());
    let queue = RefCell::new(items.into_iter());
    let queue = &queue;
    let work = &work;
    futures::future::join_all((0..workers).map(|_| async move {
        loop {
            let next = queue.borrow_mut().next();
            let Some(item) = next else { break };
            work(item).await;
        }
    }))
    .await;
}

impl Daemon {
    /// TS `liveWorktreeRecord(this.worktrees.values(), worktreePath)`: the live record at the path, as held.
    fn live_record(&self, worktree_path: &str) -> Option<Ordered<WorktreeRecord>> {
        let worktrees = self.worktrees.borrow();
        let found = live_worktree_record(worktrees.records.values().map(|record| &**record), worktree_path)?;
        worktrees
            .records
            .values()
            .find(|record| std::ptr::eq(&***record, found))
            .cloned()
    }

    /// The checkouts other clients (not `client`) run in (TS `client.repoRoot` of every other client, when truthy).
    fn other_client_roots(&self, client: &ClientHandle) -> Vec<String> {
        self.server
            .borrow()
            .clients
            .values()
            .filter(|other| other.handle.id != client.id)
            .filter_map(|other| other.subscription.repo_root.clone())
            .filter(|root| !root.is_empty())
            .collect()
    }

    /// TS: InkDaemon.worktreeRecordOf: the record of the linked worktree incarnation the session runs in.
    ///
    /// Main-checkout sessions have none.
    pub fn worktree_record_of(&self, session: &SessionRecord) -> Option<Ordered<WorktreeRecord>> {
        let id = worktree_id(session).filter(|id| !id.is_empty())?;
        self.worktrees.borrow().records.get(id).cloned()
    }

    /// TS: InkDaemon.saveWorktreeRecord: saves it and re-projects, persists and broadcasts every session of that
    /// incarnation, then syncs its tasks.
    ///
    /// They all change together (merged, unmarked, deleted), and every client hears about each of them.
    pub async fn save_worktree_record(&self, record: Ordered<WorktreeRecord>) -> Result<()> {
        let before = self
            .worktrees
            .borrow_mut()
            .records
            .insert(record.id.clone(), record.clone());
        let now = now_iso();
        let affected: Vec<Ordered<SessionRecord>> = self
            .all_sessions()
            .into_iter()
            .filter(|session| worktree_id(session) == Some(record.id.as_str()))
            .collect();
        for session in &affected {
            let mut updated = session.clone();
            updated.touch("updatedAt");
            updated.updated_at.clone_from(&now);
            self.set_session_in_workspace(updated);
        }
        self.persist().await?;
        for session in &affected {
            self.broadcast_session_updated(session);
        }
        self.sync_change_watches();
        let branch = affected
            .iter()
            .find_map(|session| worktree_branch(session).map(str::to_owned));
        self.sync_worktree_tasks(before.as_deref(), &record, branch.as_deref())
            .await;
        Ok(())
    }

    /// TS: InkDaemon.joinWorktree: the live incarnation at the path, or (created, or none) a new one; synchronous so
    /// the record cannot be dropped before the caller sets the session.
    ///
    /// A live incarnation still recorded at a path Deckhand just created a worktree at was removed outside Deckhand: it
    /// is marked deleted at once (the caller saves and broadcasts it), so there is never a second live record there.
    pub fn join_worktree(&self, worktree_path: &str, created: bool, base_sha: Option<String>) -> JoinedWorktree {
        let live = self.live_record(worktree_path);
        if let Some(live) = &live
            && !created
        {
            return JoinedWorktree {
                id: live.id.clone(),
                superseded: None,
            };
        }
        let record = WorktreeRecord {
            id: uuid::Uuid::new_v4().to_string(),
            path: path::resolve(worktree_path),
            created_at: now_iso(),
            base_sha: base_sha.filter(|sha| !sha.is_empty()),
            merged_at: None,
            merge_mode: None,
            merge_target_branch: None,
            merge_source_ref: None,
            merge_source_sha: None,
            merge_marked_manually: None,
            merge_detected: None,
            deleted_at: None,
            merge_dismissed_tip: None,
            extra: IndexMap::new(),
        };
        let id = record.id.clone();
        let superseded = live.map(|live| marked_deleted(&live, record.created_at.clone()));
        let mut worktrees = self.worktrees.borrow_mut();
        worktrees.records.insert(id.clone(), Ordered::new(record));
        if let Some(superseded) = &superseded {
            worktrees.records.insert(superseded.id.clone(), superseded.clone());
        }
        JoinedWorktree { id, superseded }
    }

    /// TS: InkDaemon.cleanupBlockers: reasons no override may bypass, for deleting the session's worktree.
    ///
    /// The single source for both the inspection shown to the user and enforcement at kill/exit time.
    pub async fn cleanup_blockers(&self, session: &SessionRecord, delete_branch: bool) -> Vec<String> {
        let Some(worktree) = session.worktree.as_ref() else {
            return vec!["session does not have a worktree".to_owned()];
        };
        let worktree_path = match worktree.mode {
            SessionWorktreeMode::None => None,
            _ => worktree.path.as_deref().filter(|root| !root.is_empty()),
        };
        let Some(worktree_path) = worktree_path else {
            return vec!["session does not have a worktree".to_owned()];
        };
        if truthy(worktree.deleted_at.as_deref()) {
            return vec!["worktree was already deleted".to_owned()];
        }
        if worktree.is_main == Some(true) {
            return vec!["cannot delete the main worktree".to_owned()];
        }
        let mut blockers = Vec::new();
        let target = realpath_or_resolve(worktree_path).await;
        if let Some(launch_root) = session.launch_worktree_root.as_deref().filter(|root| !root.is_empty())
            && target == realpath_or_resolve(launch_root).await
        {
            blockers.push("cannot delete the current worktree".to_owned());
        }
        let branch = worktree.branch.as_deref().filter(|branch| !branch.is_empty());
        if delete_branch {
            match branch {
                None => blockers.push("worktree is not on a local branch".to_owned()),
                Some(branch @ ("main" | "master")) => {
                    blockers.push(format!("refusing to delete protected branch {branch}"))
                }
                Some(_) => {}
            }
        }
        for other in self.all_sessions() {
            if other.id == session.id || other.status == SessionStatus::Exited {
                continue;
            }
            let other_worktree = match other
                .worktree
                .as_ref()
                .and_then(|worktree| worktree.path.as_deref())
                .filter(|root| !root.is_empty())
            {
                Some(root) => Some(realpath_or_resolve(root).await),
                None => None,
            };
            if other_worktree.as_deref() == Some(target.as_str())
                || is_path_inside(&target, &realpath_or_resolve(&other.cwd).await)
            {
                blockers.push(format!("worktree is in use by session \"{}\"", other.title));
                break;
            }
        }
        let list_root = session
            .launch_worktree_root
            .clone()
            .unwrap_or_else(|| session.repo_root.clone());
        match list_worktrees(&list_root).await {
            Ok(items) => {
                let mut registered = None;
                for item in items {
                    if realpath_or_resolve(&item.path).await == target {
                        registered = Some(item);
                        break;
                    }
                }
                match registered {
                    Some(registered) if !registered.is_main => {
                        if delete_branch && Some(registered.branch.as_str()) != worktree.branch.as_deref() {
                            blockers.push("Worktree branch changed; refresh before deletion".to_owned());
                        }
                    }
                    _ => blockers.push("Worktree is missing or is the main checkout".to_owned()),
                }
            }
            Err(error) => blockers.push(format!(
                "Worktree registration could not be verified: {}",
                error.message()
            )),
        }
        blockers
    }

    /// TS: InkDaemon.inspectSessionCleanup: what deleting the session's worktree would lose, plus its blockers.
    pub async fn inspect_session_cleanup(
        &self,
        session_id: &str,
        delete_branch: bool,
    ) -> Result<SessionCleanupInspection> {
        let worktree = self.require_session(session_id)?.worktree.clone();
        let mut inspection = empty_inspection(false, Vec::new());
        if let Some(worktree) = worktree.as_ref()
            && let Some(worktree_path) = worktree.path.as_deref().filter(|root| !root.is_empty())
            && worktree.mode != SessionWorktreeMode::None
            && !truthy(worktree.deleted_at.as_deref())
        {
            // Commits up to the one recorded when the worktree was merged (a squash leaves them "unmerged") are
            // integrated.
            let integrated = if truthy(worktree.merged_at.as_deref()) {
                worktree.merge_source_sha.clone()
            } else {
                None
            };
            let options = CleanupOptions {
                delete_branch,
                integrated,
            };
            // A failed inspection is a data-loss reason (overridable), never a silent pass.
            match inspect_workspace_cleanup(worktree_path, worktree.base_ref.as_deref(), &options).await {
                Ok(found) => inspection = found,
                Err(error) => {
                    inspection.reasons = vec![format!("Workspace safety could not be verified: {}", error.message())];
                }
            }
        }
        // Structural checks run after the slow git inspection, against the latest record.
        let session = self.require_session(session_id)?;
        let structural_blockers = self.cleanup_blockers(&session, delete_branch).await;
        inspection.safe = inspection.safe && structural_blockers.is_empty();
        Ok(SessionCleanupInspection {
            inspection,
            structural_blockers,
        })
    }

    /// TS: InkDaemon.assertCleanupAllowed (the blockers joined by `; `, or `Deletion blocked: <reasons>. An explicit
    /// data-loss override is required.`).
    pub async fn assert_cleanup_allowed(
        &self,
        session_id: &str,
        delete_branch: bool,
        allow_data_loss: bool,
    ) -> Result<()> {
        let found = self.inspect_session_cleanup(session_id, delete_branch).await?;
        if !found.structural_blockers.is_empty() {
            return Err(DhError::msg(found.structural_blockers.join("; ")));
        }
        if !found.inspection.safe && !allow_data_loss {
            return Err(DhError::msg(format!(
                "Deletion blocked: {}. An explicit data-loss override is required.",
                found.inspection.reasons.join("; ")
            )));
        }
        Ok(())
    }

    /// TS: InkDaemon.updateWorktreeRecord: patches a record's own fields; `broadcast`: through saveWorktreeRecord.
    ///
    /// Not markers: without `broadcast` its sessions are not touched.
    pub async fn update_worktree_record(
        &self,
        id: &str,
        patch: impl FnOnce(&mut Ordered<WorktreeRecord>),
        broadcast: bool,
    ) -> Result<()> {
        let current = self.worktrees.borrow().records.get(id).cloned();
        let Some(mut record) = current else {
            return Ok(());
        };
        patch(&mut record);
        if broadcast {
            return self.save_worktree_record(record).await;
        }
        self.worktrees.borrow_mut().records.insert(id.to_owned(), record);
        self.persist().await
    }

    // W, the worktree manager: every worktree of a repository (Git's list, plus Deckhand records whose worktree Git no
    // longer lists), with its sessions, merge state and what deleting it would lose; and deletion that does not need a
    // session to be stopped through x. The safety rules are the kill-with-delete ones (cleanupBlockers,
    // inspectWorkspaceCleanup): structural blockers never give way, data reasons only to a typed DELETE.

    /// TS: InkDaemon.repositorySessions: every session of the repository `root` belongs to.
    ///
    /// Whichever checkout it was started from.
    pub async fn repository_sessions(&self, root: &str) -> Vec<Ordered<SessionRecord>> {
        let key = self.tasks_key_for(root).await.ok();
        let mut roots: Vec<String> = Vec::new();
        for session in self.all_sessions() {
            let resolved = path::resolve(&session.repo_root);
            if !roots.contains(&resolved) {
                roots.push(resolved);
            }
        }
        futures::future::join_all(roots.iter().map(|item| self.learn_repo_key(item, false))).await;
        let sessions = self.all_sessions();
        let server = self.server.borrow();
        sessions
            .into_iter()
            .filter(|session| {
                session_matches_scope(session, root)
                    || key
                        .as_ref()
                        .is_some_and(|key| server.repo_keys.get(&path::resolve(&session.repo_root)) == Some(key))
            })
            .collect()
    }

    /// TS: InkDaemon.sessionsInWorktree: its live record's sessions and those whose workspace it is; `inside`: also
    /// any not exited whose directory is in it.
    ///
    /// Deleting it would pull the floor from under those inside. A deleted incarnation's sessions are never in a
    /// later worktree at the same path.
    pub async fn sessions_in_worktree(
        &self,
        worktree_path: &str,
        candidates: &[Ordered<SessionRecord>],
        inside: bool,
    ) -> Vec<Ordered<SessionRecord>> {
        let key = path::resolve(worktree_path);
        let record_id = self.live_record(&key).map(|record| record.id.clone());
        let target = if inside {
            realpath_or_resolve(&key).await
        } else {
            key.clone()
        };
        let mut found = Vec::new();
        for session in candidates {
            let deleted = || {
                truthy(
                    session
                        .worktree
                        .as_ref()
                        .and_then(|worktree| worktree.deleted_at.as_deref()),
                )
            };
            let belongs = if record_id.is_some() && worktree_id(session) == record_id.as_deref() {
                true
            } else if deleted() {
                false
            } else {
                self.workspace_key_of(session).as_deref() == Some(key.as_str())
                    || (inside
                        && session.status != SessionStatus::Exited
                        && is_path_inside(&target, &realpath_or_resolve(&session.cwd).await))
            };
            if belongs {
                found.push(session.clone());
            }
        }
        found
    }

    /// TS: InkDaemon.worktreeOverview: every worktree of the repository with its sessions, markers and (bounded)
    /// what deleting it would lose.
    pub async fn worktree_overview(&self, cwd: &str, client: &ClientHandle) -> Result<WorktreeOverview> {
        let root = find_repo_root(Some(cwd)).await?;
        let sessions = self.repository_sessions(&root).await;
        // Merges done elsewhere first, so the list says what is merged now (bounded, like the periodic run).
        let ids: HashSet<String> = sessions
            .iter()
            .filter_map(|session| {
                let worktree = session.worktree.as_ref()?;
                let id = worktree.id.as_deref().filter(|id| !id.is_empty())?;
                (!truthy(worktree.merged_at.as_deref()) && !truthy(worktree.deleted_at.as_deref()))
                    .then(|| id.to_owned())
            })
            .collect();
        if !ids.is_empty() {
            self.detect_merges(Some(ids)).await;
        }
        let listed = list_worktrees(&root).await?;
        let main_root = listed.iter().find(|item| item.is_main).map(|item| item.path.clone());
        let default_branch = resolve_default_branch(&root, None, BranchWhere::Local)
            .await
            .filter(|branch| !branch.is_empty());
        let defaults = match default_branch.as_deref() {
            Some(branch) => default_branch_refs(&root, branch).await,
            None => Vec::new(),
        };
        let here = realpath_or_resolve(&root).await;
        let mut others = HashSet::new();
        for other in self.other_client_roots(client) {
            others.insert(realpath_or_resolve(&other).await);
        }
        let describe = |entry: &mut WorktreeOverviewEntry,
                        record: Option<&Ordered<WorktreeRecord>>,
                        in_worktree: &[Ordered<SessionRecord>]| {
            if let Some(record) = record {
                entry.record_id = Some(record.id.clone());
                entry.created_at = Some(record.created_at.clone());
                entry.markers = Some(record.markers());
            }
            entry.session_ids = in_worktree.iter().map(|session| session.id.clone()).collect();
            entry.running_ids = in_worktree
                .iter()
                .filter(|session| session.status != SessionStatus::Exited)
                .map(|session| session.id.clone())
                .collect();
        };
        let mut entries: Vec<WorktreeOverviewEntry> = Vec::new();
        for item in &listed {
            let record = if item.is_main {
                None
            } else {
                self.live_record(&item.path)
            };
            let real = realpath_or_resolve(&item.path).await;
            let mut entry = WorktreeOverviewEntry {
                path: item.path.clone(),
                branch: item.branch.clone(),
                head: item.head.clone(),
                is_main: item.is_main,
                locked: (item.locked == Some(true)).then_some(true),
                missing: (item.prunable == Some(true)).then_some(WorktreeMissing::Prunable),
                record_id: None,
                created_at: None,
                markers: None,
                session_ids: Vec::new(),
                running_ids: Vec::new(),
                in_use: None,
                error: None,
                last_commit_at: None,
                ahead_of_default: None,
                inspection: None,
            };
            let in_worktree = self.sessions_in_worktree(&item.path, &sessions, !item.is_main).await;
            describe(&mut entry, record.as_ref(), &in_worktree);
            entry.in_use = if real == here {
                Some(WorktreeInUse::This)
            } else if others.contains(&real) {
                Some(WorktreeInUse::Other)
            } else {
                None
            };
            entries.push(entry);
        }
        // Worktrees removed outside Deckhand: their records are still live until forgotten.
        let listed_paths: HashSet<String> = listed.iter().map(|item| path::resolve(&item.path)).collect();
        let record_ids: HashSet<&str> = sessions
            .iter()
            .filter_map(|session| worktree_id(session).filter(|id| !id.is_empty()))
            .collect();
        let records: Vec<Ordered<WorktreeRecord>> = self.worktrees.borrow().records.values().cloned().collect();
        for record in &records {
            if truthy(record.deleted_at.as_deref())
                || !record_ids.contains(record.id.as_str())
                || listed_paths.contains(&path::resolve(&record.path))
            {
                continue;
            }
            let in_worktree: Vec<Ordered<SessionRecord>> = sessions
                .iter()
                .filter(|session| worktree_id(session) == Some(record.id.as_str()))
                .cloned()
                .collect();
            let branch = in_worktree
                .iter()
                .find_map(|session| worktree_branch(session).map(str::to_owned))
                .unwrap_or_default();
            let mut entry = WorktreeOverviewEntry {
                path: record.path.clone(),
                branch,
                head: String::new(),
                is_main: false,
                locked: None,
                missing: Some(WorktreeMissing::Unlisted),
                record_id: None,
                created_at: None,
                markers: None,
                session_ids: Vec::new(),
                running_ids: Vec::new(),
                in_use: None,
                error: None,
                last_commit_at: None,
                ahead_of_default: None,
                inspection: None,
            };
            describe(&mut entry, Some(record), &in_worktree);
            entries.push(entry);
        }
        let linked: Vec<(usize, String, Option<String>)> = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| !entry.is_main && entry.missing.is_none())
            .map(|(index, entry)| {
                // The categories ask: would deleting it and its branch lose anything? (A detached HEAD's commits count
                // either way.)
                let integrated = entry
                    .markers
                    .as_ref()
                    .filter(|markers| truthy(markers.merged_at.as_deref()))
                    .and_then(|markers| markers.merge_source_sha.clone());
                (index, entry.path.clone(), integrated)
            })
            .collect();
        let unchecked = linked.len().saturating_sub(WORKTREE_OVERVIEW_MAX);
        let checked: Vec<_> = linked.into_iter().take(WORKTREE_OVERVIEW_MAX).collect();
        type Checked = std::result::Result<(WorktreeActivity, CleanupInspection), String>;
        let outcomes: RefCell<Vec<(usize, Checked)>> = RefCell::new(Vec::new());
        {
            let outcomes = &outcomes;
            let defaults = &defaults;
            for_each_limited(
                checked,
                WORKTREE_OVERVIEW_CONCURRENCY,
                |(index, entry_path, integrated)| async move {
                    let deleting = self.worktrees.borrow().cleanup.contains(&path::resolve(&entry_path));
                    let outcome = if deleting {
                        Err("being deleted".to_owned())
                    } else {
                        let options = CleanupOptions {
                            delete_branch: true,
                            integrated,
                        };
                        let (inspection, activity) = futures::join!(
                            inspect_workspace_cleanup(&entry_path, None, &options),
                            worktree_activity(&entry_path, defaults)
                        );
                        match inspection {
                            Ok(inspection) => Ok((activity, inspection)),
                            Err(error) => Err(error.message().to_owned()),
                        }
                    };
                    outcomes.borrow_mut().push((index, outcome));
                },
            )
            .await;
        }
        for (index, outcome) in outcomes.into_inner() {
            let Some(entry) = entries.get_mut(index) else {
                continue;
            };
            match outcome {
                Ok((activity, inspection)) => {
                    entry.last_commit_at = activity.last_commit_at;
                    entry.ahead_of_default = activity.ahead_of_default;
                    entry.inspection = Some(inspection);
                }
                Err(error) => entry.error = Some(error),
            }
        }
        Ok(WorktreeOverview {
            main_root,
            default_branch,
            entries,
            unchecked: (unchecked > 0).then(|| i64::try_from(unchecked).unwrap_or(i64::MAX)),
            checked_at: now_iso(),
        })
    }

    /// TS: InkDaemon.worktreeTarget (`Invalid worktree path`, `This worktree is not listed by Git any more; refresh
    /// (R)`).
    ///
    /// One worktree of the repository `cwd` belongs to, as `inspectWorktree`/`deleteWorktree` act on it.
    pub async fn worktree_target(&self, cwd: &str, worktree_path: &str) -> Result<WorktreeTarget> {
        if !path::is_absolute(worktree_path) {
            return Err(DhError::msg("Invalid worktree path"));
        }
        let root = find_repo_root(Some(cwd)).await?;
        let key = path::resolve(worktree_path);
        let item = list_worktrees(&root)
            .await?
            .into_iter()
            .find(|entry| path::resolve(&entry.path) == key);
        let is_main = item.as_ref().is_some_and(|item| item.is_main);
        let record = if is_main { None } else { self.live_record(&key) };
        let repository = self.repository_sessions(&root).await;
        // A record Git no longer lists must belong to this repository to be forgotten from here.
        let owned = record.as_ref().is_some_and(|record| {
            repository
                .iter()
                .any(|session| worktree_id(session) == Some(record.id.as_str()))
        });
        if item.is_none() && !owned {
            return Err(DhError::msg("This worktree is not listed by Git any more; refresh (R)"));
        }
        let sessions = self.sessions_in_worktree(&key, &repository, !is_main).await;
        Ok(WorktreeTarget {
            root,
            key,
            item,
            record,
            sessions,
        })
    }

    /// TS: InkDaemon.worktreeBlockers: reasons no DELETE may bypass, for a worktree deleted from W.
    ///
    /// The kill path's are cleanupBlockers.
    pub async fn worktree_blockers(
        &self,
        target: &WorktreeTarget,
        delete_branch: bool,
        client: &ClientHandle,
        expected_branch: Option<&str>,
        deleting: bool,
    ) -> Vec<String> {
        // Missing: only its administrative entry or Deckhand's record goes; nothing on disk.
        let Some(item) = target.item.as_ref() else {
            return Vec::new();
        };
        if item.prunable == Some(true) {
            return Vec::new();
        }
        if item.is_main {
            return vec!["cannot delete the main checkout".to_owned()];
        }
        let mut blockers = Vec::new();
        let real = realpath_or_resolve(&target.key).await;
        if realpath_or_resolve(&target.root).await == real {
            blockers.push("this Deckhand runs in that worktree; delete it from another checkout".to_owned());
        } else {
            for other in self.other_client_roots(client) {
                if is_path_inside(&real, &realpath_or_resolve(&other).await) {
                    blockers.push("another Deckhand is open in that worktree".to_owned());
                    break;
                }
            }
        }
        if item.locked == Some(true) {
            blockers.push("worktree is locked (git worktree unlock it first)".to_owned());
        }
        if !deleting && self.worktrees.borrow().cleanup.contains(&target.key) {
            blockers.push("worktree is already being deleted".to_owned());
        }
        if let Some(expected) = expected_branch
            && item.branch != expected
        {
            blockers.push("Worktree branch changed; refresh before deletion".to_owned());
        }
        if delete_branch {
            let default_branch = resolve_default_branch(&target.root, None, BranchWhere::Local)
                .await
                .unwrap_or_else(|| "main".to_owned());
            let protected = ["main", "master", default_branch.as_str()];
            if item.branch.is_empty() {
                blockers.push("worktree is not on a local branch".to_owned());
            } else if protected.contains(&item.branch.as_str()) {
                blockers.push(format!("refusing to delete protected branch {}", item.branch));
            }
        }
        if let Some(starting) = target
            .sessions
            .iter()
            .find(|session| session.status == SessionStatus::Starting)
        {
            blockers.push(format!(
                "session \"{}\" is starting; wait for it or cancel it",
                starting.title
            ));
        }
        blockers
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
        let target = self.worktree_target(cwd, worktree_path).await?;
        let mut inspection = empty_inspection(true, Vec::new());
        if let Some(item) = target.item.as_ref()
            && !item.is_main
            && item.prunable != Some(true)
        {
            // Commits up to the one recorded when it was merged (a squash leaves them "unmerged") are integrated.
            let integrated = target
                .record
                .as_ref()
                .filter(|record| truthy(record.merged_at.as_deref()))
                .and_then(|record| record.merge_source_sha.clone());
            let options = CleanupOptions {
                delete_branch,
                integrated,
            };
            // A failed inspection is a data-loss reason (overridable), never a silent pass.
            inspection = match inspect_workspace_cleanup(&target.key, None, &options).await {
                Ok(found) => found,
                Err(error) => empty_inspection(
                    false,
                    vec![format!("Workspace safety could not be verified: {}", error.message())],
                ),
            };
        }
        let structural_blockers = self
            .worktree_blockers(&target, delete_branch, client, expected_branch, deleting)
            .await;
        let running = target
            .sessions
            .iter()
            .filter(|session| session.status == SessionStatus::Running)
            .map(|session| SessionTitle {
                id: session.id.clone(),
                title: session.title.clone(),
            })
            .collect();
        inspection.safe = inspection.safe && structural_blockers.is_empty();
        Ok(InspectedWorktree {
            inspection: WorktreeCleanupInspection {
                inspection: SessionCleanupInspection {
                    inspection,
                    structural_blockers,
                },
                running,
            },
            target,
        })
    }

    /// deleteWorktree's `check`: inspects it and throws its blockers, or its data-loss reasons without the override.
    async fn check_worktree_deletion(
        &self,
        message: &DeleteWorktree,
        client: &ClientHandle,
        delete_branch: bool,
        deleting: bool,
    ) -> Result<InspectedWorktree> {
        let result = self
            .inspect_worktree(
                &message.cwd,
                &message.path,
                delete_branch,
                client,
                message.branch.as_deref(),
                deleting,
            )
            .await?;
        let found = &result.inspection.inspection;
        if !found.structural_blockers.is_empty() {
            return Err(DhError::msg(found.structural_blockers.join("; ")));
        }
        if !found.inspection.safe && message.allow_data_loss != Some(true) {
            return Err(DhError::msg(format!(
                "Deletion blocked: {}. An explicit data-loss override is required.",
                found.inspection.reasons.join("; ")
            )));
        }
        Ok(result)
    }

    /// TS: InkDaemon.deleteWorktree: checks, stops its running sessions when asked (and checks again), stops its
    /// workspace, removes it (and its branch), marks its record deleted and archives its sessions.
    ///
    /// The second check runs because the stopped agents may have written files. A worktree whose directory is gone
    /// is pruned (Git) or forgotten (Deckhand's record only).
    pub async fn delete_worktree(
        &self,
        message: &DeleteWorktree,
        client: &ClientHandle,
    ) -> Result<WorktreeDeleteResult> {
        let delete_branch = message.delete_branch == Some(true);
        let InspectedWorktree { inspection, target } = self
            .check_worktree_deletion(message, client, delete_branch, false)
            .await?;
        if !inspection.running.is_empty() && message.stop_sessions != Some(true) {
            let titles: Vec<&str> = inspection
                .running
                .iter()
                .map(|session| session.title.as_str())
                .collect();
            return Err(DhError::msg(format!(
                "{} session(s) still running in it ({}); confirm stopping them",
                inspection.running.len(),
                titles.join(", ")
            )));
        }
        let key = target.key.clone();
        // Checked again after the awaits above: two deletions of one worktree never both proceed.
        if !self.worktrees.borrow_mut().cleanup.insert(key.clone()) {
            return Err(DhError::msg("worktree is already being deleted"));
        }
        let mut stopped: i64 = 0;
        let mut sent: usize = 0;
        let mut branch_deleted: Option<String> = None;
        let removal: Result<WorktreeRemoved> = async {
            let item = target.item.as_ref();
            let send_open_items = message.send_open_items == Some(true);
            // Missing: nothing on disk to stop or check, so its note's open items can go at once.
            let missing = item.is_none_or(|item| item.prunable == Some(true));
            if missing && send_open_items {
                let left_in = item
                    .map(|item| item.branch.clone())
                    .filter(|branch| !branch.is_empty())
                    .or_else(|| {
                        target
                            .sessions
                            .iter()
                            .find_map(|session| worktree_branch(session).map(str::to_owned))
                    });
                sent = self
                    .send_worktree_items_to_backlog(target.record.as_deref(), &target.root, left_in.as_deref())
                    .await?;
            }
            let Some(item) = item else {
                return Ok(WorktreeRemoved::Forgotten);
            };
            if item.prunable == Some(true) {
                prune_worktrees(&target.root).await?;
                return Ok(WorktreeRemoved::Pruned);
            }
            for session in &inspection.running {
                self.stop_for_deletion(&session.id).await?;
                stopped += 1;
            }
            // The agents may have written files until they stopped: check again (nothing is deleted on a new reason).
            if stopped > 0 {
                self.check_worktree_deletion(message, client, delete_branch, true)
                    .await?;
            }
            self.stop_changes_watch(&key);
            self.retire_workspace(&key).await?;
            // Its note's open items first, while the worktree record is still live (the confirmation offered it).
            if send_open_items {
                sent = self
                    .send_worktree_items_to_backlog(target.record.as_deref(), &target.root, Some(&item.branch))
                    .await?;
            }
            let name = target.sessions.iter().find_map(|session| {
                session
                    .worktree
                    .as_ref()
                    .and_then(|worktree| worktree.name.clone())
                    .filter(|name| !name.is_empty())
            });
            remove_worktree(&key, &target.root, name.as_deref()).await?;
            if delete_branch && !item.branch.is_empty() {
                delete_local_branch(&target.root, &item.branch).await?;
                branch_deleted = Some(item.branch.clone());
            }
            Ok(WorktreeRemoved::Deleted)
        }
        .await;
        self.worktrees.borrow_mut().cleanup.remove(&key);
        let removed = removal?;
        let now = now_iso();
        let record = target
            .record
            .as_ref()
            .and_then(|record| self.worktrees.borrow().records.get(&record.id).cloned());
        if let Some(record) = record
            && !truthy(record.deleted_at.as_deref())
        {
            self.save_worktree_record(marked_deleted(&record, now.clone())).await?;
        }
        // Its sessions cannot be resumed any more: out of the active list (their notes stay readable under f A).
        let mut archived: i64 = 0;
        for in_worktree in &target.sessions {
            let Some(mut session) = self.get_session(&in_worktree.id) else {
                continue;
            };
            if session.status != SessionStatus::Exited || truthy(session.archived_at.as_deref()) {
                continue;
            }
            session.touch("archivedAt");
            session.archived_at = Some(now.clone());
            self.save_session(session).await?;
            archived += 1;
        }
        let branch_note = branch_deleted
            .as_ref()
            .map(|branch| format!(", branch {branch} deleted"))
            .unwrap_or_default();
        let stopped_note = if stopped > 0 {
            format!(", {stopped} session(s) stopped")
        } else {
            String::new()
        };
        self.log(&format!(
            "worktree {key} {} from W{branch_note}{stopped_note}",
            removed.as_str()
        ))
        .await;
        Ok(WorktreeDeleteResult {
            removed,
            branch_deleted,
            stopped,
            archived,
            sent_items: (sent > 0).then(|| i64::try_from(sent).unwrap_or(i64::MAX)),
        })
    }

    /// TS: InkDaemon.stopForDeletion (`Session "<title>" did not stop; nothing was deleted`).
    ///
    /// Stops a running session as x does (its worktree kept by this kill) and waits until it has exited.
    pub async fn stop_for_deletion(&self, session_id: &str) -> Result<()> {
        let current = self.get_session(session_id);
        let title = current
            .as_ref()
            .map_or_else(|| session_id.to_owned(), |session| session.title.clone());
        if current.is_some_and(|session| session.status == SessionStatus::Running)
            && let Err(error) = self.kill_session(session_id, KillOptions::default()).await
            && !error.message().contains("not running")
        {
            return Err(error);
        }
        let deadline = Instant::now() + STOP_FOR_DELETE;
        loop {
            match self.get_session(session_id) {
                None => return Ok(()),
                Some(session) if session.status == SessionStatus::Exited => return Ok(()),
                Some(_) => {}
            }
            if Instant::now() > deadline {
                return Err(DhError::msg(format!(
                    "Session \"{title}\" did not stop; nothing was deleted"
                )));
            }
            tokio::time::sleep(STOP_POLL).await;
        }
    }

    /// TS: handleRequest `list-worktrees` (git.ts listWorktrees).
    pub async fn handle_list_worktrees(&self, request: &CwdRequest) -> Result<Vec<WorktreeInfo>> {
        list_worktrees(&request.cwd).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::{local, session, test_client, test_daemon};
    use crate::server::{Client, ClientSubscription};
    use serde_json::{Value, json};
    use std::cell::Cell;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::{Mutex, MutexGuard};

    /// Tests that set DECKHAND_HOME run one at a time.
    static ENV: Mutex<()> = Mutex::new(());

    /// DECKHAND_HOME = `dir` while this lives.
    struct Home {
        _guard: MutexGuard<'static, ()>,
        previous: Option<std::ffi::OsString>,
    }

    impl Home {
        fn set(dir: &Path) -> Home {
            let guard = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let previous = std::env::var_os(deckhand_sys::paths::HOME_ENV);
            // SAFETY: tests touching the environment hold ENV.
            unsafe { std::env::set_var(deckhand_sys::paths::HOME_ENV, dir) };
            Home {
                _guard: guard,
                previous,
            }
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            // SAFETY: as in Home::set (ENV is still held).
            unsafe {
                match self.previous.take() {
                    Some(value) => std::env::set_var(deckhand_sys::paths::HOME_ENV, value),
                    None => std::env::remove_var(deckhand_sys::paths::HOME_ENV),
                }
            }
        }
    }

    /// A daemon without a notes store (the notes projection belongs to notes_tasks.rs).
    fn daemon(dir: &Path) -> Daemon {
        let daemon = test_daemon(dir);
        daemon.notes_tasks.borrow_mut().notes = None;
        daemon
    }

    fn git(cwd: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(["-c", "user.name=Deckhand Test", "-c", "user.email=test@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    /// A temp dir (canonical, so realpaths compare equal) with a repository `repo` (one commit on main) and a linked
    /// worktree `wt` on branch `feature`.
    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        repo: String,
        linked: String,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("README.md"), "hi\n").unwrap();
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        let linked = root.join("wt");
        git(
            &repo,
            &["worktree", "add", "-q", "-b", "feature", linked.to_str().unwrap()],
        );
        Fixture {
            repo: repo.to_string_lossy().into_owned(),
            linked: linked.to_string_lossy().into_owned(),
            root,
            _dir: dir,
        }
    }

    /// A session in `cwd` of `repo` with the given `worktree` and status.
    fn in_worktree(id: &str, repo: &str, cwd: &str, worktree: Value, status: &str) -> Ordered<SessionRecord> {
        let mut value = session(id, repo, cwd).to_value().unwrap();
        value["worktree"] = worktree;
        value["status"] = json!(status);
        serde_json::from_value(value).unwrap()
    }

    fn insert(daemon: &Daemon, session: Ordered<SessionRecord>) {
        daemon
            .sessions
            .borrow_mut()
            .sessions
            .insert(session.id.clone(), session);
    }

    fn record(id: &str, path: &str) -> Ordered<WorktreeRecord> {
        serde_json::from_value(json!({"id": id, "path": path, "createdAt": "2026-01-01T00:00:00.000Z"})).unwrap()
    }

    fn info(path: &str, branch: &str, is_main: bool) -> WorktreeInfo {
        WorktreeInfo {
            path: path.to_owned(),
            branch: branch.to_owned(),
            head: "abc".to_owned(),
            is_main,
            locked: None,
            prunable: None,
        }
    }

    fn target(root: &str, key: &str, item: Option<WorktreeInfo>) -> WorktreeTarget {
        WorktreeTarget {
            root: root.to_owned(),
            key: key.to_owned(),
            item,
            record: None,
            sessions: Vec::new(),
        }
    }

    #[test]
    fn for_each_limited_runs_every_item_at_most_limit_at_a_time() {
        local(async {
            let active = Rc::new(Cell::new(0usize));
            let peak = Rc::new(Cell::new(0usize));
            let done = Rc::new(RefCell::new(Vec::new()));
            for_each_limited((0..7).collect(), 3, |item: i32| {
                let (active, peak, done) = (active.clone(), peak.clone(), done.clone());
                async move {
                    active.set(active.get() + 1);
                    peak.set(peak.get().max(active.get()));
                    tokio::time::sleep(Duration::from_millis(2)).await;
                    active.set(active.get() - 1);
                    done.borrow_mut().push(item);
                }
            })
            .await;
            let mut done = done.borrow().clone();
            done.sort_unstable();
            assert_eq!(done, (0..7).collect::<Vec<_>>());
            assert_eq!(peak.get(), 3);

            // More workers than items: one per item; none: nothing runs.
            let count = Rc::new(Cell::new(0));
            for_each_limited(vec![1, 2], 10, |_| {
                let count = count.clone();
                async move { count.set(count.get() + 1) }
            })
            .await;
            assert_eq!(count.get(), 2);
            for_each_limited(vec![1, 2], 0, |_| {
                let count = count.clone();
                async move { count.set(count.get() + 1) }
            })
            .await;
            assert_eq!(count.get(), 2);
        });
    }

    #[test]
    fn realpath_or_resolve_follows_links_and_falls_back_to_the_lexical_path() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let real = std::fs::canonicalize(dir.path()).unwrap();
            std::fs::create_dir(real.join("target")).unwrap();
            std::os::unix::fs::symlink(real.join("target"), real.join("link")).unwrap();
            let link = real.join("link").to_string_lossy().into_owned();
            assert_eq!(realpath_or_resolve(&link).await, real.join("target").to_string_lossy());
            let missing = format!("{}/missing/../gone/", real.display());
            assert_eq!(realpath_or_resolve(&missing).await, format!("{}/gone", real.display()));
            assert_eq!(realpath_or_resolve("relative/x").await, path::resolve("relative/x"));
        });
    }

    #[test]
    fn linked_worktree_roots_are_told_from_the_main_checkout() {
        let fixture = fixture();
        local(async {
            assert!(!is_linked_worktree_root(&fixture.repo).await);
            assert!(is_linked_worktree_root(&fixture.linked).await);
            assert!(is_linked_worktree_root(&format!("{}/", fixture.linked)).await);
            // Not a repository: Git fails, so no.
            assert!(!is_linked_worktree_root(&fixture.root.to_string_lossy()).await);
        });
    }

    #[test]
    fn list_worktrees_answers_the_git_shape_with_locked_and_prunable() {
        let fixture = fixture();
        git(Path::new(&fixture.repo), &["worktree", "lock", &fixture.linked]);
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon(dir.path());
        local(async {
            let request = CwdRequest {
                request_id: "r".to_owned(),
                cwd: fixture.repo.clone(),
            };
            let listed = daemon.handle_list_worktrees(&request).await.unwrap();
            assert_eq!(listed.len(), 2);
            assert_eq!(listed[0].path, fixture.repo);
            assert!(listed[0].is_main);
            assert_eq!(listed[0].branch, "main");
            assert_eq!(listed[1].path, fixture.linked);
            assert_eq!(listed[1].branch, "feature");
            assert_eq!(listed[1].locked, Some(true));
            let wire = serde_json::to_value(&listed[1]).unwrap();
            assert_eq!(
                wire.as_object().unwrap().keys().collect::<Vec<_>>(),
                ["path", "branch", "head", "isMain", "locked"]
            );
            // A directory removed behind Git's back is listed as prunable.
            git(Path::new(&fixture.repo), &["worktree", "unlock", &fixture.linked]);
            std::fs::remove_dir_all(&fixture.linked).unwrap();
            let listed = daemon.handle_list_worktrees(&request).await.unwrap();
            assert_eq!(listed[1].prunable, Some(true));
            assert_eq!(listed[1].locked, None);
            let missing = CwdRequest {
                request_id: "r".to_owned(),
                cwd: fixture.root.to_string_lossy().into_owned(),
            };
            assert!(daemon.handle_list_worktrees(&missing).await.is_err());
        });
    }

    #[test]
    fn joining_a_worktree_reuses_the_live_incarnation_unless_deckhand_created_it() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon(dir.path());
        let first = daemon.join_worktree("/repo/wt/", false, Some("abc".to_owned()));
        assert!(first.superseded.is_none());
        let created = daemon.worktrees.borrow().records.get(&first.id).cloned().unwrap();
        assert_eq!(created.path, "/repo/wt");
        assert_eq!(created.base_sha.as_deref(), Some("abc"));
        assert_eq!(
            created
                .to_value()
                .unwrap()
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            ["id", "path", "createdAt", "baseSha"]
        );

        // Another session there joins it.
        let joined = daemon.join_worktree("/repo/wt", false, None);
        assert_eq!(
            joined,
            JoinedWorktree {
                id: first.id.clone(),
                superseded: None
            }
        );

        // Created again at the path: a new incarnation, the old one marked deleted at once.
        let again = daemon.join_worktree("/repo/wt", true, Some(String::new()));
        assert_ne!(again.id, first.id);
        let superseded = again.superseded.clone().unwrap();
        assert_eq!(superseded.id, first.id);
        {
            let worktrees = daemon.worktrees.borrow();
            let fresh = worktrees.records.get(&again.id).unwrap();
            assert_eq!(fresh.base_sha, None);
            assert_eq!(superseded.deleted_at.as_deref(), Some(fresh.created_at.as_str()));
            assert_eq!(worktrees.records.get(&first.id).unwrap(), &superseded);
        }
        assert_eq!(
            superseded
                .to_value()
                .unwrap()
                .as_object()
                .unwrap()
                .keys()
                .next_back()
                .map(String::as_str),
            Some("deletedAt")
        );

        // Nothing live there: created or not, a new one.
        let elsewhere = daemon.join_worktree("/repo/other", false, None);
        assert!(elsewhere.superseded.is_none());
        assert_eq!(daemon.worktrees.borrow().records.len(), 3);
    }

    #[test]
    fn a_session_s_worktree_record_is_found_by_its_id() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon(dir.path());
        daemon
            .worktrees
            .borrow_mut()
            .records
            .insert("w1".to_owned(), record("w1", "/repo/wt"));
        let inside = in_worktree(
            "a",
            "/repo",
            "/repo/wt",
            json!({"mode": "managed", "path": "/repo/wt", "id": "w1"}),
            "running",
        );
        assert_eq!(
            daemon
                .worktree_record_of(&inside)
                .map(|record| record.id.clone())
                .as_deref(),
            Some("w1")
        );
        let unknown = in_worktree(
            "b",
            "/repo",
            "/repo/wt",
            json!({"mode": "managed", "id": "nope"}),
            "running",
        );
        assert!(daemon.worktree_record_of(&unknown).is_none());
        assert!(daemon.worktree_record_of(&session("c", "/repo", "/repo")).is_none());
        let empty = in_worktree("d", "/repo", "/repo", json!({"mode": "none", "id": ""}), "running");
        assert!(daemon.worktree_record_of(&empty).is_none());
    }

    #[test]
    fn cleanup_blockers_refuse_what_no_override_may_delete() {
        let fixture = fixture();
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon(dir.path());
        let repo = fixture.repo.as_str();
        let linked = fixture.linked.as_str();
        local(async {
            let blockers = |session: Ordered<SessionRecord>, delete_branch: bool| {
                let daemon = daemon.clone();
                async move { daemon.cleanup_blockers(&session, delete_branch).await }
            };
            assert_eq!(
                blockers(session("a", repo, repo), false).await,
                ["session does not have a worktree"]
            );
            let none = in_worktree("a", repo, repo, json!({"mode": "none", "path": repo}), "running");
            assert_eq!(blockers(none, false).await, ["session does not have a worktree"]);
            let deleted = in_worktree(
                "a",
                repo,
                linked,
                json!({"mode": "managed", "path": linked, "deletedAt": "x"}),
                "exited",
            );
            assert_eq!(blockers(deleted, false).await, ["worktree was already deleted"]);
            let main = in_worktree(
                "a",
                repo,
                repo,
                json!({"mode": "attached", "path": repo, "isMain": true}),
                "running",
            );
            assert_eq!(blockers(main, false).await, ["cannot delete the main worktree"]);

            // A deletable worktree.
            let owner = in_worktree(
                "a",
                repo,
                linked,
                json!({"mode": "managed", "path": linked, "branch": "feature"}),
                "running",
            );
            assert!(blockers(owner.clone(), true).await.is_empty());

            // Launched from inside it.
            let mut current = owner.clone();
            current.launch_worktree_root = Some(linked.to_owned());
            assert_eq!(blockers(current, false).await, ["cannot delete the current worktree"]);

            // Branch checks.
            let detached = in_worktree("a", repo, linked, json!({"mode": "managed", "path": linked}), "running");
            assert_eq!(
                blockers(detached.clone(), true).await,
                [
                    "worktree is not on a local branch",
                    "Worktree branch changed; refresh before deletion"
                ]
            );
            assert!(blockers(detached, false).await.is_empty());
            let on_master = in_worktree(
                "a",
                repo,
                linked,
                json!({"mode": "managed", "path": linked, "branch": "master"}),
                "running",
            );
            assert_eq!(
                blockers(on_master, true).await,
                [
                    "refusing to delete protected branch master",
                    "Worktree branch changed; refresh before deletion"
                ]
            );
            let renamed = in_worktree(
                "a",
                repo,
                linked,
                json!({"mode": "managed", "path": linked, "branch": "old"}),
                "running",
            );
            assert_eq!(
                blockers(renamed, true).await,
                ["Worktree branch changed; refresh before deletion"]
            );

            // Another live session in it (an exited one does not count).
            let inner = format!("{linked}/src");
            insert(
                &daemon,
                in_worktree("other", repo, &inner, json!({"mode": "none"}), "exited"),
            );
            assert!(blockers(owner.clone(), false).await.is_empty());
            let mut busy = in_worktree("other", repo, &inner, json!({"mode": "none"}), "running");
            busy.title = "Busy one".to_owned();
            insert(&daemon, busy);
            assert_eq!(
                blockers(owner.clone(), false).await,
                ["worktree is in use by session \"Busy one\""]
            );
            daemon.sessions.borrow_mut().sessions.clear();

            // Not registered with Git (or the main checkout).
            let gone = format!("{}/gone", fixture.root.display());
            let unregistered = in_worktree("a", repo, &gone, json!({"mode": "managed", "path": gone}), "running");
            assert_eq!(
                blockers(unregistered, false).await,
                ["Worktree is missing or is the main checkout"]
            );
            let main_path = in_worktree("a", repo, repo, json!({"mode": "attached", "path": repo}), "running");
            assert_eq!(
                blockers(main_path, false).await,
                ["Worktree is missing or is the main checkout"]
            );

            // Git cannot answer.
            let outside = fixture.root.join("plain");
            std::fs::create_dir(&outside).unwrap();
            let mut unverifiable = owner;
            unverifiable.repo_root = outside.to_string_lossy().into_owned();
            let found = blockers(unverifiable, false).await;
            assert_eq!(found.len(), 1);
            assert!(
                found[0].starts_with("Worktree registration could not be verified: Command failed: git ")
                    && found[0].contains("not a git repository"),
                "{found:?}"
            );
        });
    }

    #[test]
    fn session_cleanup_inspection_and_its_enforcement() {
        let fixture = fixture();
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon(dir.path());
        let repo = fixture.repo.as_str();
        let linked = fixture.linked.as_str();
        local(async {
            assert_eq!(
                daemon
                    .inspect_session_cleanup("nope", false)
                    .await
                    .unwrap_err()
                    .to_string(),
                "Session does not exist"
            );
            let feature = json!({"mode": "managed", "path": linked, "branch": "feature"});
            insert(&daemon, in_worktree("a", repo, linked, feature, "exited"));
            let clean = daemon.inspect_session_cleanup("a", false).await.unwrap();
            assert!(clean.inspection.safe, "{clean:?}");
            assert!(clean.structural_blockers.is_empty());
            daemon.assert_cleanup_allowed("a", false, false).await.unwrap();

            // Uncommitted work: a data-loss reason, overridable.
            std::fs::write(Path::new(linked).join("new.txt"), "x").unwrap();
            let dirty = daemon.inspect_session_cleanup("a", false).await.unwrap();
            assert!(!dirty.inspection.safe);
            assert!(!dirty.inspection.reasons.is_empty());
            let message = daemon
                .assert_cleanup_allowed("a", false, false)
                .await
                .unwrap_err()
                .to_string();
            assert_eq!(
                message,
                format!(
                    "Deletion blocked: {}. An explicit data-loss override is required.",
                    dirty.inspection.reasons.join("; ")
                )
            );
            daemon.assert_cleanup_allowed("a", false, true).await.unwrap();

            // Structural blockers never give way.
            let on_main = json!({"mode": "managed", "path": linked, "branch": "main"});
            insert(&daemon, in_worktree("a", repo, linked, on_main, "exited"));
            let blocked = daemon.inspect_session_cleanup("a", true).await.unwrap();
            assert!(!blocked.inspection.safe);
            assert_eq!(
                blocked.structural_blockers[0],
                "refusing to delete protected branch main"
            );
            assert_eq!(
                daemon
                    .assert_cleanup_allowed("a", true, true)
                    .await
                    .unwrap_err()
                    .to_string(),
                blocked.structural_blockers.join("; ")
            );

            // No worktree: nothing inspected (unsafe), and the structural reason.
            insert(&daemon, session("b", repo, repo));
            let none = daemon.inspect_session_cleanup("b", false).await.unwrap();
            assert!(!none.inspection.safe);
            assert!(none.inspection.reasons.is_empty());
            assert_eq!(none.structural_blockers, ["session does not have a worktree"]);
            assert_eq!(
                daemon
                    .assert_cleanup_allowed("b", false, true)
                    .await
                    .unwrap_err()
                    .to_string(),
                "session does not have a worktree"
            );

            // An invalid base ref fails the inspection: an overridable reason.
            let bad_base = json!({"mode": "managed", "path": linked, "branch": "feature", "baseRef": "-bad"});
            insert(&daemon, in_worktree("a", repo, linked, bad_base, "exited"));
            let failed = daemon.inspect_session_cleanup("a", false).await.unwrap();
            assert!(!failed.inspection.safe);
            assert_eq!(failed.inspection.reasons.len(), 1);
            assert!(failed.inspection.reasons[0].starts_with("Workspace safety could not be verified: "));
        });
    }

    #[test]
    fn worktree_blockers_guard_deletion_from_w() {
        let fixture = fixture();
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon(dir.path());
        let (client, _rx) = test_client(1);
        let repo = fixture.repo.as_str();
        let linked = fixture.linked.as_str();
        local(async {
            // Missing (no entry, or prunable): nothing on disk, nothing blocks.
            let none = target(repo, linked, None);
            assert!(
                daemon
                    .worktree_blockers(&none, true, &client, None, false)
                    .await
                    .is_empty()
            );
            let mut prunable = info(linked, "", false);
            prunable.prunable = Some(true);
            prunable.locked = Some(true);
            let prunable = target(repo, linked, Some(prunable));
            assert!(
                daemon
                    .worktree_blockers(&prunable, true, &client, Some("x"), false)
                    .await
                    .is_empty()
            );
            let main = target(repo, repo, Some(info(repo, "main", true)));
            assert_eq!(
                daemon.worktree_blockers(&main, false, &client, None, false).await,
                ["cannot delete the main checkout"]
            );

            let feature = target(repo, linked, Some(info(linked, "feature", false)));
            assert!(
                daemon
                    .worktree_blockers(&feature, true, &client, Some("feature"), false)
                    .await
                    .is_empty()
            );

            // W running inside it.
            let here = target(linked, linked, Some(info(linked, "feature", false)));
            assert_eq!(
                daemon.worktree_blockers(&here, false, &client, None, false).await,
                ["this Deckhand runs in that worktree; delete it from another checkout"]
            );

            // Another Deckhand open inside it (the asking client's own checkout does not count).
            let (other, _other_rx) = test_client(2);
            daemon.server.borrow_mut().clients.insert(
                other.id,
                Client {
                    handle: other.clone(),
                    subscription: ClientSubscription {
                        repo_root: Some(format!("{linked}/sub")),
                        ..ClientSubscription::default()
                    },
                },
            );
            assert!(
                daemon
                    .worktree_blockers(&feature, false, &other, None, false)
                    .await
                    .is_empty()
            );
            assert_eq!(
                daemon.worktree_blockers(&feature, false, &client, None, false).await,
                ["another Deckhand is open in that worktree"]
            );
            daemon.server.borrow_mut().clients.clear();

            // Locked, already being deleted, branch changed, protected or no branch, a session starting.
            let mut locked = info(linked, "main", false);
            locked.locked = Some(true);
            let mut busy = target(repo, linked, Some(locked));
            let mut starting = session("s", repo, linked);
            starting.status = SessionStatus::Starting;
            starting.title = "Starter".to_owned();
            busy.sessions = vec![session("r", repo, linked), starting];
            daemon.worktrees.borrow_mut().cleanup.insert(linked.to_owned());
            assert_eq!(
                daemon
                    .worktree_blockers(&busy, true, &client, Some("feature"), false)
                    .await,
                [
                    "worktree is locked (git worktree unlock it first)",
                    "worktree is already being deleted",
                    "Worktree branch changed; refresh before deletion",
                    "refusing to delete protected branch main",
                    "session \"Starter\" is starting; wait for it or cancel it",
                ]
            );
            // The deletion itself does not block on its own mark.
            let deleting = daemon.worktree_blockers(&busy, true, &client, None, true).await;
            assert!(!deleting.contains(&"worktree is already being deleted".to_owned()));
            let detached = target(repo, linked, Some(info(linked, "", false)));
            assert_eq!(
                daemon.worktree_blockers(&detached, true, &client, None, true).await,
                ["worktree is not on a local branch"]
            );
            // The default branch is protected too.
            git(Path::new(repo), &["branch", "trunk"]);
            git(Path::new(repo), &["update-ref", "refs/remotes/origin/trunk", "HEAD"]);
            git(
                Path::new(repo),
                &["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/trunk"],
            );
            let trunk = target(repo, linked, Some(info(linked, "trunk", false)));
            assert_eq!(
                daemon.worktree_blockers(&trunk, true, &client, None, true).await,
                ["refusing to delete protected branch trunk"]
            );
            assert!(
                daemon
                    .worktree_blockers(&trunk, false, &client, None, true)
                    .await
                    .is_empty()
            );
        });
    }

    #[test]
    fn worktree_paths_must_be_absolute() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon(dir.path());
        let (client, _rx) = test_client(1);
        local(async {
            assert_eq!(
                daemon.worktree_target("/repo", "wt").await.unwrap_err().to_string(),
                "Invalid worktree path"
            );
            assert_eq!(
                daemon
                    .inspect_worktree("/repo", "", false, &client, None, false)
                    .await
                    .unwrap_err()
                    .to_string(),
                "Invalid worktree path"
            );
            let message = DeleteWorktree {
                request_id: "r".to_owned(),
                cwd: "/repo".to_owned(),
                path: "relative".to_owned(),
                branch: None,
                delete_branch: Some(true),
                stop_sessions: None,
                allow_data_loss: Some(true),
                send_open_items: None,
            };
            assert_eq!(
                daemon.delete_worktree(&message, &client).await.unwrap_err().to_string(),
                "Invalid worktree path"
            );
            assert!(daemon.worktrees.borrow().cleanup.is_empty());
        });
    }

    #[test]
    fn updating_a_record_without_broadcast_patches_and_persists_it() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let daemon = daemon(dir.path());
        daemon
            .worktrees
            .borrow_mut()
            .records
            .insert("w1".to_owned(), record("w1", "/repo/wt"));
        local(async {
            daemon
                .update_worktree_record(
                    "w1",
                    |record| {
                        record.touch("baseSha");
                        record.base_sha = Some("abc".to_owned());
                    },
                    false,
                )
                .await
                .unwrap();
            // Unknown records are left alone.
            daemon
                .update_worktree_record("w2", |_| panic!("not called"), false)
                .await
                .unwrap();
        });
        let held = daemon.worktrees.borrow().records.get("w1").cloned().unwrap();
        assert_eq!(held.base_sha.as_deref(), Some("abc"));
        let text = std::fs::read_to_string(dir.path().join("state.json")).unwrap();
        let state: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            state["worktrees"],
            json!([{"id": "w1", "path": "/repo/wt", "createdAt": "2026-01-01T00:00:00.000Z", "baseSha": "abc"}])
        );
    }

    #[test]
    fn stopping_for_deletion_waits_until_the_session_has_exited() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon(dir.path());
        local(async {
            // Gone or exited already.
            daemon.stop_for_deletion("missing").await.unwrap();
            let mut exited = session("e", "/repo", "/repo");
            exited.status = SessionStatus::Exited;
            insert(&daemon, exited);
            daemon.stop_for_deletion("e").await.unwrap();

            // Running without a worker: the kill says it is not running, and the wait sees it exit.
            insert(&daemon, session("r", "/repo", "/repo"));
            let later = daemon.clone();
            let exiting = deckhand_sys::async_util::spawn(async move {
                tokio::time::sleep(Duration::from_millis(250)).await;
                let mut current = later.get_session("r").unwrap();
                current.status = SessionStatus::Exited;
                insert(&later, current);
            });
            let started = Instant::now();
            daemon.stop_for_deletion("r").await.unwrap();
            assert!(started.elapsed() >= Duration::from_millis(250));
            exiting.await;
        });
    }
}
