# Project configuration, trust and workflow features

How Deckhand's features behave, in detail. Back to the [README](../README.md).

Deckhand owns processes, worktrees and navigation. The agents own conversations, approvals and execution. The in-app guide (`?`) lists every key, including the config editor's, and [Keys](keys.md) lists them by screen. To try changes without touching your real Deckhand, use [the isolated dev build](dev-build.md).

## Checkouts and the session list

These docs use two names for places in a repository:

- **Main checkout**: the repository's original working tree, the one that holds `.git`, as opposed to the linked worktrees made from it. It is a place, not the `main` branch.
- **Launch checkout**: the checkout you ran `deckhand` in, the main checkout or a linked worktree, taken at its root even if you ran it from a subdirectory. New worktrees branch from its HEAD by default, `m` merges into its branch and sessions without a worktree run in it.

The sidebar lists the sessions started from the launch checkout, plus any session working inside it. Run `deckhand` in the main checkout to see your usual list. Run it inside one of your worktrees and you get a separate list for that worktree. One daemon serves every Deckhand you have open, in several terminals or several checkouts, so a session shows the same state in each of them. The task list (`b`) is the exception: it shows every checkout of the repository.

## Configuration

Deckhand reads its configuration from `~/.deckhand/config.json`. `DECKHAND_HOME` selects a separate state directory; [the isolated dev build](dev-build.md) manages one for preview testing.

Settings come from two layers with the same schema: your global defaults and a repository's `deckhand.json` (both below). In `deckhand.json` the keys are top-level, as in this example. In `~/.deckhand/config.json` the same object goes under `defaults`. Every setting is optional. Unknown keys, invalid values, empty commands and malformed JSON are rejected.

```json
{
  "defaultAgent": "claude",
  "defaultWorkspace": "new",
  "devCommand": "npm run dev",
  "setupCommand": "npm ci",
  "actions": {"test": "npm test", "typecheck": "npm run typecheck"}
}
```

| Setting | Effect |
| --- | --- |
| `defaultAgent` | `claude`, `pi` or `codex`, preselected for new top-level sessions |
| `defaultWorkspace` | `none`, `new` or `existing`, preselected for new top-level sessions |
| `devCommand` | The Dev command (see [Terminal, Git and Dev panes](#terminal-git-and-dev-panes)), or a name → command map of processes that run side by side, such as a backend and a frontend (see [Several Dev processes](#several-dev-processes)); without one, Dev runs a command named `dev` |
| `setupCommand` | Runs in a newly created worktree before the agent starts |
| `actions` | Named commands offered by `r` (after Dev) |
| `worktree` | Where new worktrees go, what their branch starts from and is called, and what is symlinked into them; see [Worktree settings](#worktree-settings) |

- **Global defaults**: the `defaults` object in your user config (`~/.deckhand/config.json`). You wrote them, so they never need trust. If they are invalid, whatever uses them (new worktree setup, Dev, actions) reports the error. Settings (`C`) shows it too: `←` (the Global column), then `E`, opens them for repair.
- **Repository override**: one `deckhand.json` at the root of the repository's main checkout. It is read live, so uncommitted edits apply at once to every worktree of that repository. Copies of `deckhand.json` inside linked worktrees are ignored. Bare repositories have no main checkout and use global defaults only. The file is limited to 64 KiB and must not be a symlink.
- **Effective settings**: the global defaults, overlaid field by field by the repository override, but only while the override is trusted. `actions` merge by name, with the repository winning, and `worktree` merges per field (below). Untrusted, only global defaults apply. The exceptions are the repository's `defaultAgent` and `defaultWorkspace`: they only preselect the new-session picker, and you confirm them, so nothing runs. They apply untrusted too, and Settings shows them in effect without *needs trust*.

A sub-session's form preselects its parent's agent and directory instead of these defaults.

### Worktree settings

`worktree` describes new worktrees declaratively. A typical setup keeps worktrees beside the repository, shares heavy directories from the launch checkout and links private env files from one canonical place:

```json
{
  "worktree": {
    "location": "{repoParent}/worktrees/{name}",
    "symlink": ["frontend/node_modules", "frontend/.vercel", "backend/.venv"],
    "files": {
      "backend/.env": "{repoParent}/backend.env",
      "frontend/.env.local": "{repoParent}/frontend.env.local"
    }
  }
}
```

| Field | Effect |
| --- | --- |
| `location` | Template for a new worktree's path. Default: the state directory's `worktrees/{name}` (normally `~/.deckhand/worktrees/{name}`). An existing worktree at that path, or an existing branch of the new branch's name, is reused. |
| `branchFrom` | Where a new branch starts. `"current"` (default) is the launch checkout's HEAD. `"default"` is the repository's default branch as a local branch: origin/HEAD's target, else `main`, else `master`, and an error if none exists. `"origin"` runs `git fetch origin <default>` non-interactively with a one-minute timeout, then starts from `origin/<default>`; if the fetch fails, creation fails rather than using a stale ref. The new branch does not track `origin`. The base actually used becomes the session's base, so cleanup and creating a PR (`i`, then `c`, which passes it as `--base`) compare against it. |
| `branchName` | Template for the new branch's name. Default `"{name}"`. Placeholders: `{name}` (required; the sanitised worktree name) and `{user}` (your OS username, slugified), for example `"feat/{name}"` or `"{user}/{name}"`. Obviously invalid templates are rejected when the settings are validated. The expanded name is checked with `git check-ref-format --branch` at creation. |
| `symlink` | Paths relative to the launch checkout's root. Each is linked into the new worktree at the same relative path, pointing at the source's real path (an existing symlink is resolved). Missing sources are skipped. |
| `files` | Destination (relative, inside the new worktree) → source. Sources may use placeholders, be absolute or be relative to the launch checkout. Linked when the source exists, skipped otherwise. |

- **Placeholders** (in `location` and `files` sources): `{name}` (the sanitised worktree name), `{repo}` (the main checkout's folder name), `{repoParent}` (its parent directory), `{repoRoot}` (the main checkout) and `{home}`. A leading `~/` is your home directory. Unknown placeholders are rejected. `location` must contain `{name}` and expand to an absolute path.
- **Validation**: `symlink` entries and `files` destinations must be normalised relative paths (no `..`, `.`, leading `/`, trailing `/` or `.git`). At most 50 entries each.
- **Merging**: a repository `location`, `branchFrom` and `branchName` replace the global ones. `symlink` is the union (global first, duplicates dropped). `files` merge by destination, the repository winning. As with every repository setting, these apply only while `deckhand.json` is trusted.
- **When links apply**: only to a worktree Deckhand just created, never to a reused or selected one. Existing content is never replaced: a destination that exists and is not a symlink is kept, while an existing symlink is replaced. Parent directories are created inside the worktree. Deckhand refuses to go through a parent that leads outside the worktree. Skipped or failed links never fail the session; `i` lists them under **Worktree links**.
- **Cleanup**: a symlink holds no data of its own. Links (any untracked or ignored entry that is a symlink) never block safe deletion, and deleting the worktree removes the links, never their targets.

### Settings (C)

`C` opens one Settings screen for the current repository (the selected session's, else the launch checkout's), using the full width of the terminal. It is a grid: one row per setting, in four sections, and two value columns side by side, **Global (all repos)** and **This repo**:

```text
                       Global (all repos)          This repo
 General
❯ Default agent      ● pi                          —
  Default workspace    —                         ● new worktree
 Commands
  Dev command        ● npm run dev:all             npm run dev ⚠ needs trust
  Actions            ● fmt, test                   test ⚠ needs trust
 Worktrees
  Location           ● ~/Dev/worktrees/app/<name>  —
  Branch from        ● current checkout (built-in) —
```

- **General**: Default agent, Default workspace.
- **Commands**: Dev command (or the names of its processes), Setup command, Actions (the names).
- **Worktrees**: Location, Branch from, Branch name, Linked items (`worktree.symlink` plus `worktree.files`, as a count).
- **Agents**: Agent signals (`agent_hooks`), Notifications (`notifications`) and [Scroll speed](#scroll-speed) (`scroll_speed`). These are global only: they describe your machine and your agent CLIs, not a project. They are stored in `config.json` itself (not under `defaults`), and a repository can never set them. Their This repo cell is empty and can't be selected. On these rows the cursor moves to Global, then returns to This repo when you leave the row. See [Lifecycle signals and notifications](#lifecycle-signals-and-notifications).

Each cell shows that layer's own stored value (`—` when it sets none). Templates show their expansion for this repository, with the template in the details box. `●` marks the value in effect, shown bold; a value the other layer overrides is dimmed. When neither layer sets a value, the Global column shows the built-in default in italics, for example *dev (built-in)*. A value this repository's `deckhand.json` sets while the file is untrusted stays in its cell with `⚠` (*⚠ needs trust* when there is room). The emphasis stays on what applies now. The value applies once you trust the file: you're asked the first time it runs, or press `T`. `defaultAgent` and `defaultWorkspace` apply untrusted too. Lists show a compact summary per layer: action names, or a count of linked items.

The cursor is a cell. `↑`/`↓` (or `j`/`k`) move between settings, `pgup`/`pgdn` and `home`/`end` jump and `←`/`→` (or `tab`) move between the columns. The column stays put while you move up and down. Settings opens on This repo, or on Global outside a Git repository or in a bare one, where only global defaults exist. Below 64 columns of width only the selected column is shown, with a *◂ Global | This repo ▸* indicator; `←`/`→` still switch it. The header shows the file's state (*trusted ✓*, *needs trust*, *not present* or *invalid*) and invalid global defaults. Rows are cut to fit, never wrapped, and the grid scrolls to keep the selection visible.

The box under the grid describes the selected cell in one line (two in narrow terminals). It names the cell's layer and file (`~` for your home) and how it relates to the other column: *overrides global*, *inherited by this repo*, *overridden by this repo* or *not set: inherits global*. For a built-in value it says what that means (*No Dev command is set, so d runs a shell command named `dev`.*). For an untrusted value it says *Applies once trusted — you'll be asked the first time it runs (or press T)*. A value the cell had to cut is shown there in full.

**Keys**: `enter` edits the selected cell, starting from that layer's stored value (an empty input shows the inherited value as a hint), and saves to that layer. `x` clears the cell after a confirmation, and the other layer (or the built-in default) takes over. `E` opens the selected column's raw JSON and returns to Settings. `T` opens the trust review and returns. `esc` closes Settings.

**Edit controls**: the control opens under the grid, titled *<Setting> · <Layer>*; `esc` cancels it.

- **Choices** (`↑`/`↓`, `enter` saves; `◉` marks the layer's own value): agent (`claude`, `pi`, `codex`), default workspace (no / new / existing worktree), and branch from (current checkout, default branch, fetch `origin/<default>` first). Location offers four: inherit (the global location when editing This repo, else Deckhand's default; removes the layer's own `location`), next to the repository (`{repoParent}/worktrees/{name}`), inside it (`{repoRoot}/.worktrees/{name}`, with a warning when `.worktrees/` is not gitignored) or **custom…** (a template). Each location shows the path it resolves to.
- **Text** (one line; `←`/`→` and `home`/`end` move, `ctrl+a`/`ctrl+e` go to line start/end): Setup command, Branch name (with an example for `my-task`) and a custom location (with a preview). Invalid values show the error inline and keep the input open.
- **Actions** (`enter` on the Actions cell): that column's actions only, name → command, with the other layer's actions dimmed underneath as context. Actions are named shell commands you run with `r` on a session. They run in that session's worktree and show on its Terminal tab. `enter` edits the command, `a` adds one in two titled steps and `x` removes it.
  - *Step 1 of 2: name*: letters, numbers, spaces and `_ . -`, starting with a letter or number, up to 48 characters (`test`, `lint frontend`, `db.migrate`). It is checked as you type (*can't start with a space*, *can't contain "/"*, …), and `enter` keeps the step open while it is invalid. It also says what saving does: *adds a new action to this repo*, *replaces the existing test action (this repo)* or *overrides the global test action in this repo*. A layer holds at most 30 actions; at the limit the step says so.
  - *Step 2 of 2: command for <name>*: runs with your shell (`$SHELL -ic`) in the session's worktree, like typing it in a terminal, so `&&`, pipes, `cd` and env vars work. For example `npm test`, `cd backend && .venv/bin/pytest -x` or `make lint`. It must not be empty (at most 8192 characters).
- **Dev command** (`enter` on the Dev command cell): a list of what Dev runs in that column, the other layer's shown dimmed underneath while this one sets none. With nothing set it holds only **+ Add a process**. Adding or editing a process takes two steps: its command, then its name, pre-filled from the command when adding (`cd frontend && npm run dev` → `frontend`, `python3 server.py` → `server`) and with its own name when editing, so `enter` twice keeps the suggestion. One process is the [Dev command](#terminal-git-and-dev-panes); more run as [several Dev processes](#several-dev-processes). `x` removes one. A layer holds at most 8, and one layer's Dev replaces the other's whole. Settings saves `devCommand` as a name → command map; a plain command you write by hand still works, listed under its derived name.
- **Linked items** (`enter` on the Linked items cell): the untracked and ignored entries of the main checkout (`git status --ignored=matching --untracked-files=normal`, so directories collapse; at most 200, then "+N more"). Sizes are computed in the background (`…` while pending, `?` when unknown or too slow), and symlink targets are shown. Configured entries are marked as link, with the layers that list them. Configured entries that no longer exist show as "missing in checkout". Everything else is suggested. Dependencies and env files (`node_modules`, `.venv`, `venv`, `vendor`, `.vercel`, `.env`, `.env.*`, `*.env`) are suggested as link. Build output, caches and clutter (`dist`, `build`, `out`, `coverage`, `.next`, `.turbo`, `.cache`, `__pycache__`, `.pytest_cache`, `.DS_Store`, `*.log`) and everything else are suggested as skip. `space` toggles link/skip, `a` adds a path that isn't listed, and `enter` saves the column's `worktree.symlink`. Its own entries keep their order, and entries the other layer links stay linked. `worktree.files` entries are listed read-only: edit them in the raw JSON.

Each confirmed edit saves immediately through the same safe save as the JSON editor, preserving every other key. The status line says where it saved and what happened to trust: *Saved Dev command to this repo · still trusted*, *· created and trusted* or *· review required (the file had changes you haven't reviewed)*. Global saves never involve trust. A file that changed on disk since Settings read it rejects the edit; Settings reloads and says what was not saved.

**Raw JSON editor** (`E`): a missing file opens from a starter draft, and nothing is written until `ctrl+s`. `ctrl+f` formats the JSON. Malformed JSON can be opened for repair, and the repository file keeps its permissions. `esc` or `ctrl+c` returns and asks before discarding edits. A second `ctrl+c` at that prompt, or `ctrl+c` while a save is in flight, quits the UI (unsaved edits are lost).

Saving never runs anything or commits. Edits you make in Deckhand keep `deckhand.json` trusted if it was trusted (or new); changes from outside need review (see [Trust](#trust-inline-review)).

### Trust (inline review)

A repository's `deckhand.json` is trusted by its exact content. A repository without one has nothing to trust. Trust gates running, not seeing or choosing: every list shows every option, and you're asked when something from the repository is about to run. Deckhand then shows its exact bytes inline, with what is about to run on top:

- **New sessions** (`n` and `N`) open the picker at once, with the repository's `defaultAgent`/`defaultWorkspace` preselected even untrusted. Only confirming a new worktree that would use untrusted repository parts reviews first: its `setupCommand` or `worktree` location, branch or link settings. `enter` trusts and creates, `s` creates with global settings only and `esc` returns to the form. No-worktree and existing-worktree sessions never ask.
- **Actions** (`r`) lists the worktree's Dev command first, then global and all repository actions. Untrusted repository ones are marked *· needs trust*. Choosing one reviews first: `enter` trusts and runs it, `s` cancels that run (or runs the global action of the same name, if there is one) and `esc` returns to the list. Global and trusted actions run at once.
- **Starting Dev** (`d` twice quickly, or `r` then `enter` on **Dev**), when the override defines a `devCommand` (one command or several processes): `enter` trusts and starts it, and `s` starts the global (or built-in) Dev command instead.
- **Retrying setup** (`s` on a session whose setup has not completed), when the override defines a `setupCommand`: `s` in the review retries without it.

`esc` always backs out without running anything. In Settings (`C`), `T` opens the same review. Tabs are shown as spaces, and invisible or bidirectional characters as `<U+XXXX>`.

- Trust lives in your user `config.json`, never in the repository. It is keyed per repository: by its main checkout, or by its Git common directory for bare repositories. It keeps the 20 most recently trusted fingerprints, so any change to the bytes needs a fresh review, except your own edits made in Deckhand (next point).
- **Edits you make in Deckhand keep the file trusted if it was trusted (or new); changes from outside need review.** A save through Settings, its Actions and Linked items editors or the raw JSON editor (`E`) trusts the new bytes in two cases: the version it replaced was trusted, or there was no `deckhand.json` to review. The check uses exactly the bytes you edited from (the save's revision check), so a file changed since you last trusted it does not carry over. The status then says *review required (the file had changes you haven't reviewed)*, and you're asked once, with the whole file shown. Edits from outside Deckhand (your editor, `git pull`, an agent) always need a review.
- A trust request is checked against the reviewed fingerprint. If the file changed after you opened the review, the request is refused and the review shows the new bytes.
- The daemon enforces trust independently of the prompt: untrusted repository setup, Dev and action commands never run.

> [!WARNING]
> Trust is permission to run these strings, not a sandbox. Commands run as you, with your files and environment. Cancellation and timeouts signal the command's process group, but background processes a command detaches are not tracked.

### Setup and actions

- **Setup** runs only for sessions that create a new worktree, before the agent launches, with a ten-minute timeout. While it runs, Preview shows the most recent output. If you continued without trusting the override, only the global `setupCommand` (if any) runs. If the override changed after review, setup does not run and the session says so. If setup fails or you cancel it (`x` or `X` while starting), the worktree is kept and the agent is not started. `s` retries setup with the current effective settings, reviewing first if needed. Sessions in an existing worktree or the current directory never run setup.
- **Actions** (`r`) run a named command from the effective settings in the session's worktree, in a process of their own. They never run in your shell or in Dev, so a running dev server is untouched. The Terminal tab switches to the action: its header shows `shell │ test ● running`, then `✓ exit 0` or `✗ exit 1`. `v` switches between the shell and the action, and `enter` (or `o`) attaches to the action while it runs. Every session in that worktree shares it, whether or not the session's agent is running. One action runs at a time per worktree, and nothing is queued: running another while one runs is refused. `x` in the `r` list stops the running action (or Dev). Its output and exit code stay visible until the next action replaces them.
- **New worktrees** are created with `git worktree add` at the effective `worktree.location`, using `branchFrom` and `branchName`; `worktree` links are applied afterwards.
- If a new worktree could not be registered, the session can't be restarted in the original checkout; create a new session instead.

Known limitation: a `deckhand.json` that is not valid UTF-8 can't be opened in the editor; fix it outside Deckhand.

## Cleanup

When you stop a worktree session with `x`, Deckhand inspects the worktree before offering deletion, and checks again at deletion time. `x` asks before stopping any session and can delete the worktree and its branch too. `X` force-stops at once and never deletes anything.

- **Structural protections can never be overridden**: the main checkout, the session's launch checkout (where you ran `deckhand` when you created it), a worktree used by another active session, sessions without a worktree, a worktree Git no longer lists and (when deleting the branch) `main`/`master` or a branch that changed since inspection. Deletion is not offered for them.
- **Data that blocks safe deletion**: modified or staged files, untracked files and ignored files other than `node_modules` (for example `.env` or build output). Entries that are symlinks (for example links from [worktree settings](#worktree-settings)) do not count. Commits matter only when the branch is deleted too or HEAD is detached. Then commits not reachable from any other local branch or remote-tracking ref block deletion. If any of this can't be determined, deletion is blocked.
- Typing `DELETE` authorises data loss and overrides every data check above, including dirty files. Force-stopping with `X` does not authorise data loss.
- Git evidence is local: there is no implicit fetch, so unknown upstreams stay conservative. A squash merge leaves the branch's commits "unmerged" as far as Git knows. So the merged marker (`✓`, see [Merging and done](#merging-and-done)) records the commit that was merged: Deckhand's own merge, a kept conflicted one or the head of a PR found merged. Marking a worktree merged by hand with `M` records none, since Deckhand can't verify it. That commit and its ancestors count as integrated when deleting the branch. Commits made after it still need `DELETE`, and clearing the marker (`M`) makes them all count again.
- A failed removal is reported as **Worktree retained…** with the Git error; it never silently succeeds.

Deletion applies to the worktree, not just the stopped session. Every session that was in it (including sessions that joined it with **Existing worktree**, and sub-sessions) can no longer be resumed or merged, and its Terminal, Git and Dev tabs say the worktree was deleted. A worktree created later at the same path is a new one: those sessions don't join it, and it starts without their markers.

Review and stage changes in the Git tab (`g`, see [Git changes](#git-changes)); attach lazygit there (`o`) for commits and everything else.

### Worktrees (W)

`W` lists every worktree of the repository in the right pane. It shows Git's list, including worktrees made with `git worktree add` (marked *not from Deckhand*), plus worktrees Deckhand had that Git no longer lists. It runs merge detection first. Then it checks each linked worktree (up to 60, four at a time) for what deleting it with its branch would lose (the same inspection as `x`), its last commit and its commits not in the default branch (local or `origin/`, never fetched). Groups:

- **Merged · safe to delete**: merged (Deckhand's merge, found merged into the default branch, a merged PR or `M`), or clean with no commits beyond the default branch. Deleting it with its branch loses nothing.
- **Merged · has leftovers**: merged, but something is still there: uncommitted, untracked or valuable ignored files, commits made after the merge or work that is not verifiably merged (for example a squash marked with `M`).
- **In progress** and **Idle**: not merged. Idle means nothing running and no commit, session or record activity for 14 days.
- **Missing**: Git lists it but the directory is gone, or only Deckhand's record is left.
- **Main checkout**: listed last for its sessions, never deleted.

Each row shows its sessions (or how many run), changed files, commits of its own and age. The details below the list name the sessions, the path and what deleting it would lose. `enter` selects its session, `M` sets or clears the merged marker, `E` opens it in your editor and `R` checks again.

`x` asks first: delete the worktree and keep its branch, or delete both. Deleting both is offered first for merged work, and never applies to `main`, `master` or the default branch. Sessions still running in it are named in the confirmation and stopped before anything is deleted. Deckhand then checks again, since they may have written files. The data checks are those of [Cleanup](#cleanup): anything that would be lost needs `DELETE` typed. Never deleted from here: the main checkout, this Deckhand's launch checkout, the launch checkout of another open Deckhand, a locked worktree and one with a session still starting. `x` on a missing worktree does what its state allows, and you don't choose. It is pruned (`git worktree prune`; its branch stays) while Git still lists it, or forgotten when only Deckhand's record is left.

`x` on the **Merged · safe to delete** heading (not on a row) deletes every worktree in the group and its branch. One confirmation lists them and the sessions it stops. Each is checked again on its own, and any that fail are named.

After a deletion, the worktree is marked deleted for every session of it (as with `x` on a session), and its exited sessions are archived. They can't be resumed, and `f` `A` shows them with their notes, still readable.

## Merging and done

`m` merges (or squash-merges) the selected session's worktree into the branch of the [launch checkout](#checkouts-and-the-session-list), without committing, so you review and commit the result. The confirmation offers **Merge**, **Squash merge** and **Cancel**, and shows, before anything runs:

- `Into <branch> · <path>`: in yellow when the target is not the repository's main checkout or not on its default branch, so a surprising target stands out.
- The commits that would be merged (count, then each one's short hash, age and subject, up to 100 before `+N more`) and the diff stat of `<target>...<source>`. A long list scrolls with `J`/`K`.
- `N uncommitted files` when the worktree has uncommitted (or untracked, not ignored) files, with a toggle that is on by default: `☑ commit them first ("<session title>")`. `space` switches it; off, they stay in the worktree. On, Deckhand runs `git add -A` and `git commit -m "<session title>"` in the worktree first, and your Git hooks run. If that fails, its output is shown and nothing is merged. A worktree with only uncommitted work is merged this way.
- The open steps still in the worktree's note, on a row of their own: they go to the backlog with the merge (see [Tasks](#tasks)). `space` or `enter` on that row keeps them in the note instead.
- A warning when the target has uncommitted changes in files the merge would touch, since Git would refuse or mix them. Unrelated uncommitted files are fine. A merge, rebase, cherry-pick or revert in progress in the target (or unresolved conflicts) is named, and the merge is refused until it is finished.
- The notes, which give way first when the pane is short.

On conflicts, one small view lists the conflicted files and offers two keys. `enter` (or `esc`) keeps the merge in progress, with conflict markers in the files, and marks the worktree merged now. Resolve the conflicts in your editor or the Git tab's Merge Conflicts group, then commit. `x` aborts it with `git merge --abort`. A squash is not tracked by Git as a merge, so for a squash Deckhand restores the target with `git reset --merge`, keeping your unrelated uncommitted edits. It does that only for a squash it started itself on an unchanged HEAD; otherwise it refuses and says so.

`✓` (merged) belongs to the worktree: every session in a linked worktree shows it, and `M` from any of them sets or clears it. It is also set by itself when the branch was merged outside Deckhand. That means its tip is in the default branch (local, or `origin/<default>` as last fetched; Deckhand never fetches) and it has at least one commit of its own beyond where it started. A fresh branch, or one only fast-forwarded to a newer main, never counts. `i` → `P` finding its PR merged also sets it (GitHub squash merges included). This is checked at daemon start, every five minutes and whenever `i` runs. It never unmarks, and after `M` clears a found marker it stays cleared until the branch gets new commits. Sessions in the main checkout have nothing to merge: `M` says so and points at `space`.

`space` marks any session done (or not done): `☑` in its row, a muted title and `☑2d` (done 2 days ago) after the branch in the details. It is independent of merged (a worktree session can be both) and of archiving; handoffs mention it. If an older Deckhand marked a main-checkout session merged with `M`, it shows as done instead.

## Organizing and inspecting sessions

- **Archiving**: `A` archives or unarchives a session without stopping it. The footer offers it for exited and archived sessions, and archiving from the active view says where the session went.
- **Removing**: `backspace` archives an exited session; nothing is lost, and `f` `A` shows the archived view. `backspace` on an archived session removes it for good, and always asks first: `enter` removes it, `esc` keeps it. Its notes move to `notes/trash/`: its own, and its worktree's when it is the last session there (see [Notes](#notes)). When those notes still have open checklist items, they are listed. `enter` moves them to the backlog first, and `x` removes the session without moving them.
- **Filter**: `f` opens the filter menu at the top of the sidebar, listing each filter with its key and how many rows it would list. One key picks it: `a` active, `r` running, `!` attention, `e` exited, `A` archived and `*` all. So any filter is two keys away (`f` `r`), and `f` `f` goes back to active. `j`/`k` show each filter as you move, `enter` keeps it and `esc` restores the one you had. Active means unarchived, not necessarily running. The sidebar header names any other filter (or a search) in cyan with its `shown/total` count, and shows `! N` when N sessions need you. `!` selects the next of them (sessions blocked on you first, then unread replies), switching to the attention filter and clearing any search.
- **Search**: `/` searches title, notes (the session's and its worktree's), tasks, agent, branch and path. `enter` keeps the query; `esc` clears it. Matching children keep their ancestors visible for context, and searches and filters show matches inside collapsed trees.
- **Persistence**: selection, per-session tabs, collapsed and hidden sessions, filter and search persist per repository in `ui-state.json`.
- **Terminal title**: Deckhand titles its terminal `deckhand · <repository>`. Quitting gives the terminal its previous title back where the terminal supports that.
- **Notes count**: the Notes tab reads `✎ a:Notes ☐ N` while the notes it shows (the session's and its worktree's) have open checklist items.
- **Sidebar width**: until you choose one, the sidebar is fitted to the sessions: 30% of the terminal, or less when every listed row fits in less (never under 24 columns). It is measured at launch and when the terminal is resized, not as sessions come and go. `<` and `>` move it 2% of the terminal from where it is (15% to 50%) and save that share as yours, for every repository, as `sidebar_width_percent` in `~/.deckhand/config.json`; from then on it is used exactly, even when titles need more or less. `=` forgets it and fits the sidebar to the sessions again.
- **Inspecting**: `i` shows the worktree's branch and HEAD, changed and untracked counts, diff size, the base comparison and the cached upstream ahead/behind. `↑`/`↓`, `j`/`k` and `pgup`/`pgdn` move through it, and `esc` closes it. Inside it, `P` asks the optional `gh` CLI for PR and check status, `b` opens the PR (https URLs only), `c` creates a PR and `g` opens the Git tab. Missing tools, auth or network are reported without blocking local use. Reopen `i` to refresh.
- **Creating a PR**: `c` first asks: *Push <branch> to <remote> and open GitHub's new-PR form in your browser?* Only `enter` proceeds; `esc` cancels. Deckhand then runs `git push -u <remote> <branch>`, never forced. The remote is the branch's upstream remote, else `origin`. Then it runs `gh pr create --web --head <branch>`, or `gh pr view --web` if an open PR already exists. `--base` is passed only when the session's base branch exists on that remote (for example `origin/main`); otherwise `gh` chooses. It refuses a detached HEAD, `main`/`master`, the session's base branch, sessions without a worktree branch and a branch that changed since you confirmed. Prompts are disabled and stdin is closed, so it never waits for input; push and `gh` failures are shown as errors. Press `P` afterwards to see the new PR.

## Terminal, Git and Dev panes

Terminal (your `$SHELL`), Git (the Changes view, plus `lazygit` on `o`) and Dev belong to the session's worktree, or, for sessions without a worktree, to the checkout they run in. Every session in the same worktree sees the same shell, the same changes, the same lazygit and the same Dev command. Attaching (`o`) from any of them opens that one process, also after the session's agent has exited. Git state belongs to the worktree too, so agents sharing a worktree see the same changes.

- **Starting and stopping**: the shell starts the first time you view the Terminal tab, and lazygit the first time you attach it. One that exited (`exit`, `q`) starts again the next time you view (Terminal) or attach it. They stop when the worktree is deleted, when the worktree's last session is removed or when the daemon stops. While a new worktree is still being created, these tabs say so; a deleted worktree has none, for every session that was in it.
- **Dev command**: press `d` twice quickly to start or stop it, or press `r` and choose **Dev**, always first in the run list. A single `d` shows the Dev tab and its output. `enter` opens Dev full screen only while it runs. Starting or stopping it from any session of the worktree acts on that one process. It keeps running after the agents exit (exited sessions still show it, attach to it and stop it). It runs until you stop it, delete the worktree or remove the worktree's last session, or until the daemon stops. The sidebar shows `▶` on the worktree's first row while it runs, and a muted `▶` on every other session that shares it.
- **Setting the command**: set `devCommand` in Settings (`C`), or by hand. In `deckhand.json` it is a top-level key:

  ```json
  {"devCommand": "npm run dev"}
  ```

  In your user `config.json` it goes under `defaults`, like every per-project setting: `{"defaults": {"devCommand": "npm run dev"}}`.

  With no Dev command set, Dev runs a literal `dev`. The older top-level `dev_command` still works, but only in the user `config.json` (never in `deckhand.json`): it is used when the global defaults set no `devCommand`.

### Several Dev processes

A project with a backend and a frontend (or a worker, a database, a tunnel) can run them all as Dev, each in its own process, with no tmux or wrapper script. Make `devCommand` a map of names to commands, in Settings (`C`, `enter` on the Dev command, then **+ Add a process**) or in `deckhand.json`:

```json
{
  "devCommand": {
    "backend": "cd backend && source .venv/bin/activate && exec python3 main.py",
    "frontend": "cd frontend && exec npm run dev"
  }
}
```

Each command runs with your shell (`$SHELL -ic`) in the session's worktree, so `cd` into a subdirectory first. Names follow the action rules (letters, numbers, spaces, `_ . -`), and a layer holds at most 8.

- **Starting and stopping**: `d` `d` (or **Dev** in the `r` list) starts every process and stops them all. Like a single Dev command, they belong to the worktree, are shared by its sessions and keep running after the agents exit. The sidebar's `▶` and the tab's dot show while any of them runs.
- **The Dev tab** splits into one cell per process, each with its name, command and state (`● live`, `✓ exit 0`, `✗ exit 1`, `○ stopped`) over its live output. They sit side by side while each column is at least 40 wide, otherwise stacked. `v` selects the next process (`▸` marks it), and `enter` opens the selected one full screen; Deckhand selects it again when you come back.
- **One at a time**: the `r` list shows a row per process after **Dev**. `enter` starts a stopped one alone, or restarts a running one (it waits for the old process to exit, so its port is free); `x` stops it and leaves the others running.
- **One worktree at a time**: see below; it applies to processes as a whole.

### Dev in one worktree at a time

Every worktree of a repository runs the same Dev config, so its servers want the same ports. Dev therefore runs in one worktree of a repository at a time: starting it in one worktree (`d` `d`, or a process from the `r` list) first stops it in the repository's other worktrees, and the status line says where (*Started Dev · stopped it in fix-login*). Other repositories are not affected.

## Scroll speed

While attached, and while you scroll the preview, Deckhand scales trackpad and mouse-wheel scrolling by the scroll speed. Pick it in Settings (`C` → **Scroll speed** in the Agents rows: 0.5, 1, 2 or 3, or **custom…** to type any number from 0 to 3), or while scrolling a preview (`→` on Preview) press `+` / `-` to raise or lower it by 0.1. Either is saved immediately and applies to both. Or edit the user config directly:

```json
{
  "scroll_speed": 1
}
```

The default, `1`, passes every wheel event on as your terminal sends it. Lower values scroll slower and `0` ignores vertical wheel events; values up to `3` scroll faster (each event is sent several times). Whole numbers scroll most evenly: Deckhand can only pass on whole wheel events, so a speed like `1.5` sends one, then two, in turn. Below `1`, the first notch of a scroll always moves, and a pause or a change of direction starts over, so a short flick never does nothing.

Version 2's `attach_scroll_sensitivity` (default `0.12`) is kept in `config.json` as it was but no longer read: its values were tuned to the old behaviour and would now scroll very slowly.

Every agent scrolls a fixed distance per wheel event, so the multiplier alone sets the speed:

- **Claude** is launched scrolling one line per event with its wheel acceleration off: `--settings` carries `CLAUDE_CODE_SCROLL_SPEED=1` in its `env` and `wheelScrollAccelerationEnabled: false` (Claude Code 2.1.174 or later), and the environment repeats the speed for a Claude without `--settings`. Claude otherwise guesses a speed from the terminal it was started in (for agents in Deckhand, whichever terminal started the daemon) and speeds up on fast scrolls, which the scaled event stream confused. This overrides a speed you saved with `/scroll-speed`, inside Deckhand only. To keep your own Claude scroll settings and acceleration in Deckhand, set this in the user config; it has no Settings (`C`) row and applies to sessions started after the change:

  ```json
  {
    "claude_scroll_override": false
  }
  ```

- **Codex** scrolls `tui.mouse_scroll_speed` rows per event (1 unless set) and does not accelerate.
- **Pi** scrolls one line per event on macOS; on Linux, or when started over SSH, it accelerates on fast scrolls unless `fullscreenWheelScrollLines` is set to a number in its settings.

In the preview of a fullscreen agent, `g` / `G` send Ctrl+Home / Ctrl+End, which all three treat as the top of the conversation and the latest output.

## Colours

Deckhand uses your terminal's own colours, so it follows your terminal theme: magenta for the brand and focused boxes, cyan for the selection, yellow for what needs you, and green and red for success and errors.

It can also draw itself in the dusk palette of [the design system](brand/deckhand-design-system.html), on its own slate background. That is mainly for recording demos and screenshots in the brand's colours. It needs a terminal with 24-bit colour. Turn it on in the user config (`~/.deckhand/config.json`); there is no Settings (`C`) row for it:

```json
{
  "theme": "dusk"
}
```

Remove the key (or set `"terminal"`) to go back. Deckhand reads it when the UI starts and each time you come back from an attached session, so quit and reopen the UI after changing it. The status line of an attached view (and under a note in Vim) follows the same setting. Only Deckhand's own UI changes colour; agents' output keeps its own colours.

## Git changes

The Git tab (`g`) is a built-in, read-mostly take on VS Code's Source Control panel for the selected session's worktree. You rarely need to open the worktree in an editor just to see what changed. lazygit is still there for everything else: `o` attaches it, and it starts on that first attach, not when you open the tab.

- **Groups**: in VS Code's order, each with a count: **Merge Conflicts**, **Staged Changes**, **Changes** (unstaged changes to tracked files) and **Untracked**. Then **Committed on this branch**: every file the branch's commits changed since it left the branch the worktree was created from (for a worktree made by your creation script or attached, which records none, the default branch: local, else `origin`'s), so an agent that commits as it goes still shows its work. A file with staged and unstaged edits is listed in both Staged and Changes. Empty groups are hidden; with nothing to show, the tab says *No changes*. The header counts each group (`● clean · 12 committed` when only commits remain). Sessions in the main checkout have no Committed group. The list shows at most 2000 files in all, and a group that was cut ends with *+N more* (its count stays exact).
- **Rows**: the status letter (`M` modified, `A` added, `D` deleted, `R`/`C` renamed/copied, `T` type change, `U` conflict, `?` untracked), the file name with its directory dimmed after it, `old → new` for renames and `+added -removed` lines. Binary files show `bin`. Untracked files count their lines, up to 500 files of at most 1 MB.
- **Refresh**: while a Deckhand window shows the Git tab of a worktree, the daemon reads `git status` about every two seconds, without taking Git's index lock. It updates every window looking at any session of that worktree, but only when something changed. It reads again right after you stage or unstage. The Committed group is read again only when the branch or its base moves. Leaving the tab stops it.
- **Browse** (`→` or `l`, like Preview; keys in [Git changes](keys.md#git-changes)): the selection follows the file when the list refreshes. When the file moves to another group (you staged it), the selection goes to the next file of the group it left, or to the first file below that group once it is empty.
  - **Staging**: `space` stages the selected file from Changes, Untracked or Merge Conflicts; staging a conflict marks it resolved, as in VS Code. On a staged file it unstages it, and a rename unstages both paths. A committed file is already committed, so `space` says so.
  - **Everything**: `a` stages everything (`git add -A`). When there are merge conflicts it stages everything else and leaves the conflicted files for you to stage one by one once resolved. `A` unstages everything staged; an ongoing merge stays in progress.
  - **Opening**: `enter` or `E` opens the file in Cursor or VS Code at its first changed line (`-g file:line`; VS Code reuses its last active window by default). A deleted file can't be opened.
  - **Scrolling**: `J`/`K` scroll the diff preview by three lines, `pgup`/`pgdn` by a page.
- **Diff preview**: beside the list from 100 columns of pane width, below it otherwise. Staged files show `git diff --cached`, changes and conflicts `git diff`, untracked files their contents as added lines (a symlink shows its target), and committed files everything the branch changed in them across its commits (`git diff <merge-base> HEAD`). Additions are green, deletions red and hunk headers cyan. It is read-only and cut at 256 KB; binary files say so.
- **Safety**: only paths in the current `git status` of that worktree can be staged or unstaged, passed literally (no globs). Before the first commit, unstaging removes the file from the index (`git rm --cached`). Deckhand never commits, discards, edits files, stages hunks or switches branches here; use lazygit (`o`) for that.

## Notes

The Notes tab (`a`) shows the note of the place you work. A session in a worktree has one note, **Worktree · *branch* (shared by N sessions)**. Every session in that worktree sees and edits it, including sessions that joined it with **Existing worktree**, and sub-sessions. It lives as long as the worktree, not any one session. A session in the main checkout has two sections: **Main checkout (shared by N sessions)**, the repository's note, and **This session · *title***, its own. Main-checkout sessions are usually separate jobs. Each header shows its open checklist items (`☐ 2 open`). Height is split between the sections: an empty one is one line, and when both are long each keeps at least three rows (the one you are editing gets more). Without editing, the notes are rendered from the top, checklist items as ☐/☑ (checked ones dimmed), ending in *+N more lines* when they don't fit.

- **Editing**: `enter` (or `→`) edits the section you edited last, else the session's; `esc` stops. `tab` switches section in the main checkout, keeping each one's cursor. The editor's keys are in [Notes editor](keys.md#notes-editor). Long lines wrap at word boundaries and the view scrolls with the cursor.
- **Checklists**: lines like `- [ ] item`, `- [x] item` or `* [ ] item`, indented or not. `ctrl+x` checks or unchecks the cursor's line (a line without a checkbox gets `- [ ] `), and `ctrl+t` adds an item below. `enter` on an item starts the next one, and `enter` on an empty item ends the list. The sidebar's details show `☐ 3 open (2 worktree)` for the selected session.
- **In vim**: `o` on the Notes tab opens the note (the section you edited last, else the session's) full screen in your terminal editor: `$VISUAL`, else `$EDITOR`. Only when both are unset does Deckhand fall back to `nvim`, `vim` or `vi`, whichever is installed. Quit it (`:wq`, `ctrl+]` or `ctrl+space`) to come back to the Notes tab, edits included. In vim and nvim, `ctrl+x`, `ctrl+t` and `enter` on an item work as in Deckhand's editor, in that note only; vim's own meanings of those keys are kept in every other buffer. `ctrl+p` is not there: send items from the task list's Notes view (`b`, `tab`, then `a` / `A`). Other editors just open the file.
- **Files**: each note is a Markdown file in the state directory's `notes/`: `sessions/<session>.md` for a session's own, `worktrees/<id>.md` for a worktree and `repos/<hash>.md` for the main checkout. `E` on the Notes tab (or `ctrl+o` while editing) opens it in Cursor or VS Code, creating it if needed. Edits saved there appear in Deckhand within a second or two. Deckhand autosaves about 0.3 s after you stop typing, and only if the file is still the version you were editing. If it changed in between, Deckhand reloads it and tells you instead of overwriting it. Each note holds up to 50 000 characters; a longer file is shown cut, and you edit it in your editor. When upgrading, notes an older Deckhand kept in `state.json` move into these files on the first start.
- **Notes from older versions**: older Deckhands gave each session in a worktree a note of its own. Those notes are merged into the worktree's note when the daemon starts. When there are several, each goes under a `## <session title>` heading. The originals go to the trash.
- **Lifetime**: removing a session or a worktree never deletes a note with text in it. A session's note moves to `notes/trash/` when you remove the session, and a worktree's when its last session is removed. Trash files are named by date, kind and what they belonged to. Empty notes are deleted instead. A worktree deleted with its session keeps showing the note, read-only, until its last session is removed. A new worktree at the same path starts with an empty note. The main checkout's note is never removed.
- **Where notes show up**: handoffs (`H`) include a **Worktree notes** section, the merge confirmation (`m`) shows the worktree's note above the sessions' and `/` searches both notes.
- **Sending to Tasks**: `ctrl+p` while editing sends the cursor line's open checklist item to the repository's task list (below). The line becomes `- ↗ <item> <!-- dh:t=<id> -->`, shown as *↗ item · in Tasks*.

## Tasks

There are two things: tasks, one list per repository, and notes, one per worktree (and per main-checkout session). A worktree has one task, `◆`, the work it is for. Its steps are its note's checklist, which the task list shows under the task. Notes stay scratch space: a note's checkbox only becomes a task when you send it (`ctrl+p` while editing, or `a` / `A` in the task list's Notes view). It then lands in the backlog, because something worth a task is work of its own.

`b` opens the task list in the right pane. It is the same in every Deckhand of the repository, whichever checkout it was opened in: it shows every checkout's sessions and notes, while the sidebar keeps to its own. Each worktree (or main-checkout session) with an open task gets a group, headed `⎇ branch` with the most urgent state of its sessions (needs you, working, idle, exited). The task `◆` sits in the group with its steps under it (`◆ Ship parser 2/5`, then `☐`/`☑` rows). Then come **Backlog** and **Done · this week**. Done tasks older than a week fold into one row; `enter` or `space` on it shows or hides them. The task list opens on the selected session's worktree, and `v` shows only that worktree's tasks (and back). The header shows `☐ N tasks · b` while any are open.

- **Adding and editing**: `a` adds, depending on the selection. In a worktree's group it adds a step at the end of its note (the editor says *New step · the note of ⎇ branch*). In a worktree's group that has no task yet (for example after `v` on a session started from the sidebar, which shows *No open tasks here*), it adds that worktree's task. Elsewhere it adds a backlog task. `A` adds a backlog task wherever the selection is. An empty backlog's *Nothing waiting* row can be selected (`G`), and the footer says `a add step` or `a add task`. `enter` edits a task: the title, then `tab` for its details. `enter` saves from the title, `ctrl+s` from the details, and `esc` cancels.
- **Other keys**: `space` ticks or reopens a task, `x` twice deletes, and `J`/`K` reorder within a group (done tasks can't be reordered). `j`/`k` (`↑`/`↓`) and `g`/`G` move, `o` selects the session doing it and `E` opens the file in Cursor or VS Code.
- **Steps**: the checklist in a worktree's note (a main-checkout session's own note, for work in the main checkout) is the breakdown of its task. `space` on a step ticks or unticks it in the note, revision-checked like typing in the Notes tab. `enter` opens the note. The session's Notes tab banner says `2/5 steps`. The task list only reads the note, so it stays plain Markdown.
- **Moving** (`w`): to the backlog, back to the note it was sent from, or to a worktree or main-checkout session. **↩ Back to its note** is listed while a note still holds its `↗` line. The line becomes the checklist item again, with its details indented under it, and the task leaves the list. Onto work without a task (a session started from the sidebar, say), it becomes that work's task, exactly as if the session had been started from it with `n`. Onto work that has one, it becomes a step: an open item at the end of its note (or on its `↗` line, if that note sent it), and it leaves the list. Each row says which (`◆ its task` / `→ a step in its note`). The menu has two lists, **Worktrees** and **Sessions** (main checkout), and `tab` switches between them. `j`/`k` or `↑`/`↓` choose. `/` searches names, branches and session titles (`enter` keeps the search, `esc` clears it, as in the sidebar). `esc` clears a kept search, then cancels.
- **Starting a session** (`n` on a backlog task): the usual new-session form opens, named after the task (cut to 64 characters), in a new worktree by default, with *Task:* and *Base:* lines. `↑`/`↓` choose where the new branch starts: the `worktree.branchFrom` setting (first), or any local branch, most recently committed first. The task becomes the task of the session's worktree (in the main checkout, of the session).
- **Typing the task**: its text (`<title>: <details>` on one line) is typed into the agent's input but not sent, so you edit it and press `enter` yourself. Deckhand types it once the agent's screen has not changed for 0.7 s (or its activity turns idle), with no menu or question showing. That rules out Claude's and Codex's folder-trust prompts and numbered menus. It types the text as one bracketed paste, so no character acts as a shortcut. If you type letters into the agent first, or it doesn't settle within two minutes, nothing is typed.
- **Notes view** (`tab`): every note with text, in full, under a heading per worktree in sidebar order (its note), then **main checkout** (its note, then each session's own). Each note has its name and open-item count. Every line is listed (checklist items as ☐/☑, `↗` links, headings). `a` on an open item sends it to the backlog as a task, and `A` sends every open item of the selected note at once. `enter` opens the note in its session's Notes tab, `f` shows only open items (and back) and `v` only the selected session's worktree (or, in the main checkout, its note and the main checkout's). Empty notes and deleted worktrees' notes are left out. A sent item's line becomes `↗ item`, which says where its task is: *in Tasks*, *done* or *not in Tasks* (deleted).
- **The end of the work**: merging the worktree (`m`, `M` or merged found by Deckhand) ticks its task, and the merge confirmation names it. Marking done (`space`) ticks it once every session of the worktree is done. Undoing the merged marker or done reopens it. When the worktree is deleted unmerged, or its last session (or a main-checkout session) is removed, its open task goes back to the backlog marked *tried in ⎇ branch*.
- **Nothing left behind**: merging (`m`) or deleting the worktree (`x` on a session, or `W`) lists the open steps still in its note. It sends them to the backlog first, marked *left open in ⎇ branch*, and each becomes a `↗` line in the note. `space` on the confirmation keeps them in the note instead. Removing a session for good does the same with its notes' open items when you choose to (`enter` on its confirmation). Merges Deckhand finds by itself and `M` do not move them.
- **Where the task shows**: the session's Notes tab starts with **◆ Task · title**, its steps (`2/5 steps`) and its state. Its sidebar details add `◆ title` and count the worktree's other open tasks. `/` finds sessions by their task's title, and handoffs (`H`) start with a **Task** section.
- **File**: `notes/tasks/<hash of the main checkout>.md` in the state directory. A task is a top-level checklist item, the indented lines below it are its details, and every other line (headings, prose) is kept as written. Deckhand's bookkeeping is a `<!-- dh:t=… wt=… s=… done=… auto=… tried=… from=… added=… -->` comment at the end of the item's line: the task's ID, the worktree or session it is for and when it was added or done. Each change is applied to the file as it is on disk, never a stale copy, so edits made in an editor are kept. Items added there get an ID with Deckhand's next change.
- **Follow-up tasks from older versions**: before steps existed, a worktree could also have *follow-up* tasks (`assigned=` in the file). After upgrading, the daemon moves each open one into its worktree's note (or its main-checkout session's) as an open item, once. One whose work is gone stays in the backlog.

## Sessions and sub-sessions

When you create a session (`n`), Deckhand launches it in one of three workspace modes; `tab` in the create form switches between them:

| Mode | Behaviour |
| --- | --- |
| **No worktree** | Runs in the [launch checkout](#checkouts-and-the-session-list) |
| **New worktree** | Creates or resolves a worktree for the session; `↑`/`↓` pick the branch it starts from |
| **Existing worktree** | Picks from the repository's existing worktrees, including the main checkout |

New worktrees are created with `git worktree add` at the configured `worktree.location`: by default the state directory's `worktrees/`, normally `~/.deckhand/worktrees/`. The `worktree` setting also chooses the new branch's start point and name and symlinks heavy directories and private files into each new worktree. Settings (`C`) shows what is in effect and edits it in place; see [Worktree settings](#worktree-settings).

`N` on a selected session creates a sub-session for related follow-up work. Sub-sessions render indented under their parent in the sidebar. `c` on a parent cycles its sub-sessions: hide the exited ones, collapse, expand. A sub-session defaults to its parent's current directory, so it opens in the parent's worktree unless you choose a different mode.

- **Clean**: choosing `claude`, `pi` or `codex` creates a clean sub-session: a fresh agent context in the parent's directory or worktree.
- **Fork**: choosing **⑂ Fork parent** forks the parent's conversation into a new one (Claude `--fork-session`, Pi `--fork`, `codex fork`). Nothing is typed into the agent. A fork copies the conversation as saved at that moment, so a turn still in progress in the parent isn't included.
- **Where forks run**: forks start in the parent's worktree. Claude and Pi forks can go into a new or existing worktree instead (`tab` in the create form). The child's first message then tells it which worktree it is in and not to touch the parent's. That is because the copied conversation's paths point at the parent's worktree, and the parent's uncommitted changes aren't in the new one. Codex forks always stay in the parent's worktree, because Codex may reopen a fork in the directory it was recorded in.
- **Forking Codex**: a Codex parent can be forked once its conversation ID is known, from Codex's SessionStart hook or its exit hint.
- **From a handoff**: **↳ From its handoff** creates a clean child from the handoff exported with `H` (see [Resume and handoffs](#resume-and-handoffs)).

## Agent identity

Claude and Pi sessions get an exact conversation ID (a UUID) chosen by Deckhand at launch. They also get a readable session label, `dh-{title}-{id}`: the session title, sanitised and cut to 40 characters, then the first 8 characters of the Deckhand session ID. A fresh restart (`S`) adds a `-fresh-…` suffix. With agent signals on, Claude is also passed `--settings <hooks JSON>` (see [Lifecycle signals](#lifecycle-signals-and-notifications)).

**Claude**:

```text
create   claude --session-id <uuid> --name dh-{title}-{id}
resume   claude --resume <uuid>
fork     claude --resume <parent-uuid> --fork-session --session-id <child-uuid> --name dh-{title}-{id}
```

An unknown ID asks you to press `S`, which creates a fresh ID. Claude forks can go into any worktree.

**Pi**:

```text
create   pi --session-id <uuid> --name dh-{title}-{id}
resume   pi --session-id <uuid>
fork     pi --fork <parent-uuid> --session-id <child-uuid> --name dh-{title}-{id}
```

`S` creates a fresh ID. Pi forks can go into any worktree.

**Codex**:

```text
create   codex                      (the conversation ID is captured from its hook or exit hint)
resume   codex resume <id>          (only when the ID is known; otherwise press S)
fork     codex fork <parent-id>     (the parent's ID must be known; parent's worktree only)
```

- **Pi's files**: Pi session files live in Pi's normal session tree at `~/.pi/agent/sessions/`, not under `~/.deckhand`. They stay visible in Pi's own `/resume` UI under the readable label, and survive deletion of Deckhand state.
- **Sessions from older versions**: sessions an older Deckhand recorded without an exact conversation ID keep resuming by their stored name (Claude, `claude --resume <name>`) or session path (Pi, `pi --session <path>`).
- **Claude's exit hint**: Claude prints a `claude --resume "..."` command when it exits. Deckhand parses that final preview and keeps the conversation ID when it finds one. If Claude reports `No conversation found with session ID`, Deckhand shows a hint to press `S` instead of starting fresh silently.
- **Fresh restarts**: `S` restarts an exited session without its previous conversation ID.
- **Forks**: a forked sub-session stores the parent's conversation ID. Claude and Pi children get their own exact ID at launch. A Codex child's ID is captured like any Codex session's, never the parent's. `s` resumes the child's own conversation. Some forks fork the parent again with a new child ID on `s`: one that never launched, one whose fork failed (the parent had no saved conversation yet; a conversation is saved once it has a message) and one whose Codex ID was never reported.
- **Forks from older versions**: forks an older Deckhand made before it used `--fork-session` keep working. One stored with its `/branch` name resumes by that name; one still holding the parent's conversation ID forks again.

## Resume and handoffs

Claude and Pi get an exact conversation ID at launch (see [Agent identity](#agent-identity)), so `s` reopens that conversation even after a rename, a duplicate name or a fork. Codex's conversation ID is captured from an authenticated SessionStart callback or its `codex resume <id>` exit hint. `s` resumes a known conversation and `S` starts a fresh one. An unknown ID is an error: if Claude reports no conversation for its ID, the exited session says to press `S`, and an unknown Codex ID refuses `s`. Deckhand never guesses with `--last` or starts a blank conversation silently.

After a daemon crash, sessions are marked interrupted and keep their conversation IDs, notes and archive state. This is conversation recovery, not reconnection to a live process.

`H` writes a private Markdown handoff to the state directory's `handoffs/` and opens it in Cursor or VS Code when available. It holds the notes, worktree metadata and a **Workspace changes** section, not a transcript. After reviewing it, `N` → *↳ From its handoff* creates a clean child session whose initial prompt is the document's path. It is not a fork and never types into a running terminal; export again after notes change. A handoff never includes terminal output, because it may contain secrets.

**Workspace changes** is gathered from Git when the handoff is exported. It contains the base ref, commits since the base (up to 30, then "+N more"), uncommitted files with their status (up to 50) and a committed diff stat against the base (`<base>...HEAD`: files changed, insertions, deletions and per-file counts). It lists only file names and numbers, never diff content. If Git fails, the section says *Git information unavailable* and the export still succeeds. Sessions outside a Git repository, or whose worktree was deleted, have no such section.

## Agent versions and updates

`U` opens the Agents screen (full width, like Settings). It has one row per agent with:

- its installed version (`<agent> --version`, found on the daemon's PATH, the one sessions launch with);
- its latest release (the npm `latest` dist-tag of `@anthropic-ai/claude-code`, `@openai/codex` and `@earendil-works/pi-coding-agent`);
- a status (`up to date`, `update available`, `not installed`, `latest unknown`);
- how many running sessions use an older version.

Opening it looks the latest releases up again, up to about 10 s each; `r` repeats that. The daemon also checks in the background when it starts and at most every 6 hours. Offline, or without npm, the latest is just unknown: there is no error.

`enter` runs the selected agent's own updater, non-interactively (stdin closed, the daemon's environment, at most 5 minutes): `claude update`, `codex update` or `pi update --self` (Pi itself, not its packages). If the agent has running sessions, you are asked first. The details box shows the running state, then the new version or the updater's exit code and the last lines of its output. Leaving the screen does not stop it, and the footer says when it finished. Only one update per agent runs at a time. Agents that are not installed are left to `deckhand setup`. `esc` goes back.

Updating never restarts or touches sessions. Each launch (create, `s`, `S`) records the version it started with. A running session whose version is older than the one now installed shows a dim `↑` before its agent glyph (the first marker dropped on a narrow sidebar). Its details line names both versions (`claude 2.1.287 · 2.1.290 installed`). Restart it (`x`, then `s`) to pick up the new build. Exited sessions are never marked: their next start uses the installed version. When any installed agent has a newer release, the header's right side says so quietly (`codex update · U`, `2 agent updates · U`).

## Lifecycle signals and notifications

Agent signals have three states. Change them in Settings (`C` → the Agents rows in the Global column) or as `agent_hooks` in the user config (`~/.deckhand/config.json`, or `~/.deckhand-dev/config.json` for the dev build), keeping existing settings:

| `agent_hooks` | Agents that report signals |
| --- | --- |
| not set (the default) | Claude only |
| `true` | Claude and Codex (Codex needs its hooks, below) |
| `false` | Neither |

Notifications (`notifications`) are off unless you switch them on:

```json
{"agent_hooks": true, "notifications": true}
```

Notifications on their own still notify when a session exits; agent signals add *needs you* and *response ended*. Enable agent signals before creating or restarting sessions, since running agents are not reconfigured.

**Notifications on macOS** come from **Deckhand**, with its anchor icon. macOS asks once whether to allow them: see [Allowing notifications on macOS](#allowing-notifications-on-macos). Elsewhere, Deckhand uses `notify-send`. Either way, notifications are best effort.

- **States**: supported hooks report **working**, **needs-input**, **response-ended**, **failed** and **limited**. Everything else stays unknown or activity-based. **Response-ended is not task success**, and silence is not completion. `!` jumps to the next session that needs you, including failed or interrupted exits.
- **Why it waits**: the session details under the list, a started task's details in the task list and the notification say what a session waits on, in one or two lines. That is `asks: <question>` for a question tool (`(+N)` when it asks more), `plan ready: <first line>` for plan approval, or a permission prompt: `wants to run: <command>`, `wants to edit: <file>`, `wants to fetch: <host>` or `wants to use: <tool>`. When a turn ends it is `said: <last line of its reply>`, and on failure the failure (`rate limited`, `API overloaded`, …). The agent's own generic notification text never replaces one of these. A question or plan approval counts as needs input, not working. This text is held in memory only: `state.json` never stores it.
- **Approvals**: the agent's own approvals stay in its terminal. Attach to it (`enter`) to answer.
- **Settling**: a **working** signal on a screen that has settled (no change for 5 s) becomes unknown, so the session shows idle. Claude sends no hook when `esc` interrupts it.
- **Claude**: unless `agent_hooks` is false, new launches check `claude --help` and pass hook settings for the supported events with `--settings` (the same flag also carries the [scroll settings](#scroll-speed), with or without hooks). They are `async` hooks, so the agent never waits on Deckhand. Repository and user agent config is not modified. `deckhand hooks` (no argument) prints these Claude-style settings.
- **Codex**: with `agent_hooks: true`, launches add `--no-daemon` when the installed Codex supports it, so callback identity is not shared through a Codex daemon. The isolated dev build requires `--no-daemon` even with signals off, and refuses Codex versions without it. Codex hooks can't be passed at launch, so set them up once in Codex's own config:
  1. Run `deckhand hooks codex` (`scripts/deckhand-dev.sh hooks codex` for the dev build).
  2. Save its output as `$CODEX_HOME/hooks.json` (by default `~/.codex/hooks.json`). If the file exists, merge it into its `hooks` object, replacing stale Deckhand entries. Check `config.toml` in the same directory for stale entries too.
  3. Trust the hooks with `/hooks` in Codex.

  Deckhand never writes to the Codex home. Check your version's [Codex hooks reference](https://learn.chatgpt.com/docs/hooks) for which events it supports.
- **Checking Codex hooks**: Settings checks them only while agent signals are on and a Codex home exists. It then shows **⚠ Codex** on the row, with the command above, when no hook calls this Deckhand or one calls another Deckhand install. A hook that still calls the old Node bridge counts as another install. Settings never names stale `node … cli.js hook` entries; `deckhand doctor --check` (read-only) does, in `hooks.json` and `config.toml`, whether or not signals are on.
- **Pi**: no automatic adapter.
- **Callbacks** must carry the launch's secret token and launch ID. Stale and unauthorised callbacks are rejected, and one that started before the last applied callback is dropped (async hooks can arrive out of order). A native subagent's events are ignored except its permission prompts, which block the session's terminal, and the tool use after one (the prompt was answered).
- **The `deckhand hook` bridge** reads bounded input and forwards only lifecycle fields: event name, session and agent IDs, tool name, notification and error types. It also forwards, each bounded, what the reason line is made from: a question tool's questions and options, a plan, a permission prompt's command, file path or URL, the end of the last reply and a notification's text. Other tool input and all tool output stay in the hook process. It never starts a daemon and always prints `{}`, so it never approves, blocks or alters an agent action.
- **When notifications fire**: the daemon sends one when a session starts needing you (and when a waiting session's reason changes) and when a process exits. Their text is the reason line when there is one (a question, a command awaiting permission, the last line of a reply), never terminal output.

See also [Claude's hook guide](https://code.claude.com/docs/en/hooks-guide).

### Allowing notifications on macOS

macOS asks once whether Deckhand may show notifications. Deckhand makes sure you see that question:

1. Switch Notifications on in Settings (`C`, the Agents rows). Deckhand sends a test notification straight away, so macOS's prompt appears while you are looking.
2. Allow Deckhand in the prompt.

If you missed the prompt, dismissed it or chose not to allow:

- Open System Settings › Notifications › Deckhand and switch on **Allow Notifications**. Choose **Persistent** as the alert style if you want each one to stay until you dismiss it.
- Deckhand appears in that list only once it has asked. If it isn't there, switch Notifications off and on in Settings to send the test again.
- While macOS blocks Deckhand or still waits for an answer, the Notifications row in Settings shows `⚠ macOS` with the fix under it. `deckhand doctor --check` reports the same, and the daemon notes it once in `~/.deckhand/daemon.log`.

While the prompt is unanswered, or after you turn notifications off in System Settings, Deckhand shows nothing in their place.

**How it works**: the daemon keeps a small background app, `Deckhand.app`, in the state directory (`~/.deckhand/Deckhand.app`). It is a copy of the binary, signed ad hoc, registered with LaunchServices and rebuilt when the binary changes. Your answer carries over to new versions, because macOS remembers it by the app's identifier (`io.github.tejgor.deckhand`). If the app can't post at all (for example when the state directory is under a temporary directory, where macOS ignores apps), Deckhand falls back to `osascript`, which shows the Script Editor icon.
