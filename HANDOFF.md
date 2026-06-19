# Deckhand Handoff

This is the continuity document for Deckhand. It should preserve implementation details that matter for future development while avoiding repeated prose.

## Current state

Deckhand is a standalone TypeScript/Ink application backed by a long-lived local Node daemon. It is inspired by `claude-squad`, but it has its own state, protocol, daemon model, and session lifecycle.

Implemented behavior:

- standalone Ink UI
- supervisor daemon with local IPC over `~/.deckhand/daemon.sock`
- per-session worker processes that own PTYs through `node-pty`
- repo-scoped session list with manual sidebar ordering plus clean/forked sub-session nesting
- persistent split layout:
  - left session sidebar
  - right tabbed pane: Preview, Terminal, Git, Dev, Notes
  - active right-pane tab is remembered per session within the frontend/attach loop
- daemon-side terminal preview rendering using `@xterm/headless`
- read-only Preview focus mode with scrolling
- external full-screen attach/detach for agent, terminal, git, and dev PTYs
- create/restart/kill/remove flows for `claude`, `pi`, and `codex`
- worktree modes:
  - no worktree
  - new managed worktree
  - existing/attached worktree
- safe worktree deletion and optional branch deletion on kill, including cleanup of leftover worktree directories/remnants
- merge/squash-merge of a session worktree into the Deckhand launch/current branch without committing
  - no new commits => skipped with a warning
  - conflicts => expected resolvable state, not a Deckhand error
- lazy Git tab backed by daemon-owned `lazygit`
- explicit Dev tab backed by configurable global dev command
- preview-change based active/idle detection without agent hooks
- frozen last preview frame for exited sessions
- stale-session cleanup after daemon restart
- daemon PID/log files and protocol-version safeguards

## Architecture

### Core model

- Frontend is disposable UI/controller.
- Daemon is the source of truth for live control state.
- Workers own PTY runtime for individual sessions.
- A worker crash should exit only that session, not the daemon.
- Daemon crash/restart does **not** preserve live PTYs; stale running sessions are marked exited on next daemon start.

### Frontend

`src/app.tsx` renders the Ink UI and talks to the daemon through `src/client.ts`.

The UI:

- filters sessions to the current repo
- subscribes to session updates
- watches preview/pane updates for the selected session
- manages create, merge, kill, restart, remove, attach, and Dev command actions
- reconnects if the daemon connection drops

### Daemon

`src/daemon.ts` is the supervisor/control plane.

Responsibilities:

- load/save persisted session metadata
- own the IPC socket
- start/stop session workers
- route attach/input/resize requests to workers
- receive worker snapshots and lifecycle messages
- broadcast session, preview, terminal, git, and dev events
- manage worktree creation/deletion/merge safety
- manage daemon PID and lifecycle logging

### Workers

`src/sessionWorker.ts` owns all PTYs for one session:

- agent PTY
- companion shell/Terminal PTY
- companion Git/lazygit PTY
- companion Dev PTY
- daemon-side preview models using `@xterm/headless`

Workers spawn agents with persisted `session.args`, not just the bare command, so restarts can resume supported agents.

## Important design rules

- Lifecycle and activity are distinct:
  - lifecycle `status`: `starting`, `running`, `exited`
  - activity `agentStatus`: `unknown`, `active`, `idle`
- Activity is inferred from visible preview changes, not agent-specific hooks.
- Do not overload lifecycle status to mean activity.
- Preview is a rendered plain-text snapshot, not a full embedded terminal emulator.
- Attach mode intentionally exits Ink temporarily and gives stdin/stdout directly to the selected PTY.
- PTY sizing is shared per PTY:
  - Preview sizes the agent PTY to the preview viewport.
  - Terminal/Git/Dev size their companion PTYs to pane viewport.
  - Attach mode sizes the active PTY to the full terminal.
  - Returning from attach reapplies pane sizing.
- Resize-only redraws must not mark idle agents active.

## UI behavior and controls

### Main layout

- Sidebar shows compact program glyphs:
  - Claude: `✶`
  - Pi: `π`
  - Codex: `◇`
- Sidebar activity/lifecycle indicators:
  - spinner for starting/active
  - green `●` for idle running sessions
  - yellow `◌` for unknown running sessions
  - gray `○` for exited sessions
- Dev-running indicators:
  - selected session: green `●` suffix on the Dev tab
  - all sessions: subtle `▹` suffix in sidebar row
- Sidebar has numbered per-row markers like `[1]`; typing the number jumps to that visible session. With 10 or fewer visible sessions, single-digit jumps are instant and `0` selects 10; with more than 10 visible sessions, numeric input is briefly buffered for multi-digit selection. Parent sessions with sub-sessions show `▾` / `▸` and can be expanded/collapsed.
- Right pane is one rounded bordered frame with tab bar at the top; sub-panes are borderless content containers.

### Controls

- `n` create session
- during create name entry, `tab` cycles workspace mode: no/new/existing worktree
- in existing-worktree mode, `enter` opens the worktree picker
- in worktree picker, `j` / `k` move and `enter` selects
- `j` / `k` move selected session
- session numbers jump to matching sidebar rows; with more than 10 sessions, multi-digit input is buffered briefly and `enter` confirms immediately
- `J` / `K` manually reorder selected session among its siblings
- `c` collapses/expands the selected session's sub-session subtree in the sidebar
- `N` creates a sub-session under the selected session; the normal agent picker creates clean sub-sessions in the parent's cwd/worktree by default, and Claude/Pi parents add a fourth `Fork parent` option. Claude forks send `/fork <dh-name>` so the fork receives Deckhand's deterministic child session name.
- `h` / `l` resize sidebar
- left/right arrows also resize sidebar in browse mode
- `[` / `]` decrease/increase `attach_scroll_sensitivity` live and persist it to config
- `tab` switches Preview / Terminal / Git / Dev / Notes for the selected session
- `p` / `t` / `g` / `d` / `a` directly focus Preview / Terminal / Git / Dev / Notes for the selected session
- Notes is per-session persisted text; selecting the Notes tab is read-only until `o` enters notes edit/focus mode; `esc` exits notes editing
- switching sessions restores that session's most recently selected tab, defaulting to Preview
- `v` enters Preview focus mode for running sessions
- in Preview focus:
  - mouse wheel / trackpad scrolls
  - `j` / `k` are keyboard fallbacks
  - `g` jumps upward
  - `G` jumps back down/live follow
  - `esc` or `v` returns to browse mode
- `o` attaches to selected session's active pane:
  - Preview => agent
  - Terminal => shell
  - Git => lazygit
  - Dev => dev command PTY
  - Notes => enters notes edit/focus mode
- `O` opens the selected session directory/worktree in Cursor if available, otherwise Code (`cursor`/`code` CLI, macOS falls back to `open -a Cursor`)
- attach mode title is `dh/<pane> <session>`
- `Ctrl+Space` detaches back to Deckhand
- `m` opens merge/squash/cancel confirmation for worktree-backed sessions
- `x` kills selected running session
- for worktree-backed sessions, `x` opens keep/delete/delete-branch/cancel when applicable
- `s` resumes/restarts selected exited session; Claude forked sub-sessions restart directly from their parsed exit resume handle when available, otherwise Deckhand falls back to resuming the parent and sending `/fork`
- `S` fresh-restarts selected exited session without using any persisted/parsed resume handle; Claude gets a new deterministic Deckhand name suffix and Pi gets a new session file path
- `d` focuses the Dev tab; when already focused on Dev, it starts/stops the selected session's Dev command
- `backspace` removes selected exited session
- `r` refreshes/resubscribes
- `?` opens help
- `q` quits

### Preview scrolling

Preview focus is read-only for most agents and scrolls Deckhand's daemon-side xterm scrollback snapshot. Claude Code behaves more like a TUI, so Preview focus sends synthetic SGR mouse-wheel events to the Claude PTY instead of only scrolling Deckhand state.

Both attach mode and Preview focus use `attach_scroll_sensitivity` from config, defaulting to `0.12`. The multiplier can be adjusted live from the UI with `[` / `]`; Deckhand persists the new value to `~/.deckhand/config.json`, and subsequent attach sessions pick it up without restarting the app.

Exited sessions show only the frozen `lastPreview` frame.

## Worktree behavior

### Creation

New worktree creation is agent-agnostic. The daemon resolves/creates the target cwd before launching the selected agent normally in that directory.

Creation strategy:

1. Use `.claude/scripts/create-worktree.sh` in the current worktree root if present.
2. Else use `.claude/scripts/create-worktree.sh` in the main/original worktree root if present.
3. Else fall back to built-in `git worktree add` under `~/.deckhand/worktrees`.

When a hook script is used:

- command: `bash <scriptPath>`
- working directory: current git worktree root
- env: `CLAUDE_PROJECT_DIR=<exact Deckhand launch cwd>`
- stdin JSON:

```json
{"name":"sanitized/session_name","cwd":"/exact/deckhand/launch/cwd"}
```

`cwd` intentionally points to the exact directory where Deckhand was launched, which may be the main repo or a linked worktree. Hooks should prefer JSON `.cwd`, with `CLAUDE_PROJECT_DIR` as compatibility fallback.

The sanitizer:

- lowercases input
- allows `a-z`, `0-9`, `_`, `-`, `/`
- replaces other characters with `_`
- collapses repeated `_` and `/`
- trims leading/trailing `/`, `_`, `-`
- falls back to `worktree`

Example: `Fix API/Login Bug!` => `fix_api/login_bug`.

Hooks that copy/link dependency directories from the source worktree should resolve source symlinks with `realpath` if they want new worktrees to point to canonical targets rather than through another linked worktree.

### Deletion safety

Worktree deletion is guarded:

- current worktree cannot be deleted
- main worktree cannot be deleted
- a worktree used by another non-exited Deckhand session cannot be deleted

When safe, kill confirmation offers:

- kill only / keep worktree (restartable)
- kill and delete worktree (not restartable)
- kill, delete worktree and branch (not restartable)
- cancel

After Git unregisters a deleted worktree, Deckhand also force-removes the worktree path to clear ignored/untracked remnants. For fallback managed worktrees under `~/.deckhand/worktrees`, it also prunes empty nested parent folders left by slash-preserving worktree names.

Deleted worktrees are recorded with `worktree.deletedAt`; Deckhand hides restart/merge hints and refuses restart/merge for those exited sessions.

Branch deletion refuses protected branches `main` and `master`.

### Merge behavior

Implemented in `src/git.ts`.

- normal merge: `git merge --no-commit --no-ff <source>`
- squash merge: `git merge --squash <source>`
- target is the Deckhand launch/current worktree
- source is the selected session worktree's current branch, or its HEAD SHA if detached
- target worktree must be on a branch
- source and target roots must differ
- before merge, Deckhand checks `HEAD..<source>` and skips if there are no new commits
- if Git exits nonzero and leaves unmerged files, Deckhand returns a `conflicted: true` result instead of throwing
- UI returns to browse mode and shows a status message for conflicts

## Agent resume handles

Deckhand assigns deterministic handles where supported:

- Child session titles inherit parent context daemon-side as `parent title / child title` (trimmed to 64 chars) so agent/worktree/session names are not orphaned; Deckhand UI strips that parent prefix for nested sub-session display because the sidebar already shows the hierarchy.
- Claude:
  - create: `--name dh-{sanitized-title}-{short-id}`
  - forked sub-session create: resume parent, then send `/fork dh-{sanitized-title}-{short-id}` while persisting the child handle for direct restart
  - on exit, parse Claude Code's printed `claude --resume "..."` command from the final preview and persist that handle as `agentSessionRef`
  - resume restart: `--resume <parsed-or-created-handle>`; restart also re-parses `lastPreview` so sessions that exited before this change can still recover the handle
  - fresh restart: `--name dh-{sanitized-title}-{short-id}-fresh-{timestamp}` and does not parse or use prior resume handles
- Pi:
  - create/resume restart: explicit `--session` path under Pi's normal `~/.pi/agent/sessions/` tree
  - directory encoding matches Pi's `getDefaultSessionDir()`:
    - `--${cwd.replace(/^[/\\]/, '').replace(/[/\\:]/g, '-')}--`
  - filename is Deckhand-specific and includes timestamp, sanitized title, short id, and Deckhand session id
- Codex:
  - no deterministic handle yet; launches normally

## Persistence, socket, PID, and logs

Deckhand writes under `~/.deckhand`:

- `state.json` — persisted sessions
- `config.json` — app config
- `daemon.sock` — Unix socket
- `daemon.pid` — active daemon PID
- `daemon.log` — daemon/client diagnostics
- worker runtime/log files via helpers in `src/paths.ts`

Config currently includes:

- `dev_command`, default `dev`
- `attach_scroll_sensitivity`, default `0.12`, adjustable in the UI with `[` / `]`

Protocol:

- line-delimited JSON
- current protocol version: **v20**

If an older daemon is still running:

```bash
kill $(cat ~/.deckhand/daemon.pid)
```

### Client/daemon replacement behavior

`src/client.ts`:

- auto-starts daemon when socket is missing/stale and no live daemon PID exists
- writes daemon stdout/stderr to `~/.deckhand/daemon.log`
- removes stale socket only when no live daemon PID exists
- retries if PID is alive but ping fails, then surfaces an error instead of blindly replacing it
- refuses to auto-replace a live daemon with mismatched protocol version; user must stop old daemon first

### Persisted session fields

Tracked metadata includes:

- `id`
- `title`
- `program`
- `command`
- `args`
- `agentSessionRef`
- `cwd`
- `repoRoot`
- `launchCwd`
- `launchWorktreeRoot`
- `worktree` metadata:
  - mode: `none`, `managed`, `attached`
  - path
  - branch
  - HEAD
  - main-worktree flag
  - origin/creator/name metadata
  - `deletedAt` when Deckhand deleted the worktree on session exit
- lifecycle `status`
- activity `agentStatus`
- `agentStatusUpdatedAt`
- timestamps
- `pid`
- exit details
- `lastPreview`
- `notes`
- `parentSessionId`
- `subSessionKind`
- `forkedFromSessionId`
- `forkedFromAgentSessionRef`
- `sidebarOrder`

`agentStatus` is persisted only on activity transitions to avoid excessive disk writes.

## IPC request/event types

Supported request types include:

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
- `update-session-notes`
- `create`
- `reorder-session`
- `restart`
- `kill`
- `merge-worktree`
- `remove`
- `attach`, `input`, `resize`, `detach`
- `attach-terminal`, `terminal-input`, `terminal-resize`, `terminal-detach`
- `attach-git`, `git-input`, `git-resize`, `git-detach`
- `attach-dev`, `dev-input`, `dev-resize`, `dev-detach`

Emitted event types include:

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
- `attached`, `detached`
- `terminal-attached`, `terminal-detached`
- `git-attached`, `git-detached`
- `dev-attached`, `dev-detached`

## File map

- `package.json` — package scripts/dependencies/bin metadata.
- `tsconfig.json` — TypeScript config.
- `README.md` — user-facing overview.
- `HANDOFF.md` — this continuity document.
- `src/cli.ts` — entry point; runs UI or daemon; loops around attach/detach.
- `src/app.tsx` — main Ink UI and interaction state.
- `src/client.ts` — daemon client, autostart, protocol version checks, persistent live client.
- `src/daemon.ts` — supervisor daemon and IPC handling.
- `src/sessionWorker.ts` — per-session PTY owner.
- `src/sessionOrder.ts` — sidebar hierarchy sorting, depth, child detection, and collapse filtering.
- `src/attach.ts` — full-screen attach/detach mode.
- `src/storage.ts` — state/config loading and persistence.
- `src/git.ts` — git repo, worktree, deletion, branch, and merge helpers.
- `src/paths.ts` — config/socket/PID/log/runtime path helpers.
- `src/types.ts` — shared session/protocol/UI types.
- `src/nodePty.ts` — macOS `node-pty` helper repair logic.
- `src/sidebar.tsx` — session sidebar rendering.
- `src/preview.tsx` — Preview pane rendering.
- `src/terminalPane.tsx` — Terminal pane rendering.
- `src/gitPane.tsx` — Git pane rendering.
- `src/devPane.tsx` — Dev pane rendering.
- `src/notesPane.tsx` — per-session Notes pane rendering.
- `src/tabs.tsx` — tab UI.
- `src/terminalPreview.ts` — headless xterm preview model.
- `src/ui.ts` — shared theme, glyph, path, truncation, and display helpers.
- `scripts/fix-node-pty.js` — install-time macOS `node-pty` fixup.

## File-specific notes

### `src/cli.ts`

- Runs Ink UI normally.
- Runs daemon with `--daemon`.
- Loops so app can render Ink, exit for attach, then return to Ink after detach.

### `src/app.tsx`

- Owns UI modes, selected session, active tab, pane subscriptions, and user input handling.
- Create/worktree picker/kill confirmation currently replace the right pane rather than using true overlays.
- Kill confirmation uses a red border for destructive actions.
- Sidebar width is preserved across attach/detach in the same frontend process, but not yet across full frontend restarts.

### `src/attach.ts`

- Clears Ink UI and resets inherited terminal modes.
- Opens persistent daemon connection.
- Attaches to agent/terminal/git/dev based on active pane.
- Sets/reasserts terminal/window title with OSC 0/2 and best-effort `process.title`.
- Puts stdin in raw mode.
- Dampens matched vertical mouse wheel events using `attach_scroll_sensitivity`.
- Detaches on `Ctrl+Space`.
- On cleanup, resets terminal modes such as scroll regions, mouse/focus tracking, bracketed paste, and child-owned alternate screens.

### `src/nodePty.ts` and `scripts/fix-node-pty.js`

On macOS, `node-pty` can fail with `posix_spawnp failed` if the helper is not executable:

- `node_modules/node-pty/prebuilds/darwin-arm64/spawn-helper`

The install-time script best-effort:

- fixes executable bit
- removes quarantine attributes
- applies ad-hoc codesigning to helper/native module

If `node-pty` fails on macOS, check helper permissions first.

### `src/terminalPreview.ts`

- Owns an `@xterm/headless` terminal instance.
- Consumes PTY output.
- Resizes with preview viewport.
- Produces plain-text snapshots of the rendered terminal screen.
- PTY writes mark preview dirty but do not immediately serialize the screen.
- Snapshot serialization happens only when a broadcast/request needs it.
- Broadcasts are coalesced/throttled.

### `src/ui.ts`

- Uses ANSI named colors so terminal themes remain respected.
- Identity accent: `magenta`.
- Focus/selection accent: `cyan`.
- Includes shared helpers for compact paths, truncation, line fitting, glyphs, status colors, and labels.

## Runtime lifecycle

Typical flow:

1. Ink UI starts.
2. Client pings daemon.
3. Daemon auto-starts if missing/stale.
4. Daemon loads persisted state.
5. Daemon marks previously-running sessions exited on daemon restart.
6. Ink subscribes to repo sessions.
7. Ink watches preview for selected session.
8. User creates a session and chooses worktree mode.
9. Daemon resolves final cwd/worktree.
10. Daemon starts a worker.
11. Worker spawns agent PTY and maintains preview state.
12. Daemon broadcasts session/preview/pane updates.
13. User can attach/detach without killing PTY.
14. User can quit/reopen frontend while daemon keeps sessions alive.
15. If a worker exits, only that session exits and last preview is frozen.
16. If daemon restarts, stale running sessions are marked exited.

## Validated

Validated during development:

- `npm install`
- `npm run build`
- daemon autostart when socket missing
- daemon stdout/stderr log wiring
- PID file write/remove
- stale socket replacement only when no live daemon PID exists
- protocol mismatch detection and refusal to replace live mismatched daemon
- Pi session creation
- Claude session creation
- command resolution for local binaries
- TypeScript build with Claude/Pi resume args and Codex support
- TypeScript build with Claude exit resume-handle parsing for forked sub-session restarts
- TypeScript build with fresh restart/no-resume mode and parent-inherited child titles
- TypeScript build with deleted-worktree sessions marked non-restartable/non-mergeable
- TypeScript build with post-removal cleanup of leftover worktree directories/remnants
- TypeScript build with named Claude `/fork <dh-name>` creation
- TypeScript build with collapsible sidebar sub-session trees
- sanitizer behavior, including slash-preserving names
- current/main worktree root lookup
- `git worktree list --porcelain` parsing, including main worktree
- preview subscriptions and `preview-updated` events
- live preview changes from agent PTY output
- cursor-rewrite/progress rendering via headless xterm
- debounced preview serialization and `lastPreview` persistence on exit
- activity transitions `unknown` -> `active` -> `idle`
- attach request/output/detach/return to Ink
- killed session transitions to `exited`
- frozen preview for exited sessions
- split layout rendering
- compact sidebar glyphs and per-row index markers
- sidebar resize with `h` / `l`
- sidebar width persistence across attach/detach in same process
- resize does not mark idle sessions active
- stale-session cleanup after daemon restart

Not fully manually validated:

- Codex session creation
- real hook-script worktree creation from main worktree
- real hook-script worktree creation from linked worktree
- fallback worktree creation under `~/.deckhand/worktrees`
- existing-worktree picker in a repo with many worktrees
- worktree delete paths in disposable repos
- branch deletion path for existing-worktree sessions
- attempted deletion of current/main worktree remains blocked
- multiple sessions pointing at one worktree block deletion

## Caveats

- This is still a prototype, though the architecture is established.
- Frontend restarts are supported; daemon crash/restart does not preserve running PTYs.
- Preview and panes are read-only snapshots; attach is required for direct interaction.
- Preview text is not equivalent to full styled terminal rendering.
- Attach mode still temporarily exits Ink by design.
- Create/worktree picker/kill confirmation are pane replacements, not true modals.
- Worktree support is implemented but needs more real-world exercise.
- Codex deterministic resume support is not implemented.
- Terminal/Git/Dev scrollback controls are still future work.

## Recommended next steps

Near term:

1. Manually exercise worktree flows in disposable repos:
   - hook creation from main worktree
   - hook creation from linked worktree
   - fallback creation
   - existing picker
   - keep/delete/cancel kill behavior
   - delete-branch behavior
   - merge success/skipped/conflicted cases
2. Add tests around:
   - worktree parsing
   - sanitizer behavior
   - merge skipped/conflicted/success cases
   - delete safety checks
   - preview serialization
   - activity transitions
   - resize suppression
   - IPC flows
3. Polish create/worktree UX:
   - true overlays/modals
   - better validation and error feedback
   - better truncation/filtering for long paths
4. Clean up attach/detach transition visuals.
5. Add structured daemon logging.
6. Add stronger daemon health/protocol compatibility handling.

Later:

- Persist sidebar width across full frontend restarts.
- Add richer sidebar branch/worktree metadata.
- Add optional numeric shortcuts for sidebar row markers.
- Add terminal/git/dev scrollback controls.
- Add dev stop confirmation or persisted dev state if useful.
- Add cleanup/restart lifecycle commands.
- Monitor long-running macOS `node-pty` behavior under repeated spawn/exit churn.
- Add Codex deterministic resume support if possible.
- Consider embedded terminal rendering only if single-screen interaction becomes important.

## How to run

```bash
npm install
npm run build
npm link
deckhand
```

## Final takeaway

Deckhand now has a solid foundation:

- independent state/protocol/runtime model
- daemon-owned long-lived sessions
- worker-owned PTYs
- explicit attach/detach
- split-view Ink frontend
- daemon-side rendered Preview pipeline
- initial agent-agnostic worktree support
