import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {applyStage, readChangeDiff, readChanges} from '../src/changesGit.js';
import {findChange} from '../src/changesModel.js';
import {git, repo} from './helpers.js';

const shape = async (cwd: string) => (await readChanges(cwd)).record.entries.map(entry => `${entry.group}:${entry.status}:${entry.origPath ? `${entry.origPath} → ` : ''}${entry.path}`);

test('Changes read, diff and stage/unstage a fixture repository (spaces, globs, renames, binary, untracked)', async t => {
	const cwd = await repo(); t.after(() => fs.rm(cwd, {recursive: true, force: true}));
	await fs.writeFile(path.join(cwd, 'move me.txt'), 'a\nb\n'); await fs.writeFile(path.join(cwd, 'gone.txt'), 'x\n');
	await git(cwd, 'add', '.'); await git(cwd, 'commit', '-m', 'more');
	await fs.writeFile(path.join(cwd, 'file.txt'), 'first\nsecond\n'); await git(cwd, 'add', 'file.txt');
	await fs.writeFile(path.join(cwd, 'file.txt'), 'first\nsecond\nthird\n');
	await git(cwd, 'mv', 'move me.txt', 'moved me.txt');
	await fs.rm(path.join(cwd, 'gone.txt'));
	await fs.writeFile(path.join(cwd, 'new *.txt'), 'one\ntwo\nthree');
	await fs.writeFile(path.join(cwd, 'new a.txt'), 'glob victim\n');
	await fs.writeFile(path.join(cwd, 'blob.bin'), Buffer.from([0, 1, 2, 3]));
	let snapshot = await readChanges(cwd);
	assert.equal(snapshot.record.branch, 'main'); assert.equal(snapshot.hasHead, true);
	assert.deepEqual(await shape(cwd), ['staged:M:file.txt', 'staged:R:move me.txt → moved me.txt', 'unstaged:M:file.txt', 'unstaged:D:gone.txt', 'untracked:?:blob.bin', 'untracked:?:new *.txt', 'untracked:?:new a.txt']);
	const record = snapshot.record;
	assert.deepEqual([findChange(record.entries, 'staged', 'file.txt')?.additions, findChange(record.entries, 'unstaged', 'file.txt')?.additions, findChange(record.entries, 'unstaged', 'gone.txt')?.deletions], [1, 1, 1]);
	assert.equal(findChange(record.entries, 'untracked', 'new *.txt')?.additions, 3);
	assert.equal(findChange(record.entries, 'untracked', 'blob.bin')?.binary, true);

	// Diffs: staged vs unstaged sides, an untracked file as additions, binary detection.
	const stagedDiff = await readChangeDiff(cwd, findChange(snapshot.all, 'staged', 'file.txt')!);
	assert.match(stagedDiff.text, /^\+second$/m); assert.doesNotMatch(stagedDiff.text, /third/); assert.equal(stagedDiff.firstLine, 2);
	const unstagedDiff = await readChangeDiff(cwd, findChange(snapshot.all, 'unstaged', 'file.txt')!);
	assert.match(unstagedDiff.text, /^\+third$/m); assert.equal(unstagedDiff.firstLine, 3);
	const renameDiff = await readChangeDiff(cwd, findChange(snapshot.all, 'staged', 'moved me.txt')!);
	assert.match(renameDiff.text, /rename from move me.txt/);
	const untrackedDiff = await readChangeDiff(cwd, findChange(snapshot.all, 'untracked', 'new *.txt')!);
	assert.equal(untrackedDiff.text, '@@ -0,0 +1,3 @@\n+one\n+two\n+three'); assert.equal(untrackedDiff.firstLine, 1);
	assert.equal((await readChangeDiff(cwd, findChange(snapshot.all, 'untracked', 'blob.bin')!)).binary, true);

	// Staging a glob-like name stages only that file; staging a deletion works; unstaging a rename restores both paths.
	await applyStage(cwd, snapshot, 'stage', {group: 'untracked', path: 'new *.txt'});
	snapshot = await readChanges(cwd);
	assert.ok(findChange(snapshot.all, 'staged', 'new *.txt')); assert.ok(findChange(snapshot.all, 'untracked', 'new a.txt'));
	await applyStage(cwd, snapshot, 'stage', {group: 'unstaged', path: 'gone.txt'});
	snapshot = await readChanges(cwd);
	assert.equal(findChange(snapshot.all, 'staged', 'gone.txt')?.status, 'D');
	await applyStage(cwd, snapshot, 'unstage', {group: 'staged', path: 'moved me.txt'});
	assert.deepEqual((await shape(cwd)).filter(line => /move/.test(line)), ['unstaged:D:move me.txt', 'untracked:?:moved me.txt']);
	// Paths not in the current status (or on the wrong side) are refused.
	snapshot = await readChanges(cwd);
	await assert.rejects(applyStage(cwd, snapshot, 'stage', {group: 'unstaged', path: '../outside'}), /not among the unstaged changes/);
	await assert.rejects(applyStage(cwd, snapshot, 'unstage', {group: 'untracked', path: 'new a.txt'}), /not among the staged changes/);
	await assert.rejects(applyStage(cwd, snapshot, 'stage', {group: 'staged', path: 'file.txt'}), /not among the unstaged changes/);

	// Stage all, then unstage all.
	assert.equal((await applyStage(cwd, snapshot, 'stage')).skippedConflicts, 0);
	assert.ok((await shape(cwd)).every(line => line.startsWith('staged:')));
	await applyStage(cwd, await readChanges(cwd), 'unstage');
	assert.ok((await shape(cwd)).every(line => !line.startsWith('staged:')));
	await assert.rejects(readChanges(path.join(cwd, 'missing')), /Worktree directory is missing/);
});

test('without a HEAD commit unstaging removes paths from the index; stage all leaves conflicts alone', async t => {
	const cwd = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-unborn-')); t.after(() => fs.rm(cwd, {recursive: true, force: true}));
	await git(cwd, 'init', '-b', 'main');
	await fs.mkdir(path.join(cwd, 'dir')); await fs.writeFile(path.join(cwd, 'dir', 'a b.txt'), 'x\n'); await fs.writeFile(path.join(cwd, 'c.txt'), 'y\n');
	await git(cwd, 'add', '-A');
	let snapshot = await readChanges(cwd);
	assert.equal(snapshot.hasHead, false); assert.deepEqual(await shape(cwd), ['staged:A:c.txt', 'staged:A:dir/a b.txt']);
	await applyStage(cwd, snapshot, 'unstage', {group: 'staged', path: 'dir/a b.txt'});
	assert.deepEqual(await shape(cwd), ['staged:A:c.txt', 'untracked:?:dir/a b.txt']);
	await applyStage(cwd, await readChanges(cwd), 'unstage');
	assert.deepEqual(await shape(cwd), ['untracked:?:c.txt', 'untracked:?:dir/a b.txt']);

	// A conflict: stage all stages the rest and reports the conflict; staging the file itself marks it resolved.
	const conflicted = await repo(); t.after(() => fs.rm(conflicted, {recursive: true, force: true}));
	await git(conflicted, 'checkout', '-b', 'other'); await fs.writeFile(path.join(conflicted, 'file.txt'), 'other\n'); await git(conflicted, 'commit', '-am', 'other');
	await git(conflicted, 'checkout', 'main'); await fs.writeFile(path.join(conflicted, 'file.txt'), 'main\n'); await git(conflicted, 'commit', '-am', 'main');
	await git(conflicted, 'merge', 'other').catch(() => {});
	await fs.writeFile(path.join(conflicted, 'extra.txt'), 'extra\n');
	snapshot = await readChanges(conflicted);
	assert.deepEqual(await shape(conflicted), ['conflicts:U:file.txt', 'untracked:?:extra.txt']);
	const diff = await readChangeDiff(conflicted, findChange(snapshot.all, 'conflicts', 'file.txt')!);
	assert.match(diff.text, /^diff --cc file.txt/m); assert.ok(diff.firstLine >= 1);
	assert.deepEqual(await applyStage(conflicted, snapshot, 'stage'), {changed: 1, skippedConflicts: 1});
	assert.deepEqual(await shape(conflicted), ['conflicts:U:file.txt', 'staged:A:extra.txt']);
	await applyStage(conflicted, await readChanges(conflicted), 'stage', {group: 'conflicts', path: 'file.txt'});
	assert.deepEqual(await shape(conflicted), ['staged:A:extra.txt', 'staged:M:file.txt']);
	await fs.access(path.join(conflicted, '.git', 'MERGE_HEAD')); // Unstaging/staging never ends the merge.
	await applyStage(conflicted, await readChanges(conflicted), 'unstage');
	await fs.access(path.join(conflicted, '.git', 'MERGE_HEAD'));
});
