//! Messages between sessions in the UI, in a real PTY: a message sent by hand to a session that shows a menu is held
//! (never typed into a menu), so its row says `✉1`; `e` lists it, and `x` twice cancels it.

use std::time::Duration;

use deckhand_e2e::fakes::install_sandbox_agents;
use deckhand_e2e::harness::{Sandbox, wait_for};
use deckhand_e2e::ui::{Ui, launch_sandbox_ui};
use serde_json::Value;

/// `deckhand <args>` by hand through the dev launcher (against the UI's daemon); its stdout, which must succeed.
fn by_hand(sandbox: &Sandbox, args: &[&str]) -> String {
    let mut all = vec!["--sandbox"];
    all.extend_from_slice(args);
    let output = sandbox.exec_launcher(&all, Duration::from_secs(60));
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

/// Waits until the session list is idle (its footer drawn, nothing being started).
fn settled(ui: &Ui) {
    wait_for(
        || ui.screen_text(),
        |frame| frame.contains("q quit") && !frame.contains("Working…") && !frame.contains("starting"),
        Duration::from_secs(15),
    );
}

#[test]
fn a_held_message_marks_its_session_and_e_lists_it_and_x_twice_cancels_it() {
    let sandbox = Sandbox::new("messages-ui");
    // An agent that shows a menu: a message waits until it is gone, so it stays held.
    let bin = install_sandbox_agents(&sandbox, "printf 'Pick one\\n❯ 1. Yes\\n  2. No\\n'");
    let mut ui = launch_sandbox_ui(&sandbox, &bin, &[]);
    ui.screen("DEV (isolated)");
    ui.screen("● ready");
    ui.press("n");
    ui.screen("Choose an agent");
    ui.press("\r");
    ui.screen("Workspace: new worktree");
    ui.press("tokens");
    ui.screen("Name: tokens");
    ui.press("\r");
    ui.screen("Pick one");
    settled(&ui);

    let listed: Value = serde_json::from_str(&by_hand(&sandbox, &["ls", "--json"])).expect("ls --json");
    let id = listed
        .as_array()
        .and_then(|sessions| sessions.iter().find(|session| session["title"] == "tokens"))
        .and_then(|session| session["id"].as_str())
        .unwrap_or_else(|| panic!("the session: {listed}"))
        .to_owned();
    let sent: Value = serde_json::from_str(&by_hand(
        &sandbox,
        &["send", &id, "Rebase on main, then run the tests.", "--json"],
    ))
    .expect("send --json");
    assert_eq!(sent["message"]["status"], "held", "{sent}");
    let message = sent["message"]["id"].as_str().unwrap_or_default().to_owned();

    // The row and the details say it waits.
    let frame = ui.screen("✉1");
    assert!(frame.contains("✉ 1 waiting · last from you"), "{frame}");

    // e lists it: from you, held, with its text and ID.
    ui.press("e");
    let view = ui.screen("Messages · tokens");
    assert!(view.contains("› ← you · "), "{view}");
    assert!(view.contains("held"), "{view}");
    assert!(view.contains(&message), "{view}");
    assert!(view.contains("Rebase on main, then run the tests."), "{view}");
    assert!(view.contains("x x cancel it"), "{view}");

    // x twice cancels it; the marker goes.
    ui.press("x");
    ui.screen(&format!("x again cancels message {message} from you"));
    ui.press("x");
    ui.screen(&format!("Cancelled message {message}"));
    let cancelled = ui.screen("cancelled");
    assert!(!cancelled.contains("x x cancel it"), "{cancelled}");
    ui.press("\x1b");
    let back = wait_for(
        || ui.screen_text(),
        |frame| frame.contains("q quit") && !frame.contains("Messages · tokens"),
        Duration::from_secs(10),
    );
    assert!(!back.contains("✉1"), "{back}");
    assert!(back.contains("✉ last from you"), "{back}");
}
