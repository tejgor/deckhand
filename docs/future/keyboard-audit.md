# Keyboard audit: placement and misclicks

**Status:** audited 2026-10-27; nothing built. Proposals are numbered so they can be picked one by one.

Deckhand's keys were audited for **where they sit on the keyboard**, not what they mean. The goal: frequent actions are easy to reach, and a slip onto a neighbouring key, a missed Shift or a key repeat never costs anything. The baseline is a US QWERTY Mac laptop keyboard, typed touch-style, plus a check of UK, German, French and Nordic layouts for anything that uses punctuation or Ctrl chords.

Three audits read the code in parallel: the session list, every other screen, and modifier keys and conflicts with terminals and agents. Items 2.1–2.6 were then checked against the code by hand. The rest come from the audits' reading of the code. The points about terminals and layouts (kitty encodings, Ctrl+] behind Option, Ctrl+Space as the macOS input-source switch) still need a check on a real terminal.

**The constraint that shapes the proposals:** every action keeps a home-row way to do it. Arrows are an alternative, never the only way. So `l` stays as the way into a pane, even though its neighbour `k` makes slips into typing mode possible; the slip guard in 1.1 deals with that instead.

## What the audits found

- **Typing into an agent is easy to start by accident and hard to stop.**
  - `l` (next to `k`) and `→` (next to `↓`) start typing on Preview and Terminal (`crates/ui/src/app/modes/browse.rs`, `step_into_pane`).
  - From then on every key goes to the agent. A digit meant to pick a session, or Enter meant to attach, can answer a permission prompt. A reflex Esc interrupts Claude's turn.
  - The way out, Ctrl+], is a right-pinky stretch, and the only one shown on screen (`crates/ui/src/app/layout.rs` footer, `crates/client/src/attach_status.rs`).
  - On Mac German, French and Nordic layouts, `]` needs Option. Ctrl+Space, the other way out, is macOS's default input-source switch.
  - `detach_position` (`crates/client/src/attach.rs`) doesn't recognise Ctrl+] in the kitty or modifyOtherKeys encodings.
- **Shift twins skip a confirmation.**
  - `x` asks before stopping; `X` force-stops at once.
  - `s` resumes; `S` starts a fresh conversation at once, and the old one can't be resumed from Deckhand afterwards.
- **Dev is easy to toggle by accident.**
  - `d d` within 500 ms starts or stops Dev (`DEV_DOUBLE_TAP_MS`). Key repeat on a held `d` can trigger it, and starting Dev stops it in the other worktrees.
  - In the run list, Enter on the Dev row stops Dev when it runs (`action_picker.rs`).
  - `x` in the run list stops the running action or closes a shell with no confirmation.
- **Some confirmations start on the riskier choice.** The Worktrees bulk delete starts on "delete N worktrees and their branches" (`crates/ui/src/flows/worktrees.rs`). The merged-worktree default is deliberate.
- **Typed work can be lost.** Esc in the task editor discards the draft, and discard prompts put "discard" on Enter.
- **Reach.**
  - `!` (next session that needs you) needs Shift and a pinky stretch.
  - A Mac laptop has no PgUp/PgDn without fn, and `J`/`K` mean something different on four kinds of screen.
  - `h` doesn't go back on several screens.

## Batch 1: typing into an agent or shell (`l`/`→` on Preview and Terminal)

| # | Proposal | Cost |
|---|---|---|
| 1.1 | **Slip guard:** if the *first* key after entering is `h`, `j`, `k` or an arrow, leave typing and apply that key to the list. Nothing is sent to the agent. `l` stays as the way in. | S |
| 1.2 | **Decide:** does Esc as the first key also count as a slip? Yes protects against a reflex Esc interrupting Claude. The cost is that stopping an agent from the list takes `l`, another key, then Esc. Leaning yes. | XS |
| 1.3 | Footer and attach status line show **Ctrl+Space** first (thumb plus pinky, near the home row), with Ctrl+] as the alternative. Today only Ctrl+] is shown. | XS |
| 1.4 | Make typing mode obvious: dim the sidebar and its `›`, and put **TYPING · ctrl+space back** in the pane's border title, where it can't be cut off. | S |
| 1.5 | When typing ends on its own (the agent exits, the shell dies, the connection drops), ignore list keys for about 0.75 s. Otherwise the rest of a sentence runs commands: `⌫` archives, `s`/`S` restart. | S |
| 1.6 | Fix `docs/keys.md` and the guide: `←`/`h`/Esc don't leave the typing panes. | XS |
| 1.7 | Clicking the sidebar leaves typing mode. Clicks are received and ignored today. This exit works on every layout. | S |

## Batch 2: destructive actions on a single key

| # | Proposal | Cost |
|---|---|---|
| 2.1 | `X` opens the stop confirmation with "force" preselected, so one Enter confirms. | S |
| 2.2 | `S` (fresh conversation) asks first; one Enter confirms. | S |
| 2.3 | Move Dev start/stop from `d d` to **`D`**, which is free and on the home row. `d` only shows the Dev tab, and `last_dev_tap` goes. | M: about 20 strings, footer hints, 2 E2E tests |
| 2.4 | Run list: Enter on a running Dev opens it rather than stopping it. Enter on a shell row opens the shell; restarting moves to `x x`, or another key. | S |
| 2.5 | Run list: stopping an action or closing a shell takes `x x`, as deleting a task does, and acts only on the selected row. | S |
| 2.6 | Worktrees bulk delete starts on "keep the branches". The merged-worktree default stays. | XS |
| 2.7 | Git changes: `a` (stage all) and `A` (unstage all) need a double press. Either undoes hunk-level staging done in lazygit. | S |
| 2.8 | *(Optional)* `A` (archive) needs a double press. It can be undone, so leaning no. | XS |

## Batch 3: losing typed work

| # | Proposal | Cost |
|---|---|---|
| 3.1 | Esc in the task editor saves the edit, as the notes editor does. | S |
| 3.2 | Discard prompts: Enter keeps editing and `d` discards, so Esc then Enter no longer loses the draft. | S |
| 3.3 | Ctrl+C in the task and notes editors acts as Esc on the first press, as Settings does, and notes are saved before quitting. The 300 ms save may be lost today; this needs checking. | S |

## Batch 4: reach and the home row

| # | Proposal | Cost |
|---|---|---|
| 4.1 | **`;`** (right pinky, home row, free) selects the next session that needs you, as well as `!`. | XS |
| 4.2 | **`ctrl+d` / `ctrl+u`** scroll half a page in every list and scroll area. | M |
| 4.3 | **`h`** goes back on Tasks, Worktrees, Agents and the run list. | S |
| 4.4 | `J`/`K` page where they're free: the Worktrees list and the task picker. They keep their meaning elsewhere. | S |
| 4.5 | The key that opens a screen also closes it (`b`, `W`, `C`, `G`), as `?` already does. | S |

## Batch 5: consistency between screens

| # | Proposal | Cost |
|---|---|---|
| 5.1 | Agents reloads with `R`, as every other screen does, not `r`. | XS |
| 5.2 | `/` search in Tasks and Worktrees. | M |
| 5.3 | ↑/↓ move the selection while typing a session search, as they do in the task picker. | S |
| 5.4 | Esc behaves the same in both DELETE prompts. Worktrees goes back to the choices; the kill prompt goes back to the session list. | XS |

## Batch 6: the leave key on other keyboards and terminals

| # | Proposal | Cost |
|---|---|---|
| 6.1 | Recognise Ctrl+] (and Ctrl+Space with Caps Lock or Num Lock on) in the kitty (`ESC[93;5u`) and modifyOtherKeys (`ESC[27;5;93~`) encodings, ignoring lock bits and key releases. | S |
| 6.2 | A configurable leave key (`detachKey` in `config.json`), used by `detach_position`, the typing-mode keymap and every hint. | S–M |
| 6.3 | **Decide:** an extra leave key on by default. Candidates: `ctrl+\` (0x1c, the dtach convention, right pinky) or `ctrl+q` (left hand; it quits VS Code on Linux, so opt-in). | XS once 6.2 exists |

## Batch 7: small fixes

| # | Proposal | Cost |
|---|---|---|
| 7.1 | The Tasks `x x` delete expires after about 2 s. | XS |
| 7.2 | Digits that arrive together (`"12"`) select session 12; today they're ignored. | XS |
| 7.3 | The first Ctrl+C while typing into an agent shows "ctrl+space returns to the list; ctrl+c goes to the agent", once. | XS |

## Keep as is

- j/k and digit selection.
- Tab letters on the left hand while the right stays on j/k.
- `x` and `m` confirmations that start on the safe choice.
- Typing DELETE before data is lost.
- Capitals for rare screens (`C`, `G`, `W`).
- `space`, `/`, `?` and `q`.
- Ctrl+S, Ctrl+Q, Ctrl+C and Ctrl+Z reach Deckhand rather than the terminal, since flow control and signal keys are off.
- A leave key inside a paste is ignored.

## Suggested starting set

All of Batch 1 (with 1.2 = yes), 2.1–2.6, Batch 3, 4.1–4.3, 6.1 and 7.1. Each is small, and together they cover every way a slip can cost something. Batch 6 matters most for keyboard layouts where `]` needs Option.
