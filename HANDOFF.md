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
- One worker process per running session (the agent PTY), plus one workspace worker per worktree whose shared Terminal/Git/Dev panes are in use; workers own the live PTYs.
- Supported agents: `claude`, `pi`, `codex`.
- Session create/restart/kill/remove flows, including resume/fresh restart where supported.
- Sub-sessions under parent sessions, with clean and forked variants (Claude `--fork-session`, Pi `--fork`, `codex fork`).
- Repo-scoped session list with persisted manual ordering among siblings and collapsible subtrees.
- Worker-side terminal preview rendering with `@xterm/headless`.
- Read-only Preview focus mode with scrollback; Claude gets synthetic wheel input because its TUI behaves differently.
- External attach/detach for agent, terminal, git, and dev PTYs.
- Worktree modes: no worktree, new managed worktree, existing/attached worktree.
- Safe worktree deletion, optional branch deletion, and cleanup of leftover directories/remnants.
- Merge/squash-merge of a session worktree into the Deckhand launch/current branch without committing: a preview (target, commits, diff stat, uncommitted files with a commit-first toggle, target checks), a two-key conflict view (keep / abort), and a merged marker (`✓`) per worktree, shared by every session in it, also set automatically when the branch was merged elsewhere (see *Merge behavior*).
- A per-session done marker (`D`, `☑`), independent of merged; `M` is worktree-only.
- Terminal (shell) tab, shared by every session in the same workspace (worktree), started on first view and usable after the agents exit.
- Git tab: a native **Changes** view of the workspace (VS Code-style groups, line counts, diff preview, stage/unstage, open in editor at the first change), polled by the daemon while watched; `o` attaches the workspace's shared `lazygit` (when installed) for everything else.
- Dev tab, shared by every session in the same workspace (worktree) and independent of their agents, powered by `devCommand` from effective settings (global defaults, overlaid by a trusted repository `deckhand.json`; legacy `dev_command` fallback).
- Two-layer configuration: global `defaults` in the user config plus one repository `deckhand.json` in the main checkout (worktree copies ignored) that applies only when trusted (its defaultAgent/defaultWorkspace preselect the picker regardless), with an inline content-fingerprint review shown only right before repository config would run (lists show everything, untrusted actions marked), one editable Settings grid (C: a Global and a This repo column, the cursor a cell) for both layers, self-edits that keep a trusted file trusted, archive/search/filter, handoffs, optional lifecycle hooks/notifications, and conservative cleanup inspection. User-facing behaviour: `docs/no-brainers.md`.
- `DECKHAND_HOME` state namespaces and an isolated dev launcher (`scripts/deckhand-dev.mjs`, `docs/dev-build.md`).
- Notes tab: Markdown files per session plus one per worktree (and one for the main checkout) shared by its sessions, a cursor editor with checklists (`☐`/`☑`, Ctrl+X), autosave that never overwrites edits made in an editor, and live pickup of those edits (see *Notes*).
- Preview-change-based active/idle detection without agent hooks.
- Frozen last preview frame for exited sessions.
- Stale-session cleanup after daemon restart.
- Daemon PID/log files and protocol-version safeguards.
- `deckhand setup` / `deckhand doctor` helper for checking/installing supported agents.
- Agent versions and updates (U): installed vs latest (npm) per agent, the agent's own updater run by the daemon, the version each session launched with, and a dim `↑` on running sessions older than the installed agent. No restarts.

## Architecture

### Core model

- Frontend is disposable UI/controller.
- Daemon is the source of truth for persisted session metadata and live control routing.
- Workers own PTY runtime: session workers for one session's agent, workspace workers for the companion panes (Terminal, Git, Dev) shared by every session in one worktree.
- A worker crash exits only that session (or stops only that workspace's panes), not the daemon.
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
- start/stop session workers and workspace workers
- route attach/input/resize/snapshot requests to workers
- receive worker snapshots, output, and lifecycle messages
- broadcast session, preview, terminal, git, dev, and changes events
- read Git status/diffs and stage/unstage for the Git tab's Changes view (`src/changesGit.ts`), polling watched workspaces
- manage worktree creation/deletion/merge safety and the per-worktree merge/deleted records
- own the notes files (`notes/`): load, migrate, project into sessions, revision-checked saves, watch for outside edits
- manage daemon PID, socket lifecycle, and logging

The daemon owns no PTYs. The agent PTY lives in the session worker; Terminal, Git (lazygit) and Dev live in the session's workspace worker, which does not depend on the session's worker, so they work the same for starting (once the worktree is prepared, e.g. during setup), running and exited sessions. A session whose worktree is still being prepared has no workspace, and its Terminal/Git/Dev panes say *unavailable: its worktree is not ready yet* until it has one.

### Workers (`src/sessionWorker.ts`)

Each session worker (`--session-worker`) owns the agent PTY of one session and its `@xterm/headless` preview model; it refuses pane commands for any other target. Workers spawn agents with persisted `session.args`, not just the bare command, so restarts can resume supported agents.

Each workspace worker (`--workspace-worker`, `WorkspaceWorker`) owns the companion panes of one workspace in a `PaneHost` (same file): PTY spawn in the workspace root, previews, throttled `<pane>-updated` records, attach/input/resize, and stop (SIGHUP, SIGKILL after a grace period). It hosts `WORKSPACE_PANES` (Terminal, Git, Dev):

- Terminal: `$SHELL` (no args). Git: `lazygit`, resolved per spawn with `$SHELL -ic 'command -v lazygit'` (if missing, the watch/attach fails with *lazygit is not installed or not on PATH*). Both spawn lazily on the first `snapshot`/`attach`; a shell or lazygit that exited is respawned by the next snapshot or attach (restart on view: switching tabs, a resize or a lifecycle change re-watches). The UI does not send `watch-git` (the Git tab shows the daemon's Changes view), so in practice lazygit starts on the first `attach-git` (`o` on the Git tab); `watch-git`/`git-updated` remain in the protocol (the daemon test uses them).
- Dev: only `start-dev` spawns it; snapshots never restart it, so its output and exit code stay visible.
- `idle` (no pane runtime, live or exited, and none starting) is reported after `stop-dev` and on the `idle` command; once `shutdown` arrives, the worker refuses every other command (nothing may start in a worktree about to be removed).

Worker stdout/stderr are appended to per-worker files under `~/.deckhand/workers/`.

### Workspaces and workspace workers (`src/workspace.ts`)

A **workspace** is the git worktree a session runs in. It is derived, never persisted: `workspaceKey(session)` is the resolved worktree path for managed/attached sessions (including a session attached to the main checkout), otherwise the launch checkout root (`launchWorktreeRoot`, falling back to `cwd` for old records). It is undefined, so the session has no workspace and no Terminal/Git/Dev, when the worktree was deleted (`worktree.deletedAt`, projected from the worktree record onto every session of it — see the next section — so no session of a deleted worktree shares a workspace with a new worktree later created at the same path) or while a requested new/existing worktree is not prepared yet (it would otherwise resolve to the launch checkout). The daemon also treats sessions in `preparingSessions` as having none, until `launchWorktreeRoot` is resolved by Git; it reports that as a pane record for the session without `workspace`, which `workspacePaneUnavailable` (UI) turns into the *not ready yet* message. Keys are lexical; every input path comes from Git (`rev-parse --show-toplevel`, `worktree list`), so they agree.

Terminal, Git and Dev are owned by the workspace: one workspace worker per workspace whose panes are in use, spawned by the daemon on the first `watch-terminal`/`watch-git`/`attach-terminal`/`attach-git` (lazy) or `start-dev`/`run-action`; `watch-dev` never spawns one. PID/log at `workers/workspace-<sha256(key)[:16]>.pid|.log` (`workspaceWorkerId`). `openWorkspace` refuses a worktree in cleanup (`assertWorkspaceAvailable`) or whose directory is gone (*Worktree directory is missing*). Why a process rather than daemon-owned PTYs: PTY I/O and `@xterm/headless` parsing (a chatty dev server, a busy shell) stay out of the daemon's event loop, a PTY crash cannot take the daemon down, and the daemon owns no PTYs at all. The cost is one idle-ish Node process per workspace in use.

Semantics (`InkDaemon` `requireWorkspace`/`openWorkspace`/`watchWorkspacePane`/`attachWorkspacePane`/`requestPane`/`retireIfIdle`/`retireWorkspace`/`publishWorkspacePane`/`syncDevRunning`):

- Every Terminal/Git/Dev request (`watch-*`, `attach-*`, `*-input`, `*-resize`, `*-detach`, `start-dev`, `stop-dev`, `run-action`) still names a session; the daemon resolves its workspace and acts on the one shared pane. Sessions in one worktree share one shell, one lazygit and one Dev; another worktree has its own. Starting Dev from any session starts (or reuses, same command) the shared process; stopping from any stops it. The Dev command is resolved from that session's effective settings (`resolveDevCommand`, trust review in the UI unchanged).
- The panes run independently of agents: view/attach (and Dev start/stop, actions) work for starting (once prepared), running and exited sessions alike; the UI gates on "has a workspace", not "is running".
- Lifetime: the worker is retired (all panes stopped, awaited) when kill-with-delete removes the worktree (after the cleanup checks pass, before `git worktree remove`), when the workspace's last session is removed (nothing could show or stop the panes otherwise; a shell never exits on its own), and when the daemon stops (`cleanup` retires every workspace; on a daemon crash the worker's IPC disconnect stops its panes). It is also retired once it is **idle** and no pane request is in flight (`busy`): after `stop-dev` (no Terminal/Git/exited-Dev left) and after a failed pane request (`requestPane` → `retireIfIdle`, e.g. lazygit is not installed), so a failed view never leaves an empty worker behind. Its own panes never make it idle: a shell or lazygit that exited, like a Dev that exits on its own, keeps its runtime so the output stays visible and the worker stays.
- Records: the worker's records carry no session; the daemon stamps each watching client's session ID plus `workspace` (`TerminalRecord`/`GitRecord`/`DevRecord`) and sends `<pane>-updated` to every client watching that pane on any session of the workspace (`publishWorkspacePane`). The last record per pane is kept in `WorkspaceRuntime.records`: on an unexpected worker exit viewers get it back marked not live; on retirement they get an empty not-live record (only panes that existed). One attach per workspace pane (`terminal/git/dev command is already attached elsewhere`); output goes to the attaching socket as `<pane>-output` under its session ID; retiring the worker sends `<pane>-detached` to it. Each PTY is sized by the last watch/attach/resize of that pane (last viewer wins).
- `devRunning` stays on `SessionRecord` (so every response/broadcast carries it) but mirrors the workspace: `syncDevRunning` sets it on every session of the workspace, exited ones included, whenever the worker reports a Dev record, and `saveSession`/`saveWorktreeRecord` re-derive it for sessions that join or leave a workspace (create, worktree deleted). Live PTYs never survive a daemon restart, so `markAllNonExitedSessionsExited` clears it on startup.

Adding another workspace pane: a PTY pane is a `PaneTarget` in `WORKSPACE_PANES` (worker) plus `WorkspacePane`/`WorkspaceEvent`/`paneUpdatedMessage` and the request cases in the daemon. A non-PTY workspace view does not need the worker: compute it in the daemon from the workspace key (like `workspace-summary`, `src/workspaceGit.ts`) or, if it must push updates, keep a per-workspace watcher and fan out like `publishWorkspacePane` — the Git tab's Changes view (below) is that pattern.

### Worktree records: shared merge/deleted markers (`src/worktreeRecords.ts`)

Merge and deletion markers describe a worktree, not a session, so every session in a linked worktree shares them. State: one `WorktreeRecord` per worktree **incarnation** (`{id, path, createdAt}` plus the markers `mergedAt`/`mergeMode`/`mergeTargetBranch`/`mergeSourceRef`/`mergeSourceSha`/`mergeMarkedManually`/`mergeDetected`/`deletedAt`, and two unprojected fields `baseSha`/`mergeDismissedTip`), persisted in `state.json` `worktrees`; sessions store only its ID as `worktree.id`.

- `mergeSourceSha`: the source commit Deckhand merged (`mergeSourceRef` is a branch name that moves on), or the head of a PR found merged; cleanup treats it and its ancestors as integrated (see *Deletion safety*). Never set by `M` (unverified) nor for a kept conflict whose pending record was lost.
- `mergeDetected`: `'ancestor'` or `'pr'` when the marker was set by merge detection rather than by `m`/`M`.
- `baseSha`: where the branch started (its reflog's `branch: Created from …` entry, else HEAD when the record was created; filled in lazily from the reflog for records older than the field). Merge detection needs a commit beyond it.
- `mergeDismissedTip`: the branch tip when `M` cleared the marker; detection leaves the record alone while the tip is unchanged. Dropped whenever markers are set.

- **Scope**: sessions running in a linked (non-main) worktree: managed or attached, and sessions without their own worktree (mode `none`, e.g. sub-sessions from `N`) launched in one (`isLinkedWorktreeRoot`, from `git worktree list`). The main checkout (mode `none` there, or attached to the main worktree) has no record and no merge marker: `M` refuses there (*… Use D to mark it done*; it can still clear a leftover marker), and the done marker (`doneAt`, per session) is what such sessions use.
- **Incarnations** (`joinWorktree`, called in `finishCreateSession` with no await between the lookup and setting the session, so the record cannot be dropped as unreferenced in between): attaching an existing worktree, or a mode-`none` session launched in one, joins the live record at that path (at most one per path; created if none). A worktree Deckhand just created (`origin: 'created'`) always starts a new record; a live record still at that path means the old worktree was removed outside Deckhand, so it is marked deleted (`superseded`). A deleted record is never joined, so old sessions never share a later worktree at the same path.
- **Projection**: the daemon's `sessions` map (`SessionMap`) projects the record's markers into `session.worktree` on every `set`, so every reader — `workspaceKey`, restart/merge/create-pr guards, `cleanupBlockers`, handoff export, the UI (sidebar `✓`, hints, `M` status) — keeps reading `session.worktree.mergedAt`/`deletedAt`. `persist()` writes `storedSession` (projection stripped) plus the records: the record is the only persisted copy. `saveSession`/`patchSession` return the projected session.
- **Writes**: `saveWorktreeRecord` replaces a record, re-sets (re-projects, re-derives `devRunning`) and bumps `updatedAt` of every session referencing it, persists once, broadcasts `session-updated` for each, and re-syncs Changes watches. Used by merge success and a kept conflicted merge (`saveMergeMarkers`), `M` (`markSessionMerged` → `saveMergeMarkers`: replaces or clears the merge markers wherever they live; clearing stores `mergeDismissedTip`), merge detection (`markDetectedMerge`), kill-with-delete (`markDeleted`, after `stopChangesWatch` → `retireWorkspace` → `removeWorktree`) and superseding. `updateWorktreeRecord(…, false)` patches an unprojected field (`baseSha`) without touching sessions.
- **Lifetime**: `removeSession` drops the record once no session references it; loading drops unreferenced records too.
- **Migration** (`migrateWorktreeRecords`, run by `loadState`; `markAllNonExitedSessionsExited` writes a migrated state back at daemon start): sessions without `worktree.id` are grouped by worktree root (their own linked worktree; for mode `none`, the launch root when some session owns a linked worktree there). Each recorded `deletedAt` ends one incarnation: the session that deleted it belongs to it, any other session to the first deletion at or after its last launch (`agentStartedAt`, else `createdAt`), and the rest to the live incarnation (joining an existing live record at that path). Each incarnation takes the most recent merge marker of its sessions (a mode-`none` sub-session's top-level `M` marker included, then removed from the session). It also repairs stored state: stray markers on sessions with an ID are dropped, a missing referenced record is rebuilt from the session. Idempotent; unit-tested (`tests/worktreeRecords.test.ts`).
- **Done migration** (`migrateDoneMarkers`, run by `loadState` right after `migrateWorktreeRecords`, written back at daemon start the same way): before `D`, `M` doubled as "done" in the main checkout. Every session without a worktree record that still has its own merge marker (top-level `mergedAt`/`mergeTargetBranch`/`mergeSourceRef`/`mergeMarkedManually`, or the merge markers under `worktree` of a main-worktree attach) gets `doneAt = mergedAt` (an existing `doneAt` wins) and loses those fields. Worktree records and sessions with a record are untouched. Idempotent; unit-tested beside the record migration (nine production-like `mergeMarkedManually` main-checkout sessions through `loadState` and the daemon-start write-back).

### Notes (`src/notes.ts`, `src/notesStore.ts`, `src/notesFlow.tsx`, `src/notesPane.tsx`)

Two notes per session, both plain Markdown files under `notes/` in the state directory, the single source of truth:

- **Session note**: `notes/sessions/<session id>.md`. Deleted when the session is removed (`removeSession`).
- **Shared note** (`sharedNoteIdentity`): a session with a worktree record (`worktree.id`, i.e. a linked worktree incarnation, mode-`none` sub-sessions in it included) shares `notes/worktrees/<record id>.md` with every session of that incarnation; a new worktree at the same path is a new record, so it starts empty. A session without a record but with a workspace (the main checkout: mode `none` there, or attached to the main worktree) shares the repository note `notes/repos/<sha256(workspaceKey)[:16]>.md` (`repoNoteId`, lexical like workspace keys). A session still preparing its worktree has none. A deleted worktree's note stays visible, read-only (`sharedNotes.readOnly`), while its record exists; it is deleted when the record is dropped (last referencing session removed). Repository notes are never deleted. Nothing is deleted at startup (orphan files are left alone, so a reset state.json never costs notes).
- **Daemon** (`NotesStore`, one per daemon): `load` reads every file into memory at start (after `migrate`, below); `SessionMap.set` projects `notes` (the session note's text, for the existing readers: search, handoff, merge screen, sidebar), `notesFile` (`path`, `revision`, `tooLarge`) and `sharedNotes` (`kind` `worktree`/`repo`, `id`, `path`, `text`, `revision`, `tooLarge`, `readOnly`) into every session, and `persistedSession` strips them (and the worktree markers) before `saveState`. A note change re-sets and broadcasts `session-updated` for every session showing it (`notesChanged`); notes changes never write state.json or bump `updatedAt`.
- **Revisions and conflicts**: a revision is a content hash (`noteRevision`, sha256[:16]; a missing file is the empty text). `save-note` names the revision the UI edited; the store re-reads the file from disk inside a per-note queue and writes only when it still has that revision (atomically: temp file `.<name>.tmp-…` in the same directory, then rename, 0600). Otherwise nothing is written and the response is `{saved: false, session}` with the file as it is. Shared saves also name the note (`noteId` = `kind:id`) so a session that moved to another note is refused. Text is cut at 50 000 characters (`MAX_NOTES_CHARS`); a file longer than that (or over 1 MB) is projected cut with `tooLarge` and refused for saving (edit it in the editor). Saving the same text writes nothing; saving `''` to a missing file creates nothing.
- **Watching**: `fs.watch` on each of the three directories (non-recursive, so editors that save via rename are seen), per-file debounce of 120 ms, then a re-read in the same per-note queue; a changed revision updates the cache and broadcasts. A watcher event without a file name rescans the directory; a watcher error retries after 1 s. Deckhand's own writes re-read as unchanged, so they never echo. Hidden/temporary files (leading `.`, not `.md`) are ignored.
- **Migration** (`NotesStore.migrate`, daemon start, before `load`): every stored session with a `notes` string gets its file written if missing (an existing file wins, so a re-run or a later edit is never overwritten); when any stored session still had `notes`, state.json is written again without them. Idempotent; covered by `tests/notesDaemon.test.ts`.
- **`open-note`** creates the file (empty, `wx`) if missing and returns its path; the UI opens it with `openInEditor`.
- **UI** (`useNotesFlow`): one draft per note key (`session:<id>`, `worktree:<id>`, `repo:<hash>`; a shared note is one draft for every session showing it, so its cursor and unsaved text follow it) holding text, cursor, the last known file text and revision, the save in flight and the editor's scroll. Edits schedule a save 300 ms later; one save per note is in flight at a time (typing during it is saved after it returns); leaving notes focus saves at once, and E/Ctrl+O save before opening. An incoming record with a new revision is applied silently when nothing is unsaved (an editor's edit shows up), its own save's echo only updates the revision, and anything else reloads the file and says so (*changed outside Deckhand … your last unsaved edit was not saved*); a refused save does the same. Notes focus turns bracketed paste on (`?2004h`, off on leaving): between the markers Tab and Enter are text.
- **Editor** (`src/textEditor.ts`, shared with the JSON editor): `editText` gained word jumps (Alt/Ctrl+←→, Alt+B/F), word deletion (Alt+Backspace via `normalizeTerminalKey`'s ESC DEL, Ctrl+Backspace, Ctrl+W, Alt+Delete) and, through `EditOptions`, a character limit, Ctrl+A/Ctrl+E as line start/end instead of select-all, Tab left to the caller, and Up/Down/PageUp/PageDown by soft-wrapped rows (`wrapRows`: word wrap at the last fitting space, mid-word otherwise, one column per code point; `moveVisual`, `wrappedEditorLines`, `scrollTopFor`). Checklists (`src/notes.ts`): `- [ ]`/`* [x]`/`+ [X]`, indented or not (`parseChecklistLine`); Ctrl+X `toggleChecklist` (adds `- [ ] `, or `[ ] ` after a plain bullet), Ctrl+T `insertChecklistItem` (not Ctrl+N: editors commonly bind it in their integrated terminal, e.g. new terminal), Enter `continueChecklist` (an empty item ends the list).
- **Layout** (`notesLayout`, pure): the shared section (header `Worktree · <branch | main checkout> (shared by N sessions)` with `☐ N open`/`☑ N done`, then `deleted, read-only`/`too large` flags), a rule, `This session · <title>`. `budgetSections` gives each body its need when both fit (the session section the rest), else at least 3 rows each and the rest in proportion to what is missing, the focused section's share counting double; tiny panes favour the focused one. An empty section not being edited is one muted line (`No worktree notes · tab to add`). Read mode renders `☐`/`☑` items (checked muted, hanging indent) and bold headings, cut with a muted `+N more lines`; the edited section shows the raw text with an inverse cursor, scrolled to keep it visible. Sidebar details add `☐ 3 open (2 worktree)` (`openChecklistText`; shortened to `(2 wt)`, then without the split) as the first line to go.

### Git tab: the Changes view (`src/changesModel.ts`, `src/changesGit.ts`, `src/changesFlow.tsx`, `src/changesPane.tsx`)

The Git tab shows the workspace's changes like VS Code's Source Control panel; lazygit is only attached (`o`). No worker or PTY: the daemon runs Git in the workspace root.

- **Model** (`changesModel.ts`, pure, unit-tested): `groupChanges` turns porcelain-v2 entries (`parseStatus(...).entries`, `src/workspaceGit.ts`; `StatusEntry` = kind, XY, path, origPath) plus numstat (`parseNumstat`, renames keyed by the new path; a conflict's doubled combined record keeps the last) into groups in VS Code order — `conflicts` (`u` records, letter U, `conflict` = XY), `staged` (X ≠ `.`; R/C keep `origPath`), `unstaged` (Y ≠ `.`; so a partially staged file is in both), `untracked` (`?`) — sorted by path, capped at `MAX_CHANGES` (2000) in group order with exact `counts` and per-group `omitted`. Also `changeRows` (headers/entries/"+N more"), `reselect`/`groupOffset` (selection by (group, path); if gone, the same offset within that group, else the first entry below an emptied group), `stageMode`, `changeLabel` (name first, `old → new`), `firstChangedLine` (new-side line of the first change in the first hunk, combined `@@@` diffs included; 1 without a hunk), `classifyDiff` (meta only before a file's first hunk; one prefix column per parent) and `untrackedDiff`.
- **Git I/O** (`changesGit.ts`): `readChanges` = `git --no-optional-locks status --porcelain=v2 -z --branch --untracked-files=all` + `diff --cached --numstat -z -M` + `diff --numstat -z` in parallel; untracked line counts from the files themselves (lstat, regular files ≤ 1 MB, first 500, NUL sniff for binary, cached by size+mtime per workspace). Throws *Worktree directory is missing* for a gone directory. `readChangeDiff`: bounded spawn (256 KB, then cut at the last newline; 8 s timeout): staged → `diff --cached -M -- <orig> <path>`, unstaged/conflicts → `diff -- <path>` (combined for conflicts), untracked → file read (symlink: its target; directory: nothing; binary sniff), all with `--literal-pathspecs --no-color --no-ext-diff`. `applyStage(cwd, snapshot, mode, target?)` validates against a fresh snapshot (a target must be listed on the matching side, else *not among the staged/unstaged changes*), then: stage = `git --literal-pathspecs add -A --pathspec-from-file=- --pathspec-file-nul` (paths NUL-separated on stdin; deletions and conflict resolution included); unstage = `restore --staged` the same way (both paths of a rename), or `rm --cached -r -q --ignore-unmatch` without HEAD; stage all = `git add -A`, except with conflicts present: the unstaged/untracked paths only (`skippedConflicts` reported; mirrors VS Code's confirm-before-staging-conflicts); unstage all = every staged path (never `git reset`, which would also drop MERGE_HEAD). Pathspec-from-file needs Git ≥ 2.26.
- **Daemon** (`changeWatches: Map<key, ChangesWatch>`): `watch-changes` stores `watchedChangesSessionId` on the client, `syncChangeWatches` polls exactly the workspaces some client watches (interval `CHANGES_POLL_MS` 2 s, unref'd, skipped while a read is queued or running) and forgets the rest; it runs on watch, client disconnect, `saveSession` (sessions joining/leaving a workspace) and session removal. Every Git run of a workspace (status, diff, stage) goes through its `queue` (no overlap); `refreshChanges` shares a read that is queued but not started, stores `last`, and `publishChanges` sends `changes-updated` (stamped with each watcher's session ID and `workspace`) to every client watching any session of the workspace, only when the record's JSON changed. A failed read (cleanup in progress, directory gone, not a repo) becomes a record with `error`. `changes-diff` reuses a status read younger than 2.5 s to validate (group, path). `change-stage` re-reads status inside the exclusive section, applies, refreshes (pushing to every watcher) and responds `{changed, skippedConflicts, changes}`. Kill-with-delete stops the workspace's watch before removing the worktree; daemon cleanup stops all. Errors for sessions without a workspace use the Git pane label (*Git is unavailable: …*); the watch itself answers like the other panes (no `workspace` → `workspacePaneUnavailable`).
- **UI**: `app.tsx` owns the record (`watch-changes` while the Git tab is shown, `watch-changes` without a session when leaving it, `changes-updated` filtered by the selected session) and mode `changes-focus`; `changesFlow.tsx` owns selection, the debounced (60 ms) diff fetch (refetched when the record changes) and the focus keys; `changesPane.tsx` renders: a header (workspace path · branch, counts), the list (browse: from the top with "↓ N more"; focus: windowed around the selection with its group header), and in focus the diff (side by side from 100 columns of pane width, else stacked under the list). `o` on the Git tab is ready once the Changes record confirms the workspace (lazygit starts on attach). Enter/e: `openInEditor(file, onError, line)` (`src/desktop.ts`: `-g <abs>:<line>` for the cursor/code CLIs; the macOS `open -a` fallback ignores the line); a file missing on disk (deleted) gets a status message instead.

## Design rules

- Lifecycle and activity are distinct:
  - lifecycle `status`: `starting`, `running`, `exited`
  - activity `agentStatus`: `unknown`, `active`, `idle`
- Activity is inferred from visible preview changes, not agent-specific hooks. Optional lifecycle hooks set a separate advisory `attention` field.
- Do not overload lifecycle status to mean activity.
- Terminal, Git and Dev belong to the workspace, not the session: gate them on "has a workspace" (`workspaceKey`, plus the daemon's *not ready* record), never on "is running"; agent exit must not stop or detach them.
- Merge/deleted markers of a linked worktree belong to its worktree record: write them with `saveWorktreeRecord`, never onto a session (the session copy is a projection and is not persisted).
- Notes live only in their files (`notes/` in the state directory): `notes`, `notesFile` and `sharedNotes` on a session are projections the daemon recomputes on every set and strips before persisting. Write notes with `NotesStore.save` (revision-checked), never by setting them on a session.
- Resize-only redraws must not mark idle agents active.
- Preview is a rendered plain-text snapshot, not a full embedded terminal emulator.
- Preview/pane snapshots are read-only; attach mode is required for direct interaction.
- Attach mode intentionally exits Ink temporarily and gives stdin/stdout directly to the selected PTY.
- PTY sizing is per PTY:
  - Preview sizes the agent PTY to the preview viewport.
  - Terminal/Dev size their shared workspace PTYs to the pane viewport of the last viewer (last watch/resize wins); lazygit is sized by its attach.
  - Attach mode sizes the active PTY to the full terminal.
  - Returning from attach reapplies pane sizing.

## UI behavior and controls

### Layout and indicators

- Sidebar layout (`src/sidebarModel.ts`, pure and unit-tested; `src/sidebar.tsx` renders): a row is `<cursor><number> <tree><status> <title> … <markers> <agent>`. The cursor column is the frame's left padding column (`›` on the selected row). Numbers carry no brackets, are right-aligned to the widest and dim unless selected. Example (width 34):
  ```
  │ Sessions         all 7/7 · ! 2 │
  │╎ 1 ▾ ⠋ auth refactor       ▶ ✶ │
  │› 2   ↳ ● write tests         π │
  │╎ 3   ⑂ ○ try alt approach    ✶ │
  │  4 ? fix flaky checkout e2e… ◇ │
  ```
- Status glyph before the title (`statusGlyph`): spinner for starting/active/working, green `●` idle, yellow `◌` running with unknown activity, gray `○` exited; with agent signals `?` needs input, `◆` response ended, `!` failed (also failed/interrupted exits), `⌛` rate-limited.
- Tree: sub-session rows are indented two columns per level (up to 4); clean children show `↳`, forked `⑂`; parents show `▾` / `▸` (expanded/collapsed, `c`). Rows without children carry no placeholder column.
- Right-hand suffix, in order: `▶` Dev running, `▣` archived, `!` cleanup error, `✓` merged (the worktree's marker), `☑` done (`doneAt`, `DONE_MARKER`; the glyph notes use for a ticked item), `+N` collapsed/hidden sub-sessions, `↑` outdated (dim: a running session whose `agentVersion` is older than its agent's installed version, `sessionOutdated`), then the agent glyph (`✶` Claude, `π` Pi, `◇` Codex) last. When the title would get fewer than 4 columns (or fewer than it needs), `↑`, `+N`, `✓`, `☑`, `▣`, `!` and `▶` are dropped in that order; the agent glyph stays.
- Workspace awareness: `devRunning` mirrors the workspace, so `▶` is shown once per workspace (`workspaceKey`), on its first row on screen; sessions without a workspace key keep their own. Rows on screen sharing the selected session's workspace (its Terminal/Git/Dev) get a dim cyan `╎` in the cursor column. The selected session's Dev tab still shows the green `●` suffix.
- Dimming: archived rows are dimmed (title and glyphs; `▣` and `☑` stay readable) in every filter except `archived`; there the non-archived ancestors shown only for tree context are dimmed instead. Done rows that are not dimmed by those rules get `SidebarRow.done`: only their title and tree are muted (gray, not dim), so the status glyph keeps its color (a signal on a done session still shows) and `☑` stays readable. The selected row is never dimmed.
- Header (`sidebarHeader`): `Sessions` left; right the plain row count in the default view, otherwise the filter and/or `/query` with `shown/total` highlighted in cyan; then `· ! N` (yellow) when N non-archived sessions need attention (`sessionNeedsAttention`, the ones `!` cycles). Narrow widths drop the title first, then cut the filter/search, keeping the count and `! N`. The app header no longer repeats filter/count: its second row is the repo path at full width (the transient `f` message stays in the footer).
- Details block (`sessionDetails`): pinned to the bottom of the sidebar in the rows the list leaves free (the list has priority): a separator, the full title (wrapped, two lines with five free rows, else one, ending in `…`), `<agent glyph> <agent> · <state in words> · <age>` (an outdated session first tries `✶ claude 2.1.287 · 2.1.290 installed · …`, then `✶ claude 2.1.287 → 2.1.290 · …`, then without the name and the age, before the plain forms; `statusWords` mirrors `statusGlyph`; the age counts from the attention signal while running, else `agentStatusUpdatedAt`, which is also set at exit; `now`/`12m`/`3h`/`2d`; narrow widths drop the agent name, then the age), and the location: `⎇ <branch>` (a sub-session without its own branch takes one from a session sharing the workspace), `main checkout`, `worktree deleted` or `preparing worktree`, plus `shared with N`, `▶ dev`, `merged`, `done 2d ago` (`doneText`: `done now` within a minute), `archived` (cut at whole markers, ` …` for the rest). With three free rows the location goes; below three, or without a selection, the block is hidden. Only session data is used (no daemon requests, no Git). The ages need a clock: App passes `Date.now()` on every render and, when no spinner is animating, schedules one render exactly when the first shown age (state or done) would change (`msUntilAgeChanges`).
- Sidebar row numbers are positions in the list. With 10 or fewer visible sessions, single digits jump immediately and `0` selects row 10; with more than 10, numeric input is briefly buffered for multi-digit selection.
- The in-app help (`?`) has a Sidebar topic listing every glyph.
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
- `v` enters Preview focus mode for running sessions; on the Git tab it enters Changes focus (`j`/`k`/arrows, `g`/`G` select; `space` stage/unstage; `a`/`A` stage/unstage all; `enter`/`e` open in editor; `J`/`K` scroll the diff by 3, PgUp/PgDn by a page; `o` lazygit; `esc`/`v` back)
- `o` attaches to selected session's active pane (Terminal/Git/Dev also for exited sessions, as long as the session has a workspace):
  - Preview => agent
  - Terminal => the workspace's shared shell
  - Git => the workspace's shared lazygit (started on this attach; the tab itself shows the Changes view)
  - Dev => the workspace's shared dev command PTY
  - Notes => enter notes edit/focus mode
- `E` on the Notes tab opens the active note (the session's; the worktree's after editing that) in Cursor / VS Code, creating the file if missing (`e` is the actions picker and `O` the worktree, so neither could be reused)
- `O` opens selected session directory/worktree in Cursor if available, otherwise Code (`cursor`/`code` CLI; macOS fallback is `open -a Cursor`)
- `m` opens the merge confirmation for worktree-backed sessions (preview, `space` commit-first toggle, merge/squash/cancel; see *Merge behavior*); a conflicted result opens the conflict view (`enter` keep, `a` abort)
- `M` toggles the worktree's merged marker (every session of it); worktree-only: in the main checkout it says *Not in a worktree, so there is nothing to mark merged. Use D to mark it done* (the daemon refuses the same way)
- `D` toggles the selected session's done marker (`set-session-done`; any session, sub-sessions in a worktree included; independent of merged and archived)
- `x` kills selected running session
- `X` force-kills selected running session; workers send SIGTERM first and SIGKILL after a short delay if still alive
- for worktree-backed sessions, kill confirmation offers keep/delete/delete-branch/cancel when applicable
- `s` resume/restart selected exited session
- `S` fresh-restart selected exited session without using prior parsed/persisted resume handle
- `d` focuses Dev; when already on Dev, starts/stops the Dev command of the selected session's workspace (any session with a workspace, running or not)
- `backspace` removes selected exited session
- `r` refreshes/resubscribes
- `U` opens the Agents screen (versions and updates, see *Agent versions and updates*)
- `?` opens help
- `q` quits UI; daemon and running sessions continue

### Notes

- Two sections: the worktree's shared note (the main checkout's repository note for sessions there) above the session's own; see *Notes* under Architecture for storage and sync.
- Selecting the Notes tab is read mode (rendered checklists, from the top, `+N more lines` when cut); `o` enters notes focus (the section last edited, the session's by default), `esc` leaves it (and saves at once).
- In notes focus: type anywhere, arrows, Home/End and Ctrl+A/Ctrl+E (line), Ctrl+Home/End (note), Alt/Ctrl+←→ and Alt+B/F (words), Alt+Backspace/Ctrl+W (delete word), Up/Down and PageUp/PageDown by soft-wrapped rows, Enter (continues a checklist), paste (bracketed: Tab/Enter inside are text), `tab` switches section (cursor kept per note; a deleted worktree's note is read-only and skipped), Ctrl+X toggles the line's checkbox (adding `- [ ] ` if none), Ctrl+T inserts an item below, Ctrl+O opens the edited note in the editor. Ctrl+X/T/O/W are plain control bytes (CAN, DC4, SI, ETB), which raw mode delivers — unless the host terminal keeps the key: an editor's integrated terminal can bind Ctrl keys (one user's Cursor binds Ctrl+N/E/W/R with `terminalFocus`), so every line/word key keeps a non-Ctrl alternative.
- Notes autosave (300 ms debounce) with the revision they were based on; an edit made in an editor meanwhile wins and the UI reloads it with a message.

### Preview focus and scrolling

Preview focus is read-only for most agents and scrolls Deckhand's worker-side xterm scrollback snapshot. Claude Code behaves more like a TUI, so Preview focus sends synthetic SGR mouse-wheel events to the Claude PTY instead of only scrolling Deckhand state.

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
- Attach mode mirrors bracketed-paste state across the PTY boundary: agent attaches enable bracketed paste on the outer terminal, while Terminal/Git/Dev attaches enable it only when the workspace worker observed the child PTY request `?2004h` (`terminalModes` in the attach response, passed through by `attachWorkspacePane`). This prevents multi-line paste from being delivered as separate Enter presses without forcing paste markers into arbitrary programs.
- A Terminal/Git/Dev attach ends only on detach or `<pane>-detached` (workspace retired); the agent exiting does not end it. A shell or lazygit that exits while attached does not detach automatically (unchanged; press the detach key).

## Worktree behavior

### Creation

New worktree creation is agent-agnostic. The daemon resolves/creates the target cwd, then launches the selected agent normally in that directory.

Creation strategy:

1. Use `.claude/scripts/create-worktree.sh` in the current worktree root if present.
2. Else use `.claude/scripts/create-worktree.sh` in the main/original worktree root if present.
3. Else fall back to built-in `git worktree add` at the effective `worktree.location` template (default `~/.deckhand/worktrees/{name}`; `expandWorktreeTemplate`/`worktreeLocation` in `src/worktreeLinks.ts`), reusing an existing worktree/branch.

Hooks 1–2 apply only while enabled: `worktree.hook: false` (global defaults, or the repository file even when untrusted, since it can only disable) makes `loadProjectConfig` skip the script entirely (`disabledHook`: not read, fingerprinted, reviewed or run). `loadProjectConfig(cwd, user)` therefore takes the user config; every daemon caller passes it so fingerprints agree.

The fallback (`fallbackCreateWorktree` in `src/git.ts`) names the branch from `worktree.branchName` (`{name}`/`{user}`, checked with `git check-ref-format --branch`) and starts it per `worktree.branchFrom`: `current` (launch HEAD), `default` (local origin/HEAD target, else main, else master) or `origin` (bounded non-interactive `git fetch origin +refs/heads/<b>:refs/remotes/origin/<b>`, failure aborts creation; `--no-track`). The base it used is returned as `baseRef` and stored on the session (cleanup / create-pr `--base`); reused worktrees/branches keep the old launch-branch baseRef. A hook ignores location/branch settings.

After a worktree is newly **created** (by either path, never for reused/attached ones), `applyWorktreeLinks` (`src/worktreeLinks.ts`) links the effective `worktree.symlink` / `worktree.files` entries. It never throws: missing sources, existing real content (kept), and parents that resolve outside the worktree (checked segment by segment before any mkdir, then by realpath) become notes stored as `worktree.links` on the session (shown in `i`, logged by the daemon). Existing symlinks are replaced atomically (temp link + rename). Effective settings merge in `resolveSettings` via `mergeWorktreeSettings`.

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

Worktree deletion is guarded in two layers (`cleanupBlockers`/`inspectSessionCleanup` in `src/daemon.ts`, `inspectWorkspaceCleanup` in `src/workspaceGit.ts`):

- structural blockers, never overridable: main worktree, the session's launch worktree, a worktree used by another non-exited session, no/unregistered worktree, and for branch deletion a protected or changed branch
- data reasons, overridable only by typing `DELETE` (`allowDataLoss`): modified/untracked/valuable ignored files (ignored `node_modules` and any untracked/ignored entry that is itself a symlink, checked with lstat and capped at 2000 checks, are disposable), and, when deleting the branch or on a detached HEAD, commits not reachable from another branch or remote ref
- both are re-checked at kill/exit time, not just when the confirmation opens
- squash merges: Git never considers a squash-merged branch merged, so the "not reachable" count would demand `DELETE` for commits that were integrated. When the worktree has a merge marker with `mergeSourceSha` (set by Deckhand's merge/squash, a kept conflicted merge, or a PR found merged with its head commit present locally), `inspectWorkspaceCleanup(…, {integrated})` excludes that commit too (`rev-list HEAD --not --branches --remotes <sha>`, only after `rev-parse --verify` confirms it). Commits added after it still count; an unmarked worktree, a manual `M` (no SHA) or an unknown/invalid SHA change nothing.

When safe, kill confirmation offers:

- kill only / keep worktree (restartable)
- kill and delete worktree (not restartable)
- kill, delete worktree and branch (not restartable)
- cancel

After Git unregisters a deleted worktree, Deckhand force-removes the worktree path to clear ignored/untracked remnants (only after `git worktree remove` succeeded, so only for a path Git had registered; neither Git nor `fs.rm` follows symlinks, so link targets survive — tested). It prunes empty parents under `~/.deckhand/worktrees`, and for custom locations only the intermediate directories of a slash-containing worktree name (`rmdir`, empty only).

A deleted worktree is recorded on its worktree record (`deletedAt`), so every session of it — not only the killed one — is shown without restart/merge hints, refuses restart/merge/`M` (unmarking still works), and has no workspace (Terminal/Git/Dev say *its worktree was deleted*). The structural blockers above are unchanged.

Branch deletion refuses protected branches `main` and `master`.

### Merge behavior

Git in `src/git.ts` (`mergeWorktreeIntoCurrent`, `operationInProgress`, `commitWorktreeChanges`, `unmergedFiles`, `abortMerge`, `branchCreationCommit`, `branchHasOwnCommits`) and `src/workspaceGit.ts` (`getMergePreview`, `mergedIntoDefault`, the injectable PR lookup); the daemon's `mergePreview`/`mergeSessionWorktree`/`resolveMerge`/`markSessionMerged`/`detectMerges`; the screen model in `src/mergeModel.ts` (pure, unit-tested), rendered by `MergeConfirmPane`/`MergeConflictPane` in `src/app.tsx`.

- normal merge: `git merge --no-commit --no-ff <source>`; squash merge: `git merge --squash <source>`
- target is the Deckhand launch/current worktree, which must be on a branch; source is the selected session worktree's current branch, or its HEAD SHA if detached; source and target roots must differ
- **target checks** (`operationInProgress`, per worktree via `rev-parse --git-path`): a merge (MERGE_HEAD), rebase (`rebase-merge`/`rebase-apply`), cherry-pick, revert, or unresolved conflicts without MERGE_HEAD (a kept squash) refuse the merge with a message naming it (`operationProblem`); the preview shows the same line
- **preview** (`merge-preview`, `getMergePreview`, read-only, a handful of bounded Git calls): `targetRoot`/`targetBranch`, `targetIsMain` (vs `resolveRepoContext().mainRoot`), `defaultBranch` (`resolveDefaultBranch`, local), `HEAD..<source sha>` count plus the first `MERGE_PREVIEW_COMMITS` (6) subjects, the numstat of `HEAD...<source sha>` (files, +/−), the source's uncommitted files (porcelain v2: changed + untracked, not ignored), and `overlap`: target paths with uncommitted changes (status entries, untracked included) that the committed diff touches (`committed`) or that the source's uncommitted files touch (`uncommitted`, which only counts while the toggle is on); capped at 200 each
- **confirmation screen** (`mergeConfirmLayout`, height-budgeted): title; `Into <branch> · <compact target path>` (THEME.warn when `surprisingTarget`: not the main checkout, detached, or not the default branch when that is known); an in-progress warning; `N commits · F files +A −D` (`· +N uncommitted files` while the toggle is on; `No new commits` otherwise); the subjects (`+N more`); `N uncommitted files` and the toggle line `☑ commit them first ("<title>")` / `☐ … · they stay in the worktree` (the quoted title is cut first); the overlap warning; the three options naming the target branch; one hint line (`space commit first` only with uncommitted files); a failed attempt's output; the notes. Short panes drop the notes first (cut with `+N more lines`, then gone), then the error output (its first and last lines kept), then the commit subjects; enter waits for the preview (or its error)
- **commit-first** (`merge-worktree` `commitFirst`, default on in the UI when the preview has uncommitted files): `git add -A` then `git commit --no-edit -m <session title>` in the source worktree, hooks running as usual (GIT_EDITOR=true); a failure throws *Commit failed in the worktree, so nothing was merged:* with the last 12 lines of output (shown on the confirmation; whatever `add` staged stays staged). With it off, uncommitted files stay behind and a worktree with no commits is skipped (*No new commits to merge*); with it on, such a worktree merges its new commit
- before merging, Deckhand resolves the source commit (`mergeSourceSha`) and checks `HEAD..<sha>`; no new commits → `skipped`
- success records `mergedAt`, `mergeMode`, `mergeTargetBranch`, `mergeSourceRef`, `mergeSourceSha` on the worktree record, so every session of that worktree shows `✓`
- **conflicts**: Git exits nonzero with unmerged files → `conflicted: true`, `conflicts` (≤ 200 paths) and `conflictCount`; the daemon remembers the attempt per target root (`pendingMerges`: mode, source ref/SHA, target HEAD and whether the index was clean). The UI shows only `Merged with conflicts in N files`, up to five names (four plus `+N more` beyond five) and two keys (`conflictView`): `enter` → `resolve-merge` `keep` (left in progress for the editor or the Git tab's Merge Conflicts group; the marker is set now, with the remembered SHA) and `a` → `abort`. Abort: with MERGE_HEAD, `git merge --abort`; a squash has none, so `git reset --merge` (what `merge --abort` runs), then SQUASH_MSG is removed — only for a squash this daemon started on a clean index whose target HEAD has not moved, otherwise it refuses (*Cannot abort this squash merge safely…*). Verified in fixtures: `reset --merge` restores the merged files and the index and keeps unrelated unstaged edits and untracked files; ort refuses any merge while unrelated changes are staged (*Your local changes … would be overwritten by merge*), so a conflicted squash always started on a clean index
- **merge detection** (`detectMerges`/`runMergeDetection`/`detectMergeFromSummary`/`markDetectedMerge`): live, unmarked records whose worktree Git still lists and whose directory exists, at most 50 per run, one run at a time, grouped by repository (`git worktree list` + default branch once each). Ancestry (`mergedIntoDefault`, one `rev-list` per record whose tip moved past `baseSha`): the tip is reachable from `refs/heads/<default>` or `refs/remotes/origin/<default>` (never fetched) **and** has a commit of its own beyond `baseSha` (`merge-base --is-ancestor`, tip ≠ start) **and** the branch reflog shows a commit made on it (`commit…`, `cherry-pick…`, a non-fast-forward merge/pull; unknown when the reflog is empty → not required). So a fresh branch, one only fast-forwarded/reset to a newer main, the default branch itself and records without a starting point are never marked. PR: when a `workspace-summary` with `includePr` reports `MERGED` (catches GitHub squash merges), the record is marked `pr` with `mergeTargetBranch` = the PR base and `mergeSourceSha` = its `headRefOid` when that commit exists locally. Runs at daemon start, every `MERGE_DETECT_MS` (5 min, unref'd) and after every `workspace-summary` (`i`) for that session's record. It never unmarks; `M` can clear a detected marker, after which `mergeDismissedTip` keeps detection away until the tip moves. Changes broadcast like other marker writes (`saveWorktreeRecord`)
- `M` (`mark-session-merged`) toggles the worktree's marker after external/manual resolution; it records no `mergeSourceSha` (deletion safety is not relaxed by an unverified claim). Main-checkout sessions: refused (*Use D to mark it done*)
- `D` (`set-session-done`): `doneAt` per session, kept when set again; independent of merge markers

## Agent identity, forks, and restarts

Claude and Pi get an exact native conversation ID (a UUID Deckhand generates) at launch, stored as an `id` ref, so resume never depends on name lookup or Pi's private file layout. The display label is `dh-{sanitized-title}-{short-id}`. Sessions persisted before this keep their `name` (Claude) or `path` (Pi) refs and resume exactly as before.

Child session titles inherit parent context daemon-side as `parent title / child title` (trimmed to 64 chars). The UI strips that parent prefix for nested sidebar display because the sidebar already shows the hierarchy.

### Per-agent table (`src/agents.ts`)

`AGENTS` holds, per agent: `idAtLaunch` (Deckhand picks the conversation ID, Claude/Pi; Codex reports its own), `forks`, `forksAcrossDirectories`, `args(plan)`, the exit-screen readers (`exitRef`, `missingConversation`, `forkFailed`) and `version` (npm package of the latest release, the update command's args). A `LaunchPlan` is `new` (create, clean child, `S`), `resume` or `fork`; `launchArgs(program, plan)` turns it into argv. `relaunchPlan` decides what `s`/`S` launch (`forksParentAgain`: a forked child without its own conversation forks its parent again); the daemon only adds integration args (`agentSignals.integrationArgs`, appended after the plan's argv for every plan, so `codex fork <id> --no-daemon` like `codex resume <id> --no-daemon`) and handoff prompts. `readAgentExit` (daemon) applies the exit readers: the ref to keep, `failed`, and the note appended to the preview.

Final argv (before integration args):

| | create / clean child | `s` resume | fork (create, or `s` re-fork) | `S` fresh |
| --- | --- | --- | --- | --- |
| Claude | `--session-id <uuid> --name <label>` | `--resume <uuid>` (legacy name refs `--resume <name>`) | `--resume <parent uuid or legacy name> --fork-session --session-id <child uuid> --name <label>` | `--session-id <new uuid> --name <label>-fresh-<ts>` |
| Pi | `--session-id <uuid> --name <label>` | `--session-id <uuid>` (legacy `--session <path>`) | `--fork <parent id or legacy path> --session-id <child uuid> --name <label>` | `--session-id <new uuid> --name <label>-fresh-<ts>` |
| Codex | (no args) | `resume <id>` | `fork <parent id>` | (no args) |

### Forks

- A fork copies the parent's conversation as saved at that moment (all three agents read the saved transcript); a turn still in progress in the parent is not included. Nothing is typed into the PTY (the old Claude `/branch` keystrokes, `branchCommandInput` and its 500 ms timer, and the interim `name` ref are gone).
- Create (`createSession`): the parent's agent must have `forks`; the child uses the parent's program; the parent needs an agent ref (Codex: *has not reported the parent's conversation ID yet*); an agent without `forksAcrossDirectories` refuses any worktree mode but `none` and any cwd other than the parent's (*Codex forks stay in the parent's worktree*), so it never fails at launch. `forkedFromAgentSessionRef` is the parent's ref at that moment. The child's `id` ref is stored at launch for Claude/Pi; Codex children start without one.
- Identity: `acceptedNativeRef` never replaces an `id` ref Deckhand assigned (Claude/Pi, forks included) and never lets a forked child adopt its parent's ID (stored `forkedFromAgentSessionRef` or the parent session's current ref) from a hook; `readAgentExit` ignores an exit hint naming the parent.
- `s` on a forked child (`relaunchPlan`): resumes its own ref; forks the parent again with a new child ID when it never launched, has no ref (Codex never reported one: the exit note says so), or holds the parent's ref (fork failed, or a legacy child). Fork failure: Pi prints `No session found matching` (parent has no saved session); Claude prints `No conversation found with session ID: <id>` for the parent, or for the child's own new ID (nothing was saved for it); either way `readAgentExit` stores the parent's ref, marks the exit failed and notes *Press s to fork the parent again, or S…*. It never starts fresh silently. A missing conversation on a non-fork still refuses `s` (*Use S*).
- `S` on a forked child starts a fresh conversation and drops `forkedFromAgentSessionRef`, so a later `s` never re-forks the parent over it.
- Legacy: Claude children stored with their `/branch` `name` ref resume by name; children still holding the parent's ref (the `/branch` never reported) fork again with `--fork-session`; legacy `name`-ref parents fork with `--resume <name> --fork-session` (Claude's `--resume` accepts a name). Pi path refs unchanged.

Forks into another worktree: every fork launch (create, or `s` forking again) whose workspace differs from the parent's appends `-- <note>` (`movedForkPrompt` via `firstMessageArgs` in `src/daemon.ts`, the same first-message mechanism as handoff children): it names the child's worktree and branch, says earlier paths under the parent's worktree belong to the parent and must not be read or edited, that the parent's uncommitted changes are not there, and asks the agent to confirm and wait. Forks in the parent's worktree (the default) send nothing.

Cross-directory forks (why Codex forks stay in the parent's worktree):

- Claude: verified by running Claude Code 2.1.287 against a throwaway conversation: `--resume <parent> --fork-session --session-id <child>` from another cwd stores the child under the child's cwd project, with the parent's history and nothing written to the parent. Plain `--resume <id> --session-id <uuid>` is rejected without `--fork-session`.
- Pi (1.0.2, read from `dist/main.js` and `dist/core/session-manager.js` of the installed package, not run): `--fork <arg>` uses a path as is; an ID is looked up in the cwd's project (`findById`, then prefix `list`), then across every project (`SessionManager.listAll` over `~/.pi/agent/sessions/*`). `SessionManager.forkFrom(source, cwd)` copies the entries into a new file in the target cwd's project with `cwd` set to the target, and `--session-id` later finds it there. So Pi forks work from another worktree.
- Codex (0.157, from `codex fork --help` and strings in the installed native binary; no Rust source ships in the npm package; not run): sessions are rollout files under `$CODEX_HOME/sessions/%Y/%m/%d`, found by ID with `find_thread_path_by_id_str_in_subdir` (not cwd-scoped: cwd filtering applies to the picker, `--all` disables it). But resuming or forking a session recorded in another directory goes through the TUI's `cwd_prompt` (*Use session directory (…) / Use current directory (…)*, with *Always use …* stored as `tui.resume_cwd`), so a cross-directory fork could prompt or run in the parent's directory. That could not be established without a real run, so `forksAcrossDirectories` is false: the create form shows the fork staying in the parent's worktree (`tab` does nothing, a note says so) and the daemon refuses other modes. `codex fork` accepts the same `--no-daemon` as launches/`resume`. Its exit hint is the same `To continue this session, run: codex resume <id>` (no fork-specific hint in the binary), naming the current thread, which `codexResumeFromOutput` parses.

### Agent versions and updates (`src/agentVersions.ts`, `src/agentVersionCheck.ts`, `src/agentsFlow.tsx`)

| | `--version` prints | latest (npm `latest` dist-tag) | update command |
| --- | --- | --- | --- |
| Claude | `2.1.287 (Claude Code)` | `@anthropic-ai/claude-code` | `claude update` |
| Codex | `codex-cli 0.157.0` | `@openai/codex` | `codex update` |
| Pi | `1.0.2` | `@earendil-works/pi-coding-agent` | `pi update --self` (pi only; `pi update` without a target does the same, `--all` would also update packages) |

- Claude's native builds are published to npm with the same numbers. Its dist-tags have `stable` (older) and `latest`; `latest` is used because it is `claude update`'s default channel (a user on the `stable` channel would see an update `claude update` does not install). `claude update` may refuse when auto-updates are disabled in Claude's own settings (`DISABLE_AUTOUPDATER`); that shows as a failed update with its output, nothing more.
- **Pure** (`agentVersions.ts`, unit-tested): `parseVersion` (first x.y.z, suffixes ignored), `compareVersions` (numeric), `agentUpdateStatus` (`up to date`/`update available`/`not installed`/`latest unknown`), `sessionOutdated` (running only; exited sessions use the installed version on their next start), `updateHint` (the header's `codex update · U` / `N agent updates · U`).
- **Daemon** (`AgentVersionChecker` in `agentVersionCheck.ts`, one per daemon): installed = `findOnPath(program)` on the daemon's PATH (the launches' env; `resolveProgramCommand` uses the same lookup and now caches only hits) + `<binary> --version` (10 s), cached by realpath + mtime/size, only successful reads kept. Latest = `npm view <pkg> version` (10 s), cached; `refreshLatest(force)` shares one run; the daemon refreshes in the background at start and on a 30-minute unref'd tick that only looks up once the last check is 6 h old; `agent-versions` with `refresh` forces it (the Agents screen on open and `r`). Failures leave `latest` unset with `error` (`latest unknown: …`), never a failed request. `DECKHAND_AGENT_LATEST` (JSON program → version) replaces npm entirely: tests set it (`'{}'` everywhere, values in `tests/agentUpdates.test.ts`) so they never reach the network. `snapshot` adds per-agent `running`/`outdated` counts and `updating`. `publishAgentVersions` broadcasts `agent-versions-updated` to subscribed clients only when the JSON changed (after refreshes, launches, update start/end).
- **Update** (`update-agent`): refuses an agent not on PATH (*run deckhand setup*); runs `<binary> <updateArgs>` with stdin ignored, the daemon's env, cwd = home, its own process group, 5 min timeout (SIGTERM, then SIGKILL), combined output tail-bounded to 8000 chars; one run per agent (concurrent requests share it); then drops the `--version` cache, re-reads the installed version and responds `{ok, exitCode, command, output, before, after, versions}`. Output is returned at the end, not streamed. Sessions are never touched.
- **Session version**: `startWorker` reads the launched binary's version (`session.command`) beside the spawn and patches `agentVersion` before the callers save the running session (create, `s`, `S`), so every launch records what it runs.
- **UI**: App holds `agentVersions` (fetched after subscribe, then `agent-versions-updated`); sidebar gets `installedVersions`; `useAgentsFlow` (mode `agents`, full width like Settings) renders `AgentsPane` with `MenuPane`: rows label · installed · latest · outdated sessions, status right; details box (running state or the result with the output's last 8 lines first, then installed/path, latest/package/checked, update command, sessions, error); enter confirms first when the agent has running sessions; esc leaves while an update keeps running (the footer status reports the end).

### Claude

- create and clean sub-session: `--session-id <uuid> --name dh-{sanitized-title}-{short-id}`
- resume restart: `--resume <uuid>` (never `--session-id` without `--fork-session`: Claude rejects it, and refuses an ID already in use); legacy `name` refs use `--resume <name>`
- unknown ID: Claude prints `No conversation found with session ID: <uuid>` and exits; Deckhand marks the exit failed, appends a "press S" hint to the preview and refuses `s` for that ID — it never starts fresh silently (forks: see above)
- SessionStart hooks cannot replace an assigned `id` ref (e.g. after `/clear`)
- on exit, parse Claude Code's printed `claude --resume "..."` command from final preview and persist it (`id` kind for UUIDs, `name` otherwise); restart also re-parses `lastPreview`
- fresh restart: new UUID, labelled `dh-{sanitized-title}-{short-id}-fresh-{timestamp}`

### Pi

- create: `--session-id <uuid> --name dh-{sanitized-title}-{short-id}`; resume: `--session-id <uuid>` (Pi opens the exact project session ID, or creates it if absent)
- forked sub-session: `--fork <parent id or legacy path> --session-id <child uuid> --name ...` — Pi copies the parent before its TUI starts; resume then uses the child's own ID
- legacy `path` refs keep `--session <path>`; legacy forked children (stored with the parent's path) fork again with `--fork <path>`
- fresh restart: new UUID

### Codex

- launches normally; the native ID is captured from an authenticated SessionStart hook or the `codex resume <id>` exit hint
- resume uses `codex resume <id>`; an unknown ID refuses resume (use `S`) rather than guessing `--last`
- forked sub-session: `codex fork <parent id>` in the parent's worktree; the child's ID is captured the same way (never the parent's); without one, `s` forks the parent again

## Persistence, socket, PID, and logs

Deckhand writes under `~/.deckhand`:

- `state.json` — persisted sessions plus `worktrees` (one merge/deleted record per linked worktree incarnation, see *Worktree records*); no notes (older ones are migrated into `notes/`)
- `notes/sessions/<session id>.md`, `notes/worktrees/<worktree record id>.md`, `notes/repos/<sha256(main checkout root)[:16]>.md` — the notes (0600 files, 0700 directories; see *Notes*)
- `config.json` — app config, including `defaults` (global deckhand.json-schema settings) and `trustedProjects` (trust root → up to 20 fingerprints, newest first); written under a lockfile shared by UI and daemon
- `ui-state.json` — per-repository UI preferences (selection, tabs, width, collapse/hidden, filter/search)
- `handoffs/` — exported Markdown handoffs (0600 files in a 0700 directory)
- `daemon.sock` — Unix socket
- `daemon.pid` — active daemon PID
- `daemon.log` — daemon/client diagnostics
- `workers/<session>.pid` — session worker PID files
- `workers/<session>.log` — session worker stdout/stderr
- `workers/workspace-<hash>.pid` / `.log` — workspace worker PID and stdout/stderr (`workspaceWorkerId`)
- `worktrees/` — fallback managed worktree root

Pi session files are intentionally under Pi's own `~/.pi/agent/sessions/` tree, not under `~/.deckhand`.

`DECKHAND_HOME` replaces `~/.deckhand` for every path above. `scripts/deckhand-dev.mjs` uses it to run a dev-channel daemon (`DECKHAND_CHANNEL=dev`) in `~/.deckhand-dev` (or `DECKHAND_DEV_HOME`); only that channel accepts the `shutdown` request used by `npm stop`.

Config currently includes:

- `defaults` (validated where used by `globalDefaults`/`resolveSettings`)
- `dev_command` (legacy fallback after `defaults.devCommand`), default behavior is command `dev`
- `attach_scroll_sensitivity`, default `0.12`, adjustable in the UI with `[` / `]`
- `agent_hooks`, `notifications` (both default off)
- `trustedProjects`

Protocol:

- line-delimited JSON
- current protocol version: **v39** (v39: `merge-preview` (`MergePreview`), `merge-worktree` `commitFirst`, `resolve-merge` (`keep`/`abort`), `set-session-done`; `WorktreeMergeResult.sourceSha`/`conflicts`/`conflictCount`/`committed`; `SessionRecord.doneAt`; `SessionWorktreeRecord.mergeSourceSha`/`mergeDetected`; `WorkspaceSummary.pr.headSha`/`baseBranch`; v38: notes in files: `save-note` (revision-checked, `{saved, session}`) and `open-note` replace `update-session-notes`; `SessionRecord.notesFile` and `sharedNotes` are projected beside `notes`; v37: `agent-versions`, `update-agent`, `agent-versions-updated`, `SessionRecord.agentVersion`; v36: Terminal, Git and Dev are shared per workspace; `TerminalRecord`/`GitRecord`/`DevRecord.workspace`; the Git tab's Changes view: `watch-changes`, `changes-diff`, `change-stage`, `changes-updated`; `SessionWorktreeRecord.id`, with merge/deleted markers shared per worktree) (`PROTOCOL_VERSION` in `src/types.ts`; bump it on any request/response shape change)

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
  - `id` of the worktree record when the session runs in a linked worktree (also for mode `none` sessions launched in one)
  - path, branch, HEAD, main-worktree flag
  - origin/creator/name metadata, `baseRef`, `links`
  - merge markers (`mergedAt` / `mergeMode` / `mergeTargetBranch` / `mergeSourceRef` / `mergeSourceSha` / `mergeMarkedManually` / `mergeDetected`) and `deletedAt`: for sessions with an `id` they live in the worktree record and are only projected into the session the daemon holds and sends (a main-worktree attach's old merge markers are migrated into `doneAt`)
- `doneAt` (the `D` marker, per session)
- legacy top-level `mergedAt` / `mergeTargetBranch` / `mergeSourceRef` / `mergeMarkedManually` (main-checkout `M` before `D`): migrated into `doneAt` on load and removed
- lifecycle `status`
- activity `agentStatus`, `agentStatusUpdatedAt`
- timestamps, `pid`, exit details, `lastPreview`
- not `notes`: the daemon projects `notes`, `notesFile` and `sharedNotes` from the notes files and never stores them (state from older versions is migrated)
- `devRunning` (mirrors the workspace's shared Dev on every session in it)
- `agentVersion` (x.y.z the agent last launched with; outdated while running and older than the installed version)
- `parentSessionId`, `subSessionKind`, `forkedFromSessionId`, `forkedFromAgentSessionRef`
- `sidebarOrder`

`agentStatus` is persisted only on activity transitions to avoid excessive disk writes. `devRunning` is cleared during daemon restart recovery because live dev PTYs are not preserved. Workspaces themselves are not persisted (derived by `workspaceKey`); worktree records are (`worktrees` in `state.json`).

## IPC request/event types

Request types:

- `ping`, `shutdown` (dev channel only)
- `project-info`, `save-config` (global or repository; revision-checked, never runs anything; a repository save returns `trust` and keeps the file trusted when the replaced version was trusted or absent — `savedProjectTrust` in `src/projectConfig.ts`, decided inside the serialized write from exactly the replaced bytes plus the hook as it is now; a hook is never newly trusted by a save), `trust-project`, `run-action`
- `settings-info` (C → Settings: `explainSettings` rows — each effective value with its source and untrusted repository values (`pending`) — the deckhand.json trust state, both editable documents with revisions (the grid's per-layer cells come from these), hook file, template vars, default/origin branch; read-only)
- `worktree-candidates` (Settings → Linked items: untracked/ignored entries of the main checkout plus configured links), `worktree-candidate-sizes` (≤24 relative paths, `du -sk` with a 4s timeout each, null when unknown)
- `workspace-summary`, `inspect-cleanup`, `archive-session`, `export-handoff`, `cancel-start`
- `agent-versions` (optional `refresh`: look the latest releases up again first), `update-agent` (`program`; runs its update command, responds with exit code, output and the new versions)
- `create-pr` (push `-u` without force, then `gh pr create --web` / `gh pr view --web`; refuses detached/main/master/base; optional `branch` must still match; invalidates the summary cache)
- `agent-hook` (token + launch ID authenticated)
- `list`, `subscribe`
- `list-worktrees`
- `watch-preview`, `watch-terminal`, `watch-git`, `watch-dev` (the last three, the start/stop and the terminal/git/dev paths below take a `sessionId` and act on that session's workspace pane)
- `start-dev`, `stop-dev`
- `watch-changes` (optional `sessionId`; none stops watching), `changes-diff` (`sessionId`, `group`, `path` of a listed entry), `change-stage` (`sessionId`, `mode: stage|unstage`, optional `group`+`path`; none = everything) — the Git tab's Changes view
- `save-note` (`sessionId`, `section: session|shared`, `noteId` for shared, `text`, `revision`; refused while the file has another revision: `{saved: false, session}`), `open-note` (`sessionId`, `section`; creates the file if missing, responds with its path)
- `create`, `reorder-session`, `restart`, `kill`, `merge-preview` (read-only: what `m` would merge), `merge-worktree` (optional `commitFirst`), `resolve-merge` (`action: keep|abort` after a conflicted merge), `mark-session-merged` (worktrees only), `set-session-done` (`done: boolean`), `remove`
- agent attach path: `attach`, `input`, `resize`, `detach`
- terminal path: `attach-terminal`, `terminal-input`, `terminal-resize`, `terminal-detach`
- git path: `attach-git`, `git-input`, `git-resize`, `git-detach`
- dev path: `attach-dev`, `dev-input`, `dev-resize`, `dev-detach`

Event types:

- `session-updated` (also when a note changes: saved, or edited in a file; sent for every session showing it), `session-removed`
- `preview-updated`, `terminal-updated`, `git-updated`, `dev-updated`, `changes-updated`
- `agent-versions-updated` (every subscribed client, on change)
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
- `src/sessionWorker.ts` — PTY owners: per-session worker (agent only) and per-workspace worker (Terminal, Git, Dev in a `PaneHost`).
- `src/workspace.ts` — `workspaceKey` (which worktree a session runs in; unit-tested), `noWorkspaceReason`, `workspacePaneUnavailable` (UI gating of the workspace panes), `workspaceWorkerId`.
- `src/worktreeRecords.ts` — per-worktree merge/deleted records: scope (`ownWorktreePath`), `liveWorktreeRecord`, projection into sessions (`projectWorktree`/`storedSession`) and the legacy-state migration (`migrateWorktreeRecords`; unit-tested).
- `src/sessionOrder.ts` — sidebar hierarchy sorting, depth, child detection, and collapse filtering.
- `src/attach.ts` — external attach/detach mode.
- `src/storage.ts` — state/config loading and persistence (`loadState` migrates legacy worktree markers).
- `src/git.ts` — git repo, worktree, deletion, branch, and merge helpers (merge with commit-first, target operation checks, conflict listing and abort, branch creation/own-commit reflog reads).
- `src/mergeModel.ts` — the merge confirmation and conflict view as pure data (`mergeConfirmLayout` with its height budget, `mergeOptions`, `surprisingTarget`, `overlappingFiles`, `conflictView`; unit-tested in `tests/mergeModel.test.ts`).
- `src/paths.ts` — config/socket/PID/log/runtime path helpers.
- `src/types.ts` — shared session/protocol/UI types.
- `src/nodePty.ts` — macOS `node-pty` helper repair logic.
- `src/terminalState.ts` — terminal escape reset helpers used before/after UI and attach transitions.
- `src/sidebarModel.ts` — the sidebar as pure data (unit-tested): rows (prefix/suffix layout and truncation, `▶` once per workspace, shared-workspace marker, archived dimming, the done marker and done rows), header text, the selected session's details block (`statusWords`, `statusSince`, `formatAge`, `locationText`). `src/sidebar.tsx` — its rendering.
- `src/preview.tsx` — Preview pane rendering.
- `src/terminalPane.tsx`, `src/devPane.tsx` — rendering of the workspace's shared Terminal and Dev panes (unavailable/exited messages).
- `src/changesModel.ts` — the Git tab's Changes view as pure data (groups, rows, selection, first changed line, diff classification; unit-tested). `src/changesGit.ts` — its Git I/O (status + numstat read, bounded diff, validated stage/unstage). `src/changesFlow.tsx` — selection, diff fetch and focus keys. `src/changesPane.tsx` — rendering (list, diff, layout).
- `src/notes.ts` — notes as pure data (unit-tested): limits, revisions, shared-note identity (`sharedNoteIdentity`, `repoNoteId`), checklist parsing/toggling/insertion/continuation and counts, read-mode rows (`noteReadRows`, `fitReadRows`), the two-section height budget (`budgetSections`), header/sidebar texts (`checklistLabel`, `openChecklistText`). `src/notesStore.ts` — the daemon's note files (load, legacy migration, revision-checked atomic saves, `ensureFile`, removal, directory watchers). `src/notesFlow.tsx` — the Notes tab's drafts, debounced saves, conflict reloads and notes-focus keys. `src/notesPane.tsx` — its layout (`notesLayout`, pure) and rendering.
- `src/tabs.tsx` — tab UI.
- `src/terminalPreview.ts` — headless xterm preview model.
- `src/ui.ts` — shared theme, glyph, path, truncation, and display helpers.
- `src/menu.tsx` — the one picker selection style (`SelectableRow`: ❯ marker + full-width inverse bold bar; `SelectableCell`: the same bar limited to one grid cell, for the Settings grid), `MenuList` (aligned label column, truncated rows, windowed around the selection), `MenuPane` (menu + details box + one hint line, budgeted to the pane height) and `fitHint` (one hint line that fits the width; the browse footer uses it with ` • ` and per-part drop priorities, offering `A archive`/`A unarchive` only for exited/archived sessions). Used by Settings (C), the action/program/worktree pickers and the kill/merge confirmations. Screens that replace the right pane show their own single hint line; the app footer hint row stays empty for them.
- `src/projectConfig.ts` — schema validation, loading the main checkout's `deckhand.json` + hook, fingerprints, trust lookup/update, the self-edit trust rule (`savedProjectTrust`), and `resolveSettings` (the single source of effective settings: setup, Dev, actions, new-session defaults); `explainSettings` breaks them down per row with sources for C → Settings (tested to equal `resolveSettings`).
- `src/projectConfigDocument.ts` — editor documents: global defaults (inside config.json) and repository targets, revision-checked saves, starter config.
- `src/configDraft.ts` — pure editor helpers (size limit, JSON formatting).
- `src/settingsModel.ts` — the Settings screen as pure data (unit-tested): `settingsGrid` (per setting, each layer's own value as written and which one applies, from `explainSettings`; built-in/legacy values in the Global column; `needsTrust` for untrusted repository values), `cellDetail` (the one-line details), `initialColumn`/`columnProblem`, per-layer `layerActions`, choice options, text validation, link selection → `worktree.symlink`, save/trust status wording, and `applyChange` (one key set/removed in one layer's JSON, other keys kept in place, validated).
- `src/settingsInfo.ts` — daemon readers for Settings: `readSettingsInfo`, link candidates via porcelain v2 `--ignored=matching --untracked-files=normal` with `classifyCandidate`, bounded sizes.
- `src/settingsFlow.tsx` (state/keys: the selected row and sticky column; saves through `save-config` with the column's revision, reloads after every save or rejection; e/T return here) and `src/settingsPane.tsx` (rendering, full terminal width: the grid — two columns from 64 inner columns, else the selected one with ◂ ▸ — edit controls, one layer's Actions list, link picker, height-budgeted with one hint line).
- `src/configEditorPane.tsx` — the raw JSON editor (reached with e from Settings).
- `src/textEditor.ts` — pure multiline text editing and rendering model: `editText` (with `EditOptions` for notes), word jumps, soft-wrapped rows (`wrapRows`, `moveVisual`, `wrappedEditorLines`) and `scrollTopFor`; used by the JSON editor and Notes.
- `src/terminalKeys.ts`, `src/useTerminalInput.ts` — raw key normalization (DEL/Kitty Backspace vs forward Delete, key releases) and the Ink input hook (one stable listener calling the latest handler, so no key reaches a stale render's handler).
- `src/workspaceGit.ts` — porcelain-v2 status (`parseStatus`, including the `entries` the Changes view groups), workspace summary, optional `gh` PR lookup (`ghPullRequest`, injectable as `getWorkspaceSummary`'s `lookupPr`), `createPullRequest`, handoff Git context (`getHandoffGitContext`: commits/changes/numstat, never diff content), cleanup inspection (with the `integrated` merge SHA), the merge preview (`getMergePreview`) and merge detection by ancestry (`mergedIntoDefault`).
- `src/worktreeLinks.ts` — worktree settings schema/merge, location template expansion, and link application.
- `src/sessionFeatures.ts` — filters/search and handoff Markdown/export (pure; the daemon passes the Git context in).
- `src/sessionScope.ts` — which sessions belong to the current repo/worktree.
- `src/agents.ts` — the per-agent table: capabilities (ID at launch, forks, forks across directories), create/resume/fork/fresh argv (`LaunchPlan`, `launchArgs`), exit-screen readers, `relaunchPlan` (unit-tested in `tests/agents.test.ts`).
- `src/agentSignals.ts` — hook normalization, Claude/Codex integration args, Codex resume parsing.
- `src/agentVersions.ts` — agent versions as pure data (parse/compare, status, `sessionOutdated`, header hint; unit-tested in `tests/agentVersions.test.ts`). `src/agentVersionCheck.ts` — the daemon's version I/O (`findOnPath`, bounded runs, `--version` cache, npm lookup with the `DECKHAND_AGENT_LATEST` override, the updater). `src/agentsFlow.tsx` — the Agents screen (U): `AgentsPane` rendering and `useAgentsFlow` keys/requests.
- `src/uiState.ts` — `ui-state.json` normalization and persistence.
- `src/detailTexts.ts`, `src/detailsPane.tsx` — text for review/inspection panes and their scrolling renderer.
- `src/desktop.ts` — editor/URL opening helpers (`openInEditor` takes an optional line: `-g file:line`).
- `src/help.ts` — in-app `?` guide content (topics of key → description rows and notes); `src/helpPane.tsx` renders it (topic list, aligned key column, `/` search).
- `scripts/deckhand-dev.mjs` — isolated dev launcher and sandbox.
- `tests/` — `node:test` suite (`npm test`); `tests/helpers.ts` holds fixture repos, env/temp helpers, the fake agent (`fakeAgent`: records argv/input, reports SessionStart; `codex fork` reports the parent's ID around its own; `withoutHooks` reports nothing; `--version` prints its `FAKE_VERSION` in each agent's format and its update command rewrites the file with the next patch version, logging to `updates.log`) and the PTY harness (`terminalUi`: condition-based screen waits with a generous ceiling, `UI_WAIT_MS`, since the suite runs files in parallel).
- `scripts/fix-node-pty.js` — install-time macOS `node-pty` fixup.

## File-specific notes

### `src/cli.ts`

- Runs UI normally.
- Runs daemon with `--daemon`.
- Runs session worker with `--session-worker`, workspace worker with `--workspace-worker`.
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
- Attaches to agent/terminal/git/dev based on active pane; only an agent attach ends when the session exits (Terminal/Git/Dev belong to the workspace).
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
5. Daemon loads persisted state (lifting legacy per-session worktree markers into worktree records, and legacy `notes` into `notes/` files), reads every note file, starts watching the notes directories, and marks previously running sessions exited if this is a daemon restart.
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

Automated: `npm test` (build, then `node --test` over `tests/`) covers config/trust, editor and key handling, storage, cleanup inspection, a real daemon with fake agents (setup/actions/cleanup/hooks/resume; one Dev shared per worktree, its lifetime and stop on worktree deletion/last-session removal; one shell and one (fake) lazygit shared per worktree, fan-out to watchers, attach with bracketed-paste mirroring and one attacher, use from an exited session, lazygit restart on view, Dev stop not retiring a worker in use, teardown on kill-with-delete and last-session removal; the Changes view: watch delivers groups, polling pushes to a second session of the worktree, stage/unstage/stage-all/unstage-all, refused unlisted paths, unstaging without HEAD, unwatching stops pushes; worktree records: merge from one session marks its attached sibling and sub-session with `session-updated` broadcasts, `M` from either toggles all, main-checkout sessions keep their own markers, kill-with-delete makes every session of the worktree non-restartable/non-mergeable without a workspace, a new worktree at the same path is a new incarnation (also after an outside removal), the last referencing session's removal drops the record), the worktree-record migration of legacy state (incarnations split at deletions, sub-sessions, restarted sessions, main checkout untouched, repair, idempotence, write-back at daemon start), the Changes model and its Git I/O against fixture repos (spaces, glob-like names, renames, binary, untracked, conflicts, no HEAD), the workspace key, the sidebar model (row layout and truncation at 24 and 48 columns, `▶` once per workspace, the shared-workspace marker, archived dimming in both views, header text, the details block and its shrinking, ages) plus one rendered sidebar, the dev launcher, a real-PTY Git tab run (browse list, `v` focus, diff preview, `space` stages, `esc`), and two more real-PTY UI runs (inline review on n, raw-key JSON editing via C → e, persistence; the Settings grid: columns, repo/global cell edits, Linked items, x, T, e; self-edits keep trust so d runs without asking until an outside edit).

Notes (`tests/notesDaemon.test.ts`, its own daemon and state directory; `tests/notes.test.ts`, `tests/textEditor.test.ts`, `tests/notesUi.test.ts`): legacy `session.notes` migrated into its file and dropped from state.json, a re-run with a stale copy in state.json keeping the (edited) file; a save broadcast and written; an in-place write and a rename-save made outside picked up and broadcast; a stale-revision save refused without writing; the 50 000-character cap, a too-large file shown cut and refused; three sessions of one worktree (new, attached, mode-none child) sharing its note, a save naming another note refused; handoff `## Worktree notes`; `open-note` creating the file; main-checkout sessions sharing the repository note, which outlives them, while a session's note goes with it; a deleted worktree's note read-only, a new worktree at the path starting empty, the note deleted with the record. Unit: cursor moves, word jumps and deletion, insert/delete mid-text, the character limit, soft wrap and visual Up/Down/PageDown, scroll-to-cursor, checklist parsing (indented, `*`, `+`, `[X]`), toggling/inserting/continuing and counts, read rows with `+N more lines`, the section budget, the rendered layout in read and edit mode, the sidebar `☐` line and its drop order, shared-note identity, search and handoff. Real PTY: `o`, typing, ← and Alt+← (ESC b) then typing mid-line, Ctrl+X, `tab` to the worktree note, a bracketed paste with a Tab, `esc`, the rendered `☐` item and sidebar count, the files, an outside edit appearing. Not exercised: opening the editor (E / Ctrl+O would launch a real Cursor/VS Code); the `CSI 1;3D`/`CSI 1;5D` Alt/Ctrl+arrow sequences only as Ink's parsed key flags (ESC b went through the real PTY).

Merging and done (`tests/merge.test.ts` against fixture repositories with the user's Git config ignored, `tests/mergeDaemon.test.ts` with its own daemon and a fake `gh` that answers from a file, `tests/mergeModel.test.ts`, `tests/mergeUi.test.ts` in a real PTY, plus additions to the sidebar, records and handoff tests): preview counts, subjects, diff stat, target/main/default, uncommitted count; commit-first commits with the title then merges (and a worktree with only uncommitted work merges through it; off, it is skipped and the files stay), a failing pre-commit hook returns its output and merges nothing; a merge/rebase/cherry-pick/conflicts in progress refuse (a linked worktree's own state counts), overlapping target edits listed by cause; after a squash, cleanup with the recorded SHA no longer needs DELETE for those commits but does for a later one, an unknown or non-SHA value is ignored; conflicts in normal and squash merges: kept (markers, UU/A status), aborted (`merge --abort`; `reset --merge` only for Deckhand's own squash on the same HEAD and a clean index), unrelated unstaged edits and untracked files preserved, staged unrelated changes refused by Git; ancestry detection true for a merged branch (also only on origin/main), false for a fresh one, a fast-forwarded one, one with later commits, one reset behind its start, without a start or default refs; the PR lookup injected (MERGED with head/base, ENOENT → prError, not run without includePr). Over IPC: `D` persisted, broadcast, kept on repeat, cleared; `M` refused on the main checkout; preview, a commit-first squash (marker with `mergeSourceSha` on every session of the worktree, the sub-session's done untouched, cleanup without DELETE for it); conflict abort then keep (marker, merge left in progress, preview reports it, another merge refused, an abort Deckhand cannot vouch for refused); detection by ancestry on `workspace-summary`, not for a fresh branch, staying cleared after `M`; by PR through the fake `gh` (`pr`, the head as SHA, cleanup integrated). Unit: the done migration (idempotent, records untouched, through `loadState` and the daemon-start write-back), sidebar `☑` placement/drop order/done styles/details `done …`, the confirmation layout (texts, toggle, warn target, in-progress and overlap warnings, the budget at every height) and the conflict view, rendered panes. PTY: `m`, `space` twice, squash with commit-first, `D` (`✓ ☑`), a conflict aborted with `a` and kept with `enter`. Not exercised: a real `gh` (only the fake), a real GitHub squash merge, the 5-minute timer (the start and summary triggers are the same code).

Agent versions (`tests/agentUpdates.test.ts`, its own daemon whose PATH holds only the fakes, node and the system directories, so no real agent can run; plus `tests/agentVersions.test.ts`): `agent-versions` reads the fakes' `--version` and the stubbed latest (one unknown), a session records `agentVersion` at launch, `update-agent` runs `claude update` with stdin closed, reports before/after/output, broadcasts `updating` then the new version, leaves the running session alone and makes it outdated; a later launch and a restart are current, exited sessions never outdated, `agentVersion` persisted; concurrent updates share one run; a failing `pi update --self` returns its exit code and output; not installed and unknown agents are refused. Unit: parsing of each agent's format, numeric comparison, statuses, `sessionOutdated`, header hint, sidebar `↑` placement and drop order, the details line's version forms, a rendered sidebar and the Agents screen (rows, details, confirmation, working, success, failure). The real updaters were never run.

Forks (`tests/forks.test.ts`, its own daemon with the shared fake agent from `tests/helpers.ts`, beside `tests/daemonFeatures.test.ts`; plus `tests/agents.test.ts`): Claude fork argv `--resume <parent> --fork-session --session-id <child> --name …` with the child ID stored at launch and nothing typed; `s` resumes the child; a fork whose parent has no saved conversation fails with a note and `s` forks again with a new ID; a Claude fork into a new worktree; legacy `/branch` name-ref children resume by name, parent-ref children fork again, name-ref parents fork by name; Pi fork argv unchanged; `codex fork <parent id>`, the child's ID from a hook while the parent's ID (reported before and after it) is never adopted, `codex resume <child>` on `s`, re-fork when no child ID was reported, refusal of other worktrees/directories and of a parent without an ID; the per-agent argv table and `relaunchPlan`. Not run against the real agents through Deckhand: Claude `--fork-session` was verified directly (see *Forks*), Pi and Codex forks only from their help and package contents.

Validated during the workbench refactor (shared workspace panes, the Changes view, worktree records):

- `npm run build`; `npm test` repeatedly, also two suites at once. The real-PTY UI tests had flaked under load: a key typed right after a screen change reached Ink's previous-render listener (Ink re-subscribes `useInput` in a passive effect) and was lost; `useTerminalInput` now keeps one stable listener calling the latest handler, and UI waits are condition-based with a 30 s ceiling (`UI_WAIT_MS`).
- the Git tab in the isolated sandbox, rendered through a real PTY at 130 and 190 columns: browse list, focus with stacked and side-by-side diff, `j`, `J`, `space` (opening in an editor and `o` → lazygit were not exercised there)
- worktree records only through the daemon protocol (tests above); the UI reads the same projected fields as before and was not re-run by hand

Historically validated during development, but not exhaustively rechecked recently:

- daemon autostart, PID/log/socket handling, and protocol mismatch refusal
- Pi and Claude session creation/resume paths; Codex launch compiles cleanly
- Claude exit resume-handle parsing (exact `--session-id`/`--fork`/`--fork-session`/`codex fork` launch argv and forked restart paths are covered by the fake-agent daemon test)
- fresh restart/no-resume mode and parent-inherited child titles
- leftover directory cleanup after worktree deletion
- worktree sanitizer and `git worktree list --porcelain` parsing
- preview subscriptions, xterm rendering, frozen `lastPreview`, and activity transitions
- attach request/output/detach/return-to-Ink flow
- sidebar hierarchy, numbering, resize, and persisted in-process width across attach/detach
- resize suppression for agent activity detection
- stale-session cleanup after daemon restart

Not fully manually validated recently:

- Codex session creation, and `codex fork` (argv and the Codex cwd prompt were read from help/binary, never run)
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
- Codex resume and fork depend on capturing its native ID; Codex forks stay in the parent's worktree (see *Forks*).
- Terminal/Dev scrollback controls are still future work.
- The Changes diff preview refetches when the record changes (status or line counts); an edit that keeps a file's `+/−` counts identical is shown after the next change or reselecting the file. Untracked line counts stop after 500 files (or files over 1 MB).
- The Changes view does no hunk staging, commits, discards or branch operations by design: lazygit (`o`) covers them.
- A workspace worker (and its shell) lives as long as the workspace has sessions, even exited/archived ones, unless nothing is left in it; remove sessions to free it. There is no idle timeout (a shell may run a long job).
- While a session's new worktree is being prepared its Terminal/Git/Dev are unavailable; they become available as soon as the worktree exists, e.g. during setup.
- Worktree records cover worktrees Deckhand knows: a linked worktree removed outside Deckhand keeps a live record (its sessions resolve to the missing path and their panes report *Worktree directory is missing*) until Deckhand creates a worktree at that path again, which marks it deleted. Migration cannot ask Git: a legacy mode-`none` session launched in a linked worktree that no session owns keeps per-session markers.
- Records are keyed by Git's path strings (lexical, like workspace keys); a worktree reached through two different path spellings would get two records.

## Recommended next steps

Near term:

1. Manually exercise worktree flows in disposable repos:
   - hook creation from main worktree
   - hook creation from linked worktree
   - fallback creation
   - existing picker
   - keep/delete/cancel kill behavior
   - delete-branch behavior
   - merge success/skipped/conflicted cases against a real repository with hooks (fixtures cover them, see *Validation status*)
2. Add tests around:
   - worktree parsing
   - sanitizer behavior
   - preview serialization
   - activity transitions
   - resize suppression
   - setup/doctor detection behavior
3. Polish create/worktree UX:
   - true overlays/modals
   - better validation and error feedback
   - better truncation/filtering for long paths
4. Clean up attach/detach transition visuals.
5. Add structured daemon logging.
6. Add stronger daemon health/protocol compatibility handling.

Later:

- Run a real `codex fork` from another worktree: if `-C <cwd>` or `tui.resume_cwd` reliably keeps the child in the current directory without the cwd prompt, set Codex `forksAcrossDirectories` (`src/agents.ts`) and pass it.

- Add terminal/git/dev scrollback controls.
- Add dev stop confirmation or persisted dev state if useful.
- Monitor long-running macOS `node-pty` behavior under repeated spawn/exit churn.
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
npm start              # build + isolated dev daemon in a disposable sandbox (see docs/dev-build.md)
npm stop / npm restart # stop / rebuild-and-reopen only the dev daemon; npm run status checks it
npm test
```

## Final takeaway

Deckhand's foundation is established: daemon-owned long-lived sessions, worker-owned PTYs, explicit attach/detach, split-view Ink frontend, worker-side rendered Preview pipeline, exact Claude/Pi conversation IDs assigned at launch, sub-session hierarchy, and agent-agnostic worktree support.
