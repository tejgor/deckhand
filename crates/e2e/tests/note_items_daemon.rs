//! Port of tests/noteItemsDaemon.test.ts (black-box: drives DECKHAND_TEST_BIN).
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

// ---- src/tasks.ts `parseTasks`, ported locally (the file's tasks in order) ----

#[derive(Debug, Clone)]
struct Task {
    id: String,
    title: String,
    done: bool,
    body: String,
    meta: std::collections::BTreeMap<String, String>,
}

const META_KEYS: [&str; 8] = ["t", "wt", "s", "done", "auto", "tried", "from", "added"];

/// `^([ \t]*)([-*+])([ \t]+)\[([ xX])\](?:[ \t]|$)`: (indent, checked, text after the marker).
fn checklist_line(line: &str) -> Option<(&str, bool, &str)> {
    let rest = line.trim_start_matches([' ', '\t']);
    let indent = &line[..line.len() - rest.len()];
    let mut chars = rest.chars();
    if !matches!(chars.next()?, '-' | '*' | '+') {
        return None;
    }
    let after = &rest[1..];
    let body = after.trim_start_matches([' ', '\t']);
    if body.len() == after.len() {
        return None;
    }
    let mut chars = body.chars();
    if chars.next()? != '[' {
        return None;
    }
    let mark = chars.next()?;
    if !matches!(mark, ' ' | 'x' | 'X') || chars.next()? != ']' {
        return None;
    }
    let tail = &body[3..];
    if !(tail.is_empty() || tail.starts_with([' ', '\t'])) {
        return None;
    }
    let text = if tail.is_empty() { tail } else { &tail[1..] };
    Some((indent, mark != ' ', text))
}

/// `[ \t]*<!--[ \t]*dh:([^>]*?)[ \t]*-->[ \t]*$`: (index where the match starts, captured pairs).
fn task_meta(text: &str) -> Option<(usize, &str)> {
    let trimmed = text.trim_end_matches([' ', '\t']);
    let inner_end = trimmed.strip_suffix("-->")?.len();
    let mut from = 0;
    while let Some(found) = trimmed[from..].find("<!--") {
        let start = from + found;
        let inner = trimmed[start + 4..inner_end].trim_start_matches([' ', '\t']);
        if let Some(captured) = inner.strip_prefix("dh:")
            && !captured.contains('>')
        {
            let begin = trimmed[..start].trim_end_matches([' ', '\t']).len();
            return Some((begin, captured.trim_end_matches([' ', '\t'])));
        }
        from = start + 4;
    }
    None
}

fn sha8(text: &str) -> String {
    use std::io::Write;
    let tool = if Command::new("shasum").arg("--version").output().is_ok() {
        ("shasum", vec!["-a", "256"])
    } else {
        ("sha256sum", vec![])
    };
    let mut child = Command::new(tool.0)
        .args(tool.1)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("sha256 tool");
    child.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    String::from_utf8_lossy(&output.stdout)[..8].to_string()
}

fn parse_tasks(text: &str) -> Vec<Task> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let mut tasks = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        index += 1;
        let Some((indent, checked, item_text)) = checklist_line(line) else {
            continue;
        };
        if !indent.is_empty() {
            continue;
        }
        let (title, meta_raw) = match task_meta(item_text) {
            Some((begin, captured)) => (item_text[..begin].trim().to_string(), Some(captured)),
            None => (item_text.trim().to_string(), None),
        };
        let mut body: Vec<&str> = Vec::new();
        while index < lines.len() && lines[index].starts_with([' ', '\t']) {
            body.push(lines[index]);
            index += 1;
        }
        let mut meta = std::collections::BTreeMap::new();
        for pair in meta_raw.unwrap_or("").split([' ', '\t']) {
            let Some(at) = pair.find('=').filter(|at| *at >= 1) else {
                continue;
            };
            let (key, value) = (&pair[..at], &pair[at + 1..]);
            if value.is_empty() || !META_KEYS.contains(&key) {
                continue;
            }
            if key == "auto" && !matches!(value, "merge" | "done") {
                continue;
            }
            meta.insert(key.to_string(), value.to_string());
        }
        let indents: Vec<usize> = body
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.len() - l.trim_start_matches([' ', '\t']).len())
            .collect();
        let cut = indents.iter().min().copied().unwrap_or(0);
        let dedented: Vec<&str> = body
            .iter()
            .map(|l| {
                let own = l.len() - l.trim_start_matches([' ', '\t']).len();
                &l[cut.min(own)..]
            })
            .collect();
        let position = tasks.len();
        tasks.push(Task {
            id: meta
                .get("t")
                .cloned()
                .unwrap_or_else(|| format!("~{position}:{}", sha8(&title))),
            title,
            done: checked,
            body: dedented.join("\n"),
            meta,
        });
    }
    tasks
}

fn meta_of(task: &Task, key: &str) -> Option<String> {
    task.meta.get(key).cloned()
}

fn id_of(value: &Value) -> String {
    value["id"].as_str().unwrap().to_string()
}

// ported from tests/noteItemsDaemon.test.ts: open note items go to Tasks at once: A, a merge, deletion from W and x with deletion
#[test]
fn open_note_items_go_to_tasks_at_once_a_merge_deletion_from_w_and_x_with_deletion() {
    let repo_dir = repo();
    let sandbox = Sandbox::new("items");
    let root_path = fs::canonicalize(repo_dir.path()).expect("realpath");
    let root_s = p(&root_path);
    let root = root_s.as_str();
    let home = sandbox.home();
    let bin = install_agents(&sandbox);
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
