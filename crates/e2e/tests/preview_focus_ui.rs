//! Typing to an agent in its Preview (→), without attaching, in a real PTY driving the isolated dev launcher: keys
//! reach the agent as typed (a lone Esc included), and Ctrl+] returns to the session list without sending itself.

use std::fs;
use std::time::Duration;

use deckhand_e2e::fakes::install_sh_agents;
use deckhand_e2e::harness::{Sandbox, wait_for};
use deckhand_e2e::ui::{UI_WAIT, launch_sandbox_ui};

#[test]
fn keys_typed_in_the_preview_reach_the_agent_and_ctrl_bracket_returns() {
    let sandbox = Sandbox::new("preview-focus");
    let home = sandbox.dev_home();
    let log = sandbox.root().join("agent-input.log");
    // The stand-in agent logs every byte its terminal gets, unechoed and unprocessed.
    let ready = format!(
        "echo 'fake agent ready'\nstty raw -echo\nexec cat > '{}'",
        log.display()
    );
    let bin = install_sh_agents(&home.join("bin"), &ready);
    let mut ui = launch_sandbox_ui(&sandbox, &bin, &[]);
    ui.screen("DEV (isolated)");
    ui.screen("● ready");
    ui.press("n");
    ui.screen("Choose an agent");
    ui.press("\r");
    ui.screen("Workspace: new worktree");
    ui.press("typer");
    ui.screen("Name: typer");
    ui.press("\r");
    ui.screen("fake agent ready");
    // → steps in only once the session runs.
    wait_for(
        || ui.screen_text(),
        |frame| frame.contains("→ type • enter open"),
        Duration::from_secs(10),
    );
    ui.press("\x1b[C");
    ui.screen("typing to the agent");
    ui.screen("ctrl+] back");
    let input = || fs::read_to_string(&log).unwrap_or_default();
    ui.press("hi");
    wait_for(&input, |text| text == "hi", UI_WAIT);
    // A lone Esc goes to the agent rather than stepping out.
    ui.press("\x1b");
    wait_for(&input, |text| text == "hi\x1b", UI_WAIT);
    ui.screen("typing to the agent");
    ui.press("\x1d");
    wait_for(
        || ui.screen_text(),
        |frame| frame.contains("→ type • enter open") && !frame.contains("typing to the agent"),
        UI_WAIT,
    );
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(input(), "hi\x1b", "Ctrl+] is not sent");
}
