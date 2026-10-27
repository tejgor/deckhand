//! Launching sessions: the agent's argv and environment, and the session worker.
//!
//! A session's agent runs in its own session worker (one PTY). This module resolves the agent binary, builds the
//! launch arguments' extra parts (first message, handoff), runs a new worktree's setup command, starts the worker and
//! handles what it reports; it also serves the agent pane (preview, attach, input) through it.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::process::Stdio;
use std::rc::Rc;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use deckhand_core::agent_signals::answers_prompt;
use deckhand_core::agents::{LaunchPlan, forks_parent_again};
use deckhand_core::error::{DhError, Result, errno_code};
use deckhand_core::jsstr::{self, date};
use deckhand_core::workspace::workspace_key;
use deckhand_protocol::Ordered;
use deckhand_protocol::data::{
    AgentSessionRef, AgentSessionRefKind, PreviewRecord, ProgramKey, RestartMode, SessionRecord, SessionSetup,
    SessionStatus, SetupState,
};
use deckhand_protocol::events::{PreviewUpdated, ServerMessage, SessionOutput, SessionRef};
use deckhand_protocol::wire::{Attach, Detach, Input, Loose, Resize, WatchPreview};
use deckhand_protocol::worker_ipc::WorkerMessage;
use deckhand_sys::async_util::{Timer, spawn};
use deckhand_sys::claude_transcripts::{LatestConversationOptions, latest_claude_conversation};
use deckhand_sys::proc::{self, ExecOptions};
use deckhand_term::utf8::Utf8Decoder;
use indexmap::IndexMap;
use nix::sys::signal::Signal;
use regex::Regex;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::daemon::Daemon;
use crate::dispatch::response;
use crate::hooks::{DRAFT_SETTLE, types_letters};
use crate::server::{
    ClientHandle, ClientId, Connection, DEFAULT_PREVIEW_COLS, DEFAULT_PREVIEW_ROWS, clamp_non_negative, clamp_size,
    send_message,
};
use crate::worker_channel::{
    ChannelEvent, ChannelEvents, SpawnOptions, WORKER_KILL_GRACE, WorkerChannel, WorkerKind, worker_exe,
};

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
    /// Not in the TS: the `preview-mode` last sent — on while some client types to the agent in its Preview.
    pub preview_styled: Cell<bool>,
}

impl WorkerRuntime {
    /// TS: `{process: child, pending: new Map(), attached: {}, hookToken, launchId}`.
    fn new(channel: WorkerChannel, hook_token: String, launch_id: String) -> Self {
        WorkerRuntime {
            channel,
            attached_agent: RefCell::new(None),
            delete_worktree_on_exit: Cell::new(false),
            delete_branch_on_exit: Cell::new(false),
            send_open_items_on_exit: Cell::new(false),
            hook_token,
            launch_id,
            allow_data_loss: Cell::new(false),
            screen: RefCell::new(None),
            draft: RefCell::new(None),
            last_hook_at: Cell::new(None),
            preview_styled: Cell::new(false),
        }
    }
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

/// `/_*\/+_*/g`.
static SLASH_SEPARATOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"_*/+_*").expect("valid regex"));

/// A JSON object literal as a request payload (anything else: empty).
fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// `new Date().toISOString()`.
fn now_iso() -> String {
    date::to_iso_string(date::now_ms()).unwrap_or_default()
}

/// The setup output's last `limit` UTF-16 units (TS `.slice(-limit)`).
fn last_units(text: &str, limit: usize) -> String {
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    jsstr::js_slice(text, -limit, None)
}

/// Node's `spawn` failure (the child's `error` event): `spawn <file> <CODE>`.
fn spawn_error(program: &str, error: &std::io::Error) -> DhError {
    match errno_code(error) {
        Some(code) => DhError::with_code(code, format!("spawn {program} {code}")),
        None => DhError::msg(error.to_string()),
    }
}

/// Reads a setup stream to its end, handing each decoded chunk to `append` (TS `data` with a StringDecoder per stream,
/// then its `end` with whatever the decoder still held).
async fn read_setup_stream(stream: Option<impl AsyncRead + Unpin>, append: &dyn Fn(&str)) {
    let Some(mut stream) = stream else {
        return;
    };
    let mut decoder = Utf8Decoder::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let read = match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        let Some(bytes) = chunk.get(..read) else {
            break;
        };
        append(&decoder.write(bytes));
    }
    let rest = decoder.end();
    if !rest.is_empty() {
        append(&rest);
    }
}

/// TS: daemon.ts buildDeckhandAgentName: `dh-<safe title>-<id prefix>[-<suffix>]`.
pub fn build_deckhand_agent_name(title: &str, session_id: &str, suffix: Option<&str>) -> String {
    let sanitized = deckhand_sys::git::sanitize_worktree_name(title);
    let dashed = SLASH_SEPARATOR.replace_all(&sanitized, "-");
    let cut = jsstr::js_slice(&dashed, 0, Some(40));
    let trimmed = cut.trim_matches(['-', '_']);
    let safe_title = if trimmed.is_empty() { "session" } else { trimmed };
    let prefix = jsstr::js_slice(session_id, 0, Some(8));
    match suffix.filter(|suffix| !suffix.is_empty()) {
        Some(suffix) => format!("dh-{safe_title}-{prefix}-{suffix}"),
        None => format!("dh-{safe_title}-{prefix}"),
    }
}

/// TS: daemon.ts prepareAgentSessionRef: a path ref's directory is created.
pub async fn prepare_agent_session_ref(agent_session_ref: Option<&AgentSessionRef>) -> Result<()> {
    if let Some(reference) = agent_session_ref.filter(|reference| reference.kind == AgentSessionRefKind::Path) {
        let dir = jsstr::path::dirname(&reference.value);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|error| DhError::io(&error, "mkdir", &dir))?;
    }
    Ok(())
}

/// TS: daemon.ts resolveShellCommand (`$SHELL`, else `/bin/sh`).
pub fn resolve_shell_command() -> String {
    std::env::var_os("SHELL")
        .map(|shell| shell.to_string_lossy().into_owned())
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| "/bin/sh".to_owned())
}

/// TS: daemon.ts handoffPrompt: a handoff child's first message.
pub fn handoff_prompt(handoff_path: &str) -> String {
    format!(
        "Read the handoff document at {handoff_path}. Use it as task context, inspect the workspace, and continue the work described there. Keep all normal permission checks."
    )
}

/// TS: daemon.ts movedForkPrompt: a fork into another worktree is told where it now is.
pub fn moved_fork_prompt(child: &SessionRecord, parent_root: &str, child_root: &str) -> String {
    let branch = child
        .worktree
        .as_ref()
        .and_then(|worktree| worktree.branch.as_deref())
        .filter(|branch| !branch.is_empty())
        .map(|branch| format!(" on branch {branch}"))
        .unwrap_or_default();
    format!(
        "Deckhand note: this conversation was forked from another session into a different worktree. You are now in {child_root}{branch}. Earlier messages refer to files under {parent_root}, which is the parent session's worktree: do not read or edit anything there. Work only in {child_root}. Changes the parent had not committed are not in this worktree. Reply briefly to confirm, then wait for instructions."
    )
}

/// TS: InkDaemon.buildPreviewRecord.
pub fn build_preview_record(
    session: &SessionRecord,
    content: &str,
    scroll_offset: i64,
    max_scroll_offset: i64,
) -> PreviewRecord {
    PreviewRecord {
        session_id: Some(session.id.clone()),
        content: content.to_owned(),
        live: session.status == SessionStatus::Running,
        status: Some(session.status),
        agent_status: session.agent_status,
        scroll_offset: Some(scroll_offset),
        max_scroll_offset: Some(max_scroll_offset),
        screen: None,
    }
}

impl Daemon {
    /// TS: `this.sessions.get(id)` (a plain read of the session map).
    fn held_session(&self, session_id: &str) -> Option<Ordered<SessionRecord>> {
        self.sessions.borrow().sessions.get(session_id).cloned()
    }

    /// TS: `this.workers.get(id)`.
    fn worker_of(&self, session_id: &str) -> Option<Rc<WorkerRuntime>> {
        self.launch.borrow().workers.get(session_id).cloned()
    }

    /// TS: `this.workers.get(id) === worker`.
    fn is_current_worker(&self, session_id: &str, worker: &Rc<WorkerRuntime>) -> bool {
        self.worker_of(session_id)
            .is_some_and(|current| Rc::ptr_eq(&current, worker))
    }

    /// TS: getWorkerPidPath.
    fn worker_pid_path(&self, session_id: &str) -> std::path::PathBuf {
        self.paths.worker_dir.join(format!("{session_id}.pid"))
    }

    /// TS: daemon.ts resolveProgramCommand: the agent's binary on PATH (cached while it exists), else its name.
    pub async fn resolve_program_command(&self, program: ProgramKey) -> String {
        let name = program.as_str();
        let cached = self.launch.borrow().program_command_cache.get(&program).cloned();
        // A cached path is used only while it still exists: an agent reinstalled elsewhere (e.g. moved from an npm
        // global to its own installer) is looked up again.
        if let Some(cached) = cached.filter(|cached| !cached.is_empty())
            && (cached == name || nix::unistd::access(cached.as_str(), nix::unistd::AccessFlags::F_OK).is_ok())
        {
            return cached;
        }
        self.launch.borrow_mut().program_command_cache.remove(&program);

        if name.contains('/') {
            self.launch
                .borrow_mut()
                .program_command_cache
                .insert(program, name.to_owned());
            return name.to_owned();
        }

        // Only a found binary is cached, so an agent installed while the daemon runs is found by the next launch.
        let found = deckhand_sys::agent_version_check::find_on_path(name, None)
            .await
            .map(|found| found.to_string_lossy().into_owned());
        match found {
            Some(found) => {
                self.launch
                    .borrow_mut()
                    .program_command_cache
                    .insert(program, found.clone());
                found
            }
            None => name.to_owned(),
        }
    }

    /// TS: InkDaemon.isCurrentLaunch: the session is still starting with this launch.
    pub fn is_current_launch(&self, session_id: &str, launch_id: Option<&str>) -> bool {
        let sessions = self.sessions.borrow();
        sessions.sessions.get(session_id).is_some_and(|current| {
            current.status == SessionStatus::Starting && current.launch_id.as_deref() == launch_id
        })
    }

    /// TS: InkDaemon.assertCurrentLaunch (`Startup cancelled`).
    pub fn assert_current_launch(&self, session_id: &str, launch_id: Option<&str>) -> Result<()> {
        if !self.is_current_launch(session_id, launch_id) {
            return Err(DhError::msg("Startup cancelled"));
        }
        Ok(())
    }

    /// TS: runSetup's `update`: records the setup's state and output (bounded once finished, dropped on success) and
    /// its status line, while the launch is current.
    fn update_setup(
        &self,
        session_id: &str,
        launch_id: Option<&str>,
        command: &str,
        state: SetupState,
        output: &str,
        exit_code: Option<Option<i64>>,
    ) {
        if !self.is_current_launch(session_id, launch_id) {
            return;
        }
        // Output lives only in `setup` (bounded once finished, dropped on success); lastPreview is a status line.
        let stored = match state {
            SetupState::Running => output.to_owned(),
            SetupState::Complete => String::new(),
            _ => last_units(output, SETUP_OUTPUT_STORED_LIMIT),
        };
        let updated = self.patch_session(session_id, |session| {
            session.touch("setup");
            session.setup = Some(SessionSetup {
                command: command.to_owned(),
                state,
                output: stored,
                exit_code,
                extra: IndexMap::new(),
            });
            session.touch("lastPreview");
            session.last_preview = Some(format!("Setup ({state}): {command}"));
        });
        if let Some(updated) = updated {
            self.broadcast_session_updated(&updated);
        }
    }

    /// TS: InkDaemon.runSetup: runs a new worktree's setup command in a detached shell, streaming its output into the
    /// session's `setup` (`Worktree setup failed (<code>); press s to retry`, `Worktree setup timed out`).
    pub async fn run_setup(&self, session_id: &str, command: &str) -> Result<()> {
        let initial = self.require_session(session_id)?;
        let launch_id = initial.launch_id.clone();
        let launch = launch_id.as_deref();
        self.assert_workspace_available(&initial.cwd)?;
        self.update_setup(session_id, launch, command, SetupState::Running, "", None);
        self.persist().await?;
        self.assert_current_launch(session_id, launch)?;

        let shell = resolve_shell_command();
        // Detached: the setup leads its own process group, so the timeout (and a cancel) stop all of it.
        let mut child_command = proc::command(&shell, &["-c", command], &ExecOptions::default().cwd(&initial.cwd));
        child_command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let spawned = child_command.spawn();
        let process = SetupProcess {
            pid: spawned.as_ref().ok().and_then(tokio::process::Child::id),
        };
        if spawned.is_ok() {
            self.launch
                .borrow_mut()
                .setup_processes
                .insert(session_id.to_owned(), process);
        }

        let output = RefCell::new(String::new());
        let last_broadcast: Cell<Option<Instant>> = Cell::new(None);
        let append = |text: &str| {
            let next = last_units(&format!("{}{text}", output.borrow()), SETUP_OUTPUT_LIVE_LIMIT);
            *output.borrow_mut() = next;
            if last_broadcast
                .get()
                .is_none_or(|at| at.elapsed() > SETUP_BROADCAST_INTERVAL)
            {
                last_broadcast.set(Some(Instant::now()));
                let current = output.borrow().clone();
                self.update_setup(session_id, launch, command, SetupState::Running, &current, None);
            }
        };
        let timed_out = Rc::new(Cell::new(false));
        let mut timer = {
            let timed_out = Rc::clone(&timed_out);
            Timer::after(SETUP_TIMEOUT, move || {
                timed_out.set(true);
                if let Some(pid) = process.pid {
                    let _ = proc::kill_group(pid, Signal::SIGKILL);
                }
            })
        };

        let attempt: Result<()> = async {
            let mut child = spawned.map_err(|error| spawn_error(&shell, &error))?;
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            // TS `close`: the process exited and both streams ended.
            let (_, _, status) = tokio::join!(
                read_setup_stream(stdout, &append),
                read_setup_stream(stderr, &append),
                child.wait()
            );
            let code = status.map_err(DhError::from)?.code().map(i64::from);
            self.assert_current_launch(session_id, launch)?;
            let note = if timed_out.get() {
                "\nSetup timed out after 10 minutes"
            } else {
                ""
            };
            let final_output = format!("{}{note}", output.borrow());
            let state = if code == Some(0) {
                SetupState::Complete
            } else {
                SetupState::Failed
            };
            self.update_setup(session_id, launch, command, state, &final_output, Some(code));
            self.persist().await?;
            if code != Some(0) {
                return Err(DhError::msg(if timed_out.get() {
                    "Worktree setup timed out".to_owned()
                } else {
                    let code = code.map_or_else(|| "null".to_owned(), |code| code.to_string());
                    format!("Worktree setup failed ({code}); press s to retry")
                }));
            }
            Ok(())
        }
        .await;

        let result = match attempt {
            Ok(()) => Ok(()),
            Err(error) => {
                let running = self
                    .sessions
                    .borrow()
                    .sessions
                    .get(session_id)
                    .and_then(|session| session.setup.as_ref())
                    .is_some_and(|setup| setup.state == SetupState::Running);
                if running {
                    // TS `${output}\n${String(error)}`: an Error stringifies as `Error: <message>`.
                    let failed_output = format!("{}\nError: {error}", output.borrow());
                    self.update_setup(session_id, launch, command, SetupState::Failed, &failed_output, None);
                }
                self.persist().await.and(Err(error))
            }
        };
        timer.clear();
        if spawned_matches(&self.launch.borrow().setup_processes, session_id, process) {
            self.launch.borrow_mut().setup_processes.remove(session_id);
        }
        result
    }

    /// TS: InkDaemon.startWorker: starts the session worker and its agent (`Session has no launch identity`,
    /// `Startup cancelled`); answers the running session with the agent version it runs.
    pub async fn start_worker(
        &self,
        session: Ordered<SessionRecord>,
        cols: i64,
        rows: i64,
        scroll_override: bool,
    ) -> Result<Ordered<SessionRecord>> {
        let Some(launch_id) = session.launch_id.clone().filter(|launch_id| !launch_id.is_empty()) else {
            return Err(DhError::msg("Session has no launch identity"));
        };
        self.assert_workspace_available(&session.cwd)?;
        let worker_dir = self.paths.worker_dir.clone();
        tokio::fs::create_dir_all(&worker_dir)
            .await
            .map_err(|error| DhError::io(&error, "mkdir", &worker_dir.to_string_lossy()))?;
        self.assert_workspace_available(&session.cwd)?;
        self.assert_current_launch(&session.id, Some(&launch_id))?;
        let hook_token = uuid::Uuid::new_v4().to_string();
        // The version this launch runs (read beside the spawn; cached per binary, so usually instant).
        let launch_version = {
            let checker = self.agent_versions();
            let command = session.command.clone();
            spawn(async move { checker.version_of(&command).await.version })
        };
        let (channel, events) = WorkerChannel::spawn(SpawnOptions {
            kind: WorkerKind::Session,
            exe: worker_exe(self.options.worker_exe.as_deref())?,
            env: vec![
                ("DECKHAND_SESSION_ID".to_owned(), session.id.clone()),
                ("DECKHAND_LAUNCH_ID".to_owned(), launch_id.clone()),
                ("DECKHAND_HOOK_TOKEN".to_owned(), hook_token.clone()),
            ],
            log_path: self.paths.worker_dir.join(format!("{}.log", session.id)),
        })?;
        let worker = Rc::new(WorkerRuntime::new(channel, hook_token, launch_id.clone()));
        // Every listener is wired before the first await so no worker event can be missed.
        {
            let daemon = self.clone();
            let session_id = session.id.clone();
            let worker = Rc::clone(&worker);
            drop(spawn(async move {
                daemon.pump_worker_events(session_id, worker, events).await;
            }));
        }
        self.launch
            .borrow_mut()
            .workers
            .insert(session.id.clone(), Rc::clone(&worker));

        let pid_path = self.worker_pid_path(&session.id);
        let started: Result<Ordered<SessionRecord>> = async {
            let pid = worker.channel.pid().map(|pid| pid.to_string()).unwrap_or_default();
            let _ = tokio::fs::write(&pid_path, pid).await;
            // A cancel during the await above already sent its kill; never start an agent after it.
            self.assert_current_launch(&session.id, Some(&launch_id))?;
            let mut payload = Map::new();
            payload.insert("type".to_owned(), Value::String("start".to_owned()));
            payload.insert("session".to_owned(), serde_json::to_value(&session)?);
            payload.insert("cols".to_owned(), Value::from(cols));
            payload.insert("rows".to_owned(), Value::from(rows));
            payload.insert("scrollOverride".to_owned(), Value::Bool(scroll_override));
            self.send_worker_request(&session.id, payload).await
        }
        .await;
        let mut started = match started {
            Ok(started) => started,
            Err(error) => {
                if self.is_current_worker(&session.id, &worker) {
                    self.launch.borrow_mut().workers.shift_remove(&session.id);
                    let _ = tokio::fs::remove_file(&pid_path).await;
                }
                worker.channel.kill(Signal::SIGKILL);
                return Err(error);
            }
        };
        let current = self.held_session(&session.id);
        if current.is_none_or(|current| {
            current.status == SessionStatus::Exited || current.launch_id.as_deref() != Some(launch_id.as_str())
        }) {
            // Cancelled while the agent was spawning: stop it. The exit handlers retire the worker.
            self.stop_worker(&worker);
            return Err(DhError::msg("Startup cancelled"));
        }
        let agent_version = launch_version.await;
        // Unlike TS, where the cached version is usually settled and this await never yields to worker events, the
        // `--version` read of a changed binary can outlast the agent here: its running message lets a kill through and
        // the exit record lands meanwhile. The caller's running save would revive it, so the cancel check repeats.
        let current = self.held_session(&session.id);
        let same_launch = current
            .as_ref()
            .is_some_and(|current| current.launch_id.as_deref() == Some(launch_id.as_str()));
        if same_launch {
            let patched = agent_version.clone();
            self.patch_session(&session.id, move |current| {
                current.touch("agentVersion");
                current.agent_version = patched;
            });
        }
        // A launch may be the first to see a new installed version (updated outside Deckhand).
        {
            let daemon = self.clone();
            drop(spawn(async move {
                daemon.publish_agent_versions().await;
            }));
        }
        if !same_launch || current.is_some_and(|current| current.status == SessionStatus::Exited) {
            self.stop_worker(&worker);
            return Err(DhError::msg("Startup cancelled"));
        }
        started.touch("agentVersion");
        started.agent_version = agent_version;
        Ok(started)
    }

    /// TS: child.on('message') / child.on('exit') of a session worker: starts a handler per event in order until it
    /// exits (a failure handling one is logged, never fatal). As TS's `void this.handleWorkerMessage(...)`, each
    /// handler runs inline up to its first await and then goes on beside the pump: a handler waiting on a request to
    /// this worker (a scrolled watcher's snapshot) holds up neither later events nor the exit that fails the request.
    pub async fn pump_worker_events(&self, session_id: String, worker: Rc<WorkerRuntime>, mut events: ChannelEvents) {
        while let Some(event) = events.recv().await {
            match event {
                ChannelEvent::Message(message) => {
                    let daemon = self.clone();
                    let session_id = session_id.clone();
                    let worker = Rc::clone(&worker);
                    start_detached(async move {
                        // A failure handling one event is logged: it must never stop the daemon and every agent with it.
                        if let Err(error) = daemon.handle_worker_message(&session_id, *message, &worker).await {
                            daemon.log_soon(format!("handling a worker event of {session_id} failed: {error}"));
                        }
                    })
                    .await;
                }
                ChannelEvent::Exit(_) => {
                    if self.is_current_worker(&session_id, &worker) {
                        if let Err(error) = self.handle_worker_process_exit(&session_id).await {
                            self.log_soon(format!("handling the worker exit of {session_id} failed: {error}"));
                        }
                    } else {
                        worker.channel.reject_pending("session worker exited");
                    }
                    return;
                }
            }
        }
    }

    /// TS: InkDaemon.stopWorker: asks the worker to kill its agent, SIGKILLs it after WORKER_KILL_GRACE.
    pub fn stop_worker(&self, worker: &Rc<WorkerRuntime>) {
        if worker.channel.is_connected() {
            let kill = object(json!({
                "type": "kill",
                "requestId": uuid::Uuid::new_v4().to_string(),
                "force": true,
            }));
            let _ = worker.channel.send_message(&kill);
        }
        let channel = worker.channel.clone();
        Timer::after(WORKER_KILL_GRACE, move || {
            if !channel.has_process_exited() {
                channel.kill(Signal::SIGKILL);
            }
        })
        .detach();
    }

    /// TS: InkDaemon.sendWorkerRequest (`session worker is not running`).
    pub async fn send_worker_request<T: DeserializeOwned>(
        &self,
        session_id: &str,
        payload: Map<String, Value>,
    ) -> Result<T> {
        let Some(worker) = self.worker_of(session_id) else {
            return Err(DhError::msg("session worker is not running"));
        };
        worker.channel.request(payload).await
    }

    /// TS: InkDaemon.sendWorkerEvent (nothing when the session has no worker).
    pub fn send_worker_event(&self, session_id: &str, payload: Map<String, Value>) {
        if let Some(worker) = self.worker_of(session_id) {
            worker.channel.send_event(payload);
        }
    }

    /// TS: InkDaemon.handleWorkerMessage: running, exit, agent-status, preview-updated (draft settle, watchers) and
    /// output (to the attached client), from the session's current worker only.
    pub async fn handle_worker_message(
        &self,
        session_id: &str,
        message: WorkerMessage,
        worker: &Rc<WorkerRuntime>,
    ) -> Result<()> {
        if let WorkerMessage::Response(response) = message {
            worker.channel.settle_response(response);
            return Ok(());
        }
        if !self.is_current_worker(session_id, worker) {
            return Ok(());
        }
        match message {
            WorkerMessage::Running(running) => {
                if let Some(mut session) = self.held_session(session_id)
                    && session.status != SessionStatus::Exited
                {
                    session.touch("status");
                    session.status = SessionStatus::Running;
                    session.touch("agentStartedAt");
                    session.agent_started_at = Some(now_iso());
                    session.touch("pid");
                    session.pid = Some(running.pid);
                    session.touch("updatedAt");
                    session.updated_at = now_iso();
                    self.save_session(session).await?;
                }
            }
            WorkerMessage::Exit(exit) => {
                self.handle_worker_session_exit(session_id, exit.exit_code, exit.exit_signal, &exit.last_preview)
                    .await?;
            }
            WorkerMessage::AgentStatus(status) => {
                self.set_worker_agent_status(session_id, status.agent_status).await?;
            }
            WorkerMessage::PreviewUpdated(updated) => {
                *worker.screen.borrow_mut() = Some(updated.preview.content.clone());
                // A draft is typed once the screen stops changing (each update restarts the wait).
                if let Some(draft) = worker.draft.borrow_mut().as_mut() {
                    let daemon = self.downgrade();
                    let id = session_id.to_owned();
                    // Replacing the timer clears the previous one.
                    draft.settle = Some(Timer::after(DRAFT_SETTLE, move || {
                        let Some(daemon) = daemon.upgrade() else {
                            return;
                        };
                        drop(spawn(async move {
                            if let Err(error) = daemon.type_draft(&id).await {
                                daemon.log_soon(format!("typing the task draft failed: {error}"));
                            }
                        }));
                    }));
                }
                let ids: Vec<ClientId> = self.server.borrow().clients.keys().copied().collect();
                for id in ids {
                    // A client gone meanwhile is skipped, as a Map iteration skips deleted entries.
                    let client = self
                        .server
                        .borrow()
                        .clients
                        .get(&id)
                        .map(|client| (client.handle.clone(), client.subscription.clone()));
                    let Some((handle, subscription)) = client else {
                        continue;
                    };
                    if subscription.watched_preview_session_id.as_deref() != Some(session_id) {
                        continue;
                    }
                    let preview = if subscription.preview_scroll_offset > 0 {
                        self.get_preview_record(
                            Some(session_id),
                            subscription.preview_cols,
                            subscription.preview_rows,
                            subscription.preview_scroll_offset,
                            subscription.preview_interactive,
                        )
                        .await?
                    } else if subscription.preview_interactive {
                        updated.preview.clone()
                    } else {
                        // Another client types to the agent: this one keeps its plain preview.
                        PreviewRecord {
                            screen: None,
                            ..updated.preview.clone()
                        }
                    };
                    send_message(&handle, &ServerMessage::PreviewUpdated(PreviewUpdated { preview }));
                }
            }
            WorkerMessage::Output(output) => {
                let attached = worker.attached_agent.borrow().clone();
                if let Some(attached) = attached.filter(|attached| !attached.is_destroyed()) {
                    send_message(
                        &attached,
                        &ServerMessage::Output(SessionOutput {
                            session_id: session_id.to_owned(),
                            data: output.data,
                        }),
                    );
                }
            }
            // Workspace worker messages and the handshake never reach a session worker's handler.
            _ => {}
        }
        Ok(())
    }

    /// TS: InkDaemon.handleWorkerProcessExit: fails pending requests (`session worker exited`) and records the exit
    /// of a session still running (`Session worker exited unexpectedly`).
    pub async fn handle_worker_process_exit(&self, session_id: &str) -> Result<()> {
        if let Some(worker) = self.worker_of(session_id) {
            worker.channel.mark_exited();
            worker.channel.reject_pending("session worker exited");
        }
        let _ = tokio::fs::remove_file(self.worker_pid_path(session_id)).await;
        match self.held_session(session_id) {
            Some(session) if session.status != SessionStatus::Exited => {
                let last_preview = session
                    .last_preview
                    .clone()
                    .unwrap_or_else(|| "Session worker exited unexpectedly".to_owned());
                self.handle_worker_session_exit(session_id, None, None, &last_preview)
                    .await?;
            }
            _ => {
                self.launch.borrow_mut().workers.shift_remove(session_id);
            }
        }
        Ok(())
    }

    /// TS: InkDaemon.firstMessageArgs: `['--', handoffPrompt]` on a handoff child's first launch, `['--',
    /// movedForkPrompt]` for a fork in another worktree, else none.
    pub fn first_message_args(&self, session: &SessionRecord, plan: &LaunchPlan, first_launch: bool) -> Vec<String> {
        if first_launch && let Some(handoff_path) = session.handoff_path.as_deref().filter(|path| !path.is_empty()) {
            return vec!["--".to_owned(), handoff_prompt(handoff_path)];
        }
        if !matches!(plan, LaunchPlan::Fork { .. }) {
            return Vec::new();
        }
        let parent = session
            .forked_from_session_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .and_then(|id| self.held_session(id));
        let parent_root = parent
            .and_then(|parent| workspace_key(&parent))
            .filter(|root| !root.is_empty());
        let child_root = workspace_key(session).filter(|root| !root.is_empty());
        match (parent_root, child_root) {
            (Some(parent_root), Some(child_root)) if parent_root != child_root => {
                vec!["--".to_owned(), moved_fork_prompt(session, &parent_root, &child_root)]
            }
            _ => Vec::new(),
        }
    }

    /// TS: InkDaemon.continuedConversation: the Claude conversation a resumed session continued in, from its
    /// transcripts (never a fork's; never fails the restart).
    pub async fn continued_conversation(
        &self,
        session: Ordered<SessionRecord>,
        mode: RestartMode,
        never_started: bool,
    ) -> Ordered<SessionRecord> {
        let Some(reference) = session
            .agent_session_ref
            .clone()
            .filter(|reference| reference.kind == AgentSessionRefKind::Id)
        else {
            return session;
        };
        if mode != RestartMode::Resume
            || never_started
            || session.program != ProgramKey::Claude
            || forks_parent_again(&session, never_started)
        {
            return session;
        }
        let exclude: HashSet<String> = self
            .sessions
            .borrow()
            .sessions
            .values()
            .filter(|other| other.id != session.id)
            .filter_map(|other| other.agent_session_ref.as_ref())
            .filter(|other| other.kind == AgentSessionRefKind::Id)
            .map(|other| other.value.clone())
            .collect();
        let cwd_hints = [Some(session.cwd.clone()), session.launch_worktree_root.clone()]
            .into_iter()
            .flatten()
            .filter(|dir| !dir.is_empty())
            .collect();
        let options = LatestConversationOptions {
            projects_dir: None,
            cwd_hints,
            exclude,
        };
        match latest_claude_conversation(&reference.value, &options).await {
            Ok(Some(next)) if !next.is_empty() => {
                self.log_soon(format!(
                    "{} resumes conversation {next}, which continued {} (Claude transcripts)",
                    session.id, reference.value
                ));
                let mut session = session;
                if let Some(current) = session.agent_session_ref.as_mut() {
                    current.value = next;
                }
                session
            }
            Ok(_) => session,
            Err(error) => {
                self.log_soon(format!("reading Claude transcripts for {} failed: {error}", session.id));
                session
            }
        }
    }

    /// TS: InkDaemon.getPreviewRecord: the worker's snapshot while it runs, else the stored last screen. `styled`
    /// (not in the TS) asks a running worker for the styled screen too.
    pub async fn get_preview_record(
        &self,
        session_id: Option<&str>,
        cols: i64,
        rows: i64,
        scroll_offset: i64,
        styled: bool,
    ) -> Result<PreviewRecord> {
        let Some(session_id) = session_id.filter(|id| !id.is_empty()) else {
            return Ok(PreviewRecord::default());
        };
        let Some(session) = self.held_session(session_id) else {
            return Ok(PreviewRecord {
                session_id: Some(session_id.to_owned()),
                ..PreviewRecord::default()
            });
        };
        if self.worker_of(session_id).is_some() {
            let mut payload = object(json!({
                "type": "snapshot",
                "target": "agent",
                "cols": cols,
                "rows": rows,
                "scrollOffset": scroll_offset,
            }));
            if styled {
                payload.insert("styled".to_owned(), Value::Bool(true));
            }
            return self.send_worker_request(session_id, payload).await;
        }
        Ok(build_preview_record(
            &session,
            session.last_preview.as_deref().unwrap_or(""),
            0,
            0,
        ))
    }

    /// TS: handleRequest `watch-preview`: records what the client watches (clamped sizes) and answers the preview.
    pub async fn handle_watch_preview(&self, conn: &Connection, request: &WatchPreview) -> Result<PreviewRecord> {
        let interactive = request.interactive == Some(true);
        let (previous, cols, rows, scroll_offset) = self.get_client(&conn.client, |client| {
            let previous = std::mem::replace(&mut client.watched_preview_session_id, request.session_id.clone());
            client.preview_cols = clamp_size(request.cols, client.preview_cols);
            client.preview_rows = clamp_size(request.rows, client.preview_rows);
            client.preview_scroll_offset = clamp_non_negative(request.scroll_offset);
            client.preview_interactive = interactive;
            (
                previous,
                client.preview_cols,
                client.preview_rows,
                client.preview_scroll_offset,
            )
        });
        if let Some(previous) = previous.filter(|previous| request.session_id.as_ref() != Some(previous)) {
            self.sync_preview_styled(&previous);
        }
        if let Some(session_id) = request.session_id.as_deref() {
            self.sync_preview_styled(session_id);
        }
        self.get_preview_record(request.session_id.as_deref(), cols, rows, scroll_offset, interactive)
            .await
    }

    /// Not in the TS: tells the session's worker whether to send styled previews at typing pace — while any client
    /// watching it types to it — each time that changes.
    pub fn sync_preview_styled(&self, session_id: &str) {
        let Some(worker) = self.worker_of(session_id) else {
            return;
        };
        let styled = self.server.borrow().clients.values().any(|client| {
            client.subscription.preview_interactive
                && client.subscription.watched_preview_session_id.as_deref() == Some(session_id)
        });
        if worker.preview_styled.replace(styled) == styled {
            return;
        }
        let mut payload = Map::new();
        payload.insert("type".to_owned(), Value::String("preview-mode".to_owned()));
        payload.insert("target".to_owned(), Value::String("agent".to_owned()));
        payload.insert("styled".to_owned(), Value::Bool(styled));
        worker.channel.send_event(payload);
    }

    /// TS: handleRequest `attach` (`session is not running`, `session is already attached elsewhere`): answers
    /// `{...session, ...attachData}` itself, then `attached` and the initial frame.
    pub async fn handle_attach(&self, conn: &Connection, request: &Attach) -> Result<()> {
        let Some(worker) = self.worker_of(&request.session_id) else {
            return Err(DhError::msg("session is not running"));
        };
        let Some(session) = self
            .held_session(&request.session_id)
            .filter(|session| session.status != SessionStatus::Exited)
        else {
            return Err(DhError::msg("session is not running"));
        };
        let elsewhere = worker
            .attached_agent
            .borrow()
            .as_ref()
            .is_some_and(|attached| attached.id != conn.client.id && !attached.is_destroyed());
        if elsewhere {
            return Err(DhError::msg("session is already attached elsewhere"));
        }
        // `clampSize(cols ?? DEFAULT, DEFAULT)`: absent and not a number alike are the default.
        let cols = clamp_size(request.cols, DEFAULT_PREVIEW_COLS);
        let rows = clamp_size(request.rows, DEFAULT_PREVIEW_ROWS);
        let payload = object(json!({"type": "attach", "target": "agent", "cols": cols, "rows": rows}));
        let attach_data: Value = self.send_worker_request(&request.session_id, payload).await?;
        let attach_data = object(attach_data);
        *worker.attached_agent.borrow_mut() = Some(conn.client.clone());
        *conn.attached_session_id.borrow_mut() = Some(session.id.clone());
        // `{...session, ...attachData}`: the snapshot's keys override the session's in place, new ones follow.
        let mut merged = object(session.to_value()?);
        let initial_frame = attach_data
            .get("initialFrame")
            .and_then(Value::as_str)
            .filter(|frame| !frame.is_empty())
            .map(str::to_owned);
        merged.extend(attach_data);
        send_message(&conn.client, &response(&request.request_id, &Value::Object(merged))?);
        send_message(
            &conn.client,
            &ServerMessage::Attached(SessionRef {
                session_id: session.id.clone(),
            }),
        );
        if let Some(data) = initial_frame {
            send_message(
                &conn.client,
                &ServerMessage::Output(SessionOutput {
                    session_id: session.id.clone(),
                    data,
                }),
            );
        }
        Ok(())
    }

    /// TS: handleRequest `input`: typed letters drop a pending draft; keys go to the agent; an answer to a prompt
    /// marks the session working again (in the background).
    pub fn handle_input(&self, request: &Input) -> Result<()> {
        // Text typed into the agent first wins over a task's draft (answering a menu with Enter or a digit does not).
        let has_draft = self
            .worker_of(&request.session_id)
            .is_some_and(|worker| worker.draft.borrow().is_some());
        // TS reads `message.data` as text here and in answersPrompt: anything else is a TypeError there (reported
        // only to a client that sent a requestId), after the keys went to the agent as sent.
        if has_draft {
            let data = request
                .data
                .known()
                .ok_or_else(|| replace_type_error(&request.data, "message.data"))?;
            if types_letters(data) {
                self.drop_draft(&request.session_id, Some("you typed first"), true);
            }
        }
        let mut event = object(json!({"type": "input", "target": "agent"}));
        if let Some(data) = request.data.to_value() {
            event.insert("data".to_owned(), data);
        }
        self.send_worker_event(&request.session_id, event);
        let data = request
            .data
            .known()
            .ok_or_else(|| replace_type_error(&request.data, "data"))?;
        if answers_prompt(data) {
            // markAnswered's synchronous part runs now, as TS calls it here: a hook queued behind this input finds
            // the answer's time already recorded. Only the save goes on in the background.
            let answered = self.mark_answered(&request.session_id);
            let daemon = self.clone();
            let session_id = request.session_id.clone();
            drop(spawn(async move {
                if let Err(error) = answered.await {
                    daemon.log_soon(format!("marking {session_id} answered failed: {error}"));
                }
            }));
        }
        Ok(())
    }

    /// TS: handleRequest `resize`.
    pub fn handle_resize(&self, request: &Resize) -> Result<()> {
        self.send_worker_event(&request.session_id, resize_event("agent", request));
        Ok(())
    }

    /// TS: handleRequest `detach`: releases this client's agent attach and answers `detached`.
    pub fn handle_detach(&self, conn: &Connection, request: &Detach) -> Result<()> {
        if let Some(worker) = self.worker_of(&request.session_id) {
            let mine = worker
                .attached_agent
                .borrow()
                .as_ref()
                .is_some_and(|attached| attached.id == conn.client.id);
            if mine {
                worker.attached_agent.borrow_mut().take();
            }
            self.send_worker_event(
                &request.session_id,
                object(json!({"type": "detach", "target": "agent"})),
            );
        }
        conn.attached_session_id.borrow_mut().take();
        send_message(
            &conn.client,
            &ServerMessage::Detached(SessionRef {
                session_id: request.session_id.clone(),
            }),
        );
        Ok(())
    }
}

/// V8's TypeError for `<expression>.replace(...)` on a value that is not text.
fn replace_type_error(value: &Loose<String>, expression: &str) -> DhError {
    DhError::msg(match value.to_value() {
        None => "Cannot read properties of undefined (reading 'replace')".to_owned(),
        Some(Value::Null) => "Cannot read properties of null (reading 'replace')".to_owned(),
        Some(_) => format!("{expression}.replace is not a function"),
    })
}

/// TS: `{type: 'resize', target, cols: message.cols, rows: message.rows}`, the sizes as sent (absent: no key).
pub(crate) fn resize_event(target: &str, request: &Resize) -> Map<String, Value> {
    let mut event = object(json!({"type": "resize", "target": target}));
    for (key, size) in [("cols", request.cols), ("rows", request.rows)] {
        if let Some(size) = size {
            event.insert(key.to_owned(), Value::from(size));
        }
    }
    event
}

/// TS `this.setupProcesses.get(sessionId) === child`.
/// A JS `void promise()` call: polls `future` once right here (its synchronous part runs in call order) and, if it
/// is still waiting, lets it finish as its own task. Never waits on it.
async fn start_detached(future: impl Future<Output = ()> + 'static) {
    let mut future = Box::pin(future);
    let pending = std::future::poll_fn(|cx| std::task::Poll::Ready(future.as_mut().poll(cx).is_pending())).await;
    if pending {
        drop(spawn(future));
    }
}

fn spawned_matches(processes: &HashMap<String, SetupProcess>, session_id: &str, process: SetupProcess) -> bool {
    process.pid.is_some() && processes.get(session_id) == Some(&process)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::{hello_line, local, session, test_client, test_daemon};
    use crate::worker_channel::ProcessExit;
    use deckhand_protocol::data::SessionWorktreeMode;
    use deckhand_protocol::wire::WatchPreview;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;
    use tokio::sync::{mpsc, oneshot};

    /// The other end of a fake session worker's channel (it already said hello).
    struct FakeWorker {
        theirs: BufReader<UnixStream>,
        exit: Option<oneshot::Sender<()>>,
    }

    impl FakeWorker {
        /// The next message the daemon sent.
        async fn next(&mut self) -> Value {
            let mut line = String::new();
            self.theirs.read_line(&mut line).await.unwrap();
            serde_json::from_str(&line).unwrap()
        }

        /// Answers `request` with `data`.
        async fn answer(&mut self, request: &Value, data: Value) {
            let answer = json!({"type": "response", "requestId": request["requestId"], "ok": true, "data": data});
            let line = format!("{answer}\n");
            self.theirs.get_mut().write_all(line.as_bytes()).await.unwrap();
        }

        fn exit(&mut self) {
            if let Some(exit) = self.exit.take() {
                let _ = exit.send(());
            }
        }
    }

    /// A session worker over a socketpair (no process), not registered anywhere.
    async fn fake_channel() -> (WorkerChannel, ChannelEvents, FakeWorker) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let (exit_tx, exit_rx) = oneshot::channel::<()>();
        let (channel, events) = WorkerChannel::attach(WorkerKind::Session, None, ours, async move {
            let _ = exit_rx.await;
            ProcessExit {
                code: Some(0),
                signal: None,
            }
        });
        let mut theirs = BufReader::new(theirs);
        theirs
            .get_mut()
            .write_all(format!("{}\n", hello_line()).as_bytes())
            .await
            .unwrap();
        let fake = FakeWorker {
            theirs,
            exit: Some(exit_tx),
        };
        (channel, events, fake)
    }

    /// A fake worker registered as `session_id`'s.
    async fn fake_worker(daemon: &Daemon, session_id: &str) -> (Rc<WorkerRuntime>, ChannelEvents, FakeWorker) {
        let (channel, events, fake) = fake_channel().await;
        let worker = Rc::new(WorkerRuntime::new(channel, "token".into(), "launch-1".into()));
        daemon
            .launch
            .borrow_mut()
            .workers
            .insert(session_id.to_owned(), Rc::clone(&worker));
        (worker, events, fake)
    }

    fn put(daemon: &Daemon, session: Ordered<SessionRecord>) {
        daemon
            .sessions
            .borrow_mut()
            .sessions
            .insert(session.id.clone(), session);
    }

    fn lines(rx: &mut mpsc::UnboundedReceiver<String>) -> Vec<Value> {
        let mut out = Vec::new();
        while let Ok(line) = rx.try_recv() {
            out.push(serde_json::from_str(&line).unwrap());
        }
        out
    }

    fn plan(kind: &str) -> LaunchPlan {
        let reference = json!({"provider": "claude", "kind": "id", "value": "p"});
        serde_json::from_value(match kind {
            "fork" => json!({"kind": "fork", "parent": reference, "name": "n"}),
            "resume" => json!({"kind": "resume", "ref": reference}),
            _ => json!({"kind": "new", "name": "n"}),
        })
        .unwrap()
    }

    #[test]
    fn agent_names_are_safe_and_short() {
        assert_eq!(
            build_deckhand_agent_name("Fix the Login Bug!", "12345678-abcd", None),
            "dh-fix_the_login_bug-12345678"
        );
        assert_eq!(
            build_deckhand_agent_name("feature/new thing", "abcdef", Some("fork")),
            "dh-feature-new_thing-abcdef-fork"
        );
        assert_eq!(build_deckhand_agent_name("!!!", "id", Some("")), "dh-worktree-id");
        assert_eq!(build_deckhand_agent_name("__/--", "s", None), "dh-worktree-s");
        let long = "a".repeat(39) + "_b";
        // Cut at 40, then the trailing underscore goes.
        assert_eq!(
            build_deckhand_agent_name(&long, "0123456789", None),
            format!("dh-{}-01234567", "a".repeat(39))
        );
    }

    #[test]
    fn prompts_and_preview_records_match_the_ts_text() {
        assert_eq!(
            handoff_prompt("/h/doc.md"),
            "Read the handoff document at /h/doc.md. Use it as task context, inspect the workspace, and continue the work described there. Keep all normal permission checks."
        );
        let mut child = session("c", "/repo", "/wt");
        assert_eq!(
            moved_fork_prompt(&child, "/repo", "/wt"),
            "Deckhand note: this conversation was forked from another session into a different worktree. You are now in /wt. Earlier messages refer to files under /repo, which is the parent session's worktree: do not read or edit anything there. Work only in /wt. Changes the parent had not committed are not in this worktree. Reply briefly to confirm, then wait for instructions."
        );
        let mut worktree = deckhand_protocol::data::SessionWorktreeRecord::new(SessionWorktreeMode::Managed);
        worktree.branch = Some("feat".into());
        child.worktree = Some(worktree);
        assert!(moved_fork_prompt(&child, "/repo", "/wt").contains("You are now in /wt on branch feat. Earlier"));

        let record = build_preview_record(&child, "screen", 2, 9);
        assert_eq!(
            serde_json::to_value(&record).unwrap().to_string(),
            r#"{"sessionId":"c","content":"screen","live":true,"status":"running","scrollOffset":2,"maxScrollOffset":9}"#
        );
        child.status = SessionStatus::Exited;
        assert!(!build_preview_record(&child, "", 0, 0).live);
    }

    #[test]
    fn the_shell_is_shell_or_sh() {
        let expected = std::env::var("SHELL")
            .ok()
            .filter(|shell| !shell.is_empty())
            .unwrap_or_else(|| "/bin/sh".into());
        assert_eq!(resolve_shell_command(), expected);
    }

    #[test]
    fn a_path_ref_gets_its_directory() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("a/b/session.jsonl");
            let reference: AgentSessionRef =
                serde_json::from_value(json!({"provider": "pi", "kind": "path", "value": file.to_string_lossy()}))
                    .unwrap();
            prepare_agent_session_ref(Some(&reference)).await.unwrap();
            assert!(dir.path().join("a/b").is_dir());
            assert!(!file.exists());
            prepare_agent_session_ref(None).await.unwrap();
            let named: AgentSessionRef =
                serde_json::from_value(json!({"provider": "claude", "kind": "id", "value": "/nope/x/y"})).unwrap();
            prepare_agent_session_ref(Some(&named)).await.unwrap();
            assert!(!std::path::Path::new("/nope").exists());
        });
    }

    #[test]
    fn a_launch_is_current_only_while_starting() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = test_daemon(dir.path());
        assert!(!daemon.is_current_launch("s", Some("l1")));
        let mut starting = session("s", "/repo", "/repo");
        starting.status = SessionStatus::Starting;
        starting.launch_id = Some("l1".into());
        put(&daemon, starting.clone());
        assert!(daemon.is_current_launch("s", Some("l1")));
        assert!(!daemon.is_current_launch("s", Some("l2")));
        assert!(!daemon.is_current_launch("s", None));
        assert!(daemon.assert_current_launch("s", Some("l1")).is_ok());
        assert_eq!(
            daemon.assert_current_launch("s", Some("l2")).unwrap_err().to_string(),
            "Startup cancelled"
        );
        starting.status = SessionStatus::Running;
        put(&daemon, starting);
        assert!(!daemon.is_current_launch("s", Some("l1")));
    }

    #[test]
    fn first_messages_name_the_handoff_or_the_moved_fork() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = test_daemon(dir.path());
        let mut child = session("c", "/repo", "/wt/child");
        child.handoff_path = Some("/h.md".into());
        assert_eq!(
            daemon.first_message_args(&child, &plan("new"), true),
            vec!["--".to_owned(), handoff_prompt("/h.md")]
        );
        assert!(daemon.first_message_args(&child, &plan("new"), false).is_empty());

        child.handoff_path = None;
        child.forked_from_session_id = Some("p".into());
        child.launch_worktree_root = Some("/wt/child".into());
        // No parent known: nothing.
        assert!(daemon.first_message_args(&child, &plan("fork"), true).is_empty());
        let mut parent = session("p", "/repo", "/repo");
        parent.launch_worktree_root = Some("/repo".into());
        put(&daemon, parent.clone());
        assert_eq!(
            daemon.first_message_args(&child, &plan("fork"), false),
            vec!["--".to_owned(), moved_fork_prompt(&child, "/repo", "/wt/child")]
        );
        // Not a fork, or the same worktree: nothing.
        assert!(daemon.first_message_args(&child, &plan("resume"), false).is_empty());
        parent.launch_worktree_root = Some("/wt/child".into());
        put(&daemon, parent);
        assert!(daemon.first_message_args(&child, &plan("fork"), false).is_empty());
    }

    #[test]
    fn a_cached_agent_binary_is_used_while_it_exists() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let daemon = test_daemon(dir.path());
            let binary = dir.path().join("claude");
            std::fs::write(&binary, "").unwrap();
            let binary = binary.to_string_lossy().into_owned();
            daemon
                .launch
                .borrow_mut()
                .program_command_cache
                .insert(ProgramKey::Claude, binary.clone());
            assert_eq!(daemon.resolve_program_command(ProgramKey::Claude).await, binary);
            let gone = dir.path().join("gone/codex").to_string_lossy().into_owned();
            daemon
                .launch
                .borrow_mut()
                .program_command_cache
                .insert(ProgramKey::Codex, gone.clone());
            // Looked up again (on PATH, else the bare name); the missing path is forgotten.
            let resolved = daemon.resolve_program_command(ProgramKey::Codex).await;
            assert_ne!(resolved, gone);
            let cached = daemon
                .launch
                .borrow()
                .program_command_cache
                .get(&ProgramKey::Codex)
                .cloned();
            assert_ne!(cached.as_deref(), Some(gone.as_str()));
            if resolved == "codex" {
                assert!(cached.is_none());
            }
        });
    }

    #[test]
    fn only_a_resumed_claude_conversation_is_followed() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let daemon = test_daemon(dir.path());
            let mut resumed = session("s", "/repo", "/repo");
            // No ref, a fresh restart, a never-started session, another agent or a fork: unchanged, nothing read.
            let same = daemon
                .continued_conversation(resumed.clone(), RestartMode::Resume, false)
                .await;
            assert_eq!(same, resumed);
            resumed.agent_session_ref =
                Some(serde_json::from_value(json!({"provider": "claude", "kind": "id", "value": "a"})).unwrap());
            for (mode, never_started) in [(RestartMode::Fresh, false), (RestartMode::Resume, true)] {
                let same = daemon
                    .continued_conversation(resumed.clone(), mode, never_started)
                    .await;
                assert_eq!(same, resumed);
            }
            let mut pi = resumed.clone();
            pi.program = ProgramKey::Pi;
            assert_eq!(
                daemon
                    .continued_conversation(pi.clone(), RestartMode::Resume, false)
                    .await,
                pi
            );
            let mut forked = resumed.clone();
            forked.sub_session_kind = Some(deckhand_protocol::data::SubSessionKind::Forked);
            forked.forked_from_agent_session_ref = forked.agent_session_ref.clone();
            assert_eq!(
                daemon
                    .continued_conversation(forked.clone(), RestartMode::Resume, false)
                    .await,
                forked
            );
        });
    }

    #[test]
    fn previews_come_from_the_worker_else_the_stored_screen() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let daemon = test_daemon(dir.path());
            let none = daemon.get_preview_record(None, 80, 24, 0, false).await.unwrap();
            assert_eq!(serde_json::to_string(&none).unwrap(), r#"{"content":"","live":false}"#);
            let unknown = daemon.get_preview_record(Some("x"), 80, 24, 0, false).await.unwrap();
            assert_eq!(
                serde_json::to_string(&unknown).unwrap(),
                r#"{"sessionId":"x","content":"","live":false}"#
            );
            let mut stored = session("s", "/repo", "/repo");
            stored.status = SessionStatus::Exited;
            stored.last_preview = Some("bye".into());
            put(&daemon, stored);
            let preview = daemon.get_preview_record(Some("s"), 80, 24, 3, false).await.unwrap();
            assert_eq!(
                serde_json::to_string(&preview).unwrap(),
                r#"{"sessionId":"s","content":"bye","live":false,"status":"exited","scrollOffset":0,"maxScrollOffset":0}"#
            );

            let (_worker, _events, mut fake) = fake_worker(&daemon, "s").await;
            let request = daemon.get_preview_record(Some("s"), 100, 30, 2, false);
            let (preview, ()) = tokio::join!(request, async {
                let sent = fake.next().await;
                let keys: Vec<&String> = sent.as_object().unwrap().keys().collect();
                assert_eq!(keys, ["type", "target", "cols", "rows", "scrollOffset", "requestId"]);
                assert_eq!(sent["cols"], 100);
                assert_eq!(sent["scrollOffset"], 2);
                fake.answer(&sent, json!({"sessionId": "s", "content": "live", "live": true}))
                    .await;
            });
            assert_eq!(preview.unwrap().content, "live");
            fake.exit();
        });
    }

    #[test]
    fn watching_a_preview_records_the_clamped_sizes() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let daemon = test_daemon(dir.path());
            let (client, _rx) = test_client(1);
            let conn = Connection::new(client.clone());
            let request = WatchPreview {
                request_id: "r".into(),
                session_id: Some("x".into()),
                cols: Some(0),
                rows: Some(50),
                scroll_offset: Some(-4),
                interactive: None,
            };
            let preview = daemon.handle_watch_preview(&conn, &request).await.unwrap();
            assert_eq!(preview.session_id.as_deref(), Some("x"));
            let subscription = daemon.get_client(&client, |client| client.clone());
            assert_eq!(subscription.watched_preview_session_id.as_deref(), Some("x"));
            assert_eq!(
                (
                    subscription.preview_cols,
                    subscription.preview_rows,
                    subscription.preview_scroll_offset
                ),
                (1, 50, 0)
            );
        });
    }

    fn watch(session_id: &str, interactive: bool) -> WatchPreview {
        WatchPreview {
            request_id: "r".into(),
            session_id: Some(session_id.into()),
            cols: Some(80),
            rows: Some(24),
            scroll_offset: Some(0),
            interactive: interactive.then_some(true),
        }
    }

    /// Runs `watch` while the fake worker answers its snapshot; returns the messages the worker got, the snapshot
    /// request last.
    async fn watch_answered(
        daemon: &Daemon,
        conn: &Connection,
        request: WatchPreview,
        fake: &mut FakeWorker,
    ) -> Vec<Value> {
        let (preview, sent) = tokio::join!(daemon.handle_watch_preview(conn, &request), async {
            let mut sent = Vec::new();
            loop {
                let message = fake.next().await;
                sent.push(message.clone());
                if message["type"] == "snapshot" {
                    fake.answer(&message, json!({"sessionId": "s", "content": "live", "live": true}))
                        .await;
                    return sent;
                }
            }
        });
        preview.unwrap();
        sent
    }

    fn preview_mode(styled: bool) -> Value {
        json!({"type": "preview-mode", "target": "agent", "styled": styled})
    }

    #[test]
    fn interactive_watches_switch_the_workers_preview_mode() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let daemon = test_daemon(dir.path());
            put(&daemon, session("s", "/repo", "/repo"));
            put(&daemon, session("t", "/repo", "/repo"));
            let (worker, _events, mut fake) = fake_worker(&daemon, "s").await;
            let (first, _first_rx) = test_client(1);
            let first = Connection::new(first);
            let (second, _second_rx) = test_client(2);
            let second = Connection::new(second);

            // A plain watch leaves the mode alone and asks for no styled screen.
            let sent = watch_answered(&daemon, &first, watch("s", false), &mut fake).await;
            assert_eq!(sent.len(), 1);
            assert!(sent[0].get("styled").is_none(), "{}", sent[0]);

            // Going interactive switches it on once, and the answer already carries the styled screen.
            let sent = watch_answered(&daemon, &first, watch("s", true), &mut fake).await;
            assert_eq!(sent[0], preview_mode(true));
            assert_eq!(sent[1]["styled"], true);
            assert!(worker.preview_styled.get());
            let sent = watch_answered(&daemon, &first, watch("s", true), &mut fake).await;
            assert_eq!(sent.len(), 1, "already on: {sent:?}");
            let sent = watch_answered(&daemon, &second, watch("s", true), &mut fake).await;
            assert_eq!(sent.len(), 1, "already on: {sent:?}");

            // The first client moves on; the second still types to the agent.
            daemon.handle_watch_preview(&first, &watch("t", false)).await.unwrap();
            assert!(worker.preview_styled.get());
            // The second disconnects: nobody types any more.
            daemon.client_disconnected(&second);
            assert_eq!(fake.next().await, preview_mode(false));
            assert!(!worker.preview_styled.get());

            // Switching sessions while interactive turns the old one off.
            let sent = watch_answered(&daemon, &first, watch("s", true), &mut fake).await;
            assert_eq!(sent[0], preview_mode(true));
            daemon.handle_watch_preview(&first, &watch("t", true)).await.unwrap();
            assert_eq!(fake.next().await, preview_mode(false));
            fake.exit();
        });
    }

    #[test]
    fn only_interactive_watchers_get_the_styled_screen() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let daemon = test_daemon(dir.path());
            put(&daemon, session("s", "/repo", "/repo"));
            let (worker, _events, mut fake) = fake_worker(&daemon, "s").await;
            let (typing, mut typing_rx) = test_client(1);
            daemon.get_client(&typing, |client| {
                client.watched_preview_session_id = Some("s".into());
                client.preview_interactive = true;
            });
            let (plain, mut plain_rx) = test_client(2);
            daemon.get_client(&plain, |client| {
                client.watched_preview_session_id = Some("s".into());
            });
            let screen = json!({"lines": [[[0, 2, "hi", "p16:1", "d", ""]]], "modes": {"bracketedPaste": false, "applicationCursorKeys": false, "mouseTracking": false}});
            let preview = json!({"sessionId": "s", "content": "hi", "live": true, "screen": screen});
            let updated: WorkerMessage =
                serde_json::from_value(json!({"type": "preview-updated", "preview": preview})).unwrap();
            daemon
                .handle_worker_message("s", updated.clone(), &worker)
                .await
                .unwrap();
            assert_eq!(
                lines(&mut typing_rx),
                vec![json!({"type": "preview-updated", "preview": preview})]
            );
            assert_eq!(
                lines(&mut plain_rx),
                vec![json!({"type": "preview-updated", "preview": {"sessionId": "s", "content": "hi", "live": true}})]
            );

            // A scrolled interactive watcher's own snapshot asks for the styled screen; a plain one's does not.
            daemon.get_client(&typing, |client| client.preview_scroll_offset = 5);
            daemon.get_client(&plain, |client| client.preview_scroll_offset = 5);
            let (handled, ()) = tokio::join!(daemon.handle_worker_message("s", updated, &worker), async {
                let sent = fake.next().await;
                assert_eq!(sent["styled"], true);
                fake.answer(&sent, json!({"sessionId": "s", "content": "older", "live": true}))
                    .await;
                let sent = fake.next().await;
                assert!(sent.get("styled").is_none(), "{sent}");
                fake.answer(&sent, json!({"sessionId": "s", "content": "older", "live": true}))
                    .await;
            });
            handled.unwrap();
            fake.exit();
        });
    }

    #[test]
    fn requests_and_events_need_the_sessions_worker() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let daemon = test_daemon(dir.path());
            let error = daemon
                .send_worker_request::<Value>("nope", Map::new())
                .await
                .unwrap_err();
            assert_eq!(error.to_string(), "session worker is not running");
            // Nothing to send to: nothing happens.
            daemon.send_worker_event("nope", object(json!({"type": "detach", "target": "agent"})));
            daemon
                .handle_resize(&Resize {
                    session_id: "nope".into(),
                    cols: Some(1),
                    rows: Some(1),
                })
                .unwrap();
        });
    }

    #[test]
    fn attach_input_resize_and_detach_go_through_the_worker() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let daemon = test_daemon(dir.path());
            let (client, mut rx) = test_client(1);
            let conn = Connection::new(client.clone());
            let attach = Attach {
                request_id: "r1".into(),
                session_id: "s".into(),
                cols: Some(0),
                rows: None,
                process: None,
                shell: None,
            };
            let not_running = daemon.handle_attach(&conn, &attach).await.unwrap_err();
            assert_eq!(not_running.to_string(), "session is not running");

            let mut running = session("s", "/repo", "/repo");
            running.status = SessionStatus::Exited;
            put(&daemon, running.clone());
            let (worker, _events, mut fake) = fake_worker(&daemon, "s").await;
            let exited = daemon.handle_attach(&conn, &attach).await.unwrap_err();
            assert_eq!(exited.to_string(), "session is not running");

            running.status = SessionStatus::Running;
            put(&daemon, running);
            let (other, _other_rx) = test_client(2);
            *worker.attached_agent.borrow_mut() = Some(other.clone());
            let elsewhere = daemon.handle_attach(&conn, &attach).await.unwrap_err();
            assert_eq!(elsewhere.to_string(), "session is already attached elsewhere");
            // A destroyed client holds nothing.
            other.destroy();

            let (attached, ()) = tokio::join!(daemon.handle_attach(&conn, &attach), async {
                let sent = fake.next().await;
                assert_eq!(
                    sent.to_string(),
                    format!(
                        r#"{{"type":"attach","target":"agent","cols":1,"rows":24,"requestId":{}}}"#,
                        sent["requestId"]
                    )
                );
                fake.answer(
                    &sent,
                    json!({"sessionId": "s", "content": "x", "live": true, "status": "running", "initialFrame": "F"}),
                )
                .await;
            });
            attached.unwrap();
            assert_eq!(conn.attached_session_id.borrow().as_deref(), Some("s"));
            assert_eq!(worker.attached_agent.borrow().as_ref().map(|c| c.id), Some(client.id));
            let sent = lines(&mut rx);
            assert_eq!(sent.len(), 3);
            let data = &sent[0]["data"];
            let keys: Vec<&String> = data.as_object().unwrap().keys().collect();
            assert_eq!(
                keys,
                [
                    "id",
                    "title",
                    "program",
                    "command",
                    "cwd",
                    "repoRoot",
                    "status",
                    "createdAt",
                    "updatedAt",
                    "sessionId",
                    "content",
                    "live",
                    "initialFrame"
                ]
            );
            assert_eq!(sent[0]["requestId"], "r1");
            assert_eq!(sent[1], json!({"type": "attached", "sessionId": "s"}));
            assert_eq!(sent[2], json!({"type": "output", "sessionId": "s", "data": "F"}));

            daemon
                .handle_input(&Input {
                    session_id: "s".into(),
                    data: "ls".into(),
                    shell: None,
                })
                .unwrap();
            assert_eq!(
                fake.next().await.to_string(),
                r#"{"type":"input","target":"agent","data":"ls"}"#
            );
            daemon
                .handle_resize(&Resize {
                    session_id: "s".into(),
                    cols: Some(90),
                    rows: Some(20),
                })
                .unwrap();
            assert_eq!(
                fake.next().await.to_string(),
                r#"{"type":"resize","target":"agent","cols":90,"rows":20}"#
            );
            daemon.handle_detach(&conn, &Detach { session_id: "s".into() }).unwrap();
            assert_eq!(fake.next().await.to_string(), r#"{"type":"detach","target":"agent"}"#);
            assert!(worker.attached_agent.borrow().is_none());
            assert!(conn.attached_session_id.borrow().is_none());
            assert_eq!(lines(&mut rx), vec![json!({"type": "detached", "sessionId": "s"})]);
            fake.exit();
        });
    }

    #[test]
    fn worker_output_and_previews_reach_their_clients() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let daemon = test_daemon(dir.path());
            put(&daemon, session("s", "/repo", "/repo"));
            let (worker, _events, mut fake) = fake_worker(&daemon, "s").await;
            let (attached, mut attached_rx) = test_client(1);
            *worker.attached_agent.borrow_mut() = Some(attached);
            let (watcher, mut watcher_rx) = test_client(2);
            daemon.get_client(&watcher, |client| {
                client.watched_preview_session_id = Some("s".into());
            });
            let (other, mut other_rx) = test_client(3);
            daemon.get_client(&other, |_| {});

            let output: WorkerMessage =
                serde_json::from_value(json!({"type": "output", "target": "agent", "data": "hi"})).unwrap();
            daemon.handle_worker_message("s", output, &worker).await.unwrap();
            assert_eq!(
                lines(&mut attached_rx),
                vec![json!({"type": "output", "sessionId": "s", "data": "hi"})]
            );

            let preview = json!({"sessionId": "s", "content": "screen", "live": true});
            let updated: WorkerMessage =
                serde_json::from_value(json!({"type": "preview-updated", "preview": preview})).unwrap();
            daemon
                .handle_worker_message("s", updated.clone(), &worker)
                .await
                .unwrap();
            assert_eq!(worker.screen.borrow().as_deref(), Some("screen"));
            assert_eq!(
                lines(&mut watcher_rx),
                vec![json!({"type": "preview-updated", "preview": preview})]
            );
            assert!(lines(&mut other_rx).is_empty());

            // A scrolled watcher gets its own snapshot.
            daemon.get_client(&watcher, |client| client.preview_scroll_offset = 5);
            let (handled, ()) = tokio::join!(daemon.handle_worker_message("s", updated.clone(), &worker), async {
                let sent = fake.next().await;
                assert_eq!(sent["type"], "snapshot");
                assert_eq!(sent["scrollOffset"], 5);
                fake.answer(&sent, json!({"sessionId": "s", "content": "older", "live": true}))
                    .await;
            });
            handled.unwrap();
            assert_eq!(lines(&mut watcher_rx)[0]["preview"]["content"], "older");

            // A replaced worker's messages are ignored.
            let (stale_channel, _stale_events, mut stale_fake) = fake_channel().await;
            let stale = Rc::new(WorkerRuntime::new(stale_channel, "t".into(), "l".into()));
            daemon.handle_worker_message("s", updated, &stale).await.unwrap();
            assert!(stale.screen.borrow().is_none());
            assert!(lines(&mut watcher_rx).is_empty());
            fake.exit();
            stale_fake.exit();
        });
    }

    #[test]
    fn a_worker_exit_fails_its_requests_and_retires_it() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let daemon = test_daemon(dir.path());
            let mut exited = session("s", "/repo", "/repo");
            exited.status = SessionStatus::Exited;
            put(&daemon, exited);
            std::fs::create_dir_all(&daemon.paths.worker_dir).unwrap();
            let pid_file = daemon.paths.worker_dir.join("s.pid");
            std::fs::write(&pid_file, "1").unwrap();
            let (worker, events, mut fake) = fake_worker(&daemon, "s").await;
            let pump = spawn({
                let daemon = daemon.clone();
                let worker = Rc::clone(&worker);
                async move { daemon.pump_worker_events("s".into(), worker, events).await }
            });
            let request = daemon.send_worker_request::<Value>("s", object(json!({"type": "snapshot"})));
            let (result, ()) = tokio::join!(request, async {
                let _ = fake.next().await;
                fake.exit();
            });
            assert_eq!(result.unwrap_err().to_string(), "session worker exited");
            pump.await;
            assert!(worker.channel.is_exited());
            assert!(daemon.launch.borrow().workers.is_empty());
            assert!(!pid_file.exists());
        });
    }

    #[test]
    fn a_handler_waiting_on_its_worker_blocks_neither_output_nor_the_exit() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let daemon = test_daemon(dir.path());
            let mut exited = session("s", "/repo", "/repo");
            exited.status = SessionStatus::Exited;
            put(&daemon, exited);
            std::fs::create_dir_all(&daemon.paths.worker_dir).unwrap();
            let pid_file = daemon.paths.worker_dir.join("s.pid");
            std::fs::write(&pid_file, "1").unwrap();
            let (worker, events, mut fake) = fake_worker(&daemon, "s").await;
            let (attached, mut attached_rx) = test_client(1);
            *worker.attached_agent.borrow_mut() = Some(attached);
            let (watcher, mut watcher_rx) = test_client(2);
            daemon.get_client(&watcher, |client| {
                client.watched_preview_session_id = Some("s".into());
                client.preview_scroll_offset = 5;
            });
            let pump = spawn({
                let daemon = daemon.clone();
                let worker = Rc::clone(&worker);
                async move { daemon.pump_worker_events("s".into(), worker, events).await }
            });
            // The scrolled watcher's snapshot request stays unanswered while the agent's output follows it.
            let preview = json!({"sessionId": "s", "content": "screen", "live": true});
            let sent = format!(
                "{}\n{}\n",
                json!({"type": "preview-updated", "preview": preview}),
                json!({"type": "output", "target": "agent", "data": "hi"})
            );
            fake.theirs.get_mut().write_all(sent.as_bytes()).await.unwrap();
            let snapshot = fake.next().await;
            assert_eq!(snapshot["type"], "snapshot");
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            let output = loop {
                let output = lines(&mut attached_rx);
                if !output.is_empty() || tokio::time::Instant::now() > deadline {
                    break output;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            };
            assert_eq!(output, vec![json!({"type": "output", "sessionId": "s", "data": "hi"})]);
            // The worker's exit fails the outstanding snapshot at once and retires it.
            fake.exit();
            tokio::time::timeout(Duration::from_secs(2), pump)
                .await
                .expect("the exit is handled behind no handler");
            assert!(worker.channel.is_exited());
            assert!(daemon.launch.borrow().workers.is_empty());
            assert!(!pid_file.exists());
            assert!(lines(&mut watcher_rx).is_empty());
        });
    }

    #[test]
    fn a_replaced_workers_exit_only_fails_its_own_requests() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let daemon = test_daemon(dir.path());
            let (channel, events, mut fake) = fake_channel().await;
            let old = Rc::new(WorkerRuntime::new(channel, "t".into(), "l".into()));
            let (current, _current_events, mut current_fake) = fake_worker(&daemon, "s").await;
            let pump = spawn({
                let daemon = daemon.clone();
                let old = Rc::clone(&old);
                async move { daemon.pump_worker_events("s".into(), old, events).await }
            });
            let request = old.channel.request::<Value>(object(json!({"type": "snapshot"})));
            let (result, ()) = tokio::join!(request, async {
                let _ = fake.next().await;
                fake.exit();
            });
            assert_eq!(result.unwrap_err().to_string(), "session worker exited");
            pump.await;
            assert!(!current.channel.is_exited());
            assert_eq!(daemon.launch.borrow().workers.len(), 1);
            current_fake.exit();
        });
    }

    #[test]
    fn stopping_a_worker_asks_it_to_kill_its_agent() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let daemon = test_daemon(dir.path());
            let (worker, _events, mut fake) = fake_worker(&daemon, "s").await;
            daemon.stop_worker(&worker);
            let sent = fake.next().await;
            let keys: Vec<&String> = sent.as_object().unwrap().keys().collect();
            assert_eq!(keys, ["type", "requestId", "force"]);
            assert_eq!(sent["type"], "kill");
            assert_eq!(sent["force"], true);
            fake.exit();
        });
    }

    #[test]
    fn setup_helpers_match_the_ts() {
        let mut processes = HashMap::new();
        let child = SetupProcess { pid: Some(7) };
        assert!(!spawned_matches(&processes, "s", child));
        processes.insert("s".to_owned(), child);
        assert!(spawned_matches(&processes, "s", child));
        assert!(!spawned_matches(&processes, "s", SetupProcess { pid: Some(8) }));
        assert!(!spawned_matches(&processes, "s", SetupProcess { pid: None }));
        assert_eq!(last_units("abcdef", 4), "cdef");
        assert_eq!(last_units("ab", 4), "ab");
        assert_eq!(
            spawn_error("/bin/zsh", &std::io::Error::from_raw_os_error(libc::ENOENT)).to_string(),
            "spawn /bin/zsh ENOENT"
        );
    }

    // Regression (forks.test.ts / daemonFeatures.test.ts under load): the agent says running, is killed and exits
    // while the launch still reads a changed binary's `--version`; the launch must not then answer it as running
    // (its caller's save revived the exited record, so the kill never showed as exited).
    #[test]
    fn an_exit_during_the_version_read_is_not_revived_as_running() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let _home = crate::hooks::tests::Home::set(dir.path());
        let executable = |name: &str, text: &str| {
            let file = dir.path().join(name);
            std::fs::write(&file, text).unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
            file
        };
        let agent = executable("codex", "#!/bin/sh\nsleep 1\necho 'codex-cli 1.2.3'\n");
        let cwd = dir.path().join("repo");
        std::fs::create_dir_all(&cwd).unwrap();
        let cwd = cwd.to_str().unwrap().to_owned();
        let mut starting = session("s", &cwd, &cwd);
        starting.command = agent.to_str().unwrap().to_owned();
        starting.status = SessionStatus::Starting;
        starting.launch_id = Some("l1".into());
        let data = dir.path().join("started.json");
        std::fs::write(&data, serde_json::to_string(&starting).unwrap()).unwrap();
        // Answers `start` as running at once, then the agent exits 0.3 s later (well inside the 1 s version read).
        let worker = executable(
            "worker.sh",
            &format!(
                r#"#!/bin/sh
printf '{hello}\n' >&3
while IFS= read -r line <&3; do
  id=$(printf '%s' "$line" | sed -n 's/.*"requestId":"\([^"]*\)".*/\1/p')
  case "$line" in
    *'"type":"start"'*)
      printf '{{"type":"running","pid":%s}}\n' $$ >&3
      printf '{{"type":"response","requestId":"%s","ok":true,"data":' "$id" >&3
      cat '{data}' >&3
      printf '}}\n' >&3
      sleep 0.3
      printf '{{"type":"exit","exitCode":0,"exitSignal":15,"lastPreview":""}}\n' >&3
      exit 0 ;;
  esac
done
"#,
                data = data.display(),
                hello = hello_line(),
            ),
        );
        let daemon = Daemon::with_paths(
            crate::daemon::DaemonOptions {
                worker_exe: Some(worker),
            },
            test_daemon(dir.path()).paths.clone(),
        );
        daemon.notes_tasks.borrow_mut().notes = None;
        put(&daemon, starting.clone());
        local(async {
            let result = daemon.start_worker(starting, 80, 24, true).await;
            assert_eq!(result.unwrap_err().to_string(), "Startup cancelled");
            let held = daemon.get_session("s").unwrap();
            assert_eq!(held.status, SessionStatus::Exited);
            assert_eq!(held.exit_signal, Some(Some(15)));
            assert_eq!(held.agent_version.as_deref(), Some("1.2.3"));
        });
    }
}
