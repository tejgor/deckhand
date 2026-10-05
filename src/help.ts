// The ? guide as data: short topics of key → what it does, with a few notes. Rendered by helpPane.tsx (topic list,
// aligned key column, search). Keep rows to one idea each; the full reference lives in docs/no-brainers.md.

export type HelpLine = {keys: string; text: string} | {note: string} | {heading: string};
export interface HelpTopic {title: string; lines: HelpLine[]}

const key = (keys: string, text: string): HelpLine => ({keys, text});
const note = (text: string): HelpLine => ({note: text});
const heading = (text: string): HelpLine => ({heading: text});

export const HELP_TOPICS: readonly HelpTopic[] = [
	{title: 'Start here', lines: [
		note('Each session is a coding agent (Claude, Codex or Pi) in its own terminal. Quitting the UI leaves agents running.'),
		key('n', 'New session: pick an agent, name it, Tab picks the workspace, Enter launches'),
		key('o', 'Attach to the agent\'s terminal'),
		key('Ctrl+]', 'Back to Deckhand from an attached terminal (Ctrl+Space works too)'),
		key('j k', 'Select a session (or ↑↓, or its number)'),
		key('Tab', 'Switch the right pane: Preview, Terminal, Git, Dev, Notes'),
		key('C', 'Settings'),
		key('?', 'This guide; / searches it'),
		key('q', 'Quit the UI (agents and the daemon keep running)'),
	]},
	{title: 'Sessions', lines: [
		key('N', 'Child session in the selected one\'s workspace (can fork a Claude/Pi conversation)'),
		key('x  X', 'Stop / force-stop; while starting, cancel setup (the worktree is kept)'),
		key('s', 'Resume an exited session\'s conversation, or retry a failed setup'),
		key('S', 'Restart with a fresh conversation (needed when its ID is unknown)'),
		key('A', 'Archive / unarchive (doesn\'t stop it)'),
		key('Backspace', 'Remove an exited session from the list (its files stay)'),
		key('M', 'Mark as merged/pushed (your own marker, not checked)'),
		key('H', 'Export a Markdown handoff (notes, commits, changed files, diff stat) and open it'),
		key('F', 'New clean child session that starts from that handoff (not a conversation fork)'),
		note('Deleting a worktree on stop is blocked by uncommitted work or unpushed commits unless you type DELETE.'),
	]},
	{title: 'Find & organize', lines: [
		key('1-9  0', 'Select by number (0 is 10; with more than 10, type the number then Enter)'),
		key('/', 'Search titles, notes, agent, branch and path (Enter keeps it, Esc clears)'),
		key('f', 'Filter: active, archived, all, attention, running, exited'),
		key('!', 'Next session that needs attention'),
		key('J K', 'Move the session down / up among its siblings'),
		key('c', 'Hide exited children → collapse → expand'),
		key('h l', 'Narrow / widen the sidebar'),
		key('r', 'Refresh the list'),
	]},
	{title: 'Panes', lines: [
		key('p t g d a', 'Preview, Terminal, Git, Dev, Notes'),
		key('o', 'In Notes: edit them (Esc finishes; they autosave)'),
		key('v', 'Scroll the preview: j/k, g/G top/bottom, Esc returns'),
		key('[ ]', 'Preview scroll speed'),
		key('O', 'Open the workspace in Cursor / VS Code'),
		note('Agents ask for approvals in their own terminal: attach (o) to answer.'),
	]},
	{title: 'Dev & actions', lines: [
		key('d', 'Select the Dev pane; d again starts / stops the Dev command'),
		key('e', 'Run an action (a named command) in the Dev pane'),
		note('Set the Dev command, actions and a setup command for new worktrees in Settings (C).'),
	]},
	{title: 'Git & PRs', lines: [
		key('i', 'Workspace overview: branch, changes, diff size, base, upstream'),
		key('m', 'Merge / squash a worktree into the current branch, uncommitted, for review'),
		heading('Inside i'),
		key('c', 'Create a PR: push the branch, then open GitHub\'s PR form (asks first)'),
		key('P', 'Fetch PR and check status (gh)'),
		key('b', 'Open the PR in the browser'),
		key('g', 'Open lazygit'),
	]},
	{title: 'Settings', lines: [
		note('Two columns: Global (every repo) and This repo (deckhand.json in the main checkout; fine to commit). This repo wins.'),
		key('●', 'The value in effect'),
		key('⚠', 'A repo value that applies once trusted'),
		key('(built-in)', 'Nothing sets it: Deckhand\'s default'),
		note('Commands from a repo\'s deckhand.json only run once you trust it; you\'re asked right before the first one runs. Your own edits in Settings keep it trusted; changes from elsewhere need a new review.'),
	]},
	{title: 'Agent signals', lines: [
		note('Off by default; switch them on in Settings → Agents.'),
		key('Agent signals', 'Sessions report working, needs input, done, failed or rate-limited (! jumps to them)'),
		key('Notifications', 'Desktop notification when a session needs you or exits'),
		note('Claude works automatically. Codex needs `deckhand hooks codex` saved to ~/.codex/hooks.json and trusted with /hooks (Settings shows ⚠ Codex until then). Pi has no signals. "Done" means the agent stopped, not that the task succeeded.'),
	]},
];
