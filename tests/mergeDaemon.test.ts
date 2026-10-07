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
import type {ClientRequest, MergePreview, ServerMessage, SessionCleanupInspection, SessionRecord, WorkspaceSummary, WorktreeMergeResult} from '../src/types.js';
import {cli, exec, repo, git, waitFor, withEnv, fakeAgent, stop} from './helpers.js';
import {loadState} from '../src/storage.js';

// A fake `gh`: answers `gh pr view` from gh-pr.json in the state directory (or fails like an unknown branch), never the network.
const fakeGh = `#!/usr/bin/env node
const fs = require('fs'), path = require('path');
try { process.stdout.write(fs.readFileSync(path.join(process.env.DECKHAND_HOME, 'gh-pr.json'), 'utf8')); }
catch { console.error('no pull requests found for branch'); process.exit(1); }
`;

// D (done), M's worktree-only rule, and m's commit-first / conflict / detection paths through a real daemon (its own
// daemon and state directory, so it runs beside the others).
test('done marker, worktree-only M, merge with commit-first, conflict keep/abort and merge detection over IPC', {timeout: 150000}, async t => {
	const root = await fs.realpath(await repo());
	const home = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-merge-'));
	const bin = path.join(home, 'bin'); await fs.mkdir(bin);
	for (const provider of ['claude', 'pi', 'codex']) await fs.writeFile(path.join(bin, provider), fakeAgent, {mode: 0o755});
	await fs.writeFile(path.join(bin, 'gh'), fakeGh, {mode: 0o755});
	withEnv(t, {DECKHAND_HOME: home});
	const env = {...process.env, HOME: home, DECKHAND_HOME: home, DECKHAND_DEV: '0', DECKHAND_AGENT_LATEST: '{}', PATH: `${bin}${path.delimiter}${process.env.PATH}`, SHELL: '/bin/sh', TEST_CLI: cli, GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1'};
	const daemon = spawn(process.execPath, [cli, '--daemon'], {cwd: root, env, stdio: ['ignore', 'ignore', 'pipe']}); daemon.stderr?.on('data', () => {});
	t.after(async () => { await stop(daemon); await fs.rm(root, {recursive: true, force: true}); await fs.rm(home, {recursive: true, force: true}); });
	const call = <T>(message: Omit<Extract<ClientRequest, {requestId: string}>, 'requestId'>) => request<T>({...message, requestId: randomUUID()} as Extract<ClientRequest, {requestId: string}>);
	await waitFor(async () => { try { return await call<{ok: boolean}>({type: 'ping'}); } catch { return {ok: false}; } }, result => result.ok);
	const state = (id: string) => call<SessionRecord[]>({type: 'list'}).then(items => items.find(item => item.id === id)!);
	const create = (title: string, worktreeMode: 'none' | 'new' = 'none', extra: Record<string, unknown> = {}) => call<SessionRecord>({type: 'create', input: {title, program: 'claude', cwd: root, repoRoot: root, cols: 80, rows: 24, worktreeMode, ...extra}} as any);
	const running = (id: string) => waitFor(() => state(id), item => item.status === 'running');
	const status = async (dir: string) => (await exec('git', ['-C', dir, 'status', '--porcelain', '--untracked-files=all'], {env})).stdout.trimEnd();
	const socket = net.createConnection(getSocketPath()); await once(socket, 'connect');
	const events: ServerMessage[] = []; attachJsonParser(socket, message => void events.push(message));
	t.after(() => socket.destroy());
	const subscribeId = randomUUID();
	writeMessage(socket, {type: 'subscribe', requestId: subscribeId, repoRoot: root});
	await waitFor(async () => events.some(event => event.type === 'response' && event.requestId === subscribeId), Boolean);
	const updated = (id: string, matches: (session: SessionRecord) => boolean) => waitFor(async () => events.some(event => event.type === 'session-updated' && event.session.id === id && matches(event.session)), Boolean);

	// D: a main-checkout session is marked done (persisted, broadcast) and back; M there refuses and points at D.
	const mainSession = await create('tidy readme'); await running(mainSession.id);
	const done = await call<SessionRecord>({type: 'set-session-done', sessionId: mainSession.id, done: true} as any);
	assert.ok(done.doneAt);
	await updated(mainSession.id, session => session.doneAt === done.doneAt);
	assert.equal((await loadState()).sessions.find(item => item.id === mainSession.id)?.doneAt, done.doneAt);
	// Marking it done again keeps the first time.
	assert.equal((await call<SessionRecord>({type: 'set-session-done', sessionId: mainSession.id, done: true} as any)).doneAt, done.doneAt);
	await assert.rejects(call({type: 'mark-session-merged', sessionId: mainSession.id, targetCwd: root} as any), /Use D to mark it done/);
	assert.equal((await state(mainSession.id)).mergedAt, undefined);
	assert.equal((await call<SessionRecord>({type: 'set-session-done', sessionId: mainSession.id, done: false} as any)).doneAt, undefined);
	await updated(mainSession.id, session => !session.doneAt);
	assert.equal((await loadState()).sessions.find(item => item.id === mainSession.id)?.doneAt, undefined);

	// m with commit-first: a worktree with only uncommitted work, merged through the toggle; done is independent of merged.
	const feature = await create('commit first', 'new');
	const featureState = await running(feature.id);
	const wt = featureState.cwd;
	const child = await create('commit first child', 'none', {cwd: wt, parentSessionId: feature.id, subSessionKind: 'clean'});
	await running(child.id);
	assert.ok((await call<SessionRecord>({type: 'set-session-done', sessionId: child.id, done: true} as any)).doneAt);
	assert.equal((await state(feature.id)).doneAt, undefined);
	await fs.writeFile(path.join(wt, 'feature.txt'), 'feature\n');
	const preview = await call<MergePreview>({type: 'merge-preview', sessionId: feature.id, targetCwd: root} as any);
	assert.deepEqual([preview.commitCount, preview.uncommitted, preview.targetBranch, preview.targetIsMain, preview.targetRoot], [0, 1, 'main', true, root]);
	const skipped = await call<WorktreeMergeResult>({type: 'merge-worktree', sessionId: feature.id, mode: 'squash', targetCwd: root, commitFirst: false} as any);
	assert.equal(skipped.skipped, true); assert.match(await status(wt), /feature\.txt/);
	const merged = await call<WorktreeMergeResult>({type: 'merge-worktree', sessionId: feature.id, mode: 'squash', targetCwd: root, commitFirst: true} as any);
	assert.ok(!merged.skipped && !merged.conflicted); assert.equal(merged.committed?.files, 1);
	assert.equal(await git(wt, 'log', '-1', '--format=%s'), 'commit first');
	const sourceSha = await git(wt, 'rev-parse', 'HEAD');
	assert.equal(merged.sourceSha, sourceSha);
	await updated(child.id, session => session.worktree?.mergeSourceSha === sourceSha && session.worktree.mergeMode === 'squash');
	assert.ok((await state(child.id)).doneAt); // Still done, and now merged.
	assert.equal((await loadState()).worktrees.find(record => record.id === featureState.worktree?.id)?.mergeSourceSha, sourceSha);
	await git(root, 'commit', '-m', 'squashed commit first');
	// After that squash, deleting the branch no longer asks for DELETE for its commits.
	const inspection = await call<SessionCleanupInspection>({type: 'inspect-cleanup', sessionId: feature.id, deleteBranch: true} as any);
	assert.ok(!inspection.reasons.some(reason => /commit\(s\) exist only/.test(reason)), inspection.reasons.join());

	// Conflicts: abort (normal merge), then keep (squash): marked merged now, the merge left in progress.
	const conflicting = await create('conflicting', 'new');
	const conflictingState = await running(conflicting.id);
	await fs.writeFile(path.join(conflictingState.cwd, 'file.txt'), 'feature side\n'); await git(conflictingState.cwd, 'commit', '-am', 'feature side');
	await fs.writeFile(path.join(root, 'file.txt'), 'main side\n'); await git(root, 'commit', '-am', 'main side');
	await fs.writeFile(path.join(root, 'scratch.txt'), 'unrelated\n');
	const normal = await call<WorktreeMergeResult>({type: 'merge-worktree', sessionId: conflicting.id, mode: 'merge', targetCwd: root} as any);
	assert.deepEqual([normal.conflicted, normal.conflicts, normal.conflictCount], [true, ['file.txt'], 1]);
	assert.equal((await state(conflicting.id)).worktree?.mergedAt, undefined);
	assert.equal((await call<MergePreview>({type: 'merge-preview', sessionId: conflicting.id, targetCwd: root} as any)).inProgress, 'merge');
	await assert.rejects(call({type: 'merge-worktree', sessionId: conflicting.id, mode: 'merge', targetCwd: root} as any), /A merge is in progress in the target/);
	await call<SessionRecord>({type: 'resolve-merge', sessionId: conflicting.id, targetCwd: root, action: 'abort'} as any);
	assert.equal(await status(root), '?? scratch.txt');
	assert.equal((await state(conflicting.id)).worktree?.mergedAt, undefined);
	const squash = await call<WorktreeMergeResult>({type: 'merge-worktree', sessionId: conflicting.id, mode: 'squash', targetCwd: root} as any);
	assert.equal(squash.conflicted, true);
	const kept = await call<SessionRecord>({type: 'resolve-merge', sessionId: conflicting.id, targetCwd: root, action: 'keep'} as any);
	assert.ok(kept.worktree?.mergedAt); assert.equal(kept.worktree?.mergeMode, 'squash'); assert.equal(kept.worktree?.mergeSourceSha, await git(conflictingState.cwd, 'rev-parse', 'HEAD'));
	assert.match(await status(root), /^UU file\.txt$/m);
	assert.equal((await call<MergePreview>({type: 'merge-preview', sessionId: feature.id, targetCwd: root} as any)).inProgress, 'conflicts');
	// The user resolves it later; here the squash is undone (a squash Deckhand started on a clean index).
	await assert.rejects(call({type: 'merge-worktree', sessionId: conflicting.id, mode: 'squash', targetCwd: root} as any), /The target has unresolved conflicts/);
	await git(root, 'reset', '--merge');
	// A squash abort Deckhand cannot vouch for is refused (it did not start this one).
	await git(root, 'merge', '--squash', conflictingState.worktree!.branch!).catch(() => {});
	await assert.rejects(call({type: 'resolve-merge', sessionId: feature.id, targetCwd: root, action: 'abort'} as any), /Cannot abort this squash merge safely/);
	await git(root, 'reset', '--merge');

	// Detection by ancestry: a branch merged into main outside Deckhand is marked; a fresh branch never.
	const outside = await create('merged outside', 'new');
	const outsideState = await running(outside.id);
	const fresh = await create('fresh branch', 'new');
	await running(fresh.id);
	await fs.writeFile(path.join(outsideState.cwd, 'outside.txt'), 'outside\n'); await git(outsideState.cwd, 'add', '.'); await git(outsideState.cwd, 'commit', '-m', 'outside');
	await git(root, 'merge', '--no-ff', '-m', 'merge outside', outsideState.worktree!.branch!);
	events.length = 0;
	await call<WorkspaceSummary>({type: 'workspace-summary', sessionId: outside.id} as any);
	await updated(outside.id, session => session.worktree?.mergeDetected === 'ancestor' && Boolean(session.worktree.mergedAt));
	const detected = await state(outside.id);
	assert.deepEqual([detected.worktree?.mergeTargetBranch, detected.worktree?.mergeSourceRef, detected.worktree?.mergeSourceSha], ['main', 'merged_outside', await git(outsideState.cwd, 'rev-parse', 'HEAD')]);
	await call<WorkspaceSummary>({type: 'workspace-summary', sessionId: fresh.id} as any);
	await new Promise(resolve => setTimeout(resolve, 400));
	assert.equal((await state(fresh.id)).worktree?.mergedAt, undefined);
	// M clears a detected marker, and it stays cleared until the branch moves on.
	await call({type: 'mark-session-merged', sessionId: outside.id, targetCwd: root} as any);
	await new Promise(resolve => setTimeout(resolve, 4200)); // Past the summary cache.
	await call<WorkspaceSummary>({type: 'workspace-summary', sessionId: outside.id} as any);
	await new Promise(resolve => setTimeout(resolve, 400));
	assert.equal((await state(outside.id)).worktree?.mergedAt, undefined);

	// Detection by PR: gh reports the PR MERGED (a GitHub squash merge: nothing local is merged).
	const pr = await create('squashed on github', 'new');
	const prState = await running(pr.id);
	await fs.writeFile(path.join(prState.cwd, 'pr.txt'), 'pr\n'); await git(prState.cwd, 'add', '.'); await git(prState.cwd, 'commit', '-m', 'pr work');
	const prHead = await git(prState.cwd, 'rev-parse', 'HEAD');
	await call<WorkspaceSummary>({type: 'workspace-summary', sessionId: pr.id} as any);
	await new Promise(resolve => setTimeout(resolve, 300));
	assert.equal((await state(pr.id)).worktree?.mergedAt, undefined);
	await fs.writeFile(path.join(home, 'gh-pr.json'), JSON.stringify({number: 12, url: 'https://github.com/acme/app/pull/12', state: 'MERGED', statusCheckRollup: [], headRefOid: prHead, baseRefName: 'main'}));
	const summary = await call<WorkspaceSummary>({type: 'workspace-summary', sessionId: pr.id, includePr: true} as any);
	assert.equal(summary.pr?.state, 'MERGED');
	await updated(pr.id, session => session.worktree?.mergeDetected === 'pr');
	assert.equal((await state(pr.id)).worktree?.mergeSourceSha, prHead);
	// The squash on GitHub left the commit "unmerged" locally; the recorded head makes it integrated for cleanup.
	const prInspection = await call<SessionCleanupInspection>({type: 'inspect-cleanup', sessionId: pr.id, deleteBranch: true} as any);
	assert.ok(!prInspection.reasons.some(reason => /commit\(s\) exist only/.test(reason)), prInspection.reasons.join());
	assert.equal((await loadState()).worktrees.find(record => record.id === prState.worktree?.id)?.mergeDetected, 'pr');
});
