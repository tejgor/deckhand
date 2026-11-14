//! Open steps when the work ends, black-box: drives DECKHAND_TEST_BIN. A session's steps are the lines under its line;
//! merging its worktree ticks the line and moves its open steps up out of it (unless the merge keeps them); deleting
//! the worktree unmerged leaves the line, with its steps, where it is.

use deckhand_e2e::harness::repo;
use deckhand_e2e::tasks::*;
use std::fs;
use std::path::PathBuf;

use deckhand_e2e::daemon::*;
use deckhand_e2e::harness::{Sandbox, wait_for_default};
use serde_json::{Value, json};

#[test]
fn open_steps_move_up_when_the_work_merges_and_stay_when_it_is_deleted() {
    let repo_dir = repo();
    let sandbox = Sandbox::new("items");
    let root_path = fs::canonicalize(repo_dir.path()).expect("realpath");
    let root_s = p(&root_path);
    let root = root_s.as_str();
    let bin = install_agents(&sandbox, deckhand_e2e::fake_agent!());
    let mut daemon = launch(&sandbox, &root_path, &bin);
    let socket = sandbox.socket();
    ready(&socket);
    let op = |value: Value| must(&socket, json!({"type": "task-op", "cwd": root, "op": value}));
    let lines = || lines_of(&must(&socket, json!({"type": "watch-tasks", "cwd": root})));
    let line = |title: &str| line_titled(&lines(), title).unwrap_or_else(|| panic!("no line {title}"));
    // A running session in a new worktree, with a line and these steps (the first ticked).
    let with_steps = |title: &str, steps: &[&str]| -> (Value, ListLine) {
        let session = running(&socket, &id_of(&create(&socket, root, title, "new", json!({}))));
        let session = wait_for_default(
            || state(&socket, &id_of(&session)),
            |item| !falsy(&item["worktree"]["branch"]),
        );
        op(json!({"type": "session-line", "s": session["id"]}));
        let own = line(title);
        assert_eq!(own.meta("s"), session["id"].as_str());
        for step in steps {
            op(json!({"type": "add", "title": step, "parent": own.id}));
        }
        if let Some(first) = steps.first() {
            op(json!({"type": "tick", "id": line(first).id}));
        }
        (session, own)
    };
    let commit = |session: &Value| {
        let cwd = PathBuf::from(session["cwd"].as_str().unwrap());
        fs::write(cwd.join(format!("{}.txt", id_of(session))), "feature\n").unwrap();
        assert!(git_try(&cwd, &["add", "."]).0);
        assert!(git_try(&cwd, &["commit", "-m", "feature"]).0);
    };

    // A clean merge: the line ticks, its open steps move up beside it (to the top level), marked as left open in the
    // branch; the ticked step stays under it.
    let (one, one_line) = with_steps("items one", &["Done already", "Parse tabs", "Parse spaces"]);
    commit(&one);
    let merged = must(
        &socket,
        json!({"type": "merge-worktree", "sessionId": one["id"], "mode": "merge", "targetCwd": root}),
    );
    assert_eq!(merged["liftedSteps"], 2);
    let after = lines();
    let ticked = line_titled(&after, "items one").unwrap();
    assert!(ticked.done && ticked.meta("auto") == Some("merge"));
    assert_eq!(titles_under(&after, &one_line.id), ["Done already"]);
    for title in ["Parse tabs", "Parse spaces"] {
        let lifted = line_titled(&after, title).unwrap();
        assert_eq!(lifted.parent, None, "{title}");
        assert!(!lifted.done && lifted.meta("s").is_none());
        assert_eq!(lifted.meta("from"), one["worktree"]["branch"].as_str());
    }

    // The merge stops before committing: commit it, as you would.
    assert!(git_try(&root_path, &["commit", "-m", "merged items one"]).0);

    // keepOpenSteps: they stay under the ticked line.
    let (two, two_line) = with_steps("items two", &["Kept step"]);
    op(json!({"type": "reopen", "id": line("Kept step").id}));
    commit(&two);
    let merged = must(
        &socket,
        json!({"type": "merge-worktree", "sessionId": two["id"], "mode": "squash", "targetCwd": root, "keepOpenSteps": true}),
    );
    assert!(merged["liftedSteps"].is_null(), "{merged}");
    let after = lines();
    assert!(line_titled(&after, "items two").unwrap().done);
    assert_eq!(titles_under(&after, &two_line.id), ["Kept step"]);

    // Deleting from W, unmerged: the line stays where it is with its steps, nobody on it, tried in the branch.
    let (three, three_line) = with_steps("items three", &["Done", "Write the docs"]);
    let deleted = must(
        &socket,
        json!({"type": "delete-worktree", "cwd": root, "path": three["worktree"]["path"], "branch": three["worktree"]["branch"], "stopSessions": true}),
    );
    assert_eq!(deleted["removed"], "deleted");
    assert!(deleted.get("sentItems").is_none());
    let left = wait_for_default(|| line("items three"), |left| left.meta("s").is_none());
    assert!(!left.done);
    assert_eq!(left.meta("tried"), three["worktree"]["branch"].as_str());
    assert_eq!(titles_under(&lines(), &three_line.id), ["Done", "Write the docs"]);

    // x with the worktree deleted: the same.
    let (four, _) = with_steps("items four", &["Done", "Clean up flags"]);
    must(
        &socket,
        json!({"type": "kill", "sessionId": four["id"], "deleteWorktree": true}),
    );
    let left = wait_for_default(|| line("items four"), |left| left.meta("s").is_none());
    assert_eq!(left.meta("tried"), four["worktree"]["branch"].as_str());
    assert!(line_titled(&lines(), "Clean up flags").is_some());
    deckhand_e2e::harness::stop(&mut daemon);
}
