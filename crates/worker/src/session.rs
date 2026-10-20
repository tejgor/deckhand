//! Port of sessionWorker.ts (session worker: agent PTY, preview debounce, activity heuristic).
//!
//! A session worker owns one session's agent PTY; its companion panes belong to the session's workspace worker
//! ([`crate::workspace`]). It mirrors the agent's output into a headless [`TerminalPreview`], posts debounced
//! `preview-updated` snapshots, derives `agent-status` (active/idle) from how much the preview changes, and streams
//! raw output while the daemon is attached. The helpers shared with the workspace worker (PTY spawn and pump,
//! [`RuntimePty`], sizes, signals, terminal modes) live here too, because W2 builds on W1.
//!
//! Time: `Date.now()` comparisons use `tokio::time::Instant`, so tests can pause the clock.
//!
//! Exiting: TS called `process.exit(0)` from timers. Here the worker asks [`SessionWorker::start`] to return instead
//! (after [`EXIT_DELAY`] once the agent exited, or after the disconnect's kill escalation); [`run_session_worker`]
//! then flushes the channel and returns, and the binary exits with status 0.

use std::cell::RefCell;
use std::ffi::OsString;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use nix::sys::signal::Signal;
use tokio::sync::watch;
use tokio::time::Instant;

use deckhand_core::error::DhError;
use deckhand_core::jsstr::date;
use deckhand_protocol::Ordered;
use deckhand_protocol::WorkerCommand;
use deckhand_protocol::data::{
    AgentActivityStatus, AttachSnapshot, AttachTarget, OkResult, PreviewRecord, ProgramKey, SessionRecord,
    SessionStatus, TerminalModes,
};
use deckhand_protocol::worker_ipc::{
    AgentExit, AgentStatusChanged, Running, WorkerMessage, WorkerOutput, WorkerPreviewUpdated,
};
use deckhand_sys::async_util::{Task, Timer, spawn};
use deckhand_term::preview::{ScrollInfo, TerminalPreview};
use deckhand_term::pty::{Pty, PtyExit, PtyOptions};
use deckhand_term::utf8::Utf8Decoder;

use crate::channel::{ChannelReader, SessionIncoming, WireRole, WorkerChannel, decode_session_command};

/// TS: sessionWorker.ts DEFAULT_COLS
pub const DEFAULT_COLS: u16 = 80;
/// TS: sessionWorker.ts DEFAULT_ROWS
pub const DEFAULT_ROWS: u16 = 24;
/// The `name` every worker PTY is spawned with (`TERM` of the child).
pub const PTY_TERM_NAME: &str = "xterm-256color";
/// TS: sessionWorker.ts PREVIEW_BROADCAST_DELAY_MS (75 ms debounce of `preview-updated` / pane updates).
pub const PREVIEW_BROADCAST_DELAY: Duration = Duration::from_millis(75);
/// TS: sessionWorker.ts ACTIVITY_EVALUATION_DELAY_MS
pub const ACTIVITY_EVALUATION_DELAY: Duration = Duration::from_millis(150);
/// TS: sessionWorker.ts ACTIVITY_WINDOW_MS
pub const ACTIVITY_WINDOW: Duration = Duration::from_millis(3000);
/// TS: sessionWorker.ts IDLE_AFTER_MS
pub const IDLE_AFTER: Duration = Duration::from_millis(5000);
/// TS: sessionWorker.ts ACTIVE_MIN_CHANGED_CHARS
pub const ACTIVE_MIN_CHANGED_CHARS: usize = 1;
/// TS: sessionWorker.ts RESIZE_ACTIVITY_SUPPRESSION_MS
pub const RESIZE_ACTIVITY_SUPPRESSION: Duration = Duration::from_millis(750);
/// TS: sessionWorker.ts EXIT_OUTPUT_QUIET_MS
pub const EXIT_OUTPUT_QUIET: Duration = Duration::from_millis(60);
/// TS: sessionWorker.ts EXIT_OUTPUT_MAX_WAIT_MS
pub const EXIT_OUTPUT_MAX_WAIT: Duration = Duration::from_millis(400);
/// TS: sessionWorker.ts waitForQuietOutput poll interval (20 ms).
pub const EXIT_OUTPUT_POLL: Duration = Duration::from_millis(20);
/// TS: sessionWorker.ts kill/disconnect: SIGKILL follows SIGTERM after 1 s.
pub const KILL_ESCALATION: Duration = Duration::from_millis(1000);
/// TS: sessionWorker.ts `setTimeout(() => process.exit(0), 25)` after the agent exit or a workspace shutdown.
pub const EXIT_DELAY: Duration = Duration::from_millis(25);
/// node-pty: `onExit` fires once the PTY socket closed, or 200 ms after the child exited. [`pump_pty`] drains output
/// to EOF for at most this long after the exit before reporting it.
pub const PTY_EOF_WAIT: Duration = Duration::from_millis(200);

/// How much one PTY read takes at most.
const PTY_READ_CHUNK: usize = 64 * 1024;

/// TS: sessionWorker.ts RuntimePty: one PTY, its headless preview and what its pane record reports.
pub struct RuntimePty {
    /// Shared so a caller can clone it out of the `RefCell` and await a write or exit without holding the borrow.
    pub term: Rc<Pty>,
    pub preview: TerminalPreview,
    pub cwd: String,
    pub exited: bool,
    pub terminal_modes: TerminalModes,
    /// `exitCode?: number | null`: `None` while running (omitted from records), `Some(None)` for `null`.
    pub exit_code: Option<Option<i64>>,
    /// `exitSignal?: number | null`, like `exit_code`.
    pub exit_signal: Option<Option<i64>>,
    /// The pending debounced broadcast (`broadcastTimer`).
    pub broadcast_timer: Option<Timer>,
    /// A Dev or action pane's command (its label).
    pub command: Option<String>,
    /// An action's name (the `action` pane).
    pub name: Option<String>,
    /// New in the port: the task pumping `onData`/`onExit` ([`pump_pty`]); aborted when the runtime is forgotten.
    pub pump: Option<Task<()>>,
}

impl RuntimePty {
    /// A fresh runtime for `term`: `new TerminalPreview(cols, rows)`, not exited, bracketed paste off.
    pub fn new(term: Pty, cols: f64, rows: f64, cwd: String, command: Option<String>) -> Self {
        RuntimePty {
            term: Rc::new(term),
            preview: TerminalPreview::new(cols, rows),
            cwd,
            exited: false,
            terminal_modes: TerminalModes { bracketed_paste: false },
            exit_code: None,
            exit_signal: None,
            broadcast_timer: None,
            command,
            name: None,
            pump: None,
        }
    }
}

/// A [`RuntimePty`] shared between its pump task, timers and the command handlers.
pub type SharedRuntime = Rc<RefCell<RuntimePty>>;

/// TS: sessionWorker.ts AgentRuntime previewChangeEvents item `{at, changedChars}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewChangeEvent {
    pub at: Instant,
    pub changed_chars: usize,
}

/// TS: sessionWorker.ts AgentRuntime (RuntimePty plus the activity heuristic's state).
pub struct AgentRuntime {
    pub pty: RuntimePty,
    pub activity_evaluation_timer: Option<Timer>,
    pub activity_idle_timer: Option<Timer>,
    pub suppress_activity_until: Option<Instant>,
    pub last_preview_snapshot: String,
    pub preview_change_events: Vec<PreviewChangeEvent>,
    pub last_data_at: Option<Instant>,
}

/// An [`AgentRuntime`] shared between its pump task, timers and the command handlers.
pub type SharedAgent = Rc<RefCell<AgentRuntime>>;

/// TS: sessionWorker.ts size: `fallback` when not finite, else `max(1, floor(value))` (clamped to `u16`).
pub fn size(value: f64, fallback: u16) -> u16 {
    if !value.is_finite() {
        return fallback;
    }
    // `as` saturates at u16::MAX: a PTY cannot be larger anyway.
    value.floor().max(1.0) as u16
}

/// TS: sessionWorker.ts shellCommand: `$SHELL`, or `/bin/sh` when unset or empty.
pub fn shell_command() -> String {
    match std::env::var_os("SHELL") {
        Some(shell) if !shell.is_empty() => shell.to_string_lossy().into_owned(),
        _ => "/bin/sh".to_owned(),
    }
}

/// The agent's environment: this process's, plus `CLAUDE_CODE_NO_FLICKER=1` for Claude (replacing any value). Every
/// scroll path sends the wheel to the agent's own TUI, and only Claude's fullscreen renderer consumes it; the classic
/// one ignores mouse input, which leaves nothing to scroll in Preview focus or attach.
///
/// With `scroll_override` (`claude_scroll_override`, on unless false) Claude also gets `CLAUDE_CODE_SCROLL_SPEED=1`:
/// with its acceleration off (`integration_args`) each wheel event then scrolls one line, so Deckhand's scroll
/// sensitivity alone sets the speed. Otherwise Claude guesses a speed from the terminal it inherits from the daemon's
/// environment (`TERM_PROGRAM`, `CURSOR_TRACE_ID`), which need not be the one attached. Claude applies its settings
/// files' `env` over this, so `integration_args` repeats it in `--settings`, which wins over them; this copy is for
/// a Claude without `--settings`.
pub fn agent_env(program: ProgramKey, scroll_override: bool) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    if program == ProgramKey::Claude {
        let mut forced = vec![("CLAUDE_CODE_NO_FLICKER", "1")];
        if scroll_override {
            forced.push((
                "CLAUDE_CODE_SCROLL_SPEED",
                deckhand_sys::agent_path::CLAUDE_SCROLL_SPEED,
            ));
        }
        env.retain(|(key, _)| !forced.iter().any(|(name, _)| key == name));
        env.extend(forced.into_iter().map(|(key, value)| (key.into(), value.into())));
    }
    env
}

/// TS: sessionWorker.ts changedCharacterCount: the length difference plus the differing positions of the shared
/// prefix, capped at the longer length. Lengths and positions are UTF-16 code units, as in JS.
pub fn changed_character_count(previous: &str, next: &str) -> usize {
    let previous: Vec<u16> = previous.encode_utf16().collect();
    let next: Vec<u16> = next.encode_utf16().collect();
    let max_length = previous.len().max(next.len());
    let differing = previous.iter().zip(next.iter()).filter(|(a, b)| a != b).count();
    let changed = previous.len().abs_diff(next.len()) + differing;
    changed.min(max_length)
}

/// TS: sessionWorker.ts signalPtyProcess: signals the PTY's process group (`kill(-pid)`), falling back to the child
/// alone when that fails; errors are ignored.
pub fn signal_pty_process(term: &Pty, signal: Signal) {
    if term.kill_group(signal).is_ok() {
        return;
    }
    let _ = term.kill(Some(signal));
}

/// TS: sessionWorker.ts updateTerminalModes: each `ESC[?2004h` / `ESC[?2004l` in `output`, in order, sets
/// `bracketed_paste` (a sequence split across chunks is missed, as in TS).
pub fn update_terminal_modes(modes: &mut TerminalModes, output: &str) {
    const PREFIX: &str = "\x1b[?2004";
    for (index, _) in output.match_indices(PREFIX) {
        match output.as_bytes().get(index + PREFIX.len()) {
            Some(b'h') => modes.bracketed_paste = true,
            Some(b'l') => modes.bracketed_paste = false,
            _ => {}
        }
    }
}

/// TS: node-pty `pty.spawn(command, args, {name: 'xterm-256color', cwd, env, cols: size(cols, 80), rows: size(rows,
/// 24)})`. `env` `None` is this process's environment (`{...process.env}`). A spawn failure carries the error's text.
pub fn spawn_pty(
    command: &str,
    args: &[String],
    cwd: &str,
    env: Option<Vec<(OsString, OsString)>>,
    cols: f64,
    rows: f64,
) -> Result<Pty, DhError> {
    // `{...process.env}` is a copy, so node-pty does not sanitise it (it only does for `env === process.env`).
    let env = env.unwrap_or_else(|| std::env::vars_os().collect());
    let options = PtyOptions {
        name: Some(PTY_TERM_NAME.to_owned()),
        cols: size(cols, DEFAULT_COLS),
        rows: size(rows, DEFAULT_ROWS),
        // node-pty: `opt.cwd || process.cwd()`.
        cwd: (!cwd.is_empty()).then(|| PathBuf::from(cwd)),
        env: Some(env),
    };
    Pty::spawn(command, args, options).map_err(DhError::from)
}

/// TS: node-pty `term.onData` + `term.onExit`: reads `term` until EOF, decoding UTF-8 across chunks exactly as
/// node-pty did ([`deckhand_term::utf8::Utf8Decoder`]) and calling `on_data` per chunk, then resolves with the exit
/// status. EOF is waited for at most [`PTY_EOF_WAIT`] after the child exited (node-pty's order: last output, then
/// exit).
pub async fn pump_pty(term: Rc<Pty>, mut on_data: impl FnMut(&str)) -> PtyExit {
    let mut decoder = Utf8Decoder::new();
    let mut buf = vec![0u8; PTY_READ_CHUNK];
    let mut exit: Option<PtyExit> = None;
    let mut eof_deadline: Option<Instant> = None;
    let mut reached_eof = false;
    loop {
        tokio::select! {
            biased;
            read = term.read(&mut buf) => match read {
                Ok(0) | Err(_) => {
                    reached_eof = true;
                    break;
                }
                Ok(read) => {
                    let text = decoder.write(buf.get(..read).unwrap_or_default());
                    if !text.is_empty() {
                        on_data(&text);
                    }
                }
            },
            status = term.wait(), if exit.is_none() => {
                exit = Some(status);
                eof_deadline = Some(Instant::now() + PTY_EOF_WAIT);
            }
            _ = tokio::time::sleep_until(eof_deadline.unwrap_or_else(Instant::now)), if eof_deadline.is_some() => break,
        }
    }
    if reached_eof {
        // The stream ended: Node's StringDecoder flushes a held incomplete sequence (as U+FFFD).
        let rest = decoder.end();
        if !rest.is_empty() {
            on_data(&rest);
        }
    }
    match exit {
        Some(exit) => exit,
        None => term.wait().await,
    }
}

/// TS: sessionWorker.ts waitForQuietOutput: node-pty can report exit before the final output chunks are delivered;
/// exit-time parsing (resume hints, "No conversation found") needs those last lines, so wait (polling every
/// [`EXIT_OUTPUT_POLL`]) until no output arrived for [`EXIT_OUTPUT_QUIET`], at most [`EXIT_OUTPUT_MAX_WAIT`].
pub async fn wait_for_quiet_output(runtime: &SharedAgent) {
    let deadline = Instant::now() + EXIT_OUTPUT_MAX_WAIT;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        // `Date.now() - (lastDataAt ?? 0)`: without any output the agent has been quiet forever.
        let last_data_at = runtime.borrow().last_data_at;
        match last_data_at {
            Some(at) if now.saturating_duration_since(at) < EXIT_OUTPUT_QUIET => {}
            _ => return,
        }
        tokio::time::sleep(EXIT_OUTPUT_POLL).await;
    }
}

/// The `requestId` of a session worker command, if it has one (TS `'requestId' in command`).
pub fn request_id(command: &WorkerCommand) -> Option<&str> {
    match command {
        WorkerCommand::Snapshot(snapshot) => Some(&snapshot.request_id),
        WorkerCommand::Attach(attach) => Some(&attach.request_id),
        WorkerCommand::Start(start) => Some(&start.request_id),
        WorkerCommand::Kill(kill) => Some(&kill.request_id),
        WorkerCommand::Detach(_) | WorkerCommand::Input(_) | WorkerCommand::Resize(_) => None,
    }
}

/// `new Date().toISOString()`.
fn now_iso() -> String {
    date::to_iso_string(date::now_ms()).unwrap_or_default()
}

/// A usize scroll value as the JS number it is.
fn js_int(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// TS: sessionWorker.ts SessionWorker: owns one session's agent PTY.
///
/// A cheap handle (clones share one worker), so timers and the pump task can call back into it.
#[derive(Clone)]
pub struct SessionWorker {
    inner: Rc<SessionWorkerInner>,
}

struct SessionWorkerInner {
    channel: WorkerChannel,
    state: RefCell<SessionWorkerState>,
    /// Set when the worker should exit (TS `process.exit(0)` from a timer): [`SessionWorker::start`] returns.
    exit: watch::Sender<bool>,
}

#[derive(Default)]
struct SessionWorkerState {
    /// The session as last started (`agentStatus` updated by the heuristic); keeps the daemon's key order.
    session: Option<Ordered<SessionRecord>>,
    agent: Option<SharedAgent>,
    agent_attached: bool,
}

impl SessionWorker {
    /// A worker posting to `channel`, with no agent yet.
    pub fn new(channel: WorkerChannel) -> Self {
        let (exit, _) = watch::channel(false);
        SessionWorker {
            inner: Rc::new(SessionWorkerInner {
                channel,
                state: RefCell::new(SessionWorkerState::default()),
                exit,
            }),
        }
    }

    fn agent(&self) -> Option<SharedAgent> {
        self.inner.state.borrow().agent.clone()
    }

    /// The current agent's PTY, cloned out so no borrow outlives the call.
    fn agent_term(&self) -> Option<Rc<Pty>> {
        self.agent().map(|agent| Rc::clone(&agent.borrow().pty.term))
    }

    /// TS `process.exit(0)`: makes [`SessionWorker::start`] return.
    fn request_exit(&self) {
        self.inner.exit.send_replace(true);
    }

    /// TS: SessionWorker.start: serves commands from `reader` until the daemon disconnects. Each command runs in its
    /// own local task (TS `void this.handle(message)`), so a slow `start` does not hold up `input`. On disconnect it
    /// runs [`SessionWorker::on_disconnect`] and returns.
    pub async fn start(&self, reader: ChannelReader) {
        self.inner.channel.set_role(WireRole::Worker);
        let mut exit = self.inner.exit.subscribe();
        let mut reader = reader;
        let serve = async {
            while let Some(message) = reader.next_value().await {
                match decode_session_command(message) {
                    SessionIncoming::Command(command) => {
                        let worker = self.clone();
                        drop(spawn(async move { worker.handle(command).await }));
                    }
                    // TS's catch: a command it cannot run is answered, never dropped (in turn with the commands
                    // before it, which run as tasks).
                    SessionIncoming::Failure { request_id, error } => {
                        let channel = self.inner.channel.clone();
                        drop(spawn(async move { channel.fail(&request_id, &DhError::msg(error)) }));
                    }
                    SessionIncoming::Ignored => {}
                }
            }
            self.on_disconnect().await;
        };
        tokio::select! {
            _ = serve => {}
            _ = exit.wait_for(|exit| *exit) => {}
        }
    }

    /// TS: the `process.on('disconnect')` handler: SIGTERM the agent's group, then after [`KILL_ESCALATION`] SIGKILL
    /// it and exit (resolves when the worker should exit with status 0).
    pub async fn on_disconnect(&self) {
        if let Some(term) = self.agent_term() {
            signal_pty_process(&term, Signal::SIGTERM);
        }
        tokio::time::sleep(KILL_ESCALATION).await;
        if let Some(term) = self.agent_term() {
            signal_pty_process(&term, Signal::SIGKILL);
        }
    }

    /// TS: SessionWorker.handle: runs one command, answering `fail(requestId, error)` when it has a request ID and
    /// throws.
    pub async fn handle(&self, command: WorkerCommand) {
        let request_id = request_id(&command).map(str::to_owned);
        if let Err(error) = self.dispatch(command).await
            && let Some(request_id) = request_id
        {
            self.inner.channel.fail(&request_id, &error);
        }
    }

    /// The `try` body of TS SessionWorker.handle. Pane commands for any target but `agent` fail with
    /// `<target> pane is hosted by the workspace worker`.
    pub async fn dispatch(&self, command: WorkerCommand) -> Result<(), DhError> {
        let channel = &self.inner.channel;
        let check = |target: AttachTarget| {
            if target == AttachTarget::Agent {
                Ok(())
            } else {
                Err(DhError::msg(format!("{target} pane is hosted by the workspace worker")))
            }
        };
        match command {
            WorkerCommand::Start(start) => {
                let start = *start;
                let scroll_override = start.scroll_override != Some(false);
                let session = self.start_agent(start.session, start.cols, start.rows, scroll_override)?;
                channel.ok(&start.request_id, &session);
            }
            WorkerCommand::Kill(kill) => {
                self.kill(kill.force.unwrap_or(false))?;
                channel.ok(&kill.request_id, &OkResult { ok: true });
            }
            WorkerCommand::Snapshot(snapshot) => {
                check(snapshot.target)?;
                let record = self.snapshot_agent(snapshot.cols, snapshot.rows, snapshot.scroll_offset);
                channel.ok(&snapshot.request_id, &record);
            }
            WorkerCommand::Attach(attach) => {
                check(attach.target)?;
                self.inner.state.borrow_mut().agent_attached = true;
                let record = self.attach_agent(attach.cols, attach.rows);
                channel.ok(&attach.request_id, &record);
            }
            WorkerCommand::Detach(detach) => {
                check(detach.target)?;
                self.inner.state.borrow_mut().agent_attached = false;
            }
            WorkerCommand::Input(input) => {
                check(input.target)?;
                self.input_agent(&input.data).await;
            }
            WorkerCommand::Resize(resize) => {
                check(resize.target)?;
                self.resize_agent(resize.cols, resize.rows);
            }
        }
        Ok(())
    }

    /// TS: SessionWorker.startAgent: spawns `session.command`/`args` in `session.cwd` (with [`agent_env`]),
    /// posts `running {pid}`, arms the initial idle timer ([`IDLE_AFTER`]) and starts the output pump. Answers
    /// `{...session, status: 'running', pid}`.
    pub fn start_agent(
        &self,
        session: Ordered<SessionRecord>,
        cols: i64,
        rows: i64,
        scroll_override: bool,
    ) -> Result<Ordered<SessionRecord>, DhError> {
        self.inner.state.borrow_mut().session = Some(session.clone());
        let args = session.args.clone().unwrap_or_default();
        let env = agent_env(session.program, scroll_override);
        let term = spawn_pty(
            &session.command,
            &args,
            &session.cwd,
            Some(env),
            cols as f64,
            rows as f64,
        )?;
        let pid = term.pid();
        let agent: SharedAgent = Rc::new(RefCell::new(AgentRuntime {
            pty: RuntimePty::new(term, cols as f64, rows as f64, session.cwd.clone(), None),
            activity_evaluation_timer: None,
            activity_idle_timer: None,
            suppress_activity_until: None,
            last_preview_snapshot: String::new(),
            preview_change_events: Vec::new(),
            last_data_at: None,
        }));
        self.inner.state.borrow_mut().agent = Some(Rc::clone(&agent));
        self.inner
            .channel
            .post(&WorkerMessage::Running(Running { pid: i64::from(pid) }));

        let idle_timer = {
            let worker = self.clone();
            let runtime = Rc::clone(&agent);
            Timer::after(IDLE_AFTER, move || {
                if let Some(timer) = runtime.borrow_mut().activity_idle_timer.take() {
                    timer.detach();
                }
                worker.set_agent_status(AgentActivityStatus::Idle);
            })
        };
        let pump = {
            let worker = self.clone();
            let runtime = Rc::clone(&agent);
            let term = Rc::clone(&agent.borrow().pty.term);
            spawn(async move {
                let exit = pump_pty(term, |output| worker.on_agent_data(&runtime, output)).await;
                worker
                    .handle_agent_exit(Some(i64::from(exit.exit_code)), Some(i64::from(exit.signal)))
                    .await;
            })
        };
        {
            let mut runtime = agent.borrow_mut();
            runtime.activity_idle_timer = Some(idle_timer);
            runtime.pty.pump = Some(pump);
        }
        self.schedule_preview_broadcast();

        let mut running = session;
        running.touch("status");
        running.status = SessionStatus::Running;
        running.touch("pid");
        running.pid = Some(i64::from(pid));
        Ok(running)
    }

    /// TS: the agent's `term.onData` handler: stamps `lastDataAt`, updates terminal modes, feeds the preview,
    /// schedules the activity evaluation and the preview broadcast, and posts `output` while attached.
    pub fn on_agent_data(&self, agent: &SharedAgent, output: &str) {
        {
            let mut runtime = agent.borrow_mut();
            runtime.last_data_at = Some(Instant::now());
            update_terminal_modes(&mut runtime.pty.terminal_modes, output);
            runtime.pty.preview.write(output);
        }
        self.schedule_activity_evaluation();
        self.schedule_preview_broadcast();
        let attached = self.inner.state.borrow().agent_attached;
        if attached {
            self.inner.channel.post(&WorkerMessage::Output(WorkerOutput {
                target: AttachTarget::Agent,
                data: output.to_owned(),
            }));
        }
    }

    /// TS: SessionWorker.handleAgentExit: marks the agent exited, clears its timers, waits for quiet output, posts
    /// `exit {exitCode, exitSignal, lastPreview}` and exits the process after [`EXIT_DELAY`].
    pub async fn handle_agent_exit(&self, exit_code: Option<i64>, exit_signal: Option<i64>) {
        let Some(agent) = self.agent() else {
            return;
        };
        {
            let mut runtime = agent.borrow_mut();
            if runtime.pty.exited {
                return;
            }
            runtime.pty.exited = true;
            SessionWorker::clear_activity_timers(&mut runtime);
            // Cleared but kept set, as `clearTimeout` leaves the field truthy: no broadcast is scheduled any more.
            if let Some(timer) = runtime.pty.broadcast_timer.as_mut() {
                timer.clear();
            }
        }
        wait_for_quiet_output(&agent).await;
        let last_preview = agent.borrow_mut().pty.preview.get_snapshot(None);
        self.inner.state.borrow_mut().agent = None;
        self.inner.channel.post(&WorkerMessage::Exit(AgentExit {
            exit_code,
            exit_signal,
            last_preview,
        }));
        let worker = self.clone();
        Timer::after(EXIT_DELAY, move || worker.request_exit()).detach();
    }

    /// TS: SessionWorker.kill: SIGTERM the agent's process group (coding agents often run below a shell, so the
    /// direct child alone is not enough); `force` adds SIGKILL after [`KILL_ESCALATION`]. Fails with
    /// `session is not running`.
    pub fn kill(&self, force: bool) -> Result<(), DhError> {
        let term = match self.agent() {
            Some(agent) => {
                let runtime = agent.borrow();
                (!runtime.pty.exited).then(|| Rc::clone(&runtime.pty.term))
            }
            None => None,
        };
        let Some(term) = term else {
            return Err(DhError::msg("session is not running"));
        };
        // Always signal the PTY process group. Coding agents often run below a shell/bootstrap process, and killing
        // only the direct PTY child can leave the actual agent alive and the session stuck in running state.
        signal_pty_process(&term, Signal::SIGTERM);
        if force {
            let worker = self.clone();
            Timer::after(KILL_ESCALATION, move || {
                let term = worker.agent().and_then(|agent| {
                    let runtime = agent.borrow();
                    (!runtime.pty.exited).then(|| Rc::clone(&runtime.pty.term))
                });
                if let Some(term) = term {
                    signal_pty_process(&term, Signal::SIGKILL);
                }
            })
            .detach();
        }
        Ok(())
    }

    /// TS: SessionWorker.attachAgent: the snapshot plus `{terminalModes, initialFrame}` while the agent exists.
    pub fn attach_agent(&self, cols: i64, rows: i64) -> Ordered<AttachSnapshot> {
        let record = self.snapshot_agent(cols, rows, None);
        let extra = self.agent().map(|agent| {
            let mut runtime = agent.borrow_mut();
            let modes = runtime.pty.terminal_modes.clone();
            (modes, runtime.pty.preview.get_ansi_frame())
        });
        let (terminal_modes, initial_frame) = match extra {
            Some((modes, frame)) => (Some(modes), Some(frame)),
            None => (None, None),
        };
        Ordered::new(AttachSnapshot {
            session_id: record.session_id,
            content: record.content,
            live: record.live,
            cwd: None,
            command: None,
            name: None,
            exit_code: None,
            exit_signal: None,
            status: record.status,
            agent_status: record.agent_status,
            scroll_offset: record.scroll_offset,
            max_scroll_offset: record.max_scroll_offset,
            terminal_modes,
            initial_frame,
        })
    }

    /// TS: SessionWorker.snapshotAgent: resizes PTY and preview (a real size change suppresses activity), then
    /// `{sessionId, content, live: true, status: 'running', agentStatus, scrollOffset, maxScrollOffset}`;
    /// `{content: '', live: false}` without an agent.
    pub fn snapshot_agent(&self, cols: i64, rows: i64, scroll_offset: Option<i64>) -> PreviewRecord {
        let (agent, session_id) = {
            let state = self.inner.state.borrow();
            match (&state.agent, &state.session) {
                (Some(agent), Some(session)) => (Rc::clone(agent), session.id.clone()),
                _ => {
                    return PreviewRecord {
                        session_id: None,
                        content: String::new(),
                        live: false,
                        status: None,
                        agent_status: None,
                        scroll_offset: None,
                        max_scroll_offset: None,
                    };
                }
            }
        };
        let next_cols = size(cols as f64, DEFAULT_COLS);
        let next_rows = size(rows as f64, DEFAULT_ROWS);
        let resized = {
            let mut runtime = agent.borrow_mut();
            let term = Rc::clone(&runtime.pty.term);
            let resized = term.cols() != next_cols || term.rows() != next_rows;
            let _ = term.resize(next_cols, next_rows);
            runtime.pty.preview.resize(cols as f64, rows as f64);
            resized
        };
        if resized {
            self.suppress_resize_activity();
        }
        // TS `scrollOffset = 0` default.
        let offset = Some(scroll_offset.unwrap_or(0) as f64);
        let (content, scroll) = {
            let mut runtime = agent.borrow_mut();
            let content = runtime.pty.preview.get_snapshot(offset);
            (content, runtime.pty.preview.get_scroll_info(offset))
        };
        // Read after the awaits in TS: the heuristic may have changed it meanwhile.
        let agent_status = self
            .inner
            .state
            .borrow()
            .session
            .as_ref()
            .and_then(|session| session.agent_status);
        preview_record(session_id, content, agent_status, scroll)
    }

    /// TS: SessionWorker.resizeAgent (a no-op once exited).
    pub fn resize_agent(&self, cols: i64, rows: i64) {
        let Some(agent) = self.agent() else {
            return;
        };
        {
            let mut runtime = agent.borrow_mut();
            if runtime.pty.exited {
                return;
            }
            let term = Rc::clone(&runtime.pty.term);
            let _ = term.resize(size(cols as f64, DEFAULT_COLS), size(rows as f64, DEFAULT_ROWS));
            runtime.pty.preview.resize(cols as f64, rows as f64);
            runtime.pty.preview.get_snapshot(None);
        }
        self.suppress_resize_activity();
        self.schedule_preview_broadcast();
    }

    /// TS: `this.agent?.term.write(data)` (`input` for the agent).
    pub async fn input_agent(&self, data: &str) {
        let Some(term) = self.agent_term() else {
            return;
        };
        // node-pty reports write failures on the socket, never to the caller.
        let _ = term.write(data.as_bytes()).await;
    }

    /// TS: SessionWorker.schedulePreviewBroadcast: at most one pending broadcast; after [`PREVIEW_BROADCAST_DELAY`]
    /// posts `preview-updated` with the current snapshot and scroll info.
    pub fn schedule_preview_broadcast(&self) {
        let Some(runtime) = self.agent() else {
            return;
        };
        if runtime.borrow().pty.broadcast_timer.is_some() {
            return;
        }
        let worker = self.clone();
        let captured = Rc::clone(&runtime);
        let timer = Timer::after(PREVIEW_BROADCAST_DELAY, move || {
            let runtime = captured;
            if let Some(timer) = runtime.borrow_mut().pty.broadcast_timer.take() {
                timer.detach();
            }
            let session_id = {
                let state = worker.inner.state.borrow();
                match (&state.session, &state.agent) {
                    (Some(session), Some(_)) => session.id.clone(),
                    _ => return,
                }
            };
            let (content, scroll) = {
                let mut runtime = runtime.borrow_mut();
                let content = runtime.pty.preview.get_snapshot(None);
                (content, runtime.pty.preview.get_scroll_info(None))
            };
            let agent_status = worker
                .inner
                .state
                .borrow()
                .session
                .as_ref()
                .and_then(|session| session.agent_status);
            worker
                .inner
                .channel
                .post(&WorkerMessage::PreviewUpdated(WorkerPreviewUpdated {
                    preview: preview_record(session_id, content, agent_status, scroll),
                }));
        });
        runtime.borrow_mut().pty.broadcast_timer = Some(timer);
    }

    /// TS: SessionWorker.scheduleActivityEvaluation: at most one pending evaluation, none while resize-suppressed.
    pub fn schedule_activity_evaluation(&self) {
        let Some(runtime) = self.agent() else {
            return;
        };
        {
            let current = runtime.borrow();
            if current.activity_evaluation_timer.is_some() || suppressed(&current, Instant::now()) {
                return;
            }
        }
        let worker = self.clone();
        let captured = Rc::clone(&runtime);
        let timer = Timer::after(ACTIVITY_EVALUATION_DELAY, move || {
            if let Some(timer) = captured.borrow_mut().activity_evaluation_timer.take() {
                timer.detach();
            }
            worker.evaluate_preview_activity();
        });
        runtime.borrow_mut().activity_evaluation_timer = Some(timer);
    }

    /// TS: SessionWorker.evaluatePreviewActivity: counts changed characters since the last snapshot, keeps the
    /// changes of the last [`ACTIVITY_WINDOW`], goes `active` when they reach [`ACTIVE_MIN_CHANGED_CHARS`] and re-arms
    /// the idle timer ([`IDLE_AFTER`], which also clears the window).
    pub fn evaluate_preview_activity(&self) {
        let Some(runtime) = self.agent() else {
            return;
        };
        let total = {
            let mut current = runtime.borrow_mut();
            if suppressed(&current, Instant::now()) {
                return;
            }
            let snapshot = current.pty.preview.get_snapshot(None);
            let changed_chars = changed_character_count(&current.last_preview_snapshot, &snapshot);
            current.last_preview_snapshot = snapshot;
            if changed_chars < ACTIVE_MIN_CHANGED_CHARS {
                return;
            }
            let now = Instant::now();
            current
                .preview_change_events
                .retain(|event| now.saturating_duration_since(event.at) <= ACTIVITY_WINDOW);
            current
                .preview_change_events
                .push(PreviewChangeEvent { at: now, changed_chars });
            current
                .preview_change_events
                .iter()
                .map(|event| event.changed_chars)
                .sum::<usize>()
        };
        if total >= ACTIVE_MIN_CHANGED_CHARS {
            self.set_agent_status(AgentActivityStatus::Active);
        }
        let worker = self.clone();
        let captured = Rc::clone(&runtime);
        let timer = Timer::after(IDLE_AFTER, move || {
            {
                let mut current = captured.borrow_mut();
                if let Some(timer) = current.activity_idle_timer.take() {
                    timer.detach();
                }
                current.preview_change_events.clear();
            }
            worker.set_agent_status(AgentActivityStatus::Idle);
        });
        // Replacing the previous idle timer cancels it (`clearTimeout`).
        runtime.borrow_mut().activity_idle_timer = Some(timer);
    }

    /// TS: SessionWorker.suppressResizeActivity: ignore preview changes for [`RESIZE_ACTIVITY_SUPPRESSION`] and take
    /// the resized snapshot as the new baseline.
    pub fn suppress_resize_activity(&self) {
        let Some(agent) = self.agent() else {
            return;
        };
        let mut runtime = agent.borrow_mut();
        runtime.suppress_activity_until = Some(Instant::now() + RESIZE_ACTIVITY_SUPPRESSION);
        runtime.last_preview_snapshot = runtime.pty.preview.get_snapshot(None);
    }

    /// TS: SessionWorker.setAgentStatus: on a change, stamps `agentStatus` and `agentStatusUpdatedAt` (ISO time) on
    /// the session, posts `agent-status` and schedules a preview broadcast.
    pub fn set_agent_status(&self, agent_status: AgentActivityStatus) {
        {
            let mut state = self.inner.state.borrow_mut();
            let Some(session) = state.session.as_mut() else {
                return;
            };
            if session.agent_status == Some(agent_status) {
                return;
            }
            // `{...this.session, agentStatus, agentStatusUpdatedAt}`.
            session.touch("agentStatus");
            session.agent_status = Some(agent_status);
            session.touch("agentStatusUpdatedAt");
            session.agent_status_updated_at = Some(now_iso());
        }
        self.inner
            .channel
            .post(&WorkerMessage::AgentStatus(AgentStatusChanged { agent_status }));
        self.schedule_preview_broadcast();
    }

    /// TS: SessionWorker.clearActivityTimers
    pub fn clear_activity_timers(runtime: &mut AgentRuntime) {
        // Cleared in place: like `clearTimeout`, the fields stay set, so nothing new is scheduled afterwards.
        if let Some(timer) = runtime.activity_evaluation_timer.as_mut() {
            timer.clear();
        }
        if let Some(timer) = runtime.activity_idle_timer.as_mut() {
            timer.clear();
        }
    }
}

/// `Date.now() < (runtime.suppressActivityUntil ?? 0)`.
fn suppressed(runtime: &AgentRuntime, now: Instant) -> bool {
    runtime.suppress_activity_until.is_some_and(|until| now < until)
}

/// The live agent's preview record (`{sessionId, content, live: true, status: 'running', agentStatus,
/// ...scrollInfo}`).
fn preview_record(
    session_id: String,
    content: String,
    agent_status: Option<AgentActivityStatus>,
    scroll: ScrollInfo,
) -> PreviewRecord {
    PreviewRecord {
        session_id: Some(session_id),
        content,
        live: true,
        status: Some(SessionStatus::Running),
        agent_status,
        scroll_offset: Some(js_int(scroll.scroll_offset)),
        max_scroll_offset: Some(js_int(scroll.max_scroll_offset)),
    }
}

/// TS: sessionWorker.ts runSessionWorker (`deckhand --session-worker`): opens the channel on fd 3 (posting `hello`)
/// and serves the daemon until it disconnects or the agent exits. Must run inside the process's `LocalSet`. Returns
/// when the worker should exit with status 0; fails only when the channel cannot be opened. The TS
/// `process.title = 'deckhand-session-worker'` has no portable equivalent and is not ported.
pub async fn run_session_worker() -> Result<(), DhError> {
    let (channel, reader) = WorkerChannel::open(WireRole::Worker)?;
    let worker = SessionWorker::new(channel.clone());
    worker.start(reader).await;
    channel.flush().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
    use tokio::net::UnixStream;
    use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

    fn run_local<F: std::future::Future>(future: F) -> F::Output {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        tokio::task::LocalSet::new().block_on(&runtime, future)
    }

    /// The daemon's end of a session worker under test.
    struct Daemon {
        lines: Lines<BufReader<OwnedReadHalf>>,
        writer: Option<OwnedWriteHalf>,
        worker: SessionWorker,
        served: Task<()>,
    }

    impl Daemon {
        fn start() -> Daemon {
            let (worker_end, daemon_end) = UnixStream::pair().expect("pair");
            let (channel, reader) = WorkerChannel::from_stream(worker_end, WireRole::Worker);
            let worker = SessionWorker::new(channel);
            let serving = worker.clone();
            let served = spawn(async move { serving.start(reader).await });
            let (read_half, write_half) = daemon_end.into_split();
            Daemon {
                lines: BufReader::new(read_half).lines(),
                writer: Some(write_half),
                worker,
                served,
            }
        }

        async fn send(&mut self, command: Value) {
            let mut line = serde_json::to_string(&command).expect("json");
            line.push('\n');
            let writer = self.writer.as_mut().expect("connected");
            writer.write_all(line.as_bytes()).await.expect("send");
        }

        /// The next message as its JSON text.
        async fn next_text(&mut self) -> String {
            tokio::time::timeout(Duration::from_secs(10), self.lines.next_line())
                .await
                .expect("message in time")
                .expect("read")
                .expect("worker still connected")
        }

        async fn next(&mut self) -> Value {
            let text = self.next_text().await;
            serde_json::from_str(&text).expect("JSON message")
        }

        /// Messages until one matches `pred` (returned), collecting the skipped ones.
        async fn until(&mut self, pred: impl Fn(&Value) -> bool, skipped: &mut Vec<Value>) -> Value {
            loop {
                let message = self.next().await;
                if pred(&message) {
                    return message;
                }
                skipped.push(message);
            }
        }

        async fn response(&mut self, request_id: &str) -> Value {
            let mut skipped = Vec::new();
            self.until(
                |message| message["type"] == "response" && message["requestId"] == request_id,
                &mut skipped,
            )
            .await
        }
    }

    fn session(command: &str, args: &[&str]) -> Value {
        json!({
            "id": "s-1",
            "title": "Test",
            "program": "claude",
            "command": command,
            "args": args,
            "cwd": std::env::temp_dir().to_string_lossy(),
            "repoRoot": "/repo",
            "status": "starting",
            "createdAt": "2026-01-01T00:00:00.000Z",
            "updatedAt": "2026-01-01T00:00:00.000Z",
        })
    }

    fn start_command(command: &str, args: &[&str]) -> Value {
        json!({"type": "start", "session": session(command, args), "cols": 80, "rows": 24, "requestId": "start-1"})
    }

    fn pid_alive(pid: i64) -> bool {
        let Ok(pid) = i32::try_from(pid) else {
            return false;
        };
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
    }

    #[test]
    fn size_floors_finite_values_with_a_minimum_of_one() {
        assert_eq!(size(f64::NAN, 80), 80);
        assert_eq!(size(f64::INFINITY, 24), 24);
        assert_eq!(size(f64::NEG_INFINITY, 24), 24);
        assert_eq!(size(2.9, 80), 2);
        assert_eq!(size(0.0, 80), 1);
        assert_eq!(size(-3.0, 80), 1);
        assert_eq!(size(1e9, 80), u16::MAX);
    }

    #[test]
    fn changed_character_count_counts_utf16_units() {
        assert_eq!(changed_character_count("", ""), 0);
        assert_eq!(changed_character_count("", "abc"), 3);
        assert_eq!(changed_character_count("abc", "abd"), 1);
        assert_eq!(changed_character_count("abc", "xbcde"), 3);
        assert_eq!(changed_character_count("abcd", "b"), 4);
        // Surrogate pairs share their high unit: one changed unit.
        assert_eq!(changed_character_count("a\u{1F600}", "a\u{1F601}"), 1);
        assert_eq!(changed_character_count("", "\u{1F600}"), 2);
    }

    #[test]
    fn update_terminal_modes_follows_the_last_bracketed_paste_sequence() {
        let mut modes = TerminalModes::default();
        update_terminal_modes(&mut modes, "x\x1b[?2004hy");
        assert!(modes.bracketed_paste);
        update_terminal_modes(&mut modes, "plain");
        assert!(modes.bracketed_paste);
        update_terminal_modes(&mut modes, "\x1b[?2004h\x1b[?2004l");
        assert!(!modes.bracketed_paste);
        update_terminal_modes(&mut modes, "\x1b[?2004l\x1b[?2004h\x1b[?2004");
        assert!(modes.bracketed_paste);
        update_terminal_modes(&mut modes, "\x1b[?2004x\x1b[?1049l");
        assert!(modes.bracketed_paste);
    }

    #[test]
    fn request_id_only_for_requests() {
        let command = |value: Value| serde_json::from_value::<WorkerCommand>(value).expect("command");
        assert_eq!(
            request_id(&command(json!({"type": "kill", "requestId": "k"}))),
            Some("k")
        );
        assert_eq!(
            request_id(&command(
                json!({"type": "snapshot", "target": "agent", "cols": 1, "rows": 1, "requestId": "s"})
            )),
            Some("s")
        );
        assert_eq!(
            request_id(&command(
                json!({"type": "attach", "target": "agent", "cols": 1, "rows": 1, "requestId": "a"})
            )),
            Some("a")
        );
        assert_eq!(request_id(&command(start_command("true", &[]))), Some("start-1"));
        assert_eq!(request_id(&command(json!({"type": "detach", "target": "agent"}))), None);
        assert_eq!(
            request_id(&command(json!({"type": "input", "target": "agent", "data": "x"}))),
            None
        );
        assert_eq!(
            request_id(&command(
                json!({"type": "resize", "target": "agent", "cols": 1, "rows": 1})
            )),
            None
        );
    }

    #[test]
    fn agent_env_forces_claudes_fullscreen_renderer() {
        let flag = |env: &[(OsString, OsString)]| -> Vec<OsString> {
            env.iter()
                .filter(|(key, _)| key == "CLAUDE_CODE_NO_FLICKER")
                .map(|(_, value)| value.clone())
                .collect()
        };
        assert_eq!(flag(&agent_env(ProgramKey::Claude, false)), [OsString::from("1")]);
        // Pi and Codex inherit the environment unchanged.
        let inherited: Vec<_> = std::env::vars_os().collect();
        assert_eq!(agent_env(ProgramKey::Pi, true), inherited);
        assert_eq!(agent_env(ProgramKey::Codex, true), inherited);
    }

    #[test]
    fn agent_env_fixes_claudes_scroll_speed_unless_opted_out() {
        let speed = |env: &[(OsString, OsString)]| -> Vec<OsString> {
            env.iter()
                .filter(|(key, _)| key == "CLAUDE_CODE_SCROLL_SPEED")
                .map(|(_, value)| value.clone())
                .collect()
        };
        assert_eq!(speed(&agent_env(ProgramKey::Claude, true)), [OsString::from("1")]);
        let inherited: Vec<_> = std::env::vars_os()
            .filter(|(key, _)| key == "CLAUDE_CODE_SCROLL_SPEED")
            .collect();
        assert_eq!(speed(&agent_env(ProgramKey::Claude, false)), speed(&inherited));
    }

    #[test]
    fn shell_command_is_never_empty() {
        assert!(!shell_command().is_empty());
    }

    #[test]
    fn pump_reads_output_to_eof_before_reporting_the_exit() {
        run_local(async {
            let args = ["-c".to_owned(), "printf 'abc\\303\\251'; exit 3".to_owned()];
            let term = spawn_pty("/bin/sh", &args, "", None, 80.0, 24.0).expect("spawn");
            let mut output = String::new();
            let exit = pump_pty(Rc::new(term), |chunk| output.push_str(chunk)).await;
            assert_eq!(
                exit,
                PtyExit {
                    exit_code: 3,
                    signal: 0
                }
            );
            assert_eq!(output, "abc\u{e9}");
        });
    }

    #[test]
    fn spawn_pty_sets_term_size_and_cwd() {
        run_local(async {
            let dir = tempfile::tempdir().expect("tempdir");
            let cwd = dir.path().canonicalize().expect("canonical");
            let args = [
                "-c".to_owned(),
                "printf '%s|%s|' \"$TERM\" \"$(pwd -P)\"; stty size".to_owned(),
            ];
            let env = vec![(OsString::from("PATH"), OsString::from("/usr/bin:/bin"))];
            let term = spawn_pty("/bin/sh", &args, &cwd.to_string_lossy(), Some(env), 100.7, f64::NAN).expect("spawn");
            assert_eq!((term.cols(), term.rows()), (100, 24));
            let mut output = String::new();
            pump_pty(Rc::new(term), |chunk| output.push_str(chunk)).await;
            assert_eq!(output.trim_end(), format!("xterm-256color|{}|24 100", cwd.display()));
        });
    }

    #[test]
    fn start_streams_preview_activity_and_exit() {
        run_local(async {
            let mut daemon = Daemon::start();
            assert_eq!(daemon.next_text().await, r#"{"type":"hello","version":48}"#);
            daemon
                .send(start_command("/bin/sh", &["-c", "printf hello; sleep 0.6"]))
                .await;
            let running = daemon.next().await;
            assert_eq!(running["type"], "running");
            let pid = running["pid"].as_i64().expect("pid");
            assert!(pid > 0);
            // `{...session, status: 'running', pid}`: status keeps its place, pid is appended.
            let response = daemon.next_text().await;
            let mut expected = session("/bin/sh", &["-c", "printf hello; sleep 0.6"]);
            expected["status"] = json!("running");
            expected["pid"] = json!(pid);
            let expected = json!({"type": "response", "requestId": "start-1", "ok": true, "data": expected});
            assert_eq!(response, serde_json::to_string(&expected).expect("json"));
            let mut seen = Vec::new();
            let status = daemon.until(|m| m["type"] == "agent-status", &mut seen).await;
            assert_eq!(status, json!({"type": "agent-status", "agentStatus": "active"}));
            let exit = daemon.until(|m| m["type"] == "exit", &mut seen).await;
            assert_eq!(exit["exitCode"], 0);
            assert_eq!(exit["exitSignal"], 0);
            assert_eq!(exit["lastPreview"], "hello");
            let preview = seen
                .iter()
                .rev()
                .find(|m| m["type"] == "preview-updated")
                .expect("a preview update");
            assert_eq!(
                serde_json::to_string(&preview["preview"]).expect("json"),
                r#"{"sessionId":"s-1","content":"hello","live":true,"status":"running","agentStatus":"active","scrollOffset":0,"maxScrollOffset":0}"#
            );
            assert!(seen.iter().all(|m| m["type"] != "output"), "not attached: {seen:?}");
            // The worker stops serving shortly after the exit.
            tokio::time::timeout(Duration::from_secs(2), daemon.served)
                .await
                .expect("worker returns after the agent exit");
            assert!(daemon.worker.agent().is_none());
        });
    }

    #[test]
    fn panes_and_missing_agent_fail_like_ts() {
        run_local(async {
            let mut daemon = Daemon::start();
            daemon.next().await;
            daemon
                .send(json!({"type": "snapshot", "target": "terminal", "cols": 80, "rows": 24, "requestId": "a"}))
                .await;
            daemon.send(json!({"type": "detach", "target": "git"})).await;
            daemon
                .send(json!({"type": "input", "target": "dev", "data": "x"}))
                .await;
            daemon.send(json!({"type": "nonsense", "requestId": "n"})).await;
            daemon.send(json!({"type": "kill", "requestId": "b"})).await;
            daemon
                .send(json!({"type": "attach", "target": "action", "cols": 80, "rows": 24, "requestId": "c"}))
                .await;
            daemon
                .send(json!({"type": "snapshot", "target": "agent", "cols": 80, "rows": 24, "requestId": "d"}))
                .await;
            daemon
                .send(json!({"type": "attach", "target": "agent", "cols": 80, "rows": 24, "requestId": "e"}))
                .await;
            assert_eq!(
                daemon.next_text().await,
                r#"{"type":"response","requestId":"a","ok":false,"error":"terminal pane is hosted by the workspace worker"}"#
            );
            // An unknown command fails TS's target check (it has no target), in turn.
            assert_eq!(
                daemon.next_text().await,
                r#"{"type":"response","requestId":"n","ok":false,"error":"undefined pane is hosted by the workspace worker"}"#
            );
            assert_eq!(
                daemon.next_text().await,
                r#"{"type":"response","requestId":"b","ok":false,"error":"session is not running"}"#
            );
            assert_eq!(
                daemon.next_text().await,
                r#"{"type":"response","requestId":"c","ok":false,"error":"action pane is hosted by the workspace worker"}"#
            );
            assert_eq!(
                daemon.next_text().await,
                r#"{"type":"response","requestId":"d","ok":true,"data":{"content":"","live":false}}"#
            );
            assert_eq!(
                daemon.next_text().await,
                r#"{"type":"response","requestId":"e","ok":true,"data":{"content":"","live":false}}"#
            );
        });
    }

    #[test]
    fn spawn_failure_answers_the_start_request() {
        run_local(async {
            let mut daemon = Daemon::start();
            daemon.next().await;
            daemon.send(start_command("/bin/sh", &["a\u{0}b"])).await;
            let response = daemon.next().await;
            assert_eq!(response["ok"], false);
            assert_eq!(response["requestId"], "start-1");
            assert_eq!(response["error"], "argument contains a NUL byte");
            // The session was taken, but nothing runs.
            daemon.send(json!({"type": "kill", "requestId": "k"})).await;
            assert_eq!(daemon.response("k").await["error"], "session is not running");
        });
    }

    #[test]
    fn attach_forwards_output_and_kill_reports_the_signal() {
        run_local(async {
            let mut daemon = Daemon::start();
            daemon.next().await;
            let script = "printf '\\033[?2004hready'; while read line; do printf 'got:%s;' \"$line\"; done";
            daemon.send(start_command("/bin/sh", &["-c", script])).await;
            daemon.response("start-1").await;
            // Going active means `ready` was read, so the bracketed paste switch before it was too.
            let mut seen = Vec::new();
            daemon.until(|m| m["type"] == "agent-status", &mut seen).await;
            daemon
                .send(json!({"type": "attach", "target": "agent", "cols": 90, "rows": 20, "requestId": "att"}))
                .await;
            let attach = daemon.response("att").await;
            let data = &attach["data"];
            assert_eq!(data["sessionId"], "s-1");
            assert_eq!(data["live"], true);
            assert_eq!(data["status"], "running");
            assert_eq!(data["terminalModes"], json!({"bracketedPaste": true}));
            assert!(
                data["initialFrame"]
                    .as_str()
                    .expect("frame")
                    .starts_with("\x1b[2J\x1b[H")
            );
            let keys: Vec<&str> = data.as_object().expect("object").keys().map(String::as_str).collect();
            assert_eq!(keys.first(), Some(&"sessionId"));
            assert_eq!(keys.get(keys.len() - 2..), Some(&["terminalModes", "initialFrame"][..]));

            daemon
                .send(json!({"type": "input", "target": "agent", "data": "ping\r"}))
                .await;
            let mut output = String::new();
            while !output.contains("got:ping;") {
                let message = daemon.until(|m| m["type"] == "output", &mut seen).await;
                assert_eq!(message["target"], "agent");
                output.push_str(message["data"].as_str().expect("data"));
            }

            daemon.send(json!({"type": "detach", "target": "agent"})).await;
            daemon
                .send(json!({"type": "resize", "target": "agent", "cols": 70, "rows": 10}))
                .await;
            daemon
                .send(json!({"type": "snapshot", "target": "agent", "cols": 70, "rows": 10, "scrollOffset": 0, "requestId": "snap"}))
                .await;
            let snapshot = daemon.response("snap").await;
            assert!(
                snapshot["data"]["content"]
                    .as_str()
                    .expect("content")
                    .contains("got:ping;")
            );
            assert_eq!(snapshot["data"]["scrollOffset"], 0);

            daemon
                .send(json!({"type": "kill", "requestId": "kill", "force": true}))
                .await;
            assert_eq!(
                daemon.response("kill").await,
                json!({"type": "response", "requestId": "kill", "ok": true, "data": {"ok": true}})
            );
            let mut after = Vec::new();
            let exit = daemon.until(|m| m["type"] == "exit", &mut after).await;
            assert_eq!(exit["exitCode"], 0);
            assert_eq!(exit["exitSignal"], 15);
            assert!(after.iter().all(|m| m["type"] != "output"), "detached: {after:?}");
        });
    }

    #[test]
    fn disconnect_terminates_the_agent_and_returns() {
        run_local(async {
            let mut daemon = Daemon::start();
            daemon.next().await;
            daemon.send(start_command("/bin/sh", &["-c", "sleep 30"])).await;
            let pid = daemon.next().await["pid"].as_i64().expect("pid");
            daemon.response("start-1").await;
            let worker = daemon.worker.clone();
            let Daemon {
                lines, writer, served, ..
            } = daemon;
            drop(writer);
            drop(lines);
            tokio::time::timeout(Duration::from_secs(3), served)
                .await
                .expect("worker returns after the disconnect");
            assert!(!worker.inner.channel.is_connected());
            // SIGTERM reached the group: the agent is gone (allowing for the reaper).
            for _ in 0..50 {
                if !pid_alive(pid) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(!pid_alive(pid));
        });
    }

    #[test]
    fn agent_goes_idle_after_quiet_period() {
        run_local(async {
            let mut daemon = Daemon::start();
            daemon.next().await;
            daemon
                .send(start_command("/bin/sh", &["-c", "printf busy; sleep 30"]))
                .await;
            daemon.response("start-1").await;
            let mut seen = Vec::new();
            let active = daemon.until(|m| m["type"] == "agent-status", &mut seen).await;
            assert_eq!(active["agentStatus"], "active");
            let started = Instant::now();
            let idle = daemon.until(|m| m["type"] == "agent-status", &mut seen).await;
            assert_eq!(idle["agentStatus"], "idle");
            let waited = started.elapsed();
            assert!(waited >= Duration::from_millis(4500), "idle after {waited:?}");
            let text = {
                let state = daemon.worker.inner.state.borrow();
                let session = state.session.as_ref().expect("session");
                assert_eq!(session.agent_status, Some(AgentActivityStatus::Idle));
                serde_json::to_string(session).expect("json")
            };
            // `{...this.session, agentStatus, agentStatusUpdatedAt}` appends both keys.
            assert!(
                text.contains(
                    r#""updatedAt":"2026-01-01T00:00:00.000Z","agentStatus":"idle","agentStatusUpdatedAt":""#
                ),
                "{text}"
            );
            daemon.worker.kill(false).expect("kill");
        });
    }

    #[test]
    fn evaluate_counts_changes_and_resize_suppresses_them() {
        run_local(async {
            let mut daemon = Daemon::start();
            daemon.next().await;
            daemon.send(start_command("/bin/sh", &["-c", "sleep 30"])).await;
            daemon.response("start-1").await;
            let worker = daemon.worker.clone();
            let agent = worker.agent().expect("agent");
            worker.on_agent_data(&agent, "first");
            assert!(agent.borrow().last_data_at.is_some());
            worker.evaluate_preview_activity();
            {
                let runtime = agent.borrow();
                assert_eq!(runtime.last_preview_snapshot, "first");
                let changes: Vec<usize> = runtime.preview_change_events.iter().map(|e| e.changed_chars).collect();
                assert_eq!(changes, [5]);
            }
            // An unchanged preview adds no event.
            worker.evaluate_preview_activity();
            assert_eq!(agent.borrow().preview_change_events.len(), 1);
            // A resize takes the resized snapshot as the baseline and suppresses evaluation for a while.
            worker.resize_agent(40, 10);
            assert!(agent.borrow().suppress_activity_until.is_some());
            worker.on_agent_data(&agent, " second");
            worker.evaluate_preview_activity();
            assert_eq!(agent.borrow().preview_change_events.len(), 1);
            assert_eq!(agent.borrow().last_preview_snapshot, "first");
            worker.kill(true).expect("kill");
        });
    }
}
