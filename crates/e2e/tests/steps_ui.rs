//! Checklist steps in worktree notes, the Tasks board and A (send open items), in a real PTY driving the isolated dev
//! launcher.

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

/// `text` contains `prefix` + 8 lowercase hex digits + `suffix`.
fn has_hex8(text: &str, prefix: &str, suffix: &str) -> bool {
    text.match_indices(prefix).any(|(at, _)| {
        let rest = &text[at + prefix.len()..];
        rest.len() >= 8
            && rest.as_bytes()[..8]
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
            && rest[8..].starts_with(suffix)
    })
}

// ported from tests/stepsUi.test.ts: steps: the worktree note's checklist under its task on the board (space ticks one), A sends a note's open items at once
#[test]
fn steps_checklist_under_its_task_on_the_board_and_a_sends_open_items() {
    let sandbox = Sandbox::new("ui");
    let home = sandbox.dev_home();
    let mut ui = launch_with_agents(&sandbox);
    let worktree_note = || only_note(home.join("notes/worktrees"));
    ui.screen("DEV (isolated)");
    ui.screen("● ready");
    // A task, and a session started for it in a new worktree.
    ui.press("b");
    ui.screen("No tasks yet · a adds one");
    ui.press("a");
    ui.screen("New task");
    ui.press("Ship parser");
    ui.screen("Ship parser");
    ui.press("\r");
    ui.screen("Added to the backlog");
    ui.press("n");
    ui.screen("Choose an agent");
    ui.press("\r");
    ui.screen("Workspace: new worktree");
    ui.press("\r");
    ui.screen("fake agent ready");
    // Its worktree note gets a checklist (Notes tab, o to edit; Enter continues the list).
    ui.press("a");
    ui.screen("enter edit notes");
    ui.press("\r");
    ui.screen("esc done");
    ui.press("\x14");
    ui.screen("- [ ]");
    ui.press("Read the spec");
    ui.screen("Read the spec");
    ui.press("\r");
    ui.screen("- [ ] ");
    ui.press("Write tests");
    ui.screen("Write tests");
    ui.press("\x1b");
    // Wait for editing to end, not only for the file (the debounced save may already have written it): a key sent
    // in the same read as the Esc would arrive as Alt+key.
    ui.screen("enter edit notes");
    wait_for(&worktree_note, |text| text.contains("- [ ] Write tests"), UI_WAIT);

    // The board: the steps under the task, counted; space on one ticks it in the note.
    ui.press("b");
    ui.screen("◆ Ship parser");
    ui.screen("0/2");
    ui.screen("☐ Read the spec");
    ui.screen("☐ Write tests");
    ui.press("j");
    ui.screen("A step: a checklist item in the note of");
    ui.screen("space tick");
    ui.press(" ");
    ui.screen("Ticked: Read the spec");
    ui.screen("1/2");
    ui.screen("☑ Read the spec");
    wait_for(&worktree_note, |text| text.contains("- [x] Read the spec"), UI_WAIT);
    // The session's Notes tab banner counts them too.
    ui.press("\x1b");
    ui.screen("1/2 steps");

    // The Notes view: A sends every open item of the selected note to Tasks; the note keeps a ↗ link each.
    ui.press("b");
    ui.screen("◆ Ship parser");
    ui.press("\t");
    ui.screen("✎ Notes");
    ui.screen("A send all");
    ui.press("A");
    ui.screen("Sent 1 item to Tasks");
    ui.screen("↗ Write tests · in Tasks");
    assert!(
        has_hex8(&worktree_note(), "- ↗ Write tests <!-- dh:t=", " -->"),
        "note: {}",
        worktree_note()
    );
    // Its task is in the backlog (work of its own); ticking it shows · done on the ↗ line.
    ui.press("\t");
    ui.screen("BACKLOG · 1");
    ui.press("G");
    ui.screen("› ○ Write tests");
    ui.press(" ");
    ui.screen("Done: Write tests");
    ui.press("\t");
    ui.screen("↗ Write tests · done");
}

// ported from tests/stepsUi.test.ts: w onto a session started from the sidebar makes the task its own (◆, with steps); a there then adds a step
#[test]
fn w_onto_a_sidebar_session_makes_the_task_its_own_and_a_adds_a_step() {
    let sandbox = Sandbox::new("ui");
    let mut ui = launch_with_agents(&sandbox);
    ui.screen("DEV (isolated)");
    ui.screen("● ready");
    // A session started from the sidebar, in a new worktree, with a checklist in its note: no task yet.
    ui.press("n");
    ui.screen("Choose an agent");
    ui.press("\r");
    ui.screen("Workspace: new worktree");
    ui.press("flaky");
    ui.screen("Name: flaky");
    ui.press("\r");
    ui.screen("fake agent ready");
    ui.press("a");
    ui.screen("enter edit notes");
    ui.press("\r");
    ui.screen("esc done");
    ui.press("\x14");
    ui.screen("- [ ]");
    ui.press("Reproduce it");
    ui.screen("Reproduce it");
    ui.press("\x1b");
    ui.screen("enter edit notes");

    // A task written afterwards, moved there with w: as the work's own task by default (the row says so).
    ui.press("b");
    ui.screen("No tasks yet · a adds one");
    ui.press("a");
    ui.screen("New task");
    ui.press("Fix flaky test");
    ui.screen("Fix flaky test");
    ui.press("\r");
    ui.screen("Added to the backlog");
    ui.press("w");
    ui.screen("Move “Fix flaky test” to");
    ui.screen("◆ its task");
    ui.press("\r");
    ui.screen("Now the task of ⎇");
    ui.screen("◆ Fix flaky test");
    ui.screen("0/1");
    ui.screen("☐ Reproduce it");
    // The work has its task now: a adds a step to its note.
    ui.press("a");
    ui.screen("New step · the note of ⎇");
    ui.press("Write docs");
    ui.screen("Write docs");
    ui.press("\r");
    ui.screen("Added a step to");
    ui.screen("☐ Write docs");
    ui.screen("0/2");
    // G: the empty backlog (selectable, so a adds there) is last; the last step is just above it.
    ui.press("G");
    ui.screen("› Nothing waiting");
    ui.press("k");
    ui.screen("›   ☐ Write docs");
    ui.press("w");
    ui.screen("A step from the note");
}
