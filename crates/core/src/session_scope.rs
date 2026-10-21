//! Port of sessionScope.ts.

use crate::jsstr::path;
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

    // covers: B-P2-01 session scope links a main repo and its worktrees without matching lookalike paths
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
}
