/**
 * Gen script for src/tasks.ts. Regenerate with `node --import tsx parity/gen/tasks.gen.ts`.
 * Skipped: newTaskId (random). applyTaskOp is recorded only for inputs whose result involves no random ID
 * (every task already has `t=`, `add` carries an `id`), so the Rust port can match the text byte for byte.
 */
import fs from 'node:fs';
import {Recorder, arb, fc, installDeterminism, mutate, registerFn, runTap, sample} from '../lib.ts';
import {
	appendNoteItem, applyTaskOp, cleanTaskTitle, clientTaskOp, findNoteTaskLink, groupTasks, isLinked, keyOfLink, linkKey, linkMatches, linkOfKey,
	migrateFollowUps, noteTaskLinkLine, openNoteItems, openTaskCount, parseNoteTaskLink, parseTasks, promoteNoteLine, promoteOpenNoteLines,
	returnTaskToNote, taskPrompt, withoutTaskMeta,
} from '../../src/tasks.ts';

installDeterminism();
const recorder = new Recorder('tasks');
const rc = (name: string, fn: Function, args: unknown[]) => { try { recorder.call(name, fn, args); } catch { /* the throw is recorded */ } };

const TAPPED = ['cleanTaskTitle', 'parseTasks', 'linkMatches', 'isLinked', 'linkKey', 'keyOfLink', 'linkOfKey', 'applyTaskOp', 'clientTaskOp', 'groupTasks', 'openTaskCount',
	'taskPrompt', 'parseNoteTaskLink', 'noteTaskLinkLine', 'promoteNoteLine', 'findNoteTaskLink', 'returnTaskToNote', 'openNoteItems', 'appendNoteItem', 'withoutTaskMeta', 'migrateFollowUps'];
const allTapped = await runTap({tests: ['tests/tasks.test.ts'], tap: TAPPED.map(name => `tasks:${name}`)});
// The tests' subprocess draws random task IDs: keep only calls whose IDs the test file itself spells out.
const knownIds = new Set(fs.readFileSync(new URL('../../tests/tasks.test.ts', import.meta.url), 'utf8').match(/t=[0-9a-z]+/g));
const tapped = allTapped.filter(entry => (entry.line.match(/dh:t=[0-9a-z]+/g) ?? []).every(id => knownIds.has(id.slice(3))));
// applyTaskOp calls from the tests involve random IDs; their inputs are re-run below, filtered.
recorder.addTap(tapped.filter(entry => entry.fn !== 'applyTaskOp' && entry.fn !== 'migrateFollowUps'), {module: 'tasks'});
const tappedApply = tapped.filter(entry => entry.fn === 'applyTaskOp' || entry.fn === 'migrateFollowUps');

const NOW = new Date('2026-10-09T12:00:00Z');
const nowsAround = [NOW, new Date('2026-10-02T00:00:00Z'), new Date('2026-10-01T23:59:59.999Z'), new Date('2026-01-01T00:00:00Z'), new Date('2024-02-29T10:00:00Z')];

// A random-ID-free applyTaskOp call?
function deterministicSafe(args: unknown[]): boolean {
	const [text, op] = args as [unknown, any];
	if (typeof text !== 'string' || !op || typeof op !== 'object') return false;
	try { if (!parseTasks(text).every(task => task.meta.t)) return false; } catch { return false; }
	if (op.type === 'add' && !(typeof op.id === 'string' && op.id)) return false;
	return true;
}

const words = ['Fix stuck footer', 'a  b', '😀 emoji', '中文 title', 'é', 'x<!--y-->z', 'tab\there', ' sep ', 'a\u0000b', '  pad  ', '', ' ', 'long '.repeat(80), '<!-- dh:t=zz -->', 'a>b'];
const metas = [
	'', ' <!-- dh:t=aaaa1111 -->', ' <!-- dh:t=aaaa1111 wt=w1 added=2026-10-01 -->', ' <!-- dh:t=bbbb2222 s=sess1 done=2026-10-05 auto=merge -->',
	' <!-- dh:t=cccc3333 done=2026-09-01 auto=done tried=feat/x from=main -->', ' <!-- dh:t=dddd4444 auto=bogus wt= unknown=1 =x s -->',
	' <!-- dh:t=eeee5555 assigned=w1 -->', ' <!-- dh:t=ffff6666 was=main wt=w2 -->', '<!--dh:t=gggg7777-->', ' <!-- dh:t=hhhh8888 wt=w1\ts=s1 -->', ' <!-- other -->', ' <!-- dh:t=iiii9999 done=2026-10-08 -->',
];
const rawLines = ['# Tasks', '', 'Some prose.', '  - [ ] indented checklist', '1. [ ] numbered', 'Trailing prose', '- plain bullet', '- [ ]', '- [X] caps', '- [x] 中文 <!-- dh:t=jjjj0000 -->'];
const taskBlock = fc.record({
	bullet: fc.constantFrom('-', '*', '+'), done: fc.boolean(), title: fc.constantFrom(...words.slice(0, 9), 'T'), meta: fc.constantFrom(...metas),
	body: fc.array(fc.constantFrom('  detail', '    deeper', '\tTab', '  ', ' ', '  - [ ] sub', '  中文😀'), {maxLength: 3}),
});
const fileArb = fc.array(fc.oneof(taskBlock.map(b => [`${b.bullet} [${b.done ? 'x' : ' '}] ${b.title}${b.meta}`, ...b.body]), fc.constantFrom(...rawLines).map(l => [l])), {maxLength: 7})
	.chain(blocks => fc.constantFrom('\n', '\r\n', '').map(end => blocks.flat().join(end === '\r\n' ? '\r\n' : '\n') + (blocks.length ? end : '')));
const fileSamples = sample(fileArb, 200, 11);

const handFiles = [
	'', '\n', '# Tasks\n\nSome prose.\n- [ ] First <!-- dh:t=aaaa1111 wt=w1 added=2026-10-01 -->\n  detail one\n    deeper\n- [x] Second\n  - [ ] a sub-item stays body\nTrailing prose\n',
	'- [ ] A <!-- dh:t=a1 -->\n- [ ] B <!-- dh:t=b2 wt=w1 -->\n- [x] C <!-- dh:t=c3 wt=w1 done=2026-10-01 -->\n- [x] D <!-- dh:t=d4 s=s1 done=2026-10-02 auto=merge -->\n- [x] E <!-- dh:t=e5 wt=w1 done=2026-10-03 auto=done -->\n- [ ] F <!-- dh:t=f6 s=s1 -->\n',
	'- [ ] A <!-- dh:t=a1 -->\n  body\n\n  after blank\n- [ ] B\n', '- [x] only done <!-- dh:t=a1 done=2026-10-01 -->\n', 'no tasks at all', '- [ ] A <!-- dh:t=a1 -->\r\n  x\r\n- [ ] B <!-- dh:t=b2 -->',
	'- [ ] dup <!-- dh:t=same -->\n- [ ] dup2 <!-- dh:t=same -->\n',
];
const withIds = (t: string) => parseTasks(t).every(task => task.meta.t);
const idFiles = [...handFiles.filter(withIds), ...fileSamples.filter(withIds)];

// parseTasks
for (const text of [...handFiles, ...fileSamples]) rc('parseTasks', parseTasks, [text]);
for (const args of mutate(recorder.argsOf('parseTasks'), {variants: 3})) rc('parseTasks', parseTasks, args);
for (const text of sample(arb.text(40), 80, 12)) rc('parseTasks', parseTasks, [`- [ ] ${text}`]);
for (const text of sample(arb.text(40), 80, 13)) rc('parseTasks', parseTasks, [`- [ ] T ${text} <!-- dh:${text} -->\n${text}`]);

for (const title of [...words, ...sample(arb.text(60), 150, 14), 'x'.repeat(299) + '😀', 'x'.repeat(300), 'x'.repeat(301), ' nbsp ', ' em ']) rc('cleanTaskTitle', cleanTaskTitle, [title]);
for (const args of mutate(recorder.argsOf('cleanTaskTitle'), {variants: 3})) rc('cleanTaskTitle', cleanTaskTitle, args);

const metaSamples: unknown[] = [{}, {wt: 'w1'}, {s: 's1'}, {wt: 'w1', s: 's1'}, {t: 'x'}, {wt: ''}, {s: ''}, {wt: '', s: 's'}, {done: '2026-10-01', auto: 'merge'}];
const linkSamples: unknown[] = [{wt: 'w1'}, {s: 's1'}, {wt: 'w2'}, {s: 's2'}, {wt: ''}, {s: ''}, {wt: '中'}];
for (const meta of metaSamples) {
	rc('linkKey', linkKey, [meta]);
	rc('isLinked', isLinked, [{meta}]);
	for (const link of linkSamples) rc('linkMatches', linkMatches, [meta, link]);
}
for (const link of linkSamples) rc('keyOfLink', keyOfLink, [link]);
for (const key of ['wt:w1', 's:s1', 'wt:', 's:', 'wt', 's', '', 'x:y', 'wt:wt:', '中', 'wt:😀', 'S:a']) rc('linkOfKey', linkOfKey, [key]);

// applyTaskOp
const links = [{wt: 'w1'}, {s: 's1'}, {wt: 'w9'}, {s: 's9'}];
const ops: any[] = [];
for (const id of ['a1', 'b2', 'c3', 'd4', 'e5', 'f6', 'missing', 'aaaa1111', '~0:00000000', '']) {
	ops.push({type: 'toggle', id}, {type: 'remove', id}, {type: 'edit', id, title: 'New  title', body: 'new\nbody'}, {type: 'edit', id, title: '  ', body: ''}, {type: 'assign', id}, {type: 'move', id, target: 'a1'}, {type: 'move', id, target: 'f6', after: true}, {type: 'move', id, target: id});
	for (const link of links) ops.push({type: 'link', id, link}, {type: 'assign', id, link});
}
for (const link of links) for (const auto of ['merge', 'done']) ops.push({type: 'tick-linked', link, auto}, {type: 'reopen-linked', link, auto});
for (const link of links) ops.push({type: 'abandon-linked', link}, {type: 'abandon-linked', link, tried: 'feat/x'}, {type: 'abandon-linked', link, tried: ''});
for (const title of ['New', '  ', '', 'x<!--y-->', '中😀', 'x'.repeat(400)]) {
	for (const body of [undefined, '', 'b', '\n\nlead\n\n  ind\n\n', 'crlf\r\nx', 'x'.repeat(4100), 'ctl\u0001\u007Fz']) ops.push({type: 'add', title, id: 'n1', ...body !== undefined ? {body} : {}});
	ops.push({type: 'add', title, id: 'n1', link: links[0]}, {type: 'add', title, id: 'n1', from: 'main'}, {type: 'add', title, id: 'a1'}, {type: 'add', title, id: 'n1', link: links[1], from: 'main'});
}
let handSkipped = 0;
for (const text of handFiles.filter(withIds).slice(0, 8)) for (const op of ops) {
	const args = [text, op, NOW];
	if (!deterministicSafe(args)) continue;
	let fails = false;
	try { applyTaskOp(text, op, NOW); } catch { fails = true; }
	if (fails && handSkipped++ % 8) continue;
	rc("applyTaskOp", applyTaskOp, args);
}
let skipped = 0;
for (const [text, op, now] of sample(fc.tuple(fc.constantFrom(...idFiles), fc.constantFrom(...ops), fc.constantFrom(...nowsAround)),  4000, 15)) {
	const args = [text, op, now];
	if (!deterministicSafe(args)) continue;
	let fails = false;
	try { applyTaskOp(text, op, now); } catch { fails = true; }
	if (fails && skipped++ % 6) continue;
	rc('applyTaskOp', applyTaskOp, args);
}
// The tests' own inputs (decoded), with an explicit clock, and mutations of them.
const testInputs = tappedApply.filter(entry => entry.fn === 'applyTaskOp').map(entry => (JSON.parse(entry.line) as {args: any[]}).args).filter(a => typeof a[0] === 'string' && a[1] && typeof a[1] === 'object').map(a => [a[0], a[1], NOW]);
for (const args of [...testInputs, ...mutate(testInputs, {variants: 4})]) if (deterministicSafe(args)) rc('applyTaskOp', applyTaskOp, args);

// clientTaskOp
const rawOps: unknown[] = [
	null, undefined, 5, 'x', [], {}, {type: 'nope'}, {type: 'add'}, {type: 'add', title: 5}, {type: 'add', title: 'T'}, {type: 'add', title: 'T', body: 'B'}, {type: 'add', title: 'T', body: 5},
	{type: 'add', title: 'T', link: {wt: 'w1'}}, {type: 'add', title: 'T', link: {s: 's1'}}, {type: 'add', title: 'T', link: {}}, {type: 'add', title: 'T', link: null}, {type: 'add', title: 'T', link: {wt: ''}},
	{type: 'add', title: 'T', link: {wt: 'a', s: 'b'}}, {type: 'add', title: 'T', link: {wt: 1}}, {type: 'add', title: 'T', link: 'wt:a'}, {type: 'add', title: 'T', id: 'forged', from: 'x'},
	{type: 'assign', id: 'a'}, {type: 'assign', id: 'a', link: {wt: 'w'}}, {type: 'assign', id: 'a', link: {s: 's'}}, {type: 'assign', id: 'a', link: {}}, {type: 'assign', id: 1}, {type: 'assign'},
	{type: 'edit', id: 'a', title: 't', body: 'b'}, {type: 'edit', id: 'a', title: 't'}, {type: 'edit', id: 'a', body: 'b'}, {type: 'edit', title: 't', body: 'b'},
	{type: 'toggle', id: 'a'}, {type: 'toggle'}, {type: 'remove', id: 'a'}, {type: 'remove', id: ''}, {type: 'toggle', id: 'a', extra: 1},
	{type: 'move', id: 'a', target: 'b'}, {type: 'move', id: 'a', target: 'b', after: true}, {type: 'move', id: 'a', target: 'b', after: false}, {type: 'move', id: 'a', target: 'b', after: 'yes'}, {type: 'move', id: 'a'},
	{type: 'link', id: 'a', link: {wt: 'w'}}, {type: 'tick-linked', link: {wt: 'w'}, auto: 'merge'}, {type: 'abandon-linked'}, {type: 5},
];
for (const raw of rawOps) rc('clientTaskOp', clientTaskOp, [raw]);

// groupTasks / openTaskCount
const mkTask = (title: string, done: boolean, meta: object) => ({id: title, title, done, body: '', meta});
const taskSets: unknown[][] = [
	[], [mkTask('a', false, {}), mkTask('b', false, {wt: 'w'}), mkTask('c', false, {s: 's'})],
	[mkTask('d1', true, {done: '2026-10-09'}), mkTask('d2', true, {done: '2026-10-02'}), mkTask('d3', true, {done: '2026-10-01'}), mkTask('d4', true, {}), mkTask('d5', true, {done: '2026-10-08'}), mkTask('d6', true, {done: '2026-10-09'})],
	[mkTask('x', true, {done: '2026-10-02', wt: 'w'}), mkTask('y', false, {done: '2026-10-02'})],
];
for (const set of [...taskSets, ...handFiles.map(t => parseTasks(t)), ...fileSamples.slice(0, 30).map(t => parseTasks(t))]) {
	rc('openTaskCount', openTaskCount, [set]);
	for (const now of nowsAround) rc('groupTasks', groupTasks, [set, now]);
}

// taskPrompt
const bodiesP = ['', '  ', 'one', 'a\nb', ' a \n\n b ', 'a \r\n b', 'x y', '中\n😀', '\n'];
for (const title of ['T', '', '中', 'a: b']) for (const body of bodiesP) rc('taskPrompt', taskPrompt, [{title, body}]);
for (const [title, body] of sample(fc.tuple(arb.text(20), arb.text(40)), 100, 16)) rc('taskPrompt', taskPrompt, [{title, body}]);

// note links
const noteLines = [
	'- ↗ Title <!-- dh:t=abc123 -->', '  - ↗ Nested <!-- dh:t=abc123 -->', '\t* ↗  spaced   <!-- dh:t=0z -->  ', '+ ↗ x <!--dh:t=a-->', '- ↗ <!-- dh:t=a -->', '- ↗ T <!-- dh:t=ABC -->', '- ↗ T <!-- dh:t= -->',
	'- ↗ T', '- [ ] T', '↗ T <!-- dh:t=a -->', '- ↗ 中😀 <!-- dh:t=a1 -->', '- ↗ T <!-- dh:t=a1 wt=w -->', '', '- ↗ a <!-- dh:t=a1 --> trailing', '1. ↗ T <!-- dh:t=a1 -->',
];
for (const line of noteLines) { rc('parseNoteTaskLink', parseNoteTaskLink, [line]); rc('withoutTaskMeta', withoutTaskMeta, [line]); }
for (const line of sample(arb.text(40), 100, 17)) rc('withoutTaskMeta', withoutTaskMeta, [`${line} <!-- dh:${line} -->${line}<!--dh:x-->`]);
for (const indent of ['', '  ', '\t']) for (const title of ['T', '', '中😀', 'a <!-- b']) rc('noteTaskLinkLine', noteTaskLinkLine, [indent, title, 'abc']);
for (const line of noteLines) rc('parseNoteTaskLink', parseNoteTaskLink, [noteTaskLinkLine('  ', line, 'a1')]);

const notes = [
	'', '- [ ] a\n- [x] b\n  - [ ] nested\n- c\n', '# Note\n\n- [ ] one <!-- dh:t=x1 -->\n- [ ]\n- [ ] \n- [ ]  two  \n', '- ↗ linked <!-- dh:t=abc -->\n- [ ] open\n- ↗ other <!-- dh:t=def -->', '- [ ] 中😀 title\r\n- [ ] crlf',
	'- [ ] x<!--y-->z\n- [ ] ' + 'l'.repeat(400), '* [ ] star\n+ [X] plus done\n\t- [ ] tabbed',
];
for (const note of notes) {
	for (const line of [-1, 0, 1, 2, 3, 4, 5, 9]) {
		rc('promoteNoteLine', promoteNoteLine, [note, line, 'id1']);
		for (const task of [{title: 'T', body: ''}, {title: 'T', body: 'a\n\nb'}, {title: '中', body: '😀'}]) rc('returnTaskToNote', returnTaskToNote, [note, line, task]);
	}
	rc('openNoteItems', openNoteItems, [note]);
	for (const [title, body] of [['T', undefined], ['T', ''], ['  <!--x--> T ', 'a\n\nb'], ['', 'x'], ['中😀', '  \n'], ['T', '\n lead']] as const) rc('appendNoteItem', appendNoteItem, body === undefined ? [note, title] : [note, title, body]);
}
rc('openNoteItems', openNoteItems, [undefined]);
for (const id of ['abc', 'def', 'x', '', 'a1']) rc('findNoteTaskLink', findNoteTaskLink, [notes[3], id]);
for (const note of fileSamples.slice(0, 60)) { for (const line of [0, 1, 2]) rc('promoteNoteLine', promoteNoteLine, [note, line, 'z']); rc('openNoteItems', openNoteItems, [note]); }
for (const [text, line] of sample(fc.tuple(arb.text(40), fc.nat(3)), 80, 19)) {
	rc('promoteNoteLine', promoteNoteLine, [`- [ ] ${text}\n- [x] ${text}\n${text}`, line, 'i']);
	rc('appendNoteItem', appendNoteItem, [text, text, text]);
}
// returnTaskToNote on linked lines, with indent
const linkedNote = '- ↗ T <!-- dh:t=a1 -->\n  - ↗ N <!-- dh:t=b2 -->\n\t* ↗ M <!-- dh:t=c3 -->';
for (const line of [0, 1, 2, 3]) for (const task of [{title: 'T', body: ''}, {title: 'T', body: 'a\n\nb'}, {title: '中', body: '😀\n'}]) rc('returnTaskToNote', returnTaskToNote, [linkedNote, line, task]);

// promoteOpenNoteLines (function argument: a per-call id counter, registered as "seq")
for (const note of [...notes, ...fileSamples.slice(0, 40)]) {
	let n = 0;
	const next = registerFn('seq', () => `id${++n}`);
	rc('promoteOpenNoteLines', promoteOpenNoteLines, [note, next]);
}

// migrateFollowUps (function argument: hasNote)
const hasAll = registerFn('hasAll', () => true);
const hasNone = registerFn('hasNone', () => false);
const hasW1 = registerFn('hasW1', (link: any) => link.wt === 'w1');
const legacy = [
	'- [ ] A <!-- dh:t=a1 assigned=w1 -->\n  body\n- [ ] B <!-- dh:t=b2 assigned=w2 wt=w2 -->\n- [x] C <!-- dh:t=c3 assigned=w1 done=2026-10-01 -->\n- [ ] D <!-- dh:t=d4 was=main -->\n- [ ] E <!-- dh:t=e5 s=s1 assigned=s1 -->\n',
	'# T\n- [ ] A <!-- dh:t=a1 -->\n', '- [ ] A <!-- dh:t=a1 assigned=w1 -->\n- [ ] B <!-- dh:t=b2 assigned= -->\n', '- [ ] x <!-- dh:t=a1 assigned=w1 -->\n  - [ ] sub\n    deep\n', 'prose assigned=x\n- [ ] A\n',
	'- [ ] 中😀 <!-- dh:t=a1 wt=w1 assigned=w1 -->\n  x\n\n  y\n- [ ] k <!-- dh:t=k1 was=w1 wt=w1 -->\r\n', '', '- [ ] A <!-- dh:t=a1 assigned=w1 -->', '- [ ] A <!-- dh:t=a1 assigned=nowork -->\n',
	...fileSamples.filter(t => /assigned=|was=/.test(t)),
];
for (const text of legacy) for (const fn of [hasAll, hasNone, hasW1]) rc('migrateFollowUps', migrateFollowUps, [text, fn]);
for (const args of mutate(legacy.map(t => [t, hasAll]), {variants: 3})) rc('migrateFollowUps', migrateFollowUps, args);

await recorder.write({prune: true});
