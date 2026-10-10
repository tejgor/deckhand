import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import {spawn} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import {request} from '../src/client.js';
import {parseTasks} from '../src/tasks.js';
import type {ClientRequest, NoteSaveResult, SessionRecord, TasksDoc, WorktreeDeleteResult, WorktreeMergeResult} from '../src/types.js';
import {cli, repo, git, waitFor, withEnv, fakeAgent, stop} from './helpers.js';

// A note's open checklist items sent to Tasks at once, through a real daemon (its own daemon and state directory):
// `A` in the Notes view (send-open-items), and at the end of a worktree's work: a clean merge, deleting it from W,
// and x on a session with its worktree deleted. Each line becomes its ↗ link; merged or deleted work's items land in
// the backlog marked as left open in its branch.
test('open note items go to Tasks at once: A, a merge, deletion from W and x with deletion', {timeout: 150000}, async t => {
	const root = await fs.realpath(await repo());
	const home = await fs.realpath(await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-items-')));
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
	const running = (id: string) => waitFor(() => state(id), item => item.status === 'running' && Boolean(item.sharedNotes), 20_000);
	const tasks = async () => parseTasks((await call<TasksDoc>({type: 'watch-tasks', cwd: root})).text);
	// Writes the worktree's note through the daemon, as the Notes tab does.
	const writeNote = async (session: SessionRecord, text: string) => {
		const current = await state(session.id);
		const saved = await call<NoteSaveResult>({type: 'save-note', sessionId: session.id, section: 'shared', noteId: `worktree:${current.worktree!.id}`, text, revision: current.sharedNotes!.revision});
		assert.equal(saved.saved, true);
		return saved.session;
	};
	const sharedText = async (id: string) => (await state(id)).sharedNotes?.text ?? '';

	// A in the Notes view: every open item of the note, each to the note's work (the worktree, as a follow-up).
	const one = await running((await create('items one')).id);
	const wt1 = one.worktree!.id!;
	let saved = await writeNote(one, '- [x] Done already\n- [ ] Parse tabs\n- [ ] Parse spaces\nprose');
	await assert.rejects(call({type: 'send-open-items', sessionId: one.id, section: 'shared', noteId: `worktree:${wt1}`, revision: 'stale'}), /changed meanwhile/);
	const sent = await call<{sent: number; session: SessionRecord}>({type: 'send-open-items', sessionId: one.id, section: 'shared', noteId: `worktree:${wt1}`, revision: saved.sharedNotes!.revision});
	assert.equal(sent.sent, 2);
	let list = await tasks();
	for (const title of ['Parse tabs', 'Parse spaces']) {
		const task = list.find(item => item.title === title)!;
		assert.equal(task.meta.wt, wt1); assert.ok(task.meta.assigned);
		assert.ok(sent.session.sharedNotes!.text.includes(`- ↗ ${title} <!-- dh:t=${task.id} -->`));
	}
	assert.match(sent.session.sharedNotes!.text, /^- \[x\] Done already\n/);

	// A clean merge with sendOpenItems: the items left in its note go to the backlog, marked as left open in its branch.
	saved = await writeNote(one, `${sent.session.sharedNotes!.text}\n- [ ] Benchmark it`);
	await fs.writeFile(path.join(one.cwd, 'feature.txt'), 'feature\n');
	await git(one.cwd, 'add', '.'); await git(one.cwd, 'commit', '-m', 'feature');
	const merged = await call<WorktreeMergeResult>({type: 'merge-worktree', sessionId: one.id, mode: 'merge', targetCwd: root, sendOpenItems: true});
	assert.equal(merged.sentItems, 1);
	list = await tasks();
	const benchmark = list.find(item => item.title === 'Benchmark it')!;
	assert.equal(benchmark.meta.wt, undefined); assert.equal(benchmark.meta.from, one.worktree!.branch);
	assert.match(await sharedText(one.id), /- ↗ Benchmark it <!-- dh:t=/);

	// Deleting from W with sendOpenItems: before the worktree goes (its running session is stopped too).
	const two = await running((await create('items two')).id);
	await writeNote(two, '- [ ] Write the docs');
	const deleted = await call<WorktreeDeleteResult>({type: 'delete-worktree', cwd: root, path: two.worktree!.path!, branch: two.worktree!.branch, stopSessions: true, sendOpenItems: true});
	assert.equal(deleted.removed, 'deleted'); assert.equal(deleted.sentItems, 1);
	const docs = (await tasks()).find(item => item.title === 'Write the docs')!;
	assert.equal(docs.meta.from, two.worktree!.branch); assert.equal(docs.done, false);
	// Without it, the items stay in the (now read-only) note.
	const three = await running((await create('items three')).id);
	await writeNote(three, '- [ ] Stays put');
	assert.equal((await call<WorktreeDeleteResult>({type: 'delete-worktree', cwd: root, path: three.worktree!.path!, branch: three.worktree!.branch, stopSessions: true})).sentItems, undefined);
	assert.ok(!(await tasks()).some(item => item.title === 'Stays put'));
	assert.equal(await sharedText(three.id), '- [ ] Stays put');

	// x with the worktree deleted (kill sendOpenItems): sent before the worktree is removed.
	const four = await running((await create('items four')).id);
	await writeNote(four, '- [ ] Clean up flags');
	await call({type: 'kill', sessionId: four.id, deleteWorktree: true, sendOpenItems: true} as any);
	await waitFor(() => state(four.id), item => item.status === 'exited' && Boolean(item.worktree?.deletedAt), 20_000);
	const flags = (await tasks()).find(item => item.title === 'Clean up flags')!;
	assert.equal(flags.meta.from, four.worktree!.branch);
	assert.match(await sharedText(four.id), /- ↗ Clean up flags <!-- dh:t=/);
});
