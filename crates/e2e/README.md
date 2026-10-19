# deckhand-e2e

Black-box tests for the `deckhand` executable (port of `tests/helpers.ts`). Tests drive whatever binary
`DECKHAND_TEST_BIN` names and never link the deckhand crates.

- `DECKHAND_TEST_BIN`: absolute, or relative to the workspace root. Default `target/debug/deckhand`.
- A test counts only when it passes against **both** the Rust binary and the TS shim. Create the shim by running any TS
  test once (it writes `tests/.bin/deckhand`), or write it yourself: `#!/bin/sh` + `exec node <repo>/dist/cli.js "$@"`.

```
cargo build -p deckhand && npm run build
DECKHAND_TEST_BIN=target/debug/deckhand cargo test -p deckhand-e2e
DECKHAND_TEST_BIN=tests/.bin/deckhand   cargo test -p deckhand-e2e
```

## Pieces

| Module | Port of | Use |
| --- | --- | --- |
| `client` | `src/client.ts` | `request(socket, json!({...}))` one-shot; `Client::connect` + `subscribe` + `events_of("session-updated")` for a live connection. `requestId` is added for you; errors come back as `Err(message)`. |
| `harness` | `tests/helpers.ts` | `Sandbox` (temp `DECKHAND_HOME`, `HOME`, XDG, dev home; stops daemons and kills process groups on drop), `repo()`, `git()`, `wait_for`, `bin()`, `install_agent`, `stop`. |
| `ui` | `terminalUi` | `Ui::spawn` runs a program in a 130x36 PTY (TERM=xterm-256color, SHELL=/bin/sh) read through `vt100`; `screen(text)` waits for output after the last `press()` and for the text on the rendered screen. |
| `bin/fake_agent` | `fakeAgent` | A fake claude/codex/pi. `install_agent(dir, "claude", Path::new(env!("CARGO_BIN_EXE_fake_agent")), AgentOptions::default())` copies it into `dir`. |

`Sandbox::new` refuses (panics) unless its directory is under the temp dir and not inside or around `~/.deckhand*`,
`~/.codex` or `~/.claude`. Every command built through `sandbox.command/cli` gets a private environment, hermetic Git
(`GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1`), `DECKHAND_DEV=0`, `DECKHAND_AGENT_LATEST={}` (never npm) and
`TEST_CLI` = the binary under test. Never run a real agent: the daemon's `PATH` should hold only the fakes and `/usr/bin:/bin`.

## Fake agent

Provider comes from the installed file name. `--help`, `--version` (per provider format), `update` (bumps the patch
version in `<binary>.version`, logs to `$DECKHAND_HOME/updates.log`), trace file `trace-<session>.json`, `ready-<launch>`,
input log `input-<session>`, SessionStart hooks via `$TEST_CLI hook` (codex fork reports the parent id first and last).
Variants are sidecar files: `AgentOptions { without_hooks, update_fails }`.

## Adding a suite

1. Create `crates/e2e/tests/<name>.rs` (no edits to `Cargo.toml` needed; integration tests are auto-discovered).
2. Put `// ported from tests/<file>.test.ts: <exact title>` above each `#[test]`, and port every assertion.
3. Start with `let sandbox = Sandbox::new("name");`, spawn with `sandbox.spawn_cli(&[...])` / `exec_cli`, talk to the daemon
   at `sandbox.socket()`. Declare the sandbox before clients, daemons and `Ui` so it drops last.
4. Run against both binaries (above) before committing.
