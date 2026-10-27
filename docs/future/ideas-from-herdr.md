# Ideas from herdr

**Status:** reviewed 2026-10-27 from herdr v0.9.5's docs and source (https://github.com/herdrdev/herdr, a Rust terminal multiplexer for coding agents) and from some of its plugins, mainly herdr-remote (https://github.com/dcolinmorgan/herdr-remote), herdr-projects and herdr-reviewr. Nothing is built. Where an idea joins an existing plan, it is listed there in the README's *Suggested order*; this doc keeps the mechanics and the reasons.

herdr-remote is AGPL-3.0 and Deckhand is MIT: take the ideas below, not the code.

## Already in Deckhand

herdr has these too, so they need nothing new:
- A server that keeps agents running when the client quits, and resuming by conversation ID.
- A *needs you* state for questions and permission prompts, with a key that jumps to the next one.
- Finished but not yet seen (herdr's `done`, Deckhand's `◆`).
- Worktrees grouped under their repository, with safe removal.
- A guide agents can read about the tool (herdr's `herdr --skill`; Deckhand plans `deckhand skill`, README 2b).

Deckhand is ahead in one place: herdr still decides Claude's and Codex's state from the screen, with a rule file per agent, and keeps fixing edge cases. Deckhand gets it from hooks.

## Answering without attaching: how herdr-remote does it (joins roadmap 7)

herdr itself does not answer questions. It marks the pane blocked, and its CLI offers `pane read` and `pane send-keys`. A pull request that added guarded answering for Claude (#5099) was closed by herdr's contributor rules without review. The answering people use comes from the herdr-remote plugin, a relay with web, menu bar and Telegram clients. It reads the screen and types keys, which is Deckhand's planned approach. Its details are the lessons (`relay/herdr_relay.py`, roughly lines 1900–2830, and the `respond` handler near 3719):

- **Read only the visible screen.** herdr-remote reads `pane read --source visible`. herdr's longer reads scroll the agent's TUI to collect history: 6–12 seconds for 200–400 lines, only while the agent is idle, and the user watches their terminal scroll. Deckhand's headless grid is the visible screen already.
- **Fingerprint the question, not the screen.** The card a client shows carries a `prompt_id`: a hash of the question text and option labels only. A hash of the whole screen changed every second, because Claude's elapsed timer and token counter redraw while the dialog is up, so every answer was refused as stale. The client sends the id back with its answer; the relay reads the screen again and refuses with "prompt changed" if it no longer matches.
- **A numbered menu takes the digit key.** Claude draws approvals and questions as `1.`…`N.` menus. Typing the option's text, or a digit and Enter, does not select it: Enter takes whichever row is highlighted, usually *Yes*.
- **A custom answer is a short sequence.** Move to *Other* / *Type something* with arrow keys, press Enter, wait up to about 1.5 s for the text field (`Enter your response:`) to appear, then send the text and Enter. Refuse if the field never appears.
- **Multi-select is its own path.** A checkbox menu (`AskUserQuestion` with `multiSelect`) is answered by toggling each option, then submitting. Hash its labels without the check marks, or the second toggle is refused as a changed question.
- **Free text only when a question was detected.** Otherwise the relay refuses rather than typing into whatever has focus. Shell panes, where text is a command, are a separate opt-in.
- **Match text with whitespace normalised.** Claude's dialog footer (`Enter to select · ↑/↓ to navigate · Esc to cancel`) is 49 cells and wraps on narrow terminals; herdr's rule then misses it and reports idle (blocked at 103 columns, idle at 46). herdr-remote checks idle panes itself for that reason.
- **Log every answer** with the device it came from.

From the closed herdr pull request: deliver **at most once**. When it is unclear whether the keys arrived, never resend automatically; a second Enter can answer the next dialog.

For Deckhand:
- The question and its options already arrive by hook in `attention.reason`, so the screen check only has to confirm that dialog is the one on screen, not find it.
- Answering from the Preview pane or the queue, and later the phone page, all use one daemon request carrying the session, the question's fingerprint and the choice. The daemon reads the grid, refuses on a mismatch and sends the keys.
- A multi-question `AskUserQuestion` has a tab per question and a *review your answers* screen, which herdr still gets wrong (open issue #4668). Refuse those at first; the reason line keeps only the first question's options anyway.
- Keys sent through the daemon's input path already clear needs-input (`answers_prompt`, then `mark_answered`), but that only says keys went out. The agent's next hook (the question tool's `PostToolUse`, or any later tool use) says it accepted the answer; a client shows "answered" from that, and "sent" until then.

## Worth adopting

### Agents report waiting themselves (joins README 2b)
- herdr's `pane report-agent --source X --agent X --state working|blocked|idle --seq N --message "…" -- <resume command>` lets any agent report its state. A report with an older `seq` than the last one is ignored, so reports arriving out of order can't undo a newer state. Several agents report this way natively.
- herdr-projects makes it a rule: an agent that needs you must run `herdr-projects report <id> --state blocked --kind question|permission|auth --msg "<why>"` before it stops. A question asked in plain words with no report looks exactly like a finished turn, and is reviewed as finished.
- For Deckhand:
  - Extend the planned `deckhand status --set "…"` with `--state waiting --kind question|permission|auth`, authenticated by the hook token like the checkpoints, ordered like hook signals (`sentAt`; older ones are dropped).
  - It gives Pi a waiting state without a Pi extension, and gives orchestrated sessions (MCP) a reliable "waiting, not finished".

### "Why this state?" (joins hook health, README 1b)
- `herdr agent explain [--file screen.txt]` prints which detection rule matched and why.
- For Deckhand: the details line, or `deckhand doctor <session>`, shows the last signal, where it came from (hook or screen), its age and what set the state. With hook health it tells "Codex hooks not set up" apart from "the screen settled".

### Notifications (joins phone push, README 1c)
herdr offers, and Deckhand lacks:
- **Delivery through the outer terminal**: an OSC 9 / 777 notification escape that the terminal shows. It works over SSH and needs no `Deckhand.app` or `notify-send`.
- **Click to jump**: clicking a notification brings the terminal forward on that session. A key does the same for the last notification.
- **Sounds**: one for *done*, another for *needs you*, overridable per agent.
- **Quiet for what you're looking at**: no notification for the session on screen, and an optional delay so a quick question answered at the desk never pings the phone.

### Order the sidebar by attention (next to the queue, README 1)
- herdr's `agent_panel_sort = "priority"` orders agents: needs you, then finished and unseen, then working, then idle and seen.
- For Deckhand: an optional sidebar order using the queue's `urgency()`, for people who live in the sidebar rather than the queue.

### A live screen stream (new, before the phone page)
- `herdr terminal session observe` streams a pane's screen as JSON frames (base64 ANSI); `terminal session control` accepts input, resize and scrolling, from one controller at a time, with `--takeover`. herdr's third-party web, phone and macOS clients are all built on these two.
- For Deckhand: a view-only stream of a session's rendered screen, and separately an input channel that one client holds at a time. The phone page needs the first; answering stays a narrower request (above) rather than raw input.
- A remote viewer must never resize the session. herdr-remote's footer bug exists because a phone attaching shrinks every pane to the handset's width.

### Phone page details (joins README 6)
herdr-remote ships the page Deckhand plans:
- A notification opens straight to that session's last output and reply box (*Open output & reply*), and blocked notifications carry the answer buttons.
- The WebSocket checks its `Origin` as well as the token.
- It installs as a home-screen app with web push (a service worker), so no native app.

For Deckhand, also: make viewing and answering separate permissions of a paired device, so a phone can be paired to watch only.

### Diff notes: a "last turn" scope (joins README 7)
- herdr-reviewr reviews uncommitted changes, the branch, single commits, and **the last turn**: only what changed since the prompt was sent. Line comments go back to the agent as one prompt.
- For Deckhand: snapshot the worktree when the user sends a prompt (`UserPromptSubmit`; `git stash create` records it without touching the tree) and diff against it.

### Scriptable CLI guards (joins README 2)
herdr's `agent prompt <name> "text" --wait --until idle|done|blocked`:
- refuses with `agent_blocked`, sending nothing, when the agent waits at a question or approval; otherwise a scripted prompt is typed into the dialog;
- fails with `agent_prompt_stalled` when no working state appears within 5 seconds;
- submits the text and Enter as one ordered step, with bracketed paste.

For Deckhand: `deckhand send` refuses on needs-input the same way, `wait` takes `--until idle|ended|waiting`, and `read --screen` stays the visible screen (history comes from transcripts, as `deckhand-mcp-cli.md` says).

## Not adopting

- **Detecting state from the screen with a rule file per agent**, with rule updates downloaded from herdr's site: Deckhand's hooks are more reliable for the agents it supports. The screen stays the fallback.
- **Several machines in one window** (`herdr machine add`, SSH remotes): ruled out with Orca's remote runtime (`ideas-from-orca.md`).
- **Plugins and token-built sidebar rows**: herdr's plugin system (any program, unsandboxed, built on its CLI) only pays off with third-party authors. Deckhand's CLI gives scripts the same reach.
- **Permissions bypassed by default**, as herdr-projects launches its agents: Deckhand's rule is that approvals stay with the agent.
