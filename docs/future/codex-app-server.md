# Codex via the app-server (replacing Codex hooks)

**Status:** researched 2026-10-10 against `codex-cli 0.160.1` (the shared daemon on the same machine ran 0.162.1). The mechanism is confirmed from docs, source and a read-only local probe; the one behaviour that decides it (the TUI's prompt closing when Deckhand answers) still needs the live experiment below. Nothing is built.

## The idea

Today Codex sessions run the TUI in a PTY with `--no-daemon`, and status comes from hooks the user installs by hand in `~/.codex/hooks.json` (see `docs/no-brainers.md`, *Lifecycle signals and notifications*). Instead:

1. Each Codex session's worker starts its own private app-server: `codex app-server --listen unix://<short path>`.
2. The native TUI still runs in the PTY, as `codex --remote unix://<short path>` (also `codex resume <id> --remote …`). Attaching works exactly as today.
3. Deckhand's daemon connects to the same server as a second client and receives typed events: thread status, turn start/end, approval and question requests (and can answer them), errors, the last message.

Codex hooks would no longer be needed. Claude keeps hooks (see *Claude* below); Pi is unaffected.

## Why: app-server vs Codex hooks

| What Deckhand wants | Codex hooks (today) | App-server (second client) | Gap |
|---|---|---|---|
| Working / waiting / ended | Inferred from hook events, once installed | `thread/status/changed`: `idle`, `systemError`, or `active` with `waitingOnApproval` / `waitingOnUserInput`; `turn/started`, `turn/completed` (completed / interrupted / failed) | Small |
| Failure / rate limit | Not reported (no failure event) | `error` with typed info (usage limit, rate limit, server overloaded, `willRetry`); `account/rateLimits/updated` | Medium |
| The question and its options | `request_user_input` payload shape unverified | `item/tool/requestUserInput`: questions with id, header, text, options (label, description) | Medium |
| Approval details | Command or file only | Command, cwd, parsed actions, reason, network host; file changes as separate requests | Small–medium |
| Setup | Manual `hooks.json`, then trust in `/hooks`; redo when Node or the repo path changes | None: Deckhand passes the flags | Medium (friction) |
| Answering without attaching | Not possible | Respond to the request; the server sends each request to every subscribed client, the first answer wins, and the others get `serverRequest/resolved` | **Large** |
| Orchestration (see `deckhand-mcp-cli.md`) | Typing into the PTY | Start threads and turns, read results | **Large** |
| Conversation ID | SessionStart hook or the exit hint | `thread/started` | Small |

For an accurate sidebar alone, Codex hooks get most of the way once installed. The app-server earns its cost if answering from Deckhand or the MCP orchestrator matters, or if Codex is used a lot.

## Key question: can a second client follow the TUI's own conversation?

Yes (high confidence on the mechanism; medium on how the TUI reacts to someone else answering).

- The TUI is an app-server client. `codex --help`: `--remote <ADDR>` "Connect the TUI to a remote app server endpoint"; `--no-daemon` runs without the shared server. Since 0.157.0 the TUI starts and uses a shared daemon by default ("#47179 Enable automatic daemon startup by default"). With `--no-daemon` the server runs in-process with no socket (`codex-rs/tui/src/daemon_startup.rs`), which is why nothing outside can see Deckhand's Codex sessions today.
- The server takes several clients. The shared daemon listens on `~/.codex/app-server-control/app-server-control.sock`; a read-only probe connected and initialized in a few milliseconds.
- A second client can join a running thread: `thread/resume` "rejoins" a running thread; upstream tests `thread_resume_keeps_in_flight_turn_streaming` and `thread_resume_replays_pending_command_execution_request_approval` (a pending approval is re-sent on resume).
- Requests go to every subscriber, first answer wins (`codex-rs/app-server/src/outgoing_message.rs`); then `serverRequest/resolved`, which the TUI handles by dropping its pending request (`tui/src/app/app_server_events.rs`, `pending_interactive_replay.rs`). Not yet seen live: the TUI's visible overlay closing.
- A thread is writer-locked to one app-server process, so Deckhand must connect to the server the TUI uses.

### Why a private server per session, not the shared daemon

- The shared daemon updates itself and can run a different version from the CLI (0.162.1 vs 0.160.1 here).
- Its updater restarts it; active or queued work may be interrupted.
- Shared clients use the environment the daemon started with: the same identity problem `--no-daemon` works around today.
- `--listen unix://` without a path resolves to the shared daemon's socket, so always pass an explicit path.

## Is it "not fit for production"? (checked)

The official page (https://learn.chatgpt.com/docs/app-server) says: *"The app-server command and WebSocket transport are experimental and aren't supported for production workloads."* `codex app-server --help` also says `[experimental]`. So the label is real, but narrower and softer than "don't use it":

- **What it means:** no stability or support promise for third-party clients. The page makes no versioning guarantee; generated schemas are "specific to the Codex version you ran".
- **What it doesn't mean:** it is not a side project. OpenAI says it is "the interface Codex uses to power rich clients (for example, the Codex VS Code extension)" and recommends it "when you want a deep integration inside your own product". The Codex TUI itself uses it by default since 0.157.
- **Transports:** `stdio://` is the default and unlabelled. `ws://IP:PORT` is "experimental and unsupported". `unix://` has no label on the page (in practice it carries WebSocket framing over the Unix socket).
- **The methods Deckhand needs are on the stable surface.** The docs gate some methods behind the `experimentalApi` capability; clients that don't opt in stay "on the stable API surface". Checked by generating both schemas from the installed 0.160.1 (`codex app-server generate-ts --out <dir>`, with and without the experimental flag): every method needed here is in the stable schema — `thread/status/changed`, `turn/started`, `turn/completed`, `item/completed`, `thread/started`, `serverRequest/resolved`, `item/commandExecution/requestApproval`, `item/fileChange/requestApproval`, `item/tool/requestUserInput`, `thread/resume`, `thread/unsubscribe`, `thread/loaded/list`, `turn/interrupt`, `account/rateLimits/updated`.
- **One exception:** the question types (`ToolRequestUserInputParams`, `…Question`, `…Option`, `…Answer`, `…Response`) are each commented `EXPERIMENTAL` even in the stable schema. Questions may change shape; status and approvals are on firmer ground.

Practical reading: usable, but version-specific. Gate on the Codex version, ignore unknown fields, and fall back to screen activity when the handshake fails.

## Claude: why it stays on hooks

There is no local equivalent for Claude. **Remote Control** (`claude --remote-control`) lets claude.ai or the mobile app join a terminal session, but it is relayed through Anthropic's servers (the local session makes outbound HTTPS only) with no local socket or documented protocol Deckhand could connect to (https://code.claude.com/docs/en/remote-control). `claude -p` with JSON output and the Agent SDK are headless, so attaching would be lost.

Claude's hooks already cover most of what the app-server gives Codex, documented and stable (https://code.claude.com/docs/en/hooks): state and details (shipped in v43), failures (`StopFailure`, not yet registered), and answering (a `PreToolUse` hook returning `updatedInput` with `answers` for a question; `PermissionRequest` returning allow/deny). Two gaps remain: no Esc/interrupt event (the screen-idle fallback covers it), and answering through a hook means holding the request while Deckhand decides, with the TUI's dialog hidden meanwhile, whereas the app-server lets the TUI and Deckhand both see it and either answer.

## The experiment (run by hand; needs one or two cheap model turns)

1. `codex app-server --listen unix:///tmp/dh-test.sock` (keep the socket path under 104 bytes; a long path failed silently).
2. In another terminal: `codex --remote unix:///tmp/dh-test.sock`.
3. A small Node `ws` client on `ws+unix:///tmp/dh-test.sock:/`: send `initialize`, then `initialized`, then `thread/loaded/list`, then `thread/resume {threadId, excludeTurns: true}`; log every message.
4. In the TUI, ask for a command that needs approval; answer it from the script. Check the TUI's prompt closes and the turn continues.
5. Repeat for a question in plan mode (`request_user_input`), Ctrl-C (`turn/completed` interrupted), `codex resume <id> --remote …`, `codex fork`, and exiting the TUI (then `thread/unsubscribe`, or the server keeps the thread loaded for about 30 minutes).
6. Check whether the standalone server's connection to chatgpt.com (seen in `RUST_LOG=info`) can be turned off by config.

## How it would plug into Deckhand

- `src/agents.ts`, `agentSignals.integrationArgs`: for Codex, `--remote unix://<sock>` instead of `--no-daemon`, only when `--remote` appears in `codex --help`.
- `src/sessionWorker.ts`: start and own the app-server child next to the PTY (about 60 ms to `initialize`), remove the socket on exit.
- New `src/codexAppServer.ts`: the client and a `normalizeAppServer()` producing the existing `AgentSignal`: `turn/started` → working; `waitingOnApproval` / `waitingOnUserInput` → needs-input with the reason from the request; `turn/completed` → response-ended (last agent message), failed or limited; `serverRequest/resolved` → working; `thread/started` → the conversation ID.
- `src/daemon.ts`: feed those signals into the existing attention and notification pipeline; later an "answer request" call (roadmap item 7) and the MCP.
- Settings and docs: retire the "⚠ Codex hooks" setup.
- Optionally generate types per Codex version with `codex app-server generate-ts`.

## Risks

- Protocol churn: releases every few days (0.161.0, 0.162.0, 0.162.1 between Oct 7 and Oct 9). Stay off the `experimentalApi` surface, tolerate unknown fields, gate on version.
- Remote-mode TUI parity: some features reached remote mode late (`/import` only in 0.157).
- One extra process per Codex session; a socket file to clean up.
- The standalone server's outbound remote-control connection (see experiment step 6).

## Other options considered

- `codex exec --json` and the TypeScript SDK (`@openai/codex-sdk`, which spawns the CLI and exchanges JSONL): headless. Fine for an orchestrator, but no native TUI and no interactive approvals.
- There is no `mcp-server` subcommand in 0.160.1.

## Sources

- Codex app-server docs: https://learn.chatgpt.com/docs/app-server (redirected from developers.openai.com/codex/app-server)
- openai/codex: `codex-rs/app-server-daemon/README.md`, `codex-rs/app-server/src/outgoing_message.rs`, `codex-rs/app-server/src/request_processors/thread_lifecycle.rs`, `codex-rs/app-server/tests/suite/v2/thread_resume.rs`, `codex-rs/tui/src/daemon_startup.rs`, `codex-rs/tui/src/app/app_server_events.rs`, `codex-rs/tui/src/app/pending_interactive_replay.rs`, `sdk/typescript/README.md`; release notes rust-v0.156.0, 0.157.0, 0.160.0, 0.162.0
- Local, read-only: `codex --help`, `codex app-server --help` (and its subcommands), `codex agents --help`, `codex resume --help`, `codex fork --help`; `generate-ts` with and without experimental; a probe of the shared daemon (`initialize`, `thread/loaded/list`, `thread/list`). No model calls; nothing under `~/.codex` modified.
- Claude: https://code.claude.com/docs/en/remote-control, https://code.claude.com/docs/en/hooks
- Inferred, not seen live: the TUI's overlay closing when another client answers; `--remote` with resume and fork; the effect of the remote-control connection.
