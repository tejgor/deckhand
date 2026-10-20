//! Port of cli.ts (argv dispatch).
//!
//! cli.ts `main` is an if-chain over argv; [`parse_args`] keeps its order of precedence and [`run`] carries out the
//! result. The UI loop (`runUi` and the attach / `$EDITOR` hand-off) is `crate::ui_loop` (unit A1); the hook bridge
//! is `crate::hook`, the hooks printer `crate::hooks`, setup and doctor `crate::setup` / `crate::doctor`.

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use deckhand_client::client::request;
use deckhand_core::error::DhError;
use deckhand_protocol::data::PingResult;
use deckhand_protocol::wire::{Request, RequestOnly};
use deckhand_sys::paths;
use serde_json::Value;

use crate::hooks::HooksTarget;
use crate::{doctor, hook, hooks, setup, ui_loop};

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
    /// `help`, `--help` or `-h` (not in TS, where they opened the UI): prints [`HELP_TEXT`].
    Help,
    /// Anything else: the terminal UI for the current directory's repository.
    Ui,
}

/// `deckhand help` (not in TS). Lists what a user runs by hand: not the internal `--daemon`, `--session-worker`,
/// `--workspace-worker` and `--notify`, nor the dev channel's `status`/`stop`.
pub const HELP_TEXT: &str = "\
Usage: deckhand [command]

Commands:
  (none)                       Open the UI in the current Git repository
  setup [--check] [--yes|-y]   Check which agents are installed and offer to install missing ones
  doctor [--check] [--yes|-y]  The same as setup, then check Codex's hook config
  hooks [codex]                Print the hooks config to save in an agent's settings (codex: for Codex)
  --version                    Print the version
  help, --help, -h             Show this help

--check only reports; --yes (-y) installs without asking.
deckhand hook is run by agents, not by hand.";

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
    if dev_channel { DEV_PROCESS_TITLE } else { PROCESS_TITLE }
}

/// TS: cli.ts main's argv tests, in order: dev `status`/`stop` (first argument, dev channel only), `hook`, `hooks`
/// (second argument `codex` selects Codex), `--session-worker`, `--workspace-worker`, `--daemon` (anywhere), `setup`
/// / `doctor` (first argument; the rest are passed on), `--version` (first argument), else the UI. `args` excludes
/// the program name (`process.argv.slice(2)`). The port adds `help` / `--help` / `-h` (first argument) beside
/// `--version`, so the worker and daemon flags still win over it.
pub fn parse_args(args: &[String], dev_channel: bool) -> Command {
    let first = args.first().map(String::as_str);
    match first {
        Some("status") if dev_channel => return Command::DevDaemon(DevDaemonAction::Status),
        Some("stop") if dev_channel => return Command::DevDaemon(DevDaemonAction::Stop),
        Some("hook") => return Command::Hook,
        Some("hooks") => return Command::Hooks(HooksTarget::from_arg(args.get(1).map(String::as_str))),
        _ => {}
    }
    let has = |flag: &str| args.iter().any(|arg| arg == flag);
    if has("--session-worker") {
        return Command::SessionWorker;
    }
    if has("--workspace-worker") {
        return Command::WorkspaceWorker;
    }
    if has("--daemon") {
        return Command::Daemon;
    }
    let rest = || args.get(1..).unwrap_or_default().to_vec();
    match first {
        Some("setup") => Command::Setup(rest()),
        Some("doctor") => Command::Doctor(rest()),
        Some("--version") => Command::Version,
        Some("help" | "--help" | "-h") => Command::Help,
        _ => Command::Ui,
    }
}

/// Sets the process name `ps`/`top` show where the platform allows it. Node's `process.title` rewrites argv; Linux's
/// `PR_SET_NAME` (the comm name) is the closest equivalent, and macOS has none.
fn set_process_title(title: &str) {
    #[cfg(target_os = "linux")]
    {
        if let Ok(name) = std::ffi::CString::new(title) {
            // SAFETY: PR_SET_NAME reads a NUL-terminated string (cut to 16 bytes) and keeps no pointer to it.
            unsafe {
                libc::prctl(libc::PR_SET_NAME, name.as_ptr() as libc::c_ulong, 0, 0, 0);
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = title;
}

/// `console.log(line)`: a closed stdout must not panic (as `println!` would).
pub(crate) fn print_line(line: &str) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{line}");
    let _ = stdout.flush();
}

/// TS: cli.ts main: sets the process title, then runs `command`. Errors are printed by `main` (message only) with
/// exit status 1. `Daemon` never returns.
pub async fn run(command: Command) -> Result<ExitCode, DhError> {
    set_process_title(process_title(paths::is_dev_channel()));
    match command {
        Command::DevDaemon(action) => run_dev_daemon(action).await?,
        Command::Hook => hook::run_hook().await,
        Command::Hooks(target) => hooks::run_hooks(target)?,
        Command::SessionWorker => run_session_worker().await?,
        Command::WorkspaceWorker => run_workspace_worker().await?,
        Command::Daemon => run_daemon().await?,
        Command::Setup(args) => setup::run_setup(&args).await?,
        Command::Doctor(args) => return Ok(doctor_exit_code(doctor::run_doctor(&args).await?)),
        Command::Version => print_line(&version_line()),
        Command::Help => print_line(HELP_TEXT),
        Command::Ui => ui_loop::run_ui_loop().await?,
    }
    Ok(ExitCode::SUCCESS)
}

/// The exit status of `deckhand doctor [--check]`. TS doctor is setup and exits 0 whatever it finds; the port's Codex
/// hooks check only adds its report, so problems found (e.g. stale Codex hooks) are not a failure either.
pub fn doctor_exit_code(status: doctor::DoctorStatus) -> ExitCode {
    match status {
        doctor::DoctorStatus::Healthy | doctor::DoctorStatus::Problems => ExitCode::SUCCESS,
    }
}

/// A fresh request id (TS `randomUUID()`).
pub(crate) fn request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// TS: cli.ts main's dev `status`/`stop` branch: pings the daemon (1500 ms). No daemon: prints `No isolated dev
/// daemon running (<config dir>)` and succeeds. A daemon outside the isolated dev namespace is refused (see
/// [`check_dev_daemon`]). `stop` sends `shutdown` and prints `Stopping isolated dev daemon. Production daemon
/// untouched.`; `status` prints `Isolated dev daemon running (protocol v<version>): <home>`.
pub async fn run_dev_daemon(action: DevDaemonAction) -> Result<(), DhError> {
    let ping = Request::Ping(RequestOnly {
        request_id: request_id(),
    });
    let daemon: PingResult = match request(&ping, Some(DEV_CONTROL_TIMEOUT_MS)).await {
        Ok(daemon) => daemon,
        Err(_) => {
            print_line(&format!(
                "No isolated dev daemon running ({})",
                paths::config_dir().display()
            ));
            return Ok(());
        }
    };
    check_dev_daemon(&daemon)?;
    match action {
        DevDaemonAction::Stop => {
            let shutdown = Request::Shutdown(RequestOnly {
                request_id: request_id(),
            });
            request::<Value>(&shutdown, Some(DEV_CONTROL_TIMEOUT_MS)).await?;
            print_line("Stopping isolated dev daemon. Production daemon untouched.");
        }
        DevDaemonAction::Status => print_line(&status_line(&daemon)),
    }
    Ok(())
}

/// `Isolated dev daemon running (protocol v${daemon.version}): ${daemon.home}` (missing fields print `undefined`).
fn status_line(daemon: &PingResult) -> String {
    let version = daemon
        .version
        .map_or_else(|| "undefined".to_owned(), |version| version.to_string());
    let home = daemon.home.as_deref().unwrap_or("undefined");
    format!("Isolated dev daemon running (protocol v{version}): {home}")
}

/// TS: cli.ts the dev-namespace check on the `ping` answer: its home must be this config dir (by realpath,
/// `deckhand_sys::paths::is_same_config_dir`) and its channel `dev`, else `Refusing to control a daemon outside the
/// isolated dev namespace`.
pub fn check_dev_daemon(ping: &PingResult) -> Result<(), DhError> {
    // A ping without a home cannot be shown to be ours (TS fails resolving `undefined`); refuse it as well.
    let same_home = ping
        .home
        .as_deref()
        .is_some_and(|home| paths::is_same_config_dir(Path::new(home)));
    if !same_home || ping.channel.as_deref() != Some("dev") {
        return Err(DhError::msg(
            "Refusing to control a daemon outside the isolated dev namespace",
        ));
    }
    Ok(())
}

/// TS: cli.ts `--session-worker`: `runSessionWorker()` (deckhand-worker's session worker entry point).
pub async fn run_session_worker() -> Result<(), DhError> {
    deckhand_worker::session::run_session_worker().await
}

/// TS: cli.ts `--workspace-worker`: `runWorkspaceWorker()` (deckhand-worker's workspace worker entry point).
pub async fn run_workspace_worker() -> Result<(), DhError> {
    deckhand_worker::workspace::run_workspace_worker().await
}

/// TS: cli.ts `--daemon`: `new InkDaemon().start()`, then wait forever (deckhand-daemon's entry point). Returns only
/// when start fails.
pub async fn run_daemon() -> Result<(), DhError> {
    deckhand_daemon::daemon::run_daemon(deckhand_daemon::daemon::DaemonOptions::default()).await
}

/// `deckhand --version` (not in TS): `deckhand <crate version>`.
pub fn version_line() -> String {
    format!("deckhand {}", env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn doctor_exits_zero_whatever_it_finds() {
        assert_eq!(doctor_exit_code(doctor::DoctorStatus::Healthy), ExitCode::SUCCESS);
        assert_eq!(doctor_exit_code(doctor::DoctorStatus::Problems), ExitCode::SUCCESS);
    }

    #[test]
    fn doctor_check_with_stale_codex_hooks_exits_zero() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().expect("tempdir");
        let dir = |name: &str| {
            let path = root.path().join(name);
            std::fs::create_dir(&path).expect("mkdir");
            path
        };
        let (home, deckhand_home, codex_home, bin) = (dir("home"), dir("deckhand"), dir("codex"), dir("bin"));
        // A scratch login shell (setup probes `$SHELL -ic 'command -v <tool>'`) with an empty PATH: no rc files run
        // and no agent is found (nor started).
        let shell = root.path().join("sh");
        std::fs::write(
            &shell,
            "#!/bin/sh\n[ \"$1\" = -ic ] || exit 64\nexec /bin/sh -c \"$2\"\n",
        )
        .expect("shell");
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let stale = "'/usr/local/bin/node' '/Users/me/deckhand/dist/cli.js' 'hook'";
        let hooks = serde_json::json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": stale}]}]}});
        std::fs::write(codex_home.join(doctor::CODEX_HOOKS_FILE), hooks.to_string()).expect("hooks.json");
        // SAFETY: nextest runs each test in its own process, so nothing else reads the environment concurrently.
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var(paths::HOME_ENV, &deckhand_home);
            std::env::set_var(doctor::CODEX_HOME_ENV, &codex_home);
            std::env::set_var("SHELL", &shell);
            std::env::set_var("PATH", &bin);
        }
        let status = crate::block_on_local(doctor::run_doctor(&args(&["--check"])))
            .expect("runtime")
            .expect("doctor");
        assert_eq!(status, doctor::DoctorStatus::Problems);
        let code = crate::block_on_local(run(Command::Doctor(args(&["--check"]))))
            .expect("runtime")
            .expect("doctor");
        assert_eq!(code, ExitCode::SUCCESS);
        // Doctor only reads the Codex home.
        assert_eq!(
            std::fs::read_to_string(codex_home.join(doctor::CODEX_HOOKS_FILE)).expect("hooks.json"),
            hooks.to_string()
        );
    }

    #[test]
    fn parse_args_keeps_the_ts_order_of_precedence() {
        assert_eq!(parse_args(&[], false), Command::Ui);
        assert_eq!(parse_args(&args(&["hook"]), false), Command::Hook);
        assert_eq!(parse_args(&args(&["hook", "--daemon"]), false), Command::Hook);
        assert_eq!(
            parse_args(&args(&["hooks"]), false),
            Command::Hooks(HooksTarget::Default)
        );
        assert_eq!(
            parse_args(&args(&["hooks", "codex"]), false),
            Command::Hooks(HooksTarget::Codex)
        );
        assert_eq!(
            parse_args(&args(&["hooks", "claude"]), false),
            Command::Hooks(HooksTarget::Default)
        );
        assert_eq!(
            parse_args(&args(&["x", "--session-worker", "--daemon"]), false),
            Command::SessionWorker
        );
        assert_eq!(
            parse_args(&args(&["--daemon", "--workspace-worker"]), false),
            Command::WorkspaceWorker
        );
        assert_eq!(parse_args(&args(&["ui", "--daemon"]), false), Command::Daemon);
        assert_eq!(parse_args(&args(&["setup", "--daemon"]), false), Command::Daemon);
        assert_eq!(
            parse_args(&args(&["setup", "--check", "-y"]), false),
            Command::Setup(args(&["--check", "-y"]))
        );
        assert_eq!(parse_args(&args(&["doctor"]), false), Command::Doctor(Vec::new()));
        assert_eq!(parse_args(&args(&["--version"]), false), Command::Version);
        assert_eq!(parse_args(&args(&["x", "--version"]), false), Command::Ui);
        assert_eq!(parse_args(&args(&["hook"]), true), Command::Hook);
    }

    #[test]
    fn help_is_the_first_argument_and_internal_flags_win_over_it() {
        for flag in ["help", "--help", "-h"] {
            assert_eq!(parse_args(&args(&[flag]), false), Command::Help);
            assert_eq!(parse_args(&args(&[flag]), true), Command::Help);
            assert_eq!(parse_args(&args(&[flag, "--version"]), false), Command::Help);
            assert_eq!(parse_args(&args(&["x", flag]), false), Command::Ui);
            assert_eq!(parse_args(&args(&[flag, "--daemon"]), false), Command::Daemon);
            assert_eq!(
                parse_args(&args(&[flag, "--session-worker"]), false),
                Command::SessionWorker
            );
            assert_eq!(
                parse_args(&args(&[flag, "--workspace-worker"]), false),
                Command::WorkspaceWorker
            );
            assert_eq!(
                parse_args(&args(&["setup", flag]), false),
                Command::Setup(args(&[flag]))
            );
            assert_eq!(parse_args(&args(&["hook", flag]), false), Command::Hook);
        }
        assert_eq!(parse_args(&args(&["--version", "--help"]), false), Command::Version);
        assert_eq!(parse_args(&args(&["--HELP"]), false), Command::Ui);
    }

    #[test]
    fn help_text_lists_the_user_commands_only() {
        for listed in [
            "setup [--check] [--yes|-y]",
            "doctor [--check] [--yes|-y]",
            "hooks [codex]",
            "--version",
        ] {
            assert!(HELP_TEXT.contains(listed), "{listed}");
        }
        assert!(HELP_TEXT.contains("deckhand hook is run by agents, not by hand."));
        for internal in [
            "--daemon",
            "--session-worker",
            "--workspace-worker",
            "--notify",
            "status",
            "stop",
        ] {
            assert!(!HELP_TEXT.contains(internal), "{internal}");
        }
        assert!(!HELP_TEXT.ends_with('\n'));
    }

    #[test]
    fn status_and_stop_are_dev_channel_only() {
        assert_eq!(parse_args(&args(&["status"]), false), Command::Ui);
        assert_eq!(parse_args(&args(&["stop"]), false), Command::Ui);
        assert_eq!(
            parse_args(&args(&["status"]), true),
            Command::DevDaemon(DevDaemonAction::Status)
        );
        assert_eq!(
            parse_args(&args(&["stop", "--daemon"]), true),
            Command::DevDaemon(DevDaemonAction::Stop)
        );
        assert_eq!(parse_args(&args(&["x", "status"]), true), Command::Ui);
    }

    #[test]
    fn process_title_follows_the_channel() {
        assert_eq!(process_title(true), "deckhand-dev");
        assert_eq!(process_title(false), "deckhand");
    }

    #[test]
    fn version_line_names_the_crate_version() {
        assert_eq!(version_line(), format!("deckhand {}", env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn foreign_daemons_are_refused() {
        let refused = "Refusing to control a daemon outside the isolated dev namespace";
        let ping = |home: Option<&str>, channel: Option<&str>| PingResult {
            ok: true,
            version: Some(46),
            home: home.map(str::to_owned),
            channel: channel.map(str::to_owned),
        };
        let other = tempfile::tempdir().ok();
        let other_home = other
            .as_ref()
            .map(|dir| dir.path().display().to_string())
            .unwrap_or_default();
        for daemon in [
            ping(None, Some("dev")),
            ping(Some(&other_home), Some("dev")),
            ping(Some(&other_home), None),
        ] {
            assert_eq!(
                check_dev_daemon(&daemon).map_err(|error| error.to_string()),
                Err(refused.to_owned())
            );
        }
        let ours = paths::config_dir().display().to_string();
        assert!(check_dev_daemon(&ping(Some(&ours), Some("dev"))).is_ok());
        assert_eq!(
            check_dev_daemon(&ping(Some(&ours), Some("stable"))).map_err(|error| error.to_string()),
            Err(refused.to_owned())
        );
    }

    #[test]
    fn status_line_prints_undefined_for_missing_fields() {
        let daemon = PingResult {
            ok: true,
            version: Some(46),
            home: Some("/tmp/x".into()),
            channel: Some("dev".into()),
        };
        assert_eq!(
            status_line(&daemon),
            "Isolated dev daemon running (protocol v46): /tmp/x"
        );
        let bare = PingResult {
            ok: true,
            version: None,
            home: None,
            channel: None,
        };
        assert_eq!(
            status_line(&bare),
            "Isolated dev daemon running (protocol vundefined): undefined"
        );
    }
}
