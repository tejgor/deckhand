# Deckhand

A lightweight agent workbench for your IDE terminal.

> **Status:** early/experimental. Behavior, on-disk state, and the daemon IPC protocol may change between versions.

- Start `claude`, `pi`, or `codex` sessions from one UI
- Preview output without attaching to every session
- Attach/detach while sessions keep running in the background
- Restart Deckhand-managed Claude/Pi sessions into their original agent conversation
- Pick a worktree mode per session: none, new, or existing
- Designed to work well in IDE integrated terminals

<!-- TODO: add a screenshot or asciinema GIF here once available, e.g.:
![Deckhand](assets/screenshot.png)
-->

## Context

Tools like [claude-squad](https://github.com/smtg-ai/claude-squad) and [agent-deck](https://github.com/asheshgoplani/agent-deck) manage parallel agent work with [`tmux`](https://github.com/tmux/tmux) and git worktrees. Deckhand uses a different stack with no `tmux` dependency: an [Ink](https://github.com/vadimdemedes/ink) UI, a local daemon, and [`node-pty`](https://github.com/microsoft/node-pty) workers.

## Features

- **Split view** with session sidebar and Preview / Terminal / Git / Dev tabs
- **Live previews** of session output without attaching
- **Sessions persist** across UI quits and crashes — the daemon owns them
- **Resumable agent identity** for new Claude/Pi sessions, so restarts reopen the same conversation instead of a blank agent
- **Cleanup prompts** after a session exits to remove its worktree and branch
- **Merge helpers** to merge or squash-merge a session's worktree into the current branch (staged, not committed) for review
- **Optional Git tab** powered by `lazygit`
- **Configurable Dev tab** for a command such as `npm run dev`

---

## Requirements

- Node.js `>= 20`
- macOS or Linux
- A POSIX shell
- `git` available on `PATH`
- `claude`, `pi`, and/or `codex` available on `PATH`, depending on which agents you want to run
- Optional: [`lazygit`](https://github.com/jesseduffield/lazygit) on `PATH` for the Git tab

---

## Installation

Install from npm:

```bash
npm install -g @tejgor/deckhand
```

Then run:

```bash
deckhand
```

For local development, install from source:

```bash
git clone https://github.com/tejgor/deckhand.git
cd deckhand
npm install
npm run build
npm link
```

After changing source code, rebuild before re-running the linked CLI:

```bash
npm run build
```

The CLI binary is declared in `package.json` as `deckhand → dist/cli.js`.

> **macOS note:** `npm install` runs `scripts/fix-node-pty.js`, which best-effort repairs the `node-pty` `spawn-helper` binary (executable bit, `xattr`, ad-hoc `codesign`). See [Troubleshooting](#troubleshooting) if install fails.

---

## Quick start

From inside a git repository:

```bash
deckhand
```

Then:

1. Press `n` to create a new session
2. Choose `claude`, `pi`, or `codex`
3. Enter a session name
4. Press `tab` to choose a workspace mode: no worktree, new worktree, or existing worktree
5. Press `enter` to launch

Use `j` / `k` to move between sessions and `o` to attach to the selected session.

---

## Controls

### Main view

| Key | Action |
| --- | --- |
| `n` | New session |
| `tab` | Cycle Preview / Terminal / Git / Dev tabs |
| `o` | Attach to the selected session, opening whichever tab is currently shown |
| `d` | Start/stop the selected session's Dev tab command |
| `m` | Merge selected worktree into the current branch |
| `j` / `k` | Move between sessions |
| `h` / `l` | Resize the sidebar |
| `x` / `X` | Kill selected running session / force kill |
| `s` | Restart selected exited session; new Claude/Pi sessions resume their original agent conversation |
| `backspace` | Drop selected exited session from the list |
| `r` | Refresh session list |
| `?` | Show keyboard shortcuts |
| `q` | Quit the UI; running sessions continue in the daemon |

Worktree sessions may prompt to keep/delete the worktree, or delete both the managed worktree and branch when it is safe to do so.

### Attach mode

| Key | Action |
| --- | --- |
| *(any)* | Sent directly to the attached pane/session |
| `Ctrl+Space` | Detach and return to Deckhand |

---

## Agent session identity and restarts

For new sessions, Deckhand assigns a deterministic agent handle based on the visible session name plus a short immutable Deckhand id:

```text
dh-{sanitized-session-name}-{short-id}
```

This keeps handles readable while avoiding collisions between similarly named sessions.

Current provider behavior:

| Agent | Create behavior | Restart behavior |
| --- | --- | --- |
| Claude | `claude --name dh-{name}-{short-id}` | `claude --resume dh-{name}-{short-id}` |
| Pi | `pi --session ~/.pi/agent/sessions/--{cwd}--/{timestamp}_dh-{name}-{short-id}_{id}.jsonl` | same `--session` path |
| Codex | normal launch | normal launch; Codex session-id discovery is not implemented yet |

Pi session files are stored in Pi's normal session tree under `~/.pi/agent/sessions/`, not under `~/.deckhand`, so they remain available in Pi's own `/resume` UI and are not removed if Deckhand state is deleted.

Older Deckhand sessions created before this metadata existed do not have an agent handle and will keep the previous restart behavior.

---

## Workspace modes

When creating a session, Deckhand can launch it in one of three workspace modes:

| Mode | Behavior |
| --- | --- |
| No worktree | Run in the current repository directory |
| New worktree | Create or resolve a worktree for the session |
| Existing worktree | Pick from existing git worktrees, including the current/main one |

New worktrees are created through a project hook when present; otherwise Deckhand falls back to `git worktree add` under `~/.deckhand/worktrees/`.

---

## Configuration

Deckhand reads configuration from `~/.deckhand/config.json`.

### Dev command

The Dev tab is started explicitly with `d`. Configure the global command like this:

```json
{
  "dev_command": "npm run dev"
}
```

Set `dev_command` before pressing `d`. If omitted, Deckhand tries to runs a `dev` alias.

### State and logs

| Path | Purpose |
| --- | --- |
| `~/.deckhand/state.json` | Persisted session list |
| `~/.deckhand/config.json` | User configuration |
| `~/.deckhand/daemon.log` | Supervisor daemon diagnostics |
| `~/.deckhand/daemon.pid` | Active supervisor daemon PID |
| `~/.deckhand/daemon.sock` | Local IPC socket |
| `~/.deckhand/workers/` | Per-session worker PID/log files |
| `~/.deckhand/worktrees/` | Default location for auto-created worktrees |
| `~/.pi/agent/sessions/` | Normal Pi session storage; Deckhand-created Pi sessions are stored here |

---

## Worktree hooks

For new-worktree sessions, Deckhand first creates or resolves a git worktree, then starts the agent inside it. Deckhand uses this project hook if present; otherwise it falls back to `git worktree add`:

```text
.claude/scripts/create-worktree.sh
```

### Hook contract

- Read JSON from `stdin`
- Use `name` as the sanitized worktree/session name
- Use `cwd` as the directory where `deckhand` was launched
- Create or register a git worktree
- Print the absolute worktree path to `stdout` as the final non-empty line
- Exit `0` on success

Deckhand also sets `CLAUDE_PROJECT_DIR` to the launch cwd for compatibility with Claude-style hooks.

### Input example

```json
{ "name": "my_feature", "cwd": "/path/to/current/worktree" }
```

<details>
<summary><strong>Minimal hook example</strong></summary>

```bash
#!/bin/bash
set -e

INPUT="$(cat)"
NAME="$(echo "$INPUT" | jq -r '.name // "worktree"')"
CWD="$(echo "$INPUT" | jq -r '.cwd // env.CLAUDE_PROJECT_DIR // env.PWD')"
DIR="$HOME/.deckhand/worktrees/$NAME"
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

If your hook copies symlinks from the source worktree, prefer resolving them with `realpath` so newly-created worktrees point directly at the real target instead of through another linked worktree.

---

## Architecture

Deckhand has three main pieces:

- **Ink frontend** — renders the terminal UI, sends requests to the daemon, and attaches to live panes when requested.
- **Local daemon** — owns session state, IPC, worktree operations, and worker supervision.
- **Session workers** — one worker per running session; each worker owns the agent PTY plus optional Terminal / Git / Dev PTYs.

Terminal output is fed into a headless [`xterm.js`](https://github.com/xtermjs/xterm.js) model. The UI receives rendered snapshots for previews, while attach mode streams input/output directly between your terminal and the selected PTY.

The UI can quit or crash without taking sessions down; the daemon owns them. New Claude and Pi sessions also persist a Deckhand-owned agent resume handle, allowing exited sessions to restart into the same conversation when the agent supports it.

### Daemon lifecycle

Deckhand spawns a long-lived supervisor daemon the first time you launch the UI. Quitting the UI with `q` leaves the daemon (and any running sessions) in place; relaunching `deckhand` reattaches. Stopping the daemon kills all running sessions.

| Action | How |
| --- | --- |
| Check whether the daemon is running | `pgrep -F ~/.deckhand/daemon.pid` |
| Tail daemon logs | `tail -f ~/.deckhand/daemon.log` |
| Stop the daemon (and all sessions) | `kill $(cat ~/.deckhand/daemon.pid)` |
| Recover from a crashed daemon | Remove `~/.deckhand/daemon.pid` and `~/.deckhand/daemon.sock`, then relaunch |

---

## Development

```bash
npm install
npm run dev      # run from source via tsx
npm run daemon   # run only the daemon in dev mode
npm run build    # compile to dist/
```

Useful checks before linking a build:

```bash
npm run build
npm pack --dry-run --ignore-scripts
```

---

## Troubleshooting

**`deckhand` can't find an agent.** Confirm the binary you want is on `PATH`: `which claude`, `which pi`, `which codex`. Deckhand inherits the launching shell's environment.

**`node-pty` fails to load on macOS** (e.g. `spawn-helper` permission errors). Re-run the repair script directly:

```bash
node scripts/fix-node-pty.js
```

If that doesn't help, reinstall: `rm -rf node_modules && npm install`.

**Stale daemon socket / PID.** If `deckhand` hangs at startup or reports it can't reach the daemon, the supervisor may have exited uncleanly. Remove the stale files and relaunch:

```bash
rm -f ~/.deckhand/daemon.pid ~/.deckhand/daemon.sock
deckhand
```

**Git tab is empty.** Install [`lazygit`](https://github.com/jesseduffield/lazygit) and ensure it is on `PATH`.

**Dev tab does nothing.** The Dev command is started explicitly with `d`. If `~/.deckhand/config.json` does not set `dev_command`, Deckhand runs the literal command `dev`, which usually doesn't exist.

---

## Uninstall

```bash
npm unlink -g deckhand
```

To remove all local state (sessions, logs, auto-created worktrees):

```bash
rm -rf ~/.deckhand
```

If Deckhand created git worktrees under `~/.deckhand/worktrees/`, prefer removing them through the UI (or `git worktree remove`) before deleting the directory, so git's bookkeeping stays consistent.

---

## Contributing

Issues and pull requests are welcome. For larger changes, please open an issue first to discuss the approach. Run `npm run build` and confirm the CLI launches before sending a PR.

---

## License

Deckhand is released under the [MIT License](LICENSE).
