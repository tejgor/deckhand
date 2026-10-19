/**
 * Gen script for src/worktreesModel.ts. Regenerate with `node --import tsx parity/gen/worktreesModel.gen.ts`.
 *
 * Constants and records (WORKTREE_GROUPS, GROUP_TITLES, IDLE_DAYS) are covered by the port's own tests. worktreeName
 * uses path.basename (POSIX paths only are generated). Calls whose output would hold a lone surrogate are not recorded.
 */
import {Recorder, arb, decode, encode, fc, installDeterminism, mutate, runTap, sample} from '../lib.ts';
import {bulkTargets, deleteRefusal, lastActivity, mergedText, protectedBranch, selectableWorktreeRow, worktreeGroup, worktreeName, worktreeRowKey, worktreeRows, worktreeTags} from '../../src/worktreesModel.ts';

const MODULE = 'worktreesModel';
installDeterminism();
const recorder = new Recorder(MODULE);

function rec(name: string, fn: Function, args: unknown[]): void {
	let out: unknown;
	try { out = fn(...structuredClone(args)); } catch { recorder.call(name, fn, structuredClone(args)); return; }
	if (JSON.stringify(encode(out)).includes('"$utf16"')) return;
	recorder.call(name, fn, structuredClone(args));
}
const fns: Record<string, Function> = {mergedText, lastActivity, worktreeGroup, worktreeRows, worktreeName, selectableWorktreeRow, worktreeRowKey, deleteRefusal, protectedBranch, bulkTargets, worktreeTags};

// 1. Tap the existing tests.
const tapped = await runTap({tests: ['tests/worktreesModel.test.ts'], tap: Object.keys(fns).map(name => `worktreesModel:${name}`)});
for (const record of tapped) if (record.module === 'worktreesModel') rec(record.fn, fns[record.fn]!, decode(JSON.parse(record.line).args) as unknown[]);
const seeds = (name: string) => recorder.argsOf(name);

// 2. Building blocks.
const NOW = Date.parse('2026-10-09T12:00:00Z');
const DAY = 24 * 60 * 60_000;
const iso = (offsetMs: number) => new Date(NOW + offsetMs).toISOString();
const clean = {safe: true, reasons: [] as string[], dirtyFiles: 0, untrackedFiles: 0, ignoredFiles: 0};
const dirty = {safe: false, reasons: ['3 modified/staged file(s)'], dirtyFiles: 3, untrackedFiles: 2, ignoredFiles: 7};
const entry = (overrides: object = {}) => ({path: '/w/x', branch: 'x', head: 'a'.repeat(40), isMain: false, sessionIds: [], runningIds: [], lastCommitAt: iso(-DAY), inspection: clean, aheadOfDefault: 2, ...overrides});
const session = (id: string, overrides: object = {}) => ({id, title: id, program: 'claude', command: 'claude', args: [], cwd: '/w', repoRoot: '/r', status: 'exited', createdAt: iso(-30 * DAY), updatedAt: iso(-30 * DAY), ...overrides});
const noteSession = (id: string, recordId: string, text: string, extra: object = {}) => session(id, {worktree: {mode: 'managed', id: recordId}, sharedNotes: {kind: 'worktree', id: recordId, text, path: '/n.md', revision: 'r1'}, ...extra});

const markerSets: Array<object | undefined> = [undefined, {}, {mergedAt: 't'}, {mergedAt: 't', mergeDetected: 'pr'}, {mergedAt: 't', mergeDetected: 'ancestor'}, {mergedAt: 't', mergeDetected: 'ancestor', mergeTargetBranch: 'dev'},
	{mergedAt: 't', mergeMarkedManually: true}, {mergedAt: 't', mergeMode: 'squash'}, {mergedAt: 't', mergeMode: 'squash', mergeTargetBranch: 'main'}, {mergedAt: 't', mergeMode: 'merge', mergeTargetBranch: '功能/😀'},
	{mergedAt: 't', mergeMode: 'merge'}, {mergedAt: '', mergeMode: 'merge'}, {mergeMode: 'squash'}, {mergedAt: 't', mergeDetected: 'pr', mergeMarkedManually: true}, {mergedAt: 't', mergeTargetBranch: ''}];
for (const markers of markerSets) for (const defaultBranch of [undefined, 'main', '']) rec('mergedText', mergedText, [entry(markers ? {markers} : {}), ...defaultBranch === undefined ? [] : [defaultBranch]]);
for (const defaultBranch of [undefined, 'main', 'trunk 😀']) {
	for (const overrides of [{aheadOfDefault: 0}, {aheadOfDefault: 0, head: ''}, {aheadOfDefault: 0, inspection: undefined}, {aheadOfDefault: 0, inspection: dirty}, {aheadOfDefault: undefined}, {aheadOfDefault: 1}, {aheadOfDefault: 0, error: 'x', inspection: undefined}]) rec('mergedText', mergedText, [entry(overrides), ...defaultBranch === undefined ? [] : [defaultBranch]]);
}

for (const branch of ['x', 'feat/ü', '', '中文-分支', '😀']) for (const p of ['/w/x', '/a/b/c/', '/', '', '/w/中文目录', '/w/dir.with.dots', '/w//double']) rec('worktreeName', worktreeName, [entry({branch, path: p})]);

const sessionsFor = [[], [session('s')], [session('s', {updatedAt: iso(-DAY)})], [session('s', {updatedAt: 'garbage'}), session('t', {updatedAt: iso(-2 * DAY)})], [session('other', {updatedAt: iso(0)})]];
const activityEntries = [entry({sessionIds: ['s']}), entry({sessionIds: ['s', 't'], createdAt: iso(-3 * DAY)}), entry({lastCommitAt: undefined, createdAt: undefined}), entry({lastCommitAt: 'bad', createdAt: iso(-5 * DAY)}), entry({lastCommitAt: '', createdAt: ''}), entry({lastCommitAt: iso(2 * DAY), sessionIds: ['s']})];
for (const e of activityEntries) for (const sessions of sessionsFor) rec('lastActivity', lastActivity, [e, sessions]);

const old = entry({lastCommitAt: iso(-20 * DAY), sessionIds: ['s'], recordId: 'r', createdAt: iso(-40 * DAY)});
const groupCases: Array<[object, object[]]> = [
	[entry({isMain: true, aheadOfDefault: 0}), []], [entry({missing: 'prunable', markers: {mergedAt: 't'}}), []], [entry({missing: 'unlisted'}), []], [entry({isMain: true, missing: 'prunable'}), []],
	[entry({markers: {mergedAt: 't'}}), []], [entry({markers: {mergedAt: 't'}, inspection: dirty}), []], [entry({markers: {mergedAt: 't'}, inspection: undefined, error: 'boom'}), []],
	[entry({aheadOfDefault: 0}), []], [entry({aheadOfDefault: 0, inspection: dirty}), []], [entry({}), []], [old, [session('s')]], [{...old, runningIds: ['s']}, [session('s', {status: 'running'})]],
	[old, [session('s', {updatedAt: iso(-DAY)})]], [entry({lastCommitAt: undefined, createdAt: undefined}), []], [entry({lastCommitAt: iso(-14 * DAY)}), []], [entry({lastCommitAt: iso(-14 * DAY - 1)}), []], [entry({lastCommitAt: iso(-14 * DAY + 1)}), []],
];
for (const [e, sessions] of groupCases) for (const defaultBranch of [undefined, 'main']) for (const now of [NOW, NOW + 30 * DAY, 0]) rec('worktreeGroup', worktreeGroup, [e, sessions, now, ...defaultBranch ? [defaultBranch] : []]);

// A mixed overview exercising every group, tie-breaking by activity, and the empty rows.
const mixed = [
	entry({path: '/r', branch: 'main', isMain: true, aheadOfDefault: 0}),
	entry({path: '/w/ready1', branch: 'ready1', markers: {mergedAt: 't', mergeMode: 'squash'}, lastCommitAt: iso(-2 * DAY)}),
	entry({path: '/w/ready2', branch: 'ready2', aheadOfDefault: 0, lastCommitAt: iso(-1 * DAY)}),
	entry({path: '/w/left', branch: 'left', markers: {mergedAt: 't'}, inspection: dirty}),
	entry({path: '/w/prog', branch: 'prog'}), entry({path: '/w/prog2', branch: 'prog2', lastCommitAt: iso(-3 * DAY)}),
	entry({path: '/w/idle', branch: 'idle', lastCommitAt: iso(-40 * DAY)}),
	entry({path: '/w/gone', branch: 'gone', missing: 'prunable'}), entry({path: '/w/detached', branch: '', head: 'b'.repeat(40), lastCommitAt: undefined, createdAt: undefined}),
	entry({path: '/w/locked', branch: 'locked', locked: true, inUse: 'this', markers: {mergedAt: 't'}}),
];
const overviews: Array<object | undefined> = [undefined, {entries: [], checkedAt: iso(0)}, {entries: [mixed[0]], checkedAt: iso(0), defaultBranch: 'main'}, {entries: mixed, defaultBranch: 'main', mainRoot: '/r', checkedAt: iso(0)}, {entries: mixed, checkedAt: iso(0)}, {entries: [...mixed].reverse(), defaultBranch: 'main', unchecked: 4, checkedAt: iso(0)}, {entries: [mixed[1]!], checkedAt: iso(0)}];
for (const overview of overviews) for (const now of [NOW, NOW + 20 * DAY]) rec('worktreeRows', worktreeRows, [overview, [session('s'), session('t', {updatedAt: iso(-DAY)})], now]);

for (const row of [{kind: 'heading', group: 'ready', count: 2}, {kind: 'heading', group: 'idle', count: 1}, {kind: 'heading', group: 'main', count: 1}, {kind: 'worktree', group: 'ready', entry: entry()}, {kind: 'worktree', group: 'main', entry: entry({path: '/r/中文'})}, {kind: 'empty', text: 'No other worktrees'}]) {
	rec('selectableWorktreeRow', selectableWorktreeRow, [row]);
	rec('worktreeRowKey', worktreeRowKey, [row]);
}

for (const overrides of [{}, {isMain: true}, {inUse: 'this'}, {inUse: 'other'}, {locked: true}, {locked: true, missing: 'prunable'}, {locked: true, missing: 'unlisted'}, {isMain: true, inUse: 'this', locked: true}, {inUse: 'this', locked: true}, {inUse: 'other', locked: true}, {missing: 'prunable'}]) rec('deleteRefusal', deleteRefusal, [entry(overrides)]);

for (const [branch, def] of [['main'], ['master'], ['dev', 'dev'], ['dev', 'main'], ['dev'], ['', ''], ['', undefined], ['Main'], ['main ', 'main'], ['😀', '😀'], ['feature/中文', 'release'], ['trunk', 'trunk']] as Array<[string, string?]>) rec('protectedBranch', protectedBranch, def === undefined ? [branch] : [branch, def]);

const rowsOf = (overview: object | undefined) => worktreeRows(overview as any, [], NOW);
for (const overview of overviews) rec('bulkTargets', bulkTargets, [rowsOf(overview)]);
rec('bulkTargets', bulkTargets, [[]]);
rec('bulkTargets', bulkTargets, [rowsOf({entries: [entry({path: '/w/a', branch: 'a', aheadOfDefault: 0}), entry({path: '/w/b', branch: 'b', aheadOfDefault: 0, locked: true}), entry({path: '/w/c', branch: 'c', aheadOfDefault: 0, inUse: 'other'}), entry({path: '/w/d', branch: 'd', aheadOfDefault: 0, inUse: 'this'}), entry({path: '/w/e', branch: 'e', aheadOfDefault: 0, locked: true, missing: 'prunable'})], checkedAt: iso(0)})]);

const tagEntries = [
	entry(), entry({missing: 'prunable'}), entry({missing: 'unlisted'}), entry({error: 'boom'}), entry({inspection: dirty}), entry({inspection: {...clean, ignoredFiles: 4}}), entry({inspection: {...clean, untrackedFiles: 1}}),
	entry({aheadOfDefault: 1}), entry({aheadOfDefault: 0}), entry({aheadOfDefault: undefined}), entry({runningIds: ['a'], sessionIds: ['a', 'b']}), entry({sessionIds: ['a']}), entry({sessionIds: ['a', 'b', 'c']}),
	entry({recordId: undefined}), entry({recordId: 'rec'}), entry({isMain: true, recordId: undefined}), entry({lastCommitAt: undefined, createdAt: undefined}), entry({lastCommitAt: iso(-30 * 60_000)}), entry({lastCommitAt: iso(-30_000)}), entry({lastCommitAt: iso(-5 * 3600_000)}), entry({lastCommitAt: iso(-45 * DAY)}), entry({lastCommitAt: iso(60_000)}),
	entry({missing: 'prunable', error: 'e', inspection: dirty, aheadOfDefault: 3, recordId: 'rec', sessionIds: ['s1'], runningIds: ['s1']}),
];
const tagSessions: object[][] = [[], [session('s1')], [noteSession('s1', 'rec', '- [ ] one\n- [x] done\n- [ ] two')], [noteSession('s1', 'rec', '- [ ] one', {archivedAt: iso(-DAY)}), noteSession('s2', 'rec', '- [ ] a\n- [ ] b\n- [ ] c')], [noteSession('s1', 'rec', '- [ ] one', {sharedNotes: {kind: 'worktree', id: 'rec', text: '- [ ] one', path: '/n', revision: 'r', readOnly: true}})], [noteSession('s1', 'other', '- [ ] x')], [noteSession('s1', 'rec', '- [ ] 😀 item\n- [ ]   \n↗ link')]];
for (const e of tagEntries) for (const sessions of tagSessions) rec('worktreeTags', worktreeTags, [e, sessions, NOW]);

// 3. Mutations of recorded inputs.
for (const name of ['mergedText', 'lastActivity', 'worktreeGroup', 'deleteRefusal', 'worktreeName', 'worktreeTags', 'protectedBranch', 'worktreeRows'] as const) {
	for (const args of mutate(seeds(name), {variants: 3})) rec(name, fns[name]!, args);
}

// 4. fast-check generated entries, sessions and overviews.
const text = arb.text(10);
const when = fc.option(fc.integer({min: -60, max: 5}).chain(days => fc.integer({min: 0, max: 86_399_999}).map(ms => iso(days * DAY + ms))), {nil: undefined});
const strip = (value: object) => Object.fromEntries(Object.entries(value).filter(([, v]) => v !== undefined));
const markersArb = fc.option(fc.record({mergedAt: fc.option(fc.constantFrom('t'), {nil: undefined}), mergeMode: fc.option(fc.constantFrom('merge', 'squash'), {nil: undefined}), mergeTargetBranch: fc.option(text, {nil: undefined}), mergeMarkedManually: fc.option(fc.boolean(), {nil: undefined}), mergeDetected: fc.option(fc.constantFrom('pr', 'ancestor'), {nil: undefined})}).map(strip), {nil: undefined});
const inspectionArb = fc.option(fc.record({dirtyFiles: fc.nat(3), untrackedFiles: fc.nat(3), ignoredFiles: fc.nat(3), safe: fc.boolean(), reasons: fc.constant([] as string[])}), {nil: undefined});
const ids = ['s1', 's2', 's3'];
const entryArb = fc.record({
	path: fc.array(text.map(part => part.replaceAll('/', '_')), {minLength: 1, maxLength: 3}).map(parts => '/' + parts.join('/')), branch: fc.oneof(text, fc.constant('')), head: fc.constantFrom('', 'c'.repeat(40)), isMain: fc.boolean(),
	locked: fc.option(fc.boolean(), {nil: undefined}), missing: fc.option(fc.constantFrom('prunable', 'unlisted'), {nil: undefined}), recordId: fc.option(fc.constantFrom('rec', 'rec2'), {nil: undefined}), createdAt: when, markers: markersArb,
	sessionIds: fc.subarray(ids), runningIds: fc.subarray(ids), inUse: fc.option(fc.constantFrom('this', 'other'), {nil: undefined}), lastCommitAt: when, aheadOfDefault: fc.option(fc.nat(4), {nil: undefined}), inspection: inspectionArb, error: fc.option(text, {nil: undefined}),
}).map(strip);
const sessionArb = fc.record({id: fc.constantFrom(...ids), updatedAt: fc.oneof(when.map(value => value ?? iso(0)), fc.constant('nope')), status: fc.constantFrom('running', 'exited'), notes: fc.option(fc.constantFrom('- [ ] a\n- [ ] b', '- [x] a', 'plain'), {nil: undefined}), rec: fc.constantFrom('rec', 'rec2', undefined)})
	.map(value => value.rec && value.notes ? noteSession(value.id, value.rec, value.notes, {updatedAt: value.updatedAt, status: value.status}) : session(value.id, {updatedAt: value.updatedAt, status: value.status}));
const sessionsArb = fc.array(sessionArb, {maxLength: 4});
const nowArb = fc.integer({min: -5, max: 40}).map(days => NOW + days * DAY);
const defaultArb = fc.option(fc.constantFrom('main', 'master', 'trunk', 'x'), {nil: undefined});
for (const [e, sessions, now, def] of sample(fc.tuple(entryArb, sessionsArb, nowArb, defaultArb), 200, 21)) {
	rec('worktreeGroup', worktreeGroup, [e, sessions, now, ...def === undefined ? [] : [def]]);
	rec('worktreeTags', worktreeTags, [e, sessions, now]);
	rec('lastActivity', lastActivity, [e, sessions]);
	rec('mergedText', mergedText, [e, ...def === undefined ? [] : [def]]);
	rec('deleteRefusal', deleteRefusal, [e]);
	rec('worktreeName', worktreeName, [e]);
}
for (const [entries, sessions, now, def] of sample(fc.tuple(fc.array(entryArb, {maxLength: 7}), sessionsArb, nowArb, defaultArb), 90, 22)) {
	const overview = strip({entries, defaultBranch: def, checkedAt: iso(0)});
	rec('worktreeRows', worktreeRows, [overview, sessions, now]);
	rec('bulkTargets', bulkTargets, [worktreeRows(overview as any, sessions as any, now)]);
}
for (const [branch, def] of sample(fc.tuple(fc.oneof(text, fc.constantFrom('main', 'master')), fc.option(fc.oneof(text, fc.constantFrom('main', 'dev')), {nil: undefined})), 60, 23)) rec('protectedBranch', protectedBranch, def === undefined ? [branch] : [branch, def]);

await recorder.write({prune: true});
