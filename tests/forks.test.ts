import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import {spawn} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import {request} from '../src/client.js';
import type {ClientRequest, SessionRecord} from '../src/types.js';
import {cli, repo, waitFor, withEnv, fakeAgent, withoutHooks, stop} from './helpers.js';
import {loadState, saveState} from '../src/storage.js';

// Forked sub-sessions against a real daemon with fake agents (its own daemon, so it runs beside daemonFeatures).
test('forked sub-sessions with fake agents', {timeout: 120000}, async t => {
	const root = await repo();
	const home = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-forks-'));
	const bin = path.join(home, 'bin'); await fs.mkdir(bin);
	for (const provider of ['claude', 'pi', 'codex']) await fs.writeFile(path.join(bin, provider), fakeAgent, {mode: 0o755});
	withEnv(t, {DECKHAND_HOME: home});
	const env = {...process.env, HOME: home, DECKHAND_HOME: home, DECKHAND_DEV: '0', PATH: `${bin}${path.delimiter}${process.env.PATH}`, SHELL: '/bin/sh', TEST_CLI: cli};
	const launch = () => { const child = spawn(process.execPath, [cli, '--daemon'], {cwd: root, env, stdio: ['ignore', 'ignore', 'pipe']}); child.stderr?.on('data', () => {}); return child; };
	let daemon = launch();
	t.after(async () => { await stop(daemon); await fs.rm(root, {recursive: true, force: true}); await fs.rm(home, {recursive: true, force: true}); });
	const call = <T>(message: Omit<Extract<ClientRequest, {requestId: string}>, 'requestId'>) => request<T>({...message, requestId: randomUUID()} as Extract<ClientRequest, {requestId: string}>);
	const ready = () => waitFor(async () => { try { return await call<{ok: boolean}>({type: 'ping'}); } catch { return {ok: false}; } }, result => result.ok);
	await ready();
	const state = (id: string) => call<SessionRecord[]>({type: 'list'}).then(items => items.find(item => item.id === id)!);
	const create = (title: string, program: 'claude' | 'pi' | 'codex' = 'claude') => call<SessionRecord>({type: 'create', input: {title, program, cwd: root, repoRoot: root, cols: 80, rows: 24}} as any);
	const forkOf = (parent: SessionRecord, title: string, worktreeMode: 'none' | 'new' = 'none') => call<SessionRecord>({type: 'create', input: {title, program: parent.program, cwd: parent.cwd, repoRoot: root, cols: 80, rows: 24, parentSessionId: parent.id, subSessionKind: 'forked', worktreeMode}} as any);
	const trace = async (id: string) => { try { return JSON.parse(await fs.readFile(path.join(home, `trace-${id}.json`), 'utf8')); } catch { return undefined; } };
	const killAndWait = async (id: string) => { await call({type: 'kill', sessionId: id} as any); await waitFor(() => state(id), item => item.status === 'exited'); };
	const relaunch = async (id: string) => {
		const before = (await trace(id))?.launchId;
		await call({type: 'restart', sessionId: id, cols: 80, rows: 24, mode: 'resume'} as any);
		return (await waitFor(() => trace(id), item => item?.launchId !== before)).args as string[];
	};
	const restartDaemonWith = async (edit: (sessions: SessionRecord[]) => SessionRecord[]) => {
		await stop(daemon);
		const saved = await loadState();
		await saveState({...saved, sessions: edit(saved.sessions)});
		daemon = launch();
		await ready();
	};

	await t.test('Claude forks launch with --fork-session and their own ID, never type, re-fork when nothing was saved, and keep legacy children working', async () => {
		const parent = await create('fork-parent');
		const parentRef = (await waitFor(() => state(parent.id), item => item.status === 'running' && Boolean(item.agentSessionRef))).agentSessionRef!;
		const child = await forkOf(parent, 'child');
		const started = await waitFor(() => state(child.id), item => item.status === 'running' && item.attention?.event === 'SessionStart');
		const childRef = started.agentSessionRef!;
		assert.equal(childRef.kind, 'id'); assert.notEqual(childRef.value, parentRef.value); // Assigned at launch; the hook's ID was not adopted.
		assert.deepEqual((await trace(child.id)).args.slice(0, 7), ['--resume', parentRef.value, '--fork-session', '--session-id', childRef.value, '--name', `dh-fork-parent-child-${child.id.slice(0, 8)}`]);
		await new Promise(resolve => setTimeout(resolve, 800)); // Past the old /branch keystroke delay.
		assert.equal(await fs.readFile(path.join(home, `input-${child.id}`), 'utf8').catch(() => ''), '');
		await killAndWait(child.id);
		const resumed = await relaunch(child.id);
		assert.deepEqual(resumed.slice(0, 2), ['--resume', childRef.value]); assert.ok(!resumed.includes('--session-id') && !resumed.includes('--fork-session'));
		await killAndWait(child.id);

		// The parent has no saved conversation yet: the fork fails, says so, and s forks again with a new child ID.
		const realClaude = await fs.readFile(path.join(bin, 'claude'), 'utf8');
		await fs.writeFile(path.join(bin, 'claude'), `#!/usr/bin/env node\nconst fs = require('fs'), path = require('path');\nfs.writeFileSync(path.join(process.env.DECKHAND_HOME, 'trace-' + process.env.DECKHAND_SESSION_ID + '.json'), JSON.stringify({args: process.argv.slice(2), launchId: process.env.DECKHAND_LAUNCH_ID}));\nconsole.log('No conversation found with session ID: ' + process.argv[process.argv.indexOf('--resume') + 1]);\nsetTimeout(() => process.exit(1), 200);\n`, {mode: 0o755});
		const unsaved = await forkOf(parent, 'unsaved');
		const failed = await waitFor(() => state(unsaved.id), item => item.status === 'exited');
		const firstTry = (await trace(unsaved.id)).args as string[];
		assert.equal(failed.exitReason, 'failed'); assert.match(failed.lastPreview!, /Press s to fork the parent again/);
		assert.deepEqual(failed.agentSessionRef, parentRef);
		await fs.writeFile(path.join(bin, 'claude'), realClaude, {mode: 0o755});
		const retried = await relaunch(unsaved.id);
		assert.deepEqual(retried.slice(0, 4), ['--resume', parentRef.value, '--fork-session', '--session-id']);
		assert.notEqual(retried[4], firstTry[4]); assert.notEqual(retried[4], parentRef.value);
		assert.equal((await waitFor(() => state(unsaved.id), item => item.status === 'running')).agentSessionRef?.value, retried[4]);
		await killAndWait(unsaved.id);

		// Claude forks may go into another worktree.
		const elsewhere = await forkOf(parent, 'elsewhere', 'new');
		const moved = await waitFor(() => state(elsewhere.id), item => item.status === 'running');
		assert.notEqual(moved.cwd, root); assert.equal(moved.worktree?.mode, 'managed');
		assert.deepEqual((await waitFor(() => trace(elsewhere.id), Boolean)).args.slice(0, 3), ['--resume', parentRef.value, '--fork-session']);
		await killAndWait(elsewhere.id); await killAndWait(parent.id);

		// Legacy children: a /branch name resumes by name; one still holding the parent's ref forks again. A legacy
		// name-ref parent forks by name.
		await restartDaemonWith(sessions => sessions.map(item =>
			item.id === child.id ? {...item, agentSessionRef: {provider: 'claude', kind: 'name', value: 'dh-legacy-branch'}}
			: item.id === unsaved.id ? {...item, agentSessionRef: parentRef}
			: item.id === parent.id ? {...item, agentSessionRef: {provider: 'claude', kind: 'name', value: 'dh-legacy-parent'}} : item));
		assert.deepEqual((await relaunch(child.id)).slice(0, 2), ['--resume', 'dh-legacy-branch']);
		const reforked = await relaunch(unsaved.id);
		assert.deepEqual(reforked.slice(0, 4), ['--resume', parentRef.value, '--fork-session', '--session-id']); assert.notEqual(reforked[4], parentRef.value);
		const fromName = await forkOf(await state(parent.id), 'from-name');
		const fromNameRef = (await waitFor(() => state(fromName.id), item => item.status === 'running')).agentSessionRef!;
		assert.deepEqual((await waitFor(() => trace(fromName.id), Boolean)).args.slice(0, 5), ['--resume', 'dh-legacy-parent', '--fork-session', '--session-id', fromNameRef.value]);
		await killAndWait(child.id); await killAndWait(unsaved.id); await killAndWait(fromName.id);
	});

	await t.test('Codex forks run codex fork <parent id>, capture their own ID without adopting the parent\'s, and stay in the parent\'s worktree', async () => {
		await fs.writeFile(path.join(bin, 'codex'), fakeAgent, {mode: 0o755});
		const parent = await create('codex-parent', 'codex');
		const parentId = (await waitFor(() => state(parent.id), item => item.status === 'running' && Boolean(item.agentSessionRef))).agentSessionRef!.value;
		const child = await forkOf(parent, 'child');
		const launched = await waitFor(() => trace(child.id), Boolean);
		assert.deepEqual(launched.args.slice(0, 2), ['fork', parentId]);
		await waitFor(() => fs.access(path.join(home, `ready-${launched.launchId}`)).then(() => true, () => false), Boolean); // All three SessionStart hooks sent.
		assert.equal((await state(child.id)).agentSessionRef?.value, 'fixture-child-codex');
		await killAndWait(child.id);
		assert.deepEqual((await relaunch(child.id)).slice(0, 2), ['resume', 'fixture-child-codex']);
		await killAndWait(child.id);

		// Without a reported ID the fork cannot be reopened: the exit says so and s forks the parent again.
		await fs.writeFile(path.join(bin, 'codex'), withoutHooks, {mode: 0o755});
		const silent = await forkOf(parent, 'silent');
		await waitFor(() => state(silent.id), item => item.status === 'running');
		await killAndWait(silent.id);
		assert.equal((await state(silent.id)).agentSessionRef, undefined); assert.match((await state(silent.id)).lastPreview!, /did not report this fork's conversation ID/);
		assert.deepEqual((await relaunch(silent.id)).slice(0, 2), ['fork', parentId]);
		await killAndWait(silent.id);

		// Codex may reopen a fork in the parent's directory, so other worktrees are refused before anything launches.
		await assert.rejects(forkOf(parent, 'elsewhere', 'new'), /Codex forks stay in the parent's worktree/);
		await assert.rejects(call({type: 'create', input: {title: 'elsewhere', program: 'codex', cwd: home, repoRoot: root, cols: 80, rows: 24, parentSessionId: parent.id, subSessionKind: 'forked'}} as any), /stay in the parent's worktree/);
		// A parent whose ID was never captured cannot be forked.
		const unknown = await create('codex-no-id', 'codex');
		await waitFor(() => state(unknown.id), item => item.status === 'running');
		await assert.rejects(forkOf(unknown, 'child'), /has not reported the parent's conversation ID/);
		await killAndWait(unknown.id); await killAndWait(parent.id);
	});
});
