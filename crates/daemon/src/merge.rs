//! The daemon's merge flow.
//!
//! Merging a session's linked worktree into a target checkout (merge or squash, optionally committing first), keeping
//! or aborting a conflicted merge Deckhand started, marking a worktree merged by hand, and detecting merges done
//! outside Deckhand (the branch tip now in the default branch, or its PR merged). Merge markers live on the worktree
//! record, so every session of the worktree changes together.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use deckhand_core::error::{DhError, Result};
use deckhand_core::jsstr::{date, js_trim, path};
use deckhand_core::worktree_records::MERGE_MARKERS;
use deckhand_protocol::Ordered;
use deckhand_protocol::data::{
    MergeDetected, MergePreview, ResolveMergeAction, SessionRecord, SessionWorktreeMode, WorkspaceSummary,
    WorktreeMarkers, WorktreeMergeMode, WorktreeMergeResult,
};
use deckhand_protocol::records::WorktreeRecord;
use deckhand_sys::async_util::{Interval, SharedTask, spawn};
use deckhand_sys::git::{
    BranchWhere, ConflictedMerge, MergeOptions, MergeOutcome, abort_merge, branch_creation_commit, current_branch,
    find_repo_root, head_sha, list_worktrees, merge_worktree_into_current, optional_git, resolve_default_branch,
};
use deckhand_sys::workspace_git::{AncestryCheck, get_merge_preview, merged_into_default};
use futures::FutureExt;
use indexmap::IndexMap;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

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

/// A JS-truthy optional string (set and not empty).
fn truthy(value: Option<&str>) -> bool {
    value.is_some_and(|text| !text.is_empty())
}

/// `new Date().toISOString()`.
fn now_iso() -> String {
    date::to_iso_string(date::now_ms()).unwrap_or_default()
}

/// The JSON keys of MERGE_MARKERS.
fn merge_marker_keys() -> impl Iterator<Item = &'static str> {
    MERGE_MARKERS.iter().map(|marker| marker.as_str())
}

/// `value` rebuilt the way a JS spread edits an object: `edit` works on its keys in their current order (a removed
/// key that is set again goes last; an existing key set again keeps its place).
fn edit_object<T: Serialize + DeserializeOwned>(
    value: &Ordered<T>,
    edit: impl FnOnce(&mut Map<String, Value>) -> Result<()>,
) -> Result<Ordered<T>> {
    let mut json = value.to_value()?;
    if let Value::Object(map) = &mut json {
        edit(map)?;
    }
    Ok(serde_json::from_value(json)?)
}

/// `{...object, ...markers}` (only the markers that are set: undefined values are not written).
fn spread_markers(object: &mut Map<String, Value>, markers: &WorktreeMarkers) -> Result<()> {
    if let Value::Object(set) = serde_json::to_value(markers)? {
        for (key, value) in set {
            object.insert(key, value);
        }
    }
    Ok(())
}

/// TS saveMergeMarkers on a worktree record: `{...withoutMarkers(record, MERGE_MARKERS) minus mergeDismissedTip,
/// ...markers, ...!markers && dismissedTip ? {mergeDismissedTip} : {}}`.
fn record_with_markers(
    record: &Ordered<WorktreeRecord>,
    markers: Option<&WorktreeMarkers>,
    dismissed_tip: Option<&str>,
) -> Result<Ordered<WorktreeRecord>> {
    edit_object(record, |object| {
        for key in merge_marker_keys().chain(["mergeDismissedTip"]) {
            object.shift_remove(key);
        }
        match markers {
            Some(markers) => spread_markers(object, markers)?,
            None => {
                if let Some(tip) = dismissed_tip.filter(|tip| !tip.is_empty()) {
                    object.insert("mergeDismissedTip".into(), Value::String(tip.to_owned()));
                }
            }
        }
        Ok(())
    })
}

/// TS saveMergeMarkers on a session without a worktree record: its top-level merge markers dropped, `updatedAt` set,
/// and the markers under `worktree` when it names a linked worktree, else top-level (the four session fields).
fn session_with_markers(
    session: &Ordered<SessionRecord>,
    markers: Option<&WorktreeMarkers>,
    now: &str,
) -> Result<Ordered<SessionRecord>> {
    let linked = session
        .worktree
        .as_ref()
        .is_some_and(|worktree| truthy(worktree.path.as_deref()) && worktree.mode != SessionWorktreeMode::None);
    edit_object(session, |object| {
        for key in merge_marker_keys() {
            object.shift_remove(key);
        }
        object.insert("updatedAt".into(), Value::String(now.to_owned()));
        if linked {
            if let Some(Value::Object(worktree)) = object.get_mut("worktree") {
                for key in merge_marker_keys() {
                    worktree.shift_remove(key);
                }
                if let Some(markers) = markers {
                    spread_markers(worktree, markers)?;
                }
            }
        } else if let Some(markers) = markers {
            // Object.assign(updated, {mergedAt, mergeTargetBranch, mergeSourceRef, mergeMarkedManually}).
            let fields = [
                ("mergedAt", markers.merged_at.clone().map(Value::String)),
                (
                    "mergeTargetBranch",
                    markers.merge_target_branch.clone().map(Value::String),
                ),
                ("mergeSourceRef", markers.merge_source_ref.clone().map(Value::String)),
                ("mergeMarkedManually", markers.merge_marked_manually.map(Value::Bool)),
            ];
            for (key, value) in fields {
                if let Some(value) = value {
                    object.insert(key.into(), value);
                }
            }
        }
        Ok(())
    })
}

/// TS markDetectedMerge's record: `{...rest (no mergeDismissedTip), mergedAt, mergeDetected, mergeTargetBranch?,
/// mergeSourceRef, mergeSourceSha?}`.
fn detected_record(
    record: &Ordered<WorktreeRecord>,
    kind: MergeDetected,
    found: &DetectedMerge,
    now: &str,
) -> Result<Ordered<WorktreeRecord>> {
    edit_object(record, |object| {
        object.shift_remove("mergeDismissedTip");
        object.insert("mergedAt".into(), Value::String(now.to_owned()));
        object.insert("mergeDetected".into(), Value::String(kind.as_str().to_owned()));
        if let Some(branch) = found.target_branch.as_deref().filter(|branch| !branch.is_empty()) {
            object.insert("mergeTargetBranch".into(), Value::String(branch.to_owned()));
        }
        object.insert("mergeSourceRef".into(), Value::String(found.source_ref.clone()));
        if let Some(sha) = found.source_sha.as_deref().filter(|sha| !sha.is_empty()) {
            object.insert("mergeSourceSha".into(), Value::String(sha.to_owned()));
        }
        Ok(())
    })
}

/// TS: daemon.ts defaultBranchRefs: `refs/heads/<default>`, plus `refs/remotes/origin/<default>` when fetched.
pub async fn default_branch_refs(repo_root: &str, default_branch: &str) -> Vec<String> {
    let mut refs = vec![format!("refs/heads/{default_branch}")];
    let remote = format!("refs/remotes/origin/{default_branch}");
    if optional_git(repo_root, &["show-ref", "--verify", "--quiet", &remote], None)
        .await
        .is_some()
    {
        refs.push(remote);
    }
    refs
}

/// `await currentBranch(root) || await headSha(root)`.
async fn branch_or_head(root: &str) -> Result<String> {
    let branch = current_branch(root).await?;
    if !branch.is_empty() {
        return Ok(branch);
    }
    Ok(head_sha(root).await?)
}

impl Daemon {
    /// TS: InkDaemon.mergeSource (`session does not exist`, `session does not have a worktree to merge`, `cannot merge
    /// session because its worktree was deleted`).
    pub fn merge_source(&self, session_id: &str) -> Result<MergeSource> {
        let Some(session) = self.get_session(session_id) else {
            return Err(DhError::msg("session does not exist"));
        };
        let worktree = session
            .worktree
            .as_ref()
            .filter(|worktree| truthy(worktree.path.as_deref()) && worktree.mode != SessionWorktreeMode::None);
        let Some(worktree) = worktree else {
            return Err(DhError::msg("session does not have a worktree to merge"));
        };
        if truthy(worktree.deleted_at.as_deref()) {
            return Err(DhError::msg("cannot merge session because its worktree was deleted"));
        }
        let worktree_path = worktree.path.clone().unwrap_or_default();
        Ok(MergeSource { session, worktree_path })
    }

    /// TS: InkDaemon.mergePreview.
    pub async fn merge_preview(&self, session_id: &str, target_cwd: &str) -> Result<MergePreview> {
        let MergeSource { worktree_path, .. } = self.merge_source(session_id)?;
        get_merge_preview(&worktree_path, target_cwd).await
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
        let MergeSource { session, worktree_path } = self.merge_source(session_id)?;
        let options = MergeOptions {
            commit_first: commit_first.then(|| {
                let title = js_trim(&session.title);
                if title.is_empty() { "Deckhand session" } else { title }.to_owned()
            }),
        };
        let MergeOutcome {
            mut result,
            target_root,
            target_head,
            index_clean,
        } = merge_worktree_into_current(&worktree_path, target_cwd, mode, &options).await?;
        let title = session.title.clone();
        if let Some(committed) = &result.committed {
            self.log(&format!(
                "committed {} file(s) in {title} before merging ({})",
                committed.files, committed.sha
            ))
            .await;
        }
        if result.skipped == Some(true) {
            let reason = result.reason.as_deref().unwrap_or("no new commits");
            self.log(&format!(
                "{mode} merge skipped for {title} ({}) into {}: {reason}",
                result.source_ref, result.target_branch
            ))
            .await;
        } else if result.conflicted == Some(true) {
            // Left in progress until the user keeps it (marked merged) or aborts it (resolve-merge).
            let pending = PendingMerge {
                merge: ConflictedMerge {
                    mode,
                    target_head,
                    index_clean,
                },
                session_id: session_id.to_owned(),
                source_ref: result.source_ref.clone(),
                source_sha: result.source_sha.clone(),
                target_branch: result.target_branch.clone(),
            };
            self.merge.borrow_mut().pending.insert(target_root, pending);
            self.log(&format!(
                "{mode} merge for {title} ({}) into {} has conflicts in {} file(s)",
                result.source_ref,
                result.target_branch,
                result.conflict_count.unwrap_or(0)
            ))
            .await;
        } else {
            let markers = WorktreeMarkers {
                merged_at: Some(now_iso()),
                merge_mode: Some(mode),
                merge_target_branch: Some(result.target_branch.clone()),
                merge_source_ref: Some(result.source_ref.clone()),
                merge_source_sha: result.source_sha.clone(),
                ..WorktreeMarkers::default()
            };
            self.save_merge_markers(session_id, Some(markers), None).await?;
            self.log(&format!(
                "{mode} merged {title} ({}) into {}",
                result.source_ref, result.target_branch
            ))
            .await;
            // Steps left open in its note go to the backlog (the confirmation offered it): the work they belonged to
            // is merged.
            if send_open_items {
                let held = self.require_session(session_id)?;
                let record = self.worktree_record_of(&held);
                let branch = session.worktree.as_ref().and_then(|worktree| worktree.branch.clone());
                let sent = match self
                    .send_worktree_items_to_backlog(record.as_deref(), &session.repo_root, branch.as_deref())
                    .await
                {
                    Ok(sent) => sent,
                    Err(error) => {
                        self.log(&format!("sending open note items of {title} failed: {error}"))
                            .await;
                        0
                    }
                };
                result.touch("sentItems");
                result.sent_items = Some(i64::try_from(sent).unwrap_or(i64::MAX));
            }
        }
        Ok(result)
    }

    /// TS: InkDaemon.resolveMerge: `keep` leaves the conflicted merge in progress and marks the worktree merged;
    /// `abort` undoes it.
    ///
    /// Abort is `git merge --abort`, or for a squash Deckhand started, `git reset --merge`.
    pub async fn resolve_merge(
        &self,
        session_id: &str,
        target_cwd: &str,
        action: ResolveMergeAction,
    ) -> Result<Ordered<SessionRecord>> {
        let target_root = path::resolve(&find_repo_root(Some(target_cwd)).await?);
        let pending = self.merge.borrow().pending.get(&target_root).cloned();
        let started = pending.as_ref().filter(|pending| pending.session_id == session_id);
        if action == ResolveMergeAction::Abort {
            abort_merge(&target_root, started.map(|started| &started.merge)).await?;
            if pending.is_some() {
                self.merge.borrow_mut().pending.remove(&target_root);
            }
            self.log(&format!("aborted the conflicted merge into {target_root}"))
                .await;
            return self.require_session(session_id);
        }
        let MergeSource { session, worktree_path } = self.merge_source(session_id)?;
        // Without a record of it (e.g. the daemon restarted), the source is described as it is now (no commit is
        // vouched for).
        let source_ref = match started {
            Some(started) => started.source_ref.clone(),
            None => branch_or_head(&worktree_path).await?,
        };
        let merged_at = now_iso();
        let target_branch = match started {
            Some(started) => started.target_branch.clone(),
            None => current_branch(&target_root).await?,
        };
        let markers = WorktreeMarkers {
            merged_at: Some(merged_at),
            merge_mode: started.map(|started| started.merge.mode),
            merge_target_branch: Some(target_branch),
            merge_source_ref: Some(source_ref.clone()),
            // Only the commit Deckhand merged counts as integrated; without its record, none does.
            merge_source_sha: started
                .and_then(|started| started.source_sha.clone())
                .filter(|sha| !sha.is_empty()),
            ..WorktreeMarkers::default()
        };
        let updated = self.save_merge_markers(session_id, Some(markers), None).await?;
        if started.is_some() {
            self.merge.borrow_mut().pending.remove(&target_root);
        }
        self.log(&format!(
            "kept the conflicted merge of {} ({source_ref}) into {target_root}",
            session.title
        ))
        .await;
        Ok(updated)
    }

    /// TS: InkDaemon.saveMergeMarkers: replaces the merge markers (`None`: clears them, remembering `dismissed_tip`)
    /// on the worktree record, else on the session.
    ///
    /// On the worktree record of a linked worktree every session of it changes; else on the session (under
    /// `worktree` for a linked worktree, else top-level; only ever cleared there: `M` is worktree-only).
    pub async fn save_merge_markers(
        &self,
        session_id: &str,
        markers: Option<WorktreeMarkers>,
        dismissed_tip: Option<String>,
    ) -> Result<Ordered<SessionRecord>> {
        let session = self.require_session(session_id)?;
        if let Some(record) = self.worktree_record_of(&session) {
            let updated = record_with_markers(&record, markers.as_ref(), dismissed_tip.as_deref())?;
            self.save_worktree_record(updated).await?;
            return self.require_session(session_id);
        }
        let updated = session_with_markers(&session, markers.as_ref(), &now_iso())?;
        self.save_session(updated).await
    }

    /// TS: InkDaemon.markSessionMerged: toggles the manual merged marker (`session does not exist`, `cannot mark
    /// session merged because its worktree was deleted`, `Not in a worktree, so there is nothing to mark merged. Use
    /// Space to mark it done`, `target worktree is detached; checkout a branch before marking merged`).
    pub async fn mark_session_merged(&self, session_id: &str, target_cwd: &str) -> Result<Ordered<SessionRecord>> {
        let Some(session) = self.get_session(session_id) else {
            return Err(DhError::msg("session does not exist"));
        };
        let worktree = session.worktree.as_ref();
        let worktree_path = worktree
            .filter(|worktree| worktree.mode != SessionWorktreeMode::None)
            .and_then(|worktree| worktree.path.clone());
        let deleted = worktree.is_some_and(|worktree| truthy(worktree.deleted_at.as_deref()));
        let merged = worktree.is_some_and(|worktree| truthy(worktree.merged_at.as_deref()));
        if merged || truthy(session.merged_at.as_deref()) {
            // A detected merge stays cleared until the branch moves on (merge detection would set it again otherwise).
            let root = worktree
                .and_then(|worktree| worktree.id.as_deref())
                .filter(|id| !id.is_empty())
                .and_then(|id| {
                    self.worktrees
                        .borrow()
                        .records
                        .get(id)
                        .map(|record| record.path.clone())
                })
                .filter(|root| !root.is_empty());
            let tip = match root {
                Some(root) if !deleted => head_sha(&root).await.ok(),
                _ => None,
            };
            let updated = self.save_merge_markers(session_id, None, tip).await?;
            self.log(&format!("unmarked {} as merged", session.title)).await;
            return Ok(updated);
        }
        if deleted {
            return Err(DhError::msg(
                "cannot mark session merged because its worktree was deleted",
            ));
        }
        // Merged describes a worktree; a session in the main checkout has nothing to merge, but can be done (D).
        if self.worktree_record_of(&session).is_none() {
            return Err(DhError::msg(
                "Not in a worktree, so there is nothing to mark merged. Use Space to mark it done",
            ));
        }
        let source_root = find_repo_root(Some(worktree_path.as_deref().unwrap_or(&session.cwd))).await?;
        let target_root = find_repo_root(Some(target_cwd)).await?;
        let source_ref = branch_or_head(&source_root).await?;
        let target_branch = current_branch(&target_root).await?;
        if target_branch.is_empty() {
            return Err(DhError::msg(
                "target worktree is detached; checkout a branch before marking merged",
            ));
        }
        // No mergeSourceSha: a manual marker is a claim Deckhand did not verify, so it never relaxes deletion safety.
        let markers = WorktreeMarkers {
            merged_at: Some(now_iso()),
            merge_target_branch: Some(target_branch.clone()),
            merge_source_ref: Some(source_ref.clone()),
            merge_marked_manually: Some(true),
            ..WorktreeMarkers::default()
        };
        let updated = self.save_merge_markers(session_id, Some(markers), None).await?;
        self.log(&format!(
            "manually marked {} ({source_ref}) merged into {target_branch}",
            session.title
        ))
        .await;
        Ok(updated)
    }

    /// TS: InkDaemon.detectMerges: one detection run at a time (a caller during a run shares it); failures are logged.
    ///
    /// Marks linked worktrees merged that were merged outside Deckhand: the branch tip is in the default branch (local
    /// or `origin/`, never fetched) and the branch has a commit of its own beyond where it started
    /// (mergedIntoDefault). Only live, unmarked records whose directory Git still lists; never unmarks.
    pub fn detect_merges(&self, only_ids: Option<HashSet<String>>) -> SharedTask<()> {
        let running = self.merge.borrow().detection.clone();
        if let Some(running) = running {
            return running;
        }
        let daemon = self.clone();
        let run = spawn(async move {
            if let Err(error) = daemon.run_merge_detection(only_ids).await {
                daemon.log(&format!("merge detection failed: {error}")).await;
            }
            daemon.merge.borrow_mut().detection = None;
        })
        .boxed_local()
        .shared();
        self.merge.borrow_mut().detection = Some(run.clone());
        run
    }

    /// TS: InkDaemon.runMergeDetection: live, unmarked records Git still lists whose tip is in the default branch
    /// beyond their base (bounded by MERGE_DETECT_MAX).
    pub async fn run_merge_detection(&self, only_ids: Option<HashSet<String>>) -> Result<()> {
        // Records grouped by repository (a session's repoRoot): one worktree listing and default-branch lookup each.
        let mut repos: IndexMap<String, Vec<Ordered<WorktreeRecord>>> = IndexMap::new();
        {
            let worktrees = self.worktrees.borrow();
            let sessions = self.sessions.borrow();
            let mut count = 0;
            for record in worktrees.records.values() {
                if truthy(record.deleted_at.as_deref())
                    || truthy(record.merged_at.as_deref())
                    || only_ids.as_ref().is_some_and(|ids| !ids.contains(&record.id))
                    || count >= MERGE_DETECT_MAX
                {
                    continue;
                }
                let owner = sessions.sessions.values().find(|session| {
                    session.worktree.as_ref().and_then(|worktree| worktree.id.as_deref()) == Some(record.id.as_str())
                });
                let Some(owner) = owner else { continue };
                if worktrees.cleanup.contains(&path::resolve(&record.path)) {
                    continue;
                }
                repos.entry(owner.repo_root.clone()).or_default().push(record.clone());
                count += 1;
            }
        }
        for (repo_root, records) in repos {
            if self.lifecycle.borrow().shutting_down {
                return Ok(());
            }
            let Ok(listed) = list_worktrees(&repo_root).await else {
                continue;
            };
            let Some(default_branch) = resolve_default_branch(&repo_root, None, BranchWhere::Local)
                .await
                .filter(|branch| !branch.is_empty())
            else {
                continue;
            };
            let defaults = default_branch_refs(&repo_root, &default_branch).await;
            for record in records {
                let key = path::resolve(&record.path);
                let Some(entry) = listed.iter().find(|item| path::resolve(&item.path) == key) else {
                    continue;
                };
                // Missing worktrees, the default branch itself and dismissed tips are left alone.
                if entry.is_main
                    || entry.branch == default_branch
                    || entry.head.is_empty()
                    || record.merge_dismissed_tip.as_deref() == Some(entry.head.as_str())
                {
                    continue;
                }
                if tokio::fs::metadata(&record.path).await.is_err() {
                    continue;
                }
                let mut base_sha = record.base_sha.clone().filter(|sha| !sha.is_empty());
                let unknown = self.merge.borrow().no_base_sha.contains(&record.id);
                if base_sha.is_none() && !unknown {
                    // Records from before baseSha: the branch's creation in its reflog, once.
                    base_sha = branch_creation_commit(&record.path, &entry.branch)
                        .await
                        .filter(|sha| !sha.is_empty());
                    match &base_sha {
                        Some(sha) => {
                            let sha = sha.clone();
                            self.update_worktree_record(
                                &record.id,
                                move |record| {
                                    record.touch("baseSha");
                                    record.base_sha = Some(sha);
                                },
                                false,
                            )
                            .await?;
                        }
                        None => {
                            self.merge.borrow_mut().no_base_sha.insert(record.id.clone());
                        }
                    }
                }
                let Some(base_sha) = base_sha else { continue };
                let check = AncestryCheck {
                    cwd: record.path.clone(),
                    tip: entry.head.clone(),
                    branch: entry.branch.clone(),
                    start: base_sha,
                    defaults: defaults.clone(),
                };
                if !merged_into_default(&check).await {
                    continue;
                }
                let source_ref = if entry.branch.is_empty() {
                    entry.head.clone()
                } else {
                    entry.branch.clone()
                };
                let found = DetectedMerge {
                    target_branch: Some(default_branch.clone()),
                    source_ref,
                    source_sha: Some(entry.head.clone()),
                    tip: entry.head.clone(),
                };
                self.mark_detected_merge(&record.id, MergeDetected::Ancestor, found)
                    .await?;
            }
        }
        Ok(())
    }

    /// TS: InkDaemon.detectMergeFromSummary: a merged PR marks the worktree merged; otherwise ancestry is checked.
    ///
    /// The PR counts even when no commit of the branch is in the default branch (GitHub squash merges).
    pub async fn detect_merge_from_summary(&self, session_id: &str, summary: &WorkspaceSummary) -> Result<()> {
        let Some(session) = self.get_session(session_id) else {
            return Ok(());
        };
        let Some(record) = self.worktree_record_of(&session) else {
            return Ok(());
        };
        if truthy(record.deleted_at.as_deref()) || truthy(record.merged_at.as_deref()) {
            return Ok(());
        }
        if let Some(pr) = summary.pr.as_ref().filter(|pr| pr.state == "MERGED")
            && record.merge_dismissed_tip.as_deref() != Some(summary.head.as_str())
        {
            let mut source_sha = None;
            if let Some(head) = pr.head_sha.as_deref().filter(|head| !head.is_empty()) {
                let commit = format!("{head}^{{commit}}");
                let known = optional_git(&record.path, &["rev-parse", "--verify", "--quiet", &commit], None).await;
                if known.is_some_and(|sha| !sha.is_empty()) {
                    source_sha = Some(head.to_owned());
                }
            }
            let found = DetectedMerge {
                target_branch: pr.base_branch.clone(),
                source_ref: summary.branch.clone(),
                source_sha,
                tip: summary.head.clone(),
            };
            return self.mark_detected_merge(&record.id, MergeDetected::Pr, found).await;
        }
        self.detect_merges(Some(HashSet::from([record.id.clone()]))).await;
        Ok(())
    }

    /// TS: InkDaemon.markDetectedMerge: marks the record merged (unless marked, dismissed or deleted meanwhile).
    pub async fn mark_detected_merge(&self, record_id: &str, kind: MergeDetected, found: DetectedMerge) -> Result<()> {
        // Re-read after the Git calls: it may have been marked, unmarked (dismissed) or deleted meanwhile.
        let record = self.worktrees.borrow().records.get(record_id).cloned();
        let Some(record) = record else { return Ok(()) };
        if truthy(record.deleted_at.as_deref())
            || truthy(record.merged_at.as_deref())
            || record.merge_dismissed_tip.as_deref() == Some(found.tip.as_str())
        {
            return Ok(());
        }
        let updated = detected_record(&record, kind, &found, &now_iso())?;
        self.save_worktree_record(updated).await?;
        let into = match kind {
            MergeDetected::Pr => " (PR merged)".to_owned(),
            MergeDetected::Ancestor => format!(" into {}", found.target_branch.as_deref().unwrap_or("undefined")),
        };
        self.log(&format!(
            "detected that worktree {} ({}) was merged{into}",
            record.path, found.source_ref
        ))
        .await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::{local, session, test_daemon};
    use serde_json::json;
    use std::path::{Path, PathBuf};
    use std::sync::MutexGuard;

    /// DECKHAND_HOME = `dir` while this lives (persist writes state.json there).
    struct Home {
        _guard: MutexGuard<'static, ()>,
        previous: Option<std::ffi::OsString>,
    }

    impl Home {
        fn set(dir: &Path) -> Home {
            let guard = crate::server::tests::env_lock();
            let previous = std::env::var_os(deckhand_sys::paths::HOME_ENV);
            // SAFETY: tests touching the environment hold server::tests::env_lock.
            unsafe { std::env::set_var(deckhand_sys::paths::HOME_ENV, dir) };
            Home {
                _guard: guard,
                previous,
            }
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            // SAFETY: as in Home::set (the environment lock is still held).
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

    fn git(cwd: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(["-c", "user.name=Deckhand Test", "-c", "user.email=test@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn commit(cwd: &Path, file: &str, text: &str, message: &str) {
        std::fs::write(cwd.join(file), text).unwrap();
        git(cwd, &["add", file]);
        git(cwd, &["commit", "-q", "-m", message]);
    }

    /// A temp dir (canonical) with a repository `repo` (one commit on main) and a linked worktree `wt` on `feature`.
    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        repo: String,
        linked: String,
    }

    impl Fixture {
        fn repo(&self) -> &Path {
            Path::new(&self.repo)
        }

        fn linked(&self) -> &Path {
            Path::new(&self.linked)
        }
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        // The daemon's own git runs (merges) need an identity too; on a host without a domain git cannot invent one.
        git(&repo, &["config", "user.name", "Deckhand Test"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        commit(&repo, "README.md", "hi\n", "init");
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

    /// A session in `cwd` of `repo` with the given `worktree`.
    fn in_worktree(id: &str, repo: &str, cwd: &str, worktree: Value) -> Ordered<SessionRecord> {
        let mut value = session(id, repo, cwd).to_value().unwrap();
        value["worktree"] = worktree;
        serde_json::from_value(value).unwrap()
    }

    fn insert(daemon: &Daemon, session: Ordered<SessionRecord>) {
        daemon
            .sessions
            .borrow_mut()
            .sessions
            .insert(session.id.clone(), session);
    }

    fn record(value: Value) -> Ordered<WorktreeRecord> {
        serde_json::from_value(value).unwrap()
    }

    fn insert_record(daemon: &Daemon, record: Ordered<WorktreeRecord>) {
        daemon.worktrees.borrow_mut().records.insert(record.id.clone(), record);
    }

    fn keys(value: &Value) -> Vec<String> {
        value.as_object().unwrap().keys().cloned().collect()
    }

    fn log_text(dir: &Path) -> String {
        std::fs::read_to_string(dir.join("daemon.log")).unwrap_or_default()
    }

    #[test]
    fn default_branch_refs_add_origin_only_when_fetched() {
        let fx = fixture();
        local(async {
            assert_eq!(
                default_branch_refs(&fx.repo, "main").await,
                vec!["refs/heads/main".to_owned()]
            );
            let head = git(fx.repo(), &["rev-parse", "HEAD"]);
            git(fx.repo(), &["update-ref", "refs/remotes/origin/main", &head]);
            assert_eq!(
                default_branch_refs(&fx.repo, "main").await,
                vec!["refs/heads/main".to_owned(), "refs/remotes/origin/main".to_owned()]
            );
        });
    }

    #[test]
    fn merge_source_explains_why_there_is_nothing_to_merge() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon(dir.path());
        let message = |id: &str| daemon.merge_source(id).unwrap_err().to_string();
        assert_eq!(message("missing"), "session does not exist");
        insert(&daemon, session("main", "/r", "/r"));
        assert_eq!(message("main"), "session does not have a worktree to merge");
        insert(
            &daemon,
            in_worktree("none", "/r", "/r", json!({"mode": "none", "path": "/r/wt"})),
        );
        assert_eq!(message("none"), "session does not have a worktree to merge");
        insert(
            &daemon,
            in_worktree("empty", "/r", "/r", json!({"mode": "managed", "path": ""})),
        );
        assert_eq!(message("empty"), "session does not have a worktree to merge");
        insert(
            &daemon,
            in_worktree(
                "gone",
                "/r",
                "/r/wt",
                json!({"mode": "managed", "path": "/r/wt", "deletedAt": "2026-01-02T00:00:00.000Z"}),
            ),
        );
        assert_eq!(message("gone"), "cannot merge session because its worktree was deleted");
        insert(
            &daemon,
            in_worktree("live", "/r", "/r/wt", json!({"mode": "attached", "path": "/r/wt"})),
        );
        let source = daemon.merge_source("live").unwrap();
        assert_eq!(source.worktree_path, "/r/wt");
        assert_eq!(source.session.id, "live");
    }

    #[test]
    fn merge_preview_compares_the_worktree_with_the_target() {
        let fx = fixture();
        commit(fx.linked(), "feature.txt", "new\n", "feature work");
        let daemon = daemon(&fx.root);
        insert(
            &daemon,
            in_worktree(
                "s1",
                &fx.repo,
                &fx.linked,
                json!({"mode": "managed", "path": fx.linked}),
            ),
        );
        local(async {
            let preview = daemon.merge_preview("s1", &fx.repo).await.unwrap();
            assert_eq!(preview.source_ref, "feature");
            assert_eq!(preview.target_branch.as_deref(), Some("main"));
            assert_eq!(preview.commit_count, 1);
            assert_eq!(preview.commits.len(), 1);
            assert_eq!(
                daemon.merge_preview("missing", &fx.repo).await.unwrap_err().to_string(),
                "session does not exist"
            );
        });
    }

    #[test]
    fn a_merge_with_nothing_new_is_skipped_and_marks_nothing() {
        let fx = fixture();
        let daemon = daemon(&fx.root);
        insert_record(
            &daemon,
            record(json!({"id": "w1", "path": fx.linked, "createdAt": "2026-01-01T00:00:00.000Z"})),
        );
        insert(
            &daemon,
            in_worktree(
                "s1",
                &fx.repo,
                &fx.linked,
                json!({"mode": "managed", "path": fx.linked, "id": "w1"}),
            ),
        );
        local(async {
            let result = daemon
                .merge_session_worktree("s1", WorktreeMergeMode::Squash, &fx.repo, false, true)
                .await
                .unwrap();
            assert_eq!(result.skipped, Some(true));
            assert_eq!(result.sent_items, None);
            let value = result.to_value().unwrap();
            assert_eq!(
                keys(&value),
                [
                    "mode",
                    "sourceRef",
                    "sourceSha",
                    "targetBranch",
                    "skipped",
                    "reason",
                    "stdout",
                    "stderr"
                ]
            );
        });
        assert!(daemon.worktrees.borrow().records.get("w1").unwrap().merged_at.is_none());
        assert!(daemon.merge.borrow().pending.is_empty());
        assert!(
            log_text(&fx.root).contains("squash merge skipped for s1 (feature) into main: No new commits to merge"),
            "{}",
            log_text(&fx.root)
        );
    }

    #[test]
    fn a_conflicted_merge_stays_pending_until_it_is_aborted() {
        let fx = fixture();
        commit(fx.linked(), "README.md", "feature\n", "feature edit");
        commit(fx.repo(), "README.md", "main\n", "main edit");
        let daemon = daemon(&fx.root);
        insert_record(
            &daemon,
            record(json!({"id": "w1", "path": fx.linked, "createdAt": "2026-01-01T00:00:00.000Z"})),
        );
        insert(
            &daemon,
            in_worktree(
                "s1",
                &fx.repo,
                &fx.linked,
                json!({"mode": "managed", "path": fx.linked, "id": "w1"}),
            ),
        );
        local(async {
            let result = daemon
                .merge_session_worktree("s1", WorktreeMergeMode::Merge, &fx.repo, false, false)
                .await
                .unwrap();
            assert_eq!(result.conflicted, Some(true));
            assert_eq!(result.conflicts.as_deref(), Some(&["README.md".to_owned()][..]));
            let pending = daemon.merge.borrow().pending.get(&fx.repo).cloned().unwrap();
            assert_eq!(pending.session_id, "s1");
            assert_eq!(pending.source_ref, "feature");
            assert_eq!(pending.target_branch, "main");
            assert_eq!(pending.merge.mode, WorktreeMergeMode::Merge);
            assert_eq!(
                pending.source_sha.as_deref(),
                Some(git(fx.linked(), &["rev-parse", "HEAD"]).as_str())
            );
            assert_eq!(pending.merge.index_clean, Some(true));
            // Not merged yet: nothing is marked until the user keeps it.
            assert!(daemon.worktrees.borrow().records.get("w1").unwrap().merged_at.is_none());

            let held = daemon
                .resolve_merge("s1", &fx.repo, ResolveMergeAction::Abort)
                .await
                .unwrap();
            assert_eq!(held.id, "s1");
            assert!(daemon.merge.borrow().pending.is_empty());
            let merge_head = git(
                fx.repo(),
                &["rev-parse", "--path-format=absolute", "--git-path", "MERGE_HEAD"],
            );
            assert!(!Path::new(&merge_head).exists());
            assert_eq!(git(fx.repo(), &["status", "--porcelain"]), "");
            // Nothing in progress any more: a second abort is refused by Git's rules, and an unknown session too.
            assert!(
                daemon
                    .resolve_merge("s1", &fx.repo, ResolveMergeAction::Abort)
                    .await
                    .is_err()
            );
        });
        let log = log_text(&fx.root);
        assert!(
            log.contains("merge merge for s1 (feature) into main has conflicts in 1 file(s)"),
            "{log}"
        );
        assert!(
            log.contains(&format!("aborted the conflicted merge into {}", fx.repo)),
            "{log}"
        );
    }

    #[test]
    fn marking_merged_explains_what_cannot_be_marked() {
        let fx = fixture();
        let daemon = daemon(&fx.root);
        local(async {
            let message = |id: &'static str| {
                let daemon = daemon.clone();
                let repo = fx.repo.clone();
                async move { daemon.mark_session_merged(id, &repo).await.unwrap_err().to_string() }
            };
            assert_eq!(message("missing").await, "session does not exist");
            insert(
                &daemon,
                in_worktree(
                    "gone",
                    &fx.repo,
                    &fx.linked,
                    json!({"mode": "managed", "path": fx.linked, "id": "w0", "deletedAt": "2026-01-02T00:00:00.000Z"}),
                ),
            );
            assert_eq!(
                message("gone").await,
                "cannot mark session merged because its worktree was deleted"
            );
            insert(&daemon, session("main", &fx.repo, &fx.repo));
            assert_eq!(
                message("main").await,
                "Not in a worktree, so there is nothing to mark merged. Use Space to mark it done"
            );
            insert_record(
                &daemon,
                record(json!({"id": "w1", "path": fx.linked, "createdAt": "2026-01-01T00:00:00.000Z"})),
            );
            insert(
                &daemon,
                in_worktree(
                    "s1",
                    &fx.repo,
                    &fx.linked,
                    json!({"mode": "managed", "path": fx.linked, "id": "w1"}),
                ),
            );
            git(fx.repo(), &["checkout", "-q", "--detach"]);
            assert_eq!(
                message("s1").await,
                "target worktree is detached; checkout a branch before marking merged"
            );
        });
    }

    #[test]
    fn merge_markers_replace_the_record_s_markers_in_spread_order() {
        let base = record(json!({
            "id": "w1", "path": "/r/wt", "createdAt": "2026-01-01T00:00:00.000Z",
            "mergedAt": "2026-01-02T00:00:00.000Z", "mergeDetected": "ancestor", "deletedAt": "2026-01-04T00:00:00.000Z",
            "mergeDismissedTip": "old", "baseSha": "b0", "future": 1
        }));
        let markers = WorktreeMarkers {
            merged_at: Some("2026-01-03T00:00:00.000Z".into()),
            merge_mode: Some(WorktreeMergeMode::Squash),
            merge_target_branch: Some("main".into()),
            merge_source_ref: Some("feature".into()),
            merge_source_sha: None,
            ..WorktreeMarkers::default()
        };
        let marked = record_with_markers(&base, Some(&markers), Some("ignored")).unwrap();
        let value = marked.to_value().unwrap();
        assert_eq!(
            keys(&value),
            [
                "id",
                "path",
                "createdAt",
                "deletedAt",
                "baseSha",
                "future",
                "mergedAt",
                "mergeMode",
                "mergeTargetBranch",
                "mergeSourceRef"
            ]
        );
        assert_eq!(value["mergeMode"], "squash");
        assert_eq!(marked.merge_detected, None);
        assert_eq!(marked.merge_dismissed_tip, None);

        let cleared = record_with_markers(&marked, None, Some("tip")).unwrap();
        let value = cleared.to_value().unwrap();
        assert_eq!(
            keys(&value),
            [
                "id",
                "path",
                "createdAt",
                "deletedAt",
                "baseSha",
                "future",
                "mergeDismissedTip"
            ]
        );
        assert_eq!(value["mergeDismissedTip"], "tip");
        // No tip (or an empty one): nothing is remembered.
        let cleared = record_with_markers(&marked, None, Some("")).unwrap();
        assert_eq!(cleared.merge_dismissed_tip, None);
        assert_eq!(record_with_markers(&marked, None, None).unwrap().merged_at, None);
    }

    #[test]
    fn session_markers_land_under_a_linked_worktree_else_top_level() {
        let markers = WorktreeMarkers {
            merged_at: Some("2026-01-03T00:00:00.000Z".into()),
            merge_target_branch: Some("main".into()),
            merge_source_ref: Some("feature".into()),
            merge_marked_manually: Some(true),
            ..WorktreeMarkers::default()
        };
        let mut linked = in_worktree(
            "s1",
            "/r",
            "/r/wt",
            json!({"mode": "managed", "mergedAt": "x", "path": "/r/wt", "mergeMode": "merge", "branch": "b"}),
        )
        .to_value()
        .unwrap();
        linked["mergedAt"] = json!("old");
        let linked: Ordered<SessionRecord> = serde_json::from_value(linked).unwrap();
        let updated = session_with_markers(&linked, Some(&markers), "2026-01-05T00:00:00.000Z").unwrap();
        let value = updated.to_value().unwrap();
        assert!(value.get("mergedAt").is_none());
        assert_eq!(value["updatedAt"], "2026-01-05T00:00:00.000Z");
        assert_eq!(
            keys(&value["worktree"]),
            [
                "mode",
                "path",
                "branch",
                "mergedAt",
                "mergeTargetBranch",
                "mergeSourceRef",
                "mergeMarkedManually"
            ]
        );
        let cleared = session_with_markers(&updated, None, "2026-01-06T00:00:00.000Z").unwrap();
        assert_eq!(
            keys(&cleared.to_value().unwrap()["worktree"]),
            ["mode", "path", "branch"]
        );

        let top = session("s2", "/r", "/r");
        let updated = session_with_markers(&top, Some(&markers), "2026-01-05T00:00:00.000Z").unwrap();
        let value = updated.to_value().unwrap();
        let names = keys(&value);
        assert_eq!(
            names[names.len() - 4..],
            ["mergedAt", "mergeTargetBranch", "mergeSourceRef", "mergeMarkedManually"]
        );
        assert_eq!(updated.merge_marked_manually, Some(true));
        let cleared = session_with_markers(&updated, None, "2026-01-06T00:00:00.000Z").unwrap();
        assert_eq!(cleared.merged_at, None);
        assert_eq!(cleared.merge_source_ref, None);
        assert_eq!(cleared.updated_at, "2026-01-06T00:00:00.000Z");
    }

    #[test]
    fn detected_merges_are_spread_onto_the_record() {
        let base = record(json!({
            "id": "w1", "path": "/r/wt", "mergeDismissedTip": "old", "createdAt": "2026-01-01T00:00:00.000Z"
        }));
        let found = DetectedMerge {
            target_branch: Some("main".into()),
            source_ref: "feature".into(),
            source_sha: Some("abc".into()),
            tip: "abc".into(),
        };
        let marked = detected_record(&base, MergeDetected::Ancestor, &found, "2026-01-02T00:00:00.000Z").unwrap();
        let value = marked.to_value().unwrap();
        assert_eq!(
            keys(&value),
            [
                "id",
                "path",
                "createdAt",
                "mergedAt",
                "mergeDetected",
                "mergeTargetBranch",
                "mergeSourceRef",
                "mergeSourceSha"
            ]
        );
        assert_eq!(value["mergeDetected"], "ancestor");
        let pr = DetectedMerge {
            target_branch: None,
            source_ref: "feature".into(),
            source_sha: None,
            tip: "abc".into(),
        };
        let marked = detected_record(&base, MergeDetected::Pr, &pr, "2026-01-02T00:00:00.000Z").unwrap();
        assert_eq!(
            keys(&marked.to_value().unwrap()),
            ["id", "path", "createdAt", "mergedAt", "mergeDetected", "mergeSourceRef"]
        );
    }

    #[test]
    fn detection_learns_a_missing_base_once_and_leaves_unmerged_branches_alone() {
        let fx = fixture();
        let _home = Home::set(&fx.root);
        commit(fx.linked(), "feature.txt", "new\n", "feature work");
        // A detached worktree has no branch reflog to learn its base from.
        let detached = fx.root.join("detached");
        git(
            fx.repo(),
            &["worktree", "add", "-q", "--detach", detached.to_str().unwrap()],
        );
        let detached = detached.to_string_lossy().into_owned();
        let daemon = daemon(&fx.root);
        insert_record(
            &daemon,
            record(json!({"id": "w1", "path": fx.linked, "createdAt": "2026-01-01T00:00:00.000Z"})),
        );
        insert_record(
            &daemon,
            record(json!({"id": "w2", "path": detached, "createdAt": "2026-01-01T00:00:00.000Z"})),
        );
        // No session: never checked.
        insert_record(
            &daemon,
            record(json!({"id": "w3", "path": fx.linked, "createdAt": "2026-01-01T00:00:00.000Z"})),
        );
        insert(
            &daemon,
            in_worktree(
                "s1",
                &fx.repo,
                &fx.linked,
                json!({"mode": "managed", "path": fx.linked, "id": "w1"}),
            ),
        );
        insert(
            &daemon,
            in_worktree(
                "s2",
                &fx.repo,
                &detached,
                json!({"mode": "attached", "path": detached, "id": "w2"}),
            ),
        );
        local(async {
            let first = daemon.detect_merges(None);
            // A caller during the run shares it.
            let second = daemon.detect_merges(Some(HashSet::from(["w1".to_owned()])));
            assert!(first.ptr_eq(&second));
            first.await;
            assert!(daemon.merge.borrow().detection.is_none());
        });
        let base = git(fx.repo(), &["rev-parse", "main"]);
        let records = daemon.worktrees.borrow().records.clone();
        assert_eq!(records.get("w1").unwrap().base_sha.as_deref(), Some(base.as_str()));
        assert!(records.get("w1").unwrap().merged_at.is_none());
        assert_eq!(records.get("w2").unwrap().base_sha, None);
        assert_eq!(records.get("w3").unwrap().base_sha, None);
        assert!(daemon.merge.borrow().no_base_sha.contains("w2"));
        let state: Value = serde_json::from_str(&std::fs::read_to_string(fx.root.join("state.json")).unwrap()).unwrap();
        let saved = state["worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == "w1")
            .unwrap();
        assert_eq!(keys(saved), ["id", "path", "createdAt", "baseSha"]);
        assert!(!log_text(&fx.root).contains("merge detection failed"));
    }

    #[test]
    fn detection_skips_records_it_must_not_touch() {
        let fx = fixture();
        let daemon = daemon(&fx.root);
        insert_record(
            &daemon,
            record(
                json!({"id": "w1", "path": fx.linked, "createdAt": "2026-01-01T00:00:00.000Z", "mergedAt": "2026-01-02T00:00:00.000Z"}),
            ),
        );
        insert_record(
            &daemon,
            record(
                json!({"id": "w2", "path": fx.linked, "createdAt": "2026-01-01T00:00:00.000Z", "deletedAt": "2026-01-02T00:00:00.000Z"}),
            ),
        );
        insert_record(
            &daemon,
            record(json!({"id": "w3", "path": fx.linked, "createdAt": "2026-01-01T00:00:00.000Z"})),
        );
        for id in ["w1", "w2", "w3"] {
            insert(
                &daemon,
                in_worktree(
                    &format!("s-{id}"),
                    &fx.repo,
                    &fx.linked,
                    json!({"mode": "managed", "path": fx.linked, "id": id}),
                ),
            );
        }
        // Being deleted: left alone.
        daemon.worktrees.borrow_mut().cleanup.insert(fx.linked.clone());
        local(async {
            daemon.run_merge_detection(None).await.unwrap();
        });
        let records = daemon.worktrees.borrow().records.clone();
        assert!(records.values().all(|record| record.base_sha.is_none()));
        assert!(daemon.merge.borrow().no_base_sha.is_empty());
        // Only the requested records are checked.
        daemon.worktrees.borrow_mut().cleanup.clear();
        local(async {
            daemon
                .run_merge_detection(Some(HashSet::from(["other".to_owned()])))
                .await
                .unwrap();
        });
        assert!(daemon.worktrees.borrow().records.get("w3").unwrap().base_sha.is_none());
        // Shutting down: no Git work.
        daemon.lifecycle.borrow_mut().shutting_down = true;
        local(async {
            daemon.run_merge_detection(None).await.unwrap();
        });
        assert!(daemon.worktrees.borrow().records.get("w3").unwrap().base_sha.is_none());
    }

    #[test]
    fn summaries_and_detections_leave_marked_dismissed_and_unknown_records_alone() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon(dir.path());
        insert_record(
            &daemon,
            record(
                json!({"id": "w1", "path": "/r/wt", "createdAt": "2026-01-01T00:00:00.000Z", "mergedAt": "2026-01-02T00:00:00.000Z"}),
            ),
        );
        insert_record(
            &daemon,
            record(
                json!({"id": "w2", "path": "/r/wt2", "createdAt": "2026-01-01T00:00:00.000Z", "mergeDismissedTip": "tip"}),
            ),
        );
        insert(
            &daemon,
            in_worktree(
                "s1",
                "/r",
                "/r/wt",
                json!({"mode": "managed", "path": "/r/wt", "id": "w1"}),
            ),
        );
        let summary: WorkspaceSummary = serde_json::from_value(json!({
            "cwd": "/r/wt", "branch": "feature", "head": "tip", "changedFiles": 0, "untrackedFiles": 0,
            "additions": 0, "deletions": 0,
            "pr": {"number": 1, "url": "u", "state": "MERGED", "checks": "unknown"}
        }))
        .unwrap();
        let found = DetectedMerge {
            target_branch: Some("main".into()),
            source_ref: "feature".into(),
            source_sha: None,
            tip: "tip".into(),
        };
        local(async {
            daemon.detect_merge_from_summary("missing", &summary).await.unwrap();
            daemon.detect_merge_from_summary("s1", &summary).await.unwrap();
            daemon
                .mark_detected_merge("missing", MergeDetected::Pr, found.clone())
                .await
                .unwrap();
            daemon
                .mark_detected_merge("w1", MergeDetected::Pr, found.clone())
                .await
                .unwrap();
            daemon
                .mark_detected_merge("w2", MergeDetected::Ancestor, found.clone())
                .await
                .unwrap();
        });
        let records = daemon.worktrees.borrow().records.clone();
        assert_eq!(records.get("w1").unwrap().merge_detected, None);
        assert_eq!(records.get("w2").unwrap().merged_at, None);
        assert_eq!(log_text(dir.path()), "");
    }
}
