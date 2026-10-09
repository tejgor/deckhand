import assert from 'node:assert/strict';
import {test} from 'node:test';
import React from 'react';
import {renderToString} from 'ink';
import {doneText, formatAge, locationText, msUntilAgeChanges, sessionDetails, sidebarHeader, sidebarRows, statusSince, statusWords, type SidebarRowsInput} from '../src/sidebarModel.js';
import {Sidebar, moreText, partStyle} from '../src/sidebar.js';
import {sortSessionsForSidebar} from '../src/sessionOrder.js';
import type {SessionRecord} from '../src/types.js';

const NOW = Date.parse('2026-10-05T12:00:00.000Z');
const ago = (minutes: number) => new Date(NOW - minutes * 60_000).toISOString();
const plain = (text: string) => text.replace(/\x1b\[[0-9;]*m/g, '');
let order = 0;
const session = (id: string, fields: Partial<SessionRecord> = {}): SessionRecord => ({
	id, title: id, program: 'claude', command: 'claude', cwd: '/repo', repoRoot: '/repo', launchWorktreeRoot: '/repo', worktree: {mode: 'none'},
	status: 'running', agentStatus: 'idle', agentStatusUpdatedAt: ago(5), createdAt: ago(300), updatedAt: ago(5), sidebarOrder: order++, ...fields,
});
const auth = {mode: 'managed', id: 'w1', path: '/wt/auth', branch: 'feat/auth'} as const;
const inAuth = {cwd: '/wt/auth', launchWorktreeRoot: '/wt/auth', worktree: {mode: 'none', id: 'w1'}} as const;
// A parent with a clean and a forked child in one worktree running Dev, a codex session waiting for input, a failed
// exit, an archived merged session and one still starting.
const fixture = (): SessionRecord[] => sortSessionsForSidebar([
	session('a', {title: 'auth refactor', cwd: '/wt/auth', worktree: auth, agentStatus: 'active', attention: {state: 'working', event: 'PreToolUse', at: ago(1)}, devRunning: true}),
	session('b', {title: 'auth refactor / write tests', program: 'pi', parentSessionId: 'a', subSessionKind: 'clean', ...inAuth, devRunning: true, agentStatusUpdatedAt: ago(7)}),
	session('c', {title: 'auth refactor / try alt approach', parentSessionId: 'a', subSessionKind: 'forked', ...inAuth, devRunning: true, status: 'exited', exitReason: 'completed', agentStatusUpdatedAt: ago(42)}),
	session('d', {title: 'fix flaky checkout e2e test on CI', program: 'codex', attention: {state: 'needs-input', event: 'PermissionRequest', at: ago(12)}}),
	session('e', {title: 'migrate billing webhooks to v2', cwd: '/wt/billing', worktree: {mode: 'managed', id: 'w2', path: '/wt/billing', branch: 'feat/billing'}, status: 'exited', exitReason: 'failed', agentStatusUpdatedAt: ago(190)}),
	session('f', {title: 'docs cleanup', cwd: '/wt/docs', worktree: {mode: 'managed', id: 'w3', path: '/wt/docs', branch: 'docs/cleanup', mergedAt: ago(3000)}, status: 'exited', exitReason: 'completed', archivedAt: ago(2900), agentStatusUpdatedAt: ago(3100)}),
	session('g', {title: 'bump deps', program: 'pi', status: 'starting', agentStatus: undefined, requestedWorktreeMode: 'new', worktree: undefined}),
]);
const rows = (input: Partial<SidebarRowsInput> & {rows: SessionRecord[]}) => sidebarRows({allSessions: input.rows, firstNumber: 1, numberWidth: String(input.rows.length).length, width: 45, spinnerFrame: '⠋', filter: 'all', ...input});
const texts = (input: Partial<SidebarRowsInput> & {rows: SessionRecord[]}) => rows(input).map(row => row.parts.map(part => part.text).join(''));

test('sidebar rows: unbracketed numbers, tree and status before the title, markers and the agent last; exact width', () => {
	const all = fixture();
	assert.deepEqual(texts({rows: all, selectedId: 'd'}), [
		'  1 ▾ ⠋ auth refactor                     ▶ ✶',
		'  2   ↳ ● write tests                       π',
		'  3   ⑂ ○ try alt approach                  ✶',
		'› 4 ? fix flaky checkout e2e test on CI     ◇',
		'  5 ! migrate billing webhooks to v2        ✶',
		'  6 ○ docs cleanup                      ▣ ✓ ✶',
		'  7 ⠋ bump deps                             π',
	]);
	// Narrow (24 columns: 21 with the cursor column): titles are cut, markers stay.
	const narrow = texts({rows: all, selectedId: 'd', width: 21});
	assert.deepEqual(narrow, [
		'  1 ▾ ⠋ auth ref… ▶ ✶',
		'  2   ↳ ● write te… π',
		'  3   ⑂ ○ try alt … ✶',
		'› 4 ? fix flaky ch… ◇',
		'  5 ! migrate bill… ✶',
		'  6 ○ docs cle… ▣ ✓ ✶',
		'  7 ⠋ bump deps     π',
	]);
	for (const row of narrow) assert.equal(row.length, 21);
	// Numbers are positions in the whole list, right-aligned to the widest; the parts carry roles for styling.
	const [row] = rows({rows: [all[3]!], firstNumber: 12, numberWidth: 2, selectedId: 'x'});
	assert.equal(row!.parts.map(part => part.text).join(''), '  12 ? fix flaky checkout e2e test on CI    ◇');
	assert.deepEqual(row!.parts.filter(part => part.text.trim()).map(part => part.role), ['number', 'status', 'title', 'agent']);
	// Deep nesting at a narrow width gives up the count, then other markers, before the title gets under 4 columns.
	const deep = [session('p0', {cleanupError: 'x'})];
	for (let level = 1; level <= 5; level++) deep.push(session(`p${level}`, {parentSessionId: `p${level - 1}`, subSessionKind: 'clean', mergedAt: ago(1)}));
	const collapsed = texts({rows: deep, width: 21, collapsedSessionIds: new Set(['p2', 'p4'])});
	assert.equal(collapsed[0], '  1 ▾ ● p0        ! ✶');
	// p2 (collapsed, merged) drops its +3 count; at depth 4 only the agent stays and the title gets what is left.
	assert.equal(collapsed[2], '  3     ▸ ↳ ● p2  ✓ ✶');
	assert.equal(collapsed[4], '  5         ▸ ↳ ● p ✶');
});

test('sidebar rows: ▶ once per workspace, on its first row on screen; rows sharing the selected workspace get ╎', () => {
	const all = fixture();
	const marks = (selectedId: string, list = all) => rows({rows: list, selectedId, allSessions: all}).map(row => `${row.parts[0]!.text}${row.parts.some(part => part.role === 'dev') ? '▶' : ''}`);
	assert.deepEqual(marks('b'), ['╎▶', '›', '╎', ' ', ' ', ' ', ' ']);
	// Scrolled past the parent: the first row of the workspace on screen carries ▶.
	assert.deepEqual(marks('c', all.slice(1, 4)), ['╎▶', '›', ' ']);
	// Main-checkout sessions share /repo; a session without a workspace keeps its own ▶.
	const mainA = session('m1', {devRunning: true}), mainB = session('m2', {devRunning: true});
	const loose = session('m3', {devRunning: true, requestedWorktreeMode: 'new', worktree: undefined});
	const loose2 = session('m4', {devRunning: true, worktree: {mode: 'managed', path: '/wt/x', deletedAt: ago(1)}});
	assert.deepEqual(rows({rows: [mainA, mainB, loose, loose2], selectedId: 'm2'}).map(row => `${row.parts[0]!.text}${row.parts.some(part => part.role === 'dev') ? '▶' : ''}`), ['╎▶', '›', ' ▶', ' ▶']);
	// No marker without a selection on screen, and none for an unshared workspace.
	assert.deepEqual(marks('none'), [' ▶', ' ', ' ', ' ', ' ', ' ', ' ']);
	assert.deepEqual(marks('e'), [' ▶', ' ', ' ', ' ', '›', ' ', ' ']);
});

test('sidebar rows: archived rows dimmed, except in the archived view, which dims the context ancestors instead', () => {
	const parent = session('parent', {title: 'parent'});
	const archivedChild = session('child', {parentSessionId: 'parent', subSessionKind: 'clean', archivedAt: ago(1)});
	const archived = session('old', {archivedAt: ago(1)});
	const list = [parent, archivedChild, archived];
	const dimmed = (filter: SidebarRowsInput['filter'], selectedId?: string) => rows({rows: list, filter, selectedId}).map(row => row.dimmed);
	assert.deepEqual(dimmed('all'), [false, true, true]);
	assert.deepEqual(dimmed('archived'), [true, false, false]);
	// The selected row is never dimmed; ▣ stays.
	assert.deepEqual(dimmed('all', 'old'), [false, true, false]);
	assert.ok(rows({rows: list, filter: 'all'})[2]!.parts.some(part => part.text === '▣'));
});

test('sidebar header: plain count by default, filter/search with counts otherwise, attention count in front of the title', () => {
	const all = fixture();
	const header = (fields: Partial<Parameters<typeof sidebarHeader>[0]>) => sidebarHeader({width: 30, filter: 'active', query: '', shown: 6, total: 7, allSessions: all, ...fields});
	// d needs input and e failed; an archived session needing attention is not counted.
	assert.deepEqual(header({}), {title: 'Sessions', label: '6', attention: '! 2', highlighted: false});
	assert.deepEqual(header({allSessions: [all[0]!, {...all[4]!, archivedAt: ago(1)}]}), {title: 'Sessions', label: '6', attention: '', highlighted: false});
	assert.deepEqual(header({filter: 'archived', shown: 1}), {title: 'Sessions', label: 'archived 1/7', attention: '! 2', highlighted: true});
	assert.deepEqual(header({query: 'auth', shown: 3}), {title: 'Sessions', label: '/auth 3/7', attention: '! 2', highlighted: true});
	assert.deepEqual(header({filter: 'attention', query: ' auth ', shown: 2, allSessions: []}), {title: 'Sessions', label: 'attention /auth 2/7', attention: '', highlighted: true});
	// Narrow: the title goes first, then the filter/search is cut; the count and attention stay.
	assert.deepEqual(header({width: 20, filter: 'archived', shown: 1}), {title: '', label: 'archived 1/7', attention: '! 2', highlighted: true});
	assert.deepEqual(header({width: 20, query: 'checkout flake', shown: 1}), {title: '', label: '/checkout… 1/7', attention: '! 2', highlighted: true});
	assert.deepEqual(header({width: 12, query: 'checkout flake', shown: 1}), {title: '', label: '1/7', attention: '! 2', highlighted: true});
});

test('sidebar details: title, agent · state · age, location; shrinks with fewer free rows', () => {
	const all = fixture();
	const byId = (id: string) => all.find(item => item.id === id)!;
	const details = (id: string | undefined, width = 30, free = 10, list = all) => sessionDetails(id ? list.find(item => item.id === id) : undefined, list, width, free, NOW).map(line => line.map(part => part.text).join(''));
	assert.deepEqual(details('d'), ['─'.repeat(30), 'fix flaky checkout e2e test on', 'CI', '◇ codex · needs input · 12m', 'main checkout']);
	// Title to one line with four free rows, the location dropped with three, nothing below that or without a selection.
	assert.deepEqual(details('d', 30, 4), ['─'.repeat(30), 'fix flaky checkout e2e test o…', '◇ codex · needs input · 12m', 'main checkout']);
	assert.deepEqual(details('d', 30, 3), ['─'.repeat(30), 'fix flaky checkout e2e test o…', '◇ codex · needs input · 12m']);
	assert.deepEqual(details('d', 30, 2), []);
	assert.deepEqual(details(undefined), []);
	// More than two lines of title: the second ends in ….
	assert.deepEqual(details('d', 12).slice(1, 3), ['fix flaky', 'checkout e2…']);
	// Worktree sessions: the branch (a sub-session without its own takes the workspace's), who shares it, Dev.
	assert.deepEqual(details('b', 48).slice(2), ['π pi · idle · 7m', '⎇ feat/auth · shared with 2 · ▶ dev']);
	assert.deepEqual(details('c', 48).slice(2), ['✶ claude · exited · 42m', '⎇ feat/auth · shared with 2 · ▶ dev']);
	assert.deepEqual(details('e', 48).slice(2), ['✶ claude · exited (failed) · 3h', '⎇ feat/billing']);
	assert.deepEqual(details('f', 48).slice(2), ['✶ claude · exited · 2d', '⎇ docs/cleanup · merged · archived']);
	assert.deepEqual(details('g', 48).slice(2), ['π pi · starting · 5m', 'preparing worktree']);
	// Narrow: the agent's name goes before the age, the age before the state.
	assert.deepEqual(details('e', 20).slice(3), ['✶ exited (failed)', '⎇ feat/billing']);
	assert.deepEqual(details('c', 20).slice(2), ['✶ exited · 42m', '⎇ feat/auth …']);
	// The location is cut at whole markers (… marks the rest); only a long branch itself is cut mid-word.
	assert.equal(locationText(byId('b'), all, 30), '⎇ feat/auth · shared with 2 …');
	assert.equal(locationText(byId('b'), all, 35), '⎇ feat/auth · shared with 2 · ▶ dev');
	assert.equal(locationText(session('long', {worktree: {...auth, id: 'w8', path: '/wt/long', branch: 'feature/a-very-long-branch'}, archivedAt: ago(1)}), [], 20), '⎇ feature/a-very-lo…');
	const deleted = session('z', {title: 'gone', worktree: {...auth, id: 'w9', deletedAt: ago(1)}, status: 'exited', exitReason: 'interrupted'});
	assert.deepEqual(details('z', 48, 10, [...all, deleted]).slice(2), ['✶ claude · interrupted · 5m', 'worktree deleted']);
	assert.equal(locationText(byId('d'), [...all, session('d2')]), 'main checkout · shared with 1');
	assert.equal(locationText(session('main', {worktree: {mode: 'attached', path: '/repo', isMain: true, branch: 'main'}}), []), 'main checkout');
	assert.equal(locationText(session('nb', {cwd: '/wt/x', launchWorktreeRoot: '/wt/x', worktree: {mode: 'none', id: 'w7'}}), []), 'worktree x');
});

test('sidebar details: state words and their timestamps follow the status glyph inputs; compact ages', () => {
	const words = (fields: Partial<SessionRecord>) => statusWords(session('s', fields));
	assert.equal(words({}), 'idle');
	assert.equal(words({agentStatus: 'active'}), 'working');
	assert.equal(words({agentStatus: 'unknown'}), 'running');
	assert.equal(words({attention: {state: 'unknown', event: 'SessionStart', at: ago(1)}, agentStatus: undefined}), 'running');
	assert.equal(words({attention: {state: 'response-ended', event: 'Stop', at: ago(1)}}), 'response ended');
	assert.equal(words({attention: {state: 'failed', event: 'StopFailure', at: ago(1)}}), 'failed');
	assert.equal(words({attention: {state: 'limited', event: 'StopFailure', at: ago(1)}}), 'rate-limited');
	assert.equal(words({status: 'starting'}), 'starting');
	assert.equal(words({status: 'exited', exitReason: 'stopped', attention: {state: 'needs-input', event: 'x', at: ago(1)}}), 'exited');
	assert.equal(statusSince(session('s', {attention: {state: 'needs-input', event: 'x', at: ago(12)}})), ago(12));
	assert.equal(statusSince(session('s', {status: 'exited', attention: {state: 'needs-input', event: 'x', at: ago(12)}})), ago(5));
	assert.equal(statusSince(session('s', {agentStatusUpdatedAt: undefined, updatedAt: ago(3)})), ago(3));
	assert.deepEqual([0, 59_999, 60_000, 59 * 60_000, 3_600_000, 23 * 3_600_000 + 1, 86_400_000 * 3, -5, NaN].map(formatAge), ['now', 'now', '1m', '59m', '1h', '23h', '3d', 'now', 'now']);
	// The next change of the shown age: the next minute, hour or day boundary.
	assert.equal(msUntilAgeChanges(30_000), 30_050);
	assert.equal(msUntilAgeChanges(90 * 60_000), 30 * 60_000 + 50);
	assert.equal(msUntilAgeChanges(-1), 60_000);
});

test('Sidebar renders the cursor in the padding column and pins the details to the bottom', () => {
	const all = fixture();
	const lines = plain(renderToString(React.createElement(Sidebar, {sessions: all, allSessions: all, selectedId: 'd', width: 34, height: 16, spinnerFrame: '⠋', filter: 'all', query: '', now: NOW}), {columns: 34})).split('\n');
	assert.deepEqual(lines, [
		'╭────────────────────────────────╮',
		'│ Sessions         all 7/7 · ! 2 │',
		'│  1 ▾ ⠋ auth refactor       ▶ ✶ │',
		'│  2   ↳ ● write tests         π │',
		'│  3   ⑂ ○ try alt approach    ✶ │',
		'│› 4 ? fix flaky checkout e2e… ◇ │',
		'│  5 ! migrate billing webhoo… ✶ │',
		'│  6 ○ docs cleanup        ▣ ✓ ✶ │',
		'│  7 ⠋ bump deps               π │',
		'│                                │',
		'│ ────────────────────────────── │',
		'│ fix flaky checkout e2e test on │',
		'│ CI                             │',
		'│ ◇ codex · needs input · 12m    │',
		'│ main checkout                  │',
		'╰────────────────────────────────╯',
	]);
});

test('sidebar done marker: ☑ after ✓ in the suffix, dropped after ✓ when narrow; done rows slightly dimmed; details say done 2d ago', () => {
	// A worktree session merged and done, a main-checkout session only done, an archived done one, and a plain one.
	const both = session('both', {title: 'ship auth', cwd: '/wt/auth', worktree: {...auth, mergedAt: ago(10)}, doneAt: ago(3000), status: 'exited', exitReason: 'completed'});
	const main = session('main', {title: 'tidy readme', doneAt: ago(30)});
	const archived = session('old', {title: 'old idea', doneAt: ago(60), archivedAt: ago(50), status: 'exited'});
	const plainRow = session('plain', {title: 'plain'});
	const list = [both, main, archived, plainRow];
	assert.deepEqual(texts({rows: list, width: 34}), [
		'  1 ○ ship auth              ✓ ☑ ✶',
		'  2 ● tidy readme              ☑ ✶',
		'  3 ○ old idea               ▣ ☑ ✶',
		'  4 ● plain                      ✶',
	]);
	assert.ok(rows({rows: [main]})[0]!.parts.some(part => part.role === 'done' && part.text === '☑'));
	// Narrow: ✓ goes first, then ☑, then ▣; the agent glyph stays.
	assert.deepEqual(texts({rows: [both], width: 14}), ['  1 ○ shi… ☑ ✶']);
	assert.deepEqual(texts({rows: [both], width: 12}), ['  1 ○ shi… ✶']);
	assert.deepEqual(texts({rows: [archived], width: 14}), ['  1 ○ old… ▣ ✶']);
	// Dimming: done rows are `done` (muted title), never archive-dimmed for it; archived rules win, the selected row is neither.
	const flags = (filter: SidebarRowsInput['filter'], selectedId?: string) => rows({rows: list, filter, selectedId}).map(row => `${row.dimmed ? 'dim' : ''}${row.done ? 'done' : ''}` || '-');
	assert.deepEqual(flags('all'), ['done', 'done', 'dim', '-']);
	assert.deepEqual(flags('archived'), ['dim', 'dim', 'done', 'dim']);
	assert.deepEqual(flags('all', 'main'), ['done', '-', 'dim', '-']);
	// Details: `done 2d ago` after merged, before archived; `done now` within a minute.
	const detail = (item: SessionRecord) => sessionDetails(item, [item], 48, 10, NOW).map(line => line.map(part => part.text).join('')).at(-1);
	assert.equal(detail(both), '⎇ feat/auth · merged · done 2d ago');
	assert.equal(detail(main), 'main checkout · done 30m ago');
	assert.equal(detail(archived), 'main checkout · done 1h ago · archived');
	assert.equal(doneText(session('x', {doneAt: ago(0)}), NOW), 'done now');
	assert.equal(doneText(main), 'done');
	assert.equal(locationText(both, [both], 30, NOW), '⎇ feat/auth · merged …');
	// Styles: a done row's title is muted, not dim; its ☑ readable; the status glyph keeps its color. Archived ones stay dim.
	const [, mainRow, archivedRow] = rows({rows: list, filter: 'all'});
	const style = (row: typeof mainRow, role: string) => partStyle(row!.parts.find(part => part.role === role)!, row!);
	assert.deepEqual(style(mainRow, 'title'), {color: 'gray'});
	assert.deepEqual(style(mainRow, 'done'), {color: 'gray'});
	assert.deepEqual(style(mainRow, 'status'), {color: 'green'});
	assert.deepEqual(style(archivedRow, 'title'), {color: 'gray', dimColor: true});
	assert.deepEqual(style(archivedRow, 'done'), {color: 'gray'});
	const out = renderToString(React.createElement(Sidebar, {sessions: [main, plainRow], allSessions: [main, plainRow], selectedId: 'plain', width: 30, height: 6, spinnerFrame: '⠋', filter: 'active', query: '', now: NOW}), {columns: 30});
	const doneLine = out.split('\n').find(line => line.includes('tidy readme'))!;
	assert.match(plain(doneLine), /☑ ✶/);
});

test('Sidebar with more sessions than fit keeps rows for the details and scrolls the list around the selection', () => {
	const many = Array.from({length: 40}, (_, index) => session(`s${index}`, {title: `session number ${index}`}));
	const lines = plain(renderToString(React.createElement(Sidebar, {sessions: many, allSessions: many, selectedId: 's20', width: 34, height: 24, spinnerFrame: '⠋', filter: 'all', query: '', now: NOW}), {columns: 34})).split('\n');
	const listRows = lines.filter(line => /^│.\s*\d+ /.test(line));
	// 24 rows: border, header and border leave 21; the details keep the four they use, the two "more" lines two.
	assert.equal(listRows.length, 15);
	assert.ok(listRows.some(line => line.includes('› 21 ● session number 20')), 'the selection stays in view');
	assert.ok(lines.some(line => line.includes('↑ 13 more')), 'sessions above are counted');
	assert.ok(lines.some(line => line.includes('↓ 12 more')), 'sessions below are counted');
	assert.ok(lines.some(line => line.includes('──────')), 'the details rule is shown');
	assert.ok(lines.some(line => line.includes('✶ claude · idle')), 'the details state line is shown');
	// A short list still leaves the details only what it does not use.
	const few = many.slice(0, 3);
	const short = plain(renderToString(React.createElement(Sidebar, {sessions: few, allSessions: few, selectedId: 's1', width: 34, height: 24, spinnerFrame: '⠋', filter: 'all', query: '', now: NOW}), {columns: 34})).split('\n');
	assert.equal(short.filter(line => /^│.\s*\d+ /.test(line)).length, 3);
	assert.ok(!short.some(line => line.includes(' more')), 'a list that fits has no "more" lines');
	// Hidden sessions that need you are counted on their side.
	const waiting = session('w', {attention: {state: 'needs-input', event: 'PermissionRequest', at: ago(1)}});
	assert.equal(moreText('↓', [many[0]!, waiting]), '↓ 2 more · ! 1');
	assert.equal(moreText('↑', []), '');
});

test('a scrolling list keeps the same rows whichever session is selected: the details get the largest block any listed session needs', () => {
	const many = Array.from({length: 40}, (_, index) => session(`s${index}`, {title: `session number ${index}`}));
	const taskOf = (entry: SessionRecord) => (entry.id === 's21' ? {title: 'Fix the footer', done: false} : undefined);
	const render = (selectedId: string) => plain(renderToString(React.createElement(Sidebar, {sessions: many, allSessions: many, selectedId, width: 34, height: 24, spinnerFrame: '⠋', filter: 'all', query: '', now: NOW, taskOf}), {columns: 34})).split('\n');
	const withTask = render('s21'), without = render('s20');
	const shape = (lines: string[]) => ({list: lines.filter(line => /^│.\s*\d+ /.test(line)).length, more: lines.findIndex(line => line.includes('↓ ')), rule: lines.findIndex(line => line.startsWith('│ ──────'))});
	assert.deepEqual(shape(without), shape(withTask));
	// The rule sits right under the "more" line; the session with a task shows it, the other leaves that row blank.
	assert.equal(shape(withTask).rule, shape(withTask).more + 1);
	assert.ok(withTask.some(line => line.includes('◆ Fix the footer')));
	assert.ok(!without.some(line => line.includes('◆')));
});
