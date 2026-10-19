//! Port of the helpers.ts `fakeAgent` script: a fake claude/codex/pi.
//!
//! Install copies of this binary named `claude`/`codex`/`pi` (`harness::install_agent`); the provider is the file name.
//! It records its argv and input under `$DECKHAND_HOME` and reports SessionStart through `$TEST_CLI hook`. `--version`
//! prints the version in the agent's own format; its update command (`claude update`, `codex update`, `pi update
//! --self`) bumps the patch version and logs the call to `updates.log`. TS rewrote its own source for that; here the
//! "self rewrite" is `<binary>.version` next to the binary. Sidecars `<binary>.nohooks` (never reports hooks) and
//! `<binary>.update-fails` (the updater fails with an npm error) select the variants.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Map, Value, json};

const DEFAULT_VERSION: &str = "1.0.0";

fn sidecar(binary: &Path, suffix: &str) -> PathBuf {
    let mut name = binary.as_os_str().to_owned();
    name.push(".");
    name.push(suffix);
    PathBuf::from(name)
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn home_file(name: &str) -> PathBuf {
    PathBuf::from(env("DECKHAND_HOME").unwrap_or_default()).join(name)
}

/// JS template-literal behaviour for an unset variable, since the TS names its files with `undefined`.
fn or_undefined(name: &str) -> String {
    env(name).unwrap_or_else(|| "undefined".into())
}

fn append(file: &Path, line: &str) {
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(file) {
        let _ = f.write_all(line.as_bytes());
    }
}

fn hook(event: &str, session_id: &str) {
    let Some(cli) = env("TEST_CLI") else { return };
    let Ok(mut child) = Command::new(cli)
        .arg("hook")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(
            json!({"hook_event_name": event, "session_id": session_id})
                .to_string()
                .as_bytes(),
        );
    }
    let _ = child.wait();
}

fn flag(args: &[String], name: &str) -> Option<String> {
    let at = args.iter().position(|a| a == name)?;
    args.get(at + 1).cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let binary = std::env::current_exe()
        .ok()
        .or_else(|| std::env::args().next().map(PathBuf::from))
        .unwrap_or_default();
    let provider = binary
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let version = fs::read_to_string(sidecar(&binary, "version"))
        .map(|v| v.trim().to_string())
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_VERSION.into());

    if args.iter().any(|a| a == "--help") {
        println!("PROMPT --settings --no-daemon resume");
        return;
    }
    if args.first().map(String::as_str) == Some("--version") {
        match provider.as_str() {
            "claude" => println!("{version} (Claude Code)"),
            "codex" => println!("codex-cli {version}"),
            _ => println!("{version}"),
        }
        return;
    }
    if args.first().map(String::as_str) == Some("update") {
        update(&binary, &provider, &args, &version);
    }

    // `codex fork <parent>` reports its own ID, but first and last also the parent's (a child must never adopt it).
    let fork_of = (provider == "codex" && args.first().map(String::as_str) == Some("fork"))
        .then(|| args.get(1).cloned())
        .flatten();
    let id = if fork_of.is_some() {
        "fixture-child-codex".to_string()
    } else if provider == "claude" {
        flag(&args, "--session-id")
            .or_else(|| flag(&args, "--resume"))
            .unwrap_or_else(|| "fixture-native-claude".into())
    } else {
        format!("fixture-native-{provider}")
    };

    let mut trace = Map::new();
    trace.insert("args".into(), json!(args));
    if let Some(token) = env("DECKHAND_HOOK_TOKEN") {
        trace.insert("token".into(), json!(token));
    }
    if let Some(launch) = env("DECKHAND_LAUNCH_ID") {
        trace.insert("launchId".into(), json!(launch));
    }
    trace.insert("pid".into(), json!(std::process::id()));
    let _ = fs::write(
        home_file(&format!("trace-{}.json", or_undefined("DECKHAND_SESSION_ID"))),
        Value::Object(trace).to_string(),
    );

    // Hooks run beside the input loop, as TS's async hook spawns did.
    let hooks = !sidecar(&binary, "nohooks").exists();
    std::thread::spawn(move || {
        if hooks {
            match &fork_of {
                Some(parent) => {
                    hook("SessionStart", parent);
                    hook("SessionStart", &id);
                    hook("SessionStart", parent);
                }
                None => hook("SessionStart", &id),
            }
        }
        let _ = fs::write(home_file(&format!("ready-{}", or_undefined("DECKHAND_LAUNCH_ID"))), "");
        println!("ready");
        let _ = std::io::stdout().flush();
    });

    raw_mode();
    let input = home_file(&format!("input-{}", or_undefined("DECKHAND_SESSION_ID")));
    let mut stdin = std::io::stdin();
    let mut buffer = [0u8; 4096];
    loop {
        match stdin.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&input) {
                    let _ = f.write_all(&buffer[..n]);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    // Node keeps running after stdin ends (the interval timer); so does this.
    loop {
        std::thread::sleep(std::time::Duration::from_secs(10));
    }
}

/// `process.stdin.setRawMode(true)` when stdin is a terminal.
fn raw_mode() {
    let stdin = std::io::stdin();
    if let Ok(mut term) = nix::sys::termios::tcgetattr(&stdin) {
        nix::sys::termios::cfmakeraw(&mut term);
        let _ = nix::sys::termios::tcsetattr(&stdin, nix::sys::termios::SetArg::TCSANOW, &term);
    }
}

fn update(binary: &Path, provider: &str, args: &[String], version: &str) -> ! {
    let log = home_file("updates.log");
    if sidecar(binary, "update-fails").exists() {
        append(&log, &format!("{}\n", json!({"provider": provider, "args": args})));
        println!("Updating {provider}...");
        eprintln!("npm error code EACCES");
        std::process::exit(1);
    }
    let mut parts = version.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    let (major, minor, patch) = (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    );
    let next = format!("{major}.{minor}.{}", patch + 1);
    let _ = fs::write(sidecar(binary, "version"), &next);
    let stdin_kind = match nix::sys::stat::fstat(0).map(|st| st.st_mode & libc::S_IFMT) {
        Ok(libc::S_IFIFO) => "pipe",
        Ok(libc::S_IFCHR) => "tty-or-null",
        _ => "other",
    };
    append(
        &log,
        &format!("{}\n", json!({"provider": provider, "args": args, "stdin": stdin_kind})),
    );
    println!("Updating {provider}...");
    println!("Updated {version} -> {next}");
    std::process::exit(0);
}
