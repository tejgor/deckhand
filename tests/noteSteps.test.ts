import assert from 'node:assert/strict';
import {test} from 'node:test';
import {applyTaskOp, clientTaskOp, parseTasks, promoteOpenNoteLines} from '../src/tasks.js';
import {noteReadRows, toggleChecklistLine} from '../src/notes.js';
import {boardRows, hasMainTask, noteLine, noteSteps, taskLinkLookup, workNote, worktreeOpenItems, type BoardRow} from '../src/tasksBoard.js';
import {mergeConfirmLayout} from '../src/mergeModel.js';
import type {MergePreview, SessionRecord} from '../src/types.js';

// A worktree's note checklist as the steps of the task the worktree was started for (the board, the banner), sending a
// note's open items to Tasks at once (A, and at merge or deletion), and `↗` lines saying where their task is.

const NOW = new Date('2026-10-09T12:00:00Z');
const session = (id: string, fields: Partial<SessionRecord> = {}) => ({id, title: id, cwd: '/repo', repoRoot: '/repo', program: 'claude', status: 'running', createdAt: '2026-10-01T00:00:00Z', updatedAt: '2026-10-01T00:00:00Z', ...fields}) as SessionRecord;
const note = '# Plan\n- [x] Read the spec\n- [ ] Write the parser\n  - [ ] Handle tabs\n- ↗ Docs <!-- dh:t=d0c5 -->\n- [ ] \nprose';
const sessions = [
	session('main', {notes: '- [ ] Ask about rate limits', notesFile: {path: '/n/main.md', revision: 'rm'}}),
	session('auth', {cwd: '/wt/auth', worktree: {mode: 'managed', id: 'w1', path: '/wt/auth', branch: 'feat/auth'}, sharedNotes: {kind: 'worktree', id: 'w1', path: '/n/w1.md', text: note, revision: 'r1'}} as Partial<SessionRecord>),
	session('gone', {cwd: '/wt/gone', worktree: {mode: 'managed', id: 'w9', path: '/wt/gone', branch: 'gone'}, sharedNotes: {kind: 'worktree', id: 'w9', path: '/n/w9.md', text: '- [ ] Left behind', revision: 'r9', readOnly: true}} as Partial<SessionRecord>),
];

test('a work\'s steps: its note\'s checklist (open and ticked, not ↗ links or empty items), under the task it was started for', () => {
	assert.deepEqual(noteSteps(note), [
		{line: 1, text: 'Read the spec', done: true}, {line: 2, text: 'Write the parser', done: false}, {line: 3, text: 'Handle tabs', done: false},
	]);
	assert.deepEqual(workNote('wt:w1', sessions), {sessionId: 'auth', section: 'shared', noteId: 'worktree:w1', text: note, revision: 'r1'});
	assert.deepEqual(workNote('s:main', sessions), {sessionId: 'main', section: 'session', text: '- [ ] Ask about rate limits', revision: 'rm'});
	assert.equal(workNote('wt:w9', sessions)?.readOnly, true);
	const tasks = parseTasks(['- [ ] Add OAuth <!-- dh:t=a wt=w1 -->', '- [ ] Add tests <!-- dh:t=t wt=w1 assigned=2026-10-09 -->'].join('\n'));
	const rows = boardRows(tasks, false, NOW, undefined, section => noteSteps(workNote(section, sessions)?.text));
	const describe = (row: BoardRow) => row.kind === 'task' ? `${row.task.title}${row.steps ? ` ${row.steps.done}/${row.steps.total}` : ''}` : row.kind === 'step' ? `  ${row.step.done ? '☑' : '☐'} ${row.step.text}` : row.kind;
	// Under the started task (◆), before the follow-ups; it counts them.
	assert.deepEqual(rows.slice(0, 6).map(describe), ['work', 'Add OAuth 1/3', '  ☑ Read the spec', '  ☐ Write the parser', '  ☐ Handle tabs', 'Add tests']);
	// Only follow-ups on the work: no task to hang them on, so none are listed.
	assert.ok(!boardRows(parseTasks('- [ ] Add tests <!-- dh:t=t wt=w1 assigned=2026-10-09 -->'), false, NOW, undefined, () => noteSteps(note)).some(row => row.kind === 'step'));
	// Space on a step ticks the note's line (and back).
	assert.equal(toggleChecklistLine(note, 2)!.split('\n')[2], '- [x] Write the parser');
	assert.equal(toggleChecklistLine(note, 1)!.split('\n')[1], '- [ ] Read the spec');
	assert.equal(toggleChecklistLine(note, 3)!.split('\n')[3], '  - [x] Handle tabs');
	assert.equal(toggleChecklistLine(note, 0), undefined);
});

test('sending a note\'s open items at once: each becomes its ↗ link; the backlog remembers the branch they were left in', () => {
	let next = 0;
	const sent = promoteOpenNoteLines(note, () => `id${next++}`);
	assert.deepEqual(sent.items, [{id: 'id0', title: 'Write the parser'}, {id: 'id1', title: 'Handle tabs'}]);
	assert.deepEqual(sent.text.split('\n'), ['# Plan', '- [x] Read the spec', '- ↗ Write the parser <!-- dh:t=id0 -->', '  - ↗ Handle tabs <!-- dh:t=id1 -->', '- ↗ Docs <!-- dh:t=d0c5 -->', '- [ ] ', 'prose']);
	assert.deepEqual(promoteOpenNoteLines('just text', () => 'x'), {text: 'just text', items: []});
	const added = applyTaskOp('', {type: 'add', title: 'Write the parser', id: 'id0', from: 'feat/auth'}, NOW);
	assert.match(added.text, /^- \[ \] Write the parser <!-- dh:t=id0 from=feat\/auth added=2026-10-09 -->/);
	// Assigned to a work, `from` does not apply.
	assert.doesNotMatch(applyTaskOp('', {type: 'add', title: 'x', link: {wt: 'w1'}, from: 'feat/auth'}, NOW).text, /from=/);
	// What merging or deleting the worktree offers to send: its note's open items (not a deleted worktree's read-only note).
	assert.deepEqual(worktreeOpenItems('w1', sessions), ['Write the parser', 'Handle tabs']);
	assert.deepEqual(worktreeOpenItems('w9', sessions), []);
	assert.deepEqual(worktreeOpenItems(undefined, sessions), []);
});

test('↗ lines say where their task is: in Tasks, done, or no longer in the list', () => {
	const links = taskLinkLookup(parseTasks('- [ ] Open <!-- dh:t=aaaa -->\n- [x] Done <!-- dh:t=bbbb -->'))!;
	assert.equal(noteLine('- ↗ Open <!-- dh:t=aaaa -->', links)?.text, '↗ Open · in Tasks');
	assert.equal(noteLine('- ↗ Done <!-- dh:t=bbbb -->', links)?.text, '↗ Done · done');
	assert.equal(noteLine('- ↗ Gone <!-- dh:t=cccc -->', links)?.text, '↗ Gone · not in Tasks');
	// Before the list is loaded nothing is claimed.
	assert.equal(taskLinkLookup(undefined), undefined);
	assert.equal(noteLine('- ↗ Gone <!-- dh:t=cccc -->')?.text, '↗ Gone · in Tasks');
	assert.deepEqual(noteReadRows('- ↗ Done <!-- dh:t=bbbb -->', 40, links), [{text: '↗ Done · done', kind: 'link'}]);
});

test('the merge confirmation offers to send the worktree note\'s open items to the backlog: one row space switches', () => {
	const preview: MergePreview = {targetRoot: '/repo', targetBranch: 'main', targetIsMain: true, defaultBranch: 'main', commitCount: 1, commits: ['feat'], diff: {files: 1, insertions: 1, deletions: 0}, uncommitted: 0, overlap: {committed: [], uncommitted: []}} as unknown as MergePreview;
	const input = {title: 'auth', preview, commitFirst: true, commitMessage: 'auth', notes: [], width: 100, height: 30};
	const send = mergeConfirmLayout({...input, followUps: [{title: 'Add tests', ticked: false}], openItems: {titles: ['Write the parser', 'Handle tabs'], send: true}});
	assert.deepEqual(send.followUps, ['☐ Add tests · back to the backlog', '☑ 2 open note items → backlog: Write the parser, Handle tabs']);
	assert.equal(send.hint, 'enter choose · space toggle · j/k move · esc cancel');
	const keep = mergeConfirmLayout({...input, openItems: {titles: ['Write the parser'], send: false}});
	assert.deepEqual(keep.followUps, ['☐ 1 open note item · stay in the note: Write the parser']);
	// No follow-ups heading for the items row alone; nothing before the preview arrives.
	assert.ok(!keep.details.some(line => line.text.startsWith('Follow-ups')));
	assert.deepEqual(mergeConfirmLayout({...input, preview: undefined, openItems: {titles: ['x'], send: true}}).followUps, []);
});

test('w onto work without a ◆ task makes the task that work\'s own; ctrl+f (no main) makes it a follow-up; the role can change in place', () => {
	const text = '- [ ] Fix flaky test <!-- dh:t=f1 -->\n- [ ] Write docs <!-- dh:t=d1 -->';
	const main = applyTaskOp(text, {type: 'assign', id: 'f1', link: {s: 'flaky'}, main: true}, NOW);
	assert.match(main.text, /^- \[ \] Fix flaky test <!-- dh:t=f1 s=flaky -->/);
	assert.equal(hasMainTask('s:flaky', parseTasks(main.text)), true);
	assert.equal(hasMainTask('s:flaky', parseTasks(main.text), 'f1'), false);
	// Once it has one, the next task assigned there is a follow-up (the UI sends no `main`).
	const follow = applyTaskOp(main.text, {type: 'assign', id: 'd1', link: {s: 'flaky'}}, NOW);
	assert.match(follow.text, /Write docs <!-- dh:t=d1 s=flaky assigned=2026-10-09 -->/);
	// Same work, other role (only with an explicit `main`): a follow-up becomes the work's own task, and back.
	assert.match(applyTaskOp(follow.text, {type: 'assign', id: 'd1', link: {s: 'flaky'}, main: true}, NOW).text, /Write docs <!-- dh:t=d1 s=flaky -->/);
	assert.match(applyTaskOp(main.text, {type: 'assign', id: 'f1', link: {s: 'flaky'}, main: false}, NOW).text, /Fix flaky test <!-- dh:t=f1 s=flaky assigned=2026-10-09 -->/);
	assert.equal(applyTaskOp(main.text, {type: 'assign', id: 'f1', link: {s: 'flaky'}, main: true}, NOW).changed, 0);
	assert.equal(applyTaskOp(main.text, {type: 'assign', id: 'f1', link: {s: 'flaky'}}, NOW).changed, 0);
	// From a client: `main` only with a link.
	assert.deepEqual(clientTaskOp({type: 'assign', id: 'x', link: {wt: 'w1'}, main: true}), {type: 'assign', id: 'x', link: {wt: 'w1'}, main: true});
	assert.deepEqual(clientTaskOp({type: 'assign', id: 'x', link: {wt: 'w1'}, main: false}), {type: 'assign', id: 'x', link: {wt: 'w1'}, main: false});
	assert.deepEqual(clientTaskOp({type: 'assign', id: 'x', main: true}), {type: 'assign', id: 'x'});
});
