//! Daemon-suite plumbing shared by the black-box tests: one-shot requests, launching `deckhand --daemon` with fake agents
//! on PATH, waiting for it to answer `ping`, subscriptions, session lookups and small JSON/file helpers.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

use crate::client::Client;
use crate::harness::{AgentOptions, Sandbox, install_agent, sidecar, wait_for, wait_for_default};

/// helpers.ts `request(..., 60_000)`: one connection per call.
pub fn call(socket: &Path, message: Value) -> Result<Value, String> {
    Client::connect(socket)
        .map_err(|e| e.to_string())?
        .request_timeout(message, Duration::from_secs(60))
}

pub fn must(socket: &Path, message: Value) -> Value {
    let shown = message.to_string();
    call(socket, message).unwrap_or_else(|e| panic!("request {shown} failed: {e}"))
}

/// `assert.rejects(promise, /needle/)`.
pub fn rejects(result: Result<Value, String>, needle: &str) {
    match result {
        Ok(value) => panic!("expected a rejection matching {needle:?}, got {value}"),
        Err(error) => assert!(error.contains(needle), "{error:?} does not contain {needle:?}"),
    }
}

/// The fake agents (claude, pi, codex) in `<sandbox>/bin`; returns that directory. `fake` is `fake_agent!()`.
pub fn install_agents(sandbox: &Sandbox, fake: &Path) -> PathBuf {
    let bin = sandbox.root().join("bin");
    for provider in ["claude", "pi", "codex"] {
        install_agent(&bin, provider, fake, AgentOptions::default());
    }
    bin
}

/// Starts `deckhand --daemon` in `cwd` (stdio discarded) with `PATH` and extra environment variables. Does not wait.
pub fn spawn_daemon(sandbox: &Sandbox, cwd: &Path, path: &str, vars: &[(&str, &OsStr)]) -> Child {
    let mut command: Command = sandbox.cli(&["--daemon"]);
    command
        .current_dir(cwd)
        .env("PATH", path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (key, value) in vars {
        command.env(key, value);
    }
    sandbox.spawn(command)
}

/// `deckhand --daemon` in `root` with the fakes first on a PATH that holds nothing else but the system directories.
pub fn launch(sandbox: &Sandbox, root: &Path, bin: &Path) -> Child {
    spawn_daemon(sandbox, root, &format!("{}:/usr/bin:/bin", bin.display()), &[])
}

/// `bin` first on the inherited PATH.
pub fn path_with(bin: &Path) -> String {
    format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default())
}

/// Waits until a `ping` sent through `call` answers `ok`.
pub fn wait_ping(call: impl Fn(Value) -> Result<Value, String>, timeout: Duration) {
    wait_for(
        || call(json!({"type": "ping"})).map(|v| v["ok"] == true).unwrap_or(false),
        |ok| *ok,
        timeout,
    );
}

/// Waits (8s) until the daemon at `socket` answers `ping`.
pub fn ready(socket: &Path) {
    wait_ping(|message| call(socket, message), Duration::from_millis(8000));
}

/// `wait_for` with the 15s the long daemon flows use.
pub fn wf<T: std::fmt::Debug>(read: impl FnMut() -> T, ok: impl Fn(&T) -> bool) -> T {
    wait_for(read, ok, Duration::from_secs(15))
}

pub fn id_of(value: &Value) -> String {
    value["id"].as_str().unwrap_or_default().to_string()
}

pub fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

/// `assert.rejects(promise, /a.*b/)`: the daemon's error must contain every needle, in order.
pub fn assert_rejects(result: Result<Value, String>, needles: &[&str]) {
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

pub fn strings(value: &Value) -> Vec<String> {
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

pub fn write_atomic(path: &Path, bytes: &[u8]) {
    let temporary = path.with_extension("tmp-write");
    fs::write(&temporary, bytes).expect("write temp");
    fs::rename(&temporary, path).expect("rename");
}

pub fn write_json(path: &Path, value: &Value) {
    write_atomic(
        path,
        format!("{}\n", serde_json::to_string_pretty(value).expect("json")).as_bytes(),
    );
}

/// Writes an executable script as a new file (rewriting a signed binary in place can get it killed on macOS).
pub fn install_script(path: &Path, script: &str) {
    let _ = fs::remove_file(path);
    fs::write(path, script).expect("write script");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod");
}

/// Reinstalls the fake agent (`fake` = `fake_agent!()`), dropping the sidecars of the variant it replaces.
pub fn reinstall(bin: &Path, provider: &str, fake: &Path, options: AgentOptions) {
    let target = bin.join(provider);
    for suffix in ["nohooks", "update-fails"] {
        let _ = fs::remove_file(sidecar(&target, suffix));
    }
    let _ = fs::remove_file(&target);
    install_agent(bin, provider, fake, options);
}

pub fn exists(path: impl AsRef<Path>) -> bool {
    fs::symlink_metadata(path.as_ref()).is_ok()
}

/// A persistent connection subscribed to `root` (the TS tests' raw socket + `events`).
pub fn subscribed(socket: &Path, root: &str) -> Client {
    let mut live = Client::connect(socket).expect("live client");
    live.subscribe(root).expect("subscribe");
    live
}

/// `updated(id, matches)`: waits for a `session-updated` event after the first `from` events.
pub fn updated(live: &Client, from: usize, id: &str, matches: impl Fn(&Value) -> bool) {
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

pub fn state(socket: &Path, id: &str) -> Value {
    must(socket, json!({"type": "list"}))
        .as_array()
        .and_then(|items| items.iter().find(|item| item["id"] == id).cloned())
        .unwrap_or(Value::Null)
}

pub fn try_create(socket: &Path, root: &str, title: &str, mode: &str, extra: Value) -> Result<Value, String> {
    let mut input = json!({"title": title, "program": "claude", "cwd": root, "repoRoot": root, "cols": 80, "rows": 24, "worktreeMode": mode});
    for (key, value) in extra.as_object().into_iter().flatten() {
        input[key] = value.clone();
    }
    call(socket, json!({"type": "create", "input": input}))
}

pub fn create(socket: &Path, root: &str, title: &str, mode: &str, extra: Value) -> Value {
    try_create(socket, root, title, mode, extra).unwrap_or_else(|e| panic!("create {title}: {e}"))
}

/// Waits for the session to run; one that exits instead will never run, so this fails at once with its record (its
/// `lastPreview` says why).
pub fn running(socket: &Path, id: &str) -> Value {
    let item = wait_for_default(
        || state(socket, id),
        |item| item["status"] == "running" || item["status"] == "exited",
    );
    assert_eq!(item["status"], "running", "session exited instead of running: {item}");
    item
}

/// Git that may fail: (success, trimmed-end stdout).
pub fn git_try(cwd: &Path, args: &[&str]) -> (bool, String) {
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

pub fn p(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub fn text_of(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

/// The persisted `state.json` (`loadState`).
pub fn load_state(home: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(home.join("state.json")).expect("state.json")).expect("state json")
}

pub fn pause(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// Falsy in the JS sense for the fields the tests check (`undefined`, `false`).
pub fn falsy(value: &Value) -> bool {
    value.is_null() || *value == json!(false)
}
