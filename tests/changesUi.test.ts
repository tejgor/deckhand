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

test('Git tab: the worktree\'s Changes list; v focuses it, space stages the selected file, the diff previews it, esc returns', {timeout: UI_TEST_TIMEOUT_MS}, async t => {
	const home = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-ui-'));
	const bin = path.join(home, 'bin'); await fs.mkdir(bin);
	for (const agent of ['claude', 'pi', 'codex']) await fs.writeFile(path.join(bin, agent), fakeAgent, {mode: 0o755});
	const {ui} = terminalUi(t, {args: [launcher, '--sandbox'], cwd: home, home, env: {PATH: `${bin}${path.delimiter}${process.env.PATH}`}});
	const {screen, press} = ui;
	await screen('DEV (isolated)'); await screen('● ready');
	// The sandbox preselects a new worktree.
	press('n'); await screen('Choose an agent'); press('\r'); await screen('Workspace: new worktree');
	press('chg'); await screen('Name: chg'); press('\r'); await screen('fake agent ready');
	const worktree = path.join(home, 'worktrees', 'chg');
	await fs.writeFile(path.join(worktree, 'README.md'), '# Deckhand dev sandbox\n\nEdited in the worktree.\n');
	await fs.writeFile(path.join(worktree, 'notes.md'), 'scratch\n');

	// Browse: the list only (read-only), grouped like VS Code, with the keys in the footer.
	press('g'); await screen('→ changes • enter lazygit');
	await screen('Changes 1'); await screen('Untracked 1'); await screen('README.md'); await screen('1 changed · 1 untracked');
	// Focus: the selection starts on the first file; its diff shows below the list.
	press('\x1b[C'); await screen('space stage/unstage'); await screen('unstaged · README.md'); await screen('+Edited in the worktree.');
	press(' '); await screen('Staged README.md'); await screen('Staged Changes 1');
	await waitFor(() => git(worktree, 'diff', '--cached', '--name-only'), names => names === 'README.md', UI_WAIT_MS);
	// The selection moved on to the untracked file; J scrolls nothing it cannot, esc returns to browse.
	await screen('untracked · notes.md');
	press('\x1b'); await screen('→ changes • enter lazygit');
	assert.equal(await git(worktree, 'status', '--porcelain'), 'M  README.md\n?? notes.md');
});
