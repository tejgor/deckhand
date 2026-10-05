import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {launcher, terminalUi} from './helpers.js';

test('C → Worktree setup in the sandbox: suggestions are preselected, space links a skipped entry, Ctrl+S writes the repository worktree section; Effective settings shows sources and trusts inline', {timeout: 30000}, async t => {
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

	// C → Effective settings: untrusted, so the repository's values are shown as pending; Esc returns to the picker,
	// T trusts inline and returns here, and Enter on a repository row opens deckhand.json.
	press('C'); await screen('Edit configuration');
	press('j'); await screen('› Repository'); press('j'); await screen('› Worktree setup'); press('j'); await screen('› Effective settings');
	press('\r'); await screen('Effective settings · sandbox'); await screen('not trusted — repo values ignored'); await screen('(repo, pending trust) new'); await screen('built-in default');
	press('\x1b'); await screen('› Effective settings');
	press('\r'); await screen('Effective settings · sandbox');
	press('T'); await screen('Review repository configuration'); press('\r'); await screen('Repository deckhand.json: trusted ✓');
	press('j'); await screen('› defaultWorkspace'); press('j'); await screen('› devCommand');
	press('\r'); await screen('Repository config · deckhand.json');
	press('\x1b'); await screen('C config');
});
