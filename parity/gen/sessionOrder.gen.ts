/**
 * Parity fixtures for src/sessionOrder.ts (unit P2). Regenerate with
 *   node --import tsx parity/gen/sessionOrder.gen.ts
 * Ids are lowercase ASCII letters, digits and `-` and createdAt values are ISO timestamps, so `localeCompare` agrees
 * with a plain code-point comparison (ICU orders `_`, uppercase and non-ASCII differently; not recorded). Sets
 * (hidden/collapsed ids) are recorded as `$set`; results that return the input array are plain arrays.
 */
import {Recorder, fc, installDeterminism, mutate, sample} from '../lib.ts';
import {compareSessionOrder, countHiddenSessionDescendants, countSessionDescendants, filterCollapsedSessions, sessionDepth, sessionDescendants, sessionHasChildren, sortSessionsForSidebar} from '../../src/sessionOrder.ts';

const MODULE = 'sessionOrder';
installDeterminism();
const recorder = new Recorder(MODULE);

const ID_POOL = ['a', 'b', 'c', 'd', 'e', 'f', 'a1', 'a2', 'a10', 'b-1', 'b-2', '0', '9'];
const ID = fc.constantFrom(...ID_POOL);
const CREATED = ['2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.001Z', '2026-01-02T00:00:00.000Z', '2026-02-01T00:00:00.000Z', '2025-12-31T23:59:59.999Z'];
const ORDER = fc.constantFrom(undefined, 0, 1, 2, -1, 1.5, 1e9, Number.NaN, Infinity, -Infinity, '3' as unknown as number, null as unknown as number);
const opt = <T>(arbitrary: fc.Arbitrary<T>) => fc.option(arbitrary, {nil: undefined, freq: 2});
const make = (id: string, extra: Record<string, unknown> = {}) => ({id, title: id, program: 'claude', command: 'claude', cwd: '/r', repoRoot: '/r', status: 'running', createdAt: CREATED[0], updatedAt: CREATED[0], ...extra});
const session = fc.record({id: ID, createdAt: fc.constantFrom(...CREATED), sidebarOrder: ORDER, parentSessionId: opt(fc.oneof(ID, fc.constant('missing'), fc.constant('')))}, {requiredKeys: ['id', 'createdAt']})
	.map(({sidebarOrder, ...rest}) => make(rest.id, {...rest, ...(sidebarOrder !== undefined ? {sidebarOrder} : {})}));
const uniqueBy = <T extends {id: string}>(items: T[], allowDuplicates: boolean) => allowDuplicates ? items : [...new Map(items.map(item => [item.id, item])).values()];
const sessionList = (maxLength: number, duplicates = false) => fc.array(session, {maxLength}).map(items => uniqueBy(items, duplicates));
const idSet = fc.subarray([...ID_POOL, 'missing']).map(ids => new Set(ids));

const localeSafe = (value: unknown): boolean => {
	if (Array.isArray(value)) return value.every(localeSafe);
	if (typeof value === 'string' || !value || typeof value !== 'object' || value instanceof Set) return true;
	return Object.entries(value).every(([key, entry]) => {
		if (key === 'id' || key === 'parentSessionId') return typeof entry !== 'string' || /^[a-z0-9-]*$/.test(entry);
		if (key === 'createdAt') return typeof entry === 'string' && /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/.test(entry);
		return localeSafe(entry);
	});
};

// compareSessionOrder
for (const [a, b] of sample(fc.tuple(session, session), 300, 41)) recorder.call('compareSessionOrder', compareSessionOrder, [a, b]);
for (const [a, b] of [
	[make('a'), make('a')], [make('a'), make('b')], [make('b'), make('a')], [make('a', {sidebarOrder: 1}), make('b')], [make('a', {sidebarOrder: Number.NaN}), make('b', {sidebarOrder: 0})], [make('a', {sidebarOrder: Infinity}), make('b', {sidebarOrder: 5})],
	[make('a', {createdAt: CREATED[2]}), make('b', {createdAt: CREATED[0]})], [make('a1'), make('a10')], [make('a2'), make('a10')], [make('b-1'), make('b')], [make('0'), make('a')], [make('a', {sidebarOrder: '2'}), make('b', {sidebarOrder: 1})],
	[make('a', {sidebarOrder: 0.1}), make('b', {sidebarOrder: 0.3})], [make('a', {sidebarOrder: 1e300}), make('b', {sidebarOrder: -1e300})],
]) recorder.call('compareSessionOrder', compareSessionOrder, [a, b]);

// Hand-built trees and corner cases for the tree functions.
const chain = [make('a'), make('b', {parentSessionId: 'a'}), make('c', {parentSessionId: 'b'}), make('d', {parentSessionId: 'c'})];
const cycle = [make('a', {parentSessionId: 'b'}), make('b', {parentSessionId: 'a'}), make('c', {parentSessionId: 'c'}), make('d', {parentSessionId: 'a'})];
const forest = [make('a', {sidebarOrder: 2}), make('b', {sidebarOrder: 1}), make('c', {parentSessionId: 'a', sidebarOrder: 5}), make('d', {parentSessionId: 'a', sidebarOrder: 1}), make('e', {parentSessionId: 'd'}), make('f', {parentSessionId: 'missing'})];
const lists: unknown[][][] = [[[]], [[make('a')]], [chain], [[...chain].reverse()], [cycle], [forest], [[make('a'), make('a')]], [[make('a', {parentSessionId: 'a'})]], [[make('a', {parentSessionId: ''}), make('b', {parentSessionId: 'a'})]]];
for (const [list] of lists) {
	recorder.call('sortSessionsForSidebar', sortSessionsForSidebar, [list]);
	for (const id of ['a', 'b', 'c', 'd', 'e', 'f', 'missing', '']) {
		recorder.call('sessionHasChildren', sessionHasChildren, [id, list]);
		recorder.call('sessionDescendants', sessionDescendants, [id, list]);
		recorder.call('countSessionDescendants', countSessionDescendants, [id, list]);
		for (const hidden of [new Set<string>(), new Set(['b']), new Set(['a', 'c', 'e'])]) recorder.call('countHiddenSessionDescendants', countHiddenSessionDescendants, [id, list, hidden]);
	}
	for (const target of (list as {id: string}[]).slice(0, 6)) recorder.call('sessionDepth', sessionDepth, [target, list]);
	recorder.call('sessionDepth', sessionDepth, [make('zz', {parentSessionId: 'a'}), list]);
	for (const collapsed of [new Set<string>(), new Set(['a']), new Set(['b', 'd']), new Set(['missing'])]) {
		recorder.call('filterCollapsedSessions', filterCollapsedSessions, [list, collapsed]);
		for (const hidden of [new Set<string>(), new Set(['c']), new Set(['a', 'e'])]) recorder.call('filterCollapsedSessions', filterCollapsedSessions, [list, collapsed, hidden]);
	}
}

// Generated graphs (parents drawn from the same id pool: forests, cycles, dangling parents, deep chains).
for (const [list] of sample(fc.tuple(sessionList(8)), 200, 42)) recorder.call('sortSessionsForSidebar', sortSessionsForSidebar, [list]);
for (const [list] of sample(fc.tuple(sessionList(6, true)), 40, 43)) recorder.call('sortSessionsForSidebar', sortSessionsForSidebar, [list]);
for (const [list, id] of sample(fc.tuple(sessionList(8), fc.oneof(ID, fc.constant('missing'))), 150, 44)) {
	recorder.call('sessionHasChildren', sessionHasChildren, [id, list]);
	recorder.call('sessionDescendants', sessionDescendants, [id, list]);
	recorder.call('countSessionDescendants', countSessionDescendants, [id, list]);
}
for (const [list, id, hidden] of sample(fc.tuple(sessionList(8), ID, idSet), 150, 45)) recorder.call('countHiddenSessionDescendants', countHiddenSessionDescendants, [id, list, hidden]);
for (const [list, which] of sample(fc.tuple(sessionList(8), fc.nat(10)), 150, 46)) if (list.length) recorder.call('sessionDepth', sessionDepth, [list[which % list.length], list]);
for (const [list, collapsed, hidden] of sample(fc.tuple(sessionList(8), idSet, idSet), 200, 47)) {
	recorder.call('filterCollapsedSessions', filterCollapsedSessions, [list, collapsed, hidden]);
	recorder.call('filterCollapsedSessions', filterCollapsedSessions, [list, collapsed]);
}

// Mutations of the recorded inputs.
for (const name of ['compareSessionOrder', 'sortSessionsForSidebar', 'sessionHasChildren', 'sessionDescendants', 'countSessionDescendants', 'countHiddenSessionDescendants', 'sessionDepth', 'filterCollapsedSessions'] as const) {
	const fn = {compareSessionOrder, sortSessionsForSidebar, sessionHasChildren, sessionDescendants, countSessionDescendants, countHiddenSessionDescendants, sessionDepth, filterCollapsedSessions}[name] as Function;
	// Keep only mutants whose ids / timestamps stay in the alphabet where `localeCompare` agrees with code points.
	for (const args of mutate(recorder.argsOf(name), {variants: 1, alphabet: ['a', 'b', '1', '-']}).filter(localeSafe)) recorder.call(name, fn, args);
}

await recorder.write({prune: true});
