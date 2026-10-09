import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {launcher, terminalUi, waitFor, UI_TEST_TIMEOUT_MS, UI_WAIT_MS} from './helpers.js';

// A stand-in agent (never a real one): stays alive and prints a marker.
const fakeAgent = `#!/usr/bin/env node
if (process.argv.includes('--help')) { console.log('PROMPT --settings --no-daemon resume'); process.exit(0); }
if (process.argv.includes('--version')) { console.log('1.0.0'); process.exit(0); }
console.log('fake agent ready');
process.stdin.resume();
setInterval(() => {}, 10000);
`;

test('Tasks board: b opens it, a adds a task, n starts a session from it (base branch in the form), the Notes tab shows it, ctrl+p sends a note item to it', {timeout: UI_TEST_TIMEOUT_MS}, async t => {
	const home = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-ui-'));
	const bin = path.join(home, 'bin'); await fs.mkdir(bin);
	for (const agent of ['claude', 'pi', 'codex']) await fs.writeFile(path.join(bin, agent), fakeAgent, {mode: 0o755});
	const {ui} = terminalUi(t, {args: [launcher, '--sandbox'], cwd: home, home, env: {PATH: `${bin}${path.delimiter}${process.env.PATH}`}});
	const {screen, press} = ui;
	const tasksFile = async () => {
		const directory = path.join(home, 'notes', 'tasks');
		const names = (await fs.readdir(directory).catch(() => [] as string[])).filter(name => name.endsWith('.md'));
		return names.length === 1 ? fs.readFile(path.join(directory, names[0]!), 'utf8') : '';
	};
	await screen('DEV (isolated)'); await screen('● ready');

	press('b'); await screen('☐ Tasks'); await screen('No tasks yet · a adds one'); await screen('a add');
	press('a'); await screen('New task'); await screen('tab details');
	press('Write the docs'); await screen('Write the docs');
	press('\t'); await screen('ctrl+s save');
	// Each control key waits for the screen: a key written with the text before it can arrive as one chunk.
	press('why: people ask'); await screen('why: people ask'); press('\x13'); await screen('Added to the backlog');
	await screen('☐ 1 task · b');
	assert.match(await tasksFile(), /^- \[ \] Write the docs <!-- dh:t=[0-9a-f]{8} added=\d{4}-\d\d-\d\d -->\n  why: people ask\n$/);

	// n: the usual form, named after the task, in a new worktree; ↑↓ picks where its branch starts.
	press('n'); await screen('Choose an agent'); press('\r');
	await screen('Task: Write the docs'); await screen('Name: Write the docs'); await screen('Workspace: new worktree');
	await screen('Base: current checkout'); await screen('↑↓ 1/');
	press('\x1b[B'); await screen('↑↓ 2/'); press('\x1b[A'); await screen('↑↓ 1/');
	press('\r'); await screen('fake agent ready');
	const linked = await waitFor(tasksFile, text => / wt=[0-9a-f-]{36}/.test(text), UI_WAIT_MS);
	assert.match(linked, /^- \[ \] Write the docs <!-- dh:t=[0-9a-f]{8} wt=/);

	press('a'); await screen('◆ Task Write the docs'); await screen('b board');
	// The board groups open tasks by worktree: ◆ the task it was started for. v shows only this session's worktree,
	// where a adds a follow-up (☐) to it.
	press('b'); await screen('◆ Write the docs'); await screen('⎇ write'); await screen('o session');
	assert.doesNotMatch(await screen('◆ Write the docs'), /Write the docs +… │/, 'a row fits its pane');
	press('v'); await screen('v all tasks');
	press('a'); await screen('New task · ⎇ write'); press('Add tests'); await screen('Add tests');
	press('\r'); await screen('Added to ⎇ write'); await screen('☐ Add tests');
	assert.match(await tasksFile(), /- \[ \] Add tests <!-- dh:t=[0-9a-f]{8} wt=[0-9a-f-]{36} assigned=\d{4}-\d\d-\d\d added=/);
	press('v'); await screen('BACKLOG');
	press('\x1b'); await screen('+1 open here');
	press('\x1b'); await screen('enter edit notes');

	// A note's checklist item goes to Tasks with ctrl+p and leaves a ↗ link in the note.
	press('o'); await screen('esc done');
	press('\x14'); await screen('- [ ]'); press('Follow up on review'); await screen('ctrl+p → tasks');
	press('\x10'); await screen('Sent to Tasks');
	press('\x1b'); await screen('↗ Follow up on review · in Tasks');
	// A note item sent from a worktree session lands on that worktree; w moves it to the backlog.
	press('b'); await screen('☐ Follow up on review');
	assert.match(await tasksFile(), /- \[ \] Follow up on review <!-- dh:t=[0-9a-f]{8} wt=[0-9a-f-]{36} assigned=\d{4}-\d\d-\d\d added=/);
	// tab: every note, by worktree; f: only open checklist items (here none: the item became a task).
	press('\t'); await screen('✎ Notes'); await screen('Worktree note'); await screen('↗ Follow up on review · in Tasks');
	press('f'); await screen('No open checklist items in these notes');
	press('f'); await screen('↗ Follow up on review · in Tasks');
	press('\t'); await screen('☐ Tasks');
	// w: the backlog and the way back to its note come first, then the worktrees (Tab: main-checkout sessions); typing searches.
	press('G'); await screen('› ☐ Follow up on review');
	press('w'); await screen('Move “Follow up on review” to'); await screen('↩ Back to its note'); await screen('› ⎇ write');
	press('\t'); await screen('No sessions in the main checkout');
	press('\t'); await screen('› ⎇ write');
	press('zz'); await screen('Nothing in worktrees matches “zz”');
	press('\x1b'); await screen('type to search');
	press('\x1b[A'); await screen('› ↩ Back to its note');
	press('\x1b[A'); await screen('› Backlog · no worktree');
	press('\r'); await screen('Moved to the backlog'); await screen('BACKLOG · 1');
	assert.match(await tasksFile(), /- \[ \] Follow up on review <!-- dh:t=[0-9a-f]{8} added=/);
	// And back to the note it was sent from: an open item there again, gone from the list.
	press('w'); await screen('↩ Back to its note');
	press('\x1b[A'); await screen('› ↩ Back to its note');
	press('\r'); await screen('Back in the note of'); await screen('Nothing waiting');
	assert.doesNotMatch(await tasksFile(), /Follow up on review/);

	// Keymap: [ goes to the previous tab (Notes → Dev), x asks before stopping, Esc backs out of it, Space marks done.
	press('\x1b'); await screen('enter edit notes');
	press('['); await screen('d d start Dev');
	press('x'); await screen('Kill only, keep worktree'); press('\x1b'); await screen('d d start Dev');
	press('\x1b[A'); press('\x1b[B'); await screen('x stop');
	press(' '); await screen('Marked done');
	// Backspace archives a finished session; in the archived view (f A) Backspace removes it for good.
	press('X'); await screen('s resume');
	press('\x7f'); await screen('Archived Write the docs'); await screen('No sessions match');
	press('f'); await screen('A  archived'); press('A'); await screen('A unarchive'); await screen('backspace remove');
	// Removing always asks; its note has the open item sent back from Tasks, so it is listed and x removes anyway.
	press('\x7f'); await screen('for good?'); await screen('Follow up on review');
	press('x'); await screen('No sessions yet.');
});
