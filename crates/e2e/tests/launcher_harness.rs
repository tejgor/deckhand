//! Covers the harness's launcher plumbing (`launcher`, `Sandbox::exec_launcher`, `Sandbox::stop_dev_daemon`,
//! `ui::spawn_launcher`): `scripts/deckhand-dev.sh` is executable and is run directly, never through `sh`.

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use deckhand_e2e::daemon::call;
use deckhand_e2e::fakes::{READY, install_sh_agents};
use deckhand_e2e::harness::{Sandbox, is_alive, launcher, wait_for};
use deckhand_e2e::ui::{launch_sandbox_ui, spawn_launcher};
use serde_json::json;

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn launcher_script_is_executable() {
    let mode = std::fs::metadata(launcher()).expect("launcher").permissions().mode();
    assert!(
        mode & 0o111 != 0,
        "scripts/deckhand-dev.sh must be executable: {mode:o}"
    );
}

#[test]
fn exec_launcher_runs_the_script_directly_with_the_dev_environment() {
    let sandbox = Sandbox::new("launcher-exec");
    let help = sandbox.exec_launcher(&["--help"], Duration::from_secs(30));
    assert!(help.status.success(), "stderr: {}", text(&help.stderr));
    assert!(text(&help.stdout).starts_with("Usage: scripts/deckhand-dev.sh"));
    // `status` reads DECKHAND_DEV_HOME (the sandbox's own), where nothing runs yet.
    let status = sandbox.exec_launcher(&["status"], Duration::from_secs(30));
    assert!(status.status.success(), "stderr: {}", text(&status.stderr));
    assert!(
        text(&status.stdout).contains("No isolated dev daemon"),
        "{}",
        text(&status.stdout)
    );
    assert!(
        !sandbox.dev_home().join("daemon.pid").exists(),
        "status must not start a daemon"
    );
}

#[test]
fn spawn_launcher_starts_the_isolated_ui_and_stop_dev_daemon_stops_its_daemon() {
    let sandbox = Sandbox::new("launcher-ui");
    let bin = install_sh_agents(&sandbox.root().join("agents"), READY);
    let ui = launch_sandbox_ui(&sandbox, &bin, &[]);
    ui.screen("DEV (isolated)");
    ui.screen("● ready");
    let pid_file = sandbox.dev_home().join("daemon.pid");
    let pid: i32 = wait_for(
        || {
            std::fs::read_to_string(&pid_file)
                .ok()
                .and_then(|t| t.trim().parse().ok())
        },
        |pid: &Option<i32>| pid.is_some(),
        Duration::from_secs(15),
    )
    .expect("daemon pid");
    assert!(is_alive(pid), "dev daemon {pid} should be running");
    assert_eq!(
        call(&sandbox.dev_home().join("daemon.sock"), json!({"type": "ping"})).map(|v| v["ok"] == true),
        Ok(true)
    );
    sandbox.stop_dev_daemon();
    assert!(!is_alive(pid), "stop_dev_daemon must wait for the dev daemon to exit");
    drop(ui);
}

#[test]
fn spawn_launcher_runs_help_in_a_pty() {
    let sandbox = Sandbox::new("launcher-pty");
    // The `--help` banner needs no daemon and starts no agent.
    let mut ui = spawn_launcher(&sandbox, &["--help"], sandbox.root());
    ui.screen("Usage: scripts/deckhand-dev.sh");
    let status = ui.ended(Duration::from_secs(15)).expect("--help exits");
    assert!(status.success());
}
