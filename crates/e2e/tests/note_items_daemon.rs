//! Port of tests/noteItemsDaemon.test.ts (black-box: drives DECKHAND_TEST_BIN).

use deckhand_e2e::harness::repo;
use deckhand_e2e::tasks::*;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use deckhand_e2e::daemon::*;
use deckhand_e2e::harness::Sandbox;
use serde_json::{Value, json};

// ported from tests/noteItemsDaemon.test.ts: open note items go to Tasks at once: A, a merge, deletion from W and x with deletion
#[test]
fn open_note_items_go_to_tasks_at_once_a_merge_deletion_from_w_and_x_with_deletion() {
    let repo_dir = repo();
    let sandbox = Sandbox::new("items");
    let root_path = fs::canonicalize(repo_dir.path()).expect("realpath");
    let root_s = p(&root_path);
    let root = root_s.as_str();
    let home = sandbox.home();
    let bin = install_agents(&sandbox, deckhand_e2e::fake_agent!());
    let mut daemon = launch(&sandbox, &root_path, &bin);
    let socket = sandbox.socket();
    ready(&socket);
    let create_new = |title: &str| create(&socket, root, title, "new", json!({}));
    let running_notes = |id: &str| {
        deckhand_e2e::harness::wait_for(
            || state(&socket, id),
            |item| item["status"] == "running" && !falsy(&item["sharedNotes"]),
            Duration::from_secs(20),
        )
    };
    let tasks = || {
        parse_tasks(
            must(&socket, json!({"type": "watch-tasks", "cwd": root}))["text"]
                .as_str()
                .unwrap(),
        )
    };
    let find = |list: &[Task], title: &str| list.iter().find(|t| t.title == title).cloned();
    // Writes the worktree's note through the daemon, as the Notes tab does.
    let write_note = |session: &Value, text: &str| -> Value {
        let current = state(&socket, session["id"].as_str().unwrap());
        let saved = must(
            &socket,
            json!({"type": "save-note", "sessionId": session["id"], "section": "shared", "noteId": format!("worktree:{}", current["worktree"]["id"].as_str().unwrap()), "text": text, "revision": current["sharedNotes"]["revision"]}),
        );
        assert_eq!(saved["saved"], true);
        saved["session"].clone()
    };
    let shared_text = |id: &str| -> String {
        state(&socket, id)["sharedNotes"]["text"]
            .as_str()
            .unwrap_or("")
            .to_string()
    };

    // A in the Notes view: every open item of the note to the backlog (work of its own; the note's work's to-dos are its steps).
    let one = running_notes(&id_of(&create_new("items one")));
    let wt1 = one["worktree"]["id"].as_str().unwrap().to_string();
    let saved = write_note(&one, "- [x] Done already\n- [ ] Parse tabs\n- [ ] Parse spaces\nprose");
    rejects(
        call(
            &socket,
            json!({"type": "send-open-items", "sessionId": one["id"], "section": "shared", "noteId": format!("worktree:{wt1}"), "revision": "stale"}),
        ),
        "changed meanwhile",
    );
    let sent = must(
        &socket,
        json!({"type": "send-open-items", "sessionId": one["id"], "section": "shared", "noteId": format!("worktree:{wt1}"), "revision": saved["sharedNotes"]["revision"]}),
    );
    assert_eq!(sent["sent"], 2);
    let mut list = tasks();
    for title in ["Parse tabs", "Parse spaces"] {
        let task = find(&list, title).expect(title);
        assert_eq!(meta_of(&task, "wt"), None);
        assert!(!task.done);
        let text = sent["session"]["sharedNotes"]["text"].as_str().unwrap();
        assert!(
            text.contains(&format!("- \u{2197} {title} <!-- dh:t={} -->", task.id)),
            "{text}"
        );
    }
    assert!(
        sent["session"]["sharedNotes"]["text"]
            .as_str()
            .unwrap()
            .starts_with("- [x] Done already\n")
    );

    // A clean merge with sendOpenItems: the items left in its note go to the backlog, marked as left open in its branch.
    write_note(
        &one,
        &format!(
            "{}\n- [ ] Benchmark it",
            sent["session"]["sharedNotes"]["text"].as_str().unwrap()
        ),
    );
    let one_cwd = PathBuf::from(one["cwd"].as_str().unwrap());
    fs::write(one_cwd.join("feature.txt"), "feature\n").unwrap();
    assert!(git_try(&one_cwd, &["add", "."]).0);
    assert!(git_try(&one_cwd, &["commit", "-m", "feature"]).0);
    let merged = must(
        &socket,
        json!({"type": "merge-worktree", "sessionId": one["id"], "mode": "merge", "targetCwd": root, "sendOpenItems": true}),
    );
    assert_eq!(merged["sentItems"], 1);
    list = tasks();
    let benchmark = find(&list, "Benchmark it").expect("Benchmark it");
    assert_eq!(meta_of(&benchmark, "wt"), None);
    assert_eq!(
        meta_of(&benchmark, "from").as_deref(),
        one["worktree"]["branch"].as_str()
    );
    assert!(shared_text(&id_of(&one)).contains("- \u{2197} Benchmark it <!-- dh:t="));

    // Deleting from W with sendOpenItems: before the worktree goes (its running session is stopped too).
    let two = running_notes(&id_of(&create_new("items two")));
    write_note(&two, "- [ ] Write the docs");
    let deleted = must(
        &socket,
        json!({"type": "delete-worktree", "cwd": root, "path": two["worktree"]["path"], "branch": two["worktree"]["branch"], "stopSessions": true, "sendOpenItems": true}),
    );
    assert_eq!(deleted["removed"], "deleted");
    assert_eq!(deleted["sentItems"], 1);
    let docs = find(&tasks(), "Write the docs").expect("Write the docs");
    assert_eq!(meta_of(&docs, "from").as_deref(), two["worktree"]["branch"].as_str());
    assert!(!docs.done);
    // Without it, the items stay in the (now read-only) note.
    let three = running_notes(&id_of(&create_new("items three")));
    write_note(&three, "- [ ] Stays put");
    assert!(
        must(
            &socket,
            json!({"type": "delete-worktree", "cwd": root, "path": three["worktree"]["path"], "branch": three["worktree"]["branch"], "stopSessions": true}),
        )["sentItems"]
            .is_null()
    );
    assert!(find(&tasks(), "Stays put").is_none());
    assert_eq!(shared_text(&id_of(&three)), "- [ ] Stays put");

    // x with the worktree deleted (kill sendOpenItems): sent before the worktree is removed.
    let four = running_notes(&id_of(&create_new("items four")));
    write_note(&four, "- [ ] Clean up flags");
    must(
        &socket,
        json!({"type": "kill", "sessionId": four["id"], "deleteWorktree": true, "sendOpenItems": true}),
    );
    deckhand_e2e::harness::wait_for(
        || state(&socket, &id_of(&four)),
        |item| item["status"] == "exited" && !falsy(&item["worktree"]["deletedAt"]),
        Duration::from_secs(20),
    );
    let flags = find(&tasks(), "Clean up flags").expect("Clean up flags");
    assert_eq!(meta_of(&flags, "from").as_deref(), four["worktree"]["branch"].as_str());
    assert!(shared_text(&id_of(&four)).contains("- \u{2197} Clean up flags <!-- dh:t="));

    // Follow-ups of the old model become steps at daemon start: an open task assigned to a live worktree moves into its
    // note; one assigned to a deleted worktree stays, in the backlog; the old bookkeeping goes.
    deckhand_e2e::harness::stop(&mut daemon);
    let tasks_dir = home.join("notes").join("tasks");
    let mut files: Vec<String> = fs::read_dir(&tasks_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".md"))
        .collect();
    files.sort();
    let file = tasks_dir.join(&files[0]);
    let legacy = format!(
        "- [ ] Legacy step <!-- dh:t=1e9acy01 wt={wt1} assigned=2026-10-01 -->\n  its details\n- [ ] Orphan <!-- dh:t=1e9acy02 wt={} assigned=2026-10-01 -->\n",
        four["worktree"]["id"].as_str().unwrap()
    );
    let mut existing = fs::read_to_string(&file).unwrap();
    existing.push_str(&legacy);
    fs::write(&file, existing).unwrap();
    daemon = launch(&sandbox, &root_path, &bin);
    ready(&socket);
    let after = tasks();
    assert!(find(&after, "Legacy step").is_none());
    let orphan = find(&after, "Orphan").expect("Orphan");
    assert_eq!(
        orphan.meta,
        std::collections::BTreeMap::from([("t".to_string(), "1e9acy02".to_string())])
    );
    let one_note = shared_text(&id_of(&one));
    assert!(one_note.ends_with("- [ ] Legacy step\n  its details\n"), "{one_note:?}");
    assert!(!fs::read_to_string(&file).unwrap().contains("assigned="));
    deckhand_e2e::harness::stop(&mut daemon);
}
