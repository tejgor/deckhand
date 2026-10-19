import {test} from 'node:test';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {launcher, terminalUi, UI_TEST_TIMEOUT_MS, UI_WAIT_MS, waitFor} from './helpers.js';

const fakeAgent = `#!/usr/bin/env node
if (process.argv.includes('--help')) { console.log('PROMPT --settings --no-daemon resume'); process.exit(0); }
if (process.argv.includes('--version')) { console.log('1.0.0'); process.exit(0); }
console.log('fake agent ready');
process.stdin.resume();
setInterval(() => {}, 10000);
`;

// The run list (r) opens on the Dev row; x there stops the worktree's running action (as on any row), not Dev.
test('x in the run list stops the running action from the Dev row too', {timeout: UI_TEST_TIMEOUT_MS}, async t => {
	const home = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-ui-'));
	const bin = path.join(home, 'bin'); await fs.mkdir(bin);
	for (const agent of ['claude', 'pi', 'codex']) await fs.writeFile(path.join(bin, agent), fakeAgent, {mode: 0o755});
	await fs.writeFile(path.join(home, 'config.json'), JSON.stringify({defaults: {actions: {slow: 'echo slow-started; sleep 60'}}}));
	const {ui} = terminalUi(t, {args: [launcher, '--sandbox'], cwd: home, home, env: {PATH: `${bin}${path.delimiter}${process.env.PATH}`}});
	const {screen, press} = ui;
	await screen('DEV (isolated)'); await screen('● ready');
	press('n'); await screen('Choose an agent'); press('\r'); await screen('Workspace: new worktree');
	press('runs'); await screen('Name: runs'); press('\r');
	await screen('fake agent ready');
	press('r'); await screen('○ stopped'); press('j'); await screen('Selected command'); press('\r');
	await screen('│ slow-started'); // its output in the pane, not the command line in the run list
	// Its output shows while starting it still holds the UI busy (keys are dropped then): wait for the latest frame to
	// be idle before pressing r again: back on the session (its footer drawn, so not a half-drawn frame) without Working…,
	// which is drawn below that footer.
	await waitFor(() => ui.screenText(), frame => frame.includes('q quit') && !frame.includes('Working…'), UI_WAIT_MS);
	// Back in the run list, on the Dev row: the running action is named, and x stops it.
	press('r'); await screen('slow is still running in this worktree: x stops it.'); await screen('x stop slow');
	press('x'); await screen('Stopped slow');
});
