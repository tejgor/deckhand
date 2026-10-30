//! `deckhand restart`, black-box: a real daemon with fake agents, the CLI as a user runs it.
//!
//! Restarting replaces the daemon process (an update reaches the daemon only then) and interrupts its sessions, so
//! the command asks first, and refuses without a terminal unless `--yes` says to go ahead.

use std::path::Path;
use std::process::{Child, Output};
use std::time::Duration;

use deckhand_e2e::daemon::{create, id_of, install_agents, launch, must, ready, running, state};
use deckhand_e2e::fake_agent;
use deckhand_e2e::harness::{Sandbox, is_alive, repo, stop, wait_exit};
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
    // The session it hosted is interrupted (resumable with s), not lost, and offered for resuming.
    let session: Value = state(&socket, &id);
    assert_eq!(session["status"], "exited", "{session}");
    assert_eq!(session["exitReason"], "interrupted", "{session}");
    assert!(session["interruptedWhile"].is_string(), "{session}");
}

#[test]
fn daemons_started_at_once_leave_one_that_offers_the_interrupted_sessions() {
    let root = repo();
    let sandbox = Sandbox::new("restart-race");
    let socket = sandbox.socket();
    let (mut old_daemon, id, bin) = daemon_with_a_running_session(&sandbox, root.path());
    // Stopped as `deckhand restart` stops it.
    let old_pid = nix::unistd::Pid::from_raw(i32::try_from(old_daemon.id()).unwrap_or_default());
    nix::sys::signal::kill(old_pid, nix::sys::signal::Signal::SIGTERM).expect("SIGTERM");
    wait_exit(&mut old_daemon, Duration::from_secs(10)).expect("the old daemon exited");

    // Two clients find no daemon at the same moment (`deckhand restart` and an open UI) and each starts one.
    let first = launch(&sandbox, root.path(), &bin);
    let second = launch(&sandbox, root.path(), &bin);
    ready(&socket);
    let serving = ping_pid(&socket);
    let (mut winner, mut loser) = if serving == i64::from(first.id()) {
        (first, second)
    } else {
        (second, first)
    };
    assert_eq!(serving, i64::from(winner.id()));
    // The other gives up once it cannot take daemon.lock, before it touched the state or the socket.
    let status = wait_exit(&mut loser, Duration::from_secs(10)).expect("the second daemon gave up");
    assert!(!status.success(), "{status:?}");
    assert!(is_alive(i32::try_from(winner.id()).unwrap_or_default()));
    // The one serving recovered the session, so it offers it for resuming.
    let session: Value = state(&socket, &id);
    assert_eq!(session["exitReason"], "interrupted", "{session}");
    assert!(session["interruptedWhile"].is_string(), "{session}");
    assert!(wait_exit(&mut winner, Duration::ZERO).is_none());
}

#[test]
fn restarting_sessions_boots_at_most_three_at_a_time() {
    let root = repo();
    let sandbox = Sandbox::new("restart-many");
    let socket = sandbox.socket();
    let bin = install_agents(&sandbox, fake_agent!());
    let mut daemon = launch(&sandbox, root.path(), &bin);
    ready(&socket);
    let root_path = root.path().to_string_lossy().into_owned();
    let ids: Vec<String> = (1..=4)
        .map(|n| {
            let id = id_of(&create(&socket, &root_path, &format!("agent {n}"), "none", json!({})));
            running(&socket, &id);
            id
        })
        .collect();
    let launched = |id: &str| {
        state(&socket, id)["agentStartedAt"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    let before: Vec<String> = ids.iter().map(|id| launched(id)).collect();

    // Running ones are stopped, then resumed: every one comes back.
    let outcomes = must(
        &socket,
        json!({"type": "restart-sessions", "sessionIds": ids, "cols": 80, "rows": 24}),
    );
    let expected: Vec<Value> = ids.iter().map(|id| json!({"sessionId": id})).collect();
    assert_eq!(outcomes, Value::Array(expected));
    let after: Vec<String> = ids.iter().map(|id| launched(id)).collect();
    for (id, (old, new)) in ids.iter().zip(before.iter().zip(&after)) {
        assert!(new > old, "{id} was not launched again: {old} → {new}");
        assert_eq!(state(&socket, id)["status"], "running");
    }
    // The fourth waited for one of the first three to settle (its screen still for a second at least).
    let millis = |iso: &str| {
        let time = iso.get(11..23).unwrap_or_default();
        let parts: Vec<f64> = time
            .split([':', '.'])
            .map(|part| part.parse().unwrap_or_default())
            .collect();
        ((parts[0] * 60.0 + parts[1]) * 60.0 + parts[2]) * 1000.0 + parts[3]
    };
    let first = after[..3].iter().map(|at| millis(at)).fold(f64::INFINITY, f64::min);
    assert!(
        millis(&after[3]) - first >= 900.0,
        "the fourth started with the first three: {after:?}"
    );
    stop(&mut daemon);
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
        "Not restarting the Deckhand daemon: 1 running session(s) (1 idle) would be interrupted (Deckhand offers \
         to resume them when you next open it). Run `deckhand restart --yes` to restart it anyway.\n"
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
