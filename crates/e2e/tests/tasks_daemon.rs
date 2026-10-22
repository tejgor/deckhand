//! Tasks, black-box: drives DECKHAND_TEST_BIN.

use deckhand_e2e::harness::repo;
use deckhand_e2e::tasks::*;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use deckhand_e2e::client::Client;
use deckhand_e2e::daemon::*;
use deckhand_e2e::harness::{Sandbox, wait_for_default};
use serde_json::{Value, json};

// ported from tests/tasksDaemon.test.ts: tasks: one list per repository, linked to the work started from it
#[test]
fn tasks_one_list_per_repository_linked_to_the_work_started_from_it() {
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
    let current = || tasks_of(&must(&socket, json!({"type": "watch-tasks", "cwd": root})));
    let by_title = |title: &str| current().into_iter().find(|t| t.title == title);
    let by_title_now = |title: &str| by_title(title).unwrap_or_else(|| panic!("no task {title}"));
    let kill_and_wait = |id: &str| {
        must(&socket, json!({"type": "kill", "sessionId": id}));
        wait_for_default(|| state(&socket, id), |item| item["status"] == "exited");
    };

    let mut watch = Client::connect(&socket).expect("watch client");
    watch
        .request(json!({"type": "watch-tasks", "cwd": root}))
        .expect("watch-tasks");
    let pushed = |matches: &dyn Fn(&[Task]) -> bool| {
        wait_for_default(
            || {
                watch
                    .events_of("tasks-updated")
                    .iter()
                    .any(|e| matches(&tasks_of(&e["tasks"])))
            },
            |found| *found,
        );
    };

    // ported from tests/tasksDaemon.test.ts: ops change the file and reach watching clients; an editor's edit shows up; daemon-only ops are refused
    op(json!({"type": "add", "title": "First task", "body": "why it matters"})).unwrap();
    let mut doc = op(json!({"type": "add", "title": "Second task"})).unwrap();
    assert_eq!(
        doc["path"],
        json!(p(&home
            .join("notes")
            .join("tasks")
            .join(format!("{}.md", doc["key"].as_str().unwrap()))))
    );
    assert_eq!(
        tasks_of(&doc)
            .iter()
            .map(|t| (t.title.clone(), t.body.clone()))
            .collect::<Vec<_>>(),
        vec![
            ("First task".to_string(), "why it matters".to_string()),
            ("Second task".to_string(), String::new())
        ]
    );
    pushed(&|next| next.len() == 2);
    {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(doc["path"].as_str().unwrap())
            .unwrap();
        file.write_all(b"- [ ] From the editor\n").unwrap();
    }
    pushed(&|next| next.iter().any(|t| t.title == "From the editor"));
    // A task added in the editor has no ID yet; its provisional one still addresses it.
    let editor_task = by_title_now("From the editor");
    assert!(editor_task.id.starts_with('~'));
    doc = op(json!({"type": "toggle", "id": editor_task.id})).unwrap();
    assert!(
        tasks_of(&doc)
            .iter()
            .find(|t| t.title == "From the editor")
            .unwrap()
            .done
    );
    assert!(
        tasks_of(&doc)
            .iter()
            .all(|t| t.id.len() == 8 && t.id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')))
    );
    rejects(
        op(json!({"type": "link", "id": tasks_of(&doc)[0].id, "link": {"s": "x"}})),
        "Unknown task change",
    );
    rejects(op(json!({"type": "toggle", "id": "gone"})), "no longer in the list");
    // A linked worktree of the same repository shares the list.
    let other = home.join("other-worktree");
    assert!(git_try(&root_path, &["worktree", "add", "-b", "other", &p(&other)]).0);
    assert_eq!(
        must(&socket, json!({"type": "watch-tasks", "cwd": p(&other)}))["key"],
        doc["key"]
    );

    // ported from tests/tasksDaemon.test.ts: branches to start from; a session started from a task links it, finds the task typed (not sent) in its agent's input and starts at the chosen base
    {
        let branches = must(&socket, json!({"type": "list-branches", "cwd": root}));
        assert_eq!(branches["current"], "main");
        assert_eq!(branches["defaultBranch"], "main");
        assert_eq!(branches["branchFrom"], "current");
        assert!(branches["branches"].as_array().unwrap().contains(&json!("release")));
        let first = by_title_now("First task");
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
        // Once the agent's screen settles (idle), the task arrives as one bracketed paste, without Enter.
        let typed = deckhand_e2e::harness::wait_for(
            || text_of(&home.join(format!("input-{session_id}"))).unwrap_or_default(),
            |text| !text.is_empty(),
            Duration::from_secs(20),
        );
        assert_eq!(typed, "\x1b[200~First task: why it matters\x1b[201~");
        wait_for_default(|| state(&socket, &session_id), |next| next["startPrompt"].is_null());
        assert_eq!(started["worktree"]["baseRef"], "release");
        assert!(
            !PathBuf::from(started["cwd"].as_str().unwrap())
                .join("main-only.txt")
                .exists()
        );
        assert_eq!(
            meta_of(&by_title_now("First task"), "wt").as_deref(),
            started["worktree"]["id"].as_str()
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
    }

    // ported from tests/tasksDaemon.test.ts: merged ticks the worktree's tasks, unmarking reopens them; D on the last session ticks, un-D reopens
    {
        let listed = must(&socket, json!({"type": "list"}));
        let session = listed
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["title"] == "First task")
            .cloned()
            .unwrap();
        let session_id = id_of(&session);
        must(
            &socket,
            json!({"type": "mark-session-merged", "sessionId": session_id, "targetCwd": root}),
        );
        wait_for_default(
            || by_title("First task"),
            |task| {
                task.as_ref()
                    .is_some_and(|t| t.done && meta_of(t, "auto").as_deref() == Some("merge"))
            },
        );
        must(
            &socket,
            json!({"type": "mark-session-merged", "sessionId": session_id, "targetCwd": root}),
        );
        wait_for_default(
            || by_title("First task"),
            |task| task.as_ref().is_some_and(|t| !t.done && meta_of(t, "auto").is_none()),
        );
        // A second session in the same worktree: D ticks only once both are done.
        let child = must(
            &socket,
            json!({"type": "create", "input": {"title": "helper", "program": "claude", "cwd": session["cwd"], "repoRoot": root, "cols": 80, "rows": 24, "worktreeMode": "none", "parentSessionId": session_id, "subSessionKind": "clean"}}),
        );
        let child_id = id_of(&child);
        running(&socket, &child_id);
        must(
            &socket,
            json!({"type": "set-session-done", "sessionId": session_id, "done": true}),
        );
        pause(300);
        assert!(!by_title_now("First task").done);
        must(
            &socket,
            json!({"type": "set-session-done", "sessionId": child_id, "done": true}),
        );
        wait_for_default(
            || by_title("First task"),
            |task| {
                task.as_ref()
                    .is_some_and(|t| t.done && meta_of(t, "auto").as_deref() == Some("done"))
            },
        );
        must(
            &socket,
            json!({"type": "set-session-done", "sessionId": child_id, "done": false}),
        );
        wait_for_default(|| by_title("First task"), |task| task.as_ref().is_some_and(|t| !t.done));
    }

    // ported from tests/tasksDaemon.test.ts: work deleted unmerged goes back to the backlog, naming the branch it was tried in
    {
        let sessions: Vec<Value> = must(&socket, json!({"type": "list"}))
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| {
                item["title"] == "First task" || item["title"].as_str().is_some_and(|t| t.ends_with("helper"))
            })
            .cloned()
            .collect();
        for item in &sessions {
            kill_and_wait(&id_of(item));
        }
        let child = sessions
            .iter()
            .find(|item| item["title"].as_str().is_some_and(|t| t.ends_with("helper")))
            .unwrap();
        let parent = sessions.iter().find(|item| item["title"] == "First task").unwrap();
        must(&socket, json!({"type": "remove", "sessionId": child["id"]}));
        assert!(
            meta_of(&by_title_now("First task"), "wt").is_some(),
            "still linked while a session refers to the worktree"
        );
        must(&socket, json!({"type": "remove", "sessionId": parent["id"]}));
        let task = wait_for_default(
            || by_title("First task"),
            |next| next.as_ref().is_some_and(|t| meta_of(t, "wt").is_none()),
        )
        .unwrap();
        assert_eq!(meta_of(&task, "tried").as_deref(), Some("first_task"));
        // A main-checkout session's task: linked to the session, back to the backlog when it is removed.
        let second = by_title_now("Second task");
        let plain = create_in("Second task", "none", json!({"taskId": second.id}));
        let plain_id = id_of(&plain);
        running(&socket, &plain_id);
        // Typing into the agent before it settles wins: the draft is dropped, never mixed into your text.
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
        wait_for_default(
            || by_title("Second task"),
            |next| {
                next.as_ref()
                    .is_some_and(|t| meta_of(t, "s").as_deref() == Some(plain_id.as_str()))
            },
        );
        kill_and_wait(&plain_id);
        must(&socket, json!({"type": "remove", "sessionId": plain_id}));
        wait_for_default(
            || by_title("Second task"),
            |next| next.as_ref().is_some_and(|t| meta_of(t, "s").is_none()),
        );
    }

    // ported from tests/tasksDaemon.test.ts: a note's checklist item goes to Tasks and leaves a link; removing a session can move its open items first
    {
        let session = create_in("notes holder", "none", json!({}));
        let session_id = id_of(&session);
        running(&socket, &session_id);
        let fresh = state(&socket, &session_id);
        let saved = must(
            &socket,
            json!({"type": "save-note", "sessionId": session_id, "section": "session", "text": "scratch\n- [ ] Promote me\n- [x] Ticked\n- [ ] Keep me", "revision": fresh["notesFile"]["revision"]}),
        );
        let revision = saved["session"]["notesFile"]["revision"].as_str().unwrap().to_string();
        let promote = |line: u32, revision: &str| {
            call(
                &socket,
                json!({"type": "promote-note-item", "sessionId": session_id, "section": "session", "line": line, "revision": revision}),
            )
        };
        rejects(promote(0, &revision), "checklist item");
        rejects(promote(2, &revision), "already ticked");
        rejects(promote(1, "stale"), "changed meanwhile");
        let promoted = promote(1, &revision).unwrap();
        let task = tasks_of(&promoted["tasks"])
            .into_iter()
            .find(|t| t.title == "Promote me")
            .unwrap();
        assert_eq!(
            promoted["session"]["notes"],
            json!(format!(
                "scratch\n- \u{2197} Promote me <!-- dh:t={} -->\n- [x] Ticked\n- [ ] Keep me",
                task.id
            ))
        );
        // It is work of its own: the backlog (the note's work's to-dos are its steps).
        assert!(meta_of(&task, "s").is_none() && meta_of(&task, "wt").is_none());
        // And back: the arrow line becomes the item again (with the task's details), and the task leaves the list.
        op(json!({"type": "edit", "id": task.id, "title": "Promote me", "body": "details"})).unwrap();
        let returned = must(
            &socket,
            json!({"type": "return-task-to-note", "cwd": root, "taskId": task.id}),
        );
        assert!(!tasks_of(&returned).iter().any(|t| t.id == task.id));
        assert_eq!(
            state(&socket, &session_id)["notes"],
            "scratch\n- [ ] Promote me\n  details\n- [x] Ticked\n- [ ] Keep me"
        );
        rejects(
            call(
                &socket,
                json!({"type": "return-task-to-note", "cwd": root, "taskId": task.id}),
            ),
            "no longer in the list",
        );
        rejects(
            call(
                &socket,
                json!({"type": "return-task-to-note", "cwd": root, "taskId": by_title_now("Second task").id}),
            ),
            "No note links",
        );
        kill_and_wait(&session_id);
        must(
            &socket,
            json!({"type": "remove", "sessionId": session_id, "moveOpenItems": true}),
        );
        // As when a worktree's work ends: a backlog task (its branch, if any, remembered), nothing written into its details.
        let moved = by_title_now("Keep me");
        assert_eq!(moved.body, "");
        assert!(meta_of(&moved, "from").is_none());
        assert_eq!(current().iter().filter(|t| t.title == "Ticked").count(), 0);
        assert!(
            meta_of(&by_title_now("Promote me"), "s").is_none(),
            "back to the backlog with its session"
        );
    }

    // ported from tests/tasksDaemon.test.ts: a UI opened in one checkout hears about the sessions of every checkout of its repository, not other repositories
    {
        let other = home.join("other-worktree");
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
        // Live updates too: a UI subscribed in the worktree hears about a change in the main checkout.
        let listener = subscribed(&socket, &other_s);
        must(
            &socket,
            json!({"type": "set-session-done", "sessionId": in_main["id"], "done": true}),
        );
        updated(&listener, 0, &id_of(&in_main), |s| !falsy(&s["doneAt"]));
        for item in [&in_main, &in_worktree, &unrelated] {
            kill_and_wait(&id_of(item));
            must(&socket, json!({"type": "remove", "sessionId": item["id"]}));
        }
    }

    // ported from tests/tasksDaemon.test.ts: a work's task and its steps: w onto work that has a task puts the task into its note; a merge ticks the task, its open steps can go to the backlog
    {
        op(json!({"type": "add", "title": "Merge me"})).unwrap();
        let session = create_in("Merge me", "new", json!({"taskId": by_title_now("Merge me").id}));
        let session_id = id_of(&session);
        running(&socket, &session_id);
        let started = state(&socket, &session_id);
        let wt = started["worktree"]["id"].as_str().unwrap().to_string();
        // The work has its task: another one moved there becomes a step at the end of its note (and leaves the list).
        op(json!({"type": "add", "title": "Step A", "body": "with care"})).unwrap();
        let step_a = by_title_now("Step A");
        let task_to_note = |task_id: &str, section: &str| {
            call(
                &socket,
                json!({"type": "task-to-note", "cwd": root, "taskId": task_id, "section": section}),
            )
        };
        let moved = task_to_note(&step_a.id, &format!("wt:{wt}")).unwrap();
        assert!(!tasks_of(&moved).iter().any(|t| t.id == step_a.id));
        assert_eq!(
            state(&socket, &session_id)["sharedNotes"]["text"],
            "- [ ] Step A\n  with care\n"
        );
        rejects(task_to_note(&step_a.id, &format!("wt:{wt}")), "no longer in the list");
        rejects(
            task_to_note(&by_title_now("Second task").id, "wt:nope"),
            "worktree is gone",
        );
        // Only work of this repository; a session in a worktree is not work of its own.
        rejects(
            op(json!({"type": "assign", "id": by_title_now("Second task").id, "link": {"wt": "nope"}})),
            "worktree is gone",
        );
        rejects(
            op(json!({"type": "add", "title": "x", "link": {"s": session_id}})),
            "now works in a worktree",
        );
        rejects(
            try_create(
                &socket,
                root,
                "again",
                "new",
                json!({"taskId": by_title_now("Merge me").id}),
            ),
            "already being worked on",
        );
        // A merge ticks the task; its note's open step goes to the backlog, left open in the branch.
        let started_cwd = PathBuf::from(started["cwd"].as_str().unwrap());
        fs::write(started_cwd.join("feature.txt"), "feature\n").unwrap();
        assert!(git_try(&started_cwd, &["add", "."]).0);
        assert!(git_try(&started_cwd, &["commit", "-m", "feature"]).0);
        let result = must(
            &socket,
            json!({"type": "merge-worktree", "sessionId": session_id, "mode": "squash", "targetCwd": root, "sendOpenItems": true}),
        );
        assert!(falsy(&result["conflicted"]));
        assert_eq!(result["sentItems"], 1);
        wait_for_default(
            || by_title("Merge me"),
            |task| {
                task.as_ref()
                    .is_some_and(|t| t.done && meta_of(t, "auto").as_deref() == Some("merge"))
            },
        );
        let step = by_title_now("Step A");
        assert_eq!(
            (step.done, meta_of(&step, "wt"), meta_of(&step, "from")),
            (false, None, started["worktree"]["branch"].as_str().map(str::to_string))
        );
        // Unmarking the merge reopens the task.
        must(
            &socket,
            json!({"type": "mark-session-merged", "sessionId": session_id, "targetCwd": root}),
        );
        wait_for_default(|| by_title("Merge me"), |task| task.as_ref().is_some_and(|t| !t.done));
    }
    deckhand_e2e::harness::stop(&mut daemon);
}
