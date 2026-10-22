//! `deckhand restart [--yes|-y]`: replace the running daemon with this executable's.
//!
//! A daemon keeps the binary it started from, so an update reaches the daemon and its workers only once the daemon is
//! restarted (and a client refuses a daemon of another protocol until then). Restarting interrupts the running
//! sessions (the next daemon marks them interrupted; `s` resumes them), so with any running and without `--yes` it
//! asks first, and refuses when there is no terminal to ask on. The daemon is stopped with the `shutdown` request
//! where it accepts it (the dev channel), else with SIGTERM to its pid, which every daemon (older and other-protocol
//! ones included) handles by shutting down cleanly; SIGKILL follows after a grace period. The next daemon starts the
//! usual way (`deckhand_client::client::ensure_daemon_running`).

use std::io::IsTerminal;
use std::process::ExitCode;

use deckhand_client::client::{
    DAEMON_STOP_GRACE, DaemonState, StopOutcome, count_live_sessions, daemon_state, ensure_daemon_running, stop_daemon,
};
use deckhand_core::daemon_build::{restart_prompt, restart_refusal, restart_summary};
use deckhand_core::error::DhError;

use crate::cli::print_line;
use crate::setup::prompt_yes;

/// `deckhand restart`'s usage line.
pub const USAGE: &str = "Usage: deckhand restart [--yes|-y]";

/// `deckhand restart`'s arguments.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RestartOptions {
    /// `--yes` / `-y`: restart without asking, whatever runs.
    pub yes: bool,
}

/// Parses the arguments after `restart`; anything but `--yes`/`-y` is refused with the usage line.
pub fn parse_restart_args(args: &[String]) -> Result<RestartOptions, DhError> {
    let mut options = RestartOptions::default();
    for arg in args {
        match arg.as_str() {
            "--yes" | "-y" => options.yes = true,
            other => return Err(DhError::msg(format!("Unknown argument: {other}\n{USAGE}"))),
        }
    }
    Ok(options)
}

/// Whether to ask before restarting: no `--yes`, and sessions would be interrupted (or the daemon could not say).
pub fn needs_confirmation(running: Option<usize>, yes: bool) -> bool {
    !yes && running != Some(0)
}

/// `deckhand restart [--yes|-y]`. Exit status 1 when the user declines.
pub async fn run_restart(args: &[String]) -> Result<ExitCode, DhError> {
    let options = parse_restart_args(args)?;
    let state = daemon_state().await;
    let old_build = match &state {
        DaemonState::NotRunning => {
            let daemon = ensure_daemon_running().await?;
            let build = daemon.build.map(|build| format!(" ({build})")).unwrap_or_default();
            print_line(&format!("No daemon was running; started one{build}."));
            return Ok(ExitCode::SUCCESS);
        }
        DaemonState::Running { ping, .. } => ping.build.clone(),
        DaemonState::OtherProtocol { .. } | DaemonState::Unresponsive { .. } => None,
    };
    let running = count_live_sessions().await;
    if needs_confirmation(running, options.yes) {
        if !std::io::stdin().is_terminal() {
            return Err(DhError::msg(restart_refusal(running)));
        }
        if prompt_yes(&restart_prompt(running), false).await != Some(true) {
            print_line("Daemon not restarted.");
            return Ok(ExitCode::FAILURE);
        }
    }
    if stop_daemon(state.pid()).await? == StopOutcome::Killed {
        eprintln!(
            "The daemon did not exit within {} s, so it was killed.",
            DAEMON_STOP_GRACE.as_secs()
        );
    }
    let daemon = ensure_daemon_running().await?;
    print_line(&restart_summary(old_build.as_deref(), daemon.build.as_deref()));
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn restart_takes_only_yes() {
        assert_eq!(parse_restart_args(&[]).ok(), Some(RestartOptions { yes: false }));
        for yes in ["--yes", "-y"] {
            assert_eq!(
                parse_restart_args(&args(&[yes])).ok(),
                Some(RestartOptions { yes: true })
            );
        }
        let error = parse_restart_args(&args(&["--force"])).unwrap_err().to_string();
        assert_eq!(error, "Unknown argument: --force\nUsage: deckhand restart [--yes|-y]");
    }

    #[test]
    fn restart_asks_only_when_something_would_be_interrupted() {
        assert!(!needs_confirmation(Some(0), false));
        assert!(needs_confirmation(Some(2), false));
        // A daemon that cannot list its sessions (another protocol, unresponsive) may have some.
        assert!(needs_confirmation(None, false));
        assert!(!needs_confirmation(Some(2), true));
        assert!(!needs_confirmation(None, true));
    }
}
