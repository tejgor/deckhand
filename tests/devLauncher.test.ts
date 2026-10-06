import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {spawn, type ChildProcess} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import {isolatedHome, createSandbox} from '../scripts/deckhand-dev.mjs';
import {request} from '../src/client.js';
import {cli, exec, launcher, tempDir, waitFor, withEnv} from './helpers.js';

test('dev isolation rejects production paths and symlink aliases', async t => {
	const temp = await tempDir(t, 'deckhand-launcher-');
	const production = path.join(os.homedir(), '.deckhand');
	await assert.rejects(isolatedHome(production), /separate/);
	await assert.rejects(isolatedHome(path.join(production, 'dev')), /separate/);
	await fs.symlink(production, path.join(temp, 'alias'));
	await assert.rejects(isolatedHome(path.join(temp, 'alias')), /separate/);
	assert.equal(await isolatedHome(path.join(temp, 'preview')), path.join(await fs.realpath(temp), 'preview'));
	withEnv(t, {DECKHAND_HOME: path.join(temp, 'custom'), DECKHAND_CHANNEL: undefined});
	await assert.rejects(isolatedHome(path.join(await fs.realpath(temp), 'custom')), /separate/);
});
test('dev sandbox does not overwrite existing work and is built aside', async t => {
	const home = await tempDir(t, 'deckhand-sandbox-');
	const cwd = await createSandbox(home);
	await fs.writeFile(path.join(cwd, 'README.md'), 'keep my work');
	await createSandbox(home); assert.equal(await fs.readFile(path.join(cwd, 'README.md'), 'utf8'), 'keep my work');
	assert.deepEqual((await fs.readdir(home)).filter(name => name.startsWith('sandbox.tmp-')), []);
});
test('dev status/stop control only the isolated dev daemon; source helpers resolve outside the checkout', {timeout: 15000}, async t => {
	// HOME is a temp dir, so "production" (~/.deckhand) is a throwaway stable daemon, never the user's.
	const fakeHome = await fs.mkdtemp(path.join(os.tmpdir(), 'dh-home-')), devHome = path.join(fakeHome, 'dev'), stableHome = path.join(fakeHome, '.deckhand');
	const base = {...process.env, HOME: fakeHome, DECKHAND_DEV: '0', DECKHAND_AGENT_LATEST: '{}'} as NodeJS.ProcessEnv;
	for (const key of ['DECKHAND_HOME', 'DECKHAND_CHANNEL', 'DECKHAND_SESSION_ID', 'DECKHAND_LAUNCH_ID', 'DECKHAND_HOOK_TOKEN']) delete base[key];
	const env = {...base, DECKHAND_DEV_HOME: devHome};
	const daemons: ChildProcess[] = [];
	const start = (extra: NodeJS.ProcessEnv) => { const child = spawn(process.execPath, [cli, '--daemon'], {env: {...base, ...extra}, stdio: 'ignore'}); daemons.push(child); return child; };
	t.after(async () => { for (const child of daemons) if (child.exitCode === null && child.signalCode === null) { child.kill('SIGTERM'); await new Promise(resolve => child.once('exit', resolve)); } await fs.rm(fakeHome, {recursive: true, force: true}); });
	const stable = start({});
	// Not canonicalized: macOS reports /var/... while the launcher resolves /private/var/....
	const dev = start({DECKHAND_HOME: devHome, DECKHAND_CHANNEL: 'dev'});
	const pingStable = async () => {
		const previous = process.env.DECKHAND_HOME; process.env.DECKHAND_HOME = stableHome;
		const pending = request<{channel: string}>({type: 'ping', requestId: randomUUID()}, 1500); // Socket path is read synchronously.
		if (previous === undefined) delete process.env.DECKHAND_HOME; else process.env.DECKHAND_HOME = previous;
		return pending;
	};
	await waitFor(async () => (await exec(process.execPath, [launcher, 'status'], {env})).stdout, output => output.startsWith('Isolated dev daemon running'));
	await waitFor(() => pingStable().catch(() => undefined), result => result?.channel === 'stable');
	const source = await exec(process.execPath, [launcher, '--source', 'hooks', 'codex'], {cwd: fakeHome, env});
	assert.match(source.stdout, /SessionStart/); assert.doesNotMatch(source.stdout, /StopFailure/);
	await assert.rejects(exec(process.execPath, [launcher, 'stop'], {env: {...env, DECKHAND_DEV_HOME: stableHome}}), /separate/);
	const ended = new Promise(resolve => dev.once('exit', resolve));
	await exec(process.execPath, [launcher, 'stop'], {env}); await ended;
	assert.match((await exec(process.execPath, [launcher, 'status'], {env})).stdout, /No isolated dev daemon/);
	assert.equal((await pingStable()).channel, 'stable'); assert.equal(stable.exitCode, null);
});
