//! Port of daemon.ts (request dispatch match).
//!
//! TS InkDaemon.handleRequest: one `switch` over the request type inside a `try`. A case either answers
//! `response(requestId, data)` or answers by itself (attach); anything it throws becomes `failure(requestId, error)`
//! when the request has a `requestId` (the attach input/resize/detach messages have none and get nothing). Here the
//! cases' work lives in the owning modules' methods; this module only routes and builds the envelope.

use deckhand_core::error::DhError;
use deckhand_protocol::data::{OkResult, RestartMode};
use deckhand_protocol::events::{ServerMessage, ServerResponse};
use deckhand_protocol::wire::Request;
use serde::Serialize;

use crate::daemon::Daemon;
use crate::server::{ClientHandle, Connection, send_message};
use crate::sessions::KillOptions;
use crate::workspaces::WorkspacePane;

/// TS: daemon.ts response: `{type: 'response', requestId, ok: true, data}`.
pub fn response<T: Serialize>(request_id: &str, data: &T) -> Result<ServerMessage, DhError> {
    ServerResponse::success(request_id, data)
        .map(ServerMessage::Response)
        .map_err(|error| DhError::msg(error.to_string()))
}

/// TS: daemon.ts failure: `{type: 'response', requestId, ok: false, error: <message>}` (an Error's `message`, else
/// `String(error)`: [`DhError`]'s `Display` is its message).
pub fn failure(request_id: &str, error: impl std::fmt::Display) -> ServerMessage {
    ServerMessage::Response(ServerResponse::failure(request_id, error.to_string()))
}

/// Answers a case's result: `response` on success; an error goes back to [`Daemon::handle_request`] as a failure.
fn reply<T: Serialize>(client: &ClientHandle, request_id: &str, data: Result<T, DhError>) -> Result<(), DhError> {
    let message = response(request_id, &data?)?;
    send_message(client, &message);
    Ok(())
}

/// `{ok: true}`, the answer of the cases that report nothing else.
fn ok(result: Result<(), DhError>) -> Result<OkResult, DhError> {
    result.map(|()| OkResult::default())
}

impl Daemon {
    /// TS: InkDaemon.handleRequest: runs the request's case; a failure is answered when the request has an ID.
    pub async fn handle_request(&self, conn: &Connection, request: Request) {
        let request_id = request.request_id().map(str::to_owned);
        if let Err(error) = self.dispatch(conn, request).await
            && let Some(request_id) = request_id
        {
            send_message(&conn.client, &failure(&request_id, &error));
        }
    }

    /// TS: handleRequest's `switch`.
    async fn dispatch(&self, conn: &Connection, request: Request) -> Result<(), DhError> {
        let client = &conn.client;
        match request {
            Request::Ping(r) => reply(client, &r.request_id, self.handle_ping()),
            Request::Shutdown(r) => reply(client, &r.request_id, self.handle_shutdown()),
            Request::SaveConfig(r) => reply(client, &r.request_id, self.handle_save_config(&r).await),
            Request::SettingsInfo(r) => reply(client, &r.request_id, self.handle_settings_info(&r).await),
            Request::WorktreeCandidates(r) => reply(client, &r.request_id, self.handle_worktree_candidates(&r).await),
            Request::GlobalLinkCandidates(r) => {
                reply(client, &r.request_id, self.handle_global_link_candidates(&r).await)
            }
            Request::WorktreeCandidateSizes(r) => {
                reply(client, &r.request_id, self.handle_worktree_candidate_sizes(&r).await)
            }
            Request::ProjectInfo(r) => reply(client, &r.request_id, self.handle_project_info(&r).await),
            Request::TrustProject(r) => reply(client, &r.request_id, self.handle_trust_project(&r).await),
            Request::WorkspaceSummary(r) => reply(client, &r.request_id, self.handle_workspace_summary(&r).await),
            Request::CreatePr(r) => reply(client, &r.request_id, self.handle_create_pr(&r).await),
            Request::InspectCleanup(r) => reply(
                client,
                &r.request_id,
                self.inspect_session_cleanup(&r.session_id, r.delete_branch.unwrap_or(true))
                    .await,
            ),
            Request::SetSessionDone(r) => reply(client, &r.request_id, self.handle_set_session_done(&r).await),
            Request::ArchiveSession(r) => reply(client, &r.request_id, self.handle_archive_session(&r).await),
            Request::ExportHandoff(r) => reply(client, &r.request_id, self.handle_export_handoff(&r).await),
            Request::AgentHook(r) => reply(client, &r.request_id, ok(self.handle_agent_hook(&r).await)),
            Request::CancelStart(r) => reply(client, &r.request_id, self.handle_cancel_start(&r).await),
            Request::RunAction(r) => reply(client, &r.request_id, self.handle_run_action(&r).await),
            Request::AgentVersions(r) => reply(client, &r.request_id, self.handle_agent_versions(&r).await),
            Request::UpdateAgent(r) => reply(client, &r.request_id, self.update_agent(r.program).await),
            Request::List(r) => reply(client, &r.request_id, self.handle_list()),
            Request::Subscribe(r) => reply(client, &r.request_id, self.handle_subscribe(conn, &r).await),
            Request::ListWorktrees(r) => reply(client, &r.request_id, self.handle_list_worktrees(&r).await),
            Request::WorktreeOverview(r) => reply(client, &r.request_id, self.worktree_overview(&r.cwd, client).await),
            Request::InspectWorktree(r) => {
                let inspected = self
                    .inspect_worktree(&r.cwd, &r.path, r.delete_branch.unwrap_or(false), client, None, false)
                    .await;
                reply(client, &r.request_id, inspected.map(|found| found.inspection))
            }
            Request::DeleteWorktree(r) => reply(client, &r.request_id, self.delete_worktree(&r, client).await),
            Request::WatchPreview(r) => reply(client, &r.request_id, self.handle_watch_preview(conn, &r).await),
            Request::WatchTerminal(r) => reply(
                client,
                &r.request_id,
                self.handle_watch_pane(conn, WorkspacePane::Terminal, &r).await,
            ),
            Request::WatchGit(r) => reply(
                client,
                &r.request_id,
                self.handle_watch_pane(conn, WorkspacePane::Git, &r).await,
            ),
            Request::WatchDev(r) => reply(
                client,
                &r.request_id,
                self.handle_watch_pane(conn, WorkspacePane::Dev, &r).await,
            ),
            Request::WatchAction(r) => reply(
                client,
                &r.request_id,
                self.handle_watch_pane(conn, WorkspacePane::Action, &r).await,
            ),
            Request::WatchChanges(r) => reply(client, &r.request_id, self.handle_watch_changes(conn, &r).await),
            Request::ChangesDiff(r) => reply(client, &r.request_id, self.handle_changes_diff(&r).await),
            Request::ChangeStage(r) => reply(client, &r.request_id, self.handle_change_stage(&r).await),
            Request::StartDev(r) => reply(client, &r.request_id, self.handle_start_dev(&r).await),
            Request::StopDev(r) => reply(
                client,
                &r.request_id,
                self.handle_stop_pane(&r, WorkspacePane::Dev).await,
            ),
            Request::StopAction(r) => reply(
                client,
                &r.request_id,
                self.handle_stop_pane(&r, WorkspacePane::Action).await,
            ),
            Request::SaveNote(r) => reply(client, &r.request_id, self.save_note(&r).await),
            Request::OpenNote(r) => reply(client, &r.request_id, self.handle_open_note(&r).await),
            Request::WatchTasks(r) => reply(client, &r.request_id, self.handle_watch_tasks(conn, &r).await),
            Request::TaskOp(r) => reply(client, &r.request_id, self.handle_task_op(&r).await),
            Request::OpenTasks(r) => reply(client, &r.request_id, self.handle_open_tasks(&r).await),
            Request::SendOpenItems(r) => reply(client, &r.request_id, self.send_note_items(&r).await),
            Request::PromoteNoteItem(r) => reply(client, &r.request_id, self.promote_note_item(&r).await),
            Request::ReturnTaskToNote(r) => reply(
                client,
                &r.request_id,
                self.return_task_to_note(&r.cwd, &r.task_id).await,
            ),
            Request::TaskToNote(r) => reply(
                client,
                &r.request_id,
                self.task_to_work_note(&r.cwd, &r.task_id, &r.section).await,
            ),
            Request::ListBranches(r) => reply(client, &r.request_id, self.list_branches(&r.cwd).await),
            Request::Create(r) => reply(client, &r.request_id, self.create_session(r.input).await),
            Request::ReorderSession(r) => reply(
                client,
                &r.request_id,
                self.reorder_session(&r.session_id, r.direction).await,
            ),
            Request::Restart(r) => {
                let mode = r.mode.unwrap_or(RestartMode::Resume);
                let restarted = self
                    .restart_session(&r.session_id, r.cols, r.rows, mode, r.project_fingerprint.clone())
                    .await;
                reply(client, &r.request_id, restarted)
            }
            Request::Kill(r) => {
                let options = KillOptions {
                    delete_worktree: r.delete_worktree.unwrap_or(false),
                    delete_branch: r.delete_branch.unwrap_or(false),
                    force: r.force.unwrap_or(false),
                    allow_data_loss: r.allow_data_loss.unwrap_or(false),
                    send_open_items: r.send_open_items == Some(true),
                };
                reply(
                    client,
                    &r.request_id,
                    ok(self.kill_session(&r.session_id, options).await),
                )
            }
            Request::MergePreview(r) => reply(
                client,
                &r.request_id,
                self.merge_preview(&r.session_id, &r.target_cwd).await,
            ),
            Request::MergeWorktree(r) => {
                let merged = self
                    .merge_session_worktree(
                        &r.session_id,
                        r.mode,
                        &r.target_cwd,
                        r.commit_first.unwrap_or(false),
                        r.send_open_items == Some(true),
                    )
                    .await;
                reply(client, &r.request_id, merged)
            }
            Request::ResolveMerge(r) => reply(
                client,
                &r.request_id,
                self.resolve_merge(&r.session_id, &r.target_cwd, r.action).await,
            ),
            Request::MarkSessionMerged(r) => reply(
                client,
                &r.request_id,
                self.mark_session_merged(&r.session_id, &r.target_cwd).await,
            ),
            Request::Remove(r) => reply(client, &r.request_id, self.handle_remove(&r).await),
            // Attaches answer by themselves (the response, then `*attached` and the initial frame).
            Request::Attach(r) => self.handle_attach(conn, &r).await,
            Request::AttachTerminal(r) => {
                self.attach_workspace_pane(
                    client,
                    &r.request_id,
                    WorkspacePane::Terminal,
                    &r.session_id,
                    r.cols,
                    r.rows,
                )
                .await
            }
            Request::AttachGit(r) => {
                self.attach_workspace_pane(client, &r.request_id, WorkspacePane::Git, &r.session_id, r.cols, r.rows)
                    .await
            }
            Request::AttachDev(r) => {
                self.attach_workspace_pane(client, &r.request_id, WorkspacePane::Dev, &r.session_id, r.cols, r.rows)
                    .await
            }
            Request::AttachAction(r) => {
                self.attach_workspace_pane(
                    client,
                    &r.request_id,
                    WorkspacePane::Action,
                    &r.session_id,
                    r.cols,
                    r.rows,
                )
                .await
            }
            // No requestId: no response, and a failure is not reported.
            Request::Input(r) => self.handle_input(&r),
            Request::Resize(r) => self.handle_resize(&r),
            Request::Detach(r) => self.handle_detach(conn, &r),
            Request::TerminalInput(r) => self.handle_pane_input(WorkspacePane::Terminal, &r),
            Request::GitInput(r) => self.handle_pane_input(WorkspacePane::Git, &r),
            Request::DevInput(r) => self.handle_pane_input(WorkspacePane::Dev, &r),
            Request::ActionInput(r) => self.handle_pane_input(WorkspacePane::Action, &r),
            Request::TerminalResize(r) => self.handle_pane_resize(WorkspacePane::Terminal, &r),
            Request::GitResize(r) => self.handle_pane_resize(WorkspacePane::Git, &r),
            Request::DevResize(r) => self.handle_pane_resize(WorkspacePane::Dev, &r),
            Request::ActionResize(r) => self.handle_pane_resize(WorkspacePane::Action, &r),
            Request::TerminalDetach(r) => self.handle_pane_detach(conn, WorkspacePane::Terminal, &r),
            Request::GitDetach(r) => self.handle_pane_detach(conn, WorkspacePane::Git, &r),
            Request::DevDetach(r) => self.handle_pane_detach(conn, WorkspacePane::Dev, &r),
            Request::ActionDetach(r) => self.handle_pane_detach(conn, WorkspacePane::Action, &r),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelopes_match_the_ts_literals() {
        let message = response("r1", &OkResult::default()).unwrap();
        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            r#"{"type":"response","requestId":"r1","ok":true,"data":{"ok":true}}"#
        );
        let message = failure("r2", DhError::msg("Session does not exist"));
        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            r#"{"type":"response","requestId":"r2","ok":false,"error":"Session does not exist"}"#
        );
    }

    #[test]
    fn requests_are_answered_on_their_connection() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = crate::server::tests::test_daemon(dir.path());
        let (client, mut rx) = crate::server::tests::test_client(1);
        let conn = Connection::new(client);
        crate::server::tests::local(async {
            let ping = Request::Ping(deckhand_protocol::wire::RequestOnly {
                request_id: "p1".into(),
            });
            daemon.handle_request(&conn, ping).await;
        });
        let line = rx.try_recv().unwrap();
        let expected = format!(
            "{{\"type\":\"response\",\"requestId\":\"p1\",\"ok\":true,\"data\":{{\"ok\":true,\"version\":{},\"home\":{},\"channel\":{}}}}}\n",
            deckhand_protocol::PROTOCOL_VERSION,
            serde_json::to_string(dir.path().to_str().unwrap()).unwrap(),
            serde_json::to_string(&std::env::var("DECKHAND_CHANNEL").unwrap_or_else(|_| "stable".into())).unwrap(),
        );
        assert_eq!(line, expected);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_path_answer_is_a_json_string() {
        let message = response("r", &"/tmp/x.md".to_owned()).unwrap();
        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            r#"{"type":"response","requestId":"r","ok":true,"data":"/tmp/x.md"}"#
        );
    }
}
