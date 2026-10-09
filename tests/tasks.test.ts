import assert from 'node:assert/strict';
import {test} from 'node:test';
import {applyTaskOp, cleanTaskTitle, groupTasks, openNoteItems, parseNoteTaskLink, parseTasks, promoteNoteLine, taskPrompt, withoutTaskMeta} from '../src/tasks.js';

const NOW = new Date('2026-10-09T12:00:00Z');

test('tasks: top-level checklist items with their indented body; other lines are kept as written', () => {
	const text = '# Tasks\n\nSome prose.\n- [ ] First <!-- dh:t=aaaa1111 wt=w1 added=2026-10-01 -->\n  detail one\n    deeper\n- [x] Second\n  - [ ] a sub-item stays body\nTrailing prose\n';
	const tasks = parseTasks(text);
	assert.deepEqual(tasks.map(task => [task.title, task.done, task.body]), [['First', false, 'detail one\n  deeper'], ['Second', true, '- [ ] a sub-item stays body']]);
	assert.deepEqual(tasks[0]!.meta, {t: 'aaaa1111', wt: 'w1', added: '2026-10-01'});
	assert.equal(tasks[0]!.id, 'aaaa1111');
	assert.match(tasks[1]!.id, /^~1:[0-9a-f]{8}$/);
	// A no-op round trip through an op keeps the prose and gives every task an ID.
	const {text: next} = applyTaskOp(text, {type: 'toggle', id: 'aaaa1111'}, NOW);
	assert.match(next, /^# Tasks\n\nSome prose\.\n- \[x\] First <!-- dh:t=aaaa1111 wt=w1 done=2026-10-09 added=2026-10-01 -->\n  detail one\n    deeper\n- \[x\] Second <!-- dh:t=[0-9a-f]{8} -->\n  - \[ \] a sub-item stays body\nTrailing prose\n$/);
});

test('task ops: add after the last open task, edit, toggle, move, remove; unknown IDs throw', () => {
	let {text} = applyTaskOp('', {type: 'add', title: '  Write  docs\n', body: '\nwhy:\n\n  because\n'}, NOW);
	assert.match(text, /^- \[ \] Write docs <!-- dh:t=[0-9a-f]{8} added=2026-10-09 -->\n  why:\n  \n    because\n$/);
	text = applyTaskOp(text, {type: 'add', title: 'Done one'}, NOW).text;
	const done = parseTasks(text)[1]!;
	text = applyTaskOp(text, {type: 'toggle', id: done.id}, NOW).text;
	text = applyTaskOp(text, {type: 'add', title: 'Third'}, NOW).text;
	assert.deepEqual(parseTasks(text).map(task => task.title), ['Write docs', 'Third', 'Done one']);
	assert.equal(parseTasks(text)[0]!.body, 'why:\n\n  because');
	const [first, third] = parseTasks(text);
	text = applyTaskOp(text, {type: 'move', id: third!.id, target: first!.id}, NOW).text;
	assert.deepEqual(parseTasks(text).map(task => task.title), ['Third', 'Write docs', 'Done one']);
	text = applyTaskOp(text, {type: 'edit', id: first!.id, title: 'Write the docs', body: ''}, NOW).text;
	assert.deepEqual(parseTasks(text)[1]!.body, '');
	text = applyTaskOp(text, {type: 'remove', id: third!.id}, NOW).text;
	assert.deepEqual(parseTasks(text).map(task => task.title), ['Write the docs', 'Done one']);
	assert.throws(() => applyTaskOp(text, {type: 'toggle', id: 'nope'}), /no longer in the list/);
	assert.throws(() => applyTaskOp(text, {type: 'add', title: '  '}), /needs a title/);
	// Toggling a done task reopens it and forgets when and why it was ticked.
	const reopened = applyTaskOp('- [x] x <!-- dh:t=b done=2026-10-01 auto=merge wt=w -->\n', {type: 'toggle', id: 'b'}, NOW).text;
	assert.equal(reopened, '- [ ] x <!-- dh:t=b wt=w -->\n');
});

test('linked work: tick on merge or done, reopen when undone, abandon back to the backlog', () => {
	let text = '- [ ] A <!-- dh:t=a wt=w1 -->\n- [ ] B <!-- dh:t=b wt=w1 -->\n- [ ] C <!-- dh:t=c s=s1 -->\n- [x] D <!-- dh:t=d wt=w1 done=2026-10-01 -->\n';
	let result = applyTaskOp(text, {type: 'tick-linked', link: {wt: 'w1'}, auto: 'merge'}, NOW);
	assert.equal(result.changed, 2);
	text = result.text;
	assert.deepEqual(parseTasks(text).map(task => [task.done, task.meta.auto]), [[true, 'merge'], [true, 'merge'], [false, undefined], [true, undefined]]);
	// Only what the merge ticked reopens (D was ticked by hand).
	result = applyTaskOp(text, {type: 'reopen-linked', link: {wt: 'w1'}, auto: 'merge'}, NOW);
	assert.equal(result.changed, 2);
	assert.deepEqual(parseTasks(result.text).map(task => task.done), [false, false, false, true]);
	assert.equal(applyTaskOp(result.text, {type: 'reopen-linked', link: {wt: 'w1'}, auto: 'done'}).changed, 0);
	result = applyTaskOp(result.text, {type: 'abandon-linked', link: {wt: 'w1'}, tried: 'fix-footer'}, NOW);
	assert.deepEqual(parseTasks(result.text).map(task => task.meta), [{t: 'a', tried: 'fix-footer'}, {t: 'b', tried: 'fix-footer'}, {t: 'c', s: 's1'}, {t: 'd', wt: 'w1', done: '2026-10-01'}]);
	// Linking clears an old attempt.
	assert.deepEqual(parseTasks(applyTaskOp(result.text, {type: 'link', id: 'a', link: {s: 's9'}}).text)[0]!.meta, {t: 'a', s: 's9'});
});

test('board groups: in progress, backlog, recent done (newest first) and older done', () => {
	const tasks = parseTasks([
		'- [ ] linked <!-- dh:t=1 wt=w -->', '- [ ] free <!-- dh:t=2 tried=x -->', '- [x] old <!-- dh:t=3 done=2026-09-01 -->',
		'- [x] new <!-- dh:t=4 done=2026-10-08 -->', '- [x] newer <!-- dh:t=5 done=2026-10-09 -->', '- [x] undated',
	].join('\n'));
	const groups = groupTasks(tasks, NOW);
	assert.deepEqual(groups.progress.map(task => task.title), ['linked']);
	assert.deepEqual(groups.backlog.map(task => task.title), ['free']);
	assert.deepEqual(groups.done.map(task => task.title), ['newer', 'new']);
	assert.deepEqual(groups.olderDone.map(task => task.title), ['old', 'undated']);
});

test('notes: a checklist line becomes a link to its task; open items; display without metadata', () => {
	const note = 'scratch\n  - [ ] Retry setup when trust changes\nmore';
	const promoted = promoteNoteLine(note, 1, 'abcd1234');
	assert.deepEqual(promoted, {title: 'Retry setup when trust changes', done: false, text: 'scratch\n  - ↗ Retry setup when trust changes <!-- dh:t=abcd1234 -->\nmore'});
	assert.deepEqual(parseNoteTaskLink(promoted!.text.split('\n')[1]!), {indent: '  ', title: 'Retry setup when trust changes', id: 'abcd1234'});
	assert.equal(promoteNoteLine(note, 0, 'x'), undefined);
	assert.equal(promoteNoteLine(note, 9, 'x'), undefined);
	assert.deepEqual(openNoteItems('- [ ] a\n- [x] b\n  * [ ] c\n- ↗ d <!-- dh:t=1 -->'), ['a', 'c']);
	assert.equal(withoutTaskMeta('- [ ] a <!-- dh:t=1 wt=2 -->'), '- [ ] a');
	assert.equal(cleanTaskTitle('a <!-- b --> c\u0007'), 'a b c');
	assert.equal(taskPrompt({title: 'Fix it', body: '\nwhy\n'}), 'Task: Fix it\n\nwhy');
});
