import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import {spawn} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import {createLiveClient, request} from '../src/client.js';
import {sessionOutdated} from '../src/agentVersions.js';
import type {AgentUpdateResult, AgentVersions, ClientRequest, SessionRecord} from '../src/types.js';
import {cli, repo, waitFor, withEnv, fakeAgent, stop} from './helpers.js';

// Agent versions and updates against a real daemon with fake agents (its own daemon, beside the other daemon tests).
// The daemon's PATH holds only the fakes, node and the system directories, so no real agent can ever be run here.
test('agent versions and updates with fake agents', {timeout: 120000}, async t => {
	const root = await repo();
	const home = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-agent-updates-'));
	const bin = path.join(home, 'bin'), tools = path.join(home, 'tools');
	await fs.mkdir(bin); await fs.mkdir(tools);
	await fs.symlink(process.execPath, path.join(tools, 'node'));
	for (const provider of ['claude', 'codex']) await fs.writeFile(path.join(bin, provider), fakeAgent, {mode: 0o755});
	// Pi's updater fails (after logging how it was called).
	await fs.writeFile(path.join(bin, 'pi'), fakeAgent.replace("if (process.argv[2] === 'update') {", "if (process.argv[2] === 'update') { fs.appendFileSync(path.join(process.env.DECKHAND_HOME, 'updates.log'), JSON.stringify({provider, args: process.argv.slice(2)}) + '\\n'); console.log('Updating pi...'); console.error('npm error code EACCES'); process.exit(1); }\nif (false) {"), {mode: 0o755});
	withEnv(t, {DECKHAND_HOME: home});
	// The latest releases are stubbed (never npm): Claude and Codex have newer ones, Pi's is unknown.
	const env = {...process.env, HOME: home, DECKHAND_HOME: home, DECKHAND_DEV: '0', DECKHAND_AGENT_LATEST: JSON.stringify({claude: '1.0.1', codex: '1.0.5'}), PATH: [bin, tools, '/usr/bin', '/bin'].join(path.delimiter), SHELL: '/bin/sh', TEST_CLI: cli};
	const daemon = spawn(process.execPath, [cli, '--daemon'], {cwd: root, env, stdio: ['ignore', 'ignore', 'pipe']});
	daemon.stderr?.on('data', () => {});
	t.after(async () => { await stop(daemon); await fs.rm(root, {recursive: true, force: true}); await fs.rm(home, {recursive: true, force: true}); });
	const call = <T>(message: Omit<Extract<ClientRequest, {requestId: string}>, 'requestId'>) => request<T>({...message, requestId: randomUUID()} as Extract<ClientRequest, {requestId: string}>);
	await waitFor(async () => { try { return await call<{ok: boolean}>({type: 'ping'}); } catch { return {ok: false}; } }, result => result.ok);
	const state = (id: string) => call<SessionRecord[]>({type: 'list'}).then(items => items.find(item => item.id === id)!);
	const create = (title: string) => call<SessionRecord>({type: 'create', input: {title, program: 'claude', cwd: root, repoRoot: root, cols: 80, rows: 24}} as any);
	const running = (id: string) => waitFor(() => state(id), item => item.status === 'running' && Boolean(item.agentVersion));
	const updates = async () => (await fs.readFile(path.join(home, 'updates.log'), 'utf8').catch(() => '')).split('\n').filter(Boolean).map(line => JSON.parse(line));
	// A subscribed client receives agent-versions-updated broadcasts.
	const broadcasts: AgentVersions[] = [];
	const live = await createLiveClient({onAgentVersionsUpdated: versions => broadcasts.push(versions)});
	t.after(() => live.close());
	await live.subscribe(root);

	// Installed from `<agent> --version` in each agent's format, the latest from the stub.
	const versions = await call<AgentVersions>({type: 'agent-versions', refresh: true});
	assert.deepEqual({installed: versions.claude.installed, latest: versions.claude.latest, path: versions.claude.path}, {installed: '1.0.0', latest: '1.0.1', path: path.join(bin, 'claude')});
	assert.equal(versions.codex.installed, '1.0.0'); assert.equal(versions.codex.latest, '1.0.5');
	assert.equal(versions.pi.installed, '1.0.0'); assert.equal(versions.pi.latest, undefined); assert.match(versions.pi.error ?? '', /latest unknown/);
	assert.ok(versions.claude.checkedAt);

	// A session records the version it launched with; it is current.
	const before = await create('before update');
	const launched = await running(before.id);
	assert.equal(launched.agentVersion, '1.0.0');
	assert.equal((await call<AgentVersions>({type: 'agent-versions'})).claude.outdated, 0);

	// The update runs the agent's own command non-interactively, re-reads the version and broadcasts it; sessions are untouched.
	const result = await call<AgentUpdateResult>({type: 'update-agent', program: 'claude'});
	assert.equal(result.ok, true); assert.equal(result.exitCode, 0); assert.equal(result.command, 'claude update');
	assert.deepEqual([result.before, result.after], ['1.0.0', '1.0.1']);
	assert.match(result.output, /Updated 1\.0\.0 -> 1\.0\.1/);
	assert.deepEqual({installed: result.versions.claude.installed, running: result.versions.claude.running, outdated: result.versions.claude.outdated}, {installed: '1.0.1', running: 1, outdated: 1});
	assert.deepEqual(await updates(), [{provider: 'claude', args: ['update'], stdin: 'tty-or-null'}]); // stdin closed, not a pipe.
	await waitFor(async () => broadcasts, list => list.some(item => item.claude.installed === '1.0.1'));
	assert.ok(broadcasts.some(item => item.claude.updating), 'the update was broadcast as running');
	const after = await state(before.id);
	assert.equal(after.status, 'running'); assert.equal(after.pid, launched.pid); assert.equal(after.agentVersion, '1.0.0');
	assert.equal(sessionOutdated(after, result.versions.claude.installed), true);

	// A session launched after the update is current; restarting the outdated one makes it current too.
	const later = await create('after update');
	assert.equal((await running(later.id)).agentVersion, '1.0.1');
	assert.equal(sessionOutdated(await state(later.id), '1.0.1'), false);
	await call({type: 'kill', sessionId: before.id} as any);
	await waitFor(() => state(before.id), item => item.status === 'exited');
	assert.equal(sessionOutdated(await state(before.id), '1.0.1'), false, 'an exited session is never outdated');
	await call({type: 'restart', sessionId: before.id, cols: 80, rows: 24, mode: 'fresh'} as any);
	await waitFor(() => state(before.id), item => item.status === 'running' && item.agentVersion === '1.0.1');
	assert.equal((await call<AgentVersions>({type: 'agent-versions'})).claude.outdated, 0);
	// Persisted with the session.
	const stored = JSON.parse(await fs.readFile(path.join(home, 'state.json'), 'utf8')) as {sessions: SessionRecord[]};
	assert.equal(stored.sessions.find(item => item.id === later.id)?.agentVersion, '1.0.1');

	// One update at a time per agent: concurrent requests share one run.
	const [first, second] = await Promise.all([call<AgentUpdateResult>({type: 'update-agent', program: 'codex'}), call<AgentUpdateResult>({type: 'update-agent', program: 'codex'})]);
	assert.deepEqual([first.after, second.after, first.output], ['1.0.1', '1.0.1', second.output]);
	assert.equal((await updates()).filter(item => item.provider === 'codex').length, 1);

	// A failing updater: its exit code and output come back, the version stays.
	const failed = await call<AgentUpdateResult>({type: 'update-agent', program: 'pi'});
	assert.equal(failed.ok, false); assert.equal(failed.exitCode, 1); assert.equal(failed.command, 'pi update --self');
	assert.match(failed.output, /npm error code EACCES/); assert.deepEqual([failed.before, failed.after], ['1.0.0', '1.0.0']);
	assert.deepEqual((await updates()).find(item => item.provider === 'pi')?.args, ['update', '--self']);

	// Not installed: nothing runs; setup installs agents.
	await fs.rename(path.join(bin, 'pi'), path.join(home, 'pi-removed'));
	await assert.rejects(call({type: 'update-agent', program: 'pi'}), /Pi is not installed; run deckhand setup/);
	assert.equal((await call<AgentVersions>({type: 'agent-versions'})).pi.installed, undefined);
	await assert.rejects(call({type: 'update-agent', program: 'nope'} as any), /Unknown agent/);

	await call({type: 'kill', sessionId: before.id} as any); await call({type: 'kill', sessionId: later.id} as any);
});
