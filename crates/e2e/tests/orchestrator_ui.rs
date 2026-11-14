//! The orchestrator in the UI, in a real PTY: a plan applied while you are attached to a session detaches you to the
//! confirmation, `enter` creates it and attaches you again; `O` starts the repository's orchestrator, then attaches.

use std::fs;
use std::io::Read;
use std::process::{Child, Stdio};
use std::time::Duration;

use deckhand_e2e::fakes::install_sandbox_agents;
use deckhand_e2e::harness::{Sandbox, launcher, wait_exit, wait_for};
use deckhand_e2e::ui::{UI_WAIT, Ui, launch_sandbox_ui};
use serde_json::{Value, json};

/// `deckhand plan apply <file> --json` by hand in the sandbox repository (through the dev launcher, so against the
/// UI's daemon), left running: it waits for the answer in Deckhand.
fn apply_plan(sandbox: &Sandbox, bin: &std::path::Path, plan: &Value) -> Child {
    let file = sandbox.root().join("plan.json");
    fs::write(&file, plan.to_string()).expect("write the plan");
    let mut command = sandbox.command(launcher());
    sandbox.dev_env(&mut command);
    command
        .args(["--sandbox", "plan", "apply"])
        .arg(&file)
        .arg("--json")
        .env("PATH", deckhand_e2e::daemon::path_with(bin))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    sandbox.spawn(command)
}

/// The finished command's stdout as JSON (it must have succeeded).
fn outcome(mut child: Child) -> Value {
    let mut stdout = child.stdout.take().expect("stdout");
    let mut stderr = child.stderr.take().expect("stderr");
    let out = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let err = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let status = wait_exit(&mut child, Duration::from_secs(60)).expect("plan apply finished");
    let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    assert!(status.success(), "{out}{err}");
    serde_json::from_str(&out).unwrap_or_else(|error| panic!("{error}: {out}{err}"))
}

/// Waits until the selected session can be attached (its agent runs: the details no longer say it is starting).
fn wait_until_attachable(ui: &Ui) {
    wait_for(
        || ui.screen_text(),
        |frame| {
            frame.contains("q quit")
                && !frame.contains("Working…")
                && !frame.contains("starting")
                && !frame.contains("preparing workspace")
        },
        Duration::from_secs(15),
    );
}

/// Waits for the attached status line (and Deckhand's own screen gone).
fn wait_attached(ui: &Ui, title: &str) -> String {
    wait_for(
        || ui.screen_text(),
        |frame| frame.contains("Ctrl+] detach") && frame.contains(title) && !frame.contains("q quit"),
        UI_WAIT,
    )
}

#[test]
fn a_plan_detaches_you_to_its_confirmation_and_enter_creates_it_then_attaches_again() {
    let sandbox = Sandbox::new("plan-ui");
    let bin = install_sandbox_agents(&sandbox, "echo 'fake agent ready'");
    let mut ui = launch_sandbox_ui(&sandbox, &bin, &[]);
    ui.screen("DEV (isolated)");
    ui.screen("● ready");
    // A session to talk in (the sandbox preselects a new worktree), attached.
    ui.press("n");
    ui.screen("Choose an agent");
    ui.press("\r");
    ui.screen("Workspace: new worktree");
    ui.press("chat");
    ui.screen("Name: chat");
    ui.press("\r");
    ui.screen("fake agent ready");
    wait_until_attachable(&ui);
    ui.press("\r");
    wait_attached(&ui, "chat");

    // A plan is applied while attached: Deckhand detaches to ask, keys typed on are not an answer.
    let plan = json!({
        "sessions": [{"key": "auth", "title": "Auth rework", "prompt": "Brief: rework auth", "worktree": "none"}],
        "tasks": [{"title": "Remove legacy endpoint", "details": "After Auth rework merges"}],
    });
    let applying = apply_plan(&sandbox, &bin, &plan);
    let dialog = ui.screen("Create this plan?");
    assert!(dialog.contains("A plan of 1 session, 1 task."), "{dialog}");
    assert!(dialog.contains("◇ Auth rework  (no worktree · claude)"), "{dialog}");
    assert!(dialog.contains("☐ Remove legacy endpoint"), "{dialog}");
    assert!(dialog.contains("enter create • esc cancel"), "{dialog}");
    // Past the dialog's quiet moment, enter creates it and you are back where you were.
    std::thread::sleep(Duration::from_millis(1000));
    ui.press("\r");
    let created = outcome(applying);
    assert_eq!(created["status"], "created", "{created}");
    assert_eq!(created["sessions"][0]["session"]["title"], "Auth rework", "{created}");
    assert_eq!(created["tasks"][0]["title"], "Remove legacy endpoint", "{created}");
    let back = wait_attached(&ui, "chat");
    assert!(!back.contains("Create this plan?"), "{back}");

    // Detached: the plan's session is in the list.
    ui.press("\x1d");
    ui.screen("Auth rework");

    // O starts the repository's orchestrator (◎) and selects it; once it runs, O attaches to it.
    ui.press("O");
    ui.screen("◎ orchestrator · pl");
    wait_until_attachable(&ui);
    ui.press("O");
    wait_attached(&ui, "Orchestrator");
}
