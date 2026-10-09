import assert from 'node:assert/strict';
import {test} from 'node:test';
import {bulkTargets, deleteRefusal, mergedText, worktreeGroup, worktreeRows, worktreeTags} from '../src/worktreesModel.js';
import type {SessionRecord, WorktreeOverview, WorktreeOverviewEntry} from '../src/types.js';

const NOW = Date.parse('2026-10-09T12:00:00Z');
const DAY = 24 * 60 * 60_000;
const clean = {safe: true, reasons: [], dirtyFiles: 0, untrackedFiles: 0, ignoredFiles: 0};
const entry = (overrides: Partial<WorktreeOverviewEntry>): WorktreeOverviewEntry => ({
	path: `/w/${overrides.branch ?? 'x'}`, branch: 'x', head: 'a'.repeat(40), isMain: false, sessionIds: [], runningIds: [],
	lastCommitAt: new Date(NOW - DAY).toISOString(), inspection: clean, aheadOfDefault: 2, ...overrides,
});
const session = (id: string, overrides: Partial<SessionRecord> = {}): SessionRecord => ({
	id, title: id, program: 'claude', command: 'claude', args: [], cwd: '/w', repoRoot: '/r', status: 'exited',
	createdAt: new Date(NOW - 30 * DAY).toISOString(), updatedAt: new Date(NOW - 30 * DAY).toISOString(), ...overrides,
} as SessionRecord);

test('worktrees are grouped by what to do with them', () => {
	// Merged by Deckhand and clean: safe; with uncommitted files: leftovers.
	assert.equal(worktreeGroup(entry({markers: {mergedAt: 't', mergeMode: 'squash', mergeTargetBranch: 'main'}}), [], NOW), 'ready');
	assert.equal(worktreeGroup(entry({markers: {mergedAt: 't'}, inspection: {...clean, safe: false, dirtyFiles: 1, reasons: ['1 modified/staged file(s)']}}), [], NOW), 'leftovers');
	// Never checked (an error): not called safe.
	assert.equal(worktreeGroup(entry({markers: {mergedAt: 't'}, inspection: undefined, error: 'boom'}), [], NOW), 'leftovers');
	// Nothing beyond the default branch counts as merged (a worktree Deckhand does not track included).
	assert.equal(worktreeGroup(entry({aheadOfDefault: 0}), [], NOW), 'ready');
	// …but a fresh branch with uncommitted work is in progress, not merged.
	assert.equal(worktreeGroup(entry({aheadOfDefault: 0, inspection: {...clean, safe: false, untrackedFiles: 1, reasons: ['1 untracked file(s)']}}), [], NOW), 'progress');
	// Not merged: in progress, or idle once nothing ran for two weeks.
	assert.equal(worktreeGroup(entry({}), [], NOW), 'progress');
	const old = entry({lastCommitAt: new Date(NOW - 20 * DAY).toISOString(), sessionIds: ['s'], recordId: 'r', createdAt: new Date(NOW - 40 * DAY).toISOString()});
	assert.equal(worktreeGroup(old, [session('s')], NOW), 'idle');
	assert.equal(worktreeGroup({...old, runningIds: ['s']}, [session('s', {status: 'running'})], NOW), 'progress');
	// A session updated recently keeps it out of idle.
	assert.equal(worktreeGroup(old, [session('s', {updatedAt: new Date(NOW - DAY).toISOString()})], NOW), 'progress');
	assert.equal(worktreeGroup(entry({missing: 'prunable', markers: {mergedAt: 't'}}), [], NOW), 'missing');
	assert.equal(worktreeGroup(entry({isMain: true, aheadOfDefault: 0}), [], NOW), 'main');
});

test('merge labels say how it was merged', () => {
	assert.equal(mergedText(entry({markers: {mergedAt: 't', mergeDetected: 'pr'}})), 'PR merged');
	assert.equal(mergedText(entry({markers: {mergedAt: 't', mergeDetected: 'ancestor'}}), 'main'), 'merged into main');
	assert.equal(mergedText(entry({markers: {mergedAt: 't', mergeMarkedManually: true}})), 'marked merged (M)');
	assert.equal(mergedText(entry({markers: {mergedAt: 't', mergeMode: 'squash', mergeTargetBranch: 'dev'}})), 'squash-merged into dev');
	assert.equal(mergedText(entry({aheadOfDefault: 0}), 'main'), 'nothing beyond main');
	assert.equal(mergedText(entry({aheadOfDefault: 0, inspection: undefined}), 'main'), undefined);
	assert.equal(mergedText(entry({})), undefined);
});

test('rows: groups in order, the main checkout last, x on the ready heading deletes what may be deleted', () => {
	const overview: WorktreeOverview = {defaultBranch: 'main', checkedAt: new Date(NOW).toISOString(), entries: [
		entry({isMain: true, branch: 'main', path: '/r', inUse: 'this'}),
		entry({branch: 'wip'}),
		entry({branch: 'done-a', aheadOfDefault: 0, lastCommitAt: new Date(NOW - 3 * DAY).toISOString()}),
		entry({branch: 'done-b', markers: {mergedAt: 't'}, inUse: 'other'}),
		entry({branch: 'done-c', markers: {mergedAt: 't'}}),
	]};
	const rows = worktreeRows(overview, [], NOW);
	assert.deepEqual(rows.map(row => row.kind === 'heading' ? `# ${row.group} ${row.count}` : row.kind === 'worktree' ? row.entry.branch : row.text),
		['# ready 3', 'done-b', 'done-c', 'done-a', '# progress 1', 'wip', '# main 1', 'main']);
	// Another Deckhand open in one: never deleted from here.
	assert.deepEqual(bulkTargets(rows).map(item => item.branch), ['done-c', 'done-a']);
	assert.match(deleteRefusal(overview.entries[3]!)!, /Another Deckhand/);
	assert.match(deleteRefusal(overview.entries[0]!)!, /main checkout/);
	assert.equal(deleteRefusal(overview.entries[1]!), undefined);
	// Only the main checkout: a hint how to make one.
	assert.equal(worktreeRows({...overview, entries: [overview.entries[0]!]}, [], NOW).at(-1)?.kind, 'empty');
});

test('tags: changes, commits, sessions and age', () => {
	const dirty = entry({inspection: {...clean, safe: false, dirtyFiles: 2, untrackedFiles: 1, ignoredFiles: 1}, sessionIds: ['a', 'b'], runningIds: ['a'], aheadOfDefault: 1});
	assert.deepEqual(worktreeTags(dirty, [], NOW), ['3 changed', '1 ignored', '1 commit', '1 running', '1d']);
	assert.deepEqual(worktreeTags(entry({aheadOfDefault: 0}), [], NOW), ['not from Deckhand', '1d']);
	assert.deepEqual(worktreeTags(entry({recordId: 'r', sessionIds: ['a'], aheadOfDefault: 0}), [], NOW), ['1 session', '1d']);
	assert.deepEqual(worktreeTags(entry({missing: 'unlisted', recordId: 'r', lastCommitAt: undefined, inspection: undefined, aheadOfDefault: undefined}), [], NOW), ['not listed by Git']);
});
