//! Port of cli.ts (argv dispatch).
//!
//! cli.ts `main` is an if-chain over argv; [`parse_args`] keeps its order of precedence and [`run`] carries out the
//! result. The UI loop (`runUi` and the attach / `$EDITOR` hand-off) is `crate::ui_loop` (unit A1); the hook bridge
//! is `crate::hook`, the hooks printer `crate::hooks`, setup and doctor `crate::setup` / `crate::doctor`.

// Wave 0 skeleton: the binary does not reach these items until their unit wires them; drop this then.
#![allow(dead_code)]

use std::process::ExitCode;

use deckhand_core::error::DhError;
use deckhand_protocol::data::PingResult;

use crate::hooks::HooksTarget;

/// TS: cli.ts `process.title` in the dev channel.
pub const DEV_PROCESS_TITLE: &str = "deckhand-dev";
/// TS: cli.ts `process.title` otherwise.
pub const PROCESS_TITLE: &str = "deckhand";
/// TS: cli.ts the timeout of the dev channel's `ping` and `shutdown` requests (ms).
pub const DEV_CONTROL_TIMEOUT_MS: u64 = 1500;

/// What argv asks for (TS: the branches of cli.ts `main`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// `status` / `stop` in the dev channel (`DECKHAND_CHANNEL=dev`); elsewhere these words open the UI, as in TS.
    DevDaemon(DevDaemonAction),
    /// `hook`: relay an agent hook's stdin to the daemon (see `crate::hook`).
    Hook,
    /// `hooks [codex]`: print the hooks config (see `crate::hooks`).
    Hooks(HooksTarget),
    /// `--session-worker` anywhere in argv.
    SessionWorker,
    /// `--workspace-worker` anywhere in argv.
    WorkspaceWorker,
    /// `--daemon` anywhere in argv.
    Daemon,
    /// `setup [args...]` (args: `argv.slice(3)`).
    Setup(Vec<String>),
    /// `doctor [args...]` (TS: same as setup; the port adds the Codex hooks check, see `crate::doctor`).
    Doctor(Vec<String>),
    /// `--version` (not in TS): prints [`version_line`].
    Version,
    /// Anything else: the terminal UI for the current directory's repository.
    Ui,
}

/// The dev channel's daemon controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DevDaemonAction {
    /// `status`: says whether the isolated dev daemon runs, its protocol version and home.
    Status,
    /// `stop`: asks the isolated dev daemon to shut down.
    Stop,
}

/// TS: cli.ts `process.title = process.env.DECKHAND_CHANNEL === 'dev' ? 'deckhand-dev' : 'deckhand'`.
pub fn process_title(dev_channel: bool) -> &'static str {
    let _ = dev_channel;
    todo!("B1")
}

/// TS: cli.ts main's argv tests, in order: dev `status`/`stop` (first argument, dev channel only), `hook`, `hooks`
/// (second argument `codex` selects Codex), `--session-worker`, `--workspace-worker`, `--daemon` (anywhere), `setup`
/// / `doctor` (first argument; the rest are passed on), `--version` (first argument), else the UI. `args` excludes
/// the program name (`process.argv.slice(2)`).
pub fn parse_args(args: &[String], dev_channel: bool) -> Command {
    let _ = (args, dev_channel);
    todo!("B1")
}

/// TS: cli.ts main: sets the process title, then runs `command`. Errors are printed by `main` (message only) with
/// exit status 1. `Daemon` never returns.
pub async fn run(command: Command) -> Result<ExitCode, DhError> {
    let _ = command;
    todo!("B1")
}

/// TS: cli.ts main's dev `status`/`stop` branch: pings the daemon (1500 ms). No daemon: prints `No isolated dev
/// daemon running (<config dir>)` and succeeds. A daemon outside the isolated dev namespace is refused (see
/// [`check_dev_daemon`]). `stop` sends `shutdown` and prints `Stopping isolated dev daemon. Production daemon
/// untouched.`; `status` prints `Isolated dev daemon running (protocol v<version>): <home>`.
pub async fn run_dev_daemon(action: DevDaemonAction) -> Result<(), DhError> {
    let _ = action;
    todo!("B1")
}

/// TS: cli.ts the dev-namespace check on the `ping` answer: its home must be this config dir (by realpath,
/// `deckhand_sys::paths::is_same_config_dir`) and its channel `dev`, else `Refusing to control a daemon outside the
/// isolated dev namespace`.
pub fn check_dev_daemon(ping: &PingResult) -> Result<(), DhError> {
    let _ = ping;
    todo!("B1")
}

/// TS: cli.ts `--session-worker`: `runSessionWorker()` (deckhand-worker's session worker entry point).
pub async fn run_session_worker() -> Result<(), DhError> {
    todo!("B1")
}

/// TS: cli.ts `--workspace-worker`: `runWorkspaceWorker()` (deckhand-worker's workspace worker entry point).
pub async fn run_workspace_worker() -> Result<(), DhError> {
    todo!("B1")
}

/// TS: cli.ts `--daemon`: `new InkDaemon().start()`, then wait forever (deckhand-daemon's entry point). Returns only
/// when start fails.
pub async fn run_daemon() -> Result<(), DhError> {
    todo!("B1")
}

/// `deckhand --version` (not in TS): `deckhand <crate version>`.
pub fn version_line() -> String {
    todo!("B1")
}
