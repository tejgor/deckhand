# Ideas from Orca

**Status:** reviewed 2026-10-20 from Orca's docs and parts of its source (https://github.com/stablyai/orca, an Electron app that runs Claude Code, Codex, Pi and many other agent CLIs in git worktrees). Nothing is built. Where an idea joins an existing plan, it is listed there in the README's *Suggested order*; this doc keeps the mechanics and the reasons.

## Already in Deckhand

Orca has these too, so they need nothing new:
- A daemon that keeps agents running when the UI quits (Orca's session restore).
- Relaunching an exited agent in the same place (Orca's restart chip).
- Resuming by conversation ID.
- Waiting for setup to finish before the agent starts (Orca's `wait-for-setup`).
- Spotting PRs merged on GitHub (`i` → **P**).
- Symlinked dependency directories (`worktree.symlink`; Orca's `sharedDirectories` and `.worktreeinclude`).
- Trust review for repository commands.

Two other Orca details need nothing either:
- **Hook endpoint:** Orca re-reads its endpoint file on every call. Deckhand's hook bridge already reaches the daemon through a fixed socket path.
- **Claude permission hooks:** Orca prints `{}` first because these fail closed on empty output. `deckhand hook` already always prints `{}`.

## Worth adopting

### Unread and a Needs you / Working / Done view (joins roadmap 3 and 6)
- Orca's Agent Dashboard has the columns **Needs You / Working / Done / Idle**. Idle means no report for about 30 minutes, and it is hidden by default.
- A finished turn makes a worktree unread (shown bold) until you look at it. Scripts can set or clear unread themselves (`orca worktree set --unread`).
- For Deckhand:
  - The waiting queue (roadmap 3) is that dashboard's Needs You column.
  - Roadmap 6's "reviewed" flag is the unread state: set when a turn ends, cleared when you open the session, settable from the CLI.
  - Show unread rows bold in the sidebar.

### CLI shape (joins the scriptable CLI, `deckhand-mcp-cli.md`)
- Orca's CLI uses `--json` on every command, plus selectors (`active`, `id:`, `path:`, `branch:`, `name:`).
- `orca terminal read --screen` returns the rendered screen; `read --cursor <n>` returns the output since a cursor, escapes stripped.
- `orca terminal send --text … --enter` types text into a terminal.
- **`orca terminal wait --for tui-idle --timeout-ms`** blocks until the agent stops; it is the primitive scripts are built on.
- Copy for Deckhand's `deckhand wait`: the selectors, `read --screen`, and `--json` throughout.

### Agent checkpoints (joins the CLI)
- Agents report progress with `orca worktree set --comment "…" --workspace-status todo|in-progress|in-review|completed`, shown in the UI.
- The guidance: the first line says what the agent is doing; read the current value (`worktree current`) before writing so the user's text is not overwritten.
- For Deckhand: `deckhand status --set "…"` from inside a session. It is authenticated by the session's hook token (as the MCP doc plans) and shown on the details line next to the live activity (roadmap 4).

### Skill served by the binary (joins the CLI)
- Orca publishes tiny `SKILL.md` stubs. Each one only tells the agent to run `orca skills get <topic>`, so the full guide always matches the installed version.
- For Deckhand: `deckhand skill` prints the CLI guide, plus a one-paragraph stub users add to `~/.claude/skills`. Deckhand would print the stub and never install it, as with `deckhand hooks`.

### Diff notes sent to an agent (new)
- In Orca's diff view, `c` on a line adds a comment that stays anchored as the code changes.
- **Send to agent** turns every unresolved comment into one prompt (file, line, the code, the comment) and sends it to an agent in that worktree, or starts a new one.
- Resolved comments collapse; unresolved ones go out with the next batch.
- For Deckhand:
  - Comments on lines in the Changes view's diff preview, kept in the session's note or a sidecar file.
  - One key sends the batch: typed as a draft (as tasks are today), or sent if the session is idle.

### Hibernation (new)
Orca stops an idle agent's terminal process and relaunches it with its resume flag when you come back. It only does this when all of these hold:
- the agent finished its turn and was not waiting on you;
- the session is not the one on screen;
- nothing was typed since it finished;
- the agent can be resumed;
- it has been idle for the configured time (30 minutes by default);
- no subagent is still running.

For Deckhand:
- The daemon has the same facts: `attention`, the PTY's last input and output, and `agent_session_ref`.
- **s** already resumes a conversation. Hibernation would do that automatically on attach, and say "hibernated" rather than exited.
- It saves memory and CPU when many sessions are open.
- Off by default, set globally.
- Codex only qualifies once its conversation ID is known.

### Fan out and pick a winner (new)
- In Orca this is only a recipe: the same prompt to several agents, each in its own worktree from the same ref; compare the diffs, ship one, delete the rest.
- Deckhand already has every piece: create with a prompt, Changes, merge, worktree deletion with safety checks.
- What it would add:
  - One form: a prompt, N agents, base branch.
  - A grouped comparison: per worktree the state, files changed and +/− lines, and the turn summary (roadmap 6).
  - "Merge this one, delete the others". The other worktrees still go through the normal safety checks.
- The CLI and MCP get the same thing as `deckhand new --fan-out`.

### Usage and rate limits (new; Codex half depends on `codex-app-server.md`)
- **Codex:** Orca asks the app-server (`account/rateLimits/read`), falling back to an internal chatgpt.com endpoint. With the app-server integration Deckhand gets this almost for free, and `account/rateLimits/updated` pushes changes.
- **Claude:** Orca calls `https://api.anthropic.com/api/oauth/usage` with the OAuth token it reads from the macOS Keychain. That endpoint is undocumented, and reading Claude's credentials is something Deckhand should not do. Leave Claude out until there is a supported source.
- **Tokens and cost:** these come from scanning transcripts (`~/.claude/projects`, `~/.codex/sessions`) with a local price table. Possible later; Deckhand already reads Claude transcripts (`crates/sys/src/claude_transcripts.rs`).
- **UI:** the status line shows the tightest window, with a warning from 80%.

### Phone: push notifications and a companion page (new)
Orca's mobile app shows each agent's status and scrollback, sends quick replies (`continue`, `yes`) and pushes notifications. It pairs with the desktop through `orca serve --mobile-pairing`, which issues a revocable token per phone. For Deckhand, the useful core is the same three things, and the daemon already has the data:
- **See what waits on you:** the waiting queue (roadmap 3), with each session's state, reason line and how long it has waited.
- **Answer:** a question's options are already in `attention.reason` and become big buttons. A permission prompt gets Yes / No. A text box sends a free-form reply.
- **Glance at progress:** the last lines of the session's screen, plus the turn summary (roadmap 6) once it exists.

Why not the terminal UI over SSH (Blink or Termius with mosh over Tailscale):
- It works as an emergency fallback today, but not for daily use.
- A phone in portrait gives about 45 columns, and the keyboard covers half of what is left. Esc and Ctrl sit behind a key bar, and touch scrolling in a full-screen terminal app is unreliable.
- Attaching shows the agent's own TUI, which Deckhand cannot lay out for a phone.
- An attached session has one terminal size, so attaching from the phone would also reflow the session on the desktop.
- A narrow Deckhand layout would not fix any of this, so it is not planned.

The plan, in three parts:
1. **Push notifications (small, early).** An optional push target in user config (global only, like `notifications`): an ntfy topic URL or a webhook the daemon POSTs title and text to, next to the desktop notification.
   - It is useful before anything else exists.
   - The text includes the reason line (the question, the command an agent wants to run), so it leaves your machine. Keep it opt-in and say so in Settings.
   - The topic name is the only secret on the public ntfy.sh server, so self-hosting ntfy is safer.
   - Once the companion page exists, the notification links straight to that session.
2. **Answering without attaching (roadmap 7, moved up).** This is the companion page's core. Start with Claude questions and permission prompts: send the option's keystrokes only when the expected dialog is on screen, and refuse otherwise. Codex through its app-server comes later.
3. **A companion page served by the daemon (`deckhand serve`).** A small, phone-first web page you can add to the home screen, showing the queue, a session's last lines and its pending question with answer buttons. Its cost is security, because a device that can press Yes can approve any command an agent asks to run:
   - reachable over Tailscale (or another private network) only, never a public endpoint;
   - a revocable token per paired device;
   - starting with viewing and answering only.

   It is a remote client over the same building blocks as the scriptable CLI (read a screen, send text), so it comes after that CLI and after part 2.

A native iOS or Android app is not planned. It would mean a separate codebase, store releases and a push relay server, and the page covers the same needs.

### Session history browser (later)
- Orca lists every conversation in each agent's own store, not just the ones it started. You can search by title, folder, branch and first prompt, and resume one (`claude --resume`, `codex resume`, `pi --session`).
- For Deckhand: "adopt an earlier conversation" would create a Deckhand session that resumes it in its folder.

### GitHub checks per worktree (later)
- Orca shows the linked PR's state and failed Actions per worktree. **Fix broken checks** sends the failing check names and links to an agent.
- For Deckhand: a `gh pr checks` summary on the details line, and a key that drafts the failures into the session, like diff notes.

### A removal hook (later)
- Orca's `scripts.archive` runs before a worktree is removed, and a failure blocks the removal.
- For Deckhand: an optional `worktree.removeCommand` in `deckhand.json` (for example stopping containers), trust-reviewed like `setupCommand`, run before the existing safety checks delete the worktree.

### Scheduled runs (later)
- `orca automations create --trigger daily|<cron> --prompt … --precheck "<shell>"` runs a prompt on a schedule. When the precheck exits non-zero, the run is skipped without starting an agent.
- For Deckhand the daemon could run these. But agents running unattended spend money and need permission defaults that Deckhand deliberately leaves to the agent, so this waits until the MCP orchestration exists.

### Accounts per launch (later)
- Orca keeps each Codex account in its own `CODEX_HOME`, and each Claude account in its own `CLAUDE_CONFIG_DIR`. Switching only affects new launches; running sessions keep theirs.
- For Deckhand: an optional account per agent in user config, passed as that environment variable at launch.

## Not adopting

- **Permissions bypassed by default:** Orca launches agents with `--dangerously-skip-permissions` and the like. Deckhand's rule is that native approvals stay in the agent's terminal.
- **Managed hooks written into agent configs:** Orca writes `~/.codex/hooks.json` and `config.toml`. Deckhand never writes to `~/.codex`; it prints the config for you to merge.
- **GUI-only features:** embedded browser and Design Mode, the code editor, Quick Open, image and PDF viewers, computer use, the remote runtime, SSH worktrees with port forwarding.
- **A native mobile app:** the companion page above covers the same needs (see *Phone*).
- **Line attribution** (which lines the agent wrote): Orca's docs describe it, but no implementation was found.
- **Linear and Jira:** possible later through the same "draft into a session" pattern, but nothing asks for it yet.
