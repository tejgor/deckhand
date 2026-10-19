/**
 * Fixtures for src/sidebarModel.ts (pure sidebar layout). Regenerate with
 *
 *   node --import tsx parity/gen/sidebarModel.gen.ts
 *
 * Generated cases whose result contains a lone surrogate (truncation inside a surrogate pair) are skipped: a Rust
 * String cannot hold one.
 */
import fc from 'fast-check';
import {Recorder, installDeterminism, mutate, runTap, sample} from '../lib.ts';
import {SESSION_FILTERS} from '../../src/sessionFeatures.ts';
import {attentionReasonLines, filterMenuLines, formatAge, locationText, msUntilAgeChanges, rowDimmed, sessionDetails, sidebarHeader, sidebarRows, statusSince, statusWords} from '../../src/sidebarModel.ts';

const MODULE = 'sidebarModel';
installDeterminism();
const recorder = new Recorder(MODULE);

const LONE = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/;
// JSON.stringify escapes lone surrogates (`\ud83d`), so look at every string of the result itself.
const hasLone = (value: unknown): boolean => typeof value === 'string' ? LONE.test(value)
	: Array.isArray(value) ? value.some(hasLone)
	: value && typeof value === 'object' ? Object.values(value).some(hasLone) : false;
const clean = (fn: Function, args: unknown[]): boolean => {
	try { return !hasLone(fn(...args)); } catch { return false; } // invalid shapes (TypeError text) are not portable
};

// Rust records are typed: a string-union field holding a value outside its union (a TS type error the mutations
// below produce) cannot be represented, nor can a negative count or row number. Such calls are not recorded.
const SESSION_UNIONS: Record<string, readonly unknown[]> = {
	status: ['starting', 'running', 'exited'], program: ['claude', 'pi', 'codex'], agentStatus: ['unknown', 'active', 'idle'],
	exitReason: ['stopped', 'failed', 'completed', 'interrupted'], subSessionKind: ['clean', 'forked'], requestedWorktreeMode: ['none', 'new', 'existing'],
};
const REASON_KINDS: readonly unknown[] = ['question', 'plan', 'permission', 'message', 'failure', 'notice'];
const ATTENTION_STATES: readonly unknown[] = ['unknown', 'working', 'needs-input', 'response-ended', 'failed', 'limited'];
const typedSession = (session: any): boolean => !session || typeof session !== 'object' || (
	Object.entries(SESSION_UNIONS).every(([key, values]) => session[key] === undefined || values.includes(session[key]))
	&& (session.worktree?.mode === undefined || ['none', 'managed', 'attached'].includes(session.worktree.mode))
	&& (session.attention === undefined || (ATTENTION_STATES.includes(session.attention.state)
		&& (session.attention.reason === undefined || REASON_KINDS.includes(session.attention.reason.kind)))));
const typedList = (list: unknown): boolean => !Array.isArray(list) || list.every(typedSession);
const isFilter = (value: unknown): boolean => (SESSION_FILTERS as unknown[]).includes(value);
const typed = (name: string, args: any[]): boolean => {
	switch (name) {
		case 'rowDimmed': return typedSession(args[0]) && isFilter(args[1]);
		case 'statusWords': case 'statusSince': case 'attentionReasonLines': return typedSession(args[0]);
		case 'sidebarHeader': return isFilter(args[0].filter) && typedList(args[0].allSessions) && args[0].shown >= 0 && args[0].total >= 0;
		case 'filterMenuLines': return isFilter(args[0]) && Object.values(args[1] as Record<string, number>).every(count => count >= 0);
		case 'sidebarRows': return isFilter(args[0].filter) && typedList(args[0].rows) && typedList(args[0].allSessions) && args[0].firstNumber >= 0;
		case 'locationText': return typedSession(args[0]) && typedList(args[1]);
		case 'sessionDetails': return typedSession(args[0]) && typedList(args[1]) && !(args[6]?.more < 0);
		default: return true;
	}
};
const call = (name: string, fn: Function, args: unknown[]) => {
	if (!typed(name, args as any[]) || !clean(fn, args)) return;
	try { recorder.call(name, fn, args); } catch {}
};

const NOW = Date.parse('2026-10-19T12:00:00.000Z');
const FNS: Record<string, Function> = {rowDimmed, sidebarRows, sidebarHeader, filterMenuLines, statusWords, statusSince, formatAge, msUntilAgeChanges, locationText, sessionDetails, attentionReasonLines};

// 1. Tap the existing tests.
recorder.addTap(await runTap({tests: ['tests/sidebarModel.test.ts'], tap: Object.keys(FNS).map(name => `sidebarModel:${name}`)}), {module: 'sidebarModel'});

// 2. Generators.
const ago = (minutes: number) => new Date(NOW - minutes * 60_000).toISOString();
const STATES = ['unknown', 'working', 'needs-input', 'response-ended', 'failed', 'limited'] as const;
const title = fc.array(fc.constantFrom('a', 'b', 'auth', ' ', '  ', 'é', '😀', '中', ' / ', '/', 'Fix bug', 'x'.repeat(30), 'migrate billing'), {maxLength: 5}).map(parts => parts.join(''));
const stamp = fc.oneof(fc.integer({min: 0, max: 4000}).map(ago), fc.constantFrom('', 'garbage', ago(-5)));
const notes = fc.array(fc.constantFrom('- [ ] open', '- [x] done', '* [ ] star', 'text', '', '  - [ ] nested', '- [X] big'), {maxLength: 4}).map(lines => lines.join('\n'));
const reasonArb = fc.record({kind: fc.constantFrom('question', 'plan', 'permission', 'message', 'failure', 'notice'), text: fc.array(fc.constantFrom('wants to run: npm test', 'asks:', 'long long words', 'x'.repeat(40), '中文', '😀', ' ', 'plan ready'), {maxLength: 6}).map(parts => parts.join(' '))}, {requiredKeys: ['kind', 'text']});
const VERSIONS = ['2.1.287', '2.1.290', '2.0.0', '3.0.0', 'bad', ''];
const IDS = ['a', 'b', 'c', 'd', 'e', 'f'];
const worktreeArb = fc.oneof(
	fc.constant(undefined),
	fc.constant({mode: 'none'}),
	fc.constant({mode: 'none', id: 'w1'}),
	fc.record({mode: fc.constantFrom('none', 'managed', 'attached'), id: fc.option(fc.constantFrom('w1', 'w2'), {nil: undefined}), path: fc.option(fc.constantFrom('/wt/auth', '/wt/billing', '/wt/日本'), {nil: undefined}), branch: fc.option(fc.constantFrom('feat/auth', 'docs/cleanup', 'fix/中文', 'a'.repeat(40), ''), {nil: undefined}), isMain: fc.option(fc.boolean(), {nil: undefined}), mergedAt: fc.option(stamp, {nil: undefined}), deletedAt: fc.option(stamp, {nil: undefined})}, {requiredKeys: ['mode']}),
);
const sessionArb = (id: string) => fc.record({
	id: fc.constant(id), title, program: fc.constantFrom('claude', 'pi', 'codex'), command: fc.constant('x'),
	cwd: fc.constantFrom('/repo', '/wt/auth', '/wt/billing', '/r/😀'), repoRoot: fc.constant('/repo'),
	launchWorktreeRoot: fc.option(fc.constantFrom('/repo', '/wt/auth', '/repo/'), {nil: undefined}),
	worktree: worktreeArb,
	status: fc.constantFrom('starting', 'running', 'running', 'exited'),
	agentStatus: fc.option(fc.constantFrom('unknown', 'active', 'idle'), {nil: undefined}),
	agentStatusUpdatedAt: fc.option(stamp, {nil: undefined}),
	createdAt: stamp, updatedAt: stamp,
	devRunning: fc.option(fc.boolean(), {nil: undefined}),
	mergedAt: fc.option(stamp, {nil: undefined}),
	parentSessionId: fc.option(fc.constantFrom(...IDS, 'zz'), {nil: undefined}),
	subSessionKind: fc.option(fc.constantFrom('clean', 'forked'), {nil: undefined}),
	archivedAt: fc.option(stamp, {nil: undefined}),
	attention: fc.option(fc.record({state: fc.constantFrom(...STATES), event: fc.constant('Stop'), at: stamp, reason: fc.option(reasonArb, {nil: undefined})}, {requiredKeys: ['state', 'event', 'at']}), {nil: undefined}),
	replyRead: fc.option(fc.boolean(), {nil: undefined}),
	exitReason: fc.option(fc.constantFrom('stopped', 'failed', 'completed', 'interrupted'), {nil: undefined}),
	cleanupError: fc.option(fc.constantFrom('boom', ''), {nil: undefined}),
	doneAt: fc.option(stamp, {nil: undefined}),
	agentVersion: fc.option(fc.constantFrom(...VERSIONS), {nil: undefined}),
	requestedWorktreeMode: fc.option(fc.constantFrom('none', 'new', 'existing'), {nil: undefined}),
	notes: fc.option(notes, {nil: undefined}),
	sharedNotes: fc.option(fc.record({text: notes}), {nil: undefined}),
}, {requiredKeys: ['id', 'title', 'program', 'command', 'cwd', 'repoRoot', 'status', 'createdAt', 'updatedAt']}) as fc.Arbitrary<any>;
const listArb = fc.integer({min: 1, max: 6}).chain(n => fc.tuple(...Array.from({length: n}, (_, i) => sessionArb(IDS[i]!)))) as fc.Arbitrary<any[]>;
const filterArb = fc.constantFrom(...SESSION_FILTERS);
const installedArb = fc.record({claude: fc.constantFrom('2.1.290', '2.0.0', '', 'bad'), pi: fc.constantFrom('1.0.0', '3.0.0'), codex: fc.constantFrom('0.5.0', '9.9.9')}, {requiredKeys: []});
const msArb = fc.oneof(fc.integer({min: -100, max: 500_000_000}), fc.constantFrom(0, 59_999, 60_000, 3_599_999, 3_600_000, 86_399_999, 86_400_000, 172_800_000, Number.NaN, Infinity, -Infinity, 1e15, 0.5));

// 3. Scalar functions.
for (const ms of [...sample(msArb, 150, 1), 1, 59_999.9, 60_000, 119_999, 120_000, 3_600_000 - 1, 3_600_000, 86_400_000 - 1, 86_400_000, 86_400_000 * 400]) {
	call('formatAge', formatAge, [ms]);
	call('msUntilAgeChanges', msUntilAgeChanges, [ms]);
}
for (const session of sample(sessionArb('s'), 220, 2)) {
	call('statusWords', statusWords, [session]);
	call('statusSince', statusSince, [session]);
	for (const filter of SESSION_FILTERS) call('rowDimmed', rowDimmed, [session, filter]);
}
for (const status of ['starting', 'running', 'exited']) for (const state of [undefined, ...STATES]) for (const replyRead of [false, true]) {
	call('statusWords', statusWords, [{id: 's', status, attention: state ? {state, event: 'e', at: ago(3)} : undefined, replyRead, exitReason: status === 'exited' ? 'failed' : undefined}]);
	call('statusSince', statusSince, [{id: 's', status, attention: state ? {state, event: 'e', at: ago(3)} : undefined, agentStatusUpdatedAt: ago(9), updatedAt: ago(8), createdAt: ago(7)}]);
}
for (const agentStatus of [undefined, 'unknown', 'active', 'idle']) call('statusWords', statusWords, [{id: 's', status: 'running', agentStatus}]);
call('statusSince', statusSince, [{id: 's', status: 'running', createdAt: ago(7), updatedAt: ''}]);

// 4. Header and filter menu.
const countsArb = fc.record(Object.fromEntries(SESSION_FILTERS.map(filter => [filter, fc.oneof(fc.nat(30), fc.constantFrom(100, 12345, 0))])) as any) as fc.Arbitrary<any>;
const queryArb = fc.oneof(fc.constantFrom('', ' ', 'auth', '  padded  ', 'a very long search query indeed', '中文', '😀😀', 'x'), title);
const widthArb = fc.oneof(fc.integer({min: -2, max: 70}), fc.constantFrom(0, 1, 2, 3, 4, 8, 10, 12, 20, 45));
for (const [width, filter, query, shown, total, list] of sample(fc.tuple(widthArb, filterArb, queryArb, fc.nat(150), fc.nat(150), fc.oneof(fc.constant([] as any[]), listArb)), 260, 3)) {
	call('sidebarHeader', sidebarHeader, [{width, filter, query, shown, total, allSessions: list}]);
}
for (const [filter, counts, width] of sample(fc.tuple(filterArb, countsArb, widthArb), 220, 4)) call('filterMenuLines', filterMenuLines, [filter, counts, width]);

// 5. locationText.
for (const [list, width, withNow] of sample(fc.tuple(listArb, fc.oneof(widthArb, fc.constant(undefined), fc.constant(Infinity)), fc.boolean()), 300, 5)) {
	for (const session of list.slice(0, 3)) call('locationText', locationText, withNow ? [session, list, ...(width === undefined ? [Infinity] : [width]), NOW] : width === undefined ? [session, list] : [session, list, width]);
}

// 6. sessionDetails and attentionReasonLines.
for (const [list, width, freeRows, installed, task] of sample(fc.tuple(listArb, widthArb, fc.integer({min: 0, max: 10}), installedArb, fc.option(fc.record({title, done: fc.boolean(), more: fc.option(fc.nat(5), {nil: undefined})}, {requiredKeys: ['title', 'done']}), {nil: undefined})), 320, 6)) {
	const session = list[0]!;
	call('sessionDetails', sessionDetails, [session, list, width, freeRows, NOW, installed, ...(task ? [task] : [])]);
	call('attentionReasonLines', attentionReasonLines, [session, width, freeRows]);
}
call('sessionDetails', sessionDetails, [undefined, [], 40, 8, NOW]);
for (const [session, width, rows] of sample(fc.tuple(sessionArb('s'), fc.integer({min: 0, max: 50}), fc.integer({min: 0, max: 5})), 150, 7)) {
	call('attentionReasonLines', attentionReasonLines, [{...session, status: 'running', attention: {state: 'needs-input', event: 'e', at: ago(1), reason: {kind: 'question', text: 'asks: pick one of the long options please', ...(session.attention?.reason ?? {})}}}, width, rows]);
}

// 7. sidebarRows: whole lists, selected row, collapsed/hidden sets, versions, widths.
const subsetArb = fc.constantFrom(...IDS);
for (const [list, filter, width, selected, collapsed, hidden, installed, first, spinner] of sample(fc.tuple(listArb, filterArb, fc.oneof(fc.integer({min: 0, max: 80}), fc.constantFrom(12, 21, 30, 45)), fc.option(subsetArb, {nil: undefined}), fc.array(subsetArb, {maxLength: 2}), fc.array(subsetArb, {maxLength: 2}), installedArb, fc.integer({min: 1, max: 120}), fc.constantFrom('⠋', '⠙', '*')), 360, 8)) {
	const input: any = {rows: list, allSessions: list, firstNumber: first, numberWidth: String(first + list.length).length, width, spinnerFrame: spinner, filter};
	if (selected) input.selectedId = selected;
	if (collapsed.length || first % 2) input.collapsedSessionIds = new Set(collapsed);
	if (hidden.length || first % 3 === 0) input.hiddenSessionIds = new Set(hidden);
	if (first % 2 === 0) input.installedVersions = installed;
	// A view that lists only part of the sessions (as the app does after search or collapse).
	if (list.length > 2 && first % 4 === 0) input.rows = list.slice(1);
	call('sidebarRows', sidebarRows, [input]);
}
call('sidebarRows', sidebarRows, [{rows: [], allSessions: [], firstNumber: 1, numberWidth: 1, width: 30, spinnerFrame: '⠋', filter: 'all'}]);

// 8. Mutations of everything tapped or generated so far (one leaf changed per variant).
for (const name of Object.keys(FNS)) {
	for (const args of mutate(recorder.argsOf(name).slice(0, 120), {variants: 2})) call(name, FNS[name]!, args);
}

await recorder.write({prune: true});
