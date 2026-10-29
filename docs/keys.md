# Keys

Every Deckhand key, screen by screen; press `?` in Deckhand for the in-app guide. Back to the [README](../README.md).

## Session list

**Layout**: Deckhand has two places, the session list on the left and the pane on the right. `→` or `l` steps into the pane, and `←`, `h` or `esc` steps back out, except where you type: typing to the agent or a shell leaves with `ctrl+]`, and the notes editor with `esc` (`←` and `h` are text there). The same keys mean the same thing on every screen: `enter` opens or chooses, `space` toggles and `E` opens your editor. In lists, `a` adds and `x` removes. On the session list itself, `a` opens the Notes tab and `x` stops a session. `o` (or `enter`) attaches: it opens what the tab shows full screen, and `ctrl+]` comes back ([Attach](#attach)).

| Key | Action |
| --- | --- |
| `j`, `↓` | Select the next visible session |
| `k`, `↑` | Select the previous visible session |
| `1`–`9` | Select that numbered session |
| `0` | Select session 10 (while 10 or fewer are visible) |
| a number | With more than 10 visible, type it: `enter` or a short pause selects, `esc` clears |
| `/` | Search titles, notes, tasks, agent, branch and path. While typing, `↑`/`↓`, `ctrl+n`/`ctrl+p` or `ctrl+j`/`ctrl+k` move the selection, `enter` keeps the search and `esc` clears it |
| `f` | Open the filter menu: `a` active, `r` running, `!` attention, `e` exited, `A` archived, `*` all |
| `f` `f` | Go back to the active filter |
| `!`, `;` | Select the next session that needs you, switching to the attention filter |
| `ctrl+d` / `ctrl+u` | Half a list down / up (`pgdn` / `pgup` too). In every list and scroll area, not in text fields |
| `V` | Switch the list between your own order (the default) and [sections by state](features.md#organizing-and-inspecting-sessions) (needs you, unread, working, idle, exited) |
| `J` | Move the selected session down past the sibling below it (in the state view, within its section) |
| `K` | Move the selected session up past the sibling above it (in the state view, within its section) |
| `c` | In your own order: cycle its sub-sessions (hide the exited ones, collapse, expand; when they have all exited, collapse at once). In sections by state: fold or unfold the **Exited** section (`▸` / `▾`); the selected session stays until you move off it. Both in the active and all (`*`) views; searches and the other filters show every match |
| `<` | Narrow the sidebar: 2% less of the terminal, saved as your width for every repository ([sidebar width](features.md#organizing-and-inspecting-sessions)) |
| `>` | Widen the sidebar: 2% more of the terminal, saved |
| `=` | Fit the sidebar to the sessions again (forgets your width) |
| `tab`, `]` | Next tab: Preview, Terminal, Git, Dev, Notes |
| `shift+tab`, `[` | Previous tab |
| `p` | Show the Preview tab |
| `t` | Show the Terminal tab |
| `g` | Show the Git tab |
| `d` | Show the Dev tab |
| `D` | Start or stop the worktree's Dev command (and show the Dev tab); starting it stops Dev in the repository's other worktrees |
| `a` | Show the Notes tab |
| `→`, `l` | Step into the pane: [type to the agent or a shell](#typing-to-the-agent), browse [Git changes](#git-changes) or [edit the notes](#notes-editor) |
| `←`, `h`, `esc` | Step back out to the session list (typing to the agent: `ctrl+]`; the notes editor: `esc`) |
| `enter`, `o` | Open full screen what the tab shows: the agent, the shell or action, lazygit or Dev |
| `enter` *(on Notes)* | Edit the note here |
| `o` *(on Notes)* | Edit the note in your terminal editor ([in vim](features.md#notes)) |
| `v` *(on Terminal)* | Show the next of the worktree's [shells](features.md#several-shells), then its last action |
| `v` *(on Dev)* | Select the next [Dev process](features.md#several-dev-processes); `enter` opens the selected one |
| `E` | Open the session's directory in Cursor or VS Code (on Notes: the note) |
| `n` | New top-level session |
| `N` | New sub-session: clean, a `⑂` fork or `↳` from the handoff ([sub-sessions](features.md#sessions-and-sub-sessions)) |
| `x` | Stop the selected session, asking first (and whether to delete its worktree and branch) |
| `X` | Force-stop the selected session, keeping its worktree: asks first, with the force kill chosen (one `enter`) |
| `s` | Resume the selected exited session (or retry a failed setup) |
| `S` `S` | Restart the selected exited session with a fresh conversation (press twice: the old conversation can't be resumed from Deckhand afterwards) |
| `space` | Mark the selected session done or not done (`☑`) |
| `U` | Mark the selected session's finished reply unread (`◆`), or read again ([unread replies](features.md#organizing-and-inspecting-sessions)) |
| `m` | Merge the selected worktree into the current branch, uncommitted ([merging](features.md#merging-and-done)) |
| `M` | Set or clear the worktree's merged marker without merging |
| `r` | Open the [run list](#run-list): Dev, your actions and the worktree's shells |
| `A` | Archive or unarchive the selected session (it keeps running) |
| `backspace` | Archive the selected exited session; on an archived one, remove it for good ([archiving](features.md#organizing-and-inspecting-sessions)) |
| `H` | Export and open a handoff ([handoffs](features.md#resume-and-handoffs)) |
| `i` | Show the worktree's Git summary and PR lookups ([inspecting](features.md#organizing-and-inspecting-sessions)) |
| `b` | Open the repository's task list ([Tasks](#tasks)) |
| `W` | Open the Worktrees screen ([Worktrees](#worktrees)) |
| `C` | Open Settings ([Settings](features.md#settings-c)) |
| `G` | Open the Agents screen ([versions and updates](features.md#agent-versions-and-updates)) |
| `R` | Reload the session list (it updates live, so this is rarely needed) |
| `?` | Open the in-app guide |
| `q` | Quit the UI; running sessions continue in the daemon |
| `ctrl+c` | Quit from any screen. In a text field (a search, a name, a note, a task, a DELETE prompt, a Settings edit) the first press acts as `esc`; while [typing to the agent](#typing-to-the-agent) it goes to the agent |

`q` works only on the session list: every other screen closes with `esc`.

**Ctrl and letters**: outside a text field, `ctrl` plus a letter never does what the letter does (`ctrl+x` doesn't stop a session, `ctrl+a` doesn't open Notes). The exceptions are `ctrl+d` / `ctrl+u`, which page wherever `pgdn` / `pgup` do, since a laptop has no page keys without `fn`.

**Press twice**: a few keys that are hard to undo but don't need a dialog act on the second press. The first press says in the status line what the second will do; press the same key again within 2 seconds to do it, and any other key cancels. These are `S` (fresh restart), `x` in the [run list](#run-list) and in [Tasks](#tasks), and `a` / `A` in [Git changes](#git-changes) while a file is partly staged.

**In the guide** (`?`): `↑`/`↓` or `1`–`9` pick a topic (`g`/`G` the first or last), `/` searches every topic, `J`/`K` (or `pgdn`/`pgup`, `ctrl+d`/`ctrl+u`, `space`) scroll and `esc` closes.

**Dialogs and the merge screen**: when what they show does not fit, `J`/`K` (or `pgdn`/`pgup`, `ctrl+d`/`ctrl+u`) scroll it, and the border says how much more there is (`↓ 12 more · J`). In `i`, the trust review and the DELETE prompt, `J`/`K` scroll a page. In Settings lists, `J`/`K` move five rows and `g`/`G` go to the first or last. In a DELETE prompt, `esc` goes back to the choices.

> [!NOTE]
> Deletion is conservative. Unknown or unsafe Git state needs `DELETE` typed, and `X` never authorises data loss. The protections for the main checkout, the current worktree and an actively shared worktree can't be overridden. See [Cleanup](features.md#cleanup).

## New session

`n` (or `N` for a sub-session) opens the new-session form in two steps. First pick the agent:

| Key | Action |
| --- | --- |
| `j`, `↓` / `k`, `↑` | Next / previous agent (for `N`, also **⑂ Fork parent** and **↳ From its handoff**) |
| `enter` | Continue to the name |
| `esc` | Cancel |

The list starts on your default agent (Claude unless [`defaultAgent`](features.md#configuration) says otherwise), or on the parent's agent for `N`. Then name the session:

| Key | Action |
| --- | --- |
| typing | The session's name, which can't be empty. With a new worktree, it also names the worktree's folder and branch, sanitised ([worktree settings](features.md#worktree-settings)) |
| `tab` | Next workspace: **no worktree** (the launch checkout), **new worktree**, **existing worktree** ([modes](features.md#sessions-and-sub-sessions)) |
| `↑` / `↓`, `ctrl+p` / `ctrl+n`, `ctrl+k` / `ctrl+j` | With a new worktree: the branch it starts from |
| `enter` | Create the session (with an existing worktree: pick it first) |
| `esc` | Back to the agent |

The form opens on **no worktree** unless [`defaultWorkspace`](features.md#configuration) says otherwise. With **existing worktree**, `enter` lists the repository's worktrees: `j`/`k` pick one, `/` searches (while typing, `↑`/`↓`, `ctrl+p`/`ctrl+n` or `ctrl+k`/`ctrl+j` move), `enter` creates the session there and `esc` goes back.

The agent starts with no task: attach (`o`) and type it, or start the session from a task (`n` in the [task list](#tasks)) to have the task typed for you.

## Sidebar

```text
│ Sessions         all 7/7 · ! 2 │   filter/search and count; ! N need you
│╎ 1 ▾ ⠋ auth refactor       ▶ ✶ │   ╎ shares the selected session's worktree
│› 2   ↳ ● write tests         π │   › selected
│╎ 3   ⑂ ○ try alt approach    ✶ │
│  4 ? fix flaky checkout e2e… ◇ │
│ ────────────────────────────── │
│ write tests                    │   the selected session, when there is room
│ π pi · idle · 7m               │
│ ⎇ feat/auth ╎2 ▶               │   branch; ╎2 two others share it, ▶ Dev runs
```

| Glyph | Meaning |
| --- | --- |
| `⠋` (spinner) | Before the title: starting or working |
| `●` | Before the title: idle |
| `○` | Before the title: exited |
| `?` | Before the title: needs you (agent signals) |
| `◆` | Before the title: response ended, unread until you attach to the agent or press `U` (agent signals) |
| `!` | Before the title: failed (red) or interrupted (yellow) |
| `⌛` | Before the title: rate-limited (agent signals) |
| `▾` / `▸` | Parent with sub-sessions, expanded / collapsed (`c`) |
| `↳` | Clean sub-session |
| `⑂` | Forked sub-session |
| `· name` | After a sub-session's title: its parent, when it isn't listed right above it (another section, a search); cut first when narrow |
| `▶` | After the title: the worktree's Dev command runs (on its first row; muted on the worktree's other rows, which share it) |
| `▣` | After the title: archived |
| `!` | After the title: cleanup error |
| `✓` | After the title: merged (by `m`, `M` or found merged elsewhere) |
| `☑` | After the title: done (`space`) |
| `+N` | After the title: N hidden sub-sessions |
| `↑` | Runs an older agent version than the one installed: restart it (`x`, then `s`) to update |
| `✶` | Agent: Claude |
| `π` | Agent: Pi |
| `◇` | Agent: Codex |
| `╎` | Left edge: shares the selected session's worktree (and its Terminal, Git and Dev) |
| `›` | Left edge: the selected session |
| `! N` | Header: N sessions need you (`!` or `;` jumps to them) |
| `╎N` | Details: N other sessions share the worktree |
| `☑2d` | Details: done 2 days ago |

The sketch shows your own order, the default. With `V` the rows sit under section headers instead, **Needs you**, **Unread**, **Working**, **Idle** and **Exited**, each with its count (`  Idle 3 ───`); a sub-session is indented under its parent when both are in the same section, and otherwise under a muted line naming its parent (`auth refactor ┄┄┄`) at the head of its run. `c` folds **Exited** to its header (`Exited 4 ▸`). While you type to an agent or shell, every row but the selected one is dimmed.

Archived rows are dimmed. The archived view (`f` `A`) lists only archived sessions. Done rows have a muted title, while their markers and status stay readable. Below the branch, the details repeat the row's markers as glyphs (`╎N`, `▶`, `✓`, `☑2d`, `▣`), then `☐ N open` checklist items. A long branch is shortened to keep them. When `↑` shows, the details line names both versions, for example `✶ claude 2.1.287 · 2.1.290 installed`. In the app, `?` → Sidebar lists all of this.

## Typing to the agent

`→` (or `l`) on the Preview tab of a running session types to the agent without attaching; [Typing to an agent in the Preview](features.md#typing-to-an-agent-in-the-preview) describes it. On the Terminal tab, `→` types into the shell shown the same way.

| Key | Action |
| --- | --- |
| `ctrl+]`, `ctrl+space` | Back to the session list |
| any other key | Goes to the agent (or the shell), `esc`, `ctrl+c` and the arrows included. The first `ctrl+c` of each stint says so in the status line |
| mouse wheel | On Preview: scroll the agent's view (or the preview's history); the [scroll speed](features.md#scroll-speed) is set in Settings |

While you type, the rest of the sidebar is dimmed (the selected row stays bright) and the pane's header shows a filled *typing to the agent* (or *typing into shell N*). When typing ends by itself, because the agent exited or the shell closed, keys you were still typing are held back until you pause for 0.75 s, so they can't run list commands.

## Attach

`enter` or `o` attaches to the selected pane: the agent, the shell, lazygit, Dev or a running action. Every attached view gets a Deckhand status line, with a rule separating it from the pane: the session, the agent's state or the pane's name, how many other sessions need you and the detach key. It sits on the bottom row of every view, so the shell's and Dev's output still reaches your terminal's scrollback. A note opened in Vim or Neovim (`o` on Notes) shows the same bar in its status line, with the note keys (Ctrl+X toggles a checkbox, Ctrl+T adds an item) when there is room.

| Key | Action |
| --- | --- |
| any other key | Sent to the attached pane |
| `ctrl+space` | Detach and return to Deckhand |
| `ctrl+]` | Detach and return to Deckhand |
| mouse wheel | Scroll the pane, scaled by the [scroll speed](features.md#scroll-speed) |

Both detach keys still work when the attached program switches the terminal to an enhanced keyboard mode (kitty's or xterm's modifyOtherKeys, as Codex can).

## Notes editor

Press `enter` (or `→`) on the Notes tab to edit the note; [Notes](features.md#notes) explains which note you see and where it is stored.

| Key | Action |
| --- | --- |
| typing, paste | Insert at the cursor (multi-line paste works) |
| `enter` | New line (on a `- [ ]` item, start the next item) |
| `backspace` | Delete the character before the cursor |
| `delete` | Delete the character after the cursor |
| `←` / `→` | Move a character |
| `↑` / `↓` | Move a line, following wrapped lines |
| `home`, `ctrl+a` | Go to the start of the line |
| `end`, `ctrl+e` | Go to the end of the line |
| `alt+←`, `ctrl+←`, `alt+b` | Jump a word back (Option+← on macOS) |
| `alt+→`, `ctrl+→`, `alt+f` | Jump a word forward (Option+→ on macOS) |
| `alt+backspace`, `ctrl+w` | Delete the word before the cursor |
| `alt+delete` | Delete the word after the cursor |
| `pgup` / `pgdn` | Move a screen |
| `ctrl+home` / `ctrl+end` | Go to the start / end of the note |
| `tab` | In the main checkout: switch between this session's note and the main checkout's |
| `ctrl+x` | Check or uncheck the line's item, or make the line a `- [ ]` item |
| `ctrl+t` | Add a checklist item below |
| `ctrl+o` | Open the note in Cursor or VS Code |
| `ctrl+p` | Send the line's open item to [Tasks](#tasks), leaving a `↗` link in the note |
| `esc` | Stop editing |

An editor's integrated terminal (Cursor, VS Code) may keep some `ctrl` keys for itself. Every line and word key has a non-`ctrl` alternative.

## Run list

`r` lists what can run in the selected session's worktree: **Dev** first (then a row per [Dev process](features.md#several-dev-processes)), then your [actions](features.md#setup-and-actions), then a row per [shell](features.md#several-shells) and **+ New shell**. Every row follows one rule: `enter` starts what is stopped and shows what runs, and `x` twice stops it.

| Key | Action |
| --- | --- |
| `j`, `↓` / `k`, `↑` | Next / previous row |
| `g`, `home` / `G`, `end` | First / last row |
| `enter` | Start the row if it is stopped (an action from an untrusted `deckhand.json` is shown for review first); show it on its tab if it runs. On **+ New shell**: open another shell |
| `x` `x` | Stop the selected row (Dev, a Dev process or the running action). On an extra shell: close it. On the first shell: restart it, killing whatever is stuck in it ([restarting](features.md#terminal-git-and-dev-panes)) |
| `x` `x`, then `enter` | Restart anything else |
| `esc`, `h`, `←` | Close the run list |

The running action's row says `● running`. Starting Dev stops it in the repository's [other worktrees](features.md#dev-in-one-worktree-at-a-time).

## Agents

`G` lists the installed agents with their versions; [Agent versions and updates](features.md#agent-versions-and-updates) explains what an update runs.

| Key | Action |
| --- | --- |
| `j`, `↓` / `k`, `↑` | Next / previous agent |
| `enter` | Update the selected agent (asks first while it has running sessions) |
| `R` | Look the latest releases up again |
| `esc`, `h`, `←` | Back |

## Tasks

`b` opens the repository's task list; [Tasks](features.md#tasks) explains groups, steps and what ticks a task.

| Key | Action |
| --- | --- |
| `j`, `↓` | Select the next row |
| `k`, `↑` | Select the previous row |
| `g`, `home` | Select the first row |
| `G`, `end` | Select the last row |
| `a` | Add here: in a worktree's group a step in its note, else a task |
| `A` | Add a backlog task, wherever the selection is |
| `enter` | Edit the selected task (title, then `tab` for its details) |
| `enter` *(on a step)* | Open its note |
| `enter`, `space` *(on the older-done row)* | Show or hide the older done tasks |
| `space` | Tick the task done, or reopen it |
| `space` *(on a step)* | Tick or untick it in its note |
| `x` | Delete the task (press twice) |
| `J` | Move the task down within its group (not done tasks) |
| `K` | Move the task up within its group (not done tasks) |
| `n` | Start a session for a backlog task, in a new worktree |
| `w` | Move the task to the backlog, its note, a worktree or a session ([moving tasks](features.md#tasks)) |
| `o` | Select the session doing it |
| `v` | Show only the selected session's worktree, or everything again |
| `tab` | Switch to the Notes view, and back |
| `E` | Open the task list file in Cursor or VS Code |
| `pgdn` / `pgup`, `ctrl+d` / `ctrl+u` | Half a list down / up |
| `esc`, `h`, `←` | Back |

While editing a task, `enter` saves the title, `ctrl+s` saves from the details, and `esc` saves too (as in the notes editor); with an empty title it just closes.

**In the Notes view** (`tab`):

| Key | Action |
| --- | --- |
| `a` | Send the selected open item to the backlog as a task |
| `A` | Send every open item of the selected note |
| `enter` | Open the note in its session's Notes tab |
| `f` | Show only open items, or every line again |
| `v` | Show only this worktree's notes, or all of them |

`E` does nothing in the Notes view.

## Worktrees

`W` lists every worktree of the repository; [Worktrees](features.md#worktrees-w) explains the groups and what deleting checks.

| Key | Action |
| --- | --- |
| `j`, `↓` | Select the next worktree |
| `k`, `↑` | Select the previous worktree |
| `pgdn` / `pgup`, `ctrl+d` / `ctrl+u` | Move ten rows |
| `g`, `home` | Select the first worktree |
| `G`, `end` | Select the last worktree |
| `x` | Delete the selected worktree, asking first (on a missing one: prune or forget it) |
| `x` *(on the "Merged · safe to delete" heading)* | Delete every worktree in that group after one confirmation |
| `space` *(in a confirmation)* | Keep the note's open items in the note, or send them to the backlog |
| `J` / `K` *(in a confirmation)* | Scroll it when it does not fit (`pgdn` / `pgup` too) |
| `enter`, `o` | Select its session in the sidebar |
| `M` | Mark it merged, or clear the marker |
| `E` | Open it in Cursor or VS Code |
| `R` | Check them all again |
| `esc`, `h`, `←` | Back (in a DELETE prompt, `esc` goes back to the choices) |

## Git changes

On the Git tab, `→` (or `l`) browses the worktree's changes and the files its branch committed; [Git changes](features.md#git-changes) describes the groups, rows and diff preview.

| Key | Action |
| --- | --- |
| `j`, `↓` | Select the next file (across groups) |
| `k`, `↑` | Select the previous file |
| `g`, `home` | Select the first file |
| `G`, `end` | Select the last file |
| `space` | Stage the selected file, or unstage it if staged (staging a conflict marks it resolved; a committed file is left as it is) |
| `a` | Stage everything except conflicted files (press twice when a file is partly staged) |
| `A` | Unstage everything (press twice when a file is partly staged) |
| `enter`, `E` | Open the file in Cursor or VS Code at its first changed line |
| `J` | Scroll the diff preview down |
| `K` | Scroll the diff preview up |
| `pgdn` / `pgup`, `ctrl+d` / `ctrl+u` | Scroll the diff preview a page |
| `o` | Attach lazygit (commits, discards, hunks, branches, history) |
| `esc`, `←`, `h` | Back to the session list |
