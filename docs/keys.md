# Keys

Every Deckhand key, screen by screen; press `?` in Deckhand for the in-app guide. Back to the [README](../README.md).

## Session list

**Layout**: Deckhand has two places, the session list on the left and the pane on the right. `→` or `l` steps into the pane, and `←`, `h` or `esc` steps back out. The same keys mean the same thing on every screen: `enter` opens or chooses, `space` toggles and `E` opens your editor. In lists, `a` adds and `x` removes. On the session list itself, `a` opens the Notes tab and `x` stops a session. `o` opens full screen.

| Key | Action |
| --- | --- |
| `j`, `↓` | Select the next visible session |
| `k`, `↑` | Select the previous visible session |
| `1`–`9` | Select that numbered session |
| `0` | Select session 10 (while 10 or fewer are visible) |
| a number | With more than 10 visible, type it: `enter` or a short pause selects, `esc` clears |
| `/` | Search titles, notes, tasks, agent, branch and path |
| `f` | Open the filter menu: `a` active, `r` running, `!` attention, `e` exited, `A` archived, `*` all |
| `f` `f` | Go back to the active filter |
| `!` | Select the next session that needs you, switching to the attention filter |
| `J` | Move the selected session down among its siblings |
| `K` | Move the selected session up among its siblings |
| `c` | Cycle its sub-sessions: hide the exited ones, collapse, expand |
| `<` | Narrow the sidebar |
| `>` | Widen the sidebar |
| `tab`, `]` | Next tab: Preview, Terminal, Git, Dev, Notes |
| `shift+tab`, `[` | Previous tab |
| `p` | Show the Preview tab |
| `t` | Show the Terminal tab |
| `g` | Show the Git tab |
| `d` | Show the Dev tab |
| `d` `d` | Start or stop the worktree's Dev command (press `d` twice quickly) |
| `a` | Show the Notes tab |
| `→`, `l` | Step into the pane: scroll the preview, browse [Git changes](#git-changes) or [edit the notes](#notes-editor) |
| `←`, `h`, `esc` | Step back out to the session list |
| `enter`, `o` | Open full screen what the tab shows: the agent, the shell or action, lazygit or Dev |
| `enter` *(on Notes)* | Edit the note here |
| `o` *(on Notes)* | Edit the note in your terminal editor ([in vim](features.md#notes)) |
| `v` *(on Terminal)* | Switch between the shell and the worktree's last action |
| `E` | Open the session's directory in Cursor or VS Code (on Notes: the note) |
| `n` | New top-level session |
| `N` | New sub-session: clean, a `⑂` fork or `↳` from the handoff ([sub-sessions](features.md#sessions-and-sub-sessions)) |
| `x` | Stop the selected session, asking first (and whether to delete its worktree and branch) |
| `X` | Force-stop the selected session at once, keeping its worktree |
| `s` | Resume the selected exited session (or retry a failed setup) |
| `S` | Restart the selected exited session with a fresh conversation |
| `space` | Mark the selected session done or not done (`☑`) |
| `m` | Merge the selected worktree into the current branch, uncommitted ([merging](features.md#merging-and-done)) |
| `M` | Set or clear the worktree's merged marker without merging |
| `r` | Open the run list: the Dev command first, then your actions ([actions](features.md#setup-actions-and-creation-hooks)) |
| `A` | Archive or unarchive the selected session (it keeps running) |
| `backspace` | Archive the selected exited session; on an archived one, remove it for good ([archiving](features.md#organizing-and-inspecting-sessions)) |
| `H` | Export and open a handoff ([handoffs](features.md#resume-and-handoffs)) |
| `i` | Show the worktree's Git summary and PR lookups ([inspecting](features.md#organizing-and-inspecting-sessions)) |
| `b` | Open the repository's task list ([Tasks](#tasks)) |
| `W` | Open the Worktrees screen ([Worktrees](#worktrees)) |
| `C` | Open Settings ([Settings](features.md#settings-c)) |
| `U` | Open the Agents screen ([versions and updates](features.md#agent-versions-and-updates)) |
| `R` | Reload the session list (it updates live, so this is rarely needed) |
| `?` | Open the in-app guide |
| `q` | Quit the UI; running sessions continue in the daemon |
| `ctrl+c` | Quit from any screen (in a Settings edit, the first press acts as `esc`) |

`q` works only on the session list: every other screen closes with `esc`.

**Scrolling the preview** (`→` on the Preview tab of a running session):

| Key | Action |
| --- | --- |
| `j`, `↓` | Scroll down |
| `k`, `↑` | Scroll up |
| `pgdn` / `pgup` | Scroll a page |
| `g` / `G` | Go to the top / bottom |
| mouse wheel | Scroll |
| `+` (or `=`) / `-` | Raise / lower the wheel speed ([scroll sensitivity](features.md#attach-scroll-sensitivity)) |

**In the guide** (`?`): `↑`/`↓` or `1`–`9` pick a topic, `/` searches every topic, `pgup`/`pgdn` scroll and `esc` closes.

> [!NOTE]
> Deletion is conservative. Unknown or unsafe Git state needs `DELETE` typed, and `X` never authorises data loss. The protections for the main checkout, the current worktree and an actively shared worktree can't be overridden. See [Cleanup](features.md#cleanup).

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
| `◌` | Before the title: running, activity unknown |
| `○` | Before the title: exited |
| `?` | Before the title: needs you (agent signals) |
| `◆` | Before the title: response ended, unread until you attach to the agent (agent signals) |
| `!` | Before the title: failed (red) or interrupted (yellow) |
| `⌛` | Before the title: rate-limited (agent signals) |
| `▾` / `▸` | Parent with sub-sessions, expanded / collapsed (`c`) |
| `↳` | Clean sub-session |
| `⑂` | Forked sub-session |
| `▶` | After the title: the worktree's Dev command runs (on its first row) |
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
| `! N` | Header: N sessions need you (`!` jumps to them) |
| `╎N` | Details: N other sessions share the worktree |
| `☑2d` | Details: done 2 days ago |

Archived rows are dimmed. The archived view (`f` `A`) lists only archived sessions. Done rows have a muted title, while their markers and status stay readable. Below the branch, the details repeat the row's markers as glyphs (`╎N`, `▶`, `✓`, `☑2d`, `▣`), then `☐ N open` checklist items. A long branch is shortened to keep them. When `↑` shows, the details line names both versions, for example `✶ claude 2.1.287 · 2.1.290 installed`. In the app, `?` → Sidebar lists all of this.

## Attach

`enter` or `o` attaches to the selected pane: the agent, the shell, lazygit, Dev or a running action. An attached agent gets a Deckhand status line on its top row: the session, how many other sessions need you and the detach key.

| Key | Action |
| --- | --- |
| any other key | Sent to the attached pane |
| `ctrl+space` | Detach and return to Deckhand |
| `ctrl+]` | Detach and return to Deckhand |
| mouse wheel | Scroll the pane, damped by the [scroll sensitivity](features.md#attach-scroll-sensitivity) |

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
| `esc` | Back |

While editing a task, `enter` saves the title, `ctrl+s` saves from the details and `esc` cancels.

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
| `pgdn` / `pgup` | Move ten rows |
| `g`, `home` | Select the first worktree |
| `G`, `end` | Select the last worktree |
| `x` | Delete the selected worktree, asking first (on a missing one: prune or forget it) |
| `x` *(on the "Merged · safe to delete" heading)* | Delete every worktree in that group after one confirmation |
| `space` *(in a confirmation)* | Keep the note's open items in the note, or send them to the backlog |
| `enter`, `o` | Select its session in the sidebar |
| `M` | Mark it merged, or clear the marker |
| `E` | Open it in Cursor or VS Code |
| `R` | Check them all again |
| `esc` | Back |

## Git changes

On the Git tab, `→` (or `l`) browses the worktree's changes; [Git changes](features.md#git-changes) describes the groups, rows and diff preview.

| Key | Action |
| --- | --- |
| `j`, `↓` | Select the next file (across groups) |
| `k`, `↑` | Select the previous file |
| `g`, `home` | Select the first file |
| `G`, `end` | Select the last file |
| `space` | Stage the selected file, or unstage it if staged (staging a conflict marks it resolved) |
| `a` | Stage everything except conflicted files |
| `A` | Unstage everything |
| `enter`, `E` | Open the file in Cursor or VS Code at its first changed line |
| `J` | Scroll the diff preview down |
| `K` | Scroll the diff preview up |
| `pgdn` / `pgup` | Scroll the diff preview a page |
| `o` | Attach lazygit (commits, discards, hunks, branches, history) |
| `esc`, `←`, `h` | Back to the session list |
