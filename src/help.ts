export const HELP_TEXT = `QUICK START
C  Edit configuration: Global defaults (all repositories), Repository (deckhand.json in the main checkout), Worktree setup or Effective settings.
n  New session: pick an agent, enter a name, Tab chooses workspace mode, Enter launches.
N  Child session in the parent's workspace; optionally fork supported Claude/Pi conversations.
o  Attach the active pane. Ctrl+Space or Ctrl+] returns to Deckhand.
q  Quit the UI (Ctrl+C too; in the config editor the first Ctrl+C cancels). Agents and the daemon keep running.

CONFIGURATION
Two layers, same settings. Global defaults live in your user config.json under "defaults" and never need trust.
A repository can override them with ONE deckhand.json in its main checkout (read live, uncommitted edits included).
Copies of deckhand.json inside linked worktrees are ignored. Bare repositories have global defaults only.
Effective = global defaults, overlaid field by field by the repository file only while it is trusted.
Actions merge by name (the repository wins). The legacy dev_command is used when no devCommand is set.
worktree merges per field: repository location wins, symlink lists are unioned, files merge by destination.

Supported settings:
  defaultAgent: claude, pi, or codex
  defaultWorkspace: none, new, or existing
  devCommand: command for the Dev pane
  setupCommand: command before an agent launches in a newly-created worktree
  actions: named commands, e.g. {"test": "npm test", "typecheck": "npm run typecheck"}
  worktree: new-worktree layout, e.g. {"location": "{repoParent}/worktrees/{name}",
    "symlink": ["frontend/node_modules"], "files": {"backend/.env": "{repoParent}/backend.env"}}
    location: path template ({name} {repo} {repoParent} {repoRoot} {home}, ~/); default ~/.deckhand/worktrees/{name}
    symlink: paths from the launch checkout linked at the same place (real targets; missing ones skipped)
    files: destination in the new worktree -> source file/dir, linked when the source exists
    branchFrom: current (launch HEAD, default), default (local main/master), origin (fetch origin/<default> first)
    branchName: new branch template, {name} required, {user} optional; default {name}
    hook: false ignores .claude/scripts/create-worktree.sh (not run, not reviewed); an active hook decides location/branch
    Links apply only to newly created worktrees, never replace real files, and never block safe deletion.

C shows each target's exact path and whether it exists. j/k/arrows choose; Enter opens the terminal JSON editor.
Worktree setup edits the worktree section with presets and suggestions, saving to deckhand.json or global defaults:
  ↑↓ move • Space link/skip an untracked/ignored entry (or toggle) • ←→ change Location/Branch from • Enter edits Branch name
  t toggles the save target • h switches the creation hook on/off • Ctrl+S saves • e opens the JSON • Esc cancels (asks if edited)
  Suggestions: dependencies/env files link, build/cache/clutter skip. Saving never runs or trusts anything.
Effective settings (read-only) lists every setting in effect for this repository with its source: repo, global,
  legacy dev_command, built-in default or not set; an untrusted deckhand.json's values are shown as (repo, pending trust).
  ↑↓ move • Enter/e opens the file that sets the row (repo → deckhand.json, otherwise global defaults) • T review/trust • Esc back
A missing target starts from a starter draft. Nothing is written until Ctrl+S.
Arrows/Home/End move the cursor; Enter inserts a newline; Tab inserts two spaces.
Ctrl+A selects all; typing/paste replaces it. Backspace removes left, forward Delete removes right.
Ctrl+F formats valid JSON. Ctrl+S validates settings and saves. Esc or Ctrl+C cancels; edited drafts require confirmation. Ctrl+C again at that prompt, or during a save, quits the UI.
Invalid JSON/settings are never saved. A target changed on disk since opening rejects a stale draft.
Saving never runs, trusts or commits anything.

TRUST (inline review)
A repository deckhand.json or .claude/scripts/create-worktree.sh hook must be trusted before Deckhand runs it.
When it matters and is untrusted, n/N/F, e, starting Dev and s (retrying setup) first show the exact bytes:
  Enter trusts them and continues • s continues without them (global defaults only) • Esc cancels.
T opens the same review on its own. Changed bytes need another review; trust is stored in your config.json.
The review shows tabs as spaces and invisible/bidi characters as <U+XXXX>. j/k, arrows, Page keys scroll.
e chooses an action (global plus trusted repository actions) for the shared Dev pane. Stop a running command first; nothing is queued.
d selects Dev; d again starts/stops the command. Output and exit code remain visible after it exits.
Setup runs before the agent. Failed/cancelled setup retains its worktree; s retries it (after review if needed).
x/X while starting cancels setup/startup, not the worktree. In-flight Git preparation may still finish.

ORGANIZE AND FIND WORK
1-9, 0 and multi-digit numbers select visible sessions (0 means 10).
j/k select; J/K reorder siblings. c cycles hiding exited children, collapsing, and expanding (search/filters show all matches).
h/l resize the sidebar. Selection, tabs, width, tree visibility and search/filter persist.
A archives/unarchives without stopping an agent.
f cycles active, archived, all, attention, running, exited. Active means unarchived, not running.
/ searches title, notes, agent, branch and path. Enter keeps the query; Esc clears it.
Matching children retain ancestors for context, even when those ancestors don't match the filter.
r refreshes the session list. Backspace removes an exited session record, not its files.

PANES, NOTES AND HANDOFFS
Tab cycles Preview/Terminal/Git/Dev/Notes; p/t/g/d/a selects one directly.
O opens the workspace in Cursor/VS Code. Notes: a, then o to edit; Esc finishes. Notes autosave.
v focuses Preview scrolling; j/k scroll, g/G jump to top/bottom (Claude: 12 wheel steps), Esc/v returns.
[ and ] adjust scrolling sensitivity. Native approvals remain inside agent terminals.
H exports a private Markdown handoff (notes, plus commits, changed file names and a diff stat; never diff content)
and opens it in an editor when available.
Inspect/edit that document, then F creates a clean child with its path as initial context.
F is not a conversation fork or silent input to an existing terminal. Re-export after notes change.

GIT AND PR VISIBILITY
i shows branch/HEAD, changed/untracked counts, diff size, base and cached upstream comparisons.
Inside i: P explicitly asks optional gh for PR/check status; b opens a PR; g opens lazygit.
Inside i: c creates a PR after confirmation (Enter/Esc): git push -u <remote> <branch> (never forced), then
  gh pr create --web (or gh pr view --web for an open PR). Refused for detached HEAD, main/master and the base branch.
Only https PR URLs are opened. Reopen i to refresh. Git state belongs to the workspace, not one agent. No implicit fetch.
m merges/squashes a worktree into the current branch for review, without committing.
M is a personal merged/pushed marker, not verified remote state.

STOPPING, CLEANUP AND RESUME
x stops a running agent; X force-stops it. Worktree sessions offer keep/delete options.
Dirty/untracked/valuable ignored files, unpublished/unmerged commits and unknown Git evidence block safe deletion.
Typing DELETE separately acknowledges data loss. Force kill does not authorize deleting work.
Main/current worktrees and actively shared workspaces remain protected: deletion is not offered for them.
s resumes a known native conversation; S explicitly starts a fresh conversation.
Claude/Pi IDs are assigned at launch. Unknown Claude/Codex IDs require S rather than guessing or silently starting blank.
Daemon crashes preserve known conversation references/notes, not live workers or auxiliary terminals.

ATTENTION AND OPTIONAL NOTIFICATIONS
! selects the next session with known attention, including failed/interrupted exits.
Opt-in supported hooks can show working, needs-input, response-ended, failed and limited.
Response-ended is NOT task success. Silence/idle isn't completion. Unsupported hooks remain unknown/activity-based.
agent_hooks and notifications in user config default off. New launches use capability checks.
Codex requires native hook configuration for supported events; Pi has no guaranteed automatic adapter here.
Notifications are daemon-owned and best effort; approvals are never handled by Deckhand.

ISOLATED DEV PREVIEW
From the feature checkout: npm start builds and opens a disposable Git sandbox.
The separate dev daemon/state/socket/logs/worktrees use ~/.deckhand-dev, not ~/.deckhand.
npm run status checks it; npm stop stops only dev sessions/daemon.
After daemon/worker code changes use npm restart. Plain npm run dev is not isolated.
Agents still use real credentials/subscriptions. Trusted scripts can access files outside the sandbox.

? or Esc closes this guide. j/k, arrows and PgUp/PgDn scroll; Home/End jump.
`;
