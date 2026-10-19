//! Port of tests/mergeDaemon.test.ts (black-box: drives DECKHAND_TEST_BIN).
#![allow(dead_code)]

use deckhand_e2e::harness::repo;
use std::fs;
use std::os::unix::fs::PermissionsExt;
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

// A fake `gh`: answers `gh pr view` from gh-pr.json in the state directory (or fails like an unknown branch), never the network.
const FAKE_GH: &str = "#!/bin/sh\ncat \"$DECKHAND_HOME/gh-pr.json\" 2>/dev/null || { echo 'no pull requests found for branch' >&2; exit 1; }\n";

fn status(dir: &Path) -> String {
    git_try(dir, &["status", "--porcelain", "--untracked-files=all"]).1
}

// ported from tests/mergeDaemon.test.ts: done marker, worktree-only M, merge with commit-first, conflict keep/abort and merge detection over IPC
#[test]
fn done_marker_worktree_only_m_merge_with_commit_first_conflict_keep_abort_and_merge_detection_over_ipc() {
    let repo_dir = repo();
    let sandbox = Sandbox::new("merge");
    let root_path = fs::canonicalize(repo_dir.path()).expect("realpath");
    let root_s = p(&root_path);
    let root = root_s.as_str();
    let home = sandbox.home();
    let bin = install_agents(&sandbox);
    fs::write(bin.join("gh"), FAKE_GH).expect("fake gh");
    fs::set_permissions(bin.join("gh"), fs::Permissions::from_mode(0o755)).expect("chmod gh");
    let mut daemon = launch(&sandbox, &root_path, &bin);
    let socket = sandbox.socket();
    ready(&socket);
    let live = subscribed(&socket, root);

    // D: a main-checkout session is marked done (persisted, broadcast) and back; M there refuses and points at D.
    let main_session = create(&socket, root, "tidy readme", "none", json!({}));
    let main_id = main_session["id"].as_str().unwrap().to_string();
    running(&socket, &main_id);
    let done = must(
        &socket,
        json!({"type": "set-session-done", "sessionId": main_id, "done": true}),
    );
    assert!(!falsy(&done["doneAt"]));
    updated(&live, 0, &main_id, |s| s["doneAt"] == done["doneAt"]);
    let persisted_done = |id: &str| -> Value {
        load_state(&home)["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == id)
            .map(|s| s["doneAt"].clone())
            .unwrap_or(Value::Null)
    };
    assert_eq!(persisted_done(&main_id), done["doneAt"]);
    // Marking it done again keeps the first time.
    assert_eq!(
        must(
            &socket,
            json!({"type": "set-session-done", "sessionId": main_id, "done": true})
        )["doneAt"],
        done["doneAt"]
    );
    rejects(
        call(
            &socket,
            json!({"type": "mark-session-merged", "sessionId": main_id, "targetCwd": root}),
        ),
        "Use Space to mark it done",
    );
    assert!(state(&socket, &main_id)["mergedAt"].is_null());
    assert!(
        must(
            &socket,
            json!({"type": "set-session-done", "sessionId": main_id, "done": false})
        )["doneAt"]
            .is_null()
    );
    updated(&live, 0, &main_id, |s| falsy(&s["doneAt"]));
    assert!(persisted_done(&main_id).is_null());

    // m with commit-first: a worktree with only uncommitted work, merged through the toggle; done is independent of merged.
    let feature = create(&socket, root, "commit first", "new", json!({}));
    let feature_id = feature["id"].as_str().unwrap().to_string();
    let feature_state = running(&socket, &feature_id);
    let wt = PathBuf::from(feature_state["cwd"].as_str().unwrap());
    let child = create(
        &socket,
        root,
        "commit first child",
        "none",
        json!({"cwd": p(&wt), "parentSessionId": feature_id, "subSessionKind": "clean"}),
    );
    let child_id = child["id"].as_str().unwrap().to_string();
    running(&socket, &child_id);
    assert!(!falsy(
        &must(
            &socket,
            json!({"type": "set-session-done", "sessionId": child_id, "done": true})
        )["doneAt"]
    ));
    assert!(state(&socket, &feature_id)["doneAt"].is_null());
    fs::write(wt.join("feature.txt"), "feature\n").unwrap();
    let preview = must(
        &socket,
        json!({"type": "merge-preview", "sessionId": feature_id, "targetCwd": root}),
    );
    assert_eq!(
        json!([
            preview["commitCount"],
            preview["uncommitted"],
            preview["targetBranch"],
            preview["targetIsMain"],
            preview["targetRoot"]
        ]),
        json!([0, 1, "main", true, root])
    );
    let skipped = must(
        &socket,
        json!({"type": "merge-worktree", "sessionId": feature_id, "mode": "squash", "targetCwd": root, "commitFirst": false}),
    );
    assert_eq!(skipped["skipped"], true);
    assert!(status(&wt).contains("feature.txt"));
    let merged = must(
        &socket,
        json!({"type": "merge-worktree", "sessionId": feature_id, "mode": "squash", "targetCwd": root, "commitFirst": true}),
    );
    assert!(falsy(&merged["skipped"]) && falsy(&merged["conflicted"]));
    assert_eq!(merged["committed"]["files"], 1);
    assert_eq!(git_try(&wt, &["log", "-1", "--format=%s"]).1, "commit first");
    let source_sha = git_try(&wt, &["rev-parse", "HEAD"]).1;
    assert_eq!(merged["sourceSha"], json!(source_sha));
    updated(&live, 0, &child_id, |s| {
        s["worktree"]["mergeSourceSha"] == json!(source_sha) && s["worktree"]["mergeMode"] == "squash"
    });
    assert!(!falsy(&state(&socket, &child_id)["doneAt"])); // Still done, and now merged.
    let record_of = |id: &Value| -> Value {
        load_state(&home)["worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == *id)
            .cloned()
            .unwrap_or(Value::Null)
    };
    assert_eq!(
        record_of(&feature_state["worktree"]["id"])["mergeSourceSha"],
        json!(source_sha)
    );
    assert!(git_try(&root_path, &["commit", "-m", "squashed commit first"]).0);
    // After that squash, deleting the branch no longer asks for DELETE for its commits.
    let inspection = must(
        &socket,
        json!({"type": "inspect-cleanup", "sessionId": feature_id, "deleteBranch": true}),
    );
    let reasons: Vec<String> = inspection["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap().to_string())
        .collect();
    assert!(
        !reasons.iter().any(|r| r.contains("commit(s) exist only")),
        "{}",
        reasons.join(",")
    );

    // Conflicts: abort (normal merge), then keep (squash): marked merged now, the merge left in progress.
    let conflicting = create(&socket, root, "conflicting", "new", json!({}));
    let conflicting_id = conflicting["id"].as_str().unwrap().to_string();
    let conflicting_state = running(&socket, &conflicting_id);
    let conflicting_cwd = PathBuf::from(conflicting_state["cwd"].as_str().unwrap());
    fs::write(conflicting_cwd.join("file.txt"), "feature side\n").unwrap();
    assert!(git_try(&conflicting_cwd, &["commit", "-am", "feature side"]).0);
    fs::write(root_path.join("file.txt"), "main side\n").unwrap();
    assert!(git_try(&root_path, &["commit", "-am", "main side"]).0);
    fs::write(root_path.join("scratch.txt"), "unrelated\n").unwrap();
    let normal = must(
        &socket,
        json!({"type": "merge-worktree", "sessionId": conflicting_id, "mode": "merge", "targetCwd": root}),
    );
    assert_eq!(
        json!([normal["conflicted"], normal["conflicts"], normal["conflictCount"]]),
        json!([true, ["file.txt"], 1])
    );
    assert!(state(&socket, &conflicting_id)["worktree"]["mergedAt"].is_null());
    assert_eq!(
        must(
            &socket,
            json!({"type": "merge-preview", "sessionId": conflicting_id, "targetCwd": root})
        )["inProgress"],
        "merge"
    );
    rejects(
        call(
            &socket,
            json!({"type": "merge-worktree", "sessionId": conflicting_id, "mode": "merge", "targetCwd": root}),
        ),
        "A merge is in progress in the target",
    );
    must(
        &socket,
        json!({"type": "resolve-merge", "sessionId": conflicting_id, "targetCwd": root, "action": "abort"}),
    );
    assert_eq!(status(&root_path), "?? scratch.txt");
    assert!(state(&socket, &conflicting_id)["worktree"]["mergedAt"].is_null());
    let squash = must(
        &socket,
        json!({"type": "merge-worktree", "sessionId": conflicting_id, "mode": "squash", "targetCwd": root}),
    );
    assert_eq!(squash["conflicted"], true);
    let kept = must(
        &socket,
        json!({"type": "resolve-merge", "sessionId": conflicting_id, "targetCwd": root, "action": "keep"}),
    );
    assert!(!falsy(&kept["worktree"]["mergedAt"]));
    assert_eq!(kept["worktree"]["mergeMode"], "squash");
    assert_eq!(
        kept["worktree"]["mergeSourceSha"],
        json!(git_try(&conflicting_cwd, &["rev-parse", "HEAD"]).1)
    );
    assert!(status(&root_path).lines().any(|line| line == "UU file.txt"));
    assert_eq!(
        must(
            &socket,
            json!({"type": "merge-preview", "sessionId": feature_id, "targetCwd": root})
        )["inProgress"],
        "conflicts"
    );
    // The user resolves it later; here the squash is undone (a squash Deckhand started on a clean index).
    rejects(
        call(
            &socket,
            json!({"type": "merge-worktree", "sessionId": conflicting_id, "mode": "squash", "targetCwd": root}),
        ),
        "The target has unresolved conflicts",
    );
    assert!(git_try(&root_path, &["reset", "--merge"]).0);
    // A squash abort Deckhand cannot vouch for is refused (it did not start this one).
    let conflicting_branch = conflicting_state["worktree"]["branch"].as_str().unwrap();
    git_try(&root_path, &["merge", "--squash", conflicting_branch]);
    rejects(
        call(
            &socket,
            json!({"type": "resolve-merge", "sessionId": feature_id, "targetCwd": root, "action": "abort"}),
        ),
        "Cannot abort this squash merge safely",
    );
    assert!(git_try(&root_path, &["reset", "--merge"]).0);

    // Detection by ancestry: a branch merged into main outside Deckhand is marked; a fresh branch never.
    let outside = create(&socket, root, "merged outside", "new", json!({}));
    let outside_id = outside["id"].as_str().unwrap().to_string();
    let outside_state = running(&socket, &outside_id);
    let outside_cwd = PathBuf::from(outside_state["cwd"].as_str().unwrap());
    let fresh = create(&socket, root, "fresh branch", "new", json!({}));
    let fresh_id = fresh["id"].as_str().unwrap().to_string();
    running(&socket, &fresh_id);
    fs::write(outside_cwd.join("outside.txt"), "outside\n").unwrap();
    assert!(git_try(&outside_cwd, &["add", "."]).0);
    assert!(git_try(&outside_cwd, &["commit", "-m", "outside"]).0);
    assert!(
        git_try(
            &root_path,
            &[
                "merge",
                "--no-ff",
                "-m",
                "merge outside",
                outside_state["worktree"]["branch"].as_str().unwrap()
            ]
        )
        .0
    );
    let mark = live.events().len();
    must(&socket, json!({"type": "workspace-summary", "sessionId": outside_id}));
    updated(&live, mark, &outside_id, |s| {
        s["worktree"]["mergeDetected"] == "ancestor" && !falsy(&s["worktree"]["mergedAt"])
    });
    let detected = state(&socket, &outside_id);
    assert_eq!(
        json!([
            detected["worktree"]["mergeTargetBranch"],
            detected["worktree"]["mergeSourceRef"],
            detected["worktree"]["mergeSourceSha"]
        ]),
        json!([
            "main",
            "merged_outside",
            git_try(&outside_cwd, &["rev-parse", "HEAD"]).1
        ])
    );
    must(&socket, json!({"type": "workspace-summary", "sessionId": fresh_id}));
    pause(400);
    assert!(state(&socket, &fresh_id)["worktree"]["mergedAt"].is_null());
    // M clears a detected marker, and it stays cleared until the branch moves on.
    must(
        &socket,
        json!({"type": "mark-session-merged", "sessionId": outside_id, "targetCwd": root}),
    );
    pause(4200); // Past the summary cache.
    must(&socket, json!({"type": "workspace-summary", "sessionId": outside_id}));
    pause(400);
    assert!(state(&socket, &outside_id)["worktree"]["mergedAt"].is_null());

    // Detection by PR: gh reports the PR MERGED (a GitHub squash merge: nothing local is merged).
    let pr = create(&socket, root, "squashed on github", "new", json!({}));
    let pr_id = pr["id"].as_str().unwrap().to_string();
    let pr_state = running(&socket, &pr_id);
    let pr_cwd = PathBuf::from(pr_state["cwd"].as_str().unwrap());
    fs::write(pr_cwd.join("pr.txt"), "pr\n").unwrap();
    assert!(git_try(&pr_cwd, &["add", "."]).0);
    assert!(git_try(&pr_cwd, &["commit", "-m", "pr work"]).0);
    let pr_head = git_try(&pr_cwd, &["rev-parse", "HEAD"]).1;
    must(&socket, json!({"type": "workspace-summary", "sessionId": pr_id}));
    pause(300);
    assert!(state(&socket, &pr_id)["worktree"]["mergedAt"].is_null());
    fs::write(
        home.join("gh-pr.json"),
        json!({"number": 12, "url": "https://github.com/acme/app/pull/12", "state": "MERGED", "statusCheckRollup": [], "headRefOid": pr_head, "baseRefName": "main"}).to_string(),
    )
    .unwrap();
    let summary = must(
        &socket,
        json!({"type": "workspace-summary", "sessionId": pr_id, "includePr": true}),
    );
    assert_eq!(summary["pr"]["state"], "MERGED");
    updated(&live, 0, &pr_id, |s| s["worktree"]["mergeDetected"] == "pr");
    assert_eq!(state(&socket, &pr_id)["worktree"]["mergeSourceSha"], json!(pr_head));
    // The squash on GitHub left the commit "unmerged" locally; the recorded head makes it integrated for cleanup.
    let pr_inspection = must(
        &socket,
        json!({"type": "inspect-cleanup", "sessionId": pr_id, "deleteBranch": true}),
    );
    let pr_reasons: Vec<&str> = pr_inspection["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap())
        .collect();
    assert!(
        !pr_reasons.iter().any(|r| r.contains("commit(s) exist only")),
        "{}",
        pr_reasons.join(",")
    );
    assert_eq!(record_of(&pr_state["worktree"]["id"])["mergeDetected"], "pr");
    deckhand_e2e::harness::stop(&mut daemon);
}
