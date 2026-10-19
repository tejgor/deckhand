//! Port of tests/daemonFeatures.test.ts (black-box: drives DECKHAND_TEST_BIN against a real daemon with fake agents).
//!
//! Like the TS file this is one scenario: the subtests share a repository, a daemon and its state and run in order.
//! The TS-only helpers (`loadState`, `updateAppConfig`, `workspaceKey`, `applyChange`, `linkSelection`, ...) are small
//! local reimplementations over the JSON files and the wire records; this crate never links deckhand code.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use deckhand_e2e::client::request;
use deckhand_e2e::harness::{AgentOptions, Sandbox, git, install_agent, is_alive, repo, sidecar, stop, wait_for};
use serde_json::{Value, json};

const FAKE_AGENT: &str = env!("CARGO_BIN_EXE_fake_agent");

fn wf<T: std::fmt::Debug>(read: impl FnMut() -> T, ok: impl Fn(&T) -> bool) -> T {
    wait_for(read, ok, Duration::from_secs(15))
}

fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn id_of(value: &Value) -> String {
    value["id"].as_str().unwrap_or_default().to_string()
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

/// `assert.rejects(promise, /text/)`: the daemon's error must contain every needle, in order (`a.*b` is `[a, b]`).
fn assert_rejects(result: Result<Value, String>, needles: &[&str]) {
    let error = match result {
        Err(error) => error,
        Ok(value) => panic!("expected a rejection containing {needles:?}, got {value}"),
    };
    let mut rest = error.as_str();
    for needle in needles {
        match rest.find(needle) {
            Some(at) => rest = &rest[at + needle.len()..],
            None => panic!("error {error:?} does not match {needles:?}"),
        }
    }
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|v| v.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn write_atomic(path: &Path, bytes: &[u8]) {
    let temporary = path.with_extension("tmp-write");
    fs::write(&temporary, bytes).expect("write temp");
    fs::rename(&temporary, path).expect("rename");
}

fn write_json(path: &Path, value: &Value) {
    write_atomic(
        path,
        format!("{}\n", serde_json::to_string_pretty(value).expect("json")).as_bytes(),
    );
}

/// Writes an executable script as a new file (rewriting a signed binary in place can get it killed on macOS).
fn install_script(path: &Path, script: &str) {
    let _ = fs::remove_file(path);
    fs::write(path, script).expect("write script");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod");
}

/// Reinstalls the fake agent, dropping the sidecars of the variant it replaces.
fn reinstall(bin: &Path, provider: &str, options: AgentOptions) {
    let target = bin.join(provider);
    for suffix in ["nohooks", "update-fails"] {
        let _ = fs::remove_file(sidecar(&target, suffix));
    }
    let _ = fs::remove_file(&target);
    install_agent(bin, provider, Path::new(FAKE_AGENT), options);
}

fn exists(path: impl AsRef<Path>) -> bool {
    fs::symlink_metadata(path.as_ref()).is_ok()
}

fn digits_after(content: &str, prefix: &str) -> Option<i32> {
    content.match_indices(prefix).find_map(|(at, _)| {
        let digits: String = content[at + prefix.len()..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        digits.parse().ok()
    })
}

/// `new RegExp(`${marker}=(\d+):(\S+)`)` over a terminal's content.
fn marker_result(content: &str, marker: &str) -> Option<(i32, String)> {
    let prefix = format!("{marker}=");
    content.match_indices(&prefix).find_map(|(at, _)| {
        let rest = &content[at + prefix.len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let pid = digits.parse().ok()?;
        let cwd: String = rest[digits.len()..]
            .strip_prefix(':')?
            .chars()
            .take_while(|c| !c.is_whitespace())
            .collect();
        Some((pid, cwd))
    })
}

/// `Date.prototype.toISOString` for now plus `minutes`.
fn iso(minutes: i64) -> String {
    let ms = now_ms() + minutes * 60_000;
    let days = ms.div_euclid(86_400_000);
    let rest = ms.rem_euclid(86_400_000);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3_600_000,
        rest / 60_000 % 60,
        rest / 1000 % 60,
        rest % 1000
    )
}

fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98,
        0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
        0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8,
        0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
        0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
        0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in message.chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [
                t1.wrapping_add(t2),
                v[0],
                v[1],
                v[2],
                v[3].wrapping_add(t1),
                v[4],
                v[5],
                v[6],
            ];
        }
        for i in 0..8 {
            h[i] = h[i].wrapping_add(v[i]);
        }
    }
    h.iter().map(|word| format!("{word:08x}")).collect()
}

/// src/workspace.ts `workspaceKey`.
fn workspace_key(session: &Value) -> Option<String> {
    let worktree = &session["worktree"];
    if !worktree["deletedAt"].is_null() {
        return None;
    }
    if !worktree.is_null() && worktree["mode"] != "none" {
        return worktree["path"].as_str().filter(|p| !p.is_empty()).map(str::to_string);
    }
    if !session["requestedWorktreeMode"].is_null() && session["requestedWorktreeMode"] != "none" {
        return None;
    }
    session["launchWorktreeRoot"]
        .as_str()
        .or(session["cwd"].as_str())
        .map(str::to_string)
}

/// src/workspace.ts `workspacePaneUnavailable`.
fn workspace_pane_unavailable(session: &Value, record: &Value) -> Option<&'static str> {
    if workspace_key(session).is_none() || (record["sessionId"] == session["id"] && record["workspace"].is_null()) {
        return Some(if session["worktree"]["deletedAt"].is_null() {
            "its worktree is not ready yet"
        } else {
            "its worktree was deleted"
        });
    }
    None
}

/// src/workspace.ts `workspaceWorkerId`.
fn workspace_worker_id(workspace: &str) -> String {
    format!("workspace-{}", &sha256_hex(workspace.as_bytes())[..16])
}

/// A persistent connection that keeps every message it receives (src/client.ts `attachJsonParser`) and can send
/// without a request id (`writeMessage`).
struct Raw {
    stream: UnixStream,
    messages: Arc<Mutex<Vec<Value>>>,
}

impl Raw {
    fn connect(socket: &Path) -> Raw {
        let stream = UnixStream::connect(socket).expect("connect");
        let reader = BufReader::new(stream.try_clone().expect("clone"));
        let messages: Arc<Mutex<Vec<Value>>> = Arc::default();
        let sink = Arc::clone(&messages);
        std::thread::spawn(move || {
            for line in reader.split(b'\n') {
                let Ok(line) = line else { break };
                if let Ok(message) = serde_json::from_slice::<Value>(&line) {
                    sink.lock().unwrap_or_else(|e| e.into_inner()).push(message);
                }
            }
        });
        Raw { stream, messages }
    }

    fn send(&self, message: Value) {
        let mut line = message.to_string();
        line.push('\n');
        (&self.stream).write_all(line.as_bytes()).expect("send");
    }

    fn events(&self) -> Vec<Value> {
        self.messages.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn clear(&self) {
        self.messages.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    fn has(&self, matches: impl Fn(&Value) -> bool) -> bool {
        self.events().iter().any(matches)
    }

    fn find(&self, matches: impl Fn(&Value) -> bool) -> Option<Value> {
        self.events().into_iter().find(matches)
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

struct Ctx {
    sock: PathBuf,
    /// `DECKHAND_HOME`.
    home: PathBuf,
    /// `HOME` (what `{home}` in a worktree location expands to).
    user_home: PathBuf,
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

    fn list(&self) -> Vec<Value> {
        self.call(json!({"type": "list"}))
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
    }

    fn state(&self, id: &str) -> Value {
        self.list().into_iter().find(|s| s["id"] == id).unwrap_or(Value::Null)
    }

    fn wait(&self, id: &str, ok: impl Fn(&Value) -> bool) -> Value {
        wf(|| self.state(id), ok)
    }

    fn create_mode(&self, title: &str, program: &str, worktree_mode: &str) -> Value {
        self.ok(json!({"type": "create", "input": {"title": title, "program": program, "cwd": self.root, "repoRoot": self.root, "cols": 80, "rows": 24, "worktreeMode": worktree_mode}}))
    }

    fn create(&self, title: &str, program: &str) -> Value {
        self.create_mode(title, program, "none")
    }

    /// A session attached to an existing worktree.
    fn attach(&self, title: &str, program: &str, repo_root: &Path, path: &str) -> Value {
        self.ok(json!({"type": "create", "input": {"title": title, "program": program, "cwd": repo_root, "repoRoot": repo_root, "cols": 80, "rows": 24, "worktreeMode": "existing", "existingWorktreePath": path}}))
    }

    fn main_worktree_path(&self) -> String {
        let worktrees = self.ok(json!({"type": "list-worktrees", "cwd": self.root}));
        let main = worktrees
            .as_array()
            .and_then(|l| l.iter().find(|w| w["isMain"] == true))
            .expect("main worktree");
        text(&main["path"]).to_string()
    }

    fn trace(&self, id: &str) -> Option<Value> {
        serde_json::from_str(&fs::read_to_string(self.home.join(format!("trace-{id}.json"))).ok()?).ok()
    }

    fn kill_and_wait(&self, id: &str) {
        self.ok(json!({"type": "kill", "sessionId": id}));
        self.wait(id, |s| s["status"] == "exited");
    }

    fn relaunch(&self, id: &str, mode: &str) -> Vec<String> {
        let before = self.trace(id).map(|t| t["launchId"].clone());
        self.ok(json!({"type": "restart", "sessionId": id, "cols": 80, "rows": 24, "mode": mode}));
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

    /// src/storage.ts `loadState`, as the raw file.
    fn load_state(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.home.join("state.json")).expect("state.json"))
            .expect("state json")
    }

    fn load_config(&self) -> Value {
        fs::read_to_string(self.home.join("config.json"))
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_else(|| json!({}))
    }

    /// `updateAppConfig(current => ({...current, defaults}))`; `None` removes the key.
    fn set_defaults(&self, defaults: Option<Value>) {
        let mut config = self.load_config();
        match defaults {
            Some(value) => config["defaults"] = value,
            None => {
                if let Some(map) = config.as_object_mut() {
                    map.shift_remove("defaults");
                }
            }
        }
        write_json(&self.home.join("config.json"), &config);
    }

    fn hook(&self, id: &str, launch: &Value, payload: Value, sent_at: i64) -> Result<Value, String> {
        self.call(json!({"type": "agent-hook", "sessionId": id, "launchId": launch["launchId"], "token": launch["token"], "payload": payload, "sentAt": sent_at}))
    }

    fn watch_action(&self, id: &str) -> Value {
        self.ok(json!({"type": "watch-action", "sessionId": id, "cols": 80, "rows": 24}))
    }

    fn git(&self, args: &[&str]) -> String {
        git(&self.root, args)
    }
}

struct Daemon<'a> {
    sandbox: &'a Sandbox,
    ctx: &'a Ctx,
    child: Child,
}

impl<'a> Daemon<'a> {
    fn launch(sandbox: &'a Sandbox, ctx: &'a Ctx) -> Daemon<'a> {
        let path = format!("{}:{}", ctx.bin.display(), std::env::var("PATH").unwrap_or_default());
        let mut command = sandbox.cli(&["--daemon"]);
        command
            .current_dir(&ctx.root)
            .env("PATH", path)
            .env("CLAUDE_CONFIG_DIR", ctx.home.join(".claude"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let daemon = Daemon {
            sandbox,
            ctx,
            child: sandbox.spawn(command),
        };
        daemon.ready();
        daemon
    }

    fn ready(&self) {
        wf(
            || {
                self.ctx
                    .call(json!({"type": "ping"}))
                    .map(|v| v["ok"] == true)
                    .unwrap_or(false)
            },
            |ok| *ok,
        );
    }

    fn kill9(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        *self = Daemon::launch(self.sandbox, self.ctx);
    }

    fn restart_with(&mut self, edit: impl FnOnce(&mut Vec<Value>)) {
        stop(&mut self.child);
        let mut saved = self.ctx.load_state();
        edit(saved["sessions"].as_array_mut().expect("sessions"));
        write_json(&self.ctx.home.join("state.json"), &saved);
        *self = Daemon::launch(self.sandbox, self.ctx);
    }
}

// ported from tests/daemonFeatures.test.ts: daemon features operate in isolated state with fake agents
#[test]
fn daemon_features_operate_in_isolated_state_with_fake_agents() {
    let sandbox = Sandbox::new("daemon-features");
    let repo_dir = repo();
    let bin = sandbox.root().join("bin");
    for provider in ["claude", "pi", "codex"] {
        install_agent(&bin, provider, Path::new(FAKE_AGENT), AgentOptions::default());
    }
    let ctx = Ctx {
        sock: sandbox.socket(),
        home: sandbox.home(),
        user_home: sandbox.user_home(),
        root: repo_dir.path().to_path_buf(),
        bin,
    };
    let mut daemon = Daemon::launch(&sandbox, &ctx);

    agent_signals_say_why_a_session_waits(&ctx);
    hook_bridge_records_the_codex_identity(&ctx, &mut daemon);
    untrusted_repository_commands_never_run(&ctx);
    handoffs_are_initial_context_only(&ctx);
    unknown_codex_identity_never_silently_becomes_a_new_conversation(&ctx);
    cancelling_setup_kills_its_process(&ctx);
    the_main_checkout_can_never_be_deleted(&ctx);
    global_worktree_settings_and_create_pr(&ctx);
    settings_reads_sources_and_saves_single_keys(&ctx);
    claude_and_pi_launch_with_exact_native_ids(&ctx, &mut daemon);
    sessions_in_one_worktree_share_one_dev(&ctx);
    sessions_in_one_worktree_share_one_terminal_and_lazygit(&ctx);
    the_git_tabs_changes_are_per_worktree(&ctx);
    merge_and_deletion_markers_belong_to_the_worktree(&ctx);
}

// ported from tests/daemonFeatures.test.ts: agent signals say why a session waits: broadcast, never persisted, not replaced by older or generic signals; a settled screen ends a stale working
fn agent_signals_say_why_a_session_waits(c: &Ctx) {
    let session = c.create("reasons", "claude");
    let id = id_of(&session);
    c.wait(&id, |s| {
        s["status"] == "running" && s["attention"]["event"] == "SessionStart"
    });
    let launch = c.trace(&id).expect("trace");
    let hook = |payload: Value, sent_at: i64| c.hook(&id, &launch, payload, sent_at).expect("hook");
    let at = now_ms() + 60_000; // later than the fake agent's own SessionStart hook
    // The fake agent printed `ready` and goes idle 5 s later: a working signal left over then becomes unknown.
    hook(json!({"hook_event_name": "UserPromptSubmit"}), at);
    assert_eq!(c.state(&id)["attention"]["state"], "working");
    c.wait(&id, |s| {
        s["attention"]["event"] == "ScreenIdle" && s["attention"]["state"] == "unknown"
    });
    hook(
        json!({"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion", "tool_input": {"questions": [{"question": "Which DB?", "options": [{"label": "Postgres"}, {"label": "SQLite"}]}]}}),
        at + 1,
    );
    let asking = c.state(&id);
    assert_eq!(asking["attention"]["state"], "needs-input");
    assert_eq!(
        asking["attention"]["reason"],
        json!({"kind": "question", "text": "asks: Which DB?", "options": ["Postgres", "SQLite"]})
    );
    // state.json keeps the state, never the reason.
    let state = c.load_state();
    let stored = state["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id.as_str())
        .unwrap()
        .clone();
    assert_eq!(stored["attention"]["state"], "needs-input");
    assert!(stored["attention"]["reason"].is_null());
    // An async hook that started earlier arrives late: dropped.
    hook(json!({"hook_event_name": "PostToolUse", "tool_name": "Bash"}), at);
    assert_eq!(c.state(&id)["attention"]["state"], "needs-input");
    // The agent's generic notifications do not replace the question, and the idle prompt never turns needs-input into
    // response-ended.
    hook(
        json!({"hook_event_name": "Notification", "notification_type": "permission_prompt", "message": "Claude needs your attention"}),
        at + 2,
    );
    hook(
        json!({"hook_event_name": "Notification", "notification_type": "idle_prompt", "message": "Claude is waiting for your input"}),
        at + 2,
    );
    let waiting = c.state(&id);
    assert_eq!(waiting["attention"]["state"], "needs-input");
    assert_eq!(waiting["attention"]["reason"]["text"], "asks: Which DB?");
    // A native subagent's tool use shows the prompt was answered; it never overrides the root's other states.
    hook(json!({"hook_event_name": "PostToolUse", "agent_id": "sub"}), at + 3);
    let answered = c.state(&id);
    assert_eq!(answered["attention"]["state"], "working");
    assert!(answered["attention"]["reason"].is_null());
    hook(
        json!({"hook_event_name": "Stop", "last_assistant_message": "Done.\n\nShall I push?"}),
        at + 4,
    );
    assert_eq!(
        c.state(&id)["attention"]["reason"],
        json!({"kind": "message", "text": "said: Shall I push?"})
    );
    hook(json!({"hook_event_name": "PostToolUse", "agent_id": "sub"}), at + 5);
    assert_eq!(c.state(&id)["attention"]["state"], "response-ended");
    // The idle prompt after a reply keeps it ended (and its reason), never needs-input.
    hook(
        json!({"hook_event_name": "Notification", "notification_type": "idle_prompt", "message": "Claude is waiting for your input"}),
        at + 6,
    );
    let idle = c.state(&id);
    assert_eq!(idle["attention"]["state"], "response-ended");
    assert_eq!(idle["attention"]["reason"]["text"], "said: Shall I push?");
    // Answering a permission prompt (Enter typed into the agent) means it works again, before the allowed tool ends.
    // Arrow keys only move through the menu.
    hook(
        json!({"hook_event_name": "PermissionRequest", "tool_name": "Bash", "tool_input": {"command": "npm test"}}),
        at + 7,
    );
    let socket = Raw::connect(&c.sock);
    socket.send(json!({"type": "input", "sessionId": id, "data": "\x1b[B"}));
    sleep_ms(300);
    assert_eq!(c.state(&id)["attention"]["state"], "needs-input");
    socket.send(json!({"type": "input", "sessionId": id, "data": "\r"}));
    let answered2 = c.wait(&id, |s| s["attention"]["event"] == "Answered");
    assert_eq!(answered2["attention"]["state"], "working");
    assert!(answered2["attention"]["reason"].is_null());
    drop(socket);
    c.kill_and_wait(&id);
}

// ported from tests/daemonFeatures.test.ts: hook bridge records the Codex identity, callbacks are authenticated, and a crashed daemon resumes the exact conversation
fn hook_bridge_records_the_codex_identity(c: &Ctx, daemon: &mut Daemon) {
    let session = c.create("context", "codex");
    let id = id_of(&session);
    c.wait(&id, |s| {
        s["status"] == "running" && s["agentSessionRef"]["value"] == "fixture-native-codex"
    });
    c.ok(json!({"type": "archive-session", "sessionId": id, "archived": true}));
    let first = c.trace(&id).expect("trace");
    let stop_payload = || json!({"hook_event_name": "Stop"});
    assert_rejects(
        c.call(json!({"type": "agent-hook", "sessionId": id, "launchId": first["launchId"], "token": "wrong", "payload": stop_payload()})),
        &["unauthorized"],
    );
    c.ok(json!({"type": "agent-hook", "sessionId": id, "launchId": first["launchId"], "token": first["token"], "payload": stop_payload()}));
    assert_eq!(c.state(&id)["attention"]["state"], "response-ended");
    daemon.kill9();
    wf(|| c.state(&id), |s| s["exitReason"] == "interrupted");
    assert!(!c.state(&id)["archivedAt"].is_null());
    c.ok(json!({"type": "restart", "sessionId": id, "cols": 80, "rows": 24}));
    let resumed = wf(
        || c.trace(&id),
        |t| t.as_ref().is_some_and(|t| t["launchId"] != first["launchId"]),
    )
    .expect("trace");
    assert_eq!(
        strings(&resumed["args"])[..2],
        ["resume".to_string(), "fixture-native-codex".into()]
    );
    assert!(c.state(&id)["archivedAt"].is_null());
    assert_rejects(
        c.call(json!({"type": "agent-hook", "sessionId": id, "launchId": first["launchId"], "token": first["token"], "payload": stop_payload()})),
        &["Stale"],
    );
    // Kill then restart inside one daemon: the old worker's exit must not mark the new launch exited.
    c.kill_and_wait(&id);
    c.ok(json!({"type": "restart", "sessionId": id, "cols": 80, "rows": 24}));
    let relaunched = wf(
        || c.trace(&id),
        |t| t.as_ref().is_some_and(|t| t["launchId"] != resumed["launchId"]),
    )
    .expect("trace");
    let running = c.wait(&id, |s| s["status"] == "running");
    assert_eq!(running["launchId"], relaunched["launchId"]);
    c.kill_and_wait(&id);
}

// ported from tests/daemonFeatures.test.ts: untrusted repository commands never run while global defaults do; setup retries after inline trust; stale trust is refused
fn untrusted_repository_commands_never_run(c: &Ctx) {
    // Global defaults are user-authored: they run without trust.
    c.set_defaults(Some(json!({"setupCommand": "touch global-setup", "devCommand": "printf global-dev", "actions": {"gtest": "printf global-action", "gsleep": "sleep 30"}})));
    // The repository override (uncommitted, live in the main checkout) and a creation hook that must not run untrusted.
    let setup = r#"touch repo-setup; node -e "const fs=require('fs'); if(!fs.existsSync('.attempt')){fs.writeFileSync('.attempt','1');console.log('first attempt');process.exit(1)} console.log('setup complete')""#;
    fs::write(
        c.root.join("deckhand.json"),
        json!({"setupCommand": setup, "devCommand": "printf repo-dev", "actions": {"test": "printf action-output; exit 7"}}).to_string(),
    )
    .unwrap();
    let hook = c.root.join(".claude/scripts/create-worktree.sh");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(&hook, "#!/bin/bash\ntouch \"$CLAUDE_PROJECT_DIR/hook-ran\"\nexit 1\n").unwrap();
    let mut info = c.ok(json!({"type": "project-info", "cwd": c.root}));
    assert_eq!(info["needsReview"], true);
    assert_eq!(info["effective"]["setupCommand"], "touch global-setup");
    // Not reviewed: the repository setup is refused (the race path), the worktree retained, the hook never run.
    let unreviewed = c.create_mode("unreviewed-setup", "claude", "new");
    let unreviewed_id = id_of(&unreviewed);
    let mut item = c.wait(&unreviewed_id, |s| s["status"] == "exited");
    assert_eq!(item["setup"]["state"], "failed");
    assert!(text(&item["setup"]["output"]).contains("not trusted"), "{item}");
    assert!(item["agentStartedAt"].is_null());
    assert_eq!(item["worktree"]["creator"], "fallback");
    assert!(!exists(c.root.join("hook-ran")));
    assert!(!exists(Path::new(text(&item["cwd"])).join("repo-setup")));
    // Reviewed and skipped: only the global setup runs.
    let skipped = c.ok(json!({"type": "create", "input": {"title": "skipped-setup", "program": "claude", "cwd": c.root, "repoRoot": c.root, "cols": 80, "rows": 24, "worktreeMode": "new", "projectFingerprint": info["fingerprint"]}}));
    let skipped_id = id_of(&skipped);
    item = c.wait(&skipped_id, |s| s["status"] == "running");
    assert!(exists(Path::new(text(&item["cwd"])).join("global-setup")));
    assert!(!exists(Path::new(text(&item["cwd"])).join("repo-setup")));
    let run_action = |id: &str, action: &str| {
        c.call(json!({"type": "run-action", "sessionId": id, "action": action, "cols": 80, "rows": 24}))
    };
    assert_rejects(run_action(&skipped_id, "test"), &["trust"]);
    let ran = run_action(&skipped_id, "gtest").expect("run-action");
    assert!(text(&ran["command"]).contains("global-action"));
    assert_eq!(ran["name"], "gtest");
    assert_eq!(
        wf(
            || c.watch_action(&skipped_id),
            |r| r["live"] != true && r["exitCode"] == 0
        )["name"],
        "gtest"
    );
    // Actions have their own process: they never touch Dev, and one runs at a time per worktree until stopped.
    assert!(c.ok(json!({"type": "watch-dev", "sessionId": skipped_id, "cols": 80, "rows": 24}))["command"].is_null());
    assert_eq!(run_action(&skipped_id, "gsleep").expect("run-action")["live"], true);
    assert_rejects(run_action(&skipped_id, "gtest"), &["gsleep action is still running"]);
    assert!(
        text(&c.ok(json!({"type": "start-dev", "sessionId": skipped_id, "cols": 80, "rows": 24}))["command"])
            .contains("global-dev")
    );
    assert_eq!(c.watch_action(&skipped_id)["live"], true);
    c.ok(json!({"type": "stop-action", "sessionId": skipped_id}));
    wf(|| c.watch_action(&skipped_id), |r| r["live"] != true);
    c.ok(json!({"type": "stop-dev", "sessionId": skipped_id}));
    c.kill_and_wait(&skipped_id);
    // A trust request for bytes that changed after review is refused.
    fs::remove_dir_all(c.root.join(".claude")).unwrap();
    assert_rejects(
        c.call(json!({"type": "trust-project", "cwd": c.root, "fingerprint": info["fingerprint"]})),
        &["changed"],
    );
    info = c.ok(json!({"type": "project-info", "cwd": c.root}));
    info = c.ok(json!({"type": "trust-project", "cwd": item["cwd"], "fingerprint": info["fingerprint"]}));
    assert_eq!(info["trusted"], true);
    assert_eq!(info["effective"]["setupCommand"], setup);
    // After trust, s retries the refused setup with the repository command (first attempt fails, the retry succeeds).
    let _ = c.call(json!({"type": "restart", "sessionId": unreviewed_id, "cols": 80, "rows": 24}));
    item = c.wait(&unreviewed_id, |s| s["status"] == "exited");
    assert!(text(&item["setup"]["output"]).contains("first attempt"), "{item}");
    c.ok(json!({"type": "restart", "sessionId": unreviewed_id, "cols": 80, "rows": 24}));
    item = c.wait(&unreviewed_id, |s| s["status"] == "running");
    assert_eq!(item["setup"]["state"], "complete");
    assert!(!exists(Path::new(text(&item["cwd"])).join("global-setup")));
    let run = run_action(&unreviewed_id, "test").expect("run-action");
    assert!(text(&run["command"]).contains("action-output"));
    let output = wf(|| c.watch_action(&unreviewed_id), |r| r["exitCode"] == 7);
    assert!(text(&output["content"]).contains("action-output"));
    // Editing the live main-checkout file revokes action rights at once.
    fs::write(
        c.root.join("deckhand.json"),
        json!({"actions": {"test": "printf edited"}}).to_string(),
    )
    .unwrap();
    assert_rejects(run_action(&unreviewed_id, "test"), &["trust"]);
    assert_rejects(
        c.call(json!({"type": "kill", "sessionId": unreviewed_id, "deleteWorktree": true})),
        &["Deletion blocked"],
    );
    assert_eq!(c.state(&unreviewed_id)["status"], "running");
    c.ok(json!({"type": "kill", "sessionId": unreviewed_id, "deleteWorktree": true, "deleteBranch": true, "allowDataLoss": true}));
    wf(|| c.state(&unreviewed_id), |s| !s["worktree"]["deletedAt"].is_null());
    assert!(!exists(text(&item["cwd"])));
    c.set_defaults(None);
}

// ported from tests/daemonFeatures.test.ts: handoffs are initial context only and shared workspaces resist forced deletion
fn handoffs_are_initial_context_only(c: &Ctx) {
    // The repository config now has no setup (and is untrusted); commit it so the main checkout stays clean.
    c.git(&["add", "deckhand.json"]);
    c.git(&["commit", "-m", "no setup"]);
    let parent = c.create_mode("shared-feature", "claude", "new");
    let parent_id = id_of(&parent);
    c.wait(&parent_id, |s| s["status"] == "running");
    // A session in a worktree writes in the worktree's note.
    let shared = c.state(&parent_id)["sharedNotes"].clone();
    c.ok(json!({"type": "save-note", "sessionId": parent_id, "section": "shared", "noteId": format!("{}:{}", text(&shared["kind"]), text(&shared["id"])), "text": "Review this work", "revision": shared["revision"]}));
    let handoff = c.ok(json!({"type": "export-handoff", "sessionId": parent_id}));
    let handoff = text(&handoff).to_string();
    let content = fs::read_to_string(&handoff).expect("handoff file");
    let at = content.find("Review this work").expect("note text in handoff");
    assert!(
        content[at..].contains("## Workspace changes\n\nBase: main"),
        "{content}"
    );
    let parent_state = c.state(&parent_id);
    let child = c.ok(json!({"type": "create", "input": {"title": "review", "program": "pi", "cwd": parent_state["cwd"], "repoRoot": parent_state["cwd"], "cols": 80, "rows": 24, "parentSessionId": parent_id, "subSessionKind": "clean", "handoffFromSessionId": parent_id}}));
    let child_id = id_of(&child);
    c.wait(&child_id, |s| s["status"] == "running");
    let child_trace = wf(|| c.trace(&child_id), |t| t.is_some()).expect("trace");
    assert!(strings(&child_trace["args"]).iter().any(|arg| arg.contains(&handoff)));
    assert_rejects(
        c.call(json!({"type": "kill", "sessionId": parent_id, "deleteWorktree": true, "allowDataLoss": true})),
        &["in use"],
    );
    c.kill_and_wait(&child_id);
    c.kill_and_wait(&parent_id);
}

// ported from tests/daemonFeatures.test.ts: unknown Codex identity never silently becomes a new conversation
fn unknown_codex_identity_never_silently_becomes_a_new_conversation(c: &Ctx) {
    reinstall(
        &c.bin,
        "codex",
        AgentOptions {
            without_hooks: true,
            ..AgentOptions::default()
        },
    );
    let session = c.create("unknown-codex", "codex");
    let id = id_of(&session);
    c.wait(&id, |s| s["status"] == "running");
    c.kill_and_wait(&id);
    assert_rejects(
        c.call(json!({"type": "restart", "sessionId": id, "cols": 80, "rows": 24})),
        &["conversation ID is unknown"],
    );
}

// ported from tests/daemonFeatures.test.ts: cancelling setup, including a retry, kills its process, retains files and stopped metadata
fn cancelling_setup_kills_its_process(c: &Ctx) {
    fs::write(
        c.root.join("deckhand.json"),
        json!({"setupCommand": r#"node -e "require('fs').writeFileSync('setup.pid', String(process.pid)); setTimeout(()=>{},300000)""#}).to_string(),
    )
    .unwrap();
    c.git(&["add", "deckhand.json"]);
    c.git(&["commit", "-m", "long setup"]);
    let info = c.ok(json!({"type": "project-info", "cwd": c.root}));
    c.ok(json!({"type": "trust-project", "cwd": c.root, "fingerprint": info["fingerprint"]}));
    let session = c.create_mode("cancel-setup", "claude", "new");
    let id = id_of(&session);
    let cwd = text(&c.wait(&id, |s| s["setup"]["state"] == "running")["cwd"]).to_string();
    let pid_file = Path::new(&cwd).join("setup.pid");
    let setup_pid = || -> i32 {
        wf(|| fs::read_to_string(&pid_file).unwrap_or_default(), |s| !s.is_empty())
            .trim()
            .parse()
            .expect("pid")
    };
    let first = setup_pid();
    c.ok(json!({"type": "cancel-start", "sessionId": id}));
    assert_eq!(c.state(&id)["exitReason"], "stopped");
    wf(|| is_alive(first), |alive| !alive);
    fs::remove_file(&pid_file).unwrap(); // Retained worktree; only the marker goes.
    let retry = {
        let sock = c.sock.clone();
        let id = id.clone();
        std::thread::spawn(move || {
            request(
                &sock,
                json!({"type": "restart", "sessionId": id, "cols": 80, "rows": 24}),
            )
        })
    };
    c.wait(&id, |s| s["status"] == "starting" && s["setup"]["state"] == "running");
    let second = setup_pid();
    c.ok(json!({"type": "cancel-start", "sessionId": id}));
    let retry_error = retry
        .join()
        .expect("retry thread")
        .expect_err("the retried restart is rejected");
    assert!(
        retry_error.contains("cancelled") || retry_error.contains("code"),
        "{retry_error}"
    );
    let stopped = c.state(&id);
    assert_eq!(stopped["exitReason"], "stopped");
    assert_eq!(stopped["setup"]["state"], "cancelled");
    wf(|| is_alive(second), |alive| !alive);
}

// ported from tests/daemonFeatures.test.ts: the main checkout can never be deleted, even with the data-loss override
fn the_main_checkout_can_never_be_deleted(c: &Ctx) {
    let main = c.main_worktree_path();
    let session = c.attach("main-checkout", "claude", &c.root, &main);
    let id = id_of(&session);
    c.wait(&id, |s| s["status"] == "running");
    let inspection = c.ok(json!({"type": "inspect-cleanup", "sessionId": id}));
    assert_eq!(inspection["safe"], false);
    assert!(
        strings(&inspection["structuralBlockers"])
            .join(",")
            .contains("main worktree"),
        "{inspection}"
    );
    assert_rejects(
        c.call(json!({"type": "kill", "sessionId": id, "deleteWorktree": true, "deleteBranch": true, "allowDataLoss": true})),
        &["main worktree"],
    );
    assert_eq!(c.state(&id)["status"], "running");
    c.kill_and_wait(&id);
}

// ported from tests/daemonFeatures.test.ts: global worktree settings place new worktrees and link shared files; create-pr pushes and opens gh, never from main
fn global_worktree_settings_and_create_pr(c: &Ctx) {
    // The previous test left a trusted long-running setup; an untrusted empty override has nothing to run.
    fs::write(c.root.join("deckhand.json"), "{}").unwrap();
    c.git(&["commit", "-am", "empty config"]);
    let shared = c.user_home.join("shared-deps");
    fs::create_dir_all(&shared).unwrap();
    fs::write(shared.join("keep.js"), "dependency").unwrap();
    std::os::unix::fs::symlink(&shared, c.root.join("deps")).unwrap();
    fs::write(c.user_home.join("app.env"), "SECRET=1").unwrap();
    c.set_defaults(Some(json!({"worktree": {"location": "{home}/custom/{repo}/{name}", "symlink": ["deps", "missing"], "files": {"config/.env.local": "{home}/app.env"}}})));
    let session = c.create_mode("linked-wt", "claude", "new");
    let id = id_of(&session);
    let item = c.wait(&id, |s| s["status"] == "running");
    let cwd = PathBuf::from(text(&item["cwd"]));
    let expected = fs::canonicalize(&c.user_home)
        .unwrap()
        .join("custom")
        .join(c.root.file_name().unwrap())
        .join("linked-wt");
    assert_eq!(cwd, expected);
    assert_eq!(
        item["worktree"]["links"]["linked"],
        json!(["deps", "config/.env.local"])
    );
    assert!(
        strings(&item["worktree"]["links"]["notes"])
            .join(",")
            .contains("missing: source missing"),
        "{item}"
    );
    assert_eq!(
        fs::read_link(cwd.join("deps")).unwrap(),
        fs::canonicalize(&shared).unwrap()
    );
    assert_eq!(fs::read_to_string(cwd.join("config/.env.local")).unwrap(), "SECRET=1");

    // Create PR: a local bare origin and a fake gh that records its arguments.
    let origin = c.user_home.join("origin.git");
    git(&c.user_home, &["init", "--bare", origin.to_str().unwrap()]);
    c.git(&["remote", "add", "origin", origin.to_str().unwrap()]);
    c.git(&["push", "origin", "main"]);
    c.git(&["fetch", "origin"]);
    let calls = c.user_home.join("gh-calls.log");
    install_script(
        &c.bin.join("gh"),
        &format!(
            "#!/usr/bin/env node\nrequire('fs').appendFileSync({}, JSON.stringify(process.argv.slice(2)) + '\\n');\nif (process.argv[3] === 'view' && process.argv.includes('--json')) {{ console.error('no pull requests found for branch'); process.exit(1); }}\n",
            serde_json::to_string(calls.to_str().unwrap()).unwrap()
        ),
    );
    fs::write(cwd.join("file.txt"), "pr change\n").unwrap();
    git(&cwd, &["commit", "-am", "PR change"]);
    let result = c.ok(json!({"type": "create-pr", "sessionId": id, "branch": "linked-wt"}));
    assert_eq!(
        result,
        json!({"branch": "linked-wt", "remote": "origin", "base": "main", "existing": false})
    );
    assert_eq!(
        git(&origin, &["rev-parse", "refs/heads/linked-wt"]),
        git(&cwd, &["rev-parse", "HEAD"])
    );
    let argv: Vec<Value> = fs::read_to_string(&calls)
        .unwrap()
        .trim()
        .split('\n')
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(
        argv.last().unwrap(),
        &json!(["pr", "create", "--web", "--head", "linked-wt", "--base", "main"])
    );
    assert_rejects(
        c.call(json!({"type": "create-pr", "sessionId": id, "branch": "other"})),
        &["Branch changed"],
    );
    let main = c.main_worktree_path();
    let main_session = c.attach("main-pr", "claude", &c.root, &main);
    let main_id = id_of(&main_session);
    c.wait(&main_id, |s| s["status"] == "running");
    assert_rejects(
        c.call(json!({"type": "create-pr", "sessionId": main_id})),
        &["Refusing to create a PR from main"],
    );
    c.kill_and_wait(&main_id);

    // The links never force the data-loss override, and deleting the worktree keeps their targets.
    c.ok(json!({"type": "kill", "sessionId": id, "deleteWorktree": true, "deleteBranch": true}));
    wf(|| c.state(&id), |s| !s["worktree"]["deletedAt"].is_null());
    assert!(!exists(&cwd));
    assert_eq!(fs::read_to_string(shared.join("keep.js")).unwrap(), "dependency");
    assert_eq!(fs::read_to_string(c.user_home.join("app.env")).unwrap(), "SECRET=1");
    fs::remove_file(c.root.join("deps")).unwrap();
    c.set_defaults(None);
}

/// A copy of the document's JSON with one key path set (src/settingsModel.ts `applyChange`, for set).
fn apply_change(document: &Value, path: &[&str], value: Value) -> String {
    let mut next: Value = if document["exists"] == false {
        json!({})
    } else {
        serde_json::from_str(text(&document["raw"])).expect("document json")
    };
    let mut cursor = &mut next;
    for key in &path[..path.len() - 1] {
        if cursor.get(*key).is_none() {
            cursor[*key] = json!({});
        }
        cursor = cursor.get_mut(*key).unwrap();
    }
    cursor[path[path.len() - 1]] = value;
    format!("{}\n", serde_json::to_string_pretty(&next).unwrap())
}

fn layer_symlinks(document: &Value) -> Vec<String> {
    if document.is_null() || document["exists"] == false {
        return vec![];
    }
    let layer: Value = serde_json::from_str(text(&document["raw"])).unwrap_or_else(|_| json!({}));
    strings(&layer["worktree"]["symlink"])
}

// ported from tests/daemonFeatures.test.ts: Settings reads sources and both documents, lists main-checkout link candidates and sizes, and saves single keys through save-config keeping other keys
fn settings_reads_sources_and_saves_single_keys(c: &Ctx) {
    fs::create_dir_all(c.root.join("node_modules/pkg")).unwrap();
    fs::write(c.root.join("node_modules/pkg/index.js"), "x".repeat(8192)).unwrap();
    fs::write(c.root.join("notes.txt"), "n").unwrap();
    fs::write(c.root.join("deckhand.json"), r#"{"devCommand":"printf keep"}"#).unwrap();
    let info = c.ok(json!({"type": "settings-info", "cwd": c.root}));
    assert_eq!(info["repository"]["state"], "untrusted");
    assert_eq!(info["originBranch"], "main");
    assert!(info["hookFile"].is_null());
    let row = info["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["key"] == "devCommand")
        .expect("devCommand row");
    assert_eq!(row["pending"]["value"], "printf keep");
    let candidates = c.ok(json!({"type": "worktree-candidates", "cwd": c.root}))["candidates"]
        .as_array()
        .unwrap()
        .clone();
    let by_path = |p: &str| candidates.iter().find(|candidate| candidate["path"] == p).cloned();
    assert_eq!(by_path("node_modules").unwrap()["suggestion"], "link");
    assert_eq!(by_path("node_modules").unwrap()["kind"], "dir");
    assert_eq!(by_path("notes.txt").unwrap()["suggestion"], "skip");
    assert!(by_path("deckhand.json").is_none());
    let sizes =
        c.ok(json!({"type": "worktree-candidate-sizes", "cwd": c.root, "paths": ["node_modules", "notes.txt"]}));
    assert!(sizes["node_modules"].as_f64().unwrap_or(0.0) >= 8.0, "{sizes}");
    assert_eq!(sizes["notes.txt"], 1);
    assert_rejects(
        c.call(json!({"type": "worktree-candidate-sizes", "cwd": c.root, "paths": ["../escape"]})),
        &["relative"],
    );
    // The picker's selection (settingsModel `initialLinks` + `linkSelection`): configured or suggested candidates are
    // linked; the layer's own entries stay, newly linked ones are appended unless the other layer already links them.
    let own = layer_symlinks(&info["targets"]["repository"]);
    let others = layer_symlinks(&info["targets"]["global"]);
    let mut symlink = own.clone();
    for candidate in &candidates {
        let path = text(&candidate["path"]).to_string();
        let linked =
            !candidate["configured"].is_null() && candidate["configured"] != false || candidate["suggestion"] == "link";
        if candidate["configured"] != "files" && linked && !own.contains(&path) && !others.contains(&path) {
            symlink.push(path);
        }
    }
    assert_eq!(symlink, vec!["node_modules".to_string()]);
    let document = info["targets"]["repository"].clone();
    let raw = apply_change(&document, &["worktree", "symlink"], json!(symlink));
    // Saved over bytes nobody reviewed (written outside Deckhand): the file still needs review.
    let saved = c.ok(json!({"type": "save-config", "target": "repository", "cwd": c.root, "raw": raw, "revision": document["revision"]}));
    assert_eq!(saved["trust"], "unreviewed");
    assert_eq!(c.ok(json!({"type": "project-info", "cwd": c.root}))["trusted"], false);
    let written: Value = serde_json::from_str(&fs::read_to_string(c.root.join("deckhand.json")).unwrap()).unwrap();
    assert_eq!(
        written,
        json!({"devCommand": "printf keep", "worktree": {"symlink": ["node_modules"]}})
    );
    assert_rejects(c.call(json!({"type": "save-config", "target": "repository", "cwd": c.root, "raw": raw, "revision": document["revision"]})), &["changed on disk"]);
    let trusted = c.load_config()["trustedProjects"].clone();
    let global = info["targets"]["global"].clone();
    c.ok(json!({"type": "save-config", "target": "global", "cwd": c.root, "raw": apply_change(&global, &["worktree", "branchFrom"], json!("default")), "revision": global["revision"]}));
    let config = c.load_config();
    assert_eq!(config["defaults"], json!({"worktree": {"branchFrom": "default"}}));
    assert_eq!(config["trustedProjects"], trusted);
    fs::remove_dir_all(c.root.join("node_modules")).unwrap();
    fs::remove_file(c.root.join("notes.txt")).unwrap();
    fs::write(c.root.join("deckhand.json"), "{}").unwrap();
    c.set_defaults(None);
}

// ported from tests/daemonFeatures.test.ts: Claude and Pi launch with exact native IDs; forks never type into Pi; legacy refs still resume
fn claude_and_pi_launch_with_exact_native_ids(c: &Ctx, daemon: &mut Daemon) {
    let claude = c.create("exact-claude", "claude");
    let claude_id = id_of(&claude);
    let started = c.wait(&claude_id, |s| {
        s["status"] == "running" && s["attention"]["event"] == "SessionStart"
    });
    let id = text(&started["agentSessionRef"]["value"]).to_string();
    assert_eq!(started["agentSessionRef"]["kind"], "id");
    let args = strings(&c.trace(&claude_id).unwrap()["args"]);
    assert_eq!(
        args[..4],
        [
            "--session-id".to_string(),
            id.clone(),
            "--name".into(),
            format!("dh-exact-claude-{}", &claude_id[..8])
        ]
    );
    c.kill_and_wait(&claude_id);
    let resumed = c.relaunch(&claude_id, "resume");
    assert_eq!(resumed[..2], ["--resume".to_string(), id.clone()]);
    assert!(!resumed.iter().any(|a| a == "--session-id"));
    // Accepting a plan with "clear context" moves Claude to a new conversation and sends no SessionStart: the next
    // root event's ID is followed, a native subagent's never, and s resumes the conversation on screen.
    let launch = c.trace(&claude_id).unwrap();
    let hook = |payload: Value| c.hook(&claude_id, &launch, payload, now_ms() + 60_000).expect("hook");
    let implementing = uuid::Uuid::new_v4().to_string();
    hook(
        json!({"hook_event_name": "PermissionRequest", "agent_id": "sub", "session_id": uuid::Uuid::new_v4().to_string(), "tool_name": "Bash", "tool_input": {"command": "ls"}}),
    );
    assert_eq!(c.state(&claude_id)["agentSessionRef"]["value"], id.as_str());
    hook(json!({"hook_event_name": "PreToolUse", "tool_name": "Bash", "session_id": implementing}));
    assert_eq!(c.state(&claude_id)["agentSessionRef"]["value"], implementing.as_str());
    c.kill_and_wait(&claude_id);
    assert_eq!(
        c.relaunch(&claude_id, "resume")[..2],
        ["--resume".to_string(), implementing.clone()]
    );
    c.kill_and_wait(&claude_id);
    // Missed hooks: Claude's transcripts show the conversation this one continued in (same --name), so s resumes it.
    let real_root = fs::canonicalize(&c.root).unwrap();
    let slug: String = real_root
        .to_string_lossy()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect();
    let project = c.home.join(".claude/projects").join(slug);
    fs::create_dir_all(&project).unwrap();
    let later = uuid::Uuid::new_v4().to_string();
    let title = format!("dh-exact-claude-{}", &claude_id[..8]);
    let line = |record: Value| format!("{record}\n");
    fs::write(
        project.join(format!("{implementing}.jsonl")),
        line(json!({"type": "custom-title", "customTitle": title}))
            + &line(json!({"type": "user", "uuid": "u1", "parentUuid": null, "timestamp": iso(-10)}))
            + &line(json!({"type": "assistant", "uuid": "u2", "parentUuid": "u1", "timestamp": iso(-9)})),
    )
    .unwrap();
    fs::write(
        project.join(format!("{later}.jsonl")),
        line(json!({"type": "custom-title", "customTitle": title}))
            + &line(json!({"type": "user", "uuid": "u3", "parentUuid": "u2", "timestamp": iso(-5)}))
            + &line(json!({"type": "assistant", "uuid": "u4", "parentUuid": "u3", "timestamp": iso(-4)})),
    )
    .unwrap();
    assert_eq!(
        c.relaunch(&claude_id, "resume")[..2],
        ["--resume".to_string(), later.clone()]
    );
    assert_eq!(c.state(&claude_id)["agentSessionRef"]["value"], later.as_str());
    c.kill_and_wait(&claude_id);
    fs::remove_dir_all(c.home.join(".claude")).unwrap();

    install_script(
        &c.bin.join("claude"),
        "#!/usr/bin/env node\nconsole.log('No conversation found with session ID: ' + process.argv[process.argv.indexOf('--resume') + 1]);\nsetTimeout(() => process.exit(1), 200);\n",
    );
    c.ok(json!({"type": "restart", "sessionId": claude_id, "cols": 80, "rows": 24}));
    let missing = c.wait(&claude_id, |s| {
        s["status"] == "exited" && text(&s["lastPreview"]).contains("Press S")
    });
    assert_eq!(missing["exitReason"], "failed");
    assert!(
        text(&missing["lastPreview"]).contains("Press S to start a fresh conversation"),
        "{missing}"
    );
    assert_rejects(
        c.call(json!({"type": "restart", "sessionId": claude_id, "cols": 80, "rows": 24})),
        &["no saved conversation", "Use S"],
    );
    reinstall(&c.bin, "claude", AgentOptions::default());
    let fresh = c.relaunch(&claude_id, "fresh");
    assert_eq!(fresh[0], "--session-id");
    assert_ne!(fresh[1], id);
    c.kill_and_wait(&claude_id);

    let pi = c.create("exact-pi", "pi");
    let pi_id = id_of(&pi);
    let pi_ref = c.wait(&pi_id, |s| s["status"] == "running")["agentSessionRef"].clone();
    assert_eq!(pi_ref["kind"], "id");
    let pi_value = text(&pi_ref["value"]).to_string();
    let pi_args = strings(&wf(|| c.trace(&pi_id), |t| t.is_some()).unwrap()["args"]);
    assert_eq!(pi_args[..2], ["--session-id".to_string(), pi_value.clone()]);
    let fork = c.ok(json!({"type": "create", "input": {"title": "branch", "program": "pi", "cwd": c.root, "repoRoot": c.root, "cols": 80, "rows": 24, "parentSessionId": pi_id, "subSessionKind": "forked"}}));
    let fork_id = id_of(&fork);
    let fork_value = text(&c.wait(&fork_id, |s| s["status"] == "running")["agentSessionRef"]["value"]).to_string();
    assert_ne!(fork_value, pi_value);
    let fork_args = strings(&wf(|| c.trace(&fork_id), |t| t.is_some()).unwrap()["args"]);
    assert_eq!(
        fork_args[..4],
        [
            "--fork".to_string(),
            pi_value.clone(),
            "--session-id".into(),
            fork_value.clone()
        ]
    );
    sleep_ms(800); // Past the old /fork keystroke delay.
    assert_eq!(
        fs::read_to_string(c.home.join(format!("input-{fork_id}"))).unwrap_or_default(),
        ""
    );
    c.kill_and_wait(&fork_id);
    assert_eq!(
        c.relaunch(&fork_id, "resume")[..2],
        ["--session-id".to_string(), fork_value.clone()]
    );
    c.kill_and_wait(&fork_id);
    c.kill_and_wait(&pi_id);

    // Sessions persisted before exact IDs keep their name/path handles.
    let legacy_path = c.home.join("legacy-pi.jsonl");
    daemon.restart_with(|sessions| {
        for item in sessions.iter_mut() {
            if item["id"] == claude_id.as_str() {
                item["agentSessionRef"] = json!({"provider": "claude", "kind": "name", "value": "dh-legacy"});
            } else if item["id"] == pi_id.as_str() {
                item["agentSessionRef"] = json!({"provider": "pi", "kind": "path", "value": legacy_path});
            }
        }
    });
    assert_eq!(
        c.relaunch(&claude_id, "resume")[..2],
        ["--resume".to_string(), "dh-legacy".into()]
    );
    assert_eq!(
        c.relaunch(&pi_id, "resume")[..2],
        ["--session".to_string(), legacy_path.to_string_lossy().into_owned()]
    );
    c.kill_and_wait(&claude_id);
    c.kill_and_wait(&pi_id);
}

// ported from tests/daemonFeatures.test.ts: sessions in one worktree share one Dev that outlives their agents; another worktree has its own
fn sessions_in_one_worktree_share_one_dev(c: &Ctx) {
    c.set_defaults(Some(json!({"devCommand": r#"node -e "console.log('dev-pid=' + process.pid + ' cwd=' + process.cwd()); setInterval(() => {}, 1000)""#})));
    let watch = |id: &str| c.ok(json!({"type": "watch-dev", "sessionId": id, "cols": 120, "rows": 24}));
    let start_dev = |id: &str| c.call(json!({"type": "start-dev", "sessionId": id, "cols": 120, "rows": 24}));
    let dev_pid = |id: &str| -> i32 {
        let record = wf(
            || watch(id),
            |r| r["live"] == true && digits_after(text(&r["content"]), "dev-pid=").is_some(),
        );
        digits_after(text(&record["content"]), "dev-pid=").unwrap()
    };
    let dev_running = |ids: &[&str], expected: &[bool]| {
        wf(
            || c.list(),
            |items| {
                ids.iter().zip(expected).all(|(id, want)| {
                    items
                        .iter()
                        .find(|i| i["id"] == *id)
                        .is_some_and(|i| (i["devRunning"] == true) == *want)
                })
            },
        );
    };
    // A new worktree and a second session attached to it share one workspace; a third worktree is another.
    let first = c.create_mode("ws-shared", "claude", "new");
    let first_id = id_of(&first);
    let first_state = c.wait(&first_id, |s| s["status"] == "running");
    let second = c.attach("ws-attached", "pi", &c.root, text(&first_state["cwd"]));
    let second_id = id_of(&second);
    let solo = c.create_mode("ws-solo", "claude", "new");
    let solo_id = id_of(&solo);
    let second_state = c.wait(&second_id, |s| s["status"] == "running");
    let solo_state = c.wait(&solo_id, |s| s["status"] == "running");
    let key = workspace_key(&first_state).unwrap();
    assert_eq!(workspace_key(&second_state), Some(key.clone()));
    assert_ne!(workspace_key(&solo_state), Some(key.clone()));

    let started = start_dev(&first_id).expect("start-dev");
    assert_eq!(started["live"], true);
    assert_eq!(started["workspace"], key.as_str());
    assert_eq!(started["sessionId"], first_id.as_str());
    let pid = dev_pid(&second_id);
    assert_eq!(dev_pid(&first_id), pid);
    // Starting from the other session reuses the shared process.
    assert_eq!(
        start_dev(&second_id).expect("start-dev")["sessionId"],
        second_id.as_str()
    );
    assert_eq!(dev_pid(&second_id), pid);
    dev_running(&[&first_id, &second_id, &solo_id], &[true, true, false]);
    let solo_dev = watch(&solo_id);
    assert_ne!(solo_dev["live"], true);
    assert_eq!(solo_dev["workspace"], workspace_key(&solo_state).unwrap().as_str());
    start_dev(&solo_id).expect("start-dev");
    let solo_pid = dev_pid(&solo_id);
    assert_ne!(solo_pid, pid);
    let solo_content = watch(&solo_id);
    // The 120-column pane wraps the (long, temp-dir) path: compare the content with the line breaks removed.
    assert!(
        text(&solo_content["content"])
            .replace('\n', "")
            .contains(&format!("cwd={}", text(&solo_state["cwd"]))),
        "{solo_content} vs {}",
        solo_state["cwd"]
    );
    dev_running(&[&first_id, &second_id, &solo_id], &[true, true, true]);

    // Dev outlives an agent: still visible from, stoppable and startable by the exited session.
    c.kill_and_wait(&second_id);
    assert!(is_alive(pid));
    assert_eq!(watch(&second_id)["live"], true);
    dev_running(&[&first_id, &second_id], &[true, true]);
    c.ok(json!({"type": "stop-dev", "sessionId": second_id}));
    wf(|| is_alive(pid), |alive| !alive);
    dev_running(&[&first_id, &second_id], &[false, false]);
    assert_ne!(watch(&first_id)["live"], true);
    start_dev(&second_id).expect("start-dev");
    let restarted = dev_pid(&first_id);

    // Deleting the worktree stops its Dev before removal; a deleted worktree has no Dev.
    c.ok(json!({"type": "kill", "sessionId": first_id, "deleteWorktree": true, "deleteBranch": true}));
    let deleted = wf(|| c.state(&first_id), |s| !s["worktree"]["deletedAt"].is_null());
    assert!(!is_alive(restarted));
    assert_ne!(deleted["devRunning"], true);
    dev_running(&[&second_id], &[false]);
    assert_rejects(start_dev(&first_id), &["worktree was deleted"]);

    // Removing a workspace's last session stops its Dev.
    c.kill_and_wait(&solo_id);
    assert!(is_alive(solo_pid));
    c.ok(json!({"type": "remove", "sessionId": solo_id}));
    wf(|| is_alive(solo_pid), |alive| !alive);
    c.set_defaults(None);
}

// ported from tests/daemonFeatures.test.ts: sessions in one worktree share one Terminal and one lazygit, also once exited; the workspace stops them with its worktree or last session
fn sessions_in_one_worktree_share_one_terminal_and_lazygit(c: &Ctx) {
    // A fake lazygit (bin is first on the daemon's PATH) that reports its PID and cwd and stays up.
    let lazygit = c.bin.join("lazygit");
    install_script(
        &lazygit,
        "#!/usr/bin/env node\nconsole.log('lazygit-pid=' + process.pid + ' cwd=' + process.cwd());\nsetInterval(() => {}, 1000);\n",
    );
    c.set_defaults(Some(json!({"devCommand": r#"node -e "setInterval(() => {}, 1000)""#})));
    // One persistent connection: the fire-and-forget input path, and a UI-like watcher that receives fan-out.
    let socket = Raw::connect(&c.sock);
    // Wide enough that no echoed worktree path wraps.
    let watch = |id: &str| c.ok(json!({"type": "watch-terminal", "sessionId": id, "cols": 300, "rows": 24}));
    let watch_git = |id: &str| c.ok(json!({"type": "watch-git", "sessionId": id, "cols": 300, "rows": 24}));
    // Typed lines echo `$$`; only the shell's output has digits there.
    let shell = |id: &str| -> (i32, String) {
        let marker = uuid::Uuid::new_v4().to_string()[..8].to_string();
        wf(|| watch(id), |r| r["live"] == true); // Input reaches only a started shell.
        socket
            .send(json!({"type": "terminal-input", "sessionId": id, "data": format!("echo {marker}=$$:$(pwd -P)\r")}));
        let record = wf(|| watch(id), |r| marker_result(text(&r["content"]), &marker).is_some());
        marker_result(text(&record["content"]), &marker).unwrap()
    };
    let git_pid = |id: &str| -> i32 {
        let record = wf(
            || watch_git(id),
            |r| r["live"] == true && digits_after(text(&r["content"]), "lazygit-pid=").is_some(),
        );
        digits_after(text(&record["content"]), "lazygit-pid=").unwrap()
    };
    let first = c.create_mode("term-shared", "claude", "new");
    let first_id = id_of(&first);
    let first_state = c.wait(&first_id, |s| s["status"] == "running");
    let second = c.attach("term-attached", "pi", &c.root, text(&first_state["cwd"]));
    let second_id = id_of(&second);
    let solo = c.create_mode("term-solo", "claude", "new");
    let solo_id = id_of(&solo);
    c.wait(&second_id, |s| s["status"] == "running");
    let solo_state = c.wait(&solo_id, |s| s["status"] == "running");
    let key = workspace_key(&first_state).unwrap();
    let worker_pid_file = c
        .home
        .join("workers")
        .join(format!("{}.pid", workspace_worker_id(&key)));

    // Terminal and Git start on first view, in the workspace worker; both sessions see the same shell and lazygit.
    let viewed = watch(&first_id);
    assert_eq!(viewed["workspace"], key.as_str());
    assert_eq!(viewed["sessionId"], first_id.as_str());
    assert!(exists(&worker_pid_file));
    let (shell_pid, cwd) = shell(&first_id);
    assert_eq!(
        cwd,
        fs::canonicalize(text(&first_state["cwd"])).unwrap().to_string_lossy()
    );
    assert_eq!(shell(&second_id).0, shell_pid);
    let lazygit_pid = git_pid(&first_id);
    assert_eq!(git_pid(&second_id), lazygit_pid);
    let (solo_shell_pid, solo_cwd) = shell(&solo_id);
    assert_ne!(solo_shell_pid, shell_pid);
    assert_eq!(
        solo_cwd,
        fs::canonicalize(text(&solo_state["cwd"])).unwrap().to_string_lossy()
    );
    assert_ne!(git_pid(&solo_id), lazygit_pid);

    // Fan-out: a client watching one session sees output typed through another session of the workspace.
    socket.send(json!({"type": "watch-terminal", "requestId": uuid::Uuid::new_v4().to_string(), "sessionId": first_id, "cols": 300, "rows": 24}));
    socket.send(json!({"type": "terminal-input", "sessionId": second_id, "data": "echo fanout-$((40+2))\r"}));
    wf(
        || {
            socket.has(|e| {
                e["type"] == "terminal-updated"
                    && e["terminal"]["sessionId"] == first_id.as_str()
                    && e["terminal"]["workspace"] == key.as_str()
                    && text(&e["terminal"]["content"]).contains("fanout-42")
            })
        },
        |found| *found,
    );
    assert!(!socket.has(|e| e["type"] == "terminal-updated" && e["terminal"]["sessionId"] != first_id.as_str()));

    // Attach goes through the workspace, mirrors bracketed paste, and allows one attacher per pane.
    socket.send(json!({"type": "terminal-input", "sessionId": first_id, "data": "printf '\\033[?2004h'; echo paste-$((2+3))\r"}));
    wf(|| watch(&first_id), |r| text(&r["content"]).contains("paste-5"));
    let attach_id = uuid::Uuid::new_v4().to_string();
    socket.send(
        json!({"type": "attach-terminal", "requestId": attach_id, "sessionId": second_id, "cols": 300, "rows": 30}),
    );
    let attached = wf(
        || socket.find(|e| e["type"] == "response" && e["requestId"] == attach_id.as_str()),
        |e| e.is_some(),
    )
    .unwrap();
    assert_eq!(attached["ok"], true);
    assert_eq!(attached["data"]["terminalModes"]["bracketedPaste"], true);
    assert_rejects(
        c.call(json!({"type": "attach-terminal", "sessionId": first_id, "cols": 80, "rows": 24})),
        &["already attached elsewhere"],
    );
    socket.send(json!({"type": "terminal-input", "sessionId": second_id, "data": "echo attached-$((1+1))\r"}));
    wf(
        || {
            socket.has(|e| {
                e["type"] == "terminal-output"
                    && e["sessionId"] == second_id.as_str()
                    && text(&e["data"]).contains("attached-2")
            })
        },
        |found| *found,
    );
    socket.send(json!({"type": "terminal-detach", "sessionId": second_id}));
    wf(
        || socket.has(|e| e["type"] == "terminal-detached" && e["sessionId"] == second_id.as_str()),
        |found| *found,
    );

    // The panes outlive agents: an exited session still has the shared shell and lazygit.
    c.kill_and_wait(&second_id);
    assert!(is_alive(shell_pid));
    assert_eq!(watch(&second_id)["live"], true);
    assert_eq!(shell(&second_id).0, shell_pid);
    // A lazygit that exited starts again on the next view.
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(lazygit_pid),
        nix::sys::signal::Signal::SIGKILL,
    )
    .expect("kill lazygit");
    wf(
        || watch_git(&first_id),
        |r| r["live"] == true && !text(&r["content"]).contains(&format!("lazygit-pid={lazygit_pid}")),
    );
    let relaunched_git = git_pid(&second_id);
    assert_ne!(relaunched_git, lazygit_pid);
    // Stopping Dev keeps a worker whose Terminal/Git are in use.
    c.ok(json!({"type": "start-dev", "sessionId": second_id, "cols": 80, "rows": 24}));
    c.ok(json!({"type": "stop-dev", "sessionId": second_id}));
    sleep_ms(300);
    assert!(is_alive(shell_pid));
    assert_eq!(shell(&first_id).0, shell_pid);

    // Kill-with-delete stops the workspace's panes before removing the worktree; the deleted session has no workspace.
    c.ok(json!({"type": "kill", "sessionId": first_id, "deleteWorktree": true, "allowDataLoss": true}));
    let deleted = wf(|| c.state(&first_id), |s| !s["worktree"]["deletedAt"].is_null());
    assert!(!is_alive(shell_pid));
    assert!(!is_alive(relaunched_git));
    assert!(!exists(&worker_pid_file));
    assert!(!exists(text(&deleted["cwd"])));
    let gone = watch(&first_id);
    assert_ne!(gone["live"], true);
    assert!(gone["workspace"].is_null());
    assert_rejects(
        c.call(json!({"type": "attach-terminal", "sessionId": first_id, "cols": 80, "rows": 24})),
        &["Terminal is unavailable: its worktree was deleted"],
    );
    // The deletion belongs to the worktree: its other (exited) session has no workspace either.
    assert_eq!(
        c.state(&second_id)["worktree"]["deletedAt"],
        deleted["worktree"]["deletedAt"]
    );
    assert!(watch(&second_id)["workspace"].is_null());

    // Removing a workspace's last session stops its shell (which never exits on its own).
    c.kill_and_wait(&solo_id);
    assert!(is_alive(solo_shell_pid));
    c.ok(json!({"type": "remove", "sessionId": solo_id}));
    wf(|| is_alive(solo_shell_pid), |alive| !alive);
    let solo_worker = c.home.join("workers").join(format!(
        "{}.pid",
        workspace_worker_id(&workspace_key(&solo_state).unwrap())
    ));
    wf(|| exists(&solo_worker), |present| !present);
    drop(socket);
    let _ = fs::remove_file(&lazygit);
    c.set_defaults(None);
}

fn shape(changes: &Value) -> Vec<String> {
    changes["entries"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|e| format!("{}:{}:{}", text(&e["group"]), text(&e["status"]), text(&e["path"])))
                .collect()
        })
        .unwrap_or_default()
}

fn svec(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

// ported from tests/daemonFeatures.test.ts: the Git tab's Changes are per worktree: watched sessions get pushed updates; stage/unstage only touch listed paths
fn the_git_tabs_changes_are_per_worktree(c: &Ctx) {
    let socket = Raw::connect(&c.sock);
    let pushed = |session_id: &str, matches: &dyn Fn(&Value) -> bool| -> Value {
        let found = wait_for(
            || {
                socket.find(|e| {
                    e["type"] == "changes-updated" && e["changes"]["sessionId"] == session_id && matches(&e["changes"])
                })
            },
            |e| e.is_some(),
            Duration::from_secs(20),
        );
        found.unwrap()["changes"].clone()
    };
    let first = c.create_mode("changes-a", "claude", "new");
    let first_id = id_of(&first);
    let first_state = c.wait(&first_id, |s| s["status"] == "running");
    let second = c.attach("changes-b", "pi", &c.root, text(&first_state["cwd"]));
    let second_id = id_of(&second);
    c.wait(&second_id, |s| s["status"] == "running");
    let key = workspace_key(&first_state).unwrap();
    let key_path = Path::new(&key);

    // The second session's viewer watches; changes made in the worktree arrive by polling.
    socket
        .send(json!({"type": "watch-changes", "requestId": uuid::Uuid::new_v4().to_string(), "sessionId": second_id}));
    let initial = c.ok(json!({"type": "watch-changes", "sessionId": first_id}));
    assert_eq!(initial["workspace"], key.as_str());
    assert_eq!(initial["loaded"], true);
    assert_eq!(initial["entries"], json!([]));
    fs::write(key_path.join("file.txt"), "first\nchanged\n").unwrap();
    fs::write(key_path.join("new file.txt"), "new\n").unwrap();
    let polled = pushed(&second_id, &|changes| changes["counts"]["untracked"] == 1);
    assert_eq!(polled["workspace"], key.as_str());
    assert_eq!(
        shape(&polled),
        svec(&["unstaged:M:file.txt", "untracked:?:new file.txt"])
    );
    assert_eq!(polled["entries"][0]["additions"], 1);

    // One session stages; the other session's viewer receives the update at once.
    socket.clear();
    let staged = c.ok(json!({"type": "change-stage", "sessionId": first_id, "mode": "stage", "group": "unstaged", "path": "file.txt"}));
    assert_eq!(
        shape(&staged["changes"]),
        svec(&["staged:M:file.txt", "untracked:?:new file.txt"])
    );
    assert_eq!(staged["changes"]["sessionId"], first_id.as_str());
    pushed(&second_id, &|changes| changes["counts"]["staged"] == 1);
    let diff = c.ok(json!({"type": "changes-diff", "sessionId": second_id, "group": "staged", "path": "file.txt"}));
    assert!(text(&diff["text"]).lines().any(|l| l == "+changed"), "{diff}");
    assert_eq!(diff["firstLine"], 2);
    // Paths that are not listed (or listed on the other side) are refused, and so are sessions without a workspace.
    assert_rejects(c.call(json!({"type": "change-stage", "sessionId": first_id, "mode": "stage", "group": "untracked", "path": "../outside.txt"})), &["not among the unstaged changes"]);
    assert_rejects(c.call(json!({"type": "change-stage", "sessionId": first_id, "mode": "unstage", "group": "untracked", "path": "new file.txt"})), &["not among the staged changes"]);
    assert_rejects(
        c.call(json!({"type": "changes-diff", "sessionId": first_id, "group": "unstaged", "path": "nope.txt"})),
        &["not in the current changes"],
    );
    assert_eq!(git(key_path, &["diff", "--cached", "--name-only"]), "file.txt");
    // Stage all, unstage all.
    assert_eq!(
        shape(&c.ok(json!({"type": "change-stage", "sessionId": second_id, "mode": "stage"}))["changes"]),
        svec(&["staged:M:file.txt", "staged:A:new file.txt"])
    );
    assert_eq!(
        shape(&c.ok(json!({"type": "change-stage", "sessionId": first_id, "mode": "unstage"}))["changes"]),
        svec(&["unstaged:M:file.txt", "untracked:?:new file.txt"])
    );
    pushed(&second_id, &|changes| {
        changes["counts"]["staged"] == 0 && changes["counts"]["untracked"] == 1
    });

    // A session in a repository without commits: unstaging removes the path from the index.
    let unborn = tempfile::Builder::new().prefix("deckhand-unborn-").tempdir().unwrap();
    git(unborn.path(), &["init", "-b", "main"]);
    fs::write(unborn.path().join("a.txt"), "a\n").unwrap();
    git(unborn.path(), &["add", "a.txt"]);
    let fresh = c.ok(json!({"type": "create", "input": {"title": "changes-unborn", "program": "claude", "cwd": unborn.path(), "repoRoot": unborn.path(), "cols": 80, "rows": 24}}));
    let fresh_id = id_of(&fresh);
    c.wait(&fresh_id, |s| s["status"] == "running");
    assert_eq!(
        shape(&c.ok(json!({"type": "watch-changes", "sessionId": fresh_id}))),
        svec(&["staged:A:a.txt"])
    );
    assert_eq!(
        shape(&c.ok(json!({"type": "change-stage", "sessionId": fresh_id, "mode": "unstage", "group": "staged", "path": "a.txt"}))["changes"]),
        svec(&["untracked:?:a.txt"])
    );

    // Unwatching stops the pushes (and the polling).
    socket.send(json!({"type": "watch-changes", "requestId": uuid::Uuid::new_v4().to_string()}));
    sleep_ms(100);
    socket.clear();
    fs::write(key_path.join("later.txt"), "later\n").unwrap();
    sleep_ms(2600);
    assert!(!socket.has(|e| e["type"] == "changes-updated"));
    for id in [&first_id, &second_id, &fresh_id] {
        c.kill_and_wait(id);
    }
}

// ported from tests/daemonFeatures.test.ts: merge and deletion markers belong to the worktree: every session of it shares them, a new worktree at the path does not
fn merge_and_deletion_markers_belong_to_the_worktree(c: &Ctx) {
    let socket = Raw::connect(&c.sock);
    let subscribe_id = uuid::Uuid::new_v4().to_string();
    socket.send(json!({"type": "subscribe", "requestId": subscribe_id, "repoRoot": c.root}));
    wf(
        || socket.has(|e| e["type"] == "response" && e["requestId"] == subscribe_id.as_str()),
        |found| *found,
    );
    let updated = |id: &str, matches: &dyn Fn(&Value) -> bool| {
        wf(
            || socket.has(|e| e["type"] == "session-updated" && e["session"]["id"] == id && matches(&e["session"])),
            |found| *found,
        );
    };
    let merged = |id: &str| c.state(id)["worktree"]["mergedAt"].clone();
    // A new worktree, a session attached to it and a sub-session launched in it (no worktree of its own).
    let first = c.create_mode("records-a", "claude", "new");
    let first_id = id_of(&first);
    let first_state = c.wait(&first_id, |s| s["status"] == "running");
    let attached = c.attach("records-b", "pi", &c.root, text(&first_state["cwd"]));
    let attached_id = id_of(&attached);
    let child = c.ok(json!({"type": "create", "input": {"title": "records-child", "program": "pi", "cwd": first_state["cwd"], "repoRoot": c.root, "cols": 80, "rows": 24, "parentSessionId": first_id, "subSessionKind": "clean"}}));
    let child_id = id_of(&child);
    let attached_state = c.wait(&attached_id, |s| s["status"] == "running");
    let child_state = c.wait(&child_id, |s| s["status"] == "running");
    let worktree_id = first_state["worktree"]["id"].clone();
    assert!(!worktree_id.is_null());
    assert_eq!(attached_state["worktree"]["id"], worktree_id);
    assert_eq!(child_state["worktree"]["id"], worktree_id);
    assert_eq!(child_state["worktree"]["mode"], "none");

    // m from one session marks the worktree: every session of it gets the marker, and every client hears about each.
    let cwd = PathBuf::from(text(&first_state["cwd"]));
    fs::write(cwd.join("records.txt"), "records\n").unwrap();
    git(&cwd, &["add", "."]);
    git(&cwd, &["commit", "-m", "records"]);
    socket.clear();
    let result = c.ok(json!({"type": "merge-worktree", "sessionId": first_id, "mode": "squash", "targetCwd": c.root}));
    assert!(result["skipped"] != true && result["conflicted"] != true, "{result}");
    c.git(&["commit", "-m", "squashed records"]);
    for id in [&attached_id, &child_id] {
        updated(id, &|session| !session["worktree"]["mergedAt"].is_null());
    }
    let sibling = c.state(&attached_id);
    assert_eq!(sibling["worktree"]["mergeMode"], "squash");
    assert_eq!(sibling["worktree"]["mergeTargetBranch"], "main");
    assert_eq!(sibling["worktree"]["mergeSourceRef"], "records-a");
    assert_eq!(merged(&child_id), sibling["worktree"]["mergedAt"]);
    // M from another session toggles it for all of them.
    let unmarked = c.ok(json!({"type": "mark-session-merged", "sessionId": attached_id, "targetCwd": c.root}));
    assert!(unmarked["worktree"]["mergedAt"].is_null());
    for id in [&first_id, &attached_id, &child_id] {
        assert!(merged(id).is_null());
    }
    let marked = c.ok(json!({"type": "mark-session-merged", "sessionId": child_id, "targetCwd": c.root}));
    assert_eq!(marked["worktree"]["mergeMarkedManually"], true);
    for id in [&first_id, &attached_id] {
        assert_eq!(merged(id), marked["worktree"]["mergedAt"]);
    }
    // The marker is stored once, in the worktree record.
    let stored = c.load_state();
    let record = stored["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == worktree_id)
        .expect("worktree record");
    assert_eq!(record["mergedAt"], marked["worktree"]["mergedAt"]);
    assert!(
        stored["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["worktree"]["mergedAt"].is_null() || s["worktree"]["id"].is_null())
    );

    // Main-checkout sessions have no merge marker (M points at D); done is their own, per session.
    let main_one = c.create("records-main-1", "claude");
    let main_two = c.create("records-main-2", "claude");
    let main = c.main_worktree_path();
    let main_attached = c.attach("records-main-3", "claude", &c.root, &main);
    let mains = [id_of(&main_one), id_of(&main_two), id_of(&main_attached)];
    for id in &mains {
        c.wait(id, |s| s["status"] == "running");
    }
    for id in [&mains[0], &mains[2]] {
        assert_rejects(
            c.call(json!({"type": "mark-session-merged", "sessionId": id, "targetCwd": c.root})),
            &["Use Space to mark it done"],
        );
    }
    let main_done = c.ok(json!({"type": "set-session-done", "sessionId": mains[0], "done": true}));
    assert!(!main_done["doneAt"].is_null());
    assert!(main_done["mergedAt"].is_null());
    assert!(main_done["worktree"]["id"].is_null());
    let main_other = c.state(&mains[1]);
    assert!(main_other["doneAt"].is_null());
    assert!(main_other["mergedAt"].is_null());
    assert!(c.state(&mains[2])["worktree"]["id"].is_null());
    for id in &mains {
        c.kill_and_wait(id);
        c.ok(json!({"type": "remove", "sessionId": id}));
    }

    // Kill-with-delete marks the worktree deleted: every session of it loses its workspace, restart and merge.
    c.kill_and_wait(&attached_id);
    c.kill_and_wait(&child_id);
    socket.clear();
    c.ok(json!({"type": "kill", "sessionId": first_id, "deleteWorktree": true, "allowDataLoss": true}));
    let deleted_at =
        wf(|| c.state(&first_id), |s| !s["worktree"]["deletedAt"].is_null())["worktree"]["deletedAt"].clone();
    for id in [&attached_id, &child_id] {
        updated(id, &|session| session["worktree"]["deletedAt"] == deleted_at);
    }
    for id in [&attached_id, &child_id] {
        let item = c.state(id);
        assert!(workspace_key(&item).is_none());
        assert_rejects(
            c.call(json!({"type": "restart", "sessionId": id, "cols": 80, "rows": 24})),
            &["worktree was deleted"],
        );
        let terminal = c.ok(json!({"type": "watch-terminal", "sessionId": id, "cols": 80, "rows": 24}));
        assert!(terminal["workspace"].is_null());
        assert_eq!(
            workspace_pane_unavailable(&item, &terminal),
            Some("its worktree was deleted")
        );
        let changes = c.ok(json!({"type": "watch-changes", "sessionId": id}));
        assert_eq!(
            workspace_pane_unavailable(&item, &changes),
            Some("its worktree was deleted")
        );
    }
    assert_rejects(
        c.call(json!({"type": "merge-worktree", "sessionId": attached_id, "mode": "merge", "targetCwd": c.root})),
        &["worktree was deleted"],
    );
    // M can still clear a deleted worktree's marker (for all of its sessions), but not set one.
    assert!(c.ok(json!({"type": "mark-session-merged", "sessionId": attached_id, "targetCwd": c.root}))["worktree"]["mergedAt"].is_null());
    assert!(merged(&child_id).is_null());
    assert_rejects(
        c.call(json!({"type": "mark-session-merged", "sessionId": child_id, "targetCwd": c.root})),
        &["worktree was deleted"],
    );

    // A new worktree created at the same path is a new incarnation: no inherited markers, and the old sessions stay out.
    let again = c.create_mode("records-a", "claude", "new");
    let again_id = id_of(&again);
    let again_state = c.wait(&again_id, |s| s["status"] == "running");
    assert_eq!(again_state["cwd"], first_state["cwd"]);
    assert!(!again_state["worktree"]["id"].is_null());
    assert_ne!(again_state["worktree"]["id"], worktree_id);
    assert!(again_state["worktree"]["mergedAt"].is_null());
    assert!(again_state["worktree"]["deletedAt"].is_null());
    assert_eq!(workspace_key(&again_state), workspace_key(&first_state));
    for id in [&first_id, &attached_id, &child_id] {
        let item = c.state(id);
        assert_eq!(item["worktree"]["deletedAt"], deleted_at);
        assert!(workspace_key(&item).is_none());
        assert!(
            c.ok(json!({"type": "watch-terminal", "sessionId": id, "cols": 80, "rows": 24}))["workspace"].is_null()
        );
    }

    // The record lives as long as a session references it.
    c.ok(json!({"type": "remove", "sessionId": first_id}));
    c.ok(json!({"type": "remove", "sessionId": attached_id}));
    assert!(
        c.load_state()["worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == worktree_id)
    );
    c.ok(json!({"type": "remove", "sessionId": child_id}));
    let remaining = c.load_state()["worktrees"].as_array().unwrap().clone();
    assert!(!remaining.iter().any(|r| r["id"] == worktree_id));
    assert!(remaining.iter().any(|r| r["id"] == again_state["worktree"]["id"]));

    // A worktree removed outside Deckhand: when Deckhand creates one at that path again, the old incarnation is deleted.
    c.kill_and_wait(&again_id);
    c.git(&["worktree", "remove", "--force", text(&again_state["cwd"])]);
    let third = c.create_mode("records-a", "claude", "new");
    let third_id = id_of(&third);
    let third_state = c.wait(&third_id, |s| s["status"] == "running");
    assert_eq!(third_state["cwd"], again_state["cwd"]);
    assert_ne!(third_state["worktree"]["id"], again_state["worktree"]["id"]);
    let superseded = c.state(&again_id);
    assert!(!superseded["worktree"]["deletedAt"].is_null());
    assert!(workspace_key(&superseded).is_none());
    assert_eq!(workspace_key(&third_state), workspace_key(&again_state));
    c.kill_and_wait(&third_id);
}
