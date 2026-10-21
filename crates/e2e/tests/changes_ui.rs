//! The Git tab's Changes view in a real PTY.

use std::fs;

use deckhand_e2e::fakes::install_sandbox_agents;
use deckhand_e2e::harness::{Sandbox, git, wait_for};
use deckhand_e2e::ui::{UI_WAIT, launch_sandbox_ui};

// ported from tests/changesUi.test.ts: Git tab: the worktree's Changes list; v focuses it, space stages the selected file, the diff previews it, esc returns
#[test]
fn git_tab_changes_list_focus_stage_diff_and_return() {
    let sandbox = Sandbox::new("chg");
    let bin = install_sandbox_agents(&sandbox, "echo 'fake agent ready'");
    let mut ui = launch_sandbox_ui(&sandbox, &bin, &[]);
    let home = sandbox.dev_home();
    ui.screen("DEV (isolated)");
    ui.screen("● ready");
    // The sandbox preselects a new worktree.
    ui.press("n");
    ui.screen("Choose an agent");
    ui.press("\r");
    ui.screen("Workspace: new worktree");
    ui.press("chg");
    ui.screen("Name: chg");
    ui.press("\r");
    ui.screen("fake agent ready");
    let worktree = home.join("worktrees").join("chg");
    fs::write(
        worktree.join("README.md"),
        "# Deckhand dev sandbox\n\nEdited in the worktree.\n",
    )
    .expect("edit README");
    fs::write(worktree.join("notes.md"), "scratch\n").expect("write notes");

    // Browse: the list only (read-only), grouped like VS Code, with the keys in the footer.
    ui.press("g");
    ui.screen("→ changes • enter lazygit");
    ui.screen("Changes 1");
    ui.screen("Untracked 1");
    ui.screen("README.md");
    ui.screen("1 changed · 1 untracked");
    // Focus: the selection starts on the first file; its diff shows below the list.
    ui.press("\x1b[C");
    ui.screen("space stage/unstage");
    ui.screen("unstaged · README.md");
    ui.screen("+Edited in the worktree.");
    ui.press(" ");
    ui.screen("Staged README.md");
    ui.screen("Staged Changes 1");
    wait_for(
        || git(&worktree, &["diff", "--cached", "--name-only"]),
        |names| names == "README.md",
        UI_WAIT,
    );
    // The selection moved on to the untracked file; J scrolls nothing it cannot, esc returns to browse.
    ui.screen("untracked · notes.md");
    ui.press("\x1b");
    ui.screen("→ changes • enter lazygit");
    assert_eq!(git(&worktree, &["status", "--porcelain"]), "M  README.md\n?? notes.md");
}
