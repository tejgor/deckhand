//! `deckhand restart`, black-box: a real daemon with fake agents, the CLI as a user runs it.
//!
//! Restarting replaces the daemon process (an update reaches the daemon only then) and interrupts its sessions, so
//! the command asks first, and refuses without a terminal unless `--yes` says to go ahead.

use std::path::Path;
use std::process::{Child, Output};
use std::time::Duration;

use deckhand_e2e::daemon::{
    create, exists, id_of, install_agents, launch, load_state, must, ready, running, state, text_of, wf, write_json,
};
use deckhand_e2e::fake_agent;
use deckhand_e2e::harness::{Sandbox, is_alive, repo, stop, wait_exit};
use serde_json::{Value, json};

/// The build the binary under test reports (the workspace version, as this crate's).
const BUILD: &str = env!("CARGO_PKG_VERSION");

/// `deckhand restart <args>` with no terminal (stdin closed) and only the fakes and system tools on PATH, so a daemon
/// it starts can never find a real agent.
fn restart(sandbox: &Sandbox, bin: &Path, args: &[&str]) -> Output {
    let mut command = sandbox.cli(&[&["restart"], args].concat());
    command.env("PATH", format!("{}:/usr/bin:/bin", bin.display()));
    sandbox.exec(command, Duration::from_secs(40))
}

fn ping_pid(socket: &Path) -> i64 {
    must(socket, json!({"type": "ping"}))["pid"]
        .as_i64()
        .unwrap_or_default()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A daemon in `root` with one running fake Claude session; returns the daemon, the session ID and the fakes' dir.
fn daemon_with_a_running_session(sandbox: &Sandbox, root: &Path) -> (Child, String, std::path::PathBuf) {
    let bin = install_agents(sandbox, fake_agent!());
    let daemon = launch(sandbox, root, &bin);
    ready(&sandbox.socket());
    let root_path = root.to_string_lossy().into_owned();
    let session = create(&sandbox.socket(), &root_path, "agent", "none", json!({}));
    let id = id_of(&session);
    running(&sandbox.socket(), &id);
    (daemon, id, bin)
}

#[test]
fn restart_yes_replaces_the_daemon_and_interrupts_its_sessions() {
    let root = repo();
    let sandbox = Sandbox::new("restart");
    let socket = sandbox.socket();
    let (mut old_daemon, id, bin) = daemon_with_a_running_session(&sandbox, root.path());
    let old_pid = ping_pid(&socket);
    assert_eq!(old_pid, i64::from(old_daemon.id()));
    assert_eq!(must(&socket, json!({"type": "ping"}))["build"], BUILD);

    let output = restart(&sandbox, &bin, &["--yes"]);
    assert!(
        output.status.success(),
        "{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    assert_eq!(text(&output.stdout), format!("Daemon restarted ({BUILD}).\n"));

    // The old daemon shut down by itself (SIGTERM: a stable daemon refuses `shutdown`), cleanly.
    let status = wait_exit(&mut old_daemon, Duration::from_secs(5)).expect("the old daemon exited");
    assert!(status.success(), "{status:?}");
    // A new daemon answers, from another process, and daemon.pid names it.
    let new_pid = ping_pid(&socket);
    assert_ne!(new_pid, old_pid);
    assert!(is_alive(i32::try_from(new_pid).unwrap_or_default()));
    let pid_file = std::fs::read_to_string(sandbox.home().join("daemon.pid")).unwrap_or_default();
    assert_eq!(pid_file.trim(), new_pid.to_string());
    // The session it hosted is interrupted (resumable with s), not lost, and offered for resuming.
    let session: Value = state(&socket, &id);
    assert_eq!(session["status"], "exited", "{session}");
    assert_eq!(session["exitReason"], "interrupted", "{session}");
    assert!(session["interruptedWhile"].is_string(), "{session}");
}

#[test]
fn daemons_started_at_once_leave_one_that_offers_the_interrupted_sessions() {
    let root = repo();
    let sandbox = Sandbox::new("restart-race");
    let socket = sandbox.socket();
    let (mut old_daemon, id, bin) = daemon_with_a_running_session(&sandbox, root.path());
    // Stopped as `deckhand restart` stops it.
    let old_pid = nix::unistd::Pid::from_raw(i32::try_from(old_daemon.id()).unwrap_or_default());
    nix::sys::signal::kill(old_pid, nix::sys::signal::Signal::SIGTERM).expect("SIGTERM");
    wait_exit(&mut old_daemon, Duration::from_secs(10)).expect("the old daemon exited");

    // Two clients find no daemon at the same moment (`deckhand restart` and an open UI) and each starts one.
    let first = launch(&sandbox, root.path(), &bin);
    let second = launch(&sandbox, root.path(), &bin);
    ready(&socket);
    let serving = ping_pid(&socket);
    let (mut winner, mut loser) = if serving == i64::from(first.id()) {
        (first, second)
    } else {
        (second, first)
    };
    assert_eq!(serving, i64::from(winner.id()));
    // The other gives up once it cannot take daemon.lock, before it touched the state or the socket.
    let status = wait_exit(&mut loser, Duration::from_secs(10)).expect("the second daemon gave up");
    assert!(!status.success(), "{status:?}");
    assert!(is_alive(i32::try_from(winner.id()).unwrap_or_default()));
    // The one serving recovered the session, so it offers it for resuming.
    let session: Value = state(&socket, &id);
    assert_eq!(session["exitReason"], "interrupted", "{session}");
    assert!(session["interruptedWhile"].is_string(), "{session}");
    assert!(wait_exit(&mut winner, Duration::ZERO).is_none());
}

#[test]
fn restarting_sessions_boots_at_most_three_at_a_time() {
    let root = repo();
    let sandbox = Sandbox::new("restart-many");
    let socket = sandbox.socket();
    let bin = install_agents(&sandbox, fake_agent!());
    let mut daemon = launch(&sandbox, root.path(), &bin);
    ready(&socket);
    let root_path = root.path().to_string_lossy().into_owned();
    let ids: Vec<String> = (1..=4)
        .map(|n| {
            let id = id_of(&create(&socket, &root_path, &format!("agent {n}"), "none", json!({})));
            running(&socket, &id);
            id
        })
        .collect();
    let launched = |id: &str| {
        state(&socket, id)["agentStartedAt"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    let before: Vec<String> = ids.iter().map(|id| launched(id)).collect();

    // Running ones are stopped, then resumed: every one comes back.
    let outcomes = must(
        &socket,
        json!({"type": "restart-sessions", "sessionIds": ids, "cols": 80, "rows": 24}),
    );
    let expected: Vec<Value> = ids.iter().map(|id| json!({"sessionId": id})).collect();
    assert_eq!(outcomes, Value::Array(expected));
    let after: Vec<String> = ids.iter().map(|id| launched(id)).collect();
    for (id, (old, new)) in ids.iter().zip(before.iter().zip(&after)) {
        assert!(new > old, "{id} was not launched again: {old} → {new}");
        assert_eq!(state(&socket, id)["status"], "running");
    }
    // The fourth waited for one of the first three to settle (its screen still for a second at least).
    let millis = |iso: &str| {
        let time = iso.get(11..23).unwrap_or_default();
        let parts: Vec<f64> = time
            .split([':', '.'])
            .map(|part| part.parse().unwrap_or_default())
            .collect();
        ((parts[0] * 60.0 + parts[1]) * 60.0 + parts[2]) * 1000.0 + parts[3]
    };
    let first = after[..3].iter().map(|at| millis(at)).fold(f64::INFINITY, f64::min);
    assert!(
        millis(&after[3]) - first >= 900.0,
        "the fourth started with the first three: {after:?}"
    );
    stop(&mut daemon);
}

#[test]
fn restart_without_yes_refuses_without_a_terminal_while_sessions_run() {
    let root = repo();
    let sandbox = Sandbox::new("restart-no");
    let socket = sandbox.socket();
    let (_daemon, id, bin) = daemon_with_a_running_session(&sandbox, root.path());
    let pid = ping_pid(&socket);

    let output = restart(&sandbox, &bin, &[]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stderr),
        "Not restarting the Deckhand daemon: 1 running session(s) (1 idle) would be interrupted (Deckhand offers \
         to resume them when you next open it). Run `deckhand restart --yes` to restart it anyway.\n"
    );
    assert_eq!(text(&output.stdout), "");
    // Nothing was touched.
    assert_eq!(ping_pid(&socket), pid);
    assert_eq!(state(&socket, &id)["status"], "running");
}

#[test]
fn restart_starts_a_daemon_when_none_runs() {
    let sandbox = Sandbox::new("restart-none");
    let bin = install_agents(&sandbox, fake_agent!());
    let output = restart(&sandbox, &bin, &[]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        format!("No daemon was running; started one ({BUILD}).\n")
    );
    assert!(ping_pid(&sandbox.socket()) > 0);
    // With nothing running there is nothing to ask about: a second restart goes ahead without --yes.
    let output = restart(&sandbox, &bin, &[]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), format!("Daemon restarted ({BUILD}).\n"));
}

/// Session `id` running again after a launch, its agent booted (its SessionStart hook applied, as the fake agent sends
/// it before it is ready) and idle; the launch's ID and hook token.
fn booted(sandbox: &Sandbox, id: &str) -> (String, String) {
    let socket = sandbox.socket();
    let launch = running(&socket, id)["launchId"].as_str().unwrap_or_default().to_owned();
    wf(
        || exists(sandbox.home().join(format!("ready-{launch}"))),
        |ready| *ready,
    );
    wf(|| state(&socket, id), |session| session["agentStatus"] == "idle");
    let trace: Value = text_of(&sandbox.home().join(format!("trace-{id}.json")))
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    assert_eq!(trace["launchId"], launch.as_str());
    (launch, trace["token"].as_str().unwrap_or_default().to_owned())
}

/// A lifecycle hook from session `id`'s current agent, as `deckhand hook` relays it.
fn hook(sandbox: &Sandbox, id: &str, payload: Value) {
    let (launch, token) = booted(sandbox, id);
    must(
        &sandbox.socket(),
        json!({"type": "agent-hook", "sessionId": id, "launchId": launch, "token": token, "payload": payload}),
    );
}

/// A turn of session `id` that ends with a reply saying `said`.
fn turn(sandbox: &Sandbox, id: &str, said: &str) {
    hook(sandbox, id, json!({"hook_event_name": "UserPromptSubmit"}));
    hook(
        sandbox,
        id,
        json!({"hook_event_name": "Stop", "last_assistant_message": said}),
    );
}

fn attention(socket: &Path, id: &str) -> Value {
    state(socket, id)["attention"].clone()
}

/// The attention reason state.json keeps for session `id`, if any.
fn stored_reason(sandbox: &Sandbox, id: &str) -> Value {
    load_state(&sandbox.home())["sessions"]
        .as_array()
        .and_then(|sessions| sessions.iter().find(|session| session["id"] == id).cloned())
        .map(|session| session["attention"]["reason"]["text"].clone())
        .unwrap_or_default()
}

/// Resumes `ids` as the checklist after a restart (or an agent update) does, and waits for each to boot.
fn restart_sessions(sandbox: &Sandbox, ids: &[&str]) {
    let outcomes = must(
        &sandbox.socket(),
        json!({"type": "restart-sessions", "sessionIds": ids, "cols": 80, "rows": 24}),
    );
    let expected: Vec<Value> = ids.iter().map(|id| json!({"sessionId": id})).collect();
    assert_eq!(outcomes, Value::Array(expected));
    for id in ids {
        booted(sandbox, id);
    }
}

#[test]
fn unread_replies_and_the_state_view_times_survive_relaunches_and_a_daemon_restart() {
    let root = repo();
    let sandbox = Sandbox::new("restart-unread");
    let socket = sandbox.socket();
    let bin = install_agents(&sandbox, fake_agent!());
    let mut old_daemon = launch(&sandbox, root.path(), &bin);
    ready(&socket);
    let root_path = root.path().to_string_lossy().into_owned();
    let [replied, read, asked] = ["replied", "read", "asked"].map(|title| {
        let id = id_of(&create(&socket, &root_path, title, "none", json!({})));
        booted(&sandbox, &id);
        id
    });
    let (replied, read, asked) = (replied.as_str(), read.as_str(), asked.as_str());
    // A reply you have not read; one you have; a permission it waits on you for (you looked, and left it).
    turn(&sandbox, replied, "All 12 tests pass.");
    turn(&sandbox, read, "Done.");
    hook(
        &sandbox,
        asked,
        json!({"hook_event_name": "PermissionRequest", "tool_name": "Bash", "tool_input": {"command": "npm test"}}),
    );
    for id in [read, asked] {
        must(
            &socket,
            json!({"type": "set-reply-read", "sessionId": id, "read": true}),
        );
    }
    let replied_at = attention(&socket, replied)["at"].clone();
    let asked_at = attention(&socket, asked)["at"].clone();
    // Idle orders it by the later of reading its reply and its activity changing.
    let read_session = state(&socket, read);
    let went_idle = read_session["replyReadAt"]
        .as_str()
        .max(read_session["agentStatusUpdatedAt"].as_str())
        .map(str::to_owned);
    // Nothing is carried yet, and state.json keeps no reason.
    assert_eq!(attention(&socket, replied)["carried"], Value::Null);
    assert_eq!(stored_reason(&sandbox, replied), Value::Null);

    // What each shows, with the times the state view orders them by, as long as nothing else happens to them.
    let shows_as_before = |stage: &str, replied_at: &Value, said: &str| {
        let replied = attention(&socket, replied);
        assert_eq!(replied["state"], "response-ended", "{stage}: {replied}");
        assert_eq!(replied["carried"], "unread", "{stage}: {replied}");
        assert_eq!(&replied["at"], replied_at, "{stage}: {replied}");
        assert_eq!(replied["reason"]["text"], format!("said: {said}"), "{stage}: {replied}");
        let session = state(&socket, asked);
        let asked = &session["attention"];
        assert_eq!(asked["state"], "response-ended", "{stage}: {asked}");
        assert_eq!(asked["carried"], "needs-input", "{stage}: {asked}");
        assert_eq!(asked["at"], asked_at, "{stage}: {asked}");
        assert_eq!(asked["reason"]["text"], "wants to run: npm test", "{stage}: {asked}");
        // Waiting on you is unread again, though you had looked.
        assert_eq!(session["replyReadAt"], Value::Null, "{stage}: {session}");
        let read = attention(&socket, read);
        assert_eq!(read["carried"], "idle", "{stage}: {read}");
        assert_eq!(read["at"].as_str(), went_idle.as_deref(), "{stage}: {read}");
    };

    // The daemon restarts: the sessions it had running are interrupted, and carry over what they showed (its last
    // write kept the two reasons).
    let output = restart(&sandbox, &bin, &["--yes"]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    wait_exit(&mut old_daemon, Duration::from_secs(5)).expect("the old daemon exited");
    for id in [replied, read, asked] {
        assert_eq!(state(&socket, id)["exitReason"], "interrupted");
    }
    shows_as_before("restarted daemon", &replied_at, "All 12 tests pass.");
    assert_eq!(stored_reason(&sandbox, replied), json!("said: All 12 tests pass."));
    assert_eq!(stored_reason(&sandbox, asked), json!("wants to run: npm test"));
    assert_eq!(stored_reason(&sandbox, read), Value::Null);
    // Resumed: the agents starting again (their SessionStart hooks) change none of it.
    restart_sessions(&sandbox, &[replied, read, asked]);
    shows_as_before("resumed", &replied_at, "All 12 tests pass.");

    // A new turn is real activity: a new reply, unread from now, not carried.
    turn(&sandbox, replied, "Fixed the flaky test.");
    let fresh = attention(&socket, replied);
    assert_eq!(fresh["carried"], Value::Null);
    assert!(fresh["at"].as_str() > replied_at.as_str());
    let replied_at = fresh["at"].clone();
    assert_eq!(stored_reason(&sandbox, replied), Value::Null);

    // `s` (stopped, then resumed): still unread, from its own time.
    must(&socket, json!({"type": "kill", "sessionId": replied}));
    wf(|| state(&socket, replied), |session| session["status"] == "exited");
    must(
        &socket,
        json!({"type": "restart", "sessionId": replied, "cols": 80, "rows": 24}),
    );
    booted(&sandbox, replied);
    shows_as_before("s", &replied_at, "Fixed the flaky test.");
    // A controlled restart of all three (as after an agent update).
    restart_sessions(&sandbox, &[replied, read, asked]);
    shows_as_before("controlled restart", &replied_at, "Fixed the flaky test.");
    assert_eq!(stored_reason(&sandbox, replied), json!("said: Fixed the flaky test."));

    // Read: no longer unread, and state.json no longer keeps its line.
    must(
        &socket,
        json!({"type": "set-reply-read", "sessionId": replied, "read": true}),
    );
    assert_eq!(stored_reason(&sandbox, replied), Value::Null);
    // A new turn ends what the other carried.
    hook(&sandbox, asked, json!({"hook_event_name": "UserPromptSubmit"}));
    let working = attention(&socket, asked);
    assert_eq!(
        (working["state"].as_str(), &working["carried"]),
        (Some("working"), &Value::Null)
    );
    assert_eq!(stored_reason(&sandbox, asked), Value::Null);
}

#[test]
fn upgrading_from_a_state_json_without_carried_markers_keeps_unread_replies_by_their_old_times() {
    let root = repo();
    let sandbox = Sandbox::new("restart-upgrade");
    let socket = sandbox.socket();
    let bin = install_agents(&sandbox, fake_agent!());
    let mut old_daemon = launch(&sandbox, root.path(), &bin);
    ready(&socket);
    let root_path = root.path().to_string_lossy().into_owned();
    let [replied, read, asked] = ["replied", "read", "asked"].map(|title| {
        let id = id_of(&create(&socket, &root_path, title, "none", json!({})));
        booted(&sandbox, &id);
        id
    });
    let (replied, read, asked) = (replied.as_str(), read.as_str(), asked.as_str());
    turn(&sandbox, replied, "All 12 tests pass.");
    turn(&sandbox, read, "Done.");
    hook(
        &sandbox,
        asked,
        json!({"hook_event_name": "PermissionRequest", "tool_name": "Bash"}),
    );
    for id in [read, asked] {
        must(
            &socket,
            json!({"type": "set-reply-read", "sessionId": id, "read": true}),
        );
    }
    let replied_at = attention(&socket, replied)["at"].clone();
    let asked_at = attention(&socket, asked)["at"].clone();
    stop(&mut old_daemon);

    // state.json as the version before carried attention left it when it stopped: the sessions still running, each
    // attention `{state, event, at}`, never a reason (nor anything this version adds).
    let path = sandbox.home().join("state.json");
    let mut stored = load_state(&sandbox.home());
    for session in stored["sessions"].as_array_mut().into_iter().flatten() {
        assert_eq!(session["status"], "running", "{session}");
        if let Some(attention) = session["attention"].as_object_mut() {
            attention.retain(|key, _| ["state", "event", "at"].contains(&key.as_str()));
        }
    }
    write_json(&path, &stored);

    let mut new_daemon = launch(&sandbox, root.path(), &bin);
    ready(&socket);
    let shows_as_before = |stage: &str| {
        let replied = attention(&socket, replied);
        assert_eq!(
            (&replied["state"], &replied["carried"], &replied["at"]),
            (&json!("response-ended"), &json!("unread"), &replied_at),
            "{stage}: {replied}"
        );
        assert_eq!(replied["reason"], Value::Null, "{stage}: {replied}");
        let session = state(&socket, asked);
        let asked = &session["attention"];
        assert_eq!(
            (&asked["state"], &asked["carried"], &asked["at"]),
            (&json!("response-ended"), &json!("needs-input"), &asked_at),
            "{stage}: {asked}"
        );
        assert_eq!(asked["reason"], Value::Null, "{stage}: {asked}");
        assert_eq!(session["replyReadAt"], Value::Null, "{stage}: {session}");
        assert_eq!(attention(&socket, read)["carried"], "idle", "{stage}");
    };
    for id in [replied, read, asked] {
        assert_eq!(state(&socket, id)["exitReason"], "interrupted");
    }
    shows_as_before("upgraded daemon");
    restart_sessions(&sandbox, &[replied, read, asked]);
    shows_as_before("resumed");
    must(&socket, json!({"type": "kill", "sessionId": replied}));
    wf(|| state(&socket, replied), |session| session["status"] == "exited");
    must(
        &socket,
        json!({"type": "restart", "sessionId": replied, "cols": 80, "rows": 24}),
    );
    booted(&sandbox, replied);
    shows_as_before("s");
    stop(&mut new_daemon);
}
