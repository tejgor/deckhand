# ⚓ Deckhand

[![npm version](https://img.shields.io/npm/v/@tejgor/deckhand.svg)](https://www.npmjs.com/package/@tejgor/deckhand)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Node.js Version](https://img.shields.io/node/v/@tejgor/deckhand.svg)](https://nodejs.org)

**A lightweight agent workbench for your IDE terminal.**

> **Status:** 🧪 Early/Experimental. Behavior, on-disk state, and the daemon IPC protocol may change between versions.

https://github.com/user-attachments/assets/89ceed64-18c0-4006-bd9e-7500204ca02a

---

## 📖 Table of Contents

- [Why Deckhand?](#-why-deckhand)
- [Features](#-features)
- [Requirements](#-requirements)
- [Installation](#-installation)
- [Quick Start](#-quick-start)
- [Controls](#-controls)
- [Sessions & Workspaces](#-sessions--workspaces)
- [Configuration](#-configuration)
- [Worktree Hooks](#-worktree-hooks)
- [Architecture](#-architecture)
- [Development](#-development)
- [Troubleshooting](#-troubleshooting)

---

## 🤔 Why Deckhand?

Tools for running coding agents in parallel typically rely on [`tmux`](https://github.com/tmux/tmux) to host isolated agent sessions, paired with git worktrees for branch isolation. That works well in a dedicated terminal, but `tmux` fits awkwardly inside an IDE's integrated terminal, where prefix-key collisions, nested key handling, and resize quirks get in the way.

**Deckhand** targets the integrated terminal and drops the `tmux` dependency. It runs as a single program built on an [Ink](https://github.com/vadimdemedes/ink) UI, a local daemon, and [`node-pty`](https://github.com/microsoft/node-pty) workers, behaving consistently in whatever terminal it is launched from.

> *For `tmux`-based alternatives, see [claude-squad](https://github.com/smtg-ai/claude-squad) and [agent-deck](https://github.com/asheshgoplani/agent-deck).*

---

## ✨ Features

- **Split View** — A numbered session sidebar beside Preview, Terminal, Git, Dev, and Notes tabs.
- **Live Previews** — Watch a session's output without attaching to it, with read-only preview focus/scrolling.
- **Persistent Sessions** — The daemon owns sessions, so they survive UI quits and UI crashes. Daemon crashes preserve conversation references, not live processes.
- **Keyboard Reordering** — Move sessions up and down among their siblings from the keyboard.
- **Sub-sessions** — Group related work under a parent session, indented in the sidebar; each one starts clean in the parent's directory, or forks the parent's Claude/Pi conversation.
- **Resumable Agents** — Claude/Pi retain native identities; Codex resumes when its native ID is captured. Unknown IDs never silently become a blank conversation. Fresh restart remains explicit.
- **Per-session Notes** — Keep persisted scratch notes alongside each session.
- **Safer Cleanup** — Check uncommitted, untracked and valuable ignored files, and commits that deleting a branch would lose, before deletion; force kill and data-loss authorization are separate.
- **Merge Helpers** — Merge or squash-merge a session's worktree into the current branch, staged for review rather than committed.
- **Optional Tabs** — A Git tab powered by `lazygit`, and a configurable Dev tab for a command such as `npm run dev`.
- **Trusted Project Actions** — Global defaults plus an optional per-repository `deckhand.json` (defaults, setup, Dev command, named actions, creation hook), reviewed inline before any repository command runs.
- **Organization & Visibility** — Persistent archive/search/filter/tree preferences, local Git summaries, and explicit optional PR/check lookups.
- **Handoffs & Attention** — Inspectable Markdown context for clean children and capability-gated lifecycle signals/desktop notifications.

See [the feature guide](docs/no-brainers.md) and [isolated dev testing](docs/dev-build.md).

---

## 📋 Requirements

- **Node.js**: `>= 20`
- **OS**: macOS or Linux with a POSIX shell
- **Git**: `git` on `PATH`
- **Agents**: `claude`, `pi`, and/or `codex` on `PATH` — whichever agents you plan to run
- **Optional**: [`lazygit`](https://github.com/jesseduffield/lazygit) on `PATH` for the Git tab

---

## 🚀 Installation

Deckhand requires Node.js 20 or newer. If you do not have Node installed, install the latest recommended version from the [Node.js website](https://nodejs.org/en/download/).

Install Deckhand globally from npm:

```bash
npm install -g @tejgor/deckhand
deckhand
```

### First-Time Setup

For an easier first-time setup, Deckhand can check for missing agents and offer to install them:

```bash
deckhand setup
```

The setup helper installs:
- **Claude Code**: `curl -fsSL https://claude.ai/install.sh | bash`
- **Pi**: `npm install -g --ignore-scripts @earendil-works/pi-coding-agent`
- **Codex**: `npm install -g @openai/codex`

> Use `deckhand setup --check` for a read-only check, or `deckhand setup --yes` to accept the agent install prompts automatically.

### Optional: Lazygit

`lazygit` is optional and is not installed by `deckhand setup`. If you want the Git tab, install it separately:

```bash
# macOS with Homebrew
brew install lazygit

# Ubuntu/Debian
sudo apt-get update && sudo apt-get install -y lazygit
```

See the [lazygit installation docs](https://github.com/jesseduffield/lazygit#installation) for other platforms.

---

## 🏃 Quick Start

Run `deckhand` from inside a git repository, then create a session:

1. Press `n` for a top-level session.
2. Choose `claude`, `pi`, or `codex`.
3. Enter a session name.
4. Press `tab` to pick a workspace mode — no worktree, new worktree, or existing worktree.
5. Press `enter` to launch.

Press `o` to attach to the selected session's active pane. To branch off related work, select a session and press `N` for a sub-session.

---

## ⌨️ Controls

### Main View

| Key | Action |
| --- | --- |
| `n` | New top-level session |
| `N` | New sub-session under the selected session |
| `1`–`9`, `0` | Jump to that numbered visible session (`0` selects visible session 10). Multi-digit jumps (e.g. `12`) work via brief buffering. |
| `j` / `k` | Move between visible sessions |
| `J` / `K` | Move the selected session down / up among its siblings (order is persisted) |
| `c` | Collapse or expand the selected session's sub-sessions in the sidebar |
| `tab` | Cycle the Preview / Terminal / Git / Dev / Notes tabs |
| `p` / `t` / `g` / `d` / `a` | Jump directly to Preview / Terminal / Git / Dev / Notes |
| `d` *(on Dev tab)* | Start or stop the selected session's Dev command |
| `o` | Attach to the selected session (opens current tab); on Notes, enter edit mode |
| `O` | Open the selected session directory/worktree in Cursor (if available) or VS Code |
| `v` *(on Preview tab)* | Focus preview scrolling (`j`/`k` scroll, `g`/`G` jump, `esc` exits focus) |
| `esc` *(in Notes)* | Stop editing notes |
| `[` / `]` | Decrease / increase the scroll multiplier and save it to config |
| `m` | Merge the selected worktree into the current branch |
| `M` | Toggle the merged/pushed marker for the selected session |
| `h` / `l` | Resize the sidebar |
| `x` / `X` | Kill the selected running session / force kill |
| `s` / `S` | Resume / fresh-restart the selected exited session |
| `backspace` | Drop the selected exited session from the list |
| `r` | Refresh the session list |
| `A` | Archive/unarchive (does not stop an agent) |
| `f` / `/` | Cycle filters / search title, notes, provider, branch, path |
| `i` | Workspace Git summary; `P` queries PR, `b` opens it, `c` pushes and opens GitHub's new-PR form (after confirmation), `g` opens lazygit |
| `C` | Edit global defaults or the repository's `deckhand.json` (main checkout); Ctrl+S saves |
| `T` | Review the repository config/hook; Enter trusts the exact contents (also shown inline by `n`, `e`, Dev and setup retry when needed) |
| `e` | Choose an action (global, plus trusted repository actions) for the shared Dev pane |
| `H` / `F` | Export/open handoff (notes plus commits and changed files, no diff content) / create a clean child from the reviewed document |
| `!` | Next known attention session |
| `x` / `X` *(while starting)* | Cancel startup/setup, retaining its worktree |
| `?` | Scrollable help and workflow guide; j/k/arrows/Page keys scroll, Home/End jump |
| `q` | Quit the UI; running sessions continue in the daemon |

> *Deletion is conservative: unknown/unsafe Git state requires typing `DELETE`. `X` does not authorize data loss. Main/current/actively shared worktree protections cannot be overridden.*

### Attach Mode

| Key | Action |
| --- | --- |
| *(most keys)* | Sent directly to the attached pane/session |
| `Ctrl+Space` | Detach and return to Deckhand |
| `Ctrl+]` | Detach and return to Deckhand |

---

## 📂 Sessions & Workspaces

### Workspace Modes

When you create a session, Deckhand launches it in one of three workspace modes:

| Mode | Behavior |
| --- | --- |
| **No worktree** | Runs in the current repository directory |
| **New worktree** | Creates or resolves a worktree for the session |
| **Existing worktree** | Picks from existing git worktrees, including the current/main one |

A sub-session defaults to its parent's current directory, so a clean sub-session opens in the parent's worktree unless you choose a different mode.

New worktrees use an explicitly trusted [project hook](#-worktree-hooks); otherwise Deckhand falls back to `git worktree add` at the configured `worktree.location` (default: the active state directory's `worktrees/`, normally `~/.deckhand/worktrees/`). The `worktree` setting can also choose the new branch's start point (current checkout, default branch, or a freshly fetched `origin/<default>`) and name template, switch the hook off, and symlink heavy directories (such as `node_modules` or a virtualenv) and private files into each new worktree. **C → Worktree setup** edits all of this with presets and link suggestions from your checkout, and **C → Effective settings** shows what is in effect and which layer sets it — see [worktree settings](docs/no-brainers.md#worktree-settings).

### Sub-sessions

Press `N` on a selected session to create a sub-session for related follow-up work. Sub-sessions render indented under their parent in the sidebar; press `c` on a parent to collapse or expand its subtree.

- Choosing `claude`, `pi`, or `codex` creates a **clean** sub-session — a fresh agent context in the parent's directory or worktree.
- For Claude and Pi parents, choosing **`Fork parent`** forks the parent's conversation: Claude resumes it and sends `/branch`; Pi launches with `--fork`. *(Claude's branch input includes an insert-mode safeguard for users with vim mode enabled.)*

### Agent Identity and Restarts

Claude and Pi sessions get an exact conversation ID (a UUID) chosen by Deckhand at launch, plus a readable label built from the session name and a short, immutable Deckhand id: `dh-{sanitized-session-name}-{short-id}`.

| Agent | Create | Restart | Forked sub-sessions |
| --- | --- | --- | --- |
| **Claude** | `claude --session-id <uuid> --name dh-{name}-{short-id}` | `claude --resume <uuid>`; an unknown ID asks for `S`, which creates a fresh ID | Resume parent, then send `/branch dh-{name}-{short-id}` |
| **Pi** | `pi --session-id <uuid> --name dh-{name}-{short-id}` | Same `--session-id`; `S` creates a fresh ID | `pi --fork <parent> --session-id <child-uuid>` |
| **Codex** | Normal launch; capture native ID via supported hook/exit hint | `codex resume <id>` when known; otherwise explicit `S` required | Not supported yet |

<details>
<summary><strong>More details on Agent Identity</strong></summary>

Pi session files live in Pi's normal session tree at `~/.pi/agent/sessions/`, not under `~/.deckhand`. They therefore stay visible in Pi's own `/resume` UI (under the readable label) and survive deletion of Deckhand state. Sessions recorded before exact IDs keep resuming by their stored name (Claude) or `--session` path (Pi).

- Claude prints a `claude --resume "..."` command when it exits; Deckhand parses that final preview and persists the parsed handle when available. If Claude reports `No conversation found with session ID`, Deckhand shows a hint to press `S` instead of starting fresh silently.
- `S` fresh-restarts an exited session without using the prior resume handle.
- Forked sub-sessions store the parent agent reference. Claude children send `/branch` at startup; Pi children are created by `pi --fork` before the TUI starts, so nothing is typed into Pi.

</details>

---

## ⚙️ Configuration

Deckhand reads configuration from `~/.deckhand/config.json`. `DECKHAND_HOME` selects a separate state namespace; the [isolated dev launcher](docs/dev-build.md) safely manages this for preview testing.

### Project Configuration

Session defaults, a Dev command, a setup command and layout/symlinks for new worktrees, and named actions come from two layers, both edited with **C**: global `defaults` in your user `config.json` (never need trust), overridden per repository by one `deckhand.json` in the main checkout. The repository file applies only once you trust its exact contents; Deckhand asks inline (Enter trusts, `s` continues with global defaults only) when you create a session, open actions, start Dev or retry setup — see [project configuration, trust and cleanup](docs/no-brainers.md) for the full behaviour.

Optional `agent_hooks` and `notifications` in user config enable capability-gated lifecycle integration and best-effort desktop notifications. They default off; native approvals stay in agent terminals. A response ending is **not task success**.

### Dev Command

Focus the Dev tab with `d`, then press `d` again while it is focused to start or stop the command. Set the command globally (or per repository as `devCommand` in `deckhand.json`):

```json
{
  "defaults": {"devCommand": "npm run dev"}
}
```

The older top-level `dev_command` still works when no `devCommand` is set.

### Notes

The Notes tab stores per-session text in `~/.deckhand/state.json`. Select Notes with `a` or by cycling tabs, press `o` to edit, and press `esc` to leave notes edit mode. Notes autosave while you type.

### Attach Scroll Sensitivity

Attached sessions and Preview focus dampen trackpad and mouse-wheel scrolling. Press `[` / `]` in Deckhand to decrease/increase the multiplier immediately and save it, or edit the config directly:

```json
{
  "attach_scroll_sensitivity": 0.12
}
```

Use `1` for normal terminal scrolling, lower values for slower scrolling, or `0` to ignore vertical wheel events while attached/in Preview focus. Default: `0.12`.

### State and Logs

| Path | Purpose |
| --- | --- |
| `~/.deckhand/state.json` | Persisted session list |
| `~/.deckhand/config.json` | User configuration, global `defaults` and exact repository trust fingerprints |
| `~/.deckhand/ui-state.json` | Per-repository selection, tabs, width, tree/filter/search preferences |
| `~/.deckhand/handoffs/` | Private, inspectable Markdown handoffs |
| `~/.deckhand/daemon.log` | Supervisor daemon diagnostics |
| `~/.deckhand/daemon.pid` | Active supervisor daemon PID |
| `~/.deckhand/daemon.sock` | Local IPC socket |
| `~/.deckhand/workers/` | Per-session worker PID and log files |
| `~/.deckhand/worktrees/` | Default location for auto-created worktrees (`worktree.location` changes it) |
| `~/.pi/agent/sessions/` | Pi's normal session storage |

---

## 🪝 Worktree Hooks

For new-worktree sessions, Deckhand creates or resolves a git worktree and then starts the agent inside it. Most layouts (location, shared dependency directories, env files) are covered by the declarative [`worktree` setting](docs/no-brainers.md#worktree-settings); the hook is the escape hatch for anything else. It uses this project hook only after its exact contents are reviewed and trusted (together with the repository's `deckhand.json`), and otherwise falls back to `git worktree add`:

```text
.claude/scripts/create-worktree.sh
```

Set `"worktree": {"hook": false}` (or press **h** in **C → Worktree setup**) to ignore the script entirely: it is then never run or reviewed. A repository's `deckhand.json` can switch it off even before it is trusted, since that only prevents execution.

### Hook Contract

- Read JSON from `stdin`.
- Use `name` as the sanitized worktree/session name.
- Use `cwd` as the directory where `deckhand` was launched.
- Create or register a git worktree.
- Print the absolute worktree path to `stdout` as the final non-empty line.
- Exit `0` on success.

> Deckhand also sets `CLAUDE_PROJECT_DIR` to the launch cwd, for compatibility with Claude-style hooks.
>
> Deckhand runs the reviewed bytes with `bash -c`, so `$0` is the script path but `${BASH_SOURCE[0]}` is empty; locate sibling files with `$0`.

<details>
<summary><strong>Minimal hook example</strong></summary>

```bash
#!/bin/bash
set -e

INPUT="$(cat)"
NAME="$(echo "$INPUT" | jq -r '.name // "worktree"')"
CWD="$(echo "$INPUT" | jq -r '.cwd // env.CLAUDE_PROJECT_DIR // env.PWD')"
DIR="${DECKHAND_HOME:-$HOME/.deckhand}/worktrees/$NAME"
START="$(git -C "$CWD" rev-parse HEAD)"

if [ -d "$DIR/.git" ] || git -C "$DIR" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "$DIR"
  exit 0
fi

if [ -e "$DIR" ]; then
  echo "Path exists but is not a git worktree: $DIR" >&2
  exit 1
fi

if git -C "$CWD" show-ref --verify --quiet "refs/heads/$NAME"; then
  git -C "$CWD" worktree add "$DIR" "$NAME" >&2
else
  git -C "$CWD" worktree add -b "$NAME" "$DIR" "$START" >&2
fi

echo "$DIR"
```

</details>

---

## 🏗️ Architecture

Deckhand has three main pieces:

1. **Ink Frontend** — renders the terminal UI, sends requests to the daemon, and attaches to live panes on request.
2. **Local Daemon** — owns session state, IPC, worktree operations, and worker supervision.
3. **Session Workers** — one per running session; each owns the agent PTY plus optional Terminal, Git, and Dev PTYs.

Terminal output is fed into a headless [`xterm.js`](https://github.com/xtermjs/xterm.js) model. The UI receives rendered snapshots for previews, while attach mode streams input and output directly between your terminal and the selected PTY.

### Daemon Lifecycle

Deckhand spawns a long-lived supervisor daemon the first time you launch the UI. Quitting with `q` leaves the daemon — and any running sessions — in place; relaunching `deckhand` reattaches. Stopping the daemon kills all running sessions.

| Action | Command |
| --- | --- |
| Check if daemon is running | `pgrep -F ~/.deckhand/daemon.pid` |
| Tail daemon logs | `tail -f ~/.deckhand/daemon.log` |
| Stop the daemon | `kill $(cat ~/.deckhand/daemon.pid)` |
| Recover crashed daemon | `rm ~/.deckhand/daemon.pid ~/.deckhand/daemon.sock` then relaunch |

---

## 🛠️ Development

**Preview safely alongside a production daemon:** `npm start` builds and opens the isolated dev build in a disposable sandbox; `npm stop`, `npm restart` and `npm run status` control only that dev daemon. No global link/install or production restart. See [the testing checklist](docs/dev-build.md).

For ordinary local development, install from source. The plain `dev`/`daemon` commands below share production state unless you explicitly configure isolation:

```bash
git clone https://github.com/tejgor/deckhand.git
cd deckhand
npm install
npm run dev      # run from source via tsx
npm run daemon   # run only the daemon in dev mode
npm run build    # compile to dist/
npm link         # link globally
```

After changing source code, rebuild with `npm run build` before re-running the linked CLI.

> **macOS note:** `npm install` runs `scripts/fix-node-pty.js`, which attempts to repair the `node-pty` `spawn-helper` binary. See [Troubleshooting](#-troubleshooting) if install fails.

---

## 🚑 Troubleshooting

- **`deckhand` can't find an agent:** Confirm the binary is on `PATH` with `which claude`, `which pi`, or `which codex`. Deckhand inherits the launching shell's environment.
- **`node-pty` fails to load on macOS:** Re-run the repair script directly: `node scripts/fix-node-pty.js`. If that doesn't help, reinstall: `rm -rf node_modules && npm install`.
- **Stale daemon socket or PID:** If `deckhand` hangs at startup, the supervisor may have exited uncleanly. Remove stale files: `rm -f ~/.deckhand/daemon.pid ~/.deckhand/daemon.sock` and relaunch.
- **Git tab is empty:** Install [`lazygit`](https://github.com/jesseduffield/lazygit) and ensure it is on `PATH`.
- **Dev tab does nothing:** Press `d` once to focus the Dev tab, then press `d` again to start/stop the command. Ensure a `devCommand` is set in global defaults (**C**) or a trusted `deckhand.json`.

---

## 🗑️ Uninstall

```bash
npm uninstall -g @tejgor/deckhand
```

To remove all local state (sessions, logs, and auto-created worktrees):

```bash
rm -rf ~/.deckhand
```

> *If Deckhand created git worktrees under `~/.deckhand/worktrees/`, remove them through the UI (or with `git worktree remove`) before deleting the directory, so git's bookkeeping stays consistent.*

---

## 🤝 Contributing

Issues and pull requests are welcome. For larger changes, please open an issue first to discuss the approach. Run `npm test` and confirm the isolated CLI launches before sending a PR.

## 📄 License

Deckhand is released under the [MIT License](LICENSE).
