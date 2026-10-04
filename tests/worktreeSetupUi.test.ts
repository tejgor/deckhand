import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {launcher, terminalUi} from './helpers.js';

test('C → Worktree setup in the sandbox: suggestions are preselected, space links a skipped entry, Ctrl+S writes the repository worktree section', {timeout: 30000}, async t => {
	const home = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-ui-'));
	const {ui} = terminalUi(t, {args: [launcher, '--sandbox'], cwd: home, home});
	const {screen, press} = ui;
	await screen('DEV (isolated)'); await screen('● ready');
	const sandbox = path.join(home, 'sandbox'), file = path.join(sandbox, 'deckhand.json');
	await fs.mkdir(path.join(sandbox, 'node_modules', 'pkg'), {recursive: true}); await fs.writeFile(path.join(sandbox, 'node_modules', 'pkg', 'index.js'), 'module.exports = 1;\n');
	await fs.writeFile(path.join(sandbox, 'notes.txt'), 'local notes\n');

	press('C'); await screen('Edit configuration');
	press('j'); await screen('› Repository'); press('j'); await screen('› Worktree setup');
	press('\r'); await screen('Missing from new worktrees');
	await screen('[link] node_modules/');
	// Rows: Location, Branch from, Branch name, then candidates (suggested links first).
	press('j'); await screen('› Branch from'); press('j'); await screen('› Branch name'); press('j'); await screen('› [link] node_modules/');
	press('j'); await screen('› [skip] notes.txt');
	press(' '); await screen('› [link] notes.txt');
	press('\x13'); await screen('Saved worktree setup to deckhand.json');
	const saved = JSON.parse(await fs.readFile(file, 'utf8'));
	assert.deepEqual(saved, {defaultWorkspace: 'new', devCommand: 'npm run dev', actions: {test: 'npm test'}, worktree: {symlink: ['node_modules', 'notes.txt']}});
});
