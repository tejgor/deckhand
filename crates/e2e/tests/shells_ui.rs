//! Several shells per worktree, in a real PTY driving the isolated dev launcher: the run list opens another, → types
//! into it without attaching, v switches between them, and x in the run list closes the extra one.

use std::time::Duration;

use deckhand_e2e::fakes::{READY, install_sh_agents};
use deckhand_e2e::harness::{Sandbox, wait_for};
use deckhand_e2e::ui::{UI_WAIT, launch_sandbox_ui};

#[test]
fn a_second_shell_opens_takes_keys_without_attaching_and_closes() {
    let sandbox = Sandbox::new("shells");
    let bin = install_sh_agents(&sandbox.dev_home().join("bin"), READY);
    let mut ui = launch_sandbox_ui(&sandbox, &bin, &[]);
    ui.screen("DEV (isolated)");
    ui.screen("● ready");
    ui.press("n");
    ui.screen("Choose an agent");
    ui.press("\r");
    ui.screen("Workspace: new worktree");
    ui.press("shells");
    ui.screen("Name: shells");
    ui.press("\r");
    ui.screen("fake agent ready");
    wait_for(
        || ui.screen_text(),
        |frame| frame.contains("→ type • enter open") && !frame.contains("preparing workspace"),
        Duration::from_secs(10),
    );

    // The Terminal tab's shell, then a second one from the run list (its last row).
    ui.press("t");
    ui.screen_within("● live", UI_WAIT);
    ui.press("r");
    ui.screen("+ New shell");
    ui.press("G");
    ui.screen("enter open a shell");
    ui.press("\r");
    ui.screen("Opened shell 2");
    ui.screen("shell 1 │ shell 2");

    // → types into it without attaching.
    ui.press("\x1b[C");
    ui.screen("typing into shell 2");
    ui.press("echo two-$((1+1))\r");
    ui.screen("two-2");
    ui.press("\x1d");
    wait_for(
        || ui.screen_text(),
        |frame| frame.contains("→ type • enter open") && !frame.contains("typing into"),
        UI_WAIT,
    );

    // v shows the first shell, which never saw the command.
    ui.press("v");
    wait_for(&|| ui.screen_text(), |frame| !frame.contains("two-2"), UI_WAIT);

    // x twice on its run-list row closes the second shell.
    ui.press("r");
    ui.screen("Shell 2");
    ui.press("G");
    ui.screen("enter open a shell");
    ui.press("k");
    ui.screen("x close shell 2");
    ui.press("x");
    ui.screen("x again closes shell 2");
    ui.press("x");
    ui.screen("Closed shell 2");
    ui.press("\x1b");
    wait_for(&|| ui.screen_text(), |frame| !frame.contains("shell 2"), UI_WAIT);
}
