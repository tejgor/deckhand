import assert from 'node:assert/strict';
import {test} from 'node:test';
import React from 'react';
import {renderToString} from 'ink';
import {MergeConfirmPane, MergeConflictPane} from '../src/app.js';
import type {SessionRecord} from '../src/types.js';
import {conflictView, mergeConfirmLayout, mergeConfirmRows, overlappingFiles, surprisingTarget, type MergeConfirmInput} from '../src/mergeModel.js';
import type {MergePreview} from '../src/types.js';
import {THEME} from '../src/ui.js';
import {withEnv} from './helpers.js';

const preview = (fields: Partial<MergePreview> = {}): MergePreview => ({
	sourceRef: 'feat/auth', sourceSha: 'a'.repeat(40), targetRoot: '/home/me/code/app', targetBranch: 'main', targetIsMain: true, defaultBranch: 'main',
	commitCount: 8, commits: ['add login form', 'wire the session cookie', 'fix tests', 'rename handler', 'docs', 'lint'],
	diff: {files: 5, insertions: 120, deletions: 14}, uncommitted: 4, overlap: {committed: [], uncommitted: []}, ...fields,
});
const input = (fields: Partial<MergeConfirmInput> = {}): MergeConfirmInput => ({
	title: 'auth refactor', preview: preview(), commitFirst: true, commitMessage: 'auth refactor', notes: [], width: 60, height: 40, ...fields,
});
const texts = (lines: Array<{text: string}>) => lines.map(line => line.text);

test('merge confirmation: target line, summary with commits, the uncommitted toggle, options and hint', t => {
	withEnv(t, {HOME: '/home/me'});
	const layout = mergeConfirmLayout(input({notes: [{key: 'w', title: 'Worktree notes · feat/auth', lines: ['- [ ] review', 'ship it']}]}));
	assert.equal(layout.title, 'Merge "auth refactor"?');
	assert.deepEqual(texts(layout.details), [
		'Into main · ~/code/app',
		'8 commits · 5 files +120 −14 · +4 uncommitted files',
		'  add login form', '  wire the session cookie', '  fix tests', '  rename handler', '  docs', '  lint', '  +2 more',
		'4 uncommitted files',
		'☑ commit them first ("auth refactor")',
	]);
	// The expected target (main checkout, default branch) is muted; the toggle is highlighted while on.
	assert.equal(layout.details[0]!.color, THEME.muted);
	assert.equal(layout.details.at(-1)!.color, THEME.active);
	assert.deepEqual(layout.options, ['Merge into main without committing', 'Squash merge into main without committing', 'Cancel']);
	assert.equal(layout.hint, 'enter choose · space commit first · j/k move · esc cancel');
	assert.deepEqual(texts(layout.notes), ['Notes', 'Worktree notes · feat/auth', '  - [ ] review', '  ship it']);
	assert.ok(mergeConfirmRows(layout) <= 40);

	// Toggle off: the summary has no uncommitted part and the files stay in the worktree.
	const off = mergeConfirmLayout(input({commitFirst: false, width: 72}));
	assert.equal(off.details[1]!.text, '8 commits · 5 files +120 −14');
	assert.equal(off.details.at(-1)!.text, '☐ commit them first ("auth refactor") · they stay in the worktree');
	// Narrow: the message is cut, never what happens to the files.
	assert.equal(mergeConfirmLayout(input({commitFirst: false, width: 60})).details.at(-1)!.text, '☐ commit them first ("auth re…") · they stay in the worktree');
	// Nothing uncommitted: no toggle and no space in the hint; no commits either: says so.
	const clean = mergeConfirmLayout(input({preview: preview({uncommitted: 0, commitCount: 1, commits: ['one'], diff: {files: 1, insertions: 1, deletions: 0}})}));
	assert.deepEqual(texts(clean.details), ['Into main · ~/code/app', '1 commit · 1 file +1 −0', '  one']);
	assert.equal(clean.hint, 'enter choose · j/k move · esc cancel');
	const onlyUncommitted = mergeConfirmLayout(input({preview: preview({commitCount: 0, commits: [], diff: {files: 0, insertions: 0, deletions: 0}, uncommitted: 1})}));
	assert.deepEqual(texts(onlyUncommitted.details), ['Into main · ~/code/app', 'No new commits · +1 uncommitted file', '1 uncommitted file', '☑ commit them first ("auth refactor")']);
	assert.equal(texts(mergeConfirmLayout(input({preview: preview({commitCount: 0, commits: [], uncommitted: 0})})).details)[1], 'No new commits');
	// Loading and a failed preview.
	assert.deepEqual(texts(mergeConfirmLayout(input({preview: undefined})).details), ['Checking what will be merged…']);
	assert.deepEqual(texts(mergeConfirmLayout(input({preview: undefined, previewError: 'not a git repository'})).details), ['Preview unavailable: not a git repository']);
	assert.deepEqual(mergeConfirmLayout(input({preview: undefined})).options, ['Merge into the current branch without committing', 'Squash merge into the current branch without committing', 'Cancel']);
});

test('merge confirmation: a surprising target is warned, operations in progress and overlapping target edits are flagged', t => {
	withEnv(t, {HOME: '/home/me'});
	// Not the main checkout, or not the default branch: warn.
	for (const fields of [{targetIsMain: false, targetRoot: '/home/me/wt/other'}, {targetBranch: 'release'}, {targetBranch: undefined}] as Partial<MergePreview>[]) {
		const layout = mergeConfirmLayout(input({preview: preview(fields)}));
		assert.equal(layout.details[0]!.color, THEME.warn, JSON.stringify(fields));
		assert.ok(surprisingTarget(preview(fields)));
	}
	assert.equal(mergeConfirmLayout(input({preview: preview({targetIsMain: false, targetRoot: '/home/me/wt/other'})})).details[0]!.text, 'Into main · ~/wt/other');
	assert.equal(mergeConfirmLayout(input({preview: preview({targetBranch: undefined})})).details[0]!.text, 'Into (detached) · ~/code/app');
	// No default branch known: only the checkout decides.
	assert.equal(surprisingTarget(preview({defaultBranch: undefined, targetBranch: 'trunk'})), false);
	// A long path is compacted to fit after the branch.
	assert.equal(mergeConfirmLayout(input({width: 30, preview: preview({targetRoot: '/home/me/projects/clients/acme/app'})})).details[0]!.text, 'Into main · …/acme/app');

	const busy = mergeConfirmLayout(input({width: 80, preview: preview({inProgress: 'rebase'})}));
	assert.deepEqual([busy.details[1]!.text, busy.details[1]!.color], ['⚠ A rebase is in progress in the target: finish or abort it first', THEME.warn]);
	assert.equal(mergeConfirmLayout(input({width: 80, preview: preview({inProgress: 'conflicts'})})).details[1]!.text, '⚠ The target has unresolved conflicts: resolve and commit them first');

	// Target edits the commits touch always count; those the uncommitted files touch only when they are committed first.
	const overlapping = preview({overlap: {committed: ['src/a.ts'], uncommitted: ['src/b.ts', 'src/a.ts']}});
	assert.deepEqual(overlappingFiles(overlapping, true), ['src/a.ts', 'src/b.ts']);
	assert.deepEqual(overlappingFiles(overlapping, false), ['src/a.ts']);
	const warned = mergeConfirmLayout(input({width: 100, preview: overlapping}));
	assert.deepEqual([warned.details.at(-1)!.text, warned.details.at(-1)!.color], ['⚠ Target has uncommitted changes in 2 files the merge touches: src/a.ts, src/b.ts', THEME.warn]);
	assert.equal(mergeConfirmLayout(input({width: 100, commitFirst: false, preview: overlapping})).details.at(-1)!.text, '⚠ Target has uncommitted changes in 1 file the merge touches: src/a.ts');
	assert.ok(!mergeConfirmLayout(input({preview: preview()})).details.some(line => line.text.startsWith('⚠')));
});

test('merge confirmation is height-budgeted: notes shrink first, then the error output, then commit subjects', () => {
	const notes = [{key: 's', title: 'auth refactor', lines: Array.from({length: 12}, (_, index) => `note ${index + 1}`)}];
	const error = 'Commit failed in the worktree, so nothing was merged:\nhusky - pre-commit\nlint: 3 problems\nsrc/a.ts:1 no-unused-vars\nsrc/b.ts:9 eqeqeq';
	const at = (height: number) => mergeConfirmLayout(input({notes, error, height}));
	// Never more rows than the pane, down to the essentials (13 rows here).
	for (let height = 13; height <= 45; height++) assert.ok(mergeConfirmRows(at(height)) <= height, String(height));
	// Plenty of room: everything, the notes complete.
	assert.deepEqual([at(42).notes.length, at(42).error.length, at(42).details.length], [14, 5, 11]);
	// Less: the notes are cut (with a count), then gone, before anything else.
	assert.deepEqual(texts(at(40).notes).slice(-1), ['+2 more lines']);
	assert.equal(mergeConfirmRows(at(40)), 40);
	assert.deepEqual([at(28).notes.length, at(28).error.length, at(28).details.length], [0, 5, 11]);
	// Then the error keeps its first and last lines (a hook's verdict comes last), then goes.
	assert.deepEqual(texts(at(24).error), ['Commit failed in the worktree, so nothing was merged:', '  src/a.ts:1 no-unused-vars', '  src/b.ts:9 eqeqeq']);
	assert.deepEqual([at(21).error.length, at(21).details.length], [0, 11]);
	// Then the commit subjects shrink; the last row shown counts the rest.
	const tight = at(15);
	assert.deepEqual(texts(tight.details), ['Into main · /home/me/code/app', '8 commits · 5 files +120 −14 · +4 uncommitted files', '  add login form', '  +7 more', '4 uncommitted files', '☑ commit them first ("auth refactor")']);
	assert.equal(mergeConfirmRows(tight), 15);
	// The essentials stay even when nothing else fits.
	assert.deepEqual(texts(at(9).details), ['Into main · /home/me/code/app', '8 commits · 5 files +120 −14 · +4 uncommitted files', '4 uncommitted files', '☑ commit them first ("auth refactor")']);
});

test('conflict view: the count, up to five files (+N more) and only the two choices', () => {
	const view = conflictView({conflicts: ['src/a.ts', 'src/b.ts', 'README.md'], conflictCount: 3}, 60);
	assert.deepEqual(view, {
		title: 'Merged with conflicts in 3 files',
		files: ['  src/a.ts', '  src/b.ts', '  README.md'],
		choices: [{key: 'enter', text: 'keep it: resolve it in your editor or the Git tab (esc too)'}, {key: 'a', text: 'abort the merge'}],
	});
	const many = conflictView({conflicts: Array.from({length: 9}, (_, index) => `f${index}.ts`), conflictCount: 240}, 60);
	assert.deepEqual(many.files, ['  f0.ts', '  f1.ts', '  f2.ts', '  f3.ts', '  +236 more']);
	assert.equal(many.title, 'Merged with conflicts in 240 files');
	assert.deepEqual(conflictView({conflicts: ['only.ts'], conflictCount: 1}, 60).title, 'Merged with conflicts in 1 file');
	assert.deepEqual(conflictView({conflicts: ['a', 'b', 'c', 'd', 'e']}, 60).files.length, 5);
});

test('rendered: the confirmation fills exactly its pane (warn target, toggle, notes) and the conflict view shows only the choices', t => {
	withEnv(t, {HOME: '/home/me'});
	const plain = (text: string) => text.replace(/\x1b\[[0-9;]*m/g, '');
	const session = {id: 's', title: 'auth refactor', program: 'claude', command: 'claude', cwd: '/home/me/wt/auth', repoRoot: '/home/me/code/app', worktree: {mode: 'managed', id: 'w', path: '/home/me/wt/auth', branch: 'auth'}, status: 'running', createdAt: '', updatedAt: '', notes: '- [ ] cookie flags'} as SessionRecord;
	const flow = {sessionId: 's', commitFirst: true, preview: preview({targetRoot: '/home/me/wt/release', targetBranch: 'release', targetIsMain: false, overlap: {committed: ['src/auth.ts'], uncommitted: []}})};
	for (const height of [30, 16]) {
		const lines = plain(renderToString(React.createElement(MergeConfirmPane, {session, sessions: [session], flow, selectedIndex: 1, width: 84, height}), {columns: 84})).split('\n');
		assert.equal(lines.length, height, String(height));
		assert.match(lines[2]!, /Into release · ~\/wt\/release/);
		assert.ok(lines.some(line => line.includes('☑ commit them first ("auth refactor")')));
		assert.ok(lines.some(line => line.includes('⚠ Target has uncommitted changes in 1 file the merge touches: src/auth.ts')));
		assert.ok(lines.some(line => line.includes('❯ Squash merge into release without committing')));
		assert.equal(lines.some(line => line.includes('cookie flags')), height === 30);
	}
	const conflict = plain(renderToString(React.createElement(MergeConflictPane, {result: {mode: 'merge', sourceRef: 'auth', targetBranch: 'main', conflicted: true, conflicts: ['src/auth.ts'], conflictCount: 1, stdout: '', stderr: ''}, width: 70}), {columns: 70})).split('\n');
	assert.deepEqual(conflict.slice(1, -1).map(line => line.slice(2, -1).trimEnd()), ['Merged with conflicts in 1 file', '  src/auth.ts', '', 'enter keep it: resolve it in your editor or the Git tab (esc too)', 'a     abort the merge']);
});
