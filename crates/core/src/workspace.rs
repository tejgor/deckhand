//! Port of workspace.ts.
//!
//! A workspace is the git worktree a session runs in. It is derived, never stored: every session in one worktree
//! shares the workspace's companion panes (Terminal, Git and Dev), which outlive the agents.

use deckhand_protocol::data::SessionRecord;

/// TS: workspace.ts workspaceKey (the resolved worktree root identifying the session's workspace, if any).
pub fn workspace_key(session: &SessionRecord) -> Option<String> {
    let _ = session;
    todo!("P2")
}

/// TS: workspace.ts noWorkspaceReason.
pub fn no_workspace_reason(session: &SessionRecord) -> &'static str {
    let _ = session;
    todo!("P2")
}

/// TS: workspace.ts workspacePaneUnavailable. `record_session_id` / `record_workspace` are the pane record's
/// `sessionId` and `workspace` stamps (PaneRecord, ChangesRecord).
pub fn workspace_pane_unavailable(
    session: &SessionRecord,
    record_session_id: Option<&str>,
    record_workspace: Option<&str>,
) -> Option<&'static str> {
    let _ = (session, record_session_id, record_workspace);
    todo!("P2")
}

/// TS: workspace.ts workspaceWorkerId (`workspace-<first 16 hex of sha256>`).
pub fn workspace_worker_id(workspace: &str) -> String {
    let _ = workspace;
    todo!("P2")
}
