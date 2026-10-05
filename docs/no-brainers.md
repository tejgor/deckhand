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
| `actions` | Named commands offered by **e** |
| `worktree` | Where new worktrees go, what their branch starts from and is called, whether the creation hook is used, and what is symlinked into them; see [Worktree settings](#worktree-settings) |

- **Global defaults**: the `defaults` object in your user config (`~/.deckhand/config.json`). You wrote them, so they never need trust. If they are invalid, whatever uses them (new worktree setup, Dev, actions) reports the error; **C** opens them for repair.
- **Repository override**: one `deckhand.json` at the root of the repository's **main checkout**, read live (uncommitted edits apply at once to every worktree of that repository). Copies of `deckhand.json` inside linked worktrees are ignored. Bare repositories have no main checkout and use global defaults only. The file is limited to 64 KiB and must not be a symlink.
- **Effective settings** are the global defaults overlaid field by field by the repository override, **only while the override is trusted**; `actions` merge by name with the repository winning, and `worktree` merges per field (below). Untrusted, only global defaults apply.

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

### Worktree setup (C → Worktree setup)

A guided editor for the `worktree` section, so you rarely need to write it by hand:

- **Save target**: the repository's `deckhand.json` (default, when the repository has a main checkout) or global defaults; **t** toggles. The screen starts from both layers as written (the repository file counts even before it is trusted) and saves the complete chosen section into that target only, keeping every other setting there. Values the target would inherit anyway (for the repository, from global defaults) are not repeated, and the target's own `files` entries are kept untouched.
- **Hook**: when `.claude/scripts/create-worktree.sh` exists it is shown with its state; **h** switches it on or off (`worktree.hook`). While it is on, Location and Branch are marked as ignored.
- **Location**: next to the repository (`{repoParent}/worktrees/{name}`), Deckhand's default (writes no `location`; for the repository target this shows the global location if one is set), inside the repository (`{repoRoot}/.worktrees/{name}`, with a warning when `.worktrees/` is not gitignored), or the existing custom value. Each shows the path it resolves to.
- **Branch from**: current checkout, default branch (named), or fetch `origin/<default>` first. **Branch name**: Enter edits the template inline and shows an example for `my-task`.
- **Missing from new worktrees**: the untracked and ignored entries of the main checkout (`git status --ignored=matching --untracked-files=normal`, so directories collapse; at most 200, then "+N more (edit JSON)"), with sizes computed in the background (`…` while pending, `?` when unknown or too slow) and symlink targets. Entries already in the effective `symlink` list or `files` are marked as link, configured entries that no longer exist are listed as "missing in checkout" so they can be removed, and everything else is suggested: dependencies and env files (`node_modules`, `.venv`, `venv`, `vendor`, `.vercel`, `.env`, `.env.*`, `*.env`) as link; build output, caches and clutter (`dist`, `build`, `out`, `coverage`, `.next`, `.turbo`, `.cache`, `__pycache__`, `.pytest_cache`, `.DS_Store`, `*.log`) and everything else as skip. **Space** toggles link/skip; `files` entries are changed in JSON.
- **Keys**: ↑↓ (j/k) move, Space toggles, ←→ change the option, **t** save target, **h** hook, **Ctrl+S** save, **e** opens the regular JSON editor for the same target (save or discard first), **Esc** cancels and asks before discarding changes.

Saving goes through the same revision-checked save as the editor below: it runs nothing and grants no trust, so the next **n** asks you to review a changed `deckhand.json`.

### Editing (C)

**C** offers four targets: **Global defaults** (the `defaults` object in `config.json`; other keys are preserved) and **Repository** (`deckhand.json` in the main checkout), each with its exact path and whether it exists, plus [**Worktree setup**](#worktree-setup-c--worktree-setup) and **Effective settings**. A missing target opens from a starter draft. Nothing is written until **Ctrl+S**. Saving validates the settings and rejects a draft whose target changed since it was opened; the repository file keeps its permissions. Malformed JSON can be opened for repair. Esc or Ctrl+C cancels and asks before discarding edits; a second Ctrl+C at that prompt, or Ctrl+C while a save is in flight, quits the UI (unsaved edits are lost).

Saving never runs anything, grants trust or commits.

To see what's in effect and where it comes from, use **C → Effective settings**: a read-only list of every setting for the current repository (the selected session's, else the launch directory's) with its value and source (`repo`, `global`, `legacy dev_command`, `built-in default` or `not set`), one row per action, symlink and file entry. Templates show their expansion for this repository with the raw template beside it, and the hook row says whether `create-worktree.sh` was detected and whether it will run. While `deckhand.json` is untrusted, the global values are shown as in effect and each repository value beneath them as *(repo, pending trust)*. **Enter** (or **e**) opens the file that sets the selected row (repository rows open `deckhand.json`, everything else global defaults), **T** opens the trust review and returns here, **Esc** goes back to the targets.

### Trust (inline review)

A repository's `deckhand.json` and its optional `.claude/scripts/create-worktree.sh` creation hook are trusted together, by exact content. A repository with neither has nothing to trust. A hook switched off with `worktree.hook: false` is not part of the trust at all. When an untrusted override matters, Deckhand shows its exact bytes inline before continuing:

- **n**, **N** and **F** (new sessions), before the program picker, since the override can set the defaults and the new worktree's setup.
- **e**, before the action list.
- Starting Dev (**d**), when the override defines a `devCommand`.
- **s** on a session whose setup has not completed.

In the review, **Enter** trusts these exact bytes and continues, **s** continues without them (global defaults only, this time), and **Esc** cancels the action. **T** opens the same review on its own. Tabs are shown as spaces and invisible or bidirectional characters as `<U+XXXX>`.

- Trust lives in your user `config.json`, never in the repository. It is keyed per repository (by its main checkout, or by its Git common directory for bare repositories) and keeps the 20 most recently trusted fingerprints, so any change to the bytes needs a fresh review.
- A trust request is checked against the reviewed fingerprint: if the file changed after you opened the review, it is refused and the review shows the new bytes.
- The daemon enforces trust independently of the prompt: untrusted repository setup, Dev and action commands and creation hooks never run.

Trust is permission to run these strings, **not a sandbox**. Commands run as you, with your files and environment. Cancellation and timeouts signal the command's process group, but background processes a command detaches are not tracked.

### Setup, actions and creation hooks

- **Setup** runs only for sessions that create a new worktree, before the agent launches, with a ten-minute timeout. While it runs, Preview shows the most recent output. If you continued without trusting the override, only the global `setupCommand` (if any) runs. If the override changed after review, setup does not run and the session says so. If setup fails or you cancel it (**x**/**X** while starting), the worktree is kept and the agent is not started; **s** retries setup with the current effective settings, reviewing first if needed. Sessions in an existing worktree or the current directory never run setup.
- **Actions** (**e**) run a named command from the effective settings in the session's Dev pane. One command runs at a time; nothing is queued. Its output and exit code stay visible after it exits.
- **Creation hooks** run only while the reviewed hook is trusted and not switched off (`worktree.hook: false`); otherwise Deckhand creates the worktree with `git worktree add` at the effective `worktree.location`, using `branchFrom` and `branchName`. Either way, `worktree` links are applied afterwards to a newly created worktree. The hook is looked up in the launch checkout, then the main checkout, and runs from the reviewed in-memory bytes via `bash -c`, so a file swapped after review never executes. `$0` is the script path; `${BASH_SOURCE[0]}` is empty, so hooks should locate sibling files with `$0`.
- If a new worktree could not be registered, the session cannot be restarted in the original checkout; create a new session instead.

Known limitation: a `deckhand.json` that is not valid UTF-8 cannot be opened in the editor; fix it outside Deckhand.

## Cleanup

When you stop a worktree session (**x**, or **X** to force), Deckhand inspects the workspace before offering deletion, and checks again at deletion time.

- **Structural protections can never be overridden**: the main checkout, the worktree the session was launched from, a worktree used by another active session, sessions without a worktree, a worktree Git no longer lists, and (when deleting the branch) `main`/`master` or a branch that changed since inspection. Deletion is not offered for them.
- **Data that blocks safe deletion**: modified or staged files, untracked files, and ignored files other than `node_modules` (for example `.env` or build output). Entries that are symlinks (for example links from [worktree settings](#worktree-settings)) do not count. Commits matter only when the branch is deleted too or HEAD is detached; then commits not reachable from any other local branch or remote-tracking ref block deletion. If any of this cannot be determined, deletion is blocked.
- Typing **DELETE** authorizes data loss and overrides every data check above, including dirty files. Force-stopping with **X** does not authorize data loss.
- Git evidence is local: there is no implicit fetch, so unknown upstreams and squash merges stay conservative.
- A failed removal is reported as **Worktree retained…** with the Git error; it never silently succeeds.

Use lazygit (**g**) for source control. **M** is a personal merged/pushed marker, not verified remote state.

## Organizing and inspecting sessions

- **A** archives or unarchives a session without stopping it.
- **f** cycles active, archived, all, attention, running and exited. Active means unarchived, not necessarily running.
- **/** searches title, notes, agent, branch and path. Enter keeps the query; Esc clears it. Matching children keep their ancestors visible for context, and searches and filters show matches inside collapsed trees.
- Selection, per-session tabs, sidebar width, collapsed and hidden sessions, filter and search persist per repository in `ui-state.json`.
- **i** shows the workspace's branch and HEAD, changed and untracked counts, diff size, the base comparison and the cached upstream ahead/behind. Inside it, **P** asks the optional `gh` CLI for PR and check status, **b** opens the PR (https URLs only), **c** creates a PR and **g** opens lazygit. Missing tools, auth or network are reported without blocking local use. Reopen **i** to refresh.
- **c** (create PR) first asks: *Push <branch> to <remote> and open GitHub's new-PR form in your browser?* Only **Enter** proceeds; **Esc** cancels. Deckhand then runs `git push -u <remote> <branch>` (never forced; the remote is the branch's upstream remote, else `origin`) and `gh pr create --web --head <branch>`, or `gh pr view --web` if an open PR already exists. `--base` is passed only when the session's base branch exists on that remote (for example `origin/main`); otherwise `gh` chooses. It refuses a detached HEAD, `main`/`master`, the session's base branch, sessions without a worktree branch, and a branch that changed since you confirmed. Prompts are disabled and stdin is closed, so it never waits for input; push and `gh` failures are shown as errors. Press **P** afterwards to see the new PR.

Git state belongs to the workspace, so agents sharing a worktree see the same changes.

## Resume and handoffs

Claude and Pi get an exact conversation ID assigned at launch (`--session-id <uuid>`), so **s** reopens that conversation even after a rename, a duplicate name or a fork (`claude --resume <uuid>`, `pi --session-id <uuid>`); sessions recorded before this keep resuming by their stored name or Pi path. Forking a Pi session launches `pi --fork <parent> --session-id <child>`; Claude forks still resume the parent and send `/branch`. Codex's ID is captured from an authenticated SessionStart callback or its `codex resume <id>` exit hint. **s** resumes a known conversation and **S** starts a fresh one. An unknown ID is an error: if Claude reports no conversation for its ID, the exited session says to press **S**; an unknown Codex ID refuses **s**. Deckhand never guesses with `--last` or starts a blank conversation silently.

After a daemon crash, sessions are marked interrupted and keep their conversation references, notes and archive state. This is conversation recovery, not reconnection to a live process.

**H** writes a private Markdown handoff (notes, workspace metadata and a **Workspace changes** section, not a transcript) to the state directory's `handoffs/` and opens it in Cursor or VS Code when available. After reviewing it, **F** creates a clean child session whose initial prompt is the document's path. It is not a fork and never types into a running terminal; export again after notes change. The protocol can add a bounded, labelled terminal excerpt, but no key does so because it may contain secrets.

**Workspace changes** is gathered from Git when the handoff is exported. It contains the base ref, commits since the base (up to 30, then "+N more"), uncommitted files with their status (up to 50) and a committed diff stat against the base (`<base>...HEAD`: files changed, insertions, deletions and per-file counts). It lists only file names and numbers, never diff content. If Git fails, the section says *Git information unavailable* and the export still succeeds. Sessions outside a Git repository, or whose worktree was deleted, have no such section.

## Lifecycle signals and notifications

Opt in through the user config (`~/.deckhand/config.json`, or `~/.deckhand-dev/config.json` for the dev build), keeping existing settings:

```json
{"agent_hooks": true, "notifications": true}
```

- Supported hooks report **working**, **needs-input**, **response-ended**, **failed** and **limited**; everything else stays unknown or activity-based. **Response-ended is not task success**, and silence is not completion. **!** jumps to the next session with known attention, including failed or interrupted exits.
- Claude: with `agent_hooks` on, new launches check `claude --help` and pass hook settings for the supported events with `--settings`. Repository and user provider config is not modified.
- Codex: with `agent_hooks` on, launches add `--no-daemon` when the installed version supports it, so callback identity is not shared through a native daemon. The isolated dev build requires it even with hooks off and refuses Codex versions without it. Configure Codex's own hook settings from `deckhand hooks codex` (`node scripts/deckhand-dev.mjs hooks codex` for the dev build); check your version's [Codex hooks reference](https://developers.openai.com/codex/hooks) for which events it supports.
- Pi has no automatic adapter.
- Callbacks must carry the launch's secret token and launch ID; stale and unauthorized callbacks are rejected and native-subagent events are ignored. The `deckhand hook` bridge reads bounded input, forwards only lifecycle fields (event name, session/agent IDs, notification and error types) and never starts a daemon. It always prints `{}`, so it never approves, blocks or alters an agent action.
- Notifications are sent by the daemon on attention changes and process exits via `osascript` (macOS) or `notify-send` (Linux), best effort. They contain no terminal output or approval payloads.

Enable hooks before creating or restarting sessions; running agents are not reconfigured. See also [Claude's hook guide](https://code.claude.com/docs/en/hooks-guide).
