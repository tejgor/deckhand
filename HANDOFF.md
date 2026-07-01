# Deckhand Handoff

Continuity notes for future Deckhand development. Keep this file focused on implementation details that are not obvious from the README or a quick file scan.

## Snapshot

Deckhand is a standalone TypeScript/Ink app backed by a long-lived local Node daemon. It is inspired by `claude-squad`, but owns its own state, IPC protocol, worker model, and session lifecycle.

Package/runtime facts:

- npm package: `@tejgor/deckhand`
- binary: `deckhand`
- supported Node: `>=20`
- supported OS in package metadata: `darwin`, `linux`
- package is ESM (`"type": "module"`)
- build output lives in `dist/`; dev mode uses `tsx`

Implemented behavior:

- Ink dashboard with sidebar plus Preview, Terminal, Git, Dev, and Notes tabs.
- Local daemon IPC over `~/.deckhand/daemon.sock`.
- One worker process per running session; workers own the live PTYs.
- Supported agents: `claude`, `pi`, `codex`.
- Session create/restart/kill/remove flows, including resume/fresh restart where supported.
- Sub-sessions under parent sessions, with clean and forked variants for Claude/Pi parents.
- Repo-scoped session list with persisted manual ordering among siblings and collapsible subtrees.
- Daemon-side terminal preview rendering with `@xterm/headless`.
- Read-only Preview focus mode with scrollback; Claude gets synthetic wheel input because its TUI behaves differently.
- External attach/detach for agent, terminal, git, and dev PTYs.
- Worktree modes: no worktree, new managed worktree, existing/attached worktree.
- Safe worktree deletion, optional branch deletion, and cleanup of leftover directories/remnants.
- Merge/squash-merge of a session worktree into the Deckhand launch/current branch without committing, with successfully merged sessions marked in the sidebar.
- Lazy Git tab powered by `lazygit` when installed.
- Dev tab powered by configurable global `dev_command`.
- Per-session persisted Notes tab.
- Preview-change-based active/idle detection without agent hooks.
- Frozen last preview frame for exited sessions.
- Stale-session cleanup after daemon restart.
- Daemon PID/log files and protocol-version safeguards.
- `deckhand setup` / `deckhand doctor` helper for checking/installing supported agents.

## Architecture

### Core model

- Frontend is disposable UI/controller.
- Daemon is the source of truth for persisted session metadata and live control routing.
- Workers own PTY runtime for individual sessions.
- A worker crash exits only that session, not the daemon.
- Frontend quit does not kill running sessions.
- Daemon crash/restart does **not** preserve live PTYs; persisted non-exited sessions are marked exited on next daemon start.

### Frontend (`src/app.tsx`, `src/cli.ts`)

`src/cli.ts` chooses between UI, daemon, session-worker, and setup/doctor modes. Normal UI flow enters an alternate screen, renders `App`, exits Ink for attach mode, then re-enters Ink after detach while preserving in-process UI state such as selected session, selected tab, sidebar width, per-session tab selection, and collapsed/hidden sidebar state.

`src/app.tsx`:

- filters sessions to the current repo
- subscribes to daemon updates
- watches preview/pane updates for the selected session
- owns create, merge, kill, restart, remove, attach, editor-open, Notes, and Dev-command actions
- reconnects if the daemon connection drops
- uses pane replacements for create/worktree picker/kill/merge/help rather than true overlays/modals

### Daemon (`src/daemon.ts`)

Responsibilities:

- load/save persisted session metadata
- own the IPC socket
- start/stop session workers
- route attach/input/resize/snapshot requests to workers
- receive worker snapshots, output, and lifecycle messages
- broadcast session, preview, terminal, git, and dev events
- manage worktree creation/deletion/merge safety
- manage daemon PID, socket lifecycle, and logging

Important technical-debt note: `src/daemon.ts` still contains legacy in-daemon PTY runtime maps and methods (`runtime`, `terminals`, `gits`, `devs`) used as fallback paths if a session has no worker. Current create/restart paths start workers, so be careful not to duplicate fixes across both paths unless the legacy fallback still matters.

### Workers (`src/sessionWorker.ts`)

Each worker owns all live PTYs for one session:

- agent PTY
- companion shell/Terminal PTY
- companion Git/lazygit PTY
- companion Dev PTY
- `@xterm/headless` preview models for all panes

Workers spawn agents with persisted `session.args`, not just the bare command, so restarts can resume supported agents.

Worker stdout/stderr are appended to per-session files under `~/.deckhand/workers/`.

## Design rules

- Lifecycle and activity are distinct:
  - lifecycle `status`: `starting`, `running`, `exited`
  - activity `agentStatus`: `unknown`, `active`, `idle`
- Activity is inferred from visible preview changes, not agent-specific hooks.
- Do not overload lifecycle status to mean activity.
- Resize-only redraws must not mark idle agents active.
- Preview is a rendered plain-text snapshot, not a full embedded terminal emulator.
- Preview/pane snapshots are read-only; attach mode is required for direct interaction.
- Attach mode intentionally exits Ink temporarily and gives stdin/stdout directly to the selected PTY.
- PTY sizing is per PTY:
  - Preview sizes the agent PTY to the preview viewport.
  - Terminal/Git/Dev size their companion PTYs to pane viewport.
  - Attach mode sizes the active PTY to the full terminal.
  - Returning from attach reapplies pane sizing.

## UI behavior and controls

### Layout and indicators

- Sidebar glyphs:
  - Claude: `✶`
  - Pi: `π`
  - Codex: `◇`
- Sidebar status indicators:
  - spinner for starting/active
  - green `●` for idle running sessions
  - yellow `◌` for unknown running sessions
  - gray `○` for exited sessions
- Sub-session rows are indented. Clean children show `↳`; forked children show `⑂`.
- Parent sessions with children show `▾` / `▸` and can be expanded/collapsed.
- Dev-running indicators:
  - selected session: green `●` suffix on Dev tab
  - all sessions: prominent `▶` near the left side of the sidebar row, after lifecycle status and before agent glyph
- Sidebar row markers are numeric (`[1]`, `[2]`, ...). With 10 or fewer visible sessions, single digits jump immediately and `0` selects row 10; with more than 10, numeric input is briefly buffered for multi-digit selection.
- Right pane is one rounded bordered frame with a tab bar; sub-panes are borderless content containers.

### Main controls

- `n` create top-level session
- `N` create sub-session under selected session
- in create program picker, Claude/Pi parents add `⑂ Fork parent`
- during create name entry, `tab` cycles workspace mode: no/new/existing worktree
- in existing-worktree picker, type to search, `j`/`k` or arrows move, `enter` selects
- `j` / `k` move selected session
- session numbers jump to matching visible rows; multi-digit input is buffered when needed and `enter` confirms immediately
- `J` / `K` manually reorder selected session among siblings
- `c` cycles selected session's subtree: collapse exited sub-sessions only, then collapse all sub-sessions, then expand all
- `h` / `l` resize sidebar; left/right arrows also resize sidebar in browse mode
- `[` / `]` decrease/increase `attach_scroll_sensitivity` live and persist it to config
- `tab` cycles Preview / Terminal / Git / Dev / Notes for selected session
- `p` / `t` / `g` / `d` / `a` directly focus Preview / Terminal / Git / Dev / Notes
- switching sessions restores that session's most recently selected tab, defaulting to Preview
- `v` enters Preview focus mode for running sessions
- `o` attaches to selected session's active pane:
  - Preview => agent
  - Terminal => shell
  - Git => lazygit
  - Dev => dev command PTY
  - Notes => enter notes edit/focus mode
- `O` opens selected session directory/worktree in Cursor if available, otherwise Code (`cursor`/`code` CLI; macOS fallback is `open -a Cursor`)
- `m` opens merge/squash/cancel confirmation for worktree-backed sessions
- `x` kills selected running session
- `X` force-kills selected running session; workers send SIGTERM first and SIGKILL after a short delay if still alive
- for worktree-backed sessions, kill confirmation offers keep/delete/delete-branch/cancel when applicable
- `s` resume/restart selected exited session
- `S` fresh-restart selected exited session without using prior parsed/persisted resume handle
- `d` focuses Dev; when already on Dev, starts/stops selected session's Dev command
- `backspace` removes selected exited session
- `r` refreshes/resubscribes
- `?` opens help
- `q` quits UI; daemon and running sessions continue

### Notes

- Notes are persisted per session in `~/.deckhand/state.json`.
- Selecting the Notes tab is read-only until `o` enters notes edit/focus mode.
- `esc` exits notes editing.
- Notes autosave through `update-session-notes` as text changes.

### Preview focus and scrolling

Preview focus is read-only for most agents and scrolls Deckhand's daemon/worker-side xterm scrollback snapshot. Claude Code behaves more like a TUI, so Preview focus sends synthetic SGR mouse-wheel events to the Claude PTY instead of only scrolling Deckhand state.

Preview focus controls:

- mouse wheel / trackpad scrolls
- `j` / `k` are keyboard fallbacks
- `g` jumps upward
- `G` jumps back down/live follow
- `esc` or `v` returns to browse mode

Both attach mode and Preview focus use `attach_scroll_sensitivity` from config, defaulting to `0.12`. `[` / `]` adjust it live and persist to `~/.deckhand/config.json`. Attach sessions pick up the latest value when entered.

Exited sessions show only the frozen `lastPreview` frame.

### Attach mode

- Attach mode title is `dh/<pane> <session>`.
- Attach clears/reset inherited terminal modes before handing off to the child PTY.
- `Ctrl+Space` is the primary universal detach key.
- `Ctrl+]` is a secondary universal detach key.
- Attach recognizes normal NUL `Ctrl+Space` plus common enhanced-keyboard encodings emitted when a child TUI enables CSI-u / modifyOtherKeys mode.
- Attach cleanup resets scroll regions, mouse/focus tracking, bracketed paste, alternate-screen state, enhanced-keyboard modes, and other child-owned terminal modes.
- Attach mode mirrors bracketed-paste state across the PTY boundary: agent attaches enable bracketed paste on the outer terminal, while Terminal/Git/Dev attaches enable it only when the worker observed the child PTY request `?2004h`. This prevents multi-line paste from being delivered as separate Enter presses without forcing paste markers into arbitrary programs.

## Worktree behavior

### Creation

New worktree creation is agent-agnostic. The daemon resolves/creates the target cwd, then launches the selected agent normally in that directory.

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

The final non-empty stdout line must be an absolute path to a registered git worktree. Hooks time out after 60 seconds.

The sanitizer:

- lowercases input
- allows `a-z`, `0-9`, `_`, `-`, `/`
- replaces other characters with `_`
- collapses repeated `_` and `/`
- trims leading/trailing `/`, `_`, `-`
- limits to 96 chars
- falls back to `worktree`

Example: `Fix API/Login Bug!` => `fix_api/login_bug`.

Hooks that copy/link dependency directories from a source worktree should resolve source symlinks with `realpath` if they want new worktrees to point to canonical targets rather than through another linked worktree.

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

After Git unregisters a deleted worktree, Deckhand force-removes the worktree path to clear ignored/untracked remnants. For fallback managed worktrees under `~/.deckhand/worktrees`, it also prunes empty nested parent folders left by slash-preserving worktree names.

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
- successful merge/squash operations persist `worktree.mergedAt`, `mergeMode`, `mergeTargetBranch`, and `mergeSourceRef`; sidebar shows a trailing `✓` for those sessions
- skipped or conflicted merge attempts do not set the merged marker
- if Git exits nonzero and leaves unmerged files, Deckhand returns a `conflicted: true` result instead of throwing
- UI returns to browse mode and shows a status message for skipped/conflicted results

## Agent identity, forks, and restarts

Deckhand assigns deterministic handles where supported. The base name is `dh-{sanitized-title}-{short-id}`.

Child session titles inherit parent context daemon-side as `parent title / child title` (trimmed to 64 chars). The UI strips that parent prefix for nested sidebar display because the sidebar already shows the hierarchy.

### Claude

- create: `--name dh-{sanitized-title}-{short-id}`
- clean sub-session: fresh Claude handle in the selected/parent cwd
- forked sub-session create: resume parent, then send `/branch <dh-name>` while persisting the child handle for direct restart
- branch input includes a small insert-mode safeguard: `a`, backspace, then `/branch...`, for Claude users in vim normal mode
- on exit, parse Claude Code's printed `claude --resume "..."` command from final preview and persist that handle as `agentSessionRef`
- resume restart: `--resume <parsed-or-created-handle>`; restart also re-parses `lastPreview` so older exited sessions can recover a handle
- fresh restart: `--name dh-{sanitized-title}-{short-id}-fresh-{timestamp}` and does not use prior resume handles

### Pi

- create/resume restart: explicit `--session` path under Pi's normal `~/.pi/agent/sessions/` tree
- directory encoding matches Pi's `getDefaultSessionDir()`:
  - `--${cwd.replace(/^[/\\]/, '').replace(/[/\\:]/g, '-')}--`
- filename includes timestamp, deterministic Deckhand name, and Deckhand session id
- forked sub-session create/restart resumes the parent/fork source session file and sends `/fork`
- fresh restart creates a new `--session` path

### Codex

- launches normally
- no deterministic resume/fork support yet

## Persistence, socket, PID, and logs

Deckhand writes under `~/.deckhand`:

- `state.json` — persisted sessions
- `config.json` — app config
- `daemon.sock` — Unix socket
- `daemon.pid` — active daemon PID
- `daemon.log` — daemon/client diagnostics
- `workers/<session>.pid` — session worker PID files
- `workers/<session>.log` — session worker stdout/stderr
- `worktrees/` — fallback managed worktree root

Pi session files are intentionally under Pi's own `~/.pi/agent/sessions/` tree, not under `~/.deckhand`.

Config currently includes:

- `dev_command`, default behavior is command `dev`
- `attach_scroll_sensitivity`, default `0.12`, adjustable in the UI with `[` / `]`

Protocol:

- line-delimited JSON
- current protocol version: **v22**

If an older live daemon has a protocol mismatch, Deckhand refuses to auto-replace it. Stop it manually:

```bash
kill $(cat ~/.deckhand/daemon.pid)
```

### Client/daemon replacement behavior (`src/client.ts`)

- auto-starts daemon when socket is missing/stale and no live daemon PID exists
- writes daemon stdout/stderr to `~/.deckhand/daemon.log`
- removes stale socket only when no live daemon PID exists
- retries if PID is alive but ping fails, then surfaces an error instead of blindly replacing it
- refuses to auto-replace a live daemon with mismatched protocol version

### Persisted session fields

Tracked metadata includes:

- `id`, `title`, `program`, `command`, `args`
- `agentSessionRef`
- `cwd`, `repoRoot`, `launchCwd`, `launchWorktreeRoot`
- `worktree` metadata:
  - mode: `none`, `managed`, `attached`
  - path, branch, HEAD, main-worktree flag
  - origin/creator/name metadata
  - `mergedAt` / `mergeMode` / `mergeTargetBranch` / `mergeSourceRef` when Deckhand successfully applied a merge/squash merge
  - `deletedAt` when Deckhand deleted the worktree on session exit
- lifecycle `status`
- activity `agentStatus`, `agentStatusUpdatedAt`
- timestamps, `pid`, exit details, `lastPreview`
- `notes`
- `devRunning`
- `parentSessionId`, `subSessionKind`, `forkedFromSessionId`, `forkedFromAgentSessionRef`
- `sidebarOrder`

`agentStatus` is persisted only on activity transitions to avoid excessive disk writes. `devRunning` is cleared during daemon restart recovery because live dev PTYs are not preserved.

## IPC request/event types

Request types:

- `ping`
- `list`, `subscribe`
- `list-worktrees`
- `watch-preview`, `watch-terminal`, `watch-git`, `watch-dev`
- `start-dev`, `stop-dev`
- `update-session-notes`
- `create`, `reorder-session`, `restart`, `kill`, `merge-worktree`, `remove`
- agent attach path: `attach`, `input`, `resize`, `detach`
- terminal path: `attach-terminal`, `terminal-input`, `terminal-resize`, `terminal-detach`
- git path: `attach-git`, `git-input`, `git-resize`, `git-detach`
- dev path: `attach-dev`, `dev-input`, `dev-resize`, `dev-detach`

Event types:

- `session-updated`, `session-removed`
- `preview-updated`, `terminal-updated`, `git-updated`, `dev-updated`
- `output`, `terminal-output`, `git-output`, `dev-output`
- `attached`, `detached`
- `terminal-attached`, `terminal-detached`
- `git-attached`, `git-detached`
- `dev-attached`, `dev-detached`

## File map

- `package.json` — package scripts, dependency, bin, engine, OS, and publish metadata.
- `tsconfig.json` — TypeScript config.
- `README.md` — user-facing overview.
- `HANDOFF.md` — this continuity document.
- `src/cli.ts` — entry point; runs UI, daemon, session worker, setup/doctor; loops around attach/detach.
- `src/setup.ts` — setup/doctor tool detection and optional agent install prompts.
- `src/app.tsx` — main Ink UI and interaction state.
- `src/client.ts` — daemon client, autostart, protocol version checks, persistent live client.
- `src/daemon.ts` — supervisor daemon and IPC handling.
- `src/sessionWorker.ts` — per-session PTY owner.
- `src/sessionOrder.ts` — sidebar hierarchy sorting, depth, child detection, and collapse filtering.
- `src/attach.ts` — external attach/detach mode.
- `src/storage.ts` — state/config loading and persistence.
- `src/git.ts` — git repo, worktree, deletion, branch, and merge helpers.
- `src/paths.ts` — config/socket/PID/log/runtime path helpers.
- `src/types.ts` — shared session/protocol/UI types.
- `src/nodePty.ts` — macOS `node-pty` helper repair logic.
- `src/terminalState.ts` — terminal escape reset helpers used before/after UI and attach transitions.
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

- Runs UI normally.
- Runs daemon with `--daemon`.
- Runs session worker with `--session-worker`.
- Runs setup/doctor with `setup` or `doctor`.
- Loops so the app can render Ink, exit for attach, then return to Ink after detach.

### `src/setup.ts`

- `deckhand setup` checks `claude`, `pi`, `codex`, and optional `lazygit`.
- `--check` is read-only.
- `--yes` / `-y` accepts agent install prompts.
- Installs only missing supported agents; `lazygit` remains optional and is not installed by setup.

### `src/app.tsx`

- Owns UI modes, selected session, active tab, pane subscriptions, and user input handling.
- Create/worktree picker/kill confirmation currently replace the right pane rather than using true overlays.
- Kill confirmation uses a red border for destructive actions.
- Sidebar width and collapsed/hidden session state are preserved across attach/detach in the same frontend process, but not across full frontend restarts.

### `src/attach.ts`

- Clears Ink UI and resets inherited terminal modes.
- Opens persistent daemon connection.
- Attaches to agent/terminal/git/dev based on active pane.
- Sets/reasserts terminal/window title with OSC 0/2 and best-effort `process.title`.
- Puts stdin in raw mode.
- Dampens matched vertical mouse wheel events using `attach_scroll_sensitivity`.
- Re-enables bracketed paste on the outer terminal for agent attaches, and for Terminal/Git/Dev attaches when the worker reports the child PTY had requested bracketed paste.
- Detaches on `Ctrl+Space` or `Ctrl+]`.
- On cleanup, resets terminal modes such as scroll regions, mouse/focus tracking, bracketed paste, enhanced-keyboard modes, and child-owned alternate screens.

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
- Resizes with preview/pane viewport.
- Produces plain-text snapshots of the rendered terminal screen.
- Can produce an ANSI frame for attach handoff.
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

1. CLI ensures Deckhand is running inside a git repository.
2. Ink UI starts in alternate screen.
3. Client pings daemon.
4. Daemon auto-starts if missing/stale.
5. Daemon loads persisted state and marks previously running sessions exited if this is a daemon restart.
6. Ink subscribes to repo sessions.
7. Ink watches preview/pane for selected session.
8. User creates a session and chooses worktree mode.
9. Daemon resolves final cwd/worktree.
10. Daemon starts a worker.
11. Worker spawns agent PTY and maintains preview state.
12. Daemon broadcasts session/preview/pane updates.
13. User can attach/detach without killing PTY.
14. User can quit/reopen frontend while daemon keeps sessions alive.
15. If a worker exits, only that session exits and last preview is frozen.
16. If daemon restarts, stale running sessions are marked exited.

## Validation status

Validated during this cleanup:

- `npm run build`

Historically validated during development, but not exhaustively rechecked in this cleanup:

- daemon autostart, PID/log/socket handling, and protocol mismatch refusal
- Pi and Claude session creation/resume paths; Codex launch compiles cleanly
- Claude exit resume-handle parsing, named `/branch <dh-name>`, and forked restart paths
- fresh restart/no-resume mode and parent-inherited child titles
- deleted-worktree sessions marked non-restartable/non-mergeable; leftover directory cleanup
- worktree sanitizer and `git worktree list --porcelain` parsing
- preview subscriptions, xterm rendering, frozen `lastPreview`, and activity transitions
- attach request/output/detach/return-to-Ink flow
- sidebar hierarchy, numbering, resize, and persisted in-process width across attach/detach
- resize suppression for agent activity detection
- stale-session cleanup after daemon restart

Not fully manually validated recently:

- Codex session creation
- setup/doctor install flows beyond build-level coverage
- real hook-script worktree creation from main worktree
- real hook-script worktree creation from linked worktree
- fallback worktree creation under `~/.deckhand/worktrees`
- existing-worktree picker in a repo with many worktrees
- worktree delete paths in disposable repos
- branch deletion path for existing-worktree sessions
- attempted deletion of current/main worktree remains blocked
- multiple sessions pointing at one worktree block deletion
- force-kill behavior against stubborn child process groups

## Caveats

- Deckhand is still experimental; state shape and IPC protocol may change.
- Frontend restarts are supported; daemon crash/restart does not preserve running PTYs.
- Preview and panes are read-only snapshots; attach is required for direct interaction.
- Preview text is not equivalent to full styled terminal rendering.
- Attach mode temporarily exits Ink by design.
- Create/worktree picker/kill confirmation are pane replacements, not true modals.
- Worktree support exists but still needs more real-world exercise.
- Codex deterministic resume support is not implemented.
- Terminal/Git/Dev scrollback controls are still future work.
- `src/daemon.ts` still has legacy in-daemon PTY paths alongside the worker model.

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
   - setup/doctor detection behavior
3. Decide whether to remove or formally support the legacy in-daemon PTY fallback paths.
4. Polish create/worktree UX:
   - true overlays/modals
   - better validation and error feedback
   - better truncation/filtering for long paths
5. Clean up attach/detach transition visuals.
6. Add structured daemon logging.
7. Add stronger daemon health/protocol compatibility handling.

Later:

- Persist sidebar width across full frontend restarts.
- Add richer sidebar branch/worktree metadata.
- Add terminal/git/dev scrollback controls.
- Add dev stop confirmation or persisted dev state if useful.
- Monitor long-running macOS `node-pty` behavior under repeated spawn/exit churn.
- Add Codex deterministic resume support if possible.
- Consider embedded terminal rendering only if single-screen interaction becomes important.

## How to run locally

```bash
npm install
npm run build
npm link
deckhand
```

Useful development commands:

```bash
npm run dev       # run UI from source via tsx
npm run daemon    # run only daemon in dev mode
deckhand setup    # check/install supported agents
deckhand doctor   # alias for setup behavior
```

## Final takeaway

Deckhand's foundation is established: daemon-owned long-lived sessions, worker-owned PTYs, explicit attach/detach, split-view Ink frontend, daemon/worker-side rendered Preview pipeline, deterministic Claude/Pi identity where possible, sub-session hierarchy, and agent-agnostic worktree support.
