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
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
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
use deckhand_protocol::wire::{
    AgentVersionsRequest, ArchiveSession, ChangeStage, ChangesDiff, Create, CreatePr, CwdRequest, DeleteWorktree,
    ExportHandoff, Input, InspectCleanup, InspectWorktree, Kill, MergeWorktree, OpenNote, PromoteNoteItem, Remove,
    ReorderSession, RequestOnly, ResolveMerge, Restart, ReturnTaskToNote, RunAction, SaveConfig, SaveNote,
    SendOpenItems, SessionRequest, SessionSizeRequest, SessionTargetRequest, SetSessionDone, Subscribe, TaskOpRequest,
    TaskToNote, TrustProject, UpdateAgent, WatchChanges, WatchPane, WatchPreview, WorkspaceSummaryRequest,
    WorktreeCandidateSizes,
};

use crate::client::{
    JsonParser, decode_data, frame_message, js_truthy, open_persistent_connection, response_error, socket_error,
};

/// Read buffer size for the daemon connection.
const READ_CHUNK: usize = 64 * 1024;

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

/// One request awaiting its response: a TS `pending` entry (`seq` keeps the Map's insertion order for `rejectAll`).
struct Pending {
    seq: u64,
    reply: oneshot::Sender<Result<Value, DhError>>,
}

/// The state the reader task and the methods share. Never borrowed across an await.
struct LiveState {
    /// TS `pending`: resolve/reject per `requestId`.
    pending: HashMap<String, Pending>,
    /// Next [`Pending::seq`].
    next_seq: u64,
    /// TS `closed`.
    closed: bool,
    /// TS `socket.destroyed`: the connection failed or ended.
    destroyed: bool,
    /// TS: the `data` listener is attached (until `stopParsing()`).
    parsing: bool,
}

/// TS: LiveClient.rejectAll: every pending request fails with `error`, in the order they were sent.
fn reject_all(state: &RefCell<LiveState>, error: &DhError) {
    let mut pending: Vec<Pending> = state.borrow_mut().pending.drain().map(|(_, entry)| entry).collect();
    pending.sort_by_key(|entry| entry.seq);
    for entry in pending {
        let _ = entry.reply.send(Err(error.clone()));
    }
}

/// TS: the socket's `error` listener (the socket is destroyed with it; a later failure is not reported again).
fn socket_failed(state: &RefCell<LiveState>, handlers: &LiveClientHandlers, error: DhError) {
    {
        let mut state = state.borrow_mut();
        if state.destroyed {
            return;
        }
        state.destroyed = true;
    }
    reject_all(state, &error);
    if let Some(on_error) = &handlers.on_error {
        on_error(error);
    }
}

/// TS: the socket's `close` listener.
fn socket_closed(state: &RefCell<LiveState>, handlers: &LiveClientHandlers) {
    {
        let mut state = state.borrow_mut();
        state.closed = true;
        state.destroyed = true;
        state.parsing = false;
    }
    reject_all(state, &DhError::msg("daemon connection closed"));
    if let Some(on_close) = &handlers.on_close {
        on_close();
    }
}

/// `message[key]` as `T` (absent reads as `null`, TS `undefined`).
fn field<T: DeserializeOwned>(message: &Value, key: &str) -> Result<T, DhError> {
    serde_json::from_value(message.get(key).cloned().unwrap_or(Value::Null))
        .map_err(|error| DhError::msg(format!("invalid {key} in daemon message: {error}")))
}

/// Calls `handler` with `message[key]` when there is a handler (TS `handler?.(message.key)`); a payload that does not
/// fit goes to `on_error`, as an exception in the TS handler would.
fn deliver<T: DeserializeOwned>(
    handlers: &LiveClientHandlers,
    handler: Option<&dyn Fn(T)>,
    message: &Value,
    key: &str,
) {
    let Some(handler) = handler else { return };
    match field::<T>(message, key) {
        Ok(value) => handler(value),
        Err(error) => {
            if let Some(on_error) = &handlers.on_error {
                on_error(error);
            }
        }
    }
}

/// TS: the LiveClient constructor's message callback.
fn dispatch(state: &RefCell<LiveState>, handlers: &LiveClientHandlers, message: &Value) {
    let kind = message.get("type").and_then(Value::as_str).unwrap_or_default();
    if kind == "response" {
        let Some(id) = message.get("requestId").and_then(Value::as_str) else {
            return;
        };
        let Some(pending) = state.borrow_mut().pending.remove(id) else {
            return;
        };
        let outcome = if js_truthy(message.get("ok")) {
            Ok(message.get("data").cloned().unwrap_or(Value::Null))
        } else {
            Err(response_error(message))
        };
        let _ = pending.reply.send(outcome);
        return;
    }
    let h = handlers;
    match kind {
        "session-updated" => deliver(h, h.on_session_updated.as_deref(), message, "session"),
        "session-removed" => deliver(h, h.on_session_removed.as_deref(), message, "sessionId"),
        "preview-updated" => deliver(h, h.on_preview_updated.as_deref(), message, "preview"),
        "terminal-updated" => deliver(h, h.on_terminal_updated.as_deref(), message, "terminal"),
        "changes-updated" => deliver(h, h.on_changes_updated.as_deref(), message, "changes"),
        "dev-updated" => deliver(h, h.on_dev_updated.as_deref(), message, "dev"),
        "action-updated" => deliver(h, h.on_action_updated.as_deref(), message, "action"),
        "agent-versions-updated" => deliver(h, h.on_agent_versions_updated.as_deref(), message, "versions"),
        "tasks-updated" => deliver(h, h.on_tasks_updated.as_deref(), message, "tasks"),
        _ => {}
    }
}

/// The reader task: TS `attachJsonParser` plus the socket's `error` and `close` listeners.
async fn read_loop(
    mut reader: OwnedReadHalf,
    state: Rc<RefCell<LiveState>>,
    handlers: Rc<LiveClientHandlers>,
    outgoing: mpsc::UnboundedSender<Vec<u8>>,
) {
    let mut parser = JsonParser::new();
    let mut buffer = vec![0u8; READ_CHUNK];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => {
                if !state.borrow().parsing {
                    continue;
                }
                let parsed = parser.push_values(buffer.get(..read).unwrap_or_default());
                // As in TS, a close() from a handler stops later chunks, not the rest of this one.
                for message in &parsed.messages {
                    dispatch(&state, &handlers, message);
                }
                if let Some(error) = parsed.error {
                    // TS: the SyntaxError escapes the data listener and takes the process down; here the connection
                    // fails instead (pending requests and on_error get the JSON.parse message).
                    socket_failed(&state, &handlers, error);
                    break;
                }
            }
            Err(error) => {
                socket_failed(&state, &handlers, socket_error(&error, "read"));
                break;
            }
        }
    }
    // allowHalfOpen is false: the end of the daemon's side ends ours too.
    let _ = outgoing.send(Vec::new());
    socket_closed(&state, &handlers);
}

/// The writer task: lines in order; an empty line is TS `socket.end()`.
async fn write_loop(
    mut writer: OwnedWriteHalf,
    mut lines: mpsc::UnboundedReceiver<Vec<u8>>,
    state: Rc<RefCell<LiveState>>,
    handlers: Rc<LiveClientHandlers>,
) {
    while let Some(line) = lines.recv().await {
        if line.is_empty() {
            let _ = writer.shutdown().await;
            break;
        }
        if state.borrow().destroyed {
            continue;
        }
        if let Err(error) = writer.write_all(&line).await {
            socket_failed(&state, &handlers, socket_error(&error, "write"));
        }
    }
}

/// A reply that already failed.
fn failed<T: 'static>(error: DhError) -> Reply<T> {
    Box::pin(async move { Err(error) })
}

fn new_request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// `...flag ? {key: true} : {}`.
fn only_true(flag: bool) -> Option<bool> {
    flag.then_some(true)
}

/// `...noteId ? {noteId} : {}` (an empty ID is left out too).
fn non_empty(text: Option<&str>) -> Option<String> {
    text.filter(|text| !text.is_empty()).map(str::to_owned)
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
    /// Shared with the tasks, which call them (the TS listeners stay registered after `close()`, for `on_close`).
    #[allow(dead_code)]
    handlers: Rc<LiveClientHandlers>,
    /// Framed lines for the writer task (TS `socket.write` is synchronous from the caller's view); an empty line ends
    /// the socket.
    outgoing: mpsc::UnboundedSender<Vec<u8>>,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for LiveClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveClient").finish_non_exhaustive()
    }
}

impl Drop for LiveClient {
    /// The connection goes with the client (TS lets the socket live on until garbage collection).
    fn drop(&mut self) {
        for task in [self.reader.take(), self.writer.take()].into_iter().flatten() {
            task.abort();
        }
    }
}

impl LiveClient {
    /// TS: LiveClient constructor: starts reading `socket` (responses settle pending requests, events go to
    /// `handlers`; a socket error rejects everything pending and reports it; close marks the client closed, rejects
    /// pending requests with `daemon connection closed` and calls `on_close`).
    pub fn new(socket: UnixStream, handlers: LiveClientHandlers) -> Self {
        let state = Rc::new(RefCell::new(LiveState {
            pending: HashMap::new(),
            next_seq: 0,
            closed: false,
            destroyed: false,
            parsing: true,
        }));
        let handlers = Rc::new(handlers);
        let (outgoing, lines) = mpsc::unbounded_channel();
        let (read_half, write_half) = socket.into_split();
        let reader = tokio::task::spawn_local(read_loop(
            read_half,
            Rc::clone(&state),
            Rc::clone(&handlers),
            outgoing.clone(),
        ));
        let writer = tokio::task::spawn_local(write_loop(write_half, lines, Rc::clone(&state), Rc::clone(&handlers)));
        LiveClient {
            state,
            handlers,
            outgoing,
            reader: Some(reader),
            writer: Some(writer),
        }
    }

    /// TS: LiveClient.rejectAll.
    fn reject_all(&self, error: DhError) {
        reject_all(&self.state, &error);
    }

    /// TS `this.closed || this.socket.destroyed`.
    fn is_closed(&self) -> bool {
        let state = self.state.borrow();
        state.closed || state.destroyed
    }

    /// TS: LiveClient.request: writes `message` now; the reply is its response's `data`. Fails at once with
    /// `daemon connection is closed` after close; a failed response rejects with its `error` (or
    /// `daemon request failed`).
    pub fn request<T: DeserializeOwned + 'static>(&self, message: Request) -> Reply<T> {
        if self.is_closed() {
            return failed(DhError::msg("daemon connection is closed"));
        }
        let Some(id) = message.request_id().map(str::to_owned) else {
            return failed(DhError::msg(format!(
                "{} messages get no response",
                message.type_name()
            )));
        };
        let line = match frame_message(&message) {
            Ok(line) => line,
            Err(error) => return failed(error),
        };
        let (reply, response) = oneshot::channel();
        {
            let mut state = self.state.borrow_mut();
            let seq = state.next_seq;
            state.next_seq += 1;
            state.pending.insert(id, Pending { seq, reply });
        }
        let _ = self.outgoing.send(line);
        Box::pin(async move {
            match response.await {
                Ok(Ok(data)) => decode_data(Some(data)),
                Ok(Err(error)) => Err(error),
                // Replaced by a request with the same ID, or the client was dropped.
                Err(_) => Err(DhError::msg("daemon connection closed")),
            }
        })
    }

    /// TS: LiveClient.subscribe: this repository's sessions, then `session-updated` / `session-removed` events.
    pub fn subscribe(&self, repo_root: &str) -> Reply<Vec<SessionRecord>> {
        self.request(Request::Subscribe(Subscribe {
            request_id: new_request_id(),
            repo_root: repo_root.to_owned(),
        }))
    }

    /// TS: LiveClient.watchPreview (`scroll_offset` defaults to 0 and is always written; no session stops watching).
    pub fn watch_preview(
        &self,
        session_id: Option<&str>,
        cols: i64,
        rows: i64,
        scroll_offset: Option<i64>,
    ) -> Reply<PreviewRecord> {
        self.request(Request::WatchPreview(WatchPreview {
            request_id: new_request_id(),
            session_id: session_id.map(str::to_owned),
            cols,
            rows,
            scroll_offset: Some(scroll_offset.unwrap_or(0)),
        }))
    }

    /// TS: LiveClient.watchTerminal.
    pub fn watch_terminal(&self, session_id: Option<&str>, cols: i64, rows: i64) -> Reply<TerminalRecord> {
        self.request(Request::WatchTerminal(WatchPane {
            request_id: new_request_id(),
            session_id: session_id.map(str::to_owned),
            cols,
            rows,
        }))
    }

    /// TS: LiveClient.watchChanges: the Git tab's Changes view; no session stops watching.
    pub fn watch_changes(&self, session_id: Option<&str>) -> Reply<ChangesRecord> {
        self.request(Request::WatchChanges(WatchChanges {
            request_id: new_request_id(),
            session_id: session_id.map(str::to_owned),
        }))
    }

    /// TS: LiveClient.changesDiff.
    pub fn changes_diff(&self, session_id: &str, group: ChangeGroup, path: &str) -> Reply<ChangeDiff> {
        self.request(Request::ChangesDiff(ChangesDiff {
            request_id: new_request_id(),
            session_id: session_id.to_owned(),
            group,
            path: path.to_owned(),
        }))
    }

    /// TS: LiveClient.changeStage: stages/unstages one entry, or everything without `entry`.
    pub fn change_stage(
        &self,
        session_id: &str,
        mode: StageMode,
        entry: Option<StageEntry>,
    ) -> Reply<ChangeStageResult> {
        let (group, path) = match entry {
            Some(entry) => (Some(entry.group), Some(entry.path)),
            None => (None, None),
        };
        self.request(Request::ChangeStage(ChangeStage {
            request_id: new_request_id(),
            session_id: session_id.to_owned(),
            mode,
            group,
            path,
        }))
    }

    /// TS: LiveClient.watchDev.
    pub fn watch_dev(&self, session_id: Option<&str>, cols: i64, rows: i64) -> Reply<DevRecord> {
        self.request(Request::WatchDev(WatchPane {
            request_id: new_request_id(),
            session_id: session_id.map(str::to_owned),
            cols,
            rows,
        }))
    }

    /// TS: LiveClient.startDev.
    pub fn start_dev(&self, session_id: &str, cols: i64, rows: i64) -> Reply<DevRecord> {
        self.request(Request::StartDev(SessionSizeRequest {
            request_id: new_request_id(),
            session_id: session_id.to_owned(),
            cols,
            rows,
        }))
    }

    /// TS: LiveClient.stopDev.
    pub fn stop_dev(&self, session_id: &str) -> Reply<()> {
        self.request(Request::StopDev(session_request(session_id)))
    }

    /// TS: LiveClient.listSessions.
    pub fn list_sessions(&self) -> Reply<Vec<SessionRecord>> {
        self.request(Request::List(RequestOnly {
            request_id: new_request_id(),
        }))
    }

    /// TS: LiveClient.createSession.
    pub fn create_session(&self, input: CreateSessionInput) -> Reply<SessionRecord> {
        self.request(Request::Create(Create {
            request_id: new_request_id(),
            input,
        }))
    }

    /// TS: LiveClient.reorderSession.
    pub fn reorder_session(&self, session_id: &str, direction: MoveDirection) -> Reply<Vec<SessionRecord>> {
        self.request(Request::ReorderSession(ReorderSession {
            request_id: new_request_id(),
            session_id: session_id.to_owned(),
            direction,
        }))
    }

    /// TS: LiveClient.listWorktrees.
    pub fn list_worktrees(&self, cwd: &str) -> Reply<Vec<WorktreeInfoRecord>> {
        self.request(Request::ListWorktrees(cwd_request(cwd)))
    }

    /// TS: LiveClient.worktreeOverview: the repository's worktrees with their sessions, merge state and what deleting
    /// each would lose.
    pub fn worktree_overview(&self, cwd: &str) -> Reply<WorktreeOverview> {
        self.request(Request::WorktreeOverview(cwd_request(cwd)))
    }

    /// TS: LiveClient.inspectWorktree.
    pub fn inspect_worktree(
        &self,
        cwd: &str,
        worktree_path: &str,
        delete_branch: bool,
    ) -> Reply<WorktreeCleanupInspection> {
        self.request(Request::InspectWorktree(InspectWorktree {
            request_id: new_request_id(),
            cwd: cwd.to_owned(),
            path: worktree_path.to_owned(),
            delete_branch: Some(delete_branch),
        }))
    }

    /// TS: LiveClient.deleteWorktree.
    pub fn delete_worktree(
        &self,
        cwd: &str,
        worktree_path: &str,
        options: DeleteWorktreeOptions,
    ) -> Reply<WorktreeDeleteResult> {
        self.request(Request::DeleteWorktree(DeleteWorktree {
            request_id: new_request_id(),
            cwd: cwd.to_owned(),
            path: worktree_path.to_owned(),
            branch: options.branch,
            delete_branch: options.delete_branch,
            stop_sessions: options.stop_sessions,
            allow_data_loss: options.allow_data_loss,
            send_open_items: options.send_open_items,
        }))
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
        self.request(Request::Restart(Restart {
            request_id: new_request_id(),
            session_id: session_id.to_owned(),
            cols,
            rows,
            mode: Some(mode.unwrap_or(RestartMode::Resume)),
            project_fingerprint: project_fingerprint.map(str::to_owned),
        }))
    }

    /// TS: LiveClient.killSession.
    pub fn kill_session(&self, session_id: &str, options: KillOptions) -> Reply<()> {
        self.request(Request::Kill(Kill {
            request_id: new_request_id(),
            session_id: session_id.to_owned(),
            delete_worktree: Some(options.delete_worktree),
            delete_branch: Some(options.delete_branch),
            force: Some(options.force),
            allow_data_loss: Some(options.allow_data_loss),
            send_open_items: only_true(options.send_open_items),
        }))
    }

    /// TS: LiveClient.mergePreview.
    pub fn merge_preview(&self, session_id: &str, target_cwd: &str) -> Reply<MergePreview> {
        self.request(Request::MergePreview(session_target(session_id, target_cwd)))
    }

    /// TS: LiveClient.mergeWorktree.
    pub fn merge_worktree(
        &self,
        session_id: &str,
        mode: WorktreeMergeMode,
        target_cwd: &str,
        options: MergeOptions,
    ) -> Reply<WorktreeMergeResult> {
        self.request(Request::MergeWorktree(MergeWorktree {
            request_id: new_request_id(),
            session_id: session_id.to_owned(),
            mode,
            target_cwd: target_cwd.to_owned(),
            commit_first: Some(options.commit_first),
            send_open_items: only_true(options.send_open_items),
        }))
    }

    /// TS: LiveClient.resolveMerge.
    pub fn resolve_merge(
        &self,
        session_id: &str,
        target_cwd: &str,
        action: ResolveMergeAction,
    ) -> Reply<SessionRecord> {
        self.request(Request::ResolveMerge(ResolveMerge {
            request_id: new_request_id(),
            session_id: session_id.to_owned(),
            target_cwd: target_cwd.to_owned(),
            action,
        }))
    }

    /// TS: LiveClient.setSessionDone.
    pub fn set_session_done(&self, session_id: &str, done: bool) -> Reply<SessionRecord> {
        self.request(Request::SetSessionDone(SetSessionDone {
            request_id: new_request_id(),
            session_id: session_id.to_owned(),
            done,
        }))
    }

    /// TS: LiveClient.markSessionMerged.
    pub fn mark_session_merged(&self, session_id: &str, target_cwd: &str) -> Reply<SessionRecord> {
        self.request(Request::MarkSessionMerged(session_target(session_id, target_cwd)))
    }

    /// TS: LiveClient.removeSession. `move_open_items` (default false, written only when true): first send the open
    /// checklist items of the notes removed with it to the repository's tasks.
    pub fn remove_session(&self, session_id: &str, move_open_items: Option<bool>) -> Reply<()> {
        self.request(Request::Remove(Remove {
            request_id: new_request_id(),
            session_id: session_id.to_owned(),
            move_open_items: only_true(move_open_items.unwrap_or(false)),
        }))
    }

    /// TS: LiveClient.watchTasks: the task list of `cwd`'s repository; `tasks-updated` follows its changes.
    pub fn watch_tasks(&self, cwd: &str) -> Reply<TasksDoc> {
        self.request(Request::WatchTasks(cwd_request(cwd)))
    }

    /// TS: LiveClient.taskOp.
    pub fn task_op(&self, cwd: &str, op: TaskOp) -> Reply<TasksDoc> {
        self.request(Request::TaskOp(TaskOpRequest {
            request_id: new_request_id(),
            cwd: cwd.to_owned(),
            op,
        }))
    }

    /// TS: LiveClient.openTasks: the tasks file's path, to open it in an editor.
    pub fn open_tasks(&self, cwd: &str) -> Reply<String> {
        self.request(Request::OpenTasks(cwd_request(cwd)))
    }

    /// TS: LiveClient.returnTaskToNote.
    pub fn return_task_to_note(&self, cwd: &str, task_id: &str) -> Reply<TasksDoc> {
        self.request(Request::ReturnTaskToNote(ReturnTaskToNote {
            request_id: new_request_id(),
            cwd: cwd.to_owned(),
            task_id: task_id.to_owned(),
        }))
    }

    /// TS: LiveClient.taskToNote: the task into the note of work `section` (which has its task already), as a step.
    pub fn task_to_note(&self, cwd: &str, task_id: &str, section: &str) -> Reply<TasksDoc> {
        self.request(Request::TaskToNote(TaskToNote {
            request_id: new_request_id(),
            cwd: cwd.to_owned(),
            task_id: task_id.to_owned(),
            section: section.to_owned(),
        }))
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
        self.request(Request::PromoteNoteItem(PromoteNoteItem {
            request_id: new_request_id(),
            session_id: session_id.to_owned(),
            section,
            line,
            revision: revision.to_owned(),
            note_id: non_empty(note_id),
        }))
    }

    /// TS: LiveClient.sendOpenItems: every open checklist item of one note (at `revision`) to Tasks.
    pub fn send_open_items(
        &self,
        session_id: &str,
        section: NoteSection,
        revision: &str,
        note_id: Option<&str>,
    ) -> Reply<SendOpenItemsResult> {
        self.request(Request::SendOpenItems(SendOpenItems {
            request_id: new_request_id(),
            session_id: session_id.to_owned(),
            section,
            revision: revision.to_owned(),
            note_id: non_empty(note_id),
        }))
    }

    /// TS: LiveClient.listBranches.
    pub fn list_branches(&self, cwd: &str) -> Reply<BranchList> {
        self.request(Request::ListBranches(cwd_request(cwd)))
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
        self.request(Request::SaveNote(SaveNote {
            request_id: new_request_id(),
            session_id: session_id.to_owned(),
            section,
            note_id: note_id.map(str::to_owned),
            text: text.to_owned(),
            revision: revision.to_owned(),
        }))
    }

    /// TS: LiveClient.openNote: the note's file (created if missing), to open it in an editor.
    pub fn open_note(&self, session_id: &str, section: NoteSection) -> Reply<String> {
        self.request(Request::OpenNote(OpenNote {
            request_id: new_request_id(),
            session_id: session_id.to_owned(),
            section,
        }))
    }

    /// TS: LiveClient.saveConfig (`revision` `null` is written as `null`).
    pub fn save_config(
        &self,
        target: ConfigTargetKind,
        cwd: &str,
        raw: &str,
        revision: Option<&str>,
    ) -> Reply<SavedConfigDocument> {
        self.request(Request::SaveConfig(SaveConfig {
            request_id: new_request_id(),
            target,
            cwd: cwd.to_owned(),
            raw: raw.to_owned(),
            revision: revision.map(str::to_owned),
        }))
    }

    /// TS: LiveClient.settingsInfo.
    pub fn settings_info(&self, cwd: &str) -> Reply<SettingsInfo> {
        self.request(Request::SettingsInfo(cwd_request(cwd)))
    }

    /// TS: LiveClient.worktreeCandidates.
    pub fn worktree_candidates(&self, cwd: &str) -> Reply<WorktreeCandidates> {
        self.request(Request::WorktreeCandidates(cwd_request(cwd)))
    }

    /// TS: LiveClient.globalLinkCandidates.
    pub fn global_link_candidates(&self, cwd: &str) -> Reply<WorktreeCandidates> {
        self.request(Request::GlobalLinkCandidates(cwd_request(cwd)))
    }

    /// TS: LiveClient.worktreeCandidateSizes.
    pub fn worktree_candidate_sizes(&self, cwd: &str, paths: &[String]) -> Reply<CandidateSizes> {
        self.request(Request::WorktreeCandidateSizes(WorktreeCandidateSizes {
            request_id: new_request_id(),
            cwd: cwd.to_owned(),
            paths: paths.to_vec(),
        }))
    }

    /// TS: LiveClient.projectInfo: the repository of `cwd` (its main checkout's deckhand.json), with trust state and
    /// effective settings.
    pub fn project_info(&self, cwd: &str) -> Reply<ProjectInfo> {
        self.request(Request::ProjectInfo(cwd_request(cwd)))
    }

    /// TS: LiveClient.trustProject.
    pub fn trust_project(&self, cwd: &str, fingerprint: &str) -> Reply<ProjectInfo> {
        self.request(Request::TrustProject(TrustProject {
            request_id: new_request_id(),
            cwd: cwd.to_owned(),
            fingerprint: fingerprint.to_owned(),
        }))
    }

    /// TS: LiveClient.workspaceSummary (`include_pr` defaults to false, always written).
    pub fn workspace_summary(&self, session_id: &str, include_pr: Option<bool>) -> Reply<WorkspaceSummary> {
        self.request(Request::WorkspaceSummary(WorkspaceSummaryRequest {
            request_id: new_request_id(),
            session_id: session_id.to_owned(),
            include_pr: Some(include_pr.unwrap_or(false)),
        }))
    }

    /// TS: LiveClient.createPr.
    pub fn create_pr(&self, session_id: &str, branch: Option<&str>) -> Reply<CreatePrResult> {
        self.request(Request::CreatePr(CreatePr {
            request_id: new_request_id(),
            session_id: session_id.to_owned(),
            branch: branch.map(str::to_owned),
        }))
    }

    /// TS: LiveClient.inspectCleanup (`delete_branch` defaults to true, always written).
    pub fn inspect_cleanup(&self, session_id: &str, delete_branch: Option<bool>) -> Reply<SessionCleanupInspection> {
        self.request(Request::InspectCleanup(InspectCleanup {
            request_id: new_request_id(),
            session_id: session_id.to_owned(),
            delete_branch: Some(delete_branch.unwrap_or(true)),
        }))
    }

    /// TS: LiveClient.archiveSession.
    pub fn archive_session(&self, session_id: &str, archived: bool) -> Reply<SessionRecord> {
        self.request(Request::ArchiveSession(ArchiveSession {
            request_id: new_request_id(),
            session_id: session_id.to_owned(),
            archived,
        }))
    }

    /// TS: LiveClient.exportHandoff: the written file's path (`include_output` defaults to false, always written).
    pub fn export_handoff(&self, session_id: &str, include_output: Option<bool>) -> Reply<String> {
        self.request(Request::ExportHandoff(ExportHandoff {
            request_id: new_request_id(),
            session_id: session_id.to_owned(),
            include_output: Some(include_output.unwrap_or(false)),
        }))
    }

    /// TS: LiveClient.runAction.
    pub fn run_action(&self, session_id: &str, action: &str, cols: i64, rows: i64) -> Reply<ActionRecord> {
        self.request(Request::RunAction(RunAction {
            request_id: new_request_id(),
            session_id: session_id.to_owned(),
            action: action.to_owned(),
            cols,
            rows,
        }))
    }

    /// TS: LiveClient.watchAction.
    pub fn watch_action(&self, session_id: Option<&str>, cols: i64, rows: i64) -> Reply<ActionRecord> {
        self.request(Request::WatchAction(WatchPane {
            request_id: new_request_id(),
            session_id: session_id.map(str::to_owned),
            cols,
            rows,
        }))
    }

    /// TS: LiveClient.stopAction.
    pub fn stop_action(&self, session_id: &str) -> Reply<()> {
        self.request(Request::StopAction(session_request(session_id)))
    }

    /// TS: LiveClient.cancelStart.
    pub fn cancel_start(&self, session_id: &str) -> Reply<()> {
        self.request(Request::CancelStart(session_request(session_id)))
    }

    /// TS: LiveClient.agentVersions: installed and latest version of every agent; `refresh` (default false, always
    /// written) looks the latest up again first (up to ~10 s).
    pub fn agent_versions(&self, refresh: Option<bool>) -> Reply<AgentVersions> {
        self.request(Request::AgentVersions(AgentVersionsRequest {
            request_id: new_request_id(),
            refresh: Some(refresh.unwrap_or(false)),
        }))
    }

    /// TS: LiveClient.updateAgent: runs the agent's own update command (may take minutes); resolves with its exit code
    /// and output.
    pub fn update_agent(&self, program: ProgramKey) -> Reply<AgentUpdateResult> {
        self.request(Request::UpdateAgent(UpdateAgent {
            request_id: new_request_id(),
            program,
        }))
    }

    /// TS: LiveClient.sendAgentInput: an `input` message (no response); dropped once closed.
    pub fn send_agent_input(&self, session_id: &str, data: &str) {
        if self.is_closed() {
            return;
        }
        let message = Request::Input(Input {
            session_id: session_id.to_owned(),
            data: data.to_owned(),
        });
        if let Ok(line) = frame_message(&message) {
            let _ = self.outgoing.send(line);
        }
    }

    /// TS: LiveClient.close: idempotent; rejects pending requests with `daemon connection closed` and ends the socket
    /// (`on_close` still follows once the socket has closed, as the TS `close` listener stays registered).
    pub fn close(&self) {
        let destroyed = {
            let mut state = self.state.borrow_mut();
            if state.closed {
                return;
            }
            state.closed = true;
            state.parsing = false;
            state.destroyed
        };
        self.reject_all(DhError::msg("daemon connection closed"));
        if !destroyed {
            let _ = self.outgoing.send(Vec::new());
        }
    }
}

fn cwd_request(cwd: &str) -> CwdRequest {
    CwdRequest {
        request_id: new_request_id(),
        cwd: cwd.to_owned(),
    }
}

fn session_request(session_id: &str) -> SessionRequest {
    SessionRequest {
        request_id: new_request_id(),
        session_id: session_id.to_owned(),
    }
}

fn session_target(session_id: &str, target_cwd: &str) -> SessionTargetRequest {
    SessionTargetRequest {
        request_id: new_request_id(),
        session_id: session_id.to_owned(),
        target_cwd: target_cwd.to_owned(),
    }
}

/// TS: client.ts createLiveClient (exported): [`crate::client::open_persistent_connection`], then a [`LiveClient`].
pub async fn create_live_client(handlers: LiveClientHandlers) -> Result<LiveClient, DhError> {
    let socket = open_persistent_connection().await?;
    Ok(LiveClient::new(socket, handlers))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, BufReader};

    use super::*;

    type Log = Rc<RefCell<Vec<String>>>;

    /// Handlers that record what they were called with.
    fn recording(log: &Log) -> LiveClientHandlers {
        let removed = Rc::clone(log);
        let error = Rc::clone(log);
        let close = Rc::clone(log);
        let preview = Rc::clone(log);
        LiveClientHandlers {
            on_session_removed: Some(Box::new(move |id| removed.borrow_mut().push(format!("removed {id}")))),
            on_preview_updated: Some(Box::new(move |_| preview.borrow_mut().push(String::from("preview")))),
            on_error: Some(Box::new(move |e| error.borrow_mut().push(format!("error {e}")))),
            on_close: Some(Box::new(move || close.borrow_mut().push(String::from("close")))),
            ..LiveClientHandlers::default()
        }
    }

    struct Daemon {
        lines: BufReader<OwnedReadHalf>,
        writer: OwnedWriteHalf,
    }

    impl Daemon {
        async fn next(&mut self) -> Option<Value> {
            let mut line = String::new();
            if self.lines.read_line(&mut line).await.unwrap() == 0 {
                return None;
            }
            Some(serde_json::from_str(&line).unwrap())
        }

        async fn next_line(&mut self) -> String {
            let mut line = String::new();
            self.lines.read_line(&mut line).await.unwrap();
            line
        }

        async fn send(&mut self, text: &str) {
            self.writer.write_all(text.as_bytes()).await.unwrap();
        }

        async fn respond(&mut self, request: &Value, body: Value) {
            let mut message = json!({"type": "response", "requestId": request["requestId"]});
            if let (Some(message), Some(body)) = (message.as_object_mut(), body.as_object()) {
                message.extend(body.clone());
            }
            self.send(&format!("{message}\n")).await;
        }
    }

    fn connect(handlers: LiveClientHandlers) -> (LiveClient, Daemon) {
        let (client, server) = UnixStream::pair().unwrap();
        let (read, writer) = server.into_split();
        (
            LiveClient::new(client, handlers),
            Daemon {
                lines: BufReader::new(read),
                writer,
            },
        )
    }

    async fn settle() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }

    fn run(test: impl std::future::Future<Output = ()>) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        tokio::task::LocalSet::new().block_on(&runtime, test);
    }

    #[test]
    fn responses_settle_their_own_requests_and_events_reach_the_handlers() {
        run(async {
            let log = Log::default();
            let (client, mut daemon) = connect(recording(&log));
            let first = client.open_tasks("/repo");
            let second = client.export_handoff("s1", None);
            let a = daemon.next().await.unwrap();
            let b = daemon.next().await.unwrap();
            assert_eq!(a["type"], "open-tasks");
            assert_eq!(b["type"], "export-handoff");
            daemon
                .send("{\"type\":\"session-removed\",\"sessionId\":\"s9\"}\n{\"type\":\"git-updated\",\"git\":{}}\n")
                .await;
            daemon.respond(&b, json!({"ok": true, "data": "/handoff.md"})).await;
            daemon.respond(&a, json!({"ok": true, "data": "/repo/tasks.md"})).await;
            assert_eq!(second.await.unwrap(), "/handoff.md");
            assert_eq!(first.await.unwrap(), "/repo/tasks.md");
            assert_eq!(*log.borrow(), vec!["removed s9"]);
        });
    }

    #[test]
    fn lines_serde_json_rejects_but_json_parse_accepts_keep_the_connection() {
        run(async {
            let log = Log::default();
            let (client, mut daemon) = connect(recording(&log));
            let reply = client.open_tasks("/repo");
            let request = daemon.next().await.unwrap();
            // A title cut through an emoji by UTF-16 index, a number beyond f64, nesting deeper than 128.
            daemon
                .send(&format!(
                    "{{\"type\":\"session-removed\",\"sessionId\":\"ab\\ud83d\"}}\n{{\"type\":\"git-updated\",\"n\":1e400,\"git\":{}{}}}\n",
                    "[".repeat(200),
                    "]".repeat(200)
                ))
                .await;
            daemon
                .respond(&request, json!({"ok": true, "data": "/repo/tasks.md"}))
                .await;
            assert_eq!(reply.await.unwrap(), "/repo/tasks.md");
            assert_eq!(*log.borrow(), vec!["removed ab\u{fffd}"]);
        });
    }

    #[test]
    fn failed_responses_reject_with_their_error() {
        run(async {
            let (client, mut daemon) = connect(LiveClientHandlers::default());
            let reply = client.stop_dev("s1");
            let fallback = client.cancel_start("s1");
            let request = daemon.next().await.unwrap();
            daemon
                .respond(&request, json!({"ok": false, "error": "no dev server"}))
                .await;
            let request = daemon.next().await.unwrap();
            daemon.respond(&request, json!({"ok": false})).await;
            assert_eq!(reply.await.unwrap_err().to_string(), "no dev server");
            assert_eq!(fallback.await.unwrap_err().to_string(), "daemon request failed");
        });
    }

    #[test]
    fn void_requests_ignore_the_response_data() {
        run(async {
            let (client, mut daemon) = connect(LiveClientHandlers::default());
            let reply = client.kill_session("s1", KillOptions::default());
            let request = daemon.next().await.unwrap();
            daemon
                .respond(&request, json!({"ok": true, "data": {"ok": true}}))
                .await;
            reply.await.unwrap();
        });
    }

    #[test]
    fn requests_are_written_with_the_ts_keys_and_defaults() {
        run(async {
            let (client, mut daemon) = connect(LiveClientHandlers::default());
            let id = |line: &str| -> String {
                serde_json::from_str::<Value>(line).unwrap()["requestId"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            };
            drop(client.kill_session("s1", KillOptions::default()));
            let line = daemon.next_line().await;
            assert_eq!(
                line,
                format!(
                    "{{\"type\":\"kill\",\"requestId\":\"{}\",\"sessionId\":\"s1\",\"deleteWorktree\":false,\
                     \"deleteBranch\":false,\"force\":false,\"allowDataLoss\":false}}\n",
                    id(&line)
                )
            );
            drop(client.watch_preview(None, 80, 24, None));
            let line = daemon.next_line().await;
            assert_eq!(
                line,
                format!(
                    "{{\"type\":\"watch-preview\",\"requestId\":\"{}\",\"cols\":80,\"rows\":24,\"scrollOffset\":0}}\n",
                    id(&line)
                )
            );
            drop(client.merge_worktree(
                "s1",
                WorktreeMergeMode::Merge,
                "/repo",
                MergeOptions {
                    commit_first: false,
                    send_open_items: true,
                },
            ));
            let line = daemon.next_line().await;
            assert_eq!(
                line,
                format!(
                    "{{\"type\":\"merge-worktree\",\"requestId\":\"{}\",\"sessionId\":\"s1\",\"mode\":\"merge\",\
                     \"targetCwd\":\"/repo\",\"commitFirst\":false,\"sendOpenItems\":true}}\n",
                    id(&line)
                )
            );
            drop(client.send_open_items("s1", NoteSection::Session, "rev", Some("")));
            let line = daemon.next_line().await;
            assert!(!line.contains("noteId"), "{line}");
            drop(client.save_config(ConfigTargetKind::Global, "/repo", "{}", None));
            let line = daemon.next_line().await;
            assert!(line.ends_with(",\"revision\":null}\n"), "{line}");
            client.send_agent_input("s1", "ls\r");
            assert_eq!(
                daemon.next_line().await,
                "{\"type\":\"input\",\"sessionId\":\"s1\",\"data\":\"ls\\r\"}\n"
            );
        });
    }

    #[test]
    fn close_rejects_pending_requests_and_refuses_new_ones() {
        run(async {
            let log = Log::default();
            let (client, mut daemon) = connect(recording(&log));
            let pending = client.list_sessions();
            daemon.next().await.unwrap();
            client.close();
            client.close();
            assert_eq!(pending.await.unwrap_err().to_string(), "daemon connection closed");
            assert_eq!(
                client.list_sessions().await.unwrap_err().to_string(),
                "daemon connection is closed"
            );
            client.send_agent_input("s1", "x");
            // The socket was ended: the daemon reads EOF, and on_close follows once it closes its side.
            assert!(daemon.next().await.is_none());
            assert!(log.borrow().is_empty());
            drop(daemon);
            settle().await;
            assert_eq!(*log.borrow(), vec!["close"]);
        });
    }

    #[test]
    fn the_daemon_closing_rejects_pending_requests() {
        run(async {
            let log = Log::default();
            let (client, mut daemon) = connect(recording(&log));
            let pending = client.list_sessions();
            daemon.next().await.unwrap();
            drop(daemon);
            assert_eq!(pending.await.unwrap_err().to_string(), "daemon connection closed");
            settle().await;
            assert_eq!(*log.borrow(), vec!["close"]);
            assert_eq!(
                client.list_sessions().await.unwrap_err().to_string(),
                "daemon connection is closed"
            );
        });
    }

    #[test]
    fn malformed_messages_fail_the_connection_or_reach_on_error() {
        run(async {
            let log = Log::default();
            let (client, mut daemon) = connect(recording(&log));
            let pending = client.list_sessions();
            daemon.next().await.unwrap();
            daemon.send("{\"type\":\"preview-updated\",\"preview\":5}\n").await;
            settle().await;
            assert_eq!(log.borrow().len(), 1);
            assert!(
                log.borrow()[0].starts_with("error invalid preview in daemon message"),
                "{:?}",
                log.borrow()
            );
            daemon.send("not json\n").await;
            assert_eq!(
                pending.await.unwrap_err().to_string(),
                "Unexpected token 'o', \"not json\" is not valid JSON"
            );
            settle().await;
            assert_eq!(
                log.borrow()[1..],
                [
                    String::from("error Unexpected token 'o', \"not json\" is not valid JSON"),
                    String::from("close")
                ]
            );
        });
    }
}
