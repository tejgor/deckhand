//! Messages between sessions (`deckhand send`, `deckhand reply`), black-box: the CLI against a real daemon with fake
//! agents, by hand and from inside sessions (their `DECKHAND_SESSION_ID`, `DECKHAND_LAUNCH_ID` and
//! `DECKHAND_HOOK_TOKEN`, as the fake agent saw them). What a message types into an agent is read back from the fake
//! agent's input log; turn signals are sent as its hooks would send them.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use deckhand_e2e::daemon::{install_agents, launch, must, ready, running, state, text_of, wf};
use deckhand_e2e::fake_agent;
use deckhand_e2e::harness::{Sandbox, repo, stop};
use serde_json::{Value, json};

/// `deckhand <args>` in `cwd`, with only the fakes and system tools on PATH and `env` added.
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

fn json_of(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("{error}: {}", text(&output.stdout)))
}

/// A session started by hand (`--parent` when given), running and idle at its input; its ID.
fn started(sandbox: &Sandbox, bin: &Path, root: &Path, title: &str, parent: Option<&str>) -> String {
    let mut args = vec!["new", "--title", title, "--worktree", "none", "--json"];
    match parent {
        Some(parent) => args.extend(["--parent", parent]),
        None => args.push("--top-level"),
    }
    let created = json_of(&cli(sandbox, bin, root, &args, &[]));
    let id = created["id"].as_str().unwrap_or_default().to_owned();
    running(&sandbox.socket(), &id);
    wf(
        || state(&sandbox.socket(), &id),
        |session| session["agentStatus"] == "idle",
    );
    id
}

/// The identity session `id`'s agent was launched with: its `deckhand` commands act for it.
struct Identity {
    id: String,
    launch_id: String,
    token: String,
}

impl Identity {
    fn of(sandbox: &Sandbox, id: &str) -> Identity {
        let path = sandbox.home().join(format!("trace-{id}.json"));
        let trace = wf(
            || {
                fs::read_to_string(&path)
                    .ok()
                    .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            },
            Option::is_some,
        )
        .unwrap_or_default();
        Identity {
            id: id.to_owned(),
            launch_id: trace["launchId"].as_str().unwrap_or_default().to_owned(),
            token: trace["token"].as_str().unwrap_or_default().to_owned(),
        }
    }

    fn env(&self) -> [(&str, &str); 3] {
        [
            ("DECKHAND_SESSION_ID", self.id.as_str()),
            ("DECKHAND_LAUNCH_ID", self.launch_id.as_str()),
            ("DECKHAND_HOOK_TOKEN", self.token.as_str()),
        ]
    }

    /// A lifecycle hook from this session's agent (as `deckhand hook` relays it).
    fn hook(&self, socket: &Path, event: &str) {
        must(
            socket,
            json!({"type": "agent-hook", "sessionId": self.id, "launchId": self.launch_id, "token": self.token,
                "payload": {"hook_event_name": event}}),
        );
    }
}

/// What has been typed into session `id`'s agent so far.
fn typed(sandbox: &Sandbox, id: &str) -> String {
    text_of(&sandbox.home().join(format!("input-{id}"))).unwrap_or_default()
}

/// Waits until `needle` has been typed into session `id`'s agent; everything typed by then.
fn typed_containing(sandbox: &Sandbox, id: &str, needle: &str) -> String {
    wf(|| typed(sandbox, id), |input| input.contains(needle))
}

/// The ID in a message's header (`[Message <id> ...`) at the end of `input`.
fn message_id(input: &str) -> String {
    input
        .rsplit("[Message ")
        .next()
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_default()
        .trim_end_matches(',')
        .to_owned()
}

#[test]
fn by_hand_a_message_is_typed_into_an_idle_session_under_its_header_and_submitted() {
    let root = repo();
    let sandbox = Sandbox::new("messages-hand");
    let root_path: PathBuf = fs::canonicalize(root.path()).expect("realpath");
    let bin = install_agents(&sandbox, fake_agent!());
    let mut daemon = launch(&sandbox, &root_path, &bin);
    ready(&sandbox.socket());
    let id = started(&sandbox, &bin, &root_path, "Token storage", None);

    let sent = json_of(&cli(
        &sandbox,
        &bin,
        &root_path,
        &["send", &id[..8], "Rebase on main,\nthen run the tests.", "--json"],
        &[],
    ));
    let message = sent["message"]["id"].as_str().unwrap_or_default().to_owned();
    assert_eq!(message.len(), 6, "{sent}");
    let expected = format!(
        "\x1b[200~[Message {message} from you (the user) via deckhand]\nRebase on main,\nthen run the tests.\x1b[201~\r"
    );
    let input = typed_containing(&sandbox, &id, "\x1b[201~\r");
    assert!(input.ends_with(&expected), "{input:?}");
    // The kept record says so.
    let listed = must(&sandbox.socket(), json!({"type": "list-messages", "sessionId": id}));
    assert_eq!(listed[0]["status"], "delivered", "{listed}");
    assert!(
        fs::read_to_string(sandbox.home().join("messages.json"))
            .unwrap_or_default()
            .contains(&message)
    );

    stop(&mut daemon);
}

#[test]
fn a_session_asks_its_sibling_and_waits_for_the_reply_and_later_replies_are_typed_into_it() {
    let root = repo();
    let sandbox = Sandbox::new("messages-wait");
    let root_path: PathBuf = fs::canonicalize(root.path()).expect("realpath");
    let bin = install_agents(&sandbox, fake_agent!());
    let mut daemon = launch(&sandbox, &root_path, &bin);
    ready(&sandbox.socket());
    let socket = sandbox.socket();
    let lead = started(&sandbox, &bin, &root_path, "Auth rework", None);
    let login = started(&sandbox, &bin, &root_path, "Login UI", Some(&lead));
    let tokens = started(&sandbox, &bin, &root_path, "Token storage", Some(&lead));
    let other = started(&sandbox, &bin, &root_path, "Other idea", None);
    let (a, b) = (Identity::of(&sandbox, &login), Identity::of(&sandbox, &tokens));
    let as_a = |args: &[&str]| cli(&sandbox, &bin, &root_path, args, &a.env());
    let as_b = |args: &[&str]| cli(&sandbox, &bin, &root_path, args, &b.env());

    // Outside its tree: refused, nothing typed.
    let refused = as_a(&["send", &other, "Hello?"]);
    assert!(!refused.status.success());
    assert!(
        text(&refused.stderr).contains("only to the sessions in its tree"),
        "{}",
        text(&refused.stderr)
    );
    assert_eq!(typed(&sandbox, &other), "");

    // A asks B and waits; B's agent gets the question, starts a turn and answers with deckhand reply.
    let (asked, answered) = std::thread::scope(|scope| {
        let asking = scope.spawn(|| as_a(&["send", &tokens, "Which token store?", "--wait", "--timeout", "40s"]));
        let input = typed_containing(&sandbox, &tokens, "which wants an answer.");
        let id = message_id(&input);
        assert!(
            input.contains(&format!(
                "[Message {id} from \"Login UI\", which wants an answer.\n Reply once you have it: deckhand reply {id} \
                 \"<answer>\"]\nWhich token store?"
            )),
            "{input:?}"
        );
        b.hook(&socket, "UserPromptSubmit");
        let answered = as_b(&["reply", &id[..4], "The", "keychain"]);
        b.hook(&socket, "Stop");
        (asking.join().expect("send --wait"), answered)
    });
    assert!(answered.status.success(), "{}", text(&answered.stderr));
    assert!(
        text(&answered.stdout).contains("Login UI\" was waiting for it"),
        "{}",
        text(&answered.stdout)
    );
    assert!(asked.status.success(), "{}{}", text(&asked.stdout), text(&asked.stderr));
    assert!(
        text(&asked.stdout).contains("[Reply from \"Token storage\" to your message")
            && text(&asked.stdout).trim_end().ends_with("\nThe keychain"),
        "{}",
        text(&asked.stdout)
    );
    // Handed to the waiting command, never typed into A.
    std::thread::sleep(Duration::from_millis(500));
    assert!(!typed(&sandbox, &login).contains("The keychain"));

    // Without --wait, B's answer is queued to A and typed in once A is idle.
    let sent = json_of(&as_a(&["send", &tokens, "Tell me when the store is done", "--json"]));
    let id = sent["message"]["id"].as_str().unwrap_or_default().to_owned();
    typed_containing(
        &sandbox,
        &tokens,
        &format!("[Message {id} from \"Login UI\" via deckhand]"),
    );
    b.hook(&socket, "UserPromptSubmit");
    let queued = json_of(&as_b(&["reply", &id, "Done: see the keychain module", "--json"]));
    assert_ne!(queued["status"], "returned", "{queued}");
    let input = typed_containing(&sandbox, &login, "Done: see the keychain module\x1b[201~\r");
    assert!(
        input.ends_with(&format!(
            "\x1b[200~[Reply from \"Token storage\" to your message {id}]\nDone: see the keychain module\x1b[201~\r"
        )),
        "{input:?}"
    );
    // Only the session a message went to may answer it.
    let refused = as_a(&["reply", &id, "Me?"]);
    assert!(!refused.status.success());
    assert!(text(&refused.stderr).contains("was not sent to this session"));

    stop(&mut daemon);
}

#[test]
fn a_message_waits_while_its_target_works_then_is_delivered() {
    let root = repo();
    let sandbox = Sandbox::new("messages-busy");
    let root_path: PathBuf = fs::canonicalize(root.path()).expect("realpath");
    let bin = install_agents(&sandbox, fake_agent!());
    let mut daemon = launch(&sandbox, &root_path, &bin);
    ready(&sandbox.socket());
    let socket = sandbox.socket();
    let lead = started(&sandbox, &bin, &root_path, "Auth rework", None);
    let tokens = started(&sandbox, &bin, &root_path, "Token storage", Some(&lead));
    let (lead_id, b) = (Identity::of(&sandbox, &lead), Identity::of(&sandbox, &tokens));

    b.hook(&socket, "UserPromptSubmit");
    let sent = json_of(&cli(
        &sandbox,
        &bin,
        &root_path,
        &["send", &tokens, "When you are done: rebase", "--json"],
        &lead_id.env(),
    ));
    assert_eq!(sent["message"]["status"], "held", "{sent}");
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(typed(&sandbox, &tokens), "");
    let listed = must(&socket, json!({"type": "list-messages", "sessionId": tokens}));
    assert_eq!(listed[0]["status"], "held", "{listed}");
    // Its turn ends: the message goes in.
    b.hook(&socket, "Stop");
    let input = typed_containing(&sandbox, &tokens, "When you are done: rebase\x1b[201~\r");
    assert!(input.starts_with("\x1b[200~[Message "), "{input:?}");
    assert!(input.ends_with("\x1b[201~\r"), "{input:?}");

    stop(&mut daemon);
}
