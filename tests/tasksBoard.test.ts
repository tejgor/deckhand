import assert from 'node:assert/strict';
import {test} from 'node:test';
import {parseTasks} from '../src/tasks.js';
import {boardRows, linkedTask, selectableRow, noteGroupLabel, noteItems, notesViewRows, otherOpenTasks, pickerCounts, pickerRows, pickerViewOf, taskOrigin, workKeyOf, workLabel, workOptions, type BoardRow} from '../src/tasksBoard.js';
import type {SessionRecord} from '../src/types.js';

const NOW = new Date('2026-10-09T12:00:00Z');
const session = (id: string, fields: Partial<SessionRecord> = {}) => ({id, title: id, cwd: '/repo', repoRoot: '/repo', program: 'claude', status: 'running', createdAt: '2026-10-01T00:00:00Z', updatedAt: '2026-10-01T00:00:00Z', ...fields}) as SessionRecord;
const inWorktree = (id: string, wt: string, branch: string, fields: Partial<SessionRecord> = {}) => session(id, {cwd: `/wt/${branch}`, worktree: {mode: 'managed', id: wt, path: `/wt/${branch}`, branch}, ...fields} as Partial<SessionRecord>);
const sessions = [session('main'), inWorktree('auth', 'w1', 'feat/auth'), inWorktree('auth-tests', 'w1', 'feat/auth'), inWorktree('e2e', 'w2', 'fix/e2e'), inWorktree('old', 'w3', 'old/work', {archivedAt: '2026-10-02T00:00:00Z'})];
const tasks = parseTasks([
	'- [ ] Fix flaky e2e <!-- dh:t=e wt=w2 -->',
	'- [ ] Add OAuth <!-- dh:t=a wt=w1 -->',
	'- [ ] Rate-limit export <!-- dh:t=r -->',
	'- [ ] Add tests <!-- dh:t=t wt=w1 -->',
	'- [x] Shipped <!-- dh:t=s wt=w1 done=2026-10-08 auto=merge -->',
].join('\n'));
const describe = (rows: BoardRow[]) => rows.map(row => row.kind === 'work' ? `[${row.section} ${row.count}]` : row.kind === 'heading' ? `# ${row.text}` : row.kind === 'task' ? `${row.section}: ${row.task.title}` : row.kind === 'empty' ? `(${row.text})` : row.kind);

test('board: a group per worktree with open tasks (first task\'s order), then the backlog and done', () => {
	assert.deepEqual(describe(boardRows(tasks, false, NOW)), [
		'[wt:w2 1]', 'wt:w2: Fix flaky e2e',
		'[wt:w1 2]', 'wt:w1: Add OAuth', 'wt:w1: Add tests',
		'# BACKLOG', 'backlog: Rate-limit export',
		'# DONE · this week', 'done: Shipped',
	]);
	// Nothing on a worktree yet: a hint where the groups go.
	assert.deepEqual(describe(boardRows(parseTasks('- [ ] Solo <!-- dh:t=x -->'), false, NOW)).slice(0, 2), ['# IN PROGRESS', '(n on a backlog task starts a session for it · w gives it to a worktree)']);
});

test('board: an empty backlog can be selected, so a adds there when every task is on a worktree', () => {
	const rows = boardRows(parseTasks('- [ ] Add OAuth <!-- dh:t=a wt=w1 -->'), false, NOW);
	assert.deepEqual(describe(rows), ['[wt:w1 1]', 'wt:w1: Add OAuth', '# BACKLOG', '(Nothing waiting · a adds a task here)']);
	assert.deepEqual(rows.filter(selectableRow).map(row => row.kind), ['task', 'empty']);
	// Other placeholders stay unselectable.
	assert.equal(boardRows(parseTasks('- [ ] Solo <!-- dh:t=x -->'), false, NOW).filter(selectableRow).length, 1);
});

test('board v: only one work\'s group (even empty) and its done tasks', () => {
	assert.deepEqual(describe(boardRows(tasks, false, NOW, 'wt:w1')), ['[wt:w1 2]', 'wt:w1: Add OAuth', 'wt:w1: Add tests', '# DONE · this week', 'done: Shipped']);
	assert.deepEqual(describe(boardRows(tasks, false, NOW, 's:main')), ['[s:main 0]', '(No open tasks here · a adds one)']);
});

test('work keys, labels and where w can move a task', () => {
	assert.equal(workKeyOf(sessions[1]!), 'wt:w1');
	assert.equal(workKeyOf(sessions[0]!), 's:main');
	assert.equal(workLabel('wt:w1', sessions), '⎇ feat/auth');
	assert.equal(workLabel('s:main', sessions), 'main checkout · main');
	assert.equal(workLabel('wt:gone', sessions), '⎇ a worktree not listed');
	// The backlog, then each worktree and main-checkout session once, in sidebar order (not a worktree whose sessions are all archived).
	const options = workOptions([...sessions, inWorktree('merged', 'w4', 'done/it', {worktree: {mode: 'managed', id: 'w4', path: '/wt/done', branch: 'done/it', mergedAt: '2026-10-08T00:00:00Z'}} as Partial<SessionRecord>)], 'wt:w1');
	assert.deepEqual(options.map(option => [option.section, option.label, option.note]), [
		[undefined, 'Backlog · no worktree', ''],
		['wt:w1', '⎇ feat/auth', 'here'],
		['wt:w2', '⎇ fix/e2e', ''],
		['s:main', 'main checkout · main', ''],
		['wt:w4', '⎇ done/it', '✓ merged'],
	]);
	assert.equal(workOptions(sessions)[0]!.note, 'here');
});

test('a session\'s task: the open one it was started for, then a follow-up; the rest counted', () => {
	assert.equal(linkedTask(tasks, sessions[2])?.title, 'Add OAuth');
	assert.equal(otherOpenTasks(tasks, sessions[2]), 1);
	const followUpOnly = parseTasks('- [x] Done <!-- dh:t=d wt=w1 done=2026-10-08 -->\n- [ ] Later <!-- dh:t=l wt=w1 assigned=2026-10-09 -->');
	assert.equal(linkedTask(followUpOnly, sessions[1])?.title, 'Later');
	assert.equal(otherOpenTasks(followUpOnly, sessions[1]), 0);
	assert.equal(linkedTask(tasks, sessions[0]), undefined);
});

test('the w menu: backlog and the way back to the note pinned, then one view (worktrees or main-checkout sessions), searchable', () => {
	const many = [...sessions, session('notes', {title: 'triage notes'}), session('spike', {title: 'perf spike'})];
	const options = workOptions(many, 'wt:w1');
	const labels = (rows: ReturnType<typeof pickerRows>) => rows.map(row => row.label);
	assert.deepEqual(labels(pickerRows(options, 'worktrees', '')), ['Backlog · no worktree', '⎇ feat/auth', '⎇ fix/e2e']);
	assert.deepEqual(labels(pickerRows(options, 'sessions', '', 'triage notes')), ['Backlog · no worktree', '↩ Back to its note · triage notes', 'main checkout · main', 'main checkout · triage notes', 'main checkout · perf spike']);
	// Search matches every word, in labels and in the titles of a worktree's sessions; the pinned rows stay.
	assert.deepEqual(labels(pickerRows(options, 'worktrees', 'tests')), ['Backlog · no worktree', '⎇ feat/auth']);
	assert.deepEqual(labels(pickerRows(options, 'sessions', 'PERF spike')), ['Backlog · no worktree', 'main checkout · perf spike']);
	assert.deepEqual(pickerCounts(options, ''), {worktrees: 2, sessions: 3});
	assert.deepEqual(pickerCounts(options, 'e2e'), {worktrees: 1, sessions: 0});
	assert.equal(pickerViewOf('s:main'), 'sessions');
	assert.equal(pickerViewOf('wt:w1'), 'worktrees');
	assert.equal(pickerViewOf(undefined), 'worktrees');
});

test('a task\'s origin: the note (session\'s or shared) still holding its ↗ line, unless done, some work\'s task, or read-only', () => {
	const withNotes = [
		session('a', {title: 'notes holder', notes: '- ↗ From a session <!-- dh:t=11111111 -->'}),
		inWorktree('b', 'w1', 'feat/auth', {sharedNotes: {kind: 'worktree', id: 'w1', text: '- ↗ From the worktree <!-- dh:t=22222222 -->', revision: 'r'}} as Partial<SessionRecord>),
		inWorktree('c', 'w9', 'gone', {sharedNotes: {kind: 'worktree', id: 'w9', text: '- ↗ Deleted <!-- dh:t=33333333 -->', revision: 'r', readOnly: true}} as Partial<SessionRecord>),
	];
	const [one, two, three, started, done] = parseTasks([
		'- [ ] From a session <!-- dh:t=11111111 -->', '- [ ] From the worktree <!-- dh:t=22222222 -->', '- [ ] Deleted <!-- dh:t=33333333 -->',
		'- [ ] Started <!-- dh:t=11111111 wt=w1 -->', '- [x] Done <!-- dh:t=22222222 -->',
	].join('\n'));
	assert.equal(taskOrigin(one!, withNotes), 'notes holder');
	assert.equal(taskOrigin(two!, withNotes), '⎇ feat/auth');
	assert.equal(taskOrigin(three!, withNotes), undefined);
	assert.equal(taskOrigin(started!, withNotes), undefined);
	assert.equal(taskOrigin(done!, withNotes), undefined);
});

test('note items: every session\'s notes, grouped by worktree (its shared note first) in sidebar order, the main checkout last', () => {
	const shared = (id: string, text: string) => ({sharedNotes: {kind: 'worktree', id, text, revision: 'r'}});
	const notesOf = (text: string) => ({notes: text, notesFile: {path: '/n', revision: 'r'}});
	const all = [
		session('triage', {title: 'triage', ...notesOf('- [ ] main one\n- [x] ticked\n- ↗ sent <!-- dh:t=1 -->')} as Partial<SessionRecord>),
		inWorktree('a1', 'w1', 'feat/auth', {...notesOf('- [ ] from a1'), ...shared('w1', '- [ ] shared auth')} as Partial<SessionRecord>),
		inWorktree('a2', 'w1', 'feat/auth', {...notesOf(''), ...shared('w1', '- [ ] shared auth')} as Partial<SessionRecord>),
		inWorktree('e', 'w2', 'fix/e2e', {...notesOf('- [ ] from e2e')} as Partial<SessionRecord>),
	];
	const items = noteItems(all);
	assert.deepEqual(items.map(item => [item.group, item.title, item.source]), [
		['wt:w1', 'shared auth', 'worktree note'], ['wt:w1', 'from a1', 'a1'],
		['wt:w2', 'from e2e', 'e'],
		['main', 'main one', 'triage'],
	]);
	assert.equal(noteGroupLabel('wt:w2', all), '⎇ fix/e2e');
	assert.equal(noteGroupLabel('main', all), 'main checkout');
});

test('the Notes view: every note in full by worktree, then the main checkout; f only open items; v one worktree or session', () => {
	const notesOf = (text: string) => ({notes: text, notesFile: {path: '/n', revision: 'r'}});
	const all = [
		session('triage', {title: 'triage', ...notesOf('context only\n\n- [ ] main item'), sharedNotes: {kind: 'repo', id: 'r1', text: '# Repo\n- [x] shipped', revision: 'r', path: '/r'}} as Partial<SessionRecord>),
		inWorktree('a1', 'w1', 'feat/auth', {...notesOf(''), sharedNotes: {kind: 'worktree', id: 'w1', text: 'plan\n- [ ] add tests\n- ↗ sent <!-- dh:t=1234abcd -->', revision: 'r', path: '/w'}} as Partial<SessionRecord>),
		inWorktree('e', 'w2', 'fix/e2e', {...notesOf(''), sharedNotes: {kind: 'worktree', id: 'w2', text: '', revision: 'r', path: '/w2'}} as Partial<SessionRecord>),
	];
	const shape = (rows: BoardRow[]) => rows.map(row => row.kind === 'work' ? `[${row.label} ${row.count}]` : row.kind === 'notehead' ? `# ${row.block.label} (${row.open})` : row.kind === 'noteline' ? `${row.style}${row.item ? '*' : ''}: ${row.text}` : row.kind === 'empty' ? `(${row.text})` : row.kind);
	assert.deepEqual(shape(notesViewRows(all)), [
		'[⎇ feat/auth 1]', '# worktree note (1)', 'text: plan', 'open*: ☐ add tests', 'link: ↗ sent · in Tasks',
		'[main checkout 1]', '# main checkout note (0)', 'heading: Repo', 'done: ☑ shipped', '# triage (1)', 'text: context only', 'open*: ☐ main item',
	]);
	assert.deepEqual(shape(notesViewRows(all, {itemsOnly: true})), ['[⎇ feat/auth 1]', '# worktree note (1)', 'open*: ☐ add tests', '[main checkout 1]', '# triage (1)', 'open*: ☐ main item']);
	assert.deepEqual(shape(notesViewRows(all, {scope: 'wt:w1'})).slice(0, 2), ['[⎇ feat/auth 1]', '# worktree note (1)']);
	assert.deepEqual(shape(notesViewRows(all, {scope: 's:triage'})).filter(line => line.startsWith('#')), ['# main checkout note (0)', '# triage (1)']);
	assert.deepEqual(shape(notesViewRows(all, {scope: 'wt:w2'})), ['(No notes here yet · write one in a session’s Notes tab (a))']);
	// The open item carries what sending it to Tasks needs.
	const item = notesViewRows(all).find((row): row is Extract<BoardRow, {kind: 'noteline'}> => row.kind === 'noteline' && Boolean(row.item))!.item!;
	assert.deepEqual([item.section, item.noteId, item.line, item.title], ['shared', 'worktree:w1', 1, 'add tests']);
});
