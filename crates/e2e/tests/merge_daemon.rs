//! Port of tests/mergeDaemon.test.ts (black-box: drives DECKHAND_TEST_BIN).

use deckhand_e2e::harness::repo;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use deckhand_e2e::daemon::*;
use deckhand_e2e::harness::Sandbox;
use serde_json::{Value, json};

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
    let bin = install_agents(&sandbox, deckhand_e2e::fake_agent!());
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
