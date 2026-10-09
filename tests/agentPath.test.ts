import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import {spawn} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import {request} from '../src/client.js';
import type {ClientRequest, SessionRecord} from '../src/types.js';
import {cli, repo, waitFor, withEnv, fakeAgent, stop} from './helpers.js';

// An agent that printed nothing and exited (it could not start): the exited session must say so.
const silentAgent = `#!/usr/bin/env node
if (process.argv.includes('--help')) { console.log('PROMPT --settings resume'); process.exit(0); }
if (process.argv[2] === '--version') { console.log('1.0.0'); process.exit(0); }
process.exit(1);
`;

// The path stored at a session's first launch can disappear (the agent reinstalled elsewhere, e.g. Pi moving from an
// npm global to its own installer): restarting looks the agent up on PATH again instead of failing silently. PATH holds
// only the fakes, node and the system directories, so no real agent can be found.
test('restart finds an agent that moved, refuses one that is gone, and an agent that exits silently says it failed to start', {timeout: 120000}, async t => {
	const root = await repo();
	const home = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-path-'));
	const first = path.join(home, 'bin1'), second = path.join(home, 'bin2'), away = path.join(home, 'away');
	for (const directory of [first, second, away]) await fs.mkdir(directory);
	await fs.writeFile(path.join(first, 'claude'), fakeAgent, {mode: 0o755});
	await fs.writeFile(path.join(first, 'pi'), silentAgent, {mode: 0o755});
	withEnv(t, {DECKHAND_HOME: home});
	const env = {...process.env, HOME: home, DECKHAND_HOME: home, DECKHAND_DEV: '0', DECKHAND_AGENT_LATEST: '{}', PATH: [first, second, path.dirname(process.execPath), '/usr/bin', '/bin'].join(path.delimiter), SHELL: '/bin/sh', TEST_CLI: cli};
	const daemon = spawn(process.execPath, [cli, '--daemon'], {cwd: root, env, stdio: ['ignore', 'ignore', 'pipe']});
	daemon.stderr?.on('data', () => {});
	t.after(async () => { await stop(daemon); await fs.rm(root, {recursive: true, force: true}); await fs.rm(home, {recursive: true, force: true}); });
	const call = <T>(message: Omit<Extract<ClientRequest, {requestId: string}>, 'requestId'>) => request<T>({...message, requestId: randomUUID()} as Extract<ClientRequest, {requestId: string}>);
	await waitFor(async () => { try { return await call<{ok: boolean}>({type: 'ping'}); } catch { return {ok: false}; } }, result => result.ok);
	const state = (id: string) => call<SessionRecord[]>({type: 'list'}).then(items => items.find(item => item.id === id)!);
	const create = (title: string, program: string) => call<SessionRecord>({type: 'create', input: {title, program, cwd: root, repoRoot: root, cols: 80, rows: 24, worktreeMode: 'none'}} as any);

	const session = await create('moves', 'claude');
	await waitFor(() => state(session.id), item => item.status === 'running');
	assert.equal((await state(session.id)).command, path.join(first, 'claude'));
	await call({type: 'kill', sessionId: session.id} as any);
	await waitFor(() => state(session.id), item => item.status === 'exited');

	// Reinstalled elsewhere on PATH: s launches the new one.
	await fs.rename(path.join(first, 'claude'), path.join(second, 'claude'));
	await call({type: 'restart', sessionId: session.id, cols: 80, rows: 24, mode: 'resume'} as any);
	const restarted = await waitFor(() => state(session.id), item => item.status === 'running');
	assert.equal(restarted.command, path.join(second, 'claude'));
	await call({type: 'kill', sessionId: session.id} as any);
	await waitFor(() => state(session.id), item => item.status === 'exited');

	// Gone from PATH: a clear refusal instead of a launch that dies without a word.
	await fs.rename(path.join(second, 'claude'), path.join(away, 'claude'));
	await assert.rejects(call({type: 'restart', sessionId: session.id, cols: 80, rows: 24, mode: 'fresh'} as any), /Claude \(claude\) is not installed or not on PATH/);

	// An agent that exits before printing anything: the session says it failed to start, with the command.
	const silent = await create('silent', 'pi');
	const exited = await waitFor(() => state(silent.id), item => item.status === 'exited', 20000);
	assert.match(exited.lastPreview ?? '', /^pi exited with code 1 before showing anything: it failed to start\.\nCommand: .*bin1\/pi /);
});
