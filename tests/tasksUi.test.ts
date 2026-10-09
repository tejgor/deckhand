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
	press('b'); await screen('IN PROGRESS · 1'); await screen('g go');
	press('\x1b'); await screen('o edit notes');

	// A note's checklist item goes to Tasks with ctrl+p and leaves a ↗ link in the note.
	press('o'); await screen('esc done');
	press('\x14'); await screen('- [ ]'); press('Follow up on review'); await screen('ctrl+p → tasks');
	press('\x10'); await screen('Sent to Tasks');
	press('\x1b'); await screen('↗ Follow up on review · in Tasks');
	press('b'); await screen('BACKLOG · 1'); await screen('Follow up on review');
	assert.match(await tasksFile(), /- \[ \] Follow up on review <!-- dh:t=[0-9a-f]{8} added=/);
});
