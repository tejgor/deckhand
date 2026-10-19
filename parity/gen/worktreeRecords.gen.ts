/**
 * Parity fixtures for src/worktreeRecords.ts (unit P2). Regenerate with
 *   node --import tsx parity/gen/worktreeRecords.gen.ts
 * migrateWorktreeRecords is recorded with the registered id generator `seqId` (a counter that restarts at `w1` for
 * every call: `w1`, `w2`, ...); the default random id source is never recorded. WORKTREE_MARKERS / MERGE_MARKERS are
 * constants exercised through the functions that use them.
 */
import {Recorder, decode, fc, installDeterminism, mutate, registerFn, runTap, sample} from '../lib.ts';
import {liveWorktreeRecord, migrateDoneMarkers, migrateWorktreeRecords, ownWorktreePath, projectWorktree, storedSession, withoutMarkers, worktreeMarkers} from '../../src/worktreeRecords.ts';

const MODULE = 'worktreeRecords';
installDeterminism();
const recorder = new Recorder(MODULE);
const tapOnly = new Recorder('worktreeRecords-tap');

let counter = 0;
const seqId = registerFn('seqId', () => `w${++counter}`);
const migrate = (sessions: unknown, worktrees: unknown, newId: unknown) => { counter = 0; return (migrateWorktreeRecords as Function)(sessions, worktrees, newId); };

// 1. Tap the existing tests. migrateWorktreeRecords inputs are re-recorded below with the deterministic id source.
const tapped = await runTap({tests: ['tests/worktreeRecords.test.ts'], tap: ['worktreeRecords:*'], determinism: true});
for (const record of tapped) (record.fn === 'migrateWorktreeRecords' ? tapOnly : recorder).add(record.fn, record.line);
for (const entry of tapOnly.cases('migrateWorktreeRecords')) { const args = (entry.args as unknown[]).slice(0, 2).map(value => decode(value as never)); recorder.call('migrateWorktreeRecords', migrate, [args[0], args[1], seqId]); }

// 2. Generators.
const ID = fc.constantFrom('s1', 's2', 's3', 's4', 's5', 'w1', 'w2', 'w3', 'wt-é', 'wt-中');
const PATHS = ['/r', '/r/', '/r/wt-a', '/r/wt-a/', '/r/wt-b', '/r/wt-a/../wt-b', '/r/./wt-a', '/work/中文', '/work/😀 emoji', '/work/é', '/', '//r//wt-a', '/r/wt-a/sub'];
const PATH = fc.constantFrom(...PATHS);
const AT = fc.constantFrom('2026-01-01T00:00:00.000Z', '2026-01-02T00:00:00.000Z', '2026-01-03T00:00:00.000Z', '2026-01-04T00:00:00.000Z', '2026-01-05T00:00:00.000Z', '2026-01-06T00:00:00.000Z', 'not a date', '');
const opt = <T>(arbitrary: fc.Arbitrary<T>) => fc.option(arbitrary, {nil: undefined, freq: 3});
const MODES = fc.constantFrom('managed', 'attached', 'none');
const MERGE_MODES = fc.constantFrom('merge', 'squash', 'rebase', 'pr');
const markers = fc.record({
	mergedAt: opt(AT), mergeMode: opt(MERGE_MODES), mergeTargetBranch: opt(fc.constantFrom('main', 'develop', 'release/1.0', 'fé')), mergeSourceRef: opt(fc.constantFrom('feature/a', 'refs/heads/b')),
	mergeSourceSha: opt(fc.constantFrom('0123abc', 'deadbeef')), mergeMarkedManually: opt(fc.boolean()), mergeDetected: opt(fc.constantFrom('ancestor', 'pr')), deletedAt: opt(AT),
}, {requiredKeys: []});
const worktreeRecord = fc.tuple(ID, PATH, AT, markers).map(([id, path, createdAt, marks]) => ({id, path, createdAt, ...marks}));
const sessionWorktree = fc.tuple(MODES, opt(ID), opt(PATH), opt(fc.boolean()), markers, opt(fc.constantFrom('feat/a', 'main'))).map(([mode, id, path, isMain, marks, branch]) => ({mode, ...(id !== undefined ? {id} : {}), ...(path !== undefined ? {path} : {}), ...(isMain !== undefined ? {isMain} : {}), ...(branch !== undefined ? {branch} : {}), ...marks}));
const session = fc.record({
	id: ID, title: fc.constantFrom('t', '日本語', '😀'), program: fc.constant('claude'), command: fc.constant('claude'), cwd: PATH, repoRoot: fc.constant('/r'), status: fc.constant('running'),
	createdAt: AT, updatedAt: AT, launchWorktreeRoot: opt(PATH), worktree: opt(sessionWorktree), requestedWorktreeMode: opt(MODES), agentStartedAt: opt(AT), doneAt: opt(AT),
	mergedAt: opt(AT), mergeTargetBranch: opt(fc.constantFrom('main', 'dev')), mergeSourceRef: opt(fc.constant('feature/a')), mergeMarkedManually: opt(fc.boolean()), parentSessionId: opt(ID),
}, {requiredKeys: ['id', 'title', 'program', 'command', 'cwd', 'repoRoot', 'status', 'createdAt', 'updatedAt']});
const uniqueBy = <T extends {id: string}>(items: T[]) => [...new Map(items.map(item => [item.id, item])).values()];
const sessions = fc.array(session, {maxLength: 6}).map(uniqueBy);
const records = fc.array(worktreeRecord, {maxLength: 4}).map(uniqueBy);
const linkedSession = fc.tuple(session, PATH, fc.boolean()).map(([base, root, nested]) => ({...base, worktree: {mode: nested ? 'attached' : 'managed', path: root, ...(base.worktree && base.worktree.mode !== 'none' ? {deletedAt: base.worktree.deletedAt, mergedAt: base.worktree.mergedAt, mergeMode: base.worktree.mergeMode} : {})}}));
// Legacy states: linked sessions plus sub-sessions launched inside the same worktrees.
const legacyState = fc.array(fc.oneof(linkedSession, session), {minLength: 1, maxLength: 6}).map(uniqueBy);

// 3. Hand-picked edge cases.
const base = {title: 't', program: 'claude', command: 'claude', status: 'running', repoRoot: '/r', updatedAt: '2026-01-01T00:00:00.000Z'};
const s = (id: string, extra: Record<string, unknown> = {}) => ({...base, id, cwd: '/r', createdAt: '2026-01-01T00:00:00.000Z', ...extra});
const marks = {mergedAt: '2026-01-03T00:00:00.000Z', mergeMode: 'squash', mergeTargetBranch: 'main', mergeSourceRef: 'feature/a', mergeSourceSha: 'abc', mergeMarkedManually: true, mergeDetected: 'pr', deletedAt: '2026-01-04T00:00:00.000Z'};
for (const source of [{}, marks, {mergedAt: undefined, deletedAt: '2026-01-01T00:00:00.000Z'}, {mergeMarkedManually: false}, {mergedAt: ''}, {mergedAt: null}, {other: 1, mergedAt: 'x'}]) recorder.call('worktreeMarkers', worktreeMarkers, [source]);
for (const [source, keys] of [[marks, undefined], [marks, []], [marks, ['mergedAt']], [marks, ['deletedAt', 'mergeMode', 'nope']], [{mode: 'managed', ...marks}, ['mergedAt', 'mergeMode']], [{}, undefined], [{x: 1}, ['x']]] as const) {
	recorder.call('withoutMarkers', withoutMarkers, keys === undefined ? [source] : [source, keys]);
}
for (const worktree of [undefined, {mode: 'none', path: '/r/wt'}, {mode: 'managed'}, {mode: 'managed', path: ''}, {mode: 'managed', path: '/r/wt/../wt-b/', isMain: false}, {mode: 'attached', path: '/r', isMain: true}, {mode: 'attached', path: '/work/中文/😀'}, {mode: 'managed', path: '/r//x/./y'}]) recorder.call('ownWorktreePath', ownWorktreePath, [worktree]);
const recs = [{id: 'a', path: '/r/wt', createdAt: 'x', deletedAt: '2026-01-02T00:00:00.000Z'}, {id: 'b', path: '/r/wt/', createdAt: 'x'}, {id: 'c', path: '/r/other', createdAt: 'x'}, {id: 'd', path: '/r/wt', createdAt: 'y', deletedAt: ''}];
for (const [list, at] of [[recs, '/r/wt'], [recs, '/r/wt/.'], [recs, '/r/other/'], [recs, '/nothing'], [[], '/r'], [recs.slice(0, 1), '/r/wt'], [recs.slice(2), '/r/./wt/../other']] as const) recorder.call('liveWorktreeRecord', liveWorktreeRecord, [list, at]);
const recMap = new Map([['w1', {id: 'w1', path: '/r/wt', createdAt: 'x', mergedAt: '2026-01-03T00:00:00.000Z', mergeMode: 'merge'}], ['w2', {id: 'w2', path: '/r/wt2', createdAt: 'x', deletedAt: '2026-01-05T00:00:00.000Z'}]]);
for (const sess of [s('a'), s('a', {worktree: {mode: 'managed', path: '/r/wt'}}), s('a', {worktree: {mode: 'managed', id: 'w1', path: '/r/wt', mergedAt: 'stale', deletedAt: 'stale', mergeDetected: 'ancestor'}}), s('a', {worktree: {mode: 'managed', id: 'w2', path: '/r/wt2'}}), s('a', {worktree: {mode: 'none', id: 'w1'}}), s('a', {worktree: {mode: 'managed', id: 'missing', mergedAt: 'x'}}), s('a', {worktree: {mode: 'managed', id: ''}})]) recorder.call('projectWorktree', projectWorktree, [sess, recMap]);
recorder.call('projectWorktree', projectWorktree, [s('a', {worktree: {mode: 'managed', id: 'w1'}}), new Map()]);
for (const sess of [s('a'), s('a', {worktree: {mode: 'managed', path: '/r/wt', ...marks}}), s('a', {worktree: {mode: 'managed', id: 'w1', path: '/r/wt', ...marks, branch: 'b'}}), s('a', {worktree: {mode: 'managed', id: ''}})]) recorder.call('storedSession', storedSession, [sess]);

// Migration scenarios beyond the tests.
const at = (day: number) => `2026-01-0${day}T00:00:00.000Z`;
const linked = (id: string, day: number, extra: Record<string, unknown> = {}, wt: Record<string, unknown> = {}) => s(id, {createdAt: at(day), cwd: '/r/wt', worktree: {mode: 'managed', path: '/r/wt', ...wt}, ...extra});
const scenarios: unknown[][] = [
	[[], []],
	[[s('a')], []],
	[[linked('a', 1)], []],
	[[linked('a', 1, {}, {mergedAt: at(5), mergeMode: 'merge'}), linked('b', 2, {}, {mergedAt: at(6), mergeMode: 'squash', mergeTargetBranch: 'main'})], []],
	[[linked('a', 1, {}, {deletedAt: at(3)}), linked('b', 4)], []],
	[[linked('a', 1, {}, {deletedAt: at(3)}), linked('b', 2, {agentStartedAt: at(4)}), linked('c', 5)], []],
	[[linked('a', 1, {}, {deletedAt: at(3)}), linked('b', 2, {}, {deletedAt: at(6)}), s('c', {createdAt: at(4), cwd: '/r/wt', worktree: {mode: 'none'}})], []],
	[[linked('a', 1), s('sub', {createdAt: at(2), cwd: '/r/wt', worktree: {mode: 'none'}, mergedAt: at(7), mergeTargetBranch: 'main', parentSessionId: 'a'})], []],
	[[linked('a', 1), s('sub', {createdAt: at(2), cwd: '/r/other', launchWorktreeRoot: '/r/wt/'})], []],
	[[linked('a', 1), s('pending', {createdAt: at(2), cwd: '/r/wt', requestedWorktreeMode: 'managed'})], []],
	[[s('main', {mergedAt: at(4), mergeTargetBranch: 'main'})], []],
	[[linked('a', 1, {}, {id: 'w9', mergedAt: at(2)})], []],
	[[linked('a', 1, {}, {id: 'w9'})], [{id: 'w9', path: '/r/wt', createdAt: at(1), mergedAt: at(3)}]],
	[[s('a')], [{id: 'unused', path: '/r/wt', createdAt: at(1)}]],
	[[linked('a', 1)], [{id: 'old', path: '/r/wt', createdAt: at(1)}]],
	[[linked('a', 1)], [{id: 5, path: '/r/wt'}, {id: 'x'}, null]],
	[[linked('a', 1, {}, {mergedAt: 'garbage', deletedAt: 'garbage'}), linked('b', 2, {}, {deletedAt: at(3)})], []],
	[[linked('a', 1, {}, {path: '/work/中文/😀'}), linked('b', 2, {}, {path: '/work/中文/😀/'}), linked('c', 2, {}, {path: '/work/é'})], []],
	[[{...linked('a', 1), worktree: {mode: 'attached', path: '/r', isMain: true}}], []],
	[[{...linked('a', 1), worktree: {mode: 'attached', path: '/r/wt', isMain: true}}], []],
];
for (const [list, recordsIn] of scenarios) recorder.call('migrateWorktreeRecords', migrate, [list, recordsIn, seqId]);
const doneScenarios: unknown[][] = [
	[[]], [[s('a')]], [[s('a', {mergedAt: at(2), mergeTargetBranch: 'main', mergeSourceRef: 'f', mergeMarkedManually: true})]],
	[[s('a', {mergedAt: at(2), doneAt: at(4)})]], [[s('a', {worktree: {mode: 'attached', path: '/r', isMain: true, mergedAt: at(3), mergeMode: 'merge', mergeSourceSha: 'abc', deletedAt: at(9)}})]],
	[[s('a', {worktree: {mode: 'none', mergedAt: at(3)}})]], [[s('a', {worktree: {mode: 'managed', id: 'w1', mergedAt: at(3)}, mergedAt: at(3)})]],
	[[s('a', {mergeMarkedManually: false})]], [[s('a', {mergedAt: '', worktree: {mode: 'attached', mergedAt: at(3)}})]],
];
for (const args of doneScenarios) recorder.call('migrateDoneMarkers', migrateDoneMarkers, args);

// 4. Mutations of every recorded input.
for (const [name, fn] of [['worktreeMarkers', worktreeMarkers], ['withoutMarkers', withoutMarkers], ['ownWorktreePath', ownWorktreePath], ['liveWorktreeRecord', liveWorktreeRecord], ['projectWorktree', projectWorktree], ['storedSession', storedSession], ['migrateDoneMarkers', migrateDoneMarkers]] as const) {
	for (const args of mutate(recorder.argsOf(name), {variants: 2})) recorder.call(name, fn as Function, args);
}
const migrateInputs = recorder.argsOf('migrateWorktreeRecords').map(([a, b]) => [a, b]);
for (const [a, b] of mutate(migrateInputs, {variants: 2})) recorder.call('migrateWorktreeRecords', migrate, [a, b, seqId]);

// 5. fast-check.
for (const [source] of sample(fc.tuple(markers.map(marks => ({...marks, junk: 1}))), 60, 11)) recorder.call('worktreeMarkers', worktreeMarkers, [source]);
for (const [source, keys] of sample(fc.tuple(sessionWorktree, fc.option(fc.subarray(['mergedAt', 'mergeMode', 'mergeTargetBranch', 'mergeSourceRef', 'mergeSourceSha', 'mergeMarkedManually', 'mergeDetected', 'deletedAt', 'path']), {nil: undefined})), 80, 12)) recorder.call('withoutMarkers', withoutMarkers, keys === undefined ? [source] : [source, keys]);
for (const [w] of sample(fc.tuple(opt(sessionWorktree)), 100, 13)) recorder.call('ownWorktreePath', ownWorktreePath, [w]);
for (const [list, at2] of sample(fc.tuple(records, PATH), 100, 14)) recorder.call('liveWorktreeRecord', liveWorktreeRecord, [list, at2]);
for (const [sess, list] of sample(fc.tuple(session, records), 150, 15)) {
	recorder.call('projectWorktree', projectWorktree, [sess, new Map(list.map(record => [record.id, record]))]);
	recorder.call('storedSession', storedSession, [sess]);
}
for (const [list, recs2] of sample(fc.tuple(legacyState, records), 250, 16)) recorder.call('migrateWorktreeRecords', migrate, [list, recs2, seqId]);
for (const [list] of sample(fc.tuple(sessions), 150, 17)) recorder.call('migrateDoneMarkers', migrateDoneMarkers, [list]);
// Migrated output fed back (idempotence and the done-marker pass), as the daemon does.
for (const [list] of sample(fc.tuple(legacyState), 100, 18)) {
	const lifted = migrate(list, [], seqId);
	recorder.call('migrateDoneMarkers', migrateDoneMarkers, [lifted.sessions]);
	recorder.call('migrateWorktreeRecords', migrate, [lifted.sessions, lifted.worktrees, seqId]);
}

await recorder.write({prune: true});
