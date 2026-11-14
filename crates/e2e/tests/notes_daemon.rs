//! Notes, black-box: drives DECKHAND_TEST_BIN. Every session has its own note; a line has a note (and a brief); the
//! repository has a scratchpad. Each is a Markdown file under `notes/`.

use deckhand_e2e::harness::repo;
use deckhand_e2e::tasks::*;
use std::fs;
use std::path::{Path, PathBuf};

use deckhand_e2e::client::Client;
use deckhand_e2e::daemon::*;
use deckhand_e2e::harness::{Sandbox, wait_for_default};
use serde_json::{Value, json};

fn kill_and_wait(socket: &Path, id: &str) {
    must(socket, json!({"type": "kill", "sessionId": id}));
    wait_for_default(|| state(socket, id), |item| item["status"] == "exited");
}

/// Saves a session's own note (at its current revision unless `revision`).
fn save(socket: &Path, id: &str, text: &str, revision: Option<&str>) -> Result<Value, String> {
    let base = match revision {
        Some(revision) => revision.to_string(),
        None => state(socket, id)["notesFile"]["revision"].as_str().unwrap().to_string(),
    };
    call(
        socket,
        json!({"type": "save-note", "note": {"kind": "session", "id": id}, "text": text, "revision": base}),
    )
}

fn names(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn notes_are_markdown_files() {
    let repo_dir = repo();
    let sandbox = Sandbox::new("notes");
    let root_path = fs::canonicalize(repo_dir.path()).expect("realpath");
    let root_s = p(&root_path);
    let root = root_s.as_str();
    let home = sandbox.home();
    let bin = install_agents(&sandbox, deckhand_e2e::fake_agent!());
    let socket = sandbox.socket();

    // State written by an older version: the note lives in state.json.
    let legacy_id = uuid::Uuid::new_v4().to_string();
    let at = "2026-10-19T00:00:00.000Z";
    fs::create_dir_all(&home).unwrap();
    let write_state = |value: &Value| {
        fs::write(
            home.join("state.json"),
            format!("{}\n", serde_json::to_string_pretty(value).unwrap()),
        )
        .unwrap()
    };
    write_state(
        &json!({"sessions": [{"id": legacy_id, "title": "legacy", "program": "claude", "command": "claude", "cwd": root, "repoRoot": root, "launchWorktreeRoot": root, "worktree": {"mode": "none"}, "status": "exited", "createdAt": at, "updatedAt": at, "notes": "legacy note\n- [ ] carry over"}], "worktrees": []}),
    );
    let mut daemon = launch(&sandbox, &root_path, &bin);
    ready(&socket);
    let lines = || lines_of(&must(&socket, json!({"type": "watch-tasks", "cwd": root})));

    // Legacy notes move into their file, then (the work-items migration) into the list: the session's line, with the
    // note's checkbox as a step and its prose as the line's note; state.json stops carrying notes.
    {
        let legacy = state(&socket, &legacy_id);
        let file = home.join("notes").join("sessions").join(format!("{legacy_id}.md"));
        assert_eq!(legacy["notes"], "");
        assert_eq!(legacy["notesFile"]["path"], json!(p(&file)));
        let list = lines();
        let line = line_titled(&list, "legacy").expect("the session's line");
        assert_eq!(line.meta("s"), Some(legacy_id.as_str()));
        assert_eq!(titles_under(&list, &line.id), ["carry over"]);
        assert_eq!(
            text_of(&home.join("notes").join("lines").join(format!("{}.md", line.id))).as_deref(),
            Some("legacy note\n")
        );
        assert!(home.join("notes").join("work-items-migrated.json").exists());
        assert!(load_state(&home)["sessions"][0]["notes"].is_null());
        let raw = fs::read_to_string(home.join("state.json")).unwrap();
        assert!(!raw.contains("legacy note") && !raw.contains("notesFile") && !raw.contains("sharedNotes"));
        // Idempotent: a state.json that still has (older) notes never overwrites a session's file, which wins, and the
        // migration does not run again.
        fs::write(&file, "edited since").unwrap();
        deckhand_e2e::harness::stop(&mut daemon);
        let mut stored = load_state(&home);
        for session in stored["sessions"].as_array_mut().unwrap() {
            session["notes"] = json!("stale copy");
        }
        write_state(&stored);
        daemon = launch(&sandbox, &root_path, &bin);
        ready(&socket);
        assert_eq!(state(&socket, &legacy_id)["notes"], "edited since");
        assert_eq!(text_of(&file).as_deref(), Some("edited since"));
        assert_eq!(lines().len(), 2, "the list as it was");
        assert!(load_state(&home)["sessions"][0]["notes"].is_null());
    }

    let live = subscribed(&socket, root);

    // Saves are revision-checked and broadcast; an editor's edit (in place or by rename) shows up; a stale save is
    // refused.
    {
        let file = PathBuf::from(state(&socket, &legacy_id)["notesFile"]["path"].as_str().unwrap());
        let mark = live.events().len();
        let saved = save(&socket, &legacy_id, "# Plan\n- [ ] one", None).unwrap();
        assert_eq!(saved["saved"], true);
        assert_eq!(saved["note"]["kind"], "session");
        assert_eq!(saved["note"]["text"], "# Plan\n- [ ] one");
        assert_eq!(saved["session"]["notes"], "# Plan\n- [ ] one");
        assert_eq!(text_of(&file).as_deref(), Some("# Plan\n- [ ] one"));
        updated(&live, mark, &legacy_id, |s| s["notes"] == "# Plan\n- [ ] one");
        let base = state(&socket, &legacy_id)["notesFile"]["revision"]
            .as_str()
            .unwrap()
            .to_string();
        fs::write(&file, "edited in VS Code").unwrap();
        updated(&live, mark, &legacy_id, |s| s["notes"] == "edited in VS Code");
        let temporary = file.parent().unwrap().join("vim-swap.tmp");
        fs::write(&temporary, "saved via rename").unwrap();
        fs::rename(&temporary, &file).unwrap();
        updated(&live, mark, &legacy_id, |s| s["notes"] == "saved via rename");
        let stale = save(&socket, &legacy_id, "clobber", Some(&base)).unwrap();
        assert_eq!(stale["saved"], false);
        assert_eq!(stale["session"]["notes"], "saved via rename");
        assert_eq!(text_of(&file).as_deref(), Some("saved via rename"));
        // Text is capped; a file made longer elsewhere is shown cut and only editable in an editor.
        let long = "x".repeat(50_010);
        let capped = save(&socket, &legacy_id, &long, None).unwrap();
        assert_eq!(
            capped["session"]["notes"].as_str().map(|s| s.chars().count()),
            Some(50_000)
        );
        fs::write(&file, "y".repeat(60_000)).unwrap();
        updated(&live, mark, &legacy_id, |s| !falsy(&s["notesFile"]["tooLarge"]));
        rejects(save(&socket, &legacy_id, "short", None), "longer than");
        fs::write(&file, "back to normal").unwrap();
        updated(&live, mark, &legacy_id, |s| {
            s["notes"] == "back to normal" && falsy(&s["notesFile"]["tooLarge"])
        });
    }

    // Every session has its own note, sessions sharing a worktree too; a line's note reaches the clients watching the
    // list; a handoff carries the line, its note and the session's own note.
    let first = running(&socket, &id_of(&create(&socket, root, "notes-a", "new", json!({}))));
    let attached = running(
        &socket,
        &id_of(&create(
            &socket,
            root,
            "notes-b",
            "existing",
            json!({"existingWorktreePath": first["cwd"]}),
        )),
    );
    let first_id = id_of(&first);
    let attached_id = id_of(&attached);
    let line_id;
    {
        for session in [&first, &attached] {
            assert!(session.get("sharedNotes").is_none(), "{session}");
        }
        save(&socket, &first_id, "mine: check the diff", None).unwrap();
        save(&socket, &attached_id, "theirs", None).unwrap();
        assert_eq!(state(&socket, &first_id)["notes"], "mine: check the diff");
        assert_eq!(state(&socket, &attached_id)["notes"], "theirs");
        let mut watcher = Client::connect(&socket).expect("watcher");
        watcher
            .request(json!({"type": "watch-tasks", "cwd": root}))
            .expect("watch-tasks");
        let doc = must(
            &socket,
            json!({"type": "task-op", "cwd": root, "op": {"type": "session-line", "s": first_id}}),
        );
        line_id = line_titled(&lines_of(&doc), "notes-a").expect("its line").id;
        let note = json!({"kind": "line", "id": line_id});
        let saved = must(
            &socket,
            json!({"type": "save-note", "note": note, "text": "shared plan", "revision": "e3b0c44298fc1c14"}),
        );
        assert_eq!(saved["saved"], true);
        assert!(saved.get("session").is_none());
        wait_for_default(
            || {
                watcher
                    .events_of("work-note-updated")
                    .iter()
                    .any(|e| e["note"]["id"] == json!(line_id) && e["note"]["text"] == "shared plan")
            },
            |found| *found,
        );
        assert_eq!(
            text_of(&home.join("notes").join("lines").join(format!("{line_id}.md"))).as_deref(),
            Some("shared plan")
        );
        let handoff_path = must(&socket, json!({"type": "export-handoff", "sessionId": first_id}));
        let handoff = fs::read_to_string(handoff_path.as_str().unwrap()).unwrap();
        assert!(
            handoff.contains(
                "## Task\n\n[ ] notes-a\n\n## Task notes\n\nshared plan\n\n## Session notes\n\nmine: check the diff"
            ),
            "{handoff}"
        );
        // open-note creates a missing file for the editor, any kind.
        let own_file = must(
            &socket,
            json!({"type": "open-note", "note": {"kind": "session", "id": attached_id}}),
        );
        assert_eq!(
            own_file,
            json!(p(&home
                .join("notes")
                .join("sessions")
                .join(format!("{attached_id}.md"))))
        );
        let brief_file = must(
            &socket,
            json!({"type": "open-note", "note": {"kind": "brief", "id": line_id}}),
        );
        let expected = home.join("notes").join("briefs").join(format!("{line_id}.md"));
        assert_eq!(brief_file, json!(p(&expected)));
        assert_eq!(text_of(&expected).as_deref(), Some(""));
        rejects(
            call(
                &socket,
                json!({"type": "open-note", "note": {"kind": "line", "id": "deadbeef"}}),
            ),
            "no longer in the list",
        );
    }

    // The repository's scratchpad outlives every session; a session's note goes with its session, to the trash.
    {
        let key = must(&socket, json!({"type": "watch-tasks", "cwd": root}))["key"]
            .as_str()
            .unwrap()
            .to_string();
        let one = running(&socket, &id_of(&create(&socket, root, "main-1", "none", json!({}))));
        let one_id = id_of(&one);
        must(
            &socket,
            json!({"type": "save-note", "note": {"kind": "scratchpad", "id": key}, "text": "repo-wide", "revision": "e3b0c44298fc1c14"}),
        );
        save(&socket, &one_id, "mine", None).unwrap();
        let own_file = PathBuf::from(state(&socket, &one_id)["notesFile"]["path"].as_str().unwrap());
        let scratchpad = home.join("notes").join("repos").join(format!("{key}.md"));
        kill_and_wait(&socket, &one_id);
        must(&socket, json!({"type": "remove", "sessionId": one_id}));
        assert!(!own_file.exists());
        assert_eq!(text_of(&scratchpad).as_deref(), Some("repo-wide"));
        let scratch = must(&socket, json!({"type": "watch-tasks", "cwd": root}))["notes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|note| note["kind"] == "scratchpad")
            .cloned()
            .unwrap();
        assert_eq!(scratch["text"], "repo-wide");
        let trash = home.join("notes").join("trash");
        let trashed: Vec<String> = names(&trash)
            .into_iter()
            .filter(|n| n.contains("_session_main-1_"))
            .collect();
        assert_eq!(trashed.len(), 1);
        assert_eq!(text_of(&trash.join(&trashed[0])).as_deref(), Some("mine"));
    }

    // A line's note outlives its sessions and their worktree.
    {
        kill_and_wait(&socket, &attached_id);
        must(
            &socket,
            json!({"type": "kill", "sessionId": first_id, "deleteWorktree": true, "allowDataLoss": true}),
        );
        wait_for_default(
            || state(&socket, &first_id),
            |item| !falsy(&item["worktree"]["deletedAt"]),
        );
        must(&socket, json!({"type": "remove", "sessionId": first_id}));
        must(&socket, json!({"type": "remove", "sessionId": attached_id}));
        let line = line_titled(&lines(), "notes-a").expect("the line stays");
        assert!(line.meta("s").is_none());
        assert_eq!(
            text_of(&home.join("notes").join("lines").join(format!("{line_id}.md"))).as_deref(),
            Some("shared plan")
        );
    }
    deckhand_e2e::harness::stop(&mut daemon);
}
