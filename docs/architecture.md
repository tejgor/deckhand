# Architecture

How Deckhand's processes fit together, where it keeps its state and how the code is laid out. Back to the [README](../README.md).

## Processes

Deckhand is one native binary with four long-running roles, chosen by how it is started:

1. **UI**: a [ratatui](https://ratatui.rs) terminal frontend. It sends requests to the daemon and hands your terminal to a live pane when you attach.
2. **Daemon**: owns session state, the IPC socket, worktree operations and worker supervision.
3. **Session workers**: one per running session. Each owns the agent's PTY, which runs in the session's worktree.
4. **Workspace workers**: one per worktree in use. Each owns the Terminal (shell), lazygit, Dev and action PTYs shared by every session in that worktree. It starts on demand: the first Terminal view, lazygit attach, Dev start or action.

```text
     daemon.sock         fd 3
UI  ⇄  daemon  ⇄  session workers    one per running session: the agent PTY, in its worktree
               ⇄  workspace workers  one per worktree in use: shell, lazygit, Dev, actions
```

The daemon and workers are the same executable, relaunched with an internal flag. The UI talks to the daemon over `daemon.sock`, and the daemon talks to each worker over a private channel (a socket pair on fd 3). Both carry NDJSON messages.

Workers feed terminal output into a headless terminal emulator ([`alacritty_terminal`](https://crates.io/crates/alacritty_terminal)). The UI receives rendered snapshots for previews. When you attach, your keystrokes and the pane's raw output stream through the daemon between your terminal and the selected PTY.

The Git tab's Changes view needs no worker: the daemon reads `git status` for the worktrees being watched. The `deckhand hook` bridge that agents call for [agent signals](features.md#lifecycle-signals-and-notifications) is short-lived. It forwards one event to the daemon and exits, and it never starts a daemon.

## Daemon lifecycle

The first `deckhand` launch starts the daemon. Quitting the UI with `q` leaves the daemon and every running session in place, and the next `deckhand` reconnects to it. Stopping the daemon ends all running sessions. After a daemon crash, sessions are marked interrupted but keep their conversation IDs, so `s` resumes them.

| Action | Command |
| --- | --- |
| Check whether the daemon runs | `pgrep -F ~/.deckhand/daemon.pid` |
| Follow the daemon log | `tail -f ~/.deckhand/daemon.log` |
| Stop the daemon (ends running sessions) | `kill $(cat ~/.deckhand/daemon.pid)` |

A dead PID file and a stale socket are cleaned up automatically on the next launch. Two failures need you:

- **`daemon protocol mismatch: expected vN, got M; stop the old daemon first: kill <pid>`**: the running daemon speaks another protocol version. This happens after an upgrade while the old daemon (for example the TypeScript v2 one) still runs. Deckhand refuses to replace a live daemon, so stop it with the `kill` command shown, then relaunch.
- **`daemon pid N is alive but did not respond; see …/daemon.log`**: the daemon process exists but did not answer within the retry window. Read `daemon.log`, then stop it with `kill N` and relaunch.

> [!WARNING]
> A running daemon keeps the binary it started from until it is stopped, so an upgrade takes effect only after you stop the old daemon. Stopping it ends every running session; resume them afterwards with `s`.

## State and logs

Everything lives in `~/.deckhand/` unless `DECKHAND_HOME` names another directory (the [isolated dev build](dev-build.md) uses `~/.deckhand-dev`).

| Path | Purpose |
| --- | --- |
| `~/.deckhand/state.json` | The session list (with done markers) and per-worktree merged and deleted markers |
| `~/.deckhand/config.json` | User configuration: global `defaults`, agent signals, notifications, scroll sensitivity and repository trust fingerprints |
| `~/.deckhand/ui-state.json` | Per-repository selection, tabs, tree, filter and search preferences |
| `~/.deckhand/notes/` | Notes as Markdown: `sessions/`, `worktrees/` and `repos/` (the main checkout's) |
| `~/.deckhand/notes/tasks/` | Each repository's task list, one Markdown file per repository |
| `~/.deckhand/notes/trash/` | Removed notes, named by date and what they belonged to |
| `~/.deckhand/notes.vim` | The vim keys for notes, rewritten each time a note opens in vim or nvim |
| `~/.deckhand/handoffs/` | Private, inspectable Markdown handoffs |
| `~/.deckhand/daemon.log` | Daemon diagnostics |
| `~/.deckhand/daemon.pid` | The running daemon's PID |
| `~/.deckhand/daemon.sock` | The local IPC socket |
| `~/.deckhand/workers/<session-id>.pid`, `.log` | Each session worker's PID and log |
| `~/.deckhand/workers/workspace-<hex>.pid`, `.log` | Each workspace worker's PID and log |
| `~/.deckhand/worktrees/` | The default location for new worktrees (`worktree.location` changes it) |
| `~/.deckhand/agent-sessions/pi/` | Legacy Pi session files, from before exact conversation IDs |
| `~/.deckhand/Deckhand.app` | macOS only: the notifier app bundle, rebuilt when the Deckhand binary changes |
| `~/.pi/agent/sessions/` | Pi's own session storage (Deckhand's Pi sessions live here) |

## Code layout

The Rust workspace lives in `crates/`. Lower crates never depend on higher ones: `protocol` and `core` sit at the bottom, `sys` and `term` above them, then `worker`, `daemon`, `client` and `ui`, with the `deckhand` binary on top. `core` does no I/O: anything that touches the disk, Git or a process belongs in `sys`.

| Crate | What it does |
| --- | --- |
| `crates/protocol` | The wire and on-disk shapes everyone shares: session and worktree records, client requests and responses, events, daemon-to-worker messages, NDJSON framing and the protocol version |
| `crates/core` | Pure logic with no I/O: the agents table and launch arguments, agent signals, the settings model, the sidebar and session order, notes, tasks, worktree models, merge and changes models, the text editor, key decoding and the in-app guide |
| `crates/sys` | Operating-system work: paths and `DECKHAND_HOME`, atomic storage, config files, Git calls, the notes store, UI state, agent lookup and version checks, desktop notifications and process helpers |
| `crates/term` | PTYs, UTF-8 stream handling and the headless terminal emulator with its preview rendering |
| `crates/worker` | The session worker (agent PTY, preview, attach) and the workspace worker (Terminal, lazygit, Dev, actions) |
| `crates/daemon` | The daemon: socket server and request dispatch, session lifecycle and launches, agent signal callbacks, worktrees, merging, notes and tasks, changes, config and trust, and worker supervision |
| `crates/client` | The daemon client (start-up, ping, stale socket handling, protocol-mismatch refusal), live subscriptions and attach streaming |
| `crates/ui` | The ratatui app: state and effects, the screens (create, merge, settings, notes, tasks, worktrees, agents, help), panes, keymaps and theme |
| `crates/deckhand` | The binary: command-line dispatch, the UI loop and the hand-off to attach or your editor, plus `hook`, `hooks`, `setup` and `doctor` |
| `crates/testkit` | Test-only: replays the recorded cases in `fixtures/cases`, text snapshots, shared proptest strategies |
| `crates/e2e` | Test-only: drives the built binary with a real daemon, fake agents and the dev launcher |

To build and test, see the README's Development section and [the isolated dev build](dev-build.md).
