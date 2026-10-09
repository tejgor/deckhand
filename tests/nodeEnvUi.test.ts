import {test} from 'node:test';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {launcher, terminalUi, UI_TEST_TIMEOUT_MS} from './helpers.js';

// A stand-in agent that prints the NODE_ENV it was started with.
const fakeAgent = `#!/usr/bin/env node
if (process.argv.includes('--help')) { console.log('PROMPT --settings --no-daemon resume'); process.exit(0); }
if (process.argv.includes('--version')) { console.log('1.0.0'); process.exit(0); }
console.log('agent node-env=[' + (process.env.NODE_ENV ?? '') + ']');
process.stdin.resume();
setInterval(() => {}, 10000);
`;

// The UI sets NODE_ENV=production only while React loads (its fast build). It must not reach what the UI starts:
// the daemon, and through it every agent, shell, Dev command and action (npm there would skip devDependencies).
test('NODE_ENV=production set for the UI never reaches agents or actions', {timeout: UI_TEST_TIMEOUT_MS}, async t => {
	const home = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-ui-'));
	const bin = path.join(home, 'bin'); await fs.mkdir(bin);
	for (const agent of ['claude', 'pi', 'codex']) await fs.writeFile(path.join(bin, agent), fakeAgent, {mode: 0o755});
	await fs.writeFile(path.join(home, 'config.json'), JSON.stringify({defaults: {actions: {env: 'echo "action node-env=[$NODE_ENV]"'}}}));
	const {ui} = terminalUi(t, {args: [launcher, '--sandbox'], cwd: home, home, env: {PATH: `${bin}${path.delimiter}${process.env.PATH}`}});
	const {screen, press} = ui;
	await screen('DEV (isolated)'); await screen('● ready');
	press('n'); await screen('Choose an agent'); press('\r'); await screen('Workspace: new worktree');
	press('envs'); await screen('Name: envs'); press('\r');
	await screen('agent node-env=[]');
	// r: Dev first, then the global env action.
	press('r'); await screen('○ stopped'); press('j'); await screen('Selected command'); press('\r');
	await screen('action node-env=[]');
});
