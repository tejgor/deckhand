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

use deckhand_core::error::{DhError, Result};
use deckhand_core::jsstr::path;
use deckhand_core::project_config::{resolve_dev_command, resolve_settings};
use deckhand_core::session_scope::is_path_inside;
use deckhand_core::workspace::{no_workspace_reason, workspace_key, workspace_worker_id};
use deckhand_protocol::Ordered;
use deckhand_protocol::data::{AttachTarget, OkResult, PaneRecord, SessionRecord};
use deckhand_protocol::events::{
    ActionUpdated, DevUpdated, GitUpdated, ServerMessage, SessionOutput, SessionRef, TerminalUpdated,
};
use deckhand_protocol::wire::{Detach, Input, Resize, RunAction, SessionRequest, SessionSizeRequest, WatchPane};
use deckhand_protocol::worker_ipc::{IdleResult, WorkerMessage};
use deckhand_sys::async_util::{SharedTask, spawn};
use deckhand_sys::config::load_project_config;
use deckhand_sys::storage::load_app_config;
use futures::FutureExt;
use indexmap::IndexMap;
use nix::sys::signal::Signal;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::daemon::Daemon;
use crate::dispatch::response;
use crate::launch::resize_event;
use crate::server::{ClientHandle, Connection, DEFAULT_PREVIEW_COLS, DEFAULT_PREVIEW_ROWS, clamp_size, send_message};
use crate::worker_channel::{ChannelEvent, ChannelEvents, SpawnOptions, WorkerChannel, WorkerKind, worker_exe};

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
        match self {
            WorkspacePane::Terminal => "terminal",
            WorkspacePane::Git => "git",
            WorkspacePane::Dev => "dev",
            WorkspacePane::Action => "action",
        }
    }

    /// TS: daemon.ts PANE_LABELS (`Terminal`, `Git`, `Dev`, `Actions`).
    pub fn label(self) -> &'static str {
        match self {
            WorkspacePane::Terminal => "Terminal",
            WorkspacePane::Git => "Git",
            WorkspacePane::Dev => "Dev",
            WorkspacePane::Action => "Actions",
        }
    }

    /// TS: daemon.ts PANE_ATTACH_NAMES (`terminal`, `git`, `dev command`, `action`).
    pub fn attach_name(self) -> &'static str {
        match self {
            WorkspacePane::Terminal => "terminal",
            WorkspacePane::Git => "git",
            WorkspacePane::Dev => "dev command",
            WorkspacePane::Action => "action",
        }
    }

    /// The pane an `output` message's `target` names (`agent` is no workspace pane).
    fn of_target(target: AttachTarget) -> Option<WorkspacePane> {
        match target {
            AttachTarget::Terminal => Some(WorkspacePane::Terminal),
            AttachTarget::Git => Some(WorkspacePane::Git),
            AttachTarget::Dev => Some(WorkspacePane::Dev),
            AttachTarget::Action => Some(WorkspacePane::Action),
            AttachTarget::Agent => None,
        }
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

impl WorkspaceRuntime {
    /// TS: the `{key, process, pending, attached: {}, records: {}, busy: 0}` literal of startWorkspaceWorker.
    pub fn new(key: String, channel: WorkerChannel) -> Self {
        WorkspaceRuntime {
            key,
            channel,
            attached: RefCell::new(IndexMap::new()),
            records: RefCell::new(IndexMap::new()),
            busy: Cell::new(0),
        }
    }
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
    match pane {
        WorkspacePane::Terminal => ServerMessage::TerminalUpdated(TerminalUpdated { terminal: record }),
        WorkspacePane::Git => ServerMessage::GitUpdated(GitUpdated { git: record }),
        WorkspacePane::Action => ServerMessage::ActionUpdated(ActionUpdated { action: record }),
        WorkspacePane::Dev => ServerMessage::DevUpdated(DevUpdated { dev: record }),
    }
}

/// TS: daemon.ts updatedPaneRecord: the pane and record of a `<pane>-updated` worker message (`None` for any other
/// message).
pub fn updated_pane_record(message: WorkerMessage) -> Option<(WorkspacePane, WorkspacePaneRecord)> {
    match message {
        WorkerMessage::TerminalUpdated(updated) => Some((WorkspacePane::Terminal, updated.terminal)),
        WorkerMessage::GitUpdated(updated) => Some((WorkspacePane::Git, updated.git)),
        WorkerMessage::ActionUpdated(updated) => Some((WorkspacePane::Action, updated.action)),
        WorkerMessage::DevUpdated(updated) => Some((WorkspacePane::Dev, updated.dev)),
        _ => None,
    }
}

/// TS: daemon.ts startsOnView: Terminal and Git start on first view or attach; Dev and actions only when started.
pub fn starts_on_view(pane: WorkspacePane) -> bool {
    matches!(pane, WorkspacePane::Terminal | WorkspacePane::Git)
}

/// `{type: '<pane>-output', sessionId, data}`.
fn pane_output_message(pane: WorkspacePane, session_id: String, data: String) -> ServerMessage {
    let output = SessionOutput { session_id, data };
    match pane {
        WorkspacePane::Terminal => ServerMessage::TerminalOutput(output),
        WorkspacePane::Git => ServerMessage::GitOutput(output),
        WorkspacePane::Dev => ServerMessage::DevOutput(output),
        WorkspacePane::Action => ServerMessage::ActionOutput(output),
    }
}

/// `{type: '<pane>-attached', sessionId}`.
fn pane_attached_message(pane: WorkspacePane, session_id: String) -> ServerMessage {
    let session = SessionRef { session_id };
    match pane {
        WorkspacePane::Terminal => ServerMessage::TerminalAttached(session),
        WorkspacePane::Git => ServerMessage::GitAttached(session),
        WorkspacePane::Dev => ServerMessage::DevAttached(session),
        WorkspacePane::Action => ServerMessage::ActionAttached(session),
    }
}

/// `{type: '<pane>-detached', sessionId}`.
fn pane_detached_message(pane: WorkspacePane, session_id: String) -> ServerMessage {
    let session = SessionRef { session_id };
    match pane {
        WorkspacePane::Terminal => ServerMessage::TerminalDetached(session),
        WorkspacePane::Git => ServerMessage::GitDetached(session),
        WorkspacePane::Dev => ServerMessage::DevDetached(session),
        WorkspacePane::Action => ServerMessage::ActionDetached(session),
    }
}

/// A JSON object literal as a worker payload (anything else: empty).
fn payload(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// A pane record literal, in its key order (`{sessionId, content, live}` and the like).
fn record_literal(value: Value) -> Result<WorkspacePaneRecord> {
    serde_json::from_value(value).map_err(DhError::from)
}

/// `{...record, sessionId, workspace}`: a record stamped for one viewer (a key the record has keeps its place).
fn stamped(record: &WorkspacePaneRecord, session_id: &str, key: &str) -> WorkspacePaneRecord {
    let mut record = record.clone();
    record.touch("sessionId");
    record.session_id = Some(session_id.to_owned());
    record.touch("workspace");
    record.workspace = Some(key.to_owned());
    record
}

/// `{content, live: false, cwd, command, name}`: a pane's record once its worker is gone.
fn stopped_record(content: String, cwd: &str, command: Option<String>, name: Option<String>) -> WorkspacePaneRecord {
    Ordered::new(PaneRecord {
        content,
        live: false,
        cwd: Some(cwd.to_owned()),
        command,
        name,
        ..PaneRecord::default()
    })
}

/// Counts one pane request as in flight while it lives (TS `busy += 1` ... `finally { busy -= 1 }`).
struct BusyGuard<'a>(&'a Cell<u32>);

impl<'a> BusyGuard<'a> {
    fn enter(busy: &'a Cell<u32>) -> Self {
        busy.set(busy.get().saturating_add(1));
        BusyGuard(busy)
    }
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get().saturating_sub(1));
    }
}

impl Daemon {
    /// TS: `this.workspaces.get(key)`.
    fn running_workspace(&self, key: &str) -> Option<Rc<WorkspaceRuntime>> {
        self.workspaces.borrow().workspaces.get(key).cloned()
    }

    /// TS: `this.workspaces.get(workspace.key) === workspace`.
    fn is_current_workspace(&self, workspace: &Rc<WorkspaceRuntime>) -> bool {
        self.running_workspace(&workspace.key)
            .is_some_and(|current| Rc::ptr_eq(&current, workspace))
    }

    /// TS: getWorkerPidPath(workspaceWorkerId(key)).
    fn workspace_pid_path(&self, key: &str) -> std::path::PathBuf {
        self.paths.worker_dir.join(format!("{}.pid", workspace_worker_id(key)))
    }

    /// The paths.ts getConfigDir the settings resolve global defaults against.
    fn config_dir_string(&self) -> String {
        self.paths.config_dir.to_string_lossy().into_owned()
    }

    /// TS: InkDaemon.workspaceKeyOf: the session's workspace, or `None` while it has none (deleted, or still being
    /// prepared).
    pub fn workspace_key_of(&self, session: &SessionRecord) -> Option<String> {
        if self.sessions.borrow().preparing.contains(&session.id) {
            return None;
        }
        workspace_key(session).filter(|key| !key.is_empty())
    }

    /// TS: InkDaemon.requireWorkspace (`<Label> is unavailable: <noWorkspaceReason>`).
    pub fn require_workspace(&self, session_id: &str, pane: WorkspacePane) -> Result<(Ordered<SessionRecord>, String)> {
        let session = self.require_session(session_id)?;
        let Some(key) = self.workspace_key_of(&session) else {
            return Err(DhError::msg(format!(
                "{} is unavailable: {}",
                pane.label(),
                no_workspace_reason(&session)
            )));
        };
        Ok((session, key))
    }

    /// TS: InkDaemon.workspaceOfSession: the running worker of the session's workspace.
    pub fn workspace_of_session(&self, session_id: &str) -> Option<Rc<WorkspaceRuntime>> {
        let session = self.get_session(session_id)?;
        let key = self.workspace_key_of(&session)?;
        self.running_workspace(&key)
    }

    /// TS: InkDaemon.workspaceDevLive.
    pub fn workspace_dev_live(&self, key: Option<&str>) -> bool {
        let Some(key) = key.filter(|key| !key.is_empty()) else {
            return false;
        };
        self.running_workspace(key).is_some_and(|workspace| {
            workspace
                .records
                .borrow()
                .get(&WorkspacePane::Dev)
                .is_some_and(|dev| dev.live)
        })
    }

    /// TS: InkDaemon.assertWorkspaceAvailable (`Workspace cleanup is in progress`).
    pub fn assert_workspace_available(&self, cwd: &str) -> Result<()> {
        let root = path::resolve(cwd);
        let cleaning = self
            .worktrees
            .borrow()
            .cleanup
            .iter()
            .any(|target| is_path_inside(target, &root));
        if cleaning {
            return Err(DhError::msg("Workspace cleanup is in progress"));
        }
        Ok(())
    }

    /// TS: InkDaemon.openWorkspace: the workspace's worker, started on demand (`Worktree directory is missing:
    /// <key>`).
    ///
    /// Refuses a worktree that is being deleted or is gone from disk.
    pub async fn open_workspace(&self, key: &str) -> Result<Rc<WorkspaceRuntime>> {
        self.assert_workspace_available(key)?;
        if let Some(existing) = self.running_workspace(key) {
            return Ok(existing);
        }
        let present = tokio::fs::metadata(key).await.is_ok_and(|stat| stat.is_dir());
        if !present {
            return Err(DhError::msg(format!("Worktree directory is missing: {key}")));
        }
        self.assert_workspace_available(key)?;
        self.ensure_workspace(key).await
    }

    /// TS: InkDaemon.ensureWorkspace: the running worker, or the one being started, or a new start.
    pub async fn ensure_workspace(&self, key: &str) -> Result<Rc<WorkspaceRuntime>> {
        if let Some(existing) = self.running_workspace(key) {
            return Ok(existing);
        }
        let pending = self.workspaces.borrow().starts.get(key).cloned();
        if let Some(pending) = pending {
            return pending.await;
        }
        // The start runs whether or not this caller keeps waiting (a JS promise does).
        let task = {
            let daemon = self.clone();
            let key = key.to_owned();
            spawn(async move { daemon.start_workspace_worker(&key).await })
        };
        let start: SharedTask<Result<Rc<WorkspaceRuntime>>> = task.boxed_local().shared();
        self.workspaces
            .borrow_mut()
            .starts
            .insert(key.to_owned(), start.clone());
        // TS `finally { if (this.workspaceStarts.get(key) === start) this.workspaceStarts.delete(key); }`, also when
        // this caller stops waiting.
        {
            let daemon = self.downgrade();
            let key = key.to_owned();
            let mine = start.clone();
            drop(spawn(async move {
                let _ = mine.clone().await;
                if let Some(daemon) = daemon.upgrade() {
                    daemon.forget_start(&key, &mine);
                }
            }));
        }
        let result = start.clone().await;
        self.forget_start(key, &start);
        result
    }

    /// Forgets `start` once it settled, unless a newer start replaced it.
    fn forget_start(&self, key: &str, start: &SharedTask<Result<Rc<WorkspaceRuntime>>>) {
        let mut state = self.workspaces.borrow_mut();
        if state.starts.get(key).is_some_and(|current| current.ptr_eq(start)) {
            state.starts.remove(key);
        }
    }

    /// TS: InkDaemon.startWorkspaceWorker: spawns the worker and sends it `start {cwd}`.
    pub async fn start_workspace_worker(&self, key: &str) -> Result<Rc<WorkspaceRuntime>> {
        let worker_dir = self.paths.worker_dir.clone();
        tokio::fs::create_dir_all(&worker_dir)
            .await
            .map_err(|error| DhError::io(&error, "mkdir", &worker_dir.to_string_lossy()))?;
        let id = workspace_worker_id(key);
        let (channel, events) = WorkerChannel::spawn(SpawnOptions {
            kind: WorkerKind::Workspace,
            exe: worker_exe(self.options.worker_exe.as_deref())?,
            env: Vec::new(),
            log_path: worker_dir.join(format!("{id}.log")),
        })?;
        let workspace = Rc::new(WorkspaceRuntime::new(key.to_owned(), channel));
        // Every listener is wired before the first await so no worker event can be missed.
        {
            let daemon = self.clone();
            let workspace = Rc::clone(&workspace);
            drop(spawn(
                async move { daemon.pump_workspace_events(workspace, events).await },
            ));
        }
        self.workspaces
            .borrow_mut()
            .workspaces
            .insert(key.to_owned(), Rc::clone(&workspace));
        let pid = workspace.channel.pid();
        let started: Result<Value> = async {
            let _ = tokio::fs::write(
                self.workspace_pid_path(key),
                pid.map(|pid| pid.to_string()).unwrap_or_default(),
            )
            .await;
            workspace
                .channel
                .request(payload(json!({"type": "start", "cwd": key})))
                .await
        }
        .await;
        if let Err(error) = started {
            if self.is_current_workspace(&workspace) {
                self.workspaces.borrow_mut().workspaces.shift_remove(key);
            }
            workspace.channel.kill(Signal::SIGKILL);
            return Err(error);
        }
        let pid = pid.map_or_else(|| "?".to_owned(), |pid| pid.to_string());
        self.log(&format!("workspace worker pid={pid} started for {key}")).await;
        Ok(workspace)
    }

    /// TS: child.on('message') / child.on('exit') of a workspace worker: starts a handler per event in order until it
    /// exits. As TS's `void this.handleWorkspaceMessage(...)`, each handler runs inline up to its first await and
    /// then goes on beside the pump: a Dev update waiting on its persist holds up neither later output nor the exit.
    pub async fn pump_workspace_events(&self, workspace: Rc<WorkspaceRuntime>, mut events: ChannelEvents) {
        while let Some(event) = events.recv().await {
            match event {
                ChannelEvent::Message(message) => {
                    let daemon = self.clone();
                    let workspace = Rc::clone(&workspace);
                    start_detached(async move {
                        // A failure handling one event is logged: it must never stop the daemon.
                        if let Err(error) = daemon.handle_workspace_message(&workspace, *message).await {
                            daemon.log_soon(format!(
                                "handling a workspace event of {} failed: {error}",
                                workspace.key
                            ));
                        }
                    })
                    .await;
                }
                ChannelEvent::Exit(_) => {
                    if let Err(error) = self.handle_workspace_exit(&workspace).await {
                        self.log_soon(format!(
                            "handling the workspace worker exit of {} failed: {error}",
                            workspace.key
                        ));
                    }
                    return;
                }
            }
        }
    }

    /// TS: InkDaemon.requestPane: a pane request counted as busy; a failed one retires an idle worker.
    ///
    /// A pane request (snapshot, attach, start-dev). A failed one (e.g. lazygit is not installed) may leave the
    /// worker empty: retire it then.
    pub async fn request_pane<T: DeserializeOwned>(
        &self,
        workspace: &Rc<WorkspaceRuntime>,
        payload: Map<String, Value>,
    ) -> Result<T> {
        let _busy = BusyGuard::enter(&workspace.busy);
        match workspace.channel.request::<T>(payload).await {
            Ok(value) => Ok(value),
            Err(error) => {
                let daemon = self.clone();
                let workspace = Rc::clone(workspace);
                drop(spawn(async move { daemon.retire_if_idle(&workspace, None).await }));
                Err(error)
            }
        }
    }

    /// TS: InkDaemon.retireIfIdle: retires the worker once nothing lives in it (`idle`: already known; else asked)
    /// and no pane request is in flight.
    ///
    /// Nothing lives in it: no pane, live or exited with output, and none starting. Its own panes never make it idle:
    /// a shell never exits on its own, and an exited pane keeps its output visible.
    pub async fn retire_if_idle(&self, workspace: &Rc<WorkspaceRuntime>, idle: Option<bool>) {
        if !self.is_current_workspace(workspace) {
            return;
        }
        let idle = match idle {
            Some(idle) => idle,
            None => workspace
                .channel
                .request::<IdleResult>(payload(json!({"type": "idle"})))
                .await
                .is_ok_and(|result| result.idle),
        };
        if !idle || workspace.busy.get() > 0 || !self.is_current_workspace(workspace) {
            return;
        }
        if let Err(error) = self.retire_workspace(&workspace.key).await {
            self.log(&format!("stopping workspace {} failed: {error}", workspace.key))
                .await;
        }
    }

    /// TS: InkDaemon.handleWorkspaceMessage: pane output to the attached client, pane updates to every watcher.
    pub async fn handle_workspace_message(
        &self,
        workspace: &Rc<WorkspaceRuntime>,
        message: WorkerMessage,
    ) -> Result<()> {
        if let WorkerMessage::Response(response) = message {
            workspace.channel.settle_response(response);
            return Ok(());
        }
        if !self.is_current_workspace(workspace) {
            return Ok(());
        }
        if let WorkerMessage::Output(output) = message {
            let Some(pane) = WorkspacePane::of_target(output.target) else {
                return Ok(());
            };
            let attached = workspace.attached.borrow().get(&pane).cloned();
            if let Some(viewer) = attached.filter(|viewer| !viewer.client.is_destroyed()) {
                send_message(
                    &viewer.client,
                    &pane_output_message(pane, viewer.session_id, output.data),
                );
            }
            return Ok(());
        }
        let Some((pane, record)) = updated_pane_record(message) else {
            return Ok(());
        };
        workspace.records.borrow_mut().insert(pane, record.clone());
        self.publish_workspace_pane(&workspace.key, pane, &record);
        if pane == WorkspacePane::Dev {
            self.sync_dev_running(&workspace.key).await?;
        }
        Ok(())
    }

    /// TS: InkDaemon.handleWorkspaceExit: an unexpected exit leaves the panes' last output, marked stopped.
    pub async fn handle_workspace_exit(&self, workspace: &Rc<WorkspaceRuntime>) -> Result<()> {
        workspace.channel.mark_exited();
        workspace.channel.reject_pending("workspace worker exited");
        // A retired worker is finished up by retireWorkspace.
        if !self.is_current_workspace(workspace) {
            return Ok(());
        }
        self.workspaces.borrow_mut().workspaces.shift_remove(&workspace.key);
        let _ = tokio::fs::remove_file(self.workspace_pid_path(&workspace.key)).await;
        self.log(&format!("workspace worker for {} exited unexpectedly", workspace.key))
            .await;
        self.detach_workspace_clients(workspace);
        // Viewers keep the last output, marked stopped; viewing Terminal/Git again starts a new worker.
        let records: Vec<(WorkspacePane, WorkspacePaneRecord)> = workspace
            .records
            .borrow()
            .iter()
            .map(|(pane, record)| (*pane, record.clone()))
            .collect();
        for (pane, last) in records {
            let last = last.into_inner();
            let record = stopped_record(last.content, &workspace.key, last.command, last.name);
            self.publish_workspace_pane(&workspace.key, pane, &record);
        }
        self.sync_dev_running(&workspace.key).await
    }

    /// TS: InkDaemon.retireWorkspace: stops the worker and every pane in it, waiting (bounded) until it exited.
    pub async fn retire_workspace(&self, key: &str) -> Result<()> {
        let workspace = match self.running_workspace(key) {
            Some(workspace) => workspace,
            None => {
                let start = self.workspaces.borrow().starts.get(key).cloned();
                let Some(start) = start else {
                    return Ok(());
                };
                match start.await {
                    Ok(workspace) => workspace,
                    Err(_) => return Ok(()),
                }
            }
        };
        if !self.is_current_workspace(&workspace) {
            return Ok(());
        }
        self.workspaces.borrow_mut().workspaces.shift_remove(key);
        let already_exited = workspace.channel.has_process_exited();
        workspace.channel.send_event(payload(json!({
            "type": "shutdown",
            "requestId": uuid::Uuid::new_v4().to_string(),
        })));
        if !already_exited {
            tokio::select! {
                () = workspace.channel.wait_process_exit() => {}
                () = tokio::time::sleep(WORKSPACE_SHUTDOWN_TIMEOUT) => workspace.channel.kill(Signal::SIGKILL),
            }
        }
        workspace.channel.mark_exited();
        workspace.channel.reject_pending("workspace worker stopped");
        let _ = tokio::fs::remove_file(self.workspace_pid_path(key)).await;
        self.detach_workspace_clients(&workspace);
        // Panes started again meanwhile run in a new worker; leave their state alone.
        let shutting_down = self.lifecycle.borrow().shutting_down;
        if shutting_down || self.running_workspace(key).is_some() {
            return Ok(());
        }
        let panes: Vec<WorkspacePane> = workspace.records.borrow().keys().copied().collect();
        for pane in panes {
            self.publish_workspace_pane(key, pane, &stopped_record(String::new(), key, None, None));
        }
        self.sync_dev_running(key).await
    }

    /// TS: InkDaemon.detachWorkspaceClients: every attached client gets `<pane>-detached`.
    pub fn detach_workspace_clients(&self, workspace: &Rc<WorkspaceRuntime>) {
        let attached = std::mem::take(&mut *workspace.attached.borrow_mut());
        for (pane, viewer) in attached {
            if !viewer.client.is_destroyed() {
                send_message(&viewer.client, &pane_detached_message(pane, viewer.session_id));
            }
        }
    }

    /// TS: InkDaemon.watchWorkspacePane: the pane's record for a viewer of the session (last viewer sizes it).
    ///
    /// Terminal and Git start on first view or attach (a shell or lazygit that exited starts again on the next view);
    /// Dev only through start-dev and actions through run-action, so a workspace without a worker has neither to show.
    pub async fn watch_workspace_pane(
        &self,
        pane: WorkspacePane,
        session_id: Option<&str>,
        cols: i64,
        rows: i64,
    ) -> Result<WorkspacePaneRecord> {
        let Some(session_id) = session_id.filter(|id| !id.is_empty()) else {
            return record_literal(json!({"content": "", "live": false}));
        };
        let Some(session) = self.get_session(session_id) else {
            return record_literal(json!({"sessionId": session_id, "content": "", "live": false}));
        };
        // No `workspace` on the record tells the pane there is none yet (or any more).
        let Some(key) = self.workspace_key_of(&session) else {
            return record_literal(json!({"sessionId": session_id, "content": "", "live": false, "cwd": session.cwd}));
        };
        let workspace = if starts_on_view(pane) {
            Some(self.open_workspace(&key).await?)
        } else {
            self.running_workspace(&key)
        };
        let Some(workspace) = workspace else {
            return record_literal(json!({
                "sessionId": session_id,
                "content": "",
                "live": false,
                "cwd": key,
                "workspace": key,
            }));
        };
        // Last viewer to size the shared pane wins.
        let snapshot: WorkspacePaneRecord = self
            .request_pane(
                &workspace,
                payload(json!({"type": "snapshot", "target": pane.as_str(), "cols": cols, "rows": rows})),
            )
            .await?;
        Ok(stamped(&snapshot, session_id, &key))
    }

    /// TS: InkDaemon.attachWorkspacePane (`no running dev command; d d starts it`, `no action has run here; r runs
    /// one`, `<name> is already attached elsewhere`): answers `{...session, ...attachData}` itself, then
    /// `<pane>-attached` and the initial frame.
    ///
    /// One attach per workspace pane; output goes to the attaching client under its session ID.
    pub async fn attach_workspace_pane(
        &self,
        client: &ClientHandle,
        request_id: &str,
        pane: WorkspacePane,
        session_id: &str,
        cols: Option<i64>,
        rows: Option<i64>,
    ) -> Result<()> {
        let (session, key) = self.require_workspace(session_id, pane)?;
        let workspace = if starts_on_view(pane) {
            Some(self.open_workspace(&key).await?)
        } else {
            self.running_workspace(&key)
        };
        let Some(workspace) = workspace else {
            return Err(DhError::msg(if pane == WorkspacePane::Dev {
                "no running dev command; d d starts it"
            } else {
                "no action has run here; r runs one"
            }));
        };
        let elsewhere = workspace
            .attached
            .borrow()
            .get(&pane)
            .is_some_and(|current| current.client.id != client.id && !current.client.is_destroyed());
        if elsewhere {
            return Err(DhError::msg(format!(
                "{} is already attached elsewhere",
                pane.attach_name()
            )));
        }
        // `clampSize(cols ?? DEFAULT, DEFAULT)`: absent and not a number alike are the default.
        let cols = clamp_size(cols, DEFAULT_PREVIEW_COLS);
        let rows = clamp_size(rows, DEFAULT_PREVIEW_ROWS);
        let attach_data: Value = self
            .request_pane(
                &workspace,
                payload(json!({"type": "attach", "target": pane.as_str(), "cols": cols, "rows": rows})),
            )
            .await?;
        let attach_data = payload(attach_data);
        workspace.attached.borrow_mut().insert(
            pane,
            PaneViewer {
                client: client.clone(),
                session_id: session.id.clone(),
            },
        );
        let initial_frame = attach_data
            .get("initialFrame")
            .and_then(Value::as_str)
            .filter(|frame| !frame.is_empty())
            .map(str::to_owned);
        // attachData carries terminalModes, so attach mode can mirror the pane's bracketed-paste state. Its keys
        // override the session's in place, new ones follow.
        let mut merged = payload(session.to_value()?);
        merged.extend(attach_data);
        send_message(client, &response(request_id, &Value::Object(merged))?);
        send_message(client, &pane_attached_message(pane, session.id.clone()));
        if let Some(frame) = initial_frame {
            send_message(client, &pane_output_message(pane, session.id.clone(), frame));
        }
        Ok(())
    }

    /// TS: InkDaemon.startWorkspaceDev.
    pub async fn start_workspace_dev(
        &self,
        session_id: &str,
        key: &str,
        command: &str,
        cols: Option<i64>,
        rows: Option<i64>,
    ) -> Result<WorkspacePaneRecord> {
        let workspace = self.open_workspace(key).await?;
        let dev: WorkspacePaneRecord = self
            .request_pane(
                &workspace,
                payload(json!({
                    "type": "start-dev",
                    "command": command,
                    "cols": clamp_size(cols, DEFAULT_PREVIEW_COLS),
                    "rows": clamp_size(rows, DEFAULT_PREVIEW_ROWS),
                })),
            )
            .await?;
        if self.is_current_workspace(&workspace) {
            workspace.records.borrow_mut().insert(WorkspacePane::Dev, dev.clone());
            self.publish_workspace_pane(key, WorkspacePane::Dev, &dev);
            self.sync_dev_running(key).await?;
        }
        Ok(stamped(&dev, session_id, key))
    }

    /// TS: InkDaemon.startWorkspaceAction.
    pub async fn start_workspace_action(
        &self,
        session_id: &str,
        key: &str,
        name: &str,
        command: &str,
        cols: Option<i64>,
        rows: Option<i64>,
    ) -> Result<WorkspacePaneRecord> {
        let workspace = self.open_workspace(key).await?;
        let action: WorkspacePaneRecord = self
            .request_pane(
                &workspace,
                payload(json!({
                    "type": "start-action",
                    "name": name,
                    "command": command,
                    "cols": clamp_size(cols, DEFAULT_PREVIEW_COLS),
                    "rows": clamp_size(rows, DEFAULT_PREVIEW_ROWS),
                })),
            )
            .await?;
        if self.is_current_workspace(&workspace) {
            workspace
                .records
                .borrow_mut()
                .insert(WorkspacePane::Action, action.clone());
            self.publish_workspace_pane(key, WorkspacePane::Action, &action);
        }
        Ok(stamped(&action, session_id, key))
    }

    /// TS: InkDaemon.stopWorkspacePane (`dev` or `action`): stops it and retires the worker when nothing is left.
    pub async fn stop_workspace_pane(&self, key: &str, pane: WorkspacePane) -> Result<()> {
        let Some(workspace) = self.running_workspace(key) else {
            return Ok(());
        };
        let stop = format!("stop-{}", pane.as_str());
        let IdleResult { idle } = workspace.channel.request(payload(json!({"type": stop}))).await?;
        // Nothing else (Terminal, Git, Dev, an action) lives in the worker: retire it (its PTYs were already
        // signalled).
        let daemon = self.clone();
        drop(spawn(
            async move { daemon.retire_if_idle(&workspace, Some(idle)).await },
        ));
        Ok(())
    }

    /// TS: InkDaemon.publishWorkspacePane: every client watching a session of the workspace gets the pane, stamped
    /// with the session it watches.
    pub fn publish_workspace_pane(&self, key: &str, pane: WorkspacePane, record: &WorkspacePaneRecord) {
        let clients: Vec<(ClientHandle, Option<String>)> = self
            .server
            .borrow()
            .clients
            .values()
            .map(|client| {
                let subscription = &client.subscription;
                let watched = match pane {
                    WorkspacePane::Terminal => &subscription.watched_terminal_session_id,
                    WorkspacePane::Git => &subscription.watched_git_session_id,
                    WorkspacePane::Action => &subscription.watched_action_session_id,
                    WorkspacePane::Dev => &subscription.watched_dev_session_id,
                };
                (client.handle.clone(), watched.clone())
            })
            .collect();
        for (handle, watched) in clients {
            let Some(session) = watched.filter(|id| !id.is_empty()).and_then(|id| self.get_session(&id)) else {
                continue;
            };
            if self.workspace_key_of(&session).as_deref() == Some(key) {
                send_message(&handle, &pane_updated_message(pane, stamped(record, &session.id, key)));
            }
        }
    }

    /// TS: InkDaemon.syncDevRunning: devRunning mirrors the workspace's Dev on every session in it.
    pub async fn sync_dev_running(&self, key: &str) -> Result<()> {
        let dev_running = self.workspace_dev_live(Some(key));
        let mut changed: Vec<Ordered<SessionRecord>> = Vec::new();
        for session in self.all_sessions() {
            if session.dev_running.unwrap_or(false) == dev_running
                || self.workspace_key_of(&session).as_deref() != Some(key)
            {
                continue;
            }
            let mut updated = session;
            updated.touch("devRunning");
            updated.dev_running = Some(dev_running);
            self.set_session(updated.clone());
            changed.push(updated);
        }
        if changed.is_empty() {
            return Ok(());
        }
        self.persist().await?;
        for session in &changed {
            self.broadcast_session_updated(session);
        }
        Ok(())
    }

    /// TS: InkDaemon.resolveSessionDevCommand (an unreadable deckhand.json counts as untrusted).
    ///
    /// An unreadable deckhand.json is treated as untrusted: fall back to the global dev command.
    pub async fn resolve_session_dev_command(&self, session: &SessionRecord) -> Result<String> {
        let app_config = load_app_config().await?;
        let project = load_project_config(&session.cwd, Some(&app_config), None).await.ok();
        resolve_dev_command(project.as_ref(), &app_config, &self.config_dir_string())
    }

    /// TS: handleRequest `run-action` (`Review and trust deckhand.json first (r, then the action, or T in
    /// Settings)`, `Unknown project action`).
    ///
    /// Actions run in the workspace's own action PTY (shown on the Terminal tab), beside the shell and Dev; like them
    /// they need a workspace, not a running agent.
    pub async fn handle_run_action(&self, request: &RunAction) -> Result<WorkspacePaneRecord> {
        let (session, key) = self.require_workspace(&request.session_id, WorkspacePane::Action)?;
        // Untrusted repository actions are never run, only global defaults' actions.
        let config = load_app_config().await?;
        let project = load_project_config(&session.cwd, Some(&config), None).await?;
        let settings = resolve_settings(Some(&project), &config, &self.config_dir_string())?;
        // `Object.hasOwn(actions, message.action)` looks the value up as its string.
        let action = request.action.js_string();
        let command = settings
            .actions
            .as_ref()
            .and_then(|actions| actions.get(&action))
            .filter(|command| !command.is_empty())
            .cloned();
        let Some(command) = command else {
            let listed = project
                .config
                .actions
                .as_ref()
                .is_some_and(|actions| actions.contains_key(&action));
            if listed {
                return Err(DhError::msg(
                    "Review and trust deckhand.json first (r, then the action, or T in Settings)",
                ));
            }
            return Err(DhError::msg("Unknown project action"));
        };
        self.start_workspace_action(&session.id, &key, &action, &command, request.cols, request.rows)
            .await
    }

    /// TS: handleRequest `watch-terminal` / `watch-git` / `watch-dev` / `watch-action`: records what the client
    /// watches (clamped sizes) and answers the pane.
    pub async fn handle_watch_pane(
        &self,
        conn: &Connection,
        pane: WorkspacePane,
        request: &WatchPane,
    ) -> Result<WorkspacePaneRecord> {
        let (cols, rows) = self.get_client(&conn.client, |client| {
            let (watched, cols, rows) = match pane {
                WorkspacePane::Terminal => (
                    &mut client.watched_terminal_session_id,
                    &mut client.terminal_cols,
                    &mut client.terminal_rows,
                ),
                WorkspacePane::Git => (
                    &mut client.watched_git_session_id,
                    &mut client.git_cols,
                    &mut client.git_rows,
                ),
                WorkspacePane::Dev => (
                    &mut client.watched_dev_session_id,
                    &mut client.dev_cols,
                    &mut client.dev_rows,
                ),
                WorkspacePane::Action => (
                    &mut client.watched_action_session_id,
                    &mut client.action_cols,
                    &mut client.action_rows,
                ),
            };
            watched.clone_from(&request.session_id);
            *cols = clamp_size(request.cols, *cols);
            *rows = clamp_size(request.rows, *rows);
            (*cols, *rows)
        });
        self.watch_workspace_pane(pane, request.session_id.as_deref(), cols, rows)
            .await
    }

    /// TS: handleRequest `start-dev`.
    pub async fn handle_start_dev(&self, request: &SessionSizeRequest) -> Result<WorkspacePaneRecord> {
        let (session, key) = self.require_workspace(&request.session_id, WorkspacePane::Dev)?;
        let command = self.resolve_session_dev_command(&session).await?;
        self.start_workspace_dev(&session.id, &key, &command, request.cols, request.rows)
            .await
    }

    /// TS: handleRequest `stop-dev` / `stop-action` (`pane`: Dev or Action).
    pub async fn handle_stop_pane(&self, request: &SessionRequest, pane: WorkspacePane) -> Result<OkResult> {
        let session = self.require_session(&request.session_id)?;
        if let Some(key) = self.workspace_key_of(&session) {
            self.stop_workspace_pane(&key, pane).await?;
        }
        Ok(OkResult::default())
    }

    /// TS: handleRequest `<pane>-input`.
    pub fn handle_pane_input(&self, pane: WorkspacePane, request: &Input) -> Result<()> {
        if let Some(workspace) = self.workspace_of_session(&request.session_id) {
            // `data: message.data`, forwarded as sent (`undefined` writes no key).
            let mut event = payload(json!({"type": "input", "target": pane.as_str()}));
            if let Some(data) = request.data.to_value() {
                event.insert("data".to_owned(), data);
            }
            workspace.channel.send_event(event);
        }
        Ok(())
    }

    /// TS: handleRequest `<pane>-resize`.
    pub fn handle_pane_resize(&self, pane: WorkspacePane, request: &Resize) -> Result<()> {
        if let Some(workspace) = self.workspace_of_session(&request.session_id) {
            workspace.channel.send_event(resize_event(pane.as_str(), request));
        }
        Ok(())
    }

    /// TS: handleRequest `<pane>-detach`: releases this client's attach and answers `<pane>-detached`.
    pub fn handle_pane_detach(&self, conn: &Connection, pane: WorkspacePane, request: &Detach) -> Result<()> {
        if let Some(workspace) = self.workspace_of_session(&request.session_id) {
            let mine = workspace
                .attached
                .borrow()
                .get(&pane)
                .is_some_and(|viewer| viewer.client.id == conn.client.id);
            if mine {
                workspace.attached.borrow_mut().shift_remove(&pane);
                workspace
                    .channel
                    .send_event(payload(json!({"type": "detach", "target": pane.as_str()})));
            }
        }
        send_message(&conn.client, &pane_detached_message(pane, request.session_id.clone()));
        Ok(())
    }
}

/// A JS `void promise()` call: polls `future` once right here (its synchronous part runs in call order) and, if it
/// is still waiting, lets it finish as its own task. Never waits on it.
async fn start_detached(future: impl std::future::Future<Output = ()> + 'static) {
    let mut future = Box::pin(future);
    let pending = std::future::poll_fn(|cx| std::task::Poll::Ready(future.as_mut().poll(cx).is_pending())).await;
    if pending {
        drop(spawn(future));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::{local, session, test_client, test_daemon};
    use crate::server::{Client, ClientSubscription};
    use crate::worker_channel::ProcessExit;
    use deckhand_protocol::data::SessionStatus;
    use deckhand_sys::paths;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::MutexGuard;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;
    use tokio::sync::{mpsc, oneshot};

    /// DECKHAND_HOME = `dir` while this lives (persisting and config reads go there).
    struct Home {
        _guard: MutexGuard<'static, ()>,
        previous: Option<std::ffi::OsString>,
    }

    impl Home {
        fn set(dir: &Path) -> Home {
            let guard = crate::server::tests::env_lock();
            let previous = std::env::var_os(paths::HOME_ENV);
            // SAFETY: tests touching the environment hold server::tests::env_lock.
            unsafe { std::env::set_var(paths::HOME_ENV, dir) };
            Home {
                _guard: guard,
                previous,
            }
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            // SAFETY: as in Home::set (the environment lock is still held).
            unsafe {
                match self.previous.take() {
                    Some(value) => std::env::set_var(paths::HOME_ENV, value),
                    None => std::env::remove_var(paths::HOME_ENV),
                }
            }
        }
    }

    /// The other end of a fake workspace worker's channel (it already said hello).
    struct FakeWorker {
        theirs: Option<BufReader<UnixStream>>,
        exit: Option<oneshot::Sender<()>>,
    }

    impl FakeWorker {
        fn stream(&mut self) -> &mut BufReader<UnixStream> {
            self.theirs.as_mut().unwrap()
        }

        /// The next message the daemon sent.
        async fn next(&mut self) -> Value {
            let mut line = String::new();
            self.stream().read_line(&mut line).await.unwrap();
            serde_json::from_str(&line).unwrap()
        }

        async fn send(&mut self, message: Value) {
            let line = format!("{message}\n");
            self.stream().get_mut().write_all(line.as_bytes()).await.unwrap();
        }

        /// Answers `request` with `data`.
        async fn answer(&mut self, request: &Value, data: Value) {
            self.send(json!({"type": "response", "requestId": request["requestId"], "ok": true, "data": data}))
                .await;
        }

        /// Fails `request` with `error`.
        async fn fail(&mut self, request: &Value, error: &str) {
            self.send(json!({"type": "response", "requestId": request["requestId"], "ok": false, "error": error}))
                .await;
        }

        /// The process exits (its end of the channel closes too).
        fn exit(&mut self) {
            self.theirs.take();
            if let Some(exit) = self.exit.take() {
                let _ = exit.send(());
            }
        }
    }

    /// A daemon without a notes store (the notes projection belongs to notes_tasks.rs).
    fn daemon(dir: &Path) -> Daemon {
        let daemon = test_daemon(dir);
        daemon.notes_tasks.borrow_mut().notes = None;
        daemon
    }

    /// A fake workspace worker (no process) registered and pumped as `key`'s.
    async fn fake_workspace(daemon: &Daemon, key: &str) -> (Rc<WorkspaceRuntime>, FakeWorker) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let (exit_tx, exit_rx) = oneshot::channel::<()>();
        let (channel, events) = WorkerChannel::attach(WorkerKind::Workspace, None, ours, async move {
            let _ = exit_rx.await;
            ProcessExit {
                code: Some(0),
                signal: None,
            }
        });
        let mut theirs = BufReader::new(theirs);
        theirs
            .get_mut()
            .write_all(b"{\"type\":\"hello\",\"version\":46}\n")
            .await
            .unwrap();
        let workspace = Rc::new(WorkspaceRuntime::new(key.to_owned(), channel));
        daemon
            .workspaces
            .borrow_mut()
            .workspaces
            .insert(key.to_owned(), Rc::clone(&workspace));
        {
            let daemon = daemon.clone();
            let workspace = Rc::clone(&workspace);
            drop(spawn(
                async move { daemon.pump_workspace_events(workspace, events).await },
            ));
        }
        let fake = FakeWorker {
            theirs: Some(theirs),
            exit: Some(exit_tx),
        };
        (workspace, fake)
    }

    fn put(daemon: &Daemon, session: Ordered<SessionRecord>) {
        daemon
            .sessions
            .borrow_mut()
            .sessions
            .insert(session.id.clone(), session);
    }

    /// A client registered with the subscription `watch` sets up.
    fn watcher(
        daemon: &Daemon,
        id: u64,
        watch: impl FnOnce(&mut ClientSubscription),
    ) -> (ClientHandle, mpsc::UnboundedReceiver<String>) {
        let (client, rx) = test_client(id);
        let mut subscription = ClientSubscription::default();
        watch(&mut subscription);
        daemon.server.borrow_mut().clients.insert(
            client.id,
            Client {
                handle: client.clone(),
                subscription,
            },
        );
        (client, rx)
    }

    /// The lines a client got so far, as written.
    fn lines(rx: &mut mpsc::UnboundedReceiver<String>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(line) = rx.try_recv() {
            out.push(line.trim_end().to_owned());
        }
        out
    }

    fn json_text<T: Serialize>(value: &T) -> String {
        serde_json::to_string(value).unwrap()
    }

    fn some(id: &str) -> Option<String> {
        Some(id.to_owned())
    }

    /// Lets spawned tasks run until nothing is ready.
    async fn settle() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }

    /// Waits (bounded) until `done` holds.
    async fn until(mut done: impl FnMut() -> bool) {
        for _ in 0..200 {
            if done() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[test]
    fn panes_are_named_labelled_and_announced_like_the_ts() {
        let panes = [
            WorkspacePane::Terminal,
            WorkspacePane::Git,
            WorkspacePane::Dev,
            WorkspacePane::Action,
        ];
        let names: Vec<_> = panes.iter().map(|pane| pane.as_str()).collect();
        assert_eq!(names, ["terminal", "git", "dev", "action"]);
        for pane in panes {
            assert_eq!(json_text(&pane), format!("\"{}\"", pane.as_str()));
        }
        let labels: Vec<_> = panes.iter().map(|pane| pane.label()).collect();
        assert_eq!(labels, ["Terminal", "Git", "Dev", "Actions"]);
        let attach: Vec<_> = panes.iter().map(|pane| pane.attach_name()).collect();
        assert_eq!(attach, ["terminal", "git", "dev command", "action"]);
        let on_view: Vec<_> = panes.iter().map(|pane| starts_on_view(*pane)).collect();
        assert_eq!(on_view, [true, true, false, false]);
        assert_eq!(WORKSPACE_SHUTDOWN_TIMEOUT, Duration::from_millis(3000));

        let record: WorkspacePaneRecord = serde_json::from_value(json!({"content": "x", "live": true})).unwrap();
        for (pane, text) in [
            (
                WorkspacePane::Terminal,
                r#"{"type":"terminal-updated","terminal":{"content":"x","live":true}}"#,
            ),
            (
                WorkspacePane::Git,
                r#"{"type":"git-updated","git":{"content":"x","live":true}}"#,
            ),
            (
                WorkspacePane::Dev,
                r#"{"type":"dev-updated","dev":{"content":"x","live":true}}"#,
            ),
            (
                WorkspacePane::Action,
                r#"{"type":"action-updated","action":{"content":"x","live":true}}"#,
            ),
        ] {
            assert_eq!(json_text(&pane_updated_message(pane, record.clone())), text);
            let message: WorkerMessage = serde_json::from_str(text).unwrap();
            let (got, got_record) = updated_pane_record(message).unwrap();
            assert_eq!(got, pane);
            assert_eq!(json_text(&got_record), r#"{"content":"x","live":true}"#);
        }
        let output: WorkerMessage =
            serde_json::from_value(json!({"type": "output", "target": "dev", "data": "d"})).unwrap();
        assert!(updated_pane_record(output).is_none());
        assert_eq!(
            json_text(&pane_detached_message(WorkspacePane::Dev, "s".into())),
            r#"{"type":"dev-detached","sessionId":"s"}"#
        );
        assert_eq!(
            json_text(&pane_attached_message(WorkspacePane::Action, "s".into())),
            r#"{"type":"action-attached","sessionId":"s"}"#
        );
        assert_eq!(
            json_text(&pane_output_message(WorkspacePane::Git, "s".into(), "d".into())),
            r#"{"type":"git-output","sessionId":"s","data":"d"}"#
        );
        // `{...record, sessionId, workspace}`: stamps follow the worker's keys; one it already has keeps its place.
        let record: WorkspacePaneRecord =
            serde_json::from_value(json!({"workspace": "old", "content": "c", "live": false, "command": "dev"}))
                .unwrap();
        assert_eq!(
            json_text(&stamped(&record, "s1", "/w")),
            r#"{"workspace":"/w","content":"c","live":false,"command":"dev","sessionId":"s1"}"#
        );
    }

    #[test]
    fn sessions_without_a_workspace_refuse_its_panes() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon(dir.path());
        let mut deleted = session("gone", "/r", "/r/wt");
        deleted.worktree = serde_json::from_value(
            json!({"mode": "managed", "path": "/r/wt", "deletedAt": "2026-01-02T00:00:00.000Z"}),
        )
        .unwrap();
        put(&daemon, deleted);
        put(&daemon, session("prep", "/r", "/r"));
        put(&daemon, session("s1", "/r", "/r/sub/.."));
        daemon.sessions.borrow_mut().preparing.insert("prep".into());

        let error = |id: &str, pane| daemon.require_workspace(id, pane).unwrap_err().to_string();
        assert_eq!(
            error("gone", WorkspacePane::Terminal),
            "Terminal is unavailable: its worktree was deleted"
        );
        assert_eq!(
            error("prep", WorkspacePane::Action),
            "Actions is unavailable: its worktree is not ready yet"
        );
        assert_eq!(error("nope", WorkspacePane::Git), "Session does not exist");
        let (found, key) = daemon.require_workspace("s1", WorkspacePane::Dev).unwrap();
        assert_eq!((found.id.as_str(), key.as_str()), ("s1", "/r"));
        assert!(daemon.workspace_of_session("s1").is_none());
        assert!(daemon.workspace_of_session("prep").is_none());
        assert!(!daemon.workspace_dev_live(Some("/r")));
        assert!(!daemon.workspace_dev_live(None));

        // A worktree being deleted refuses everything inside it.
        assert!(daemon.assert_workspace_available("/r/wt/src").is_ok());
        daemon.worktrees.borrow_mut().cleanup.insert("/r/wt".into());
        for cwd in ["/r/wt", "/r/wt/src/.."] {
            assert_eq!(
                daemon.assert_workspace_available(cwd).unwrap_err().to_string(),
                "Workspace cleanup is in progress"
            );
        }
        assert!(daemon.assert_workspace_available("/r/wt2").is_ok());
        assert!(daemon.assert_workspace_available("/r").is_ok());

        local(async {
            let missing = dir.path().join("missing");
            let missing = missing.to_str().unwrap();
            assert_eq!(
                daemon.open_workspace(missing).await.err().unwrap().to_string(),
                format!("Worktree directory is missing: {missing}")
            );
            assert_eq!(
                daemon.open_workspace("/r/wt").await.err().unwrap().to_string(),
                "Workspace cleanup is in progress"
            );
        });
    }

    #[test]
    fn watching_answers_records_in_the_ts_key_order() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon(dir.path());
        let root = dir.path().to_str().unwrap().to_owned();
        put(&daemon, session("s1", &root, &root));
        put(&daemon, session("prep", &root, "/elsewhere"));
        daemon.sessions.borrow_mut().preparing.insert("prep".into());
        let (client, _rx) = test_client(1);
        let conn = Connection::new(client);
        local(async {
            let watch = |session_id: Option<&str>, cols, rows| WatchPane {
                request_id: "r".into(),
                session_id: session_id.map(str::to_owned),
                cols: Some(cols),
                rows: Some(rows),
            };
            let none = daemon
                .handle_watch_pane(&conn, WorkspacePane::Dev, &watch(None, 0, -3))
                .await
                .unwrap();
            assert_eq!(json_text(&none), r#"{"content":"","live":false}"#);
            // Sizes are clamped to at least 1 and remembered per pane.
            let subscription = daemon.server.borrow().clients[&conn.client.id].subscription.clone();
            assert_eq!((subscription.dev_cols, subscription.dev_rows), (1, 1));
            assert_eq!((subscription.terminal_cols, subscription.terminal_rows), (80, 24));

            let unknown = daemon
                .handle_watch_pane(&conn, WorkspacePane::Action, &watch(Some("zz"), 100, 30))
                .await
                .unwrap();
            assert_eq!(json_text(&unknown), r#"{"sessionId":"zz","content":"","live":false}"#);
            let subscription = daemon.server.borrow().clients[&conn.client.id].subscription.clone();
            assert_eq!(subscription.watched_action_session_id, some("zz"));
            assert_eq!((subscription.action_cols, subscription.action_rows), (100, 30));

            let preparing = daemon
                .handle_watch_pane(&conn, WorkspacePane::Terminal, &watch(Some("prep"), 80, 24))
                .await
                .unwrap();
            assert_eq!(
                json_text(&preparing),
                r#"{"sessionId":"prep","content":"","live":false,"cwd":"/elsewhere"}"#
            );
            // Dev and actions never start a worker by being viewed.
            let dev = daemon
                .handle_watch_pane(&conn, WorkspacePane::Dev, &watch(Some("s1"), 80, 24))
                .await
                .unwrap();
            assert_eq!(
                json_text(&dev),
                format!(r#"{{"sessionId":"s1","content":"","live":false,"cwd":"{root}","workspace":"{root}"}}"#)
            );
            assert!(daemon.workspaces.borrow().workspaces.is_empty());

            // With a worker: its snapshot at the viewer's size, stamped.
            let (_workspace, mut fake) = fake_workspace(&daemon, &root).await;
            let watching = {
                let daemon = daemon.clone();
                let conn = Rc::clone(&conn);
                spawn(async move {
                    daemon
                        .handle_watch_pane(&conn, WorkspacePane::Git, &watch(Some("s1"), 120, 40))
                        .await
                })
            };
            let request = fake.next().await;
            assert_eq!(request["type"], "snapshot");
            assert_eq!(request["target"], "git");
            assert_eq!(
                (request["cols"].clone(), request["rows"].clone()),
                (json!(120), json!(40))
            );
            fake.answer(&request, json!({"content": "lazygit", "live": true, "cwd": root}))
                .await;
            let git = watching.await.unwrap();
            assert_eq!(
                json_text(&git),
                format!(r#"{{"content":"lazygit","live":true,"cwd":"{root}","sessionId":"s1","workspace":"{root}"}}"#)
            );
        });
    }

    #[test]
    fn pane_updates_and_output_reach_the_workspace_viewers() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon(dir.path());
        put(&daemon, session("s1", "/w", "/w"));
        put(&daemon, session("s2", "/w", "/w/src/.."));
        put(&daemon, session("other", "/o", "/o"));
        let (_a, mut a) = watcher(&daemon, 1, |sub| sub.watched_terminal_session_id = some("s1"));
        let (_b, mut b) = watcher(&daemon, 2, |sub| sub.watched_terminal_session_id = some("s2"));
        let (_c, mut c) = watcher(&daemon, 3, |sub| sub.watched_git_session_id = some("s1"));
        let (_d, mut d) = watcher(&daemon, 4, |sub| sub.watched_terminal_session_id = some("other"));
        let (attached, mut attached_rx) = test_client(5);
        local(async {
            let (workspace, mut fake) = fake_workspace(&daemon, "/w").await;
            fake.send(json!({"type": "terminal-updated", "terminal": {"content": "$ ", "live": true, "cwd": "/w"}}))
                .await;
            // Output goes only to the attached client, under the session it attached through; nobody attached: dropped.
            fake.send(json!({"type": "output", "target": "terminal", "data": "lost"}))
                .await;
            until(|| workspace.records.borrow().contains_key(&WorkspacePane::Terminal)).await;
            settle().await;
            workspace.attached.borrow_mut().insert(
                WorkspacePane::Terminal,
                PaneViewer {
                    client: attached.clone(),
                    session_id: "s2".into(),
                },
            );
            fake.send(json!({"type": "output", "target": "git", "data": "nobody"}))
                .await;
            fake.send(json!({"type": "output", "target": "agent", "data": "nobody"}))
                .await;
            fake.send(json!({"type": "output", "target": "terminal", "data": "ls\r\n"}))
                .await;
            let mut got = Vec::new();
            until(|| {
                got.extend(lines(&mut attached_rx));
                !got.is_empty()
            })
            .await;
            assert_eq!(got, [r#"{"type":"terminal-output","sessionId":"s2","data":"ls\r\n"}"#]);
            assert_eq!(
                lines(&mut a),
                [
                    r#"{"type":"terminal-updated","terminal":{"content":"$ ","live":true,"cwd":"/w","sessionId":"s1","workspace":"/w"}}"#
                ]
            );
            assert_eq!(
                lines(&mut b),
                [
                    r#"{"type":"terminal-updated","terminal":{"content":"$ ","live":true,"cwd":"/w","sessionId":"s2","workspace":"/w"}}"#
                ]
            );
            assert!(lines(&mut c).is_empty());
            assert!(lines(&mut d).is_empty());
            assert_eq!(
                json_text(&workspace.records.borrow()[&WorkspacePane::Terminal]),
                r#"{"content":"$ ","live":true,"cwd":"/w"}"#
            );
            // A retired worker's late messages change nothing.
            daemon.workspaces.borrow_mut().workspaces.clear();
            fake.send(json!({"type": "git-updated", "git": {"content": "late", "live": true}}))
                .await;
            tokio::time::sleep(Duration::from_millis(30)).await;
            settle().await;
            assert!(lines(&mut c).is_empty());
            assert!(!workspace.records.borrow().contains_key(&WorkspacePane::Git));
        });
    }

    #[test]
    fn dev_runs_mirror_dev_running_on_every_session_of_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let daemon = daemon(dir.path());
        let root = dir.path().join("w");
        std::fs::create_dir_all(&root).unwrap();
        let root = root.to_str().unwrap().to_owned();
        put(&daemon, session("s1", &root, &root));
        put(&daemon, session("s2", &root, &root));
        put(&daemon, session("other", "/o", "/o"));
        let (_watching, mut watching) = watcher(&daemon, 1, |sub| sub.watched_dev_session_id = some("s2"));
        local(async {
            let (_workspace, mut fake) = fake_workspace(&daemon, &root).await;
            let starting = {
                let daemon = daemon.clone();
                let root = root.clone();
                spawn(async move {
                    daemon
                        .start_workspace_dev("s1", &root, "npm run dev", Some(0), Some(50))
                        .await
                })
            };
            let request = fake.next().await;
            let mut sent = request.as_object().unwrap().clone();
            sent.remove("requestId");
            assert_eq!(
                Value::Object(sent).to_string(),
                r#"{"type":"start-dev","command":"npm run dev","cols":1,"rows":50}"#
            );
            fake.answer(
                &request,
                json!({"content": "", "live": true, "cwd": root, "command": "npm run dev"}),
            )
            .await;
            let dev = starting.await.unwrap();
            assert_eq!(
                json_text(&dev),
                format!(
                    r#"{{"content":"","live":true,"cwd":"{root}","command":"npm run dev","sessionId":"s1","workspace":"{root}"}}"#
                )
            );
            assert_eq!(
                lines(&mut watching),
                [format!(
                    r#"{{"type":"dev-updated","dev":{{"content":"","live":true,"cwd":"{root}","command":"npm run dev","sessionId":"s2","workspace":"{root}"}}}}"#
                )]
            );
            assert!(daemon.workspace_dev_live(Some(&root)));
            let running = |id: &str| daemon.get_session(id).unwrap().dev_running;
            assert_eq!(
                (running("s1"), running("s2"), running("other")),
                (Some(true), Some(true), None)
            );
            let state = std::fs::read_to_string(paths::state_path()).unwrap();
            assert!(state.contains("\"devRunning\": true"), "{state}");

            // The worker reports Dev exited: every session follows.
            fake.send(json!({"type": "dev-updated", "dev": {"content": "bye", "live": false, "cwd": root, "command": "npm run dev", "exitCode": 0}}))
                .await;
            until(|| running("s1") == Some(false)).await;
            assert_eq!((running("s1"), running("s2")), (Some(false), Some(false)));
            assert!(!daemon.workspace_dev_live(Some(&root)));
        });
    }

    /// TS handles each worker message with `void this.handleWorkspaceMessage(...)`: a Dev update waiting on its
    /// persist holds up neither the output after it nor the worker's exit.
    #[test]
    fn a_dev_update_waiting_on_persist_holds_up_neither_output_nor_the_exit() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let daemon = daemon(dir.path());
        let root = dir.path().join("w");
        std::fs::create_dir_all(&root).unwrap();
        let root = root.to_str().unwrap().to_owned();
        put(&daemon, session("s1", &root, &root));
        let gate = Rc::new(tokio::sync::Notify::new());
        {
            let gate = Rc::clone(&gate);
            daemon.lifecycle.borrow_mut().persist = Some(deckhand_sys::async_util::Coalescer::new(move || {
                let gate = Rc::clone(&gate);
                async move {
                    gate.notified().await;
                    Ok(())
                }
            }));
        }
        let (attached, mut attached_rx) = test_client(5);
        local(async {
            let (workspace, mut fake) = fake_workspace(&daemon, &root).await;
            workspace.attached.borrow_mut().insert(
                WorkspacePane::Dev,
                PaneViewer {
                    client: attached.clone(),
                    session_id: "s1".into(),
                },
            );
            fake.send(json!({"type": "dev-updated", "dev": {"content": "", "live": true, "cwd": root, "command": "npm run dev"}}))
                .await;
            until(|| daemon.get_session("s1").unwrap().dev_running == Some(true)).await;
            fake.send(json!({"type": "output", "target": "dev", "data": "after"}))
                .await;
            let mut got = Vec::new();
            until(|| {
                got.extend(lines(&mut attached_rx));
                !got.is_empty()
            })
            .await;
            assert_eq!(got, [r#"{"type":"dev-output","sessionId":"s1","data":"after"}"#]);
            fake.exit();
            until(|| !daemon.workspaces.borrow().workspaces.contains_key(&root)).await;
            assert!(!daemon.workspaces.borrow().workspaces.contains_key(&root));
            gate.notify_one();
            settle().await;
        });
    }

    #[test]
    fn attaching_answers_the_session_then_attached_then_the_frame() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon(dir.path());
        let root = dir.path().to_str().unwrap().to_owned();
        put(&daemon, session("s1", &root, &root));
        put(&daemon, session("s2", &root, &root));
        let (mine, mut mine_rx) = test_client(1);
        let (theirs, mut theirs_rx) = test_client(2);
        local(async {
            // Dev and actions attach only to what runs.
            for (pane, text) in [
                (WorkspacePane::Dev, "no running dev command; d d starts it"),
                (WorkspacePane::Action, "no action has run here; r runs one"),
            ] {
                let error = daemon
                    .attach_workspace_pane(&mine, "r0", pane, "s1", None, None)
                    .await
                    .unwrap_err();
                assert_eq!(error.to_string(), text);
            }
            let (workspace, mut fake) = fake_workspace(&daemon, &root).await;
            let attaching = {
                let daemon = daemon.clone();
                let mine = mine.clone();
                spawn(async move {
                    daemon
                        .attach_workspace_pane(&mine, "r1", WorkspacePane::Terminal, "s1", Some(0), None)
                        .await
                })
            };
            let request = fake.next().await;
            assert_eq!(
                (
                    request["type"].clone(),
                    request["target"].clone(),
                    request["cols"].clone(),
                    request["rows"].clone()
                ),
                (json!("attach"), json!("terminal"), json!(1), json!(24))
            );
            fake.answer(
                &request,
                json!({"content": "$ ", "live": true, "cwd": root, "terminalModes": {"bracketedPaste": true}, "initialFrame": "\u{1b}[H$ "}),
            )
            .await;
            attaching.await.unwrap();
            let got = lines(&mut mine_rx);
            assert_eq!(got.len(), 3, "{got:?}");
            let answer: Value = serde_json::from_str(&got[0]).unwrap();
            assert_eq!(answer["requestId"], "r1");
            let data = answer["data"].as_object().unwrap();
            let keys: Vec<&str> = data.keys().map(String::as_str).collect();
            // `{...session, ...attachData}`: the session's keys (cwd overridden in place), then the snapshot's new ones.
            assert_eq!(&keys[..6], ["id", "title", "program", "command", "cwd", "repoRoot"]);
            assert_eq!(
                &keys[keys.len() - 4..],
                ["content", "live", "terminalModes", "initialFrame"]
            );
            assert_eq!(data["status"], "running");
            assert_eq!(got[1], r#"{"type":"terminal-attached","sessionId":"s1"}"#);
            assert_eq!(
                got[2],
                r#"{"type":"terminal-output","sessionId":"s1","data":"\u001b[H$ "}"#
            );
            assert_eq!(workspace.attached.borrow()[&WorkspacePane::Terminal].session_id, "s1");

            // One attach per pane.
            let error = daemon
                .attach_workspace_pane(&theirs, "r2", WorkspacePane::Terminal, "s2", None, None)
                .await
                .unwrap_err();
            assert_eq!(error.to_string(), "terminal is already attached elsewhere");

            // Another client's detach leaves the attach alone but is answered; the owner's releases it.
            let conn = Connection::new(theirs.clone());
            daemon
                .handle_pane_detach(
                    &conn,
                    WorkspacePane::Terminal,
                    &Detach {
                        session_id: "s2".into(),
                    },
                )
                .unwrap();
            assert_eq!(
                lines(&mut theirs_rx),
                [r#"{"type":"terminal-detached","sessionId":"s2"}"#]
            );
            assert!(workspace.attached.borrow().contains_key(&WorkspacePane::Terminal));
            daemon
                .handle_pane_input(
                    WorkspacePane::Terminal,
                    &Input {
                        session_id: "s2".into(),
                        data: "x".into(),
                    },
                )
                .unwrap();
            daemon
                .handle_pane_resize(
                    WorkspacePane::Git,
                    &Resize {
                        session_id: "s1".into(),
                        cols: Some(90),
                        rows: Some(30),
                    },
                )
                .unwrap();
            let conn = Connection::new(mine.clone());
            daemon
                .handle_pane_detach(
                    &conn,
                    WorkspacePane::Terminal,
                    &Detach {
                        session_id: "s1".into(),
                    },
                )
                .unwrap();
            assert_eq!(
                lines(&mut mine_rx),
                [r#"{"type":"terminal-detached","sessionId":"s1"}"#]
            );
            assert!(workspace.attached.borrow().is_empty());
            let mut sent = Vec::new();
            for _ in 0..3 {
                sent.push(fake.next().await.to_string());
            }
            assert_eq!(
                sent,
                [
                    r#"{"type":"input","target":"terminal","data":"x"}"#,
                    r#"{"type":"resize","target":"git","cols":90,"rows":30}"#,
                    r#"{"type":"detach","target":"terminal"}"#,
                ]
            );

            // A destroyed client's attach is taken over.
            workspace.attached.borrow_mut().insert(
                WorkspacePane::Git,
                PaneViewer {
                    client: mine.clone(),
                    session_id: "s1".into(),
                },
            );
            mine.destroy();
            let attaching = {
                let daemon = daemon.clone();
                let theirs = theirs.clone();
                spawn(async move {
                    daemon
                        .attach_workspace_pane(&theirs, "r3", WorkspacePane::Git, "s2", Some(100), Some(30))
                        .await
                })
            };
            let request = fake.next().await;
            fake.answer(&request, json!({"content": "", "live": true})).await;
            attaching.await.unwrap();
            let got = lines(&mut theirs_rx);
            assert_eq!(got.len(), 2, "{got:?}");
            assert_eq!(got[1], r#"{"type":"git-attached","sessionId":"s2"}"#);
        });
    }

    #[test]
    fn an_unexpected_exit_keeps_the_last_output_marked_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon(dir.path());
        put(&daemon, session("s1", "/w", "/w"));
        let (_watching, mut watching) = watcher(&daemon, 1, |sub| {
            sub.watched_terminal_session_id = some("s1");
            sub.watched_action_session_id = some("s1");
        });
        let (attached, mut attached_rx) = test_client(2);
        local(async {
            let (workspace, mut fake) = fake_workspace(&daemon, "/w").await;
            fake.send(json!({"type": "action-updated", "action": {"content": "built", "live": true, "cwd": "/w", "command": "make", "name": "build"}}))
                .await;
            fake.send(json!({"type": "terminal-updated", "terminal": {"content": "$ ", "live": true, "cwd": "/w"}}))
                .await;
            until(|| workspace.records.borrow().len() == 2).await;
            lines(&mut watching);
            workspace.attached.borrow_mut().insert(
                WorkspacePane::Action,
                PaneViewer {
                    client: attached.clone(),
                    session_id: "s1".into(),
                },
            );
            let pending = {
                let channel = workspace.channel.clone();
                spawn(async move { channel.request::<Value>(payload(json!({"type": "snapshot"}))).await })
            };
            settle().await;
            fake.exit();
            assert_eq!(pending.await.unwrap_err().to_string(), "workspace worker exited");
            until(|| daemon.workspaces.borrow().workspaces.is_empty()).await;
            let mut got = Vec::new();
            until(|| {
                got.extend(lines(&mut watching));
                got.len() == 2
            })
            .await;
            assert!(workspace.channel.is_exited());
            assert_eq!(
                lines(&mut attached_rx),
                [r#"{"type":"action-detached","sessionId":"s1"}"#]
            );
            assert_eq!(
                got,
                [
                    r#"{"type":"action-updated","action":{"content":"built","live":false,"cwd":"/w","command":"make","name":"build","sessionId":"s1","workspace":"/w"}}"#,
                    r#"{"type":"terminal-updated","terminal":{"content":"$ ","live":false,"cwd":"/w","sessionId":"s1","workspace":"/w"}}"#,
                ]
            );
            let log = std::fs::read_to_string(dir.path().join("daemon.log")).unwrap();
            assert!(
                log.contains("daemon workspace worker for /w exited unexpectedly"),
                "{log}"
            );
        });
    }

    #[test]
    fn stopping_a_pane_retires_an_idle_worker_and_clears_its_panes() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon(dir.path());
        put(&daemon, session("s1", "/w", "/w"));
        put(&daemon, session("s2", "/w", "/w"));
        let (_watching, mut watching) = watcher(&daemon, 1, |sub| sub.watched_action_session_id = some("s2"));
        let (attached, mut attached_rx) = test_client(2);
        let stop = |session_id: &str| SessionRequest {
            request_id: "r".into(),
            session_id: session_id.into(),
        };
        local(async {
            // No worker: nothing to stop.
            assert!(
                daemon
                    .handle_stop_pane(&stop("s1"), WorkspacePane::Dev)
                    .await
                    .unwrap()
                    .ok
            );
            assert_eq!(
                daemon
                    .handle_stop_pane(&stop("zz"), WorkspacePane::Dev)
                    .await
                    .unwrap_err()
                    .to_string(),
                "Session does not exist"
            );

            let (workspace, mut fake) = fake_workspace(&daemon, "/w").await;
            workspace.records.borrow_mut().insert(
                WorkspacePane::Action,
                serde_json::from_value(json!({"content": "done", "live": false, "name": "build"})).unwrap(),
            );
            workspace.attached.borrow_mut().insert(
                WorkspacePane::Terminal,
                PaneViewer {
                    client: attached.clone(),
                    session_id: "s1".into(),
                },
            );
            std::fs::create_dir_all(dir.path().join("workers")).unwrap();
            let pid_path = daemon.workspace_pid_path("/w");
            std::fs::write(&pid_path, "123").unwrap();
            let stopping = {
                let daemon = daemon.clone();
                let request = stop("s1");
                spawn(async move { daemon.handle_stop_pane(&request, WorkspacePane::Action).await })
            };
            let request = fake.next().await;
            assert_eq!(request["type"], "stop-action");
            fake.answer(&request, json!({"idle": true})).await;
            assert!(stopping.await.unwrap().ok);
            let shutdown = fake.next().await;
            assert_eq!(shutdown["type"], "shutdown");
            assert!(shutdown["requestId"].is_string());
            assert!(daemon.workspaces.borrow().workspaces.is_empty());
            fake.exit();
            let mut got = Vec::new();
            until(|| {
                got.extend(lines(&mut watching));
                !got.is_empty()
            })
            .await;
            assert!(!pid_path.exists());
            assert_eq!(
                lines(&mut attached_rx),
                [r#"{"type":"terminal-detached","sessionId":"s1"}"#]
            );
            assert_eq!(
                got,
                [
                    r#"{"type":"action-updated","action":{"content":"","live":false,"cwd":"/w","sessionId":"s2","workspace":"/w"}}"#
                ]
            );
        });
    }

    #[test]
    fn a_failed_pane_request_retires_a_worker_left_empty() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon(dir.path());
        let root = dir.path().to_str().unwrap().to_owned();
        put(&daemon, session("s1", &root, &root));
        local(async {
            let (workspace, mut fake) = fake_workspace(&daemon, &root).await;
            let watching = {
                let daemon = daemon.clone();
                spawn(async move {
                    daemon
                        .watch_workspace_pane(WorkspacePane::Git, Some("s1"), 80, 24)
                        .await
                })
            };
            let request = fake.next().await;
            assert_eq!(workspace.busy.get(), 1);
            fake.fail(&request, "lazygit is not installed").await;
            assert_eq!(watching.await.unwrap_err().to_string(), "lazygit is not installed");
            assert_eq!(workspace.busy.get(), 0);
            // Asked whether anything still lives in it: something does, so it stays.
            let idle = fake.next().await;
            assert_eq!(idle["type"], "idle");
            fake.answer(&idle, json!({"idle": false})).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert!(daemon.is_current_workspace(&workspace));

            let watching = {
                let daemon = daemon.clone();
                spawn(async move {
                    daemon
                        .watch_workspace_pane(WorkspacePane::Git, Some("s1"), 80, 24)
                        .await
                })
            };
            let request = fake.next().await;
            fake.fail(&request, "lazygit is not installed").await;
            assert!(watching.await.is_err());
            let idle = fake.next().await;
            fake.answer(&idle, json!({"idle": true})).await;
            let shutdown = fake.next().await;
            assert_eq!(shutdown["type"], "shutdown");
            assert!(!daemon.is_current_workspace(&workspace));
            fake.exit();
        });
    }

    #[test]
    fn run_action_runs_only_trusted_actions() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let _home = Home::set(&home);
        let daemon = daemon(&home);
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let status = std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::write(repo.join("deckhand.json"), r#"{"actions":{"deploy":"make deploy"}}"#).unwrap();
        let root = repo.to_str().unwrap().to_owned();
        put(&daemon, session("s1", &root, &root));
        put(&daemon, session("prep", &root, &root));
        daemon.sessions.borrow_mut().preparing.insert("prep".into());
        local(async {
            let run = |session_id: &str, action: &str| RunAction {
                request_id: "r".into(),
                session_id: session_id.into(),
                action: action.into(),
                cols: Some(80),
                rows: Some(24),
            };
            let error = |result: Result<WorkspacePaneRecord>| result.unwrap_err().to_string();
            assert_eq!(
                error(daemon.handle_run_action(&run("prep", "deploy")).await),
                "Actions is unavailable: its worktree is not ready yet"
            );
            assert_eq!(
                error(daemon.handle_run_action(&run("s1", "deploy")).await),
                "Review and trust deckhand.json first (r, then the action, or T in Settings)"
            );
            assert_eq!(
                error(daemon.handle_run_action(&run("s1", "nope")).await),
                "Unknown project action"
            );
            assert!(daemon.workspaces.borrow().workspaces.is_empty());
            // The dev command falls back to `dev` when nothing (trusted) sets one.
            let session = daemon.get_session("s1").unwrap();
            assert_eq!(daemon.resolve_session_dev_command(&session).await.unwrap(), "dev");
        });
    }

    /// A workspace worker in sh: says hello, answers `start` and `snapshot`, exits on `shutdown`.
    const SH_WORKSPACE_WORKER: &str = r#"#!/bin/sh
printf '{"type":"hello","version":46}\n' >&3
echo "workspace $1"
while IFS= read -r line <&3; do
  id=$(printf '%s' "$line" | sed -n 's/.*"requestId":"\([^"]*\)".*/\1/p')
  case "$line" in
    *'"type":"start"'*) printf '{"type":"response","requestId":"%s","ok":true,"data":{"ok":true}}\n' "$id" >&3 ;;
    *'"type":"snapshot"'*) printf '{"type":"response","requestId":"%s","ok":true,"data":{"content":"$ ","live":true}}\n' "$id" >&3 ;;
    *'"type":"shutdown"'*) exit 0 ;;
  esac
done
"#;

    #[test]
    fn workers_start_once_on_view_and_stop_on_retire() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let exe = dir.path().join("worker.sh");
        std::fs::write(&exe, SH_WORKSPACE_WORKER).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let daemon = Daemon::with_paths(
            crate::daemon::DaemonOptions {
                worker_exe: Some(exe.clone()),
            },
            test_daemon(dir.path()).paths.clone(),
        );
        daemon.notes_tasks.borrow_mut().notes = None;
        let root = dir.path().join("w");
        std::fs::create_dir_all(&root).unwrap();
        let root = root.to_str().unwrap().to_owned();
        let mut exited = session("s1", &root, &root);
        exited.status = SessionStatus::Exited;
        put(&daemon, exited);
        local(async {
            // Concurrent viewers share one start; the panes need a workspace, not a running agent.
            let (first, second) = futures::join!(
                daemon.watch_workspace_pane(WorkspacePane::Terminal, Some("s1"), 80, 24),
                daemon.watch_workspace_pane(WorkspacePane::Terminal, Some("s1"), 80, 24),
            );
            assert_eq!(
                json_text(&first.unwrap()),
                format!(r#"{{"content":"$ ","live":true,"sessionId":"s1","workspace":"{root}"}}"#)
            );
            assert!(second.is_ok());
            let workspace = daemon.workspace_of_session("s1").unwrap();
            assert_eq!(daemon.workspaces.borrow().workspaces.len(), 1);
            settle().await;
            assert!(daemon.workspaces.borrow().starts.is_empty());
            let again = daemon.ensure_workspace(&root).await.unwrap();
            assert!(Rc::ptr_eq(&again, &workspace));
            let id = workspace_worker_id(&root);
            let pid_path = dir.path().join("workers").join(format!("{id}.pid"));
            let pid = std::fs::read_to_string(&pid_path).unwrap();
            assert_eq!(pid, workspace.channel.pid().unwrap().to_string());

            daemon.retire_workspace(&root).await.unwrap();
            assert!(workspace.channel.has_process_exited());
            assert!(workspace.channel.is_exited());
            assert!(daemon.workspaces.borrow().workspaces.is_empty());
            assert!(!pid_path.exists());
            // Retiring what is not running does nothing.
            daemon.retire_workspace(&root).await.unwrap();
            let log = std::fs::read_to_string(dir.path().join("daemon.log")).unwrap();
            assert!(log.contains(&format!("started for {root}")), "{log}");
            let worker_log = std::fs::read_to_string(dir.path().join("workers").join(format!("{id}.log"))).unwrap();
            assert_eq!(worker_log, "workspace --workspace-worker\n");
        });
    }
}
