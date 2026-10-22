//! `deckhand restart`, black-box: a real daemon with fake agents, the CLI as a user runs it.
//!
//! Restarting replaces the daemon process (an update reaches the daemon only then) and interrupts its sessions, so
//! the command asks first, and refuses without a terminal unless `--yes` says to go ahead.

use std::path::Path;
use std::process::{Child, Output};
use std::time::Duration;

use deckhand_e2e::daemon::{create, id_of, install_agents, launch, must, ready, running, state};
use deckhand_e2e::fake_agent;
use deckhand_e2e::harness::{Sandbox, is_alive, repo, wait_exit};
use serde_json::{Value, json};

/// The build the binary under test reports (the workspace version, as this crate's).
const BUILD: &str = env!("CARGO_PKG_VERSION");

/// `deckhand restart <args>` with no terminal (stdin closed) and only the fakes and system tools on PATH, so a daemon
/// it starts can never find a real agent.
fn restart(sandbox: &Sandbox, bin: &Path, args: &[&str]) -> Output {
    let mut command = sandbox.cli(&[&["restart"], args].concat());
    command.env("PATH", format!("{}:/usr/bin:/bin", bin.display()));
    sandbox.exec(command, Duration::from_secs(40))
}

fn ping_pid(socket: &Path) -> i64 {
    must(socket, json!({"type": "ping"}))["pid"]
        .as_i64()
        .unwrap_or_default()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A daemon in `root` with one running fake Claude session; returns the daemon, the session ID and the fakes' dir.
fn daemon_with_a_running_session(sandbox: &Sandbox, root: &Path) -> (Child, String, std::path::PathBuf) {
    let bin = install_agents(sandbox, fake_agent!());
    let daemon = launch(sandbox, root, &bin);
    ready(&sandbox.socket());
    let root_path = root.to_string_lossy().into_owned();
    let session = create(&sandbox.socket(), &root_path, "agent", "none", json!({}));
    let id = id_of(&session);
    running(&sandbox.socket(), &id);
    (daemon, id, bin)
}

#[test]
fn restart_yes_replaces_the_daemon_and_interrupts_its_sessions() {
    let root = repo();
    let sandbox = Sandbox::new("restart");
    let socket = sandbox.socket();
    let (mut old_daemon, id, bin) = daemon_with_a_running_session(&sandbox, root.path());
    let old_pid = ping_pid(&socket);
    assert_eq!(old_pid, i64::from(old_daemon.id()));
    assert_eq!(must(&socket, json!({"type": "ping"}))["build"], BUILD);

    let output = restart(&sandbox, &bin, &["--yes"]);
    assert!(
        output.status.success(),
        "{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    assert_eq!(text(&output.stdout), format!("Daemon restarted ({BUILD}).\n"));

    // The old daemon shut down by itself (SIGTERM: a stable daemon refuses `shutdown`), cleanly.
    let status = wait_exit(&mut old_daemon, Duration::from_secs(5)).expect("the old daemon exited");
    assert!(status.success(), "{status:?}");
    // A new daemon answers, from another process, and daemon.pid names it.
    let new_pid = ping_pid(&socket);
    assert_ne!(new_pid, old_pid);
    assert!(is_alive(i32::try_from(new_pid).unwrap_or_default()));
    let pid_file = std::fs::read_to_string(sandbox.home().join("daemon.pid")).unwrap_or_default();
    assert_eq!(pid_file.trim(), new_pid.to_string());
    // The session it hosted is interrupted (resumable with s), not lost.
    let session: Value = state(&socket, &id);
    assert_eq!(session["status"], "exited", "{session}");
    assert_eq!(session["exitReason"], "interrupted", "{session}");
}

#[test]
fn restart_without_yes_refuses_without_a_terminal_while_sessions_run() {
    let root = repo();
    let sandbox = Sandbox::new("restart-no");
    let socket = sandbox.socket();
    let (_daemon, id, bin) = daemon_with_a_running_session(&sandbox, root.path());
    let pid = ping_pid(&socket);

    let output = restart(&sandbox, &bin, &[]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stderr),
        "Not restarting the Deckhand daemon: 1 running session(s) would be interrupted (resume them with s). \
         Run `deckhand restart --yes` to restart it anyway.\n"
    );
    assert_eq!(text(&output.stdout), "");
    // Nothing was touched.
    assert_eq!(ping_pid(&socket), pid);
    assert_eq!(state(&socket, &id)["status"], "running");
}

#[test]
fn restart_starts_a_daemon_when_none_runs() {
    let sandbox = Sandbox::new("restart-none");
    let bin = install_agents(&sandbox, fake_agent!());
    let output = restart(&sandbox, &bin, &[]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        format!("No daemon was running; started one ({BUILD}).\n")
    );
    assert!(ping_pid(&sandbox.socket()) > 0);
    // With nothing running there is nothing to ask about: a second restart goes ahead without --yes.
    let output = restart(&sandbox, &bin, &[]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), format!("Daemon restarted ({BUILD}).\n"));
}
