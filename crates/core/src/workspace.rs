//! Port of workspace.ts.
//!
//! A workspace is the git worktree a session runs in. It is derived, never stored: every session in one worktree
//! shares the workspace's companion panes (Terminal, Git and Dev), which outlive the agents.

use sha2::{Digest, Sha256};

use crate::jsstr::path;
use deckhand_protocol::data::{SessionRecord, SessionWorktreeMode, WorktreeMode};

/// A JS-truthy optional string (set and not empty).
fn truthy(value: Option<&str>) -> bool {
    value.is_some_and(|text| !text.is_empty())
}

/// TS: workspace.ts workspaceKey (the resolved worktree root identifying the session's workspace, if any).
///
/// - worktree-backed sessions (managed/attached, including one attached to the main checkout): the worktree path
/// - sessions without a worktree: the launch checkout's root (`launchWorktreeRoot`, else `cwd` for old records)
/// - a deleted worktree has none (`worktree.deletedAt`, projected from the worktree's record onto every session of
///   it), so an old session never shares a workspace with a new worktree later created at that path; neither has a
///   session still waiting for its new/selected worktree (it would otherwise resolve to the checkout it was launched
///   from).
///
/// Lexical (path.resolve); every path here comes from Git.
pub fn workspace_key(session: &SessionRecord) -> Option<String> {
    let worktree = session.worktree.as_ref();
    if worktree.is_some_and(|worktree| truthy(worktree.deleted_at.as_deref())) {
        return None;
    }
    if let Some(worktree) = worktree.filter(|worktree| worktree.mode != SessionWorktreeMode::None) {
        return worktree
            .path
            .as_deref()
            .filter(|worktree_path| !worktree_path.is_empty())
            .map(path::resolve);
    }
    if session
        .requested_worktree_mode
        .is_some_and(|mode| mode != WorktreeMode::None)
    {
        return None;
    }
    Some(path::resolve(
        session.launch_worktree_root.as_deref().unwrap_or(&session.cwd),
    ))
}

/// TS: workspace.ts noWorkspaceReason (why `workspace_key` is `None`, for errors and the workspace panes).
pub fn no_workspace_reason(session: &SessionRecord) -> &'static str {
    let deleted = session
        .worktree
        .as_ref()
        .is_some_and(|worktree| truthy(worktree.deleted_at.as_deref()));
    if deleted {
        "its worktree was deleted"
    } else {
        "its worktree is not ready yet"
    }
}

/// TS: workspace.ts workspacePaneUnavailable. `record_session_id` / `record_workspace` are the pane record's
/// `sessionId` and `workspace` stamps (PaneRecord, ChangesRecord).
///
/// The daemon also treats a session whose worktree it is still preparing as having none, and says so with a record
/// for that session without a `workspace`.
pub fn workspace_pane_unavailable(
    session: &SessionRecord,
    record_session_id: Option<&str>,
    record_workspace: Option<&str>,
) -> Option<&'static str> {
    let unavailable = workspace_key(session).is_none_or(|key| key.is_empty())
        || (record_session_id == Some(session.id.as_str()) && !truthy(record_workspace));
    unavailable.then(|| no_workspace_reason(session))
}

/// TS: workspace.ts workspaceWorkerId (`workspace-<first 16 hex of sha256>`): the file-name-safe identity of a
/// workspace worker (PID/log files live beside session workers').
pub fn workspace_worker_id(workspace: &str) -> String {
    let digest = Sha256::digest(workspace.as_bytes());
    let hex: String = digest.iter().take(8).map(|byte| format!("{byte:02x}")).collect();
    format!("workspace-{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_scope::tests::partial_session;
    use deckhand_parity::{Case, Outcome, replay, tags};
    use serde_json::json;

    fn session_arg(case: &Case, index: usize) -> Result<SessionRecord, String> {
        partial_session(&tags::untag(case.arg(index)))
    }

    /// Runs a port body whose harness steps may fail (argument decoding).
    fn port(body: impl FnOnce() -> Result<Outcome, String>) -> Outcome {
        body().unwrap_or_else(|error| Outcome::throws(format!("harness: {error}")))
    }

    #[test]
    fn workspace_key_parity() {
        replay("P2", "workspace/workspaceKey", |case: &Case| {
            port(|| {
                Ok(match workspace_key(&session_arg(case, 0)?) {
                    Some(key) => Outcome::ret(key),
                    None => Outcome::ret(tags::undefined()),
                })
            })
        });
    }

    #[test]
    fn no_workspace_reason_parity() {
        replay("P2", "workspace/noWorkspaceReason", |case: &Case| {
            Ok::<_, String>(no_workspace_reason(&session_arg(case, 0)?))
        });
    }

    #[test]
    fn workspace_pane_unavailable_parity() {
        replay("P2", "workspace/workspacePaneUnavailable", |case: &Case| {
            port(|| {
                let record = tags::untag(case.arg(1));
                let field = |key: &str| record.get(key).and_then(|value| value.as_str());
                Ok(
                    match workspace_pane_unavailable(&session_arg(case, 0)?, field("sessionId"), field("workspace")) {
                        Some(reason) => Outcome::ret(reason),
                        None => Outcome::ret(tags::undefined()),
                    },
                )
            })
        });
    }

    #[test]
    fn workspace_worker_id_parity() {
        replay("P2", "workspace/workspaceWorkerId", |case: &Case| {
            Ok::<_, String>(workspace_worker_id(case.arg_str(0)?))
        });
    }

    // covers: B-P2-02 workspace key: the worktree a session runs in, none for deleted or unprepared worktrees
    #[test]
    fn workspace_key_cases() {
        let plain = json!({"cwd": "/repos/app", "launchWorktreeRoot": "/repos/app", "worktree": {"mode": "none"}, "requestedWorktreeMode": "none"});
        let with = |base: &serde_json::Value, extra: serde_json::Value| {
            let mut out = base.clone();
            for (key, value) in extra.as_object().unwrap() {
                if value.is_null() {
                    out.as_object_mut().unwrap().remove(key);
                } else {
                    out[key] = value.clone();
                }
            }
            partial_session(&out).unwrap()
        };
        let managed_json = {
            let mut out = plain.clone();
            out["cwd"] = json!("/wt/feature");
            out["worktree"] = json!({"mode": "managed", "path": "/wt/feature/"});
            out["requestedWorktreeMode"] = json!("new");
            out
        };
        let cases: Vec<(&str, SessionRecord, Option<&str>)> = vec![
            (
                "no worktree: the launch checkout root",
                with(&plain, json!({})),
                Some("/repos/app"),
            ),
            (
                "no worktree launched in a linked worktree: that worktree",
                with(&plain, json!({"cwd": "/wt/other", "launchWorktreeRoot": "/wt/other"})),
                Some("/wt/other"),
            ),
            (
                "no worktree, older record without launchWorktreeRoot: cwd",
                with(&plain, json!({"launchWorktreeRoot": null})),
                Some("/repos/app"),
            ),
            (
                "managed worktree: its path, resolved",
                with(&managed_json, json!({})),
                Some("/wt/feature"),
            ),
            (
                "attached worktree (here the main checkout): shares the plain session workspace",
                with(
                    &plain,
                    json!({"worktree": {"mode": "attached", "path": "/repos/app", "isMain": true}, "requestedWorktreeMode": "existing"}),
                ),
                Some("/repos/app"),
            ),
            (
                "launch root never wins over the worktree",
                with(&managed_json, json!({"launchWorktreeRoot": "/repos/app"})),
                Some("/wt/feature"),
            ),
            (
                "deleted worktree: no workspace (a new worktree at that path is a new workspace)",
                with(
                    &managed_json,
                    json!({"worktree": {"mode": "managed", "path": "/wt/feature/", "deletedAt": "2026-01-01T00:00:00.000Z"}}),
                ),
                None,
            ),
            (
                "new worktree still being created: not the launch checkout",
                with(&plain, json!({"requestedWorktreeMode": "new"})),
                None,
            ),
            (
                "existing worktree not selected yet",
                with(&plain, json!({"requestedWorktreeMode": "existing"})),
                None,
            ),
            (
                "worktree record without a path",
                with(&managed_json, json!({"worktree": {"mode": "managed"}})),
                None,
            ),
        ];
        for (name, session, expected) in &cases {
            assert_eq!(workspace_key(session).as_deref(), *expected, "{name}");
        }
        assert!(no_workspace_reason(&cases[6].1).contains("deleted"));
        assert!(no_workspace_reason(&cases[7].1).contains("not ready"));
        assert_eq!(workspace_worker_id("/wt/feature"), workspace_worker_id("/wt/feature"));
        assert_ne!(workspace_worker_id("/wt/feature"), workspace_worker_id("/wt/other"));
        let id = workspace_worker_id("/wt/feature");
        let hex = id.strip_prefix("workspace-").unwrap();
        assert!(
            hex.len() == 16 && hex.chars().all(|ch| matches!(ch, '0'..='9' | 'a'..='f')),
            "{id}"
        );
    }
}
