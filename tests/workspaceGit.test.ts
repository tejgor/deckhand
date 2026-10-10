import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import path from 'node:path';
import {getWorkspaceSummary, inspectWorkspaceCleanup, parseStatus} from '../src/workspaceGit.js';
import {git, repo} from './helpers.js';

test('porcelain v2 parsing handles headers, renames with odd names, unmerged, untracked and ignored entries', () => {
	const raw = ['# branch.oid (initial)', '# branch.head (detached)', '# branch.upstream origin/x', '# branch.ab +2 -1',
		'2 R. N... 100644 100644 100644 aaa bbb R100 new name\nwith newline', 'old 1 M. x', '1 .M N... 100644 100644 100644 aaa aaa spaced 2 name',
		'u UU N... 100644 100644 100644 100644 a b c conflict', '? untracked\nname', '! node_modules/', ''].join('\0');
	assert.deepEqual(parseStatus(raw), {oid: undefined, branch: undefined, upstream: 'origin/x', ahead: 2, behind: 1, dirtyFiles: 3, untracked: ['untracked\nname'], ignored: ['node_modules/'], entries: [
		{kind: '2', xy: 'R.', path: 'new name\nwith newline', origPath: 'old 1 M. x'}, {kind: '1', xy: '.M', path: 'spaced 2 name'},
		{kind: 'u', xy: 'UU', path: 'conflict'}, {kind: '?', xy: '??', path: 'untracked\nname'},
	]});
});
test('cleanup blocks dirty, untracked and valuable ignored files but not dependency caches', async t => {
	const cwd = await repo(); t.after(() => fs.rm(cwd, {recursive: true, force: true}));
	await fs.writeFile(path.join(cwd, '.gitignore'), '.env\nnode_modules/\ntarget/\n');
	await git(cwd, 'add', '.gitignore'); await git(cwd, 'commit', '-m', 'ignore');
	await fs.writeFile(path.join(cwd, '.env'), 'secret');
	await fs.mkdir(path.join(cwd, 'node_modules')); await fs.writeFile(path.join(cwd, 'node_modules', 'cache'), 'cache');
	await fs.mkdir(path.join(cwd, 'target', 'deep'), {recursive: true}); for (let i = 0; i < 50; i++) await fs.writeFile(path.join(cwd, 'target', 'deep', `${i}`), '');
	await fs.writeFile(path.join(cwd, 'file.txt'), 'modified\n');
	await fs.writeFile(path.join(cwd, 'untracked.txt'), 'valuable');
	const inspection = await inspectWorkspaceCleanup(cwd, 'main');
	assert.equal(inspection.dirtyFiles, 1); assert.equal(inspection.untrackedFiles, 1); assert.equal(inspection.ignoredFiles, 2); assert.equal(inspection.safe, false);
	assert.ok(inspection.reasons.some(reason => reason.includes('.env') && reason.includes('target/')));
	// The files themselves, for the confirmation: changed first, then untracked, then valuable ignored (no dependency cache).
	assert.deepEqual(inspection.files, [
		{path: 'file.txt', state: 'changed'}, {path: 'untracked.txt', state: 'untracked'},
		{path: '.env', state: 'ignored'}, {path: 'target/', state: 'ignored'},
	]);
});
test('summary separates staged/unstaged changes from commits and reports local upstream', async t => {
	const cwd = await repo(); t.after(() => fs.rm(cwd, {recursive: true, force: true}));
	await git(cwd, 'checkout', '-b', 'feature'); await git(cwd, 'branch', '--set-upstream-to=main');
	await fs.writeFile(path.join(cwd, 'file.txt'), 'first\nsecond\n'); await git(cwd, 'add', '.'); await git(cwd, 'commit', '-m', 'feature');
	await fs.writeFile(path.join(cwd, 'file.txt'), 'first\nsecond\nthird\n'); await git(cwd, 'add', 'file.txt');
	await fs.writeFile(path.join(cwd, 'file.txt'), 'first\nsecond\nthird\nfourth\n');
	const summary = await getWorkspaceSummary(cwd, 'main');
	assert.equal(summary.branch, 'feature'); assert.equal(summary.changedFiles, 1); assert.equal(summary.additions, 2);
	assert.equal(summary.commitsAheadOfBase, 1); assert.equal(summary.ahead, 1); assert.equal(summary.behind, 0);
	const inspection = await inspectWorkspaceCleanup(cwd, 'main'); assert.equal(inspection.unpublishedCommits, 1); assert.equal(inspection.unmergedCommits, 1);
});
test('commits only block when deleting the branch or detached HEAD would lose them; refs cannot inject options', async t => {
	const cwd = await repo(); t.after(() => fs.rm(cwd, {recursive: true, force: true}));
	await git(cwd, 'checkout', '-b', 'feature');
	assert.equal((await inspectWorkspaceCleanup(cwd)).safe, true); // Never pushed, no new commits.
	assert.equal((await inspectWorkspaceCleanup(cwd, undefined, {deleteBranch: true})).safe, true); // main still has them.
	await fs.writeFile(path.join(cwd, 'file.txt'), 'feature\n'); await git(cwd, 'commit', '-am', 'feature');
	assert.equal((await inspectWorkspaceCleanup(cwd, 'main')).safe, true); // The branch preserves the commit.
	const deleting = await inspectWorkspaceCleanup(cwd, 'main', {deleteBranch: true});
	assert.equal(deleting.safe, false); assert.match(deleting.reasons.join(), /1 commit\(s\) exist only on branch feature/);
	await git(cwd, 'checkout', '--detach'); await fs.writeFile(path.join(cwd, 'file.txt'), 'detached\n'); await git(cwd, 'commit', '-am', 'detached');
	assert.match((await inspectWorkspaceCleanup(cwd)).reasons.join(), /detached HEAD/);
	await assert.rejects(getWorkspaceSummary(cwd, '--all'), /Invalid Git/);
	await assert.rejects(inspectWorkspaceCleanup(cwd, '--all'), /Invalid Git/);
});
test('symlinked node_modules is disposable; unborn repositories and missing paths do not fabricate data', async t => {
	const cwd = await repo(), shared = await repo(); t.after(async () => { await fs.rm(cwd, {recursive: true, force: true}); await fs.rm(shared, {recursive: true, force: true}); });
	await fs.symlink(shared, path.join(cwd, 'node_modules'));
	assert.equal((await inspectWorkspaceCleanup(cwd)).safe, true);
	await git(cwd, 'checkout', '--orphan', 'unborn'); await git(cwd, 'rm', '-f', 'file.txt');
	const summary = await getWorkspaceSummary(cwd, 'missing'); assert.equal(summary.head, ''); assert.equal(summary.baseRef, undefined); assert.equal(summary.branch, 'unborn');
	const missing = await inspectWorkspaceCleanup(path.join(cwd, 'gone'));
	assert.equal(missing.safe, false); assert.match(missing.reasons[0] ?? '', /Git status unavailable/);
});
