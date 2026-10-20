//! Port of tests/mergeUi.test.ts.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use deckhand_e2e::harness::{Sandbox, git, launcher, wait_for};
use deckhand_e2e::ui::{UI_WAIT, Ui, UiOptions};

/// A stand-in agent (never a real one): stays alive and prints a marker.
fn fake_agent_script(ready: &str) -> String {
    format!(
        "#!/bin/sh\ncase \" $* \" in\n*\" --help \"*) echo 'PROMPT --settings --no-daemon resume'; exit 0;;\n*\" --version \"*) echo '1.0.0'; exit 0;;\nesac\n{ready}\nexec sleep 100000\n"
    )
}

/// claude, pi and codex stand-ins in `<root>/agents`; returns that directory.
fn install_fakes(sandbox: &Sandbox, ready: &str) -> PathBuf {
    let bin = sandbox.root().join("agents");
    fs::create_dir_all(&bin).expect("agents dir");
    for agent in ["claude", "pi", "codex"] {
        let file = bin.join(agent);
        fs::write(&file, fake_agent_script(ready)).expect("write fake agent");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    bin
}

/// The isolated dev launcher (`--sandbox`) in a PTY, with the fakes first on PATH.
fn start_ui(sandbox: &Sandbox, bin: &Path) -> Ui {
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
    // The launcher is not marked executable: run it through sh, as dev_launcher.rs does.
    let launcher = launcher();
    let launcher = launcher.to_string_lossy();
    Ui::spawn(
        sandbox,
        UiOptions {
            program: Path::new("/bin/sh"),
            args: &[&launcher, "--sandbox"],
            cwd: &sandbox.dev_home(),
            env: &[("PATH", &path)],
            dev: true,
        },
    )
}

// ported from tests/mergeUi.test.ts: m in a real PTY: preview, space toggles commit-first, squash; space marks done (☑); a conflict is aborted with x, then kept with enter
#[test]
fn merge_in_a_pty_preview_squash_mark_done_and_conflict() {
    let sandbox = Sandbox::new("mrg");
    let bin = install_fakes(&sandbox, "echo 'fake agent ready'");
    let mut ui = start_ui(&sandbox, &bin);
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
