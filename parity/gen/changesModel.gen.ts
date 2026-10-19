/** Fixtures for src/changesModel.ts. Regenerate: node --import tsx parity/gen/changesModel.gen.ts */
import {Recorder, arb, fc, installDeterminism, mutate, runTap, sample} from '../lib.ts';
import * as m from '../../src/changesModel.ts';

const MODULE = 'changesModel';
installDeterminism();
const recorder = new Recorder(MODULE);

const FNS = ['parseNumstat', 'groupChanges', 'changeKey', 'findChange', 'totalChanges', 'changeRows', 'reselect', 'groupOffset', 'stageMode', 'changeLabel', 'lineCountsText', 'firstChangedLine', 'classifyDiff', 'untrackedDiff', 'emptyCounts', 'emptyChanges'];
const fns = m as unknown as Record<string, (...args: unknown[]) => unknown>;
const call = (name: string, args: unknown[]) => { try { recorder.call(name, fns[name]!, args); } catch { /* throws are recorded */ } };

// 1. Tap the existing tests.
recorder.addTap(await runTap({tests: ['tests/changesModel.test.ts'], tap: FNS.map(fn => `changesModel:${fn}`)}), {module: MODULE});

// 2. Mutations of tapped inputs.
for (const name of FNS) for (const args of mutate(recorder.argsOf(name), {variants: 4})) call(name, args);

// 3. Hand-written edge cases.
call('emptyCounts', []);
call('emptyChanges', []);
call('emptyChanges', [{}]);
call('emptyChanges', [{loaded: true, branch: 'main', error: 'boom', workspace: '/w'}]);
for (const raw of ['', '\0', '1\t2\tfile\0', '-\t-\tbin.png\0', '1\t2\t\0old\0new\0', '1\t2\t\0old\0', '1\t2\t\0', '3\t4\ta\0 5\t6\tb\0', '1\t2\ta\0 3\t4\ta\0', '1\t2\ta\0' + '5\t6\ta\0', 'x\ty\tz\0', '1\t2\tdir/名前😀.ts\0', '1\t2\ta\tb\nc\0', '01\t002\tz\0', '-\t2\tmixed\0', '1\t2\t\0a\0b\0' + '7\t8\tafter\0'])
	call('parseNumstat', [raw]);

const xys = ['M.', '.M', 'MM', 'A.', '.D', 'D.', 'R.', 'C.', 'RM', 'T.', '..', '?', '', 'M', 'AM', 'UU', 'AA', 'DU', 'UD'];
const paths = ['a.ts', 'b/c.ts', 'z', 'ä', '名前.md', '😀.txt', 'B', 'a', 'dir/', 'a b'];
const entryArb = fc.record({
	kind: fc.constantFrom('1', '2', 'u', '?'),
	xy: fc.constantFrom(...xys),
	path: fc.constantFrom(...paths),
	origPath: fc.option(fc.constantFrom('old.ts', 'x/y.ts', 'old/'), {nil: undefined}),
}, {requiredKeys: ['kind', 'xy', 'path']});
const numstatArb = fc.option(fc.array(fc.tuple(fc.constantFrom(...paths), fc.oneof(
	fc.record({additions: fc.nat(100), deletions: fc.nat(100)}),
	fc.constant({binary: true}))), {maxLength: 6}).map(pairs => new Map(pairs)), {nil: undefined});
const numArb = fc.record({staged: numstatArb, unstaged: numstatArb, untracked: numstatArb}, {requiredKeys: []});
for (const [entries, numstat, cap] of sample(fc.tuple(fc.array(entryArb, {maxLength: 12}), numArb, fc.oneof(fc.constant(undefined), fc.integer({min: -2, max: 14}))), 200, 11)) {
	call('groupChanges', cap === undefined ? [entries, numstat] : [entries, numstat, cap]);
}
call('groupChanges', [[]]);
call('groupChanges', [[], {}, 0]);
call('groupChanges', [[{kind: '?', xy: '?', path: 'u'}, {kind: '1', xy: 'MM', path: 'a'}, {kind: 'u', xy: 'UU', path: 'c'}], {}, 2]);

// Records for the row/reselect functions come from groupChanges outputs.
const records = sample(fc.tuple(fc.array(entryArb, {maxLength: 10}), fc.integer({min: 0, max: 8})), 80, 13).map(([entries, cap]) => m.groupChanges(entries as never, {}, cap));
for (const r of records) {
	call('changeRows', [r]);
	call('totalChanges', [r.counts]);
	r.entries.forEach((entry, index) => { call('changeKey', [entry]); call('groupOffset', [r.entries, index]); call('stageMode', [entry]); call('changeLabel', [entry]); call('findChange', [r.entries, entry.group, entry.path]); });
	call('groupOffset', [r.entries, r.entries.length]);
	call('groupOffset', [r.entries, -1]);
	call('findChange', [r.entries, 'staged', 'nope']);
	const keys = [undefined, ...r.entries.map(m.changeKey), 'staged\0gone', 'conflicts\0gone', 'untracked\0gone', 'bogus', ''];
	for (const key of keys) for (const [index, offset] of [[0, undefined], [3, 1], [-1, 0], [99, 5], [2, -3]] as const) {
		call('reselect', [r.entries, {...(key === undefined ? {} : {key}), index, ...(offset === undefined ? {} : {offset})}]);
	}
}
call('reselect', [[], {index: 0}]);
call('totalChanges', [m.emptyCounts()]);

for (const entry of [
	{path: 'a'}, {path: 'a/'}, {path: 'x/y/z.ts'}, {path: '/abs'}, {path: 'dir/sub/'}, {path: '名前/😀.ts', origPath: '名前/old.ts'},
	{path: 'a/b', origPath: 'a/c'}, {path: 'a/b', origPath: 'c/b'}, {path: 'a', origPath: ''}, {path: 'new/', origPath: 'old/'}, {path: '', origPath: ''}, {path: 'x', origPath: 'x'},
]) call('changeLabel', [entry]);
for (const counts of [{}, {binary: true}, {binary: true, additions: 1, deletions: 2}, {additions: 1}, {deletions: 1}, {additions: 0, deletions: 0}, {additions: 12, deletions: 3}, {additions: 1.5, deletions: 1e21}]) call('lineCountsText', [counts]);

const diffs = [
	'', '\n', 'no hunk\n', '@@ -1 +1 @@\n+x', '@@ -1,2 +10,2 @@\n a\n b\n-c\n+d', '@@ -1,2 +10,2 @@\n a\n b\n c', '@@ -1,2 +5 @@\n \\ No newline\n x\n+y',
	'diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n@@ -3,3 +3,3 @@ ctx\n a\n b\n-c\n+d\n', 'diff --git a/f b/f\n@@ -1 +1 @@\n a\n@@ -9 +9 @@\n-b\n',
	'diff --cc f\n@@@ -1,3 -1,3 +1,3 @@@\n  a\n +b\n- c\n++d\n', 'diff --cc f\n@@@ -1,3 -1,3 +1,3 @@@\n  a\n  b\n  c\n', '@@ -0,0 +1,2 @@\n+a\n+b', '@@@ -1 -1 +7 @@@\n  x\n  y\n++z', '@@ -1 +1 @@\r\n a\r\n+b\r\n',
	'@@ -1 +1 @@\n名前\n😀\n+x', '--- a\n+++ b\n@@ -1 +1 @@\n-old\n+new\n', 'diff a\n@@ -1 +1 @@\n x\ndiff b\n@@ -1 +4 @@\n-y\n', '@@ -1 +0 @@\n x\ndiff --git a b\n x', '@@ -1 +1 @@\n x\n\n\n',
];
for (const d of diffs) { call('firstChangedLine', [d]); call('classifyDiff', [d]); }
for (const d of sample(fc.array(fc.oneof(
	fc.constantFrom('@@ -1,2 +3,4 @@', '@@@ -1 -1 +2 @@@', 'diff --git a/x b/x', 'index 1..2', '--- a', '+++ b', '\\ No newline at end of file', ' ctx', '+add', '-del', '++both', '+-mix', '  c', '', '名前', '😀'),
	arb.text(8)), {maxLength: 12}).map(lines => lines.join('\n') + (lines.length % 2 ? '\n' : '')), 200, 17)) { call('firstChangedLine', [d]); call('classifyDiff', [d]); }
for (const c of ['', '\n', 'a', 'a\n', 'a\nb', 'a\nb\n', '\n\n', 'a\n\n', '\r\n', 'a\r\nb\r\n', '名前\n😀\n', '+x\n-y', '@@ -1 +1 @@']) call('untrackedDiff', [c]);
for (const c of sample(arb.text(30), 100, 19)) call('untrackedDiff', [c]);

await recorder.write({prune: true});
