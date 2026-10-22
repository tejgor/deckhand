//! The Worktrees screen (W) in a real PTY, driving the isolated dev launcher.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use deckhand_e2e::harness::{Sandbox, git, launcher, wait_for};
use deckhand_e2e::ui::{Ui, UiOptions};

const UI_WAIT: Duration = Duration::from_secs(30);

/// A stand-in agent (never a real one): stays alive and prints a marker. Installs claude, pi and codex in `dir`.
fn install_fake_agents(dir: &Path) {
    fs::create_dir_all(dir).expect("agent bin dir");
    let script = "#!/bin/sh\n\
        for arg in \"$@\"; do\n\
        \tcase \"$arg\" in\n\
        \t--help) echo 'PROMPT --settings --no-daemon resume'; exit 0;;\n\
        \t--version) echo '1.0.0'; exit 0;;\n\
        \tesac\n\
        done\n\
        echo 'fake agent ready'\n\
        exec sleep 100000\n";
    for agent in ["claude", "pi", "codex"] {
        let file = dir.join(agent);
        fs::write(&file, script).expect("write agent");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).expect("chmod agent");
    }
}

/// The dev launcher in a PTY with the fake agents first on PATH.
fn launch_with_agents(sandbox: &Sandbox) -> Ui {
    let home = sandbox.dev_home();
    let bin = home.join("bin");
    install_fake_agents(&bin);
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
    let script = launcher().display().to_string();
    Ui::spawn(
        sandbox,
        UiOptions {
            program: Path::new("/bin/sh"),
            args: &[script.as_str(), "--sandbox"],
            cwd: &home,
            env: &[("PATH", path.as_str())],
            dev: true,
        },
    )
}

// ported from tests/worktreesUi.test.ts: W in a real PTY: merged worktrees grouped, x on the group deletes them and stops their sessions, DELETE overrides data loss
#[test]
fn w_merged_worktrees_grouped_x_deletes_and_delete_overrides_data_loss() {
    let sandbox = Sandbox::new("ui");
    let home = sandbox.dev_home();
    let mut ui = launch_with_agents(&sandbox);
    ui.screen("DEV (isolated)");
    ui.screen("● ready");
    // A session in a new worktree whose branch is merged into main by hand while its agent runs.
    ui.press("n");
    ui.screen("Choose an agent");
    ui.press("\r");
    ui.screen("Workspace: new worktree");
    ui.press("shipped");
    ui.screen("Name: shipped");
    ui.press("\r");
    ui.screen("fake agent ready");
    let worktree = home.join("worktrees/shipped");
    let sandbox_repo = home.join("sandbox");
    let branch = git(&worktree, &["branch", "--show-current"]);
    fs::write(worktree.join("feature.txt"), "feature\n").expect("feature.txt");
    git(&worktree, &["add", "."]);
    git(&worktree, &["commit", "-m", "feature"]);
    git(&sandbox_repo, &["merge", "--no-ff", "-m", "merge shipped", &branch]);
    // A worktree made with git, with a file only it has.
    let scratch = home.join("scratch");
    let scratch_text = scratch.display().to_string();
    git(&sandbox_repo, &["worktree", "add", "-b", "scratch", &scratch_text]);
    fs::write(scratch.join("notes.txt"), "only here\n").expect("notes.txt");
    fs::write(scratch.join("README.md"), "changed here\n").expect("README.md");

    // Its worktree note has an open item left (Notes tab, o to edit, Ctrl+T adds an item).
    ui.press("a");
    ui.screen("enter edit notes");
    ui.press("\r");
    ui.screen("esc done");
    ui.press("\x14");
    ui.screen("- [ ]");
    ui.press("Follow up on perf");
    ui.screen("Follow up on perf");
    ui.press("\x1b");
    ui.screen("enter edit notes");

    // The session list's footer lists it.
    ui.press("\x1b[A");
    ui.screen("W worktrees");
    ui.press("W");
    ui.screen("⎇ Worktrees");
    ui.screen("Merged · safe to delete · 1");
    ui.screen("Main checkout");
    ui.screen("scratch");
    ui.screen("not from Deckhand");
    ui.screen("☐1");
    // The selection starts on the selected session's worktree; its details say why it can go.
    ui.screen("Clean: deleting it and its branch loses nothing");
    ui.press("k");
    ui.screen("x deletes them all");
    ui.press("x");
    ui.screen("Delete 1 merged worktree?");
    ui.screen("Stops the session running there first: shipped");
    ui.screen("Stop 1 session, delete 1 worktree and their branches");
    // Its note's open item goes to the backlog first; space would keep it in the note.
    ui.screen("☑ 1 open note item → backlog first: Follow up on perf");
    ui.press(" ");
    ui.screen("☐ 1 open note item stay in the (read-only) note");
    ui.press(" ");
    ui.screen("☑ 1 open note item → backlog first");
    ui.press("\r");
    ui.screen("Deleted 1 worktree and 1 branch · stopped 1 session · 1 open note item to the backlog");
    wait_for(|| worktree.exists(), |exists| !exists, UI_WAIT);
    assert!(
        !git(&sandbox_repo, &["branch", "--list", &branch]).contains(&branch),
        "branch {branch} still listed"
    );

    // Not merged, with an untracked file: keeping the branch is offered first, and the file needs DELETE typed.
    // The main checkout is last; scratch is in progress above it.
    ui.press("G");
    ui.screen("› ◆ main");
    ui.press("k");
    ui.screen("› ○ scratch");
    // The confirmation names what would be lost: each reason on its own line, then the files themselves.
    ui.press("x");
    ui.screen("Delete scratch?");
    ui.screen("Deleting it would lose (typing DELETE overrides):");
    ui.screen("· 1 modified/staged file(s)");
    ui.screen("· 1 untracked file(s)");
    ui.screen("Files (M changed · ? untracked · ! ignored):");
    ui.screen("M README.md");
    ui.screen("? notes.txt");
    ui.screen("Delete the worktree, keep its branch");
    ui.press("\r");
    ui.screen("Delete scratch anyway?");
    ui.screen("? notes.txt");
    ui.screen("Type DELETE then enter:");
    ui.press("DELE");
    ui.screen("Type DELETE then enter: DELE");
    ui.press("TE");
    ui.screen("Type DELETE then enter: DELETE");
    ui.press("\r");
    ui.screen("Deleted scratch");
    wait_for(|| scratch.exists(), |exists| !exists, UI_WAIT);
    assert!(git(&sandbox_repo, &["branch", "--list", "scratch"]).contains("scratch"));
    ui.screen("No other worktrees");
    // Its session was stopped and archived: back in the list, f A shows it.
    ui.press("\x1b");
    ui.screen("n new • W worktrees");
    ui.screen("Sessions");
    ui.press("f");
    ui.press("A");
    ui.screen("shipped");
    ui.screen("backspace remove");
    // Removing it for good asks even with nothing open in its notes; Esc keeps it, Enter removes it.
    ui.press("\x7f");
    ui.screen("Remove “shipped” for good?");
    ui.screen("enter remove • esc keep it");
    ui.press("\x1b");
    ui.screen("backspace remove");
    ui.press("\x7f");
    ui.screen("for good?");
    ui.press("\r");
    ui.screen("No sessions");
}
