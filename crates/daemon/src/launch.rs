//! Port of daemon.ts (agent argv/env, worker launch).
//!
//! A session's agent runs in its own session worker (one PTY). This module resolves the agent binary, builds the
//! launch arguments' extra parts (first message, handoff), runs a new worktree's setup command, starts the worker and
//! handles what it reports; it also serves the agent pane (preview, attach, input) through it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use deckhand_core::agents::LaunchPlan;
use deckhand_core::error::Result;
use deckhand_protocol::Ordered;
use deckhand_protocol::data::{AgentSessionRef, PreviewRecord, ProgramKey, RestartMode, SessionRecord};
use deckhand_protocol::wire::{Attach, Detach, Input, Resize, WatchPreview};
use deckhand_protocol::worker_ipc::WorkerMessage;
use deckhand_sys::async_util::Timer;
use indexmap::IndexMap;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::daemon::Daemon;
use crate::server::{ClientHandle, Connection};
use crate::worker_channel::{ChannelEvents, WorkerChannel};

/// TS: daemon.ts SETUP_OUTPUT_LIVE_LIMIT (setup output kept while it runs, UTF-16 units).
pub const SETUP_OUTPUT_LIVE_LIMIT: usize = 16_000;

/// TS: daemon.ts SETUP_OUTPUT_STORED_LIMIT (output kept once setup failed or was cancelled).
pub const SETUP_OUTPUT_STORED_LIMIT: usize = 4096;

/// TS: runSetup's timeout (10 minutes, then SIGKILL to the setup's process group).
pub const SETUP_TIMEOUT: Duration = Duration::from_millis(600_000);

/// TS: runSetup's broadcast throttle (running output is pushed at most this often).
pub const SETUP_BROADCAST_INTERVAL: Duration = Duration::from_millis(150);

/// TS: WorkerRuntime `draft`: a task's text waiting to be typed into the agent's input (see hooks.rs typeDraft).
pub struct Draft {
    pub text: String,
    /// Gives up after DRAFT_WAIT.
    pub timer: Timer,
    /// Restarted by every screen update; fires once the screen settled for DRAFT_SETTLE.
    pub settle: Option<Timer>,
}

/// TS: daemon.ts WorkerRuntime: a session worker (only the agent PTY; Terminal/Git/Dev live in the workspace worker).
pub struct WorkerRuntime {
    pub channel: WorkerChannel,
    /// TS `attached.agent`: the client attached to the agent.
    pub attached_agent: RefCell<Option<ClientHandle>>,
    pub delete_worktree_on_exit: Cell<bool>,
    pub delete_branch_on_exit: Cell<bool>,
    /// Before deleting the worktree on exit, send its note's open checklist items to the backlog.
    pub send_open_items_on_exit: Cell<bool>,
    pub hook_token: String,
    pub launch_id: String,
    pub allow_data_loss: Cell<bool>,
    /// The agent's screen as last broadcast by the worker (scroll offset 0), for typing a task's draft.
    pub screen: RefCell<Option<String>>,
    pub draft: RefCell<Option<Draft>>,
    /// When the hook behind the last applied signal started (its `sentAt`, ms): async hooks may arrive out of order.
    pub last_hook_at: Cell<Option<i64>>,
}

/// A running setup command (TS `setupProcesses` value: the detached child, signalled by process group).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetupProcess {
    pub pid: Option<u32>,
}

/// The launch's part of the daemon state.
#[derive(Default)]
pub struct LaunchState {
    /// TS `workers`: session workers by session ID.
    pub workers: IndexMap<String, Rc<WorkerRuntime>>,
    /// TS `setupProcesses`: running setup commands by session ID.
    pub setup_processes: HashMap<String, SetupProcess>,
    /// TS `programCommandCache`: resolved agent binaries.
    pub program_command_cache: HashMap<ProgramKey, String>,
}

/// TS: daemon.ts buildDeckhandAgentName: `dh-<safe title>-<id prefix>[-<suffix>]`.
pub fn build_deckhand_agent_name(title: &str, session_id: &str, suffix: Option<&str>) -> String {
    let _ = (title, session_id, suffix);
    todo!("D4")
}

/// TS: daemon.ts prepareAgentSessionRef: a path ref's directory is created.
pub async fn prepare_agent_session_ref(agent_session_ref: Option<&AgentSessionRef>) -> Result<()> {
    let _ = agent_session_ref;
    todo!("D4")
}

/// TS: daemon.ts resolveShellCommand (`$SHELL`, else `/bin/sh`).
pub fn resolve_shell_command() -> String {
    todo!("D4")
}

/// TS: daemon.ts handoffPrompt: a handoff child's first message.
pub fn handoff_prompt(handoff_path: &str) -> String {
    let _ = handoff_path;
    todo!("D4")
}

/// TS: daemon.ts movedForkPrompt: a fork into another worktree is told where it now is.
pub fn moved_fork_prompt(child: &SessionRecord, parent_root: &str, child_root: &str) -> String {
    let _ = (child, parent_root, child_root);
    todo!("D4")
}

/// TS: InkDaemon.buildPreviewRecord.
pub fn build_preview_record(
    session: &SessionRecord,
    content: &str,
    scroll_offset: i64,
    max_scroll_offset: i64,
) -> PreviewRecord {
    let _ = (session, content, scroll_offset, max_scroll_offset);
    todo!("D4")
}

impl Daemon {
    /// TS: daemon.ts resolveProgramCommand: the agent's binary on PATH (cached while it exists), else its name.
    pub async fn resolve_program_command(&self, program: ProgramKey) -> String {
        let _ = program;
        todo!("D4")
    }

    /// TS: InkDaemon.isCurrentLaunch: the session is still starting with this launch.
    pub fn is_current_launch(&self, session_id: &str, launch_id: Option<&str>) -> bool {
        let _ = (session_id, launch_id);
        todo!("D4")
    }

    /// TS: InkDaemon.assertCurrentLaunch (`Startup cancelled`).
    pub fn assert_current_launch(&self, session_id: &str, launch_id: Option<&str>) -> Result<()> {
        let _ = (session_id, launch_id);
        todo!("D4")
    }

    /// TS: InkDaemon.runSetup: runs a new worktree's setup command in a detached shell, streaming its output into the
    /// session's `setup` (`Worktree setup failed (<code>); press s to retry`, `Worktree setup timed out`).
    pub async fn run_setup(&self, session_id: &str, command: &str) -> Result<()> {
        let _ = (session_id, command);
        todo!("D4")
    }

    /// TS: InkDaemon.startWorker: starts the session worker and its agent (`Session has no launch identity`,
    /// `Startup cancelled`); answers the running session with the agent version it runs.
    pub async fn start_worker(
        &self,
        session: Ordered<SessionRecord>,
        cols: i64,
        rows: i64,
    ) -> Result<Ordered<SessionRecord>> {
        let _ = (session, cols, rows);
        todo!("D4")
    }

    /// TS: child.on('message') / child.on('exit') of a session worker: handles its events in order until it exits
    /// (a failure handling one is logged, never fatal).
    pub async fn pump_worker_events(&self, session_id: String, worker: Rc<WorkerRuntime>, events: ChannelEvents) {
        let _ = (session_id, worker, events);
        todo!("D4")
    }

    /// TS: InkDaemon.stopWorker: asks the worker to kill its agent, SIGKILLs it after WORKER_KILL_GRACE.
    pub fn stop_worker(&self, worker: &Rc<WorkerRuntime>) {
        let _ = worker;
        todo!("D4")
    }

    /// TS: InkDaemon.sendWorkerRequest (`session worker is not running`).
    pub async fn send_worker_request<T: DeserializeOwned>(
        &self,
        session_id: &str,
        payload: Map<String, Value>,
    ) -> Result<T> {
        let _ = (session_id, payload);
        todo!("D4")
    }

    /// TS: InkDaemon.sendWorkerEvent (nothing when the session has no worker).
    pub fn send_worker_event(&self, session_id: &str, payload: Map<String, Value>) {
        let _ = (session_id, payload);
        todo!("D4")
    }

    /// TS: InkDaemon.handleWorkerMessage: running, exit, agent-status, preview-updated (draft settle, watchers) and
    /// output (to the attached client), from the session's current worker only.
    pub async fn handle_worker_message(
        &self,
        session_id: &str,
        message: WorkerMessage,
        worker: &Rc<WorkerRuntime>,
    ) -> Result<()> {
        let _ = (session_id, message, worker);
        todo!("D4")
    }

    /// TS: InkDaemon.handleWorkerProcessExit: fails pending requests (`session worker exited`) and records the exit
    /// of a session still running (`Session worker exited unexpectedly`).
    pub async fn handle_worker_process_exit(&self, session_id: &str) -> Result<()> {
        let _ = session_id;
        todo!("D4")
    }

    /// TS: InkDaemon.firstMessageArgs: `['--', handoffPrompt]` on a handoff child's first launch, `['--',
    /// movedForkPrompt]` for a fork in another worktree, else none.
    pub fn first_message_args(&self, session: &SessionRecord, plan: &LaunchPlan, first_launch: bool) -> Vec<String> {
        let _ = (session, plan, first_launch);
        todo!("D4")
    }

    /// TS: InkDaemon.continuedConversation: the Claude conversation a resumed session continued in, from its
    /// transcripts (never a fork's; never fails the restart).
    pub async fn continued_conversation(
        &self,
        session: Ordered<SessionRecord>,
        mode: RestartMode,
        never_started: bool,
    ) -> Ordered<SessionRecord> {
        let _ = (session, mode, never_started);
        todo!("D4")
    }

    /// TS: InkDaemon.getPreviewRecord: the worker's snapshot while it runs, else the stored last screen.
    pub async fn get_preview_record(
        &self,
        session_id: Option<&str>,
        cols: i64,
        rows: i64,
        scroll_offset: i64,
    ) -> Result<PreviewRecord> {
        let _ = (session_id, cols, rows, scroll_offset);
        todo!("D4")
    }

    /// TS: handleRequest `watch-preview`: records what the client watches (clamped sizes) and answers the preview.
    pub async fn handle_watch_preview(&self, conn: &Connection, request: &WatchPreview) -> Result<PreviewRecord> {
        let _ = (conn, request);
        todo!("D4")
    }

    /// TS: handleRequest `attach` (`session is not running`, `session is already attached elsewhere`): answers
    /// `{...session, ...attachData}` itself, then `attached` and the initial frame.
    pub async fn handle_attach(&self, conn: &Connection, request: &Attach) -> Result<()> {
        let _ = (conn, request);
        todo!("D4")
    }

    /// TS: handleRequest `input`: typed letters drop a pending draft; keys go to the agent; an answer to a prompt
    /// marks the session working again (in the background).
    pub fn handle_input(&self, request: &Input) -> Result<()> {
        let _ = request;
        todo!("D4")
    }

    /// TS: handleRequest `resize`.
    pub fn handle_resize(&self, request: &Resize) -> Result<()> {
        let _ = request;
        todo!("D4")
    }

    /// TS: handleRequest `detach`: releases this client's agent attach and answers `detached`.
    pub fn handle_detach(&self, conn: &Connection, request: &Detach) -> Result<()> {
        let _ = (conn, request);
        todo!("D4")
    }
}
