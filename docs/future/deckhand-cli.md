# A scriptable Deckhand: what is still open

**Status:** v1 built 2026-10-29 (protocol v56): an agent in a Deckhand session takes Deckhand actions **when you ask it to** with `deckhand new|ls|task|note|kill|remove|guide`, told about them at every launch. How it behaves is in `docs/features.md` (*Deckhand commands for agents*), how it works, why a CLI rather than files or an MCP server, and why Codex alone gets one, in `HANDOFF.md` (*Agent commands*). Open: the per-agent gaps below, then the orchestration phase, where agents drive sessions on their own.

## Open in v1, per agent

- **Codex: the `deckhand` tool in a live session.** Codex's sandbox blocks the CLI's socket and Codex can't pre-allow one shell command, so Codex 0.122.0 and later get the commands as an MCP tool instead (`deckhand mcp`, see `HANDOFF.md`). Its `-c` value is checked against Codex 0.163.0's parser (`codex mcp get`), not yet in a session. Check: that the tool runs without a prompt (`default_tools_approval_mode = "approve"`) and acts as the session (the `env_vars` reach the server); how many tokens the tool definition costs, and whether Codex defers it behind its tool search for models that have one (its source says it does), in which case the trigger alone carries the always-loaded cost; that `0.122.0` is the right minimum (the version `default_tools_approval_mode` and `env_vars` arrived in is not checked against Codex's history).
- **Codex: other `developer_instructions`.** The trigger is merged only with the top-level `developer_instructions` of `$CODEX_HOME/config.toml`. A profile's or a project config's own value is not read; check which one wins over the `-c` override.
- **Claude: the `PATH` link.** Deckhand puts `<DECKHAND_HOME>/agent-plugin/bin` first on the agent's `PATH`. Not checked: whether a shell Claude's Bash tool starts keeps it first when the user's shell startup files set `PATH` themselves. If not, a dev build's agents would run the installed `deckhand`.
- **Pi**: `--skill` is passed when `pi --help` lists it; not yet checked that Pi then keeps only the skill's description in context.

## Later: orchestration

Once agents should drive sessions themselves (wait for children, read results, follow up):

- **Commands**: `deckhand status <id>`, `wait <id> --until idle|ended|waiting [--timeout]`, `send <id> <text>`, `read <id> --screen`. Selectors (`id:`, `branch:`, `name:`) as in Orca (`ideas-from-orca.md`).
- **Spawn limits**: nesting depth and concurrent children caps, configurable, once agents start sessions without being asked. Today the guide's "only when the user asks" is the only guard against unasked use, and the daemon's limits (own sub-sessions only, nothing deleted) are what make that acceptable.
- **Knowing a session is finished.** Use agent signals: `response-ended` with the `said: …` reason, `needs-input` with the question or permission. Screen-idle alone can't tell finished from waiting. Pi has no signals (see `agent-signals-roadmap.md`).
- **Waiting is not finished.** A worker that asks a question in plain words and stops looks exactly like one that finished. herdr-projects requires workers to report that they wait (`report --state blocked --kind question|permission|auth`) before stopping; Deckhand's version is `deckhand status --state waiting` (`README.md` 2b).
- **Sending to a session that waits.** `send` refuses, sending nothing, when the session waits on a question or permission; otherwise the prompt is typed into the dialog. herdr's `agent prompt` does this (`agent_blocked`) and also fails when no work starts within 5 seconds (`agent_prompt_stalled`) (`ideas-from-herdr.md`).
- **Reading results.** The preview is a lossy scrape. Point the caller at durable results: the `said:` line, the session's notes file, commits / the Changes view, or the agent's transcript.
- **Answering questions.** For orchestrated sessions, answering programmatically is reasonable: Claude through a `PreToolUse` hook returning the answers, Codex through the app-server (`codex-app-server.md`). See roadmap item 7.
- **Codex.** The app-server (`codex-app-server.md`) can start threads and turns and read results directly, which is more reliable than typing into the PTY. Headless options (`codex exec --json`, `@openai/codex-sdk`) also fit, but lose the TUI the user would attach to.
- **The guide** grows with these commands; the always-loaded trigger stays within its byte budget (`ALWAYS_LOADED_BUDGET`, checked by a unit test).
