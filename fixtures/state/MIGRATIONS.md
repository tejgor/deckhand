# Implicit migrations a v45 TS daemon applies on load

Every path below is something the TS code upgrades, normalises or drops when it reads older or edited state. The
"fixture" column names what in `home-v45/` exercises it; `home-v45.after-idle/` and `read-responses.jsonl` hold the
result. Line numbers are against the TS tree at the commit these fixtures were generated from.

Session IDs are abbreviated by their last two hex digits (`S01` = `5e550000-0000-4000-8000-000000000001`), worktree
record IDs likewise (`wt-0a` = `7e000000-0000-4000-8000-00000000000a`).

## state.json (read once at daemon start: `markAllNonExitedSessionsExited` → `readState`)

| # | What | Where | Fixture |
|---|------|-------|---------|
| 1 | Empty (whitespace-only) or missing state.json is replaced by `{sessions: [], worktrees: []}` and written at once. | src/storage.ts:49, src/storage.ts:60 | not exercised (would discard the fixture) |
| 2 | Unparseable state.json is renamed to `state.json.corrupt-<Date.now()>` and replaced by an empty state. Existing `.corrupt-*` files are never touched. | src/storage.ts:64 | `state.json.corrupt-1767225600000` survives untouched; the rename path itself is not exercised |
| 3 | Only `sessions` and `worktrees` survive a rewrite: unknown top-level keys are dropped (`readState` returns `{sessions, worktrees}`, `saveState` writes that). Non-array `sessions`/`worktrees` read as `[]`. | src/storage.ts:53-57, src/storage.ts:74-80 | `futureTopLevelKey` is gone after idle |
| 4 | Worktree records without a string `id` and `path` are dropped. | src/worktreeRecords.ts:74-75 | the record at `.../invalid-record-without-id` |
| 5 | A session whose `worktree.id` names no record gets the record rebuilt from the session (`path` ← `worktree.path` ?? `launchWorktreeRoot` ?? `cwd`, `createdAt` ← session's, markers ← the session's projected markers). | src/worktreeRecords.ts:88-93 | S10 → `wt-0f` (ghost) rebuilt with its `mergedAt`/`mergeMode`/… |
| 6 | Legacy per-session worktree markers (sessions written before worktree records: no `worktree.id`) are lifted into new records with fresh `randomUUID()` IDs. Sessions are grouped by worktree root: their own managed/attached non-main worktree, or (mode `none` / no `worktree`, no pending `requestedWorktreeMode`) their `launchWorktreeRoot ?? cwd` when that is a linked worktree some record or session owns. | src/worktreeRecords.ts:72-123 | S09 (managed, markers on `worktree`) and S0a (no `worktree`, top-level markers, `launchWorktreeRoot` = the legacy worktree) share one new live record (`<uuid-2>`) |
| 7 | Within a root, each recorded `worktree.deletedAt` ends one incarnation: the deleting session belongs to it, others to the first deletion at/after their last launch (`max(createdAt, agentStartedAt)`), the rest share the live one. | src/worktreeRecords.ts:99-105 | S0b (`deletedAt` 2026-08-15) gets its own deleted record (`<uuid-1>`), separate from S09/S0a |
| 8 | A lifted incarnation takes the most recent merge marker of its members (a sub-session's top-level `mergedAt`/`mergeTargetBranch`/`mergeSourceRef`/`mergeMarkedManually` included; `mergeMode` only from a `worktree`). | src/worktreeRecords.ts:110-117 | `<uuid-2>` gets S0a's later top-level marker (`mergeMarkedManually: true`, no `mergeMode`) over S09's |
| 9 | Lifted sessions get `worktree.id` (a session without `worktree` gets `{mode: 'none', id}`), lose their top-level merge fields and their worktree markers. | src/worktreeRecords.ts:124-131 | S0a: `"worktree": {"mode": "none", "id": "<uuid-2>"}`; S09/S0b keep `worktree` minus markers, `id` appended last |
| 10 | Markers stored on a session that has `worktree.id` (projections a daemon should never have written) are stripped; the record's are the only copy. | src/worktreeRecords.ts:50-52, :133-134 | S05 `worktree.mergedAt` dropped (`wt-0a` has none) |
| 11 | Records no session references are dropped. | src/worktreeRecords.ts:137-138 | `wt-0e` (orphan) is gone |
| 12 | Main-checkout sessions (no record): an old per-session `M` marker becomes the done marker — `doneAt` ← `mergedAt` (top-level, or under a non-`none` `worktree`, e.g. attached to the main worktree) unless `doneAt` is set; the merge fields are removed. | src/worktreeRecords.ts:150-166 | S0c gets `doneAt` = its `mergedAt`; S0d keeps its own `doneAt` and loses `worktree.merge*` |
| 13 | Any change from 4-12 makes the state "migrated": it is written back at start even without running sessions. | src/storage.ts:57, :88-89 | — |
| 14 | Crash/restart recovery: every non-`exited` session (`starting`, `running`) becomes `exited` with `updatedAt` = now, `pid` removed, `exitCode`/`exitSignal` ← existing ?? `null`, `devRunning: false`, `exitReason: 'interrupted'`, `attention` replaced by `{state: 'unknown', event: 'DaemonRestart', at: now}` (unknown keys inside the old attention are lost), and a `running` setup becomes `failed` with `\nInterrupted by daemon restart` appended. Keys keep their positions; new ones are appended in that order. `agentStatus` is left as it was (S01 stays `active`). | src/storage.ts:86-118 | S01, S03 (starting), S04 (setup running → failed, devRunning), S0f (setup `pending` untouched) |
| 15 | Exited sessions with `devRunning: true` get `devRunning: false`. | src/storage.ts:92-96 | S05 |
| 16 | Sessions are persisted sorted: running first, then `createdAt` ascending (`localeCompare`). | src/storage.ts:120-127, src/daemon.ts:3303 | the input order is S01..S11; after idle it is by `createdAt` |
| 17 | `attention.reason` is never persisted (held in memory only). | src/daemon.ts:381-386 | S07's `reason` is dropped, its `futureAttentionField` kept |
| 18 | `notes`, `notesFile`, `sharedNotes` are never persisted (projections). | src/daemon.ts:381-382 | S0c/S0d lose `notes` |
| 19 | Unknown keys on sessions, nested objects of sessions (`worktree`, `worktree.links`, `agentSessionRef`, `setup`, exited sessions' `attention`) and worktree records round-trip unchanged. UI-only `replyRead` is persisted as found. | spreads throughout src/storage.ts, src/worktreeRecords.ts, src/daemon.ts | `futureSessionField`, `futureWorktreeField`, `futureLinksField`, `futureRefField`, `futureSetupField`, `futureRecordField`, S07 `replyRead` |

## Worktree records after start (background merge detection, src/daemon.ts:2957-3024)

| # | What | Where | Fixture |
|---|------|-------|---------|
| 20 | Records from before `baseSha` get it once from the branch reflog's `branch: Created from` entry (persisted without touching sessions). A record whose reflog has none is remembered in memory only. | src/daemon.ts:2987-2993, src/git.ts:568-573 | `wt-0a` gains `baseSha` = the `add index` commit (appended after `futureRecordField`) |
| 21 | A live, unmerged record whose branch tip has commits beyond `baseSha`, all in the default branch, is marked merged: `mergedAt` = now, `mergeDetected: 'ancestor'`, `mergeTargetBranch`, `mergeSourceRef`, `mergeSourceSha`; `mergeDismissedTip` removed; its sessions' `updatedAt` = now. | src/daemon.ts:2995-2996, :3014-3024, :1162-1172 | `wt-0b` (feature-merged, merged `--no-ff` into main) and S06 |
| 22 | That merge ticks the record's open linked tasks (`done=<today> auto=merge`). | src/daemon.ts:1843-1849, src/tasks.ts:237-242 | task `7c3d4e5f` |
| — | Deleted, already merged, missing, ownerless records and those with `mergeDismissedTip` = tip are left alone. | src/daemon.ts:2969-2986 | `wt-0c`, `wt-0d`, `wt-0f`, `<uuid-1>`, `<uuid-2>` unchanged |

## Notes (`notes/`)

| # | What | Where | Fixture |
|---|------|-------|---------|
| 23 | Notes kept in state.json (`session.notes`) by older versions move to `notes/sessions/<id>.md` (cut to 50 000 chars) unless that file exists; an empty string moves nothing. Either way state.json is rewritten without them. | src/notesStore.ts:107-119, src/daemon.ts:438-448 | S0c's note file is created; S0d (`notes: ""`) gets none |
| 24 | Own notes of sessions in a linked worktree are merged into the worktree's note (a lone note into an empty worktree note as is, else each under `## <title>` after the existing text, trailing whitespace trimmed), then moved to `notes/trash/<date>_session_<slug of "<title> (merged into its worktree note)">_<id>.md`. | src/daemon.ts:1733-1750, src/notes.ts:50-55, src/notesStore.ts:34-38, :184-196 | S04+S05 → `worktrees/wt-0a.md`; S06 → appended to `worktrees/wt-0b.md`; three `NOW_session_…` trash files |
| 25 | All note directories (`sessions`, `worktrees`, `repos`, `tasks`) are created 0700; files are written 0600 through temp file + rename. | src/notesStore.ts:85-87, :64-70 | modes manifests |
| 26 | Hidden files and non-`.md` files in note directories are ignored (never read, never removed). | src/notesStore.ts:278-280 | `.…md.tmp-…`, `README.txt` |
| 27 | A note over 1 MiB or 50 000 chars is read cut, revision `<hash>-<size>`, flagged `tooLarge`; never rewritten. | src/notesStore.ts:55-61 | S0e's note (`notesFile.tooLarge` in the list response) |
| 28 | A deleted worktree's note is projected `readOnly`. | src/daemon.ts:375 | S08's `sharedNotes` |

## Task lists (`notes/tasks/<repo hash>.md`)

| # | What | Where | Fixture |
|---|------|-------|---------|
| 29 | Old follow-ups (`assigned=` in the `dh:` comment) become steps: an open one whose work still has a note (a live worktree record, or a main-checkout session) moves into that note as `- [ ] title` with its body indented; one whose work is gone stays in the backlog with its link cleared; done ones and released ones (`was=`) only lose that bookkeeping. Runs once at start, notes written before the list. | src/daemon.ts:1966-1983, src/tasks.ts:382-396, :398-403 | `9e5f6071` → `worktrees/wt-0a.md`; `af607182` → `sessions/S0c.md`; `b0718293` unlinked; `c18293a4`, `d293a4b5` lose `assigned=`/`was=` |
| 30 | Any write of a list gives every task without `t=` an ID (`randomBytes(4)`), serialises `[X]` as `[x]`, writes only the known meta keys in the order `t wt s done auto tried from added` (unknown keys dropped), keeps raw lines, headings and bodies as written. | src/tasks.ts:55, :59-75, :119-127, :165 | `<task-1>` on the untracked task; `* [x] Star bullet…`; `unknownkey=` dropped |

## config.json (normalised in memory on every read; written back only key by key)

| # | What | Where | Fixture |
|---|------|-------|---------|
| 31 | `trustedProjects`: a single fingerprint string per root (older versions) reads as a one-element list; non-string entries are dropped; roots left empty are dropped. | src/storage.ts:131-139 | app's string → trusted (`project-info`); lib's `42` dropped; `gone: []` dropped |
| 32 | `notifications` is `true` only when it is exactly `true`; `dev_command`/`attach_scroll_sensitivity`/`agent_hooks` are kept only with the right type. | src/storage.ts:130-144 | `"notifications": "yes"` reads as false (`settings-info`) |
| 33 | Legacy `dev_command` is the global Dev command when `defaults.devCommand` is unset (source `legacy dev_command`). | src/projectConfig.ts:194-203 | lib's effective `devCommand` = `npm run dev:legacy` (`project-info`) |
| 34 | `updateAppConfig` rewrites only the keys it changes, so unknown keys survive any write. | src/storage.ts:212-232 | `futureConfigKey` (config.json is not written by the reads; byte-identical after idle) |

## ui-state.json (UI process only; the daemon never reads or writes it)

| # | What | Where | Fixture |
|---|------|-------|---------|
| 35 | Per repository root: `activeTab`/`sessionTabs` values outside `preview|terminal|git|dev|notes` dropped; `sessionFilter` outside the known filters → `active`; `sessionQuery` cut to 4096; id lists must be string arrays (cut to 10 000); `readReplies` keeps string values only (last 1000); unknown keys are dropped on the next save. | src/uiState.ts:15-28, :39-49 | app's `bogus-tab`, `12345`, `futureUiField`; lib's `nonsense` filter and non-array `collapsedSessionIds`. Unchanged in after-idle (no UI ran). |

## Files and modes

| # | What | Where | Fixture |
|---|------|-------|---------|
| 36 | The state directory is created 0700 and tightened to 0700 when it is group/world accessible and owned by this user. | src/storage.ts:29-37 | `./` 0755 → 0700 |
| 37 | A stale `daemon.pid` naming a dead process is ignored, overwritten, then removed on clean shutdown; `daemon.sock` likewise. | src/daemon.ts:483-512, :612-625 | `daemon.pid` (4194399, above any pid_max) gone after idle |
| 38 | `workers/*.pid|log` of a crashed daemon are not cleaned up at start. | src/daemon.ts:1369-1401 (only written/removed per worker) | kept |
| 39 | `handoffs/` files are never touched after export. | src/sessionFeatures.ts:94-100 | kept |
