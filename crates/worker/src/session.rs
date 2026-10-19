//! Port of sessionWorker.ts (session worker: agent PTY, preview debounce, activity heuristic).
//!
//! A session worker owns one session's agent PTY; its companion panes belong to the session's workspace worker
//! ([`crate::workspace`]). It mirrors the agent's output into a headless [`TerminalPreview`], posts debounced
//! `preview-updated` snapshots, derives `agent-status` (active/idle) from how much the preview changes, and streams
//! raw output while the daemon is attached. The helpers shared with the workspace worker (PTY spawn and pump,
//! [`RuntimePty`], sizes, signals, terminal modes) live here too, because W2 builds on W1.
//!
//! Time: `Date.now()` comparisons use `tokio::time::Instant`, so tests can pause the clock.

use std::cell::RefCell;
use std::ffi::OsString;
use std::rc::Rc;
use std::time::Duration;

use nix::sys::signal::Signal;
use tokio::time::Instant;

use deckhand_core::error::DhError;
use deckhand_protocol::Ordered;
use deckhand_protocol::WorkerCommand;
use deckhand_protocol::data::{AgentActivityStatus, AttachSnapshot, PreviewRecord, SessionRecord, TerminalModes};
use deckhand_sys::async_util::{Task, Timer};
use deckhand_term::preview::TerminalPreview;
use deckhand_term::pty::{Pty, PtyExit};

use crate::channel::{ChannelReader, WorkerChannel};

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
        let _ = (term, cols, rows, cwd, command);
        todo!("W1")
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
    let _ = (value, fallback);
    todo!("W1")
}

/// TS: sessionWorker.ts shellCommand: `$SHELL`, or `/bin/sh` when unset or empty.
pub fn shell_command() -> String {
    todo!("W1")
}

/// TS: sessionWorker.ts changedCharacterCount: the length difference plus the differing positions of the shared
/// prefix, capped at the longer length. Lengths and positions are UTF-16 code units, as in JS.
pub fn changed_character_count(previous: &str, next: &str) -> usize {
    let _ = (previous, next);
    todo!("W1")
}

/// TS: sessionWorker.ts signalPtyProcess: signals the PTY's process group (`kill(-pid)`), falling back to the child
/// alone when that fails; errors are ignored.
pub fn signal_pty_process(term: &Pty, signal: Signal) {
    let _ = (term, signal);
    todo!("W1")
}

/// TS: sessionWorker.ts updateTerminalModes: each `ESC[?2004h` / `ESC[?2004l` in `output`, in order, sets
/// `bracketed_paste` (a sequence split across chunks is missed, as in TS).
pub fn update_terminal_modes(modes: &mut TerminalModes, output: &str) {
    let _ = (modes, output);
    todo!("W1")
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
    let _ = (command, args, cwd, env, cols, rows);
    todo!("W1")
}

/// TS: node-pty `term.onData` + `term.onExit`: reads `term` until EOF, decoding UTF-8 across chunks exactly as
/// node-pty did ([`deckhand_term::utf8::Utf8Decoder`]) and calling `on_data` per chunk, then resolves with the exit
/// status. EOF is waited for at most [`PTY_EOF_WAIT`] after the child exited (node-pty's order: last output, then
/// exit).
pub async fn pump_pty(term: Rc<Pty>, on_data: impl FnMut(&str)) -> PtyExit {
    let _ = (term, on_data);
    todo!("W1")
}

/// TS: sessionWorker.ts waitForQuietOutput: node-pty can report exit before the final output chunks are delivered;
/// exit-time parsing (resume hints, "No conversation found") needs those last lines, so wait (polling every
/// [`EXIT_OUTPUT_POLL`]) until no output arrived for [`EXIT_OUTPUT_QUIET`], at most [`EXIT_OUTPUT_MAX_WAIT`].
pub async fn wait_for_quiet_output(runtime: &SharedAgent) {
    let _ = runtime;
    todo!("W1")
}

/// The `requestId` of a session worker command, if it has one (TS `'requestId' in command`).
pub fn request_id(command: &WorkerCommand) -> Option<&str> {
    let _ = command;
    todo!("W1")
}

/// TS: sessionWorker.ts SessionWorker: owns one session's agent PTY.
///
/// A cheap handle (clones share one worker), so timers and the pump task can call back into it.
#[derive(Clone)]
pub struct SessionWorker {
    inner: Rc<SessionWorkerInner>,
}

#[allow(dead_code)] // W1: read once the methods below are written.
struct SessionWorkerInner {
    channel: WorkerChannel,
    state: RefCell<SessionWorkerState>,
}

#[allow(dead_code)] // W1: read once the methods below are written.
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
        let _ = channel;
        todo!("W1")
    }

    /// TS: SessionWorker.start: serves commands from `reader` until the daemon disconnects. Each command runs in its
    /// own local task (TS `void this.handle(message)`), so a slow `start` does not hold up `input`. On disconnect it
    /// runs [`SessionWorker::on_disconnect`] and returns.
    pub async fn start(&self, reader: ChannelReader) {
        let _ = (&self.inner, reader);
        todo!("W1")
    }

    /// TS: the `process.on('disconnect')` handler: SIGTERM the agent's group, then after [`KILL_ESCALATION`] SIGKILL
    /// it and exit (resolves when the worker should exit with status 0).
    pub async fn on_disconnect(&self) {
        todo!("W1")
    }

    /// TS: SessionWorker.handle: runs one command, answering `fail(requestId, error)` when it has a request ID and
    /// throws.
    pub async fn handle(&self, command: WorkerCommand) {
        let _ = command;
        todo!("W1")
    }

    /// The `try` body of TS SessionWorker.handle. Pane commands for any target but `agent` fail with
    /// `<target> pane is hosted by the workspace worker`.
    pub async fn dispatch(&self, command: WorkerCommand) -> Result<(), DhError> {
        let _ = command;
        todo!("W1")
    }

    /// TS: SessionWorker.startAgent: spawns `session.command`/`args` in `session.cwd` (inheriting the environment),
    /// posts `running {pid}`, arms the initial idle timer ([`IDLE_AFTER`]) and starts the output pump. Answers
    /// `{...session, status: 'running', pid}`.
    pub fn start_agent(
        &self,
        session: Ordered<SessionRecord>,
        cols: i64,
        rows: i64,
    ) -> Result<Ordered<SessionRecord>, DhError> {
        let _ = (session, cols, rows);
        todo!("W1")
    }

    /// TS: the agent's `term.onData` handler: stamps `lastDataAt`, updates terminal modes, feeds the preview,
    /// schedules the activity evaluation and the preview broadcast, and posts `output` while attached.
    pub fn on_agent_data(&self, agent: &SharedAgent, output: &str) {
        let _ = (agent, output);
        todo!("W1")
    }

    /// TS: SessionWorker.handleAgentExit: marks the agent exited, clears its timers, waits for quiet output, posts
    /// `exit {exitCode, exitSignal, lastPreview}` and exits the process after [`EXIT_DELAY`].
    pub async fn handle_agent_exit(&self, exit_code: Option<i64>, exit_signal: Option<i64>) {
        let _ = (exit_code, exit_signal);
        todo!("W1")
    }

    /// TS: SessionWorker.kill: SIGTERM the agent's process group (coding agents often run below a shell, so the
    /// direct child alone is not enough); `force` adds SIGKILL after [`KILL_ESCALATION`]. Fails with
    /// `session is not running`.
    pub fn kill(&self, force: bool) -> Result<(), DhError> {
        let _ = force;
        todo!("W1")
    }

    /// TS: SessionWorker.attachAgent: the snapshot plus `{terminalModes, initialFrame}` while the agent exists.
    pub fn attach_agent(&self, cols: i64, rows: i64) -> Ordered<AttachSnapshot> {
        let _ = (cols, rows);
        todo!("W1")
    }

    /// TS: SessionWorker.snapshotAgent: resizes PTY and preview (a real size change suppresses activity), then
    /// `{sessionId, content, live: true, status: 'running', agentStatus, scrollOffset, maxScrollOffset}`;
    /// `{content: '', live: false}` without an agent.
    pub fn snapshot_agent(&self, cols: i64, rows: i64, scroll_offset: Option<i64>) -> PreviewRecord {
        let _ = (cols, rows, scroll_offset);
        todo!("W1")
    }

    /// TS: SessionWorker.resizeAgent (a no-op once exited).
    pub fn resize_agent(&self, cols: i64, rows: i64) {
        let _ = (cols, rows);
        todo!("W1")
    }

    /// TS: `this.agent?.term.write(data)` (`input` for the agent).
    pub async fn input_agent(&self, data: &str) {
        let _ = data;
        todo!("W1")
    }

    /// TS: SessionWorker.schedulePreviewBroadcast: at most one pending broadcast; after [`PREVIEW_BROADCAST_DELAY`]
    /// posts `preview-updated` with the current snapshot and scroll info.
    pub fn schedule_preview_broadcast(&self) {
        todo!("W1")
    }

    /// TS: SessionWorker.scheduleActivityEvaluation: at most one pending evaluation, none while resize-suppressed.
    pub fn schedule_activity_evaluation(&self) {
        todo!("W1")
    }

    /// TS: SessionWorker.evaluatePreviewActivity: counts changed characters since the last snapshot, keeps the
    /// changes of the last [`ACTIVITY_WINDOW`], goes `active` when they reach [`ACTIVE_MIN_CHANGED_CHARS`] and re-arms
    /// the idle timer ([`IDLE_AFTER`], which also clears the window).
    pub fn evaluate_preview_activity(&self) {
        todo!("W1")
    }

    /// TS: SessionWorker.suppressResizeActivity: ignore preview changes for [`RESIZE_ACTIVITY_SUPPRESSION`] and take
    /// the resized snapshot as the new baseline.
    pub fn suppress_resize_activity(&self) {
        todo!("W1")
    }

    /// TS: SessionWorker.setAgentStatus: on a change, stamps `agentStatus` and `agentStatusUpdatedAt` (ISO time) on
    /// the session, posts `agent-status` and schedules a preview broadcast.
    pub fn set_agent_status(&self, agent_status: AgentActivityStatus) {
        let _ = agent_status;
        todo!("W1")
    }

    /// TS: SessionWorker.clearActivityTimers
    pub fn clear_activity_timers(runtime: &mut AgentRuntime) {
        let _ = runtime;
        todo!("W1")
    }
}

/// TS: sessionWorker.ts runSessionWorker (`deckhand --session-worker`): opens the channel on fd 3 (posting `hello`)
/// and serves the daemon until it disconnects or the agent exits. Must run inside the process's `LocalSet`. Returns
/// when the worker should exit with status 0; fails only when the channel cannot be opened. The TS
/// `process.title = 'deckhand-session-worker'` has no portable equivalent and is not ported.
pub async fn run_session_worker() -> Result<(), DhError> {
    todo!("W1")
}
