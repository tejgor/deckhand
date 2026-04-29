# Deckhand

A standalone terminal workbench for managing local coding-agent sessions.

Inspired by [claude-squad](https://github.com/smtg-ai/claude-squad), Deckhand gives you a persistent split-view UI for running multiple `claude` or `pi` sessions side-by-side, each backed by a daemon-owned PTY so sessions survive across attaches and detaches.

---

## Features

- **Standalone terminal app** — runs anywhere, no editor integration required
- **Background daemon** keeps sessions alive across detaches
- **Multiple agent types** — spawn `claude` or `pi` sessions on demand
- **Split-view UI** with an instance sidebar and Preview / Terminal / Git / Dev tabs
- **Live preview** powered by a daemon-side `@xterm/headless` model, so cursor rewrites render correctly
- **Worktree-aware** — create new git worktrees per session, or reuse existing ones
- **Persistent state** in `~/.deckhand/`

---

## Requirements

- Node.js `>= 20`
- A POSIX shell (macOS or Linux)
- `git` available on `PATH`

---

## Installation

### Global install (recommended)

```bash
npm install -g deckhand
```

### From source

```bash
git clone <this-repo>
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

> **macOS note:** `npm install` runs `scripts/fix-node-pty.js` to repair the `spawn-helper` executable bit and apply best-effort quarantine removal / ad-hoc codesigning.

---

## Quick start

From any git repository:

```bash
deckhand
```

Then:

1. Press `n` to create a new session
2. Choose `claude` or `pi`
3. Enter a name
4. Press `tab` to pick a workspace mode (no worktree / new worktree / existing worktree)
5. Press `enter` to launch

Use `j` / `k` to move between sessions and `o` to attach to the selected one.

---

## Controls

### Main view

| Key | Action |
| --- | --- |
| `n` | New session |
| `tab` | Cycle Preview / Terminal / Git / Dev tabs |
| `o` | Attach to selected running session's active tab |
| `d` | Start/stop the selected session's dev command |
| `j` / `k` | Move between sessions |
| `x` / `X` | Kill selected running session / force kill; worktree sessions prompt to optionally delete the worktree, or delete both the managed worktree and branch |
| `s` | Restart selected exited session |
| `backspace` | Remove selected exited session |
| `r` | Refresh |
| `?` | Show keyboard shortcuts |
| `q` | Quit |

### Attach mode

| Key | Action |
| --- | --- |
| *(any)* | Sent directly to the attached pane/session |
| `Ctrl+Space` | Detach and return to Deckhand |

---

## Configuration

### Dev command

The Dev tab is started explicitly with `d`. Configure the global command in `~/.deckhand/config.json`:

```json
{
  "dev_command": "npm run dev"
}
```

If `dev_command` is omitted, Deckhand runs `dev`.

### Attach scroll sensitivity

Attached sessions dampen trackpad/mouse-wheel scrolling by default. Configure the multiplier in `~/.deckhand/config.json`:

```json
{
  "attach_scroll_sensitivity": 0.25
}
```

Use `1` for normal terminal scrolling, lower values for slower scrolling, or `0` to ignore vertical wheel events while attached. The default is `0.25`.

### State and logs

| Path | Purpose |
| --- | --- |
| `~/.deckhand/state.json` | Persisted session list |
| `~/.deckhand/daemon.log` | Daemon diagnostics |
| `~/.deckhand/daemon.pid` | Active daemon PID |
| `~/.deckhand/worktrees/` | Default location for auto-created worktrees |

---

## Worktree hooks

For new-worktree sessions, Deckhand first creates or resolves a git worktree, then starts the agent inside it. If present, Deckhand uses the following project hook before falling back to built-in `git worktree add`:

```
.claude/scripts/create-worktree.sh
```

### Hook contract

- Read JSON from `stdin`
- Use `name` as the sanitized worktree/session name
- Use `cwd` as the directory where `deckhand` was launched (may be the main repo or an existing linked worktree)
- Create or register a git worktree
- Print the absolute worktree path to `stdout` as the final non-empty line
- Exit `0` on success

Deckhand also sets `CLAUDE_PROJECT_DIR` to the launch cwd for compatibility with Claude-style hooks.

### Input example

```json
{ "name": "my_feature", "cwd": "/path/to/current/worktree" }
```

### Minimal hook

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

> If your hook copies symlinks from the source worktree, prefer resolving them with `realpath` so newly-created worktrees point directly at the real target instead of through another linked worktree.

If no hook is present, Deckhand creates worktrees under `~/.deckhand/worktrees/`.

---

## Architecture

- **TypeScript + [Ink](https://github.com/vadimdemedes/ink)** for the terminal UI
- **Daemon** owns long-lived PTY sessions via [`node-pty`](https://github.com/microsoft/node-pty)
- **Live preview** uses [`@xterm/headless`](https://www.npmjs.com/package/@xterm/headless) on the daemon side so cursor rewrites and in-place updates render as terminal screen state instead of raw scrollback
- **Frozen frames** — when a session exits, Deckhand retains the last preview frame for review

---

## Development

```bash
npm install
npm run dev      # run from source via tsx
npm run daemon   # run only the daemon (dev mode)
npm run build    # compile to dist/
```

---

## License

See repository for license information.
