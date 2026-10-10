# Agent signals: roadmap

What is built on agent hooks so far, and what is planned next. Implementation details of what shipped are in `HANDOFF.md` (*Design rules* → activity, protocol v43); user-facing behaviour is in `docs/no-brainers.md` (*Lifecycle signals and notifications*).

## Done (protocol v43)

1. **Correct states.** A question tool (`AskUserQuestion`, Codex `request_user_input` unverified) or plan approval (`ExitPlanMode`) is needs-input (`?`), not working. A `working` signal becomes unknown when the screen settles (Esc sends no hook). Agent signals are on for Claude unless `agent_hooks: false` (`true` adds Codex). Claude's hooks are `async`; signals carry `sentAt` and older ones are dropped. A native subagent's permission prompt counts; its tool use only clears a needs-input.
2. **Why a session waits.** `attention.reason` (in memory, never persisted): `asks: …`, `plan ready: …`, `wants to run/edit/fetch/use: …`, `said: <last line>`, failures. Shown in the sidebar details, an in-progress task's board details, and notifications. The hook bridge forwards only the bounded fields reasons are made from (`hookPayloadFields`).

Not yet checked against real agents: real question/permission payload wording, and whether a real subagent's `PermissionRequest` carries `agent_id`.

## Next

### 3. A queue of sessions waiting on you
- A view (Tasks board section or its own key) of every running session in `needs-input`, `response-ended`, `failed` or `limited`.
- Each row: glyph, title, reason line, time waiting (`attention.at`); sorted by urgency (`urgency()` in `tasksBoard.ts`), then longest wait.
- Enter attaches, `p` previews; a session that goes back to working drops off.
- UI only: the data is already in `session.attention`. No daemon or protocol change.

### 4. "What is it doing right now"
- Forward a short, bounded activity field from `PreToolUse`: Bash's `description` (or the start of the command), the file name for edits, `pattern` for searches.
- Register `SubagentStart`/`SubagentStop` for a count of running subagents.
- Details line: `working: Run test suite · 2 subagents`.
- These fire constantly: keep activity in memory and send at most one update per second, separate from `session-updated` (no disk writes). Needs a small new event type and a protocol bump.

### 5. Plans become notes and tasks
- `ExitPlanMode` carries the full `plan` and `planFilePath`; the bridge currently cuts the plan to 2,000 characters for the reason line.
- Keep the full plan (or the path, and read the file) in the daemon.
- Notes tab banner: "Plan ready: s saves it to notes, t makes tasks". `s` appends it to the session's note via `NotesStore.save`; `t` turns each top-level list item into a task linked to the session's work (existing `task-op`).
- The plan file stays Claude's; Deckhand only reads it.

### 6. Turn-end summaries for review
- At `Stop` the daemon has `said: …`; add the worktree's change counts (`readChanges`, cheap).
- Keep `lastTurn: {at, said, files}` in memory plus a "reviewed" flag cleared when the session is opened.
- Tasks board and the queue (3): "ended · 5 files changed · 'Implemented X'"; unreviewed sessions with changes first; `m` merges from there.

### 7. Answering without attaching
- The reason already keeps the first question's `options`.
- In the Preview pane, a waiting question lists its options; picking one sends keystrokes to the PTY (the option's number or arrows, then Enter). Depends on Claude's dialog layout: check the option labels are on screen first, refuse otherwise.
- Structured alternative: a briefly waiting `PreToolUse` hook returning `permissionDecision: "allow"` with `updatedInput` (the original `questions` plus `answers`). Robust, but the dialog is hidden while it waits — better suited to orchestrated sessions (a Deckhand MCP, see `deckhand-mcp-cli.md`) than interactive use. For Codex, the app-server answers requests natively, with the TUI's prompt still visible (`codex-app-server.md`).
- Permission prompts could get y/n the same way (keystrokes).

## Also outstanding

- **Hook health:** show when a session last received a signal, so a missing Codex setup is distinguishable from guessed activity.
- **Codex:** log a real `request_user_input` payload to confirm its shape (the bridge forwards it, untested). Possibly moot: the app-server would replace Codex hooks altogether (`codex-app-server.md`).
- **Pi:** a Deckhand extension loaded with `pi --extension`, using `agent_settled` (idle) and `tool_call` (activity); Pi has no question tool.
- **Claude `StopFailure`:** now documented, but still left out of Claude's hook settings (`integrationArgs`); registering it gives real `rate limited` / `overloaded` reasons.
- **Late `working` signal:** one that arrives after the screen already settled isn't caught by the idle fallback (it triggers only on the active→idle transition). Rare, since agents animate while working; a timer would close it.
- **Hook cost:** async hooks still start a Node process per event; an `http` hook to a loopback endpoint on the daemon (authenticated by the hook token) would avoid that if it ever matters.

## References

- Claude Code hooks: https://code.claude.com/docs/en/hooks
- Codex hooks: https://learn.chatgpt.com/docs/hooks
- Pi extensions: https://pi.dev/docs/latest/extensions
- Related future work: `codex-app-server.md` (Codex without hooks), `deckhand-mcp-cli.md` (agents driving Deckhand)
