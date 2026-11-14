//! Stacked worktrees as the merge confirmation sees them (docs/future/orchestrator.md, part 1, "One hop at a time"):
//! where a worktree's target goes after it (the path), what works in the target (a merge there waits), and which
//! worktrees are stacked on the one being merged (its children, deeper ones folded into the child they sit under).
//!
//! Everything is read from the sessions' projected worktree records (`targetWorktreeId`, `targetBranch`, markers),
//! so the UI needs nothing beyond the sessions it already holds.

use indexmap::IndexMap;

use crate::jsstr::path;
use crate::sidebar_model::agent_working;
use deckhand_protocol::data::{SessionRecord, SessionWorktreeRecord};

/// A JS-truthy optional string (set and not empty).
fn truthy(value: Option<&str>) -> bool {
    value.is_some_and(|text| !text.is_empty())
}

/// The session's `worktree.id`, when truthy.
fn worktree_id(session: &SessionRecord) -> Option<&str> {
    session
        .worktree
        .as_ref()
        .and_then(|worktree| worktree.id.as_deref())
        .filter(|id| !id.is_empty())
}

/// Each worktree the sessions are in, by id, as the first session in it projects it (every session in a worktree
/// carries the same projected record), with that session.
fn worktrees<'a, I>(sessions: I) -> IndexMap<&'a str, (&'a SessionWorktreeRecord, &'a SessionRecord)>
where
    I: IntoIterator<Item = &'a SessionRecord>,
{
    let mut found = IndexMap::new();
    for session in sessions {
        if let (Some(id), Some(worktree)) = (worktree_id(session), session.worktree.as_ref()) {
            found.entry(id).or_insert((worktree, session));
        }
    }
    found
}

/// Not merged and not deleted: work can still go into it (as the daemon's `is_live_target`).
pub fn is_live(worktree: &SessionWorktreeRecord) -> bool {
    !truthy(worktree.merged_at.as_deref()) && !truthy(worktree.deleted_at.as_deref())
}

/// The branches a merge into `worktree`'s target reaches after it, nearest first: each target's own target branch up
/// the chain, then `launch_branch` (where a flat worktree merges) when it is known and not the last hop already. A
/// target no session is in any more ends the path at the last branch known; a cycle (edited state) ends it too.
pub fn later_hops<'a, I>(sessions: I, worktree: &SessionWorktreeRecord, launch_branch: Option<&str>) -> Vec<String>
where
    I: IntoIterator<Item = &'a SessionRecord>,
{
    // Flat: the merge goes into the launch checkout, nothing comes after it.
    let Some(target) = worktree.target_worktree_id.as_deref().filter(|id| !id.is_empty()) else {
        return Vec::new();
    };
    let known = worktrees(sessions);
    let mut hops: Vec<String> = Vec::new();
    let mut last = worktree.target_branch.clone();
    let mut seen: Vec<&str> = Vec::new();
    let mut next = Some(target);
    while let Some(id) = next {
        if seen.contains(&id) {
            return hops;
        }
        seen.push(id);
        let Some((target, _)) = known.get(id) else {
            return hops;
        };
        match target.target_worktree_id.as_deref().filter(|id| !id.is_empty()) {
            Some(up) => {
                if let Some(branch) = target.target_branch.as_deref().filter(|branch| !branch.is_empty()) {
                    hops.push(branch.to_owned());
                    last = Some(branch.to_owned());
                }
                next = Some(up);
            }
            None => next = None,
        }
    }
    if let Some(launch) = launch_branch.filter(|branch| !branch.is_empty())
        && last.as_deref() != Some(launch)
    {
        hops.push(launch.to_owned());
    }
    hops
}

/// The sessions whose agent is working in the worktree `target_id` (at `root`): in it by their worktree, or running
/// there without one of their own. A merge into it is refused until they are idle (the daemon's
/// `refuse_merge_while_working`).
pub fn working_in<'a, I>(sessions: I, target_id: &str, root: &str) -> Vec<&'a SessionRecord>
where
    I: IntoIterator<Item = &'a SessionRecord>,
{
    let root = path::resolve(root);
    sessions
        .into_iter()
        .filter(|session| {
            agent_working(session) && (worktree_id(session) == Some(target_id) || path::resolve(&session.cwd) == root)
        })
        .collect()
}

/// A worktree stacked directly on the one being merged, and how many more are stacked under it.
#[derive(Clone, Debug, PartialEq)]
pub struct StackedChild<'a> {
    pub worktree_id: &'a str,
    /// The first session in it: what the row is named after.
    pub session: &'a SessionRecord,
    /// Live worktrees stacked under it, at any depth.
    pub under: usize,
}

/// The live worktrees targeting `worktree_id` (not merged, not deleted), in the sessions' order, each with the count
/// of live worktrees under it. Grandchildren are never listed on their own: one hop at a time.
pub fn stacked_children<'a, I>(sessions: I, worktree_id: &str) -> Vec<StackedChild<'a>>
where
    I: IntoIterator<Item = &'a SessionRecord>,
{
    let known = worktrees(sessions);
    let children_of = |parent: &str| -> Vec<&'a str> {
        known
            .iter()
            .filter(|(id, (worktree, _))| {
                **id != parent && is_live(worktree) && worktree.target_worktree_id.as_deref() == Some(parent)
            })
            .map(|(id, _)| *id)
            .collect()
    };
    children_of(worktree_id)
        .into_iter()
        .filter_map(|child| {
            let (_, session) = known.get(child)?;
            // Everything below it, each worktree once (a cycle in edited state counts nothing twice).
            let mut seen = vec![worktree_id, child];
            let mut queue = vec![child];
            let mut under = 0;
            while let Some(id) = queue.pop() {
                for below in children_of(id) {
                    if !seen.contains(&below) {
                        seen.push(below);
                        queue.push(below);
                        under += 1;
                    }
                }
            }
            Some(StackedChild {
                worktree_id: child,
                session,
                under,
            })
        })
        .collect()
}

/// The branch of the launch checkout, when a session there says so: one in the main checkout at `cwd` on a branch.
pub fn launch_branch_from_sessions<'a, I>(sessions: I, cwd: &str) -> Option<String>
where
    I: IntoIterator<Item = &'a SessionRecord>,
{
    let cwd = path::resolve(cwd);
    sessions.into_iter().find_map(|session| {
        let worktree = session.worktree.as_ref()?;
        (worktree.is_main == Some(true) && worktree.path.as_deref().is_some_and(|root| path::resolve(root) == cwd))
            .then(|| worktree.branch.clone())
            .flatten()
            .filter(|branch| !branch.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use deckhand_protocol::data::{AgentActivityStatus, SessionStatus, SessionWorktreeMode};
    use serde_json::json;

    fn session(id: &str, worktree: Option<SessionWorktreeRecord>) -> SessionRecord {
        let mut record: SessionRecord = serde_json::from_value(json!({
            "id": id, "title": id, "program": "claude", "command": "claude", "cwd": "/repo", "repoRoot": "/repo",
            "status": "running", "createdAt": "2026-01-01T00:00:00.000Z", "updatedAt": "2026-01-01T00:00:00.000Z",
        }))
        .expect("a session literal");
        record.agent_status = Some(AgentActivityStatus::Idle);
        record.worktree = worktree;
        record
    }

    /// A linked worktree `id` on `branch`, stacked on `target` (worktree id and branch) when given.
    fn worktree(id: &str, branch: &str, target: Option<(&str, &str)>) -> SessionWorktreeRecord {
        let mut worktree = SessionWorktreeRecord::new(SessionWorktreeMode::Managed);
        worktree.id = Some(id.to_owned());
        worktree.path = Some(format!("/wt/{id}"));
        worktree.branch = Some(branch.to_owned());
        worktree.target_worktree_id = target.map(|(id, _)| id.to_owned());
        worktree.target_branch = target.map(|(_, branch)| branch.to_owned());
        worktree
    }

    /// lead (feature, flat) ← a (feature-a) ← a1 ← a1x; lead ← b.
    fn tree() -> Vec<SessionRecord> {
        vec![
            session("lead", Some(worktree("lead", "feature", None))),
            session("a", Some(worktree("a", "feature-a", Some(("lead", "feature"))))),
            session("a-helper", Some(worktree("a", "feature-a", Some(("lead", "feature"))))),
            session("a1", Some(worktree("a1", "a1", Some(("a", "feature-a"))))),
            session("a1x", Some(worktree("a1x", "a1x", Some(("a1", "a1"))))),
            session("b", Some(worktree("b", "b", Some(("lead", "feature"))))),
        ]
    }

    #[test]
    fn the_path_walks_each_targets_own_target_then_the_launch_branch() {
        let sessions = tree();
        let a1x = worktree("a1x", "a1x", Some(("a1", "a1")));
        // Into a1, then feature-a, feature, and main (the launch checkout).
        assert_eq!(
            later_hops(&sessions, &a1x, Some("main")),
            ["feature-a", "feature", "main"]
        );
        // The launch branch unknown: the path ends at the last hop known.
        assert_eq!(later_hops(&sessions, &a1x, None), ["feature-a", "feature"]);
        // A child of a flat worktree: just the launch branch after it.
        let a = worktree("a", "feature-a", Some(("lead", "feature")));
        assert_eq!(later_hops(&sessions, &a, Some("main")), ["main"]);
        // The launch branch is not repeated when the chain already ends there.
        assert!(later_hops(&sessions, &a, Some("feature")).is_empty());
        // Flat: nothing after the merge into the launch checkout.
        assert!(later_hops(&sessions, &worktree("x", "x", None), Some("main")).is_empty());
        // A target no session is in: stop at what is known.
        let orphan = worktree("o", "o", Some(("gone", "gone")));
        assert!(later_hops(&sessions, &orphan, Some("main")).is_empty());
        // A cycle (edited state) ends the walk where it repeats.
        let looped = vec![
            session("p", Some(worktree("p", "p", Some(("q", "q"))))),
            session("q", Some(worktree("q", "q", Some(("p", "p"))))),
        ];
        let child = worktree("c", "c", Some(("p", "p")));
        assert_eq!(later_hops(&looped, &child, Some("main")), ["q", "p"]);
    }

    #[test]
    fn children_are_the_direct_live_ones_with_deeper_ones_folded_under_them() {
        let mut sessions = tree();
        let children = stacked_children(&sessions, "lead");
        let summary: Vec<(&str, &str, usize)> = children
            .iter()
            .map(|child| (child.worktree_id, child.session.id.as_str(), child.under))
            .collect();
        // a has a1 and a1x under it; its second session does not list it twice; grandchildren are not listed.
        assert_eq!(summary, [("a", "a", 2), ("b", "b", 0)]);
        // A merged or deleted child is in already (or gone): not listed, and not counted under its parent.
        if let Some(worktree) = sessions[5].worktree.as_mut() {
            worktree.merged_at = Some("2026-01-02T00:00:00.000Z".to_owned());
        }
        if let Some(worktree) = sessions[4].worktree.as_mut() {
            worktree.deleted_at = Some("2026-01-02T00:00:00.000Z".to_owned());
        }
        let summary: Vec<(&str, usize)> = stacked_children(&sessions, "lead")
            .iter()
            .map(|child| (child.worktree_id, child.under))
            .collect();
        assert_eq!(summary, [("a", 1)]);
        assert!(stacked_children(&sessions, "b").is_empty());
    }

    #[test]
    fn working_sessions_are_those_taking_their_turn_in_the_target() {
        let mut sessions = tree();
        assert!(working_in(&sessions, "lead", "/wt/lead").is_empty());
        sessions[0].agent_status = Some(AgentActivityStatus::Active);
        // In another worktree: does not hold this merge up.
        sessions[1].agent_status = Some(AgentActivityStatus::Active);
        // Without a worktree of its own, running in the target's directory.
        let mut visitor = session("visitor", None);
        visitor.cwd = "/wt/lead/".to_owned();
        visitor.agent_status = Some(AgentActivityStatus::Active);
        sessions.push(visitor);
        let mut exited = session("exited", Some(worktree("lead", "feature", None)));
        exited.status = SessionStatus::Exited;
        sessions.push(exited);
        let ids: Vec<&str> = working_in(&sessions, "lead", "/wt/lead")
            .iter()
            .map(|session| session.id.as_str())
            .collect();
        assert_eq!(ids, ["lead", "visitor"]);
    }

    #[test]
    fn the_launch_branch_comes_from_a_session_in_the_main_checkout_there() {
        let mut main = SessionWorktreeRecord::new(SessionWorktreeMode::Managed);
        main.is_main = Some(true);
        main.path = Some("/repo".to_owned());
        main.branch = Some("main".to_owned());
        let sessions = vec![
            session("lead", Some(worktree("lead", "feature", None))),
            session("m", Some(main)),
        ];
        assert_eq!(
            launch_branch_from_sessions(&sessions, "/repo/").as_deref(),
            Some("main")
        );
        assert_eq!(launch_branch_from_sessions(&sessions, "/elsewhere"), None);
        assert_eq!(launch_branch_from_sessions(&sessions[..1], "/repo"), None);
    }
}
