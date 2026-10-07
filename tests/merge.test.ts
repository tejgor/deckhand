import assert from 'node:assert/strict';
import {test, type TestContext} from 'node:test';
import fs from 'node:fs/promises';
import path from 'node:path';
import {abortMerge, branchCreationCommit, mergeWorktreeIntoCurrent, operationInProgress} from '../src/git.js';
import {getMergePreview, getWorkspaceSummary, inspectWorkspaceCleanup, mergedIntoDefault} from '../src/workspaceGit.js';
import {exec, git, repo, withEnv} from './helpers.js';

// Merging a worktree (m) against real fixture repositories: preview, commit-first, target checks, conflicts, squash
// cleanup and merge detection. Library calls ignore the user's Git config like the fixture helpers do.
async function fixture(t: TestContext): Promise<{root: string; worktree: (name: string) => Promise<string>}> {
	withEnv(t, {GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1'});
	const root = await fs.realpath(await repo());
	const parent = await fs.mkdtemp(path.join(path.dirname(root), 'deckhand-merge-wt-'));
	t.after(async () => { await fs.rm(root, {recursive: true, force: true}); await fs.rm(parent, {recursive: true, force: true}); });
	await fs.writeFile(path.join(root, 'other.txt'), 'other\n'); await git(root, 'add', '.'); await git(root, 'commit', '-m', 'other');
	return {root, worktree: async name => { const dir = path.join(parent, name); await git(root, 'worktree', 'add', '-b', name, dir); return fs.realpath(dir); }};
}
const write = (dir: string, file: string, text: string) => fs.writeFile(path.join(dir, file), text);
const read = (dir: string, file: string) => fs.readFile(path.join(dir, file), 'utf8');
const commit = async (dir: string, file: string, text: string, message: string) => { await write(dir, file, text); await git(dir, 'add', '-A'); await git(dir, 'commit', '-m', message); };
// Untrimmed: the first line's leading status column matters.
const status = async (dir: string) => (await exec('git', ['-C', dir, 'status', '--porcelain', '--untracked-files=all'])).stdout.trimEnd();

test('preview: target, commits, diff stat, uncommitted files; commit-first commits them (title as message) before merging', async t => {
	const {root, worktree} = await fixture(t);
	const wt = await worktree('feat');
	await commit(wt, 'a.txt', 'one\ntwo\nthree\n', 'add a');
	await commit(wt, 'file.txt', 'first\nchanged\n', 'change file');
	await write(wt, 'other.txt', 'other edited\n'); await write(wt, 'new.txt', 'untracked\n');
	const preview = await getMergePreview(wt, root);
	assert.deepEqual({...preview, sourceSha: undefined}, {
		sourceRef: 'feat', sourceSha: undefined, targetRoot: root, targetBranch: 'main', targetIsMain: true, defaultBranch: 'main',
		commitCount: 2, commits: ['change file', 'add a'], diff: {files: 2, insertions: 4, deletions: 0}, uncommitted: 2, overlap: {committed: [], uncommitted: []},
	});
	assert.equal(preview.sourceSha, await git(wt, 'rev-parse', 'HEAD'));
	// Another linked worktree as the target is not the main checkout.
	const other = await worktree('other-target');
	assert.equal((await getMergePreview(wt, other)).targetIsMain, false);
	await assert.rejects(getMergePreview(wt, wt), /into itself/);

	const result = await mergeWorktreeIntoCurrent(wt, root, 'squash', {commitFirst: 'auth refactor'});
	assert.ok(!result.skipped && !result.conflicted);
	assert.equal(result.committed?.files, 2);
	assert.equal(await git(wt, 'log', '-1', '--format=%s'), 'auth refactor');
	assert.equal(result.sourceSha, await git(wt, 'rev-parse', 'HEAD'));
	assert.equal(await status(wt), '');
	// Everything, the committed-first files included, is staged in the target, uncommitted.
	assert.deepEqual((await git(root, 'diff', '--cached', '--name-only')).split('\n').sort(), ['a.txt', 'file.txt', 'new.txt', 'other.txt']);
});

test('commit-first: a worktree with only uncommitted work merges through the toggle; off, it is skipped and the files stay; a failing hook merges nothing', async t => {
	const {root, worktree} = await fixture(t);
	const wt = await worktree('only-edits');
	await write(wt, 'file.txt', 'edited\n'); await write(wt, 'fresh.txt', 'fresh\n');
	const preview = await getMergePreview(wt, root);
	assert.deepEqual([preview.commitCount, preview.uncommitted], [0, 2]);
	const off = await mergeWorktreeIntoCurrent(wt, root, 'merge');
	assert.deepEqual([off.skipped, off.reason, off.committed], [true, 'No new commits to merge', undefined]);
	assert.deepEqual((await status(wt)).split('\n').sort(), [' M file.txt', '?? fresh.txt']);
	assert.equal(await status(root), '');
	const on = await mergeWorktreeIntoCurrent(wt, root, 'merge', {commitFirst: 'only edits'});
	assert.ok(!on.skipped && !on.conflicted); assert.equal(on.committed?.files, 2);
	assert.match(await status(root), /fresh\.txt/);
	await git(root, 'merge', '--abort');

	// A pre-commit hook that fails: its output comes back, nothing is merged into the target.
	const hooks = path.join(root, '.git', 'failing-hooks'); await fs.mkdir(hooks);
	await fs.writeFile(path.join(hooks, 'pre-commit'), '#!/bin/sh\necho "lint: 3 problems" >&2\nexit 1\n', {mode: 0o755});
	await git(root, 'config', 'core.hooksPath', hooks);
	const blocked = await worktree('hooked');
	await write(blocked, 'blocked.txt', 'x\n');
	await assert.rejects(mergeWorktreeIntoCurrent(blocked, root, 'squash', {commitFirst: 'blocked'}), /Commit failed in the worktree, so nothing was merged:[\s\S]*lint: 3 problems/);
	assert.equal(await status(root), '');
	assert.equal(await git(blocked, 'log', '-1', '--format=%s'), 'other');
});

test('target checks: a merge, rebase, cherry-pick or conflicts in progress refuse the merge; overlapping uncommitted target edits are listed', async t => {
	const {root, worktree} = await fixture(t);
	const wt = await worktree('feat');
	await commit(wt, 'a.txt', 'a\n', 'add a');
	await commit(wt, 'file.txt', 'feature\n', 'change file');
	// Target edits: one the commits touch, one the uncommitted source file touches, one unrelated.
	await write(root, 'file.txt', 'target edit\n'); await write(root, 'notes.md', 'mine\n'); await write(root, 'unrelated.txt', 'u\n');
	await write(wt, 'notes.md', 'theirs\n');
	const preview = await getMergePreview(wt, root);
	assert.deepEqual(preview.overlap, {committed: ['file.txt'], uncommitted: ['notes.md']});
	assert.equal(preview.inProgress, undefined);
	await fs.rm(path.join(root, 'file.txt')); await git(root, 'checkout', '--', 'file.txt');
	await fs.rm(path.join(root, 'notes.md')); await fs.rm(path.join(root, 'unrelated.txt'));

	// A merge in progress (a conflicted one, started outside Deckhand).
	const other = await worktree('other');
	await commit(other, 'file.txt', 'other side\n', 'other change');
	await commit(root, 'file.txt', 'main side\n', 'main change');
	await git(root, 'merge', 'other').catch(() => {});
	assert.equal(await operationInProgress(root), 'merge');
	assert.equal((await getMergePreview(wt, root)).inProgress, 'merge');
	await assert.rejects(mergeWorktreeIntoCurrent(wt, root, 'merge'), /A merge is in progress in the target: finish or abort it first/);
	await git(root, 'merge', '--abort');
	// A rebase (its state directory) and a cherry-pick (its head file), as Git leaves them mid-way.
	const gitDir = path.join(root, '.git');
	await fs.mkdir(path.join(gitDir, 'rebase-merge'));
	await assert.rejects(mergeWorktreeIntoCurrent(wt, root, 'squash'), /A rebase is in progress/);
	await fs.rm(path.join(gitDir, 'rebase-merge'), {recursive: true});
	await fs.writeFile(path.join(gitDir, 'CHERRY_PICK_HEAD'), `${await git(root, 'rev-parse', 'HEAD')}\n`);
	assert.equal(await operationInProgress(root), 'cherry-pick');
	await fs.rm(path.join(gitDir, 'CHERRY_PICK_HEAD'));
	// In a linked worktree as the target, its own state counts (not the main checkout's).
	await fs.mkdir(path.join(gitDir, 'worktrees', 'other', 'rebase-apply'));
	assert.equal(await operationInProgress(other), 'rebase'); assert.equal(await operationInProgress(root), undefined);
});

test('squash merge: cleanup counts commits up to the merged one as integrated, later commits still need DELETE', async t => {
	const {root, worktree} = await fixture(t);
	const wt = await worktree('squashed');
	await commit(wt, 'a.txt', 'a\n', 'one'); await commit(wt, 'b.txt', 'b\n', 'two');
	const result = await mergeWorktreeIntoCurrent(wt, root, 'squash');
	await git(root, 'commit', '-m', 'squashed');
	// Git does not consider the branch merged: without the recorded commit, deleting the branch would "lose" both.
	assert.match((await inspectWorkspaceCleanup(wt, 'main', {deleteBranch: true})).reasons.join(), /2 commit\(s\) exist only on branch squashed/);
	const integrated = await inspectWorkspaceCleanup(wt, 'main', {deleteBranch: true, integrated: result.sourceSha});
	assert.equal(integrated.safe, true, integrated.reasons.join());
	await commit(wt, 'c.txt', 'c\n', 'after the merge');
	assert.match((await inspectWorkspaceCleanup(wt, 'main', {deleteBranch: true, integrated: result.sourceSha})).reasons.join(), /1 commit\(s\) exist only on branch squashed/);
	// A recorded commit that no longer exists (or is not a SHA) is ignored, never trusted.
	assert.match((await inspectWorkspaceCleanup(wt, 'main', {deleteBranch: true, integrated: 'f'.repeat(40)})).reasons.join(), /3 commit\(s\)/);
	assert.match((await inspectWorkspaceCleanup(wt, 'main', {deleteBranch: true, integrated: '--all'})).reasons.join(), /3 commit\(s\)/);
});

test('conflicts: kept, the merge stays in progress with markers; aborted, normal and squash merges restore the target and keep unrelated edits', async t => {
	const {root, worktree} = await fixture(t);
	const wt = await worktree('conflicting');
	await commit(wt, 'file.txt', 'feature side\n', 'feature'); await commit(wt, 'added.txt', 'added\n', 'add');
	await commit(root, 'file.txt', 'main side\n', 'main');
	const head = await git(root, 'rev-parse', 'HEAD');
	// Unrelated uncommitted work in the target before the merge: an unstaged edit and an untracked file.
	await write(root, 'other.txt', 'edited before the merge\n'); await write(root, 'mine.txt', 'untracked\n');
	const unrelated = async () => { assert.equal(await read(root, 'other.txt'), 'edited before the merge\n'); assert.equal(await read(root, 'mine.txt'), 'untracked\n'); };

	// A normal merge: conflicted result, MERGE_HEAD, markers; abort is git merge --abort.
	const normal = await mergeWorktreeIntoCurrent(wt, root, 'merge');
	assert.deepEqual([normal.conflicted, normal.conflicts, normal.conflictCount, normal.indexClean], [true, ['file.txt'], 1, true]);
	assert.match(await read(root, 'file.txt'), /<<<<<<<[\s\S]*>>>>>>>/);
	assert.equal(await operationInProgress(root), 'merge');
	await abortMerge(root);
	assert.equal(await operationInProgress(root), undefined); assert.equal(await read(root, 'file.txt'), 'main side\n');
	assert.equal(await git(root, 'diff', '--cached', '--name-only'), '');
	await unrelated();

	// A squash merge has no MERGE_HEAD: the conflicts are what is in progress. Only Deckhand's own (clean index, same HEAD) is undone.
	const squash = await mergeWorktreeIntoCurrent(wt, root, 'squash');
	assert.deepEqual([squash.conflicted, squash.conflicts, squash.targetHead, squash.indexClean], [true, ['file.txt'], head, true]);
	assert.equal(await operationInProgress(root), 'conflicts');
	await assert.rejects(abortMerge(root), /Cannot abort this squash merge safely/);
	await assert.rejects(abortMerge(root, {mode: 'squash', targetHead: 'f'.repeat(40), indexClean: true}), /Cannot abort/);
	await assert.rejects(abortMerge(root, {mode: 'squash', targetHead: head, indexClean: false}), /Cannot abort/);
	await abortMerge(root, {mode: 'squash', targetHead: squash.targetHead, indexClean: squash.indexClean});
	assert.equal(await operationInProgress(root), undefined); assert.equal(await read(root, 'file.txt'), 'main side\n');
	await assert.rejects(fs.access(path.join(root, 'added.txt'))); await assert.rejects(fs.access(path.join(root, '.git', 'SQUASH_MSG')));
	assert.deepEqual((await status(root)).split('\n').sort(), [' M other.txt', '?? mine.txt']);
	await unrelated();

	// Kept: left as Git left it (conflict markers, the clean part staged) for the user to resolve.
	await mergeWorktreeIntoCurrent(wt, root, 'squash');
	assert.match(await read(root, 'file.txt'), /<<<<<<</);
	assert.match(await status(root), /^UU file\.txt$/m); assert.match(await status(root), /^A {2}added\.txt$/m);
	await unrelated();
	// Git itself refuses a squash merge while unrelated changes are staged, so a conflicted squash always started clean.
	await git(root, 'reset', '--merge'); await git(root, 'add', 'other.txt');
	await assert.rejects(mergeWorktreeIntoCurrent(wt, root, 'squash'), /overwritten by merge/);
});

test('merge detection: a branch whose commits are in the default branch counts; a fresh or merely fast-forwarded branch never does', async t => {
	const {root, worktree} = await fixture(t);
	const defaults = ['refs/heads/main'];
	const check = async (wt: string, branch: string, start: string, refs = defaults) => mergedIntoDefault({cwd: wt, tip: await git(wt, 'rev-parse', 'HEAD'), branch, start, defaults: refs});
	// Fresh: no commits beyond its start (trivially an ancestor of main).
	const fresh = await worktree('fresh');
	const freshStart = await branchCreationCommit(fresh, 'fresh');
	assert.equal(freshStart, await git(fresh, 'rev-parse', 'HEAD'));
	assert.equal(await check(fresh, 'fresh', freshStart!), false);
	// Fresh, then brought up to date with a newer main (fast-forward): still nothing of its own.
	await commit(root, 'main.txt', 'm\n', 'main moves on');
	await git(fresh, 'merge', '--ff-only', 'main');
	assert.equal(await check(fresh, 'fresh', freshStart!), false);

	// A branch with a commit: not merged, then merged into main (here with a merge commit).
	const feat = await worktree('feat');
	const start = (await branchCreationCommit(feat, 'feat'))!;
	await commit(feat, 'f.txt', 'f\n', 'feature');
	assert.equal(await check(feat, 'feat', start), false);
	await git(root, 'merge', '--no-ff', '-m', 'merge feat', 'feat');
	assert.equal(await check(feat, 'feat', start), true);
	// Commits after that merge are not merged.
	await commit(feat, 'g.txt', 'g\n', 'more');
	assert.equal(await check(feat, 'feat', start), false);

	// Merged only on the remote (origin/main, never fetched by Deckhand): counts when that ref is given.
	const remote = await worktree('remote');
	const remoteStart = (await branchCreationCommit(remote, 'remote'))!;
	await commit(remote, 'r.txt', 'r\n', 'remote work');
	await git(root, 'update-ref', 'refs/remotes/origin/main', await git(remote, 'rev-parse', 'HEAD'));
	assert.equal(await check(remote, 'remote', remoteStart), false);
	assert.equal(await check(remote, 'remote', remoteStart, [...defaults, 'refs/remotes/origin/main']), true);
	// No starting point or no default refs: never.
	assert.equal(await check(remote, 'remote', 'not-a-sha', [...defaults, 'refs/remotes/origin/main']), false);
	assert.equal(await check(remote, 'remote', remoteStart, []), false);
	// A branch reset behind where it started is not "beyond" it.
	const reset = await worktree('reset');
	const resetStart = (await branchCreationCommit(reset, 'reset'))!;
	await git(reset, 'reset', '--hard', 'HEAD~1');
	assert.equal(await check(reset, 'reset', resetStart), false);
});

test('the PR lookup is injectable: a MERGED PR reaches the summary (with its head and base), a failing lookup becomes prError', async t => {
	const {worktree} = await fixture(t);
	const wt = await worktree('pr-branch');
	await commit(wt, 'p.txt', 'p\n', 'pr work');
	const headSha = await git(wt, 'rev-parse', 'HEAD');
	let asked = '';
	const summary = await getWorkspaceSummary(wt, 'main', true, async cwd => { asked = cwd; return {number: 7, url: 'https://github.com/acme/app/pull/7', state: 'MERGED', checks: 'passing', headSha, baseBranch: 'main'}; });
	assert.equal(asked, wt);
	assert.deepEqual(summary.pr, {number: 7, url: 'https://github.com/acme/app/pull/7', state: 'MERGED', checks: 'passing', headSha, baseBranch: 'main'});
	const missing = await getWorkspaceSummary(wt, 'main', true, async () => { throw Object.assign(new Error('spawn gh ENOENT'), {code: 'ENOENT'}); });
	assert.deepEqual([missing.pr, missing.prError], [undefined, 'Install gh for PR status']);
	// Without includePr the lookup never runs.
	await getWorkspaceSummary(wt, 'main', false, async () => { throw new Error('must not run'); });
});
