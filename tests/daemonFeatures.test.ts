import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import {spawn} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import net from 'node:net';
import {once} from 'node:events';
import {attachJsonParser, request, writeMessage} from '../src/client.js';
import {getSocketPath, getWorkerPidPath} from '../src/paths.js';
import type {ActionRecord, ChangeDiff, ChangesRecord, ClientRequest, SessionRecord, ProjectInfo, DevRecord, GitRecord, ServerMessage, SessionCleanupInspection, TerminalRecord} from '../src/types.js';
import type {WorktreeInfo} from '../src/git.js';
import {cli, repo, git, waitFor, withEnv, isAlive, fakeAgent, withoutHooks, stop} from './helpers.js';
import {loadAppConfig, loadState, saveState, updateAppConfig} from '../src/storage.js';
import type {SettingsInfo, WorktreeCandidates} from '../src/settingsInfo.js';
import {applyChange, initialLinks, linkSelection, infoLayer} from '../src/settingsModel.js';
import {workspaceKey, workspacePaneUnavailable, workspaceWorkerId} from '../src/workspace.js';
test('daemon features operate in isolated state with fake agents', {timeout: 180000}, async t => {
	const root = await repo();
	const home = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-state-'));
	const bin = path.join(home, 'bin'); await fs.mkdir(bin);
	for (const provider of ['claude', 'pi', 'codex']) await fs.writeFile(path.join(bin, provider), fakeAgent, {mode: 0o755});
	withEnv(t, {DECKHAND_HOME: home});
	const env = {...process.env, HOME: home, DECKHAND_HOME: home, DECKHAND_DEV: '0', DECKHAND_AGENT_LATEST: '{}', PATH: `${bin}${path.delimiter}${process.env.PATH}`, SHELL: '/bin/sh', TEST_CLI: cli};
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
		return (await waitFor(() => trace(id), item => Boolean(item) && item.launchId !== before)).args as string[];
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
		await updateAppConfig(current => ({...current, defaults: {setupCommand: 'touch global-setup', devCommand: 'printf global-dev', actions: {gtest: 'printf global-action', gsleep: 'sleep 30'}}}));
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
		const watchAction = (id: string) => call<ActionRecord>({type: 'watch-action', sessionId: id, cols: 80, rows: 24} as any);
		const ran = await call<ActionRecord>({type: 'run-action', sessionId: skipped.id, action: 'gtest', cols: 80, rows: 24} as any);
		assert.match(ran.command ?? '', /global-action/); assert.equal(ran.name, 'gtest');
		assert.equal((await waitFor(() => watchAction(skipped.id), result => !result.live && result.exitCode === 0)).name, 'gtest');
		// Actions have their own process: they never touch Dev, and one runs at a time per worktree until stopped.
		assert.equal((await call<DevRecord>({type: 'watch-dev', sessionId: skipped.id, cols: 80, rows: 24} as any)).command, undefined);
		assert.equal((await call<ActionRecord>({type: 'run-action', sessionId: skipped.id, action: 'gsleep', cols: 80, rows: 24} as any)).live, true);
		await assert.rejects(call({type: 'run-action', sessionId: skipped.id, action: 'gtest', cols: 80, rows: 24} as any), /gsleep action is still running/);
		assert.match((await call<DevRecord>({type: 'start-dev', sessionId: skipped.id, cols: 80, rows: 24} as any)).command ?? '', /global-dev/);
		assert.equal((await watchAction(skipped.id)).live, true);
		await call({type: 'stop-action', sessionId: skipped.id} as any);
		await waitFor(() => watchAction(skipped.id), result => !result.live);
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
		const run = await call<ActionRecord>({type: 'run-action', sessionId: unreviewed.id, action: 'test', cols: 80, rows: 24} as any);
		assert.match(run.command ?? '', /action-output/);
		const output = await waitFor(() => watchAction(unreviewed.id), result => result.exitCode === 7);
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
		// A session in a worktree writes in the worktree's note.
		const shared = (await state(parent.id)).sharedNotes!;
		await call({type: 'save-note', sessionId: parent.id, section: 'shared', noteId: `${shared.kind}:${shared.id}`, text: 'Review this work', revision: shared.revision} as any);
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
		await fs.writeFile(path.join(bin, 'codex'), withoutHooks, {mode: 0o755});
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

	await t.test('Settings reads sources and both documents, lists main-checkout link candidates and sizes, and saves single keys through save-config keeping other keys', async () => {
		await fs.mkdir(path.join(root, 'node_modules', 'pkg'), {recursive: true}); await fs.writeFile(path.join(root, 'node_modules', 'pkg', 'index.js'), 'x'.repeat(8192));
		await fs.writeFile(path.join(root, 'notes.txt'), 'n'); await fs.writeFile(path.join(root, 'deckhand.json'), '{"devCommand":"printf keep"}');
		const info = await call<SettingsInfo>({type: 'settings-info', cwd: root} as any);
		assert.equal(info.repository.state, 'untrusted'); assert.equal(info.originBranch, 'main'); assert.equal(info.hookFile, undefined);
		assert.deepEqual(info.rows.find(row => row.key === 'devCommand')?.pending?.value, 'printf keep');
		const {candidates} = await call<WorktreeCandidates>({type: 'worktree-candidates', cwd: root} as any);
		const byPath = new Map(candidates.map(candidate => [candidate.path, candidate]));
		assert.equal(byPath.get('node_modules')?.suggestion, 'link'); assert.equal(byPath.get('node_modules')?.kind, 'dir'); assert.equal(byPath.get('notes.txt')?.suggestion, 'skip');
		assert.equal(byPath.has('deckhand.json'), false);
		const sizes = await call<Record<string, number | null>>({type: 'worktree-candidate-sizes', cwd: root, paths: ['node_modules', 'notes.txt']} as any);
		assert.ok((sizes.node_modules ?? 0) >= 8, JSON.stringify(sizes)); assert.equal(sizes['notes.txt'], 1);
		await assert.rejects(call({type: 'worktree-candidate-sizes', cwd: root, paths: ['../escape']} as any), /relative/);
		const symlink = linkSelection(infoLayer(info, 'repository'), infoLayer(info, 'global'), candidates, initialLinks(candidates)).symlink;
		assert.deepEqual(symlink, ['node_modules']);
		const document = info.targets.repository!;
		const raw = applyChange(document, {path: ['worktree', 'symlink'], value: symlink});
		// Saved over bytes nobody reviewed (written outside Deckhand): the file still needs review.
		const saved = await call<{trust?: string}>({type: 'save-config', target: 'repository', cwd: root, raw, revision: document.revision} as any);
		assert.equal(saved.trust, 'unreviewed'); assert.equal((await call<ProjectInfo>({type: 'project-info', cwd: root} as any)).trusted, false);
		assert.deepEqual(JSON.parse(await fs.readFile(path.join(root, 'deckhand.json'), 'utf8')), {devCommand: 'printf keep', worktree: {symlink: ['node_modules']}});
		await assert.rejects(call({type: 'save-config', target: 'repository', cwd: root, raw, revision: document.revision} as any), /changed on disk/);
		const trusted = (await loadAppConfig()).trustedProjects;
		await call({type: 'save-config', target: 'global', cwd: root, raw: applyChange(info.targets.global!, {path: ['worktree', 'branchFrom'], value: 'default'}), revision: info.targets.global!.revision} as any);
		const config = await loadAppConfig();
		assert.deepEqual(config.defaults, {worktree: {branchFrom: 'default'}}); assert.deepEqual(config.trustedProjects, trusted);
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
		const missing = await waitFor(() => state(claude.id), item => item.status === 'exited' && /Press S/.test(item.lastPreview ?? ''));
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
		await saveState({...saved, sessions: saved.sessions.map(item => legacy[item.id] ? {...item, agentSessionRef: legacy[item.id]} : item)});
		daemon = launch();
		await waitFor(async () => { try { return await call<{ok: boolean}>({type: 'ping'}); } catch { return {ok: false}; } }, result => result.ok);
		assert.deepEqual((await relaunch(claude.id)).slice(0, 2), ['--resume', 'dh-legacy']);
		assert.deepEqual((await relaunch(pi.id)).slice(0, 2), ['--session', legacyPath]);
		await killAndWait(claude.id); await killAndWait(pi.id);
	});

	await t.test('sessions in one worktree share one Dev that outlives their agents; another worktree has its own', async () => {
		await updateAppConfig(current => ({...current, defaults: {devCommand: `node -e "console.log('dev-pid=' + process.pid + ' cwd=' + process.cwd()); setInterval(() => {}, 1000)"`}}));
		const watch = (id: string) => call<DevRecord>({type: 'watch-dev', sessionId: id, cols: 120, rows: 24} as any);
		const startDev = (id: string) => call<DevRecord>({type: 'start-dev', sessionId: id, cols: 120, rows: 24} as any);
		const devPid = async (id: string) => Number((await waitFor(() => watch(id), record => record.live && /dev-pid=\d+/.test(record.content))).content.match(/dev-pid=(\d+)/)![1]);
		const devRunning = (ids: string[], expected: boolean[]) => waitFor(() => list(), items => ids.every((id, index) => Boolean(items.find(item => item.id === id)?.devRunning) === expected[index]));
		// A new worktree and a second session attached to it share one workspace; a third worktree is another.
		const first = await create('ws-shared', 'claude', 'new');
		const firstState = await waitFor(() => state(first.id), item => item.status === 'running');
		const second = await call<SessionRecord>({type: 'create', input: {title: 'ws-attached', program: 'pi', cwd: root, repoRoot: root, cols: 80, rows: 24, worktreeMode: 'existing', existingWorktreePath: firstState.cwd}} as any);
		const solo = await create('ws-solo', 'claude', 'new');
		const [secondState, soloState] = await Promise.all([second, solo].map(session => waitFor(() => state(session.id), item => item.status === 'running')));
		const key = workspaceKey(firstState)!;
		assert.equal(workspaceKey(secondState!), key); assert.notEqual(workspaceKey(soloState!), key);

		const started = await startDev(first.id);
		assert.equal(started.live, true); assert.equal(started.workspace, key); assert.equal(started.sessionId, first.id);
		const pid = await devPid(second.id);
		assert.equal(await devPid(first.id), pid);
		// Starting from the other session reuses the shared process.
		assert.equal((await startDev(second.id)).sessionId, second.id);
		assert.equal(await devPid(second.id), pid);
		await devRunning([first.id, second.id, solo.id], [true, true, false]);
		const soloDev = await watch(solo.id);
		assert.equal(soloDev.live, false); assert.equal(soloDev.workspace, workspaceKey(soloState!));
		await startDev(solo.id);
		const soloPid = await devPid(solo.id);
		assert.notEqual(soloPid, pid);
		assert.ok((await watch(solo.id)).content.includes(`cwd=${soloState!.cwd}`));
		await devRunning([first.id, second.id, solo.id], [true, true, true]);

		// Dev outlives an agent: still visible from, stoppable and startable by the exited session.
		await killAndWait(second.id);
		assert.ok(isAlive(pid)); assert.equal((await watch(second.id)).live, true);
		await devRunning([first.id, second.id], [true, true]);
		await call({type: 'stop-dev', sessionId: second.id} as any);
		await waitFor(async () => isAlive(pid), alive => !alive);
		await devRunning([first.id, second.id], [false, false]);
		assert.equal((await watch(first.id)).live, false);
		await startDev(second.id);
		const restarted = await devPid(first.id);

		// Deleting the worktree stops its Dev before removal; a deleted worktree has no Dev.
		await call({type: 'kill', sessionId: first.id, deleteWorktree: true, deleteBranch: true} as any);
		const deleted = await waitFor(() => state(first.id), item => Boolean(item.worktree?.deletedAt));
		assert.equal(isAlive(restarted), false); assert.equal(deleted.devRunning, false);
		await devRunning([second.id], [false]);
		await assert.rejects(startDev(first.id), /worktree was deleted/);

		// Removing a workspace's last session stops its Dev.
		await killAndWait(solo.id);
		assert.ok(isAlive(soloPid));
		await call({type: 'remove', sessionId: solo.id} as any);
		await waitFor(async () => isAlive(soloPid), alive => !alive);
		await updateAppConfig(current => ({...current, defaults: undefined}));
	});

	await t.test('sessions in one worktree share one Terminal and one lazygit, also once exited; the workspace stops them with its worktree or last session', async () => {
		// A fake lazygit (bin is first on the daemon's PATH) that reports its PID and cwd and stays up.
		const lazygit = path.join(bin, 'lazygit');
		await fs.writeFile(lazygit, `#!/usr/bin/env node\nconsole.log('lazygit-pid=' + process.pid + ' cwd=' + process.cwd());\nsetInterval(() => {}, 1000);\n`, {mode: 0o755});
		await updateAppConfig(current => ({...current, defaults: {devCommand: `node -e "setInterval(() => {}, 1000)"`}}));
		// One persistent connection: the fire-and-forget input path, and a UI-like watcher that receives fan-out.
		const socket = net.createConnection(getSocketPath()); await once(socket, 'connect');
		const events: ServerMessage[] = []; attachJsonParser(socket, message => void events.push(message));
		const send = (message: object) => writeMessage(socket, message as ClientRequest);
		t.after(() => { socket.destroy(); return fs.rm(lazygit, {force: true}); });
		// Wide enough that no echoed worktree path wraps.
		const watch = (id: string) => call<TerminalRecord>({type: 'watch-terminal', sessionId: id, cols: 300, rows: 24} as any);
		const watchGit = (id: string) => call<GitRecord>({type: 'watch-git', sessionId: id, cols: 300, rows: 24} as any);
		// Typed lines echo `$$`; only the shell's output has digits there.
		const shell = async (id: string) => {
			const marker = randomUUID().slice(0, 8);
			await waitFor(() => watch(id), record => record.live); // Input reaches only a started shell.
			send({type: 'terminal-input', sessionId: id, data: `echo ${marker}=$$:$(pwd -P)\r`});
			const [, pid, cwd] = (await waitFor(() => watch(id), record => new RegExp(`${marker}=\\d+:`).test(record.content))).content.match(new RegExp(`${marker}=(\\d+):(\\S+)`))!;
			return {pid: Number(pid), cwd};
		};
		const gitPid = async (id: string) => Number((await waitFor(() => watchGit(id), record => record.live && /lazygit-pid=\d+/.test(record.content))).content.match(/lazygit-pid=(\d+)/)![1]);
		const first = await create('term-shared', 'claude', 'new');
		const firstState = await waitFor(() => state(first.id), item => item.status === 'running');
		const second = await call<SessionRecord>({type: 'create', input: {title: 'term-attached', program: 'pi', cwd: root, repoRoot: root, cols: 80, rows: 24, worktreeMode: 'existing', existingWorktreePath: firstState.cwd}} as any);
		const solo = await create('term-solo', 'claude', 'new');
		const [, soloState] = await Promise.all([second, solo].map(session => waitFor(() => state(session.id), item => item.status === 'running')));
		const key = workspaceKey(firstState)!;
		const workerPidFile = getWorkerPidPath(workspaceWorkerId(key));

		// Terminal and Git start on first view, in the workspace worker; both sessions see the same shell and lazygit.
		const viewed = await watch(first.id);
		assert.equal(viewed.workspace, key); assert.equal(viewed.sessionId, first.id);
		await fs.access(workerPidFile);
		const {pid: shellPid, cwd} = await shell(first.id);
		assert.equal(cwd, await fs.realpath(firstState.cwd));
		assert.equal((await shell(second.id)).pid, shellPid);
		const lazygitPid = await gitPid(first.id);
		assert.equal(await gitPid(second.id), lazygitPid);
		const solos = await shell(solo.id);
		assert.notEqual(solos.pid, shellPid); assert.equal(solos.cwd, await fs.realpath(soloState!.cwd));
		assert.notEqual(await gitPid(solo.id), lazygitPid);

		// Fan-out: a client watching one session sees output typed through another session of the workspace.
		send({type: 'watch-terminal', requestId: randomUUID(), sessionId: first.id, cols: 300, rows: 24});
		send({type: 'terminal-input', sessionId: second.id, data: 'echo fanout-$((40+2))\r'});
		await waitFor(async () => events.some(event => event.type === 'terminal-updated' && event.terminal.sessionId === first.id && event.terminal.workspace === key && /fanout-42/.test(event.terminal.content)), Boolean);
		assert.ok(!events.some(event => event.type === 'terminal-updated' && event.terminal.sessionId !== first.id));

		// Attach goes through the workspace, mirrors bracketed paste, and allows one attacher per pane.
		send({type: 'terminal-input', sessionId: first.id, data: `printf '\\033[?2004h'; echo paste-$((2+3))\r`});
		await waitFor(() => watch(first.id), record => /paste-5/.test(record.content));
		const attachId = randomUUID();
		send({type: 'attach-terminal', requestId: attachId, sessionId: second.id, cols: 300, rows: 30});
		const attached = await waitFor(async () => events.find(event => event.type === 'response' && event.requestId === attachId), Boolean) as Extract<ServerMessage, {type: 'response'}>;
		assert.equal(attached.ok, true);
		assert.equal((attached as {data?: {terminalModes?: {bracketedPaste?: boolean}}}).data?.terminalModes?.bracketedPaste, true);
		await assert.rejects(call({type: 'attach-terminal', sessionId: first.id, cols: 80, rows: 24} as any), /already attached elsewhere/);
		send({type: 'terminal-input', sessionId: second.id, data: 'echo attached-$((1+1))\r'});
		await waitFor(async () => events.some(event => event.type === 'terminal-output' && event.sessionId === second.id && /attached-2/.test(event.data)), Boolean);
		send({type: 'terminal-detach', sessionId: second.id});
		await waitFor(async () => events.some(event => event.type === 'terminal-detached' && event.sessionId === second.id), Boolean);

		// The panes outlive agents: an exited session still has the shared shell and lazygit.
		await killAndWait(second.id);
		assert.ok(isAlive(shellPid)); assert.equal((await watch(second.id)).live, true);
		assert.equal((await shell(second.id)).pid, shellPid);
		// A lazygit that exited starts again on the next view.
		process.kill(lazygitPid, 'SIGKILL');
		await waitFor(() => watchGit(first.id), record => record.live && !record.content.includes(`lazygit-pid=${lazygitPid}`));
		const relaunchedGit = await gitPid(second.id);
		assert.notEqual(relaunchedGit, lazygitPid);
		// Stopping Dev keeps a worker whose Terminal/Git are in use.
		await call({type: 'start-dev', sessionId: second.id, cols: 80, rows: 24} as any);
		await call({type: 'stop-dev', sessionId: second.id} as any);
		await new Promise(resolve => setTimeout(resolve, 300));
		assert.ok(isAlive(shellPid)); assert.equal((await shell(first.id)).pid, shellPid);

		// Kill-with-delete stops the workspace's panes before removing the worktree; the deleted session has no workspace.
		await call({type: 'kill', sessionId: first.id, deleteWorktree: true, allowDataLoss: true} as any);
		const deleted = await waitFor(() => state(first.id), item => Boolean(item.worktree?.deletedAt));
		assert.equal(isAlive(shellPid), false); assert.equal(isAlive(relaunchedGit), false);
		await assert.rejects(fs.access(workerPidFile));
		await assert.rejects(fs.access(deleted.cwd));
		const gone = await watch(first.id);
		assert.equal(gone.live, false); assert.equal(gone.workspace, undefined);
		await assert.rejects(call({type: 'attach-terminal', sessionId: first.id, cols: 80, rows: 24} as any), /Terminal is unavailable: its worktree was deleted/);
		// The deletion belongs to the worktree: its other (exited) session has no workspace either.
		assert.equal((await state(second.id)).worktree?.deletedAt, deleted.worktree?.deletedAt);
		assert.equal((await watch(second.id)).workspace, undefined);

		// Removing a workspace's last session stops its shell (which never exits on its own).
		await killAndWait(solo.id);
		assert.ok(isAlive(solos.pid));
		await call({type: 'remove', sessionId: solo.id} as any);
		await waitFor(async () => isAlive(solos.pid), alive => !alive);
		await waitFor(() => fs.access(getWorkerPidPath(workspaceWorkerId(workspaceKey(soloState!)!))).then(() => true, () => false), exists => !exists);
		socket.destroy(); await fs.rm(lazygit, {force: true});
		await updateAppConfig(current => ({...current, defaults: undefined}));
	});

	await t.test('the Git tab\'s Changes are per worktree: watched sessions get pushed updates; stage/unstage only touch listed paths', async () => {
		const socket = net.createConnection(getSocketPath()); await once(socket, 'connect');
		const events: ServerMessage[] = []; attachJsonParser(socket, message => void events.push(message));
		t.after(() => socket.destroy());
		const pushed = async (sessionId: string, matches: (changes: ChangesRecord) => boolean) => {
			const found = (await waitFor(async () => events.find(event => event.type === 'changes-updated' && event.changes.sessionId === sessionId && matches(event.changes)), Boolean, 10000)) as Extract<ServerMessage, {type: 'changes-updated'}>;
			return found.changes;
		};
		const shape = (changes: ChangesRecord) => changes.entries.map(entry => `${entry.group}:${entry.status}:${entry.path}`);
		const first = await create('changes-a', 'claude', 'new');
		const firstState = await waitFor(() => state(first.id), item => item.status === 'running');
		const second = await call<SessionRecord>({type: 'create', input: {title: 'changes-b', program: 'pi', cwd: root, repoRoot: root, cols: 80, rows: 24, worktreeMode: 'existing', existingWorktreePath: firstState.cwd}} as any);
		await waitFor(() => state(second.id), item => item.status === 'running');
		const key = workspaceKey(firstState)!;

		// The second session's viewer watches; changes made in the worktree arrive by polling.
		writeMessage(socket, {type: 'watch-changes', requestId: randomUUID(), sessionId: second.id});
		const initial = await call<ChangesRecord>({type: 'watch-changes', sessionId: first.id} as any);
		assert.equal(initial.workspace, key); assert.equal(initial.loaded, true); assert.deepEqual(initial.entries, []);
		await fs.writeFile(path.join(key, 'file.txt'), 'first\nchanged\n'); await fs.writeFile(path.join(key, 'new file.txt'), 'new\n');
		const polled = await pushed(second.id, changes => changes.counts.untracked === 1);
		assert.equal(polled.workspace, key); assert.deepEqual(shape(polled), ['unstaged:M:file.txt', 'untracked:?:new file.txt']);
		assert.equal(polled.entries[0]!.additions, 1);

		// One session stages; the other session's viewer receives the update at once.
		events.length = 0;
		const staged = await call<{changes: ChangesRecord}>({type: 'change-stage', sessionId: first.id, mode: 'stage', group: 'unstaged', path: 'file.txt'} as any);
		assert.deepEqual(shape(staged.changes), ['staged:M:file.txt', 'untracked:?:new file.txt']); assert.equal(staged.changes.sessionId, first.id);
		await pushed(second.id, changes => changes.counts.staged === 1);
		const diff = await call<ChangeDiff>({type: 'changes-diff', sessionId: second.id, group: 'staged', path: 'file.txt'} as any);
		assert.match(diff.text, /^\+changed$/m); assert.equal(diff.firstLine, 2);
		// Paths that are not listed (or listed on the other side) are refused, and so are sessions without a workspace.
		await assert.rejects(call({type: 'change-stage', sessionId: first.id, mode: 'stage', group: 'untracked', path: '../outside.txt'} as any), /not among the unstaged changes/);
		await assert.rejects(call({type: 'change-stage', sessionId: first.id, mode: 'unstage', group: 'untracked', path: 'new file.txt'} as any), /not among the staged changes/);
		await assert.rejects(call({type: 'changes-diff', sessionId: first.id, group: 'unstaged', path: 'nope.txt'} as any), /not in the current changes/);
		assert.deepEqual(await git(key, 'diff', '--cached', '--name-only'), 'file.txt');
		// Stage all, unstage all.
		assert.deepEqual(shape((await call<{changes: ChangesRecord}>({type: 'change-stage', sessionId: second.id, mode: 'stage'} as any)).changes), ['staged:M:file.txt', 'staged:A:new file.txt']);
		assert.deepEqual(shape((await call<{changes: ChangesRecord}>({type: 'change-stage', sessionId: first.id, mode: 'unstage'} as any)).changes), ['unstaged:M:file.txt', 'untracked:?:new file.txt']);
		await pushed(second.id, changes => changes.counts.staged === 0 && changes.counts.untracked === 1);

		// A session in a repository without commits: unstaging removes the path from the index.
		const unborn = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-unborn-'));
		t.after(() => fs.rm(unborn, {recursive: true, force: true}));
		await git(unborn, 'init', '-b', 'main'); await fs.writeFile(path.join(unborn, 'a.txt'), 'a\n'); await git(unborn, 'add', 'a.txt');
		const fresh = await call<SessionRecord>({type: 'create', input: {title: 'changes-unborn', program: 'claude', cwd: unborn, repoRoot: unborn, cols: 80, rows: 24}} as any);
		await waitFor(() => state(fresh.id), item => item.status === 'running');
		assert.deepEqual(shape(await call<ChangesRecord>({type: 'watch-changes', sessionId: fresh.id} as any)), ['staged:A:a.txt']);
		assert.deepEqual(shape((await call<{changes: ChangesRecord}>({type: 'change-stage', sessionId: fresh.id, mode: 'unstage', group: 'staged', path: 'a.txt'} as any)).changes), ['untracked:?:a.txt']);

		// Unwatching stops the pushes (and the polling).
		writeMessage(socket, {type: 'watch-changes', requestId: randomUUID()});
		await new Promise(resolve => setTimeout(resolve, 100));
		events.length = 0;
		await fs.writeFile(path.join(key, 'later.txt'), 'later\n');
		await new Promise(resolve => setTimeout(resolve, 2600));
		assert.ok(!events.some(event => event.type === 'changes-updated'));
		for (const session of [first, second, fresh]) await killAndWait(session.id);
	});

	await t.test('merge and deletion markers belong to the worktree: every session of it shares them, a new worktree at the path does not', async () => {
		const socket = net.createConnection(getSocketPath()); await once(socket, 'connect');
		const events: ServerMessage[] = []; attachJsonParser(socket, message => void events.push(message));
		t.after(() => socket.destroy());
		const subscribeId = randomUUID();
		writeMessage(socket, {type: 'subscribe', requestId: subscribeId, repoRoot: root});
		await waitFor(async () => events.some(event => event.type === 'response' && event.requestId === subscribeId), Boolean);
		const updated = (id: string, matches: (session: SessionRecord) => boolean) => waitFor(async () => events.some(event => event.type === 'session-updated' && event.session.id === id && matches(event.session)), Boolean);
		const merged = (id: string) => state(id).then(item => item.worktree?.mergedAt);
		// A new worktree, a session attached to it and a sub-session launched in it (no worktree of its own).
		const first = await create('records-a', 'claude', 'new');
		const firstState = await waitFor(() => state(first.id), item => item.status === 'running');
		const attached = await call<SessionRecord>({type: 'create', input: {title: 'records-b', program: 'pi', cwd: root, repoRoot: root, cols: 80, rows: 24, worktreeMode: 'existing', existingWorktreePath: firstState.cwd}} as any);
		const child = await call<SessionRecord>({type: 'create', input: {title: 'records-child', program: 'pi', cwd: firstState.cwd, repoRoot: root, cols: 80, rows: 24, parentSessionId: first.id, subSessionKind: 'clean'}} as any);
		const [attachedState, childState] = await Promise.all([attached, child].map(session => waitFor(() => state(session.id), item => item.status === 'running')));
		const worktreeId = firstState.worktree?.id;
		assert.ok(worktreeId); assert.equal(attachedState!.worktree?.id, worktreeId); assert.equal(childState!.worktree?.id, worktreeId);
		assert.equal(childState!.worktree?.mode, 'none');

		// m from one session marks the worktree: every session of it gets the marker, and every client hears about each.
		await fs.writeFile(path.join(firstState.cwd, 'records.txt'), 'records\n'); await git(firstState.cwd, 'add', '.'); await git(firstState.cwd, 'commit', '-m', 'records');
		events.length = 0;
		const result = await call<{skipped?: boolean; conflicted?: boolean}>({type: 'merge-worktree', sessionId: first.id, mode: 'squash', targetCwd: root} as any);
		assert.ok(!result.skipped && !result.conflicted);
		await git(root, 'commit', '-m', 'squashed records');
		for (const id of [attached.id, child.id]) await updated(id, session => Boolean(session.worktree?.mergedAt));
		const sibling = await state(attached.id);
		assert.equal(sibling.worktree?.mergeMode, 'squash'); assert.equal(sibling.worktree?.mergeTargetBranch, 'main'); assert.equal(sibling.worktree?.mergeSourceRef, 'records-a');
		assert.equal(await merged(child.id), sibling.worktree?.mergedAt);
		// M from another session toggles it for all of them.
		const unmarked = await call<SessionRecord>({type: 'mark-session-merged', sessionId: attached.id, targetCwd: root} as any);
		assert.equal(unmarked.worktree?.mergedAt, undefined);
		for (const id of [first.id, attached.id, child.id]) assert.equal(await merged(id), undefined);
		const marked = await call<SessionRecord>({type: 'mark-session-merged', sessionId: child.id, targetCwd: root} as any);
		assert.equal(marked.worktree?.mergeMarkedManually, true);
		for (const id of [first.id, attached.id]) assert.equal(await merged(id), marked.worktree?.mergedAt);
		// The marker is stored once, in the worktree record.
		const stored = await loadState();
		assert.equal(stored.worktrees.find(record => record.id === worktreeId)?.mergedAt, marked.worktree?.mergedAt);
		assert.ok(stored.sessions.every(item => item.worktree?.mergedAt === undefined || !item.worktree.id));

		// Main-checkout sessions have no merge marker (M points at D); done is their own, per session.
		const mainOne = await create('records-main-1'), mainTwo = await create('records-main-2');
		const main = (await call<WorktreeInfo[]>({type: 'list-worktrees', cwd: root} as any)).find(item => item.isMain)!;
		const mainAttached = await call<SessionRecord>({type: 'create', input: {title: 'records-main-3', program: 'claude', cwd: root, repoRoot: root, cols: 80, rows: 24, worktreeMode: 'existing', existingWorktreePath: main.path}} as any);
		for (const session of [mainOne, mainTwo, mainAttached]) await waitFor(() => state(session.id), item => item.status === 'running');
		for (const session of [mainOne, mainAttached]) await assert.rejects(call({type: 'mark-session-merged', sessionId: session.id, targetCwd: root} as any), /Use Space to mark it done/);
		const mainDone = await call<SessionRecord>({type: 'set-session-done', sessionId: mainOne.id, done: true} as any);
		assert.ok(mainDone.doneAt); assert.equal(mainDone.mergedAt, undefined); assert.equal(mainDone.worktree?.id, undefined);
		const mainOther = await state(mainTwo.id);
		assert.equal(mainOther.doneAt, undefined); assert.equal(mainOther.mergedAt, undefined); assert.equal((await state(mainAttached.id)).worktree?.id, undefined);
		for (const session of [mainOne, mainTwo, mainAttached]) { await killAndWait(session.id); await call({type: 'remove', sessionId: session.id} as any); }

		// Kill-with-delete marks the worktree deleted: every session of it loses its workspace, restart and merge.
		await killAndWait(attached.id); await killAndWait(child.id);
		events.length = 0;
		await call({type: 'kill', sessionId: first.id, deleteWorktree: true, allowDataLoss: true} as any);
		const deletedAt = (await waitFor(() => state(first.id), item => Boolean(item.worktree?.deletedAt))).worktree!.deletedAt;
		for (const id of [attached.id, child.id]) await updated(id, session => session.worktree?.deletedAt === deletedAt);
		for (const id of [attached.id, child.id]) {
			const item = await state(id);
			assert.equal(workspaceKey(item), undefined);
			await assert.rejects(call({type: 'restart', sessionId: id, cols: 80, rows: 24} as any), /worktree was deleted/);
			const terminal = await call<TerminalRecord>({type: 'watch-terminal', sessionId: id, cols: 80, rows: 24} as any);
			assert.equal(terminal.workspace, undefined); assert.equal(workspacePaneUnavailable(item, terminal), 'its worktree was deleted');
			const changes = await call<ChangesRecord>({type: 'watch-changes', sessionId: id} as any);
			assert.equal(workspacePaneUnavailable(item, changes), 'its worktree was deleted');
		}
		await assert.rejects(call({type: 'merge-worktree', sessionId: attached.id, mode: 'merge', targetCwd: root} as any), /worktree was deleted/);
		// M can still clear a deleted worktree's marker (for all of its sessions), but not set one.
		assert.equal((await call<SessionRecord>({type: 'mark-session-merged', sessionId: attached.id, targetCwd: root} as any)).worktree?.mergedAt, undefined);
		assert.equal(await merged(child.id), undefined);
		await assert.rejects(call({type: 'mark-session-merged', sessionId: child.id, targetCwd: root} as any), /worktree was deleted/);

		// A new worktree created at the same path is a new incarnation: no inherited markers, and the old sessions stay out.
		const again = await create('records-a', 'claude', 'new');
		const againState = await waitFor(() => state(again.id), item => item.status === 'running');
		assert.equal(againState.cwd, firstState.cwd);
		assert.ok(againState.worktree?.id); assert.notEqual(againState.worktree?.id, worktreeId);
		assert.equal(againState.worktree?.mergedAt, undefined); assert.equal(againState.worktree?.deletedAt, undefined);
		assert.equal(workspaceKey(againState), workspaceKey(firstState));
		for (const id of [first.id, attached.id, child.id]) {
			const item = await state(id);
			assert.equal(item.worktree?.deletedAt, deletedAt); assert.equal(workspaceKey(item), undefined);
			assert.equal((await call<TerminalRecord>({type: 'watch-terminal', sessionId: id, cols: 80, rows: 24} as any)).workspace, undefined);
		}

		// The record lives as long as a session references it.
		await call({type: 'remove', sessionId: first.id} as any); await call({type: 'remove', sessionId: attached.id} as any);
		assert.ok((await loadState()).worktrees.some(record => record.id === worktreeId));
		await call({type: 'remove', sessionId: child.id} as any);
		const remaining = (await loadState()).worktrees;
		assert.ok(!remaining.some(record => record.id === worktreeId)); assert.ok(remaining.some(record => record.id === againState.worktree?.id));

		// A worktree removed outside Deckhand: when Deckhand creates one at that path again, the old incarnation is deleted.
		await killAndWait(again.id);
		await git(root, 'worktree', 'remove', '--force', againState.cwd);
		const third = await create('records-a', 'claude', 'new');
		const thirdState = await waitFor(() => state(third.id), item => item.status === 'running');
		assert.equal(thirdState.cwd, againState.cwd); assert.notEqual(thirdState.worktree?.id, againState.worktree?.id);
		const superseded = await state(again.id);
		assert.ok(superseded.worktree?.deletedAt); assert.equal(workspaceKey(superseded), undefined); assert.equal(workspaceKey(thirdState), workspaceKey(againState));
		await killAndWait(third.id);
	});
});
