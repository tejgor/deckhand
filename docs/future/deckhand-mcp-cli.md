# A scriptable Deckhand: CLI and MCP server

**Status:** idea, discussed 2026-10-10; nothing built. Goal: let an agent (or a script) use Deckhand — "spin up 5 sessions for these tasks, tell me when they're done" — while the user keeps watching and taking over in the dashboard.

## Why it fits

- **The daemon already does everything the UI does.** `create`, `list`/`subscribe`, `input`, `restart`, `kill`, `task-op`, `merge-preview`, `save-note` and the rest go over `~/.deckhand/daemon.sock` (list in `HANDOFF.md`, *IPC request/event types*). A CLI or MCP server is a thin adapter, with no new logic in the daemon.
- **Sessions an agent creates show up in the dashboard for free.** The daemon broadcasts `session-updated`, so the user sees spawned sessions in the sidebar and can attach to any of them.
- **The caller is identifiable.** Every agent Deckhand launches gets `DECKHAND_SESSION_ID`, `DECKHAND_LAUNCH_ID` and `DECKHAND_HOOK_TOKEN`. An MCP server running inside a session can make new sessions sub-sessions of its caller and authenticate it the same way hook callbacks are.
- **Tasks are the natural unit.** "Spin up 5 sessions" usually means "start these 5 tasks in their own worktrees": `create` with `taskId` already links a task to its worktree, and merging or marking done ticks it.
- **Status is now good enough to drive this.** Agent signals (protocol v43) give state plus a reason line (question, permission, last reply line), which is what an orchestrator needs to know whether a session is done, stuck, or waiting.

## Plan

1. **Scriptable CLI first**, over the existing client (`src/client.ts`): `deckhand ls`, `deckhand new --agent claude --worktree --task <id> --prompt …`, `deckhand send <id> "…"`, `deckhand status <id>`, `deckhand wait <id> [--timeout]`, `deckhand tasks`. JSON output with `--json`. Useful by hand and in scripts, and agents can call it from Bash without any MCP setup.
2. **Then `deckhand mcp`** (stdio), a thin wrapper over the same client code, shipped in the same package so it always shares `PROTOCOL_VERSION` (bumped often; a separate package would break on every bump). Typed tools for Claude and Codex; Pi may not support MCP, the CLI covers it.

### First tool set

- `list_sessions`
- `create_session`: agent, title, worktree mode, base branch, `taskId` or prompt, `submit`
- `send_message`
- `get_status`: state, reason line, age (from `attention`)
- `wait_for_idle`: with a timeout; returns the state and reason it stopped on
- `read_screen`: the tail of the preview, for a quick look
- `list_tasks`, `add_task`
- `merge_preview` (read-only)

Left out at first, or behind a confirmation: `kill`, `remove`, `delete-worktree`, `merge-worktree`. "Clean up the 5 worktrees" from an agent is exactly what the deletion safety checks exist for.

## Hard parts

1. **Submitting a prompt: send by default when an agent creates the session.** Starting from a task in the UI types the task into the agent's input without sending it, on purpose: a task's title and body are usually a reminder, not a full brief, and the user adds detail before sending. An orchestrating agent is in a different position: it composes a complete prompt (context, constraints, what "done" means) from the task and its own understanding, so holding that prompt for editing only stalls the session.
   - `create_session` / `deckhand new` take a `prompt` and send it by default (`submit: true`); `submit: false` keeps today's typed draft, for "prepare sessions for me to review".
   - With a `taskId`, the task is still linked (worktree, ticking on merge or done); the orchestrator's prompt replaces the raw `<title>: <body>` text as what the agent receives.
   - Send through the first-message argv path (`firstMessageArgs` in `src/daemon.ts`, as handoff children use), not paste + Enter. That also lifts the draft's one-line limit (Claude collapses a multi-line paste into an uneditable placeholder): a prompt passed at launch can be long and structured.
2. **Knowing a session is finished.** Use agent signals: `response-ended` with the `said: …` reason, `needs-input` with the question or permission. Screen-idle alone can't tell finished from waiting. Pi has no signals (see `agent-signals-roadmap.md`).
3. **Reading results.** The preview is a lossy scrape. Point the orchestrator at durable results: the `said:` line, the session's notes file, commits / the Changes view, or the agent's transcript.
4. **Answering questions.** For orchestrated sessions, answering programmatically is reasonable: Claude through a `PreToolUse` hook returning the answers, Codex through the app-server (`codex-app-server.md`). See roadmap item 7.
5. **Runaway spawning.** A session that can create sessions can create sessions that create sessions. Cap nesting depth and concurrent sessions, and attribute every spawn to the calling session.
6. **Trust.** The MCP server acts with the user's permissions; destructive tools stay out or need confirmation, and repository actions keep their existing trust checks.

## Codex note

For an orchestrator that drives Codex, the app-server (`codex-app-server.md`) can start threads and turns and read results directly, which is more reliable than typing into the PTY. Headless options (`codex exec --json`, `@openai/codex-sdk`) also fit an orchestrator, but lose the TUI the user would attach to.
