//! Forks, black-box: drives DECKHAND_TEST_BIN against a real daemon with fake agents.

use std::fs;
use std::path::PathBuf;
use std::process::Child;
use std::time::Duration;

use deckhand_e2e::client::request;
use deckhand_e2e::daemon::*;
use deckhand_e2e::fakes::{MISSING_CONVERSATION_BODY, SH_JSON_ARGS};
use deckhand_e2e::harness::{AgentOptions, Sandbox, install_agent, repo, stop};
use serde_json::{Value, json};

struct Ctx {
    sock: PathBuf,
    home: PathBuf,
    root: PathBuf,
    bin: PathBuf,
}

impl Ctx {
    fn call(&self, message: Value) -> Result<Value, String> {
        request(&self.sock, message)
    }

    fn ok(&self, message: Value) -> Value {
        self.call(message.clone()).unwrap_or_else(|e| panic!("{message}: {e}"))
    }

    fn state(&self, id: &str) -> Value {
        self.call(json!({"type": "list"}))
            .ok()
            .and_then(|list| {
                list.as_array()
                    .and_then(|items| items.iter().find(|s| s["id"] == id).cloned())
            })
            .unwrap_or(Value::Null)
    }

    fn wait(&self, id: &str, ok: impl Fn(&Value) -> bool) -> Value {
        wf(|| self.state(id), ok)
    }

    fn create(&self, title: &str, program: &str) -> Value {
        self.ok(json!({"type": "create", "input": {"title": title, "program": program, "cwd": self.root, "repoRoot": self.root, "cols": 80, "rows": 24}}))
    }

    fn fork_of(&self, parent: &Value, title: &str, worktree_mode: &str) -> Result<Value, String> {
        self.call(json!({"type": "create", "input": {"title": title, "program": parent["program"], "cwd": parent["cwd"], "repoRoot": self.root, "cols": 80, "rows": 24, "parentSessionId": parent["id"], "subSessionKind": "forked", "worktreeMode": worktree_mode}}))
    }

    fn trace(&self, id: &str) -> Option<Value> {
        serde_json::from_str(&fs::read_to_string(self.home.join(format!("trace-{id}.json"))).ok()?).ok()
    }

    fn kill_and_wait(&self, id: &str) {
        self.ok(json!({"type": "kill", "sessionId": id}));
        self.wait(id, |s| s["status"] == "exited");
    }

    fn relaunch(&self, id: &str) -> Vec<String> {
        let before = self.trace(id).map(|t| t["launchId"].clone());
        self.ok(json!({"type": "restart", "sessionId": id, "cols": 80, "rows": 24, "mode": "resume"}));
        let trace = wf(
            || self.trace(id),
            |t| t.as_ref().is_some_and(|t| Some(&t["launchId"]) != before.as_ref()),
        )
        .expect("trace");
        // Let the fake agent finish reporting its hooks (each is a slow process under the TS shim) so a late one cannot
        // land after the test's next kill or restart.
        let ready = self
            .home
            .join(format!("ready-{}", trace["launchId"].as_str().unwrap_or_default()));
        wf(|| ready.exists(), |exists| *exists);
        strings(&trace["args"])
    }
}

struct Daemon<'a> {
    sandbox: &'a Sandbox,
    ctx: &'a Ctx,
    child: Child,
}

impl<'a> Daemon<'a> {
    fn launch(sandbox: &'a Sandbox, ctx: &'a Ctx) -> Daemon<'a> {
        let child = spawn_daemon(sandbox, &ctx.root, &path_with(&ctx.bin), &[]);
        let daemon = Daemon { sandbox, ctx, child };
        daemon.ready();
        daemon
    }

    fn ready(&self) {
        wait_ping(|message| self.ctx.call(message), Duration::from_secs(15));
    }

    fn restart_with(&mut self, edit: impl FnOnce(&mut Vec<Value>)) {
        stop(&mut self.child);
        let file = self.ctx.home.join("state.json");
        let mut saved: Value =
            serde_json::from_str(&fs::read_to_string(&file).expect("state.json")).expect("state json");
        let sessions = saved["sessions"].as_array_mut().expect("sessions");
        edit(sessions);
        write_atomic(
            &file,
            format!("{}\n", serde_json::to_string_pretty(&saved).expect("json")).as_bytes(),
        );
        *self = Daemon::launch(self.sandbox, self.ctx);
    }
}

// ported from tests/forks.test.ts: forked sub-sessions with fake agents
#[test]
fn forked_sub_sessions_with_fake_agents() {
    let sandbox = Sandbox::new("forks");
    let repo_dir = repo();
    let root = repo_dir.path().to_path_buf();
    let bin = sandbox.root().join("bin");
    for provider in ["claude", "pi", "codex"] {
        install_agent(&bin, provider, deckhand_e2e::fake_agent!(), AgentOptions::default());
    }
    let ctx = Ctx {
        sock: sandbox.socket(),
        home: sandbox.home(),
        root,
        bin,
    };
    let mut daemon = Daemon::launch(&sandbox, &ctx);

    claude_forks(&ctx, &mut daemon);
    codex_forks(&ctx);
}

// ported from tests/forks.test.ts: Claude forks launch with --fork-session and their own ID, never type, re-fork when nothing was saved, and keep legacy children working
fn claude_forks(c: &Ctx, daemon: &mut Daemon) {
    let parent = c.create("fork-parent", "claude");
    let parent_id = id_of(&parent);
    let parent_ref = c.wait(&parent_id, |s| {
        s["status"] == "running" && !s["agentSessionRef"].is_null()
    })["agentSessionRef"]
        .clone();
    let parent_value = parent_ref["value"].as_str().unwrap().to_string();
    let child = c.fork_of(&parent, "child", "none").expect("fork");
    let child_id = id_of(&child);
    let started = c.wait(&child_id, |s| {
        s["status"] == "running" && s["attention"]["event"] == "SessionStart"
    });
    let child_ref = started["agentSessionRef"].clone();
    let child_value = child_ref["value"].as_str().unwrap().to_string();
    assert_eq!(child_ref["kind"], "id");
    assert_ne!(child_value, parent_value); // Assigned at launch; the hook's ID was not adopted.
    let args = strings(&c.trace(&child_id).expect("trace")["args"]);
    let expected: Vec<String> = vec![
        "--resume".into(),
        parent_value.clone(),
        "--fork-session".into(),
        "--session-id".into(),
        child_value.clone(),
        "--name".into(),
        format!("dh-fork-parent-child-{}", &child_id[..8]),
    ];
    assert_eq!(args[..7], expected[..]);
    assert!(
        !args.iter().any(|a| a == "--"),
        "a fork in the parent's worktree sends no first message"
    );
    pause(800); // Past the old /branch keystroke delay.
    assert_eq!(
        fs::read_to_string(c.home.join(format!("input-{child_id}"))).unwrap_or_default(),
        ""
    );
    c.kill_and_wait(&child_id);
    let resumed = c.relaunch(&child_id);
    assert_eq!(resumed[..2], ["--resume".to_string(), child_value.clone()]);
    assert!(!resumed.iter().any(|a| a == "--session-id") && !resumed.iter().any(|a| a == "--fork-session"));
    c.kill_and_wait(&child_id);

    // The parent has no saved conversation yet: the fork fails, says so, and s forks again with a new child ID.
    let script = format!(
        "#!/bin/sh\nif [ -n \"$DECKHAND_HOME\" ]; then\n{SH_JSON_ARGS}printf '{{\"args\":[%s],\"launchId\":\"%s\"}}' \"$json\" \"$DECKHAND_LAUNCH_ID\" > \"$DECKHAND_HOME/trace-$DECKHAND_SESSION_ID.json\"\nfi\n{MISSING_CONVERSATION_BODY}"
    );
    install_script(&c.bin.join("claude"), &script);
    let unsaved = c.fork_of(&parent, "unsaved", "none").expect("fork");
    let unsaved_id = id_of(&unsaved);
    let failed = c.wait(&unsaved_id, |s| s["status"] == "exited");
    let first_try = strings(&c.trace(&unsaved_id).expect("trace")["args"]);
    assert_eq!(failed["exitReason"], "failed");
    assert!(
        failed["lastPreview"]
            .as_str()
            .unwrap_or_default()
            .contains("Press s to fork the parent again"),
        "{failed}"
    );
    assert_eq!(failed["agentSessionRef"], parent_ref);
    reinstall(&c.bin, "claude", deckhand_e2e::fake_agent!(), AgentOptions::default());
    let retried = c.relaunch(&unsaved_id);
    assert_eq!(
        retried[..4],
        [
            "--resume".to_string(),
            parent_value.clone(),
            "--fork-session".into(),
            "--session-id".into()
        ]
    );
    assert_ne!(retried[4], first_try[4]);
    assert_ne!(retried[4], parent_value);
    assert_eq!(
        c.wait(&unsaved_id, |s| s["status"] == "running")["agentSessionRef"]["value"],
        retried[4].as_str()
    );
    c.kill_and_wait(&unsaved_id);

    // Claude forks may go into another worktree.
    let elsewhere = c.fork_of(&parent, "elsewhere", "new").expect("fork");
    let elsewhere_id = id_of(&elsewhere);
    let moved = c.wait(&elsewhere_id, |s| s["status"] == "running");
    assert_ne!(moved["cwd"], json!(c.root));
    assert_eq!(moved["worktree"]["mode"], "managed");
    let moved_args = strings(&wf(|| c.trace(&elsewhere_id), |t| t.is_some()).expect("trace")["args"]);
    assert_eq!(
        moved_args[..3],
        ["--resume".to_string(), parent_value.clone(), "--fork-session".into()]
    );
    // Its first message says where it now is, since the copied conversation's paths point at the parent's worktree.
    let marker = moved_args.iter().position(|a| a == "--");
    let note = marker
        .and_then(|at| moved_args.get(at + 1))
        .cloned()
        .unwrap_or_default();
    let cwd = moved["cwd"].as_str().unwrap_or_default();
    assert!(
        marker.is_some()
            && note.contains(&format!("You are now in {cwd}"))
            && note.contains("do not read or edit anything there"),
        "{note}"
    );
    c.kill_and_wait(&elsewhere_id);
    c.kill_and_wait(&parent_id);

    // Legacy children: a /branch name resumes by name; one still holding the parent's ref forks again. A legacy
    // name-ref parent forks by name.
    daemon.restart_with(|sessions| {
        for item in sessions.iter_mut() {
            let id = id_of(item);
            if id == child_id {
                item["agentSessionRef"] = json!({"provider": "claude", "kind": "name", "value": "dh-legacy-branch"});
            } else if id == unsaved_id {
                item["agentSessionRef"] = parent_ref.clone();
            } else if id == parent_id {
                item["agentSessionRef"] = json!({"provider": "claude", "kind": "name", "value": "dh-legacy-parent"});
            }
        }
    });
    assert_eq!(
        c.relaunch(&child_id)[..2],
        ["--resume".to_string(), "dh-legacy-branch".into()]
    );
    let reforked = c.relaunch(&unsaved_id);
    assert_eq!(
        reforked[..4],
        [
            "--resume".to_string(),
            parent_value.clone(),
            "--fork-session".into(),
            "--session-id".into()
        ]
    );
    assert_ne!(reforked[4], parent_value);
    let from_name = c.fork_of(&c.state(&parent_id), "from-name", "none").expect("fork");
    let from_name_id = id_of(&from_name);
    let from_name_ref = c.wait(&from_name_id, |s| s["status"] == "running")["agentSessionRef"].clone();
    let from_name_args = strings(&wf(|| c.trace(&from_name_id), |t| t.is_some()).expect("trace")["args"]);
    assert_eq!(
        from_name_args[..5],
        [
            "--resume".to_string(),
            "dh-legacy-parent".into(),
            "--fork-session".into(),
            "--session-id".into(),
            from_name_ref["value"].as_str().unwrap().to_string()
        ]
    );
    c.kill_and_wait(&child_id);
    c.kill_and_wait(&unsaved_id);
    c.kill_and_wait(&from_name_id);
}

// ported from tests/forks.test.ts: Codex forks run codex fork <parent id>, capture their own ID without adopting the parent's, and stay in the parent's worktree
fn codex_forks(c: &Ctx) {
    reinstall(&c.bin, "codex", deckhand_e2e::fake_agent!(), AgentOptions::default());
    let parent = c.create("codex-parent", "codex");
    let parent_id = id_of(&parent);
    let parent_native = c.wait(&parent_id, |s| {
        s["status"] == "running" && !s["agentSessionRef"].is_null()
    })["agentSessionRef"]["value"]
        .as_str()
        .unwrap()
        .to_string();
    let child = c.fork_of(&parent, "child", "none").expect("fork");
    let child_id = id_of(&child);
    let launched = wf(|| c.trace(&child_id), |t| t.is_some()).expect("trace");
    assert_eq!(
        strings(&launched["args"])[..2],
        ["fork".to_string(), parent_native.clone()]
    );
    let ready = c
        .home
        .join(format!("ready-{}", launched["launchId"].as_str().unwrap_or_default()));
    wf(|| ready.exists(), |exists| *exists); // All three SessionStart hooks sent.
    assert_eq!(c.state(&child_id)["agentSessionRef"]["value"], "fixture-child-codex");
    c.kill_and_wait(&child_id);
    assert_eq!(
        c.relaunch(&child_id)[..2],
        ["resume".to_string(), "fixture-child-codex".into()]
    );
    c.kill_and_wait(&child_id);

    // Without a reported ID the fork cannot be reopened: the exit says so and s forks the parent again.
    reinstall(
        &c.bin,
        "codex",
        deckhand_e2e::fake_agent!(),
        AgentOptions {
            without_hooks: true,
            ..AgentOptions::default()
        },
    );
    let silent = c.fork_of(&parent, "silent", "none").expect("fork");
    let silent_id = id_of(&silent);
    c.wait(&silent_id, |s| s["status"] == "running");
    c.kill_and_wait(&silent_id);
    let after = c.state(&silent_id);
    assert!(after["agentSessionRef"].is_null());
    assert!(
        after["lastPreview"]
            .as_str()
            .unwrap_or_default()
            .contains("did not report this fork's conversation ID"),
        "{after}"
    );
    assert_eq!(c.relaunch(&silent_id)[..2], ["fork".to_string(), parent_native.clone()]);
    c.kill_and_wait(&silent_id);

    // Codex may reopen a fork in the parent's directory, so other worktrees are refused before anything launches.
    assert_rejects(
        c.fork_of(&parent, "elsewhere", "new"),
        &["Codex forks stay in the parent's worktree"],
    );
    assert_rejects(
        c.call(json!({"type": "create", "input": {"title": "elsewhere", "program": "codex", "cwd": c.home, "repoRoot": c.root, "cols": 80, "rows": 24, "parentSessionId": parent_id, "subSessionKind": "forked"}})),
        &["stay in the parent's worktree"],
    );
    // A parent whose ID was never captured cannot be forked.
    let unknown = c.create("codex-no-id", "codex");
    let unknown_id = id_of(&unknown);
    c.wait(&unknown_id, |s| s["status"] == "running");
    assert_rejects(
        c.fork_of(&unknown, "child", "none"),
        &["has not reported the parent's conversation ID"],
    );
    c.kill_and_wait(&unknown_id);
    c.kill_and_wait(&parent_id);
}
