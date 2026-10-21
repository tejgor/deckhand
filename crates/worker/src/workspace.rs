//! Workspace worker: PaneHost for Terminal/Git/Dev panes and actions.
//!
//! A workspace worker hosts the panes shared by every session in one workspace (worktree): Terminal, Git, Dev and the
//! last action. The daemon starts it on demand (first Terminal/Git view or attach, Dev start, an action run) and
//! retires it once nothing is left in it (after a Dev or action stop, or a failed pane start), before the worktree is
//! deleted, when the workspace's last session is removed, or on shutdown. Its own panes never retire it: a shell never
//! exits on its own, and exited panes keep their output.
//!
//! Pane records carry no session: the daemon stamps each viewing session's ID (and the workspace) on them.
//!
//! Exiting: TS called `process.exit(0)` from a timer after `shutdown`, and after stopping every pane on disconnect.
//! Here [`WorkspaceWorker::start`] returns instead; [`run_workspace_worker`] then flushes the channel and returns, and
//! the binary exits with status 0.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::rc::Rc;
use std::time::Duration;

use nix::sys::signal::Signal;
use tokio::sync::watch;

use deckhand_core::error::DhError;
use deckhand_core::jsstr::js_trim;
use deckhand_protocol::data::{AttachSnapshot, AttachTarget, DevProcessRecord, OkResult, PaneRecord};
use deckhand_protocol::worker_ipc::{
    DevProcessSpec, IdleResult, PaneAttach, PaneDetach, PaneInput, PaneResize, Snapshot, StartDev, WorkerActionUpdated,
    WorkerDevUpdated, WorkerGitUpdated, WorkerOutput, WorkerTerminalUpdated,
};
use deckhand_protocol::{Ordered, WorkerMessage, WorkspaceWorkerCommand};
use deckhand_sys::async_util::{KeyedSharedSlot, Task, Timer, spawn};
use deckhand_sys::proc::{ExecOptions, exec};
use deckhand_sys::process_tree::{ProcessSnapshot, kill_survivors, snapshot_descendants, survivors};
use deckhand_sys::storage::load_app_config;

use crate::channel::{ChannelReader, WireRole, WorkerChannel};
use crate::session::{
    DEFAULT_COLS, DEFAULT_ROWS, EXIT_DELAY, PREVIEW_BROADCAST_DELAY, RuntimePty, SharedRuntime, pump_pty,
    shell_command, signal_pty_process, size, spawn_pty, update_terminal_modes,
};

/// TS: sessionWorker.ts PANE_KILL_GRACE_MS: SIGKILL follows a pane stop's SIGHUP after 2 s.
pub const PANE_KILL_GRACE: Duration = Duration::from_millis(2000);

/// TS: sessionWorker.ts ROOT_SESSION_ENV: root-session credentials stay with the agent; nested agents started from
/// a pane must not reuse them, so pane environments drop these.
pub const ROOT_SESSION_ENV: &[&str] = &["DECKHAND_HOOK_TOKEN", "DECKHAND_LAUNCH_ID", "DECKHAND_SESSION_ID"];

/// The error lazygit lookup fails with.
pub const LAZYGIT_MISSING: &str = "lazygit is not installed or not on PATH";

/// TS: sessionWorker.ts PaneTarget (`Exclude<AttachTarget, 'agent'>`).
///
/// Not in the TS: Dev carries the index of its process. A single Dev command is `Dev(0)`; named processes (`devCommand` as a map)
/// are `Dev(0)`, `Dev(1)`, ... in their configured order, all shown and attached as the one Dev pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PaneTarget {
    Terminal,
    Git,
    Dev(u8),
    Action,
}

impl PaneTarget {
    /// The attach target this pane is.
    pub fn attach_target(self) -> AttachTarget {
        match self {
            PaneTarget::Terminal => AttachTarget::Terminal,
            PaneTarget::Git => AttachTarget::Git,
            PaneTarget::Dev(_) => AttachTarget::Dev,
            PaneTarget::Action => AttachTarget::Action,
        }
    }

    /// Whether this is one of Dev's processes.
    pub fn is_dev(self) -> bool {
        matches!(self, PaneTarget::Dev(_))
    }

    /// The pane an attach target names (`None` for `agent`).
    pub fn from_attach_target(target: AttachTarget) -> Option<PaneTarget> {
        match target {
            AttachTarget::Agent => None,
            AttachTarget::Terminal => Some(PaneTarget::Terminal),
            AttachTarget::Git => Some(PaneTarget::Git),
            AttachTarget::Dev => Some(PaneTarget::Dev(0)),
            AttachTarget::Action => Some(PaneTarget::Action),
        }
    }

    /// The wire string (`terminal`, `git`, `dev`, `action`).
    pub fn as_str(self) -> &'static str {
        self.attach_target().as_str()
    }
}

/// TS: sessionWorker.ts WORKSPACE_PANES: the companion panes a workspace worker hosts. `action` is the last action
/// run (shown on the Terminal tab), a PTY of its own so it never touches the shell or Dev.
pub const WORKSPACE_PANES: &[PaneTarget] = &[
    PaneTarget::Terminal,
    PaneTarget::Git,
    PaneTarget::Dev(0),
    PaneTarget::Action,
];

/// Refused when Dev runs something else than what is asked for.
const STOP_DEV_FIRST: &str = "Stop the current Dev command before starting another";

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
        match command {
            WorkspaceWorkerCommand::Snapshot(snapshot) => Ok(PaneCommand::Snapshot(snapshot)),
            WorkspaceWorkerCommand::Attach(attach) => Ok(PaneCommand::Attach(attach)),
            WorkspaceWorkerCommand::Detach(detach) => Ok(PaneCommand::Detach(detach)),
            WorkspaceWorkerCommand::Input(input) => Ok(PaneCommand::Input(input)),
            WorkspaceWorkerCommand::Resize(resize) => Ok(PaneCommand::Resize(resize)),
            other => Err(other),
        }
    }
}

/// The `requestId` of a workspace worker command, if it has one (TS `'requestId' in command`).
pub fn request_id(command: &WorkspaceWorkerCommand) -> Option<&str> {
    match command {
        WorkspaceWorkerCommand::Snapshot(snapshot) => Some(&snapshot.request_id),
        WorkspaceWorkerCommand::Attach(attach) => Some(&attach.request_id),
        WorkspaceWorkerCommand::Start(start) => Some(&start.request_id),
        WorkspaceWorkerCommand::StartDev(start) => Some(&start.request_id),
        WorkspaceWorkerCommand::StartAction(start) => Some(&start.request_id),
        WorkspaceWorkerCommand::StopDev(request) => Some(&request.request_id),
        WorkspaceWorkerCommand::StopAction(request)
        | WorkspaceWorkerCommand::Idle(request)
        | WorkspaceWorkerCommand::Shutdown(request) => Some(&request.request_id),
        WorkspaceWorkerCommand::Detach(_) | WorkspaceWorkerCommand::Input(_) | WorkspaceWorkerCommand::Resize(_) => {
            None
        }
    }
}

/// TS: sessionWorker.ts lazyGitCommand: `$SHELL -ic 'command -v lazygit'` (`$SHELL` defaulting to `/bin/bash`), the
/// trimmed path; fails with [`LAZYGIT_MISSING`] when the lookup fails or prints nothing.
pub async fn lazy_git_command() -> Result<String, DhError> {
    let shell = match std::env::var_os("SHELL") {
        Some(shell) if !shell.is_empty() => shell.to_string_lossy().into_owned(),
        _ => "/bin/bash".to_owned(),
    };
    if let Ok(output) = exec(&shell, &["-ic", "command -v lazygit"], &ExecOptions::default()).await {
        let resolved = js_trim(&output.stdout);
        if !resolved.is_empty() {
            return Ok(resolved.to_owned());
        }
    }
    Err(DhError::msg(LAZYGIT_MISSING))
}

/// TS: sessionWorker.ts paneUpdated: `terminal-updated` / `git-updated` / `action-updated` / `dev-updated`.
pub fn pane_updated(target: PaneTarget, record: Ordered<PaneRecord>) -> WorkerMessage {
    match target {
        PaneTarget::Terminal => WorkerMessage::TerminalUpdated(WorkerTerminalUpdated { terminal: record }),
        PaneTarget::Git => WorkerMessage::GitUpdated(WorkerGitUpdated { git: record }),
        PaneTarget::Action => WorkerMessage::ActionUpdated(WorkerActionUpdated { action: record }),
        PaneTarget::Dev(_) => WorkerMessage::DevUpdated(WorkerDevUpdated { dev: record }),
    }
}

/// TS: spawnPane's `env`: this process's environment without [`ROOT_SESSION_ENV`].
pub fn pane_env() -> Vec<(OsString, OsString)> {
    std::env::vars_os()
        .filter(|(key, _)| !ROOT_SESSION_ENV.iter().any(|root| key == *root))
        .collect()
}

/// TS `requestedCommand?.trim()` (and `config.dev_command?.trim()`) used as a truthy value: `None` when empty.
fn trimmed(value: Option<&str>) -> Option<String> {
    value.map(js_trim).filter(|text| !text.is_empty()).map(str::to_owned)
}

/// TS: sessionWorker.ts PaneHost: the companion pane PTYs of one workspace and their headless previews.
///
/// A cheap handle (clones share one host), so pump tasks and broadcast timers can call back into it.
#[derive(Clone)]
pub struct PaneHost {
    inner: Rc<PaneHostInner>,
}

struct PaneHostInner {
    targets: Vec<PaneTarget>,
    /// TS `owner: () => {cwd?}`: the workspace's cwd once started.
    owner: Box<dyn Fn() -> Option<String>>,
    channel: WorkerChannel,
    panes: RefCell<HashMap<PaneTarget, SharedRuntime>>,
    /// TS `startPromises`: in-flight Git/Dev/action starts, awaited by concurrent callers. A start leaves the map
    /// when it settles (TS: `finally` deletes it if it is still the current one).
    start_promises: KeyedSharedSlot<PaneTarget, Result<SharedRuntime, DhError>>,
    attached: RefCell<HashSet<PaneTarget>>,
    /// Not in the TS: the named Dev processes last started (`Dev(i)` runs the i-th); `None` for a single Dev command.
    dev_processes: RefCell<Option<Vec<DevProcessSpec>>>,
}

impl PaneHost {
    /// TS: new PaneHost(targets, owner), posting to `channel`.
    pub fn new(targets: &[PaneTarget], owner: impl Fn() -> Option<String> + 'static, channel: WorkerChannel) -> Self {
        // A Set in TS: duplicates collapse, insertion order is kept.
        let mut unique: Vec<PaneTarget> = Vec::with_capacity(targets.len());
        for target in targets {
            if !unique.contains(target) {
                unique.push(*target);
            }
        }
        PaneHost {
            inner: Rc::new(PaneHostInner {
                targets: unique,
                owner: Box::new(owner),
                channel,
                panes: RefCell::new(HashMap::new()),
                start_promises: KeyedSharedSlot::new(),
                attached: RefCell::new(HashSet::new()),
                dev_processes: RefCell::new(None),
            }),
        }
    }

    /// TS: PaneHost.idle: true when no pane runtime (live or exited with output) remains and none is starting.
    pub fn is_idle(&self) -> bool {
        self.inner.start_promises.is_empty() && self.inner.panes.borrow().is_empty()
    }

    /// TS: PaneHost.hosted: fails with `<target> pane is not hosted by this worker` for `agent` or a pane outside
    /// the host's targets.
    pub fn hosted(&self, target: AttachTarget) -> Result<PaneTarget, DhError> {
        match PaneTarget::from_attach_target(target) {
            Some(pane) if self.inner.targets.contains(&pane) => Ok(pane),
            _ => Err(DhError::msg(format!("{target} pane is not hosted by this worker"))),
        }
    }

    /// TS: PaneHost.cwd: fails with `workspace is not started`.
    pub fn cwd(&self) -> Result<String, DhError> {
        match (self.inner.owner)() {
            Some(cwd) if !cwd.is_empty() => Ok(cwd),
            _ => Err(DhError::msg("workspace is not started")),
        }
    }

    /// The target's current runtime (`this.panes[target]`).
    fn pane(&self, target: PaneTarget) -> Option<SharedRuntime> {
        self.inner.panes.borrow().get(&target).cloned()
    }

    /// The target's current runtime while it has not exited (`existing && !existing.exited`).
    fn live(&self, target: PaneTarget) -> Option<SharedRuntime> {
        self.pane(target).filter(|runtime| !runtime.borrow().exited)
    }

    /// Whether `runtime` is still the target's current one (TS `current()` in spawnPane).
    fn is_current(&self, target: PaneTarget, runtime: &SharedRuntime) -> bool {
        self.inner
            .panes
            .borrow()
            .get(&target)
            .is_some_and(|current| Rc::ptr_eq(current, runtime))
    }

    /// [`PaneHost::record`] of a shared runtime.
    fn record_of(&self, target: PaneTarget, runtime: &SharedRuntime) -> Ordered<PaneRecord> {
        let runtime = runtime.borrow();
        self.record(target, Some(&runtime))
    }

    /// TS: PaneHost.ensureTerminal: the live shell (resized), or a fresh `$SHELL` after stopping an exited one.
    pub async fn ensure_terminal(&self, cols: i64, rows: i64) -> Result<SharedRuntime, DhError> {
        if let Some(existing) = self.live(PaneTarget::Terminal) {
            return Ok(PaneHost::resize_runtime(&existing, cols, rows));
        }
        drop(self.stop(PaneTarget::Terminal));
        let command = shell_command();
        let cwd = self.cwd()?;
        self.spawn_pane(PaneTarget::Terminal, &command, &[], &cwd, cols, rows, None)
    }

    /// TS: PaneHost.ensureGit: the live lazygit (resized), the pending start, or a new one (resolving lazygit first).
    pub async fn ensure_git(&self, cols: i64, rows: i64) -> Result<SharedRuntime, DhError> {
        if let Some(existing) = self.live(PaneTarget::Git) {
            return Ok(PaneHost::resize_runtime(&existing, cols, rows));
        }
        if let Some(pending) = self.inner.start_promises.get(&PaneTarget::Git) {
            let runtime = pending.await?;
            return Ok(PaneHost::resize_runtime(&runtime, cols, rows));
        }
        // The synchronous head of the TS start: the old pane is stopped before anything is awaited.
        drop(self.stop(PaneTarget::Git));
        let host = self.clone();
        let start = self
            .inner
            .start_promises
            .get_or_start(PaneTarget::Git, move || async move {
                let command = lazy_git_command().await?;
                if let Some(current) = host.live(PaneTarget::Git) {
                    return Ok(current);
                }
                let cwd = host.cwd()?;
                host.spawn_pane(PaneTarget::Git, &command, &[], &cwd, cols, rows, None)
            });
        start.await
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
        self.hosted(AttachTarget::Dev)?;
        // Named processes still running were started from another configuration.
        if self.inner.dev_processes.borrow().is_some() {
            if self.dev_targets().into_iter().any(|target| self.live(target).is_some()) {
                return Err(DhError::msg(STOP_DEV_FIRST));
            }
            drop(self.stop_dev_targets());
            *self.inner.dev_processes.borrow_mut() = None;
        }
        let requested = trimmed(requested_command);
        let reuse = |runtime: SharedRuntime| -> Result<Ordered<PaneRecord>, DhError> {
            if let Some(requested) = &requested
                && runtime.borrow().command.as_deref() != Some(requested.as_str())
            {
                return Err(DhError::msg(STOP_DEV_FIRST));
            }
            let runtime = PaneHost::resize_runtime(&runtime, cols, rows);
            Ok(self.record_of(PaneTarget::Dev(0), &runtime))
        };
        if let Some(existing) = self.live(PaneTarget::Dev(0)) {
            return reuse(existing);
        }
        if let Some(pending) = self.inner.start_promises.get(&PaneTarget::Dev(0)) {
            return reuse(pending.await?);
        }

        drop(self.stop(PaneTarget::Dev(0)));
        let host = self.clone();
        let wanted = requested.clone();
        let start = self
            .inner
            .start_promises
            .get_or_start(PaneTarget::Dev(0), move || async move {
                let config = load_app_config().await?;
                let command = wanted
                    .or_else(|| trimmed(config.dev_command.as_deref()))
                    .unwrap_or_else(|| "dev".to_owned());
                if let Some(current) = host.live(PaneTarget::Dev(0)) {
                    return Ok(current);
                }
                let cwd = host.cwd()?;
                let args = ["-ic".to_owned(), command.clone()];
                host.spawn_pane(
                    PaneTarget::Dev(0),
                    &shell_command(),
                    &args,
                    &cwd,
                    cols,
                    rows,
                    Some(&command),
                )
            });
        let runtime = start.await?;
        Ok(self.record_of(PaneTarget::Dev(0), &runtime))
    }

    /// Not in the TS: starts Dev's named processes (`devCommand` as a map), each `$SHELL -ic <command>` in a PTY of its own,
    /// or only `only` (stopped first with `restart`, waiting until it exited so it releases its port). Running ones
    /// are kept; anything running that the list does not name the same way is refused ([`STOP_DEV_FIRST`]).
    pub async fn start_dev_processes(
        &self,
        cols: i64,
        rows: i64,
        processes: Vec<DevProcessSpec>,
        only: Option<&str>,
        restart: bool,
    ) -> Result<Ordered<PaneRecord>, DhError> {
        self.hosted(AttachTarget::Dev)?;
        if processes.is_empty() || processes.len() > usize::from(u8::MAX) {
            return Err(DhError::msg("Invalid Dev processes"));
        }
        if self.inner.start_promises.contains(&PaneTarget::Dev(0)) {
            return Err(DhError::msg(STOP_DEV_FIRST));
        }
        let current = self.inner.dev_processes.borrow().clone();
        for target in self.dev_targets() {
            let PaneTarget::Dev(index) = target else {
                continue;
            };
            let index = usize::from(index);
            let same = current
                .as_ref()
                .and_then(|current| current.get(index))
                .is_some_and(|running| processes.get(index) == Some(running));
            if !same && self.live(target).is_some() {
                return Err(DhError::msg(STOP_DEV_FIRST));
            }
            if !same {
                drop(self.stop(target));
            }
        }
        let selected: Vec<usize> = match only {
            None => (0..processes.len()).collect(),
            Some(name) => match processes.iter().position(|process| process.name == name) {
                Some(index) => vec![index],
                None => return Err(DhError::msg(format!("Unknown Dev process: {name}"))),
            },
        };
        *self.inner.dev_processes.borrow_mut() = Some(processes.clone());
        let cwd = self.cwd()?;
        for index in selected {
            let target = PaneTarget::Dev(u8::try_from(index).unwrap_or(u8::MAX));
            if let Some(live) = self.live(target) {
                if !restart {
                    PaneHost::resize_runtime(&live, cols, rows);
                    continue;
                }
                self.stop(target).await;
                // Started again meanwhile (another start-dev): that one stands.
                if self.live(target).is_some() {
                    continue;
                }
            } else {
                drop(self.stop(target));
            }
            let Some(process) = processes.get(index) else {
                continue;
            };
            let args = ["-ic".to_owned(), process.command.clone()];
            let runtime = self.spawn_pane(
                target,
                &shell_command(),
                &args,
                &cwd,
                cols,
                rows,
                Some(&process.command),
            )?;
            runtime.borrow_mut().name = Some(process.name.clone());
        }
        Ok(self.dev_record())
    }

    /// Not in the TS: the Dev targets that have a runtime, in process order.
    fn dev_targets(&self) -> Vec<PaneTarget> {
        let mut targets: Vec<PaneTarget> = self
            .inner
            .panes
            .borrow()
            .keys()
            .copied()
            .filter(|target| target.is_dev())
            .collect();
        targets.sort();
        targets
    }

    /// Not in the TS: stops every Dev process (see [`PaneHost::stop`]).
    fn stop_dev_targets(&self) -> Task<()> {
        let stops: Vec<Task<()>> = self.dev_targets().into_iter().map(|target| self.stop(target)).collect();
        spawn(async move {
            for stop in stops {
                stop.await;
            }
        })
    }

    /// Not in the TS: the Dev target a request names: the named process (`process`), else the attached one, else
    /// the first. Fails with `Unknown Dev process: <name>`.
    fn dev_target(&self, process: Option<&str>) -> Result<PaneTarget, DhError> {
        if let Some(name) = process {
            let index = self
                .inner
                .dev_processes
                .borrow()
                .as_ref()
                .and_then(|processes| processes.iter().position(|process| process.name == name));
            return match index.and_then(|index| u8::try_from(index).ok()) {
                Some(index) => Ok(PaneTarget::Dev(index)),
                None => Err(DhError::msg(format!("Unknown Dev process: {name}"))),
            };
        }
        let attached = self
            .inner
            .attached
            .borrow()
            .iter()
            .copied()
            .find(|target| target.is_dev());
        Ok(attached.unwrap_or(PaneTarget::Dev(0)))
    }

    /// Not in the TS: the pane a pane command acts on (Dev: the attached process, see [`PaneHost::dev_target`]).
    fn routed(&self, target: AttachTarget) -> Result<PaneTarget, DhError> {
        let pane = self.hosted(target)?;
        if pane.is_dev() {
            return self.dev_target(None);
        }
        Ok(pane)
    }

    /// Not in the TS: Dev's record. A single command's is its own; named processes give one record with every
    /// configured process (`live` while any runs, so `devRunning` and the sidebar's `▶` mean "Dev runs").
    pub fn dev_record(&self) -> Ordered<PaneRecord> {
        let processes = self.inner.dev_processes.borrow().clone();
        let Some(processes) = processes else {
            let runtime = self.pane(PaneTarget::Dev(0));
            let runtime = runtime.as_ref().map(|runtime| runtime.borrow());
            return self.record(PaneTarget::Dev(0), runtime.as_deref());
        };
        let processes: Vec<DevProcessRecord> = processes
            .into_iter()
            .enumerate()
            .map(|(index, process)| {
                let runtime = u8::try_from(index)
                    .ok()
                    .and_then(|index| self.pane(PaneTarget::Dev(index)));
                match runtime {
                    Some(runtime) => {
                        let runtime = runtime.borrow();
                        DevProcessRecord {
                            name: process.name,
                            content: runtime.preview.get_cached_snapshot(),
                            live: !runtime.exited,
                            command: runtime.command.clone(),
                            exit_code: runtime.exit_code.flatten(),
                            exit_signal: runtime.exit_signal.flatten(),
                        }
                    }
                    None => DevProcessRecord {
                        name: process.name,
                        command: Some(process.command),
                        ..DevProcessRecord::default()
                    },
                }
            })
            .collect();
        Ordered::new(PaneRecord {
            live: processes.iter().any(|process| process.live),
            cwd: (self.inner.owner)(),
            processes: Some(processes),
            ..PaneRecord::default()
        })
    }

    /// Not in the TS: stops Dev (every process, forgetting them) or only the named process.
    pub fn stop_dev(&self, process: Option<&str>) -> Result<(), DhError> {
        match process {
            None => {
                drop(self.stop_dev_targets());
                *self.inner.dev_processes.borrow_mut() = None;
            }
            Some(_) => {
                let target = self.dev_target(process)?;
                drop(self.stop(target));
            }
        }
        Ok(())
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
        self.hosted(AttachTarget::Action)?;
        let existing = self.pane(PaneTarget::Action);
        let running = existing.as_ref().is_some_and(|runtime| !runtime.borrow().exited);
        if self.inner.start_promises.contains(&PaneTarget::Action) || running {
            let previous = existing.and_then(|runtime| runtime.borrow().name.clone());
            let name = previous.as_deref().unwrap_or("previous");
            return Err(DhError::msg(format!(
                "The {name} action is still running; stop it first (r, then x)"
            )));
        }
        // Signalled now (the synchronous head of the TS start); the start awaits its exit.
        let stopped = self.stop(PaneTarget::Action);
        let host = self.clone();
        let command = command.to_owned();
        let name = name.to_owned();
        let start = self
            .inner
            .start_promises
            .get_or_start(PaneTarget::Action, move || async move {
                stopped.await;
                let cwd = host.cwd()?;
                let args = ["-ic".to_owned(), command.clone()];
                let runtime = host.spawn_pane(
                    PaneTarget::Action,
                    &shell_command(),
                    &args,
                    &cwd,
                    cols,
                    rows,
                    Some(&command),
                )?;
                runtime.borrow_mut().name = Some(name);
                Ok(runtime)
            });
        let runtime = start.await?;
        Ok(self.record_of(PaneTarget::Action, &runtime))
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
        let term = spawn_pty(command, args, cwd, Some(pane_env()), cols as f64, rows as f64)?;
        let runtime: SharedRuntime = Rc::new(RefCell::new(RuntimePty::new(
            term,
            cols as f64,
            rows as f64,
            cwd.to_owned(),
            label.map(str::to_owned),
        )));
        self.inner.panes.borrow_mut().insert(target, Rc::clone(&runtime));
        let pump = {
            let host = self.clone();
            let captured = Rc::clone(&runtime);
            let term = Rc::clone(&runtime.borrow().term);
            spawn(async move {
                let exit = pump_pty(term, |output| host.on_pane_data(target, &captured, output)).await;
                host.on_pane_exit(
                    target,
                    &captured,
                    Some(i64::from(exit.exit_code)),
                    Some(i64::from(exit.signal)),
                );
            })
        };
        runtime.borrow_mut().pump = Some(pump);
        self.schedule_broadcast(target, &runtime);
        Ok(runtime)
    }

    /// TS: the pane's `term.onData` handler: updates terminal modes, feeds the preview, schedules a broadcast and
    /// posts `output` while attached (only for the target's current runtime).
    pub fn on_pane_data(&self, target: PaneTarget, runtime: &SharedRuntime, output: &str) {
        if !self.is_current(target, runtime) {
            return;
        }
        {
            let mut current = runtime.borrow_mut();
            update_terminal_modes(&mut current.terminal_modes, output);
            current.preview.write(output);
        }
        self.schedule_broadcast(target, runtime);
        let attached = self.inner.attached.borrow().contains(&target);
        if attached {
            self.inner.channel.post(&WorkerMessage::Output(WorkerOutput {
                target: target.attach_target(),
                data: output.to_owned(),
            }));
        }
    }

    /// TS: the pane's `term.onExit` handler: records the exit and, for the current runtime, schedules a broadcast.
    pub fn on_pane_exit(
        &self,
        target: PaneTarget,
        runtime: &SharedRuntime,
        exit_code: Option<i64>,
        exit_signal: Option<i64>,
    ) {
        {
            let mut current = runtime.borrow_mut();
            current.exited = true;
            // `exitCode ?? null`.
            current.exit_code = Some(exit_code);
            current.exit_signal = Some(exit_signal);
        }
        if self.is_current(target, runtime) {
            self.schedule_broadcast(target, runtime);
        }
    }

    /// TS: PaneHost.snapshot: Terminal and Git start on view; Dev and actions start only on request (viewing never
    /// starts them).
    pub async fn snapshot(&self, target: AttachTarget, cols: i64, rows: i64) -> Result<Ordered<PaneRecord>, DhError> {
        let pane = self.hosted(target)?;
        match pane {
            PaneTarget::Terminal => {
                let runtime = self.ensure_terminal(cols, rows).await?;
                Ok(self.record_of(pane, &runtime))
            }
            PaneTarget::Git => {
                let runtime = self.ensure_git(cols, rows).await?;
                Ok(self.record_of(pane, &runtime))
            }
            // Every Dev process is sized alike: the Dev tab shows them side by side or stacked, in cells of one size.
            PaneTarget::Dev(_) => {
                for target in self.dev_targets() {
                    if let Some(runtime) = self.pane(target) {
                        PaneHost::resize_runtime(&runtime, cols, rows);
                    }
                }
                Ok(self.dev_record())
            }
            PaneTarget::Action => match self.pane(pane) {
                Some(runtime) => {
                    let runtime = PaneHost::resize_runtime(&runtime, cols, rows);
                    Ok(self.record_of(pane, &runtime))
                }
                None => Ok(self.record(pane, None)),
            },
        }
    }

    /// TS: PaneHost.attach: marks the pane attached, then its snapshot plus `{terminalModes, initialFrame}` while a
    /// runtime exists.
    ///
    /// Dev attaches one process (`process`, else the first): only that one is sized to the attaching terminal, and
    /// its record is its own.
    pub async fn attach(
        &self,
        target: AttachTarget,
        cols: i64,
        rows: i64,
        process: Option<&str>,
    ) -> Result<Ordered<AttachSnapshot>, DhError> {
        let mut pane = self.hosted(target)?;
        if pane.is_dev() {
            pane = match process.filter(|name| !name.is_empty()) {
                Some(name) => self.dev_target(Some(name))?,
                None => PaneTarget::Dev(0),
            };
            let mut attached = self.inner.attached.borrow_mut();
            attached.retain(|target| !target.is_dev());
            attached.insert(pane);
        } else {
            self.inner.attached.borrow_mut().insert(pane);
        }
        let record = if pane.is_dev() {
            match self.pane(pane) {
                Some(runtime) => {
                    let runtime = PaneHost::resize_runtime(&runtime, cols, rows);
                    self.record_of(pane, &runtime)
                }
                None => self.record(pane, None),
            }
        } else {
            self.snapshot(pane.attach_target(), cols, rows).await?
        }
        .into_inner();
        let extra = self.pane(pane).map(|runtime| {
            let mut runtime = runtime.borrow_mut();
            let modes = runtime.terminal_modes.clone();
            (modes, runtime.preview.get_ansi_frame())
        });
        let (terminal_modes, initial_frame) = match extra {
            Some((modes, frame)) => (Some(modes), Some(frame)),
            None => (None, None),
        };
        // `{...record, terminalModes, initialFrame}`: the record's keys first, in its order.
        Ok(Ordered::new(AttachSnapshot {
            session_id: record.session_id,
            content: record.content,
            live: record.live,
            cwd: record.cwd,
            command: record.command,
            name: record.name,
            exit_code: record.exit_code,
            exit_signal: record.exit_signal,
            status: None,
            agent_status: None,
            scroll_offset: None,
            max_scroll_offset: None,
            terminal_modes,
            initial_frame,
        }))
    }

    /// TS: PaneHost.detach
    pub fn detach(&self, target: AttachTarget) -> Result<(), DhError> {
        let pane = self.hosted(target)?;
        let mut attached = self.inner.attached.borrow_mut();
        if pane.is_dev() {
            attached.retain(|target| !target.is_dev());
        } else {
            attached.remove(&pane);
        }
        Ok(())
    }

    /// TS: PaneHost.input (`term.write(data)` on the pane, if it has a runtime).
    pub async fn input(&self, target: AttachTarget, data: &str) -> Result<(), DhError> {
        let pane = self.routed(target)?;
        let term = self.pane(pane).map(|runtime| Rc::clone(&runtime.borrow().term));
        if let Some(term) = term {
            // node-pty reports write failures on the socket, never to the caller.
            let _ = term.write(data.as_bytes()).await;
        }
        Ok(())
    }

    /// TS: PaneHost.resize (a no-op without a live runtime), then a broadcast.
    pub fn resize(&self, target: AttachTarget, cols: i64, rows: i64) -> Result<(), DhError> {
        let pane = self.routed(target)?;
        let Some(runtime) = self.live(pane) else {
            return Ok(());
        };
        PaneHost::resize_runtime(&runtime, cols, rows);
        self.schedule_broadcast(pane, &runtime);
        Ok(())
    }

    /// TS: PaneHost.resizeRuntime: resizes the PTY (unless exited) and the preview, refreshing its cached snapshot.
    pub fn resize_runtime(runtime: &SharedRuntime, cols: i64, rows: i64) -> SharedRuntime {
        {
            let mut current = runtime.borrow_mut();
            if !current.exited {
                let _ = current
                    .term
                    .resize(size(cols as f64, DEFAULT_COLS), size(rows as f64, DEFAULT_ROWS));
            }
            current.preview.resize(cols as f64, rows as f64);
            current.preview.get_snapshot(None);
        }
        Rc::clone(runtime)
    }

    /// TS: PaneHost.stop: forgets the pane and signals it at once (SIGHUP to the group: interactive shells ignore
    /// SIGTERM), snapshotting its descendants first; the task resolves once it exited, or after [`PANE_KILL_GRACE`]
    /// SIGKILLs the group and the surviving descendants (including jobs the shell moved to their own process group).
    /// The shell exiting first ends the wait only when nothing survived it. Dropping the task does not cancel the
    /// escalation (TS `void this.stop(...)`).
    pub fn stop(&self, target: PaneTarget) -> Task<()> {
        let runtime = self.inner.panes.borrow_mut().remove(&target);
        let Some(runtime) = runtime else {
            return spawn(async {});
        };
        let (term, exited) = {
            let mut current = runtime.borrow_mut();
            // Cleared but kept set, as `clearTimeout` leaves the field truthy: the forgotten pane never broadcasts.
            if let Some(timer) = current.broadcast_timer.as_mut() {
                timer.clear();
            }
            (Rc::clone(&current.term), current.exited)
        };
        // Taken before signalling: once the shell dies its children are reparented and can't be found from it.
        let descendants = if exited {
            ProcessSnapshot::new()
        } else {
            u32::try_from(term.pid()).map(snapshot_descendants).unwrap_or_default()
        };
        // Interactive shells (`$SHELL -ic cmd`) ignore SIGTERM; SIGHUP is what a closed terminal sends.
        signal_pty_process(&term, Signal::SIGHUP);
        // TS `runtime.preview.dispose()`: the preview goes with the runtime once its pump ends.
        if exited {
            return spawn(async {});
        }
        spawn(async move {
            let grace = tokio::time::sleep(PANE_KILL_GRACE);
            tokio::pin!(grace);
            tokio::select! {
                _ = &mut grace => {}
                _ = term.wait() => {
                    if survivors(&descendants).is_empty() {
                        return;
                    }
                    grace.await;
                }
            }
            // After the grace period, SIGKILL the group plus descendants that survived, including jobs the shell moved
            // to their own process group (which the group signal cannot reach).
            let exited = runtime.borrow().exited;
            if !exited {
                signal_pty_process(&term, Signal::SIGKILL);
            }
            kill_survivors(&descendants);
        })
    }

    /// TS: PaneHost.stopAll: [`PaneHost::stop`] for every hosted target, resolving when all did.
    pub fn stop_all(&self) -> Task<()> {
        let mut targets = self.inner.targets.clone();
        for target in self.dev_targets() {
            if !targets.contains(&target) {
                targets.push(target);
            }
        }
        let stops: Vec<Task<()>> = targets.iter().map(|target| self.stop(*target)).collect();
        spawn(async move {
            for stop in stops {
                stop.await;
            }
        })
    }

    /// TS: PaneHost.record: `{content, live, cwd, command?, name?, exitCode, exitSignal}` (`command` for Dev and
    /// action, `name` for action; `cwd` falls back to the workspace's).
    pub fn record(&self, target: PaneTarget, runtime: Option<&RuntimePty>) -> Ordered<PaneRecord> {
        let owner_cwd = (self.inner.owner)();
        let with_command = matches!(target, PaneTarget::Dev(_) | PaneTarget::Action);
        Ordered::new(PaneRecord {
            content: runtime
                .map(|runtime| runtime.preview.get_cached_snapshot())
                .unwrap_or_default(),
            live: runtime.is_some_and(|runtime| !runtime.exited),
            cwd: runtime.map(|runtime| runtime.cwd.clone()).or(owner_cwd),
            command: if with_command {
                runtime.and_then(|runtime| runtime.command.clone())
            } else {
                None
            },
            name: if matches!(target, PaneTarget::Dev(_) | PaneTarget::Action) {
                runtime.and_then(|runtime| runtime.name.clone())
            } else {
                None
            },
            exit_code: runtime.and_then(|runtime| runtime.exit_code),
            exit_signal: runtime.and_then(|runtime| runtime.exit_signal),
            processes: None,
            session_id: None,
            workspace: None,
        })
    }

    /// TS: PaneHost.scheduleBroadcast: at most one pending broadcast per runtime; after
    /// [`crate::session::PREVIEW_BROADCAST_DELAY`] refreshes the snapshot and posts [`pane_updated`].
    pub fn schedule_broadcast(&self, target: PaneTarget, runtime: &SharedRuntime) {
        if runtime.borrow().broadcast_timer.is_some() {
            return;
        }
        let host = self.clone();
        let captured = Rc::clone(runtime);
        let timer = Timer::after(PREVIEW_BROADCAST_DELAY, move || {
            let record = {
                let mut current = captured.borrow_mut();
                if let Some(timer) = current.broadcast_timer.take() {
                    timer.detach();
                }
                current.preview.get_snapshot(None);
                (!target.is_dev()).then(|| host.record(target, Some(&current)))
            };
            // Dev's record covers every process, so it is built once this runtime's borrow is released.
            let record = record.unwrap_or_else(|| host.dev_record());
            host.inner.channel.post(&pane_updated(target, record));
        });
        runtime.borrow_mut().broadcast_timer = Some(timer);
    }
}

/// TS: sessionWorker.ts handlePaneCommand: runs a pane command on `host`, answering `snapshot`/`attach` with `ok`.
pub async fn handle_pane_command(host: &PaneHost, command: PaneCommand) -> Result<(), DhError> {
    match command {
        PaneCommand::Snapshot(snapshot) => {
            let record = host.snapshot(snapshot.target, snapshot.cols, snapshot.rows).await?;
            host.inner.channel.ok(&snapshot.request_id, &record);
        }
        PaneCommand::Attach(attach) => {
            let record = host
                .attach(attach.target, attach.cols, attach.rows, attach.process.as_deref())
                .await?;
            host.inner.channel.ok(&attach.request_id, &record);
        }
        PaneCommand::Detach(detach) => host.detach(detach.target)?,
        PaneCommand::Input(input) => host.input(input.target, &input.data).await?,
        PaneCommand::Resize(resize) => host.resize(resize.target, resize.cols, resize.rows)?,
    }
    Ok(())
}

/// TS: sessionWorker.ts WorkspaceWorker: hosts the panes shared by every session in one workspace.
///
/// A cheap handle (clones share one worker).
#[derive(Clone)]
pub struct WorkspaceWorker {
    inner: Rc<WorkspaceWorkerInner>,
}

struct WorkspaceWorkerInner {
    channel: WorkerChannel,
    /// Shared with the [`PaneHost`]'s owner closure.
    cwd: Rc<RefCell<Option<String>>>,
    stopping: Cell<bool>,
    panes: PaneHost,
    /// Set when the worker should exit (TS `process.exit(0)` from a timer): [`WorkspaceWorker::start`] returns.
    exit: watch::Sender<bool>,
}

impl WorkspaceWorker {
    /// A worker hosting [`WORKSPACE_PANES`], posting to `channel`, not started.
    pub fn new(channel: WorkerChannel) -> Self {
        let cwd: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let owner_cwd = Rc::clone(&cwd);
        let panes = PaneHost::new(WORKSPACE_PANES, move || owner_cwd.borrow().clone(), channel.clone());
        let (exit, _) = watch::channel(false);
        WorkspaceWorker {
            inner: Rc::new(WorkspaceWorkerInner {
                channel,
                cwd,
                stopping: Cell::new(false),
                panes,
                exit,
            }),
        }
    }

    /// The worker's pane host.
    pub fn panes(&self) -> &PaneHost {
        &self.inner.panes
    }

    /// TS: WorkspaceWorker.start: switches the wire-log role to `workspace-worker`, serves commands from `reader`
    /// (each in its own local task) until the daemon disconnects, then stops every pane and returns (the worker then
    /// exits with status 0).
    pub async fn start(&self, reader: ChannelReader) {
        self.inner.channel.set_role(WireRole::WorkspaceWorker);
        let mut exit = self.inner.exit.subscribe();
        let mut reader = reader;
        let serve = async {
            while let Some(command) = reader.next::<WorkspaceWorkerCommand>().await {
                let worker = self.clone();
                drop(spawn(async move { worker.handle(command).await }));
            }
            self.inner.panes.stop_all().await;
        };
        tokio::select! {
            _ = serve => {}
            _ = exit.wait_for(|exit| *exit) => {}
        }
    }

    /// TS: WorkspaceWorker.handle: runs one command, answering `fail(requestId, error)` when it has a request ID and
    /// throws.
    pub async fn handle(&self, command: WorkspaceWorkerCommand) {
        let request_id = request_id(&command).map(str::to_owned);
        if let Err(error) = self.dispatch(command).await
            && let Some(request_id) = request_id
        {
            self.inner.channel.fail(&request_id, &error);
        }
    }

    /// The `try` body of TS WorkspaceWorker.handle. Nothing may start in a worker that is shutting down (its worktree
    /// may be about to be removed): anything but `shutdown` and `detach` then fails with `workspace worker is
    /// stopping`.
    pub async fn dispatch(&self, command: WorkspaceWorkerCommand) -> Result<(), DhError> {
        let exempt = matches!(
            command,
            WorkspaceWorkerCommand::Shutdown(_) | WorkspaceWorkerCommand::Detach(_)
        );
        if self.inner.stopping.get() && !exempt {
            return Err(DhError::msg("workspace worker is stopping"));
        }
        let channel = &self.inner.channel;
        let panes = &self.inner.panes;
        let command = match PaneCommand::from_workspace(command) {
            Ok(pane_command) => return handle_pane_command(panes, pane_command).await,
            Err(command) => command,
        };
        match command {
            WorkspaceWorkerCommand::Start(start) => {
                *self.inner.cwd.borrow_mut() = Some(start.cwd);
                channel.ok(&start.request_id, &OkResult { ok: true });
            }
            WorkspaceWorkerCommand::StartDev(start) => {
                let StartDev {
                    command,
                    processes,
                    process,
                    restart,
                    cols,
                    rows,
                    request_id,
                } = start;
                let record = match processes {
                    Some(processes) => {
                        panes
                            .start_dev_processes(cols, rows, processes, process.as_deref(), restart == Some(true))
                            .await?
                    }
                    None => panes.start_dev(cols, rows, Some(&command)).await?,
                };
                channel.ok(&request_id, &record);
            }
            WorkspaceWorkerCommand::StopDev(request) => {
                panes.stop_dev(request.process.as_deref())?;
                channel.post(&pane_updated(PaneTarget::Dev(0), panes.dev_record()));
                channel.ok(&request.request_id, &IdleResult { idle: panes.is_idle() });
            }
            WorkspaceWorkerCommand::StartAction(start) => {
                let record = panes
                    .start_action(start.cols, start.rows, &start.command, &start.name)
                    .await?;
                channel.ok(&start.request_id, &record);
            }
            WorkspaceWorkerCommand::StopAction(request) => {
                self.stop_pane(PaneTarget::Action);
                channel.ok(&request.request_id, &IdleResult { idle: panes.is_idle() });
            }
            WorkspaceWorkerCommand::Idle(request) => {
                channel.ok(&request.request_id, &IdleResult { idle: panes.is_idle() });
            }
            WorkspaceWorkerCommand::Shutdown(request) => {
                self.inner.stopping.set(true);
                panes.stop_all().await;
                channel.ok(&request.request_id, &OkResult { ok: true });
                let exit = self.inner.exit.clone();
                Timer::after(EXIT_DELAY, move || {
                    exit.send_replace(true);
                })
                .detach();
            }
            // Pane commands were handled above.
            WorkspaceWorkerCommand::Snapshot(_)
            | WorkspaceWorkerCommand::Attach(_)
            | WorkspaceWorkerCommand::Detach(_)
            | WorkspaceWorkerCommand::Input(_)
            | WorkspaceWorkerCommand::Resize(_) => {}
        }
        Ok(())
    }

    /// TS: WorkspaceWorker.stopPane: stops the pane without waiting for it to exit and posts its now-empty record
    /// (the response follows once the pane is signalled, not once it exited).
    pub fn stop_pane(&self, target: PaneTarget) {
        drop(self.inner.panes.stop(target));
        let record = self.inner.panes.record(target, None);
        self.inner.channel.post(&pane_updated(target, record));
    }
}

/// TS: sessionWorker.ts runWorkspaceWorker (`deckhand --workspace-worker`): opens the channel on fd 3 (posting
/// `hello`) and serves the daemon until it disconnects or a `shutdown`. Must run inside the process's `LocalSet`.
/// Returns when the worker should exit with status 0; fails only when the channel cannot be opened. The TS
/// `process.title = 'deckhand-workspace-worker'` is not ported.
pub async fn run_workspace_worker() -> Result<(), DhError> {
    // Opened with the workspace role so `hello` lands in the same wire log as everything after it.
    let (channel, reader) = WorkerChannel::open(WireRole::WorkspaceWorker)?;
    let worker = WorkspaceWorker::new(channel.clone());
    worker.start(reader).await;
    channel.flush().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::sync::Once;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
    use tokio::net::UnixStream;
    use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
    use tokio::time::Instant;

    /// Panes run `$SHELL -i`: point it at `/bin/sh` with a throwaway HOME (no rc files, no history), and keep config
    /// reads (`start-dev`) inside a temp DECKHAND_HOME. Set once per test process; every test sets the same values.
    fn isolate_env() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let home = tempfile::tempdir().expect("tempdir").keep();
            // SAFETY: set once, before any pane spawns in this process; all tests of this module call this first.
            unsafe {
                std::env::set_var("SHELL", "/bin/sh");
                std::env::set_var("HOME", &home);
                std::env::set_var("HISTFILE", "/dev/null");
                std::env::set_var("ENV", "/dev/null");
                std::env::set_var("DECKHAND_SESSION_ID", "root-session");
                if std::env::var_os(deckhand_sys::paths::HOME_ENV).is_none_or(|value| value.is_empty()) {
                    std::env::set_var(deckhand_sys::paths::HOME_ENV, home.join("deckhand"));
                }
            }
        });
    }

    fn run_local<F: std::future::Future>(future: F) -> F::Output {
        isolate_env();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        tokio::task::LocalSet::new().block_on(&runtime, future)
    }

    fn pid_alive(pid: i32) -> bool {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
    }

    async fn wait_dead(pid: i32) -> bool {
        for _ in 0..100 {
            if !pid_alive(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    /// The daemon's end of a workspace worker under test.
    struct Daemon {
        lines: Lines<BufReader<OwnedReadHalf>>,
        writer: Option<OwnedWriteHalf>,
        worker: WorkspaceWorker,
        served: Task<()>,
        cwd: String,
        _dir: tempfile::TempDir,
    }

    impl Daemon {
        async fn start() -> Daemon {
            let (worker_end, daemon_end) = UnixStream::pair().expect("pair");
            let (channel, reader) = WorkerChannel::from_stream(worker_end, WireRole::Worker);
            let worker = WorkspaceWorker::new(channel);
            let serving = worker.clone();
            let served = spawn(async move { serving.start(reader).await });
            let (read_half, write_half) = daemon_end.into_split();
            let dir = tempfile::tempdir().expect("tempdir");
            let cwd = dir
                .path()
                .canonicalize()
                .expect("canonical")
                .to_string_lossy()
                .into_owned();
            let mut daemon = Daemon {
                lines: BufReader::new(read_half).lines(),
                writer: Some(write_half),
                worker,
                served,
                cwd,
                _dir: dir,
            };
            assert_eq!(daemon.next_text().await, r#"{"type":"hello","version":48}"#);
            daemon
        }

        async fn started() -> Daemon {
            let mut daemon = Daemon::start().await;
            let cwd = daemon.cwd.clone();
            daemon
                .send(json!({"type": "start", "cwd": cwd, "requestId": "start"}))
                .await;
            assert_eq!(
                daemon.next_text().await,
                r#"{"type":"response","requestId":"start","ok":true,"data":{"ok":true}}"#
            );
            daemon
        }

        async fn send(&mut self, command: Value) {
            let mut line = serde_json::to_string(&command).expect("json");
            line.push('\n');
            let writer = self.writer.as_mut().expect("connected");
            writer.write_all(line.as_bytes()).await.expect("send");
        }

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

        async fn request(&mut self, command: Value) -> Value {
            let request_id = command["requestId"].as_str().expect("requestId").to_owned();
            self.send(command).await;
            self.response(&request_id).await
        }

        fn pid(&self, target: PaneTarget) -> i32 {
            let runtime = self.worker.panes().pane(target).expect("pane");
            runtime.borrow().term.pid()
        }
    }

    fn keys(value: &Value) -> Vec<String> {
        value.as_object().expect("object").keys().cloned().collect()
    }

    #[test]
    fn pane_targets_map_attach_targets() {
        for pane in WORKSPACE_PANES {
            assert_eq!(PaneTarget::from_attach_target(pane.attach_target()), Some(*pane));
            assert_eq!(pane.as_str(), pane.attach_target().as_str());
        }
        assert_eq!(PaneTarget::from_attach_target(AttachTarget::Agent), None);
        assert_eq!(PaneTarget::Action.as_str(), "action");
        assert_eq!(PaneTarget::Terminal.as_str(), "terminal");
    }

    #[test]
    fn request_id_only_for_requests() {
        let command = |value: Value| serde_json::from_value::<WorkspaceWorkerCommand>(value).expect("command");
        let with_id = [
            json!({"type": "snapshot", "target": "git", "cols": 1, "rows": 1, "requestId": "a"}),
            json!({"type": "attach", "target": "git", "cols": 1, "rows": 1, "requestId": "a"}),
            json!({"type": "start", "cwd": "/x", "requestId": "a"}),
            json!({"type": "start-dev", "command": "", "cols": 1, "rows": 1, "requestId": "a"}),
            json!({"type": "stop-dev", "requestId": "a"}),
            json!({"type": "start-action", "name": "n", "command": "c", "cols": 1, "rows": 1, "requestId": "a"}),
            json!({"type": "stop-action", "requestId": "a"}),
            json!({"type": "idle", "requestId": "a"}),
            json!({"type": "shutdown", "requestId": "a"}),
        ];
        for value in with_id {
            assert_eq!(request_id(&command(value)), Some("a"));
        }
        let without = [
            json!({"type": "detach", "target": "dev"}),
            json!({"type": "input", "target": "dev", "data": "x"}),
            json!({"type": "resize", "target": "dev", "cols": 1, "rows": 1}),
        ];
        for value in without {
            assert_eq!(request_id(&command(value)), None);
        }
    }

    #[test]
    fn pane_commands_split_from_workspace_commands() {
        let command = |value: Value| serde_json::from_value::<WorkspaceWorkerCommand>(value).expect("command");
        let detach = command(json!({"type": "detach", "target": "dev"}));
        assert!(matches!(
            PaneCommand::from_workspace(detach),
            Ok(PaneCommand::Detach(PaneDetach {
                target: AttachTarget::Dev
            }))
        ));
        let snapshot = command(json!({"type": "snapshot", "target": "git", "cols": 3, "rows": 4, "requestId": "s"}));
        assert!(matches!(
            PaneCommand::from_workspace(snapshot),
            Ok(PaneCommand::Snapshot(Snapshot { cols: 3, rows: 4, .. }))
        ));
        let idle = command(json!({"type": "idle", "requestId": "i"}));
        assert_eq!(PaneCommand::from_workspace(idle.clone()), Err(idle));
    }

    #[test]
    fn pane_updated_names_the_pane() {
        let record = || {
            Ordered::new(PaneRecord {
                content: "x".to_owned(),
                live: false,
                cwd: Some("/w".to_owned()),
                ..PaneRecord::default()
            })
        };
        let text = |target| serde_json::to_string(&pane_updated(target, record())).expect("json");
        assert_eq!(
            text(PaneTarget::Terminal),
            r#"{"type":"terminal-updated","terminal":{"content":"x","live":false,"cwd":"/w"}}"#
        );
        assert_eq!(
            text(PaneTarget::Git),
            r#"{"type":"git-updated","git":{"content":"x","live":false,"cwd":"/w"}}"#
        );
        assert_eq!(
            text(PaneTarget::Dev(0)),
            r#"{"type":"dev-updated","dev":{"content":"x","live":false,"cwd":"/w"}}"#
        );
        assert_eq!(
            text(PaneTarget::Action),
            r#"{"type":"action-updated","action":{"content":"x","live":false,"cwd":"/w"}}"#
        );
    }

    #[test]
    fn pane_env_drops_root_session_credentials() {
        isolate_env();
        let env = pane_env();
        assert!(
            env.iter()
                .all(|(key, _)| !ROOT_SESSION_ENV.iter().any(|root| key == *root))
        );
        assert!(env.iter().any(|(key, value)| key == "SHELL" && value == "/bin/sh"));
    }

    #[test]
    fn host_checks_targets_and_cwd() {
        run_local(async {
            let (worker_end, _daemon_end) = UnixStream::pair().expect("pair");
            let (channel, _reader) = WorkerChannel::from_stream(worker_end, WireRole::WorkspaceWorker);
            let cwd: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
            let owner = Rc::clone(&cwd);
            let host = PaneHost::new(
                &[PaneTarget::Terminal, PaneTarget::Dev(0), PaneTarget::Dev(0)],
                move || owner.borrow().clone(),
                channel,
            );
            assert_eq!(host.inner.targets, [PaneTarget::Terminal, PaneTarget::Dev(0)]);
            assert!(host.is_idle());
            assert_eq!(host.hosted(AttachTarget::Dev), Ok(PaneTarget::Dev(0)));
            assert_eq!(
                host.hosted(AttachTarget::Agent).unwrap_err().message(),
                "agent pane is not hosted by this worker"
            );
            assert_eq!(
                host.hosted(AttachTarget::Git).unwrap_err().message(),
                "git pane is not hosted by this worker"
            );
            assert_eq!(host.cwd().unwrap_err().message(), "workspace is not started");
            *cwd.borrow_mut() = Some(String::new());
            assert_eq!(host.cwd().unwrap_err().message(), "workspace is not started");
            assert_eq!(
                serde_json::to_string(&host.record(PaneTarget::Dev(0), None)).expect("json"),
                r#"{"content":"","live":false,"cwd":""}"#
            );
            *cwd.borrow_mut() = None;
            assert_eq!(
                serde_json::to_string(&host.record(PaneTarget::Action, None)).expect("json"),
                r#"{"content":"","live":false}"#
            );
            assert_eq!(
                host.start_action(80, 24, "true", "x").await.unwrap_err().message(),
                "action pane is not hosted by this worker"
            );
            // Nothing spawns without a workspace.
            let error = host.ensure_terminal(80, 24).await.err().expect("no workspace");
            assert_eq!(error.message(), "workspace is not started");
            assert!(host.is_idle());
            // Stopping panes that never ran resolves at once.
            host.stop_all().await;
        });
    }

    #[test]
    fn terminal_starts_on_view_and_streams_while_attached() {
        run_local(async {
            let mut daemon = Daemon::start().await;
            assert_eq!(
                daemon.request(json!({"type": "idle", "requestId": "i"})).await,
                json!({"type": "response", "requestId": "i", "ok": true, "data": {"idle": true}})
            );
            daemon
                .send(json!({"type": "snapshot", "target": "terminal", "cols": 80, "rows": 24, "requestId": "early"}))
                .await;
            assert_eq!(
                daemon.next_text().await,
                r#"{"type":"response","requestId":"early","ok":false,"error":"workspace is not started"}"#
            );
            daemon
                .send(json!({"type": "attach", "target": "agent", "cols": 80, "rows": 24, "requestId": "agent"}))
                .await;
            assert_eq!(
                daemon.next_text().await,
                r#"{"type":"response","requestId":"agent","ok":false,"error":"agent pane is not hosted by this worker"}"#
            );
            // Commands without a request ID never answer, even when they fail.
            daemon.send(json!({"type": "detach", "target": "agent"})).await;
            let cwd = daemon.cwd.clone();
            daemon
                .send(json!({"type": "start", "cwd": cwd, "requestId": "start"}))
                .await;
            assert_eq!(
                daemon.next_text().await,
                r#"{"type":"response","requestId":"start","ok":true,"data":{"ok":true}}"#
            );

            let attach = daemon
                .request(json!({"type": "attach", "target": "terminal", "cols": 80, "rows": 24, "requestId": "att"}))
                .await;
            let data = &attach["data"];
            assert_eq!(attach["ok"], true);
            assert_eq!(data["live"], true);
            assert_eq!(data["cwd"], json!(daemon.cwd));
            assert_eq!(data["terminalModes"], json!({"bracketedPaste": false}));
            assert!(
                data["initialFrame"]
                    .as_str()
                    .expect("frame")
                    .starts_with("\x1b[2J\x1b[H")
            );
            assert_eq!(keys(data), ["content", "live", "cwd", "terminalModes", "initialFrame"]);
            assert_eq!(
                daemon.request(json!({"type": "idle", "requestId": "i2"})).await["data"],
                json!({"idle": false})
            );

            daemon
                .send(json!({"type": "input", "target": "terminal", "data": "echo pane-$((40+2)) $DECKHAND_SESSION_ID.\r"}))
                .await;
            let mut seen = Vec::new();
            let mut output = String::new();
            while !output.contains("pane-42 .") {
                let message = daemon.until(|m| m["type"] == "output", &mut seen).await;
                assert_eq!(message["target"], "terminal");
                output.push_str(message["data"].as_str().expect("data"));
            }
            let update = loop {
                let update = daemon.until(|m| m["type"] == "terminal-updated", &mut seen).await;
                if update["terminal"]["content"]
                    .as_str()
                    .expect("content")
                    .contains("pane-42 .")
                {
                    break update;
                }
            };
            assert_eq!(keys(&update["terminal"]), ["content", "live", "cwd"]);
            assert_eq!(update["terminal"]["live"], true);

            // The same shell answers the next view.
            let pid = daemon.pid(PaneTarget::Terminal);
            daemon.send(json!({"type": "detach", "target": "terminal"})).await;
            daemon
                .send(json!({"type": "resize", "target": "terminal", "cols": 60, "rows": 10}))
                .await;
            let snapshot = daemon
                .request(json!({"type": "snapshot", "target": "terminal", "cols": 60, "rows": 10, "scrollOffset": 3, "requestId": "snap"}))
                .await;
            assert!(
                snapshot["data"]["content"]
                    .as_str()
                    .expect("content")
                    .contains("pane-42 .")
            );
            let runtime = daemon.worker.panes().pane(PaneTarget::Terminal).expect("terminal");
            let term = Rc::clone(&runtime.borrow().term);
            assert_eq!((term.pid(), term.cols(), term.rows()), (pid, 60, 10));

            // Detached: no more output.
            daemon
                .send(json!({"type": "input", "target": "terminal", "data": "echo quiet\r"}))
                .await;
            let mut after = Vec::new();
            loop {
                let update = daemon.until(|m| m["type"] == "terminal-updated", &mut after).await;
                if update["terminal"]["content"]
                    .as_str()
                    .expect("content")
                    .contains("\nquiet")
                {
                    break;
                }
            }
            assert!(after.iter().all(|m| m["type"] != "output"), "detached: {after:?}");

            // The disconnect stops every pane and ends the worker.
            let Daemon {
                lines, writer, served, ..
            } = daemon;
            drop(writer);
            drop(lines);
            tokio::time::timeout(Duration::from_secs(5), served)
                .await
                .expect("worker returns after the disconnect");
            assert!(wait_dead(pid).await, "shell {pid} survived the disconnect");
        });
    }

    #[test]
    fn actions_run_one_at_a_time() {
        run_local(async {
            let mut daemon = Daemon::started().await;
            let first = daemon
                .request(json!({"type": "start-action", "name": "build", "command": "printf act; sleep 0.4; (exit 3)", "cols": 80, "rows": 24, "requestId": "a1"}))
                .await;
            assert_eq!(first["ok"], true, "{first}");
            assert_eq!(keys(&first["data"]), ["content", "live", "cwd", "command", "name"]);
            assert_eq!(first["data"]["live"], true);
            assert_eq!(first["data"]["command"], "printf act; sleep 0.4; (exit 3)");
            assert_eq!(first["data"]["name"], "build");

            assert_eq!(
                daemon
                    .request(json!({"type": "start-action", "name": "other", "command": "true", "cols": 80, "rows": 24, "requestId": "a2"}))
                    .await,
                json!({"type": "response", "requestId": "a2", "ok": false, "error": "The build action is still running; stop it first (r, then x)"})
            );

            let mut seen = Vec::new();
            let finished = loop {
                let update = daemon.until(|m| m["type"] == "action-updated", &mut seen).await;
                if update["action"]["live"] == false {
                    break update;
                }
            };
            assert_eq!(
                serde_json::to_string(&finished["action"]).expect("json"),
                serde_json::to_string(&json!({
                    "content": "act",
                    "live": false,
                    "cwd": daemon.cwd,
                    "command": "printf act; sleep 0.4; (exit 3)",
                    "name": "build",
                    "exitCode": 3,
                    "exitSignal": 0,
                }))
                .expect("json")
            );
            // An exited action keeps its output until the next one replaces it.
            let view = daemon
                .request(json!({"type": "snapshot", "target": "action", "cols": 80, "rows": 24, "requestId": "v"}))
                .await;
            assert_eq!(view["data"]["content"], "act");
            assert_eq!(view["data"]["exitCode"], 3);
            assert_eq!(
                daemon.request(json!({"type": "idle", "requestId": "i"})).await["data"],
                json!({"idle": false})
            );

            let second = daemon
                .request(json!({"type": "start-action", "name": "", "command": "sleep 30", "cols": 80, "rows": 24, "requestId": "a3"}))
                .await;
            assert_eq!(second["data"]["live"], true);
            assert_eq!(second["data"]["content"], "");
            // `existing?.name ?? 'previous'` keeps an empty name.
            assert_eq!(
                daemon
                    .request(json!({"type": "start-action", "name": "x", "command": "true", "cols": 80, "rows": 24, "requestId": "a4"}))
                    .await["error"],
                "The  action is still running; stop it first (r, then x)"
            );

            let pid = daemon.pid(PaneTarget::Action);
            daemon.send(json!({"type": "stop-action", "requestId": "stop"})).await;
            let mut skipped = Vec::new();
            let update = daemon
                .until(
                    |m| m["type"] == "action-updated" || m["type"] == "response",
                    &mut skipped,
                )
                .await;
            assert_eq!(
                serde_json::to_string(&update).expect("json"),
                format!(
                    r#"{{"type":"action-updated","action":{{"content":"","live":false,"cwd":"{}"}}}}"#,
                    daemon.cwd
                )
            );
            assert_eq!(
                daemon.next_text().await,
                r#"{"type":"response","requestId":"stop","ok":true,"data":{"idle":true}}"#
            );
            assert!(wait_dead(pid).await, "action {pid} survived its stop");
        });
    }

    #[test]
    fn dev_pane_reuses_its_command_and_refuses_another() {
        run_local(async {
            let mut daemon = Daemon::started().await;
            // Viewing never starts Dev.
            let view = daemon
                .request(json!({"type": "snapshot", "target": "dev", "cols": 80, "rows": 24, "requestId": "v"}))
                .await;
            assert_eq!(
                serde_json::to_string(&view["data"]).expect("json"),
                format!(r#"{{"content":"","live":false,"cwd":"{}"}}"#, daemon.cwd)
            );
            let started = daemon
                .request(json!({"type": "start-dev", "command": "  printf dev-up; sleep 30\n", "cols": 80, "rows": 24, "requestId": "d1"}))
                .await;
            assert_eq!(started["ok"], true, "{started}");
            assert_eq!(keys(&started["data"]), ["content", "live", "cwd", "command"]);
            assert_eq!(started["data"]["command"], "printf dev-up; sleep 30");
            let pid = daemon.pid(PaneTarget::Dev(0));

            let same = daemon
                .request(json!({"type": "start-dev", "command": "printf dev-up; sleep 30", "cols": 70, "rows": 20, "requestId": "d2"}))
                .await;
            assert_eq!(same["ok"], true);
            let any = daemon
                .request(json!({"type": "start-dev", "command": " ", "cols": 70, "rows": 20, "requestId": "d3"}))
                .await;
            assert_eq!(any["ok"], true);
            assert_eq!(
                daemon
                    .request(json!({"type": "start-dev", "command": "npm run dev", "cols": 70, "rows": 20, "requestId": "d4"}))
                    .await,
                json!({"type": "response", "requestId": "d4", "ok": false, "error": "Stop the current Dev command before starting another"})
            );
            let runtime = daemon.worker.panes().pane(PaneTarget::Dev(0)).expect("dev");
            let term = Rc::clone(&runtime.borrow().term);
            assert_eq!((term.pid(), term.cols(), term.rows()), (pid, 70, 20));

            let mut seen = Vec::new();
            loop {
                let update = daemon.until(|m| m["type"] == "dev-updated", &mut seen).await;
                if update["dev"]["content"] == "dev-up" {
                    break;
                }
            }

            daemon.send(json!({"type": "stop-dev", "requestId": "stop"})).await;
            let mut skipped = Vec::new();
            let update = daemon
                .until(|m| m["type"] == "dev-updated" || m["type"] == "response", &mut skipped)
                .await;
            assert_eq!(
                serde_json::to_string(&update).expect("json"),
                format!(
                    r#"{{"type":"dev-updated","dev":{{"content":"","live":false,"cwd":"{}"}}}}"#,
                    daemon.cwd
                )
            );
            assert_eq!(
                daemon.next_text().await,
                r#"{"type":"response","requestId":"stop","ok":true,"data":{"idle":true}}"#
            );
            assert!(wait_dead(pid).await, "dev {pid} survived its stop");
        });
    }

    #[test]
    fn named_dev_processes_run_side_by_side_and_stop_one_at_a_time() {
        run_local(async {
            let mut daemon = Daemon::started().await;
            let processes = json!([
                {"name": "api", "command": "printf api-up; sleep 30"},
                {"name": "web", "command": "printf web-up; sleep 30"},
            ]);
            let start = |extra: Value, id: &str| {
                let mut command = json!({"type": "start-dev", "command": "", "processes": processes.clone(), "cols": 40, "rows": 10, "requestId": id});
                if let (Some(command), Some(extra)) = (command.as_object_mut(), extra.as_object()) {
                    command.extend(extra.clone());
                }
                command
            };
            let started = daemon.request(start(json!({}), "d1")).await;
            assert_eq!(started["ok"], true, "{started}");
            assert_eq!(keys(&started["data"]), ["content", "live", "cwd", "processes"]);
            assert_eq!(started["data"]["live"], true);
            let names: Vec<&str> = started["data"]["processes"]
                .as_array()
                .expect("processes")
                .iter()
                .map(|process| process["name"].as_str().expect("name"))
                .collect();
            assert_eq!(names, ["api", "web"]);
            let (api, web) = (daemon.pid(PaneTarget::Dev(0)), daemon.pid(PaneTarget::Dev(1)));
            assert_ne!(api, web);

            // Starting them again keeps them; a single command is refused while they run.
            assert_eq!(daemon.request(start(json!({}), "d2")).await["ok"], true);
            assert_eq!(daemon.pid(PaneTarget::Dev(1)), web);
            assert_eq!(
                daemon
                    .request(json!({"type": "start-dev", "command": "npm run dev", "cols": 40, "rows": 10, "requestId": "d3"}))
                    .await["error"],
                "Stop the current Dev command before starting another"
            );

            // One record carries both outputs.
            let mut seen = Vec::new();
            daemon
                .until(
                    |m| {
                        m["type"] == "dev-updated"
                            && m["dev"]["processes"][0]["content"] == "api-up"
                            && m["dev"]["processes"][1]["content"] == "web-up"
                    },
                    &mut seen,
                )
                .await;

            // Attaching web sizes only web; its input and resizes go to web.
            let attached = daemon
                .request(json!({"type": "attach", "target": "dev", "process": "web", "cols": 100, "rows": 30, "requestId": "a"}))
                .await;
            assert_eq!(attached["data"]["name"], "web", "{attached}");
            let size = |daemon: &Daemon, target: PaneTarget| {
                let runtime = daemon.worker.panes().pane(target).expect("pane");
                let term = Rc::clone(&runtime.borrow().term);
                (term.cols(), term.rows())
            };
            assert_eq!(size(&daemon, PaneTarget::Dev(0)), (40, 10));
            assert_eq!(size(&daemon, PaneTarget::Dev(1)), (100, 30));
            daemon
                .send(json!({"type": "resize", "target": "dev", "cols": 90, "rows": 25}))
                .await;
            daemon.send(json!({"type": "detach", "target": "dev"})).await;
            let snapshot = daemon
                .request(json!({"type": "snapshot", "target": "dev", "cols": 40, "rows": 10, "requestId": "v"}))
                .await;
            assert_eq!(snapshot["ok"], true);
            assert_eq!(size(&daemon, PaneTarget::Dev(0)), (40, 10));
            assert_eq!(size(&daemon, PaneTarget::Dev(1)), (40, 10));

            // Restarting web replaces only web, after it exited.
            let restarted = daemon
                .request(start(json!({"process": "web", "restart": true}), "r"))
                .await;
            assert_eq!(restarted["ok"], true, "{restarted}");
            assert!(wait_dead(web).await, "web {web} survived its restart");
            let web = daemon.pid(PaneTarget::Dev(1));
            assert_eq!(daemon.pid(PaneTarget::Dev(0)), api);
            assert_eq!(
                daemon.request(start(json!({"process": "db"}), "u")).await["error"],
                "Unknown Dev process: db"
            );

            // Stopping api leaves web running, and the worker in use.
            let stopped = daemon
                .request(json!({"type": "stop-dev", "process": "api", "requestId": "s1"}))
                .await;
            assert_eq!(stopped["data"]["idle"], false, "{stopped}");
            assert!(wait_dead(api).await, "api {api} survived its stop");
            let record = daemon.worker.panes().dev_record();
            let processes = record.processes.as_ref().expect("processes");
            assert_eq!(
                processes.iter().map(|process| process.live).collect::<Vec<_>>(),
                [false, true]
            );
            assert!(record.live);

            // Stopping Dev stops the rest.
            let stopped = daemon.request(json!({"type": "stop-dev", "requestId": "s2"})).await;
            assert_eq!(stopped["data"]["idle"], true, "{stopped}");
            assert!(wait_dead(web).await, "web {web} survived the stop");
            assert_eq!(daemon.worker.panes().dev_record().processes, None);
        });
    }

    #[test]
    fn concurrent_dev_starts_share_one_pane() {
        run_local(async {
            let mut daemon = Daemon::started().await;
            for (id, command) in [("d1", "sleep 30"), ("d2", "sleep 31"), ("d3", "")] {
                daemon
                    .send(json!({"type": "start-dev", "command": command, "cols": 80, "rows": 24, "requestId": id}))
                    .await;
            }
            assert_eq!(daemon.response("d1").await["data"]["command"], "sleep 30");
            assert_eq!(
                daemon.response("d2").await["error"],
                "Stop the current Dev command before starting another"
            );
            assert_eq!(daemon.response("d3").await["data"]["command"], "sleep 30");
            let pid = daemon.pid(PaneTarget::Dev(0));
            daemon.request(json!({"type": "shutdown", "requestId": "bye"})).await;
            assert!(wait_dead(pid).await, "dev {pid} survived the shutdown");
        });
    }

    #[test]
    fn shutdown_refuses_new_work_and_exits() {
        run_local(async {
            let mut daemon = Daemon::started().await;
            daemon
                .request(json!({"type": "snapshot", "target": "terminal", "cols": 80, "rows": 24, "requestId": "t"}))
                .await;
            let pid = daemon.pid(PaneTarget::Terminal);
            daemon.send(json!({"type": "shutdown", "requestId": "bye"})).await;
            daemon.send(json!({"type": "idle", "requestId": "late"})).await;
            daemon.send(json!({"type": "detach", "target": "terminal"})).await;
            daemon
                .send(json!({"type": "start-dev", "command": "x", "cols": 80, "rows": 24, "requestId": "dev"}))
                .await;
            assert_eq!(daemon.response("late").await["error"], "workspace worker is stopping");
            assert_eq!(daemon.response("dev").await["error"], "workspace worker is stopping");
            assert_eq!(
                daemon.response("bye").await,
                json!({"type": "response", "requestId": "bye", "ok": true, "data": {"ok": true}})
            );
            assert!(wait_dead(pid).await, "shell {pid} survived the shutdown");
            tokio::time::timeout(Duration::from_secs(2), daemon.served)
                .await
                .expect("worker returns after shutdown");
            assert!(daemon.worker.panes().is_idle());
        });
    }

    #[test]
    fn stop_escalates_to_sigkill_after_the_grace_period() {
        run_local(async {
            let (worker_end, _daemon_end) = UnixStream::pair().expect("pair");
            let (channel, _reader) = WorkerChannel::from_stream(worker_end, WireRole::WorkspaceWorker);
            let dir = tempfile::tempdir().expect("tempdir");
            let cwd = dir.path().to_string_lossy().into_owned();
            let owner = cwd.clone();
            let host = PaneHost::new(WORKSPACE_PANES, move || Some(owner.clone()), channel);
            let args = |script: &str| ["-c".to_owned(), script.to_owned()];

            // A pane that dies of SIGHUP ends the stop at once.
            let quick = host
                .spawn_pane(
                    PaneTarget::Dev(0),
                    "/bin/sh",
                    &args("sleep 30"),
                    &cwd,
                    80,
                    24,
                    Some("sleep 30"),
                )
                .expect("spawn");
            tokio::time::sleep(Duration::from_millis(100)).await;
            let quick_pid = quick.borrow().term.pid();
            let started = Instant::now();
            host.stop(PaneTarget::Dev(0)).await;
            assert!(
                started.elapsed() < Duration::from_millis(1500),
                "{:?}",
                started.elapsed()
            );
            assert!(wait_dead(quick_pid).await);
            assert!(host.pane(PaneTarget::Dev(0)).is_none());

            // One that ignores SIGHUP (its child too) is killed after the grace period.
            let stubborn = host
                .spawn_pane(
                    PaneTarget::Action,
                    "/bin/sh",
                    &args("trap '' HUP; sleep 30 & wait"),
                    &cwd,
                    80,
                    24,
                    None,
                )
                .expect("spawn");
            tokio::time::sleep(Duration::from_millis(200)).await;
            let stubborn_pid = stubborn.borrow().term.pid();
            let started = Instant::now();
            host.stop_all().await;
            let waited = started.elapsed();
            assert!(waited >= PANE_KILL_GRACE - Duration::from_millis(50), "{waited:?}");
            assert!(waited < PANE_KILL_GRACE + Duration::from_millis(1500), "{waited:?}");
            assert!(wait_dead(stubborn_pid).await);
            assert!(host.is_idle());
            // The forgotten pane never reports again.
            let pending = stubborn
                .borrow()
                .broadcast_timer
                .as_ref()
                .is_some_and(Timer::is_pending);
            assert!(!pending);
        });
    }

    #[test]
    fn git_pane_fails_without_lazygit() {
        run_local(async {
            if lazy_git_command().await.is_ok() {
                // lazygit is installed here: its UI is not started by tests.
                return;
            }
            assert_eq!(lazy_git_command().await.unwrap_err().message(), LAZYGIT_MISSING);
            let mut daemon = Daemon::started().await;
            daemon
                .send(json!({"type": "snapshot", "target": "git", "cols": 80, "rows": 24, "requestId": "g1"}))
                .await;
            daemon
                .send(json!({"type": "attach", "target": "git", "cols": 80, "rows": 24, "requestId": "g2"}))
                .await;
            for id in ["g1", "g2"] {
                assert_eq!(
                    daemon.response(id).await,
                    json!({"type": "response", "requestId": id, "ok": false, "error": LAZYGIT_MISSING})
                );
            }
            assert_eq!(
                daemon.request(json!({"type": "idle", "requestId": "i"})).await["data"],
                json!({"idle": true})
            );
        });
    }
}
