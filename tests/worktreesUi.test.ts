import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {git, launcher, terminalUi, waitFor, UI_TEST_TIMEOUT_MS, UI_WAIT_MS} from './helpers.js';

// A stand-in agent (never a real one): stays alive and prints a marker.
const fakeAgent = `#!/usr/bin/env node
if (process.argv.includes('--help')) { console.log('PROMPT --settings --no-daemon resume'); process.exit(0); }
if (process.argv.includes('--version')) { console.log('1.0.0'); process.exit(0); }
console.log('fake agent ready');
process.stdin.resume();
setInterval(() => {}, 10000);
`;

test('W in a real PTY: merged worktrees grouped, x on the group deletes them and stops their sessions, DELETE overrides data loss', {timeout: UI_TEST_TIMEOUT_MS}, async t => {
	const home = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-ui-'));
	const bin = path.join(home, 'bin'); await fs.mkdir(bin);
	for (const agent of ['claude', 'pi', 'codex']) await fs.writeFile(path.join(bin, agent), fakeAgent, {mode: 0o755});
	const {ui} = terminalUi(t, {args: [launcher, '--sandbox'], cwd: home, home, env: {PATH: `${bin}${path.delimiter}${process.env.PATH}`}});
	const {screen, press} = ui;
	await screen('DEV (isolated)'); await screen('● ready');
	// A session in a new worktree whose branch is merged into main by hand while its agent runs.
	press('n'); await screen('Choose an agent'); press('\r'); await screen('Workspace: new worktree');
	press('shipped'); await screen('Name: shipped'); press('\r'); await screen('fake agent ready');
	const worktree = path.join(home, 'worktrees', 'shipped'), sandbox = path.join(home, 'sandbox');
	const branch = await git(worktree, 'branch', '--show-current');
	await fs.writeFile(path.join(worktree, 'feature.txt'), 'feature\n');
	await git(worktree, 'add', '.'); await git(worktree, 'commit', '-m', 'feature');
	await git(sandbox, 'merge', '--no-ff', '-m', 'merge shipped', branch);
	// A worktree made with git, with a file only it has.
	const scratch = path.join(home, 'scratch');
	await git(sandbox, 'worktree', 'add', '-b', 'scratch', scratch);
	await fs.writeFile(path.join(scratch, 'notes.txt'), 'only here\n');
	await fs.writeFile(path.join(scratch, 'README.md'), 'changed here\n');

	// Its worktree note has an open item left (Notes tab, o to edit, Ctrl+T adds an item).
	press('a'); await screen('enter edit notes'); press('o'); await screen('esc done');
	press('\x14'); await screen('- [ ]'); press('Follow up on perf'); await screen('Follow up on perf'); press('\x1b'); await screen('enter edit notes');

	// The session list's footer lists it beside b tasks.
	press('\x1b[A'); await screen('W worktrees');
	press('W'); await screen('⎇ Worktrees'); await screen('Merged · safe to delete · 1'); await screen('Main checkout');
	await screen('scratch'); await screen('not from Deckhand'); await screen('☐1');
	// The selection starts on the selected session's worktree; its details say why it can go.
	await screen('Clean: deleting it and its branch loses nothing');
	press('k'); await screen('x deletes them all');
	press('x'); await screen('Delete 1 merged worktree?'); await screen('Stops the session running there first: shipped');
	await screen('Stop 1 session, delete 1 worktree and their branches');
	// Its note's open item goes to the backlog first; space would keep it in the note.
	await screen('☑ 1 open note item → backlog first: Follow up on perf');
	press(' '); await screen('☐ 1 open note item stay in the (read-only) note'); press(' '); await screen('☑ 1 open note item → backlog first');
	press('\r'); await screen('Deleted 1 worktree and 1 branch · stopped 1 session · 1 open note item to the backlog');
	await waitFor(() => fs.access(worktree).then(() => true, () => false), exists => !exists, UI_WAIT_MS);
	assert.doesNotMatch(await git(sandbox, 'branch', '--list', branch), new RegExp(branch));

	// Not merged, with an untracked file: keeping the branch is offered first, and the file needs DELETE typed.
	// The main checkout is last; scratch is in progress above it.
	press('G'); await screen('› ◆ main'); press('k'); await screen('› ○ scratch');
	// The confirmation names what would be lost: each reason on its own line, then the files themselves.
	press('x'); await screen('Delete scratch?'); await screen('Deleting it would lose (typing DELETE overrides):');
	await screen('· 1 modified/staged file(s)'); await screen('· 1 untracked file(s)');
	await screen('Files (M changed · ? untracked · ! ignored):'); await screen('M README.md'); await screen('? notes.txt');
	await screen('Delete the worktree, keep its branch');
	press('\r'); await screen('Delete scratch anyway?'); await screen('? notes.txt'); await screen('Type DELETE then enter:');
	press('DELE'); await screen('Type DELETE then enter: DELE'); press('TE'); await screen('Type DELETE then enter: DELETE');
	press('\r'); await screen('Deleted scratch');
	await waitFor(() => fs.access(scratch).then(() => true, () => false), exists => !exists, UI_WAIT_MS);
	assert.match(await git(sandbox, 'branch', '--list', 'scratch'), /scratch/);
	await screen('No other worktrees');
	// Its session was stopped and archived: back in the list, f A shows it.
	press('\x1b'); await screen('Sessions');
	press('f'); press('A'); await screen('shipped'); await screen('backspace remove');
	// Removing it for good asks even with nothing open in its notes; Esc keeps it, Enter removes it.
	press('\x7f'); await screen('Remove “shipped” for good?'); await screen('enter remove · esc keep it');
	press('\x1b'); await screen('backspace remove');
	press('\x7f'); await screen('for good?'); press('\r'); await screen('No sessions');
});
