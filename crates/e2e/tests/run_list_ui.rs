//! The run list (r) in a real PTY, driving the isolated dev launcher.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use deckhand_e2e::harness::{Sandbox, launcher, wait_for};
use deckhand_e2e::ui::{Ui, UiOptions};

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

fn path_with(bin: &Path) -> String {
    let existing = std::env::var("PATH").unwrap_or_default();
    format!("{}:{existing}", bin.display())
}

// ported from tests/runListUi.test.ts: x in the run list stops the running action from the Dev row too
#[test]
fn x_in_the_run_list_stops_the_running_action_from_the_dev_row_too() {
    let sandbox = Sandbox::new("ui");
    let home = sandbox.dev_home();
    let bin = home.join("bin");
    install_fake_agents(&bin);
    fs::write(
        home.join("config.json"),
        r#"{"defaults":{"actions":{"slow":"echo slow-started; sleep 60"}}}"#,
    )
    .expect("config");
    let path = path_with(&bin);
    let launcher_script = launcher().display().to_string();
    let mut ui = Ui::spawn(
        &sandbox,
        UiOptions {
            program: Path::new("/bin/sh"),
            args: &[launcher_script.as_str(), "--sandbox"],
            cwd: &home,
            env: &[("PATH", &path)],
            dev: true,
        },
    );
    ui.screen("DEV (isolated)");
    ui.screen("● ready");
    ui.press("n");
    ui.screen("Choose an agent");
    ui.press("\r");
    ui.screen("Workspace: new worktree");
    ui.press("runs");
    ui.screen("Name: runs");
    ui.press("\r");
    ui.screen("fake agent ready");
    ui.press("r");
    ui.screen("○ stopped");
    ui.press("j");
    ui.screen("Selected command");
    ui.press("\r");
    ui.screen("│ slow-started"); // its output in the pane, not the command line in the run list
    // Its output shows while starting it still holds the UI busy (keys are dropped then): wait for the latest frame to
    // be idle before pressing r again: back on the session (its footer drawn, so not a half-drawn frame) without Working…,
    // which is drawn below that footer.
    wait_for(
        || ui.screen_text(),
        |frame| frame.contains("q quit") && !frame.contains("Working…"),
        Duration::from_secs(30),
    );
    // Back in the run list, on the Dev row: the running action is named. On its own row x twice stops it.
    ui.press("r");
    ui.screen("slow is still running in this worktree: x twice on its row stops it.");
    ui.press("j");
    ui.screen("x stop slow");
    ui.press("x");
    ui.screen("x again stops slow");
    ui.press("x");
    ui.screen("Stopped slow");
}
