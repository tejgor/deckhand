//! The `deckhand` commands agents run (`new`, `task`, `step`, `ls`, `note`, `kill`), black-box: the CLI against a real
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

/// `deckhand <args>` as [`cli`] runs it, with `input` piped to its stdin (what a quoted heredoc gives it).
fn cli_piped(sandbox: &Sandbox, bin: &Path, cwd: &Path, args: &[&str], env: &[(&str, &str)], input: &str) -> Output {
    use std::io::Write;
    use std::process::Stdio;

    let mut command = sandbox.cli(args);
    command
        .current_dir(cwd)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }
    let mut child = sandbox.spawn(command);
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("output")
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
    assert!(added["parent"].is_null() && added["session"].is_null(), "{added}");
    let added_id = added["id"].as_str().unwrap_or_default().to_owned();
    // Its details are its line's note.
    assert_eq!(
        fs::read_to_string(sandbox.home().join("notes/lines").join(format!("{added_id}.md")))
            .ok()
            .as_deref(),
        Some("For the CLI\n")
    );
    let listed = json_of(&run(&["task", "list", "--json"]));
    assert!(
        listed.as_array().is_some_and(|tasks| tasks
            .iter()
            .any(|task| task["id"] == added["id"] && task["note"] == true)),
        "{listed}"
    );
    let board = text(&run(&["task", "list"]).stdout);
    assert!(
        board.contains(&format!("☐ {}  Write the docs", &added_id[..8])),
        "{board}"
    );
    // Ticked, reopened; moved under another line, then back to the backlog.
    let ticked = json_of(&run(&["task", "done", &added_id[..6], "--json"]));
    assert_eq!(ticked["done"], true);
    json_of(&run(&["task", "reopen", &added_id, "--json"]));
    let other = json_of(&run(&["task", "add", "Release", "--json"]));
    let other_id = other["id"].as_str().unwrap_or_default().to_owned();
    let moved = json_of(&run(&["task", "move", &added_id, "--to", &other_id, "--json"]));
    assert_eq!(moved["parent"], other["id"], "{moved}");
    let moved = json_of(&run(&["task", "move", &added_id, "--to", "backlog", "--json"]));
    assert!(moved["parent"].is_null(), "{moved}");

    let sessions = json_of(&run(&["ls", "--json"]));
    let listed = sessions
        .as_array()
        .and_then(|sessions| sessions.iter().find(|session| session["id"] == id.as_str()).cloned())
        .unwrap_or_default();
    assert_eq!(listed["title"], "Say hello", "{sessions}");
    let table = text(&run(&["ls"]).stdout);
    assert!(table.contains(&id[..8]) && table.contains("Say hello"), "{table}");

    // By hand there is no session whose line to add to.
    for args in [
        &["note", "add", "Check it"][..],
        &["step", "add", "Check it"],
        &["step", "list"],
    ] {
        let refused = run(args);
        assert!(!refused.status.success(), "{args:?}");
        assert!(
            text(&refused.stderr).contains("needs a Deckhand session"),
            "{}",
            text(&refused.stderr)
        );
    }

    stop(&mut daemon);
}

#[test]
fn a_dash_reads_long_text_from_stdin_and_nothing_piped_in_is_refused_before_anything_starts() {
    let root = repo();
    let sandbox = Sandbox::new("agent-cli-stdin");
    let root_path: PathBuf = fs::canonicalize(root.path()).expect("realpath");
    let bin = install_agents(&sandbox, fake_agent!());
    let mut daemon = launch(&sandbox, &root_path, &bin);
    ready(&sandbox.socket());
    let run = |args: &[&str]| cli(&sandbox, &bin, &root_path, args, &[]);
    let piped =
        |args: &[&str], env: &[(&str, &str)], input: &str| cli_piped(&sandbox, &bin, &root_path, args, env, input);

    // new --prompt - <<'EOF' … EOF: the brief, whole, is the agent's first message (never the literal "-").
    let brief = "Fix the login.\n\nKeep \"remember me\" and $HOME as they are.\n";
    let new = [
        "new",
        "--top-level",
        "--worktree",
        "none",
        "--title",
        "Login",
        "--prompt",
        "-",
        "--json",
    ];
    let created = json_of(&piped(&new, &[], brief));
    let id = created["id"].as_str().unwrap_or_default().to_owned();
    assert_eq!(created["title"], "Login");
    running(&sandbox.socket(), &id);
    let launched = trace(&sandbox, &id);
    assert!(
        args_hold(
            &launched,
            "Fix the login.\n\nKeep \"remember me\" and $HOME as they are."
        ),
        "{launched}"
    );
    assert!(
        !launched["args"]
            .as_array()
            .is_some_and(|args| args.iter().any(|arg| arg == "-")),
        "{launched}"
    );

    // Nothing piped in (stdin closed, or only blank lines): refused, and no session is started.
    let refused = run(&[
        "new",
        "--top-level",
        "--worktree",
        "none",
        "--title",
        "Empty",
        "--prompt",
        "-",
    ]);
    assert!(!refused.status.success());
    assert!(
        text(&refused.stderr).contains(
            "deckhand new --prompt - reads the prompt from stdin, but nothing was piped in. Pass it in a quoted \
             heredoc"
        ),
        "{}",
        text(&refused.stderr)
    );
    let refused = piped(&new, &[], "\n\n");
    assert!(!refused.status.success(), "{}", text(&refused.stdout));
    let sessions = json_of(&run(&["ls", "--json"]));
    assert_eq!(sessions.as_array().map(Vec::len), Some(1), "{sessions}");

    // task add --details - and note add -: the text as piped, without the heredoc's last newline.
    let added = json_of(&piped(
        &["task", "add", "Docs", "--details", "-", "--json"],
        &[],
        "Line one\n\nLine two\n",
    ));
    let added_id = added["id"].as_str().unwrap_or_default().to_owned();
    assert_eq!(
        fs::read_to_string(sandbox.home().join("notes/lines").join(format!("{added_id}.md")))
            .ok()
            .as_deref(),
        Some("Line one\n\nLine two\n")
    );
    let identity = trace(&sandbox, &id);
    let caller = [
        ("DECKHAND_SESSION_ID", id.as_str()),
        ("DECKHAND_LAUNCH_ID", identity["launchId"].as_str().unwrap_or_default()),
        ("DECKHAND_HOOK_TOKEN", identity["token"].as_str().unwrap_or_default()),
    ];
    let note = json_of(&piped(
        &["note", "add", "-", "--json"],
        &caller,
        "Found the cache.\nIt is per process.\n",
    ));
    assert_eq!(note["text"], "Found the cache.\nIt is per process.\n", "{note}");
    // A title is never read from stdin.
    let refused = piped(&["task", "add", "-"], &[], "Docs\n");
    assert!(
        text(&refused.stderr).contains("deckhand task add takes a title itself, not - (stdin)"),
        "{}",
        text(&refused.stderr)
    );

    stop(&mut daemon);
}

#[test]
fn inside_a_session_new_starts_a_sub_session_it_may_end_ls_lists_their_tree_and_steps_go_on_its_line() {
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
    let child = running(&sandbox.socket(), &child_id);

    // In a session, ls is the caller's tree: its top-level session and everything under it, indented.
    let table = text(&as_lead(&["ls"]).stdout);
    assert!(
        table.starts_with("Your tree (everything under Lead): you can message any of these with deckhand send."),
        "{table}"
    );
    assert!(table.contains(&format!("*  {}", &lead_id[..8])), "{table}");
    assert!(table.contains("    Write the tests"), "{table}");
    assert!(!table.contains("PARENT"), "{table}");
    // From the sub-session, in its own worktree, the tree is the same: the lead too, the caller marked.
    let child_identity = trace(&sandbox, &child_id);
    let child_env = [
        ("DECKHAND_SESSION_ID", child_id.as_str()),
        (
            "DECKHAND_LAUNCH_ID",
            child_identity["launchId"].as_str().unwrap_or_default(),
        ),
        (
            "DECKHAND_HOOK_TOKEN",
            child_identity["token"].as_str().unwrap_or_default(),
        ),
    ];
    let child_cwd = PathBuf::from(child["cwd"].as_str().unwrap_or_default());
    assert_ne!(child_cwd, lead_cwd, "the child works in its own worktree");
    let from_child = text(&cli(&sandbox, &bin, &child_cwd, &["ls"], &child_env).stdout);
    assert!(
        from_child.starts_with("Your tree (everything under Lead)"),
        "{from_child}"
    );
    assert!(from_child.contains(&format!("   {}", &lead_id[..8])), "{from_child}");
    assert!(from_child.contains(&format!("*  {}", &child_id[..8])), "{from_child}");
    let records = json_of(&cli(&sandbox, &bin, &child_cwd, &["ls", "--json"], &child_env));
    let ids: Vec<&str> = records
        .as_array()
        .map(|records| records.iter().filter_map(|record| record["id"].as_str()).collect())
        .unwrap_or_default();
    assert_eq!(ids, [lead_id.as_str(), child_id.as_str()], "{records}");
    // --all: the sessions of the checkout it runs in, as by hand, with each one's parent.
    let checkout = text(&as_lead(&["ls", "--all"]).stdout);
    assert!(
        checkout.contains("PARENT") && checkout.contains("Lead / Write the tests"),
        "{checkout}"
    );
    assert!(!checkout.contains("Your tree"), "{checkout}");

    // Lines on demand: the lead has none until its first step, which makes it (named after it), with the step under.
    let none = text(&as_lead(&["step", "list"]).stdout);
    assert!(none.contains("has no line yet"), "{none}");
    let step = json_of(&as_lead(&["step", "add", "Review the child's tests", "--json"]));
    let step_id = step["id"].as_str().unwrap_or_default().to_owned();
    let steps = json_of(&as_lead(&["step", "list", "--json"]));
    assert_eq!(steps[0]["title"], "Lead", "{steps}");
    assert_eq!(steps[0]["session"], lead_id.as_str(), "{steps}");
    assert_eq!(steps[1]["id"], step_id.as_str(), "{steps}");
    assert_eq!(steps[1]["parent"], steps[0]["id"], "{steps}");
    let ticked = json_of(&as_lead(&["step", "done", &step_id[..6], "--json"]));
    assert_eq!(ticked["done"], true);
    let listed = text(&as_lead(&["step", "list"]).stdout);
    assert!(listed.contains("Lead  1/1") && listed.contains("☑"), "{listed}");
    json_of(&as_lead(&["step", "reopen", &step_id, "--json"]));
    let refused = as_lead(&["step", "done", "zzzz"]);
    assert!(!refused.status.success());
    // A note goes to its line's note, paragraph by paragraph.
    let note = json_of(&as_lead(&["note", "add", "Found the cache", "--json"]));
    assert_eq!(note["line"], steps[0]["id"]);
    json_of(&as_lead(&["note", "add", "It is per process", "--json"]));
    let line_note = sandbox
        .home()
        .join("notes/lines")
        .join(format!("{}.md", steps[0]["id"].as_str().unwrap_or_default()));
    assert_eq!(
        fs::read_to_string(&line_note).ok().as_deref(),
        Some("Found the cache\n\nIt is per process\n")
    );
    // The session's own note is the user's: untouched.
    assert_eq!(state(&sandbox.socket(), &lead_id)["notes"], "");

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

/// A sub-session's new worktree is stacked on its parent's: it starts from the parent's branch and targets it (its
/// base, so changes and pull requests compare against the parent), unless `--flat`.
#[test]
fn a_sub_session_is_stacked_on_its_parent_s_worktree_unless_flat() {
    let root = repo();
    let sandbox = Sandbox::new("agent-cli-stacked");
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
    assert_eq!(lead["worktree"].get("targetBranch"), None, "{lead}");
    let lead_cwd = PathBuf::from(lead["cwd"].as_str().unwrap_or_default());
    let lead_branch = lead["worktree"]["branch"].as_str().unwrap_or_default().to_owned();
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

    let stacked = json_of(&as_lead(&["new", "--title", "Stacked", "--json"]));
    let stacked = running(&sandbox.socket(), stacked["id"].as_str().unwrap_or_default());
    assert_eq!(stacked["worktree"]["targetBranch"], lead_branch.as_str(), "{stacked}");
    assert_eq!(
        stacked["worktree"]["targetWorktreeId"], lead["worktree"]["id"],
        "{stacked}"
    );
    assert_eq!(stacked["worktree"]["baseRef"], lead_branch.as_str(), "{stacked}");

    let flat = json_of(&as_lead(&["new", "--title", "Flat", "--flat", "--json"]));
    let flat = running(&sandbox.socket(), flat["id"].as_str().unwrap_or_default());
    assert_eq!(flat["worktree"].get("targetBranch"), None, "{flat}");
    assert_eq!(flat["worktree"].get("targetWorktreeId"), None, "{flat}");

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
        call(6, serde_json::json!(["note", "add", "-"])),
        call(7, serde_json::json!(["note", "add", "Line one\n\nLine \"two\""])),
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
    assert_eq!(ids, [0, 1, 2, 3, 4, 5, 6, 7], "{replies:?}");
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
    // The tool has no stdin: a `-` runs nothing and says to pass the text as one word, which works.
    assert_eq!(replies[6]["result"]["isError"], true);
    assert!(
        tool_text(&replies[6]).starts_with("The deckhand tool has no stdin, so - cannot read the note."),
        "{}",
        replies[6]
    );
    assert_eq!(replies[7]["result"]["isError"], false, "{}", replies[7]);
    assert!(!text(&output.stdout).contains(&token));

    // The note reached the lead's line's note, as the caller's (the line made for it).
    let notes = sandbox.home().join("notes/lines");
    let written: Vec<String> = fs::read_dir(&notes)
        .map(|entries| {
            entries
                .filter_map(|entry| fs::read_to_string(entry.ok()?.path()).ok())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        written,
        ["Check the \"MCP\" path\n\nLine one\n\nLine \"two\"\n"],
        "{written:?}"
    );

    stop(&mut daemon);
}
