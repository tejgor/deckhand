//! Port of sessionWorker.ts (workspace worker: PaneHost for Terminal/Git/Dev panes and actions).
//!
//! A workspace worker hosts the panes shared by every session in one workspace (worktree): Terminal, Git, Dev and the
//! last action. The daemon starts it on demand (first Terminal/Git view or attach, Dev start, an action run) and
//! retires it once nothing is left in it (after a Dev or action stop, or a failed pane start), before the worktree is
//! deleted, when the workspace's last session is removed, or on shutdown. Its own panes never retire it: a shell never
//! exits on its own, and exited panes keep their output.
//!
//! Pane records carry no session: the daemon stamps each viewing session's ID (and the workspace) on them.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::rc::Rc;
use std::time::Duration;

use deckhand_core::error::DhError;
use deckhand_protocol::data::{AttachSnapshot, AttachTarget, PaneRecord};
use deckhand_protocol::worker_ipc::{PaneAttach, PaneDetach, PaneInput, PaneResize, Snapshot};
use deckhand_protocol::{Ordered, WorkerMessage, WorkspaceWorkerCommand};
use deckhand_sys::async_util::{SharedTask, Task};

use crate::channel::{ChannelReader, WorkerChannel};
use crate::session::{RuntimePty, SharedRuntime};

/// TS: sessionWorker.ts PANE_KILL_GRACE_MS: SIGKILL follows a pane stop's SIGHUP after 2 s.
pub const PANE_KILL_GRACE: Duration = Duration::from_millis(2000);

/// TS: sessionWorker.ts ROOT_SESSION_ENV: root-session credentials stay with the agent; nested agents started from
/// a pane must not reuse them, so pane environments drop these.
pub const ROOT_SESSION_ENV: &[&str] = &["DECKHAND_HOOK_TOKEN", "DECKHAND_LAUNCH_ID", "DECKHAND_SESSION_ID"];

/// The error lazygit lookup fails with.
pub const LAZYGIT_MISSING: &str = "lazygit is not installed or not on PATH";

/// TS: sessionWorker.ts PaneTarget (`Exclude<AttachTarget, 'agent'>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PaneTarget {
    Terminal,
    Git,
    Dev,
    Action,
}

impl PaneTarget {
    /// The attach target this pane is.
    pub fn attach_target(self) -> AttachTarget {
        todo!("W2")
    }

    /// The pane an attach target names (`None` for `agent`).
    pub fn from_attach_target(target: AttachTarget) -> Option<PaneTarget> {
        let _ = target;
        todo!("W2")
    }

    /// The wire string (`terminal`, `git`, `dev`, `action`).
    pub fn as_str(self) -> &'static str {
        todo!("W2")
    }
}

/// TS: sessionWorker.ts WORKSPACE_PANES: the companion panes a workspace worker hosts. `action` is the last action
/// run (shown on the Terminal tab), a PTY of its own so it never touches the shell or Dev.
pub const WORKSPACE_PANES: &[PaneTarget] = &[
    PaneTarget::Terminal,
    PaneTarget::Git,
    PaneTarget::Dev,
    PaneTarget::Action,
];

/// TS: sessionWorker.ts PaneCommand: the pane commands shared by both worker kinds.
#[derive(Clone, Debug, PartialEq)]
pub enum PaneCommand {
    Snapshot(Snapshot),
    Attach(PaneAttach),
    Detach(PaneDetach),
    Input(PaneInput),
    Resize(PaneResize),
}

impl PaneCommand {
    /// The pane command a workspace worker command is, or the command back when it is not one.
    pub fn from_workspace(command: WorkspaceWorkerCommand) -> Result<PaneCommand, WorkspaceWorkerCommand> {
        let _ = command;
        todo!("W2")
    }
}

/// The `requestId` of a workspace worker command, if it has one (TS `'requestId' in command`).
pub fn request_id(command: &WorkspaceWorkerCommand) -> Option<&str> {
    let _ = command;
    todo!("W2")
}

/// TS: sessionWorker.ts lazyGitCommand: `$SHELL -ic 'command -v lazygit'` (`$SHELL` defaulting to `/bin/bash`), the
/// trimmed path; fails with [`LAZYGIT_MISSING`] when the lookup fails or prints nothing.
pub async fn lazy_git_command() -> Result<String, DhError> {
    todo!("W2")
}

/// TS: sessionWorker.ts paneUpdated: `terminal-updated` / `git-updated` / `action-updated` / `dev-updated`.
pub fn pane_updated(target: PaneTarget, record: Ordered<PaneRecord>) -> WorkerMessage {
    let _ = (target, record);
    todo!("W2")
}

/// TS: spawnPane's `env`: this process's environment without [`ROOT_SESSION_ENV`].
pub fn pane_env() -> Vec<(OsString, OsString)> {
    todo!("W2")
}

/// TS: sessionWorker.ts PaneHost: the companion pane PTYs of one workspace and their headless previews.
///
/// A cheap handle (clones share one host), so pump tasks and broadcast timers can call back into it.
#[derive(Clone)]
pub struct PaneHost {
    inner: Rc<PaneHostInner>,
}

#[allow(dead_code)] // W2: read once the methods below are written.
struct PaneHostInner {
    targets: Vec<PaneTarget>,
    /// TS `owner: () => {cwd?}`: the workspace's cwd once started.
    owner: Box<dyn Fn() -> Option<String>>,
    channel: WorkerChannel,
    panes: RefCell<HashMap<PaneTarget, SharedRuntime>>,
    /// TS `startPromises`: in-flight Git/Dev/action starts, awaited by concurrent callers.
    start_promises: RefCell<HashMap<PaneTarget, SharedTask<Result<SharedRuntime, DhError>>>>,
    attached: RefCell<HashSet<PaneTarget>>,
}

impl PaneHost {
    /// TS: new PaneHost(targets, owner), posting to `channel`.
    pub fn new(targets: &[PaneTarget], owner: impl Fn() -> Option<String> + 'static, channel: WorkerChannel) -> Self {
        let _ = (targets, owner, channel);
        todo!("W2")
    }

    /// TS: PaneHost.idle: true when no pane runtime (live or exited with output) remains and none is starting.
    pub fn is_idle(&self) -> bool {
        let _ = &self.inner;
        todo!("W2")
    }

    /// TS: PaneHost.hosted: fails with `<target> pane is not hosted by this worker` for `agent` or a pane outside
    /// the host's targets.
    pub fn hosted(&self, target: AttachTarget) -> Result<PaneTarget, DhError> {
        let _ = target;
        todo!("W2")
    }

    /// TS: PaneHost.cwd: fails with `workspace is not started`.
    pub fn cwd(&self) -> Result<String, DhError> {
        todo!("W2")
    }

    /// TS: PaneHost.ensureTerminal: the live shell (resized), or a fresh `$SHELL` after stopping an exited one.
    pub async fn ensure_terminal(&self, cols: i64, rows: i64) -> Result<SharedRuntime, DhError> {
        let _ = (cols, rows);
        todo!("W2")
    }

    /// TS: PaneHost.ensureGit: the live lazygit (resized), the pending start, or a new one (resolving lazygit first).
    pub async fn ensure_git(&self, cols: i64, rows: i64) -> Result<SharedRuntime, DhError> {
        let _ = (cols, rows);
        todo!("W2")
    }

    /// TS: PaneHost.startDev: reuses a live or starting Dev pane (failing with `Stop the current Dev command before
    /// starting another` for a different requested command), else runs `$SHELL -ic <command>` where the command is
    /// the requested one, the config's `dev_command`, or `dev` (all trimmed).
    pub async fn start_dev(
        &self,
        cols: i64,
        rows: i64,
        requested_command: Option<&str>,
    ) -> Result<Ordered<PaneRecord>, DhError> {
        let _ = (cols, rows, requested_command);
        todo!("W2")
    }

    /// TS: PaneHost.startAction: one action at a time per workspace; a finished one is replaced (its output and exit
    /// code stay until then). Fails with `The <name|previous> action is still running; stop it first (r, then x)`.
    pub async fn start_action(
        &self,
        cols: i64,
        rows: i64,
        command: &str,
        name: &str,
    ) -> Result<Ordered<PaneRecord>, DhError> {
        let _ = (cols, rows, command, name);
        todo!("W2")
    }

    /// TS: PaneHost.spawnPane: spawns `command args` in `cwd` with [`pane_env`], makes it the target's current pane
    /// and starts its pump (a replaced or stopped pane may keep emitting until it dies; only the current one reports).
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_pane(
        &self,
        target: PaneTarget,
        command: &str,
        args: &[String],
        cwd: &str,
        cols: i64,
        rows: i64,
        label: Option<&str>,
    ) -> Result<SharedRuntime, DhError> {
        let _ = (target, command, args, cwd, cols, rows, label);
        todo!("W2")
    }

    /// TS: the pane's `term.onData` handler: updates terminal modes, feeds the preview, schedules a broadcast and
    /// posts `output` while attached (only for the target's current runtime).
    pub fn on_pane_data(&self, target: PaneTarget, runtime: &SharedRuntime, output: &str) {
        let _ = (target, runtime, output);
        todo!("W2")
    }

    /// TS: the pane's `term.onExit` handler: records the exit and, for the current runtime, schedules a broadcast.
    pub fn on_pane_exit(
        &self,
        target: PaneTarget,
        runtime: &SharedRuntime,
        exit_code: Option<i64>,
        exit_signal: Option<i64>,
    ) {
        let _ = (target, runtime, exit_code, exit_signal);
        todo!("W2")
    }

    /// TS: PaneHost.snapshot: Terminal and Git start on view; Dev and actions start only on request (viewing never
    /// starts them).
    pub async fn snapshot(&self, target: AttachTarget, cols: i64, rows: i64) -> Result<Ordered<PaneRecord>, DhError> {
        let _ = (target, cols, rows);
        todo!("W2")
    }

    /// TS: PaneHost.attach: marks the pane attached, then its snapshot plus `{terminalModes, initialFrame}` while a
    /// runtime exists.
    pub async fn attach(&self, target: AttachTarget, cols: i64, rows: i64) -> Result<Ordered<AttachSnapshot>, DhError> {
        let _ = (target, cols, rows);
        todo!("W2")
    }

    /// TS: PaneHost.detach
    pub fn detach(&self, target: AttachTarget) -> Result<(), DhError> {
        let _ = target;
        todo!("W2")
    }

    /// TS: PaneHost.input (`term.write(data)` on the pane, if it has a runtime).
    pub async fn input(&self, target: AttachTarget, data: &str) -> Result<(), DhError> {
        let _ = (target, data);
        todo!("W2")
    }

    /// TS: PaneHost.resize (a no-op without a live runtime), then a broadcast.
    pub fn resize(&self, target: AttachTarget, cols: i64, rows: i64) -> Result<(), DhError> {
        let _ = (target, cols, rows);
        todo!("W2")
    }

    /// TS: PaneHost.resizeRuntime: resizes the PTY (unless exited) and the preview, refreshing its cached snapshot.
    pub fn resize_runtime(runtime: &SharedRuntime, cols: i64, rows: i64) -> SharedRuntime {
        let _ = (runtime, cols, rows);
        todo!("W2")
    }

    /// TS: PaneHost.stop: forgets the pane and signals it at once (SIGHUP to the group: interactive shells ignore
    /// SIGTERM), snapshotting its descendants first; the task resolves once it exited, or after [`PANE_KILL_GRACE`]
    /// SIGKILLs the group and the surviving descendants (including jobs the shell moved to their own process group).
    /// The shell exiting first ends the wait only when nothing survived it. Dropping the task does not cancel the
    /// escalation (TS `void this.stop(...)`).
    pub fn stop(&self, target: PaneTarget) -> Task<()> {
        let _ = target;
        todo!("W2")
    }

    /// TS: PaneHost.stopAll: [`PaneHost::stop`] for every hosted target, resolving when all did.
    pub fn stop_all(&self) -> Task<()> {
        todo!("W2")
    }

    /// TS: PaneHost.record: `{content, live, cwd, command?, name?, exitCode, exitSignal}` (`command` for Dev and
    /// action, `name` for action; `cwd` falls back to the workspace's).
    pub fn record(&self, target: PaneTarget, runtime: Option<&RuntimePty>) -> Ordered<PaneRecord> {
        let _ = (target, runtime);
        todo!("W2")
    }

    /// TS: PaneHost.scheduleBroadcast: at most one pending broadcast per runtime; after
    /// [`crate::session::PREVIEW_BROADCAST_DELAY`] refreshes the snapshot and posts [`pane_updated`].
    pub fn schedule_broadcast(&self, target: PaneTarget, runtime: &SharedRuntime) {
        let _ = (target, runtime);
        todo!("W2")
    }
}

/// TS: sessionWorker.ts handlePaneCommand: runs a pane command on `host`, answering `snapshot`/`attach` with `ok`.
pub async fn handle_pane_command(host: &PaneHost, command: PaneCommand) -> Result<(), DhError> {
    let _ = (host, command);
    todo!("W2")
}

/// TS: sessionWorker.ts WorkspaceWorker: hosts the panes shared by every session in one workspace.
///
/// A cheap handle (clones share one worker).
#[derive(Clone)]
pub struct WorkspaceWorker {
    inner: Rc<WorkspaceWorkerInner>,
}

#[allow(dead_code)] // W2: read once the methods below are written.
struct WorkspaceWorkerInner {
    channel: WorkerChannel,
    /// Shared with the [`PaneHost`]'s owner closure.
    cwd: Rc<RefCell<Option<String>>>,
    stopping: Cell<bool>,
    panes: PaneHost,
}

impl WorkspaceWorker {
    /// A worker hosting [`WORKSPACE_PANES`], posting to `channel`, not started.
    pub fn new(channel: WorkerChannel) -> Self {
        let _ = channel;
        todo!("W2")
    }

    /// TS: WorkspaceWorker.start: switches the wire-log role to `workspace-worker`, serves commands from `reader`
    /// (each in its own local task) until the daemon disconnects, then stops every pane and returns (the worker then
    /// exits with status 0).
    pub async fn start(&self, reader: ChannelReader) {
        let _ = (&self.inner, reader);
        todo!("W2")
    }

    /// TS: WorkspaceWorker.handle: runs one command, answering `fail(requestId, error)` when it has a request ID and
    /// throws.
    pub async fn handle(&self, command: WorkspaceWorkerCommand) {
        let _ = command;
        todo!("W2")
    }

    /// The `try` body of TS WorkspaceWorker.handle. Nothing may start in a worker that is shutting down (its worktree
    /// may be about to be removed): anything but `shutdown` and `detach` then fails with `workspace worker is
    /// stopping`.
    pub async fn dispatch(&self, command: WorkspaceWorkerCommand) -> Result<(), DhError> {
        let _ = command;
        todo!("W2")
    }

    /// TS: WorkspaceWorker.stopPane: stops the pane without waiting for it to exit and posts its now-empty record
    /// (the response follows once the pane is signalled, not once it exited).
    pub fn stop_pane(&self, target: PaneTarget) {
        let _ = target;
        todo!("W2")
    }
}

/// TS: sessionWorker.ts runWorkspaceWorker (`deckhand --workspace-worker`): opens the channel on fd 3 (posting
/// `hello`) and serves the daemon until it disconnects or a `shutdown`. Must run inside the process's `LocalSet`.
/// Returns when the worker should exit with status 0; fails only when the channel cannot be opened. The TS
/// `process.title = 'deckhand-workspace-worker'` is not ported.
pub async fn run_workspace_worker() -> Result<(), DhError> {
    todo!("W2")
}
