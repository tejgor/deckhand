//! The agent's launch environment (black-box: drives DECKHAND_TEST_BIN against a real daemon with fake agents).
//!
//! Every scroll path sends the wheel to the agent's own TUI, so Claude must run its fullscreen renderer: the worker sets
//! `CLAUDE_CODE_NO_FLICKER=1` for Claude (over any inherited value) and leaves Pi's and Codex's environment alone.

use std::ffi::OsStr;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use deckhand_e2e::client::request;
use deckhand_e2e::daemon::{id_of, install_agents, spawn_daemon, wait_ping};
use deckhand_e2e::harness::{Sandbox, repo, stop, wait_for_default};
use serde_json::{Value, json};

#[test]
fn only_claude_gets_the_fullscreen_renderer() {
    let root = repo();
    let root_path = root.path().to_string_lossy().into_owned();
    let sandbox = Sandbox::new("agent-env");
    let bin = install_agents(&sandbox, &PathBuf::from(env!("CARGO_BIN_EXE_fake_agent")));
    // An inherited value is replaced for Claude and passed through unchanged to the others.
    let mut daemon = spawn_daemon(
        &sandbox,
        root.path(),
        &format!("{}:/usr/bin:/bin", bin.display()),
        &[
            ("HOME", sandbox.root().as_os_str()),
            ("CLAUDE_CODE_NO_FLICKER", OsStr::new("0")),
        ],
    );
    let socket = sandbox.socket();
    let call = |message: Value| request(&socket, message);
    wait_ping(call, Duration::from_millis(8000));

    for (provider, expected) in [("claude", "1"), ("pi", "0"), ("codex", "0")] {
        let created = call(json!({"type": "create", "input": {"title": provider, "program": provider, "cwd": root_path, "repoRoot": root_path, "cols": 80, "rows": 24}}))
            .unwrap_or_else(|error| panic!("create {provider}: {error}"));
        // The fake agent's trace of its launch (see fake_agent.rs).
        let trace = sandbox.home().join(format!("trace-{}.json", id_of(&created)));
        let trace = wait_for_default(
            || {
                fs::read_to_string(&trace)
                    .ok()
                    .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            },
            Option::is_some,
        )
        .unwrap_or_default();
        assert_eq!(trace["noFlicker"], expected, "{provider}");
    }

    stop(&mut daemon);
}
