//! Stacked sub-sessions: a sub-session in a new worktree starts from its parent's worktree branch and integrates into
//! it, so a tree of sessions is also a tree of branches (docs/future/orchestrator.md, part 1).
//!
//! - At creation ([`Daemon::stack_target`]) the new worktree record names its target: the parent's worktree record
//!   and that worktree's branch (`targetWorktreeId`, `targetBranch`). Without one a worktree is flat, merged into
//!   whichever checkout the merge names, as before.
//! - When a target is merged or deleted, the worktrees targeting it move to its own target ([`retargeted`]), as
//!   stacked pull requests are retargeted.
//! - Merged detection looks for a stacked worktree's tip in its target branch first (merge.rs).
//! - A merge into a linked worktree waits while an agent works there ([`Daemon::refuse_merge_while_working`]).

use deckhand_core::error::{DhError, Result};
use deckhand_core::jsstr::path;
use deckhand_core::sidebar_model::agent_working;
use deckhand_core::worktree_records::WorktreeRecords;
use deckhand_protocol::Ordered;
use deckhand_protocol::data::{CreateSessionInput, WorktreeMode};
use deckhand_protocol::records::WorktreeRecord;
use deckhand_sys::git::{current_branch, find_repo_root, head_sha};

use crate::daemon::Daemon;
use crate::worktrees::is_linked_worktree_root;

/// Where a new stacked worktree integrates: its parent's worktree record and the branch checked out there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackTarget {
    pub worktree_id: String,
    pub branch: String,
}

/// A JS-truthy optional string (set and not empty).
fn truthy(value: Option<&str>) -> bool {
    value.is_some_and(|text| !text.is_empty())
}

/// Not merged and not deleted: work can still be integrated into it.
pub fn is_live_target(record: &WorktreeRecord) -> bool {
    !truthy(record.deleted_at.as_deref()) && !truthy(record.merged_at.as_deref())
}

/// The records to save because `record` was merged or deleted: every live record targeting it, moved to the next
/// live target up the chain (none: flat), with `retargetedFrom` the branch it targeted.
///
/// Idempotent (a record moved once no longer targets `record`), so it runs on every save of a merged or deleted record,
/// whichever way it got there, a superseded incarnation included. Unmarking a merge does not move them back: by then
/// they may have been merged into their new target, and the parent's branch already holds what was merged.
pub fn retargeted(records: &WorktreeRecords, record: &WorktreeRecord) -> Vec<Ordered<WorktreeRecord>> {
    if is_live_target(record) {
        return Vec::new();
    }
    let mut moved = Vec::new();
    for child in records.values() {
        if child.id == record.id
            || child.target_worktree_id.as_deref() != Some(record.id.as_str())
            || !is_live_target(child)
        {
            continue;
        }
        // Up the chain past targets that are themselves merged or deleted; a cycle (edited state) or the child itself
        // ends it flat.
        let mut seen = vec![child.id.clone(), record.id.clone()];
        let mut next = (record.target_worktree_id.clone(), record.target_branch.clone());
        while let Some(id) = next.0.clone() {
            if seen.contains(&id) {
                next = (None, None);
                break;
            }
            match records.get(&id) {
                Some(target) if !is_live_target(target) => {
                    seen.push(id);
                    next = (target.target_worktree_id.clone(), target.target_branch.clone());
                }
                // A target with no record left is no worktree to merge into.
                None => next = (None, None),
                Some(_) => break,
            }
        }
        let mut updated = child.clone();
        let from = updated.target_branch.clone();
        for key in ["targetWorktreeId", "targetBranch", "retargetedFrom"] {
            updated.touch(key);
        }
        updated.target_branch = next.1.filter(|_| next.0.is_some());
        updated.target_worktree_id = next.0;
        updated.retargeted_from = from;
        moved.push(updated);
    }
    moved
}

impl Daemon {
    /// The target a new session's worktree stacks on, or None (flat). Stacked: a sub-session (a forked one too) asking
    /// for a new worktree, unless `stacked: false`, whose parent runs in a linked worktree that is still live (its own,
    /// or the one it shares with its own parent) on a branch, and that asks for no other base than that branch. The
    /// new branch then starts from the parent's (overriding `worktree.branchFrom`), so its changes, cleanup and pull
    /// request compare against the parent's branch.
    ///
    /// A parent in the main checkout or without a worktree has nothing to stack on: there the parent's branch is the
    /// launch branch, so flat is the same thing.
    pub async fn stack_target(&self, parent_id: Option<&str>, input: &CreateSessionInput) -> Option<StackTarget> {
        if input.worktree_mode != Some(WorktreeMode::New) || input.stacked == Some(false) {
            return None;
        }
        let parent = parent_id
            .filter(|id| !id.is_empty())
            .and_then(|id| self.get_session(id))?;
        if parent
            .worktree
            .as_ref()
            .is_some_and(|worktree| worktree.is_main == Some(true))
        {
            return None;
        }
        let record = self
            .worktree_record_of(&parent)
            .filter(|record| is_live_target(record))?;
        let branch = current_branch(&record.path)
            .await
            .ok()
            .filter(|branch| !branch.is_empty())?;
        if let Some(base) = input.base_branch.as_deref().filter(|base| !base.is_empty())
            && base != branch
        {
            return None;
        }
        Some(StackTarget {
            worktree_id: record.id.clone(),
            branch,
        })
    }

    /// Saves the records that move up because `record` was merged or deleted ([`retargeted`]); each through
    /// save_worktree_record, so its sessions are re-projected and broadcast.
    pub(crate) async fn retarget_children(&self, record: &WorktreeRecord) -> Result<()> {
        let moved = retargeted(&self.worktrees.borrow().records, record);
        for child in moved {
            self.log(&format!(
                "worktree {} now targets {} (was {})",
                child.path,
                child.target_branch.as_deref().unwrap_or("its launch checkout"),
                child.retargeted_from.as_deref().unwrap_or("unknown"),
            ))
            .await;
            // Boxed: saving a record retargets again (a moved record is live, so it stops there).
            Box::pin(self.save_worktree_record(child)).await?;
        }
        Ok(())
    }

    /// Refuses a merge into a linked worktree while an agent of a running session there is working
    /// ([`agent_working`]): the merge would change files under it. `<session> is working in <branch>; merge when it is
    /// idle`. The main checkout is left as before (its merge warns about touched files instead), as is a target that is
    /// no repository (the merge reports that).
    pub async fn refuse_merge_while_working(&self, target_cwd: &str) -> Result<()> {
        let Ok(root) = find_repo_root(Some(target_cwd)).await else {
            return Ok(());
        };
        let root = path::resolve(&root);
        if !is_linked_worktree_root(&root).await {
            return Ok(());
        }
        let record_id = self.live_record(&root).map(|record| record.id.clone());
        let working = self.all_sessions().into_iter().find(|session| {
            agent_working(session)
                && (path::resolve(&session.cwd) == root
                    || record_id.as_deref().is_some_and(|id| {
                        session.worktree.as_ref().and_then(|worktree| worktree.id.as_deref()) == Some(id)
                    }))
        });
        let Some(session) = working else {
            return Ok(());
        };
        let branch = match current_branch(&root).await {
            Ok(branch) if !branch.is_empty() => branch,
            _ => head_sha(&root).await.unwrap_or(root),
        };
        Err(DhError::msg(format!(
            "{} is working in {branch}; merge when it is idle",
            self.session_path(&session)
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge::tests::{
        Fixture, Home, commit, daemon, fixture, git, in_worktree, insert, insert_record, keys, record,
    };
    use crate::server::tests::{local, session};
    use deckhand_protocol::data::SessionRecord;
    use deckhand_protocol::data::{BranchFrom, WorktreeMarkers, WorktreeMergeMode, WorktreeSettings};
    use deckhand_sys::git::create_worktree_for_session;
    use serde_json::{Value, json};
    use std::path::Path;

    const AT: &str = "2026-01-01T00:00:00.000Z";

    fn input(value: Value) -> CreateSessionInput {
        let mut fields = json!({
            "title": "child", "program": "claude", "cwd": "/r", "repoRoot": "/r", "cols": 80, "rows": 24,
            "worktreeMode": "new", "parentSessionId": "lead", "subSessionKind": "clean",
        });
        for (key, entry) in value.as_object().cloned().unwrap_or_default() {
            fields[key] = entry;
        }
        serde_json::from_value(fields).unwrap()
    }

    /// `fx`'s linked worktree (on `feature`) as the lead's worktree record `w1`, with the lead session in it.
    fn lead_in_feature(fx: &Fixture) -> Daemon {
        let daemon = daemon(&fx.root);
        insert_record(&daemon, record(json!({"id": "w1", "path": fx.linked, "createdAt": AT})));
        insert(
            &daemon,
            in_worktree(
                "lead",
                &fx.repo,
                &fx.linked,
                json!({"mode": "managed", "path": fx.linked, "branch": "feature", "id": "w1"}),
            ),
        );
        daemon
    }

    fn stacked_on(daemon: &Daemon, parent: &str, fields: Value) -> Option<StackTarget> {
        let given = input(fields);
        local(async { daemon.stack_target(Some(parent), &given).await })
    }

    fn on_feature() -> Option<StackTarget> {
        Some(StackTarget {
            worktree_id: "w1".into(),
            branch: "feature".into(),
        })
    }

    #[test]
    fn a_sub_session_in_a_new_worktree_stacks_on_its_parent_s_worktree_branch() {
        let fx = fixture();
        let daemon = lead_in_feature(&fx);
        assert_eq!(stacked_on(&daemon, "lead", json!({})), on_feature());
        // A fork in a new worktree, and a base naming the parent's branch, stack too.
        assert_eq!(
            stacked_on(&daemon, "lead", json!({"subSessionKind": "forked"})),
            on_feature()
        );
        assert_eq!(
            stacked_on(&daemon, "lead", json!({"baseBranch": "feature"})),
            on_feature()
        );
        // A parent sharing the lead's worktree (same) stacks its children on that worktree.
        insert(
            &daemon,
            in_worktree("helper", &fx.repo, &fx.linked, json!({"mode": "none", "id": "w1"})),
        );
        assert_eq!(stacked_on(&daemon, "helper", json!({})), on_feature());
    }

    #[test]
    fn a_sub_session_stays_flat_when_asked_or_when_there_is_nothing_to_stack_on() {
        let fx = fixture();
        let daemon = lead_in_feature(&fx);
        assert_eq!(stacked_on(&daemon, "lead", json!({"stacked": false})), None);
        assert_eq!(stacked_on(&daemon, "lead", json!({"baseBranch": "main"})), None);
        assert_eq!(stacked_on(&daemon, "lead", json!({"worktreeMode": "none"})), None);
        assert_eq!(stacked_on(&daemon, "lead", json!({"worktreeMode": "existing"})), None);
        let top_level = input(json!({}));
        assert_eq!(local(daemon.stack_target(None, &top_level)), None);
        // A parent in the main checkout, or without a worktree record.
        insert(&daemon, session("main", &fx.repo, &fx.repo));
        assert_eq!(stacked_on(&daemon, "main", json!({})), None);
        insert(
            &daemon,
            in_worktree(
                "unknown",
                &fx.repo,
                &fx.linked,
                json!({"mode": "managed", "path": fx.linked, "id": "gone"}),
            ),
        );
        assert_eq!(stacked_on(&daemon, "unknown", json!({})), None);
        // A parent's worktree that is merged or deleted takes nothing more.
        for marker in ["mergedAt", "deletedAt"] {
            let mut done = record(json!({"id": "w1", "path": fx.linked, "createdAt": AT}));
            done.touch(marker);
            let mut value = done.to_value().unwrap();
            value[marker] = json!(AT);
            insert_record(&daemon, record(value));
            assert_eq!(stacked_on(&daemon, "lead", json!({})), None, "{marker}");
        }
        // A detached parent has no branch to stack on.
        insert_record(&daemon, record(json!({"id": "w1", "path": fx.linked, "createdAt": AT})));
        git(fx.linked(), &["checkout", "-q", "--detach"]);
        assert_eq!(stacked_on(&daemon, "lead", json!({})), None);
    }

    #[test]
    fn a_stacked_branch_starts_from_the_parent_s_branch_whatever_branch_from_says() {
        let fx = fixture();
        commit(fx.linked(), "lead.txt", "lead\n", "lead work");
        let lead_tip = git(fx.linked(), &["rev-parse", "HEAD"]);
        let settings = WorktreeSettings {
            location: Some(format!("{}/trees/{{name}}", fx.root.display())),
            branch_from: Some(BranchFrom::Default),
            ..WorktreeSettings::default()
        };
        local(async {
            // finish_create_session passes the stack target's branch as the base.
            let stacked = create_worktree_for_session("child", &fx.linked, Some(&settings), Some("feature"))
                .await
                .unwrap();
            assert_eq!(stacked.base_ref.as_deref(), Some("feature"));
            assert_eq!(stacked.head, lead_tip);
            // Flat, branchFrom "default" starts from main.
            let flat = create_worktree_for_session("flat", &fx.linked, Some(&settings), None)
                .await
                .unwrap();
            assert_eq!(flat.base_ref.as_deref(), Some("main"));
            assert_ne!(flat.head, lead_tip);
        });
    }

    fn records(values: &[Value]) -> WorktreeRecords {
        values
            .iter()
            .map(|value| {
                let record = record(value.clone());
                (record.id.clone(), record)
            })
            .collect()
    }

    fn get<'a>(moved: &'a [Ordered<WorktreeRecord>], id: &str) -> &'a Ordered<WorktreeRecord> {
        moved.iter().find(|record| record.id == id).unwrap()
    }

    #[test]
    fn merging_or_deleting_a_target_moves_the_worktrees_on_it_one_hop_up() {
        let held = records(&[
            json!({"id": "a", "path": "/wt/a", "createdAt": AT, "mergedAt": AT}),
            json!({"id": "b", "path": "/wt/b", "createdAt": AT, "targetWorktreeId": "a", "targetBranch": "lead", "mergedAt": AT}),
            json!({"id": "c", "path": "/wt/c", "createdAt": AT, "baseSha": "s", "targetWorktreeId": "b", "targetBranch": "lead-ui"}),
            json!({"id": "d", "path": "/wt/d", "createdAt": AT, "targetWorktreeId": "b", "targetBranch": "lead-ui", "deletedAt": AT}),
        ]);
        // b merged: its live child c moves to b's target (a), past it as a is merged too: flat. d (deleted) stays.
        let moved = retargeted(&held, &held["b"]);
        assert_eq!(moved.len(), 1);
        let c = get(&moved, "c");
        assert_eq!(
            (c.target_worktree_id.as_deref(), c.target_branch.as_deref()),
            (None, None)
        );
        assert_eq!(c.retargeted_from.as_deref(), Some("lead-ui"));
        let value = c.to_value().unwrap();
        assert_eq!(keys(&value), ["id", "path", "createdAt", "baseSha", "retargetedFrom"]);

        // A grandchild moves up one hop when the target in the middle merges.
        let chain = records(&[
            json!({"id": "a", "path": "/wt/a", "createdAt": AT}),
            json!({"id": "b", "path": "/wt/b", "createdAt": AT, "targetWorktreeId": "a", "targetBranch": "lead", "deletedAt": AT}),
            json!({"id": "c", "path": "/wt/c", "createdAt": AT, "targetWorktreeId": "b", "targetBranch": "lead-ui"}),
        ]);
        let moved = retargeted(&chain, &chain["b"]);
        let c = get(&moved, "c");
        assert_eq!(c.target_worktree_id.as_deref(), Some("a"));
        assert_eq!(c.target_branch.as_deref(), Some("lead"));
        assert_eq!(c.retargeted_from.as_deref(), Some("lead-ui"));
        // A live target moves nothing; nor does a record nobody targets.
        assert!(retargeted(&chain, &chain["a"]).is_empty());
        assert!(retargeted(&chain, &chain["c"]).is_empty());

        // Edited state with a cycle ends flat instead of looping.
        let cycle = records(&[
            json!({"id": "a", "path": "/wt/a", "createdAt": AT, "targetWorktreeId": "b", "targetBranch": "b", "mergedAt": AT}),
            json!({"id": "b", "path": "/wt/b", "createdAt": AT, "targetWorktreeId": "a", "targetBranch": "a"}),
        ]);
        let moved = retargeted(&cycle, &cycle["a"]);
        assert_eq!(get(&moved, "b").target_worktree_id, None);
    }

    fn merged_markers(into: &str) -> WorktreeMarkers {
        WorktreeMarkers {
            merged_at: Some(AT.into()),
            merge_mode: Some(WorktreeMergeMode::Merge),
            merge_target_branch: Some(into.into()),
            merge_source_ref: Some("x".into()),
            ..WorktreeMarkers::default()
        }
    }

    /// Lead (`a`, on `lead`) ← Login UI (`b`, on `lead-ui`) ← Tests (`c`), each with its session.
    fn stack(dir: &Path) -> Daemon {
        let daemon = daemon(dir);
        insert_record(&daemon, record(json!({"id": "a", "path": "/wt/a", "createdAt": AT})));
        insert_record(
            &daemon,
            record(
                json!({"id": "b", "path": "/wt/b", "createdAt": AT, "targetWorktreeId": "a", "targetBranch": "lead"}),
            ),
        );
        insert_record(
            &daemon,
            record(
                json!({"id": "c", "path": "/wt/c", "createdAt": AT, "targetWorktreeId": "b", "targetBranch": "lead-ui"}),
            ),
        );
        for id in ["a", "b", "c"] {
            let path = format!("/wt/{id}");
            daemon.set_session(in_worktree(
                &format!("s-{id}"),
                "/r",
                &path,
                json!({"mode": "managed", "path": path, "id": id}),
            ));
        }
        daemon
    }

    fn target_of(daemon: &Daemon, session_id: &str) -> (Option<String>, Option<String>, Option<String>) {
        let session = daemon.get_session(session_id).unwrap();
        let worktree = session.worktree.clone().unwrap();
        (
            worktree.target_worktree_id,
            worktree.target_branch,
            worktree.retargeted_from,
        )
    }

    fn some(text: &str) -> Option<String> {
        Some(text.to_owned())
    }

    #[test]
    fn a_merged_target_hands_its_children_on_and_unmarking_does_not_take_them_back() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let daemon = stack(dir.path());
        // The sessions carry their worktree's target.
        assert_eq!(target_of(&daemon, "s-c"), (some("b"), some("lead-ui"), None));
        local(async {
            daemon
                .save_merge_markers("s-b", Some(merged_markers("lead")), None)
                .await
                .unwrap();
        });
        assert_eq!(target_of(&daemon, "s-c"), (some("a"), some("lead"), some("lead-ui")));
        // b keeps its own target: merged into it.
        assert_eq!(target_of(&daemon, "s-b"), (some("a"), some("lead"), None));
        let state: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("state.json")).unwrap()).unwrap();
        let saved = state["worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == "c")
            .unwrap();
        assert_eq!(saved["targetWorktreeId"], "a");
        assert_eq!(saved["retargetedFrom"], "lead-ui");
        // Sessions store only the record's ID.
        let stored = state["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == "s-c")
            .unwrap();
        assert_eq!(keys(&stored["worktree"]), ["mode", "path", "id"]);

        local(async {
            daemon.save_merge_markers("s-b", None, None).await.unwrap();
        });
        assert_eq!(target_of(&daemon, "s-c"), (some("a"), some("lead"), some("lead-ui")));
        // The top merging leaves the next level flat.
        local(async {
            daemon
                .save_merge_markers("s-a", Some(merged_markers("main")), None)
                .await
                .unwrap();
        });
        assert_eq!(target_of(&daemon, "s-b"), (None, None, some("lead")));
        assert_eq!(target_of(&daemon, "s-c"), (None, None, some("lead")));
        assert!(
            std::fs::read_to_string(dir.path().join("daemon.log"))
                .unwrap()
                .contains("worktree /wt/c now targets its launch checkout (was lead)")
        );
    }

    #[test]
    fn a_deleted_target_hands_its_children_on_and_their_own_markers_keep_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let daemon = stack(dir.path());
        local(async {
            // Marking and unmarking a child merged leaves its target alone.
            daemon
                .save_merge_markers("s-c", Some(merged_markers("lead-ui")), None)
                .await
                .unwrap();
            assert_eq!(target_of(&daemon, "s-c"), (some("b"), some("lead-ui"), None));
            daemon
                .save_merge_markers("s-c", None, Some("tip".into()))
                .await
                .unwrap();
            assert_eq!(target_of(&daemon, "s-c"), (some("b"), some("lead-ui"), None));

            let mut deleted = daemon.worktrees.borrow().records.get("b").cloned().unwrap();
            deleted.touch("deletedAt");
            deleted.deleted_at = Some(AT.into());
            daemon.save_worktree_record(deleted).await.unwrap();
        });
        assert_eq!(target_of(&daemon, "s-c"), (some("a"), some("lead"), some("lead-ui")));
    }

    /// A worktree `child` on branch `child`, started from `from` in `fx`, with one commit when `work` (its record `w2`
    /// targets `target`, with its session `s-child`).
    fn child_of(daemon: &Daemon, fx: &Fixture, from: &str, target: Value, work: bool) -> String {
        let child = fx.root.join("child");
        let path = child.to_string_lossy().into_owned();
        git(fx.repo(), &["worktree", "add", "-q", "-b", "child", &path, from]);
        let base = git(&child, &["rev-parse", "HEAD"]);
        if work {
            commit(&child, "child.txt", "child\n", "child work");
        }
        let mut value = json!({"id": "w2", "path": path, "createdAt": AT, "baseSha": base});
        for (key, entry) in target.as_object().cloned().unwrap_or_default() {
            value[key] = entry;
        }
        insert_record(daemon, record(value));
        insert(
            daemon,
            in_worktree(
                "s-child",
                &fx.repo,
                &path,
                json!({"mode": "managed", "path": path, "id": "w2"}),
            ),
        );
        path
    }

    fn detect(daemon: &Daemon) -> Ordered<WorktreeRecord> {
        local(async { daemon.run_merge_detection(None).await.unwrap() });
        daemon.worktrees.borrow().records.get("w2").cloned().unwrap()
    }

    #[test]
    fn a_stacked_worktree_is_merged_once_its_tip_is_in_its_target_branch() {
        let fx = fixture();
        let _home = Home::set(&fx.root);
        commit(fx.linked(), "lead.txt", "lead\n", "lead work");
        let daemon = lead_in_feature(&fx);
        let child = child_of(
            &daemon,
            &fx,
            "feature",
            json!({"targetWorktreeId": "w1", "targetBranch": "feature"}),
            false,
        );
        // Fresh from its parent's branch: in it already, but with nothing of its own.
        assert_eq!(detect(&daemon).merged_at, None);
        commit(Path::new(&child), "child.txt", "child\n", "child work");
        assert_eq!(detect(&daemon).merged_at, None);
        git(fx.linked(), &["merge", "-q", "--no-ff", "-m", "merge child", "child"]);
        let merged = detect(&daemon);
        assert!(merged.merged_at.is_some());
        assert_eq!(merged.merge_target_branch.as_deref(), Some("feature"));
        assert_eq!(
            merged.merge_detected,
            Some(deckhand_protocol::data::MergeDetected::Ancestor)
        );
        // Its parent's branch is not in main, so the parent is not merged.
        assert_eq!(daemon.worktrees.borrow().records.get("w1").unwrap().merged_at, None);
    }

    #[test]
    fn a_worktree_whose_target_is_gone_is_merged_once_its_tip_is_in_the_default_branch() {
        let fx = fixture();
        let _home = Home::set(&fx.root);
        let daemon = lead_in_feature(&fx);
        child_of(
            &daemon,
            &fx,
            "main",
            json!({"targetWorktreeId": "gone", "targetBranch": "feature"}),
            true,
        );
        assert_eq!(detect(&daemon).merged_at, None);
        git(fx.repo(), &["merge", "-q", "--no-ff", "-m", "merge child", "child"]);
        assert_eq!(detect(&daemon).merge_target_branch.as_deref(), Some("main"));
    }

    fn with_agent(session: &Ordered<SessionRecord>, status: &str) -> Ordered<SessionRecord> {
        let mut value = session.to_value().unwrap();
        value["agentStatus"] = json!(status);
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn a_merge_into_a_worktree_waits_until_its_agent_is_idle() {
        let fx = fixture();
        let _home = Home::set(&fx.root);
        let daemon = lead_in_feature(&fx);
        let lead = daemon.get_session("lead").unwrap();
        insert(&daemon, with_agent(&lead, "active"));
        child_of(
            &daemon,
            &fx,
            "feature",
            json!({"targetWorktreeId": "w1", "targetBranch": "feature"}),
            true,
        );
        let before = git(fx.linked(), &["rev-parse", "HEAD"]);
        let merge = |into: &str| {
            let daemon = daemon.clone();
            let into = into.to_owned();
            local(async move {
                daemon
                    .merge_session_worktree("s-child", WorktreeMergeMode::Merge, &into, false, false)
                    .await
                    .map(|result| result.target_branch.clone())
                    .map_err(|error| error.to_string())
            })
        };
        assert_eq!(
            merge(&fx.linked),
            Err("lead is working in feature; merge when it is idle".to_owned())
        );
        assert_eq!(git(fx.linked(), &["rev-parse", "HEAD"]), before);
        assert_eq!(daemon.worktrees.borrow().records.get("w2").unwrap().merged_at, None);

        insert(&daemon, with_agent(&lead, "idle"));
        assert_eq!(merge(&fx.linked), Ok("feature".to_owned()));
        let merged = daemon.worktrees.borrow().records.get("w2").cloned().unwrap();
        assert_eq!(merged.merge_target_branch.as_deref(), Some("feature"));
    }

    #[test]
    fn a_merge_into_the_main_checkout_does_not_wait_for_its_agents() {
        let fx = fixture();
        let _home = Home::set(&fx.root);
        let daemon = lead_in_feature(&fx);
        insert(&daemon, with_agent(&session("main", &fx.repo, &fx.repo), "active"));
        child_of(&daemon, &fx, "main", json!({}), true);
        let merged = local(async {
            daemon
                .merge_session_worktree("s-child", WorktreeMergeMode::Merge, &fx.repo, false, false)
                .await
        });
        assert_eq!(merged.unwrap().target_branch, "main");
    }
}
