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

test('Notes tab: o edits with a real cursor (arrows, word jumps), ctrl+x makes a checklist item, tab edits the worktree\'s note, esc shows them rendered; files and editor edits round-trip', {timeout: UI_TEST_TIMEOUT_MS}, async t => {
	const home = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-ui-'));
	const bin = path.join(home, 'bin'); await fs.mkdir(bin);
	for (const agent of ['claude', 'pi', 'codex']) await fs.writeFile(path.join(bin, agent), fakeAgent, {mode: 0o755});
	const {ui} = terminalUi(t, {args: [launcher, '--sandbox'], cwd: home, home, env: {PATH: `${bin}${path.delimiter}${process.env.PATH}`}});
	const {screen, press} = ui;
	const notesFile = async (kind: string) => {
		const directory = path.join(home, 'notes', kind);
		const names = (await fs.readdir(directory).catch(() => [] as string[])).filter(name => name.endsWith('.md'));
		return names.length === 1 ? fs.readFile(path.join(directory, names[0]!), 'utf8') : undefined;
	};
	await screen('DEV (isolated)'); await screen('● ready');
	// The sandbox preselects a new worktree.
	press('n'); await screen('Choose an agent'); press('\r'); await screen('Workspace: new worktree');
	press('nts'); await screen('Name: nts'); press('\r'); await screen('fake agent ready');

	// A session in a worktree has one note, the worktree's.
	press('a'); await screen('No worktree notes · enter to add'); await screen('E open in editor');
	press('\r'); await screen('esc done');
	press('hello world'); await screen('hello world');
	// Left ×5 lands before "world"; Alt+← (ESC b, as macOS terminals send Option+←) jumps a word back.
	press('\x1b[D\x1b[D\x1b[D\x1b[D\x1b[D'); press('big '); await screen('hello big world');
	press('\x1bb'); press('\x1bb'); press('so '); await screen('so hello big world');
	// Ctrl+X makes the line a checklist item (raw Markdown while editing).
	press('\x18'); await screen('- [ ] so hello big world');
	// There is no second note to switch to.
	press('\t'); await screen('one note, shared by its sessions');
	// A bracketed paste is text: its newline and Tab are inserted (at the end of the line: Ctrl+E).
	press('\x05'); await screen('so hello big world');
	press('\x1b[200~\r\tpasted\x1b[201~'); await screen(' pasted');
	press('\x1b'); await screen('enter edit notes');
	// Read mode renders the item; the sidebar counts it.
	await screen('☐ so hello big world'); await screen('☐ 1 open');
	await waitFor(() => notesFile('worktrees'), text => text === '- [ ] so hello big world\n\tpasted', UI_WAIT_MS);
	// An edit made in an editor shows up without a restart.
	const directory = path.join(home, 'notes', 'worktrees');
	const [name] = (await fs.readdir(directory)).filter(entry => entry.endsWith('.md'));
	await fs.writeFile(path.join(directory, name!), 'shared plan\n- [x] edited in VS Code');
	await screen('☑ edited in VS Code');
	assert.equal(await notesFile('sessions'), undefined, 'nothing was written to a note of its own');
});
