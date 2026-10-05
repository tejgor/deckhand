import assert from 'node:assert/strict';
import {test, type TestContext} from 'node:test';
import fs from 'node:fs/promises';
import path from 'node:path';
import {migrateWorktreeRecords, projectWorktree, storedSession} from '../src/worktreeRecords.js';
import {loadState, markAllNonExitedSessionsExited} from '../src/storage.js';
import {workspaceKey} from '../src/workspace.js';
import type {SessionRecord, WorktreeRecord} from '../src/types.js';
import {tempDir, withEnv} from './helpers.js';

const at = (minute: number) => new Date(Date.UTC(2026, 0, 1, 0, minute)).toISOString();
const session = (id: string, created: number, worktree: SessionRecord['worktree'], extra: Partial<SessionRecord> = {}): SessionRecord =>
	({id, title: id, program: 'claude', command: 'claude', cwd: worktree?.path ?? '/repo', repoRoot: '/repo', launchWorktreeRoot: '/repo', worktree, status: 'exited', createdAt: at(created), updatedAt: at(created), ...extra}) as SessionRecord;
const managed = (extra: Partial<NonNullable<SessionRecord['worktree']>> = {}) => ({mode: 'managed' as const, path: '/wt/feature', branch: 'feature', isMain: false, ...extra});

// A legacy state: two incarnations of /wt/feature (the first deleted by A at minute 30), a sub-session, a session restarted
// into the second incarnation, an unrelated worktree, and main-checkout sessions that keep their own markers.
function legacySessions(): SessionRecord[] {
	return [
		session('a', 1, managed({mergedAt: at(10), mergeMode: 'squash', mergeTargetBranch: 'main', mergeSourceRef: 'feature', deletedAt: at(30)})),
		session('b', 2, {...managed({mergedAt: at(20), mergeTargetBranch: 'main', mergeSourceRef: 'feature', mergeMarkedManually: true}), mode: 'attached'}),
		// A sub-session: no worktree of its own, launched in the linked worktree; its top-level M marker is lifted.
		session('child', 3, {mode: 'none'}, {cwd: '/wt/feature', launchWorktreeRoot: '/wt/feature', requestedWorktreeMode: 'none', mergedAt: at(5), mergeTargetBranch: 'main', mergeSourceRef: 'feature', mergeMarkedManually: true}),
		// Created before the deletion but last launched after it, in the new worktree at the same path.
		session('restarted', 4, {...managed(), mode: 'attached'}, {agentStartedAt: at(45)}),
		session('new', 40, managed()),
		session('other', 5, managed({path: '/wt/other', mergedAt: at(8), mergeMode: 'merge', mergeTargetBranch: 'main', mergeSourceRef: 'other'})),
		session('main-none', 6, {mode: 'none'}, {mergedAt: at(9), mergeTargetBranch: 'main', mergeSourceRef: 'main', mergeMarkedManually: true}),
		session('main-attached', 7, {mode: 'attached', path: '/repo', isMain: true, mergedAt: at(9), mergeTargetBranch: 'dev', mergeSourceRef: 'main', mergeMarkedManually: true}),
		session('waiting', 8, {mode: 'none'}, {launchWorktreeRoot: '/wt/feature', requestedWorktreeMode: 'new'}),
	];
}

test('migration lifts per-session worktree markers into one record per worktree incarnation', () => {
	let next = 0;
	const {sessions, worktrees, changed} = migrateWorktreeRecords(legacySessions(), [], () => `w${++next}`);
	assert.equal(changed, true);
	const byId = new Map(sessions.map(item => [item.id, item]));
	const recordOf = (id: string) => worktrees.find(record => record.id === byId.get(id)!.worktree?.id);
	// The deleted incarnation: the session that deleted it, its sibling and the sub-session, with the latest merge marker.
	const deleted = recordOf('a')!;
	assert.equal(recordOf('b'), deleted); assert.equal(recordOf('child'), deleted);
	assert.deepEqual({...deleted, id: undefined}, {id: undefined, path: '/wt/feature', createdAt: at(1), mergedAt: at(20), mergeTargetBranch: 'main', mergeSourceRef: 'feature', mergeMarkedManually: true, deletedAt: at(30)});
	// The live incarnation at the same path: no inherited markers.
	const live = recordOf('new')!;
	assert.notEqual(live.id, deleted.id); assert.equal(recordOf('restarted'), live);
	assert.deepEqual({...live, id: undefined}, {id: undefined, path: '/wt/feature', createdAt: at(4)});
	assert.deepEqual({...recordOf('other')!, id: undefined}, {id: undefined, path: '/wt/other', createdAt: at(5), mergedAt: at(8), mergeMode: 'merge', mergeTargetBranch: 'main', mergeSourceRef: 'other'});
	assert.equal(worktrees.length, 3);
	// Sessions keep no markers of their own once they reference a record; the sub-session's top-level marker moved.
	for (const id of ['a', 'b', 'child', 'restarted', 'new', 'other']) {
		const item = byId.get(id)!;
		assert.deepEqual(Object.keys(item.worktree!).filter(key => key.startsWith('merge') || key === 'deletedAt'), [], id);
		assert.equal(item.mergedAt, undefined, id);
	}
	// Main-checkout sessions and a session still waiting for its worktree are untouched.
	const legacy = new Map(legacySessions().map(item => [item.id, item]));
	for (const id of ['main-none', 'main-attached', 'waiting']) assert.deepEqual(byId.get(id), legacy.get(id), id);

	// Projected, every session of an incarnation shows its markers, and the deleted one has no workspace.
	const records = new Map(worktrees.map(record => [record.id, record]));
	const view = (id: string) => projectWorktree(byId.get(id)!, records);
	assert.equal(view('b').worktree?.mergedAt, at(20)); assert.equal(view('child').worktree?.deletedAt, at(30));
	assert.equal(workspaceKey(view('b')), undefined); assert.equal(workspaceKey(view('child')), undefined);
	assert.equal(workspaceKey(view('restarted')), '/wt/feature'); assert.equal(view('restarted').worktree?.mergedAt, undefined);
	assert.deepEqual(storedSession(view('b')), byId.get('b'));

	// Migrating the migrated state changes nothing.
	const again = migrateWorktreeRecords(sessions, worktrees, () => 'unused');
	assert.equal(again.changed, false); assert.deepEqual(again.sessions, sessions); assert.deepEqual(again.worktrees, worktrees);
});

test('migration repairs stored state: stray markers dropped, missing records rebuilt, unreferenced records removed', () => {
	const stray = session('s', 1, managed({id: 'kept', mergedAt: at(3)}));
	const orphan = session('o', 2, managed({id: 'lost', path: '/wt/lost', deletedAt: at(4)}));
	const records: WorktreeRecord[] = [{id: 'kept', path: '/wt/feature', createdAt: at(1)}, {id: 'unused', path: '/wt/gone', createdAt: at(0)}];
	const {sessions, worktrees, changed} = migrateWorktreeRecords([stray, orphan], records);
	assert.equal(changed, true);
	assert.equal(sessions[0]!.worktree?.mergedAt, undefined); // The record is the only copy: it has no marker.
	assert.deepEqual(worktrees.map(record => record.id).sort(), ['kept', 'lost']);
	assert.equal(worktrees.find(record => record.id === 'lost')?.deletedAt, at(4));
	// Legacy sessions at a path with a live record join it.
	const joined = migrateWorktreeRecords([...sessions, session('late', 9, {...managed(), mode: 'attached'})], worktrees);
	assert.equal(joined.sessions.at(-1)!.worktree?.id, 'kept');
});

async function isolatedHome(t: TestContext): Promise<string> {
	const home = await tempDir(t, 'deckhand-records-'); withEnv(t, {DECKHAND_HOME: home}); return home;
}

test('a legacy state.json is migrated on daemon start and written back with records only', async t => {
	const home = await isolatedHome(t), file = path.join(home, 'state.json');
	await fs.writeFile(file, JSON.stringify({sessions: legacySessions()}));
	const recovered = await markAllNonExitedSessionsExited();
	const written = JSON.parse(await fs.readFile(file, 'utf8')) as {sessions: SessionRecord[]; worktrees: WorktreeRecord[]};
	assert.deepEqual(written, JSON.parse(JSON.stringify(recovered)));
	assert.equal(written.worktrees.length, 3);
	assert.ok(written.sessions.every(item => !item.worktree?.id || (!item.worktree.mergedAt && !item.worktree.deletedAt)));
	assert.deepEqual(await loadState(), recovered);
});
