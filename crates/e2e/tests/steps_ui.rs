//! Steps are lines under a session's line: added and ticked on the Tasks board, made on demand for a session started
//! from the sidebar, and a backlog line moved onto a session (w), in a real PTY driving the isolated dev launcher.

use std::fs;
use std::path::PathBuf;

use deckhand_e2e::fakes::{READY, install_sh_agents};
use deckhand_e2e::harness::{Sandbox, wait_for};
use deckhand_e2e::ui::{UI_WAIT, Ui, launch_sandbox_ui};

/// The dev launcher in a PTY with the fake agents (in `<dev home>/bin`) first on PATH.
fn launch_with_agents(sandbox: &Sandbox) -> Ui {
    let bin = install_sh_agents(&sandbox.dev_home().join("bin"), READY);
    launch_sandbox_ui(sandbox, &bin, &[])
}

/// The only `.md` file in `directory` (empty text unless there is exactly one).
fn only_note(directory: PathBuf) -> String {
    let names: Vec<_> = fs::read_dir(directory)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
                .collect()
        })
        .unwrap_or_default();
    match names.as_slice() {
        [one] => fs::read_to_string(one).unwrap_or_default(),
        _ => String::new(),
    }
}

/// A session started from the sidebar, named `name`, in a new worktree.
fn start_from_sidebar(ui: &mut Ui, name: &str) {
    ui.press("n");
    ui.screen("Choose an agent");
    ui.press("\r");
    ui.screen("Workspace: new worktree");
    ui.press(name);
    ui.screen(&format!("Name: {name}"));
    ui.press("\r");
    ui.screen("fake agent ready");
}

// A session started from the sidebar has no line until it needs one: v on the board offers to add a step, which makes
// its line (named after it) with the step under it; space ticks the step, and its line rolls up.
#[test]
fn a_step_on_the_board_makes_the_sessions_line_and_space_ticks_it() {
    let sandbox = Sandbox::new("ui");
    let home = sandbox.dev_home();
    let mut ui = launch_with_agents(&sandbox);
    let list = || only_note(home.join("notes/tasks"));
    ui.screen("DEV (isolated)");
    ui.screen("● ready");
    start_from_sidebar(&mut ui, "flaky");

    ui.press("b");
    ui.screen("No lines yet · a adds one");
    ui.press("v");
    ui.screen("No line for this session yet · a adds a step (it gets its line)");
    ui.press("a");
    ui.screen("a step of the session's line (made now)");
    ui.press("Reproduce it");
    ui.screen("Reproduce it");
    ui.press("\r");
    ui.screen("Added a step to the session's line");
    ui.screen("☐ flaky");
    ui.screen("  ☐ Reproduce it");
    ui.screen("0/1");
    let text = wait_for(
        &list,
        |text| text.contains("\n  - [ ] Reproduce it <!-- dh:t="),
        UI_WAIT,
    );
    assert!(text.starts_with("- [ ] flaky <!-- dh:t="), "list: {text:?}");
    assert!(
        text.lines().next().is_some_and(|line| line.contains(" s=")),
        "list: {text:?}"
    );

    // space ticks the step; the last step ticked ticks its line too (it can be reopened).
    ui.screen("›   ☐ Reproduce it");
    ui.screen("space ✓");
    ui.press(" ");
    ui.screen("Done: Reproduce it");
    ui.screen("☑ Reproduce it");
    ui.screen("1/1");
    wait_for(&list, |text| text.contains("- [x] Reproduce it"), UI_WAIT);
    // The session's Notes tab banner counts its line's lines.
    ui.press("\x1b");
    ui.screen("x stop");
    ui.press("a");
    ui.screen("1/1 steps");
}

// w moves a backlog line onto a session without a line (it becomes its line, ◆) and then a adds a step under it.
#[test]
fn w_onto_a_session_makes_the_line_its_own_and_a_adds_a_step() {
    let sandbox = Sandbox::new("ui");
    let home = sandbox.dev_home();
    let mut ui = launch_with_agents(&sandbox);
    let list = || only_note(home.join("notes/tasks"));
    ui.screen("DEV (isolated)");
    ui.screen("● ready");
    start_from_sidebar(&mut ui, "flaky");

    // A line written afterwards, moved there with w: the session's own line, the row says so.
    ui.press("b");
    ui.screen("No lines yet · a adds one");
    ui.press("a");
    ui.screen("New line");
    ui.press("Fix flaky test");
    ui.screen("Fix flaky test");
    ui.press("\r");
    ui.screen("Added to the backlog");
    ui.press("w");
    ui.screen("Move “Fix flaky test” to");
    ui.press("\t");
    ui.screen("Sessions 1");
    ui.screen("◆ its line");
    ui.press("j");
    ui.screen("› flaky");
    ui.press("\r");
    ui.screen("Now the line of flaky");
    ui.screen("flaky ● idle");
    wait_for(
        &list,
        |text| text.contains("- [ ] Fix flaky test <!-- dh:t=") && text.contains(" s="),
        UI_WAIT,
    );
    // a under it: a step of that session's line.
    ui.press("a");
    ui.screen("New line · under “Fix flaky test”");
    ui.press("Write docs");
    ui.screen("Write docs");
    ui.press("\r");
    ui.screen("Added under “Fix flaky test”");
    ui.screen("  ☐ Write docs");
    ui.screen("0/1");
    wait_for(&list, |text| text.contains("\n  - [ ] Write docs <!-- dh:t="), UI_WAIT);
    // n on that step: a sub-session of flaky (the form says whose).
    ui.press("n");
    ui.screen("Choose an agent");
    ui.press("\x1b");
    ui.screen("☐ Tasks");
}
