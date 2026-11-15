use std::collections::{HashMap, HashSet};

use crate::jsstr::path;
use crate::session_order::{SessionLike, parent_of};
use deckhand_protocol::data::SessionRecord;

/// TS: sessionScope.ts isPathInside (lexical: `child` is `parent` or beneath it).
pub fn is_path_inside(parent: &str, child: &str) -> bool {
    let relative = path::relative(&path::resolve(parent), &path::resolve(child));
    relative.is_empty()
        || (relative != ".." && !relative.starts_with(&format!("..{}", path::SEP)) && !path::is_absolute(&relative))
}

/// TS: sessionScope.ts sessionMatchesScope.
///
/// Keep the repo overview, but also find sessions launched into this worktree elsewhere.
pub fn session_matches_scope(session: &SessionRecord, repo_root: &str) -> bool {
    let root = path::resolve(repo_root);
    path::resolve(&session.repo_root) == root
        || session
            .worktree
            .as_ref()
            .and_then(|worktree| worktree.path.as_deref())
            .is_some_and(|worktree_path| path::resolve(worktree_path) == root)
        || is_path_inside(&root, &session.cwd)
}

/// Not in the TS: the sessions [`session_matches_scope`] keeps, and every descendant of one, in list order.
///
/// A sub-session started in a new worktree keeps its parent's `repoRoot`, which is not this checkout when the parent
/// was itself launched from another one, so it would otherwise leave the sidebar it was created from.
pub fn sessions_in_scope<'a, S: SessionLike>(sessions: &'a [S], repo_root: &str) -> Vec<&'a S> {
    let matching: HashSet<&str> = sessions
        .iter()
        .map(SessionLike::session)
        .filter(|session| session_matches_scope(session, repo_root))
        .map(|session| session.id.as_str())
        .collect();
    let parents: HashMap<&str, Option<&str>> = sessions
        .iter()
        .map(|item| (item.session().id.as_str(), parent_of(item.session())))
        .collect();
    sessions
        .iter()
        .filter(|item| {
            let session = item.session();
            if matching.contains(session.id.as_str()) {
                return true;
            }
            let mut seen: HashSet<&str> = HashSet::from([session.id.as_str()]);
            let mut parent = parent_of(session);
            while let Some(id) = parent.filter(|id| seen.insert(id)) {
                if matching.contains(id) {
                    return true;
                }
                parent = parents.get(id).copied().flatten();
            }
            false
        })
        .collect()
}

/// The top-level session above `session_id`, among `sessions`: the last of its parents the list has (itself when it
/// has none), as the daemon finds it for messages. `None` when the list does not have `session_id`.
pub fn tree_root<'a, S: SessionLike>(sessions: &'a [S], session_id: &str) -> Option<&'a S> {
    root_in(&by_id(sessions), session_id)
}

/// The tree `session_id` is in: every session under the same top-level session as it (that one and `session_id`
/// included), whichever checkout each is in, in list order. These are the sessions it may message.
pub fn session_tree<'a, S: SessionLike>(sessions: &'a [S], session_id: &str) -> Vec<&'a S> {
    let by_id = by_id(sessions);
    let Some(root) = root_in(&by_id, session_id).map(|root| root.session().id.as_str()) else {
        return Vec::new();
    };
    sessions
        .iter()
        .filter(|item| root_in(&by_id, &item.session().id).is_some_and(|other| other.session().id == root))
        .collect()
}

fn by_id<S: SessionLike>(sessions: &[S]) -> HashMap<&str, &S> {
    sessions.iter().map(|item| (item.session().id.as_str(), item)).collect()
}

fn root_in<'a, S: SessionLike>(by_id: &HashMap<&str, &'a S>, session_id: &str) -> Option<&'a S> {
    // The daemon's walk (`messages.rs`), so a parent cycle splits the same way there and here.
    let mut current = *by_id.get(session_id)?;
    let mut seen: HashSet<&str> = HashSet::new();
    while seen.insert(current.session().id.as_str()) {
        match parent_of(current.session()).and_then(|id| by_id.get(id).copied()) {
            Some(parent) => current = parent,
            None => break,
        }
    }
    Some(current)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use deckhand_testkit::{Case, replay};
    use serde_json::{Value, json};

    /// A SessionRecord from a partial TS literal (`{...} as SessionRecord`): required fields the code under test never
    /// reads get placeholder values.
    pub(crate) fn partial_session(value: &Value) -> Result<SessionRecord, String> {
        let mut full = json!({
            "id": "", "title": "", "program": "claude", "command": "", "cwd": "", "repoRoot": "",
            "status": "running", "createdAt": "", "updatedAt": "",
        });
        if let (Some(full), Some(given)) = (full.as_object_mut(), value.as_object()) {
            for (key, entry) in given {
                full.insert(key.clone(), entry.clone());
            }
        }
        serde_json::from_value(full).map_err(|error| error.to_string())
    }

    #[test]
    fn is_path_inside_parity() {
        replay("sessionScope/isPathInside", |case: &Case| {
            Ok::<_, String>(is_path_inside(case.arg_str(0)?, case.arg_str(1)?))
        });
    }

    #[test]
    fn session_matches_scope_parity() {
        replay("sessionScope/sessionMatchesScope", |case: &Case| {
            let session = partial_session(&deckhand_testkit::tags::untag(case.arg(0)))?;
            Ok::<_, String>(session_matches_scope(&session, case.arg_str(1)?))
        });
    }

    #[test]
    fn session_scope_links_main_repo_and_worktrees() {
        let session = partial_session(&json!({"repoRoot": "/repos/main", "cwd": "/worktrees/feature", "worktree": {"path": "/worktrees/feature", "mode": "managed"}})).unwrap();
        let bare = SessionRecord {
            worktree: None,
            ..session.clone()
        };
        let bare_src = SessionRecord {
            cwd: "/worktrees/feature/src".into(),
            ..bare.clone()
        };
        let cases: [(&str, &SessionRecord, &str, bool); 7] = [
            (
                "main repo includes its worktree sessions",
                &session,
                "/repos/main",
                true,
            ),
            (
                "worktree includes sessions created from the main repo",
                &session,
                "/worktrees/feature",
                true,
            ),
            ("trailing slash is ignored", &session, "/worktrees/feature/", true),
            ("unrelated worktree", &session, "/worktrees/other", false),
            ("name prefix is not containment", &session, "/worktrees/feat", false),
            ("no worktree metadata: cwd matches", &bare, "/worktrees/feature", true),
            (
                "no worktree metadata: cwd subdirectory matches",
                &bare_src,
                "/worktrees/feature",
                true,
            ),
        ];
        for (name, record, scope, expected) in cases {
            assert_eq!(session_matches_scope(record, scope), expected, "{name}");
        }
        let paths = [
            ("/a/b", "/a/b", true),
            ("/a/b/", "/a/b/c/d", true),
            ("/a/b", "/a/bc", false),
            ("/a/b", "/a", false),
            ("/a/b", "/a/b/../c", false),
            ("/a/b", "/a/b/..c", true),
        ];
        for (parent, child, expected) in paths {
            assert_eq!(is_path_inside(parent, child), expected, "{child} inside {parent}");
        }
    }

    #[test]
    fn a_sub_session_in_its_own_worktree_stays_with_its_parent() {
        // The parent was launched from the `base` checkout into `/wt/feature`; Deckhand runs in `/wt/feature`.
        let session = |id: &str, parent: Option<&str>, repo_root: &str, worktree: &str| {
            partial_session(&json!({
                "id": id, "parentSessionId": parent, "repoRoot": repo_root, "cwd": worktree,
                "worktree": {"mode": "managed", "path": worktree},
            }))
            .unwrap()
        };
        let list = [
            session("parent", None, "/wt/base", "/wt/feature"),
            session("child", Some("parent"), "/wt/base", "/wt/feature-child"),
            session("grandchild", Some("child"), "/wt/base", "/wt/feature-grandchild"),
            session("elsewhere", None, "/wt/base", "/wt/base"),
            session("cycle-a", Some("cycle-b"), "/wt/base", "/wt/a"),
            session("cycle-b", Some("cycle-a"), "/wt/base", "/wt/b"),
        ];
        let ids = |root: &str| -> Vec<String> {
            sessions_in_scope(&list, root)
                .into_iter()
                .map(|session| session.id.clone())
                .collect()
        };
        assert!(!session_matches_scope(&list[1], "/wt/feature"));
        assert_eq!(ids("/wt/feature"), ["parent", "child", "grandchild"]);
        // The checkout the parent was launched from keeps all of them.
        assert_eq!(
            ids("/wt/base"),
            list.iter().map(|session| session.id.clone()).collect::<Vec<_>>()
        );
        assert_eq!(ids("/wt/feature-child"), ["child", "grandchild"]);
        assert!(ids("/elsewhere").is_empty());
    }

    #[test]
    fn a_session_s_tree_is_everything_under_its_top_level_session_in_any_checkout() {
        // A stacked tree spread over worktrees launched from two checkouts, beside another top-level session.
        let session = |id: &str, parent: Option<&str>, repo_root: &str| {
            partial_session(&json!({
                "id": id, "parentSessionId": parent, "repoRoot": repo_root, "cwd": format!("/wt/{id}"),
                "worktree": {"mode": "managed", "path": format!("/wt/{id}")},
            }))
            .unwrap()
        };
        let list = [
            session("lead", None, "/repo"),
            session("tokens", Some("lead"), "/repo"),
            session("refresh", Some("tokens"), "/wt/tokens"),
            session("login", Some("lead"), "/repo"),
            session("other", None, "/repo"),
            session("other-child", Some("other"), "/repo"),
            session("orphan", Some("gone"), "/repo"),
        ];
        let tree = |id: &str| -> Vec<&str> {
            session_tree(&list, id)
                .into_iter()
                .map(|session| session.id.as_str())
                .collect()
        };
        let lead_tree = ["lead", "tokens", "refresh", "login"];
        assert_eq!(tree("refresh"), lead_tree, "from a grandchild in another checkout");
        assert_eq!(tree("login"), lead_tree, "from a sibling");
        assert_eq!(tree("lead"), lead_tree, "from the top-level session");
        assert_eq!(tree_root(&list, "refresh").map(|root| root.id.as_str()), Some("lead"));
        assert_eq!(tree("other-child"), ["other", "other-child"]);
        // A parent the list doesn't have: the session is its own top-level session.
        assert_eq!(tree("orphan"), ["orphan"]);
        assert!(tree("unknown").is_empty());
        assert!(tree_root(&list, "unknown").is_none());
    }
}
