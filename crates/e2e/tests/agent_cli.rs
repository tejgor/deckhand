//! The `deckhand` commands agents run (`new`, `task`, `ls`, `note`, `kill`), black-box: the CLI against a real
//! daemon with fake agents, run by hand (no caller) and from inside a session (its `DECKHAND_SESSION_ID`,
//! `DECKHAND_LAUNCH_ID` and `DECKHAND_HOOK_TOKEN`, as the fake agent saw them), directly and through `deckhand mcp`
//! (Codex's MCP server).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use deckhand_e2e::daemon::{install_agents, launch, p, ready, running, state, wf};
use deckhand_e2e::fake_agent;
use deckhand_e2e::harness::{Sandbox, repo, stop};
use serde_json::Value;

/// `deckhand <args>` in `cwd`, with only the fakes and system tools on PATH and `env` added.
fn cli(sandbox: &Sandbox, bin: &Path, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = sandbox.cli(args);
    command
        .current_dir(cwd)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()));
    for (key, value) in env {
        command.env(key, value);
    }
    sandbox.exec(command, Duration::from_secs(40))
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

fn args_hold(trace: &Value, needle: &str) -> bool {
    trace["args"].as_array().is_some_and(|args| {
        args.iter()
            .any(|arg| arg.as_str().is_some_and(|arg| arg.contains(needle)))
    })
}

#[test]
fn by_hand_new_starts_a_prompted_session_and_tasks_and_ls_list_the_repository() {
    let root = repo();
    let sandbox = Sandbox::new("agent-cli");
    let root_path: PathBuf = fs::canonicalize(root.path()).expect("realpath");
    let bin = install_agents(&sandbox, fake_agent!());
    let mut daemon = launch(&sandbox, &root_path, &bin);
    ready(&sandbox.socket());
    let run = |args: &[&str]| cli(&sandbox, &bin, &root_path, args, &[]);

    let created = json_of(&run(&[
        "new",
        "--top-level",
        "--worktree",
        "none",
        "--prompt",
        "Say hello\nThen wait for me.",
        "--json",
    ]));
    let id = created["id"].as_str().unwrap_or_default().to_owned();
    assert_eq!(created["title"], "Say hello");
    assert_eq!(created["program"], "claude");
    assert_eq!(created["repoRoot"], p(&root_path));
    assert!(created["parentSessionId"].is_null(), "{created}");
    running(&sandbox.socket(), &id);
    // The prompt reached the agent at launch.
    let launched = trace(&sandbox, &id);
    assert!(args_hold(&launched, "Then wait for me."), "{launched}");

    let added = json_of(&run(&[
        "task",
        "add",
        "Write",
        "the",
        "docs",
        "--details",
        "For the CLI",
        "--json",
    ]));
    assert_eq!(added["title"], "Write the docs");
    assert_eq!(added["details"], "For the CLI");
    assert_eq!(added["group"], "backlog");
    let listed = json_of(&run(&["task", "list", "--json"]));
    assert!(
        listed
            .as_array()
            .is_some_and(|tasks| tasks.iter().any(|task| task["id"] == added["id"])),
        "{listed}"
    );
    let board = text(&run(&["task", "list"]).stdout);
    assert!(
        board.contains("Backlog (1)\n") && board.contains("  Write the docs"),
        "{board}"
    );

    let sessions = json_of(&run(&["ls", "--json"]));
    let listed = sessions
        .as_array()
        .and_then(|sessions| sessions.iter().find(|session| session["id"] == id.as_str()).cloned())
        .unwrap_or_default();
    assert_eq!(listed["title"], "Say hello", "{sessions}");
    let table = text(&run(&["ls"]).stdout);
    assert!(table.contains(&id[..8]) && table.contains("Say hello"), "{table}");

    // By hand there is no session whose note to add to.
    let refused = run(&["note", "add", "Check it"]);
    assert!(!refused.status.success());
    assert!(
        text(&refused.stderr).contains("needs a Deckhand session"),
        "{}",
        text(&refused.stderr)
    );

    stop(&mut daemon);
}

#[test]
fn inside_a_session_new_starts_a_sub_session_it_may_end_and_note_add_adds_a_step() {
    let root = repo();
    let sandbox = Sandbox::new("agent-cli-caller");
    let root_path: PathBuf = fs::canonicalize(root.path()).expect("realpath");
    let bin = install_agents(&sandbox, fake_agent!());
    let mut daemon = launch(&sandbox, &root_path, &bin);
    ready(&sandbox.socket());
    let by_hand = |args: &[&str]| cli(&sandbox, &bin, &root_path, args, &[]);

    let lead = json_of(&by_hand(&["new", "--title", "Lead", "--json"]));
    let lead_id = lead["id"].as_str().unwrap_or_default().to_owned();
    let lead = running(&sandbox.socket(), &lead_id);
    let lead_cwd = PathBuf::from(lead["cwd"].as_str().unwrap_or_default());
    let identity = trace(&sandbox, &lead_id);
    let (launch_id, token) = (
        identity["launchId"].as_str().unwrap_or_default().to_owned(),
        identity["token"].as_str().unwrap_or_default().to_owned(),
    );
    let env = [
        ("DECKHAND_SESSION_ID", lead_id.as_str()),
        ("DECKHAND_LAUNCH_ID", launch_id.as_str()),
        ("DECKHAND_HOOK_TOKEN", token.as_str()),
    ];
    let as_lead = |args: &[&str]| cli(&sandbox, &bin, &lead_cwd, args, &env);

    let child = json_of(&as_lead(&["new", "--prompt", "Write the tests", "--json"]));
    let child_id = child["id"].as_str().unwrap_or_default().to_owned();
    assert_eq!(child["parentSessionId"], lead_id.as_str(), "{child}");
    assert_eq!(child["title"], "Write the tests");
    assert_eq!(child["requestedWorktreeMode"], "new");
    running(&sandbox.socket(), &child_id);

    let table = text(&as_lead(&["ls"]).stdout);
    assert!(table.contains(&format!("*  {}", &lead_id[..8])), "{table}");
    assert!(table.contains("Lead / Write the tests"), "{table}");

    let step = as_lead(&["note", "add", "Review the child's tests"]);
    assert!(step.status.success(), "{}", text(&step.stderr));
    let lead = state(&sandbox.socket(), &lead_id);
    let note = lead["sharedNotes"]["text"]
        .as_str()
        .or(lead["notes"].as_str())
        .unwrap_or_default();
    assert!(note.contains("- [ ] Review the child's tests"), "{lead}");

    let ended = as_lead(&["kill", &child_id[..8]]);
    assert!(ended.status.success(), "{}", text(&ended.stderr));
    wf(
        || state(&sandbox.socket(), &child_id),
        |child| child["status"] == "exited",
    );
    // The lead is not its own sub-session.
    let refused = as_lead(&["kill", &lead_id]);
    assert!(!refused.status.success(), "{}", text(&refused.stdout));

    stop(&mut daemon);
}

/// `deckhand mcp` (Codex's MCP server) over real stdio, as Codex starts it for a session: its environment holds the
/// session's identity and Deckhand home (Codex forwards them by name), and its tool runs the same commands.
#[test]
fn the_mcp_server_answers_over_stdio_and_its_tool_acts_for_the_session() {
    use std::io::Write;
    use std::process::Stdio;

    let root = repo();
    let sandbox = Sandbox::new("agent-mcp");
    let root_path: PathBuf = fs::canonicalize(root.path()).expect("realpath");
    let bin = install_agents(&sandbox, fake_agent!());
    let mut daemon = launch(&sandbox, &root_path, &bin);
    ready(&sandbox.socket());

    let lead = json_of(&cli(
        &sandbox,
        &bin,
        &root_path,
        &["new", "--title", "Lead", "--json"],
        &[],
    ));
    let lead_id = lead["id"].as_str().unwrap_or_default().to_owned();
    let lead = running(&sandbox.socket(), &lead_id);
    let lead_cwd = PathBuf::from(lead["cwd"].as_str().unwrap_or_default());
    let identity = trace(&sandbox, &lead_id);
    let (launch_id, token) = (
        identity["launchId"].as_str().unwrap_or_default().to_owned(),
        identity["token"].as_str().unwrap_or_default().to_owned(),
    );

    let call = |id: u64, args: Value| {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": "deckhand", "arguments": {"args": args}}})
    };
    let messages = [
        serde_json::json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "codex", "version": "0"}}}),
        serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
        call(2, serde_json::json!(["guide"])),
        call(3, serde_json::json!(["note", "add", "Check the \"MCP\" path"])),
        call(4, serde_json::json!(["ls"])),
        call(5, serde_json::json!(["restart", "--yes"])),
    ];
    let mut command = sandbox.cli(&["mcp"]);
    command
        .current_dir(&lead_cwd)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("DECKHAND_SESSION_ID", &lead_id)
        .env("DECKHAND_LAUNCH_ID", &launch_id)
        .env("DECKHAND_HOOK_TOKEN", &token)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut server = sandbox.spawn(command);
    {
        let mut stdin = server.stdin.take().expect("stdin");
        for message in &messages {
            writeln!(stdin, "{message}").expect("write");
        }
    }
    // The input ends: the server answers what it read, then exits.
    let output = server.wait_with_output().expect("mcp output");
    assert!(output.status.success(), "{}", text(&output.stderr));
    let replies: Vec<Value> = text(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|error| panic!("{error}: {line}")))
        .collect();
    let ids: Vec<u64> = replies.iter().map(|reply| reply["id"].as_u64().unwrap_or(99)).collect();
    assert_eq!(ids, [0, 1, 2, 3, 4, 5], "{replies:?}");
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "deckhand");
    assert_eq!(replies[1]["result"]["tools"][0]["name"], "deckhand");
    let tool_text = |reply: &Value| {
        reply["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    for reply in &replies[2..5] {
        assert_eq!(reply["result"]["isError"], false, "{reply}");
    }
    assert!(tool_text(&replies[2]).starts_with("# deckhand: "));
    assert!(
        tool_text(&replies[4]).contains(&format!("*  {}", &lead_id[..8])),
        "{}",
        replies[4]
    );
    assert_eq!(replies[5]["result"]["isError"], true);
    assert!(!text(&output.stdout).contains(&token));

    // The step reached the lead's note, as the caller's.
    let lead = state(&sandbox.socket(), &lead_id);
    let note = lead["sharedNotes"]["text"]
        .as_str()
        .or(lead["notes"].as_str())
        .unwrap_or_default();
    assert!(note.contains("- [ ] Check the \"MCP\" path"), "{lead}");

    stop(&mut daemon);
}
