# Manual smoke checklist

The automated suites cannot run real agents. This checklist is what a person runs before a release, or after a change the tests cannot cover. Every step gives the exact keys or commands and what you should see. Stop at the first step that does not match and note the step number.

**Isolation, read first.** Everything below runs in a dev sandbox (`~/.deckhand-rust-dev`) against a disposable repository under `$TMPDIR`. **Never** point these steps at `~/.deckhand`, and do not run `deckhand setup` without `--check`. Agents use their normal credentials and cost real money; keep prompts tiny ("say hi").

## 0. Setup

```sh
cd /path/to/deckhand                       # the checkout under test
cargo build --release -p deckhand          # target/release/deckhand (the launcher prefers release over debug)

export DECKHAND_DEV_HOME=~/.deckhand-rust-dev
export SMOKE=$(mktemp -d "${TMPDIR:-/tmp}/deckhand-smoke.XXXXXX")
export DH=$PWD/scripts/deckhand-dev.sh     # always start through the launcher

# a disposable repository with something to merge and run
git init -q -b main "$SMOKE/repo" && cd "$SMOKE/repo"
git config user.name smoke && git config user.email smoke@example.invalid && git config commit.gpgsign false
printf 'line one\nline two\nline three\n' > notes.txt
printf '{\n  "defaultWorkspace": "new",\n  "devCommand": "sh -c \x27echo dev up; sleep 600\x27",\n  "actions": {"hello": "echo hello from action"}\n}\n' > deckhand.json
git add . && git commit -q -m init
"$DH" status          # expected: `No isolated dev daemon running (...)`
```

Expected: the launcher prints `Deckhand dev · isolated state: <home>` with `~/.deckhand-rust-dev`, and `status` reports `No isolated dev daemon running`. Every later `$DH ...` is run from `$SMOKE/repo`. In a second terminal keep `tail -f ~/.deckhand-rust-dev/daemon.log` open: no panics or `ERROR` lines are expected during the whole run.

Sanity: `$DH --version` prints `deckhand <version>`. `ls ~/.deckhand` is never touched; `stat -f %m ~/.deckhand/state.json` (macOS) should be unchanged from before you started.

## 1. Launch and the three agents

1. Run `$DH`. Expected: the UI opens with the header **deckhand · DEV (isolated)**, an empty sidebar, tabs Preview / Terminal / Git / Dev / Notes.
2. Press `?`. Expected: the help guide opens with topics on the left. `Esc` closes it.
3. **Claude.** Press `n`, choose `claude` (Enter), type `smoke claude`, leave the workspace on **new worktree**, press Enter. If the form shows a trust review for `deckhand.json`, press Enter to trust it. Expected: the session appears as row 1 with a spinner, then `●` idle; the Preview tab shows Claude's screen (accept its own folder-trust prompt through attach, step 3). The details under the list show branch `smoke_claude` and the worktree under `~/.deckhand-rust-dev/worktrees/`.
4. **Pi.** Press `n`, choose `pi`, name `smoke pi`, Enter. Expected: a second row with `π`, its own worktree, Preview shows Pi's UI.
5. **Codex.** Press `n`, choose `codex`, name `smoke codex`, Enter. Expected: a third row with `◇`; if Codex is not installed the form says so and nothing is created.
6. Press `1`, `2`, `3`. Expected: the selection jumps to each numbered row and the Preview follows it.
7. `$DH status` from another terminal. Expected: `Isolated dev daemon running (protocol v49): <home>`.

## 2. Every tab

Select `smoke claude` (`1`) and walk the tabs with `]` / `[` and the letters.

| Keys | Expected |
| --- | --- |
| `p` | Preview: the agent's screen as plain text. `→` types to the agent: its colours and cursor appear, every key (Esc and Ctrl+C too) reaches it, the wheel scrolls; `Ctrl+]` leaves |
| `t` | Terminal: a shell in the session's worktree (`pwd` shows `~/.deckhand-rust-dev/worktrees/...`). The shell starts the first time you view the tab. `→` types into it (its colours and cursor appear); `Ctrl+]` leaves |
| `g` | Git: sections Merge Conflicts / Staged / Changes / Untracked, empty at first. In another terminal run `echo x >> <worktree>/notes.txt`; within about 2 s `notes.txt` appears under Changes with `+1 -0`. `→` focuses it; `space` stages (moves to Staged); `space` again unstages; `J`/`K` scroll the diff |
| `d` | Dev: shows "not running" or the empty pane. `d` `d` (twice quickly) starts the Dev command: output `dev up`, `▶` appears after the row title. `d` `d` again stops it |
| `a` | Notes: the worktree's note. `Enter` edits |
| `r` | Run list: **Dev** first, then `hello`. Choose `hello` and Enter: the Terminal tab switches to the action, header `hello ✓ exit 0`, output `hello from action`. `v` switches back to the shell |
| `r` `G` `k` `x` `x` | Run list, the row before last: **Shell**. First, on the Terminal tab, `o` and run `sh -c 'trap "" INT HUP; sleep 300'`, then `Ctrl+]` (`ctrl+c` won't stop it). `r`, `G`, `k`, `x`: the footer says `x again restarts the shell`; `x` again: after about 2 s the Terminal tab shows a fresh prompt and the footer says `Restarted the shell` |
| `r` `G` | Run list, last row: **+ New shell**. Enter: the Terminal header reads `shell 1 │ shell 2`, the second one shown. `→`, type `echo two` and Enter, `Ctrl+]`: its output stays there, and `v` shows the first shell without it. `r`, the **Shell 2** row, `x` `x`: `Closed shell 2` |

The sidebar lists the sessions under section headers (`Working`, `Idle`, …); while an agent works its row sits under **Working**, and moves to **Unread** when its turn ends. `V` shows them in your own order (the tree, `c` collapses) and `V` again brings the sections back. Also `<` / `>` narrow and widen the sidebar and `tab` / `shift+tab` cycle tabs. Select `smoke pi`: Terminal, Git, Dev and Notes are shared per worktree, so they show that worktree's own (not claude's) shell and changes.

## 3. Attach and detach (Ctrl+])

1. Select `smoke claude`, press `o` (or Enter on Preview). Expected: the UI gives way to Claude's full-screen TUI. Type `hi` and Enter; accept any folder-trust prompt. Replies stream live.
2. Press `Ctrl+]`. Expected: back in Deckhand, selection and tab unchanged, the session still running.
3. Repeat with `Ctrl+Space` (also detaches).
4. On the Terminal tab press `o`: you are in the worktree shell; run `echo $0 && pwd`; `Ctrl+]` returns. On Git press `o`: lazygit opens if installed (`q` quits it; it restarts on the next `o`). On Dev press `o` while Dev runs: you see its output; `Ctrl+]` returns and Dev keeps running.
5. Scroll the wheel while attached: the multiplier applies (Settings → Scroll speed changes it).

## 4. Forks and sub-sessions

1. With `smoke claude` selected and a message exchanged, press `N`. Choose `claude` (clean): a `↳` child appears indented in the parent's worktree. Expected: fresh context.
2. Press `N` again and choose **⑂ Fork parent**. Expected: a `⑂` child starts with the parent's conversation (`claude --resume <parent> --fork-session ...`); nothing is typed into it. Press `tab` in the form to see the new/existing worktree choice: a fork into another worktree makes the child's first message say where it now is.
3. Select the `smoke pi` row and `N` then **⑂ Fork parent**: a Pi fork starts (`pi --fork`). Select `smoke codex`, `N`: **Fork parent** is offered only once Codex reported its conversation ID (otherwise it says why).
4. `c` on a parent collapses and expands its subtree; `+N` shows hidden children. `J` / `K` reorder siblings; relaunch (step 14) must keep the order.

## 5. Notes, including an external vim edit

1. Select `smoke claude`, tab `a`, press `Enter`. Type `- [ ] first item`, Enter, `second line`. Expected: `- [ ]` renders as ☐; the sidebar shows an open count. `ctrl+x` on the first line ticks it (☑). `Esc` stops editing.
2. Press `o` on the Notes tab. Expected: your `$VISUAL` / `$EDITOR` (else nvim/vim/vi) opens the note full screen. Add a line `- [ ] from vim` (in vim, `Enter` after a checklist line continues it), `:wq`. Expected: you return to the Notes tab and the new line is shown.
3. From a second terminal append to the note while Deckhand shows it: `echo '- [ ] external' >> "$(ls ~/.deckhand-rust-dev/notes/worktrees/*.md | head -1)"` (pick the file of the worktree you are viewing). Expected: within about a second the item appears in Deckhand. Now start typing in Deckhand and, in the second terminal, change the file again: Deckhand must reload and say so, never overwrite your external edit.
4. `E` opens the note in Cursor or VS Code if installed (skip otherwise).

## 6. Tasks

1. Press `b`. Expected: the Tasks board; the claude worktree is selected with its checklist as steps.
2. Press `a`, type `Write the README`, Tab, add details, `ctrl+s`. Expected: a backlog task; the header shows `☐ N tasks · b`.
3. Select it and press `n`. Expected: the new-session form with *Task:* and *Base:*; `↑`/`↓` pick the base branch. Choose `pi`, Enter. Expected: a new worktree session; the board shows the task **In progress** with the session's state; its Notes tab starts with *◆ Task*; once the agent settles the task text is typed into its input but not sent.
4. In a note, put the cursor on a `- [ ]` line and press `ctrl+p`. Expected: it becomes `↗ ... · in Tasks` and appears in the backlog.
5. `space` ticks a task, `v` filters to this worktree, `x` `x` deletes. `Esc` leaves.

## 7. Merge with a conflict

1. Select `smoke claude`'s worktree. On its Terminal tab: `printf 'line one\nLINE TWO FROM BRANCH\nline three\n' > notes.txt && git add -A && git commit -qm branch-change`.
2. In `$SMOKE/repo` (another terminal, the main checkout): `printf 'line one\nline two from main\nline three\n' > notes.txt && git commit -qam main-change`.
3. Back in Deckhand press `m`. Expected: the confirmation shows `Into main · <path>` (no warning colour, the main checkout), `1 commit`, the diff stat. Confirm.
4. Expected: "Merged with conflicts"; `notes.txt` is listed. Press `Enter` to keep the merge in progress. On the Git tab, group **Merge Conflicts** lists `notes.txt` with `U`. In `$SMOKE/repo` `git status` shows `You have unmerged paths`.
5. Resolve it in the main checkout (`printf 'line one\nresolved\nline three\n' > notes.txt && git add notes.txt && git commit -qm merged`). Expected: every session of that worktree shows `✓` merged (marked at the conflict; `M` toggles the marker).
6. Repeat steps 1 to 3 with a new worktree session and, at the conflict prompt, press `x`. Expected: the merge is aborted, `git status` in the main checkout is clean and the session is not marked merged.
7. A conflict-free merge: commit a new file in the `smoke pi` worktree and `m`: the preview offers a `space` toggle to commit uncommitted files first; confirm; `git log` in the main checkout shows the merge staged (not committed) for you to review.

## 8. Worktree manager and deletion

1. Press `W`. Expected: the worktrees grouped (**Merged · safe to delete**, **Merged · has leftovers**, **In progress**, **Idle**, **Missing**, **Main checkout**), each with sessions, uncommitted files and commits.
2. Dirty a worktree (`echo y > $WT/untracked.txt` in its shell) and press `x` on it. Expected: a branch keep/delete choice, then, because of the untracked file, a screen that requires typing `DELETE`. `Esc` cancels; the worktree is untouched.
3. Select the merged worktree from step 7, `x`, choose delete worktree and branch. Expected: the sessions running there are named and stopped first; afterwards the directory under `~/.deckhand-rust-dev/worktrees/` is gone, `git worktree list` in the repo no longer lists it, and its sessions are archived (`f` then `A` shows them).
4. `x` on the main checkout row or a worktree used by another session: refused with a reason, never offered.
5. `x` on a running session (sidebar): asks stop only / stop and delete worktree / delete worktree and branch. Choose stop only; the session shows `○` exited and keeps its worktree. `X` force-stops without ever authorising data loss.

## 9. Settings and trust

1. Press `C`. Expected: a grid with a **Global** and a **This repo** column; the repo's `deckhand.json` values appear in This repo with `⚠ needs trust` until trusted, `●` marks what applies now.
2. On This repo choose **Default agent**, Enter, pick `pi`. Expected: saved to `$SMOKE/repo/deckhand.json`; the status says review required (the file is no longer the trusted bytes).
3. Press `←`, select **Dev command**, Enter, type `sh -c 'echo global dev; sleep 600'`. Expected: saved under `defaults` in `~/.deckhand-rust-dev/config.json` (check with `cat`).
4. `E` opens the selected column's raw JSON; type a syntax error and `ctrl+s`: it must refuse and leave the file untouched; `ctrl+f` formats; `Esc` returns.
5. Press `T`: the review shows the exact bytes of `deckhand.json`; `Enter` trusts it, returning to Settings with the row no longer marked. Edit `deckhand.json` outside Deckhand (`echo '' >> deckhand.json`): the next `n` or `d` `d` asks for review again.
6. **Agents**: in Settings go to Agents, switch **Agent signals** on. Expected: a Codex hooks warning if `~/.codex/hooks.json` does not call this Deckhand, naming `deckhand hooks codex`.

## 10. Help search

Press `?`, then `/` and type `merge`. Expected: topics narrow to those mentioning it; `Enter` keeps the search and gives letters back; `PgDn` scrolls; `Esc` clears the search, `Esc` again closes. Also: `G` shows the Agents screen with installed vs latest versions (latest may read "unknown" without `npm`); `Esc` leaves.

## 11. 60-column resize

1. Shrink the terminal window to 60 columns (or run `stty cols 60` is not enough; resize the window). Expected: the UI redraws without garbage: the sidebar narrows, the pane keeps working, no panic, the footer fits.
2. Open each tab and Settings (`C`), Tasks (`b`) and Worktrees (`W`) at 60 columns: text truncates with `…` rather than wrapping into the next row.
3. Attach (`o`) at 60 columns, run `stty size` in the Terminal tab shell: it reports the full terminal size. Detach and resize back to 120 columns: Preview re-wraps to the new width.

## 12. Archive, search, filter

`A` archives the selected session (the agent keeps running if it was). `f` then `A` shows archived, `f f` returns to active. `/` then a title fragment filters the list; `Esc` clears it. `!` jumps to the next session that needs you.

## 13. Exit and resume

1. Attach to `smoke pi`, send `/quit` (or Ctrl+C twice) so the agent exits. Expected: the row turns `○`, the Preview keeps the last screen, and `s` resumes the same conversation (`--session-id <same uuid>`), `S` starts a fresh one.
2. Repeat for Claude: after exit the preview ends with the resume hint; `s` runs `claude --resume <uuid>`.
3. Codex: `s` resumes (`codex resume <id>`) only once its ID was captured; otherwise it refuses and asks for `S`.

## 14. Relaunch persistence

1. Press `q` in the UI. Expected: the UI exits; `$DH status` still says the daemon is running; `pgrep -f "deckhand.*--session-worker"` still lists the workers.
2. Run `$DH` again. Expected: the same sessions, the same selected session and tab, sibling order, filter, collapsed state and notes; running agents are still live (their Preview shows current output).

## 15. Recovery after `kill -9` of the daemon

1. Note which sessions are `●`. In another terminal: `kill -9 "$(cat ~/.deckhand-rust-dev/daemon.pid)"`.
2. In the UI (if open) the connection drops: it reports the daemon is gone or exits. Run `$DH` again. Expected: a new daemon starts by itself (stale socket removed because no live PID), `daemon.log` shows the start, and all previously running sessions are listed as `○` exited with their last preview and conversation references intact; Dev shows not running.
3. `ps -axo pid,command | grep -- '--session-worker'` shows no worker of the dev home left running (any survivor of the old daemon exits when its channel closes). If one stays, note its PID.
4. `s` on a Claude session resumes the same conversation; `S` makes a fresh one. State files are intact: `python3 -m json.tool ~/.deckhand-rust-dev/state.json >/dev/null` succeeds.
5. A daemon speaking another protocol version is refused, not replaced. This needs a daemon from an older build; skip it unless you have one in the dev home.

## 16. Hooks and setup

1. `deckhand hooks codex` (use `target/release/deckhand hooks codex`): prints JSON with `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `PermissionRequest`, `Stop` and `Interrupt` (no `Notification`, no `StopFailure`), each command of the form `'<path to deckhand>' 'hook'`. `target/release/deckhand hooks` prints the Claude-style variant (adds `StopFailure` and `Notification`, no `Interrupt`). Nothing is written anywhere.
2. `target/release/deckhand doctor --check` and `target/release/deckhand setup --check`: read-only; they list claude, pi and codex as found or missing and exit 0. With a throwaway Codex home containing a hook that calls `node /x/dist/cli.js hook` (`mkdir -p $SMOKE/codex && printf '{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"node /x/dist/cli.js hook"}]}]}}' > $SMOKE/codex/hooks.json`) run `CODEX_HOME=$SMOKE/codex target/release/deckhand doctor --check`: it reports the stale TypeScript bridge and tells you to regenerate with `deckhand hooks codex`. It never edits the file.
3. `$DH setup` without `--check` must refuse ("The isolated launcher does not install global agent tools").
4. With signals on for Claude (default) a Claude session that asks a permission question shows `?` needs-input in the sidebar and the details name the command; answering it clears the flag. Hooks are posted by `deckhand hook` to the dev daemon only.

## 17. Cleanup

```sh
$DH stop                                    # stops the dev daemon and its sessions
git -C "$SMOKE/repo" worktree list          # leftover worktrees were created under ~/.deckhand-rust-dev/worktrees
rm -rf "$SMOKE"
rm -rf ~/.deckhand-rust-dev                 # only after every worktree above is removed through the UI or `git worktree remove`
```

Expected: `$DH status` says not running. `~/.deckhand` was never modified.
