//! The orchestrator, black-box: `deckhand plan check|apply` against a real daemon with fake agents, a UI connection
//! answering the confirmation (`plan-pending` → `plan-decide`), `deckhand do` starting the orchestrator and handing a
//! running one a request, and `deckhand new --parent` from the orchestrator.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Output, Stdio};
use std::time::Duration;

use deckhand_e2e::client::Client;
use deckhand_e2e::daemon::{install_agents, launch, p, ready, running, state, subscribed, wf};
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
    // The task went to the backlog.
    let tasks = json_of(&run(&["task", "list", "--json"]));
    let task = tasks
        .as_array()
        .and_then(|tasks| {
            tasks
                .iter()
                .find(|task| task["title"] == "Remove legacy endpoint")
                .cloned()
        })
        .unwrap_or_default();
    assert_eq!(task["group"], "backlog", "{tasks}");
    assert_eq!(task["details"], "After Auth rework merges", "{task}");
    assert_eq!(outcome["tasks"][0]["id"], task["id"]);

    stop(&mut daemon);
}

#[test]
fn the_orchestrator_applies_plans_it_is_recorded_as_starting_and_a_late_ui_still_confirms() {
    let root = repo();
    let sandbox = Sandbox::new("plan-orch");
    let root_path: PathBuf = fs::canonicalize(root.path()).expect("realpath");
    let bin = install_agents(&sandbox, fake_agent!());
    let mut daemon = launch(&sandbox, &root_path, &bin);
    ready(&sandbox.socket());
    let socket = sandbox.socket();

    // `deckhand do` starts the orchestrator with the request as its first message.
    let started = json_of(&cli(
        &sandbox,
        &bin,
        &root_path,
        &["do", "Split", "the", "auth", "rework", "--json"],
        &[],
    ));
    assert_eq!(started["started"], true, "{started}");
    let orchestrator = running(&socket, &id(&started["session"]));
    assert_eq!(orchestrator["role"], "orchestrator");
    assert_eq!(orchestrator["title"], "Orchestrator");
    assert!(orchestrator["parentSessionId"].is_null());
    assert_eq!(orchestrator["cwd"], p(&root_path));
    assert!(launched_with(&sandbox, &id(&orchestrator), "Split the auth rework"));
    let as_orchestrator = identity(&sandbox, &id(&orchestrator));
    let env = env_of(&as_orchestrator);

    let plan = json!({"sessions": [{"key": "auth", "title": "Auth rework", "prompt": "Brief: rework auth"}]});
    let plan_file = sandbox.root().join("plan.json");
    fs::write(&plan_file, plan.to_string()).expect("write the plan");
    let plan_path = p(&plan_file);

    // A UI that connects while the plan waits is told about it, and its answer counts. Esc: nothing is created.
    let first_ui = subscribed(&socket, &p(&root_path));
    let cancelled = spawn_cli(&sandbox, &bin, &root_path, &["plan", "apply", &plan_path], &env);
    let waiting = wait_pending(&first_ui, 0);
    let mut ui = subscribed(&socket, &p(&root_path));
    let pending = wait_pending(&ui, 0);
    assert_eq!(pending["id"], waiting["id"]);
    assert_eq!(pending["callerSessionId"], orchestrator["id"]);
    assert_eq!(pending["callerTitle"], "Orchestrator");
    ui.request(json!({"type": "plan-decide", "planId": pending["id"], "approve": false}))
        .expect("reject");
    let cancelled = finish(cancelled);
    assert!(!cancelled.status.success());
    assert!(
        text(&cancelled.stdout).contains("Cancelled in Deckhand: nothing was created."),
        "{}",
        text(&cancelled.stdout)
    );
    assert!(
        text(&cancelled.stderr).contains("Cancelled in Deckhand"),
        "{}",
        text(&cancelled.stderr)
    );
    let listed = ui.request(json!({"type": "list"})).expect("list");
    assert_eq!(listed.as_array().map(Vec::len), Some(1), "{listed}");

    // Enter: its sessions are top-level and recorded as started by the orchestrator.
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
    let auth = running(&socket, &id(&outcome["sessions"][0]["session"]));
    assert!(auth["parentSessionId"].is_null(), "{auth}");
    assert_eq!(auth["spawnedBySessionId"], orchestrator["id"], "{auth}");

    // It may start sessions under what it started (and end them); not under the user's own sessions.
    let child = json_of(&cli(
        &sandbox,
        &bin,
        &root_path,
        &[
            "new",
            "--parent",
            &id(&auth)[..8],
            "--title",
            "Token storage",
            "--prompt",
            "Brief",
            "--json",
        ],
        &env,
    ));
    let child = running(&socket, &id(&child));
    assert_eq!(child["parentSessionId"], auth["id"]);
    assert_eq!(child["spawnedBySessionId"], orchestrator["id"]);
    assert_eq!(child["worktree"]["targetBranch"], auth["worktree"]["branch"], "{child}");
    let users = json_of(&cli(
        &sandbox,
        &bin,
        &root_path,
        &["new", "--title", "Mine", "--json"],
        &[],
    ));
    let refused = cli(
        &sandbox,
        &bin,
        &root_path,
        &["new", "--parent", &id(&users), "--title", "Sneaky", "--json"],
        &env,
    );
    assert!(!refused.status.success());
    assert!(
        text(&refused.stderr).contains("starts sessions only under the session it runs in"),
        "{}",
        text(&refused.stderr)
    );
    let ended = cli(&sandbox, &bin, &root_path, &["kill", &id(&child)], &env);
    assert!(ended.status.success(), "{}", text(&ended.stderr));
    wf(|| state(&socket, &id(&child)), |child| child["status"] == "exited");
    let refused = cli(&sandbox, &bin, &root_path, &["kill", &id(&users)], &env);
    assert!(!refused.status.success());

    // Another session may not apply a plan, or hand requests to the orchestrator.
    let as_auth = identity(&sandbox, &id(&auth));
    let refused = cli(
        &sandbox,
        &bin,
        &root_path,
        &["plan", "apply", &plan_path],
        &env_of(&as_auth),
    );
    assert!(!refused.status.success());
    assert!(
        text(&refused.stderr).contains("runs only in the repository's orchestrator session"),
        "{}",
        text(&refused.stderr)
    );
    let refused = cli(&sandbox, &bin, &root_path, &["do", "More work"], &env_of(&as_auth));
    assert!(!refused.status.success());
    assert!(
        text(&refused.stderr).contains("deckhand do is for you"),
        "{}",
        text(&refused.stderr)
    );

    stop(&mut daemon);
}

#[test]
fn do_hands_a_running_orchestrator_the_request_only_while_it_is_idle() {
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
    assert!(printed.starts_with("Started the orchestrator "), "{printed}");
    assert!(
        printed.contains("select Orchestrator and press enter to attach"),
        "{printed}"
    );
    let orchestrator = wf(
        || {
            ui_list(&socket)
                .into_iter()
                .find(|session| session["role"] == "orchestrator")
        },
        Option::is_some,
    )
    .unwrap_or_default();
    let orchestrator_id = id(&orchestrator);
    assert!(printed.contains(&orchestrator_id[..8]), "{printed}");
    // Once its agent sits idle at its input, a second request is typed and sent.
    wait_for(
        || state(&socket, &orchestrator_id),
        |session| session["status"] == "running" && session["agentStatus"] == "idle",
        Duration::from_secs(20),
    );
    let second = run(&["do", "Also", "the", "login", "UI", "--json"]);
    let delivered = json_of(&second);
    assert_eq!(
        (delivered["started"].clone(), delivered["delivered"].clone()),
        (json!(false), json!(true))
    );
    assert_eq!(id(&delivered["session"]), orchestrator_id);
    let input = sandbox.home().join(format!("input-{orchestrator_id}"));
    wait_for(
        || fs::read_to_string(&input).unwrap_or_default(),
        |typed| typed.contains("\u{1b}[200~Also the login UI\u{1b}[201~\r"),
        Duration::from_secs(10),
    );
    // Only one orchestrator: no second one started.
    let orchestrators = ui_list(&socket)
        .into_iter()
        .filter(|session| session["role"] == "orchestrator")
        .count();
    assert_eq!(orchestrators, 1);

    // While it works, nothing is sent.
    let as_orchestrator = identity(&sandbox, &orchestrator_id);
    let mut hook = sandbox.cli(&["hook"]);
    hook.current_dir(&root_path).stdin(Stdio::piped()).stdout(Stdio::null());
    for (key, value) in &as_orchestrator {
        hook.env(key, value);
    }
    let mut hook = sandbox.spawn(hook);
    {
        use std::io::Write;
        let mut stdin = hook.stdin.take().expect("stdin");
        stdin
            .write_all(br#"{"hook_event_name":"UserPromptSubmit","prompt":"x"}"#)
            .expect("hook input");
    }
    wait_exit(&mut hook, Duration::from_secs(10)).expect("the hook finished");
    wait_for(
        || state(&socket, &orchestrator_id),
        |session| session["attention"]["state"] == "working",
        Duration::from_secs(10),
    );
    let before = fs::read_to_string(&input).unwrap_or_default();
    let busy = run(&["do", "Third"]);
    assert!(!busy.status.success());
    assert!(
        text(&busy.stderr).contains("is busy with a turn: try again when it is idle; nothing was sent"),
        "{}",
        text(&busy.stderr)
    );
    assert_eq!(fs::read_to_string(&input).unwrap_or_default(), before);

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
    let tasks = json_of(&run(&["task", "list", "--json"]));
    assert_eq!(tasks.as_array().map(Vec::len), Some(0), "{tasks}");

    stop(&mut daemon);
}
