//! `deckhand update [--yes|-y]`: update Deckhand the way it was installed, then offer to restart the daemon.
//!
//! It tells how this executable was installed (`deckhand_core::self_update::install_method`: Homebrew's Cellar path,
//! cargo's install record, the installer script's receipt), looks the latest release up, says which command updates
//! it and asks before running it (`--yes`: without asking; with no terminal to ask on, `--yes` is required). Installs
//! it cannot update (a checkout's `cargo install --path`, a release archive, a development build, or one two methods
//! claim) get what to do instead. After the upgrade the new binary runs `deckhand update --finish`, which compares
//! itself with the running daemon: another protocol means Deckhand refuses that daemon until it restarts; another
//! build of the same protocol keeps working. Either way `deckhand restart` is offered (it asks only when sessions
//! would be interrupted). The restart check is the new binary's, since only it knows its own protocol.

use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;

use deckhand_client::client::{DaemonState, daemon_state};
use deckhand_core::daemon_build::{BuildRelation, RESTART_COMMAND, compare_builds};
use deckhand_core::error::DhError;
use deckhand_core::jsstr::date;
use deckhand_core::self_update::{
    InstallMethod, RELEASES_URL, UpdateCheck, install_method, manual_instructions, newer_release, upgrade_command,
};
use deckhand_protocol::BUILD_VERSION;
use deckhand_sys::paths;
use deckhand_sys::self_update_check::{fetch_latest, install_facts, update_check_path, write_update_check};

use crate::cli::print_line;
use crate::setup::prompt_default_yes;

/// `deckhand update`'s usage line.
pub const USAGE: &str = "Usage: deckhand update [--yes|-y]";

/// `deckhand update`'s arguments.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UpdateOptions {
    /// `--yes` / `-y`: run the upgrade without asking.
    pub yes: bool,
    /// `--finish` (internal): the step after an upgrade, run by the new binary.
    pub finish: bool,
}

/// Parses the arguments after `update`; anything else is refused with the usage line.
pub fn parse_update_args(args: &[String]) -> Result<UpdateOptions, DhError> {
    let mut options = UpdateOptions::default();
    for arg in args {
        match arg.as_str() {
            "--yes" | "-y" => options.yes = true,
            "--finish" => options.finish = true,
            other => return Err(DhError::msg(format!("Unknown argument: {other}\n{USAGE}"))),
        }
    }
    Ok(options)
}

/// `deckhand update [--yes|-y]`. Exit status 1 when nothing was updated (declined, or an install it leaves to you).
pub async fn run_update(args: &[String]) -> Result<ExitCode, DhError> {
    let options = parse_update_args(args)?;
    if options.finish {
        return finish().await;
    }
    let exe = paths::cli_entry_path()?;
    let dev = cfg!(debug_assertions) || paths::is_dev_channel() || paths::is_dev_runtime();
    let facts = install_facts(&exe, dev);
    let method = install_method(&facts);
    print_line(&format!(
        "Deckhand {BUILD_VERSION}, installed with {} ({}).",
        method.label(),
        facts.canonical.display()
    ));
    if method == InstallMethod::Dev {
        print_line(&manual_instructions(&method, ""));
        return Ok(ExitCode::FAILURE);
    }
    let latest = fetch_latest()
        .await
        .map_err(|error| DhError::msg(format!("Couldn't look up the latest release: {error}")))?;
    // The header and the Agents screen show the answer too.
    let check = UpdateCheck {
        latest: latest.clone(),
        checked_at: date::to_iso_string(date::now_ms()).unwrap_or_default(),
    };
    let _ = write_update_check(&update_check_path(), &check).await;
    let Some(new) = newer_release(BUILD_VERSION, Some(&latest)) else {
        print_line(&format!("{BUILD_VERSION} is the latest release."));
        return finish().await;
    };
    print_line(&format!("Deckhand {new} is available: {RELEASES_URL}/tag/v{new}"));
    let Some(command) = upgrade_command(&method, new) else {
        print_line(&manual_instructions(&method, new));
        return Ok(ExitCode::FAILURE);
    };
    print_line(&format!("Update with: {}", command.display));
    if !options.yes {
        if !std::io::stdin().is_terminal() {
            return Err(DhError::msg(format!(
                "Not updating without asking: run `deckhand update --yes` to run `{}`.",
                command.display
            )));
        }
        if prompt_default_yes("Run it?").await != Some(true) {
            print_line("Not updated.");
            return Ok(ExitCode::FAILURE);
        }
    }
    let status = {
        let command = command.clone();
        tokio::task::spawn_blocking(move || {
            std::process::Command::new(&command.program)
                .args(&command.args)
                .envs(command.env.iter().map(|(key, value)| (key, value)))
                .status()
        })
        .await
        .map_err(|error| DhError::msg(error.to_string()))?
    };
    match status {
        Ok(status) if status.success() => {}
        Ok(status) => {
            return Err(DhError::msg(format!(
                "`{}` failed ({status}); Deckhand was not updated. Its output above says why; you can run it yourself.",
                command.display
            )));
        }
        Err(error) => {
            return Err(DhError::msg(format!(
                "Couldn't run `{}` ({error}); Deckhand was not updated.",
                command.display
            )));
        }
    }
    let installed = binary_version(&command.binary).await;
    match installed.as_deref() {
        Some(version) if version != BUILD_VERSION => print_line(&format!("Updated to {version}.")),
        _ => {
            let hint = if matches!(method, InstallMethod::Homebrew { .. }) {
                " Homebrew may not know the release yet: run `brew update`, then `deckhand update` again."
            } else {
                ""
            };
            print_line(&format!(
                "`{}` finished, but {} is still {}.{hint}",
                command.display,
                command.binary.display(),
                installed.as_deref().unwrap_or("unknown")
            ));
            return Ok(ExitCode::FAILURE);
        }
    }
    // The new binary knows its own protocol: it decides whether the daemon must restart.
    let finished = tokio::task::spawn_blocking(move || {
        std::process::Command::new(&command.binary)
            .args(["update", "--finish"])
            .status()
    })
    .await
    .map_err(|error| DhError::msg(error.to_string()))?;
    match finished {
        Ok(status) if status.success() => Ok(ExitCode::SUCCESS),
        _ => {
            print_line(&format!("Restart the daemon when convenient: {RESTART_COMMAND}"));
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// The version `<binary> --version` prints (`deckhand 3.4.0` → `3.4.0`).
async fn binary_version(binary: &Path) -> Option<String> {
    let binary = binary.to_path_buf();
    let output = tokio::task::spawn_blocking(move || std::process::Command::new(binary).arg("--version").output())
        .await
        .ok()?
        .ok()?;
    parse_version_line(&String::from_utf8_lossy(&output.stdout))
}

/// `deckhand 3.4.0\n` → `3.4.0`.
fn parse_version_line(line: &str) -> Option<String> {
    line.trim().strip_prefix("deckhand ").map(str::to_owned)
}

/// What an update means for the running daemon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DaemonAfterUpdate {
    /// No daemon runs: the next `deckhand` starts this build's.
    NotRunning,
    /// It already runs this build.
    Current,
    /// Another protocol: refused until restarted.
    RestartRequired,
    /// Another build of the same protocol (`build`): compatible, restart when convenient.
    RestartOptional { build: Option<String> },
    /// It did not answer.
    Unresponsive { pid: i32 },
}

/// [`DaemonAfterUpdate`] for `state`, from this build's point of view.
pub fn daemon_after_update(state: &DaemonState) -> DaemonAfterUpdate {
    match state {
        DaemonState::NotRunning => DaemonAfterUpdate::NotRunning,
        DaemonState::OtherProtocol { .. } => DaemonAfterUpdate::RestartRequired,
        DaemonState::Unresponsive { pid, .. } => DaemonAfterUpdate::Unresponsive { pid: *pid },
        DaemonState::Running { ping, .. } => match compare_builds(BUILD_VERSION, ping.build.as_deref()) {
            BuildRelation::Same => DaemonAfterUpdate::Current,
            _ => DaemonAfterUpdate::RestartOptional {
                build: ping.build.clone(),
            },
        },
    }
}

/// What `deckhand update` says about the daemon before offering the restart, or (`Err`) instead of offering one.
pub fn daemon_message(after: &DaemonAfterUpdate) -> Result<String, String> {
    match after {
        DaemonAfterUpdate::NotRunning => Err(format!(
            "No daemon is running; the next deckhand starts {BUILD_VERSION}."
        )),
        DaemonAfterUpdate::Current => Err(format!("The daemon already runs {BUILD_VERSION}.")),
        DaemonAfterUpdate::Unresponsive { pid } => Err(format!(
            "The daemon (pid {pid}) did not answer. Run `{RESTART_COMMAND}` to replace it with {BUILD_VERSION}."
        )),
        DaemonAfterUpdate::RestartRequired => Ok(format!(
            "The running daemon speaks an older protocol: Deckhand won't use it, and agents' deckhand commands fail, \
             until it restarts as {BUILD_VERSION}."
        )),
        DaemonAfterUpdate::RestartOptional { build } => Ok(format!(
            "The daemon still runs {}. It stays compatible; restarting picks up {BUILD_VERSION}'s daemon changes.",
            build.as_deref().unwrap_or("an older build")
        )),
    }
}

/// `deckhand update --finish` (and `deckhand update` when already current): says what the update means for the
/// running daemon and offers `deckhand restart`.
async fn finish() -> Result<ExitCode, DhError> {
    let after = daemon_after_update(&daemon_state().await);
    let message = match daemon_message(&after) {
        Ok(message) => message,
        Err(nothing_to_offer) => {
            print_line(&nothing_to_offer);
            return Ok(ExitCode::SUCCESS);
        }
    };
    print_line(&message);
    if !std::io::stdin().is_terminal() {
        print_line(&format!("Restart it when convenient: {RESTART_COMMAND}"));
        return Ok(ExitCode::SUCCESS);
    }
    // `deckhand restart` asks only when sessions would be interrupted, saying what each is doing.
    if crate::restart::run_restart(&[]).await? != ExitCode::SUCCESS {
        print_line(&format!("Restart it when convenient: {RESTART_COMMAND}"));
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use deckhand_protocol::data::PingResult;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn update_takes_yes_and_the_internal_finish() {
        assert_eq!(parse_update_args(&[]).ok(), Some(UpdateOptions::default()));
        assert_eq!(
            parse_update_args(&args(&["-y"])).ok(),
            Some(UpdateOptions {
                yes: true,
                finish: false
            })
        );
        assert_eq!(
            parse_update_args(&args(&["--finish"])).ok(),
            Some(UpdateOptions {
                yes: false,
                finish: true
            })
        );
        let error = parse_update_args(&args(&["--force"])).unwrap_err().to_string();
        assert_eq!(error, "Unknown argument: --force\nUsage: deckhand update [--yes|-y]");
    }

    #[test]
    fn the_new_binary_reports_its_version() {
        assert_eq!(parse_version_line("deckhand 3.4.0\n").as_deref(), Some("3.4.0"));
        assert_eq!(parse_version_line("something else"), None);
    }

    fn running(build: Option<&str>) -> DaemonState {
        DaemonState::Running {
            ping: PingResult {
                ok: true,
                build: build.map(str::to_owned),
                ..PingResult::default()
            },
            pid: Some(7),
        }
    }

    #[test]
    fn the_running_daemon_decides_whether_a_restart_is_needed() {
        assert_eq!(
            daemon_after_update(&DaemonState::NotRunning),
            DaemonAfterUpdate::NotRunning
        );
        assert_eq!(
            daemon_after_update(&running(Some(BUILD_VERSION))),
            DaemonAfterUpdate::Current
        );
        assert_eq!(
            daemon_after_update(&running(Some("0.0.1"))),
            DaemonAfterUpdate::RestartOptional {
                build: Some("0.0.1".to_owned())
            }
        );
        // A daemon from before build reporting is older.
        assert_eq!(
            daemon_after_update(&running(None)),
            DaemonAfterUpdate::RestartOptional { build: None }
        );
        let other = DaemonState::OtherProtocol {
            message: "daemon protocol mismatch: expected v64, got 63".to_owned(),
            pid: Some(7),
        };
        assert_eq!(daemon_after_update(&other), DaemonAfterUpdate::RestartRequired);
        let stuck = DaemonState::Unresponsive {
            pid: 7,
            error: "timed out".to_owned(),
        };
        assert_eq!(daemon_after_update(&stuck), DaemonAfterUpdate::Unresponsive { pid: 7 });
    }

    #[test]
    fn only_another_daemon_build_is_offered_a_restart() {
        assert!(daemon_message(&DaemonAfterUpdate::NotRunning).is_err());
        assert!(daemon_message(&DaemonAfterUpdate::Current).is_err());
        assert!(daemon_message(&DaemonAfterUpdate::Unresponsive { pid: 7 }).is_err());
        let required = daemon_message(&DaemonAfterUpdate::RestartRequired).unwrap_or_default();
        assert!(
            required.starts_with("The running daemon speaks an older protocol"),
            "{required}"
        );
        let optional = daemon_message(&DaemonAfterUpdate::RestartOptional {
            build: Some("3.3.0".to_owned()),
        })
        .unwrap_or_default();
        assert!(
            optional.starts_with("The daemon still runs 3.3.0. It stays compatible"),
            "{optional}"
        );
    }
}
