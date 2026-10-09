import assert from 'node:assert/strict';
import {test} from 'node:test';
import {parseTasks} from '../src/tasks.js';
import {boardRows, linkedTask, otherOpenTasks, workKeyOf, workLabel, workOptions, type BoardRow} from '../src/tasksBoard.js';
import type {SessionRecord} from '../src/types.js';

const NOW = new Date('2026-10-09T12:00:00Z');
const session = (id: string, fields: Partial<SessionRecord> = {}) => ({id, title: id, cwd: '/repo', repoRoot: '/repo', program: 'claude', status: 'running', createdAt: '2026-10-01T00:00:00Z', updatedAt: '2026-10-01T00:00:00Z', ...fields}) as SessionRecord;
const inWorktree = (id: string, wt: string, branch: string, fields: Partial<SessionRecord> = {}) => session(id, {cwd: `/wt/${branch}`, worktree: {mode: 'managed', id: wt, path: `/wt/${branch}`, branch}, ...fields} as Partial<SessionRecord>);
const sessions = [session('main'), inWorktree('auth', 'w1', 'feat/auth'), inWorktree('auth-tests', 'w1', 'feat/auth'), inWorktree('e2e', 'w2', 'fix/e2e'), inWorktree('old', 'w3', 'old/work', {archivedAt: '2026-10-02T00:00:00Z'})];
const tasks = parseTasks([
	'- [ ] Fix flaky e2e <!-- dh:t=e wt=w2 -->',
	'- [ ] Add OAuth <!-- dh:t=a wt=w1 -->',
	'- [ ] Rate-limit export <!-- dh:t=r -->',
	'- [ ] Add tests <!-- dh:t=t wt=w1 assigned=2026-10-09 -->',
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
	assert.deepEqual(describe(boardRows(parseTasks('- [ ] Solo <!-- dh:t=x -->'), false, NOW)).slice(0, 2), ['# IN PROGRESS', '(n on a backlog task starts a session for it · w assigns it to a worktree)']);
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
