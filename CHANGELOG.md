# Changelog

Notable changes to Deckhand, newest first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Changes before 3.3.0 are in the git log and the [GitHub releases](https://github.com/tejgor/deckhand/releases).

## [Unreleased]

### Added

- **Choose where a merge goes**: `t` in the merge confirmation lists the repository's other checkouts that are on a branch (the main checkout and every other worktree) and merges into the one you pick, instead of always the launch checkout. It replaces the stacked-only `t` that switched between two targets. See [Merging and done](docs/features.md#merging-and-done).

### Fixed

- **`m` on a sub-session in its parent's worktree**: it said *Not in a worktree, so there is nothing to merge*, although `m` on the parent worked. It now merges the worktree the sub-session works in. Needs a daemon restart (`deckhand restart`).

## [3.6.0] - 2026-11-18

### Added

- **Copy from the Preview while typing to the agent**: drag across the agent's screen to select it (shown reversed, kept inside the Preview) and let go to copy it to the clipboard; the status line says what was copied. Terminals can't select while Deckhand reports the mouse, so before you had to attach. See [Typing to an agent in the Preview](docs/features.md#typing-to-an-agent-in-the-preview).

## [3.5.0] - 2026-11-18

### Changed

- **Plan sessions report back**: each session an orchestrator's plan starts is told to message its parent with an update once it has finished: a sub-session its parent session, a top-level entry the orchestrator, which tells you in a line. See [The orchestrator](docs/features.md#the-orchestrator).

## [3.4.5] - 2026-11-15

### Changed

- **Plans no longer hold you or the orchestrator up**: `deckhand plan apply` returns at once and the orchestrator ends its turn; your answer reaches it as a message (`[The user approved your plan …]` with what was created, or `[The user cancelled your plan …]` with your note). In **Create this plan?**, `esc` now puts the plan aside to look at other sessions first (the header says `◎ 1 plan waiting · P`, and `P` opens it again), and `x` twice cancels it, optionally with a note typed between. Plans no longer expire: one waits until you answer it or its orchestrator exits, and through a daemon restart. Needs a daemon restart (`deckhand restart`). See [The orchestrator](docs/features.md#the-orchestrator).
- **`O` opens a dialog**: **New orchestrator** asks for the name and *What do you want to do?* in a pop-up over the session pane instead of the footer, so the idea can be a paragraph: `enter` starts a new line in it, a paste keeps its lines, `ctrl+s` starts the orchestrator, and `esc` keeps what you typed for the next `O`. See [The orchestrator](docs/features.md#the-orchestrator).
- **Orchestrators are listed by their state**: sections by state (`V`) no longer pin orchestrators in an **Orchestrator** section at the top. Each is listed like any session, so one asking you something is under **Needs you**, and idle ones no longer pile up at the top. See [Organizing and inspecting sessions](docs/features.md#organizing-and-inspecting-sessions).

### Upgrading

- **Daemon restart required**: the daemon protocol changed (64 to 65). `deckhand update` offers the restart; or run `deckhand restart`. Plans waiting for your answer are kept across it.

## [3.4.0] - 2026-11-15

### Added

- **`deckhand update`** updates Deckhand the way you installed it (Homebrew, the installer script or `cargo install --git`), asking first, then offers to restart the daemon when the running one is an older build. Other installs get the steps to follow. See [Updating Deckhand](docs/features.md#updating-deckhand).
- **New release check**: once a day the daemon asks GitHub for the latest release; a newer one shows in the header (`deckhand 3.3.0 → 3.4.0 · G`), on the Agents screen (`G`) and in `deckhand doctor`. Settings (`C`) › Deckhand › Update check turns it off.

### Changed

- **`deckhand ls` in a session lists its whole tree**: its top-level session and every session under it, in any checkout, indented under their parents, the sessions it can message. `--all`, or running it by hand, lists the checkout's sessions as before. See [Deckhand commands for agents](docs/features.md#deckhand-commands-for-agents).
- **One rule for long text**: `deckhand new --prompt -`, `task add --details -`, `note add -` and `do -` now read the text from stdin, as `send -` and `reply -` already did, so a quoted heredoc works for every long text. The guide agents read shows the exact form. See [Deckhand commands for agents](docs/features.md#deckhand-commands-for-agents).
- **Short session names**: the guides tell agents and orchestrators to name every session (and parked task) in 2 to 4 words naming the work, not a sentence, and to always pass `--title` to `deckhand new`. Without one, a session is named after its prompt's first sentence, not its whole first line. See [Starting a session from a command](docs/features.md#starting-a-session-from-a-command).
- **Codex replies through its tool**: a message that wants an answer tells a Codex that has the `deckhand` tool to reply with the tool rather than a shell command its sandbox would block.
- **`O` asks for a name, then the idea**: two fields, `tab` between them. The name (2–4 words, optional) names the orchestrator and its idea's line instead of the idea's first line; `deckhand do --title <name>` does the same. See [The orchestrator](docs/features.md#the-orchestrator).
- **One update hint**: the header shows Deckhand's and the agents' updates in one `· G` hint (`deckhand → 3.4.0 + codex · G`).
- **Older daemon**: an older daemon still running after an update now shows in the header (`daemon 3.3.0 · deckhand restart`) for as long as it runs, instead of once in the footer. The Agents screen says it in full.

### Fixed

- **`deckhand new --prompt -` sent the literal `-`** as the new session's prompt. It now reads the prompt from stdin, and a `-` with nothing piped in is refused before anything starts.
- **Mistakes refused, not ignored**: a blank `--prompt`, `--title` or `--details` (once silently dropped), and `-` as a title or step (never read from stdin), are refused with what to do instead. Through Codex's `deckhand` tool, which has no stdin, a `-` is refused with the call to make, the text as one word.

### Upgrading

- **Daemon restart required**: the daemon protocol changed (63 to 64). Run `deckhand restart` after updating; running sessions are interrupted and Deckhand offers to resume them. From now on, `deckhand update` does the update and offers the restart for you.

## [3.3.0] - 2026-11-15

### Added

- **Orchestrators**: `O` (or `deckhand do "<idea>"`) starts a session whose job is to split one idea into sessions. There is one per idea, and several can run at once. See [The orchestrator](docs/features.md#the-orchestrator).
- **Plans you confirm**: an orchestrator proposes a tree of sessions in its chat; Deckhand creates it only after you confirm it (**Create this plan?**). Each new session starts on its brief, and the idea gets a line in Tasks with the plan's lines under it.
- **Stacked sub-sessions**: a sub-session in a new worktree, under a parent with a worktree of its own, branches from the parent's branch and merges back into it. See [Stacked sub-sessions](docs/features.md#stacked-sub-sessions).
- **Merging a stacked worktree** goes one hop, into its parent's branch. `t` in the merge dialog switches the target to the launch checkout. See [Merging and done](docs/features.md#merging-and-done).
- **Moving up**: when a worktree is merged or deleted, the worktrees stacked on it now target the next one up.
- **`--flat`** on `deckhand new` (or another base in the create form) keeps a sub-session's worktree unstacked.
- **Sidebar `N/M in`**: a parent's row counts how many of the worktrees stacked on it are merged into it.
- **Orchestrator section**: sections by state (`V`) list every running orchestrator at the top. Its sessions are listed by their own state, under a line naming the idea. See [Organizing and inspecting sessions](docs/features.md#organizing-and-inspecting-sessions).
- **Messages between sessions**: `deckhand send <id> "<message>"` types a message into another session's agent once it is idle. `--wait` waits for the first reply and prints it; `deckhand reply` answers. See [Messages between sessions](docs/features.md#messages-between-sessions).
- **Held messages**: a message waits while the receiving agent is working, asking a question, or while you type to it. The session shows `✉N`; `e` lists its messages, and `x` twice cancels a held one ([keys](docs/keys.md#messages)).
- **Who may message whom**: a session can send to any session under the same top-level session as itself. Run by hand, `deckhand send` reaches any session. See [The caller](docs/features.md#the-caller).
- **Steps from agents**: `deckhand step add|done|reopen|list` manages the calling session's steps. See [Deckhand commands for agents](docs/features.md#deckhand-commands-for-agents).
- **`deckhand new --parent <id>`** starts a sub-session under a session in the caller's tree. See [Starting a session from a command](docs/features.md#starting-a-session-from-a-command).
- **`deckhand plan check|apply`** checks a plan of sessions, or creates it once you confirm it in Deckhand.
- **Briefs**: a line can keep a brief, what it was set up to do. An orchestrator's plan writes one for each session it starts and one for the idea. See [Notes](docs/features.md#notes).

### Changed

- **Tasks are one nested list** per repository. A session takes on at most one line (its task), and the lines under it are its steps. The backlog is the top-level lines nobody is on. See [Tasks](docs/features.md#tasks).
- **A line ticks itself** once every line under it is ticked, and reopens if one of them reopens.
- **Lines on demand**: a session started without a line gets one only when it needs it (a step, a line note or a brief).
- **Notes have separate jobs**: a line note for the work, shared by whoever works on the line; a session note for your own reminders; and one scratchpad per repository. See [Notes](docs/features.md#notes).
- **Worktree notes are gone**: their checkboxes became steps and their prose became line notes. The `↗` links, and sending note items to the backlog, are gone too: `ctrl+p` on a checkbox now makes it a step.
- **The Notes tab** (`a`) shows the session's line and steps, the brief, the line note and the session note. The Tasks board (`b`) shows the list as a tree, and its Notes view (`tab`) lists every note.
- **Merging** ticks the lines of the worktree's sessions. Their open steps move up to the line above (or the backlog) unless you keep them.
- **`deckhand note add`** now adds a paragraph to the calling session's line note, not a checkbox to its worktree note. Use `deckhand step add` for steps.
- **Sidebar nesting**: sub-sessions are indented by their real depth, each level under its parent's status glyph. See [Sessions and sub-sessions](docs/features.md#sessions-and-sub-sessions).
- **Relaunching is not activity**: after `s`, a new agent version or a daemon restart, an unread reply stays unread and sections by state keep their order. A session that was waiting on you is listed under **Unread**. See [Relaunching is not activity](docs/features.md#relaunching-is-not-activity).

### Upgrading

- **Daemon restart required**: the daemon protocol changed (57 to 63). Run `deckhand restart` after updating. Running sessions are interrupted; Deckhand offers to resume them.
- **One-time migration**: the first daemon start moves old tasks and worktree notes into the new list. It copies everything first to `~/.deckhand/notes/trash/work-items-migration-<time>/`, with a `migration-report.txt` saying what moved where. If it fails, Deckhand keeps running and tries again at the next start. See [Upgrading from tasks and worktree notes](docs/features.md#upgrading-from-tasks-and-worktree-notes).
- **Update together**: every Deckhand that shares a state directory (a stable and a dev build pointed at the same one, say) must be updated at the same time. An older one would show the new list wrongly.

[Unreleased]: https://github.com/tejgor/deckhand/compare/v3.6.0...HEAD
[3.6.0]: https://github.com/tejgor/deckhand/compare/v3.5.0...v3.6.0
[3.5.0]: https://github.com/tejgor/deckhand/compare/v3.4.5...v3.5.0
[3.4.5]: https://github.com/tejgor/deckhand/compare/v3.4.0...v3.4.5
[3.4.0]: https://github.com/tejgor/deckhand/compare/v3.3.0...v3.4.0
[3.3.0]: https://github.com/tejgor/deckhand/compare/v3.2.0...v3.3.0
