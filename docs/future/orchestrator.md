# Orchestrator, stacked sub-sessions and sidebar depth

**Status:** discussed 2026-11-13; parts 1 (stacked sub-sessions, protocol v58) and 2 (sidebar depth) built, part 3 (the orchestrator) not. What parts 1 and 2 do is in `docs/features.md` (*Stacked sub-sessions*, *Merging and done*); what is below for them is the design record. Three parts that build on each other: sub-sessions that branch from and merge into their parent (1), a sidebar that shows how deep they nest (2), and an orchestrator that turns a described problem into a tree of sessions (3). 1 and 2 are useful without 3.

## Why

Today, to have Deckhand split work into sessions you start a session, wait for the agent, then ask it, and its guide says to take Deckhand actions only when asked. What is wanted instead: describe a problem once, have it split into sessions and sub-sessions with complete briefs, then do the actual work inside those sessions. The orchestrator is a **planner and dispatcher, not a supervisor**: work never flows back into it, so it stays small and is reused for idea after idea. Supervising (waiting on children, reading results, sending follow-ups) stays in `deckhand-cli.md`, *Later: orchestration*, and is not needed here.

For a tree of sessions to mean anything in Git, a parent has to be where its children's work comes together. That is part 1.

## 1. Stacked sub-sessions

### Today

Before stacking, a sub-session's parent was organisational only. A sub-session in a new worktree branched from `worktree.branchFrom` (by default the launch checkout's HEAD — which, as a sub-session is created from its parent's directory, was already the parent's branch; `default` and `origin` were not), and `m` merges every worktree into the launch checkout's branch (`docs/features.md`, *Merging and done*). Two sub-sessions of **Auth rework** are siblings of it in Git and each merges straight into main.

### The model

- **Each worktree records its target**, the branch it integrates into, when it is created. This is a new field, separate from `mergeTargetBranch`, which records where a merge went.
- **Stacked by default, everywhere** (`N`, `deckhand new`, the orchestrator), so sub-sessions behave the same however they are made:
  - parent has its own worktree branch: the child starts from the parent's branch and targets it;
  - parent in the main checkout or without a worktree: the parent's branch is the launch branch, so stacked and flat are the same thing;
  - child in the parent's worktree (`same`): nothing to merge; it commits on the parent's branch, as today.
- **Switchable** in the create form (as the base is today), in the orchestrator's plan, and in the merge dialog (a key on `Into <branch>` toggles between the parent's branch and the launch branch).
- **Any depth.** Merging works the same at every level; the rules below keep deep stacks readable.
- **Parents are usually containers** (they integrate and test their children) **but may write code.**

### One hop at a time

Every message about a session concerns its own target and its direct children. Deeper descendants are folded into the child they sit under.

- **The path, with the hop marked.** `Into` shows the chain, the hop being merged underlined:
  ```
  Into  auth-rework  →  main
        ‾‾‾‾‾‾‾‾‾‾‾
  ```
- **Integration state before the dialog.** A parent's row shows `2/3 in`, how many of its direct children have merged into it, so the dialog tells you nothing new.
- **Consequences, not warnings.** Merging a parent whose children are not all in says what happens to them, and blocks nothing:
  ```
  Not in this branch yet:
    Login UI (+2 under it)   → will merge into main directly afterwards
  ```
- **Quiet retargeting.** When a target is merged or deleted, its children move to the next target up (as stacked pull requests are retargeted). The session's details say `target: main (Auth rework merged)`; no dialog.
- **Merged means in its target.** A child shows `✓` once its tip is in its target branch; whether its commits reached main is the parent's `✓`. Merged detection (today: tip in the default branch, see *Merging and done*) checks the target instead, falling back to any target further up.
- **Tasks tick at the target.** Merging a child into its parent ticks the child's task; the parent's task tracks shipping.

### Merging into a parent's branch

The parent's branch is checked out in the parent's worktree, so the merge runs there, possibly under a running agent.

- Refused while the parent's agent is working, saying so.
- While it is idle, the existing warning about uncommitted files the merge would touch applies.
- Children start from the parent's committed tip: its uncommitted changes are not in them (the caveat forks into new worktrees already have).

### Elsewhere

- Cleanup and pull requests compare against the session's base already. A stacked child's PR (`i` → `c`) targets its parent's branch, giving stacked PRs on GitHub.
- Handoffs and merge commit messages already name sub-sessions by their path.

## 2. Sidebar depth (built 2026-11-13)

Grandchildren used to render at their parent's level: each level stepped two columns, and a parent's `▾` took those two columns. Each row's indent now counts its listed ancestors' `▾` and `↳`/`⑂` (`tree_indent` in `crates/core/src/sidebar_model.rs`), with no four-level cap; on a narrow row deeper levels fall back to the old two columns a level so the title keeps its minimum. Behaviour is in `docs/features.md`, *Sessions and sub-sessions*. Still open: thin `│` guides instead of blank indentation, if deep trees get hard to scan.

## 3. The orchestrator

### How it is used

1. Open the repository's orchestrator (a key in the TUI, or `deckhand do "<problem>"`) and describe the problem.
2. It reads the code enough to split it well, using its own subagent for broad exploration so file contents do not stay in its context.
3. It proposes a tree, for example:
   ```
   ◇ Auth rework                    new worktree from main
     ├ Token storage                stacked · claude
     └ Login UI                     stacked · claude
   ◇ Migration script               no worktree · codex
   ☐ Backlog: remove legacy endpoint (after Auth rework merges)
   ```
   You revise it in conversation.
4. On your go, it creates the tree in one call. Each session gets a complete brief as its first message; work that must wait goes to the backlog as a task.
5. It is done. You work in the sessions.

### Stateless by design

It never relies on remembering what it made. Asked to "add a sub-session to the auth work for the tests", it reads `deckhand ls` and `deckhand task list`. Clearing its context loses nothing, so one long-lived orchestrator per repository stays small.

### What it needs

- **A role.** A session marked as the orchestrator, top-level, without a worktree, with its own guide: plan and dispatch, do not implement; write each brief so it stands alone; read state from Deckhand, not memory; propose before creating.
- **Building trees: `deckhand new --parent <id>`.** Today a command run in a session can only make the caller's own sub-sessions or a top-level one (`docs/features.md`, *The caller*). The orchestrator must put **Token storage** under **Auth rework**. Record who started a session (`spawnedBy`) separately from its parent; `--parent`, `kill` and `remove` are allowed on sessions the caller started and their descendants.
- **Placement.** Its sessions are real top-level sessions (with a small *from orchestrator* marker), not its sub-sessions, so it does not become the parent of everything.
- **`deckhand plan apply <json>`.** Validates the whole tree (parents, worktree modes, targets, agents, tasks) before creating anything, then creates it, instead of a run of `new` calls that can fail halfway. The natural place for a confirmation in the TUI too.

## Decided (2026-11-13)

- **Briefs are sent.** Each session starts on its brief at once: that is the orchestrator's initial push. The further work after it is what you do in the sessions.
- **Start what can run in parallel now; park the rest** as backlog tasks. Starting a session from a task links them, so merging ticks it.
- **Claude only** for the orchestrator at first. Codex would need the `deckhand mcp` path, since its sandbox keeps the CLI from the daemon.
- **Confirmation in Deckhand, to start with.** The plan is discussed and revised in chat; `plan apply` then opens a confirmation in Deckhand (the tree, what will be created, validation errors; `enter` creates, `esc` cancels), so no plan launches sessions without a keypress even though `deckhand` commands are pre-allowed. No editing in that dialog. Chat only (the guide's "propose before creating" as the only guard) felt more natural; if the extra keypress gets annoying, drop the dialog, or make it a setting. Build it so that is a small change: `plan apply` validates and creates the same way either way, and the dialog only gates it.

## Open questions

1. **The task system, later.** Parking work an orchestrator splits off puts more structure into the backlog than it holds today: dependencies ("after Auth rework merges"), where it belongs in a tree (a future sub-session of a given parent, stacked on its branch), and which plan it came from. Today a task is a flat checklist item with details. Revisit the task model before or alongside the orchestrator; until then a parked task carries this in its details as text.

## Build order

1. ~~Sidebar depth (part 2)~~ built.
2. ~~Stacked sub-sessions (part 1)~~ built (protocol v58). Left out: undoing a retarget when a merge is unmarked; a deeper "working" guard (a session waiting on input does not count as working).
3. `--parent`, `spawnedBy` and `plan apply`. **M**.
4. The orchestrator role, entry points and guide. **S–M**.
