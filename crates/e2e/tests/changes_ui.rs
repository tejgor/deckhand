//! Port of tests/changesUi.test.ts.

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

// ported from tests/changesUi.test.ts: Git tab: the worktree's Changes list; v focuses it, space stages the selected file, the diff previews it, esc returns
#[test]
fn git_tab_changes_list_focus_stage_diff_and_return() {
    let sandbox = Sandbox::new("chg");
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
