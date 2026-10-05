export const HELP_TEXT = `QUICK START
C  Settings: every setting as a grid, Global and This repo side by side; ←→ picks the layer, Enter edits that cell.
n  New session: pick an agent, enter a name, Tab chooses workspace mode, Enter launches.
N  Child session in the parent's workspace; optionally fork supported Claude/Pi conversations.
o  Attach the active pane. Ctrl+Space or Ctrl+] returns to Deckhand.
q  Quit the UI (Ctrl+C too; during a Settings edit or JSON draft the first Ctrl+C cancels). Agents and the daemon keep running.

CONFIGURATION
Two layers, same settings. Global defaults live in your user config.json under "defaults" and never need trust.
A repository can override them with ONE deckhand.json in its main checkout (read live, uncommitted edits included).
Copies of deckhand.json inside linked worktrees are ignored. Bare repositories have global defaults only.
Effective = global defaults, overlaid field by field by the repository file only while it is trusted (its
  defaultAgent/defaultWorkspace apply untrusted too: they only preselect the n picker).
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

C opens Settings for the selected session's repository (else the launch directory): a grid with one row per setting
  (General, Commands, Worktrees) and two columns, Global (all repos) and This repo. Each cell shows that layer's own
  value (— when unset); ● marks the one in effect, an overridden value is dimmed, and when neither layer sets it the
  Global column shows the built-in value, e.g. "dev (built-in)". A repository value that applies only once trusted
  shows ⚠ (you are asked the first time it runs, or press T); defaultAgent/defaultWorkspace apply untrusted too.
  The box below says the selected cell's layer and file and how it relates to the other column, in one line.
  ↑↓ setting • ←→ (or Tab) column, kept while you move • Enter edits that cell • x clears it (asks first; the other
  layer or the built-in takes over) • e opens that column's raw JSON • T review • Esc closes
Settings opens on This repo (Global outside a repository). Enter saves at once to the cell's layer, keeping every
  other key; a text input starts from that layer's own value (empty when unset, the inherited value as a hint).
Edit controls: choices (agent, workspace, branch from, location presets incl. custom…, hook on/off) with ↑↓ and Enter;
  text (Dev/Setup command, branch name, custom location) with errors shown inline; Esc cancels the edit.
  Actions are named shell commands you run with e on a session, in that session's worktree, shown in the Dev pane.
  Actions (Enter on its cell) lists that column's actions, name → command, with the other layer's dimmed below:
  Enter edits the command, a adds one in two steps, x removes it. A name takes letters,
  numbers, spaces, _ . - (starts with a letter or number, up to 48 characters; checked as you type, and it says when
  it replaces or overrides an existing action); the command runs with your shell like typing it in a terminal
  (&&, pipes, cd and env vars work), e.g. "test" → npm test. At most 30 actions per layer.
  Linked items lists untracked/ignored entries of the main checkout with sizes and suggestions (dependencies and env files
  link; build/cache/clutter skip): Space links/skips, Enter saves that column's worktree.symlink. worktree.files
  entries are edited in e.
Raw JSON editor (e): a missing file starts from a starter draft; nothing is written until Ctrl+S.
Arrows/Home/End move the cursor; Enter inserts a newline; Tab inserts two spaces.
Ctrl+A selects all; typing/paste replaces it. Backspace removes left, forward Delete removes right.
Ctrl+F formats valid JSON. Ctrl+S validates settings and saves. Esc or Ctrl+C returns to Settings; edited drafts require confirmation. Ctrl+C again at that prompt, or during a save, quits the UI.
Invalid JSON/settings are never saved. A file changed on disk since it was read rejects the edit (Settings reloads).

TRUST (inline review)
A repository deckhand.json or .claude/scripts/create-worktree.sh hook must be trusted before Deckhand runs it.
Trust gates running, not choosing: lists show everything (untrusted repo actions marked "· needs trust"), and the
exact bytes are shown when something from the repo is about to run: creating a new worktree with its setup, hook or
worktree settings (n/N/F confirm), an untrusted action (e), its Dev command (d), or a setup retry (s).
  Enter trusts and runs • s goes ahead without it (global settings; for an action: cancels) • Esc backs out.
In Settings (C), T opens the same review. Changed bytes need another review; trust is stored in your config.json.
Edits you make in Deckhand (Settings, its raw JSON editor) keep deckhand.json trusted if it was trusted (or new);
  changes from outside (an editor, git pull, an agent) need review, and a creation hook is only trusted by review.
The review shows tabs as spaces and invisible/bidi characters as <U+XXXX>. j/k, arrows, Page keys scroll.
e chooses an action (global plus all repository actions; untrusted ones are reviewed when chosen) for the shared Dev pane.
  Stop a running command first; nothing is queued.
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
