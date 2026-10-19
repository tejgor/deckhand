//! Port of tests/worktreesDaemon.test.ts (black-box: drives DECKHAND_TEST_BIN).
#![allow(dead_code)]

use deckhand_e2e::harness::repo;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use deckhand_e2e::client::Client;
use deckhand_e2e::harness::{AgentOptions, Sandbox, install_agent, wait_for_default};
use serde_json::{Value, json};

// ---- local plumbing shared by the daemon suites (the harness has no daemon-launch helper) ----

/// helpers.ts `request(..., 60_000)`: one connection per call.
fn call(socket: &Path, message: Value) -> Result<Value, String> {
    Client::connect(socket)
        .map_err(|e| e.to_string())?
        .request_timeout(message, Duration::from_secs(60))
}

fn must(socket: &Path, message: Value) -> Value {
    let shown = message.to_string();
    call(socket, message).unwrap_or_else(|e| panic!("request {shown} failed: {e}"))
}

/// `assert.rejects(promise, /needle/)`.
fn rejects(result: Result<Value, String>, needle: &str) {
    match result {
        Ok(value) => panic!("expected a rejection matching {needle:?}, got {value}"),
        Err(error) => assert!(error.contains(needle), "{error:?} does not contain {needle:?}"),
    }
}

/// The fake agents (claude, pi, codex) in `<sandbox>/bin`; returns that directory.
fn install_agents(sandbox: &Sandbox) -> PathBuf {
    let bin = sandbox.root().join("bin");
    let fake = PathBuf::from(env!("CARGO_BIN_EXE_fake_agent"));
    for provider in ["claude", "pi", "codex"] {
        install_agent(&bin, provider, &fake, AgentOptions::default());
    }
    bin
}

/// `deckhand --daemon` in `root` with the fakes first on a PATH that holds nothing else but the system directories.
fn launch(sandbox: &Sandbox, root: &Path, bin: &Path) -> Child {
    let mut command = sandbox.cli(&["--daemon"]);
    command
        .current_dir(root)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    sandbox.spawn(command)
}

fn ready(socket: &Path) {
    wait_for_default(
        || {
            call(socket, json!({"type": "ping"}))
                .map(|v| v["ok"] == true)
                .unwrap_or(false)
        },
        |ok| *ok,
    );
}

/// A persistent connection subscribed to `root` (the TS tests' raw socket + `events`).
fn subscribed(socket: &Path, root: &str) -> Client {
    let mut live = Client::connect(socket).expect("live client");
    live.subscribe(root).expect("subscribe");
    live
}

/// `updated(id, matches)`: waits for a `session-updated` event after the first `from` events.
fn updated(live: &Client, from: usize, id: &str, matches: impl Fn(&Value) -> bool) {
    wait_for_default(
        || {
            live.events()
                .iter()
                .skip(from)
                .any(|e| e["type"] == "session-updated" && e["session"]["id"] == id && matches(&e["session"]))
        },
        |found| *found,
    );
}

fn state(socket: &Path, id: &str) -> Value {
    must(socket, json!({"type": "list"}))
        .as_array()
        .and_then(|items| items.iter().find(|item| item["id"] == id).cloned())
        .unwrap_or(Value::Null)
}

fn try_create(socket: &Path, root: &str, title: &str, mode: &str, extra: Value) -> Result<Value, String> {
    let mut input = json!({"title": title, "program": "claude", "cwd": root, "repoRoot": root, "cols": 80, "rows": 24, "worktreeMode": mode});
    for (key, value) in extra.as_object().into_iter().flatten() {
        input[key] = value.clone();
    }
    call(socket, json!({"type": "create", "input": input}))
}

fn create(socket: &Path, root: &str, title: &str, mode: &str, extra: Value) -> Value {
    try_create(socket, root, title, mode, extra).unwrap_or_else(|e| panic!("create {title}: {e}"))
}

fn running(socket: &Path, id: &str) -> Value {
    wait_for_default(|| state(socket, id), |item| item["status"] == "running")
}

/// Git that may fail: (success, trimmed-end stdout).
fn git_try(cwd: &Path, args: &[&str]) -> (bool, String) {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("spawn git");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).trim_end().to_string(),
    )
}

fn p(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn text_of(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

/// The persisted `state.json` (`loadState`).
fn load_state(home: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(home.join("state.json")).expect("state.json")).expect("state json")
}

fn pause(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// Falsy in the JS sense for the fields the tests check (`undefined`, `false`).
fn falsy(value: &Value) -> bool {
    value.is_null() || *value == json!(false)
}

fn id_of(value: &Value) -> String {
    value["id"].as_str().unwrap().to_string()
}

// ported from tests/worktreesDaemon.test.ts: worktree overview and deletion: merged detection, refusals, stopping sessions, data-loss override, prune and forget
#[test]
fn worktree_overview_and_deletion_merged_detection_refusals_stopping_sessions_data_loss_override_prune_and_forget() {
    let repo_dir = repo();
    let sandbox = Sandbox::new("worktrees");
    let root_path = fs::canonicalize(repo_dir.path()).expect("realpath");
    let root_s = p(&root_path);
    let root = root_s.as_str();
    // The state directory by its real path, so Deckhand's worktrees are listed by Git under the path it recorded.
    let home = fs::canonicalize(sandbox.home()).expect("realpath");
    let bin = install_agents(&sandbox);
    let mut command = sandbox.cli(&["--daemon"]);
    command
        .current_dir(&root_path)
        .env("DECKHAND_HOME", &home)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut daemon = sandbox.spawn(command);
    let socket = sandbox.socket();
    ready(&socket);
    let create_new = |title: &str| create(&socket, root, title, "new", json!({}));
    let running_wt = |id: &str| {
        deckhand_e2e::harness::wait_for(
            || state(&socket, id),
            |item| item["status"] == "running" && !falsy(&item["worktree"]["path"]),
            Duration::from_secs(20),
        )
    };
    let overview = || must(&socket, json!({"type": "worktree-overview", "cwd": root}));
    let entry_at = |view: &Value, at: &Path| -> Value {
        view["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["path"] == json!(p(at)))
            .cloned()
            .unwrap_or(Value::Null)
    };
    let inspect_in = |cwd: &Path, at: &Path, delete_branch: bool| {
        must(
            &socket,
            json!({"type": "inspect-worktree", "cwd": p(cwd), "path": p(at), "deleteBranch": delete_branch}),
        )
    };
    let inspect = |at: &Path, delete_branch: bool| inspect_in(&root_path, at, delete_branch);
    let remove = |at: &Path, options: Value| {
        let mut message = json!({"type": "delete-worktree", "cwd": root, "path": p(at)});
        for (key, value) in options.as_object().into_iter().flatten() {
            message[key] = value.clone();
        }
        call(&socket, message)
    };
    let branches = || -> Vec<String> {
        git_try(&root_path, &["branch", "--format=%(refname:short)"])
            .1
            .split('\n')
            .map(str::to_string)
            .collect()
    };
    let blockers = |check: &Value| -> String {
        check["structuralBlockers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b.as_str().unwrap())
            .collect::<Vec<_>>()
            .join(",")
    };

    // A Deckhand worktree whose branch got a commit and was then merged into main outside Deckhand; its agent still runs.
    let shipped = create_new("shipped feature");
    let shipped_id = id_of(&shipped);
    let shipped_worktree = running_wt(&shipped_id)["worktree"].clone();
    let shipped_path = PathBuf::from(shipped_worktree["path"].as_str().unwrap());
    let shipped_branch = shipped_worktree["branch"].as_str().unwrap().to_string();
    fs::write(shipped_path.join("feature.txt"), "feature\n").unwrap();
    assert!(git_try(&shipped_path, &["add", "."]).0);
    assert!(git_try(&shipped_path, &["commit", "-m", "feature"]).0);
    assert!(
        git_try(
            &root_path,
            &["merge", "--no-ff", "-m", "merge feature", &shipped_branch]
        )
        .0
    );
    // A worktree made with git (no Deckhand record), with a file only it has.
    let orphan = home.join("orphan");
    assert!(git_try(&root_path, &["worktree", "add", "-b", "orphan-work", &p(&orphan)]).0);
    fs::write(orphan.join("notes.txt"), "keep me\n").unwrap();

    let mut view = overview();
    assert_eq!(view["defaultBranch"], "main");
    let main = view["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["isMain"] == true)
        .cloned()
        .unwrap();
    assert_eq!(main["path"], json!(root));
    assert_eq!(main["inUse"], "this");
    assert!(main["inspection"].is_null());
    // Found merged (the overview runs merge detection first), clean, its session listed as running.
    let shipped_entry = entry_at(&view, &shipped_path);
    assert_eq!(shipped_entry["markers"]["mergeDetected"], "ancestor");
    assert_eq!(shipped_entry["inspection"]["safe"], true);
    assert_eq!(shipped_entry["aheadOfDefault"], 0);
    assert!(!falsy(&shipped_entry["recordId"]));
    assert_eq!(shipped_entry["sessionIds"], json!([shipped_id]));
    assert_eq!(shipped_entry["runningIds"], json!([shipped_id]));
    assert!(!falsy(&shipped_entry["lastCommitAt"]));
    // Not Deckhand's: no record, no sessions; its untracked file is what deleting it would lose.
    let orphan_entry = entry_at(&view, &orphan);
    assert!(orphan_entry["recordId"].is_null());
    assert_eq!(orphan_entry["sessionIds"], json!([]));
    assert_eq!(orphan_entry["inspection"]["safe"], false);
    assert_eq!(orphan_entry["inspection"]["untrackedFiles"], 1);

    // Structural refusals: the main checkout, the worktree this Deckhand runs in, one another Deckhand has open,
    // a protected branch, a branch that changed since it was listed.
    rejects(remove(&root_path, json!({})), "main checkout");
    assert!(blockers(&inspect_in(&orphan, &orphan, false)).contains("runs in that worktree"));
    let other = subscribed(&socket, &p(&orphan));
    assert!(blockers(&inspect(&orphan, false)).contains("another Deckhand is open"));
    drop(other);
    wait_for_default(
        || inspect(&orphan, false),
        |check| check["structuralBlockers"].as_array().unwrap().is_empty(),
    );
    rejects(
        remove(
            &shipped_path,
            json!({"branch": "renamed", "deleteBranch": true, "stopSessions": true}),
        ),
        "branch changed",
    );

    // Its session runs: refused unless the deletion may stop it, and nothing was touched.
    let check = inspect(&shipped_path, true);
    assert_eq!(check["safe"], true);
    assert_eq!(
        check["running"],
        json!([{"id": shipped_id, "title": "shipped feature"}])
    );
    rejects(
        remove(&shipped_path, json!({"branch": shipped_branch, "deleteBranch": true})),
        "still running in it",
    );
    assert_eq!(state(&socket, &shipped_id)["status"], "running");
    // Stopped, removed with its branch, the record marked deleted and the session archived.
    assert_eq!(
        remove(
            &shipped_path,
            json!({"branch": shipped_branch, "deleteBranch": true, "stopSessions": true})
        )
        .unwrap(),
        json!({"removed": "deleted", "branchDeleted": shipped_branch, "stopped": 1, "archived": 1})
    );
    let after = state(&socket, &shipped_id);
    assert_eq!(after["status"], "exited");
    assert_eq!(after["exitReason"], "stopped");
    assert!(!falsy(&after["worktree"]["deletedAt"]));
    assert!(!falsy(&after["archivedAt"]));
    assert!(!shipped_path.exists());
    assert!(!branches().contains(&shipped_branch));

    // Data loss needs the override (typed DELETE in the UI); with it, the worktree and its branch go.
    rejects(
        remove(&orphan, json!({"branch": "orphan-work", "deleteBranch": true})),
        "Deletion blocked: 1 untracked file",
    );
    assert_eq!(text_of(&orphan.join("notes.txt")).as_deref(), Some("keep me\n"));
    assert_eq!(
        remove(
            &orphan,
            json!({"branch": "orphan-work", "deleteBranch": true, "allowDataLoss": true})
        )
        .unwrap(),
        json!({"removed": "deleted", "branchDeleted": "orphan-work", "stopped": 0, "archived": 0})
    );
    assert!(!orphan.exists());

    // A worktree whose directory was deleted by hand: listed as missing, pruned (its branch stays).
    let gone = home.join("gone");
    assert!(git_try(&root_path, &["worktree", "add", "-b", "gone-work", &p(&gone)]).0);
    fs::remove_dir_all(&gone).unwrap();
    view = overview();
    assert_eq!(entry_at(&view, &gone)["missing"], "prunable");
    assert_eq!(
        remove(&gone, json!({})).unwrap(),
        json!({"removed": "pruned", "stopped": 0, "archived": 0})
    );
    assert!(
        !git_try(&root_path, &["worktree", "list", "--porcelain"])
            .1
            .contains(&p(&gone))
    );
    assert!(branches().contains(&"gone-work".to_string()));

    // A Deckhand worktree removed outside Deckhand: only its record is left, and forgetting it marks it deleted.
    let outside = create_new("removed outside");
    let outside_id = id_of(&outside);
    let outside_path = PathBuf::from(running_wt(&outside_id)["worktree"]["path"].as_str().unwrap());
    must(&socket, json!({"type": "kill", "sessionId": outside_id}));
    wait_for_default(|| state(&socket, &outside_id), |item| item["status"] == "exited");
    assert!(git_try(&root_path, &["worktree", "remove", "--force", &p(&outside_path)]).0);
    view = overview();
    let unlisted = entry_at(&view, &outside_path);
    assert_eq!(unlisted["missing"], "unlisted");
    assert_eq!(unlisted["sessionIds"], json!([outside_id]));
    assert_eq!(
        remove(&outside_path, json!({})).unwrap(),
        json!({"removed": "forgotten", "stopped": 0, "archived": 1})
    );
    assert!(!falsy(&state(&socket, &outside_id)["worktree"]["deletedAt"]));
    // Deleted and forgotten worktrees are no longer listed; a path that is not a worktree of the repository is refused.
    view = overview();
    let listed: Vec<&Value> = view["entries"].as_array().unwrap().iter().map(|e| &e["path"]).collect();
    assert_eq!(listed, vec![&json!(root)]);
    rejects(remove(&home.join("nowhere"), json!({})), "not listed by Git");
    deckhand_e2e::harness::stop(&mut daemon);
}
