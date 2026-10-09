import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {launcher, terminalUi, UI_TEST_TIMEOUT_MS} from './helpers.js';

test('C → Settings in the sandbox: a grid with a Global and a This repo column; ←→ picks the cell\'s layer (sticky across rows), Enter edits that layer, Linked items writes its worktree.symlink, x clears, T trusts, e opens the raw JSON; Agents rows are global only', {timeout: UI_TEST_TIMEOUT_MS}, async t => {
	const home = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-ui-'));
	const codexHome = path.join(home, 'codex'); await fs.mkdir(codexHome);
	const {ui} = terminalUi(t, {args: [launcher, '--sandbox'], cwd: home, home, env: {CODEX_HOME: codexHome}});
	const {screen, press} = ui;
	await screen('DEV (isolated)'); await screen('● ready');
	const sandbox = path.join(home, 'sandbox'), file = path.join(sandbox, 'deckhand.json');
	const readJson = async (name: string) => JSON.parse(await fs.readFile(name, 'utf8'));
	const original = await readJson(file);
	await fs.mkdir(path.join(sandbox, 'node_modules', 'pkg'), {recursive: true}); await fs.writeFile(path.join(sandbox, 'node_modules', 'pkg', 'index.js'), 'module.exports = 1;\n');
	await fs.writeFile(path.join(sandbox, 'notes.txt'), 'local notes\n');
	await fs.writeFile(path.join(home, 'config.json'), JSON.stringify({defaults: {actions: {fmt: 'npm run fmt'}}}));

	// ? : topics on the left, j/k (or a number) switch, / searches every topic, Esc steps back out.
	press('?'); await screen('Help · Start here'); await screen('9 Tasks');
	press('j'); await screen('Help · Sessions'); press('8'); await screen('Help · Settings');
	press('/'); await screen('type to search'); press('lazygit'); await screen('7  Git & PRs');
	press('\x1b'); await screen('Help · Settings'); press('\x1b'); await screen('C settings');

	// One grid, grouped by section; the cursor starts on the This repo column. The sandbox's untrusted Dev command is
	// in its cell (needs trust) while the built-in applies.
	press('C'); await screen('Settings · sandbox'); await screen('deckhand.json: needs trust');
	for (const text of ['Global (all repos)', 'This repo', 'General', 'Commands', 'Worktrees', 'dev (built-in)', 'npm run dev ⚠ needs trust', 'not set: the built-in default applies']) await screen(text);

	// Default agent in the repo column: a choice titled with its layer, saved to deckhand.json (which still needs review).
	press('\r'); await screen('Default agent · This repo'); await screen('○ claude');
	press('j'); await screen('❯ ○ pi'); press('j'); await screen('❯ ○ codex');
	press('\r'); await screen("Saved Default agent to this repo · review required (the file had changes you haven't reviewed)");
	assert.deepEqual(await readJson(file), {...original, defaultAgent: 'codex'});

	// ← moves to the Global column; the column stays put while the row changes. Dev command saves a global value.
	press('\x1b[D'); await screen('not set; this repo sets it'); press('j'); await screen('❯ Default workspace'); press('j'); await screen('No Dev command is set, so d runs a shell command named `dev`.');
	press('\r'); await screen('Dev command · Global'); await screen('built-in: dev');
	press('printf global'); await screen('printf global');
	press('\r'); await screen('Saved Dev command to global defaults');
	assert.deepEqual((await readJson(path.join(home, 'config.json'))).defaults, {actions: {fmt: 'npm run fmt'}, devCommand: 'printf global'});
	assert.deepEqual(await readJson(file), {...original, defaultAgent: 'codex'});
	press('j'); await screen('No setup command'); press('\r'); await screen('Setup command · Global'); press('\x1b'); await screen('No setup command');

	// → back to This repo; Linked items picks what new worktrees link, saved as this repo's worktree.symlink.
	press('\x1b[C'); await screen('This repo · '); press('\x1b[F'); await screen('❯ Notifications'); press('k'); await screen('❯ Agent signals'); press('k'); await screen('❯ Creation hook'); press('k'); await screen('❯ Linked items'); press('\r'); await screen('Settings › Linked items · This repo'); await screen('[link] node_modules/');
	press('j'); await screen('❯ [skip] notes.txt');
	press(' '); await screen('❯ [link] notes.txt');
	press('\r'); await screen('Saved Linked items to this repo · review required');
	assert.deepEqual(await readJson(file), {...original, defaultAgent: 'codex', worktree: {symlink: ['node_modules', 'notes.txt']}});

	// T reviews and trusts inline, then returns here; an edit made here now keeps the file trusted.
	press('T'); await screen('Review repository configuration');
	press('\r'); await screen('deckhand.json: trusted ✓'); await screen('Settings · sandbox');
	press('x'); await screen('Clear Linked items in this repo?');
	press('\r'); await screen('Cleared Linked items in this repo · still trusted'); await screen('deckhand.json: trusted ✓');
	assert.deepEqual(await readJson(file), {...original, defaultAgent: 'codex'});
	press('E'); await screen('Repository config · deckhand.json');
	press('\x1b'); await screen('Settings · sandbox');

	// Agents: global-only config.json flags. The repo cell says so and the cursor selects Global on these rows (from
	// the This repo column too); Agent signals saves agent_hooks and hints that Codex (its home exists here) has no
	// Deckhand hooks yet. Leaving the row returns to This repo.
	press('\x1b[F'); await screen('❯ Notifications'); press('k'); await screen('❯ Agent signals'); await screen('Off: attention (! and the session markers)');
	press('\r'); await screen('Agent signals · Global'); press('k'); await screen('❯ ○ on');
	press('\r'); await screen('Agent signals on (global)'); await screen('on ⚠ Codex'); await screen('Codex hooks not set up: node scripts/deckhand-dev.mjs hooks codex >');
	assert.equal((await readJson(path.join(home, 'config.json'))).agent_hooks, true);
	press('k'); await screen('❯ Creation hook'); await screen('This repo · ');
	press('\x1b'); await screen('C settings');
});

// A stand-in agent (never a real one): stays alive and prints a marker.
const fakeAgent = `#!/usr/bin/env node
if (process.argv.includes('--help')) { console.log('PROMPT --settings --no-daemon resume'); process.exit(0); }
if (process.argv.includes('--version')) { console.log('1.0.0'); process.exit(0); }
console.log('fake agent ready');
process.stdin.resume();
setInterval(() => {}, 10000);
`;

test('a Dev command saved in Settings keeps the repository file trusted (created and trusted, so d starts it without asking); an edit from outside Deckhand is reviewed on the next d; a guided new action saves', {timeout: UI_TEST_TIMEOUT_MS}, async t => {
	const home = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-ui-'));
	const bin = path.join(home, 'bin'); await fs.mkdir(bin);
	for (const agent of ['claude', 'pi', 'codex']) await fs.writeFile(path.join(bin, agent), fakeAgent, {mode: 0o755});
	const {ui} = terminalUi(t, {args: [launcher, '--sandbox'], cwd: home, home, env: {PATH: `${bin}${path.delimiter}${process.env.PATH}`}});
	const {screen, press} = ui;
	await screen('DEV (isolated)'); await screen('● ready');
	const file = path.join(home, 'sandbox', 'deckhand.json');
	const readJson = async (name: string) => JSON.parse(await fs.readFile(name, 'utf8'));
	// No repository file yet (nothing to review); the global Dev command applies.
	await fs.rm(file);
	await fs.writeFile(path.join(home, 'config.json'), JSON.stringify({defaults: {devCommand: 'echo fallback-$((40+2))'}}));
	press('n'); await screen('Choose an agent'); press('\r'); await screen('Workspace: no worktree');
	press('s1'); await screen('Name: s1'); press('\r'); await screen('fake agent ready');

	// The Dev command row: ← shows the global value selected and inherited, → the empty repo cell.
	press('C'); await screen('deckhand.json: not present'); press('j'); await screen('❯ Default workspace'); press('j'); await screen('not set: inherits global');
	press('\x1b[D'); await screen('inherited by this repo');
	press('\x1b[C'); await screen('not set: inherits global');
	press('\r'); await screen('Dev command · This repo'); await screen('global: echo fallback-$((40+2))');
	press('echo repo-$((40+2))'); await screen('› echo repo-$((40+2))');
	press('\r'); await screen('Saved Dev command to this repo · created and trusted'); await screen('deckhand.json: trusted ✓');
	assert.equal((await readJson(file)).devCommand, 'echo repo-$((40+2))');
	assert.equal(Object.keys((await readJson(path.join(home, 'config.json'))).trustedProjects ?? {}).length, 1);

	// A new action in this repo's Actions list: name (rules and a live check), then its command; still trusted.
	press('j'); await screen('❯ Setup command'); press('j'); await screen('❯ Actions'); press('\r'); await screen('Settings › Actions · This repo'); await screen('Named shell commands you run with e on a session');
	press('a'); await screen('New action · step 1 of 2: name'); await screen('starts with a letter or number');
	press('/'); await screen('must start with a letter or number');
	press('\x7f'); await screen('step 1 of 2: name'); press('lint frontend'); await screen('adds a new action to this repo');
	press('\r'); await screen('New action · step 2 of 2: command for lint frontend'); await screen("Runs with your shell in the session's worktree");
	press('npm run lint'); await screen('› npm run lint');
	press('\r'); await screen('Saved action lint frontend to this repo · still trusted · run it with e on a session');
	assert.deepEqual((await readJson(file)).actions, {'lint frontend': 'npm run lint'});
	press('\x1b'); await screen('Settings · sandbox'); press('\x1b'); await screen('C settings');

	// d only selects Dev; r opens the run list with Dev first, and Enter starts it: trusted, so no review.
	press('d'); await screen('d:Dev'); await screen('r start Dev');
	press('r'); await screen('○ stopped'); press('\r'); await screen('repo-42');
	// An edit from outside Deckhand: the next start reviews it.
	await fs.writeFile(file, `${JSON.stringify({devCommand: 'echo outside-$((40+2))'})}\n`);
	press('r'); await screen('reviewed first'); press('\r'); await screen('devCommand: echo outside-$((40+2))');
	await screen('s start echo fallback-$((40+2)) instead');
	// Esc steps back to the run list, a second Esc to the session list.
	press('\x1b'); await screen('enter start Dev');
	press('\x1b'); await screen('C settings');
});
