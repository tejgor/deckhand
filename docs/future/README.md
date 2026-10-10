# Future work

Proposals, research and plans for work that is **not built yet**. Nothing here describes current behaviour: for that, see `README.md`, `docs/no-brainers.md` (user-facing) and `HANDOFF.md` (implementation).

Each doc starts with its status. When something here ships, move the lasting parts into `HANDOFF.md` / `docs/no-brainers.md`, then trim the doc to what is still open (or delete it).

| Doc | Status | What it covers |
|---|---|---|
| [agent-signals-roadmap.md](agent-signals-roadmap.md) | Items 1–2 shipped (protocol v43); 3–7 planned | What Deckhand builds on agent hooks: the waiting queue, live activity, plans → notes/tasks, turn summaries, answering without attaching, and smaller open items |
| [codex-app-server.md](codex-app-server.md) | Researched; needs a live experiment before any build | Replacing Codex hooks with a private Codex app-server per session that the TUI and Deckhand both connect to; why Claude stays on hooks |
| [deckhand-mcp-cli.md](deckhand-mcp-cli.md) | Idea, not started | A scriptable `deckhand` CLI and a `deckhand mcp` server so agents can create, drive and watch sessions ("spin up 5 sessions for these tasks") |

Research artifacts made while writing these (generated protocol types, probe scripts) lived in a temporary scratchpad and are not kept; each doc says how to regenerate what it relies on.
