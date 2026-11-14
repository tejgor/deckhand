//! The list, black-box: drives DECKHAND_TEST_BIN. One list per repository; it nests, and a session is on at most one
//! line of it (`docs/future/work-items.md`).

use deckhand_e2e::harness::repo;
use deckhand_e2e::tasks::*;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use deckhand_e2e::client::Client;
use deckhand_e2e::daemon::*;
use deckhand_e2e::harness::{Sandbox, wait_for_default};
use serde_json::{Value, json};

#[test]
fn one_list_per_repository_whose_lines_sessions_start_on_and_finish() {
    let repo_dir = repo();
    let elsewhere_dir = repo();
    let sandbox = Sandbox::new("tasks");
    let root_path = fs::canonicalize(repo_dir.path()).expect("realpath");
    let elsewhere_path = fs::canonicalize(elsewhere_dir.path()).expect("realpath");
    let root_s = p(&root_path);
    let root = root_s.as_str();
    assert!(git_try(&root_path, &["branch", "release"]).0);
    fs::write(root_path.join("main-only.txt"), "main\n").unwrap();
    assert!(git_try(&root_path, &["add", "."]).0);
    assert!(git_try(&root_path, &["commit", "-m", "main only"]).0);
    let home = sandbox.home();
    let bin = install_agents(&sandbox, deckhand_e2e::fake_agent!());
    let mut daemon = launch(&sandbox, &root_path, &bin);
    let socket = sandbox.socket();
    ready(&socket);
    let create_in = |title: &str, mode: &str, extra: Value| create(&socket, root, title, mode, extra);
    let op = |value: Value| call(&socket, json!({"type": "task-op", "cwd": root, "op": value}));
    let watched = || must(&socket, json!({"type": "watch-tasks", "cwd": root}));
    let current = || lines_of(&watched());
    let by_title = |title: &str| line_titled(&current(), title);
    let by_title_now = |title: &str| by_title(title).unwrap_or_else(|| panic!("no line {title}"));
    let kill_and_wait = |id: &str| {
        must(&socket, json!({"type": "kill", "sessionId": id}));
        wait_for_default(|| state(&socket, id), |item| item["status"] == "exited");
    };

    let mut watch = Client::connect(&socket).expect("watch client");
    watch
        .request(json!({"type": "watch-tasks", "cwd": root}))
        .expect("watch-tasks");
    let pushed = |matches: &dyn Fn(&[ListLine]) -> bool| {
        wait_for_default(
            || {
                watch
                    .events_of("tasks-updated")
                    .iter()
                    .any(|e| matches(&lines_of(&e["tasks"])))
            },
            |found| *found,
        );
    };

    // Ops change the file and reach watching clients; an add's note is its line's note; lines nest and roll up; an
    // editor's edit shows up; the old model's ops are refused.
    op(json!({"type": "add", "title": "First task", "note": "why it matters"})).unwrap();
    let doc = op(json!({"type": "add", "title": "Second task"})).unwrap();
    let key = doc["key"].as_str().unwrap().to_string();
    assert_eq!(
        doc["path"],
        json!(p(&home.join("notes").join("tasks").join(format!("{key}.md"))))
    );
    let first = by_title_now("First task");
    assert_eq!(
        text_of(&home.join("notes").join("lines").join(format!("{}.md", first.id))).as_deref(),
        Some("why it matters\n")
    );
    let notes = watched()["notes"].clone();
    assert!(
        notes.as_array().unwrap().iter().any(|note| note["kind"] == "line"
            && note["id"] == first.id.as_str()
            && note["text"] == "why it matters\n"),
        "{notes}"
    );
    pushed(&|next| next.len() == 2);
    let second = by_title_now("Second task");
    op(json!({"type": "add", "title": "Part B", "parent": second.id})).unwrap();
    op(json!({"type": "add", "title": "Part A", "parent": second.id, "at": 0})).unwrap();
    assert_eq!(titles_under(&current(), &second.id), ["Part A", "Part B"]);
    op(json!({"type": "tick", "id": by_title_now("Part A").id})).unwrap();
    op(json!({"type": "tick", "id": by_title_now("Part B").id})).unwrap();
    let rolled = by_title_now("Second task");
    assert!(rolled.done && rolled.meta("auto") == Some("lines"));
    op(json!({"type": "reopen", "id": by_title_now("Part B").id})).unwrap();
    assert!(!by_title_now("Second task").done);
    op(json!({"type": "remove", "id": by_title_now("Part A").id})).unwrap();
    op(json!({"type": "remove", "id": by_title_now("Part B").id})).unwrap();
    {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(doc["path"].as_str().unwrap())
            .unwrap();
        file.write_all(b"- [ ] From the editor\n").unwrap();
    }
    pushed(&|next| next.iter().any(|t| t.title == "From the editor"));
    // A line added in the editor has no ID yet; its provisional one still addresses it.
    let editor_task = parse_tasks(watched()["text"].as_str().unwrap())
        .into_iter()
        .find(|task| task.title == "From the editor")
        .unwrap();
    assert!(editor_task.id.starts_with('~'));
    let doc = op(json!({"type": "toggle", "id": editor_task.id})).unwrap();
    assert!(line_titled(&lines_of(&doc), "From the editor").unwrap().done);
    assert!(
        lines_of(&doc)
            .iter()
            .all(|t| t.id.len() == 8 && t.id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')))
    );
    for refused in [
        json!({"type": "link", "id": first.id, "link": {"s": "x"}}),
        json!({"type": "assign", "id": first.id, "link": {"wt": "w"}}),
        json!({"type": "edit", "id": first.id, "title": "x", "body": "y"}),
    ] {
        rejects(op(refused), "Unknown task change");
    }
    rejects(op(json!({"type": "toggle", "id": "gone"})), "no longer in the list");
    // A linked worktree of the same repository shares the list.
    let other = home.join("other-worktree");
    assert!(git_try(&root_path, &["worktree", "add", "-b", "other", &p(&other)]).0);
    assert_eq!(
        must(&socket, json!({"type": "watch-tasks", "cwd": p(&other)}))["key"],
        json!(key)
    );

    // A session started on a line is on it (`s=`), finds its title and note typed (not sent) in its agent's input and
    // starts at the chosen base.
    let first_session;
    {
        let branches = must(&socket, json!({"type": "list-branches", "cwd": root}));
        assert_eq!(branches["current"], "main");
        assert!(branches["branches"].as_array().unwrap().contains(&json!("release")));
        rejects(
            try_create(&socket, root, "wrong base", "none", json!({"baseBranch": "release"})),
            "only to a new worktree",
        );
        let session = create_in(
            "First task",
            "new",
            json!({"taskId": first.id, "baseBranch": "release"}),
        );
        let session_id = id_of(&session);
        // On the line at once, before its worktree is ready.
        assert_eq!(by_title_now("First task").meta("s"), Some(session_id.as_str()));
        running(&socket, &session_id);
        let started = state(&socket, &session_id);
        let trace: Value = serde_json::from_str(&wait_for_default(
            || text_of(&home.join(format!("trace-{session_id}.json"))).unwrap_or_default(),
            |text| !text.is_empty(),
        ))
        .unwrap();
        assert!(
            !trace["args"].as_array().unwrap().contains(&json!("--")),
            "nothing is passed as a first message"
        );
        // Once the agent's screen settles (idle), the line arrives as one bracketed paste, without Enter.
        let typed = deckhand_e2e::harness::wait_for(
            || text_of(&home.join(format!("input-{session_id}"))).unwrap_or_default(),
            |text| !text.is_empty(),
            Duration::from_secs(20),
        );
        assert_eq!(typed, "\x1b[200~First task: why it matters\x1b[201~");
        assert_eq!(started["worktree"]["baseRef"], "release");
        assert!(
            !PathBuf::from(started["cwd"].as_str().unwrap())
                .join("main-only.txt")
                .exists()
        );
        assert!(
            !fs::read_to_string(doc["path"].as_str().unwrap())
                .unwrap()
                .contains("wt=")
        );
        rejects(
            try_create(&socket, root, "again", "new", json!({"taskId": first.id})),
            "already being worked on",
        );
        rejects(
            try_create(
                &socket,
                root,
                "done one",
                "new",
                json!({"taskId": by_title_now("From the editor").id}),
            ),
            "is done",
        );
        first_session = started;
    }

    // Merged ticks the lines of its worktree's sessions, unmarking reopens them; done ticks the session's own line,
    // undone reopens it. A sub-session sharing the worktree may start on a line too, under the parent's.
    let first_id = id_of(&first_session);
    let helper_id;
    {
        must(
            &socket,
            json!({"type": "mark-session-merged", "sessionId": first_id, "targetCwd": root}),
        );
        wait_for_default(
            || by_title("First task"),
            |task| task.as_ref().is_some_and(|t| t.done && t.meta("auto") == Some("merge")),
        );
        must(
            &socket,
            json!({"type": "mark-session-merged", "sessionId": first_id, "targetCwd": root}),
        );
        wait_for_default(
            || by_title("First task"),
            |task| task.as_ref().is_some_and(|t| !t.done && t.meta("auto").is_none()),
        );
        op(json!({"type": "add", "title": "Help out", "parent": first.id})).unwrap();
        let child = must(
            &socket,
            json!({"type": "create", "input": {"title": "helper", "program": "claude", "cwd": first_session["cwd"], "repoRoot": root, "cols": 80, "rows": 24, "worktreeMode": "none", "parentSessionId": first_id, "subSessionKind": "clean", "taskId": by_title_now("Help out").id}}),
        );
        helper_id = id_of(&child);
        running(&socket, &helper_id);
        assert_eq!(by_title_now("Help out").meta("s"), Some(helper_id.as_str()));
        // Done on the helper ticks its line only; the parent's line still has it open... until it is ticked.
        must(
            &socket,
            json!({"type": "set-session-done", "sessionId": helper_id, "done": true}),
        );
        wait_for_default(
            || by_title("Help out"),
            |task| task.as_ref().is_some_and(|t| t.done && t.meta("auto") == Some("done")),
        );
        // Its last open line ticked: the parent's line rolls up.
        assert_eq!(by_title_now("First task").meta("auto"), Some("lines"));
        must(
            &socket,
            json!({"type": "set-session-done", "sessionId": helper_id, "done": false}),
        );
        wait_for_default(|| by_title("Help out"), |task| task.as_ref().is_some_and(|t| !t.done));
        assert!(!by_title_now("First task").done);
    }

    // Removing a session unhooks its line, which stays where it is, marked tried in its branch.
    {
        kill_and_wait(&helper_id);
        kill_and_wait(&first_id);
        must(&socket, json!({"type": "remove", "sessionId": helper_id}));
        let helped = by_title_now("Help out");
        assert!(helped.meta("s").is_none() && helped.parent.as_deref() == Some(first.id.as_str()));
        must(&socket, json!({"type": "remove", "sessionId": first_id}));
        let task = wait_for_default(
            || by_title("First task"),
            |next| next.as_ref().is_some_and(|t| t.meta("s").is_none()),
        )
        .unwrap();
        assert_eq!(task.meta("tried"), Some("first_task"));
        // A main-checkout session on a line: typing into the agent before it settles wins (the draft is dropped).
        let second = by_title_now("Second task");
        let plain = create_in("Second task", "none", json!({"taskId": second.id}));
        let plain_id = id_of(&plain);
        running(&socket, &plain_id);
        {
            let raw = deckhand_e2e::client::Raw::connect(&socket);
            raw.send(json!({"type": "input", "sessionId": plain_id, "data": "my own words"}));
            deckhand_e2e::harness::wait_for(
                || state(&socket, &plain_id),
                |next| next["startPrompt"].is_null(),
                Duration::from_secs(20),
            );
            pause(6500);
            assert_eq!(
                text_of(&home.join(format!("input-{plain_id}"))).as_deref(),
                Some("my own words")
            );
        }
        assert_eq!(by_title_now("Second task").meta("s"), Some(plain_id.as_str()));
        kill_and_wait(&plain_id);
        must(&socket, json!({"type": "remove", "sessionId": plain_id}));
        wait_for_default(
            || by_title("Second task"),
            |next| next.as_ref().is_some_and(|t| t.meta("s").is_none()),
        );
    }

    // A note's checkbox becomes a line (ctrl+p): from a session's note, under the session's line, made for it; removing
    // a session can move what is still open in its note into its line's note.
    {
        let session = create_in("notes holder", "none", json!({}));
        let session_id = id_of(&session);
        running(&socket, &session_id);
        let fresh = state(&socket, &session_id);
        let note = json!({"kind": "session", "id": session_id});
        let saved = must(
            &socket,
            json!({"type": "save-note", "note": note, "text": "scratch\n- [ ] Promote me\n- [x] Ticked\n- [ ] Keep me", "revision": fresh["notesFile"]["revision"]}),
        );
        let revision = saved["note"]["revision"].as_str().unwrap().to_string();
        let promote = |line: u32, revision: &str| {
            call(
                &socket,
                json!({"type": "promote-note-item", "note": note, "line": line, "revision": revision}),
            )
        };
        rejects(promote(0, &revision), "checklist item");
        rejects(promote(2, &revision), "already ticked");
        rejects(promote(1, "stale"), "changed meanwhile");
        let promoted = promote(1, &revision).unwrap();
        let after = lines_of(&promoted["tasks"]);
        let holder = line_titled(&after, "notes holder").expect("the session got its line");
        assert_eq!(holder.meta("s"), Some(session_id.as_str()));
        let task = line_titled(&after, "Promote me").unwrap();
        assert_eq!(promoted["lineId"], json!(task.id));
        assert_eq!(task.parent.as_deref(), Some(holder.id.as_str()));
        assert_eq!(promoted["session"]["notes"], "scratch\n- [x] Ticked\n- [ ] Keep me");
        assert!(!promoted["session"]["notes"].as_str().unwrap().contains('\u{2197}'));
        kill_and_wait(&session_id);
        must(
            &socket,
            json!({"type": "remove", "sessionId": session_id, "moveNote": true}),
        );
        assert_eq!(
            text_of(&home.join("notes").join("lines").join(format!("{}.md", holder.id))).as_deref(),
            Some("From the note of session notes holder:\n\nscratch\n- [ ] Keep me\n")
        );
        assert!(by_title_now("notes holder").meta("s").is_none());
    }

    // A UI opened in one checkout hears about the sessions of every checkout of its repository, not other repositories.
    {
        let other_s = p(&other);
        let elsewhere_s = p(&elsewhere_path);
        let in_main = create_in("in main", "none", json!({}));
        let in_worktree = must(
            &socket,
            json!({"type": "create", "input": {"title": "in worktree", "program": "claude", "cwd": other_s, "repoRoot": other_s, "cols": 80, "rows": 24, "worktreeMode": "none"}}),
        );
        let unrelated = must(
            &socket,
            json!({"type": "create", "input": {"title": "unrelated", "program": "claude", "cwd": elsewhere_s, "repoRoot": elsewhere_s, "cols": 80, "rows": 24, "worktreeMode": "none"}}),
        );
        for item in [&in_main, &in_worktree, &unrelated] {
            running(&socket, &id_of(item));
        }
        for scope in [root, other_s.as_str()] {
            let listed = must(&socket, json!({"type": "subscribe", "repoRoot": scope}));
            let seen: Vec<&str> = listed
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["title"].as_str().unwrap())
                .collect();
            assert!(
                seen.contains(&"in main") && seen.contains(&"in worktree"),
                "{scope}: {}",
                seen.join(", ")
            );
            assert!(!seen.contains(&"unrelated"), "{scope}");
        }
        let listener = subscribed(&socket, &other_s);
        must(
            &socket,
            json!({"type": "set-session-done", "sessionId": in_main["id"], "done": true}),
        );
        updated(&listener, 0, &id_of(&in_main), |s| !falsy(&s["doneAt"]));
        // A session of another repository goes on no line of this list.
        rejects(
            op(json!({"type": "session-on", "id": by_title_now("Second task").id, "s": unrelated["id"]})),
            "another repository",
        );
        for item in [&in_main, &in_worktree, &unrelated] {
            kill_and_wait(&id_of(item));
            must(&socket, json!({"type": "remove", "sessionId": item["id"]}));
        }
    }
    deckhand_e2e::harness::stop(&mut daemon);
}
