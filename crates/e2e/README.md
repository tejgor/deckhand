# deckhand-e2e

Black-box tests for the `deckhand` executable. Tests drive whatever binary
`DECKHAND_TEST_BIN` names and never link the deckhand crates.

- `DECKHAND_TEST_BIN`: absolute, or relative to the workspace root. Default `target/debug/deckhand`. The crate itself
  never needs node: the fake agents, `gh`, `lazygit` and dev/setup commands are sh scripts or the `fake_agent` binary.

```
cargo build -p deckhand
cargo test -p deckhand-e2e                                   # or: cargo nextest run -p deckhand-e2e
scripts/test.sh -E 'package(deckhand-e2e)'                   # builds first, then runs E2E (with nextest installed)
```

## Pieces

| Module | Use |
| --- | --- |
| `client` | `request(socket, json!({...}))` one-shot; `Client::connect` + `subscribe` + `events_of("session-updated")` for a live connection. `requestId` is added for you; errors come back as `Err(message)`. |
| `harness` | `Sandbox` (temp `DECKHAND_HOME`, `HOME`, XDG, dev home; stops daemons and kills process groups on drop), `repo()`, `git()`, `wait_for`, `bin()`, `launcher()`, `exec_launcher`, `stop_dev_daemon`, `install_agent`, `stop`. |
| `daemon` | `call`/`must`/`rejects`, `launch`/`spawn_daemon` (daemon with fakes on PATH) + `ready`/`wait_ping`, `subscribed`/`updated`, `create`/`running`/`state`, `install_agents`, `install_script`/`reinstall`, `write_json`, `load_state`, small parse helpers. |
| `fakes` | `install_sh_agents(dir, ready_line)`: claude/pi/codex sh stand-ins with a custom ready line (e.g. echoing `NODE_ENV`), plus sh snippets (`SH_JSON_ARGS`, `MISSING_CONVERSATION_AGENT`). |
| `tasks` | `parse_tasks`, `tasks_of`, `meta_of`: the tasks file as data. |
| `ui` | `Ui::spawn` runs a program in a 130x36 PTY (TERM=xterm-256color, SHELL=/bin/sh) read through `vt100`; `screen(text)` waits for output after the last `press()` and for the text on the rendered screen. `launch_sandbox_ui(sandbox, fake_bin, env)` / `spawn_launcher` run `scripts/deckhand-dev.sh` (executable; never via `sh`) in it. |
| `client::Raw` | A connection that keeps every message, sends without `requestId` (`send`) and can `clear()` what it has seen. |
| `bin/fake_agent` | A fake claude/codex/pi. `install_agent(dir, "claude", Path::new(env!("CARGO_BIN_EXE_fake_agent")), AgentOptions::default())` copies it into `dir`. |

`Sandbox::new` refuses (panics) unless its directory (`$TMPDIR/dh-XXXXXX`, a short prefix on purpose) is under the temp dir
and not inside or around `~/.deckhand*`, `~/.codex` or `~/.claude`, and unless the daemon socket paths fit the unix
`sun_path` limit (the message says to shorten `TMPDIR`). Pass `fake_agent!()` where a helper needs the fake agent binary. Every command built through `sandbox.command/cli` gets a private environment, hermetic Git
(`GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1`), `DECKHAND_DEV=0`, `DECKHAND_AGENT_LATEST={}` (never npm) and
`TEST_CLI` = the binary under test. Never run a real agent: the daemon's `PATH` should hold only the fakes and `/usr/bin:/bin`.

## Fake agent

Provider comes from the installed file name. `--help`, `--version` (per provider format), `update` (bumps the patch
version in `<binary>.version`, logs to `$DECKHAND_HOME/updates.log`), trace file `trace-<session>.json`, `ready-<launch>`,
input log `input-<session>`, SessionStart hooks via `$TEST_CLI hook` (codex fork reports the parent id first and last).
Variants are sidecar files: `AgentOptions { without_hooks, update_fails }`.

## Adding a suite

1. Create `crates/e2e/tests/<name>.rs` (no edits to `Cargo.toml` needed; integration tests are auto-discovered).
2. Name each test for the user behavior it protects and assert its meaningful results.
3. Start with `let sandbox = Sandbox::new("name");`, spawn with `sandbox.spawn_cli(&[...])` / `exec_cli`, talk to the daemon
   at `sandbox.socket()`. Declare the sandbox before clients, daemons and `Ui` so it drops last.
4. Rebuild the executable and run the suite without `DECKHAND_BLESS`. Keep persistence, process cleanup and trust checks as ordinary assertions.
