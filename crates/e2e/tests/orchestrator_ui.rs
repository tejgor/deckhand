//! The orchestrator in the UI, in a real PTY: `O` opens a dialog for a name and what you want to do and starts a new
//! orchestrator for the idea, every time; a plan it applies while you are attached to it detaches you to the
//! confirmation, `esc` puts the plan aside and attaches you again, `P` brings it back, `enter` creates it, and the
//! orchestrator is told in a message typed into its agent.

use std::fs;
use std::path::Path;
use std::time::Duration;

use deckhand_e2e::fakes::install_sandbox_agents;
use deckhand_e2e::harness::{Sandbox, launcher, wait_for};
use deckhand_e2e::ui::{UI_WAIT, Ui, launch_sandbox_ui};
use serde_json::{Value, json};

/// `deckhand <args>` against the UI's daemon: by hand through the dev launcher; as a session (its identity in `vars`)
/// the built binary with the dev home, as that session's agent runs it (the launcher drops the identity). Its stdout,
/// which must succeed.
fn deckhand(sandbox: &Sandbox, bin: &Path, args: &[&str], vars: &[(&str, &str)]) -> String {
    let mut command = if vars.is_empty() {
        let mut command = sandbox.command(launcher());
        sandbox.dev_env(&mut command);
        command.arg("--sandbox");
        command
    } else {
        let mut command = sandbox.cli(&[]);
        command
            .env("DECKHAND_HOME", sandbox.dev_home())
            .env("DECKHAND_CHANNEL", "dev")
            .current_dir(sandbox.dev_home());
        command
    };
    command.args(args).env("PATH", deckhand_e2e::daemon::path_with(bin));
    for (key, value) in vars {
        command.env(key, value);
    }
    let output = sandbox.exec(command, Duration::from_secs(60));
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
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
fn an_orchestrators_plan_can_be_put_aside_and_brought_back_and_the_orchestrator_hears_the_answer() {
    let sandbox = Sandbox::new("plan-ui");
    // The fake agents note who they are (the identity their `deckhand` commands act with) in `ids/<session ID>`.
    let ids = sandbox.root().join("ids");
    fs::create_dir_all(&ids).expect("ids dir");
    let ready = format!(
        "echo 'fake agent ready'; printf '%s\\n%s\\n%s\\n' \"$DECKHAND_SESSION_ID\" \"$DECKHAND_LAUNCH_ID\" \
         \"$DECKHAND_HOOK_TOKEN\" > '{}'/\"$DECKHAND_SESSION_ID\"",
        ids.display()
    );
    let bin = install_sandbox_agents(&sandbox, &ready);
    let mut ui = launch_sandbox_ui(&sandbox, &bin, &[]);
    ui.screen("DEV (isolated)");
    ui.screen("● ready");

    // O opens the New orchestrator dialog: a name, then (enter) what you want to do, enter a new line in it; ctrl+s
    // starts an orchestrator for the idea (◎, named by the name) and selects it; once it runs, enter attaches to it.
    ui.press("O");
    ui.screen("New orchestrator");
    ui.screen("Name  optional · 2–4 words");
    ui.press("Tokens");
    ui.screen("Name  Tokens");
    ui.press("\r");
    ui.press("Rework auth so tokens");
    ui.press("\r");
    ui.press("are stored encrypted");
    let dialog = ui.screen("are stored encrypted");
    assert!(dialog.contains("Rework auth so tokens"), "{dialog}");
    assert!(dialog.contains("ctrl+s start"), "{dialog}");
    ui.press("\x13");
    let started = ui.screen("◎ orchestrator · pl");
    assert!(started.contains("Tokens"), "{started}");
    assert!(!started.contains("Rework auth so"), "{started}");
    assert!(!started.contains("New orchestrator"), "{started}");
    wait_until_attachable(&ui);
    ui.press("\r");
    wait_attached(&ui, "Tokens");

    // It applies a plan while you talk to it: the command returns at once, and Deckhand detaches you to ask.
    let listed: Value = serde_json::from_str(&deckhand(&sandbox, &bin, &["ls", "--json"], &[])).expect("ls --json");
    let orchestrator = listed
        .as_array()
        .and_then(|sessions| sessions.iter().find(|session| session["title"] == "Tokens"))
        .and_then(|session| session["id"].as_str())
        .unwrap_or_else(|| panic!("the orchestrator: {listed}"))
        .to_owned();
    let identity = wait_for(
        || fs::read_to_string(ids.join(&orchestrator)).unwrap_or_default(),
        |text| text.lines().count() == 3,
        Duration::from_secs(10),
    );
    let identity: Vec<&str> = identity.lines().collect();
    let vars = [
        ("DECKHAND_SESSION_ID", identity[0]),
        ("DECKHAND_LAUNCH_ID", identity[1]),
        ("DECKHAND_HOOK_TOKEN", identity[2]),
    ];
    let plan = json!({
        "sessions": [{"key": "auth", "title": "Auth rework", "prompt": "Brief: rework auth", "worktree": "none"}],
        "tasks": [{"title": "Remove legacy endpoint", "details": "After Auth rework merges"}],
    });
    let file = sandbox.root().join("plan.json");
    fs::write(&file, plan.to_string()).expect("write the plan");
    let applied: Value = serde_json::from_str(&deckhand(
        &sandbox,
        &bin,
        &["plan", "apply", &file.to_string_lossy(), "--json"],
        &vars,
    ))
    .expect("plan apply --json");
    let plan_id = applied["planId"].as_str().unwrap_or_default().to_owned();
    assert_eq!(applied["summary"]["counts"], "1 session, 1 task", "{applied}");
    let dialog = ui.screen("Create this plan?");
    assert!(dialog.contains("A plan for ◎ Tokens: 1 session, 1 task."), "{dialog}");
    assert!(dialog.contains("◇ Auth rework  (no worktree · claude)"), "{dialog}");
    assert!(dialog.contains("☐ Remove legacy endpoint"), "{dialog}");
    assert!(dialog.contains("enter create • esc put aside • x x cancel"), "{dialog}");

    // Past the dialog's quiet moment, esc puts it aside: you are back where you were, and nothing is created.
    std::thread::sleep(Duration::from_millis(1000));
    ui.press("\x1b");
    let back = wait_attached(&ui, "Tokens");
    assert!(!back.contains("Create this plan?"), "{back}");

    // Detached, the header and the orchestrator's details say the plan waits; it does not open by itself.
    ui.press("\x1d");
    let frame = ui.screen("◎ 1 plan waiting · P");
    // In the header, and in the orchestrator's details.
    assert_eq!(frame.matches("◎ 1 plan waiting · P").count(), 2, "{frame}");
    std::thread::sleep(Duration::from_millis(500));
    let frame = ui.screen_text();
    assert!(!frame.contains("Create this plan?"), "{frame}");
    assert!(!frame.contains("Auth rework"), "{frame}");

    // P brings it back; enter creates it.
    ui.press("P");
    ui.screen("Create this plan?");
    ui.press("\r");
    let frame = ui.screen("Auth rework");
    assert!(!frame.contains("plan waiting"), "{frame}");
    // The orchestrator is told: the answer is typed into its agent once it is idle (its screen shows the echo).
    ui.screen(&format!("[The user approved your plan {plan_id}]"));

    // Another idea, while that one runs: another orchestrator. Esc closes the dialog and starts nothing, but keeps
    // what was typed for the next O.
    ui.press("O");
    ui.screen("New orchestrator");
    ui.press("\t");
    ui.press("Speed up the build");
    ui.screen("Speed up the build");
    ui.press("\x1b");
    let frame = wait_for(
        || ui.screen_text(),
        |frame| !frame.contains("New orchestrator"),
        Duration::from_secs(5),
    );
    assert!(!frame.contains("Speed up the build"), "{frame}");
    // Without a name, the idea's first line names it.
    ui.press("O");
    ui.screen("Speed up the build");
    ui.press("\x13");
    let both = wait_for(
        || ui.screen_text(),
        |frame| {
            frame.contains("Speed up the build") && frame.contains("● Tokens") && !frame.contains("New orchestrator")
        },
        Duration::from_secs(15),
    );
    assert!(both.matches("◎ ").count() >= 2, "{both}");
}
