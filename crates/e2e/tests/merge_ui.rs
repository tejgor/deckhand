//! Merging from the UI in a real PTY.

use std::fs;

use deckhand_e2e::fakes::install_sandbox_agents;
use deckhand_e2e::harness::{Sandbox, git, wait_for};
use deckhand_e2e::ui::{UI_WAIT, launch_sandbox_ui};

// ported from tests/mergeUi.test.ts: m in a real PTY: preview, space toggles commit-first, squash; space marks done (☑); a conflict is aborted with x, then kept with enter
#[test]
fn merge_in_a_pty_preview_squash_mark_done_and_conflict() {
    let sandbox = Sandbox::new("mrg");
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
    ui.press("mrg");
    ui.screen("Name: mrg");
    ui.press("\r");
    ui.screen("fake agent ready");
    let worktree = home.join("worktrees").join("mrg");
    let main = home.join("sandbox");
    fs::write(worktree.join("feature.txt"), "feature\n").expect("write feature");

    // Only uncommitted work: the preview says so, the toggle is on; space turns it off and on again.
    ui.press("m");
    ui.screen("No new commits · +1 uncommitted file");
    ui.screen("☑ commit them first (\"mrg\")");
    ui.press(" ");
    ui.screen("☐ commit them first (\"mrg\") · they stay in the worktree");
    ui.press(" ");
    ui.screen("☑ commit them first (\"mrg\")");
    // Keys are sent one at a time (a chunk of two would arrive as one input).
    ui.press("j");
    ui.screen("❯ Squash merge into main");
    ui.press("\r");
    ui.screen("Committed 1 file, then squash applied from mrg into main");
    assert_eq!(git(&worktree, &["log", "-1", "--format=%s"]), "mrg");
    assert_eq!(git(&main, &["diff", "--cached", "--name-only"]), "feature.txt");
    git(&main, &["commit", "-m", "squashed mrg"]);
    ui.press(" ");
    ui.screen("Marked done: mrg");
    ui.screen("✓ ☑ ✶");

    // Both sides change README.md: the conflict view; x aborts, then enter keeps it (in progress, marked merged).
    fs::write(worktree.join("README.md"), "worktree side\n").expect("worktree readme");
    git(&worktree, &["commit", "-am", "worktree readme"]);
    fs::write(main.join("README.md"), "main side\n").expect("main readme");
    git(&main, &["commit", "-am", "main readme"]);
    ui.press("m");
    ui.screen("2 commits");
    ui.press("\r");
    ui.screen("Merged with conflicts in 1 file");
    ui.screen("x     abort the merge");
    ui.press("x");
    ui.screen("Merge aborted");
    assert_eq!(git(&main, &["status", "--porcelain"]), "");
    ui.press("m");
    ui.screen("2 commits");
    ui.press("\r");
    ui.screen("Merged with conflicts in 1 file");
    ui.press("\r");
    ui.screen("Merge kept with conflicts in 1 file into main");
    wait_for(
        || git(&main, &["status", "--porcelain"]),
        |status| status.lines().any(|line| line == "UU README.md"),
        UI_WAIT,
    );
}
