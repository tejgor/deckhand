import assert from 'node:assert/strict';
import {test} from 'node:test';
import React from 'react';
import {renderToString} from 'ink';
import {budgetSections, checklistCounts, checklistLabel, continueChecklist, fitReadRows, insertChecklistItem, noteReadRows, openChecklistText, parseChecklistLine, repoNoteId, sharedNoteIdentity, toggleChecklist} from '../src/notes.js';
import {NotesRows, notesLayout, type NotesSectionInput} from '../src/notesPane.js';
import {sessionDetails} from '../src/sidebarModel.js';
import {filterSessionList, handoffMarkdown} from '../src/sessionFeatures.js';
import type {SessionRecord} from '../src/types.js';

const plain = (text: string) => text.replace(/\x1b\[[0-9;]*m/g, '');

test('checklists: -, * and + items, indented, [x] and [X]; counts', () => {
	assert.deepEqual(parseChecklistLine('- [ ] write tests'), {indent: '', bullet: '-', checked: false, prefix: 6, text: 'write tests', box: 3});
	assert.equal(parseChecklistLine('  * [x] done')?.checked, true);
	assert.equal(parseChecklistLine('\t+ [X] Done')?.indent, '\t');
	assert.equal(parseChecklistLine('- [ ]')?.text, '');
	for (const line of ['- [] no', '-[ ] no', '- [y] no', 'text - [ ] no', '- [ ]x']) assert.equal(parseChecklistLine(line), undefined, line);
	assert.deepEqual(checklistCounts('- [ ] a\n  - [x] b\n* [X] c\n+ [ ] d\nplain'), {open: 2, done: 2});
	assert.deepEqual(checklistCounts(undefined), {open: 0, done: 0});
	assert.equal(checklistLabel('- [ ] a\n- [x] b'), '☐ 1 open');
	assert.equal(checklistLabel('- [x] b'), '☑ 1 done');
	assert.equal(checklistLabel('nothing'), '');
});

test('ctrl+x toggles the cursor line\'s item or makes the line one; ctrl+t and enter add items', () => {
	assert.deepEqual(toggleChecklist({text: 'a\n  - [ ] b\nc', cursor: 5}), {text: 'a\n  - [x] b\nc', cursor: 5});
	assert.deepEqual(toggleChecklist({text: '* [X] b', cursor: 7}), {text: '* [ ] b', cursor: 7});
	// A plain line gets `- [ ] ` after its indent, a bullet keeps its bullet; the cursor stays on the same text.
	assert.deepEqual(toggleChecklist({text: '  fix it', cursor: 4}), {text: '  - [ ] fix it', cursor: 10});
	assert.deepEqual(toggleChecklist({text: '- fix it', cursor: 0}), {text: '- [ ] fix it', cursor: 0});
	assert.deepEqual(toggleChecklist({text: '', cursor: 0}), {text: '- [ ] ', cursor: 6});
	// Ctrl+T: below the line with its indent (and bullet); an empty line becomes the item.
	assert.deepEqual(insertChecklistItem({text: '  * [x] a\nb', cursor: 3}), {text: '  * [x] a\n  * [ ] \nb', cursor: 18});
	assert.deepEqual(insertChecklistItem({text: 'title', cursor: 0}), {text: 'title\n- [ ] ', cursor: 12});
	assert.deepEqual(insertChecklistItem({text: 'a\n\nb', cursor: 2}), {text: 'a\n- [ ] \nb', cursor: 8});
	// Enter continues a list (splitting the item), ends it on an empty item, and is a plain newline elsewhere.
	assert.deepEqual(continueChecklist({text: '- [ ] buy milk', cursor: 10}), {text: '- [ ] buy \n- [ ] milk', cursor: 17});
	assert.deepEqual(continueChecklist({text: 'x\n  - [ ] ', cursor: 10}), {text: 'x\n', cursor: 2});
	assert.equal(continueChecklist({text: '- [ ] item', cursor: 2}), undefined);
	assert.equal(continueChecklist({text: 'plain', cursor: 5}), undefined);
});

test('read mode: wrapped rows, ☐/☑ items with a hanging indent, headings, and +N more lines when cut', () => {
	const rows = noteReadRows('# Plan\n- [ ] write the migration code\n  - [x] done\nplain words wrap here', 16);
	assert.deepEqual(rows, [
		{text: '# Plan', kind: 'heading'},
		{text: '☐ write the ', kind: 'open'},
		{text: '  migration code', kind: 'open'},
		{text: '  ☑ done', kind: 'done'},
		{text: 'plain words '},
		{text: 'wrap here'},
	]);
	// Tabs and controls are shown safely; a word longer than the width is cut.
	assert.deepEqual(noteReadRows('a\tb\x07\nabcdefghij', 4).map(row => row.text), ['a b?', 'abcd', 'efgh', 'ij']);
	assert.deepEqual(fitReadRows(rows, 6), rows);
	assert.deepEqual(fitReadRows(rows, 3), [rows[0], rows[1], {text: '+4 more lines', kind: 'more'}]);
	assert.deepEqual(fitReadRows(rows, 1), [{text: '+6 more lines', kind: 'more'}]);
	assert.deepEqual(fitReadRows(rows.slice(0, 3), 2), [rows[0], {text: '+2 more lines', kind: 'more'}]);
	assert.deepEqual(fitReadRows(rows, 0), []);
});

test('the two sections share the height: needs when they fit, else a minimum each and a proportional split favouring focus', () => {
	// Everything fits: each its need, the session section the rest.
	assert.deepEqual(budgetSections(20, 4, 6), {shared: 4, session: 16});
	// An empty (collapsed) section takes one row.
	assert.deepEqual(budgetSections(20, 1, 30), {shared: 1, session: 19});
	// Both too long: 3 each, the rest split by what is missing (equal needs, equal split), focus counts double.
	assert.deepEqual(budgetSections(20, 30, 30), {shared: 10, session: 10});
	assert.deepEqual(budgetSections(20, 30, 30, 'shared'), {shared: 12, session: 8});
	assert.deepEqual(budgetSections(20, 30, 30, 'session'), {shared: 8, session: 12});
	assert.deepEqual(budgetSections(20, 50, 10), {shared: 15, session: 5});
	// The minimum holds for a short section; a section never gets more than it needs while the other wants rows.
	assert.deepEqual(budgetSections(10, 2, 40, 'shared'), {shared: 2, session: 8});
	// Tiny panes: the focused (else the session's) section first, one row for the other.
	assert.deepEqual(budgetSections(4, 10, 10), {shared: 2, session: 2});
	assert.deepEqual(budgetSections(3, 10, 10, 'shared'), {shared: 2, session: 1});
	assert.deepEqual(budgetSections(1, 10, 10), {shared: 0, session: 1});
	// No shared section: the session's gets everything.
	assert.deepEqual(budgetSections(12, undefined, 40), {shared: 0, session: 12});
});

const section = (fields: Partial<NotesSectionInput>): NotesSectionInput => ({titles: ['Title'], text: '', empty: 'empty', ...fields});
const renderLayout = (input: Parameters<typeof notesLayout>[0]) => {
	const {rows} = notesLayout(input);
	return plain(renderToString(React.createElement(NotesRows, {rows, width: input.width, height: input.height}), {columns: input.width})).split('\n').map(line => line.trimEnd());
};

test('Notes tab layout: worktree section, a rule, the session section; empty sections collapse; long ones end in +N more lines', () => {
	const shared = section({titles: ['Worktree · feat/x (shared by 2 sessions)', 'Worktree · feat/x'], text: '- [ ] review\n- [x] merge', empty: 'No worktree notes · tab to add'});
	const own = section({titles: ['This session · fix login', 'This session'], text: Array.from({length: 8}, (_, index) => `line ${index + 1}`).join('\n')});
	assert.deepEqual(renderLayout({shared, session: own, width: 56, height: 9}), [
		'Worktree · feat/x (shared by 2 sessions) · ☐ 1 open',
		'☐ review',
		'☑ merge',
		'─'.repeat(56),
		'This session · fix login',
		'line 1',
		'line 2',
		'line 3',
		'+5 more lines',
	]);
	// Narrow headers drop the "shared by" part before the count, then cut the title; an empty section is one muted line.
	assert.equal(renderLayout({shared, session: own, width: 40, height: 9})[0], 'Worktree · feat/x · ☐ 1 open');
	assert.equal(renderLayout({shared, session: own, width: 24, height: 9})[0], 'Worktree · f… · ☐ 1 open');
	assert.deepEqual(renderLayout({shared: section({...shared, text: ''}), session: own, width: 30, height: 6}).slice(0, 4), ['Worktree · feat/x', 'No worktree notes · tab to add', '─'.repeat(30), 'This session · fix login']);
	// Without a shared note (worktree not ready), only the session's.
	assert.deepEqual(renderLayout({session: section({titles: ['This session'], text: 'only'}), width: 20, height: 3}), ['This session', 'only', '']);
});

test('Notes tab layout while editing: the focused section shows the raw text with the cursor and scrolls to keep it visible', () => {
	const text = Array.from({length: 12}, (_, index) => `row ${index + 1}`).join('\n');
	const shared = section({titles: ['Worktree · main checkout (shared by 1 session)', 'Worktree · main checkout'], text: 'shared\n- [ ] item'});
	const editing = notesLayout({shared, session: section({titles: ['This session'], text, editing: {cursor: text.length, scrollTop: 0}}), width: 40, height: 10, focus: 'session'});
	// 7 body rows: the shared section keeps its 2, the session section 5 (its need is more), scrolled to the end.
	assert.equal(editing.scrollTop, 7); assert.deepEqual(editing.bodies, {shared: 2, session: 5});
	const lines = editing.rows.map(row => row.map(part => part.text).join(''));
	assert.deepEqual(lines, ['Worktree · main checkout · ☐ 1 open', 'shared', '☐ item', '─'.repeat(40), 'This session', 'row 8', 'row 9', 'row 10', 'row 11', 'row 12 ']);
	assert.equal(editing.rows.at(-1)!.at(-1)!.inverse, true);
	// Moving the cursor to the top scrolls back; the raw checklist marker shows while editing.
	const top = notesLayout({shared: {...shared, editing: {cursor: 0, scrollTop: 0}}, session: section({titles: ['This session'], text}), width: 40, height: 10, focus: 'shared'});
	assert.deepEqual(top.rows.slice(1, 3).map(row => row.map(part => part.text).join('')), ['shared', '- [ ] item']);
	assert.equal(top.rows[1]![0]!.text, ''); assert.equal(top.rows[1]![1]!.inverse, true);
});

const session = (fields: Partial<SessionRecord>): SessionRecord => ({id: 's', title: 'task', program: 'claude', command: 'claude', cwd: '/repo', repoRoot: '/repo', status: 'running', agentStatus: 'idle', createdAt: new Date(0).toISOString(), updatedAt: new Date(0).toISOString(), ...fields});

test('sidebar details: ☐ open items of the session and its worktree, shortened to fit, the first line to go', () => {
	const withNotes = session({notes: '- [ ] a', sharedNotes: {kind: 'worktree', id: 'w', path: '/n/w.md', revision: 'r', text: '- [ ] b\n- [ ] c\n- [x] d'}});
	assert.equal(openChecklistText(withNotes, 40), '☐ 3 open (2 worktree)');
	assert.equal(openChecklistText(withNotes, 16), '☐ 3 open (2 wt)');
	assert.equal(openChecklistText(withNotes, 9), '☐ 3 open');
	assert.equal(openChecklistText(withNotes, 4), '☐ 3');
	assert.equal(openChecklistText(session({notes: '- [ ] a'}), 40), '☐ 1 open');
	assert.equal(openChecklistText(session({notes: '- [x] a'}), 40), undefined);
	const details = (free: number) => sessionDetails(withNotes, [withNotes], 30, free, 0).map(line => line.map(part => part.text).join(''));
	assert.deepEqual(details(5), ['─'.repeat(30), 'task', '✶ claude · idle · now', 'main checkout', '☐ 3 open (2 worktree)']);
	assert.deepEqual(details(4), ['─'.repeat(30), 'task', '✶ claude · idle · now', 'main checkout']);
});

test('shared note identity: the worktree record, else the main checkout\'s repository note; none while preparing', () => {
	assert.deepEqual(sharedNoteIdentity({cwd: '/wt/a', worktree: {mode: 'none', id: 'rec'}}), {kind: 'worktree', id: 'rec'});
	assert.deepEqual(sharedNoteIdentity({cwd: '/wt/a', worktree: {mode: 'managed', id: 'rec', path: '/wt/a', deletedAt: 'yesterday'}}), {kind: 'worktree', id: 'rec'});
	assert.deepEqual(sharedNoteIdentity({cwd: '/repo/sub', launchWorktreeRoot: '/repo', worktree: {mode: 'none'}}), {kind: 'repo', id: repoNoteId('/repo')});
	assert.deepEqual(sharedNoteIdentity({cwd: '/repo', worktree: {mode: 'attached', path: '/repo/', isMain: true}}), {kind: 'repo', id: repoNoteId('/repo')});
	assert.equal(sharedNoteIdentity({cwd: '/repo', requestedWorktreeMode: 'new'}), undefined);
	assert.match(repoNoteId('/repo'), /^[0-9a-f]{16}$/);
});

test('search matches worktree notes too; handoffs have a Worktree notes section', () => {
	const shared = {kind: 'worktree' as const, id: 'w', path: '/n/w.md', revision: 'r', text: 'Rollout checklist\n- [ ] flip the flag'};
	const sessions = [session({id: 'a', title: 'one', sharedNotes: shared}), session({id: 'b', title: 'two', notes: 'flag elsewhere'}), session({id: 'c', title: 'three'})];
	assert.deepEqual(filterSessionList(sessions, 'active', 'rollout').map(item => item.id), ['a']);
	assert.deepEqual(filterSessionList(sessions, 'active', 'flag').map(item => item.id), ['a', 'b']);
	const markdown = handoffMarkdown(sessions[0]!);
	assert.match(markdown, /## Notes\n\n\(No notes recorded\.\)\n\n## Worktree notes\n\nRollout checklist\n- \[ \] flip the flag/);
	assert.match(handoffMarkdown(sessions[2]!), /## Worktree notes\n\n\(No worktree notes recorded\.\)/);
});
