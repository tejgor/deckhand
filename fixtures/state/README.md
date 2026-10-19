# Legacy state fixture (v45 TS daemon)

A synthetic, complete `DECKHAND_HOME` as a v45 TS daemon (protocol 45) and older versions would leave it, and what the
TS daemon makes of it. Nothing here was copied from a real `~/.deckhand`.

| path | what |
|---|---|
| `home-v45/` | the input home: `state.json`, `config.json`, `ui-state.json`, `notes/{sessions,worktrees,repos,tasks,trash}/`, `handoffs/`, `workers/`, a stale `daemon.pid`, `daemon.log`, an old `state.json.corrupt-*` |
| `home-v45.modes.json` | the input's modes (`0600` files, `0700` dirs as TS writes them; `./` is `0755` on purpose: TS tightens it). Git keeps neither modes nor empty directories, so this manifest is authoritative |
| `sandbox.json` | the world around it: placeholder root, git repositories/worktrees/commits to create, fake agent versions |
| `home-v45.after-idle/` + `home-v45.after-idle.modes.json` | the home after the TS daemon started, served the reads, and stopped cleanly (SIGTERM) |
| `read-responses.jsonl` | one `{"request", "response": {"ok", "data"|"error"}}` per read, in order (`requestId` omitted) |
| `MIGRATIONS.md` | every implicit migration the fixture exercises, with TS file:line references |

Regenerate with `npm run build && node --import tsx parity/gen/state.gen.ts` (`--check` exits 1 on drift, `--keep`
keeps the sandbox, `--sandbox-only` prints the sandbox's SHAs and placeholder note hashes for editing the fixture).
Two consecutive runs produce identical output.

## Placeholder root and sandbox

Absolute paths inside the fixture start with `/tmp/deckhand-fixture` (`P`). The generator creates a fresh sandbox
`R = realpath(mkdtemp($TMPDIR/dhs-))` (short, so `R/deckhand-home/daemon.sock` fits `sun_path`) and:

1. runs `sandbox.json` `steps` under `R`: `init` (`git init -b main`, no gpg, no hooks), `write`, `mkdir`, `commit`
   (`add -A`), `worktree` (`worktree add -b <branch> <path> HEAD`), `merge` (`--no-ff`). Git runs with
   `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1`, the fixed identity, and author/committer date
   `git.start + tick * stepSeconds` where `tick` increments before every commit, worktree and merge, so every SHA
   is fixed; they are checked against `expect` (state.json and the notes reference them);
2. writes fake `claude`/`codex`/`pi` into `R/bin` that only answer `--version` (`agents`), so the daemon's version check
   never runs a real agent;
3. copies `home-v45` to `R/deckhand-home`, replacing `P` with `R` in every file, renaming note files keyed by a path
   hash (`notes/repos/<h>.md`, `notes/tasks/<h>.md`, `h = sha256(path.resolve(root)).slice(0, 16)`) from the hash
   of `P/<root>` to the hash of `R/<root>` for every repository/worktree/dir root in `steps`, and applying the modes.
   Fixture note hashes: `repos/app` `8528dc67ee723172`, `repos/lib` `ef34d05a15277f22`.

The daemon is `node dist/cli.js --daemon`, cwd `R/repos/app`, umask `022`, environment exactly:
`PATH=R/bin:/usr/bin:/bin:/usr/sbin:/sbin`, `HOME=R/home`, `DECKHAND_HOME=R/deckhand-home`, `DECKHAND_DEV=0`,
`DECKHAND_AGENT_LATEST=<sandbox.json agentLatest>`, `SHELL=/bin/sh`, `TERM=xterm-256color`, `LANG=en_US.UTF-8`,
`TZ=UTC`, `TMPDIR=R/tmp`, `XDG_{CONFIG,STATE,CACHE,DATA}_HOME` under `R/home`, `CLAUDE_CONFIG_DIR=R/home/.claude`,
`CODEX_HOME=R/home/.codex`, `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1`.

After `ping` answers, the generator waits for the start-up merge detection to finish (S06's projected
`worktree.mergedAt` is set, and the merged worktree's task `7c3d4e5f` is ticked `auto=merge`), sleeps 300 ms, sends
the reads listed in `read-responses.jsonl` one connection each, then SIGTERMs the daemon and requires exit code 0.

Running sessions (S01, S04, S0f) and the starting one (S03) come back `exited`/`interrupted`: expected, captured.

## Normalisation (only where TS itself writes volatile values)

Applied to every captured file and response; `--check` compares normalised bytes.

| token | replaces | written by |
|---|---|---|
| `/tmp/deckhand-fixture` | `R` (and its unresolved `$TMPDIR` spelling) | everything that stores paths |
| note hash of `P/<root>` | note hash of `R/<root>` (file names and contents) | `repoNoteId` (src/notes.ts:23) |
| `<now>` | an ISO-8601 timestamp within the run (±1 s) | crash recovery `updatedAt`/`attention.at` (src/storage.ts:90), detected `mergedAt` and the sessions' `updatedAt` (src/daemon.ts:1165, :3020), `checkedAt` of agent versions and the worktree overview |
| `<now-ms>` | an integer epoch-ms value within the run | (none currently) |
| `NOW_` (file names) | the `YYYY-MM-DDTHH-MM-SS_` prefix of trash files created during the run | `trashFileName` (src/notesStore.ts:34) |
| `done=<today>` / `added=<today>` | a task date equal to the run's UTC day | `applyTaskOp` `today` (src/tasks.ts:155) |
| `<uuid-N>` | a v4 UUID that does not occur anywhere in `home-v45`, numbered by first appearance (after-idle `state.json` first, then the tree in path order, then the responses) | worktree records lifted from legacy sessions (src/worktreeRecords.ts:113 `randomUUID`) |
| `t=<task-N>` | a task ID not in `home-v45`, numbered the same way | `newTaskId` (src/tasks.ts:131, :165) |
| recomputed `revision` | a note `revision` that hashed text changed by the tokens above is replaced by the hash of the normalised text (`{text, revision}` pairs and `notes`/`notesFile.revision`) | `noteRevision` (src/notes.ts:18) |
| `"user": "<user>"` | the OS account slug in `settings-info` | `userSlug` (src/worktreeLinks.ts:67, from getpwuid) |

`daemon.log` is listed in the after-idle modes manifest but its content is not stored: TS appends wall-clock
timestamps, its pid and lines whose order depends on timing. The daemon's pid is otherwise written only to
`daemon.pid`, which a clean stop removes; no pid reaches state.json (crash recovery deletes `pid`).

## Sessions

`S01`… are the last two hex digits of `5e550000-0000-4000-8000-0000000000NN`; worktree records are
`7e000000-0000-4000-8000-0000000000NN`.

| id | program | status in | what it covers |
|---|---|---|---|
| S01 | claude | running | main checkout, root of a subtree (collapsed in ui-state), attention `working`, unknown keys, own note with a `↗` task link |
| S02 | claude | exited | clean sub-session of S01, `response-ended`, read reply (ui-state `readReplies`), `lastPreview` (`watch-preview`) |
| S03 | claude | starting | forked sub-session of S01 (`forkedFromSessionId`, `forkedFromAgentSessionRef`) |
| S04 | pi | running | managed worktree `wt-0a` with `links`, setup `running`, `devRunning`, `path` agent ref, own note |
| S05 | codex | exited | sub-session (mode `none`) in `wt-0a`, `failed` by signal 9, `devRunning`, stale projected marker, own note |
| S06 | codex | exited | managed worktree `wt-0b` whose branch was merged into main (detected), own note + existing worktree note |
| S07 | claude | exited | attached worktree `wt-0c` (manually merged record), `doneAt`, `handoffPath`, `attention.reason`, `replyRead` |
| S08 | claude | exited | archived, deleted worktree `wt-0d` (`deletedAt`, `mergeDismissedTip`), `cleanupError`, read-only shared note |
| S09, S0a, S0b | pi, claude, claude | exited | legacy (pre-record) worktree markers: two incarnations of `worktrees/app/legacy` |
| S0c | codex | exited | main checkout, legacy top-level merge markers → `doneAt`, notes in state.json |
| S0d | pi | exited | attached to the main worktree with nested markers, `notes: ""` |
| S0e | claude | exited | repo `lib`, `tooLarge` note, `startPrompt` |
| S0f | codex | running | repo `lib`, `requestedWorktreeMode: new` without a worktree, setup `pending` |
| S10 | claude | exited | references record `wt-0f` that is missing (rebuilt) |
| S11 | pi | exited | a non-git directory |

Records: `wt-0a` (no `baseSha`: backfilled), `wt-0b` (merged → detected), `wt-0c` (merged manually), `wt-0d` (deleted),
`wt-0e` (unreferenced → dropped), one without `id` (dropped). `ui-state.json` has three repository entries, including
invalid values the UI normalises (the daemon never touches it).
