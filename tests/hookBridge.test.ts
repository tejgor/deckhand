import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {spawn} from 'node:child_process';
import {fileURLToPath} from 'node:url';
const cli = fileURLToPath(new URL('../dist/cli.js', import.meta.url));
test('hook bridge is bounded, always neutral, and never starts a daemon', {timeout: 6000}, async t => {
	const home = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-hook-bridge-')); t.after(() => fs.rm(home, {recursive: true, force: true}));
	const env = {...process.env, DECKHAND_HOME: home, DECKHAND_DEV: '0', DECKHAND_SESSION_ID: 'missing', DECKHAND_LAUNCH_ID: 'missing', DECKHAND_HOOK_TOKEN: 'missing'};
	for (const input of ['{', 'x'.repeat(100000), JSON.stringify({hook_event_name: 'PermissionRequest'}), undefined]) {
		const child = spawn(process.execPath, [cli, 'hook'], {env, stdio: ['pipe', 'pipe', 'pipe']});
		let output = ''; child.stdout.on('data', data => { output += data.toString(); }); child.stderr.resume(); child.stdin.on('error', () => {});
		if (input !== undefined) child.stdin.end(input); // undefined intentionally never closes stdin.
		const code = await new Promise(resolve => child.once('exit', resolve));
		assert.equal(code, 0); assert.equal(output, '{}\n');
	}
	assert.deepEqual(await fs.readdir(home), []);
});
