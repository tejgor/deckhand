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

test('steps: the worktree note\'s checklist under its task on the board (space ticks one), A sends a note\'s open items at once', {timeout: UI_TEST_TIMEOUT_MS}, async t => {
	const home = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-ui-'));
	const bin = path.join(home, 'bin'); await fs.mkdir(bin);
	for (const agent of ['claude', 'pi', 'codex']) await fs.writeFile(path.join(bin, agent), fakeAgent, {mode: 0o755});
	const {ui} = terminalUi(t, {args: [launcher, '--sandbox'], cwd: home, home, env: {PATH: `${bin}${path.delimiter}${process.env.PATH}`}});
	const {screen, press} = ui;
	const worktreeNote = async () => {
		const directory = path.join(home, 'notes', 'worktrees');
		const names = (await fs.readdir(directory).catch(() => [] as string[])).filter(name => name.endsWith('.md'));
		return names.length === 1 ? fs.readFile(path.join(directory, names[0]!), 'utf8') : '';
	};
	await screen('DEV (isolated)'); await screen('● ready');
	// A task, and a session started for it in a new worktree.
	press('b'); await screen('No tasks yet · a adds one');
	press('a'); await screen('New task'); press('Ship parser'); await screen('Ship parser'); press('\r'); await screen('Added to the backlog');
	press('n'); await screen('Choose an agent'); press('\r'); await screen('Workspace: new worktree'); press('\r'); await screen('fake agent ready');
	// Its worktree note gets a checklist (Notes tab, o to edit; Enter continues the list).
	press('a'); await screen('enter edit notes'); press('o'); await screen('esc done');
	press('\x14'); await screen('- [ ]'); press('Read the spec'); await screen('Read the spec');
	press('\r'); await screen('- [ ] '); press('Write tests'); await screen('Write tests');
	press('\x1b'); await waitFor(worktreeNote, text => text.includes('- [ ] Write tests'), UI_WAIT_MS);

	// The board: the steps under the task, counted; space on one ticks it in the note.
	press('b'); await screen('◆ Ship parser'); await screen('0/2'); await screen('☐ Read the spec'); await screen('☐ Write tests');
	press('j'); await screen('A step: a checklist item in the note of'); await screen('space tick');
	press(' '); await screen('Ticked: Read the spec'); await screen('1/2'); await screen('☑ Read the spec');
	await waitFor(worktreeNote, text => text.includes('- [x] Read the spec'), UI_WAIT_MS);
	// The session's Notes tab banner counts them too.
	press('\x1b'); await screen('1/2 steps');

	// The Notes view: A sends every open item of the selected note to Tasks; the note keeps a ↗ link each.
	press('b'); await screen('◆ Ship parser'); press('\t'); await screen('✎ Notes'); await screen('A send all');
	press('A'); await screen('Sent 1 item to Tasks'); await screen('↗ Write tests · in Tasks');
	assert.match(await worktreeNote(), /- ↗ Write tests <!-- dh:t=[0-9a-f]{8} -->/);
	// Its task is a follow-up of the worktree; ticking it shows · done on the ↗ line.
	press('\t'); await screen('☐ Write tests');
	press('G'); await screen('› ☐ Write tests'); press(' '); await screen('Done: Write tests');
	press('\t'); await screen('↗ Write tests · done');
});
