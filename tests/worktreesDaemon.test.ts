import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import net from 'node:net';
import {once} from 'node:events';
import {spawn} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import {attachJsonParser, request, writeMessage} from '../src/client.js';
import {getSocketPath} from '../src/paths.js';
import type {ClientRequest, ServerMessage, SessionRecord, WorktreeCleanupInspection, WorktreeDeleteResult, WorktreeOverview} from '../src/types.js';
import {cli, repo, git, waitFor, withEnv, fakeAgent, stop} from './helpers.js';

// W, the worktree manager, through a real daemon (its own daemon and state directory): the overview of a repository's
// worktrees (Deckhand's, one made with git, one whose directory is gone, one removed outside Deckhand), and deleting
// them: the safety checks of x on a session, plus stopping the sessions still running in it first.
test('worktree overview and deletion: merged detection, refusals, stopping sessions, data-loss override, prune and forget', {timeout: 150000}, async t => {
	const root = await fs.realpath(await repo());
	const home = await fs.realpath(await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-worktrees-')));
	const bin = path.join(home, 'bin'); await fs.mkdir(bin);
	for (const provider of ['claude', 'pi', 'codex']) await fs.writeFile(path.join(bin, provider), fakeAgent, {mode: 0o755});
	withEnv(t, {DECKHAND_HOME: home});
	const env = {...process.env, HOME: home, DECKHAND_HOME: home, DECKHAND_DEV: '0', DECKHAND_AGENT_LATEST: '{}', PATH: `${bin}${path.delimiter}${process.env.PATH}`, SHELL: '/bin/sh', TEST_CLI: cli, GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1'};
	const daemon = spawn(process.execPath, [cli, '--daemon'], {cwd: root, env, stdio: ['ignore', 'ignore', 'pipe']}); daemon.stderr?.on('data', () => {});
	t.after(async () => { await stop(daemon); await fs.rm(root, {recursive: true, force: true}); await fs.rm(home, {recursive: true, force: true}); });
	const call = <T>(message: Omit<Extract<ClientRequest, {requestId: string}>, 'requestId'>) => request<T>({...message, requestId: randomUUID()} as Extract<ClientRequest, {requestId: string}>, 60_000);
	await waitFor(async () => { try { return await call<{ok: boolean}>({type: 'ping'}); } catch { return {ok: false}; } }, result => result.ok);
	const state = (id: string) => call<SessionRecord[]>({type: 'list'}).then(items => items.find(item => item.id === id)!);
	const create = (title: string) => call<SessionRecord>({type: 'create', input: {title, program: 'claude', cwd: root, repoRoot: root, cols: 80, rows: 24, worktreeMode: 'new'}} as any);
	const running = (id: string) => waitFor(() => state(id), item => item.status === 'running' && Boolean(item.worktree?.path), 20_000);
	const overview = () => call<WorktreeOverview>({type: 'worktree-overview', cwd: root});
	const entryAt = (view: WorktreeOverview, where: string) => view.entries.find(entry => entry.path === where);
	const inspect = (where: string, deleteBranch = false, cwd = root) => call<WorktreeCleanupInspection>({type: 'inspect-worktree', cwd, path: where, deleteBranch});
	const remove = (where: string, options: {branch?: string; deleteBranch?: boolean; stopSessions?: boolean; allowDataLoss?: boolean} = {}) => call<WorktreeDeleteResult>({type: 'delete-worktree', cwd: root, path: where, ...options});
	const branches = async () => (await git(root, 'branch', '--format=%(refname:short)')).split('\n');

	// A Deckhand worktree whose branch got a commit and was then merged into main outside Deckhand; its agent still runs.
	const shipped = await create('shipped feature');
	const shippedWorktree = (await running(shipped.id)).worktree!;
	const shippedPath = shippedWorktree.path!, shippedBranch = shippedWorktree.branch!;
	await fs.writeFile(path.join(shippedPath, 'feature.txt'), 'feature\n');
	await git(shippedPath, 'add', '.'); await git(shippedPath, 'commit', '-m', 'feature');
	await git(root, 'merge', '--no-ff', '-m', 'merge feature', shippedBranch);
	// A worktree made with git (no Deckhand record), with a file only it has.
	const orphan = path.join(home, 'orphan');
	await git(root, 'worktree', 'add', '-b', 'orphan-work', orphan);
	await fs.writeFile(path.join(orphan, 'notes.txt'), 'keep me\n');

	let view = await overview();
	assert.equal(view.defaultBranch, 'main');
	const main = view.entries.find(entry => entry.isMain)!;
	assert.equal(main.path, root); assert.equal(main.inUse, 'this'); assert.equal(main.inspection, undefined);
	// Found merged (the overview runs merge detection first), clean, its session listed as running.
	const shippedEntry = entryAt(view, shippedPath)!;
	assert.equal(shippedEntry.markers?.mergeDetected, 'ancestor');
	assert.equal(shippedEntry.inspection?.safe, true);
	assert.equal(shippedEntry.aheadOfDefault, 0);
	assert.ok(shippedEntry.recordId);
	assert.deepEqual(shippedEntry.sessionIds, [shipped.id]); assert.deepEqual(shippedEntry.runningIds, [shipped.id]);
	assert.ok(shippedEntry.lastCommitAt);
	// Not Deckhand's: no record, no sessions; its untracked file is what deleting it would lose.
	const orphanEntry = entryAt(view, orphan)!;
	assert.equal(orphanEntry.recordId, undefined); assert.deepEqual(orphanEntry.sessionIds, []);
	assert.equal(orphanEntry.inspection?.safe, false); assert.equal(orphanEntry.inspection?.untrackedFiles, 1);

	// Structural refusals: the main checkout, the worktree this Deckhand runs in, one another Deckhand has open,
	// a protected branch, a branch that changed since it was listed.
	await assert.rejects(remove(root), /main checkout/);
	assert.match((await inspect(orphan, false, orphan)).structuralBlockers.join(), /runs in that worktree/);
	const other = net.createConnection(getSocketPath()); await once(other, 'connect');
	const events: ServerMessage[] = []; attachJsonParser(other, message => void events.push(message));
	const subscribed = randomUUID(); writeMessage(other, {type: 'subscribe', requestId: subscribed, repoRoot: orphan});
	await waitFor(async () => events.some(event => event.type === 'response' && event.requestId === subscribed), Boolean);
	assert.match((await inspect(orphan)).structuralBlockers.join(), /another Deckhand is open/);
	other.destroy();
	await waitFor(() => inspect(orphan), check => check.structuralBlockers.length === 0);
	await assert.rejects(remove(shippedPath, {branch: 'renamed', deleteBranch: true, stopSessions: true}), /branch changed/);

	// Its session runs: refused unless the deletion may stop it, and nothing was touched.
	const check = await inspect(shippedPath, true);
	assert.equal(check.safe, true); assert.deepEqual(check.running, [{id: shipped.id, title: 'shipped feature'}]);
	await assert.rejects(remove(shippedPath, {branch: shippedBranch, deleteBranch: true}), /still running in it/);
	assert.equal((await state(shipped.id)).status, 'running');
	// Stopped, removed with its branch, the record marked deleted and the session archived.
	assert.deepEqual(await remove(shippedPath, {branch: shippedBranch, deleteBranch: true, stopSessions: true}), {removed: 'deleted', branchDeleted: shippedBranch, stopped: 1, archived: 1});
	const after = await state(shipped.id);
	assert.equal(after.status, 'exited'); assert.equal(after.exitReason, 'stopped');
	assert.ok(after.worktree?.deletedAt); assert.ok(after.archivedAt);
	await assert.rejects(fs.access(shippedPath));
	assert.ok(!(await branches()).includes(shippedBranch));

	// Data loss needs the override (typed DELETE in the UI); with it, the worktree and its branch go.
	await assert.rejects(remove(orphan, {branch: 'orphan-work', deleteBranch: true}), /Deletion blocked: 1 untracked file/);
	assert.equal(await fs.readFile(path.join(orphan, 'notes.txt'), 'utf8'), 'keep me\n');
	assert.deepEqual(await remove(orphan, {branch: 'orphan-work', deleteBranch: true, allowDataLoss: true}), {removed: 'deleted', branchDeleted: 'orphan-work', stopped: 0, archived: 0});
	await assert.rejects(fs.access(orphan));

	// A worktree whose directory was deleted by hand: listed as missing, pruned (its branch stays).
	const gone = path.join(home, 'gone');
	await git(root, 'worktree', 'add', '-b', 'gone-work', gone);
	await fs.rm(gone, {recursive: true, force: true});
	view = await overview();
	assert.equal(entryAt(view, gone)?.missing, 'prunable');
	assert.deepEqual(await remove(gone), {removed: 'pruned', stopped: 0, archived: 0});
	assert.ok(!(await git(root, 'worktree', 'list', '--porcelain')).includes(gone));
	assert.ok((await branches()).includes('gone-work'));

	// A Deckhand worktree removed outside Deckhand: only its record is left, and forgetting it marks it deleted.
	const outside = await create('removed outside');
	const outsidePath = (await running(outside.id)).worktree!.path!;
	await call({type: 'kill', sessionId: outside.id} as any);
	await waitFor(() => state(outside.id), item => item.status === 'exited');
	await git(root, 'worktree', 'remove', '--force', outsidePath);
	view = await overview();
	const unlisted = entryAt(view, outsidePath)!;
	assert.equal(unlisted.missing, 'unlisted'); assert.deepEqual(unlisted.sessionIds, [outside.id]);
	assert.deepEqual(await remove(outsidePath), {removed: 'forgotten', stopped: 0, archived: 1});
	assert.ok((await state(outside.id)).worktree?.deletedAt);
	// Deleted and forgotten worktrees are no longer listed; a path that is not a worktree of the repository is refused.
	view = await overview();
	assert.deepEqual(view.entries.map(entry => entry.path), [root]);
	await assert.rejects(remove(path.join(home, 'nowhere')), /not listed by Git/);
});
