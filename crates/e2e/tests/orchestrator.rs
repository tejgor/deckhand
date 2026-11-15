//! The orchestrator, black-box: `deckhand plan check|apply` against a real daemon with fake agents (`apply` returns
//! at once), a UI connection answering the confirmation (`plan-pending` → `plan-decide`), the answer reaching the
//! orchestrator as a message, `deckhand do` starting a new orchestrator per idea, an orchestrator's plan becoming its
//! tree under its idea's line, and what its commands may do (its tree).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use deckhand_e2e::client::Client;
use deckhand_e2e::daemon::{install_agents, launch, must, p, ready, running, state, subscribed, text_of, wf};
use deckhand_e2e::fake_agent;
use deckhand_e2e::harness::{Sandbox, repo, stop, wait_for};
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

/// The session titled `title`, once a plan created it (creating runs after the answer, in the background).
fn created(socket: &Path, title: &str) -> Value {
    wf(
        || ui_list(socket).into_iter().find(|session| session["title"] == title),
        Option::is_some,
    )
    .unwrap_or_default()
}

/// The notices queued to session `id` (the answers to its plans), once there are `count`.
fn notices(socket: &Path, id: &str, count: usize) -> Vec<String> {
    wf(
        || {
            must(socket, json!({"type": "list-messages", "sessionId": id}))
                .as_array()
                .map(|messages| {
                    messages
                        .iter()
                        .filter(|message| message["kind"] == "notice" && message["toSessionId"] == id)
                        .filter_map(|message| message["text"].as_str().map(str::to_owned))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        },
        |texts| texts.len() >= count,
    )
}

/// What has been typed into session `id`'s fake agent, once it holds `needle`.
fn typed_containing(sandbox: &Sandbox, id: &str, needle: &str) -> String {
    wf(
        || text_of(&sandbox.home().join(format!("input-{id}"))).unwrap_or_default(),
        |input| input.contains(needle),
    )
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
    // Applying returns at once: the plan waits for the answer in Deckhand.
    let waiting = json_of(&run(&["plan", "apply", &plan_path, "--json"]));
    let plan_id = waiting["planId"].as_str().unwrap_or_default().to_owned();
    assert_eq!(plan_id.len(), 8, "{waiting}");
    assert_eq!(waiting["summary"]["counts"], "5 sessions, 1 task");
    let pending = wait_pending(&ui, 0);
    assert_eq!(pending["id"], plan_id.as_str());
    assert_eq!(pending["repoRoot"], p(&root_path));
    assert_eq!(pending["summary"]["counts"], "5 sessions, 1 task");
    // Nothing is created before the answer.
    let listed = ui.request(json!({"type": "list"})).expect("list");
    assert_eq!(listed.as_array().map(Vec::len), Some(1), "{listed}");

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

    let auth = running(&socket, &id(&created(&socket, "Auth rework")));
    let tokens = running(&socket, &id(&created(&socket, "Token storage")));
    let ui_session = running(&socket, &id(&created(&socket, "Login UI")));
    let script = running(&socket, &id(&created(&socket, "Migration script")));
    let tests = running(&socket, &id(&created(&socket, "Login tests")));

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

    // Applied from the orchestrator, it returns at once and tells it to end its turn.
    let first_ui = subscribed(&socket, &p(&root_path));
    let applied = cli(&sandbox, &bin, &root_path, &["plan", "apply", &plan_path], &env);
    assert!(applied.status.success(), "{}", text(&applied.stderr));
    let waiting = wait_pending(&first_ui, 0);
    let cancelled_id = id(&waiting);
    assert_eq!(
        text(&applied.stdout).trim_end(),
        format!(
            "Plan {cancelled_id} is waiting for the user's answer in Deckhand. You'll get a message when they \
             answer; end your turn now."
        )
    );
    // A UI that connects while the plan waits is told about it, and its answer counts. Cancelled: nothing is created,
    // and the orchestrator is told, with the user's note.
    let mut ui = subscribed(&socket, &p(&root_path));
    let pending = wait_pending(&ui, 0);
    assert_eq!(pending["id"], waiting["id"]);
    assert_eq!(pending["callerSessionId"], orchestrator["id"]);
    assert_eq!(pending["callerTitle"], "Rework auth");
    assert_eq!(pending["summary"]["idea"], "Rework auth");
    ui.request(json!({"type": "plan-decide", "planId": pending["id"], "approve": false,
        "note": "Fewer sessions, please"}))
        .expect("cancel");
    let cancel_text = format!(
        "[The user cancelled your plan {cancelled_id}: nothing was created]\nTheir note: Fewer sessions, please"
    );
    assert_eq!(notices(&socket, &id(&orchestrator), 1), [cancel_text]);
    // Typed into the orchestrator once it is idle, as any message; it answers in a turn (its hooks say so).
    typed_containing(&sandbox, &id(&orchestrator), "Their note: Fewer sessions, please");
    for event in ["UserPromptSubmit", "Stop"] {
        must(
            &socket,
            json!({"type": "agent-hook", "sessionId": id(&orchestrator), "launchId": as_orchestrator[1].1,
                "token": as_orchestrator[2].1, "payload": {"hook_event_name": event}}),
        );
    }
    let listed = ui.request(json!({"type": "list"})).expect("list");
    assert_eq!(listed.as_array().map(Vec::len), Some(2), "{listed}");

    // Put aside, it keeps waiting; then approved, the plan becomes the orchestrator's tree.
    let applied = json_of(&cli(
        &sandbox,
        &bin,
        &root_path,
        &["plan", "apply", "--json", &plan_path],
        &env,
    ));
    let plan_id = applied["planId"].as_str().unwrap_or_default().to_owned();
    let pending = wait_pending(&ui, 1);
    assert_eq!(pending["id"], plan_id.as_str());
    ui.request(json!({"type": "plan-park", "planId": plan_id}))
        .expect("put aside");
    let parked = wait_pending(&ui, 2);
    assert_eq!(
        (parked["id"].as_str(), parked["parked"].as_bool()),
        (Some(plan_id.as_str()), Some(true))
    );
    ui.request(json!({"type": "plan-decide", "planId": plan_id, "approve": true}))
        .expect("approve");
    let tokens = running(&socket, &id(&created(&socket, "Token storage")));
    let schema = running(&socket, &id(&created(&socket, "Schema")));
    let theirs = running(&socket, &id(&created(&socket, "Their tests")));
    // The orchestrator is told what was created: a line per session (its path, where it works, its agent), then the
    // parked work.
    let told = notices(&socket, &id(&orchestrator), 2);
    let approved = told
        .iter()
        .find(|notice| notice.starts_with("[The user approved"))
        .cloned()
        .unwrap_or_default();
    assert!(
        approved.starts_with(&format!(
            "[The user approved your plan {plan_id}]\nCreated 3 sessions and 1 task; each session got its brief as its \
             first message:\n"
        )),
        "{approved}"
    );
    let tokens_branch = created(&socket, "Token storage")["worktree"]["branch"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        approved.contains(&format!(
            "{}  Rework auth / Token storage  (⎇ {tokens_branch}, flat · claude)",
            &id(&tokens)[..8]
        )),
        "{approved}"
    );
    assert!(
        approved.contains("Rework auth / Token storage / Schema  (⎇ "),
        "{approved}"
    );
    assert!(
        approved.contains("☐ Remove legacy endpoint  (parked under your idea)"),
        "{approved}"
    );
    typed_containing(
        &sandbox,
        &id(&orchestrator),
        &format!("[The user approved your plan {plan_id}]"),
    );
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
    let applied = json_of(&cli(
        &sandbox,
        &bin,
        &root_path,
        &["plan", "apply", "--json", &p(&plan_file)],
        &env_of(&env_identity),
    ));
    ui.request(json!({"type": "plan-decide", "planId": applied["planId"], "approve": true}))
        .expect("approve");
    notices(&socket, &id(&orchestrator), 1);
    let line_id = |title: &str| {
        line_titled(&lines(&sandbox, &bin, &root_path), title)["id"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    let (links, spelling) = (line_id("Fix the links"), line_id("Spell check"));

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
    let done = run(&["task", "done", &links]);
    assert!(done.status.success(), "{}", text(&done.stderr));
    assert_eq!(idea(&lines(&sandbox, &bin, &root_path))["done"], false);
    let done = run(&["task", "done", &spelling]);
    assert!(done.status.success(), "{}", text(&done.stderr));
    let all = lines(&sandbox, &bin, &root_path);
    assert_eq!(idea(&all)["done"], true, "{all:#?}");
    // Reopening a line under it reopens the idea.
    let reopened = run(&["task", "reopen", &spelling]);
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
    // --title names it (and its line) instead of the idea's first line.
    let idea = "Store the session tokens encrypted, so a stolen disk does not leak them";
    let named = run(&["do", "--title", " Auth tokens ", idea]);
    assert!(
        text(&named.stdout).contains("for your idea: ◎ Auth tokens."),
        "{}",
        text(&named.stdout)
    );
    let named = wf(orchestrators, |found| found.len() == 4)
        .into_iter()
        .find(|session| session["title"] == "Auth tokens")
        .unwrap_or_default();
    let named = running(&socket, &id(&named));
    assert!(launched_with(&sandbox, &id(&named), idea));
    assert_eq!(orchestrators().len(), 4);
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
        (&named, "Auth tokens"),
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
    assert_eq!(orchestrators().len(), 4);

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
    let applied = run(&["plan", "apply", &p(&plan_file)]);
    assert!(applied.status.success(), "{}", text(&applied.stderr));
    let pending = wait_pending(&ui, 0);
    assert_eq!(
        text(&applied.stdout).trim_end(),
        format!(
            "Plan {} (3 sessions, 1 task) is waiting for your answer in Deckhand: open it in this repository (enter \
             creates it, x x cancels it).",
            id(&pending)
        )
    );
    // While it waits, a session takes the second one's name: creating it fails when the plan gets there.
    let taken = run(&["new", "--title", "Second", "--worktree", "none", "--json"]);
    assert!(taken.status.success(), "{}", text(&taken.stderr));
    ui.request(json!({"type": "plan-decide", "planId": pending["id"], "approve": true}))
        .expect("approve");
    created(&socket, "First");
    // The line of the session created comes after it: creation has stopped by then.
    wf(
        || lines(&sandbox, &bin, &root_path),
        |lines| lines.iter().any(|line| line["title"] == "First"),
    );
    std::thread::sleep(Duration::from_millis(500));
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
