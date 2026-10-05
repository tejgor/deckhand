import assert from 'node:assert/strict';
import {test} from 'node:test';
import {changeKey, changeLabel, changeRows, classifyDiff, firstChangedLine, groupChanges, groupOffset, lineCountsText, parseNumstat, reselect, stageMode, untrackedDiff} from '../src/changesModel.js';
import {parseStatus} from '../src/workspaceGit.js';

// Porcelain v2 -z: an ordinary change (partially staged), a staged rename with spaces, a staged deletion, an
// intent-to-add file, a conflict and untracked files.
const STATUS = [
	'# branch.oid abc1234', '# branch.head feature',
	'1 MM N... 100644 100644 100644 aaa bbb src/app.ts',
	'2 R. N... 100644 100644 100644 aaa aaa R100 docs/new name.md', 'docs/old name.md',
	'1 D. N... 100644 000000 000000 aaa 000 gone.txt',
	'1 .A N... 000000 000000 100644 000 000 intent.txt',
	'u UU N... 100644 100644 100644 100644 a b c both.txt',
	'? z-last.txt', '? a first.bin', '',
].join('\0');

test('Changes groups follow VS Code: conflicts, staged, unstaged, untracked; partially staged files are in both', () => {
	const status = parseStatus(STATUS);
	const staged = parseNumstat(['3\t1\tsrc/app.ts', '0\t0\t', 'docs/old name.md', 'docs/new name.md', '0\t4\tgone.txt', ''].join('\0'));
	const unstaged = parseNumstat(['2\t0\tsrc/app.ts', '0\t0\tboth.txt', '5\t2\tboth.txt', '0\t0\tintent.txt', ''].join('\0'));
	const untracked = new Map([['a first.bin', {binary: true}], ['z-last.txt', {additions: 7, deletions: 0}]]);
	const {entries, counts, omitted} = groupChanges(status.entries, {staged, unstaged, untracked});
	assert.deepEqual(counts, {conflicts: 1, staged: 3, unstaged: 2, untracked: 2});
	assert.deepEqual(omitted, {conflicts: 0, staged: 0, unstaged: 0, untracked: 0});
	assert.deepEqual(entries.map(entry => [entry.group, entry.status, entry.path]), [
		['conflicts', 'U', 'both.txt'],
		['staged', 'R', 'docs/new name.md'], ['staged', 'D', 'gone.txt'], ['staged', 'M', 'src/app.ts'],
		['unstaged', 'A', 'intent.txt'], ['unstaged', 'M', 'src/app.ts'],
		['untracked', '?', 'a first.bin'], ['untracked', '?', 'z-last.txt'],
	]);
	const [conflict, rename, , stagedApp, , unstagedApp, binary, text] = entries;
	assert.equal(conflict!.conflict, 'UU'); assert.equal(conflict!.additions, 5); // A conflict's combined numstat: its last record.
	assert.equal(rename!.origPath, 'docs/old name.md'); assert.equal(rename!.additions, 0);
	assert.deepEqual([stagedApp!.additions, stagedApp!.deletions, unstagedApp!.additions, unstagedApp!.deletions], [3, 1, 2, 0]);
	assert.equal(lineCountsText(binary!), 'bin'); assert.equal(lineCountsText(text!), '+7 −0'); assert.equal(lineCountsText({}), '');
	assert.equal(unstagedApp!.origPath, undefined);
	assert.equal(stageMode(stagedApp!), 'unstage'); assert.equal(stageMode(unstagedApp!), 'stage'); assert.equal(stageMode(conflict!), 'stage'); assert.equal(stageMode(text!), 'stage');
});

test('the cap keeps group order, full counts and "+N more" rows', () => {
	const status = parseStatus(['1 M. N... 1 1 1 a a s1', '1 M. N... 1 1 1 a a s2', '1 .M N... 1 1 1 a a u1', '? n1', '? n2', ''].join('\0'));
	const capped = groupChanges(status.entries, {}, 3);
	assert.deepEqual(capped.counts, {conflicts: 0, staged: 2, unstaged: 1, untracked: 2});
	assert.deepEqual(capped.omitted, {conflicts: 0, staged: 0, unstaged: 0, untracked: 2});
	assert.deepEqual(changeRows(capped).map(row => row.kind === 'entry' ? row.entry.path : `${row.kind}:${row.group}:${row.count}`), ['header:staged:2', 's1', 's2', 'header:unstaged:1', 'u1', 'header:untracked:2', 'more:untracked:2']);
	assert.deepEqual(changeRows(groupChanges([], {})), []);
});

test('selection stays on the same group and path across refreshes, else the same position', () => {
	const before = groupChanges(parseStatus(['1 .M N... 1 1 1 a a a', '1 .M N... 1 1 1 a a b', '1 .M N... 1 1 1 a a c', ''].join('\0')).entries).entries;
	assert.equal(reselect(before, {key: changeKey(before[1]!), index: 1}), 1);
	assert.equal(groupOffset(before, 2), 2);
	// b was staged: it left Changes, so the selection moves to what is now at its position there (c), not to the
	// staged b that took its overall position.
	const after = groupChanges(parseStatus(['1 M. N... 1 1 1 a a b', '1 .M N... 1 1 1 a a a', '1 .M N... 1 1 1 a a c', ''].join('\0')).entries).entries;
	assert.equal(after[reselect(after, {key: 'unstaged\0b', index: 1, offset: 1})]!.path, 'c');
	assert.equal(after[reselect(after, {key: 'unstaged\0c', index: 0})]!.path, 'c');
	// The last of a group goes to the new last one; an emptied group falls back to the overall position.
	assert.equal(after[reselect(after, {key: 'unstaged\0z', index: 5, offset: 4})]!.path, 'c');
	// An emptied group: the first entry below it, else the last entry; an unknown key keeps the position.
	assert.equal(reselect(after, {key: 'conflicts\0gone', index: 2}), 0);
	assert.equal(reselect(after, {key: 'untracked\0gone', index: 0}), after.length - 1);
	assert.equal(reselect(after, {key: 'other', index: 99}), after.length - 1);
	assert.equal(groupOffset(after, 2), 1);
	assert.equal(reselect([], {index: 0}), -1);
});

test('labels put the name first; renames read old → new', () => {
	assert.deepEqual(changeLabel({path: 'src/ui/app.tsx'}), {name: 'app.tsx', dir: 'src/ui'});
	assert.deepEqual(changeLabel({path: 'top level.md'}), {name: 'top level.md', dir: ''});
	assert.deepEqual(changeLabel({path: 'docs/new name.md', origPath: 'docs/old name.md'}), {name: 'old name.md → new name.md', dir: 'docs'});
	assert.deepEqual(changeLabel({path: 'b/x.ts', origPath: 'a/x.ts'}), {name: 'a/x.ts → b/x.ts', dir: ''});
	assert.deepEqual(changeLabel({path: 'vendor/nested/'}), {name: 'nested/', dir: 'vendor'});
});

test('first changed line: the new-side line of the first hunk\'s first change', () => {
	const unified = ['diff --git a/f b/f', 'index 1..2 100644', '--- a/f', '+++ b/f', '@@ -10,7 +12,8 @@ function x() {', ' one', ' two', ' three', '-old', '+new', ' four', '@@ -40 +41 @@', '-x', '+y'].join('\n');
	assert.equal(firstChangedLine(unified), 15);
	assert.equal(firstChangedLine(['@@ -0,0 +1,2 @@', '+a', '+b'].join('\n')), 1);
	assert.equal(firstChangedLine(['@@ -1,3 +0,0 @@', '-a'].join('\n')), 1); // Everything deleted.
	assert.equal(firstChangedLine(['@@ -3,2 +3 @@', ' keep', '\\ No newline at end of file', '-gone'].join('\n')), 4);
	// A conflict's combined diff: two prefix columns, the new side is the last range.
	assert.equal(firstChangedLine(['diff --cc both.txt', '@@@ -1,2 -1,2 +1,6 @@@', '  x', '++<<<<<<< HEAD', ' +main'].join('\n')), 2);
	assert.equal(firstChangedLine('Binary files a/x and b/x differ'), 1);
	assert.equal(firstChangedLine(''), 1);
});

test('diff lines are classified for colour; header lines only before the first hunk', () => {
	const kinds = (text: string) => classifyDiff(text).map(line => line.kind);
	assert.deepEqual(kinds(['diff --git a/f b/f', 'new file mode 100644', '--- /dev/null', '+++ b/f', '@@ -0,0 +1,3 @@', '+a', '--- not a header', ' ctx', '\\ No newline at end of file'].join('\n')),
		['meta', 'meta', 'meta', 'meta', 'hunk', 'add', 'del', 'context', 'context']);
	assert.deepEqual(kinds(['diff --cc both.txt', 'index 1,2..0', '@@@ -1,2 -1,2 +1,6 @@@', '  x', '++<<<<<<< HEAD', ' +main', '+ other', ' -gone'].join('\n')),
		['meta', 'meta', 'hunk', 'context', 'add', 'add', 'add', 'del']);
	const untracked = untrackedDiff('one\ntwo\n');
	assert.equal(untracked, '@@ -0,0 +1,2 @@\n+one\n+two');
	assert.deepEqual(kinds(untracked), ['hunk', 'add', 'add']);
	assert.equal(untrackedDiff(''), '@@ -0,0 +1,0 @@');
});
