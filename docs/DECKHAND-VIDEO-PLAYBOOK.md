# Deckhand videos: a playbook

What drafts D, E and G share: the essence they communicate, the story they tell, the words that were approved, the look
and sound that made them feel like Deckhand, and the product details they get right. Use it to make more Deckhand
content with the same theme and quality, in any format and any length.

This is the companion to [VIDEO-PLAYBOOK.md](VIDEO-PLAYBOOK.md), which covers the general craft (structure, pacing,
colour, layout, motion, process). This guide covers what's specific to Deckhand. Where it quotes what past drafts did
(their length, tempo, sizes), treat that as a reference point, not a rule. The rules are the brand's: the logo, the
meanings of its colours, the truth of the product, the voice.

---

## Contents

1. [What every Deckhand video must land](#1-what-every-deckhand-video-must-land)
2. [The shared story](#2-the-shared-story)
3. [Story beats you can build with](#3-story-beats-you-can-build-with)
4. [Finding a new creative take](#4-finding-a-new-creative-take)
5. [Words](#5-words)
6. [The look](#6-the-look)
7. [The logo](#7-the-logo)
8. [Showing Deckhand](#8-showing-deckhand)
9. [Sound](#9-sound)
10. [Motion](#10-motion)
11. [Formats](#11-formats)
12. [Making it in this repository](#12-making-it-in-this-repository)
13. [Deckhand review questions](#13-deckhand-review-questions)
14. [Quick reference](#14-quick-reference)

---

## 1. What every Deckhand video must land

### The one thing to remember

In the owner's words: **Deckhand is an agent management workbench that makes running and orchestrating a large number
of coding agents easy, so you can use them to do a lot of work in parallel.**

Everything else serves that. The selling point is **managing many agents easily and intuitively**. It is not that agents
keep working, that Deckhand "handles the rest", or any single feature on its own.

### Who it's for

- Developers who already use Claude Code, Codex or Pi and are starting to run more than one at a time.
- Experienced users looking for a lightweight but powerful way to manage many agents.
- Developers curious about coding agents.

So: accessible to someone running their second agent, without patronising someone who knows what a worktree is.

### What it is, in facts you can state

- It runs **Claude Code, Codex and Pi** (named in that order, by their real names).
- It **lives in a terminal,** including the terminal inside your editor (an IDE terminal). There's no separate app or
  window. One terminal tab holds all your agents.
- **One list** shows every session's state (working, idle, waiting for your input, done, merged), beside a **live preview**
  of what the selected one is doing, so you check on any agent without opening it.
- When an agent **asks a question or needs your OK to run a command,** it's flagged. **`!`** jumps to the next session
  that needs you, and filters the list to them.
- **`enter`** opens a session full screen, so you answer it there. **`ctrl+]`** takes you back to the list.
- **`n`** starts a session: choose the agent, choose whether it gets its own worktree, and launch.
- Each session can work **in its own git worktree:** its own branch, in its own folder. So every change can be built,
  tested and reviewed on its own, and no two agents edit the same files.
- **`m`** merges a worktree back. The changes arrive **staged, not committed,** so you review them and commit when you're
  ready.
- Every worktree gets its own **terminal, Git view, dev server and notes** (the tabs `t`, `g`, `d`, `a`, beside `p`, the
  preview).
- Sessions belong to a **background process,** not the window. Quit Deckhand (`q`), or let your terminal close or your
  editor restart, and the agents keep working. Open it again and they're all still there.
- **Free and open source.** Deckhand 3 is one native binary (Rust, no Node.js) for macOS and Linux, installed with a
  one line from the README: `brew install tejgor/tap/deckhand`, or the install script (or a release archive, or
  cargo). Every close shows `brew install tejgor/tap/deckhand`, with "No Homebrew? The README has a one-line installer:
  github.com/tejgor/deckhand" under it; never the install script's command, which doesn't read on a phone.
  (Deckhand 2 was `npm install -g @tejgor/deckhand`.)

### What never to claim

- That agents keep running when the computer is off or asleep. The background process is on your machine.
- That Deckhand commits, pushes or decides anything for you. You review and commit.
- That it "handles" the work. The agents do the work; Deckhand makes managing them easy.
- Any feature or behaviour you haven't checked against the README or the code.

---

## 2. The shared story

D, E and G look different and are told differently, but they tell the same story in the same order. That shared arc is
the Deckhand story.

| Movement | What it says | D | E | G |
|---|---|---|---|---|
| **Multiply** | Agents are good; running several is easy and tempting | "Coding agents… They take a task and run with it." A wall grows from three terminals to twenty | "Run one coding agent. Run two. Run four. Run eight. All at once." | "You start a coding agent in your editor's terminal. Then you start more, each in its own tab." |
| **The cost** | But each one stops to ask you things, and keeping up becomes work | "But they keep stopping to ask you things. Before long, it's a second job" (a checklist of three chores) | "But each one stops when it needs you. And waits." (the music drops out part by part) | "You can only see one at a time. Three are waiting on you, and you can't tell which. And two are editing the same file." |
| **The turn** | Hand it over: silence, then the logo on the drop | "Hand that job to Deckhand." Silence. The logo | "And waits." Silence. The logo | A beat of dark. The logo: "One terminal tab for all your coding agents" |
| **One place** | Every agent in one list, with a live view | "All your agents in one list, with a live preview of what each one is doing." | "One list for every agent." | "Now all eight are in one list. Select one to see what it's doing, without leaving the list." |
| **Attention** | The ones that need you are flagged; `!` gets you there | "When an agent asks a question or needs your OK to run a command, it's flagged. Press ! to jump straight to it." | "The ones that need you are flagged. Press ! to jump to the next one." | "The three waiting on you are marked. Press ! to answer them, one after another." |
| **Apart** | A worktree each | "Give each agent its own branch, in its own worktree. So every change can be built, tested and reviewed on its own." | "Each agent works in its own worktree." | "Each agent works in its own worktree, so no two edit the same files." |
| **Land the work** | Review the diff, `m` to merge, you commit | "Review exactly what it changed. Press m to merge it back. You commit when you're ready." | "Review the diff. Press m to merge." | "Read what it changed, then press m to merge it. Its changes arrive staged, for you to review and commit." |
| **Peace of mind** | They keep running in the background | "If your terminal closes or your editor restarts, they keep working." | "Close Deckhand any time. Pick up where you left off." | "Close Deckhand, and your agents keep running. Open it again, and they're all still there." |
| **Close** | The benefit, turned around; how to get it | "Run more agents. Watch fewer tabs." and the install command | "Run as many agents as you like. Deckhand makes them easy to manage." and the install command | "A lightweight agent workbench for your IDE terminal." and the install command |

### Why this arc works

- **It starts from where the viewer is:** running agents is easy and appealing. The video doesn't open by telling them
  they have a problem.
- **The pain is the real one:** attention. Agents stop and wait for you, and you can't see which ones, so keeping up
  becomes a job of its own. Collisions and merging are the second and third chores.
- **The reveal arrives as relief,** after silence, on the biggest musical drop.
- **Every feature answers a pain named earlier,** in the same order: see them all, get to the one that needs you, keep
  their work apart, land it.
- **It ends where it started, turned around:** more agents, less juggling.

You can reorder, compress or expand this arc, or tell only part of it in a short piece. Keep the logic: the problem is
felt before Deckhand appears, and each feature answers something the viewer has seen.

---

## 3. Story beats you can build with

Each beat below appeared in at least one of D, E and G. For each one: what it must communicate, how it was shown, the
words that were approved, and the traps.

### The agents

- **Communicates:** these are real coding agents the viewer knows.
- **Shown as:** the three names with their glyphs: ✶ Claude Code, ◇ Codex, π Pi. In D they're big cards; in E and G they're
  terminals that start up one by one.
- **Approved:** "Coding agents". "Claude Code, Codex and Pi." "They take a task and run with it."
- **Traps:** naming the agents in three different scenes. Say it once, early, and maybe once more at `n`.

### Multiply

- **Communicates:** running many at once is easy, and it's how you get more done.
- **Shown as:** an escalating count on the beat (one, two, four, eight; or three, five, ten, twenty), terminals filling the
  frame, or tabs piling up in the editor.
- **Approved:** "Run one coding agent. Run two. Run four. Run eight. All at once." "Running agents in parallel is easy."
- **Traps:** using a specific number as if it mattered ("You started five agents"). Counts work as rhythm, not as a
  claim.

### They stop and wait

- **Communicates:** agents regularly stop to ask a question or ask permission to run a command, and then they wait.
- **Shown as:** the agent's real prompt (Claude Code's "Do you want to proceed?" with its numbered options), a waiting
  terminal turning yellow with "?", flags with plain labels ("wants to run: npm test", "asks: Keep old sessions
  valid?", "plan ready: …").
- **Approved:** "But they keep stopping to ask you things." "But each one stops when it needs you. And waits."
- **Traps:** "waiting on you" or "needs you" without showing for what. Always make the reason concrete at least once.

### You can't tell which

- **Communicates:** with a tab or window per agent, you can only see one at a time, so the waiting ones are hidden.
- **Shown as:** the editor's terminal tabs, switching one at a time, while a separate view shows what's really happening
  (in G, tiles that reveal three waiting).
- **Approved:** "You can only see one at a time. Three are waiting on you, and you can't tell which."

### Changes collide

- **Communicates:** agents working in the same checkout edit the same files.
- **Shown as:** one file with two agents' edits landing on the same lines, marked in each agent's colour, with the clash
  in the system's red.
- **Approved:** "And two are editing the same file." It works when the picture shows it concretely and the answer later
  gives the factual reason (own branch, own folder, reviewed on its own).
- **Traps:** framing it as an anecdote or a joke ("one very confusing diff"), or "breaks another's tests". The underlying
  reason is what matters: each change should be built, tested and reviewed on its own.

### The second job

- **Communicates:** keeping up with many agents becomes a job of its own.
- **Shown as:** a checklist that types itself out, with three plain items. Each later scene answers one of them.
- **Approved:** "Before long, it's a second job:" then "Unblock every agent that's waiting on you", "Keep each agent's
  changes from colliding", "Review and merge their work, one by one".
- **Traps:** cringe or trivial wording. The frame was loved; the first versions of its items weren't. Keep them plain
  and solid.

### The turn and the reveal

- **Communicates:** this is what Deckhand is for.
- **Shown as:** a handover line, a beat of silence or darkness, then the brand logo on the drop with stripes, a slam and
  a flash. Then one line saying what it is.
- **Approved handover:** "Hand that job to Deckhand." (the best-liked reveal of all the drafts)
- **Approved "what it is" lines:** "A workbench for running coding agents in parallel." "One terminal tab for all your
  coding agents." "A lightweight agent workbench for your IDE terminal."
- **Traps:** "Meet Deckhand", "You need a deckhand", "All hands on deck", or any pun on the name.

### One list, live preview

- **Communicates:** every agent in one place; see what any of them is doing without opening it.
- **Shown as:** the real sidebar (numbered rows with status glyphs and agent glyphs) beside the Preview tab; `↑`/`↓`
  moving the selection, and the preview following it.
- **Approved:** "All your agents in one list, with a live preview of what each one is doing." "Select one to see what it's
  doing, without leaving the list." "Check on any agent without opening it."
- **Traps:** "Peek at any agent without interrupting it" (presented oddly). Say plainly what it does.

### Attention and `!`

- **Communicates:** Deckhand tells you which agents need you, and takes you straight there.
- **Shown as:** rows marked with yellow `?`, flags beside them saying what each is asking, the attention count in the
  sidebar header ("! 3"), then `!` pressed (big, on a drop), the list filtering to the ones that need you.
- **Approved:** "When an agent asks a question or needs your OK to run a command, it's flagged. Press ! to jump straight
  to it." "The three waiting on you are marked."
- **Traps:** a separate full-screen card explaining the `!` key. It took the viewer out of the video. Keep it inside the
  scene.

### Open, answer, back

- **Communicates:** you answer in the agent's own screen, then you're straight back in the list.
- **Shown as:** `enter`, the session growing to full screen, the answer (a key like `1`), then `ctrl+]` and the list
  again. One cycle shown slowly is enough; then move on.
- **Approved:** "Enter opens it. Answer it there." "Ctrl+] takes you back to the list."
- **Traps:** showing all of them. The owner asked for one or two, slowed down, then cut to the next scene.

### Worktrees

- **Communicates:** each agent works on its own branch in its own folder, so every change stands alone.
- **Shown as:** a tree from `⎇ main` to each branch, its path (`~/.deckhand/worktrees/<branch>`), and chips that turn green
  as each one builds and passes its tests; or the paths printing beside the list.
- **Approved:** "Give each agent its own branch, in its own worktree. So every change can be built, tested and reviewed on
  its own." "Each agent works on its own branch, in its own folder."
- **Traps:** "so they don't clash" and "no collisions" (vague and cringe), and over-explaining worktrees to people who
  know them.

### Review and merge

- **Communicates:** you read exactly what changed, merge it with one key, and you stay in control of the commit.
- **Shown as:** the Git tab (the changed files with line counts, then the diff in green and red), `m`, the merge dialog
  ("Merge into main without committing"), `enter`, the merged mark ✓, on a drop.
- **Approved:** "When an agent is done, review exactly what it changed. Press m to merge it back. You commit when you're
  ready." "Its changes arrive staged, for you to review and commit."
- **Traps:** "Review and merge, staged for you to commit" with no picture. It needs the screen to make sense.

### In the background

- **Communicates:** peace of mind. Your agents aren't tied to the window.
- **Shown as:** the window closing while the agents keep working (spinners, meters, tiles still moving), then the window
  opening again with everything still there.
- **Approved:** "Your agents run in the background, not in the window. If your terminal closes or your editor restarts,
  they keep working." "Open it again, and they're all still there."
- **Traps:** "Close it. They keep working." on its own (why would anyone close it?), and anything implying they run with
  the computer off.

### Every session gets its own

- **Communicates:** each session comes with what you need to work on it.
- **Shown as:** the tabs pressed in turn, `t`, `g`, `d`, `a`, on the beat, with the changing word as the hit.
- **Approved:** "Every session gets its own terminal, Git view, dev server and notes." (The word is "session", not
  "agent", and the list on screen takes no commas or full stops.)

### Where it lives

- **Communicates:** no new app. It's a terminal tab, including the one in your editor.
- **Approved:** "Runs in your terminal." "Deckhand lives in the terminal you already have open." "One terminal tab for all
  your coding agents."

### The close

- **Communicates:** the benefit, then how to get it.
- **Shown as:** the brand logo, the tagline in big type, the install command typed out (its cursor blinking, because it's
  a prompt), then "Free and open source" and the GitHub address.
- **Approved taglines:** "Run more agents. Watch fewer tabs." "Run as many agents as you like. Deckhand makes them easy to
  manage." "A lightweight agent workbench for your IDE terminal." "Run Claude Code, Codex and Pi side by side, each in
  its own worktree." (the walkthrough's; it matches the README)
- **Other ways in:** a line under the command says what the viewer will find and where, not just an address: "No
  Homebrew? The README has a one-line installer: github.com/tejgor/deckhand".
- **Kept current:** the close shows the README's install as it is now. When the README's install changes, update every
  close that shows it.
- **Traps:** a long hold on "Free and open source" alone; closers about keeping agents working; anything mentioning a
  specific number of agents; the rewrite ("Rust", "no Node.js", "Deckhand 3") in an evergreen piece (see
  [Formats](#11-formats)).

---

## 4. Finding a new creative take

Each successful draft had one device that expressed Deckhand's essence. That's what made them more than feature tours.

| Draft | Device | Why it worked |
|---|---|---|
| D | A poster-style story built on a checklist: the second job, then each item answered | The checklist turns features into answers |
| E | The band: each agent plays one part of the music. A part drops out while its agent waits on you, and comes back when you answer, so the drop is every agent working again | The viewer hears the cost of waiting agents and the relief of answering them |
| G | In your editor: tabs pile up in the editor's terminal and fold into one tab that holds them all | It shows exactly where Deckhand lives and what it replaces |

What failed, and why:

| Take | Why it failed |
|---|---|
| One morning in real time (a clock running from nine to noon) | Well made, but the story was about time passing, not about managing agents. It didn't feel like Deckhand |
| Nautical themes: the sea, signal flags spelling the name, rope lines | Tacky when literal; flags read as country flags; the name's origin isn't the product's value |
| The design system applied quietly (slate everywhere, small type, soft motion) | Correct and boring. The character had gone |

To find a new device, start from the essence (many agents, your attention, one place, work kept apart, work landed), and
use the generic playbook's swap test: if the device would work just as well for a different product, it isn't
Deckhand's. Good starting points have something to do with attention, parallel work, or many things becoming one
manageable thing.

Each new piece needs its own take. Reuse the facts and the approved lines where they fit, but don't clone a past
draft's script and structure.

---

## 5. Words

### The voice

Plain, practical and confident. It describes what happens, to "you", in full sentences when explaining and in short
display lines for the hits. It's a little warm, never cute. It's written for developers without sounding like
documentation or an advert.

### Terms

| Use | Not | Why |
|---|---|---|
| coding agents, agents | AI assistants, bots, people | The real term; agents aren't people |
| Claude Code, Codex and Pi | "the agents you already use" | Name them; it was the favourite feature line |
| all your agents | every coding agent | The viewer's own agents, not a spec sheet |
| session | agent, when you mean what Deckhand manages | Deckhand's real unit: an agent running in a session |
| asks a question or needs your OK to run a command | needs you, needs attention (on their own) | Say what it's waiting for |
| flagged, marked | highlighted, notified | What the list does |
| its own worktree; its own branch, in its own folder | its own copy, its own lane | Accurate without patronising |
| review, merge, staged, you commit | ship, land it, done | What really happens |
| in the background, not in the window | always on, forever | True and bounded |
| your terminal, your editor's terminal, your IDE terminal | a new app, a dashboard | Where it actually lives |
| workbench | platform, orchestrator, command centre | The product's own word |

### Lines that were rejected, and why

| Line | Feedback |
|---|---|
| "Your agents are fast. Your attention isn't." | Sounds like AI; ignorable |
| "You started five coding agents. Now you have a second job." | Why five? Shallow |
| "All hands on deck." "Run a tight ship." "You need a deckhand." | Puns; tacky |
| "Parallel agents. Zero chaos." "Many agents. One calm terminal." | Slogans; cringe |
| "One job, one worktree. No collisions." "So they don't clash." | Vague; give the real reason |
| "Close it. They keep working." | Why would anyone close it? Present it as peace of mind |
| "…Come back to everything right where you left it." | Implies it runs with the computer closed |
| "Working, done, or waiting on you. One list." | Too many fragments; write a sentence |
| "Peek at any agent without interrupting it." | Odd; say what it does |
| "Keep your agents working." | Not the main selling point |
| "You steer. Deckhand handles the rest." | Not the essence; vague |
| "…one key away" | Tacky |
| "Every coding agent, side by side, in the terminal you already have open." | "Every coding agent" is distant, and the end adds nothing |

### Punctuation and case

- Display lines (titles, hits, the reveal, taglines, single words) take no full stops, and are set in sentence case or
  in capitals, not title case.
- Full-sentence captions keep their full stops.
- Fragments ("One list for every agent") take no full stop.
- Highlights cover the words only. The full stop or comma after a highlighted word sits outside it.
- Keys in a sentence are shown as keys (a small keycap inside the line), in their real form: `!`, `enter`, `ctrl ]`, `m`,
  `n`, `q`.

---

## 6. The look

### The foundation: Terminal Native, dusk

Deckhand's design system: the look comes from the terminal. Its parts are a character grid, flat colours, monospaced
type, box drawing and the product's own glyphs. It's a quiet, competent tool's voice: clear, plain, confident, a little
playful.

**Tokens**

| Role | Token | Hex |
|---|---|---|
| Ground (slate) | ground | `#4a5260` |
| Surfaces | surface / box / chrome | `#3c434f` / `#3f4651` / `#424955` |
| Terminal background | term | `#363c46` |
| Darkest | inkDark | `#2a2f38` |
| Text | ink / muted / dim | `#f4f2ec` / `#c3c8d0` / `#848c99` |
| Lines | rule / edge / faint | `#59616e` / `#9aa2ae` / `#555d6b` |
| Brand | mag / mag2 (light) | `#f7a8f0` / `#fbcdf6` |
| Selection | cyan | `#86e2ee` |
| Working, done | green | `#b8e69c` |
| Needs you | yel | `#f6d47e` |
| Clash, removal | red | `#ffa29b` |
| An agent's own mark | orange | `#ffb98f` |
| Logo shadow | shadow | `#3a414c` |

**Grid:** a 12 × 24 px cell (on a 1080 px-tall frame), with a faint dot on the ground every two cells. Things sit on
whole cells.

**Type:** Martian Mono for display (its condensed extra-bold cut for poster-style capitals; a hollow, outlined version
for contrast words). JetBrains Mono for the interface and longer text.

### The character: never the quiet version

The system applied quietly (slate everywhere, small type, soft motion, colour only on single words) was rejected as
"way more boring". The final drafts keep each video's character and build it from the system's parts:

- **Bold colour fields** in the system's colours, changing on the beat, each with its meaning (D, G); or **a dark stage**
  in ink-dark that floods with colour on the hits (E).
- **Big condensed capitals** for the short lines and hits, with sentences in a lighter voice. Mixing hollow and solid
  capitals is a strong Deckhand look (E).
- **Highlight bars** behind key words: ink-dark on light fields, yellow for words about waiting on you.
- **Hard one-step shadows** under windows, cards and keycaps, and ink-dark frames around windows on colour fields. No
  blur, glow or gradients.
- **Text big,** especially at the peaks.

### What each colour means in a Deckhand video

| Colour | Means | Typical use |
|---|---|---|
| Magenta | Deckhand, the brand | The logo, the stripes around it, one or two accent words, the typing cursor, the prompt |
| Yellow | Needs you | Waiting agents, flags, "? needs input", words about waiting, the attention count |
| Green | Working; done | A working agent's field or meter, passing checks, the merge, the dev server |
| Cyan | Selection; the list | The selection bar, choosing, the list scene's field |
| Orange | An agent's own | Each agent's own worktree and branch; Claude Code's colour |
| Red | A clash or a removal | The collision, removed lines in a diff |
| Ink-dark | The dark beats | Silence before a reveal, Deckhand closed |
| Cream (ink) | Paper | A quiet field for a reading-heavy moment |
| Slate (ground) | The brand's ground | Behind the logo, always |

**Agent colours:** Claude Code orange, Codex cyan, Pi magenta. Use them on each agent's own title bar, tile, meter or
row. A waiting agent's bar turns yellow whatever its own colour.

**Keep yellow for "needs you".** A key press, a stripe or a decoration is never yellow. Pressed keys invert (dark with
light text) instead.

### Signature devices, used only where they mean something

- **The magenta block cursor:** it leads text while a line is typed, and blinks only at a prompt (the shell's
  `~/code/app ❯ deckhand`, an agent's input, the install command). It never sits blinking at the end of a caption.
- **The light-pink flash:** three frames, on a hit.
- **Magenta stripes** shooting across: around the logo's arrival.
- **Keycaps:** the system's keycap, large, easing in before the press and inverted while pressed.
- **Labels with leaders:** the yellow label with an arrow to the row it's about.

---

## 7. The logo

The logo is the core of the brand, and it's shown exactly as the brand draws it.

- **The mark:** the half-block pixel anchor in flat magenta (`#f7a8f0`) with its hard one-cell shadow (`#3a414c`), beside
  the ANSI-shadow DECKHAND banner in flat magenta with its box-drawing edge lines in dim (`#848c99`). The anchor is on the
  left, the banner is centred on the anchor's height, and there's a three-cell gap between them.
- **The ground:** the slate ground (`#4a5260`) with its dot grid. The shadow colour is made for it.
- **Never:** recoloured (not dark, not cream), stacked, without its edge lines or shadow, given a soft or translucent
  shadow, or placed on a colour field. If a scene's field is magenta, the logo's moment cuts to slate.
- **Energy goes around it:** stripes, a slam (from larger, landing exactly on the mark), a shake, the anchor's pixels
  raining into place, the light-pink flash, a small bump on the beat. It always lands on the exact mark.
- **Small use:** the anchor plus "deckhand" in bold mono.
- In code, `anchor()` and `banner()` in `src/core.js` draw it. A `logo(x, y, cell, sc)` helper with
  `{fill: mag, shadow: shadow}` for the anchor and `{fill: mag, edge: dim}` for the banner, a three-cell gap and the banner
  centred on the anchor, is the reference (see `src/g.js`).

---

## 8. Showing Deckhand

### Draw the real interface

The closer the screen is to the real Deckhand, the more the video is trusted. The window is drawn the way Deckhand draws
itself in a terminal:

- **Header:** `deckhand` and the path (`~/code/app`), the state at the right ("● ready").
- **Sidebar, "Sessions":** numbered rows. Status glyph first: a spinner while working, `?` needs input (yellow), `●` idle,
  `○` exited. Then the title, then the agent's glyph at the right: `✶` Claude, `◇` Codex, `π` Pi. `✓` after the title
  when merged. The selected row is a full-width cyan bar with dark text. The header shows the count and the attention
  count (`8 · ! 3`); with `!` pressed it reads `attention 3/8`.
- **Below the list:** the selected session's title, agent, state, age and branch (`⎇ branch`).
- **The right box, with tabs:** `◈ p:Preview`, `⌁ t:Terminal`, `⑂ g:Git`, `▹ d:Dev`, `✎ a:Notes`. The preview shows the
  worktree path, its state, and the agent's live screen.
- **The key hints along the bottom:** for example `↓ scroll • enter open • x stop • E editor • ? help • n new • W worktrees •
  / search • q quit`.
- **Dialogs:** "New session" (choose an agent, name, workspace: no worktree / new worktree, `tab worktree • enter create •
  esc back`) and the merge ("Merge "<title>"? Into main · ~/code/app", the commits and files, then "Merge into main
  without committing", "Squash merge into main without committing", "Cancel").
- **The Git tab:** "Changes" and "Untracked" with line counts (`+36 -12`), then the diff with green additions and red
  removals.
- **Rounded boxes and the system's window chrome** (outlined dots, round corners).

Check details against `README.md` and the source (`src/app.tsx`, `src/ui.ts`, `src/help.ts`) when in doubt.

### The agents' own screens

Show each agent's real screen inside its session or terminal: Claude Code's welcome, its `●` tool lines with `⎿` results,
its spinner line ("Refactoring token refresh… (63s · esc to interrupt)"), and its permission prompt ("Bash command … Do you
want to proceed? 1. Yes 2. Yes, and don't ask again … 3. No"); Codex's `•` lines ("Explored", "Edited …", "Ran npm test")
and "Ask Codex to do anything"; Pi's plain lines.

### Realistic example work

Use believable tasks in one realistic app, such as these from past drafts: auth refactor (tokens rotate on refresh),
checkout tests (end-to-end), API docs (every route), dark mode (the settings page), upgrade deps, search index (make the
rebuild incremental), rate limiting (the public API), onboarding emails. Mix the three agents across them. Keep titles
short and lowercase, as Deckhand shows them.

### The picture follows the story

- Don't show Deckhand's list before the story introduces it. If the line says you start one agent, show one agent in a
  plain terminal.
- Show the tab the line is about. During "in the background", the preview, not a Git view left over from the merge.
- The scene must match what's on screen: if the line says two agents edit the same file, show both edits landing in it.
- Animations happen when the thing happens. A tile or row appears the moment its agent is created.

### Framing

- Land the camera on the exact row, flag, key or dialog the line mentions, on the beat.
- **Never cut off the interface.** In every held shot, what the line is about is whole: the preview, the permission
  prompt, the tab row, the dialog. Crop only on box borders, the gap between the list and the right box, or between whole
  rows. If it doesn't fit, zoom out or widen the window. Run a cut-text check (see section 12).
- Hold after a key press long enough to see what it did (for example, after `tab` in the new-session dialog).
- Slow down the moments that change state: opening a session full screen, coming back, closing and reopening.

### Helper visuals

Tiles, meters or a wall of terminals can show what the agents are really doing when the interface isn't on screen (in
the tabs act, or while Deckhand is closed). Bring them in where they say something, make them big, and let them fold into
Deckhand's list when Deckhand takes over. Never park them on screen for the whole video.

---

## 9. Sound

What D, E and G share:

- **An original, upbeat house-family score** (D electro house, E driving house, G disco house; tempos around 120 to 128
  BPM). Bright and euphoric, with leads soft enough to live with. Gamer-style and chiptune sounds, and harsh lead synths,
  were rejected.
- **The music tells the story.** Parts thin out as agents stop to wait, there's silence before the reveal, the logo lands
  on the biggest drop, and later drops land on the key actions (`!`, the merge, the close). E made this literal: one
  musical part per agent.
- **One timeline for picture and music.** Every key press, hit and scene change is a position both share, written in bars
  and beats, and the score reads it from the cut (see [section 12](#12-making-it-in-this-repository)).
- **Sound effects that belong to the music** (below): every visible arrival makes a sound, in time and in key.

A new piece can pick a different style within that family, or a different energy for a different purpose, as long as it
stays bright, warm and in the world of the tool. It shouldn't sound like a game, a trailer or stock music. A walkthrough
is the exception in energy: its bed sits under captions, so it is soft, mellow and atmospheric, with no bright attacks.

### Sound effects are part of the score

- **Every visible arrival makes a sound:** a cut, a slam, a key press, a typed character, a row appearing, a panel, a
  notification, a result. A cut with silent arrivals feels unfinished; stripping the effects out to calm a track made it
  sound unpolished.
- **In time:** each effect lands on its picture event, and the event lands on the grid. A whoosh rises into a cut and
  peaks on its frame; a key goes down on the frame the keycap inverts. Effects at raw picture times (between beats)
  sound pasted on: the first kinetic cut's session-spawn plucks did.
- **In key:** pitched effects (chimes, blips, knocks, the notes of a spawn run) play notes of the chord under them, on
  the track's own instruments, so a run of rows plays as an arpeggio of the song.
- **In the mix:** effects share the music's reverb and the kick's sidechain, and sit at a level relative to the music:
  slams near the music's peaks, whooshes and keys 6 to 12 dB under, typing and UI ticks 12 to 24 dB under (in a
  walkthrough, everything quieter still).
- **Varied:** two or three variants of a repeated sound, with small random changes, so repeats don't sound copied.
- **Few big hits:** impacts, crashes and glitch noises only on the moments that earn them (the hook, the reveal, the logo),
  not on every cut.

### What made scores sound wrong

Each of these was heard in a draft and fixed. Check for them before anyone listens.

- **Retro or "farmland":** a square-wave lead, twangy saw plucks, bright bells, a bouncy offbeat bass, and plain major
  triads used as win stingers (C, then F). A softer lead (a filtered saw over a sine) and warmer plucks fixed it without
  losing the energy.
- **Flat and boring:** one loop at one level for the whole piece, or a rebalance that took energy out without adding
  any back. Contrast comes from arrangement: an intro, builds (snare rolls, risers, an opening filter), a stop before a
  big entry, a breakdown, drops on the big moments, the hook coming and going. Not from more bass.
- **Bass-heavy:** generated mixes drift there. Keep the low end well under the mids on laptop and phone speakers.
- **Off and dissonant notes:** a melody looping over itself, phrases not tied to the chord they play over, effects at
  random pitches, an echo out of time with the beat. Every note in key and in the chord under it (the pitch check).
- **Jarring or stinging:** bright electric-piano attacks, stick snares and hats loud in 2 to 5 kHz, where the ear is
  most sensitive.
- **Muddy and rumbling:** everything crowded into 150 to 600 Hz, reverb on the low mids, long kick tails and the
  lowest bass notes. Clearing it by moving parts up an octave as pure tones made the mix thin and whistly instead.
  Balance every octave at once, not one band at a time.
- **Harsh endings:** stacked noise crashes and a sudden stop. Resolve to the home chord and let it ring.
- **Restarting the grid:** a groove that restarts at each scene change stutters. Keep one grid for the whole piece and
  mark changes with a swell and the drums resting.
- **Thin and synthetic:** sine-based instruments sound cheap however balanced they are. When the sound itself is the
  problem, ask for a reference track the owner likes and match its octave balance, or use a produced track.

---

## 10. Motion

- **Smooth by default:** every camera move and layout change eases in and out. Zooms change at a constant geometric rate.
- **Stepped motion only for what really steps:** typing, the cursor, the interface's own redraws. Stepped, "terminal-style"
  camera moves were rejected as jittery.
- **Hits on the beat:** slams, a short shake and the light-pink flash on the big moments only; a small push on lesser
  beats.
- **Scene changes:** a push that lands on the beat, a camera move that carries over, a wipe behind a magenta bar, or the
  layout easing to its next arrangement. Cut hard only on a hit or under a covering change.
- **Vary the layout every beat:** captions at the top, the bottom or in a side column; the window large, narrow or inset;
  keys where there's room.

---

## 11. Formats

- **Wide (1920 × 1080)** for the site, talks and YouTube.
- **Square (1080 × 1080)** for LinkedIn and other feeds, watched on phones, often with the sound off. Recompose it rather
  than crop it:
  - the caption above or below the picture;
  - the window framed closer so its text reads;
  - helpers stacked rather than side by side;
  - every line on screen;
  - text larger relative to the frame.
- Keep the story, words and timing the same across formats so one score serves both.
- **Evergreen or announcement.** A walkthrough, or anything meant to outlive one release, presents Deckhand as the
  project: what it does and how to get it. It leaves out the rewrite ("Rust", "no Node.js") and the version number.
  Those belong to release announcements, such as the first cut and the race.
- **The README video** is uploaded to GitHub, which takes at most 10 MB per video on a free plan. Render the master, then
  re-encode a web copy and check a frame of it before uploading. These settings took the three-minute walkthrough to
  7.7 MB with sharp text:

  ```sh
  ffmpeg -i master.mp4 -c:v libx264 -preset veryslow -tune animation -crf 27 -pix_fmt yuv420p \
    -c:a aac -b:a 128k -movflags +faststart web.mp4
  ```
- Past full pieces ran from about 60 to 90 seconds. Shorter pieces can take a single movement of the arc: the attention
  beat on its own, for example, or the worktree and merge beats, or the reveal and the close.

---

## 12. Making it in this repository

The drafts are built with the tooling in `promo/` (see its README): D, E and G with its first version, the Deckhand 3
drafts (race, kinetic, the walkthrough) with the current one. Both work the same way:

- **`src/<draft>.js`:** each cut is a function of time that returns the frame as HTML (a factory can draw both a wide and a
  square cut). `src/core.js` holds shared helpers: easing, the logo (`anchor`, `banner`), the agent table and fonts.
- **`build.mjs`:** builds each page (`node build.mjs [dir …]`), inlining fonts, styles and scripts, and copies the music
  beside it. `dist/final/` is the final drafts page.
- **`render.mjs`:** renders a cut to MP4, or to stills with `--stills t1,t2,…`, in headless Chromium.
- **`music/`:** the scores, written in Python (`<draft>_house.py`, on `synth.py`), rendered with `python3 music/make.py <id>`.
- **Iterate in the live page, not in renders.** Render MP4s only when a cut is approved.

The checks that kept quality up (write them per cut):

- contact sheets: one frame per second for the whole cut, and 10 to 30 frames per second around every transition;
- safe area and overlap: no text outside the margin, over other text, keys or the window;
- cut text: no interface text partly inside the window's box in a held shot;
- cursors: where each one appears, for how long, and whether it blinks;
- camera: no per-frame jumps except deliberate cuts under a covering change;
- the page loads and every frame draws without errors.

### Picture and music on one grid

- **Write the picture in bars and beats, not seconds.** A cut's tempo and grid are its music's (`SONG = { bpm, offset }`),
  and every event is placed with `at(bar, beat, sixteenth)` (`songGrid` in `src/core.js`). Events then land on the grid
  by construction. Offsets inside a shot (a panel after a key, a line after a title) are counted in beats too; only
  ease durations and reading-paced captions stay in seconds.
- **Scene changes land on the phrase,** not just on a beat: chapters, screens, results and sections start on bar lines,
  and a run of them (feature screens, receipts) is evenly spaced, so each change arrives with the music's phrase. A
  keycap can be a one-beat pickup into its screen on the next downbeat.
- **One event list.** The cut lists every picture event that makes a sound as `SFX_EVENTS` (`{ t, kind, label }`). The
  score reads it, and the timeline, from the cut (`load_draft` in `synth.py`) and turns each event into its sound, so
  picture and music can't drift apart and nothing is copied by hand. When a picture time changes, re-run the score.
- **Measured times stay measured.** A timer or progress that shows real data (a benchmark scaled on screen) keeps its
  true times; label any scaling ("shown 10× slower") and keep the ratio exact.
- **The page plays to the music's clock:** the preview draws each frame from the audio's playback position, and the MP4
  renderer adds the same file.

### The music checks

Scores are generated, so every note and every effect is logged with its time. The checks run on every render of a score
(`music/synth.py`), before anyone listens:

| Check | What it asks |
|---|---|
| Pitch (the audit) | Every note in the key; nothing held a semitone from the chord under it; the lead never plays two notes at once |
| Grid (`check_grid`) | Every effect and hit within 8 ms of the 16th-note grid (typing on its own rate) |
| Phrase (`check_phrase`) | Scene changes on bar lines, runs of them evenly spaced |
| Sync (`check_sync`) | Every picture event has an effect within 20 ms, and every effect has a picture event |
| Effect levels (`check_sfx_levels`) | Each effect, from its own stem, against the music around it, by kind |
| Octaves (`check_octaves`) | Each section against a smooth −3 dB-per-octave slope; mud (150–300 Hz over 600–1200) and sting (2–5 kHz over 0.5–2 kHz) always flagged; a score lists its deliberate character as allowances |
| Loudness (`check_loudness`) | About −14 LUFS for promos and −16 for a walkthrough, true peak under −1 dBTP; normalises the file |

Then look and listen:

- contact sheets, dense around every scene change, with stills at the event times the checks print (`render.mjs
  --stills`, named by time);
- a spectrogram of each score, read for its shape (builds, stops, drops, ring-out);
- the owner listens last. The checks catch faults, not taste: "retro", "flat" or "jarring" are judgements, and the
  list in [section 9](#9-sound) is what they turned out to mean.

---

## 13. Deckhand review questions

- Does it land the one thing: managing many agents is easy, so you can do more in parallel?
- Is the problem felt (agents stopping to ask, waiting, unseen) before Deckhand appears?
- Does the reveal arrive after a breath, on a drop, as the answer?
- Does each feature answer something the viewer has seen?
- Are Claude Code, Codex and Pi named by their real names, and not more often than needed?
- Is every claim true: background, not computer-off; staged, not committed; you decide?
- Does the close show the README's current install? In an evergreen piece, does it leave out the rewrite and the version?
- Are the words plain and specific, with no puns on the name, slogans, AI-speak or arbitrary numbers?
- Are the terms right: session, worktree, flagged, merge, staged?
- Is the interface drawn accurately, in the right state, and whole in every held shot?
- Does yellow appear only where an agent needs you? Do the agents keep their own colours?
- Is the logo exactly the brand's, on slate?
- Is the cursor only where something is typed or a prompt waits?
- Are display lines free of full stops, and full sentences ending with them?
- Is the text big at the peaks, and placed differently from scene to scene?
- Does the square cut read on a phone with the sound off?
- Does the music move with the story, with silence before the reveal and drops on the key actions?
- Does every scene change land on the music's phrase, and every visible arrival make a sound, in time and in key?
- Do all the music checks pass, with any exceptions explained?

---

## 14. Quick reference

```
Essence:     managing many coding agents is easy, so you can do more in parallel
Agents:      Claude Code ✶ (orange) · Codex ◇ (cyan) · Pi π (magenta)
Arc:         multiply → they stop and wait (unseen; colliding) → hand it over → silence → logo
             → one list + preview → flagged, ! → enter, answer, ctrl ] → own worktree
             → review, m, staged → background, reopen → tagline + install
Keys:        n new · ↑↓ select · ! next that needs you · enter open · ctrl ] back
             p t g d a tabs · m merge · q quit
Reveal:      "Hand that job to Deckhand."
What it is:  "A workbench for running coding agents in parallel."
             "One terminal tab for all your coding agents."
             "A lightweight agent workbench for your IDE terminal."
Taglines:    "Run more agents. Watch fewer tabs."
             "Run as many agents as you like. Deckhand makes them easy to manage."
Close:       brew install tejgor/tap/deckhand · "No Homebrew? The README has a one-line installer:
             github.com/tejgor/deckhand" · Free and open source; never the install script's command
Sync:        picture in bars and beats on the music's grid · scene changes on bar lines
             every arrival makes a sound, in time and in key · the checks pass before anyone listens
Colours:     magenta brand · yellow needs you · green working/done · cyan selection
             orange an agent's own · red clash · ink-dark dark beats · slate under the logo
Type:        Martian Mono (display; condensed extra-bold; hollow) · JetBrains Mono (UI, text)
Logo:        magenta anchor + one-cell shadow, magenta banner + dim edges, side by side, on slate, never altered
Cursor:      while typing; blinking only at prompts
Never:       puns on the name, "every coding agent", numbers as claims, "handles the rest",
             "runs while your computer is off", the quiet version of the design system
Evergreen:   a walkthrough presents the project: no "Rust", "no Node.js" or version · the close matches the README
README cut:  under 10 MB for GitHub: re-encode the master (-tune animation -crf 27, AAC 128k)
```
