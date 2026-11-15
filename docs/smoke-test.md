# Manual smoke checklist

The automated suites cannot run real agents. This checklist is what a person runs before a release, or after a change the tests cannot cover. Every step gives the exact keys or commands and what you should see. Stop at the first step that does not match and note the step number.

**Isolation, read first.** Everything below runs in the dev namespace (`~/.deckhand-dev`) against a disposable repository under `$TMPDIR`. To keep this run apart from the dev namespace you use day to day, `export DECKHAND_DEV_HOME=<another directory>` before step 0 and read that directory wherever this page says `~/.deckhand-dev`. **Never** point these steps at `~/.deckhand`, and do not run `deckhand setup` without `--check`. Agents use their normal credentials and cost real money; keep prompts tiny ("say hi").

## 0. Setup

```sh
cd /path/to/deckhand                       # the checkout under test
cargo build --release -p deckhand          # target/release/deckhand (the launcher prefers release over debug)

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

Expected: the launcher prints `Deckhand dev · isolated state: <home>` with `<home>` your `~/.deckhand-dev`, and `status` reports `No isolated dev daemon running`. Every later `$DH ...` is run from `$SMOKE/repo`. In a second terminal keep `tail -f ~/.deckhand-dev/daemon.log` open: no panics or `ERROR` lines are expected during the whole run.

Sanity: `$DH --version` prints `deckhand <version>`. `ls ~/.deckhand` is never touched; `stat -f %m ~/.deckhand/state.json` (macOS) should be unchanged from before you started.

## 1. Launch and the three agents

1. Run `$DH`. Expected: the UI opens with the header **deckhand · DEV (isolated)**, an empty sidebar, tabs Preview / Terminal / Git / Dev / Notes.
2. Press `?`. Expected: the help guide opens with topics on the left. `Esc` closes it.
3. **Claude.** Press `n`, choose `claude` (Enter), type `smoke claude`, leave the workspace on **new worktree**, press Enter. If the form shows a trust review for `deckhand.json`, press Enter to trust it. Expected: the session appears as row 1 with a spinner, then `●` idle; the Preview tab shows Claude's screen (accept its own folder-trust prompt through attach, section 3). The details under the list show branch `smoke_claude` and the worktree under `~/.deckhand-dev/worktrees/`.
4. **Pi.** Press `n`, choose `pi`, name `smoke pi`, Enter. Expected: a second row with `π`, its own worktree, Preview shows Pi's UI.
5. **Codex.** Press `n`, choose `codex`, name `smoke codex`, Enter. Expected: a third row with `◇`; if Codex is not installed the form says so and nothing is created.
6. Press `1`, `2`, `3`. Expected: the selection jumps to each numbered row and the Preview follows it.
7. `$DH status` from another terminal. Expected: `Isolated dev daemon running (protocol v<N>): <home>`, where `<N>` is `PROTOCOL_VERSION` in `crates/protocol/src/lib.rs` (63 in 3.3.0).

## 2. Every tab

Select `smoke claude` (`1`) and walk the tabs with `]` / `[` and the letters.

| Keys | Expected |
| --- | --- |
| `p` | Preview: the agent's screen as plain text. `→` types to the agent: its colours and cursor appear, every key (Esc and Ctrl+C too) reaches it, the wheel scrolls; `Ctrl+]` leaves |
| `t` | Terminal: a shell in the session's worktree (`pwd` shows `~/.deckhand-dev/worktrees/...`). The shell starts the first time you view the tab. `→` types into it (its colours and cursor appear); `Ctrl+]` leaves |
| `g` | Git: sections Merge Conflicts / Staged / Changes / Untracked, empty at first. In another terminal run `echo x >> <worktree>/notes.txt`; within about 2 s `notes.txt` appears under Changes with `+1 -0`. `→` focuses it; `space` stages (moves to Staged); `space` again unstages; `J`/`K` scroll the diff |
| `d` | Dev: shows "not running" or the empty pane. `D` starts the Dev command: output `dev up`, `▶` appears after the row title. `D` again stops it |
| `a` | Notes: the worktree's note. `Enter` edits |
| `r` | Run list: **Dev** first, then `hello`. Choose `hello` and Enter: the Terminal tab switches to the action, header `hello ✓ exit 0`, output `hello from action`. `v` switches back to the shell |
| `r` `G` `k` `x` `x` | Run list, the row before last: **Shell**. First, on the Terminal tab, `o` and run `sh -c 'trap "" INT HUP; sleep 300'`, then `Ctrl+]` (`ctrl+c` won't stop it). `r`, `G`, `k`, `x`: the footer says `x again restarts the shell`; `x` again: after about 2 s the Terminal tab shows a fresh prompt and the footer says `Restarted the shell` |
| `r` `G` | Run list, last row: **+ New shell**. Enter: the Terminal header reads `shell 1 │ shell 2`, the second one shown. `→`, type `echo two` and Enter, `Ctrl+]`: its output stays there, and `v` shows the first shell without it. `r`, the **Shell 2** row, `x` `x`: `Closed shell 2` |

The sidebar lists the sessions in your own order (the tree, `c` collapses). `V` shows them under section headers (`Working`, `Idle`, …); while an agent works its row sits under **Working**, and moves to **Unread** when its turn ends. `c` folds **Exited** to its header (`Exited N ▸`) and `c` again unfolds it. `V` again brings your own order back. Also `<` / `>` narrow and widen the sidebar and `tab` / `shift+tab` cycle tabs. Select `smoke pi`: Terminal, Git, Dev and Notes are shared per worktree, so they show that worktree's own (not claude's) shell and changes.

## 3. Attach and detach (Ctrl+])

1. Select `smoke claude`, press `o` (or Enter on Preview). Expected: the UI gives way to Claude's full-screen TUI. Type `hi` and Enter; accept any folder-trust prompt. Replies stream live.
2. Press `Ctrl+]`. Expected: back in Deckhand, selection and tab unchanged, the session still running.
3. Repeat with `Ctrl+Space` (also detaches).
4. On the Terminal tab press `o`: you are in the worktree shell; run `echo $0 && pwd`; `Ctrl+]` returns. On Git press `o`: lazygit opens if installed (`q` quits it; it restarts on the next `o`). On Dev press `o` while Dev runs: you see its output; `Ctrl+]` returns and Dev keeps running.
5. Scroll the wheel while attached: the multiplier applies (Settings → Scroll speed changes it).

## 4. Forks and sub-sessions

1. With `smoke claude` selected and a message exchanged, press `N`. Choose `claude` (clean): a `↳` child appears indented in the parent's worktree. Expected: fresh context.
2. Press `N` again and choose **⑂ Fork parent**. Expected: a `⑂` child starts with the parent's conversation (`claude --resume <parent> --fork-session ...`); nothing is typed into it. Press `tab` in the form to see the new/existing worktree choice: a fork into another worktree makes the child's first message say where it now is.
3. Select the `smoke pi` row and `N` then **⑂ Fork parent**: a Pi fork starts (`pi --fork`). Select `smoke codex`, `N`, **⑂ Fork parent**: the form keeps it in the parent's worktree (`tab` does nothing). Creating it is refused, with the reason, until Codex has reported its conversation ID.
4. Select `smoke claude` and press `n`: the agents are followed by **⑂ Fork smoke claude**. Choose it and name it. Expected: a top-level `⑂` row (not indented) starts with the conversation (`claude --resume <source> --fork-session ...`), its details say `⑂ fork of smoke claude`, and `s` after it exits resumes the fork's own conversation.
5. `c` on a parent collapses and expands its subtree; `+N` shows hidden children. `J` / `K` reorder siblings; relaunch (section 17) must keep the order.
6. **Agent commands.** Attach to `smoke claude` and ask: *Use deckhand to start a sub-session that lists the files in its worktree, and add "check the list" as a step.* Expected: Claude loads its `deckhand` skill (or runs `deckhand guide`), then runs `deckhand new --prompt …` and `deckhand step add …` without a permission prompt. A `↳` child appears under it in a new worktree and starts on the prompt without you typing it; `smoke claude`'s Notes tab now starts with a line named after it, with the step `☐ check the list` under it. Ask it to `deckhand kill` the child: it ends, its worktree is kept. Ask it to kill `smoke pi`: refused (*Only the sessions under this session (its sub-sessions, at any depth) can be stopped or removed from it*). Repeat the first request in `smoke pi` and `smoke codex`; Codex may ask before each command, as its own approval settings say (Deckhand can't pre-allow them there). Note whether Codex's sandbox lets the command reach the daemon.

## 5. Notes, including an external vim edit

1. Select `smoke claude`, tab `a`, press `Enter`. Type `- [ ] first item`, Enter, `second line`. Expected: `- [ ]` renders as ☐; the sidebar shows an open count. `ctrl+x` on the first line ticks it (☑). `Esc` stops editing.
2. Press `o` on the Notes tab. Expected: your `$VISUAL` / `$EDITOR` (else nvim/vim/vi) opens the note full screen. Add a line `- [ ] from vim` (in vim, `Enter` after a checklist line continues it), `:wq`. Expected: you return to the Notes tab and the new line is shown.
3. From a second terminal append to the note while Deckhand shows it: `echo '- [ ] external' >> ~/.deckhand-dev/notes/sessions/<session id>.md` (the session you are viewing; `E` on its note shows the path). Expected: within about a second the item appears in Deckhand. Now start typing in Deckhand and, in the second terminal, change the file again: Deckhand must reload and say so, never overwrite your external edit.
4. `E` opens the note in Cursor or VS Code if installed (skip otherwise).

## 6. Tasks

1. Press `b`. Expected: the Tasks board; `smoke claude`'s line (made by the step its agent added in section 4) with `☐ check the list` under it and `0/1`.
2. Press `A`, type `Write the README`, Tab, write a note, `ctrl+s`. Expected: a top-level line nobody is on (the backlog); `a` on it adds `outline` under it, and it shows `0/1`.
3. Select it and press `n`. Expected: the new-session form with *Task:* and *Base:*; `↑`/`↓` pick the base branch. Choose `pi`, Enter. Expected: a new worktree session; the board shows it on the line with its state; its Notes tab starts with `◆ Write the README · 0/1 steps` and shows the note you wrote as the line's note; once the agent settles the line's title and note are typed into its input but not sent.
4. In that session's own note (`a`, then Enter: the cursor starts in this session's note), type `- [ ] add a licence`, and with the cursor on it press `ctrl+p`. Expected: it leaves the note and appears as a step of the line (`0/2`).
5. `space` on both steps: each ticks, and the line ticks itself with the last (`2/2`); `space` on one reopens the line. `v` shows only the selected session's line, `x` `x` removes a line with its lines. `Esc` leaves.

## 7. Merge with a conflict

1. Select `smoke claude`'s worktree. On its Terminal tab: `printf 'line one\nLINE TWO FROM BRANCH\nline three\n' > notes.txt && git add -A && git commit -qm branch-change`.
2. In `$SMOKE/repo` (another terminal, the main checkout): `printf 'line one\nline two from main\nline three\n' > notes.txt && git commit -qam main-change`.
3. Back in Deckhand press `m`. Expected: the confirmation shows `Into main · <path>` (no warning colour, the main checkout), `1 commit`, the diff stat. Confirm.
4. Expected: "Merged with conflicts"; `notes.txt` is listed. Press `Enter` to keep the merge in progress. On the Git tab, group **Merge Conflicts** lists `notes.txt` with `U`. In `$SMOKE/repo` `git status` shows `You have unmerged paths`.
5. Resolve it in the main checkout (`printf 'line one\nresolved\nline three\n' > notes.txt && git add notes.txt && git commit -qm merged`). Expected: every session of that worktree shows `✓` merged (marked at the conflict; `M` toggles the marker).
6. Repeat steps 1 to 3 with a new worktree session and, at the conflict prompt, press `x`. Expected: the merge is aborted, `git status` in the main checkout is clean and the session is not marked merged.
7. A conflict-free merge: commit a new file in the `smoke pi` worktree and `m`: the preview offers a `space` toggle to commit uncommitted files first; confirm; `git log` in the main checkout shows the merge staged (not committed) for you to review.

## 8. Stacked sub-sessions

A sub-session in a new worktree whose parent has a worktree of its own starts from the parent's branch and merges back into it. The sessions above are merged by now, which keeps them flat, so start a fresh parent.

1. Press `n`, choose `claude`, name it `stack parent`, leave **new worktree**, Enter. Expected: a new row whose details show branch `stack_parent`. (Claude may ask to trust the new folder: `o`, accept, `Ctrl+]`.)
2. With `stack parent` selected press `N`, choose `claude`, type `stack child`. Expected: the form says *Workspace: same as parent (…)*. Press `tab` once. Expected: *Workspace: new worktree*, then *Base: stack_parent · stacked*, then *Merges back into stack_parent, the parent's branch*. Press Enter.
3. Expected: a `↳` row `stack child` under `stack parent`, in its own worktree; its details say `→ into stack_parent`. The `stack parent` row shows `0/1 in` after its title.
4. Select `stack child`, Terminal tab (`t`), `→`, run `echo stacked > stacked.txt && git add -A && git commit -qm stacked-change`, `Ctrl+]`.
5. Press `m`. Expected: the confirmation's first line is `Into stack_parent → main · <stack parent's worktree path>` (`stack_parent` bold, `→ main` muted), `1 commit`, and the footer offers `t into main`.
6. Press `t`. Expected: the line becomes `Into main · <the $SMOKE/repo path>` and the footer offers `t into stack_parent`. Press `t` again: back to `Into stack_parent → main`.
7. Choose **Merge** (Enter). Expected: `stack child` shows `✓`, and `stack parent`'s row now shows `1/1 in`. In `stack parent`'s worktree, `git status` shows `stacked.txt` staged, not committed.
8. **Flat from a command.** `$DH ls` and note `stack parent`'s ID (first column). Run `$DH new --parent <that id> --agent claude --title "flat child" --flat`. Expected: it prints the new session; a `↳ flat child` row appears under `stack parent` in a new worktree; its details have no `→ into` line, and `stack parent` still shows `1/1 in` (a flat child is not counted).

## 9. The orchestrator

The orchestrator and every session its plan creates are real Claude sessions: they start working at once and use your Claude usage. Keep the idea tiny.

1. On the session list press `V` until the list shows section headers. Press `O`. Expected: a one-line prompt at the bottom: *What do you want to do?*.
2. Type `Add hello.txt containing hello, and bye.txt containing bye` and Enter. Expected: the status line says `◎ <idea> is starting · enter attaches once it runs` (the session is named after the idea); a new row marked `◎` is selected, under the **Orchestrator** header. Its details say `◎ orchestrator · plans and dispatches sessions`.
3. Once its spinner stops, press `enter` to attach (accept Claude's folder-trust prompt if it shows). Answer its questions, then say: *Plan it as two sessions, one per file, nothing parked, and apply the plan.*
4. Expected: when it runs `deckhand plan apply`, Deckhand detaches you by itself and shows **Create this plan?**: `A plan for ◎ <idea>: 2 sessions.`, the two sessions as a tree, the start of the idea's brief and when it expires (15 minutes).
5. Press `enter`. Expected: the status says `Plan approved: creating 2 sessions`, and you are attached to the orchestrator again; its command prints what was created. `Ctrl+]`. Two `↳` sub-sessions are listed under a muted `◎ <idea> ┄┄┄` line, each in a new worktree, already working on its brief (answer any permission prompts they show). Their details say `↳ sub-session of ◎ <idea>`.
6. Press `b`. Expected: a top-level line named after the idea, with the orchestrator on it, `0/2`, and `✎` (it has a brief); under it a line per session with that session and its state, each also with `✎`. `Esc`. On one of the two sessions' Notes tab (`a`): `▸ Brief · from ◎ <idea> · <date>` under its line.
7. Attach to the orchestrator again and ask: *Add one more session that adds a README line, and apply it.* When **Create this plan?** shows, press `esc`. Expected: the status says `Plan cancelled: nothing was created`; no new session appears; the orchestrator is told `Cancelled in Deckhand: nothing was created.`

Leave the two sub-sessions running: section 10 uses them.

## 10. Messages between sessions

1. `$DH ls` and note `smoke claude`'s ID. With `smoke claude` idle (`●`), run `$DH send <id> "Say only: got it"`. Expected: it prints `Message <message id> queued for "smoke claude": it is typed in once that session is idle at its input. It can answer with deckhand reply <message id>.` (or `typed into "smoke claude"`). Within a few seconds its Preview shows the text typed and submitted under `[Message <message id> from you (the user) via deckhand]`, and Claude answers.
2. **Held while busy.** Prepare `$DH send <id> "Say only: second"` in the second terminal without pressing Enter. Attach to `smoke claude`, send `Write a 400-word story about a lighthouse`, `Ctrl+]` at once, then press Enter on the prepared command while the spinner shows. Expected: `queued for "smoke claude"`; its row shows `✉1` after the title and its details say `✉ 1 waiting · last from you … (held)`. When the story ends, the message is typed in and submitted, and `✉1` goes away.
3. Press `e` on `smoke claude`. Expected: **Messages · smoke claude**, `<N> messages, newest first`; your two messages are each a `← you` row with how long ago, its status (`delivered`, or `answered` if Claude replied with `deckhand reply`) and its ID, then its text. `esc` goes back.
4. **Cancel a held one.** Repeat step 2 with `$DH send <id> "Say only: never"`. While `✉1` shows, press `e`: the held message is selected (newest first). Press `x`. Expected: the status says `x again cancels message <message id> from you`. Press `x` again. Expected: `Cancelled message <message id>: it will not be typed`, the row says `cancelled`, `✉1` goes away, and nothing is typed when the story ends.
5. **A reply to you.** `$DH send <id> "What is 2+2? Answer with deckhand reply."`. Expected: Claude runs `deckhand reply <message id> "4"` (or similar). `e` on `smoke claude` shows a `→ you · reply to <message id> · … · returned` row with the answer.
6. **Between two sessions.** Use the orchestrator's two sub-sessions from section 9 (or `smoke claude` and its `⑂` fork from section 4): both must be `●` idle. Attach to the first and ask: *Use deckhand send with --wait to ask `<the other's title>` which file it added, then tell me its answer.* Expected: it runs `deckhand send <id> "…" --wait`; the other session gets the question typed in under `[Message <message id> from "<first>", which wants an answer.` / `Reply once you have it: deckhand reply <message id> "<answer>"]` and runs `deckhand reply <message id> "…"`; the first session's command prints `[Reply from "<other>" to your message <message id>]` and the answer, and it tells you. `e` on either session lists the message as `answered`.

## 11. Worktree manager and deletion

1. Press `W`. Expected: the worktrees grouped (**Merged · safe to delete**, **Merged · has leftovers**, **In progress**, **Idle**, **Missing**, **Main checkout**), each with sessions, uncommitted files and commits.
2. Dirty a worktree (`echo y > $WT/untracked.txt` in its shell) and press `x` on it. Expected: a branch keep/delete choice, then, because of the untracked file, a screen that requires typing `DELETE`. `Esc` cancels; the worktree is untouched.
3. Select the merged worktree from section 7 (step 7), `x`, choose delete worktree and branch. Expected: the sessions running there are named and stopped first; afterwards the directory under `~/.deckhand-dev/worktrees/` is gone, `git worktree list` in the repo no longer lists it, and its sessions are archived (`f` then `A` shows them).
4. `x` on the main checkout row or a worktree used by another session: refused with a reason, never offered.
5. `x` on a running session (sidebar): asks stop only / stop and delete worktree / delete worktree and branch. Choose stop only; the session shows `○` exited and keeps its worktree. `X` asks with **Force kill, keep worktree** chosen; `enter` force-stops it, never authorising data loss.

## 12. Settings and trust

1. Press `C`. Expected: a grid with a **Global** and a **This repo** column; the repo's `deckhand.json` values appear in This repo with `⚠ needs trust` until trusted, `●` marks what applies now.
2. On This repo choose **Default agent**, Enter, pick `pi`. Expected: saved to `$SMOKE/repo/deckhand.json`; the status says review required (the file is no longer the trusted bytes).
3. Press `←`, select **Dev command**, Enter, type `sh -c 'echo global dev; sleep 600'`. Expected: saved under `defaults` in `~/.deckhand-dev/config.json` (check with `cat`).
4. `E` opens the selected column's raw JSON; type a syntax error and `ctrl+s`: it must refuse and leave the file untouched; `ctrl+f` formats; `Esc` returns.
5. Press `T`: the review shows the exact bytes of `deckhand.json`; `Enter` trusts it, returning to Settings with the row no longer marked. Edit `deckhand.json` outside Deckhand (`echo '' >> deckhand.json`): the next `n` or `D` asks for review again.
6. **Agents**: in Settings go to Agents, switch **Agent signals** on. Expected: a Codex hooks warning if `~/.codex/hooks.json` does not call this Deckhand, naming `deckhand hooks codex`.

## 13. Help search

Press `?`, then `/` and type `merge`. Expected: topics narrow to those mentioning it; `Enter` keeps the search and gives letters back; `PgDn` scrolls; `Esc` clears the search, `Esc` again closes. Back on the session list, `G` shows the Agents screen with installed vs latest versions (latest may read "unknown" without `npm`); `Esc` leaves.

## 14. 60-column resize

1. Shrink the terminal window to 60 columns (or run `stty cols 60` is not enough; resize the window). Expected: the UI redraws without garbage: the sidebar narrows, the pane keeps working, no panic, the footer fits.
2. Open each tab and Settings (`C`), Tasks (`b`) and Worktrees (`W`) at 60 columns: text truncates with `…` rather than wrapping into the next row.
3. Attach (`o`) at 60 columns, run `stty size` in the Terminal tab shell: it reports the full terminal size. Detach and resize back to 120 columns: Preview re-wraps to the new width.

## 15. Archive, search, filter

`A` archives the selected session (the agent keeps running if it was). `f` then `A` shows archived, `f f` returns to active. `/` then a title fragment filters the list; `Esc` clears it. `!` jumps to the next session that needs you.

## 16. Exit and resume

1. Attach to `smoke pi`, send `/quit` (or Ctrl+C twice) so the agent exits. Expected: the row turns `○`, the Preview keeps the last screen, and `s` resumes the same conversation (`--session-id <same uuid>`), `S` `S` starts a fresh one (the first `S` says the old one can't be resumed).
2. Repeat for Claude: after exit the preview ends with the resume hint; `s` runs `claude --resume <uuid>`.
3. Codex: `s` resumes (`codex resume <id>`) only once its ID was captured; otherwise it refuses and asks for `S`.

## 17. Relaunch persistence

1. Press `q` in the UI. Expected: the UI exits; `$DH status` still says the daemon is running; `pgrep -f "deckhand.*--session-worker"` still lists the workers.
2. Run `$DH` again. Expected: the same sessions, the same selected session and tab, sibling order, filter, collapsed state and notes; running agents are still live (their Preview shows current output).

## 18. Unread survives a daemon restart

1. Make sure the list shows section headers (`V`), and that `smoke claude` runs (`s` if it shows `○`). Attach to it, send `say hi`, and press `Ctrl+]` at once, before it answers. Do the same in a second running Claude session (its `⑂` fork from section 4, or any other). Expected: as each turn ends, its row moves to **Unread** with `◆` before a bold title, the one that finished first on top; the header counts `◆ 2`. Do not attach to them from here on.
2. Press `q`. Run `$DH restart`. Expected: `Restart the Deckhand daemon? <N> running session(s) (…) will be interrupted; Deckhand offers to resume them when you next open it. [y/N]`. Type `y` and Enter. Expected: `Daemon restarted (…).`
3. Run `$DH`. Expected: the checklist **Resume the sessions the daemon stopped?** lists the sessions that were running, all ticked. Press `esc` to leave them.
4. Select `smoke claude` and press `s`, then the same on the second session. Expected: both resume and stay in **Unread** with `◆` and a bold title, in the same order as in step 1, also once their agents have started; neither moves to **Working** or **Idle**.
5. Attach to `smoke claude` (`o`), then `Ctrl+]`. Expected: it is read now and moves to **Idle**; `◆ 1` in the header.

## 19. Recovery after `kill -9` of the daemon

1. Note which sessions are `●`. In another terminal: `kill -9 "$(cat ~/.deckhand-dev/daemon.pid)"`.
2. In the UI (if open) the connection drops: it reports the daemon is gone or exits. Run `$DH` again. Expected: a new daemon starts by itself (stale socket removed because no live PID), `daemon.log` shows the start; if **Resume the sessions the daemon stopped?** opens, press `esc`. All previously running sessions are listed as `○` exited with their last preview and conversation references intact; Dev shows not running.
3. `ps -axo pid,command | grep -- '--session-worker'` shows no worker of the dev home left running (any survivor of the old daemon exits when its channel closes). If one stays, note its PID.
4. `s` on a Claude session resumes the same conversation; `S` makes a fresh one. State files are intact: `python3 -m json.tool ~/.deckhand-dev/state.json >/dev/null` succeeds.
5. A daemon speaking another protocol version is refused, not replaced. This needs a daemon from an older build; skip it unless you have one in the dev home.

## 20. Hooks and setup

1. `deckhand hooks codex` (use `target/release/deckhand hooks codex`): prints JSON with `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `PermissionRequest`, `Stop` and `Interrupt` (no `Notification`, no `StopFailure`), each command of the form `'<path to deckhand>' 'hook'`. `target/release/deckhand hooks` prints the Claude-style variant (adds `StopFailure` and `Notification`, no `Interrupt`). Nothing is written anywhere.
2. `target/release/deckhand doctor --check` and `target/release/deckhand setup --check`: read-only; they list claude, pi and codex as found or missing and exit 0. With a throwaway Codex home containing a hook that calls `node /x/dist/cli.js hook` (`mkdir -p $SMOKE/codex && printf '{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"node /x/dist/cli.js hook"}]}]}}' > $SMOKE/codex/hooks.json`) run `CODEX_HOME=$SMOKE/codex target/release/deckhand doctor --check`: it reports the stale TypeScript bridge and tells you to regenerate with `deckhand hooks codex`. It never edits the file.
3. `$DH setup` without `--check` must refuse ("The isolated launcher does not install global agent tools").
4. With signals on for Claude (default) a Claude session that asks a permission question shows `?` needs-input in the sidebar and the details name the command; answering it clears the flag. Hooks are posted by `deckhand hook` to the dev daemon only.

## 21. Cleanup

```sh
$DH stop                                    # stops the dev daemon and its sessions
git -C "$SMOKE/repo" worktree list          # leftover worktrees were created under ~/.deckhand-dev/worktrees
rm -rf "$SMOKE"
rm -rf ~/.deckhand-dev                      # only if you keep nothing else there, and after every worktree above is removed through the UI or `git worktree remove`
```

Expected: `$DH status` says not running. `~/.deckhand` was never modified.
