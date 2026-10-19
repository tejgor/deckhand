//! Port of daemon.ts (workspace workers, panes, actions).
//!
//! Every session in one worktree (its workspace, workspace.ts workspaceKey) shares one Terminal, Git (lazygit), Dev
//! and action pane, hosted by one workspace worker started on demand. Requests name a session; the daemon resolves its
//! workspace and stamps each viewer's session ID on the shared pane records. Terminal and Git start on first view or
//! attach; Dev and actions only when started. A worker with nothing left in it is retired.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use deckhand_core::error::Result;
use deckhand_protocol::Ordered;
use deckhand_protocol::data::{OkResult, PaneRecord, SessionRecord};
use deckhand_protocol::events::ServerMessage;
use deckhand_protocol::wire::{Detach, Input, Resize, RunAction, SessionRequest, SessionSizeRequest, WatchPane};
use deckhand_protocol::worker_ipc::WorkerMessage;
use deckhand_sys::async_util::SharedTask;
use indexmap::IndexMap;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::daemon::Daemon;
use crate::server::{ClientHandle, Connection};
use crate::worker_channel::{ChannelEvents, WorkerChannel};

/// TS: daemon.ts WORKSPACE_SHUTDOWN_TIMEOUT_MS (retireWorkspace SIGKILLs a worker still alive this long after
/// `shutdown`).
pub const WORKSPACE_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(3000);

/// TS: daemon.ts WorkspacePane: the panes a workspace worker hosts (`action` is the last action run, shown on the
/// Terminal tab).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WorkspacePane {
    #[serde(rename = "terminal")]
    Terminal,
    #[serde(rename = "git")]
    Git,
    #[serde(rename = "dev")]
    Dev,
    #[serde(rename = "action")]
    Action,
}

impl WorkspacePane {
    /// The wire name (`terminal`, `git`, `dev`, `action`): the `target` of worker commands and the prefix of the
    /// pane's client messages (`<pane>-output`, `<pane>-attached`, ...).
    pub fn as_str(self) -> &'static str {
        todo!("D6")
    }

    /// TS: daemon.ts PANE_LABELS (`Terminal`, `Git`, `Dev`, `Actions`).
    pub fn label(self) -> &'static str {
        todo!("D6")
    }

    /// TS: daemon.ts PANE_ATTACH_NAMES (`terminal`, `git`, `dev command`, `action`).
    pub fn attach_name(self) -> &'static str {
        todo!("D6")
    }
}

/// TS: daemon.ts WorkspacePaneRecord (= ActionRecord): one record shape serves every pane. Key order follows its
/// history (the worker's literal, then the daemon's `sessionId`/`workspace` stamp).
pub type WorkspacePaneRecord = Ordered<PaneRecord>;

/// TS: WorkspaceRuntime `attached` value: the client attached to a pane, under the session it attached through.
#[derive(Clone, Debug)]
pub struct PaneViewer {
    pub client: ClientHandle,
    pub session_id: String,
}

/// TS: daemon.ts WorkspaceRuntime: the workspace worker of one workspace. Identity: `Rc::ptr_eq`.
pub struct WorkspaceRuntime {
    /// workspace.ts workspaceKey (the worktree root).
    pub key: String,
    pub channel: WorkerChannel,
    /// One attach per pane.
    pub attached: RefCell<IndexMap<WorkspacePane, PaneViewer>>,
    /// The last record each pane reported; `dev.live` drives every workspace session's devRunning.
    pub records: RefCell<IndexMap<WorkspacePane, WorkspacePaneRecord>>,
    /// Pane requests in flight; an idle worker is retired only when none is pending.
    pub busy: Cell<u32>,
}

/// The workspaces' part of the daemon state.
#[derive(Default)]
pub struct WorkspacesState {
    /// TS `workspaces`: running workspace workers by workspace key.
    pub workspaces: IndexMap<String, Rc<WorkspaceRuntime>>,
    /// TS `workspaceStarts`: workers being started (every caller awaits the same start).
    pub starts: HashMap<String, SharedTask<Result<Rc<WorkspaceRuntime>>>>,
}

/// TS: daemon.ts paneUpdatedMessage (`terminal-updated`, `git-updated`, `dev-updated`, `action-updated`).
pub fn pane_updated_message(pane: WorkspacePane, record: WorkspacePaneRecord) -> ServerMessage {
    let _ = (pane, record);
    todo!("D6")
}

/// TS: daemon.ts updatedPaneRecord: the pane and record of a `<pane>-updated` worker message (`None` for any other
/// message).
pub fn updated_pane_record(message: WorkerMessage) -> Option<(WorkspacePane, WorkspacePaneRecord)> {
    let _ = message;
    todo!("D6")
}

/// TS: daemon.ts startsOnView: Terminal and Git start on first view or attach; Dev and actions only when started.
pub fn starts_on_view(pane: WorkspacePane) -> bool {
    let _ = pane;
    todo!("D6")
}

impl Daemon {
    /// TS: InkDaemon.workspaceKeyOf: the session's workspace, or `None` while it has none (deleted, or still being
    /// prepared).
    pub fn workspace_key_of(&self, session: &SessionRecord) -> Option<String> {
        let _ = session;
        todo!("D6")
    }

    /// TS: InkDaemon.requireWorkspace (`<Label> is unavailable: <noWorkspaceReason>`).
    pub fn require_workspace(&self, session_id: &str, pane: WorkspacePane) -> Result<(Ordered<SessionRecord>, String)> {
        let _ = (session_id, pane);
        todo!("D6")
    }

    /// TS: InkDaemon.workspaceOfSession: the running worker of the session's workspace.
    pub fn workspace_of_session(&self, session_id: &str) -> Option<Rc<WorkspaceRuntime>> {
        let _ = session_id;
        todo!("D6")
    }

    /// TS: InkDaemon.workspaceDevLive.
    pub fn workspace_dev_live(&self, key: Option<&str>) -> bool {
        let _ = key;
        todo!("D6")
    }

    /// TS: InkDaemon.assertWorkspaceAvailable (`Workspace cleanup is in progress`).
    pub fn assert_workspace_available(&self, cwd: &str) -> Result<()> {
        let _ = cwd;
        todo!("D6")
    }

    /// TS: InkDaemon.openWorkspace: the workspace's worker, started on demand (`Worktree directory is missing:
    /// <key>`).
    pub async fn open_workspace(&self, key: &str) -> Result<Rc<WorkspaceRuntime>> {
        let _ = key;
        todo!("D6")
    }

    /// TS: InkDaemon.ensureWorkspace: the running worker, or the one being started, or a new start.
    pub async fn ensure_workspace(&self, key: &str) -> Result<Rc<WorkspaceRuntime>> {
        let _ = key;
        todo!("D6")
    }

    /// TS: InkDaemon.startWorkspaceWorker: spawns the worker and sends it `start {cwd}`.
    pub async fn start_workspace_worker(&self, key: &str) -> Result<Rc<WorkspaceRuntime>> {
        let _ = key;
        todo!("D6")
    }

    /// TS: child.on('message') / child.on('exit') of a workspace worker: handles its events in order until it exits.
    pub async fn pump_workspace_events(&self, workspace: Rc<WorkspaceRuntime>, events: ChannelEvents) {
        let _ = (workspace, events);
        todo!("D6")
    }

    /// TS: InkDaemon.requestPane: a pane request counted as busy; a failed one retires an idle worker.
    pub async fn request_pane<T: DeserializeOwned>(
        &self,
        workspace: &Rc<WorkspaceRuntime>,
        payload: Map<String, Value>,
    ) -> Result<T> {
        let _ = (workspace, payload);
        todo!("D6")
    }

    /// TS: InkDaemon.retireIfIdle: retires the worker once nothing lives in it (`idle`: already known; else asked)
    /// and no pane request is in flight.
    pub async fn retire_if_idle(&self, workspace: &Rc<WorkspaceRuntime>, idle: Option<bool>) {
        let _ = (workspace, idle);
        todo!("D6")
    }

    /// TS: InkDaemon.handleWorkspaceMessage: pane output to the attached client, pane updates to every watcher.
    pub async fn handle_workspace_message(
        &self,
        workspace: &Rc<WorkspaceRuntime>,
        message: WorkerMessage,
    ) -> Result<()> {
        let _ = (workspace, message);
        todo!("D6")
    }

    /// TS: InkDaemon.handleWorkspaceExit: an unexpected exit leaves the panes' last output, marked stopped.
    pub async fn handle_workspace_exit(&self, workspace: &Rc<WorkspaceRuntime>) -> Result<()> {
        let _ = workspace;
        todo!("D6")
    }

    /// TS: InkDaemon.retireWorkspace: stops the worker and every pane in it, waiting (bounded) until it exited.
    pub async fn retire_workspace(&self, key: &str) -> Result<()> {
        let _ = key;
        todo!("D6")
    }

    /// TS: InkDaemon.detachWorkspaceClients: every attached client gets `<pane>-detached`.
    pub fn detach_workspace_clients(&self, workspace: &Rc<WorkspaceRuntime>) {
        let _ = workspace;
        todo!("D6")
    }

    /// TS: InkDaemon.watchWorkspacePane: the pane's record for a viewer of the session (last viewer sizes it).
    pub async fn watch_workspace_pane(
        &self,
        pane: WorkspacePane,
        session_id: Option<&str>,
        cols: i64,
        rows: i64,
    ) -> Result<WorkspacePaneRecord> {
        let _ = (pane, session_id, cols, rows);
        todo!("D6")
    }

    /// TS: InkDaemon.attachWorkspacePane (`no running dev command; d d starts it`, `no action has run here; r runs
    /// one`, `<name> is already attached elsewhere`): answers `{...session, ...attachData}` itself, then
    /// `<pane>-attached` and the initial frame.
    pub async fn attach_workspace_pane(
        &self,
        client: &ClientHandle,
        request_id: &str,
        pane: WorkspacePane,
        session_id: &str,
        cols: Option<i64>,
        rows: Option<i64>,
    ) -> Result<()> {
        let _ = (client, request_id, pane, session_id, cols, rows);
        todo!("D6")
    }

    /// TS: InkDaemon.startWorkspaceDev.
    pub async fn start_workspace_dev(
        &self,
        session_id: &str,
        key: &str,
        command: &str,
        cols: i64,
        rows: i64,
    ) -> Result<WorkspacePaneRecord> {
        let _ = (session_id, key, command, cols, rows);
        todo!("D6")
    }

    /// TS: InkDaemon.startWorkspaceAction.
    pub async fn start_workspace_action(
        &self,
        session_id: &str,
        key: &str,
        name: &str,
        command: &str,
        cols: i64,
        rows: i64,
    ) -> Result<WorkspacePaneRecord> {
        let _ = (session_id, key, name, command, cols, rows);
        todo!("D6")
    }

    /// TS: InkDaemon.stopWorkspacePane (`dev` or `action`): stops it and retires the worker when nothing is left.
    pub async fn stop_workspace_pane(&self, key: &str, pane: WorkspacePane) -> Result<()> {
        let _ = (key, pane);
        todo!("D6")
    }

    /// TS: InkDaemon.publishWorkspacePane: every client watching a session of the workspace gets the pane, stamped
    /// with the session it watches.
    pub fn publish_workspace_pane(&self, key: &str, pane: WorkspacePane, record: &WorkspacePaneRecord) {
        let _ = (key, pane, record);
        todo!("D6")
    }

    /// TS: InkDaemon.syncDevRunning: devRunning mirrors the workspace's Dev on every session in it.
    pub async fn sync_dev_running(&self, key: &str) -> Result<()> {
        let _ = key;
        todo!("D6")
    }

    /// TS: InkDaemon.resolveSessionDevCommand (an unreadable deckhand.json counts as untrusted).
    pub async fn resolve_session_dev_command(&self, session: &SessionRecord) -> Result<String> {
        let _ = session;
        todo!("D6")
    }

    /// TS: handleRequest `run-action` (`Review and trust deckhand.json first (r, then the action, or T in
    /// Settings)`, `Unknown project action`).
    pub async fn handle_run_action(&self, request: &RunAction) -> Result<WorkspacePaneRecord> {
        let _ = request;
        todo!("D6")
    }

    /// TS: handleRequest `watch-terminal` / `watch-git` / `watch-dev` / `watch-action`: records what the client
    /// watches (clamped sizes) and answers the pane.
    pub async fn handle_watch_pane(
        &self,
        conn: &Connection,
        pane: WorkspacePane,
        request: &WatchPane,
    ) -> Result<WorkspacePaneRecord> {
        let _ = (conn, pane, request);
        todo!("D6")
    }

    /// TS: handleRequest `start-dev`.
    pub async fn handle_start_dev(&self, request: &SessionSizeRequest) -> Result<WorkspacePaneRecord> {
        let _ = request;
        todo!("D6")
    }

    /// TS: handleRequest `stop-dev` / `stop-action` (`pane`: Dev or Action).
    pub async fn handle_stop_pane(&self, request: &SessionRequest, pane: WorkspacePane) -> Result<OkResult> {
        let _ = (request, pane);
        todo!("D6")
    }

    /// TS: handleRequest `<pane>-input`.
    pub fn handle_pane_input(&self, pane: WorkspacePane, request: &Input) -> Result<()> {
        let _ = (pane, request);
        todo!("D6")
    }

    /// TS: handleRequest `<pane>-resize`.
    pub fn handle_pane_resize(&self, pane: WorkspacePane, request: &Resize) -> Result<()> {
        let _ = (pane, request);
        todo!("D6")
    }

    /// TS: handleRequest `<pane>-detach`: releases this client's attach and answers `<pane>-detached`.
    pub fn handle_pane_detach(&self, conn: &Connection, pane: WorkspacePane, request: &Detach) -> Result<()> {
        let _ = (conn, pane, request);
        todo!("D6")
    }
}
