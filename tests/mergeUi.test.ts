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

test('m in a real PTY: preview, space toggles commit-first, squash; space marks done (☑); a conflict is aborted with x, then kept with enter', {timeout: UI_TEST_TIMEOUT_MS}, async t => {
	const home = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-ui-'));
	const bin = path.join(home, 'bin'); await fs.mkdir(bin);
	for (const agent of ['claude', 'pi', 'codex']) await fs.writeFile(path.join(bin, agent), fakeAgent, {mode: 0o755});
	const {ui} = terminalUi(t, {args: [launcher, '--sandbox'], cwd: home, home, env: {PATH: `${bin}${path.delimiter}${process.env.PATH}`}});
	const {screen, press} = ui;
	await screen('DEV (isolated)'); await screen('● ready');
	// The sandbox preselects a new worktree.
	press('n'); await screen('Choose an agent'); press('\r'); await screen('Workspace: new worktree');
	press('mrg'); await screen('Name: mrg'); press('\r'); await screen('fake agent ready');
	const worktree = path.join(home, 'worktrees', 'mrg'), sandbox = path.join(home, 'sandbox');
	await fs.writeFile(path.join(worktree, 'feature.txt'), 'feature\n');

	// Only uncommitted work: the preview says so, the toggle is on; space turns it off and on again.
	press('m'); await screen('No new commits · +1 uncommitted file'); await screen('☑ commit them first ("mrg")');
	press(' '); await screen('☐ commit them first ("mrg") · they stay in the worktree');
	press(' '); await screen('☑ commit them first ("mrg")');
	// Keys are sent one at a time (a chunk of two would arrive as one input).
	press('j'); await screen('❯ Squash merge into main'); press('\r');
	await screen('Committed 1 file, then squash applied from mrg into main');
	assert.equal(await git(worktree, 'log', '-1', '--format=%s'), 'mrg');
	assert.equal(await git(sandbox, 'diff', '--cached', '--name-only'), 'feature.txt');
	await git(sandbox, 'commit', '-m', 'squashed mrg');
	press(' '); await screen('Marked done: mrg'); await screen('✓ ☑ ✶');

	// Both sides change README.md: the conflict view; x aborts, then enter keeps it (in progress, marked merged).
	await fs.writeFile(path.join(worktree, 'README.md'), 'worktree side\n'); await git(worktree, 'commit', '-am', 'worktree readme');
	await fs.writeFile(path.join(sandbox, 'README.md'), 'main side\n'); await git(sandbox, 'commit', '-am', 'main readme');
	press('m'); await screen('2 commits'); press('\r');
	await screen('Merged with conflicts in 1 file'); await screen('x     abort the merge');
	press('x'); await screen('Merge aborted');
	assert.equal(await git(sandbox, 'status', '--porcelain'), '');
	press('m'); await screen('2 commits'); press('\r'); await screen('Merged with conflicts in 1 file');
	press('\r'); await screen('Merge kept with conflicts in 1 file into main');
	await waitFor(() => git(sandbox, 'status', '--porcelain'), status => /^UU README\.md$/m.test(status), UI_WAIT_MS);
});
