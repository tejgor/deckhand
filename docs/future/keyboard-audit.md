# Keyboard audit: placement and misclicks

**Status:** audited 2026-10-27, decided 2026-10-28. [Build](#build) **shipped** on branch `agents-without-attaching` (the lasting parts are in `HANDOFF.md`, `docs/keys.md` and `docs/features.md`); [Later](#later) is still open, and [Decided against](#decided-against) says why the rest were dropped. Numbers are kept from the first draft so they can be traced.

Deckhand's keys were audited for **where they sit on the keyboard**, not what they mean. The goal: frequent actions are easy to reach, and a slip onto a neighbouring key, a missed Shift or a key repeat never costs anything. The baseline is a US QWERTY Mac laptop keyboard, typed touch-style, plus a check of UK, German, French and Nordic layouts for anything that uses punctuation or Ctrl chords.

**How it was decided.**
1. Three audits read the code in parallel: the session list, every other screen, and modifier keys and conflicts with terminals and agents.
2. Two independent reviews then judged the proposals. One looked at UX and bloat, the other checked each premise against the code for key conflicts and side effects.
3. The owner made the final cut.

The points about terminals and layouts (kitty encodings, Ctrl+] behind Option, Ctrl+Space as the macOS input-source switch) still need a check on a real terminal.

**Constraints:**
- Every action keeps a home-row way to do it. Arrows are an alternative, never the only way, so `l` stays as the way into a pane.
- No new rules to learn without a clear payoff.
- One way to do each thing: one press-twice convention, one paging key, one way back.

## What the audits found

- **Typing into an agent is easy to start by accident.**
  - `l` (next to `k`) and `→` start typing on Preview and Terminal (`step_into_pane` in `crates/ui/src/app/modes/browse.rs`).
  - From then on every key goes to the agent: a digit or Enter can answer a permission prompt, and Esc interrupts Claude.
  - When typing ends on its own, keys still being typed run list commands.
- **Shift twins skip a confirmation.** `x` asks before stopping; `X` force-stops at once. `s` resumes; `S` starts a fresh conversation at once, and the old one can't be resumed from Deckhand afterwards.
- **Dev is easy to toggle by accident.**
  - `d d` within 500 ms starts or stops Dev (`DEV_DOUBLE_TAP_MS`). Key repeat on a held `d` can trigger it, and starting Dev stops it in the other worktrees.
  - In the run list, Enter on the Dev row stops a running Dev, and Enter on a shell row restarts the shell.
  - `x` stops an action or closes a shell with no confirmation, and on a running Dev process row it stops the action instead (`crates/ui/src/app/modes/action_picker.rs`).
- **Typed work can be lost.** Esc in the task editor throws away the draft. Ctrl+C quits from the editors, and quitting doesn't flush the notes' 300 ms save (`crates/ui/src/flows/notes.rs`).
- **List keys ignore Ctrl.** Ctrl+X opens the stop confirmation, Ctrl+A shows Notes and Ctrl+D arms the Dev double tap, exactly as `x`, `a` and `d` do (`crates/ui/src/input.rs`, `handle_browse`).
- **Reach.**
  - `!` (next session that needs you) needs Shift and a pinky stretch.
  - A Mac laptop has no PgUp/PgDn without fn.
  - `h` doesn't go back on several screens, and the branch picker and session search move only with arrows.
- **Leave key.** `detach_position` (`crates/client/src/attach.rs`) doesn't recognise Ctrl+] in the kitty or modifyOtherKeys encodings, which attached TUIs such as Codex can switch on.

## Build

Shipped on 2026-10-28, in five commits on `agents-without-attaching` (`50ff34a`, `087ada8`, `24f3b7d`, `015c4e3`, `b89ced0`). The tables below are the record of what was decided. Where the build settled a detail the table left open:

- **2.2**: `S` uses the press-twice rule (7.0), not a dialog, since it is a yes/no: the first `S` says the old conversation can't be resumed, the second restarts.
- **2.1**: the force kill's dialog offers only *Force kill, keep worktree* and *Cancel*, and checks nothing first, since it never deletes.
- **2.4 + 2.5**: "shows what runs" switches to its tab (Dev, or the action's or shell's Terminal view) and closes the run list; it doesn't attach full screen.
- **3.1**: an edit that changed nothing, like an empty title, just closes on Esc. When the save fails (no daemon), the editor stays open with the text.
- **3.3**: the text fields are the session search, the new-session name and its worktree search, both DELETE prompts, the notes and task editors, the task picker's search and the guide's search (`App::typing_in_field`). Notes are saved on every exit from the UI, attaches included.
- **4.2**: Ctrl+D / Ctrl+U are PgDn / PgUp wherever a screen pages; the session list and the Tasks board, which had no page keys, move half their list. Worktrees keeps its ten rows.
- **5.1**: a failed update's status says `G shows the output` (or `why`), since it can show on any screen.
- **5.3**: Ctrl+N / Ctrl+P also move in the existing-worktree picker while typing its search.
- **7.3**: the hint reads *ctrl+c went to the agent; ctrl+] returns to the list* (or *the shell*).

Where each key is documented now: `docs/keys.md` (the Run list and Agents sections are new, with notes on Ctrl and letters and on press twice), `docs/features.md`, the in-app guide and `HANDOFF.md`.

**Typing into an agent or shell**

| # | Change | Cost |
|---|---|---|
| 1.4 | While typing, dim the rest of the sidebar, keeping the selected row bright: it shows who gets your keys. Make the existing *typing to the agent* / *typing into shell N* badge louder rather than adding a second one. The magenta border stays. | S |
| 1.5 | When typing ends on its own (the agent exits, the shell dies, the connection drops), ignore list keys until 0.75 s pass with no key, so the rest of a sentence can't run commands. Ctrl+C still works meanwhile. | S |
| 1.6 | Fix `docs/keys.md` and the guide (`crates/core/src/help.rs`), which say `←`/`h`/Esc go back from every pane. They don't in typing mode or in the notes editor. | XS |

**Destructive keys**

| # | Change | Cost |
|---|---|---|
| 2.1 | `X` opens the stop confirmation with "force" preselected, so one Enter confirms. `app.kill.force` and the *Force kill* title already exist, unused (`confirm.rs`). Update E2E `tasks_ui.rs`. | S |
| 2.2 | `S` asks first and says the old conversation can't be resumed afterwards. It must also cover the setup-review path. | S |
| 2.3 | Dev start/stop moves from `d d` to **`D`** (free). `d` only shows the Dev tab; `last_dev_tap` and `DEV_DOUBLE_TAP_MS` go. There are about 60 references in 19 files, including the footer, panes/dev.rs, help, Settings, a daemon error string (`workspaces.rs`) and E2E `settings_ui.rs` and `tasks_ui.rs`. | M |
| 2.4 + 2.5 | **One model for every run-list row** (Dev, Dev processes, actions, shells). Enter starts a stopped row and opens a running one. `x x` (the shared press-twice rule) stops or closes the selected row only. Restarting is `x x` then Enter; on the first shell `x x` kills it and it comes back fresh. Update E2E `run_list_ui.rs` and `shells_ui.rs`, and the help line "Enter starts or restarts it". | S |
| 2.7 | Git changes: `a` (stage all) and `A` (unstage all) stay one key, except when they would undo partial staging (done in lazygit). Then the footer says so ("a again: 3 files partly staged") and a second press confirms. | S |

**Losing typed work**

| # | Change | Cost |
|---|---|---|
| 3.1 | Esc in the task editor saves, as the notes editor does. A new task with an empty title saves nothing and closes (today the empty-title error would leave the editor stuck). Update E2E `tasks_ui.rs` and the "esc cancel" hints. | S |
| 3.3 | In every text field (editors, searches, the new-session name, DELETE prompts) the first Ctrl+C acts as Esc, as it already does in Settings. Notes are saved before the UI quits, which is a bug fix in its own right. | S |

**Reach**

| # | Change | Cost |
|---|---|---|
| 4.0 | **List keys respect Ctrl**: Ctrl+X, Ctrl+A, Ctrl+D and the like no longer act as `x`, `a`, `d`. Needed by 4.2. | S |
| 4.1 | **`;`** (right pinky, home row, free) does exactly what `!` does, filter switch included. `!` stays; the header shows `! N`. | XS |
| 4.2 | **Ctrl+D / Ctrl+U** scroll half a page in lists and scroll areas, never in text fields (the notes editor uses emacs-style Ctrl keys). In a split view they scroll what PgDn/PgUp scroll. Typing mode is unaffected (keys go to the agent raw). | M |
| 4.3 | **`h`** goes back on Tasks, Worktrees, Agents and the run list, as Esc does. Not where `h` types (searches, DELETE drafts) or already means something (the task picker's `h`/`l` switch views; Help's previous topic). | S |
| 5.3 | ↑/↓ and **Ctrl+N / Ctrl+P** move the selection while typing a session search; Ctrl+N/P also move in the new-session branch picker, which is arrows-only today. | S |

**Consistency**

| # | Change | Cost |
|---|---|---|
| 5.1 | The Agents screen reloads with `R`, as every other screen does, not `r`. Its failed-update status, "U shows why" / "U shows the output" (`crates/ui/src/flows/agents.rs`), names a key that does nothing there: reword it to point at the details box, which already shows the output. | XS |
| 5.4 | Esc goes back one step in both DELETE prompts. Today Worktrees goes back to the choices; the stop confirmation goes all the way back to the session list. | XS |

**Leave key and small fixes**

| # | Change | Cost |
|---|---|---|
| 6.1 | Recognise Ctrl+] and Ctrl+Space in the kitty (`ESC[93;5u`, `ESC[32;5u`, with `:1` press events) and modifyOtherKeys (`ESC[27;5;93~`) encodings, ignoring Caps Lock and Num Lock bits and key releases. A bug fix. | S |
| 7.0 | **One press-twice rule, built once**: the same key again within 2 s; the footer says what the second press will do; any other key cancels. Used by the run list's `x x` (2.5), the Tasks `x x` (7.1) and the Git changes `a`/`A` (2.7). | S |
| 7.1 | The Tasks `x x` delete goes through 7.0, so it expires after 2 s. Today another key cancels it, but nothing times out. | XS |
| 7.2 | Digits that arrive in one chunk (`"12"`) go through the existing number buffer, as if typed one at a time. Today the chunk is ignored and even clears a pending number. | XS |
| 7.3 | The first Ctrl+C of each typing stint shows "ctrl+] returns to the list; ctrl+c goes to the agent". The Ctrl+C still reaches the agent. Nothing is saved. | XS |

End-to-end tests that these change: 2.1, 2.3, 2.5, 3.1 (see each row). They're updated as part of the change, not blessed around.

## Later

Kept, but bigger than the rest; build after [Build](#build).

| # | Change | Cost |
|---|---|---|
| 6.2 | **A configurable leave key** (`detachKey` in `config.json`), for layouts where `]` needs Option (German, French, Nordic Macs) and for people whose Ctrl+Space switches input source. It would be read by `detach_position`, the typing-mode keymap, the footer, the attach status line and the notes editor's vim mapping (`notes_vim.rs`, hard-coded today; the `notesVim` corpus would need a reviewed bless). The guide's text is static, so it would say "the leave key" and name the default. Docs give examples, Ctrl+Q among them (it quits VS Code on Linux, so not a default). | M–L |
| 5.2 | **`/` search in Tasks and Worktrees**, working exactly as the sidebar search does (Enter keeps the search, Esc clears it). It doesn't fix a misclick, so it waits. | M |

## Decided against

- **1.1 Slip guard** (the first key after entering typing, if `h`/`j`/`k`/an arrow, goes to the list). It would eat keys you need right after stepping in on purpose:
  - ↑/↓ move through Claude's and Codex's permission menus.
  - ↑ recalls shell history.
  - Words start with h, j and k.

  After a swallowed letter, the rest of the word would run as list commands. Making the mode obvious (1.4) and safe when it ends (1.5) covers the risk without a hidden rule.
- **1.2 Esc as a slip.** A lone Esc declines Claude's prompt and interrupts a turn. It's often the reason you step in.
- **1.3 Showing both leave keys in the footer and status line.** It would crowd them. Ctrl+] stays the one shown; Ctrl+Space stays in the guide and docs.
- **1.7 Click the sidebar to leave typing.** Not needed.
- **2.6 Bulk worktree delete starting on "keep the branches".** The bulk delete lists only merged worktrees, and offering the branch first for merged work is deliberate (as in the single delete).
- **2.8 Press twice to archive (`A`).** It can be undone and shows at once.
- **3.2 Discard prompts: Enter and Esc keep editing, `d` discards.** Judged confusing. The Settings discard prompts, and the Settings link editor where Esc discards, stay as they are.
- **4.4 `J`/`K` paging in the Worktrees list and task picker.** `J`/`K` already mean four things; Ctrl+D/U (4.2) is the paging key.
- **4.5 The key that opens a screen also closes it** (`b`, `W`, `C`, `G`). `G` means "last row" in every list, and `h` (4.3) already gives a home-row way back.
- **6.3 An extra default leave key.** `ctrl+\` needs Option on German and Nordic Macs too, and it sends SIGQUIT to a shell's job. Ctrl+Q is an example value for 6.2.

## Keep as is

- j/k and digit selection.
- Tab letters on the left hand while the right stays on j/k.
- `x` and `m` confirmations that start on the safe choice.
- Typing DELETE before data is lost.
- Capitals for rare screens (`C`, `G`, `W`).
- `space`, `/`, `?` and `q`.
- `l` and `→` into a pane.
- Ctrl+] as the leave key shown on screen.
- Ctrl+S, Ctrl+Q, Ctrl+C and Ctrl+Z reach Deckhand rather than the terminal, since flow control and signal keys are off.
- A leave key inside a paste is ignored.
