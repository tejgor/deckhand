import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import {spawn, type ChildProcess} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import {request} from '../src/client.js';
import type {ClientRequest, SessionRecord, ProjectInfo, DevRecord, SessionCleanupInspection} from '../src/types.js';
import type {WorktreeInfo} from '../src/git.js';
import {cli, repo, git, waitFor, withEnv, isAlive} from './helpers.js';
import {loadAppConfig, loadState, saveState, updateAppConfig} from '../src/storage.js';
import {applyWorktreeSection, initialSetupModel, worktreeSection, type WorktreeSetupInfo} from '../src/worktreeSetup.js';
const fakeAgent = `#!/usr/bin/env node
const fs = require('fs');
const cp = require('child_process');
const path = require('path');
if (process.argv.includes('--help')) { console.log('PROMPT --settings --no-daemon resume'); process.exit(0); }
const provider = path.basename(process.argv[1]);
const id = 'fixture-native-' + provider;
fs.writeFileSync(path.join(process.env.DECKHAND_HOME, 'trace-' + process.env.DECKHAND_SESSION_ID + '.json'), JSON.stringify({args: process.argv.slice(2), token: process.env.DECKHAND_HOOK_TOKEN, launchId: process.env.DECKHAND_LAUNCH_ID, pid: process.pid}));
function hook(event) { return new Promise(resolve => { const child = cp.spawn(process.execPath, [process.env.TEST_CLI, 'hook'], {stdio: ['pipe','ignore','ignore']}); child.stdin.end(JSON.stringify({hook_event_name: event, session_id: id})); child.on('close', resolve); }); }
hook('SessionStart').then(() => console.log('ready'));
if (process.stdin.isTTY) process.stdin.setRawMode(true);
process.stdin.resume();
process.stdin.on('data', data => fs.appendFileSync(path.join(process.env.DECKHAND_HOME, 'input-' + process.env.DECKHAND_SESSION_ID), data));
setInterval(() => {}, 10000);
`;
async function stop(child: ChildProcess): Promise<void> {
	if (child.exitCode !== null || child.signalCode !== null) return;
	const ended = new Promise<void>(resolve => child.once('exit', () => resolve()));
	child.kill('SIGTERM'); await ended;
}
test('daemon features operate in isolated state with fake agents', {timeout: 60000}, async t => {
	const root = await repo();
	const home = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-state-'));
	const bin = path.join(home, 'bin'); await fs.mkdir(bin);
	for (const provider of ['claude', 'pi', 'codex']) await fs.writeFile(path.join(bin, provider), fakeAgent, {mode: 0o755});
	withEnv(t, {DECKHAND_HOME: home});
	const env = {...process.env, HOME: home, DECKHAND_HOME: home, DECKHAND_DEV: '0', PATH: `${bin}${path.delimiter}${process.env.PATH}`, SHELL: '/bin/sh', TEST_CLI: cli};
	const launch = () => { const child = spawn(process.execPath, [cli, '--daemon'], {cwd: root, env, stdio: ['ignore', 'ignore', 'pipe']}); child.stderr?.on('data', () => {}); return child; };
	let daemon = launch();
	t.after(async () => { await stop(daemon); await fs.rm(root, {recursive: true, force: true}); await fs.rm(home, {recursive: true, force: true}); });
	const call = <T>(message: Omit<Extract<ClientRequest, {requestId: string}>, 'requestId'>) => request<T>({...message, requestId: randomUUID()} as Extract<ClientRequest, {requestId: string}>);
	await waitFor(async () => { try { return await call<{ok: boolean}>({type: 'ping'}); } catch { return {ok: false}; } }, result => result.ok);
	const list = () => call<SessionRecord[]>({type: 'list'});
	const state = (id: string) => list().then(items => items.find(item => item.id === id)!);
	const create = (title: string, program: 'claude' | 'pi' | 'codex' = 'claude', worktreeMode: 'none' | 'new' = 'none') => call<SessionRecord>({type: 'create', input: {title, program, cwd: root, repoRoot: root, cols: 80, rows: 24, worktreeMode}} as any);
	const trace = async (id: string) => { try { return JSON.parse(await fs.readFile(path.join(home, `trace-${id}.json`), 'utf8')); } catch { return undefined; } };
	const killAndWait = async (id: string) => { await call({type: 'kill', sessionId: id} as any); await waitFor(() => state(id), item => item.status === 'exited'); };
	const relaunch = async (id: string, mode: 'resume' | 'fresh' = 'resume') => {
		const before = (await trace(id))?.launchId;
		await call({type: 'restart', sessionId: id, cols: 80, rows: 24, mode} as any);
		return (await waitFor(() => trace(id), item => item?.launchId !== before)).args as string[];
	};

	await t.test('hook bridge records the Codex identity, callbacks are authenticated, and a crashed daemon resumes the exact conversation', async () => {
		const session = await create('context', 'codex');
		await waitFor(() => state(session.id), item => item.status === 'running' && item.agentSessionRef?.value === 'fixture-native-codex');
		await call({type: 'archive-session', sessionId: session.id, archived: true} as any);
		const first = await trace(session.id);
		await assert.rejects(call({type: 'agent-hook', sessionId: session.id, launchId: first.launchId, token: 'wrong', payload: {hook_event_name: 'Stop'}} as any), /unauthorized/);
		await call({type: 'agent-hook', sessionId: session.id, launchId: first.launchId, token: first.token, payload: {hook_event_name: 'Stop'}} as any);
		assert.equal((await state(session.id)).attention?.state, 'response-ended');
		const died = new Promise<void>(resolve => daemon.once('exit', () => resolve()));
		daemon.kill('SIGKILL'); await died; daemon = launch();
		await waitFor(async () => { try { return await state(session.id); } catch { return undefined; } }, item => item?.exitReason === 'interrupted');
		assert.ok((await state(session.id)).archivedAt);
		await call({type: 'restart', sessionId: session.id, cols: 80, rows: 24} as any);
		const resumed = await waitFor(() => trace(session.id), item => item?.launchId !== first.launchId);
		assert.deepEqual(resumed.args.slice(0, 2), ['resume', 'fixture-native-codex']);
		assert.equal((await state(session.id)).archivedAt, undefined);
		await assert.rejects(call({type: 'agent-hook', sessionId: session.id, launchId: first.launchId, token: first.token, payload: {hook_event_name: 'Stop'}} as any), /Stale/);
		// Kill then restart inside one daemon: the old worker's exit must not mark the new launch exited.
		await killAndWait(session.id);
		await call({type: 'restart', sessionId: session.id, cols: 80, rows: 24} as any);
		const relaunched = await waitFor(() => trace(session.id), item => item?.launchId !== resumed.launchId);
		const running = await waitFor(() => state(session.id), item => item.status === 'running');
		assert.equal(running.launchId, relaunched.launchId);
		await killAndWait(session.id);
	});

	await t.test('untrusted repository commands never run while global defaults do; setup retries after inline trust; stale trust is refused', async () => {
		const exists = (file: string) => fs.access(file).then(() => true, () => false);
		// Global defaults are user-authored: they run without trust.
		await updateAppConfig(current => ({...current, defaults: {setupCommand: 'touch global-setup', devCommand: 'printf global-dev', actions: {gtest: 'printf global-action'}}}));
		// The repository override (uncommitted, live in the main checkout) and a creation hook that must not run untrusted.
		const setup = `touch repo-setup; node -e "const fs=require('fs'); if(!fs.existsSync('.attempt')){fs.writeFileSync('.attempt','1');console.log('first attempt');process.exit(1)} console.log('setup complete')"`;
		await fs.writeFile(path.join(root, 'deckhand.json'), JSON.stringify({setupCommand: setup, devCommand: 'printf repo-dev', actions: {test: 'printf action-output; exit 7'}}));
		const hook = path.join(root, '.claude', 'scripts', 'create-worktree.sh'); await fs.mkdir(path.dirname(hook), {recursive: true});
		await fs.writeFile(hook, '#!/bin/bash\ntouch "$CLAUDE_PROJECT_DIR/hook-ran"\nexit 1\n');
		let info = await call<ProjectInfo>({type: 'project-info', cwd: root} as any);
		assert.equal(info.needsReview, true); assert.equal(info.effective.setupCommand, 'touch global-setup');
		// Not reviewed: the repository setup is refused (the race path), the worktree retained, the hook never run.
		const unreviewed = await create('unreviewed-setup', 'claude', 'new');
		let item = await waitFor(() => state(unreviewed.id), result => result.status === 'exited');
		assert.equal(item.setup?.state, 'failed'); assert.match(item.setup?.output ?? '', /not trusted/); assert.equal(item.agentStartedAt, undefined);
		assert.equal(item.worktree?.creator, 'fallback'); assert.equal(await exists(path.join(root, 'hook-ran')), false);
		assert.equal(await exists(path.join(item.cwd, 'repo-setup')), false);
		// Reviewed and skipped: only the global setup runs.
		const skipped = await call<SessionRecord>({type: 'create', input: {title: 'skipped-setup', program: 'claude', cwd: root, repoRoot: root, cols: 80, rows: 24, worktreeMode: 'new', projectFingerprint: info.fingerprint}} as any);
		item = await waitFor(() => state(skipped.id), result => result.status === 'running');
		assert.equal(await exists(path.join(item.cwd, 'global-setup')), true); assert.equal(await exists(path.join(item.cwd, 'repo-setup')), false);
		await assert.rejects(call({type: 'run-action', sessionId: skipped.id, action: 'test', cols: 80, rows: 24} as any), /trust/);
		assert.match((await call<DevRecord>({type: 'run-action', sessionId: skipped.id, action: 'gtest', cols: 80, rows: 24} as any)).command ?? '', /global-action/);
		await waitFor(() => call<DevRecord>({type: 'watch-dev', sessionId: skipped.id, cols: 80, rows: 24} as any), result => !result.live);
		assert.match((await call<DevRecord>({type: 'start-dev', sessionId: skipped.id, cols: 80, rows: 24} as any)).command ?? '', /global-dev/);
		await call({type: 'stop-dev', sessionId: skipped.id} as any);
		await killAndWait(skipped.id);
		// A trust request for bytes that changed after review is refused.
		await fs.rm(path.join(root, '.claude'), {recursive: true});
		await assert.rejects(call({type: 'trust-project', cwd: root, fingerprint: info.fingerprint} as any), /changed/);
		info = await call<ProjectInfo>({type: 'project-info', cwd: root} as any);
		info = await call<ProjectInfo>({type: 'trust-project', cwd: item.cwd, fingerprint: info.fingerprint} as any);
		assert.equal(info.trusted, true); assert.equal(info.effective.setupCommand, setup);
		// After trust, s retries the refused setup with the repository command (first attempt fails, the retry succeeds).
		await call({type: 'restart', sessionId: unreviewed.id, cols: 80, rows: 24} as any).catch(() => {});
		item = await waitFor(() => state(unreviewed.id), result => result.status === 'exited');
		assert.match(item.setup?.output ?? '', /first attempt/);
		await call({type: 'restart', sessionId: unreviewed.id, cols: 80, rows: 24} as any);
		item = await waitFor(() => state(unreviewed.id), result => result.status === 'running');
		assert.equal(item.setup?.state, 'complete'); assert.equal(await exists(path.join(item.cwd, 'global-setup')), false);
		const run = await call<DevRecord>({type: 'run-action', sessionId: unreviewed.id, action: 'test', cols: 80, rows: 24} as any);
		assert.match(run.command ?? '', /action-output/);
		const output = await waitFor(() => call<DevRecord>({type: 'watch-dev', sessionId: unreviewed.id, cols: 80, rows: 24} as any), result => result.exitCode === 7);
		assert.match(output.content, /action-output/);
		// Editing the live main-checkout file revokes action rights at once.
		await fs.writeFile(path.join(root, 'deckhand.json'), JSON.stringify({actions: {test: 'printf edited'}}));
		await assert.rejects(call({type: 'run-action', sessionId: unreviewed.id, action: 'test', cols: 80, rows: 24} as any), /trust/);
		await assert.rejects(call({type: 'kill', sessionId: unreviewed.id, deleteWorktree: true} as any), /Deletion blocked/);
		assert.equal((await state(unreviewed.id)).status, 'running');
		await call({type: 'kill', sessionId: unreviewed.id, deleteWorktree: true, deleteBranch: true, allowDataLoss: true} as any);
		await waitFor(() => state(unreviewed.id), result => Boolean(result.worktree?.deletedAt));
		await assert.rejects(fs.access(item.cwd));
		await updateAppConfig(current => ({...current, defaults: undefined}));
	});

	await t.test('handoffs are initial context only and shared workspaces resist forced deletion', async () => {
		// The repository config now has no setup (and is untrusted); commit it so the main checkout stays clean.
		await git(root, 'add', 'deckhand.json'); await git(root, 'commit', '-m', 'no setup');
		const parent = await create('shared-feature', 'claude', 'new');
		await waitFor(() => state(parent.id), item => item.status === 'running');
		await call({type: 'update-session-notes', sessionId: parent.id, notes: 'Review this work'} as any);
		const handoff = await call<string>({type: 'export-handoff', sessionId: parent.id} as any);
		assert.match(await fs.readFile(handoff, 'utf8'), /Review this work[\s\S]*## Workspace changes\n\nBase: main/);
		const parentState = await state(parent.id);
		const child = await call<SessionRecord>({type: 'create', input: {title: 'review', program: 'pi', cwd: parentState.cwd, repoRoot: parentState.cwd, cols: 80, rows: 24, parentSessionId: parent.id, subSessionKind: 'clean', handoffFromSessionId: parent.id}} as any);
		await waitFor(() => state(child.id), item => item.status === 'running');
		const childTrace = await waitFor(() => trace(child.id), item => Boolean(item));
		assert.ok(childTrace.args.some((arg: string) => arg.includes(handoff)));
		await assert.rejects(call({type: 'kill', sessionId: parent.id, deleteWorktree: true, allowDataLoss: true} as any), /in use/);
		await killAndWait(child.id); await killAndWait(parent.id);
	});

	await t.test('unknown Codex identity never silently becomes a new conversation', async () => {
		await fs.writeFile(path.join(bin, 'codex'), fakeAgent.replace("hook('SessionStart').then(() => console.log('ready'));", "console.log('ready');"), {mode: 0o755});
		const session = await create('unknown-codex', 'codex');
		await waitFor(() => state(session.id), item => item.status === 'running');
		await killAndWait(session.id);
		await assert.rejects(call({type: 'restart', sessionId: session.id, cols: 80, rows: 24} as any), /conversation ID is unknown/);
	});

	await t.test('cancelling setup, including a retry, kills its process, retains files and stopped metadata', async () => {
		await fs.writeFile(path.join(root, 'deckhand.json'), JSON.stringify({setupCommand: `node -e "require('fs').writeFileSync('setup.pid', String(process.pid)); setTimeout(()=>{},300000)"`}));
		await git(root, 'add', 'deckhand.json'); await git(root, 'commit', '-m', 'long setup');
		const info = await call<ProjectInfo>({type: 'project-info', cwd: root} as any);
		await call({type: 'trust-project', cwd: root, fingerprint: info.fingerprint} as any);
		const session = await create('cancel-setup', 'claude', 'new');
		const {cwd} = await waitFor(() => state(session.id), item => item.setup?.state === 'running');
		const pidFile = path.join(cwd, 'setup.pid');
		const setupPid = async () => Number(await waitFor(() => fs.readFile(pidFile, 'utf8').catch(() => ''), Boolean));
		const first = await setupPid();
		await call({type: 'cancel-start', sessionId: session.id} as any);
		assert.equal((await state(session.id)).exitReason, 'stopped');
		await waitFor(async () => isAlive(first), alive => !alive);
		await fs.rm(pidFile); // Retained worktree; only the marker goes.
		const retryError = assert.rejects(call({type: 'restart', sessionId: session.id, cols: 80, rows: 24} as any), /cancelled|code/);
		await waitFor(() => state(session.id), item => item.status === 'starting' && item.setup?.state === 'running');
		const second = await setupPid();
		await call({type: 'cancel-start', sessionId: session.id} as any); await retryError;
		const stopped = await state(session.id); assert.equal(stopped.exitReason, 'stopped'); assert.equal(stopped.setup?.state, 'cancelled');
		await waitFor(async () => isAlive(second), alive => !alive);
	});

	await t.test('the main checkout can never be deleted, even with the data-loss override', async () => {
		const main = (await call<WorktreeInfo[]>({type: 'list-worktrees', cwd: root} as any)).find(item => item.isMain)!;
		const session = await call<SessionRecord>({type: 'create', input: {title: 'main-checkout', program: 'claude', cwd: root, repoRoot: root, cols: 80, rows: 24, worktreeMode: 'existing', existingWorktreePath: main.path}} as any);
		await waitFor(() => state(session.id), item => item.status === 'running');
		const inspection = await call<SessionCleanupInspection>({type: 'inspect-cleanup', sessionId: session.id} as any);
		assert.equal(inspection.safe, false); assert.match(inspection.structuralBlockers.join(), /main worktree/);
		await assert.rejects(call({type: 'kill', sessionId: session.id, deleteWorktree: true, deleteBranch: true, allowDataLoss: true} as any), /main worktree/);
		assert.equal((await state(session.id)).status, 'running');
		await killAndWait(session.id);
	});

	await t.test('global worktree settings place new worktrees and link shared files; create-pr pushes and opens gh, never from main', async () => {
		// The previous test left a trusted long-running setup; an untrusted empty override has nothing to run.
		await fs.writeFile(path.join(root, 'deckhand.json'), '{}'); await git(root, 'commit', '-am', 'empty config');
		const shared = path.join(home, 'shared-deps'); await fs.mkdir(shared); await fs.writeFile(path.join(shared, 'keep.js'), 'dependency');
		await fs.symlink(shared, path.join(root, 'deps')); await fs.writeFile(path.join(home, 'app.env'), 'SECRET=1');
		await updateAppConfig(current => ({...current, defaults: {worktree: {location: '{home}/custom/{repo}/{name}', symlink: ['deps', 'missing'], files: {'config/.env.local': '{home}/app.env'}}}}));
		const session = await create('linked-wt', 'claude', 'new');
		const item = await waitFor(() => state(session.id), result => result.status === 'running');
		assert.equal(item.cwd, path.join(await fs.realpath(home), 'custom', path.basename(root), 'linked-wt'));
		assert.deepEqual(item.worktree?.links?.linked, ['deps', 'config/.env.local']); assert.match(item.worktree?.links?.notes.join() ?? '', /missing: source missing/);
		assert.equal(await fs.readlink(path.join(item.cwd, 'deps')), await fs.realpath(shared));
		assert.equal(await fs.readFile(path.join(item.cwd, 'config', '.env.local'), 'utf8'), 'SECRET=1');

		// Create PR: a local bare origin and a fake gh that records its arguments.
		const origin = path.join(home, 'origin.git'); await git(home, 'init', '--bare', origin);
		await git(root, 'remote', 'add', 'origin', origin); await git(root, 'push', 'origin', 'main'); await git(root, 'fetch', 'origin');
		const calls = path.join(home, 'gh-calls.log');
		await fs.writeFile(path.join(bin, 'gh'), `#!/usr/bin/env node\nrequire('fs').appendFileSync(${JSON.stringify(calls)}, JSON.stringify(process.argv.slice(2)) + '\\n');\nif (process.argv[3] === 'view' && process.argv.includes('--json')) { console.error('no pull requests found for branch'); process.exit(1); }\n`, {mode: 0o755});
		await fs.writeFile(path.join(item.cwd, 'file.txt'), 'pr change\n'); await git(item.cwd, 'commit', '-am', 'PR change');
		const result = await call<{branch: string; remote: string; base?: string; existing: boolean}>({type: 'create-pr', sessionId: session.id, branch: 'linked-wt'} as any);
		assert.deepEqual(result, {branch: 'linked-wt', remote: 'origin', base: 'main', existing: false});
		assert.equal(await git(origin, 'rev-parse', 'refs/heads/linked-wt'), await git(item.cwd, 'rev-parse', 'HEAD'));
		const argv = (await fs.readFile(calls, 'utf8')).trim().split('\n').map(line => JSON.parse(line));
		assert.deepEqual(argv.at(-1), ['pr', 'create', '--web', '--head', 'linked-wt', '--base', 'main']);
		await assert.rejects(call({type: 'create-pr', sessionId: session.id, branch: 'other'} as any), /Branch changed/);
		const main = (await call<WorktreeInfo[]>({type: 'list-worktrees', cwd: root} as any)).find(entry => entry.isMain)!;
		const mainSession = await call<SessionRecord>({type: 'create', input: {title: 'main-pr', program: 'claude', cwd: root, repoRoot: root, cols: 80, rows: 24, worktreeMode: 'existing', existingWorktreePath: main.path}} as any);
		await waitFor(() => state(mainSession.id), entry => entry.status === 'running');
		await assert.rejects(call({type: 'create-pr', sessionId: mainSession.id} as any), /Refusing to create a PR from main/);
		await killAndWait(mainSession.id);

		// The links never force the data-loss override, and deleting the worktree keeps their targets.
		await call({type: 'kill', sessionId: session.id, deleteWorktree: true, deleteBranch: true} as any);
		await waitFor(() => state(session.id), entry => Boolean(entry.worktree?.deletedAt));
		await assert.rejects(fs.access(item.cwd));
		assert.equal(await fs.readFile(path.join(shared, 'keep.js'), 'utf8'), 'dependency');
		assert.equal(await fs.readFile(path.join(home, 'app.env'), 'utf8'), 'SECRET=1');
		await fs.rm(path.join(root, 'deps')); await updateAppConfig(current => ({...current, defaults: undefined}));
	});

	await t.test('worktree setup lists main-checkout candidates and sizes, and saves its section through save-config keeping other keys', async () => {
		await fs.mkdir(path.join(root, 'node_modules', 'pkg'), {recursive: true}); await fs.writeFile(path.join(root, 'node_modules', 'pkg', 'index.js'), 'x'.repeat(8192));
		await fs.writeFile(path.join(root, 'notes.txt'), 'n'); await fs.writeFile(path.join(root, 'deckhand.json'), '{"devCommand":"printf keep"}');
		const info = await call<WorktreeSetupInfo>({type: 'worktree-setup-info', cwd: root} as any);
		const byPath = new Map(info.candidates.map(candidate => [candidate.path, candidate]));
		assert.equal(byPath.get('node_modules')?.suggestion, 'link'); assert.equal(byPath.get('node_modules')?.kind, 'dir'); assert.equal(byPath.get('notes.txt')?.suggestion, 'skip');
		assert.equal(byPath.has('deckhand.json'), false); assert.equal(info.originBranch, 'main'); assert.equal(info.hook, undefined);
		const sizes = await call<Record<string, number | null>>({type: 'worktree-candidate-sizes', cwd: root, paths: ['node_modules', 'notes.txt']} as any);
		assert.ok((sizes.node_modules ?? 0) >= 8, JSON.stringify(sizes)); assert.equal(sizes['notes.txt'], 1);
		await assert.rejects(call({type: 'worktree-candidate-sizes', cwd: root, paths: ['../escape']} as any), /relative/);
		const model = {...initialSetupModel(info), location: 'inside' as const, branchFrom: 'default' as const};
		const expected = {location: '{repoRoot}/.worktrees/{name}', symlink: ['node_modules'], branchFrom: 'default'};
		const document = info.targets.repository!;
		const raw = applyWorktreeSection(document, worktreeSection(model, info));
		await call({type: 'save-config', target: 'repository', cwd: root, raw, revision: document.revision} as any);
		assert.deepEqual(JSON.parse(await fs.readFile(path.join(root, 'deckhand.json'), 'utf8')), {devCommand: 'printf keep', worktree: expected});
		await assert.rejects(call({type: 'save-config', target: 'repository', cwd: root, raw, revision: document.revision} as any), /changed on disk/);
		const trusted = (await loadAppConfig()).trustedProjects;
		await call({type: 'save-config', target: 'global', cwd: root, raw: applyWorktreeSection(info.targets.global!, worktreeSection({...model, target: 'global'}, info)), revision: info.targets.global!.revision} as any);
		const config = await loadAppConfig();
		assert.deepEqual(config.defaults, {worktree: expected}); assert.deepEqual(config.trustedProjects, trusted);
		await fs.rm(path.join(root, 'node_modules'), {recursive: true}); await fs.rm(path.join(root, 'notes.txt')); await fs.writeFile(path.join(root, 'deckhand.json'), '{}');
		await updateAppConfig(current => ({...current, defaults: undefined}));
	});

	await t.test('Claude and Pi launch with exact native IDs; forks never type into Pi; legacy refs still resume', async () => {
		const claude = await create('exact-claude');
		const started = await waitFor(() => state(claude.id), item => item.status === 'running' && item.attention?.event === 'SessionStart');
		const id = started.agentSessionRef!.value;
		assert.equal(started.agentSessionRef!.kind, 'id'); // The hook's other ID did not replace the assigned one.
		assert.deepEqual((await trace(claude.id)).args.slice(0, 4), ['--session-id', id, '--name', `dh-exact-claude-${claude.id.slice(0, 8)}`]);
		await killAndWait(claude.id);
		const resumed = await relaunch(claude.id);
		assert.deepEqual(resumed.slice(0, 2), ['--resume', id]); assert.ok(!resumed.includes('--session-id'));
		await killAndWait(claude.id);

		const realClaude = await fs.readFile(path.join(bin, 'claude'), 'utf8');
		await fs.writeFile(path.join(bin, 'claude'), `#!/usr/bin/env node\nconsole.log('No conversation found with session ID: ' + process.argv[process.argv.indexOf('--resume') + 1]);\nsetTimeout(() => process.exit(1), 200);\n`, {mode: 0o755});
		await call({type: 'restart', sessionId: claude.id, cols: 80, rows: 24} as any);
		const missing = await waitFor(() => state(claude.id), item => item.status === 'exited' && Boolean(item.lastPreview));
		assert.equal(missing.exitReason, 'failed'); assert.match(missing.lastPreview!, /Press S to start a fresh conversation/);
		await assert.rejects(call({type: 'restart', sessionId: claude.id, cols: 80, rows: 24} as any), /no saved conversation.*Use S/);
		await fs.writeFile(path.join(bin, 'claude'), realClaude, {mode: 0o755});
		const fresh = await relaunch(claude.id, 'fresh');
		assert.equal(fresh[0], '--session-id'); assert.notEqual(fresh[1], id);
		await killAndWait(claude.id);

		const pi = await create('exact-pi', 'pi');
		const piRef = (await waitFor(() => state(pi.id), item => item.status === 'running')).agentSessionRef!;
		assert.equal(piRef.kind, 'id');
		assert.deepEqual((await waitFor(() => trace(pi.id), Boolean)).args.slice(0, 2), ['--session-id', piRef.value]);
		const fork = await call<SessionRecord>({type: 'create', input: {title: 'branch', program: 'pi', cwd: root, repoRoot: root, cols: 80, rows: 24, parentSessionId: pi.id, subSessionKind: 'forked'}} as any);
		const forkRef = (await waitFor(() => state(fork.id), item => item.status === 'running')).agentSessionRef!;
		assert.notEqual(forkRef.value, piRef.value);
		assert.deepEqual((await waitFor(() => trace(fork.id), Boolean)).args.slice(0, 4), ['--fork', piRef.value, '--session-id', forkRef.value]);
		await new Promise(resolve => setTimeout(resolve, 800)); // Past the old /fork keystroke delay.
		assert.equal(await fs.readFile(path.join(home, `input-${fork.id}`), 'utf8').catch(() => ''), '');
		await killAndWait(fork.id);
		assert.deepEqual((await relaunch(fork.id)).slice(0, 2), ['--session-id', forkRef.value]);
		await killAndWait(fork.id); await killAndWait(pi.id);

		// Sessions persisted before exact IDs keep their name/path handles.
		await stop(daemon);
		const legacyPath = path.join(home, 'legacy-pi.jsonl');
		const legacy: Record<string, SessionRecord['agentSessionRef']> = {[claude.id]: {provider: 'claude', kind: 'name', value: 'dh-legacy'}, [pi.id]: {provider: 'pi', kind: 'path', value: legacyPath}};
		const saved = await loadState();
		await saveState({sessions: saved.sessions.map(item => legacy[item.id] ? {...item, agentSessionRef: legacy[item.id]} : item)});
		daemon = launch();
		await waitFor(async () => { try { return await call<{ok: boolean}>({type: 'ping'}); } catch { return {ok: false}; } }, result => result.ok);
		assert.deepEqual((await relaunch(claude.id)).slice(0, 2), ['--resume', 'dh-legacy']);
		assert.deepEqual((await relaunch(pi.id)).slice(0, 2), ['--session', legacyPath]);
		await killAndWait(claude.id); await killAndWait(pi.id);
	});
});
