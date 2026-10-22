//! The environment agents and actions get (NODE_ENV and friends), in a real PTY.

use std::fs;

use deckhand_e2e::fakes::install_sandbox_agents;
use deckhand_e2e::harness::Sandbox;
use deckhand_e2e::ui::launch_sandbox_ui;

// The UI sets NODE_ENV=production only while React loads (its fast build). It must not reach what the UI starts:
// the daemon, and through it every agent, shell, Dev command and action (npm there would skip devDependencies).
// ported from tests/nodeEnvUi.test.ts: NODE_ENV=production set for the UI never reaches agents or actions
#[test]
fn node_env_production_set_for_the_ui_never_reaches_agents_or_actions() {
    let sandbox = Sandbox::new("env");
    // A stand-in agent that prints the NODE_ENV it was started with.
    let bin = install_sandbox_agents(&sandbox, "echo \"agent node-env=[${NODE_ENV-}]\"");
    fs::write(
        sandbox.dev_home().join("config.json"),
        r#"{"defaults":{"actions":{"env":"echo \"action node-env=[$NODE_ENV]\""}}}"#,
    )
    .expect("write config");
    let mut ui = launch_sandbox_ui(&sandbox, &bin, &[]);
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
