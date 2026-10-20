//! Port of tests/nodeEnvUi.test.ts.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use deckhand_e2e::harness::{Sandbox, launcher};
use deckhand_e2e::ui::{Ui, UiOptions};

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

// The UI sets NODE_ENV=production only while React loads (its fast build). It must not reach what the UI starts:
// the daemon, and through it every agent, shell, Dev command and action (npm there would skip devDependencies).
// ported from tests/nodeEnvUi.test.ts: NODE_ENV=production set for the UI never reaches agents or actions
#[test]
fn node_env_production_set_for_the_ui_never_reaches_agents_or_actions() {
    let sandbox = Sandbox::new("env");
    // A stand-in agent that prints the NODE_ENV it was started with.
    let bin = install_fakes(&sandbox, "echo \"agent node-env=[${NODE_ENV-}]\"");
    fs::write(
        sandbox.dev_home().join("config.json"),
        r#"{"defaults":{"actions":{"env":"echo \"action node-env=[$NODE_ENV]\""}}}"#,
    )
    .expect("write config");
    let mut ui = start_ui(&sandbox, &bin);
    ui.screen("DEV (isolated)");
    ui.screen("● ready");
    ui.press("n");
    ui.screen("Choose an agent");
    ui.press("\r");
    ui.screen("Workspace: new worktree");
    ui.press("envs");
    ui.screen("Name: envs");
    ui.press("\r");
    ui.screen("agent node-env=[]");
    // r: Dev first, then the global env action.
    ui.press("r");
    ui.screen("○ stopped");
    ui.press("j");
    ui.screen("Selected command");
    ui.press("\r");
    ui.screen("action node-env=[]");
}
