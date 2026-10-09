# Project configuration, trust and workflow features

Deckhand owns processes, worktrees and navigation; native agents own conversations, approvals and execution. The in-app guide (**?**) lists every key, including the config editor's. To try changes without touching your real Deckhand, use [the isolated dev build](dev-build.md).

## Configuration

Settings come from two layers with the same schema. Every setting is optional; unknown keys, invalid values, empty commands and malformed JSON are rejected.

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
| `defaultAgent` | `claude`, `pi` or `codex` preselected for new top-level sessions |
| `defaultWorkspace` | `none`, `new` or `existing` preselected for new top-level sessions |
| `devCommand` | Command for the Dev pane; falls back to the legacy user-level `dev_command`, then `dev` |
| `setupCommand` | Runs in a newly created worktree before the agent starts |
| `actions` | Named commands offered by **r** (after Dev) |
| `worktree` | Where new worktrees go, what their branch starts from and is called, whether the creation hook is used, and what is symlinked into them; see [Worktree settings](#worktree-settings) |

- **Global defaults**: the `defaults` object in your user config (`~/.deckhand/config.json`). You wrote them, so they never need trust. If they are invalid, whatever uses them (new worktree setup, Dev, actions) reports the error; **C** (Settings) shows it, and **←** (the Global column) then **E** opens them for repair.
- **Repository override**: one `deckhand.json` at the root of the repository's **main checkout**, read live (uncommitted edits apply at once to every worktree of that repository). Copies of `deckhand.json` inside linked worktrees are ignored. Bare repositories have no main checkout and use global defaults only. The file is limited to 64 KiB and must not be a symlink.
- **Effective settings** are the global defaults overlaid field by field by the repository override, **only while the override is trusted**; `actions` merge by name with the repository winning, and `worktree` merges per field (below). Untrusted, only global defaults apply, except the repository's `defaultAgent` and `defaultWorkspace`: they only preselect the new-session picker (you confirm them, nothing runs), so they apply untrusted too and Settings shows them in effect without *needs trust*.

Child sessions keep their parent's agent and directory instead of the defaults.

### Worktree settings

`worktree` describes new worktrees declaratively, so most projects no longer need a creation hook. A typical setup (this reproduces a hook that keeps worktrees beside the repository, shares heavy directories from the launch checkout and links private env files from one canonical place):

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
| `branchFrom` | Where a new branch starts: `"current"` (default; the launch checkout's HEAD), `"default"` (the repository's default branch as a local branch: origin/HEAD's target, else `main`, else `master`; an error if none exists) or `"origin"` (runs `git fetch origin <default>` non-interactively with a one-minute timeout, then starts from `origin/<default>`; if the fetch fails, creation fails rather than using a stale ref). The new branch does not track `origin`. The base actually used becomes the session's base, so cleanup and **c** (create PR, `--base`) compare against it. |
| `branchName` | Template for the new branch's name. Default `"{name}"`. Placeholders: `{name}` (required; the sanitized worktree name) and `{user}` (your OS username, slugified), e.g. `"feat/{name}"` or `"{user}/{name}"`. Obviously invalid templates are rejected when the settings are validated; the expanded name is checked with `git check-ref-format --branch` at creation. |
| `hook` | `false` switches the `.claude/scripts/create-worktree.sh` creation hook off entirely (default `true`): it is not run, not part of the trust fingerprint and never needs review. |
| `symlink` | Paths relative to the launch checkout's root. Each is linked into the new worktree at the same relative path, pointing at the source's real path (an existing symlink is resolved). Missing sources are skipped. |
| `files` | Destination (relative, inside the new worktree) → source. Sources may use placeholders, be absolute, or be relative to the launch checkout. Linked when the source exists, skipped otherwise. |

- **Placeholders** (in `location` and `files` sources): `{name}` (the sanitized worktree name), `{repo}` (main checkout folder name), `{repoParent}` (its parent directory), `{repoRoot}` (the main checkout), `{home}`. A leading `~/` is your home directory. Unknown placeholders are rejected. `location` must contain `{name}` and expand to an absolute path.
- **Validation**: `symlink` entries and `files` destinations must be normalized relative paths (no `..`, `.`, leading `/`, trailing `/` or `.git`). At most 50 entries each.
- **Merging**: a repository `location`, `branchFrom`, `branchName` and `hook` replace the global ones; `symlink` is the union (global first, duplicates dropped); `files` merge by destination, the repository winning. As with every repository setting, these apply only while `deckhand.json` is trusted, with one exception: `"hook": false` in the repository file disables the hook even while the file is untrusted, because it can only stop a script from running. Enabling never bypasses trust: an enabled hook is part of the reviewed fingerprint and runs only once trusted, so a repository `"hook": true` overriding a global `false` takes effect only after you trust it.
- **While a hook is active** (present, enabled and trusted) it decides the location and the branch, so `location`, `branchFrom` and `branchName` are ignored; links still apply after it runs.
- **When links apply**: only to a worktree Deckhand just **created**, never to a reused or selected one. A trusted creation hook still decides the location, and the links are applied after it runs. Existing content is never replaced: if the destination exists and is not a symlink it is kept, while an existing symlink is replaced. Parent directories are created inside the worktree. Deckhand refuses to go through a parent that leads outside the worktree. Skipped or failed links never fail the session; **i** lists them under **Worktree links**.
- **Cleanup**: a symlink holds no data of its own, so links (any untracked or ignored entry that is a symlink) never block safe deletion, and deleting the worktree removes the links, never their targets.

The creation hook (below) remains the escape hatch for anything these settings cannot express.

### Settings (C)

**C** opens one Settings screen for the current repository (the selected session's, else the launch directory's), using the full width of the terminal. It is a grid: one row per setting, in three sections, and two value columns side by side, **Global (all repos)** and **This repo**:

```
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
- **Commands**: Dev command, Setup command, Actions (the names).
- **Worktrees**: Location, Branch from, Branch name, Linked items (`worktree.symlink` plus `worktree.files`, as a count), Creation hook (on/off).
- **Agents**: Agent signals (`agent_hooks`) and Notifications (`notifications`), on/off. These are **global only**: they describe your machine and your agent CLIs, not a project, so they are stored in `config.json` itself (not under `defaults`) and a repository can never switch them on. The This repo cell says *global only* and can't be selected: on these rows the cursor moves to Global, then returns to This repo when you leave the row. See [Lifecycle signals and notifications](#lifecycle-signals-and-notifications).

Each cell shows **that layer's own stored value** (`—` when it sets none); templates show their expansion for this repository, with the template in the details box. **●** marks the value in effect (what `resolveSettings` uses, via `explainSettings`), shown bold; a value the other layer overrides is dimmed. When neither layer sets a value, the Global column shows the built-in default in italics, e.g. *dev (built-in)*. A value this repository's `deckhand.json` sets while the file is untrusted stays in its cell with **⚠** (*⚠ needs trust* when there is room): the emphasis stays on what applies now, and it applies once you trust the file — you're asked the first time it runs, or press **T**. `defaultAgent`, `defaultWorkspace` and `worktree.hook: false` apply untrusted too. Lists show a compact summary per layer: action names, or a count of linked items.

The cursor is a **cell**: **↑↓** move between settings, **←→** (or **Tab**) between the columns, and the column stays put while you move up and down. Settings opens on **This repo** (on Global outside a Git repository or in a bare one, where only global defaults exist). Below 64 columns of width only the selected column is shown, with a *◂ Global | This repo ▸* indicator; ←→ still switch it. The header shows the file's state (*trusted ✓*, *needs trust*, *not present* or *invalid*) and invalid global defaults. Rows are cut to fit, never wrapped, and the grid scrolls to keep the selection visible.

The box under the grid describes the selected cell in one line (two in narrow terminals): its layer and file (`~` for your home) and how it relates to the other column — *overrides global*, *inherited by this repo*, *overridden by this repo*, *not set: inherits global* — or, for a built-in value, what it means (*No Dev command is set, so d runs a shell command named `dev`.*), or for an untrusted value *Applies once trusted — you'll be asked the first time it runs (or press T)*. A value the cell had to cut is shown there in full.

**Keys.** **Enter** edits the selected cell, starting from that layer's stored value (an empty input shows the inherited value as a hint), and saves to that layer. **x** clears the cell after a confirmation; the other layer (or the built-in default) takes over. **E** opens the selected column's raw JSON and returns to Settings. **T** opens the trust review and returns. **Esc** closes Settings.

**Edit controls.** The control opens under the grid, titled *<Setting> · <Layer>*; Esc cancels it.

- **Choices** (↑↓, Enter saves; ◉ marks the layer's own value): agent (`claude`, `pi`, `codex`), workspace (no / new / existing worktree), branch from (current checkout, default branch, fetch `origin/<default>` first), creation hook (on/off), and location: inherit (the global location when editing This repo, else Deckhand's default; removes the layer's own `location`), next to the repository (`{repoParent}/worktrees/{name}`), inside it (`{repoRoot}/.worktrees/{name}`, with a warning on its own line when `.worktrees/` is not gitignored) or **custom…** (a template). Each location shows the path it resolves to.
- **Text** (one line; ←→/Home/End move, Ctrl+A/Ctrl+E line start/end): Dev command, Setup command, Branch name (with an example for `my-task`) and a custom location (with a preview). Invalid values show the error inline and keep the input open.
- **Actions** (Enter on the Actions cell): that column's actions only, name → command, with the other layer's actions dimmed underneath as context. Actions are named shell commands you run with **r** on a session; they run in that session's worktree and show on its Terminal tab. Enter edits the command, **a** adds one in two titled steps, **x** removes it.
  - *Step 1 of 2: name*: letters, numbers, spaces, `_ . -`; starts with a letter or number; up to 48 characters (`test`, `lint frontend`, `db.migrate`). It is checked as you type (*can't start with a space*, *can't contain "/"*, …; Enter keeps the step open) and says what saving does: *adds a new action to this repo*, *replaces the existing test action (this repo)*, *overrides the global test action in this repo*. A layer holds at most 30 actions; at the limit the step says so.
  - *Step 2 of 2: command for <name>*: runs with your shell (`$SHELL -ic`) in the session's worktree, like typing it in a terminal, so `&&`, pipes, `cd` and env vars work; for example `npm test`, `cd backend && .venv/bin/pytest -x` or `make lint`. It must not be empty (at most 8192 characters).
- **Linked items** (Enter on the Linked items cell): the untracked and ignored entries of the main checkout (`git status --ignored=matching --untracked-files=normal`, so directories collapse; at most 200, then "+N more"), with sizes computed in the background (`…` while pending, `?` when unknown or too slow) and symlink targets. Configured entries are marked as link (with the layers that list them), configured entries that no longer exist show as "missing in checkout", and everything else is suggested: dependencies and env files (`node_modules`, `.venv`, `venv`, `vendor`, `.vercel`, `.env`, `.env.*`, `*.env`) as link; build output, caches and clutter (`dist`, `build`, `out`, `coverage`, `.next`, `.turbo`, `.cache`, `__pycache__`, `.pytest_cache`, `.DS_Store`, `*.log`) and everything else as skip. **Space** toggles link/skip and **Enter** saves the column's `worktree.symlink` (its own entries keep their order; entries the other layer links stay linked). `worktree.files` entries are listed read-only: edit them in the raw JSON.

Each confirmed edit saves immediately through the same safe save as the JSON editor, preserving every other key, and the status line says where and what happened to trust: *Saved Dev command to this repo · still trusted*, *· created and trusted*, or *· review required (the file had changes you haven't reviewed)*; global saves never involve trust. A file that changed on disk since Settings read it rejects the edit; Settings reloads and says what was not saved.

The raw JSON editor (**E**): a missing file opens from a starter draft, nothing is written until **Ctrl+S**, malformed JSON can be opened for repair, and the repository file keeps its permissions. Esc or Ctrl+C returns and asks before discarding edits; a second Ctrl+C at that prompt, or Ctrl+C while a save is in flight, quits the UI (unsaved edits are lost).

Saving never runs anything or commits. Edits you make in Deckhand keep `deckhand.json` trusted if it was trusted (or new); changes from outside need review (see [Trust](#trust-inline-review)).

### Trust (inline review)

A repository's `deckhand.json` and its optional `.claude/scripts/create-worktree.sh` creation hook are trusted together, by exact content. A repository with neither has nothing to trust. A hook switched off with `worktree.hook: false` is not part of the trust at all. Trust gates **running**, not seeing or choosing: every list shows every option, and you're asked when something from the repository is about to run. Deckhand then shows its exact bytes inline, with what is about to run on top:

- **n** and **N** (new sessions) open the picker at once, with the repository's `defaultAgent`/`defaultWorkspace` preselected even untrusted. Only confirming a **new worktree** that would use untrusted repository parts (its `setupCommand`, an enabled creation hook, or `worktree` location/branch/link settings) reviews first: **Enter** trusts and creates, **s** creates with global settings only, **Esc** returns to the form. No-worktree and existing-worktree sessions never ask.
- **r** lists the worktree's Dev command first, then global and all repository actions; untrusted repository ones are marked *· needs trust*. Choosing one reviews first: **Enter** trusts and runs it, **s** cancels that run (or runs the global action of the same name, if there is one), **Esc** returns to the list. Global and trusted actions run at once.
- Starting Dev (**d** twice quickly, or **r** then Enter on **Dev**), when the override defines a `devCommand`: **Enter** trusts and starts it, **s** starts the global (or built-in) Dev command instead.
- **s** on a session whose setup has not completed, when the override defines a `setupCommand`: **s** in the review retries without it.

**Esc** always backs out without running anything. In Settings (**C**), **T** opens the same review. Tabs are shown as spaces and invisible or bidirectional characters as `<U+XXXX>`.

- Trust lives in your user `config.json`, never in the repository. It is keyed per repository (by its main checkout, or by its Git common directory for bare repositories) and keeps the 20 most recently trusted fingerprints, so any change to the bytes needs a fresh review — except your own edits made in Deckhand (next point).
- **Edits you make in Deckhand keep the file trusted if it was trusted (or new); changes from outside need review.** A save through Settings, its Actions and Linked items editors or the raw JSON editor (**E**) trusts the new bytes when the version it replaced was trusted, or when there was no `deckhand.json` and no creation hook to review. The check uses exactly the bytes you edited from (the save's revision check) together with the creation hook as it is now, so a file or hook changed since you last trusted it does not carry over: the status says *review required (the file had changes you haven't reviewed)* and you're asked once, with the whole file shown. A creation hook is never trusted through a save: a save that switches an unreviewed hook on, or creates the file next to one, needs a review too. Edits from outside Deckhand (your editor, `git pull`, an agent) always need a review.
- A trust request is checked against the reviewed fingerprint: if the file changed after you opened the review, it is refused and the review shows the new bytes.
- The daemon enforces trust independently of the prompt: untrusted repository setup, Dev and action commands and creation hooks never run.

Trust is permission to run these strings, **not a sandbox**. Commands run as you, with your files and environment. Cancellation and timeouts signal the command's process group, but background processes a command detaches are not tracked.

### Setup, actions and creation hooks

- **Setup** runs only for sessions that create a new worktree, before the agent launches, with a ten-minute timeout. While it runs, Preview shows the most recent output. If you continued without trusting the override, only the global `setupCommand` (if any) runs. If the override changed after review, setup does not run and the session says so. If setup fails or you cancel it (**x**/**X** while starting), the worktree is kept and the agent is not started; **s** retries setup with the current effective settings, reviewing first if needed. Sessions in an existing worktree or the current directory never run setup.
- **Actions** (**r**) run a named command from the effective settings in the session's worktree, in a process of their own: never in your shell, and never in Dev, so a running dev server is untouched. The Terminal tab switches to the action: its header shows `shell │ test ● running` (then `✓ exit 0` or `✗ exit 1`), **v** switches between the shell and the action, and **Enter** (or **o**) attaches to the action while it runs. Every session in that worktree shares it (actions run whether or not the session's agent is running). One action runs at a time per worktree; nothing is queued: running another while one runs is refused, and **x** in the **e** list stops the running one. Its output and exit code stay visible until the next action replaces them.
- **Creation hooks** run only while the reviewed hook is trusted and not switched off (`worktree.hook: false`); otherwise Deckhand creates the worktree with `git worktree add` at the effective `worktree.location`, using `branchFrom` and `branchName`. Either way, `worktree` links are applied afterwards to a newly created worktree. The hook is looked up in the launch checkout, then the main checkout, and runs from the reviewed in-memory bytes via `bash -c`, so a file swapped after review never executes. `$0` is the script path; `${BASH_SOURCE[0]}` is empty, so hooks should locate sibling files with `$0`.
- If a new worktree could not be registered, the session cannot be restarted in the original checkout; create a new session instead.

Known limitation: a `deckhand.json` that is not valid UTF-8 cannot be opened in the editor; fix it outside Deckhand.

## Cleanup

When you stop a worktree session with **x**, Deckhand inspects the workspace before offering deletion, and checks again at deletion time. (**x** asks before stopping any session; **X** force-stops at once and never deletes anything.)

- **Structural protections can never be overridden**: the main checkout, the worktree the session was launched from, a worktree used by another active session, sessions without a worktree, a worktree Git no longer lists, and (when deleting the branch) `main`/`master` or a branch that changed since inspection. Deletion is not offered for them.
- **Data that blocks safe deletion**: modified or staged files, untracked files, and ignored files other than `node_modules` (for example `.env` or build output). Entries that are symlinks (for example links from [worktree settings](#worktree-settings)) do not count. Commits matter only when the branch is deleted too or HEAD is detached; then commits not reachable from any other local branch or remote-tracking ref block deletion. If any of this cannot be determined, deletion is blocked.
- Typing **DELETE** authorizes data loss and overrides every data check above, including dirty files. Force-stopping with **X** does not authorize data loss.
- Git evidence is local: there is no implicit fetch, so unknown upstreams stay conservative. A squash merge leaves the branch's commits "unmerged" as far as Git knows, so the merged marker records the commit that was merged (Deckhand's own merge, a kept conflicted one, or the head of a PR found merged; not a manual **M**, which Deckhand cannot verify): it and its ancestors count as integrated when deleting the branch. Commits made after it still need **DELETE**, and clearing the marker (**M**) makes them all count again.
- A failed removal is reported as **Worktree retained…** with the Git error; it never silently succeeds.

Deletion applies to the worktree, not just the stopped session: every session that was in it (attached sessions and sub-sessions included) can no longer be resumed or merged, and its Terminal, Git and Dev tabs say the worktree was deleted. A worktree created later at the same path is a new one: those sessions don't join it, and it starts without their markers.

Review and stage changes in the Git tab (**g**, see [Git changes](#git-changes)); attach lazygit there (**o**) for commits and everything else.

## Merging and done

**m** merges (or squash-merges) the selected session's worktree into the branch of the checkout Deckhand was started in, without committing, so you review and commit the result. The confirmation shows, before anything runs:

- `Into <branch> · <path>`: in yellow when the target is not the repository's main checkout or not on its default branch, so a surprising target stands out.
- The commits that would be merged (count, the first six subjects, `+N more`) and the diff stat of `<target>...<source>`.
- `N uncommitted files` when the worktree has uncommitted (or untracked, not ignored) files, with a toggle, on by default: `☑ commit them first ("<session title>")`. **Space** switches it; off, they stay in the worktree. On, Deckhand runs `git add -A` and `git commit -m "<session title>"` in the worktree first (your hooks run); if that fails, its output is shown and nothing is merged. A worktree with only uncommitted work is merged this way.
- A warning when the target has uncommitted changes in files the merge would touch (Git would refuse or mix them); unrelated uncommitted files are fine. A merge, rebase, cherry-pick or revert in progress in the target (or unresolved conflicts) is named, and the merge refused until it is finished.
- The notes, which give way first when the pane is short.

On conflicts, one small view lists the conflicted files and offers two keys: **Enter** keeps the merge in progress (conflict markers in the files; resolve them in your editor or the Git tab's Merge Conflicts group, then commit) and marks the worktree merged now; **a** aborts it (`git merge --abort`; for a squash, which Git does not track as a merge, Deckhand restores the target with `git reset --merge`, keeping your unrelated uncommitted edits, and only for a squash it started itself on an unchanged HEAD — otherwise it refuses and says so).

**✓** (merged) belongs to the worktree: every session in a linked worktree shows it, and **M** from any of them sets or clears it. It is also set by itself when the branch was merged outside Deckhand: its tip is in the default branch (local, or `origin/<default>` as last fetched; Deckhand never fetches) and it has at least one commit of its own beyond where it started (a fresh branch, or one only fast-forwarded to a newer main, never counts), or `i` → **P** finds its PR merged (GitHub squash merges included). This is checked at daemon start, every five minutes and whenever `i` runs; it never unmarks, and after **M** clears a found marker it stays cleared until the branch gets new commits. Sessions in the main checkout have nothing to merge: **M** says so and points at **D**.

**Space** marks any session done (or not done): `☑` in its row, a muted title, and `done 2d ago` in the details. It is independent of merged (a worktree session can be both) and of archiving; handoffs mention it. Main-checkout sessions that an older version marked with **M** are shown as done.

## Organizing and inspecting sessions

- **A** archives or unarchives a session without stopping it. The footer offers it for exited and archived sessions; archiving from the active view says where the session went.
- **f** opens the filter menu at the top of the sidebar: each filter with its key and how many rows it would list. One key picks it: `a` active, `r` running, `!` attention, `e` exited, `A` archived, `*` all, so any filter is two keys away (`f r`); `f f` goes back to active. `j`/`k` show each filter as you move, Enter keeps it, Esc restores the one you had. Active means unarchived, not necessarily running. The sidebar header names any other filter (or a search) in cyan with its `shown/total` count, and shows `! N` when N sessions need attention.
- **/** searches title, notes (the session's and its worktree's), agent, branch and path. Enter keeps the query; Esc clears it. Matching children keep their ancestors visible for context, and searches and filters show matches inside collapsed trees.
- Selection, per-session tabs, sidebar width, collapsed and hidden sessions, filter and search persist per repository in `ui-state.json`.
- **i** shows the workspace's branch and HEAD, changed and untracked counts, diff size, the base comparison and the cached upstream ahead/behind. Inside it, **P** asks the optional `gh` CLI for PR and check status, **b** opens the PR (https URLs only), **c** creates a PR and **g** opens the Git tab. Missing tools, auth or network are reported without blocking local use. Reopen **i** to refresh.
- **c** (create PR) first asks: *Push <branch> to <remote> and open GitHub's new-PR form in your browser?* Only **Enter** proceeds; **Esc** cancels. Deckhand then runs `git push -u <remote> <branch>` (never forced; the remote is the branch's upstream remote, else `origin`) and `gh pr create --web --head <branch>`, or `gh pr view --web` if an open PR already exists. `--base` is passed only when the session's base branch exists on that remote (for example `origin/main`); otherwise `gh` chooses. It refuses a detached HEAD, `main`/`master`, the session's base branch, sessions without a worktree branch, and a branch that changed since you confirmed. Prompts are disabled and stdin is closed, so it never waits for input; push and `gh` failures are shown as errors. Press **P** afterwards to see the new PR.

Git state belongs to the workspace, so agents sharing a worktree see the same changes. The Git tab and the Terminal tab belong to it too: sessions in one worktree share one Changes view, one lazygit and one shell (also once their agents exit), like the Dev pane.

## Git changes

The Git tab (**g**) is a built-in, read-mostly take on VS Code's Source Control panel for the selected session's worktree, so you rarely need to open the worktree in an editor just to see what changed. lazygit is still there for everything else: **o** attaches it (it starts on that first attach, not when you open the tab).

- **Groups**, in VS Code's order and each with a count: **Merge Conflicts**, **Staged Changes**, **Changes** (unstaged changes to tracked files) and **Untracked**. A file with staged and unstaged edits is listed in both Staged and Changes. Empty groups are hidden; with nothing to show the tab says *No changes*. The list shows at most 2000 files in all; a group that was cut ends with *+N more* (its count stays exact).
- **Rows**: the status letter (**M** modified, **A** added, **D** deleted, **R**/**C** renamed/copied, **T** type change, **U** conflict, **?** untracked), the file name with its directory dimmed after it, `old → new` for renames, and `+added −removed` lines (`bin` for binary files; untracked files count their lines, up to 500 files of at most 1 MB).
- **Refresh**: while some Deckhand window shows the Git tab of a worktree, the daemon reads `git status` (without taking Git's index lock) about every two seconds and updates every window looking at any session of that worktree, but only when something changed; it reads again right after you stage or unstage. Leaving the tab stops it.
- **Browse** (**→** or **l**, like Preview): **j**/**k** or **↑**/**↓** select a file across the groups (**g**/**G** first/last). The selection follows the file when the list refreshes; when the file moves to another group (you staged it), it goes to the next file of the group it left, or the first file below that group once it is empty.
  - **Space** stages the selected file (from Changes, Untracked or Merge Conflicts; staging a conflict marks it resolved, as in VS Code) or unstages it (from Staged; a rename unstages both paths).
  - **a** stages everything (`git add -A`); when there are merge conflicts it stages everything else and leaves the conflicted files for you to stage one by one once resolved. **A** unstages everything staged (an ongoing merge stays in progress).
  - **Enter** or **E** opens the file in Cursor or VS Code at its first changed line (`-g file:line`; VS Code reuses its last active window by default). A deleted file can't be opened.
  - **J**/**K** scroll the diff preview by three lines, **PgUp**/**PgDn** by a page. **o** attaches lazygit; **Esc**, **←** or **h** returns.
- **Diff preview** (beside the list from 100 columns of pane width, below it otherwise): staged files show `git diff --cached`, changes and conflicts `git diff`, untracked files their contents as added lines (a symlink its target). Additions are green, deletions red, hunk headers cyan. It is read-only and cut at 256 KB; binary files say so.
- **Safety**: only paths in the current `git status` of that worktree can be staged or unstaged, passed literally (no globs). Before the first commit, unstaging removes the file from the index (`git rm --cached`). Deckhand never commits, discards, stages hunks or switches branches here; use lazygit (**o**) for that.

## Notes

The Notes tab (**a**) shows the note of the place you work. A session in a worktree has one note, **Worktree · *branch* (shared by N sessions)**, that every session in it sees and edits (attached sessions and sub-sessions included); it lives as long as the worktree, not any one session. A session in the main checkout has two sections: **Main checkout (shared by N sessions)**, the repository's note, and **This session · *title***, its own (main-checkout sessions are usually separate jobs). Notes that sessions in a worktree kept of their own (from before) are merged into the worktree's note when the daemon starts, each under a `## <session title>` heading when there are several; the originals go to the trash. Removing a session or a worktree never deletes a note with text in it: it moves to `notes/trash/`, named by date, kind and what it belonged to. Each header shows its open checklist items (`☐ 2 open`). Height is split between them: an empty section is one line, and when both are long each keeps at least three rows (the one you are editing gets more). Without editing, the notes are rendered from the top, checklist items as ☐/☑ (checked ones dimmed), ending in *+N more lines* when they don't fit.

- **Enter** (or **→**) edits (the section you edited last, else the session's); **Esc** stops. **Tab** switches section in the main checkout, keeping each one's cursor. The editor is a normal one: arrows, Home/End (Ctrl+A/Ctrl+E for line start/end), Alt or Ctrl with ←/→ to jump words (Option+←/→ on macOS terminals), Alt+Backspace or Ctrl+W to delete a word, PgUp/PgDn, Enter, Backspace and Delete anywhere, multi-line paste. Long lines wrap at word boundaries and the view scrolls with the cursor.
- **Checklists**: lines like `- [ ] item`, `- [x] item` or `* [ ] item`, indented or not. **Ctrl+X** checks or unchecks the cursor's line (a line without a checkbox gets `- [ ] `), **Ctrl+T** adds an item below, and Enter on an item starts the next one (Enter on an empty item ends the list). An editor's integrated terminal (Cursor, VS Code) may keep some Ctrl keys for itself, so Deckhand never sees them; every line/word key has a non-Ctrl alternative (End/Home, Alt+Backspace, Alt+←→). The sidebar's details show `☐ 3 open (2 worktree)` for the selected session.
- **Files**: each note is a Markdown file in the state directory's `notes/` (`sessions/`, `worktrees/`, `repos/`). **E** on the Notes tab (or **Ctrl+O** while editing) opens it in Cursor or VS Code, creating it if needed. Edits saved there appear in Deckhand within a second or two. Deckhand saves about 0.3 s after you stop typing, and only if the file is still the version you were editing: if it changed in between, Deckhand reloads it and tells you instead of overwriting it.
- **Lifetime**: a session's note is deleted when you remove the session; a worktree's when its last session is removed (a worktree deleted with its session keeps showing the note, read-only, until then). A new worktree at the same path starts with an empty note. The main checkout's note is never deleted.
- Handoffs (**H**) include a **Worktree notes** section, the merge confirmation (**m**) shows the worktree's note above the sessions', and **/** searches both notes.
- **Ctrl+P** while editing sends the cursor line's open checklist item to the repository's tasks (below); the line becomes `- ↗ <item> <!-- dh:t=<id> -->`, shown as *↗ item · in Tasks*. **Backspace** archives an exited session (nothing is lost; **f** shows the archived view). **Backspace** on an archived session removes it for good, deleting its notes: when they (its own, and its worktree's when it is the last session there) still have open items it asks first: **Enter** moves them to the backlog, **x** removes anyway.

## Tasks

**b** opens the repository's task list in the right pane (the same in every Deckhand of the repository, whichever checkout it was opened in: it shows every checkout's sessions and notes, while the sidebar keeps to its own): a group per worktree (or main-checkout session) with open tasks, headed `⎇ branch · N` with the most urgent state of its sessions (needs you, working, idle, exited); in it `◆` is the task the worktree was started for and `☐` a follow-up assigned to it. Then **Backlog** and **Done · this week** (older ones fold into one row; Enter shows them). The board opens on the selected session's worktree; **v** shows only that worktree's tasks (and back). Every worktree of a repository shares its list; the header shows `☐ N tasks · b` while any are open.

- **a** adds a task to the group the selection is in (a worktree's, as a follow-up, or the backlog; the editor says which), **Enter** edits one: the title, then **Tab** for its details. Enter saves from the title, **Ctrl+S** from the details, Esc cancels. **Space** ticks or reopens, **x** twice deletes, **J/K** reorder within a group, **j/k** (↑/↓) and **g/G** move, **o** selects the session doing it, **w** moves it: to the backlog, back to the note it was sent from (**↩ Back to its note**, listed while a note still holds its `↗` line: the line becomes the checklist item again, its details indented under it, and the task leaves the list), or to a worktree / main-checkout session as a follow-up. The menu has two lists, **Worktrees** and **Sessions** (main checkout), Tab switches; typing searches names, branches and session titles; ↑↓ choose; Esc clears the search, then cancels; **E** opens the file in Cursor or VS Code.
- **n** on a backlog task or a follow-up (which moves to the new worktree) opens the usual new-session form named after it (cut to 64 characters), in a new worktree by default, with *Task:* and *Base:* lines. **↑/↓** choose where the new branch starts: the `worktree.branchFrom` setting (first), or any local branch, most recently committed first. A repository whose trusted creation hook decides where worktrees go can't choose (the form says so). The task is linked to the session's worktree incarnation (in the main checkout: to the session), and its text (`<title>: <details>` on one line) is typed into the agent's input but not sent, so you edit it and press Enter yourself. Deckhand types it once the agent's screen has not changed for 0.7 s (or its activity turns idle) with no menu or question showing (Claude's and Codex's folder-trust prompts, numbered menus), as one bracketed paste so no character acts as a shortcut. If you type letters into the agent first, or it doesn't settle within two minutes, nothing is typed.
- **Tab** shows the **Notes view**: every note with text, in full, under a heading per worktree in sidebar order (its note), then **main checkout** (its note, then each session's own); each note has its name and open-item count. Every line is listed (checklist items as ☐/☑, ↗ links, headings); **a** on an open item adds it as a task, **Enter** opens the note in its session's Notes tab, **f** shows only open items (and back), **v** only the selected session's worktree (or, in the main checkout, its note and the main checkout's). Empty notes are left out. Deleted worktrees' notes are not listed. A sent item (here or Ctrl+P) lands on its note's work: a worktree's note or a worktree session's note → that worktree, a main-checkout session's note → that session, the main checkout's shared note → the backlog. Notes otherwise stay scratch space: nothing in them becomes a task on its own.
- **Linked work**: merging the worktree (**m**, **M**, or merged found by Deckhand) ticks the tasks it was started for and the merge confirmation names them; its open follow-ups are listed there as rows (up to five, the heading counts the rest): **Space** (or Enter) on one ticks it with the merge, and any left open go back to the backlog marked *left open in ⎇ branch*. Marking done (**Space**) ticks the started tasks once every session of the worktree is done; follow-ups stay. Undoing the merge marker or done reopens exactly the tasks it ticked (and assigns the released follow-ups again). When the worktree is deleted unmerged, or its last session (or a linked main-checkout session) is removed, its open tasks go back to the backlog marked *tried in ⎇ branch* (follow-ups: *left open in*).
- The linked session's Notes tab starts with **◆ Task · title**, `+N open here` for the worktree's other open tasks, and its state; its sidebar details add `◆ title +N`; **/** finds sessions by any of their tasks' titles; handoffs (**H**) include a **Task** section with the open follow-ups.
- **File**: `notes/tasks/<hash of the main checkout>.md` in the state directory. A task is a top-level checklist item, the indented lines below it are its details, and every other line is kept as written. Deckhand's bookkeeping is a `<!-- dh:t=… wt=… s=… assigned=… done=… auto=… tried=… from=… was=… added=… -->` comment at the end of the item. Each change is applied to the file as it is on disk (never a stale copy), so edits made in an editor are kept; items added there get an ID with Deckhand's next change.

## Resume and handoffs

Claude and Pi get an exact conversation ID assigned at launch (`--session-id <uuid>`), so **s** reopens that conversation even after a rename, a duplicate name or a fork (`claude --resume <uuid>`, `pi --session-id <uuid>`); sessions recorded before this keep resuming by their stored name or Pi path. Forks never type into the agent: Claude forks launch `claude --resume <parent> --fork-session --session-id <child>`, Pi forks `pi --fork <parent> --session-id <child>`, Codex forks `codex fork <parent id>` (the child's own ID is then captured as below, never the parent's; a Codex parent whose ID is unknown can't be forked). A fork copies the parent's conversation as saved at that moment: a turn still in progress isn't included. Forks start in the parent's worktree. Claude and Pi forks may go into another worktree; the child's first message then says which worktree it is in and not to touch the parent's (the copied conversation's paths point there, and the parent's uncommitted changes aren't in the new worktree). Codex forks stay in the parent's worktree (Codex may reopen a fork in the directory it was recorded in, so Deckhand refuses other workspaces when creating it). **s** on a fork resumes its own conversation; a fork that never launched, whose fork failed because the parent had nothing saved yet, or whose Codex ID was never reported forks the parent again with a new child ID. Codex's ID is captured from an authenticated SessionStart callback or its `codex resume <id>` exit hint. **s** resumes a known conversation and **S** starts a fresh one. An unknown ID is an error: if Claude reports no conversation for its ID, the exited session says to press **S**; an unknown Codex ID refuses **s**. Deckhand never guesses with `--last` or starts a blank conversation silently.

After a daemon crash, sessions are marked interrupted and keep their conversation references, notes and archive state. This is conversation recovery, not reconnection to a live process.

**H** writes a private Markdown handoff (notes, workspace metadata and a **Workspace changes** section, not a transcript) to the state directory's `handoffs/` and opens it in Cursor or VS Code when available. After reviewing it, **N** → *↳ From its handoff* creates a clean child session whose initial prompt is the document's path. It is not a fork and never types into a running terminal; export again after notes change. The protocol can add a bounded, labelled terminal excerpt, but no key does so because it may contain secrets.

**Workspace changes** is gathered from Git when the handoff is exported. It contains the base ref, commits since the base (up to 30, then "+N more"), uncommitted files with their status (up to 50) and a committed diff stat against the base (`<base>...HEAD`: files changed, insertions, deletions and per-file counts). It lists only file names and numbers, never diff content. If Git fails, the section says *Git information unavailable* and the export still succeeds. Sessions outside a Git repository, or whose worktree was deleted, have no such section.

## Agent versions and updates

**U** opens the Agents screen (full width, like Settings): one row per agent with its installed version (`<agent> --version`, found on the daemon's PATH, the one sessions launch with), its latest release (the npm `latest` dist-tag of `@anthropic-ai/claude-code`, `@openai/codex` and `@earendil-works/pi-coding-agent`), a status (`up to date`, `update available`, `not installed`, `latest unknown`) and how many running sessions use an older version. Opening it looks the latest releases up again (up to ~10 s each; **r** repeats that); the daemon also checks in the background when it starts and at most every 6 hours. Offline, or without npm, the latest is just unknown: there is no error.

**Enter** runs the selected agent's own updater, non-interactively (stdin closed, the daemon's environment, at most 5 minutes): `claude update`, `codex update` or `pi update --self` (pi itself, not its packages). If the agent has running sessions you are asked first. The details box shows the running state, then the new version or the updater's exit code and the last lines of its output; leaving the screen does not stop it, and the footer says when it finished. Only one update per agent runs at a time. Agents that are not installed are left to `deckhand setup`.

Updating never restarts or touches sessions. Each launch (create, **s**, **S**) records the version it started with; a running session whose version is older than the one now installed shows a dim **↑** before its agent glyph (the first marker dropped on a narrow sidebar), and its details line names both versions (`claude 2.1.287 · 2.1.290 installed`). Restart it (**x**, then **s**) to pick up the new build. Exited sessions are never marked: their next start uses the installed version. When any installed agent has a newer release, the header's right side says so quietly (`codex update · U`, `2 agent updates · U`).

## Lifecycle signals and notifications

Both default off. Switch them on in Settings (**C** → the **Agents** rows in the Global column), or in the user config (`~/.deckhand/config.json`, or `~/.deckhand-dev/config.json` for the dev build), keeping existing settings:

```json
{"agent_hooks": true, "notifications": true}
```

Notifications on its own still notifies when a session exits; Agent signals adds *needs input* and *done*.

- Supported hooks report **working**, **needs-input**, **response-ended**, **failed** and **limited**; everything else stays unknown or activity-based. **Response-ended is not task success**, and silence is not completion. **!** jumps to the next session with known attention, including failed or interrupted exits.
- Claude: with `agent_hooks` on, new launches check `claude --help` and pass hook settings for the supported events with `--settings`. Repository and user provider config is not modified.
- Codex: with `agent_hooks` on, launches add `--no-daemon` when the installed version supports it, so callback identity is not shared through a native daemon. The isolated dev build requires it even with hooks off and refuses Codex versions without it. Codex hooks can't be passed at launch, so set them up once in Codex's own config: save the output of `deckhand hooks codex` (`node scripts/deckhand-dev.mjs hooks codex` for the dev build) as `~/.codex/hooks.json` (merge it into the `hooks` object if the file exists), then trust it with `/hooks` in Codex. While Agent signals is on and a Codex home exists, Settings shows **⚠ Codex** on the row with this command when no hook there calls this Deckhand, or when one calls another install of it (a different Node or Deckhand path: regenerate it). Check your version's [Codex hooks reference](https://developers.openai.com/codex/hooks) for which events it supports.
- Pi has no automatic adapter.
- Callbacks must carry the launch's secret token and launch ID; stale and unauthorized callbacks are rejected and native-subagent events are ignored. The `deckhand hook` bridge reads bounded input, forwards only lifecycle fields (event name, session/agent IDs, notification and error types) and never starts a daemon. It always prints `{}`, so it never approves, blocks or alters an agent action.
- Notifications are sent by the daemon on attention changes and process exits via `osascript` (macOS) or `notify-send` (Linux), best effort. They contain no terminal output or approval payloads.

Enable hooks before creating or restarting sessions; running agents are not reconfigured. See also [Claude's hook guide](https://code.claude.com/docs/en/hooks-guide).
