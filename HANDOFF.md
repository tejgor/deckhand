# deckhand handoff

This document describes the current state of Deckhand, the main architectural choices, implemented behavior, and remaining caveats.

## High-level summary

`deckhand` is a working standalone Ink frontend backed by a long-lived Node daemon.

Deckhand is inspired by the `claude-squad` project and implemented as an independent TypeScript/Ink application.

Current implemented behavior:

- standalone Ink UI
- supervisor daemon with per-session worker-owned PTY sessions via `node-pty`
- local IPC over a Unix socket
- repo-scoped session list
- persistent split layout:
  - left sidebar of sessions
  - right tabbed Preview / Terminal / Git pane
- daemon-side terminal preview rendering via `@xterm/headless`
- selection-driven preview updates with `j` / `k`
- resizable instance sidebar with `h` / `l`
- subtle icon-based agent/activity indicators
- create flow for `claude`, `pi`, and `codex`
- create sessions in one of three workspace modes:
  - no worktree
  - new worktree
  - existing worktree
- agent-agnostic new-worktree creation using project hook scripts when available
- existing worktree picker, including the main/current worktree
- worktree-aware kill confirmation with keep/delete/cancel behavior when applicable
- merge or squash-merge without committing a selected session worktree back into the Deckhand launch/current branch, while skipping with a warning when the session branch has no new commits
- external attach / detach flow
- kill running session from UI
- remove exited session from UI
- restart exited sessions in their recorded cwd/worktree and, for Deckhand-managed agent identities, resume the same agent conversation
- lazy daemon-owned Git tab backed by `lazygit`
- explicit daemon-owned Dev tab backed by a configurable global dev command
- subtle Dev-running indicators in the tab bar and session sidebar
- stale-session cleanup on daemon restart
- frozen last preview frame for exited sessions
- preview-change based active/idle detection for any agent, without hooks
- daemon PID/log files for lifecycle diagnostics
- protocol version check and PID-aware daemon replacement safeguards

## Core decisions

### 1. Deckhand is an independent implementation

Deckhand is inspired by `claude-squad`, but the current codebase is its own TypeScript/Ink application with its own state, daemon protocol, and session lifecycle.

That means:

- state lives in Deckhand-specific files under `~/.deckhand`
- session ownership belongs to the Deckhand daemon
- migration layers are intentionally out of scope for this phase

Why:

- the daemon and frontend can evolve around Deckhand's current UX
- the runtime model stays simple and agent-agnostic
- the project can support multiple local coding agents consistently

### 2. The supervisor owns control state; session workers own PTYs

We use a central daemon as the supervisor/control plane and one child worker process per running session for the PTY runtime.

Why:

- the frontend should be disposable
- sessions should survive quitting and reopening Ink
- attach/detach should be controlled by Deckhand itself
- live preview should come from a long-lived backend source of truth
- a crash in one session runtime should not take down other sessions

### 3. PTYs are `node-pty`, with a macOS repair step

The daemon uses `node-pty` directly.

On this machine, `node-pty` initially failed on macOS with `posix_spawnp failed` because:

- `node_modules/node-pty/prebuilds/darwin-arm64/spawn-helper`

was missing the executable bit.

The project now includes:

- `scripts/fix-node-pty.js`

That script runs on `npm install` and, on macOS, best-effort:

- fixes the helper executable bit
- removes quarantine attributes
- applies ad-hoc codesigning to the helper and native module

If `node-pty` fails again on macOS, check the helper permissions first.

### 4. Preview fidelity matters more than raw scrollback

The daemon does not just expose raw PTY output.

Instead it maintains a daemon-side terminal model using:

- `@xterm/headless`

That lets the Preview pane reflect:

- cursor movement
- line rewrites
- in-place spinner/progress updates
- a more realistic current terminal screen state

This is much closer to a live terminal screen than plain scrollback.

### 5. Agent activity is inferred from preview changes, not hooks

Deckhand deliberately does not depend on agent-specific hooks.

Instead the daemon tracks changes to the rendered preview snapshot:

- visible preview changes mark the agent `active`
- no visible preview changes for a quiet period marks it `idle`
- new sessions start as `unknown` until enough signal exists

This is intentionally agent-agnostic and works for `claude`, `pi`, `codex`, and future agents.

Important distinction:

- `status` is lifecycle: `starting`, `running`, `exited`
- `agentStatus` is activity: `unknown`, `active`, `idle`

Do not overload lifecycle status to mean activity.

### 6. Worktree support is agent-agnostic

Instead, for new worktree sessions, the daemon creates or resolves the worktree first, then starts the selected agent normally inside that directory. This allows the same worktree flow to work for:

- `claude`
- `pi`
- `codex`
- future local agents

New worktree creation uses a tiered strategy:

1. Look for `.claude/scripts/create-worktree.sh` in the current git worktree root.
2. If not present, look for `.claude/scripts/create-worktree.sh` in the main/original worktree root.
3. If still not present, fall back to built-in `git worktree add` logic under `~/.deckhand/worktrees`.

When a hook script is used:

- it is executed as `bash <scriptPath>`
- its working directory is the current git worktree root
- `CLAUDE_PROJECT_DIR` is set to the exact directory where `deckhand` was launched
- stdin receives JSON with the sanitized worktree name and exact launch cwd:

```json
{"name":"sanitized/session_name","cwd":"/exact/deckhand/launch/cwd"}
```

`cwd` intentionally points at the exact directory where `deckhand` was launched, which may be the main repo or an existing linked worktree. Hooks should prefer JSON `.cwd`, with `CLAUDE_PROJECT_DIR` as a compatibility fallback. This matches Claude Code's WorktreeCreate input shape more closely while keeping existing Claude-style hooks compatible.

The sanitizer:

- lowercases the entered session title
- allows `a-z`, `0-9`, `_`, `-`, and `/`
- replaces other characters with `_`
- collapses repeated `_` and `/`
- trims leading/trailing `/`, `_`, and `-`
- falls back to `worktree`

This means worktree creation branches from the currently-open worktree/branch even when `deckhand` itself was launched from a linked worktree. Hooks that copy or link dependency directories from the source worktree should resolve source symlinks with `realpath` if they want new worktrees to point directly at the canonical target instead of through another linked worktree.

Worktree deletion is intentionally guarded:

- the current worktree cannot be deleted
- the main worktree cannot be deleted
- a worktree used by another non-exited Deckhand session cannot be deleted

When safe, worktree-backed session kill offers a one-step confirmation:

- kill only / keep worktree
- kill and delete worktree
- kill, delete worktree and branch (for any non-protected branch, including existing-worktree sessions)
- cancel

## Directory / file overview

Relevant files now include:

- `package.json`
- `tsconfig.json`
- `README.md`
- `HANDOFF.md`
- `src/cli.ts`
- `src/app.tsx`
- `src/client.ts`
- `src/daemon.ts`
- `src/sessionWorker.ts`
- `src/attach.ts`
- `src/storage.ts`
- `src/git.ts`
- `src/paths.ts`
- `src/types.ts`
- `src/nodePty.ts`
- `src/sidebar.tsx`
- `src/preview.tsx`
- `src/terminalPane.tsx`
- `src/gitPane.tsx`
- `src/devPane.tsx`
- `src/tabs.tsx`
- `src/terminalPreview.ts`
- `src/ui.ts`
- `scripts/fix-node-pty.js`

## File-by-file notes

### `src/cli.ts`

Entry point.

Responsibilities:

- run Ink UI normally
- run daemon with `--daemon`
- loop so the app can:
  1. render Ink
  2. exit temporarily for attach mode
  3. return to Ink after detach

### `src/app.tsx`

Main Ink UI.

Current behavior:

- shows sessions for the current repo only
- renders a persistent sidebar + Preview layout
- keeps a persistent live daemon connection
- subscribes to session updates
- watches preview for the selected session
- lets selection drive the Preview pane
- supports create flow for `claude`, `pi`, and `codex`
- during name entry, `tab` cycles workspace mode:
  - no worktree
  - new worktree
  - existing worktree
- existing worktree mode opens a picker in the right pane and includes all non-bare git worktrees, including the main/current worktree
- worktree-backed kill opens a confirmation pane with keep/delete/delete-branch/cancel options when deletion is safe
- shows compact program glyphs instead of program text:
  - `✶` for Claude
  - `π` for Pi
  - `◇` for Codex
- shows compact activity/lifecycle indicators instead of status text:
  - spinner for starting/active
  - green `●` for idle running sessions
  - yellow `◌` for unknown running sessions
  - gray `○` for exited sessions
  - appends a subtle `▹` on rows whose Dev command is running
- supports resizing the instance sidebar with `h` / `l`
- preserves the resized sidebar width across attach/detach within the same frontend process
- lets the user attach, kill, restart exited sessions, remove, refresh, and quit
- reconnects if the daemon connection drops

Current controls:

- `n` create session
- in create name entry, `tab` cycles no/new/existing worktree mode
- in existing-worktree mode, `enter` opens the worktree picker
- in worktree picker, `j` / `k` move and `enter` selects
- `j` / `k` move selection
- `h` / `l` resize the instance sidebar
- left / right arrows also resize the sidebar in browse mode
- `tab` switches the right pane between Preview, Terminal, Git, and Dev
- footer hints are dynamic and intentionally compact
- `?` opens a keyboard-shortcuts help pane
- `o` attaches to the selected running session's active pane (`agent` on Preview, shell on Terminal, `lazygit` on Git)
- attach mode sets the compact terminal/window title to `dh/<pane> <session>` while streaming PTY output directly; `Ctrl+Space` returns to Deckhand
- `m` merge or squash-merge selected worktree-backed session into the Deckhand launch/current branch without committing, with merge/squash/cancel confirmation; if there are no new commits, Deckhand skips the merge and shows a warning
- `x` kill selected running session
- for worktree-backed sessions, `x` opens keep/delete/delete-branch/cancel confirmation when applicable
- `s` restart selected exited session in its recorded cwd/worktree, resuming the original agent conversation when `agentSessionRef` is available
- `d` starts/stops the selected running session's Dev command and switches to the Dev tab
- `backspace` removes selected exited session
- `r` refresh / resubscribe
- `q` quit

Notes:

- the app currently has Preview, Terminal, Git, and Dev panes
- a running Dev command is indicated without opening the Dev tab:
  - selected session: green `●` suffix on the Dev tab label
  - all sessions: subtle `▹` suffix in each sidebar row
- the right side renders as a single rounded-bordered frame containing the tab
  bar at the top and the active sub-pane below; sub-panes themselves are
  borderless content containers
- the Git pane starts `lazygit` lazily in the selected session cwd/worktree
- the Dev pane starts explicitly with `d` in the selected session cwd/worktree
- the Dev command is read globally from `~/.deckhand/config.json` as `dev_command`, defaulting to `dev`
- create flow, worktree picker, and kill confirmation currently replace the right pane instead of using a true overlay/modal
- the kill confirmation uses a red border to signal a destructive action

### `src/client.ts`

Daemon client utilities.

Responsibilities:

- connect to the socket
- send JSON requests
- parse line-delimited JSON responses and events
- auto-start the daemon if needed
- version-check the daemon protocol
- provide a persistent live client abstraction for the Ink UI

Important behavior:

- daemon stdout/stderr append to `~/.deckhand/daemon.log`
- daemon PID is tracked in `~/.deckhand/daemon.pid`
- if the socket is stale or missing and no live daemon PID exists, the client removes it and starts a new daemon
- if a daemon PID is still alive but ping fails, the client retries and then surfaces an error instead of blindly replacing it
- if the daemon protocol version does not match while the daemon PID is alive, the client refuses to replace it automatically and tells the user to stop the old daemon first

### `src/daemon.ts`

Core supervisor backend.

Responsibilities:

- supervise per-session worker processes
- own persisted session metadata
- expose an IPC server over a Unix socket
- create PTY-backed sessions
- create sessions in no-worktree, new-worktree, or existing-worktree mode
- assign deterministic Deckhand-owned agent session handles for resumable agents:
  - Claude: `--name dh-{sanitized-title}-{short-id}` on create, `--resume <name>` on restart
  - Pi: `--session ~/.pi/agent/sessions/--{cwd}--/{timestamp}_dh-{sanitized-title}-{short-id}_{id}.jsonl` on create/restart, using Pi's normal session tree so sessions remain visible to Pi independently of Deckhand
  - Codex: no deterministic handle yet; falls back to a normal launch
- restart exited sessions in their existing recorded cwd/worktree without creating a new session record
- list git worktrees for the frontend picker
- call `.claude/scripts/create-worktree.sh` when available for new worktree creation
- fall back to built-in `git worktree add` creation when no script exists
- optionally remove a safe-to-delete worktree, and optionally its non-protected checked-out branch, after killing a worktree-backed session
- route attach/input/resize requests to session workers
- stream raw PTY output from workers to attached clients
- supervise lazy per-session companion shell/Git/Dev PTYs owned by workers
- receive daemon-side preview state updates from worker-owned `@xterm/headless` models
- infer agent active/idle status from rendered preview changes
- suppress activity recomputation caused only by preview/PTY resize redraws
- broadcast session, preview, and terminal updates to subscribed UI clients
- support kill/remove lifecycle actions
- own explicit per-session Dev command PTYs for the Dev tab
- freeze last preview on exit
- write/remove the daemon PID file
- write daemon lifecycle diagnostics to the daemon log
- mark stale running sessions exited on daemon restart

Supported request types:

- `ping`
- `list`
- `subscribe`
- `list-worktrees`
- `watch-preview`
- `watch-terminal`
- `watch-git`
- `watch-dev`
- `start-dev`
- `stop-dev`
- `create`
- `restart`
- `kill`
- `merge-worktree`
- `remove`
- `attach`
- `input`
- `resize`
- `detach`
- `attach-terminal`
- `terminal-input`
- `terminal-resize`
- `terminal-detach`
- `attach-git`
- `git-input`
- `git-resize`
- `git-detach`
- `attach-dev`
- `dev-input`
- `dev-resize`
- `dev-detach`

Emitted event types:

- `session-updated`
- `session-removed`
- `preview-updated`
- `terminal-updated`
- `git-updated`
- `dev-updated`
- `output`
- `terminal-output`
- `git-output`
- `dev-output`
- `attached`
- `detached`
- `terminal-attached`
- `terminal-detached`
- `git-attached`
- `git-detached`
- `dev-attached`
- `dev-detached`

### `src/attach.ts`

Direct attach mode.

Behavior:

- clears the Ink UI, resets inherited terminal modes, and hands the full terminal to the selected PTY
- opens a persistent daemon connection
- sends an `attach`, `attach-terminal`, `attach-git`, or `attach-dev` request depending on the active pane
- sets and reasserts a compact terminal/window title while attached with OSC 0/2, updates the Node process title as a best-effort fallback for terminal hosts such as VS Code, and resets to `deckhand` on cleanup
- puts stdin into raw mode
- forwards user input to the daemon
- writes PTY output directly to stdout
- resizes the agent or companion PTY to the full terminal size while attached
- detaches on `Ctrl+Space` and resets terminal modes such as scroll regions, mouse/focus tracking, bracketed paste, and child-owned alternate screens before Ink redraws

This remains intentionally separate from the Ink-rendered Preview/Terminal panes; attach is a branded full-screen handoff rather than an embedded terminal emulator.

### `src/storage.ts`

Deckhand persistence.

State file:

- `~/.deckhand/state.json`

Responsibilities:

- ensure config directory exists
- load/save Deckhand state
- sort sessions for display
- mark non-exited sessions exited on daemon restart

Persisted session metadata now includes the usual identity/status fields plus:

- `args`
- `agentSessionRef`
- `agentStatus`
- `agentStatusUpdatedAt`
- `lastPreview`

`agentStatus` is persisted only on activity transitions to avoid excessive disk writes.

`lastPreview` stores the last rendered preview frame for exited sessions.

### `src/git.ts`

Git helper layer for:

- verifying the current directory is inside a git repo
- finding the current worktree root
- finding the main/original worktree root via `git rev-parse --git-common-dir`
- listing all non-bare worktrees with `git worktree list --porcelain`
- sanitizing new worktree names from session titles
- creating new worktrees via the tiered strategy:
  1. current worktree `.claude/scripts/create-worktree.sh`
  2. main worktree `.claude/scripts/create-worktree.sh`
  3. built-in fallback under `~/.deckhand/worktrees`
- invoking worktree scripts with:
  - `bash <scriptPath>`
  - cwd set to the current worktree root
  - `CLAUDE_PROJECT_DIR` set to the exact Deckhand launch cwd
  - stdin JSON containing both `name` and `cwd`
- removing safe-to-delete worktrees with `git worktree remove -f` and `git worktree prune`
- merging a session worktree back into the launch/current worktree without committing using `git merge --no-commit --no-ff <source>`, or squash-applying without committing using `git merge --squash <source>`; before either mode it checks `HEAD..<source>` and skips when there are no new commits

### `src/paths.ts`

Centralizes paths for:

- config dir
- socket file
- daemon PID file
- daemon log file
- state file
- CLI entry path
- runtime path helpers

### `src/types.ts`

Shared types for:

- lifecycle status records
- agent activity status records
- worktree modes and worktree metadata
- session records
- preview records
- client requests
- server responses/events
- UI exit results

### `src/nodePty.ts`

macOS `node-pty` helper repair logic.

### `src/sidebar.tsx`

Sidebar rendering.

Responsibilities:

- render repo-scoped sessions
- highlight the selected session
- show icon-based lifecycle/activity glyphs and colors
- show compact program glyphs instead of program text
- show small right-aligned per-instance index markers like `[1]`, anticipating possible numeric shortcuts later
- crop the visible list when the terminal is short

### `src/preview.tsx`

Preview pane rendering.

Responsibilities:

- show the session's worktree/cwd path with a worktree/attached badge when applicable
- show live preview text
- show sensible fallback states
- preserve current preview content when activity status changes
- show frozen preview for exited sessions

The sub-pane intentionally does **not** repeat the tab name or session title:
those are already conveyed by the active tab and the sidebar selection.
Lifecycle/activity status lives in the sidebar; the right pane focuses on
content.

### `src/terminalPane.tsx` / `gitPane.tsx` / `devPane.tsx`

Each follows the same pattern as `preview.tsx`:

- single header row showing the session's worktree/cwd path on the left
- `● live` / `○ cold` (or `○ idle` for Dev) indicator on the right
- body fills the remaining height with the underlying companion PTY snapshot
- Dev appends ` · <command>` to the path when a command is configured

### `src/ui.ts`

Shared theme + UI helpers used by every pane:

- `THEME` uses ANSI named colors (`magenta`, `cyan`, `green`, etc.) so the
  palette inherits from the user's terminal theme instead of locking in hex
  values that look wrong against custom palettes
- identity accent is `magenta` (titles, branding, badges)
- focus/selection accent is `cyan` (selected sidebar row, active tab,
  modal cursors)
- `compactPath` collapses `$HOME` to `~` and trims long paths to `…/parent/leaf`
- `truncate` / `fitLines` shared truncation + padding helpers
- `programGlyph`, `statusGlyph`, `statusColor`, `previewStatusIcon`,
  `statusLabel` shared session-display helpers

### `src/sessionWorker.ts`

Per-session runtime worker.

Responsibilities:

- own the agent PTY for one session
- spawn the agent with persisted `session.args` instead of always launching the bare command
- own that session's Terminal/Git/Dev companion PTYs
- maintain `@xterm/headless` preview models
- infer active/idle activity from preview changes
- forward snapshots, raw output, lifecycle exits, and pane updates to the supervisor

### `src/terminalPreview.ts`

Daemon-side terminal preview model.

Responsibilities:

- own an `@xterm/headless` terminal instance
- consume PTY output
- resize with the preview viewport
- lazily produce a plain-text snapshot of the current rendered terminal screen

Performance note:

- PTY writes mark the preview dirty but do **not** serialize the screen immediately
- snapshot serialization happens only when a broadcast/request actually needs it
- daemon broadcasts are also coalesced/throttled, so chatty sessions do not force one Ink render per output chunk

## State, socket, and lifecycle

## Socket / PID / log

The daemon listens on:

- `~/.deckhand/daemon.sock`

The active daemon PID is tracked in:

- `~/.deckhand/daemon.pid`

Daemon/client lifecycle diagnostics are written to:

- `~/.deckhand/daemon.log`

Protocol:

- line-delimited JSON

This is local IPC only.

## State file

Session metadata is stored in:

- `~/.deckhand/state.json`

Tracked fields include:

- `id`
- `title`
- `program`
- `command`
- `args` (agent launch arguments; restart swaps create args for resume args when supported)
- `agentSessionRef` (Deckhand-owned resume handle, currently Claude name or Pi session path under Pi's normal `~/.pi/agent/sessions/` tree)
- `cwd` (actual agent cwd; may be a worktree path)
- `repoRoot` (UI grouping repo root from where Deckhand was launched)
- `launchCwd`
- `launchWorktreeRoot`
- `worktree` metadata when applicable:
  - mode: `none`, `managed`, or `attached`
  - path
  - branch
  - HEAD
  - main-worktree flag
  - origin/creator/name metadata
- lifecycle `status`
- activity `agentStatus`
- `agentStatusUpdatedAt`
- timestamps
- `pid` when known
- exit details when known
- `lastPreview`

## Runtime lifecycle

Current lifecycle is:

1. Ink UI starts
2. client pings daemon
3. daemon auto-starts if missing or stale
4. daemon loads Deckhand state
5. daemon marks any previously-running sessions `exited`
6. Ink subscribes to repo sessions
7. Ink watches preview for the selected session
8. user creates a session and chooses no/new/existing worktree mode
9. for new/existing worktree sessions, daemon resolves the target cwd before spawning the agent
10. daemon spawns a `node-pty` session in the final cwd
11. daemon maintains both:
    - raw PTY output for attach mode
    - rendered preview state for Preview mode
    - preview-change activity state for active/idle indicators
12. user can resize the sidebar; the frontend re-requests preview at the new pane size
13. daemon suppresses resize-only redraws from marking the agent active
14. user can quit and reopen Ink without killing sessions, as long as daemon stays alive
15. if a session worker dies, only that session exits; other session workers continue
16. on next startup, sessions not reachable through the current supervisor lifecycle are shown as `exited` with their last frozen preview when available
17. when killing a worktree-backed session, the daemon may remove the worktree after PTY exit if the user selected delete and safety checks passed

Important limitation:

- frontend restarts are supported
- daemon crash/restart does **not** preserve running PTYs
- stale running sessions are intentionally marked exited on next daemon start

That is still by design for this phase.

## What was validated

The following were validated during development.

### Build / install

- `npm install`
- `npm run build`

### Daemon startup and replacement

Validated:

- client auto-starts daemon when socket is missing
- client writes daemon stdout/stderr to the daemon log
- daemon writes/removes PID file on startup/shutdown
- client replaces stale sockets only when no live daemon PID is present
- protocol version mismatch is detected
- live protocol-mismatched daemons are not silently replaced; the client asks the user to stop the old daemon first

### Session creation

Validated:

- created `pi` sessions successfully
- created `claude` sessions successfully
- command resolution finds real local binaries
- TypeScript build passes with agent launch args and resumable Claude/Pi session handles
- TypeScript build passes after adding codex support

Not yet manually validated:

- created `codex` sessions successfully

### Worktree support

Validated programmatically / by build:

- sanitizer lowercases titles and permits `/`, e.g. `Fix API/Login Bug!` -> `fix_api/login_bug`
- current worktree root lookup works from this repo
- main worktree root lookup works from this repo
- `git worktree list --porcelain` parsing works and includes the main worktree
- `npm run build` passes

Not yet fully manually validated in an interactive Ink run:

- new-worktree creation through a real `.claude/scripts/create-worktree.sh`
- fallback new-worktree creation under `~/.deckhand/worktrees`
- existing-worktree picker UX in a repo with multiple worktrees
- kill confirmation delete path on a disposable worktree

### Live preview pipeline

Validated programmatically:

- subscribed to repo session updates
- watched preview for a selected session
- received live `preview-updated` events
- confirmed preview content changes as the agent writes to the PTY
- confirmed cursor-rewrite style updates look better than raw scrollback
- confirmed dirty/debounced preview serialization still emits live updates and persists `lastPreview` on exit
- confirmed activity state transitions from `unknown` -> `active` when preview changes
- confirmed activity state transitions to `idle` after the quiet timeout
- confirmed exited sessions use lifecycle `exited` while activity is internally idle

### Attach path

Validated:

- attach request succeeds
- output streams to stdout
- detach succeeds
- returning to Ink re-enters preview mode

### Exit + frozen preview behavior

Validated:

- killed a running session
- session transitioned to `exited`
- `lastPreview` persisted in state
- exited session remained previewable via frozen last frame

### Split layout render

Validated by running the Ink app in a PTY and capturing output:

- sidebar renders on the left
- Preview pane renders on the right
- initial empty states render correctly
- sidebar uses compact status/program glyphs and right-aligned per-row index markers
- sidebar can be resized with `h` / `l`
- resized sidebar width survives attach/detach within the same frontend process
- resizing does not by itself mark an idle session active

### Stale-session cleanup

Validated:

- killed daemon
- removed socket
- restarted via client
- previously-running sessions were shown as `exited`

## Important caveats and notes

### 1. This is still a prototype

The architecture is now much better established, but this is still not a finished product.

### 2. Preview and Terminal panes exist today

Current right-side pane status:

- Preview implemented
- Terminal implemented as a lazy daemon-owned companion shell per session
- Git implemented as a lazy daemon-owned `lazygit` PTY per session
- Dev implemented as an explicit daemon-owned PTY per session, started/stopped with `d` and configured by `~/.deckhand/config.json` `dev_command`

### 3. Attach mode still temporarily exits Ink

This is intentional.

Why:

- Preview and Terminal panes are read-only app chrome
- full PTY interaction wants direct stdin/stdout control
- exiting Ink for attach is simpler and more reliable than embedding a full terminal emulator right now

### 4. The daemon is the source of truth for live state

Keep this separation intact:

- frontend = disposable UI/controller
- daemon = long-lived runtime owner

### 5. Preview text is not the same as full embedded terminal rendering

The daemon is keeping a proper terminal model, which is a large step up from raw scrollback.

However, the Ink Preview pane still renders plain text snapshots, not a full interactive terminal surface.

That means:

- live state is much more accurate than before
- full terminal styling/interaction parity is still a later step if desired

### 6. PTY sizing is shared per PTY

Each agent PTY and companion terminal PTY has one active size.

So:

- in Preview mode, the daemon sizes the agent PTY to the preview viewport
- in Terminal mode, the daemon sizes the companion shell PTY to the terminal pane viewport
- resizing the sidebar changes the viewport and therefore reapplies pane sizing
- in attach mode, the active PTY is resized to the full terminal
- when Ink returns, pane sizing is reapplied

The daemon suppresses agent activity recomputation for agent-preview resize-only redraws so changing pane sizes does not incorrectly flip idle agents to active.

This is acceptable with the current attach model.

### 7. Worktree support is newly implemented and needs real-world exercise

The worktree architecture and TypeScript build are in place, but the end-to-end flows should still be exercised in a disposable repository/worktree setup.

Particularly important cases:

- hook-script creation from a main worktree
- hook-script creation from a linked worktree
- fallback creation when no script exists
- existing worktree selection including the main worktree
- delete confirmation on a non-current, non-main worktree, including branch deletion for existing worktrees
- attempted delete of current/main worktree should stay blocked
- multiple sessions pointed at one worktree should block deletion

Protocol is now v15. If an older daemon is still running, stop it first:

```bash
kill $(cat ~/.deckhand/daemon.pid)
```

## Recommended next steps

If continuing from here, I would recommend this order.

### Near-term

1. Manually exercise worktree flows in disposable repos:
   - script creation from current worktree
   - script creation from linked worktree
   - fallback creation
   - existing picker
   - keep/delete/cancel kill behavior
2. Add preview-pane polish:
   - nicer empty/loading/exited states
   - optional scroll mode
3. Refine create/worktree UX:
   - overlay/modal instead of replacing the right pane
   - better validation and error feedback
   - nicer truncation/filtering for long worktree paths
4. Add richer sidebar/status metadata:
   - show branch/worktree hints below session titles
   - optional numeric shortcuts for the right-aligned `[n]` row indicators
   - optional persisted sidebar width across full frontend restarts
5. Clean up attach/detach transition visuals
6. Add basic tests around preview serialization, activity transitions, resize suppression, IPC flows, worktree parsing, sanitizer behavior, and delete safety checks

### Medium-term

7. Further refine right-side panes:
   - terminal/git/dev scrollback controls
   - dev stop confirmation/persisted status
   - richer tab status indicators
8. Add structured daemon logging
9. Add stronger daemon health / protocol compatibility handling
10. Add more cleanup/restart lifecycle commands
11. Add tests around persistence and reconnect behavior

### Later

12. Monitor long-running macOS `node-pty` behavior under repeated spawn/exit churn
13. Consider embedded terminal rendering if single-screen interaction becomes important

## How to run

```bash
npm install
npm run build
npm link
deckhand
```

## Final takeaway

The most important win is the architecture now in place:

- no backward-compat burden
- daemon-owned long-lived sessions
- explicit attach/detach model
- persistent split-view Ink frontend
- daemon-side rendered Preview pipeline
- initial agent-agnostic worktree support

That is now a solid foundation for building Deckhand.
