# ⚓ Deckhand

[![npm version](https://img.shields.io/npm/v/@tejgor/deckhand.svg)](https://www.npmjs.com/package/@tejgor/deckhand)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Node.js Version](https://img.shields.io/node/v/@tejgor/deckhand.svg)](https://nodejs.org)

**A lightweight agent workbench for your IDE terminal.**

> **Status:** 🧪 Early/Experimental. Behavior, on-disk state, and the daemon IPC protocol may change between versions.

https://github.com/user-attachments/assets/89ceed64-18c0-4006-bd9e-7500204ca02a

---

## 📖 Table of Contents

- [Why Deckhand?](#-why-deckhand)
- [Features](#-features)
- [Requirements](#-requirements)
- [Installation](#-installation)
- [Quick Start](#-quick-start)
- [Controls](#-controls)
- [Sessions & Workspaces](#-sessions--workspaces)
- [Configuration](#-configuration)
- [Worktree Hooks](#-worktree-hooks)
- [Architecture](#-architecture)
- [Development](#-development)
- [Troubleshooting](#-troubleshooting)

---

## 🤔 Why Deckhand?

Tools for running coding agents in parallel typically rely on [`tmux`](https://github.com/tmux/tmux) to host isolated agent sessions, paired with git worktrees for branch isolation. That works well in a dedicated terminal, but `tmux` fits awkwardly inside an IDE's integrated terminal, where prefix-key collisions, nested key handling, and resize quirks get in the way.

**Deckhand** targets the integrated terminal and drops the `tmux` dependency. It runs as a single program built on an [Ink](https://github.com/vadimdemedes/ink) UI, a local daemon, and [`node-pty`](https://github.com/microsoft/node-pty) workers, behaving consistently in whatever terminal it is launched from.

> *For `tmux`-based alternatives, see [claude-squad](https://github.com/smtg-ai/claude-squad) and [agent-deck](https://github.com/asheshgoplani/agent-deck).*

---

## ✨ Features

- **Split View** — A numbered session sidebar beside Preview, Terminal, Git, Dev, and Notes tabs; it marks the sessions sharing the selected one's worktree and shows the selected session's full title, state, age and branch below the list.
- **Live Previews** — Watch a session's output without attaching to it, with read-only preview focus/scrolling.
- **Persistent Sessions** — The daemon owns sessions, so they survive UI quits and UI crashes. Daemon crashes preserve conversation references, not live processes.
- **Keyboard Reordering** — Move sessions up and down among their siblings from the keyboard.
- **Sub-sessions** — Group related work under a parent session, indented in the sidebar; each one starts clean in the parent's directory, or forks the parent's Claude, Pi or Codex conversation.
- **Resumable Agents** — Claude/Pi retain native identities; Codex resumes when its native ID is captured. Unknown IDs never silently become a blank conversation. Fresh restart remains explicit.
- **Tasks** — One task list per repository (`b`), beside the notes: in progress (with the live state of the session doing it), backlog and done. `n` starts a session for a task in a new worktree (you pick the base branch); merging that worktree ticks the task off, and work dropped unmerged goes back to the backlog.
- **Notes with checklists** — Each session's notes plus one note per worktree shared by every session in it, as Markdown files you can also edit in VS Code; `- [ ]` items render as ☐/☑ and their open count shows in the sidebar.
- **Safer Cleanup** — Check uncommitted, untracked and valuable ignored files, and commits that deleting a branch would lose, before deletion; force kill and data-loss authorization are separate.
- **Merge Helpers** — Merge or squash-merge a session's worktree into the current branch, staged for review rather than committed: a preview shows the target, commits and diff stat first, uncommitted work can be committed first, conflicts are kept to resolve or aborted in one key, and worktrees merged elsewhere (into the default branch, or a merged PR) are marked by themselves.
- **Git Changes** — The Git tab lists the worktree's changes like VS Code's Source Control panel (merge conflicts, staged, unstaged, untracked, with line counts), previews each file's diff, stages/unstages files and opens them in your editor at the first change; `lazygit` (optional) is one key away for everything else.
- **Optional Tabs** — A configurable Dev tab for a command such as `npm run dev`.
- **Shared Worktree Panes** — Terminal, Git and Dev belong to the worktree, not the session: sessions in one worktree share one shell, one Changes view and lazygit, and one Dev command, which stay available after their agents exit.
- **Trusted Project Actions** — Global defaults plus an optional per-repository `deckhand.json` (defaults, setup, Dev command, named actions, creation hook), reviewed inline before any repository command runs.
- **Organization & Visibility** — Persistent archive/search/filter/tree preferences, local Git summaries, and explicit optional PR/check lookups.
- **Handoffs & Attention** — Inspectable Markdown context for clean children and capability-gated lifecycle signals/desktop notifications.

See [the feature guide](docs/no-brainers.md) and [isolated dev testing](docs/dev-build.md).

---

## 📋 Requirements

- **Node.js**: `>= 20`
- **OS**: macOS or Linux with a POSIX shell
- **Git**: `git` on `PATH`
- **Agents**: `claude`, `pi`, and/or `codex` on `PATH` — whichever agents you plan to run
- **Optional**: [`lazygit`](https://github.com/jesseduffield/lazygit) on `PATH`, attached from the Git tab with `o`

---

## 🚀 Installation

Deckhand requires Node.js 20 or newer. If you do not have Node installed, install the latest recommended version from the [Node.js website](https://nodejs.org/en/download/).

Install Deckhand globally from npm:

```bash
npm install -g @tejgor/deckhand
deckhand
```

### First-Time Setup

For an easier first-time setup, Deckhand can check for missing agents and offer to install them:

```bash
deckhand setup
```

The setup helper installs:
- **Claude Code**: `curl -fsSL https://claude.ai/install.sh | bash`
- **Pi**: `curl -fsSL https://pi.dev/install.sh | sh` (a managed install that pins its dependencies; an older npm install of Pi can be migrated by running the same command)
- **Codex**: `curl -fsSL https://chatgpt.com/codex/install.sh | sh` (OpenAI's standalone installer: a native binary in `~/.local/bin`, releases under `~/.codex/packages/standalone`). To move an existing npm install over, run `npm uninstall -g @openai/codex` first, otherwise the installer adds a PATH block to your shell profile so its copy wins

> Use `deckhand setup --check` for a read-only check, or `deckhand setup --yes` to accept the agent install prompts automatically.

### Keeping Agents Up to Date

Press `U` in Deckhand to see each agent's installed version beside its latest release (from npm), and press Enter to run that agent's own updater (`claude update`, `codex update`, `pi update --self`). Running sessions are never restarted: they keep the version they launched with until you restart them, and the sidebar marks them with a dim `↑`. When an update is available the header says so quietly (e.g. `codex update · U`). Installing a missing agent is still `deckhand setup`'s job.

### Optional: Lazygit

`lazygit` is optional and is not installed by `deckhand setup`. If you want the Git tab, install it separately:

```bash
# macOS with Homebrew
brew install lazygit

# Ubuntu/Debian
sudo apt-get update && sudo apt-get install -y lazygit
```

See the [lazygit installation docs](https://github.com/jesseduffield/lazygit#installation) for other platforms.

---

## 🏃 Quick Start

Run `deckhand` from inside a git repository, then create a session:

1. Press `n` for a top-level session.
2. Choose `claude`, `pi`, or `codex`.
3. Enter a session name.
4. Press `tab` to pick a workspace mode — no worktree, new worktree, or existing worktree.
5. Press `enter` to launch.

Press `o` to attach to the selected session's active pane. To branch off related work, select a session and press `N` for a sub-session.

---

## ⌨️ Controls

### Main View

| Key | Action |
| --- | --- |
| `n` | New top-level session (in a new worktree, `↑`/`↓` pick the branch it starts from) |
| `b` | Tasks: the repository's task list (see [Tasks](#tasks)) |
| `N` | New sub-session under the selected session |
| `1`–`9`, `0` | Jump to that numbered visible session (`0` selects visible session 10). With more than 10, type the number: `enter` or a short pause selects it, `esc` clears it |
| `j` / `k`, `↑` / `↓` | Move between visible sessions |
| `J` / `K` | Move the selected session down / up among its siblings (order is persisted) |
| `c` | Collapse or expand the selected session's sub-sessions in the sidebar |
| `tab` / `→`, `shift+tab` / `←` | Next / previous tab: Preview, Terminal, Git, Dev, Notes |
| `p` / `t` / `g` / `d` / `a` | Jump directly to Preview / Terminal / Git / Dev / Notes |
| `d` *(on Dev tab)* | Start or stop the Dev command of the selected session's worktree (shared by every session in it); the status line says which |
| `o` | Attach to the selected session (opens current tab; Terminal, Git and Dev also for exited sessions; on Git it opens lazygit); on Notes, edit them (see [Notes](#notes)) |
| `E` *(on Notes tab)* | Open the note (the session's, or the worktree's after you edited that) in Cursor / VS Code |
| `O` | Open the selected session directory/worktree in Cursor (if available) or VS Code |
| `v` *(on Preview tab, running session)* | Focus preview scrolling (`j`/`k` or `↑`/`↓`, PgUp/PgDn, the mouse wheel, `g`/`G` jump, `esc` exits focus) |
| `v` *(on Git tab)* | Focus the Changes list (see [Git Changes](#git-changes)) |
| `esc` *(in Notes)* | Stop editing notes (saves at once) |
| `[` / `]` | Decrease / increase the mouse-wheel scroll speed (preview focus and attached terminals) and save it to config |
| `m` | Merge the selected worktree into the current branch, uncommitted: the confirmation shows `Into <branch> · <path>` (yellow when it is not the main checkout's default branch), the commits and diff stat, and for uncommitted files a `space` toggle (on by default) that commits them first with the session's title as the message. On conflicts: `enter` (or `esc`) keeps the merge in progress for you to resolve (marked merged), `a` aborts it. Every session of the worktree then shows `✓` |
| `M` | Toggle the worktree's merged marker (every session of it); worktrees only — in the main checkout use `D` |
| `D` | Mark the selected session done / not done (`☑`; any session, independent of merged) |
| `h` / `l` | Resize the sidebar |
| `x` / `X` | Stop the selected running session, asking first (in a worktree, also whether to delete it) / force-stop it at once, keeping its worktree |
| `s` / `S` | Resume / fresh-restart the selected exited session |
| `backspace` | Drop the selected exited session from the list (when its notes still have open checklist items, `m` first moves them to Tasks) |
| `r` | Refresh the session list |
| `A` | Archive/unarchive (does not stop an agent) |
| `f` / `/` | Cycle filters / search title, notes, provider, branch, path |
| `i` | Workspace Git summary; `P` queries PR, `b` opens it, `c` pushes and opens GitHub's new-PR form (after confirmation), `g` opens the Git tab |
| `C` | Settings: a grid of every setting with a Global and a This repo column (● in effect, ⚠ needs trust); ↑↓ (`j`/`k`, PgUp/PgDn, Home/End) setting, ←→ or `tab` column, Enter edits that cell's layer, x clears it, e that column's raw JSON (Ctrl+S saves, Ctrl+F formats), T reviews/trusts the repo file; in Actions `a` adds and `x` removes, in Linked items Space toggles and Enter saves (you're also asked inline right before anything from it runs) |
| `e` | Choose an action (global, plus trusted repository actions); it runs beside the shell on the worktree's Terminal tab (`v` switches) |
| `U` | Agents: installed vs latest version of Claude, Pi and Codex; Enter runs the selected agent's own updater (asks first when it has running sessions — Enter or `y` confirms — which keep their version until restarted), `r` re-checks, Esc back |
| `H` / `F` | Export/open handoff (notes plus commits and changed files, no diff content) / create a clean child from the reviewed document |
| `!` | Next known attention session |
| `x` / `X` *(while starting)* | Cancel startup/setup, retaining its worktree |
| `?` | Help: topics on the left (↑↓ or 1-9 switch), each a table of keys; `/` searches every topic, PgUp/PgDn scroll, Esc closes |
| `q` | Quit the UI; running sessions continue in the daemon (only from the session list: every other screen closes with `esc`) |

> *Deletion is conservative: unknown/unsafe Git state requires typing `DELETE`. `X` does not authorize data loss. Main/current/actively shared worktree protections cannot be overridden.*

### Sidebar

```
│ Sessions         all 7/7 · ! 2 │   filter/search and count; ! N need attention
│╎ 1 ▾ ⠋ auth refactor       ▶ ✶ │   ╎ shares the selected session's worktree
│› 2   ↳ ● write tests         π │   › selected
│╎ 3   ⑂ ○ try alt approach    ✶ │
│  4 ? fix flaky checkout e2e… ◇ │
│ ────────────────────────────── │
│ write tests                    │   the selected session, when there is room
│ π pi · idle · 7m               │
│ ⎇ feat/auth · shared with 2 …  │
```

- **Before the title:** status — spinner starting/working, `●` idle, `◌` activity unknown, `○` exited; with agent signals `?` needs input, `◆` response ended (not task success), `!` failed or failed/interrupted exit, `⌛` rate-limited. Tree — `▾`/`▸` expanded/collapsed parent, `↳` clean and `⑂` forked sub-session.
- **After the title:** `▶` Dev running (once per worktree), `▣` archived, `!` cleanup error, `✓` merged (by `m`, `M`, or found merged elsewhere), `☑` done (`D`), `+N` hidden sub-sessions, `↑` running an older agent version than the one now installed (restart it to update; the details line then shows both versions, e.g. `✶ claude 2.1.287 · 2.1.290 installed`), then the agent: `✶` Claude, `π` Pi, `◇` Codex.
- Archived rows are dimmed; in the archived view the parents shown for context are dimmed instead. Done rows have a muted title (their markers and status stay readable), and the details say `done 2d ago`. `?` → Sidebar lists all of this in the app.

### Attach Mode

| Key | Action |
| --- | --- |
| *(most keys)* | Sent directly to the attached pane/session |
| `Ctrl+Space` | Detach and return to Deckhand |
| `Ctrl+]` | Detach and return to Deckhand |

### Notes Editing

The Notes tab shows two notes: the worktree's, shared by every session in that worktree (sessions in the main checkout share one for it), above the selected session's own. Press `o` to edit:

| Key | Action |
| --- | --- |
| *(typing)*, `enter`, `backspace`, `delete`, paste | Edit at the cursor (multi-line paste works; Enter on a `- [ ]` item starts the next one) |
| arrows, `home` / `end`, `ctrl+a` / `ctrl+e` | Move (up/down follow wrapped lines; `ctrl+a`/`ctrl+e` are line start/end) |
| `alt+←` / `alt+→` (or `ctrl+`, or Option on macOS) | Jump a word; `alt+backspace` or `ctrl+w` deletes one |
| `pgup` / `pgdn`, `ctrl+home` / `ctrl+end` | Move a screen / to the start or end |
| `tab` | Switch between the worktree's note and this session's |
| `ctrl+x` | Check/uncheck the line's checklist item, or make the line a `- [ ]` item |
| `ctrl+t` | New checklist item below |
| `ctrl+o` | Open the note you are editing in Cursor / VS Code |
| `ctrl+p` | Send the line's open checklist item to [Tasks](#tasks); the note keeps a `↗` link to it |
| `esc` | Back to browsing |

### Tasks

`b` opens the repository's task list in the right pane (every worktree of the repository shares it). Notes stay scratch space: a note's checkbox only becomes a task when you send it (`ctrl+p` while editing, or `p` in the board's note list).

| Key | Action |
| --- | --- |
| `j` / `k` (`↑`/`↓`), `g` / `G` | Select a task / the first / the last |
| `a` / `enter` | Add a task / edit the selected one: title, then `tab` for its details (typed into the agent's input with the title when a session starts from it); `enter` saves the title, `ctrl+s` saves from the details |
| `n` | Start a session for a backlog task: the usual new-session form, named after it, in a new worktree; `↑`/`↓` pick the base branch |
| `space` | Tick a task done, or reopen it |
| `o` | Open (select) the session doing it |
| `J` / `K` | Reorder within its group |
| `x` | Delete (press twice) |
| `tab` | Open checklist items in notes that are not tasks yet; `p` sends one to Tasks, `enter` opens its note |
| `E` | Open the task list in Cursor / VS Code |
| `esc` | Back |

A task started with `n` is linked to its session's worktree (or, in the main checkout, to the session): the board shows the most urgent state of the sessions there (needs you, working, idle, exited), the session's Notes tab and sidebar details name the task, and the task (`title: details`, one line) is typed into the agent's input once it has started, not sent: edit it and press Enter yourself. Deckhand waits until the agent's screen settles, never types into a menu or question (a folder-trust prompt, for example), and skips it if you start typing first or the agent doesn't settle within two minutes. Merging the worktree (`m`, `M`, or found merged) ticks it, and the merge screen says so; `D` ticks it once every session there is done; undoing either reopens it. Work removed or deleted without merging puts the task back in the backlog, marked with the branch it was tried in. Done tasks fold away after a week.

### Git Changes

The Git tab shows the selected session's worktree changes in VS Code's groups: **Merge Conflicts**, **Staged Changes**, **Changes** (unstaged tracked files) and **Untracked**, each with a count. A partially staged file appears in both Staged and Changes. Each row shows the status letter (M, A, D, R, C, U, ?), the file name with its directory dimmed, `old → new` for renames, and `+added −removed` lines (`bin` for binary files). The list refreshes about every two seconds while the tab is open and right after you stage or unstage. Press `v` to focus it:

| Key | Action |
| --- | --- |
| `j` / `k` (↑↓), `g` / `G` (`home` / `end`) | Select a file (across groups) / first / last |
| `space` | Stage the selected file (unstaged, untracked, or conflicted: marks it resolved) or unstage it (staged) |
| `a` / `A` | Stage everything (conflicted files are left for you to stage one by one) / unstage everything |
| `enter` / `E` | Open the file in Cursor or VS Code at its first changed line |
| `J` / `K`, PgUp/PgDn | Scroll the diff preview |
| `o` | Attach lazygit (commits, discards, hunks, branches, history) |
| `esc` / `v` | Back to browsing |

The diff preview (beside the list in wide terminals, below it otherwise) shows the staged diff for staged files, the unstaged diff for changes and conflicts, and the whole file for untracked files; it is read-only and cut at 256 KB. Deckhand never commits, discards or edits files from this view.

---

## 📂 Sessions & Workspaces

### Workspace Modes

When you create a session, Deckhand launches it in one of three workspace modes:

| Mode | Behavior |
| --- | --- |
| **No worktree** | Runs in the current repository directory |
| **New worktree** | Creates or resolves a worktree for the session |
| **Existing worktree** | Picks from existing git worktrees, including the current/main one |

A sub-session defaults to its parent's current directory, so a clean sub-session opens in the parent's worktree unless you choose a different mode.

New worktrees use an explicitly trusted [project hook](#-worktree-hooks); otherwise Deckhand falls back to `git worktree add` at the configured `worktree.location` (default: the active state directory's `worktrees/`, normally `~/.deckhand/worktrees/`). The `worktree` setting can also choose the new branch's start point (current checkout, default branch, or a freshly fetched `origin/<default>`) and name template, switch the hook off, and symlink heavy directories (such as `node_modules` or a virtualenv) and private files into each new worktree. **C** (Settings) shows what is in effect and which layer sets it, and edits it in place: location presets, branch options, the hook switch and a link picker with suggestions from your checkout — see [worktree settings](docs/no-brainers.md#worktree-settings).

### Sub-sessions

Press `N` on a selected session to create a sub-session for related follow-up work. Sub-sessions render indented under their parent in the sidebar; press `c` on a parent to collapse or expand its subtree.

- Choosing `claude`, `pi`, or `codex` creates a **clean** sub-session — a fresh agent context in the parent's directory or worktree.
- Choosing **`⑂ Fork parent`** forks the parent's conversation into a new one (Claude `--fork-session`, Pi `--fork`, `codex fork`); nothing is typed into the agent. A fork copies the conversation **as saved at that moment**: a turn still in progress in the parent isn't included.
- Forks start in the parent's worktree. Claude and Pi forks can go into a new or existing worktree instead (`tab` in the create form); the child's first message then tells it where it now is, because the copied conversation's paths point at the parent's worktree, and the parent's uncommitted changes aren't there. Codex forks always stay in the parent's worktree, because Codex may reopen a fork in the directory it was recorded in.
- A Codex parent can be forked once its conversation ID is known (from Codex's SessionStart hook or its exit hint).

### Agent Identity and Restarts

Claude and Pi sessions get an exact conversation ID (a UUID) chosen by Deckhand at launch, plus a readable label built from the session name and a short, immutable Deckhand id: `dh-{sanitized-session-name}-{short-id}`.

| Agent | Create | Restart | Forked sub-sessions |
| --- | --- | --- | --- |
| **Claude** | `claude --session-id <uuid> --name dh-{name}-{short-id}` | `claude --resume <uuid>`; an unknown ID asks for `S`, which creates a fresh ID | `claude --resume <parent> --fork-session --session-id <child-uuid> --name dh-{name}-{short-id}`, any worktree |
| **Pi** | `pi --session-id <uuid> --name dh-{name}-{short-id}` | Same `--session-id`; `S` creates a fresh ID | `pi --fork <parent> --session-id <child-uuid> --name dh-{name}-{short-id}`, any worktree |
| **Codex** | Normal launch; capture native ID via supported hook/exit hint | `codex resume <id>` when known; otherwise explicit `S` required | `codex fork <parent-id>` (parent's ID must be known), parent's worktree only |

<details>
<summary><strong>More details on Agent Identity</strong></summary>

Pi session files live in Pi's normal session tree at `~/.pi/agent/sessions/`, not under `~/.deckhand`. They therefore stay visible in Pi's own `/resume` UI (under the readable label) and survive deletion of Deckhand state. Sessions recorded before exact IDs keep resuming by their stored name (Claude) or `--session` path (Pi).

- Claude prints a `claude --resume "..."` command when it exits; Deckhand parses that final preview and persists the parsed handle when available. If Claude reports `No conversation found with session ID`, Deckhand shows a hint to press `S` instead of starting fresh silently.
- `S` fresh-restarts an exited session without using the prior resume handle.
- Forked sub-sessions store the parent agent reference. Claude and Pi children get their own exact ID at launch; a Codex child's ID is captured like any Codex session's, never the parent's. `s` resumes the child's own conversation. A fork that never launched, whose fork failed (the parent had no saved conversation yet: a conversation is saved once it has a message), or whose Codex ID was never reported forks the parent again with a new child ID on `s`.
- Children recorded before `--fork-session` keep working: one stored with its `/branch` name resumes by that name; one still holding the parent's reference forks again.

</details>

---

## ⚙️ Configuration

Deckhand reads configuration from `~/.deckhand/config.json`. `DECKHAND_HOME` selects a separate state namespace; the [isolated dev launcher](docs/dev-build.md) safely manages this for preview testing.

### Project Configuration

Session defaults, a Dev command, a setup command and layout/symlinks for new worktrees, and named actions come from two layers, both edited in place from the **C** Settings screen: global `defaults` in your user `config.json` (never need trust), overridden per repository by one `deckhand.json` in the main checkout. The repository file applies only once you trust its exact contents; Deckhand asks inline (Enter trusts, `s` continues with global defaults only) when you create a session, open actions, start Dev or retry setup. Edits you make in Deckhand keep the file trusted if it was trusted (or new); changes from outside need review — see [project configuration, trust and cleanup](docs/no-brainers.md) for the full behaviour.

Optional **Agent signals** and **Notifications** (Settings → Agents, global only; `agent_hooks`/`notifications` in user config) enable capability-gated lifecycle integration and best-effort desktop notifications. They default off. Claude needs nothing else; Codex needs `deckhand hooks codex` in `~/.codex/hooks.json` (Settings warns when it's missing). Native approvals stay in agent terminals. A response ending is **not task success**.

### Terminal, Git and Dev Panes

Terminal (your `$SHELL`), Git (the Changes view, plus `lazygit` on `o`) and Dev belong to the session's **worktree** (or, for sessions without a worktree, the checkout they run in). Every session in the same worktree sees the same shell, the same changes, the same lazygit and the same Dev command, and attaching (`o`) from any of them opens that one process, also after the session's agent has exited. The shell starts the first time you view the Terminal tab and lazygit the first time you attach it; one that exited (`exit`, `q`) starts again the next time you view (Terminal) or attach it. They stop when the worktree is deleted, when the worktree's last session is removed, or when the daemon stops. While a new worktree is still being created, these tabs say so; a deleted worktree has none, for every session that was in it.

### Dev Command

Focus the Dev tab with `d`, then press `d` again while it is focused to start or stop the command. Dev belongs to the session's **worktree**, not to the session: every session in the same worktree (or in the same checkout, for sessions without a worktree) sees the same Dev pane and output, and starting or stopping it from any of them acts on that one process. It keeps running after the agents exit (exited sessions still show, attach to and stop it) until you stop it, delete the worktree, remove the worktree's last session, or the daemon stops; the sidebar shows `▶` on the worktree's first row while it runs. Set the command globally (or per repository as `devCommand` in `deckhand.json`):

```json
{
  "defaults": {"devCommand": "npm run dev"}
}
```

The older top-level `dev_command` still works when no `devCommand` is set.

### Notes

Notes are Markdown files in `~/.deckhand/notes/`: `sessions/<session>.md` for each session's own, `worktrees/<id>.md` for each worktree (shared by every session in it, sub-sessions and attached ones included; a new worktree later created at the same path starts with a fresh note) and `repos/<hash>.md` for the main checkout. They autosave while you type. Edit them in VS Code too (`E` opens one): changes show up in Deckhand within a second, and Deckhand never overwrites them: if a file changed while you were typing, it reloads the file and says so. A session's note is deleted with the session, a worktree's when its last session is removed; the main checkout's is kept. Older notes stored in `state.json` move into these files on the first start. Checklist items (`- [ ]`, `- [x]`, also `*`, indented) render as ☐/☑, the sidebar shows `☐ N open` for the selected session, and handoffs (`H`), the merge confirmation and `/` search include the worktree's note. Each note holds up to 50 000 characters (a longer file is shown cut and edited in your editor).

### Task Lists

Each repository's tasks are one Markdown file, `~/.deckhand/notes/tasks/<hash>.md`, beside the notes: a task is a top-level `- [ ]` item, the indented lines under it its details, and anything else (headings, prose) is kept as you wrote it. Deckhand's bookkeeping (the task's ID, its linked worktree or session, when it was added or done) sits in a `<!-- dh:… -->` comment at the end of the item's line. Edit the file in your editor too (`E` on the board): Deckhand applies each change to the file as it is on disk, so your edits are never overwritten, and items you add there get an ID on Deckhand's next change.

### Attach Scroll Sensitivity

Attached sessions and Preview focus dampen trackpad and mouse-wheel scrolling. Press `[` / `]` in Deckhand to decrease/increase the multiplier immediately and save it, or edit the config directly:

```json
{
  "attach_scroll_sensitivity": 0.12
}
```

Use `1` for normal terminal scrolling, lower values for slower scrolling, or `0` to ignore vertical wheel events while attached/in Preview focus. Default: `0.12`.

### State and Logs

| Path | Purpose |
| --- | --- |
| `~/.deckhand/state.json` | Persisted session list (with done markers) and per-worktree merged/deleted markers |
| `~/.deckhand/notes/` | Notes as Markdown files: per session, per worktree, and the main checkout's; `tasks/` holds each repository's task list |
| `~/.deckhand/config.json` | User configuration, global `defaults` and exact repository trust fingerprints |
| `~/.deckhand/ui-state.json` | Per-repository selection, tabs, width, tree/filter/search preferences |
| `~/.deckhand/handoffs/` | Private, inspectable Markdown handoffs |
| `~/.deckhand/daemon.log` | Supervisor daemon diagnostics |
| `~/.deckhand/daemon.pid` | Active supervisor daemon PID |
| `~/.deckhand/daemon.sock` | Local IPC socket |
| `~/.deckhand/workers/` | Per-session worker PID and log files |
| `~/.deckhand/worktrees/` | Default location for auto-created worktrees (`worktree.location` changes it) |
| `~/.pi/agent/sessions/` | Pi's normal session storage |

---

## 🪝 Worktree Hooks

For new-worktree sessions, Deckhand creates or resolves a git worktree and then starts the agent inside it. Most layouts (location, shared dependency directories, env files) are covered by the declarative [`worktree` setting](docs/no-brainers.md#worktree-settings); the hook is the escape hatch for anything else. It uses this project hook only after its exact contents are reviewed and trusted (together with the repository's `deckhand.json`), and otherwise falls back to `git worktree add`:

```text
.claude/scripts/create-worktree.sh
```

Set `"worktree": {"hook": false}` (or set **Creation hook** to off in **C** Settings) to ignore the script entirely: it is then never run or reviewed. A repository's `deckhand.json` can switch it off even before it is trusted, since that only prevents execution.

### Hook Contract

- Read JSON from `stdin`.
- Use `name` as the sanitized worktree/session name.
- Use `cwd` as the directory where `deckhand` was launched.
- Create or register a git worktree.
- Print the absolute worktree path to `stdout` as the final non-empty line.
- Exit `0` on success.

> Deckhand also sets `CLAUDE_PROJECT_DIR` to the launch cwd, for compatibility with Claude-style hooks.
>
> Deckhand runs the reviewed bytes with `bash -c`, so `$0` is the script path but `${BASH_SOURCE[0]}` is empty; locate sibling files with `$0`.

<details>
<summary><strong>Minimal hook example</strong></summary>

```bash
#!/bin/bash
set -e

INPUT="$(cat)"
NAME="$(echo "$INPUT" | jq -r '.name // "worktree"')"
CWD="$(echo "$INPUT" | jq -r '.cwd // env.CLAUDE_PROJECT_DIR // env.PWD')"
DIR="${DECKHAND_HOME:-$HOME/.deckhand}/worktrees/$NAME"
START="$(git -C "$CWD" rev-parse HEAD)"

if [ -d "$DIR/.git" ] || git -C "$DIR" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "$DIR"
  exit 0
fi

if [ -e "$DIR" ]; then
  echo "Path exists but is not a git worktree: $DIR" >&2
  exit 1
fi

if git -C "$CWD" show-ref --verify --quiet "refs/heads/$NAME"; then
  git -C "$CWD" worktree add "$DIR" "$NAME" >&2
else
  git -C "$CWD" worktree add -b "$NAME" "$DIR" "$START" >&2
fi

echo "$DIR"
```

</details>

---

## 🏗️ Architecture

Deckhand has four main pieces:

1. **Ink Frontend** — renders the terminal UI, sends requests to the daemon, and attaches to live panes on request.
2. **Local Daemon** — owns session state, IPC, worktree operations, and worker supervision.
3. **Session Workers** — one per running session; each owns the agent PTY.
4. **Workspace Workers** — one per worktree in use (started on demand: the first Terminal view, lazygit attach, or starting Dev); each owns the Terminal (shell), lazygit and Dev PTYs shared by every session in that worktree. The Git tab's Changes view needs no worker: the daemon reads Git status for the worktrees being watched.

Terminal output is fed into a headless [`xterm.js`](https://github.com/xtermjs/xterm.js) model. The UI receives rendered snapshots for previews, while attach mode streams input and output directly between your terminal and the selected PTY.

### Daemon Lifecycle

Deckhand spawns a long-lived supervisor daemon the first time you launch the UI. Quitting with `q` leaves the daemon — and any running sessions — in place; relaunching `deckhand` reattaches. Stopping the daemon kills all running sessions.

| Action | Command |
| --- | --- |
| Check if daemon is running | `pgrep -F ~/.deckhand/daemon.pid` |
| Tail daemon logs | `tail -f ~/.deckhand/daemon.log` |
| Stop the daemon | `kill $(cat ~/.deckhand/daemon.pid)` |
| Recover crashed daemon | `rm ~/.deckhand/daemon.pid ~/.deckhand/daemon.sock` then relaunch |

---

## 🛠️ Development

**Preview safely alongside a production daemon:** `npm start` builds and opens the isolated dev build in a disposable sandbox; `npm stop`, `npm restart` and `npm run status` control only that dev daemon. No global link/install or production restart. See [the testing checklist](docs/dev-build.md).

For ordinary local development, install from source. The plain `dev`/`daemon` commands below share production state unless you explicitly configure isolation:

```bash
git clone https://github.com/tejgor/deckhand.git
cd deckhand
npm install
npm run dev      # run from source via tsx
npm run daemon   # run only the daemon in dev mode
npm run build    # compile to dist/
npm link         # link globally
```

After changing source code, rebuild with `npm run build` before re-running the linked CLI.

> **macOS note:** `npm install` runs `scripts/fix-node-pty.js`, which attempts to repair the `node-pty` `spawn-helper` binary. See [Troubleshooting](#-troubleshooting) if install fails.

---

## 🚑 Troubleshooting

- **`deckhand` can't find an agent:** Confirm the binary is on `PATH` with `which claude`, `which pi`, or `which codex`. Deckhand inherits the launching shell's environment.
- **`node-pty` fails to load on macOS:** Re-run the repair script directly: `node scripts/fix-node-pty.js`. If that doesn't help, reinstall: `rm -rf node_modules && npm install`.
- **Stale daemon socket or PID:** If `deckhand` hangs at startup, the supervisor may have exited uncleanly. Remove stale files: `rm -f ~/.deckhand/daemon.pid ~/.deckhand/daemon.sock` and relaunch.
- **`o` on the Git tab fails:** Install [`lazygit`](https://github.com/jesseduffield/lazygit) and ensure it is on `PATH`. The Changes list itself only needs `git`.
- **Dev tab does nothing:** Press `d` once to focus the Dev tab, then press `d` again to start/stop the command. Ensure a `devCommand` is set in global defaults (**C**) or a trusted `deckhand.json`.

---

## 🗑️ Uninstall

```bash
npm uninstall -g @tejgor/deckhand
```

To remove all local state (sessions, logs, and auto-created worktrees):

```bash
rm -rf ~/.deckhand
```

> *If Deckhand created git worktrees under `~/.deckhand/worktrees/`, remove them through the UI (or with `git worktree remove`) before deleting the directory, so git's bookkeeping stays consistent.*

---

## 🤝 Contributing

Issues and pull requests are welcome. For larger changes, please open an issue first to discuss the approach. Run `npm test` and confirm the isolated CLI launches before sending a PR.

## 📄 License

Deckhand is released under the [MIT License](LICENSE).
