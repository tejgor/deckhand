//! The hook bridge (`deckhand hook`), black-box: drives DECKHAND_TEST_BIN.

use std::io::{Read, Write};
use std::process::Stdio;
use std::time::Duration;

use deckhand_e2e::harness::{Sandbox, wait_exit};

// ported from tests/hookBridge.test.ts: hook bridge is bounded, always neutral, and never starts a daemon
#[test]
fn hook_bridge_is_bounded_always_neutral_and_never_starts_a_daemon() {
    let sandbox = Sandbox::new("hook-bridge");
    // The TS test points DECKHAND_HOME at an empty dir; the sandbox has other (empty) siblings, so use its home.
    let inputs: [Option<Vec<u8>>; 4] = [
        Some(b"{".to_vec()),
        Some(vec![b'x'; 100_000]),
        Some(br#"{"hook_event_name":"PermissionRequest"}"#.to_vec()),
        None,
    ];
    for input in inputs {
        let mut command = sandbox.cli(&["hook"]);
        command
            .env("DECKHAND_DEV", "0")
            .env("DECKHAND_SESSION_ID", "missing")
            .env("DECKHAND_LAUNCH_ID", "missing")
            .env("DECKHAND_HOOK_TOKEN", "missing")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = sandbox.spawn(command);
        let mut stdout = child.stdout.take().expect("stdout");
        let mut stderr = child.stderr.take().expect("stderr");
        let reader = std::thread::spawn(move || {
            let mut output = String::new();
            let _ = stdout.read_to_string(&mut output);
            output
        });
        std::thread::spawn(move || {
            let mut sink = Vec::new();
            let _ = stderr.read_to_end(&mut sink);
        });
        let mut stdin = child.stdin.take().expect("stdin");
        // A closed pipe (the bridge exits early) is fine; `None` intentionally never closes stdin.
        let held = match input {
            Some(bytes) => {
                std::thread::spawn(move || {
                    let _ = stdin.write_all(&bytes);
                });
                None
            }
            None => Some(stdin),
        };
        // The TS test's 6s budget covers all four rounds; one round gets the same.
        let status = wait_exit(&mut child, Duration::from_secs(6)).expect("hook bridge exited within 6s");
        assert_eq!(status.code(), Some(0));
        assert_eq!(reader.join().expect("stdout reader"), "{}\n");
        drop(held);
    }
    let entries: Vec<_> = std::fs::read_dir(sandbox.home()).expect("read home").collect();
    assert!(entries.is_empty(), "home must stay empty: {entries:?}");
}
