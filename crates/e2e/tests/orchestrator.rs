//! The orchestrator, black-box: `deckhand plan check|apply` against a real daemon with fake agents, a UI connection
//! answering the confirmation (`plan-pending` → `plan-decide`), `deckhand do` starting a new orchestrator per idea,
//! an orchestrator's plan becoming its tree under its idea's line, and what its commands may do (its tree).

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Output, Stdio};
use std::time::Duration;

use deckhand_e2e::client::Client;
use deckhand_e2e::daemon::{install_agents, launch, must, p, ready, running, state, subscribed, wf};
use deckhand_e2e::fake_agent;
use deckhand_e2e::harness::{Sandbox, repo, stop, wait_exit, wait_for};
use serde_json::{Value, json};

/// `deckhand <args>` in `cwd` with only the fakes and system tools on PATH, `env` added; run to completion.
fn cli(sandbox: &Sandbox, bin: &Path, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = sandbox.cli(args);
    command
        .current_dir(cwd)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()));
    for (key, value) in env {
        command.env(key, value);
    }
    sandbox.exec(command, Duration::from_secs(60))
}

/// The same, left running (it waits for the confirmation), its output piped.
fn spawn_cli(sandbox: &Sandbox, bin: &Path, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Child {
    let mut command = sandbox.cli(args);
    command
        .current_dir(cwd)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }
    sandbox.spawn(command)
}

/// Waits for a spawned command and returns its output.
fn finish(mut child: Child) -> Output {
    let mut stdout = child.stdout.take().expect("stdout");
    let mut stderr = child.stderr.take().expect("stderr");
    let out = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stdout.read_to_end(&mut buffer);
        buffer
    });
    let err = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stderr.read_to_end(&mut buffer);
        buffer
    });
    let status = wait_exit(&mut child, Duration::from_secs(90)).expect("the command finished");
    Output {
        status,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The command's stdout as JSON; it must have succeeded.
fn json_of(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("{error}: {}", text(&output.stdout)))
}

/// What the fake agent of session `id` was launched with (see fake_agent.rs).
fn trace(sandbox: &Sandbox, id: &str) -> Value {
    let path = sandbox.home().join(format!("trace-{id}.json"));
    wf(
        || {
            fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        },
        Option::is_some,
    )
    .unwrap_or_default()
}

/// Whether the session's agent was launched with `needle` in an argument (its first message).
fn launched_with(sandbox: &Sandbox, id: &str, needle: &str) -> bool {
    trace(sandbox, id)["args"].as_array().is_some_and(|args| {
        args.iter()
            .any(|arg| arg.as_str().is_some_and(|arg| arg.contains(needle)))
    })
}

/// The caller environment of session `id` (as its agent got it).
fn identity(sandbox: &Sandbox, id: &str) -> [(String, String); 3] {
    let launched = trace(sandbox, id);
    [
        ("DECKHAND_SESSION_ID".to_owned(), id.to_owned()),
        (
            "DECKHAND_LAUNCH_ID".to_owned(),
            launched["launchId"].as_str().unwrap_or_default().to_owned(),
        ),
        (
            "DECKHAND_HOOK_TOKEN".to_owned(),
            launched["token"].as_str().unwrap_or_default().to_owned(),
        ),
    ]
}

fn env_of(identity: &[(String, String); 3]) -> Vec<(&str, &str)> {
    identity
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect()
}

/// The `plan-pending` events `live` got.
fn pending_plans(live: &Client) -> Vec<Value> {
    live.events_of("plan-pending")
}

/// Waits for a plan to be pending for `live` and answers its ID.
fn wait_pending(live: &Client, seen: usize) -> Value {
    wait_for(
        || pending_plans(live),
        |plans| plans.len() > seen,
        Duration::from_secs(20),
    )
    .last()
    .cloned()
    .unwrap_or_default()["plan"]
        .clone()
}

fn id(value: &Value) -> String {
    value["id"].as_str().unwrap_or_default().to_owned()
}

#[test]
fn a_plan_waits_for_confirmation_then_creates_parents_first_with_their_briefs() {
    let root = repo();
    let sandbox = Sandbox::new("plan-apply");
    let root_path: PathBuf = fs::canonicalize(root.path()).expect("realpath");
    let bin = install_agents(&sandbox, fake_agent!());
    let mut daemon = launch(&sandbox, &root_path, &bin);
    ready(&sandbox.socket());
    let socket = sandbox.socket();
    let run = |args: &[&str]| cli(&sandbox, &bin, &root_path, args, &[]);

    // An existing session in a worktree of its own, for a plan entry to go under.
    let login = json_of(&run(&["new", "--title", "Fix login", "--json"]));
    let login = running(&socket, &id(&login));
    let login_branch = login["worktree"]["branch"].as_str().unwrap_or_default().to_owned();

    let plan = json!({
        "sessions": [
            {"key": "tokens", "title": "Token storage", "prompt": "Brief: store the tokens", "parent": "auth"},
            {"key": "auth", "title": "Auth rework", "prompt": "Brief: rework auth"},
            {"key": "ui", "title": "Login UI", "prompt": "Brief: the login UI", "parent": "auth", "worktree": "same"},
            {"key": "script", "title": "Migration script", "prompt": "Brief: the script", "worktree": "none"},
            {"key": "tests", "title": "Login tests", "prompt": "Brief: the tests", "parent": &id(&login)[..8]},
        ],
        "tasks": [{"title": "Remove legacy endpoint", "details": "After Auth rework merges"}],
    });
    let plan_file = sandbox.root().join("plan.json");
    fs::write(&plan_file, plan.to_string()).expect("write the plan");
    let plan_path = p(&plan_file);

    // Checking only prints what it would create.
    let checked = run(&["plan", "check", &plan_path]);
    assert!(checked.status.success(), "{}", text(&checked.stderr));
    let summary = text(&checked.stdout);
    assert!(summary.contains("◇ Auth rework  (new worktree · claude)"), "{summary}");
    assert!(
        summary.contains("  ↳ Token storage  (stacked on Auth rework's branch · claude)"),
        "{summary}"
    );
    assert!(
        summary.contains(&format!(
            "◇ Login tests  (under Fix login · stacked on {login_branch} · claude)"
        )),
        "{summary}"
    );
    assert!(summary.contains("5 sessions, 1 task"), "{summary}");
    // A plan with problems says what they all are.
    fs::write(
        sandbox.root().join("bad.json"),
        r#"{"sessions": [{"key": "a", "title": "A", "parent": "nope"}]}"#,
    )
    .expect("write");
    let refused = run(&["plan", "apply", &p(&sandbox.root().join("bad.json"))]);
    assert!(!refused.status.success());
    let problems = text(&refused.stderr);
    assert!(
        problems.contains("The plan has 2 problems (nothing was created):"),
        "{problems}"
    );
    assert!(problems.contains("sessions[0] (a): prompt is required"), "{problems}");
    assert!(
        problems.contains("sessions[0] (a): parent \"nope\" is neither"),
        "{problems}"
    );

    let mut ui = subscribed(&socket, &p(&root_path));
    let applying = spawn_cli(
        &sandbox,
        &bin,
        &root_path,
        &["plan", "apply", &plan_path, "--json"],
        &[],
    );
    let pending = wait_pending(&ui, 0);
    assert_eq!(pending["repoRoot"], p(&root_path));
    assert_eq!(pending["summary"]["counts"], "5 sessions, 1 task");
    // Nothing is created before the answer.
    let listed = ui.request(json!({"type": "list"})).expect("list");
    assert_eq!(listed.as_array().map(Vec::len), Some(1), "{listed}");

    let plan_id = pending["id"].as_str().unwrap_or_default().to_owned();
    ui.request(json!({"type": "plan-decide", "planId": plan_id, "approve": true}))
        .expect("approve");
    let late = ui
        .request(json!({"type": "plan-decide", "planId": plan_id, "approve": false}))
        .expect_err("the first answer wins");
    assert!(late.starts_with(&format!("No plan {plan_id} is waiting")), "{late}");
    wait_for(
        || ui.events_of("plan-settled"),
        |settled| {
            settled
                .iter()
                .any(|event| event["planId"] == plan_id.as_str() && event["decision"] == "approved")
        },
        Duration::from_secs(10),
    );

    let outcome = json_of(&finish(applying));
    assert_eq!(outcome["status"], "created", "{outcome}");
    let created = outcome["sessions"].as_array().cloned().unwrap_or_default();
    let keys: Vec<&str> = created.iter().filter_map(|entry| entry["key"].as_str()).collect();
    // Parents first, whatever the plan's order.
    assert_eq!(keys, ["auth", "tokens", "ui", "script", "tests"], "{outcome}");
    let session_of = |key: &str| {
        created
            .iter()
            .find(|entry| entry["key"] == key)
            .map(|entry| id(&entry["session"]))
            .unwrap_or_default()
    };
    let auth = running(&socket, &session_of("auth"));
    let tokens = running(&socket, &session_of("tokens"));
    let ui_session = running(&socket, &session_of("ui"));
    let script = running(&socket, &session_of("script"));
    let tests = running(&socket, &session_of("tests"));

    // Top-level entries are top-level sessions; run by hand, nobody is recorded as their starter.
    for top in [&auth, &script] {
        assert!(top["parentSessionId"].is_null(), "{top}");
        assert!(top.get("spawnedBySessionId").is_none(), "{top}");
    }
    // A child in a new worktree under a new-worktree parent is stacked on it.
    let auth_branch = auth["worktree"]["branch"].as_str().unwrap_or_default();
    assert!(!auth_branch.is_empty(), "{auth}");
    assert_eq!(tokens["parentSessionId"], auth["id"]);
    assert_eq!(tokens["worktree"]["targetBranch"], auth_branch, "{tokens}");
    assert_eq!(
        tokens["worktree"]["targetWorktreeId"], auth["worktree"]["id"],
        "{tokens}"
    );
    // `same` runs in its parent's worktree.
    assert_eq!(ui_session["parentSessionId"], auth["id"]);
    assert_eq!(ui_session["cwd"], auth["worktree"]["path"], "{ui_session}");
    // `none` runs in the repository's checkout.
    assert_eq!(script["cwd"], p(&root_path));
    assert_eq!(script["worktree"]["mode"], "none");
    // Under an existing session: stacked on its branch.
    assert_eq!(tests["parentSessionId"], login["id"]);
    assert_eq!(tests["worktree"]["targetBranch"], login_branch.as_str(), "{tests}");
    // Each got its brief as its first message.
    for (session, brief) in [
        (&auth, "Brief: rework auth"),
        (&tokens, "Brief: store the tokens"),
        (&ui_session, "Brief: the login UI"),
        (&script, "Brief: the script"),
        (&tests, "Brief: the tests"),
    ] {
        assert!(launched_with(&sandbox, &id(session), brief), "{brief}");
    }
    // Each is on a line of its own, its brief kept as the line's brief (from the plan, run by hand); the lines nest as
    // the sessions do. The task went to the backlog, its details as its line's note.
    let tasks = json_of(&run(&["task", "list", "--json"]));
    let line_of = |title: &str| {
        tasks
            .as_array()
            .and_then(|tasks| tasks.iter().find(|task| task["title"] == title).cloned())
            .unwrap_or_default()
    };
    let doc = must(&socket, json!({"type": "watch-tasks", "cwd": p(&root_path)}));
    let brief_of = |line: &Value| {
        doc["notes"]
            .as_array()
            .and_then(|notes| {
                notes
                    .iter()
                    .find(|note| note["kind"] == "brief" && note["id"] == line["id"])
                    .cloned()
            })
            .unwrap_or_default()
    };
    for (session, title, brief) in [
        (&auth, "Auth rework", "Brief: rework auth"),
        (&tokens, "Token storage", "Brief: store the tokens"),
        (&ui_session, "Login UI", "Brief: the login UI"),
        (&script, "Migration script", "Brief: the script"),
        (&tests, "Login tests", "Brief: the tests"),
    ] {
        let line = line_of(title);
        assert_eq!(line["session"], session["id"], "{tasks}");
        assert!(line["brief"] == true && line["note"] == false, "{line}");
        let note = brief_of(&line);
        assert_eq!(note["text"], format!("{brief}\n"), "{note}");
        assert_eq!(note["from"], "a plan", "{note}");
    }
    let auth_line = line_of("Auth rework");
    assert_eq!(line_of("Token storage")["parent"], auth_line["id"]);
    assert_eq!(line_of("Login UI")["parent"], auth_line["id"]);
    assert!(line_of("Login tests")["parent"].is_null(), "Fix login has no line");
    let task = line_of("Remove legacy endpoint");
    assert!(task["parent"].is_null() && task["session"].is_null(), "{task}");
    assert_eq!(task["note"], true, "{task}");
    assert_eq!(outcome["tasks"][0]["id"], task["id"]);

    stop(&mut daemon);
}

/// The repository's lines (`deckhand task list --json`).
fn lines(sandbox: &Sandbox, bin: &Path, root: &Path) -> Vec<Value> {
    json_of(&cli(sandbox, bin, root, &["task", "list", "--json"], &[]))
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The line titled `title`.
fn line_titled(lines: &[Value], title: &str) -> Value {
    lines
        .iter()
        .find(|line| line["title"] == title)
        .cloned()
        .unwrap_or_else(|| panic!("no line {title}: {lines:#?}"))
}

/// The note of `kind` (`line`, `brief`) of line `id`, as `watch-tasks` answers it.
fn note_of(socket: &Path, root: &Path, kind: &str, id: &Value) -> Value {
    let doc = must(socket, json!({"type": "watch-tasks", "cwd": p(root)}));
    doc["notes"]
        .as_array()
        .and_then(|notes| {
            notes
                .iter()
                .find(|note| note["kind"] == kind && note["id"] == *id)
                .cloned()
        })
        .unwrap_or_default()
}

#[test]
fn an_orchestrators_plan_becomes_its_tree_under_its_idea_and_a_late_ui_still_confirms() {
    let root = repo();
    let sandbox = Sandbox::new("plan-orch");
    let root_path: PathBuf = fs::canonicalize(root.path()).expect("realpath");
    let bin = install_agents(&sandbox, fake_agent!());
    let mut daemon = launch(&sandbox, &root_path, &bin);
    ready(&sandbox.socket());
    let socket = sandbox.socket();
    let run = |args: &[&str]| cli(&sandbox, &bin, &root_path, args, &[]);

    // `deckhand do` starts an orchestrator for the idea: titled by its first line, the idea its first message, on a
    // new top-level line of the same title.
    let started = json_of(&run(&[
        "do",
        "Rework auth\nso that tokens are stored encrypted",
        "--json",
    ]));
    let orchestrator = running(&socket, &id(&started["session"]));
    assert_eq!(orchestrator["role"], "orchestrator");
    assert_eq!(orchestrator["title"], "Rework auth");
    assert!(orchestrator["parentSessionId"].is_null());
    assert_eq!(orchestrator["cwd"], p(&root_path));
    assert!(launched_with(
        &sandbox,
        &id(&orchestrator),
        "so that tokens are stored encrypted"
    ));
    let idea = line_titled(&lines(&sandbox, &bin, &root_path), "Rework auth");
    assert_eq!(idea["session"], orchestrator["id"], "{idea}");
    assert!(idea["parent"].is_null(), "{idea}");
    assert_eq!(started["lineId"], idea["id"]);
    let as_orchestrator = identity(&sandbox, &id(&orchestrator));
    let env = env_of(&as_orchestrator);

    // A session of the user's, outside the orchestrator's tree: a plan may still put work under it (the user
    // confirms), but the orchestrator's commands may not touch it.
    let mine = json_of(&run(&["new", "--title", "Mine", "--worktree", "none", "--json"]));
    let mine = running(&socket, &id(&mine));

    let plan = json!({
        "sessions": [
            {"key": "tokens", "title": "Token storage", "prompt": "Brief: store the tokens"},
            {"key": "schema", "title": "Schema", "prompt": "Brief: the schema", "parent": "tokens"},
            {"key": "theirs", "title": "Their tests", "prompt": "Brief: tests", "parent": &id(&mine)[..8],
             "worktree": "none"},
        ],
        "tasks": [{"title": "Remove legacy endpoint", "details": "Waits for Token storage to merge"}],
        "brief": "Problem: tokens sit in plain text.\nRuled out: SSO.",
    });
    let plan_file = sandbox.root().join("plan.json");
    fs::write(&plan_file, plan.to_string()).expect("write the plan");
    let plan_path = p(&plan_file);

    // Checked from the orchestrator: it goes under the idea.
    let checked = cli(&sandbox, &bin, &root_path, &["plan", "check", &plan_path], &env);
    assert!(checked.status.success(), "{}", text(&checked.stderr));
    let summary = text(&checked.stdout);
    assert!(summary.starts_with("Under ◎ Rework auth:\n"), "{summary}");
    assert!(
        summary.contains("◇ Token storage  (new worktree · claude)"),
        "{summary}"
    );
    assert!(summary.contains("☐ Parked: Remove legacy endpoint"), "{summary}");
    assert!(summary.contains("✎ The idea's brief (2 lines)"), "{summary}");
    // By hand, a brief has no idea to go to.
    let refused = run(&["plan", "check", &plan_path]);
    assert!(!refused.status.success());
    assert!(
        text(&refused.stderr).contains("brief is the idea's brief"),
        "{}",
        text(&refused.stderr)
    );

    // A UI that connects while the plan waits is told about it, and its answer counts. Esc: nothing is created.
    let first_ui = subscribed(&socket, &p(&root_path));
    let cancelled = spawn_cli(&sandbox, &bin, &root_path, &["plan", "apply", &plan_path], &env);
    let waiting = wait_pending(&first_ui, 0);
    let mut ui = subscribed(&socket, &p(&root_path));
    let pending = wait_pending(&ui, 0);
    assert_eq!(pending["id"], waiting["id"]);
    assert_eq!(pending["callerSessionId"], orchestrator["id"]);
    assert_eq!(pending["callerTitle"], "Rework auth");
    assert_eq!(pending["summary"]["idea"], "Rework auth");
    ui.request(json!({"type": "plan-decide", "planId": pending["id"], "approve": false}))
        .expect("reject");
    let cancelled = finish(cancelled);
    assert!(!cancelled.status.success());
    assert!(
        text(&cancelled.stdout).contains("Cancelled in Deckhand: nothing was created."),
        "{}",
        text(&cancelled.stdout)
    );
    let listed = ui.request(json!({"type": "list"})).expect("list");
    assert_eq!(listed.as_array().map(Vec::len), Some(2), "{listed}");

    // Enter: the plan becomes the orchestrator's tree.
    let applying = spawn_cli(
        &sandbox,
        &bin,
        &root_path,
        &["plan", "apply", "--json", &plan_path],
        &env,
    );
    let pending = wait_pending(&ui, 1);
    ui.request(json!({"type": "plan-decide", "planId": pending["id"], "approve": true}))
        .expect("approve");
    let outcome = json_of(&finish(applying));
    assert_eq!(outcome["status"], "created", "{outcome}");
    let session_of = |key: &str| {
        outcome["sessions"]
            .as_array()
            .and_then(|created| created.iter().find(|entry| entry["key"] == key))
            .map(|entry| id(&entry["session"]))
            .unwrap_or_default()
    };
    let tokens = running(&socket, &session_of("tokens"));
    let schema = running(&socket, &session_of("schema"));
    let theirs = running(&socket, &session_of("theirs"));
    // A top-level entry is the orchestrator's sub-session, flat: the orchestrator has no worktree to stack on.
    assert_eq!(tokens["parentSessionId"], orchestrator["id"], "{tokens}");
    assert_eq!(tokens["spawnedBySessionId"], orchestrator["id"], "{tokens}");
    assert!(tokens["worktree"]["targetBranch"].is_null(), "{tokens}");
    assert!(
        !tokens["worktree"]["branch"].as_str().unwrap_or_default().is_empty(),
        "{tokens}"
    );
    // Its children stack on it, as anywhere.
    assert_eq!(schema["parentSessionId"], tokens["id"]);
    assert_eq!(
        schema["worktree"]["targetBranch"], tokens["worktree"]["branch"],
        "{schema}"
    );
    // Under a session outside its tree: there, and only recorded as started by the orchestrator.
    assert_eq!(theirs["parentSessionId"], mine["id"]);
    assert_eq!(theirs["spawnedBySessionId"], orchestrator["id"]);
    // The orchestrator stays: one per idea, it is there to extend it.
    assert_eq!(state(&socket, &id(&orchestrator))["status"], "running");

    // The lines line up with the sessions: under the idea's line, the parked work too; the idea has its brief.
    let all = lines(&sandbox, &bin, &root_path);
    let tokens_line = line_titled(&all, "Token storage");
    assert_eq!(tokens_line["parent"], idea["id"], "{all:#?}");
    assert_eq!(tokens_line["session"], tokens["id"]);
    assert_eq!(line_titled(&all, "Schema")["parent"], tokens_line["id"]);
    let parked = line_titled(&all, "Remove legacy endpoint");
    assert_eq!(parked["parent"], idea["id"], "{parked}");
    assert!(parked["session"].is_null() && parked["note"] == true, "{parked}");
    assert!(
        note_of(&socket, &root_path, "line", &parked["id"])["text"]
            .as_str()
            .is_some_and(|note| note.contains("Waits for Token storage to merge")),
        "{parked}"
    );
    let brief = note_of(&socket, &root_path, "brief", &idea["id"]);
    assert_eq!(
        brief["text"], "Problem: tokens sit in plain text.\nRuled out: SSO.\n",
        "{brief}"
    );
    assert_eq!(brief["from"], "◎ Rework auth", "{brief}");
    let tokens_brief = note_of(&socket, &root_path, "brief", &tokens_line["id"]);
    assert_eq!(tokens_brief["text"], "Brief: store the tokens\n");
    assert_eq!(tokens_brief["from"], "◎ Rework auth");

    // Permissions follow the tree: under its descendants, yes; under the user's session, or its own top-level
    // session, no.
    let child = json_of(&cli(
        &sandbox,
        &bin,
        &root_path,
        &[
            "new",
            "--parent",
            &id(&schema)[..8],
            "--title",
            "Index",
            "--prompt",
            "Brief",
            "--json",
        ],
        &env,
    ));
    let child = running(&socket, &id(&child));
    assert_eq!(child["parentSessionId"], schema["id"]);
    let sibling = json_of(&cli(
        &sandbox,
        &bin,
        &root_path,
        &["new", "--top-level", "--worktree", "none", "--title", "Aside", "--json"],
        &env,
    ));
    assert_eq!(sibling["spawnedBySessionId"], orchestrator["id"], "{sibling}");
    for parent in [id(&mine), id(&theirs), id(&sibling)] {
        let refused = cli(
            &sandbox,
            &bin,
            &root_path,
            &["new", "--parent", &parent, "--title", "Sneaky", "--json"],
            &env,
        );
        assert!(!refused.status.success());
        assert!(
            text(&refused.stderr).contains("starts sessions only under the session it runs in"),
            "{}",
            text(&refused.stderr)
        );
    }
    let ended = cli(&sandbox, &bin, &root_path, &["kill", &id(&child)], &env);
    assert!(ended.status.success(), "{}", text(&ended.stderr));
    wf(|| state(&socket, &id(&child)), |child| child["status"] == "exited");
    for outside in [id(&mine), id(&theirs), id(&sibling)] {
        let refused = cli(&sandbox, &bin, &root_path, &["kill", &outside], &env);
        assert!(!refused.status.success());
        assert!(
            text(&refused.stderr).contains("Only the sessions under this session"),
            "{}",
            text(&refused.stderr)
        );
    }

    // Another session may not apply a plan, or start an orchestrator.
    let as_tokens = identity(&sandbox, &id(&tokens));
    let refused = cli(
        &sandbox,
        &bin,
        &root_path,
        &["plan", "apply", &plan_path],
        &env_of(&as_tokens),
    );
    assert!(!refused.status.success());
    assert!(
        text(&refused.stderr).contains("runs only in an orchestrator session"),
        "{}",
        text(&refused.stderr)
    );
    let refused = cli(&sandbox, &bin, &root_path, &["do", "More work"], &env_of(&as_tokens));
    assert!(!refused.status.success());
    assert!(
        text(&refused.stderr).contains("deckhand do is for you"),
        "{}",
        text(&refused.stderr)
    );

    stop(&mut daemon);
}

#[test]
fn the_idea_ticks_when_every_line_under_it_is_ticked() {
    let root = repo();
    let sandbox = Sandbox::new("plan-idea-done");
    let root_path: PathBuf = fs::canonicalize(root.path()).expect("realpath");
    let bin = install_agents(&sandbox, fake_agent!());
    let mut daemon = launch(&sandbox, &root_path, &bin);
    ready(&sandbox.socket());
    let socket = sandbox.socket();
    let run = |args: &[&str]| cli(&sandbox, &bin, &root_path, args, &[]);

    let started = json_of(&run(&["do", "Tidy the docs", "--json"]));
    let orchestrator = running(&socket, &id(&started["session"]));
    let env_identity = identity(&sandbox, &id(&orchestrator));
    let plan = json!({"tasks": [{"title": "Fix the links"}, {"title": "Spell check"}]});
    let plan_file = sandbox.root().join("plan.json");
    fs::write(&plan_file, plan.to_string()).expect("write the plan");
    let mut ui = subscribed(&socket, &p(&root_path));
    let applying = spawn_cli(
        &sandbox,
        &bin,
        &root_path,
        &["plan", "apply", "--json", &p(&plan_file)],
        &env_of(&env_identity),
    );
    let pending = wait_pending(&ui, 0);
    ui.request(json!({"type": "plan-decide", "planId": pending["id"], "approve": true}))
        .expect("approve");
    let outcome = json_of(&finish(applying));
    assert_eq!(outcome["status"], "created", "{outcome}");

    let idea_id = started["lineId"].clone();
    let idea = |all: &[Value]| {
        all.iter()
            .find(|line| line["id"] == idea_id)
            .cloned()
            .unwrap_or_default()
    };
    let all = lines(&sandbox, &bin, &root_path);
    assert_eq!(
        (idea(&all)["ticked"].clone(), idea(&all)["total"].clone()),
        (json!(0), json!(2))
    );
    let done = run(&["task", "done", outcome["tasks"][0]["id"].as_str().unwrap_or_default()]);
    assert!(done.status.success(), "{}", text(&done.stderr));
    assert_eq!(idea(&lines(&sandbox, &bin, &root_path))["done"], false);
    let done = run(&["task", "done", outcome["tasks"][1]["id"].as_str().unwrap_or_default()]);
    assert!(done.status.success(), "{}", text(&done.stderr));
    let all = lines(&sandbox, &bin, &root_path);
    assert_eq!(idea(&all)["done"], true, "{all:#?}");
    // Reopening a line under it reopens the idea.
    let reopened = run(&["task", "reopen", outcome["tasks"][1]["id"].as_str().unwrap_or_default()]);
    assert!(reopened.status.success(), "{}", text(&reopened.stderr));
    assert_eq!(idea(&lines(&sandbox, &bin, &root_path))["done"], false);

    stop(&mut daemon);
}

#[test]
fn do_always_starts_a_new_orchestrator_and_several_run_at_once() {
    let root = repo();
    let sandbox = Sandbox::new("plan-do");
    let root_path: PathBuf = fs::canonicalize(root.path()).expect("realpath");
    let bin = install_agents(&sandbox, fake_agent!());
    let mut daemon = launch(&sandbox, &root_path, &bin);
    ready(&sandbox.socket());
    let socket = sandbox.socket();
    let run = |args: &[&str]| cli(&sandbox, &bin, &root_path, args, &[]);

    let first = run(&["do", "Plan the auth work"]);
    assert!(first.status.success(), "{}", text(&first.stderr));
    let printed = text(&first.stdout);
    assert!(printed.starts_with("Started an orchestrator "), "{printed}");
    assert!(printed.contains("for your idea: ◎ Plan the auth work."), "{printed}");
    assert!(
        printed.contains("select ◎ Plan the auth work and press enter to attach"),
        "{printed}"
    );
    let orchestrators = || {
        ui_list(&socket)
            .into_iter()
            .filter(|session| session["role"] == "orchestrator")
            .collect::<Vec<_>>()
    };
    let first = wf(orchestrators, |found| found.len() == 1)
        .first()
        .cloned()
        .unwrap_or_default();
    let first = running(&socket, &id(&first));
    // Once it runs and sits idle, a second request is not typed into it: it starts another orchestrator, numbered
    // when the idea's name is taken.
    wait_for(
        || state(&socket, &id(&first)),
        |session| session["agentStatus"] == "idle",
        Duration::from_secs(20),
    );
    let second = json_of(&run(&["do", "Plan the auth work", "--json"]));
    assert!(second.get("delivered").is_none(), "{second}");
    assert_ne!(id(&second["session"]), id(&first));
    let second = running(&socket, &id(&second["session"]));
    assert_eq!(second["title"], "Plan the auth work 2");
    let third = json_of(&run(&["do", "Speed up the build", "--json"]));
    let third = running(&socket, &id(&third["session"]));
    assert!(launched_with(&sandbox, &id(&third), "Speed up the build"));
    assert_eq!(orchestrators().len(), 3);
    let input = sandbox.home().join(format!("input-{}", id(&first)));
    assert!(
        !fs::read_to_string(&input)
            .unwrap_or_default()
            .contains("Plan the auth work"),
        "nothing was typed into the first"
    );
    // Each is on its own idea's line.
    let all = lines(&sandbox, &bin, &root_path);
    for (orchestrator, title) in [
        (&first, "Plan the auth work"),
        (&second, "Plan the auth work 2"),
        (&third, "Speed up the build"),
    ] {
        let line = line_titled(&all, title);
        assert_eq!(line["session"], orchestrator["id"], "{line}");
        assert!(line["parent"].is_null(), "{line}");
    }

    // An exited one resumes while the others run (no longer one per repository).
    let mut ui = subscribed(&socket, &p(&root_path));
    ui.request(json!({"type": "kill", "sessionId": id(&first)}))
        .expect("kill");
    wf(|| state(&socket, &id(&first)), |session| session["status"] == "exited");
    ui.request(json!({"type": "restart", "sessionId": id(&first), "cols": 80, "rows": 24}))
        .expect("restart");
    running(&socket, &id(&first));
    // Without an idea, nothing starts.
    let refused = run(&["do", " "]);
    assert!(!refused.status.success());
    assert_eq!(orchestrators().len(), 3);

    stop(&mut daemon);
}

/// Every session the daemon has.
fn ui_list(socket: &Path) -> Vec<Value> {
    deckhand_e2e::daemon::must(socket, json!({"type": "list"}))
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[test]
fn a_plan_that_fails_midway_keeps_what_it_created_and_says_what_failed() {
    let root = repo();
    let sandbox = Sandbox::new("plan-fail");
    let root_path: PathBuf = fs::canonicalize(root.path()).expect("realpath");
    let bin = install_agents(&sandbox, fake_agent!());
    let mut daemon = launch(&sandbox, &root_path, &bin);
    ready(&sandbox.socket());
    let socket = sandbox.socket();
    let run = |args: &[&str]| cli(&sandbox, &bin, &root_path, args, &[]);

    let plan = json!({
        "sessions": [
            {"key": "first", "title": "First", "prompt": "Brief: first", "worktree": "none"},
            {"key": "second", "title": "Second", "prompt": "Brief: second", "worktree": "none"},
            {"key": "third", "title": "Third", "prompt": "Brief: third", "worktree": "none"},
        ],
        "tasks": [{"title": "Never added"}],
    });
    let plan_file = sandbox.root().join("plan.json");
    fs::write(&plan_file, plan.to_string()).expect("write the plan");
    let mut ui = subscribed(&socket, &p(&root_path));
    let applying = spawn_cli(&sandbox, &bin, &root_path, &["plan", "apply", &p(&plan_file)], &[]);
    let pending = wait_pending(&ui, 0);
    // While it waits, a session takes the second one's name: creating it fails when the plan gets there.
    let taken = run(&["new", "--title", "Second", "--worktree", "none", "--json"]);
    assert!(taken.status.success(), "{}", text(&taken.stderr));
    ui.request(json!({"type": "plan-decide", "planId": pending["id"], "approve": true}))
        .expect("approve");
    let failed = finish(applying);
    assert!(!failed.status.success());
    let printed = text(&failed.stdout);
    assert!(
        printed.contains("First  (no worktree (the repository's checkout) · claude)"),
        "{printed}"
    );
    assert!(
        printed.contains("Stopped at Second (second): an active session named \"Second\" already exists in this repo"),
        "{printed}"
    );
    assert!(printed.contains("They are kept (nothing is rolled back)"), "{printed}");
    assert!(
        text(&failed.stderr).contains("The plan was created only in part"),
        "{}",
        text(&failed.stderr)
    );
    // What was created before the failure stays; nothing after it was created.
    let titles: Vec<String> = ui_list(&socket)
        .iter()
        .filter_map(|session| session["title"].as_str().map(str::to_owned))
        .collect();
    assert!(titles.contains(&"First".to_owned()), "{titles:?}");
    assert!(!titles.contains(&"Third".to_owned()), "{titles:?}");
    // The list holds the line of the session created (its brief kept), not the plan's tasks.
    let tasks = json_of(&run(&["task", "list", "--json"]));
    let titles: Vec<&str> = tasks
        .as_array()
        .map(|tasks| tasks.iter().filter_map(|task| task["title"].as_str()).collect())
        .unwrap_or_default();
    assert_eq!(titles, ["First"], "{tasks}");
    assert_eq!(tasks[0]["brief"], true, "{tasks}");

    stop(&mut daemon);
}
