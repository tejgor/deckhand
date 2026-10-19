/** Regenerate: node --import tsx parity/gen/notes.gen.ts  (writes fixtures/parity/notes/) */
import {Recorder, arb, fc, installDeterminism, mutate, registerFn, runTap, sample} from '../lib.ts';
import * as notes from '../../src/notes.ts';

const MODULE = 'notes';
installDeterminism();
const recorder = new Recorder(MODULE);

const FNS = ['noteRevision', 'repoNoteId', 'sharedNoteIdentity', 'showsOwnNote', 'mergeIntoWorktreeNote', 'noteKey', 'parseChecklistLine', 'checklistCounts', 'toggleChecklistLine', 'taskLinkSuffix', 'linkedTaskId', 'toggleChecklist', 'insertChecklistItem', 'continueChecklist', 'visibleText', 'noteReadRows', 'fitReadRows', 'budgetSections', 'checklistLabel', 'fitFirst', 'openChecklistText'];

// Link lookups are closures in TS; the replay dispatches on these registered names.
const linkOpen = registerFn('linkOpen', () => 'open' as const);
const linkDone = registerFn('linkDone', () => 'done' as const);
const linkGone = registerFn('linkGone', () => 'gone' as const);
const linkById = registerFn('linkById', (id: string) => (/^[0-9]/.test(id) ? 'done' : /^[a-m]/.test(id) ? 'open' : 'gone') as 'open' | 'done' | 'gone');
const lookups = [linkOpen, linkDone, linkGone, linkById, undefined];

const call = (name: string, args: unknown[]) => { try { recorder.call(name, (notes as any)[name], args); } catch {} };
const callAll = (name: string, list: unknown[][]) => { for (const args of list) call(name, args); };

// 1. Tap the existing tests, keeping only calls whose function arguments the replay can dispatch on.
const tapped = await runTap({tests: ['tests/notes.test.ts', 'tests/noteSteps.test.ts'], tap: FNS.map(fn => `notes:${fn}`)});
const known = new Set(['linkOpen', 'linkDone', 'linkGone', 'linkById']);
const usable = tapped.filter(entry => {
	const text = JSON.stringify(JSON.parse(entry.line).args);
	return [...text.matchAll(/"\$fn":"([^"]*)"/g)].every(m => known.has(m[1]!));
});
recorder.addTap(usable, {module: 'notes'});

// Cursor must be on a code-point boundary (Rust cursors are bytes).
const boundary = (text: string, cursor: number) => !(cursor > 0 && cursor < text.length && /[\uD800-\uDBFF]/.test(text[cursor - 1]!) && /[\uDC00-\uDFFF]/.test(text[cursor]!));
const stateOk = ([state]: any[]) => typeof state?.text === 'string' && Number.isInteger(state.cursor) && state.cursor >= 0 && state.cursor <= state.text.length && boundary(state.text, state.cursor);

// 2. Hand-written edge cases.
const ROOTS = ['/', '/tmp/repo', '/tmp/repo/', '/tmp/a/../repo', '/tmp//repo', '/Users/me/proj 中文', '/tmp/😀', '/tmp/repo/.'];
for (const root of ROOTS) call('repoNoteId', [root]);
for (const text of ['', 'a', 'hello\n', '中文😀', 'a\r\nb', '\ud800']) call('noteRevision', [text]);

const wt = (extra: object = {}) => ({id: 'w1', path: '/tmp/wt', mode: 'worktree', ...extra});
const sessions: any[] = [
	{cwd: '/tmp/repo'},
	{cwd: '/tmp/repo', launchWorktreeRoot: '/tmp/root'},
	{cwd: '/tmp/repo', worktree: wt()},
	{cwd: '/tmp/repo', worktree: wt({deletedAt: '2026-01-01T00:00:00.000Z'})},
	{cwd: '/tmp/repo', worktree: wt({id: ''})},
	{cwd: '/tmp/repo', worktree: wt({path: ''})},
	{cwd: '/tmp/repo', worktree: wt({mode: 'none', path: '/tmp/x'})},
	{cwd: '/tmp/repo', requestedWorktreeMode: 'worktree'},
	{cwd: '/tmp/repo', requestedWorktreeMode: 'none'},
	{cwd: '/tmp/repo/', launchWorktreeRoot: '/tmp/中/'},
];
for (const s of sessions) call('sharedNoteIdentity', [s]);

const own: any[] = [
	{}, {worktree: wt()}, {worktree: wt(), notes: '  \n'}, {worktree: wt(), notes: 'x'}, {worktree: wt(), notesFile: {tooLarge: true}},
	{worktree: wt(), notesFile: {tooLarge: false}}, {worktree: wt({id: ''})}, {notes: ''}, {worktree: wt(), notes: '　'},
];
for (const s of own) call('showsOwnNote', [s]);

const merges: [string, Array<{title: string; text: string}>][] = [
	['', []], ['base', []], ['', [{title: 'T', text: 'note'}]], ['  ', [{title: 'T', text: 'note \n\n'}]], ['base\n\n', [{title: 'T', text: 'note'}]],
	['', [{title: 'A', text: 'a'}, {title: 'B', text: 'b'}]], ['w', [{title: 'A', text: '  '}, {title: 'B', text: 'b\n'}]],
	['', [{title: '中', text: '😀\t '}]], ['x', [{title: 'A', text: ''}]], ['\n', [{title: 'A', text: 'a'}]], ['', [{title: 'A', text: '  lead'}]],
];
for (const m of merges) call('mergeIntoWorktreeNote', m);

for (const [s, section] of [[{id: 's1'}, 'session'], [{id: 's1'}, 'shared'], [{id: 's1', sharedNotes: {kind: 'repo', id: 'abc', text: ''}}, 'shared'], [{id: 's1', sharedNotes: {kind: 'worktree', id: 'w'}}, 'shared'], [{id: '😀', sharedNotes: {kind: 'repo', id: 'q'}}, 'session']] as const) call('noteKey', [s, section]);

const LINES = ['- [ ] a', '- [x] a', '- [X] a', '* [ ] a', '+ [x] a', '  - [ ] indented', '\t- [ ] tab', ' \t* [x]', '- [ ]', '- [ ]x', '- [ ] ', '-[ ] a', '- [] a', '- [y] a', '- [ ]\ta', '-  [ ] a', '- \t[x]\tb', '1. [ ] a', '- ↗ task <!-- dh:t=ab12 -->', '', '- [ ] 中文😀', '- [x]  two gaps', '- [ ] a', '[ ] a', '-\n[ ] a'];
for (const line of LINES) call('parseChecklistLine', [line]);

const NOTES = [
	'', '- [ ] a\n- [x] b\n- [X] c', 'plain\n- bullet\n  * [ ] nested\n\n# Heading', '- [ ] 中文😀\r\n- [x] crlf', '\n\n- [ ] a\n\n', '- [ ]\n- [x]\n- [ ]x', '# H\n## H2\n#nohash\n####### seven\n###### six',
	'- ↗ Send it <!-- dh:t=abc1 -->\n- ↗ Other <!-- dh:t=9z -->\n- ↗ bare\n  * ↗ nested <!-- dh:t=m1 -->\n- ↗  <!-- dh:t=q -->', 'tab\there\u0001ctl\u007f\u0085 sep end', '- [ ] very long item that needs to wrap across several rows when the width is small, yes really',
];
for (const text of NOTES) {
	call('checklistCounts', [text]); call('checklistLabel', [text]); call('visibleText', [text]);
	for (const line of [-1, 0, 1, 2, 3, 99, 1.5]) call('toggleChecklistLine', [text, line]);
	for (const width of [-3, 0, 1, 2, 5, 8, 10, 12, 20, 40, 80]) for (const l of lookups) call('noteReadRows', l ? [text, width, l] : [text, width]);
}
call('checklistCounts', [undefined]); call('checklistLabel', [undefined]);

for (const line of ['', 'abc', '- [ ] x ↗', '<!-- dh:t=abc -->', '- ↗ t <!-- dh:t=ab12 -->', '<!--dh:t=Z9-->', '<!--  \tdh:t=0a-->', '<!-- dh:t= -->', '<!-- dh:p=1 dh:t=x -->', 'dh:t=abc', '<!-- dh:t=abc --> <!-- dh:t=def -->', '<!-- dh:t=😀 -->']) call('linkedTaskId', [line]);
for (const state of [undefined, 'open', 'done', 'gone']) call('taskLinkSuffix', [state]);

const rows = (n: number) => Array.from({length: n}, (_, i) => ({text: `row ${i}`, kind: (['open', 'done', 'heading', 'link', undefined] as const)[i % 5]}));
for (const n of [0, 1, 2, 5]) for (const h of [-1, 0, 1, 2, 3, 5, 6, 100, 2.5]) call('fitReadRows', [rows(n), h]);

for (const total of [-2, 0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 20, 30, 7.9]) for (const shared of [undefined, 0, 1, 2, 3, 5, 10, 40]) for (const session of [0, 1, 3, 4, 9, 50]) for (const focus of [undefined, 'shared', 'session'] as const) call('budgetSections', [total, shared, session, focus]);

for (const [cands, w] of [[[], 5], [[], 0], [['abc'], 5], [['abcdef', 'abc'], 4], [['abcdef', 'abcdefg'], 3], [['中文中文', 'x'], 3], [['😀😀😀'], 3], [['😀😀😀', '😀😀'], 3], [['a'], 0], [['abcdef'], 1]] as const) call('fitFirst', [[...cands], w]);

const sess: any[] = [{}, {notes: ''}, {notes: '- [ ] a'}, {notes: '- [x] a'}, {notes: '- [ ] a\n- [ ] b', sharedNotes: {kind: 'repo', id: 'r', text: '- [ ] c'}}, {sharedNotes: {kind: 'worktree', id: 'w', text: '- [ ] c\n- [ ] d\n- [x] e'}}, {notes: '- [ ] a', sharedNotes: {kind: 'repo', id: 'r'}}, {notes: '- [ ] a', sharedNotes: {kind: 'repo', id: 'r', text: Array(12).fill('- [ ] z').join('\n')}}];
for (const s of sess) for (const w of [-1, 0, 1, 2, 3, 6, 8, 11, 14, 16, 20, 25, 40]) call('openChecklistText', [s, w]);

// 3. Editor-state functions on interesting fixed states.
const STATES = ['', '- [ ] a', '- [x] a', '* [X] b\n', '  - [ ] ind\n- [ ]\n', '- plain', '  + bullet', 'plain', '\t  spaced', '\n\n', '- [ ] 中文😀\n- [x] é', '- [ ]', '- [ ] \n- [ ]x', '1. num', '- [ ] a\r\n- [ ] b'];
const fixed: unknown[][] = [];
for (const text of STATES) {
	const stops = new Set<number>([0, text.length]);
	let i = 0;
	for (const ch of text) { stops.add(i); i += ch.length; stops.add(i); }
	for (const cursor of stops) fixed.push([{text, cursor}]);
}
const EDITORS = ['toggleChecklist', 'insertChecklistItem', 'continueChecklist'];
for (const name of EDITORS) callAll(name, fixed.filter(stateOk));
// Out-of-range cursors (clamping behaviour of lineStart/lineEnd).
for (const name of EDITORS) for (const args of [[{text: 'abc', cursor: 99}], [{text: '- [ ] a', cursor: 99}], [{text: 'x', cursor: -1}]]) call(name, args);

// 4. fast-check samples (fixed seeds) and mutations.
const CK = ['- [ ] ', '- [x] ', '* [X] ', '+ [ ] ', '  - [ ] ', '\t* [x] ', '- ', '', '  ', '# ', '↗ ', '<!-- dh:t=a1 -->', 'a', '中', '😀', 'é', ' ', '\t', '-', '[', ']', 'x'];
const note = fc.array(fc.array(fc.constantFrom(...CK), {maxLength: 5}).map(p => p.join('')), {maxLength: 5}).map(l => l.join('\n'));
const noteCursor = note.chain(text => {
	const stops = [0];
	for (const ch of text) stops.push(stops.at(-1)! + ch.length);
	return fc.tuple(fc.constant(text), fc.constantFrom(...stops));
});
for (const [text, cursor] of sample(noteCursor, 250, 11)) for (const name of EDITORS) call(name, [{text, cursor}]);
for (const text of sample(note, 120, 12)) {
	call('checklistCounts', [text]); call('checklistLabel', [text]); call('visibleText', [text]);
	call('parseChecklistLine', [text.split('\n')[0]]);
}
for (const [text, line, width, k] of sample(fc.tuple(note, fc.integer({min: 0, max: 5}), fc.integer({min: 0, max: 30}), fc.nat(4)), 150, 13)) {
	call('toggleChecklistLine', [text, line]);
	call('noteReadRows', lookups[k] ? [text, width, lookups[k]] : [text, width]);
}
for (const text of sample(arb.text(30), 80, 14)) { call('noteRevision', [text]); call('visibleText', [text]); call('noteReadRows', [text, 10]); call('linkedTaskId', [text]); }
for (const [text, width] of sample(fc.tuple(arb.text(30), fc.integer({min: 1, max: 12})), 80, 15)) call('fitFirst', [[text, text.slice(0, 4)], width]);
for (const [a, b, c, d] of sample(fc.tuple(fc.integer({min: 0, max: 40}), fc.option(fc.integer({min: 0, max: 40}), {nil: undefined}), fc.integer({min: 0, max: 40}), fc.constantFrom<any>(undefined, 'shared', 'session')), 200, 16)) call('budgetSections', [a, b, c, d]);
for (const root of sample(fc.array(fc.constantFrom('a', 'b', '中', '😀', ' ', '.', 'é'), {maxLength: 6}).map(p => '/' + p.join('')), 30, 17)) call('repoNoteId', [root]);

// Mutations of recorded inputs.
for (const name of ['parseChecklistLine', 'checklistCounts', 'visibleText', 'linkedTaskId', 'noteRevision', 'checklistLabel']) callAll(name, mutate(recorder.argsOf(name), {variants: 2}));
for (const name of EDITORS) callAll(name, mutate(recorder.argsOf(name), {variants: 2}).filter(stateOk));
callAll('mergeIntoWorktreeNote', mutate(recorder.argsOf('mergeIntoWorktreeNote'), {variants: 2}));
callAll('budgetSections', mutate(recorder.argsOf('budgetSections'), {variants: 1}).filter(a => typeof a[0] === 'number'));

await recorder.write({prune: true});
