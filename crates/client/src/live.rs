//! Port of client.ts (LiveClient subscription).
//!
//! The UI's long-lived connection: requests are matched to responses by `requestId` (many may be in flight), and the
//! daemon's push events go to [`LiveClientHandlers`]. As in TS, a request is written when the method is called, not
//! when its reply is awaited, so calls keep their order on the wire; the returned [`Reply`] resolves later. Must be
//! created inside the process's `LocalSet` (the reader and writer are local tasks).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use futures::future::LocalBoxFuture;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use deckhand_core::error::DhError;
use deckhand_protocol::Request;
use deckhand_protocol::data::{
    ActionRecord, AgentUpdateResult, AgentVersions, BranchList, CandidateSizes, ChangeDiff, ChangeGroup,
    ChangeStageResult, ChangesRecord, ConfigTargetKind, CreatePrResult, CreateSessionInput, DevRecord, MergePreview,
    MoveDirection, NoteSaveResult, NoteSection, PreviewRecord, ProgramKey, ProjectInfo, PromoteNoteItemResult,
    ResolveMergeAction, RestartMode, SavedConfigDocument, SendOpenItemsResult, SessionCleanupInspection, SessionRecord,
    SettingsInfo, StageMode, TaskOp, TasksDoc, TerminalRecord, WorkspaceSummary, WorktreeCandidates,
    WorktreeCleanupInspection, WorktreeDeleteResult, WorktreeInfoRecord, WorktreeMergeMode, WorktreeMergeResult,
    WorktreeOverview,
};

/// A request's eventual `data` (or its error). The request was already written when this was returned.
pub type Reply<T> = LocalBoxFuture<'static, Result<T, DhError>>;

/// TS: client.ts LiveClientHandlers: each optional, called for the matching push event (`git-updated` and the attach
/// events are not delivered, as in TS).
#[derive(Default)]
pub struct LiveClientHandlers {
    /// `session-updated`.
    pub on_session_updated: Option<Box<dyn Fn(SessionRecord)>>,
    /// `session-removed` (the session ID).
    pub on_session_removed: Option<Box<dyn Fn(String)>>,
    /// `preview-updated`.
    pub on_preview_updated: Option<Box<dyn Fn(PreviewRecord)>>,
    /// `terminal-updated`.
    pub on_terminal_updated: Option<Box<dyn Fn(TerminalRecord)>>,
    /// `changes-updated`.
    pub on_changes_updated: Option<Box<dyn Fn(ChangesRecord)>>,
    /// `dev-updated`.
    pub on_dev_updated: Option<Box<dyn Fn(DevRecord)>>,
    /// `action-updated`.
    pub on_action_updated: Option<Box<dyn Fn(ActionRecord)>>,
    /// `agent-versions-updated`.
    pub on_agent_versions_updated: Option<Box<dyn Fn(AgentVersions)>>,
    /// `tasks-updated`.
    pub on_tasks_updated: Option<Box<dyn Fn(TasksDoc)>>,
    /// A socket error, or a message that could not be handled.
    pub on_error: Option<Box<dyn Fn(DhError)>>,
    /// The connection closed (after every pending request was rejected with `daemon connection closed`).
    pub on_close: Option<Box<dyn Fn()>>,
}

impl std::fmt::Debug for LiveClientHandlers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveClientHandlers").finish_non_exhaustive()
    }
}

/// The state the reader task and the methods share. Never borrowed across an await.
#[allow(dead_code)]
struct LiveState {
    /// TS `pending`: resolve/reject per `requestId`.
    pending: HashMap<String, oneshot::Sender<Result<Value, DhError>>>,
    /// TS `closed`.
    closed: bool,
}

/// One entry of [`LiveClient::change_stage`]: TS `entry?: {group, path}` (absent stages/unstages everything).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageEntry {
    pub group: ChangeGroup,
    pub path: String,
}

/// TS: client.ts LiveClient.deleteWorktree `options`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeleteWorktreeOptions {
    pub branch: Option<String>,
    pub delete_branch: Option<bool>,
    pub stop_sessions: Option<bool>,
    pub allow_data_loss: Option<bool>,
    pub send_open_items: Option<bool>,
}

/// TS: client.ts LiveClient.killSession's defaulted flags (all `false` by default; `sendOpenItems` is written only
/// when true, the others always).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KillOptions {
    pub delete_worktree: bool,
    pub delete_branch: bool,
    pub force: bool,
    pub allow_data_loss: bool,
    /// With `delete_worktree`: first send the worktree note's open checklist items to the backlog.
    pub send_open_items: bool,
}

/// TS: client.ts LiveClient.mergeWorktree's defaulted flags (`commitFirst` always written, `sendOpenItems` only when
/// true).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MergeOptions {
    pub commit_first: bool,
    /// After a clean merge, the worktree note's open checklist items go to the backlog.
    pub send_open_items: bool,
}

/// TS: client.ts LiveClient.
pub struct LiveClient {
    state: Rc<RefCell<LiveState>>,
    handlers: Rc<LiveClientHandlers>,
    /// Framed lines for the writer task (TS `socket.write` is synchronous from the caller's view).
    outgoing: mpsc::UnboundedSender<Vec<u8>>,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for LiveClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveClient").finish_non_exhaustive()
    }
}

impl LiveClient {
    /// TS: LiveClient constructor: starts reading `socket` (responses settle pending requests, events go to
    /// `handlers`; a socket error rejects everything pending and reports it; close marks the client closed, rejects
    /// pending requests with `daemon connection closed` and calls `on_close`).
    pub fn new(socket: UnixStream, handlers: LiveClientHandlers) -> Self {
        let _ = (socket, handlers);
        todo!("K1")
    }

    /// TS: LiveClient.rejectAll.
    #[allow(dead_code)]
    fn reject_all(&self, error: DhError) {
        let _ = (
            error,
            &self.state,
            &self.handlers,
            &self.outgoing,
            &self.reader,
            &self.writer,
        );
        todo!("K1")
    }

    /// TS: LiveClient.request: writes `message` now; the reply is its response's `data`. Fails at once with
    /// `daemon connection is closed` after close; a failed response rejects with its `error` (or
    /// `daemon request failed`).
    pub fn request<T: DeserializeOwned + 'static>(&self, message: Request) -> Reply<T> {
        let _ = message;
        todo!("K1")
    }

    /// TS: LiveClient.subscribe: this repository's sessions, then `session-updated` / `session-removed` events.
    pub fn subscribe(&self, repo_root: &str) -> Reply<Vec<SessionRecord>> {
        let _ = repo_root;
        todo!("K1")
    }

    /// TS: LiveClient.watchPreview (`scroll_offset` defaults to 0 and is always written; no session stops watching).
    pub fn watch_preview(
        &self,
        session_id: Option<&str>,
        cols: i64,
        rows: i64,
        scroll_offset: Option<i64>,
    ) -> Reply<PreviewRecord> {
        let _ = (session_id, cols, rows, scroll_offset);
        todo!("K1")
    }

    /// TS: LiveClient.watchTerminal.
    pub fn watch_terminal(&self, session_id: Option<&str>, cols: i64, rows: i64) -> Reply<TerminalRecord> {
        let _ = (session_id, cols, rows);
        todo!("K1")
    }

    /// TS: LiveClient.watchChanges: the Git tab's Changes view; no session stops watching.
    pub fn watch_changes(&self, session_id: Option<&str>) -> Reply<ChangesRecord> {
        let _ = session_id;
        todo!("K1")
    }

    /// TS: LiveClient.changesDiff.
    pub fn changes_diff(&self, session_id: &str, group: ChangeGroup, path: &str) -> Reply<ChangeDiff> {
        let _ = (session_id, group, path);
        todo!("K1")
    }

    /// TS: LiveClient.changeStage: stages/unstages one entry, or everything without `entry`.
    pub fn change_stage(
        &self,
        session_id: &str,
        mode: StageMode,
        entry: Option<StageEntry>,
    ) -> Reply<ChangeStageResult> {
        let _ = (session_id, mode, entry);
        todo!("K1")
    }

    /// TS: LiveClient.watchDev.
    pub fn watch_dev(&self, session_id: Option<&str>, cols: i64, rows: i64) -> Reply<DevRecord> {
        let _ = (session_id, cols, rows);
        todo!("K1")
    }

    /// TS: LiveClient.startDev.
    pub fn start_dev(&self, session_id: &str, cols: i64, rows: i64) -> Reply<DevRecord> {
        let _ = (session_id, cols, rows);
        todo!("K1")
    }

    /// TS: LiveClient.stopDev.
    pub fn stop_dev(&self, session_id: &str) -> Reply<()> {
        let _ = session_id;
        todo!("K1")
    }

    /// TS: LiveClient.listSessions.
    pub fn list_sessions(&self) -> Reply<Vec<SessionRecord>> {
        todo!("K1")
    }

    /// TS: LiveClient.createSession.
    pub fn create_session(&self, input: CreateSessionInput) -> Reply<SessionRecord> {
        let _ = input;
        todo!("K1")
    }

    /// TS: LiveClient.reorderSession.
    pub fn reorder_session(&self, session_id: &str, direction: MoveDirection) -> Reply<Vec<SessionRecord>> {
        let _ = (session_id, direction);
        todo!("K1")
    }

    /// TS: LiveClient.listWorktrees.
    pub fn list_worktrees(&self, cwd: &str) -> Reply<Vec<WorktreeInfoRecord>> {
        let _ = cwd;
        todo!("K1")
    }

    /// TS: LiveClient.worktreeOverview: the repository's worktrees with their sessions, merge state and what deleting
    /// each would lose.
    pub fn worktree_overview(&self, cwd: &str) -> Reply<WorktreeOverview> {
        let _ = cwd;
        todo!("K1")
    }

    /// TS: LiveClient.inspectWorktree.
    pub fn inspect_worktree(
        &self,
        cwd: &str,
        worktree_path: &str,
        delete_branch: bool,
    ) -> Reply<WorktreeCleanupInspection> {
        let _ = (cwd, worktree_path, delete_branch);
        todo!("K1")
    }

    /// TS: LiveClient.deleteWorktree.
    pub fn delete_worktree(
        &self,
        cwd: &str,
        worktree_path: &str,
        options: DeleteWorktreeOptions,
    ) -> Reply<WorktreeDeleteResult> {
        let _ = (cwd, worktree_path, options);
        todo!("K1")
    }

    /// TS: LiveClient.restartSession (`mode` defaults to `resume` and is always written).
    pub fn restart_session(
        &self,
        session_id: &str,
        cols: i64,
        rows: i64,
        mode: Option<RestartMode>,
        project_fingerprint: Option<&str>,
    ) -> Reply<SessionRecord> {
        let _ = (session_id, cols, rows, mode, project_fingerprint);
        todo!("K1")
    }

    /// TS: LiveClient.killSession.
    pub fn kill_session(&self, session_id: &str, options: KillOptions) -> Reply<()> {
        let _ = (session_id, options);
        todo!("K1")
    }

    /// TS: LiveClient.mergePreview.
    pub fn merge_preview(&self, session_id: &str, target_cwd: &str) -> Reply<MergePreview> {
        let _ = (session_id, target_cwd);
        todo!("K1")
    }

    /// TS: LiveClient.mergeWorktree.
    pub fn merge_worktree(
        &self,
        session_id: &str,
        mode: WorktreeMergeMode,
        target_cwd: &str,
        options: MergeOptions,
    ) -> Reply<WorktreeMergeResult> {
        let _ = (session_id, mode, target_cwd, options);
        todo!("K1")
    }

    /// TS: LiveClient.resolveMerge.
    pub fn resolve_merge(
        &self,
        session_id: &str,
        target_cwd: &str,
        action: ResolveMergeAction,
    ) -> Reply<SessionRecord> {
        let _ = (session_id, target_cwd, action);
        todo!("K1")
    }

    /// TS: LiveClient.setSessionDone.
    pub fn set_session_done(&self, session_id: &str, done: bool) -> Reply<SessionRecord> {
        let _ = (session_id, done);
        todo!("K1")
    }

    /// TS: LiveClient.markSessionMerged.
    pub fn mark_session_merged(&self, session_id: &str, target_cwd: &str) -> Reply<SessionRecord> {
        let _ = (session_id, target_cwd);
        todo!("K1")
    }

    /// TS: LiveClient.removeSession. `move_open_items` (default false, written only when true): first send the open
    /// checklist items of the notes removed with it to the repository's tasks.
    pub fn remove_session(&self, session_id: &str, move_open_items: Option<bool>) -> Reply<()> {
        let _ = (session_id, move_open_items);
        todo!("K1")
    }

    /// TS: LiveClient.watchTasks: the task list of `cwd`'s repository; `tasks-updated` follows its changes.
    pub fn watch_tasks(&self, cwd: &str) -> Reply<TasksDoc> {
        let _ = cwd;
        todo!("K1")
    }

    /// TS: LiveClient.taskOp.
    pub fn task_op(&self, cwd: &str, op: TaskOp) -> Reply<TasksDoc> {
        let _ = (cwd, op);
        todo!("K1")
    }

    /// TS: LiveClient.openTasks: the tasks file's path, to open it in an editor.
    pub fn open_tasks(&self, cwd: &str) -> Reply<String> {
        let _ = cwd;
        todo!("K1")
    }

    /// TS: LiveClient.returnTaskToNote.
    pub fn return_task_to_note(&self, cwd: &str, task_id: &str) -> Reply<TasksDoc> {
        let _ = (cwd, task_id);
        todo!("K1")
    }

    /// TS: LiveClient.taskToNote: the task into the note of work `section` (which has its task already), as a step.
    pub fn task_to_note(&self, cwd: &str, task_id: &str, section: &str) -> Reply<TasksDoc> {
        let _ = (cwd, task_id, section);
        todo!("K1")
    }

    /// TS: LiveClient.promoteNoteItem (`noteId` written only when given).
    pub fn promote_note_item(
        &self,
        session_id: &str,
        section: NoteSection,
        line: i64,
        revision: &str,
        note_id: Option<&str>,
    ) -> Reply<PromoteNoteItemResult> {
        let _ = (session_id, section, line, revision, note_id);
        todo!("K1")
    }

    /// TS: LiveClient.sendOpenItems: every open checklist item of one note (at `revision`) to Tasks.
    pub fn send_open_items(
        &self,
        session_id: &str,
        section: NoteSection,
        revision: &str,
        note_id: Option<&str>,
    ) -> Reply<SendOpenItemsResult> {
        let _ = (session_id, section, revision, note_id);
        todo!("K1")
    }

    /// TS: LiveClient.listBranches.
    pub fn list_branches(&self, cwd: &str) -> Reply<BranchList> {
        let _ = cwd;
        todo!("K1")
    }

    /// TS: LiveClient.saveNote: saves if the file still has `revision`; `saved` false (and the file as it is in
    /// `session`) when it changed.
    pub fn save_note(
        &self,
        session_id: &str,
        section: NoteSection,
        text: &str,
        revision: &str,
        note_id: Option<&str>,
    ) -> Reply<NoteSaveResult> {
        let _ = (session_id, section, text, revision, note_id);
        todo!("K1")
    }

    /// TS: LiveClient.openNote: the note's file (created if missing), to open it in an editor.
    pub fn open_note(&self, session_id: &str, section: NoteSection) -> Reply<String> {
        let _ = (session_id, section);
        todo!("K1")
    }

    /// TS: LiveClient.saveConfig (`revision` `null` is written as `null`).
    pub fn save_config(
        &self,
        target: ConfigTargetKind,
        cwd: &str,
        raw: &str,
        revision: Option<&str>,
    ) -> Reply<SavedConfigDocument> {
        let _ = (target, cwd, raw, revision);
        todo!("K1")
    }

    /// TS: LiveClient.settingsInfo.
    pub fn settings_info(&self, cwd: &str) -> Reply<SettingsInfo> {
        let _ = cwd;
        todo!("K1")
    }

    /// TS: LiveClient.worktreeCandidates.
    pub fn worktree_candidates(&self, cwd: &str) -> Reply<WorktreeCandidates> {
        let _ = cwd;
        todo!("K1")
    }

    /// TS: LiveClient.globalLinkCandidates.
    pub fn global_link_candidates(&self, cwd: &str) -> Reply<WorktreeCandidates> {
        let _ = cwd;
        todo!("K1")
    }

    /// TS: LiveClient.worktreeCandidateSizes.
    pub fn worktree_candidate_sizes(&self, cwd: &str, paths: &[String]) -> Reply<CandidateSizes> {
        let _ = (cwd, paths);
        todo!("K1")
    }

    /// TS: LiveClient.projectInfo: the repository of `cwd` (its main checkout's deckhand.json), with trust state and
    /// effective settings.
    pub fn project_info(&self, cwd: &str) -> Reply<ProjectInfo> {
        let _ = cwd;
        todo!("K1")
    }

    /// TS: LiveClient.trustProject.
    pub fn trust_project(&self, cwd: &str, fingerprint: &str) -> Reply<ProjectInfo> {
        let _ = (cwd, fingerprint);
        todo!("K1")
    }

    /// TS: LiveClient.workspaceSummary (`include_pr` defaults to false, always written).
    pub fn workspace_summary(&self, session_id: &str, include_pr: Option<bool>) -> Reply<WorkspaceSummary> {
        let _ = (session_id, include_pr);
        todo!("K1")
    }

    /// TS: LiveClient.createPr.
    pub fn create_pr(&self, session_id: &str, branch: Option<&str>) -> Reply<CreatePrResult> {
        let _ = (session_id, branch);
        todo!("K1")
    }

    /// TS: LiveClient.inspectCleanup (`delete_branch` defaults to true, always written).
    pub fn inspect_cleanup(&self, session_id: &str, delete_branch: Option<bool>) -> Reply<SessionCleanupInspection> {
        let _ = (session_id, delete_branch);
        todo!("K1")
    }

    /// TS: LiveClient.archiveSession.
    pub fn archive_session(&self, session_id: &str, archived: bool) -> Reply<SessionRecord> {
        let _ = (session_id, archived);
        todo!("K1")
    }

    /// TS: LiveClient.exportHandoff: the written file's path (`include_output` defaults to false, always written).
    pub fn export_handoff(&self, session_id: &str, include_output: Option<bool>) -> Reply<String> {
        let _ = (session_id, include_output);
        todo!("K1")
    }

    /// TS: LiveClient.runAction.
    pub fn run_action(&self, session_id: &str, action: &str, cols: i64, rows: i64) -> Reply<ActionRecord> {
        let _ = (session_id, action, cols, rows);
        todo!("K1")
    }

    /// TS: LiveClient.watchAction.
    pub fn watch_action(&self, session_id: Option<&str>, cols: i64, rows: i64) -> Reply<ActionRecord> {
        let _ = (session_id, cols, rows);
        todo!("K1")
    }

    /// TS: LiveClient.stopAction.
    pub fn stop_action(&self, session_id: &str) -> Reply<()> {
        let _ = session_id;
        todo!("K1")
    }

    /// TS: LiveClient.cancelStart.
    pub fn cancel_start(&self, session_id: &str) -> Reply<()> {
        let _ = session_id;
        todo!("K1")
    }

    /// TS: LiveClient.agentVersions: installed and latest version of every agent; `refresh` (default false, always
    /// written) looks the latest up again first (up to ~10 s).
    pub fn agent_versions(&self, refresh: Option<bool>) -> Reply<AgentVersions> {
        let _ = refresh;
        todo!("K1")
    }

    /// TS: LiveClient.updateAgent: runs the agent's own update command (may take minutes); resolves with its exit code
    /// and output.
    pub fn update_agent(&self, program: ProgramKey) -> Reply<AgentUpdateResult> {
        let _ = program;
        todo!("K1")
    }

    /// TS: LiveClient.sendAgentInput: an `input` message (no response); dropped once closed.
    pub fn send_agent_input(&self, session_id: &str, data: &str) {
        let _ = (session_id, data);
        todo!("K1")
    }

    /// TS: LiveClient.close: idempotent; rejects pending requests with `daemon connection closed` and ends the socket
    /// (`on_close` still follows once the socket has closed, as the TS `close` listener stays registered).
    pub fn close(&self) {
        todo!("K1")
    }
}

/// TS: client.ts createLiveClient (exported): [`crate::client::open_persistent_connection`], then a [`LiveClient`].
pub async fn create_live_client(handlers: LiveClientHandlers) -> Result<LiveClient, DhError> {
    let _ = handlers;
    todo!("K1")
}
