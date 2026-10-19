// Records fixtures/parity/tasksBoard/<fn>.jsonl from src/tasksBoard.ts (pure functions over tasks and sessions).
//   node --import tsx parity/gen/tasksBoard.gen.ts
import {Recorder, arb, fc, installDeterminism, mutate, registerFn, runTap, sample} from '../lib.ts';
import * as board from '../../src/tasksBoard.ts';
import {parseTasks, type Task} from '../../src/tasks.ts';
import type {SessionRecord} from '../../src/types.ts';

const MODULE = 'tasksBoard';
installDeterminism();
const recorder = new Recorder(MODULE);

// Functions passed as arguments; the Rust replay dispatches on these names.
const stepsNone = registerFn('stepsNone', (_section: string) => [] as board.WorkStep[]);
const stepsFixed = registerFn('stepsFixed', (section: string): board.WorkStep[] => section === 'wt:w1' ? [{line: 0, text: 'plan', done: true}, {line: 2, text: 'ship 😀', done: false}] : section.startsWith('s:') ? [{line: 5, text: '中文', done: false}] : []);
const stepsUndefined = registerFn('stepsUndefined', (_section: string) => undefined as unknown as board.WorkStep[]);
const linksOpen = registerFn('linksOpen', (_id: string) => 'open' as const);
const linksDone = registerFn('linksDone', (_id: string) => 'done' as const);
const linksGone = registerFn('linksGone', (_id: string) => 'gone' as const);
const linksByCode = registerFn('linksByCode', (id: string) => (['open', 'done', 'gone'] as const)[(id.charCodeAt(0) || 0) % 3]!);
const stepFns = [stepsNone, stepsFixed, stepsUndefined, undefined];
const linkFns = [linksOpen, linksDone, linksGone, linksByCode, undefined];

// ---- builders ----------------------------------------------------------------------------------------------------
const session = (id: string, fields: Partial<SessionRecord> = {}) => ({id, title: id, cwd: '/repo', repoRoot: '/repo', program: 'claude', status: 'running', createdAt: '2026-10-01T00:00:00Z', updatedAt: '2026-10-01T00:00:00Z', ...fields}) as SessionRecord;
const inWorktree = (id: string, wt: string, branch: string, fields: Partial<SessionRecord> = {}) => session(id, {cwd: `/wt/${branch}`, worktree: {mode: 'managed', id: wt, path: `/wt/${branch}`, branch}, ...fields} as Partial<SessionRecord>);
const NOW = new Date('2026-10-09T12:00:00Z');

const ids = ['a', 'b', 'c', 'd', 'e', 'f'];
const wts = ['w1', 'w2', 'w3', '😀', '中'];
const words = ['feat/auth', 'fix/e2e', 'perf spike', 'triage notes', 'Ünï', '😀 emoji', '中文 title', '  spaced   out  ', 'a / b', ''];
const textArb = fc.oneof(fc.constantFrom(...words), arb.text(12));
const notesArb = fc.array(fc.oneof(
	fc.constantFrom('- [ ] open item', '- [x] done item', '* [X] ticked', '  + [ ] indented', '- [ ] ', '- [ ]', '# Heading', '### Sub  ', 'plain text   ', '', '   ', '- ↗ sent <!-- dh:t=1234abcd -->', '  - ↗ nested link <!-- dh:t=ab -->', '- [ ] 😀 emoji item', '- [x] 中文', '- bullet', '-[ ] bad'),
	arb.text(10).map(text => `- [ ] ${text}`),
), {maxLength: 6}).map(lines => lines.join('\n'));
const sharedArb = fc.oneof(fc.constant(undefined), fc.record({kind: fc.constantFrom('worktree' as const, 'repo' as const), idx: fc.nat(4), text: notesArb, readOnly: fc.boolean()}));
const sessionArb = (index: number) => fc.record({
	wt: fc.option(fc.constantFrom(...wts), {nil: undefined}),
	title: textArb,
	status: fc.constantFrom('running', 'starting', 'exited', 'stopped'),
	attention: fc.option(fc.constantFrom('needs-input', 'failed', 'limited', 'working', 'response-ended', 'unknown'), {nil: undefined}),
	agentStatus: fc.option(fc.constantFrom('active', 'idle'), {nil: undefined}),
	archived: fc.boolean(),
	merged: fc.boolean(),
	deleted: fc.nat(9),
	parent: fc.option(fc.constantFrom(...ids), {nil: undefined}),
	notes: fc.option(notesArb, {nil: undefined}),
	hasNotesFile: fc.boolean(),
	shared: sharedArb,
	order: fc.option(fc.nat(5), {nil: undefined}),
	exitReason: fc.option(fc.constantFrom('stopped', 'failed', 'completed', 'interrupted'), {nil: undefined}),
	replyRead: fc.boolean(),
}).map((spec): SessionRecord => {
	const id = ids[index]!;
	const fields: any = {title: spec.title, status: spec.status};
	if (spec.wt) fields.worktree = {mode: 'managed', id: spec.wt, path: `/wt/${spec.wt}`, branch: spec.wt === 'w3' ? undefined : `br/${spec.wt}`, ...spec.merged ? {mergedAt: '2026-10-08T00:00:00Z'} : {}, ...spec.deleted === 0 ? {deletedAt: '2026-10-08T00:00:00Z'} : {}};
	if (spec.status === 'running' && spec.attention) fields.attention = {state: spec.attention, event: 'e', at: '2026-10-01T00:00:00Z'};
	if (spec.agentStatus) fields.agentStatus = spec.agentStatus;
	if (spec.archived) fields.archivedAt = '2026-10-02T00:00:00Z';
	if (spec.parent && spec.parent !== id) fields.parentSessionId = spec.parent;
	if (spec.notes !== undefined) fields.notes = spec.notes;
	if (spec.hasNotesFile) fields.notesFile = {path: '/n', revision: `rev-${id}`};
	if (spec.shared) { const sid = spec.shared.kind === 'worktree' ? (spec.wt ?? wts[spec.shared.idx % wts.length]!) : `r${spec.shared.idx % 2}`; fields.sharedNotes = {kind: spec.shared.kind, id: sid, text: spec.shared.text, revision: `sr-${sid}`, path: '/s', ...spec.shared.readOnly ? {readOnly: true} : {}}; }
	if (spec.order !== undefined) fields.sidebarOrder = spec.order;
	if (spec.exitReason) fields.exitReason = spec.exitReason;
	if (spec.replyRead) fields.replyRead = true;
	return session(id, fields);
});
const sessionsArb = fc.nat(5).chain(count => fc.tuple(...Array.from({length: count}, (_, index) => sessionArb(index))) as fc.Arbitrary<SessionRecord[]>);
const sessionsNonEmpty = fc.integer({min: 1, max: 6}).chain(count => fc.tuple(...Array.from({length: count}, (_, index) => sessionArb(index))) as fc.Arbitrary<SessionRecord[]>);

const dates = ['2026-10-08', '2026-10-09', '2026-10-01', '2026-09-01', '2025-01-01', undefined];
const taskArb = (index: number): fc.Arbitrary<Task> => fc.record({
	title: textArb, done: fc.boolean(), body: fc.constantFrom('', 'details', 'two\nlines'),
	wt: fc.option(fc.constantFrom(...wts), {nil: undefined}), s: fc.option(fc.constantFrom(...ids), {nil: undefined}),
	date: fc.constantFrom(...dates), auto: fc.option(fc.constantFrom('merge' as const, 'done' as const), {nil: undefined}),
	tried: fc.option(fc.constantFrom('x/y', '😀'), {nil: undefined}), from: fc.option(fc.constantFrom('x/y', '中'), {nil: undefined}), added: fc.option(fc.constantFrom('2026-10-01', ''), {nil: undefined}),
}).map(spec => {
	const meta: any = {t: `t${index}`};
	if (spec.wt) meta.wt = spec.wt; else if (spec.s) meta.s = spec.s;
	if (spec.done && spec.date) meta.done = spec.date;
	if (spec.auto) meta.auto = spec.auto;
	if (spec.tried) meta.tried = spec.tried;
	if (spec.from) meta.from = spec.from;
	if (spec.added !== undefined) meta.added = spec.added;
	return {id: meta.t, title: spec.title, done: spec.done, body: spec.body, meta} as Task;
});
const tasksArb = fc.nat(6).chain(count => fc.tuple(...Array.from({length: count}, (_, index) => taskArb(index))) as fc.Arbitrary<Task[]>);
const tasksNonEmpty = fc.integer({min: 1, max: 6}).chain(count => fc.tuple(...Array.from({length: count}, (_, index) => taskArb(index))) as fc.Arbitrary<Task[]>);
const sectionArb = fc.oneof(fc.constantFrom('wt:w1', 'wt:w2', 'wt:w3', 'wt:😀', 'wt:gone', 'wt:', 's:a', 's:b', 's:', 's:gone', 'main', '', 'backlog', 'x:y'), fc.tuple(fc.constantFrom('wt:', 's:'), fc.constantFrom(...wts, ...ids)).map(([prefix, id]) => prefix + id));
const optSection = fc.option(sectionArb, {nil: undefined});

// taskLinkLookup returns a closure, which cannot be recorded: the fixture applies it to `ids` (undefined while not loaded).
const lookupOf = (tasks: Task[] | undefined, lookupIds: string[]) => { const lookup = board.taskLinkLookup(tasks); return lookup ? lookupIds.map(lookup) : undefined; };
const call = (name: string, fn: Function, args: unknown[]) => { try { recorder.call(name, fn, args); } catch {} };
const fns = board as unknown as Record<string, Function>;

// ---- 1. Tap the existing tests -----------------------------------------------------------------------------------
const exported = Object.keys(board).filter(name => typeof fns[name] === 'function');
recorder.addTap(await runTap({tests: ['tests/tasksBoard.test.ts'], tap: exported.filter(name => name !== 'taskLinkLookup').map(name => `tasksBoard:${name}`)}), {module: MODULE});

// ---- 2. Hand-picked edge cases -----------------------------------------------------------------------------------
const base = [session('main'), inWorktree('auth', 'w1', 'feat/auth'), inWorktree('auth-tests', 'w1', 'feat/auth'), inWorktree('e2e', 'w2', 'fix/e2e'), inWorktree('old', 'w3', 'old/work', {archivedAt: '2026-10-02T00:00:00Z'})];
const baseTasks = parseTasks(['- [ ] Fix flaky e2e <!-- dh:t=e wt=w2 -->', '- [ ] Add OAuth <!-- dh:t=a wt=w1 -->', '- [ ] Rate-limit export <!-- dh:t=r -->', '- [ ] Add tests <!-- dh:t=t wt=w1 -->', '- [x] Shipped <!-- dh:t=s wt=w1 done=2026-10-08 auto=merge -->', '- [x] Old one <!-- dh:t=o done=2025-01-01 -->', '- [ ] Session task 😀 <!-- dh:t=z s=main -->'].join('\n'));
for (const showOlder of [false, true]) for (const scope of [undefined, 'wt:w1', 's:main', 'wt:none', '']) for (const stepsOf of stepFns) {
	call('boardRows', board.boardRows, [baseTasks, showOlder, NOW, scope, stepsOf]);
	call('boardRows', board.boardRows, [[], showOlder, NOW, scope, stepsOf]);
}
call('boardRows', board.boardRows, [baseTasks, true]);
call('boardRows', board.boardRows, [baseTasks, false, new Date('2026-10-15T12:00:00Z')]);
for (const section of ['wt:w1', 's:main', 'wt:w9', 's:', 'wt:', 'zzz', 'wt:w3']) {
	call('workNote', board.workNote, [section, base]);
	call('workLabel', board.workLabel, [section, base]);
	call('workSessions', board.workSessions, [section, base]);
	call('workState', board.workState, [section, base, '⠋']);
	call('noteGroupLabel', board.noteGroupLabel, [section, base]);
	call('hasMainTask', board.hasMainTask, [section, baseTasks]);
	call('hasMainTask', board.hasMainTask, [section, baseTasks, 'a']);
	call('hasMainTask', board.hasMainTask, [section, baseTasks, 'zz']);
	call('pickerViewOf', board.pickerViewOf, [section]);
}
call('pickerViewOf', board.pickerViewOf, [undefined]);
call('noteGroupLabel', board.noteGroupLabel, ['main', base]);
for (const text of [undefined, '', '- [ ] a\n- [x] b\n- [ ]   \n- [ ]\n　- [ ] x\n- ↗ link <!-- dh:t=1 -->', '- [ ] 😀\r\n- [x] 中', '\n\n  - [ ]    padded   ']) call('noteSteps', board.noteSteps, [text]);
for (const steps of [[], [{line: 1, text: 'a', done: true}, {line: 2, text: 'b', done: false}]]) call('stepCount', board.stepCount, [steps]);
for (const line of ['', '   ', '- [ ] open', '  - [x] done  ', '- [X]', '- ↗ t <!-- dh:t=ab -->', '    - ↗ nested <!-- dh:t=ab -->', '# H', '######  deep  ', '####### seven', '#nospace', 'text   \t', '\ttext', '- [ ] 😀', '中　', '☐ plain']) for (const links of linkFns) call('noteLine', board.noteLine, [line, links]);
for (const list of [undefined, [], baseTasks]) call('taskLinkLookup', lookupOf, [list, ['e', 'a', 'r', 's', 'zz', '', '😀']]);
call('taskCountLabel', board.taskCountLabel, [[]]);
call('taskCountLabel', board.taskCountLabel, [baseTasks.slice(0, 1)]);
call('taskCountLabel', board.taskCountLabel, [baseTasks]);
call('taskCountLabel', board.taskCountLabel, [baseTasks.filter(task => task.done)]);
for (const spinner of ['⠋', '', '😀']) for (const task of baseTasks) { call('taskState', board.taskState, [task, base, spinner]); call('taskState', board.taskState, [task, base.slice(0, 2), spinner, base]); }
for (const s of base) {
	call('workKeyOf', board.workKeyOf, [s]); call('linkedTask', board.linkedTask, [baseTasks, s]); call('linkedTasks', board.linkedTasks, [baseTasks, s]); call('otherOpenTasks', board.otherOpenTasks, [baseTasks, s]);
	call('openItemsRemovedWith', board.openItemsRemovedWith, [s, base]); call('worktreeOpenItems', board.worktreeOpenItems, [s.worktree?.id, base]);
}
call('linkedTask', board.linkedTask, [baseTasks, undefined]); call('linkedTasks', board.linkedTasks, [baseTasks, undefined]); call('otherOpenTasks', board.otherOpenTasks, [baseTasks, undefined]);
call('worktreeOpenItems', board.worktreeOpenItems, [undefined, base]); call('worktreeOpenItems', board.worktreeOpenItems, ['', base]);
call('leadSession', board.leadSession, [[]]);
call('workOptions', board.workOptions, [[]]); call('workOptions', board.workOptions, [[], 'wt:w1']);
call('workOptions', board.workOptions, [base]); call('workOptions', board.workOptions, [base, 'wt:w1']); call('workOptions', board.workOptions, [base, 's:main']);
call('workOptions', board.workOptions, [[session('moved', {cwd: '/x', requestedWorktreeMode: 'managed'} as Partial<SessionRecord>), inWorktree('del', 'w9', 'gone', {worktree: {mode: 'managed', id: 'w9', path: '/wt/gone', branch: 'gone', deletedAt: '2026-10-01T00:00:00Z'}} as Partial<SessionRecord>), session('ok')], 's:ok']);
const opts = board.workOptions([...base, session('notes', {title: 'triage notes'}), session('spike', {title: 'perf spike'}), session('emoji', {title: '😀 Ünï 中文'})], 'wt:w1');
for (const view of ['worktrees', 'sessions'] as const) for (const query of ['', '  ', 'tests', 'PERF spike', 'perf  spike', 'zzz', '😀', '中文', 'ünï', 'auth e2e', '⎇']) for (const origin of [undefined, '', 'triage notes', '⎇ feat/auth']) call('pickerRows', board.pickerRows, [opts, view, query, origin]);
for (const query of ['', 'e2e', 'main', 'x', '😀', ' \t ']) call('pickerCounts', board.pickerCounts, [opts, query]);

const noted = [
	session('triage', {title: 'triage', notes: 'context only\n\n- [ ] main one\n- [x] ticked\n- ↗ sent <!-- dh:t=1 -->', notesFile: {path: '/n', revision: 'r'}, sharedNotes: {kind: 'repo', id: 'r1', text: '# Repo\n- [x] shipped\n- [ ] repo item', revision: 'rr', path: '/r'}} as Partial<SessionRecord>),
	inWorktree('a1', 'w1', 'feat/auth', {notes: '- [ ] from a1', notesFile: {path: '/n', revision: 'r1'}, sharedNotes: {kind: 'worktree', id: 'w1', text: 'plan\n- [ ] add tests\n- ↗ sent <!-- dh:t=1234abcd -->\n- [ ] 😀 emoji', revision: 'rw', path: '/w'}} as Partial<SessionRecord>),
	inWorktree('a2', 'w1', 'feat/auth', {notes: '', notesFile: {path: '/n', revision: 'r2'}, sharedNotes: {kind: 'worktree', id: 'w1', text: 'plan\n- [ ] add tests', revision: 'rw', path: '/w'}, archivedAt: '2026-10-02T00:00:00Z'} as Partial<SessionRecord>),
	inWorktree('e', 'w2', 'fix/e2e', {notes: '   ', notesFile: {path: '/n', revision: 'r'}, sharedNotes: {kind: 'worktree', id: 'w2', text: '', revision: 'r', path: '/w2'}} as Partial<SessionRecord>),
	inWorktree('d', 'w9', 'gone', {notes: '- [ ] orphan', notesFile: {path: '/n', revision: 'r'}, sharedNotes: {kind: 'worktree', id: 'w9', text: '- [ ] readonly', revision: 'r', path: '/w9', readOnly: true}} as Partial<SessionRecord>),
];
for (const list of [[], base, noted, noted.slice().reverse()]) {
	call('noteBlocks', board.noteBlocks, [list]); call('noteItems', board.noteItems, [list]);
	call('leadSession', board.leadSession, [list]);
	for (const itemsOnly of [false, true]) for (const scope of [undefined, 'wt:w1', 'wt:w2', 'wt:w9', 's:triage', 's:a1', 's:zz', 'main']) for (const links of linkFns) call('notesViewRows', board.notesViewRows, [list, {itemsOnly, scope, links}]);
	call('notesViewRows', board.notesViewRows, [list]);
	call('notesViewRows', board.notesViewRows, [list, {}]);
}
for (const block of board.noteBlocks(noted)) call('blockItems', board.blockItems, [block, noted]);
for (const s of noted) { call('openItemsRemovedWith', board.openItemsRemovedWith, [s, noted]); call('openItemsRemovedWith', board.openItemsRemovedWith, [s, [s]]); call('taskOrigin', board.taskOrigin, [baseTasks[0]!, [s]]); }
const originTasks = parseTasks(['- [ ] A <!-- dh:t=1 -->', '- [ ] B <!-- dh:t=1234abcd -->', '- [ ] C <!-- dh:t=1234abcd wt=w1 -->', '- [x] D <!-- dh:t=1234abcd -->', '- [ ] E'].join('\n'));
for (const task of originTasks) call('taskOrigin', board.taskOrigin, [task, noted]);
for (const row of board.notesViewRows(noted).concat(board.boardRows(baseTasks, true, NOW, undefined, stepsFixed))) { call('selectableRow', board.selectableRow, [row]); call('rowKey', board.rowKey, [row]); }
for (const row of [{kind: 'empty', text: 'x'}, {kind: 'empty', text: 'x', addsTo: 'work'}, {kind: 'older', count: 2, shown: true}, {kind: 'heading', text: 'H'}, {kind: 'work', section: 'wt:w1', count: 1}, {kind: 'step', section: 'wt:w1', taskId: 't', step: {line: 3, text: 's', done: false}}]) { call('selectableRow', board.selectableRow, [row]); call('rowKey', board.rowKey, [row]); }

// ---- 3. Mutations of what the tests (and the cases above) passed -------------------------------------------------
for (const name of ['boardRows', 'workLabel', 'workOptions', 'pickerRows', 'noteLine', 'noteSteps', 'taskState', 'hasMainTask', 'workNote', 'noteItems', 'notesViewRows', 'taskOrigin', 'openItemsRemovedWith']) {
	for (const args of mutate(recorder.argsOf(name).slice(0, 60), {variants: 2})) call(name, fns[name]!, args);
}

// ---- 4. fast-check samples with fixed seeds ----------------------------------------------------------------------
const N = 120;
for (const [tasks, showOlder, scope, stepsOf, day] of sample(fc.tuple(tasksArb, fc.boolean(), optSection, fc.constantFrom(...stepFns), fc.integer({min: 1, max: 28})), N, 11)) call('boardRows', board.boardRows, [tasks, showOlder, new Date(Date.UTC(2026, 9, day, 12)), scope, stepsOf]);
for (const [section, list] of sample(fc.tuple(sectionArb, sessionsArb), N, 12)) { call('workNote', board.workNote, [section, list]); call('workSessions', board.workSessions, [section, list]); call('workLabel', board.workLabel, [section, list]); call('noteGroupLabel', board.noteGroupLabel, [section, list]); call('workState', board.workState, [section, list, '⠙']); }
for (const [section, tasks, except] of sample(fc.tuple(sectionArb, tasksArb, fc.option(fc.constantFrom('t0', 't1', 'zz'), {nil: undefined})), N, 13)) call('hasMainTask', board.hasMainTask, [section, tasks, except]);
for (const [tasks, list] of sample(fc.tuple(tasksNonEmpty, sessionsArb), N, 14)) for (const task of tasks) { call('taskSessions', board.taskSessions, [task, list]); call('taskOrigin', board.taskOrigin, [task, list]); call('taskState', board.taskState, [task, list, '⠹']); call('taskState', board.taskState, [task, list.slice(0, 1), '⠹', list]); }
for (const list of sample(sessionsArb, N, 15)) {
	call('leadSession', board.leadSession, [list]); call('noteBlocks', board.noteBlocks, [list]); call('noteItems', board.noteItems, [list]);
	for (const s of list) { call('workKeyOf', board.workKeyOf, [s]); call('openItemsRemovedWith', board.openItemsRemovedWith, [s, list]); call('worktreeOpenItems', board.worktreeOpenItems, [s.worktree?.id, list]); }
}
for (const [list, current] of sample(fc.tuple(sessionsArb, optSection), N, 16)) call('workOptions', board.workOptions, [list, current]);
for (const [list, current, view, query, origin] of sample(fc.tuple(sessionsNonEmpty, optSection, fc.constantFrom('worktrees' as const, 'sessions' as const), fc.oneof(fc.constantFrom('', 'e2e', 'br', 'main', 'w1 br', 'zz'), arb.text(6)), fc.option(textArb, {nil: undefined})), N, 17)) {
	const options = board.workOptions(list, current);
	call('pickerRows', board.pickerRows, [options, view, query, origin]); call('pickerCounts', board.pickerCounts, [options, query]);
}
for (const [tasks, list] of sample(fc.tuple(tasksNonEmpty, sessionsNonEmpty), N, 18)) for (const s of list) { call('linkedTasks', board.linkedTasks, [tasks, s]); call('linkedTask', board.linkedTask, [tasks, s]); call('otherOpenTasks', board.otherOpenTasks, [tasks, s]); }
for (const tasks of sample(tasksArb, 60, 19)) { call('taskCountLabel', board.taskCountLabel, [tasks]); call('taskLinkLookup', lookupOf, [tasks, ['t0', 't1', 't5', 'zz', '']]); }
for (const text of sample(fc.oneof(notesArb, arb.text(30)), N, 20)) { call('noteSteps', board.noteSteps, [text]); for (const links of linkFns) for (const line of text.split('\n')) call('noteLine', board.noteLine, [line, links]); }
for (const [list, itemsOnly, scope, links] of sample(fc.tuple(sessionsArb, fc.boolean(), optSection, fc.constantFrom(...linkFns)), N * 2, 21)) call('notesViewRows', board.notesViewRows, [list, {itemsOnly, scope, links}]);
for (const list of sample(sessionsArb, 60, 22)) for (const block of board.noteBlocks(list)) call('blockItems', board.blockItems, [block, list]);

await recorder.write({prune: true});
