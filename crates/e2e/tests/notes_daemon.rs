//! Port of tests/notesDaemon.test.ts (black-box: drives DECKHAND_TEST_BIN).
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

fn kill_and_wait(socket: &Path, id: &str) {
    must(socket, json!({"type": "kill", "sessionId": id}));
    wait_for_default(|| state(socket, id), |item| item["status"] == "exited");
}

fn save(socket: &Path, id: &str, section: &str, text: &str, revision: Option<&str>) -> Result<Value, String> {
    let current = state(socket, id);
    let base = match revision {
        Some(revision) => revision.to_string(),
        None if section == "session" => current["notesFile"]["revision"].as_str().unwrap().to_string(),
        None => current["sharedNotes"]["revision"].as_str().unwrap().to_string(),
    };
    let mut message = json!({"type": "save-note", "sessionId": id, "section": section, "text": text, "revision": base});
    if section == "shared" {
        message["noteId"] = json!(format!(
            "{}:{}",
            current["sharedNotes"]["kind"].as_str().unwrap(),
            current["sharedNotes"]["id"].as_str().unwrap()
        ));
    }
    call(socket, message)
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

fn id_of(value: &Value) -> String {
    value["id"].as_str().unwrap().to_string()
}

// ported from tests/notesDaemon.test.ts: notes are Markdown files: legacy migration, revision-checked saves, editor edits picked up, shared worktree and repository notes, lifetimes
#[test]
fn notes_are_markdown_files() {
    let repo_dir = repo();
    let sandbox = Sandbox::new("notes");
    let root_path = fs::canonicalize(repo_dir.path()).expect("realpath");
    let root_s = p(&root_path);
    let root = root_s.as_str();
    let home = sandbox.home();
    let bin = install_agents(&sandbox);
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

    // ported from tests/notesDaemon.test.ts: legacy notes move into their file once, and state.json stops carrying notes
    {
        let legacy = state(&socket, &legacy_id);
        let file = home.join("notes").join("sessions").join(format!("{legacy_id}.md"));
        assert_eq!(legacy["notes"], "legacy note\n- [ ] carry over");
        assert_eq!(legacy["notesFile"]["path"], json!(p(&file)));
        assert_eq!(text_of(&file).as_deref(), Some("legacy note\n- [ ] carry over"));
        assert!(load_state(&home)["sessions"][0]["notes"].is_null());
        let raw = fs::read_to_string(home.join("state.json")).unwrap();
        assert!(!raw.contains("legacy note") && !raw.contains("notesFile") && !raw.contains("sharedNotes"));
        // Idempotent: a state.json that still has (older) notes never overwrites the file, which wins.
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
        assert!(load_state(&home)["sessions"][0]["notes"].is_null());
    }

    let live = subscribed(&socket, root);

    // ported from tests/notesDaemon.test.ts: saves are revision-checked and broadcast; an editor's edit (in place or by rename) shows up; a stale save is refused
    {
        let file = PathBuf::from(state(&socket, &legacy_id)["notesFile"]["path"].as_str().unwrap());
        let mark = live.events().len();
        let saved = save(&socket, &legacy_id, "session", "# Plan\n- [ ] one", None).unwrap();
        assert_eq!(saved["saved"], true);
        assert_eq!(saved["session"]["notes"], "# Plan\n- [ ] one");
        assert_eq!(text_of(&file).as_deref(), Some("# Plan\n- [ ] one"));
        updated(&live, mark, &legacy_id, |s| s["notes"] == "# Plan\n- [ ] one");
        let base = state(&socket, &legacy_id)["notesFile"]["revision"]
            .as_str()
            .unwrap()
            .to_string();
        // An editor writing in place, then one saving through a temporary file and a rename.
        fs::write(&file, "edited in VS Code").unwrap();
        updated(&live, mark, &legacy_id, |s| s["notes"] == "edited in VS Code");
        let temporary = file.parent().unwrap().join("vim-swap.tmp");
        fs::write(&temporary, "saved via rename").unwrap();
        fs::rename(&temporary, &file).unwrap();
        updated(&live, mark, &legacy_id, |s| s["notes"] == "saved via rename");
        // Deckhand's draft was based on the old revision: refused, nothing written, the file as it is comes back.
        let stale = save(&socket, &legacy_id, "session", "clobber", Some(&base)).unwrap();
        assert_eq!(stale["saved"], false);
        assert_eq!(stale["session"]["notes"], "saved via rename");
        assert_eq!(text_of(&file).as_deref(), Some("saved via rename"));
        // Text is capped; a file made longer elsewhere is shown cut and only editable in an editor.
        let long = "x".repeat(50_010);
        let capped = save(&socket, &legacy_id, "session", &long, None).unwrap();
        assert_eq!(
            capped["session"]["notes"].as_str().map(|s| s.chars().count()),
            Some(50_000)
        );
        fs::write(&file, "y".repeat(60_000)).unwrap();
        updated(&live, mark, &legacy_id, |s| !falsy(&s["notesFile"]["tooLarge"]));
        rejects(save(&socket, &legacy_id, "session", "short", None), "longer than");
        assert_eq!(text_of(&file).map(|s| s.len()), Some(60_000));
        fs::write(&file, "back to normal").unwrap();
        updated(&live, mark, &legacy_id, |s| {
            s["notes"] == "back to normal" && falsy(&s["notesFile"]["tooLarge"])
        });
    }

    // ported from tests/notesDaemon.test.ts: every session of a worktree shares its note (attached and mode-none sub-sessions too); handoffs include it
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
    let child = running(
        &socket,
        &id_of(&must(
            &socket,
            json!({"type": "create", "input": {"title": "notes-child", "program": "pi", "cwd": first["cwd"], "repoRoot": root, "cols": 80, "rows": 24, "parentSessionId": first["id"], "subSessionKind": "clean"}}),
        )),
    );
    let first_id = id_of(&first);
    let attached_id = id_of(&attached);
    let child_id = id_of(&child);
    {
        let record_id = first["worktree"]["id"].clone();
        for session in [&first, &attached, &child] {
            assert_eq!(
                json!([session["sharedNotes"]["kind"], session["sharedNotes"]["id"]]),
                json!(["worktree", record_id])
            );
        }
        let mark = live.events().len();
        assert_eq!(
            save(&socket, &attached_id, "shared", "shared plan\n- [ ] review", None).unwrap()["saved"],
            true
        );
        for id in [&first_id, &child_id] {
            updated(&live, mark, id, |s| {
                s["sharedNotes"]["text"] == "shared plan\n- [ ] review"
            });
        }
        assert_eq!(
            text_of(
                &home
                    .join("notes")
                    .join("worktrees")
                    .join(format!("{}.md", record_id.as_str().unwrap()))
            )
            .as_deref(),
            Some("shared plan\n- [ ] review")
        );
        // The UI names the note it edited: a save for another note is refused.
        rejects(
            call(
                &socket,
                json!({"type": "save-note", "sessionId": first_id, "section": "shared", "noteId": "worktree:other", "text": "x", "revision": state(&socket, &first_id)["sharedNotes"]["revision"]}),
            ),
            "another note",
        );
        // A session in a worktree has one note, the worktree's: it has no note of its own to write.
        rejects(
            save(&socket, &child_id, "session", "child only", None),
            "worktree\u{2019}s note",
        );
        let handoff_path = must(&socket, json!({"type": "export-handoff", "sessionId": child_id}));
        let handoff = fs::read_to_string(handoff_path.as_str().unwrap()).unwrap();
        assert!(
            handoff.contains("## Notes\n\n(No notes recorded.)\n\n## Worktree notes\n\nshared plan\n- [ ] review"),
            "{handoff}"
        );
        // open-note creates a missing file for the editor.
        let own_file = must(
            &socket,
            json!({"type": "open-note", "sessionId": first_id, "section": "session"}),
        );
        let expected = home.join("notes").join("sessions").join(format!("{first_id}.md"));
        assert_eq!(own_file, json!(p(&expected)));
        assert_eq!(text_of(&expected).as_deref(), Some(""));
    }

    // ported from tests/notesDaemon.test.ts: main-checkout sessions share the repository note, which outlives them; a session note goes with its session
    {
        let one = running(&socket, &id_of(&create(&socket, root, "main-1", "none", json!({}))));
        let two = running(&socket, &id_of(&create(&socket, root, "main-2", "none", json!({}))));
        assert_eq!(one["sharedNotes"]["kind"], "repo");
        assert_eq!(two["sharedNotes"]["id"], one["sharedNotes"]["id"]);
        let (one_id, two_id) = (id_of(&one), id_of(&two));
        let mark = live.events().len();
        save(&socket, &one_id, "shared", "repo-wide", None).unwrap();
        updated(&live, mark, &two_id, |s| s["sharedNotes"]["text"] == "repo-wide");
        save(&socket, &one_id, "session", "mine", None).unwrap();
        let own_file = PathBuf::from(state(&socket, &one_id)["notesFile"]["path"].as_str().unwrap());
        let repo_file = PathBuf::from(one["sharedNotes"]["path"].as_str().unwrap());
        assert_eq!(text_of(&own_file).as_deref(), Some("mine"));
        for id in [&one_id, &two_id] {
            kill_and_wait(&socket, id);
            must(&socket, json!({"type": "remove", "sessionId": id}));
        }
        assert!(!own_file.exists());
        assert_eq!(text_of(&repo_file).as_deref(), Some("repo-wide"));
        // It went to the notes trash, named after its session (an empty note is just deleted).
        let trash = home.join("notes").join("trash");
        let trashed: Vec<String> = names(&trash)
            .into_iter()
            .filter(|n| n.contains("_session_main-1_"))
            .collect();
        assert_eq!(trashed.len(), 1);
        assert_eq!(text_of(&trash.join(&trashed[0])).as_deref(), Some("mine"));
        assert!(!names(&trash).iter().any(|n| n.contains("_session_main-2_")));
    }

    // ported from tests/notesDaemon.test.ts: a deleted worktree's note stays read-only; a new worktree at the path starts empty; the note goes with the record
    {
        kill_and_wait(&socket, &attached_id);
        kill_and_wait(&socket, &child_id);
        must(
            &socket,
            json!({"type": "kill", "sessionId": first_id, "deleteWorktree": true, "allowDataLoss": true}),
        );
        wait_for_default(
            || state(&socket, &first_id),
            |item| !falsy(&item["worktree"]["deletedAt"]),
        );
        let old = state(&socket, &child_id);
        assert_eq!(old["sharedNotes"]["readOnly"], true);
        assert_eq!(old["sharedNotes"]["text"], "shared plan\n- [ ] review");
        rejects(save(&socket, &child_id, "shared", "nope", None), "read-only");
        let again = running(&socket, &id_of(&create(&socket, root, "notes-a", "new", json!({}))));
        assert_eq!(again["cwd"], first["cwd"]);
        assert_ne!(again["sharedNotes"]["id"], old["sharedNotes"]["id"]);
        assert_eq!(again["sharedNotes"]["text"], "");
        assert!(again["sharedNotes"]["readOnly"].is_null());
        let old_file = PathBuf::from(old["sharedNotes"]["path"].as_str().unwrap());
        must(&socket, json!({"type": "remove", "sessionId": first_id}));
        must(&socket, json!({"type": "remove", "sessionId": attached_id}));
        assert_eq!(text_of(&old_file).as_deref(), Some("shared plan\n- [ ] review"));
        must(&socket, json!({"type": "remove", "sessionId": child_id}));
        assert!(!old_file.exists());
        assert!(
            names(&home.join("notes").join("trash"))
                .iter()
                .any(|n| n.contains("_worktree_")),
            "the worktree note went to the trash"
        );
        kill_and_wait(&socket, &id_of(&again));
    }

    // ported from tests/notesDaemon.test.ts: at start, notes of sessions in a worktree are merged into its note (under their titles) and go to the trash
    {
        let owner = running(&socket, &id_of(&create(&socket, root, "merge-a", "new", json!({}))));
        let helper = running(
            &socket,
            &id_of(&must(
                &socket,
                json!({"type": "create", "input": {"title": "merge-b", "program": "pi", "cwd": owner["cwd"], "repoRoot": root, "cols": 80, "rows": 24, "parentSessionId": owner["id"], "subSessionKind": "clean"}}),
            )),
        );
        let lone = running(&socket, &id_of(&create(&socket, root, "merge-lone", "new", json!({}))));
        save(&socket, &id_of(&owner), "shared", "worktree plan", None).unwrap();
        for session in [&owner, &helper, &lone] {
            kill_and_wait(&socket, &id_of(session));
        }
        // Notes written by an older version: each session's own.
        deckhand_e2e::harness::stop(&mut daemon);
        let own = |session: &Value| {
            home.join("notes")
                .join("sessions")
                .join(format!("{}.md", id_of(session)))
        };
        fs::write(own(&owner), "- [ ] from the owner\n").unwrap();
        fs::write(own(&helper), "helper context").unwrap();
        fs::write(own(&lone), "- [ ] the only note").unwrap();
        daemon = launch(&sandbox, &root_path, &bin);
        ready(&socket);
        let merged = state(&socket, &id_of(&owner))["sharedNotes"]["text"]
            .as_str()
            .unwrap()
            .to_string();
        // /^worktree plan\n\n## .*merge-a\n- \[ \] from the owner\n\n## .*merge-b\nhelper context$/
        let rest = merged
            .strip_prefix("worktree plan\n\n## ")
            .expect("starts with the plan");
        let (title_a, rest) = rest
            .split_once("merge-a\n- [ ] from the owner\n\n## ")
            .expect("owner section");
        let title_b = rest.strip_suffix("merge-b\nhelper context").expect("helper section");
        assert!(!title_a.contains('\n') && !title_b.contains('\n'), "{merged:?}");
        assert_eq!(state(&socket, &id_of(&helper))["sharedNotes"]["text"], json!(merged));
        // A lone note goes into an empty worktree note as it is.
        assert_eq!(
            state(&socket, &id_of(&lone))["sharedNotes"]["text"],
            "- [ ] the only note"
        );
        for session in [&owner, &helper, &lone] {
            assert!(!own(session).exists());
            assert_eq!(state(&socket, &id_of(session))["notes"], "");
        }
        assert_eq!(
            names(&home.join("notes").join("trash"))
                .iter()
                .filter(|n| n.contains("merged-into-its-worktree-note"))
                .count(),
            3
        );
    }
    deckhand_e2e::harness::stop(&mut daemon);
}
