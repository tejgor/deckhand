# Work items and notes

**Status:** designed 2026-11-14 with the user. Build order step 1 (the list model and file format) is built: `crates/core/src/task_list.rs`, with the ops documented on `tasks::apply_task_op`. Step 2 (the migration) is built but not run yet: `crates/core/src/work_items_migration.rs` (the rules) and `crates/daemon/src/work_items.rs` (the run: backup, marker, report); step 3 calls it at start. Replaces how tasks, steps and notes work today (`docs/features.md`, *Tasks* and *Notes*). The orchestrator's rework to one orchestrator per idea (`orchestrator.md`, *Part 3, reworked*) builds on it and comes after.

## Why

Today the same information lives in several places, joined loosely:

- **Tasks** are lines in a per-repository file (`notes/tasks/<hash>.md`), linked to a worktree (`wt=`) or a main-checkout session (`s=`).
- **Steps** are checkboxes in a **worktree's** note (`notes/worktrees/<id>.md`), or a main-checkout session's own note.
- `↗` lines join a note item to the task it was sent to, with keys to move work back and forth.

So a task belongs to a worktree rather than a session: sessions sharing a worktree share one checklist, and a session can't break down its own work. A note is two things at once, thinking and the step list. And the orchestrator would add a third layer (ideas, parked steps, dispatched sessions) on top.

## The model in one sentence

**There is one to-do list, and any line on it can have a session working on it.**

The list nests. When a session takes on a line, that line is its job and the lines under it are its checklist. Everything else is a position on that one list, not a feature of its own:

| Word | Is |
|---|---|
| **Backlog** | top-level lines nobody is on |
| **A session's task** (`◆`) | the line it is on |
| **Its steps** | the lines under that line |
| **Parked work** | lines nobody is on yet, under a line someone is |
| **An idea** | the line an orchestrator is on |

### Walkthrough

You press `O` and type "rework auth". The list gets a line, and the orchestrator is on it:

```
☐ Rework auth                          ← ◎ orchestrator
```

The plan adds lines under it. On approval, two get sessions; the third waits:

```
☐ Rework auth                          ← ◎ orchestrator
  ☐ Token storage                      ← session "Token storage"
  ☐ Login UI                           ← session "Login UI"
  ☐ Remove legacy endpoint             (nobody yet: waits for Token storage)
```

Token storage's line grows steps, added by you or its agent. Handing one off is the same action as everything else, putting a session on a line, which makes a sub-session (stacked on Token storage's branch, as built):

```
  ☐ Token storage                      ← session "Token storage"   1/3
    ☑ schema
    ☐ encryption
    ☐ refresh tokens                   ← sub-session "Refresh tokens"
```

Merging Refresh tokens ticks its line; merging Token storage ticks its own; "Rework auth" shows **1/3**, and "Remove legacy endpoint" is ready for `n` (or the orchestrator). When every line under the idea is ticked, the idea ticks.

### Rules

- **A session is on at most one line, created when something needs it.** Starting a session from a line puts it on that line. A session started without one gets a line (named after it, under its parent's line for a sub-session, else top-level) only once something needs it: you or its agent add a step, a line note or a brief to it. Until then it has none, so quick sessions don't fill the list (a repository can have hundreds of sessions). A session's line ticks when it is done and is unhooked when it is removed.
- **The trees line up.** A sub-session's line is under its parent's line. The sidebar's session tree and the list have the same shape where sessions exist; lines nobody is on are the only extra.
- **Starting work is one action.** `n` on a line (the Tasks board), a session handing off a step, or an orchestrator's plan: each puts a new session on a line. On a line nobody is on, it makes the session; on a line under the selected session's line, a sub-session.
- **Ticking.** Merging a worktree ticks the lines of its sessions; marking a session done (`space`) ticks its line; ticking a line by hand ticks only it. A line with lines under it shows `ticked/total` of its direct lines, and ticks itself when the last one ticks (it can be reopened).
- **Unticked lines when their line ticks** (merged with steps still open): they move up to the parent line (the idea, when there is one), else to the backlog, marked *left open in ⎇ branch*, as today.
- **Deleted unmerged.** A worktree deleted without merging, or a session's last removal, unhooks its line (nobody is on it) and marks it *tried in ⎇ branch*, as today; the line stays where it is in the tree.
- **Moving** (`w`) moves a line, with its lines and notes, anywhere in the tree; onto a session's line it becomes a step of it.

## Notes

Notes are separate from the list. They are prose, never ticked, never counted. There are three kinds, each with one job:

| Note | Belongs to | For | Lives |
|---|---|---|---|
| **Line note** | a line | thinking about the work: context, the approach, decisions, findings | as long as the line |
| **Session note** | a session | reminders and observations about this conversation or attempt | as long as the session |
| **Scratchpad** | the repository (shared by its checkouts) | loose thoughts not tied to any work yet | always |

- **A line note exists before anyone works on the line**, so you can jot thoughts on parked work. Whoever takes the line on is given its brief and its note.

### Briefs

A brief is not a note: it is **what the line was set up to be**, written once at the start, while a note grows as the work goes on. Kept apart, the note stays yours and the brief doesn't bloat it.

- **A line can have a brief**, its own text in its own file. An orchestrator's plan writes one for each line it puts a session on (exactly what that session was told, its first message) and one for the idea's line (what was agreed in the planning chat: the problem, the approach, decisions, what was ruled out). You can write one for a line you set up yourself.
- **Shown collapsed**: the Notes tab shows `▸ Brief · from ◎ Rework auth · 14 Nov` above the line's note, and a key expands it. It can be edited, as a record rather than a contract.
- **Passed on**: a session taking the line on (a retry, say) is given the brief and the note. Handoffs include it; the Tasks board marks lines that have one (`✎`) and shows it in the line's details.
- **Agents write to line notes.** `deckhand note add <text>` appends to the note of the caller's line: findings for whoever works on it next. The session note is yours; agents don't write to it.
- **Session notes** hold things like "review its diff of crypto.rs before merging" or "told it to keep refresh tokens out of scope". When a session is removed, Deckhand offers to move what is still open in its note into its line's note.
- **Checkboxes in any note are text** you tick in place. `ctrl+p` on one promotes it to a line: under the note's line (from a line or session note), or into the backlog (from the scratchpad).
- **The Notes tab** of a session shows its line's steps (tick with `space`), then its line's note, then its session note, each labelled; a key switches which one you edit. A sub-session's tab can show its parent's line note collapsed, for context.
- **Handoffs** (`H`) include the line's note and the session note.

## Agents

- `deckhand step add <text>` adds a line under the caller's line; `deckhand step done <id>` / `reopen <id>` tick its lines; `deckhand step list` lists them. They replace `deckhand note add` as the way to add steps.
- `deckhand note add <text>` writes prose to the caller's line note (above).
- `deckhand task …` keeps working on lines (`task add` adds to the backlog; `task move` moves a line).
- The always-loaded agent guide is at its byte budget: the new wording has to fit by replacing, not adding.

## Storage

- **The list:** one file per repository, today's `notes/tasks/<hash>.md` extended. It is already a nested Markdown checklist with `<!-- dh:… -->` bookkeeping per line: a line's ID (`t=`), who is on it (`s=` a session ID; `wt=` goes), its parent comes from indentation, done/added/tried/from as today. Hand edits in an editor keep working: each change is applied to the file as it is on disk.
- **Line notes:** a file per line, `notes/lines/<line id>.md`. Indented detail lines under a list line (today's task details) become the start of its note.
- **Briefs:** a file per line that has one, `notes/briefs/<line id>.md`, with who wrote it and when in its first line's bookkeeping comment.
- **Session notes:** `notes/sessions/<session id>.md`, as today for main-checkout sessions, now for every session.
- **Scratchpad:** `notes/repos/<hash>.md`.

## Migration (once, by the daemon, with backups)

Decided with the user on 2026-11-14, against their real data (2 task files, 16 tasks, 9 worktree notes, 4 session notes, no `↗` lines, 210 sessions).

- **Backup first**: `notes/` (the list included) is copied to `notes/trash/work-items-migration-<time>/` before anything changes; a marker makes the migration run once; old note files go to the backup only after the new files are written, so an interrupted run loses nothing and runs again.
- **Lines only where something needs one** (above): no line is created for a session with nothing to put on it.
- **A worktree-linked task** (`wt=`) moves onto one session: the worktree's owning session (the one that created it), else its oldest session. Its details become its line note.
- **A worktree note**: its checkboxes become lines under that worktree's task line (checked state kept); its prose and headings become that line's note. A worktree note with checkboxes or prose but no task first gets a line for the owning session, named after it. `↗` lines are dropped (their item is already a line).
- **A main-checkout session's note**: the same, under that session's line (created if needed), whether the session runs or has exited.
- **Open steps of merged work**: lines under a line whose worktree was merged that are still open move to the backlog marked `from=<branch>` (as merging does from now on), so nothing is hidden under a ticked line.
- **One session on two lines** (a hand-edited legacy file): it stays on the first; the others are unhooked.
- A frozen fixture of today's layout (like `fixtures/state/home-v45`) keeps migrating; the migration is pure (texts and records in, texts out) and tested on copies of real layouts.
- **As built**: the marker is `notes/work-items-migrated.json` (it travels with `notes/`, unlike a field in state.json that an older version would drop); the backup gets `migration-report.txt`, what the run did line by line, and daemon.log a one-line summary. A run reads its inputs from the backup and seeds its new IDs from the backup's name, so an interrupted run (its backup still holds `in-progress`) runs again to the same result.
- **Checked before it runs for real**: copy the real `notes/` and `state.json` into the dev namespace (`~/.deckhand-dev`) and open the dev build; the real `~/.deckhand` is untouched until the new version is installed.
- Every Deckhand of the repository needs updating together: an older one reading the new list would show it wrongly. The protocol bump makes an old UI refuse the new daemon, but two installs (say a stable and a dev build) could share one repository's files; say so in the upgrade note.

## What changes for you

- **Steps are not written inline in notes any more.** Today a checkbox anywhere in a worktree note is a step, among your prose. Now the list and notes are separate: steps are lines, notes are thinking. `ctrl+p` turns a note checkbox into a line, and the Notes tab shows the steps above the notes.
- **Each session has its own checklist**, even two sessions in one worktree (their lines nest).
- **"Back to its note", `↗` lines and the Tasks board's Notes view go**: with one list there is nothing to move between. The Notes view becomes a view of notes (line notes, session notes, the scratchpad) for reading, not an inbox of checkboxes.

## Not in this redesign

- **The agent's own todo list** (Claude's checklist for its turn, readable through hooks) as live activity: `working: 2/4 · Update the login handler`. It is the agent's scratchpad, not part of the list; it belongs to `agent-signals-roadmap.md` item 4.
- The orchestrator's rework, which comes next (`orchestrator.md`).

## Build order

1. The list model in `crates/core` (lines, parents, who is on what, roll-up, the moves) with the file format and its parser/writer, keeping today's file readable. Pure, unit-tested, with corpora.
2. The migration, against copies of real layouts and the frozen fixture.
3. The daemon: sessions always on a line, ticking on merge/done/delete, line notes and session notes, the agent commands.
4. The UI: the Tasks board as the list (nesting, counts, who is on what, `n` on any line), the Notes tab (steps, line note, session note), `ctrl+p`, the scratchpad.
5. Docs: `features.md` *Tasks* and *Notes* rewritten.
