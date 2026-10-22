//! Agent versions and updates, black-box: drives DECKHAND_TEST_BIN.
//!
//! Agent versions and updates against a real daemon with fake agents (its own daemon). The daemon's PATH holds only the
//! fakes and the system directories, so no real agent can ever be run here.

use std::cmp::Ordering;
use std::path::PathBuf;
use std::time::Duration;

use deckhand_e2e::client::{Client, request};
use deckhand_e2e::daemon::{spawn_daemon, wait_ping};
use deckhand_e2e::harness::{AgentOptions, Sandbox, install_agent, repo, stop, wait_for_default};
use serde_json::{Value, json};

fn compare_versions(a: &str, b: &str) -> Ordering {
    let parts = |v: &str| -> Vec<u64> { v.split('.').map(|p| p.parse().unwrap_or(0)).collect() };
    parts(a).cmp(&parts(b))
}

/// src/agentVersions.ts `sessionOutdated`.
fn session_outdated(session: &Value, installed: Option<&str>) -> bool {
    let version = session["agentVersion"].as_str().filter(|v| !v.is_empty());
    session["status"] == "running"
        && matches!((version, installed), (Some(v), Some(i)) if compare_versions(v, i) == Ordering::Less)
}

// ported from tests/agentUpdates.test.ts: agent versions and updates with fake agents
#[test]
fn agent_versions_and_updates_with_fake_agents() {
    let root = repo();
    let root_path = root.path().to_string_lossy().into_owned();
    let sandbox = Sandbox::new("agent-updates");
    let home = sandbox.home();
    let bin: PathBuf = sandbox.root().join("bin");
    let fake = PathBuf::from(env!("CARGO_BIN_EXE_fake_agent"));
    for provider in ["claude", "codex"] {
        install_agent(&bin, provider, &fake, AgentOptions::default());
    }
    // Pi's updater fails (after logging how it was called).
    install_agent(
        &bin,
        "pi",
        &fake,
        AgentOptions {
            update_fails: true,
            ..Default::default()
        },
    );

    // The latest releases are stubbed (never npm): Claude and Codex have newer ones, Pi's is unknown.
    let latest = json!({"claude": "1.0.1", "codex": "1.0.5"}).to_string();
    let mut daemon = spawn_daemon(
        &sandbox,
        root.path(),
        &format!("{}:/usr/bin:/bin", bin.display()),
        &[
            ("HOME", sandbox.root().as_os_str()),
            ("DECKHAND_AGENT_LATEST", latest.as_ref()),
        ],
    );

    let socket = sandbox.socket();
    let call = |message: Value| request(&socket, message);
    wait_ping(call, Duration::from_millis(8000));
    let state = |id: &str| -> Value {
        let list = call(json!({"type": "list"})).expect("list");
        list.as_array()
            .and_then(|items| items.iter().find(|item| item["id"] == id).cloned())
            .unwrap_or(Value::Null)
    };
    let create = |title: &str| -> Value {
        call(json!({"type": "create", "input": {"title": title, "program": "claude", "cwd": root_path, "repoRoot": root_path, "cols": 80, "rows": 24}}))
            .expect("create")
    };
    let running = |id: &str| {
        wait_for_default(
            || state(id),
            |item| item["status"] == "running" && item["agentVersion"].as_str().is_some_and(|v| !v.is_empty()),
        )
    };
    let updates = || -> Vec<Value> {
        std::fs::read_to_string(home.join("updates.log"))
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_str(line).expect("updates.log line"))
            .collect()
    };

    // A subscribed client receives agent-versions-updated broadcasts.
    let mut live = Client::connect(&socket).expect("live client");
    live.subscribe(&root_path).expect("subscribe");

    // Installed from `<agent> --version` in each agent's format, the latest from the stub.
    let versions = call(json!({"type": "agent-versions", "refresh": true})).expect("agent-versions");
    assert_eq!(
        json!({"installed": versions["claude"]["installed"], "latest": versions["claude"]["latest"], "path": versions["claude"]["path"]}),
        json!({"installed": "1.0.0", "latest": "1.0.1", "path": bin.join("claude").to_string_lossy()})
    );
    assert_eq!(versions["codex"]["installed"], "1.0.0");
    assert_eq!(versions["codex"]["latest"], "1.0.5");
    assert_eq!(versions["pi"]["installed"], "1.0.0");
    assert!(versions["pi"]["latest"].is_null());
    assert!(
        versions["pi"]["error"]
            .as_str()
            .unwrap_or_default()
            .contains("latest unknown")
    );
    assert!(
        !versions["claude"]["checkedAt"].is_null()
            && versions["claude"]["checkedAt"] != 0
            && versions["claude"]["checkedAt"] != ""
    );

    // A session records the version it launched with; it is current.
    let before = create("before update");
    let before_id = before["id"].as_str().expect("id").to_string();
    let launched = running(&before_id);
    assert_eq!(launched["agentVersion"], "1.0.0");
    assert_eq!(
        call(json!({"type": "agent-versions"})).expect("versions")["claude"]["outdated"],
        0
    );

    // The update runs the agent's own command non-interactively, re-reads the version and broadcasts it; sessions are untouched.
    let result = call(json!({"type": "update-agent", "program": "claude"})).expect("update claude");
    assert_eq!(result["ok"], true);
    assert_eq!(result["exitCode"], 0);
    assert_eq!(result["command"], "claude update");
    assert_eq!(json!([result["before"], result["after"]]), json!(["1.0.0", "1.0.1"]));
    assert!(
        result["output"]
            .as_str()
            .unwrap_or_default()
            .contains("Updated 1.0.0 -> 1.0.1")
    );
    assert_eq!(
        json!({"installed": result["versions"]["claude"]["installed"], "running": result["versions"]["claude"]["running"], "outdated": result["versions"]["claude"]["outdated"]}),
        json!({"installed": "1.0.1", "running": 1, "outdated": 1})
    );
    assert_eq!(
        updates(),
        vec![json!({"provider": "claude", "args": ["update"], "stdin": "tty-or-null"})]
    ); // stdin closed, not a pipe.
    wait_for_default(
        || live.events_of("agent-versions-updated"),
        |list| {
            list.iter()
                .any(|item| item["versions"]["claude"]["installed"] == "1.0.1")
        },
    );
    assert!(
        live.events_of("agent-versions-updated")
            .iter()
            .any(|item| item["versions"]["claude"]["updating"].as_bool() == Some(true)),
        "the update was broadcast as running"
    );
    let after = state(&before_id);
    assert_eq!(after["status"], "running");
    assert_eq!(after["pid"], launched["pid"]);
    assert_eq!(after["agentVersion"], "1.0.0");
    assert!(session_outdated(
        &after,
        result["versions"]["claude"]["installed"].as_str()
    ));

    // A session launched after the update is current; restarting the outdated one makes it current too.
    let later = create("after update");
    let later_id = later["id"].as_str().expect("id").to_string();
    assert_eq!(running(&later_id)["agentVersion"], "1.0.1");
    assert!(!session_outdated(&state(&later_id), Some("1.0.1")));
    call(json!({"type": "kill", "sessionId": before_id})).expect("kill");
    wait_for_default(|| state(&before_id), |item| item["status"] == "exited");
    assert!(
        !session_outdated(&state(&before_id), Some("1.0.1")),
        "an exited session is never outdated"
    );
    call(json!({"type": "restart", "sessionId": before_id, "cols": 80, "rows": 24, "mode": "fresh"})).expect("restart");
    wait_for_default(
        || state(&before_id),
        |item| item["status"] == "running" && item["agentVersion"] == "1.0.1",
    );
    assert_eq!(
        call(json!({"type": "agent-versions"})).expect("versions")["claude"]["outdated"],
        0
    );
    // Persisted with the session.
    let stored: Value = serde_json::from_str(&std::fs::read_to_string(home.join("state.json")).expect("state.json"))
        .expect("state json");
    let stored_later = stored["sessions"]
        .as_array()
        .and_then(|s| s.iter().find(|item| item["id"] == later_id.as_str()));
    assert_eq!(
        stored_later.map(|item| item["agentVersion"].clone()),
        Some(json!("1.0.1"))
    );

    // One update at a time per agent: concurrent requests share one run.
    let (first, second) = std::thread::scope(|scope| {
        let a = scope.spawn(|| call(json!({"type": "update-agent", "program": "codex"})).expect("codex update"));
        let b = scope.spawn(|| call(json!({"type": "update-agent", "program": "codex"})).expect("codex update"));
        (a.join().expect("join"), b.join().expect("join"))
    });
    assert_eq!(
        json!([first["after"], second["after"], first["output"]]),
        json!(["1.0.1", "1.0.1", second["output"]])
    );
    assert_eq!(updates().iter().filter(|item| item["provider"] == "codex").count(), 1);

    // A failing updater: its exit code and output come back, the version stays.
    let failed = call(json!({"type": "update-agent", "program": "pi"})).expect("pi update");
    assert_eq!(failed["ok"], false);
    assert_eq!(failed["exitCode"], 1);
    assert_eq!(failed["command"], "pi update --self");
    assert!(
        failed["output"]
            .as_str()
            .unwrap_or_default()
            .contains("npm error code EACCES")
    );
    assert_eq!(json!([failed["before"], failed["after"]]), json!(["1.0.0", "1.0.0"]));
    assert_eq!(
        updates()
            .iter()
            .find(|item| item["provider"] == "pi")
            .map(|item| item["args"].clone()),
        Some(json!(["update", "--self"]))
    );

    // Not installed: nothing runs; setup installs agents.
    std::fs::rename(bin.join("pi"), sandbox.root().join("pi-removed")).expect("remove pi");
    let missing = call(json!({"type": "update-agent", "program": "pi"})).expect_err("pi is not installed");
    assert!(missing.contains("Pi is not installed; run deckhand setup"), "{missing}");
    assert!(call(json!({"type": "agent-versions"})).expect("versions")["pi"]["installed"].is_null());
    let unknown = call(json!({"type": "update-agent", "program": "nope"})).expect_err("unknown agent");
    assert!(unknown.contains("Unknown agent"), "{unknown}");

    call(json!({"type": "kill", "sessionId": before_id})).expect("kill");
    call(json!({"type": "kill", "sessionId": later_id})).expect("kill");
    drop(live);
    stop(&mut daemon);
}
