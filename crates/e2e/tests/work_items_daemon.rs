//! The work-items model at the daemon, black-box: drives DECKHAND_TEST_BIN. The migration of a legacy home at start
//! (once, with a marker; a failure keeps the daemon running, tells the UI and runs again next start), and a stacked
//! sub-session's merge ticking its line and rolling up its parent's.

use deckhand_e2e::harness::repo;
use deckhand_e2e::tasks::*;
use std::fs;
use std::path::{Path, PathBuf};

use deckhand_e2e::daemon::*;
use deckhand_e2e::harness::{Sandbox, wait_for_default};
use serde_json::{Value, json};

const FEATURE: &str = "0b1c2d3e-0000-4000-8000-00000000f00d";
const MAIN: &str = "0b1c2d3e-0000-4000-8000-00000000ma1n";

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// A home as an older Deckhand leaves it: a worktree-linked task (with a follow-up of the oldest kind), a worktree's
/// note with steps and prose, a main-checkout session's note.
fn legacy_home(home: &Path, root: &str) -> PathBuf {
    let at = "2026-10-01T09:00:00.000Z";
    write(
        &home.join("state.json"),
        &serde_json::to_string_pretty(&json!({
            "sessions": [
                {"id": FEATURE, "title": "Feature", "program": "claude", "command": "claude",
                 "cwd": format!("{root}-feature"), "repoRoot": root,
                 "worktree": {"mode": "managed", "origin": "created", "id": "w-feature", "path": format!("{root}-feature"), "branch": "feature"},
                 "status": "exited", "createdAt": at, "updatedAt": at},
                {"id": MAIN, "title": "Main work", "program": "claude", "command": "claude", "cwd": root,
                 "repoRoot": root, "launchWorktreeRoot": root, "worktree": {"mode": "none"},
                 "status": "exited", "createdAt": at, "updatedAt": at},
            ],
            "worktrees": [{"id": "w-feature", "path": format!("{root}-feature"), "createdAt": at}],
        }))
        .unwrap(),
    );
    let notes = home.join("notes");
    let list = notes.join("tasks").join(format!("{}.md", repo_key(root)));
    write(
        &list,
        "- [ ] Feature <!-- dh:t=a1a1a1a1 wt=w-feature added=2026-10-01 -->\n  why we do it\n- [ ] Legacy step <!-- \
         dh:t=c3c3c3c3 wt=w-feature assigned=2026-10-01 -->\n- [ ] Backlog item <!-- dh:t=b2b2b2b2 -->\n",
    );
    write(
        &notes.join("worktrees").join("w-feature.md"),
        "- [ ] write it\n- [x] plan it\nThinking.\n",
    );
    write(&notes.join("sessions").join(format!("{MAIN}.md")), "- [ ] tidy up\n");
    list
}

#[test]
fn a_legacy_home_migrates_once_at_start_and_then_runs_on_the_new_model() {
    let repo_dir = repo();
    let sandbox = Sandbox::new("work-items");
    let root_path = fs::canonicalize(repo_dir.path()).expect("realpath");
    let root_s = p(&root_path);
    let root = root_s.as_str();
    let home = sandbox.home();
    let list_file = legacy_home(&home, root);
    let bin = install_agents(&sandbox, deckhand_e2e::fake_agent!());
    let mut daemon = launch(&sandbox, &root_path, &bin);
    let socket = sandbox.socket();
    ready(&socket);

    assert!(must(&socket, json!({"type": "ping"})).get("notice").is_none());
    let notes = home.join("notes");
    assert!(notes.join("work-items-migrated.json").exists());
    let lines = || lines_of(&must(&socket, json!({"type": "watch-tasks", "cwd": root})));
    let list = lines();
    let feature = line_titled(&list, "Feature").unwrap();
    assert_eq!(
        feature.meta("s"),
        Some(FEATURE),
        "a worktree's task goes onto its owning session"
    );
    assert_eq!(titles_under(&list, &feature.id), ["write it", "plan it", "Legacy step"]);
    assert!(line_titled(&list, "plan it").unwrap().done);
    let main = line_titled(&list, "Main work").unwrap();
    assert_eq!(main.meta("s"), Some(MAIN));
    assert_eq!(titles_under(&list, &main.id), ["tidy up"]);
    assert!(line_titled(&list, "Backlog item").unwrap().meta("s").is_none());
    let text = fs::read_to_string(&list_file).unwrap();
    assert!(!text.contains("wt=") && !text.contains("assigned="), "{text}");
    assert_eq!(
        text_of(&notes.join("lines").join("a1a1a1a1.md")).as_deref(),
        Some("why we do it\n\nThinking.\n")
    );
    assert!(!notes.join("worktrees").join("w-feature.md").exists());
    assert!(!notes.join("sessions").join(format!("{MAIN}.md")).exists());
    assert_eq!(state(&socket, MAIN)["notes"], "");
    let backups: Vec<String> = fs::read_dir(notes.join("trash"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("work-items-migration-"))
        .collect();
    assert_eq!(backups.len(), 1, "{backups:?}");
    let backup = notes.join("trash").join(&backups[0]);
    assert!(backup.join("worktrees").join("w-feature.md").exists());
    assert!(backup.join("migration-report.txt").exists());
    let log = fs::read_to_string(home.join("daemon.log")).unwrap();
    assert!(log.contains("work-items migration: "), "{log}");

    // The new model from here: a step under the session's line, ticked; the list keeps no worktree links.
    must(
        &socket,
        json!({"type": "task-op", "cwd": root, "op": {"type": "add", "title": "ship it", "parent": feature.id}}),
    );
    let ship = line_titled(&lines(), "ship it").unwrap();
    assert_eq!(ship.parent.as_deref(), Some(feature.id.as_str()));
    must(
        &socket,
        json!({"type": "task-op", "cwd": root, "op": {"type": "tick", "id": ship.id}}),
    );

    // Once: a second start changes nothing.
    let before = fs::read_to_string(&list_file).unwrap();
    deckhand_e2e::harness::stop(&mut daemon);
    daemon = launch(&sandbox, &root_path, &bin);
    ready(&socket);
    assert_eq!(fs::read_to_string(&list_file).unwrap(), before);
    let backups = fs::read_dir(notes.join("trash"))
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .is_ok_and(|entry| entry.file_name().to_string_lossy().starts_with("work-items-migration-"))
        })
        .count();
    assert_eq!(backups, 1);
    deckhand_e2e::harness::stop(&mut daemon);
}

#[test]
fn a_failed_migration_leaves_no_marker_tells_the_ui_and_runs_again_at_the_next_start() {
    let repo_dir = repo();
    let sandbox = Sandbox::new("work-items-fail");
    let root_path = fs::canonicalize(repo_dir.path()).expect("realpath");
    let root_s = p(&root_path);
    let root = root_s.as_str();
    let home = sandbox.home();
    let list_file = legacy_home(&home, root);
    // The backup cannot be made: the trash is a file.
    let trash = home.join("notes").join("trash");
    fs::write(&trash, "in the way").unwrap();
    let bin = install_agents(&sandbox, deckhand_e2e::fake_agent!());
    let mut daemon = launch(&sandbox, &root_path, &bin);
    let socket = sandbox.socket();
    ready(&socket);

    let ping = must(&socket, json!({"type": "ping"}));
    let notice = ping["notice"].as_str().unwrap_or_default();
    assert!(notice.contains("failed") && notice.contains("next start"), "{ping}");
    assert!(!home.join("notes").join("work-items-migrated.json").exists());
    assert!(
        fs::read_to_string(&list_file).unwrap().contains("wt=w-feature"),
        "nothing changed"
    );
    let log = fs::read_to_string(home.join("daemon.log")).unwrap();
    assert!(log.contains("work-items migration failed"), "{log}");
    // It keeps running on the new model.
    let doc = must(
        &socket,
        json!({"type": "task-op", "cwd": root, "op": {"type": "add", "title": "Still works"}}),
    );
    assert!(line_titled(&lines_of(&doc), "Still works").is_some());

    // Out of the way: the next start migrates.
    deckhand_e2e::harness::stop(&mut daemon);
    fs::remove_file(&trash).unwrap();
    daemon = launch(&sandbox, &root_path, &bin);
    ready(&socket);
    assert!(must(&socket, json!({"type": "ping"})).get("notice").is_none());
    assert!(home.join("notes").join("work-items-migrated.json").exists());
    let lines = lines_of(&must(&socket, json!({"type": "watch-tasks", "cwd": root})));
    assert_eq!(line_titled(&lines, "Feature").unwrap().meta("s"), Some(FEATURE));
    assert!(line_titled(&lines, "Still works").is_some());
    deckhand_e2e::harness::stop(&mut daemon);
}

#[test]
fn a_stacked_sub_sessions_merge_ticks_its_line_and_rolls_up_its_parents() {
    let repo_dir = repo();
    let sandbox = Sandbox::new("work-items-stacked");
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

    let lead = running(
        &socket,
        &id_of(&create(&socket, root, "Token storage", "new", json!({}))),
    );
    let lead = wait_for_default(
        || state(&socket, &id_of(&lead)),
        |item| !falsy(&item["worktree"]["path"]),
    );
    let child = running(
        &socket,
        &id_of(&must(
            &socket,
            json!({"type": "create", "input": {"title": "Refresh tokens", "program": "claude", "cwd": lead["cwd"], "repoRoot": root, "cols": 80, "rows": 24, "worktreeMode": "new", "parentSessionId": lead["id"], "subSessionKind": "clean"}}),
        )),
    );
    let child = wait_for_default(
        || state(&socket, &id_of(&child)),
        |item| !falsy(&item["worktree"]["path"]),
    );
    assert_eq!(
        child["worktree"]["targetBranch"], lead["worktree"]["branch"],
        "stacked: {child}"
    );
    // Lines on demand: the lead's, then the child's under it (the trees line up).
    op(json!({"type": "session-line", "s": lead["id"]}));
    op(json!({"type": "session-line", "s": child["id"]}));
    let lead_line = line("Token storage");
    let child_line = line("Refresh tokens");
    assert_eq!(child_line.parent.as_deref(), Some(lead_line.id.as_str()));
    op(json!({"type": "add", "title": "schema", "parent": lead_line.id}));
    op(json!({"type": "tick", "id": line("schema").id}));
    assert!(!line("Token storage").done);

    // The child is merged into its parent's branch: its line ticks, and it was the last open line under the lead's.
    let child_cwd = PathBuf::from(child["cwd"].as_str().unwrap());
    fs::write(child_cwd.join("refresh.txt"), "refresh\n").unwrap();
    assert!(git_try(&child_cwd, &["add", "."]).0);
    assert!(git_try(&child_cwd, &["commit", "-m", "refresh tokens"]).0);
    let merged = must(
        &socket,
        json!({"type": "merge-worktree", "sessionId": child["id"], "mode": "merge", "targetCwd": lead["cwd"]}),
    );
    assert!(falsy(&merged["conflicted"]), "{merged}");
    let ticked = wait_for_default(|| line("Refresh tokens"), |line| line.done);
    assert_eq!(ticked.meta("auto"), Some("merge"));
    let rolled = line("Token storage");
    assert!(rolled.done && rolled.meta("auto") == Some("lines"), "{rolled:?}");
    deckhand_e2e::harness::stop(&mut daemon);
}
