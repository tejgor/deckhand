/**
 * Fixtures for the pure session-feature functions unit P8 ports (filters, attention, handoff markdown).
 * exportHandoff (writes a file) and the I7 exports are not recorded here. Regenerate with
 *
 *   node --import tsx parity/gen/sessionFeatures-P8.gen.ts
 */
import fc from 'fast-check';
import {Recorder, installDeterminism, mutate, registerFn, runTap, sample} from '../lib.ts';
import {SESSION_FILTERS, SESSION_FILTER_KEYS, filterForKey, filterSessionList, handoffGitLines, handoffMarkdown, sessionBlocked, sessionFilterCounts, sessionHasUnreadReply, sessionNeedsAttention, withReplyRead} from '../../src/sessionFeatures.ts';

const MODULE = 'sessionFeatures-P8';
installDeterminism();
const recorder = new Recorder(MODULE);

// Lone surrogates cannot be held by a Rust String; skip generated cases whose result contains one.
const LONE = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/;
// JSON.stringify escapes lone surrogates (`\ud83d`), so look at every string of the result itself.
const hasLone = (value: unknown): boolean => typeof value === 'string' ? LONE.test(value)
	: Array.isArray(value) ? value.some(hasLone)
	: value && typeof value === 'object' ? Object.values(value).some(hasLone) : false;
const clean = (fn: Function, args: unknown[]): boolean => {
	try { return !hasLone(fn(...args)); } catch { return true; }
};

// Rust records are typed: a string-union field holding a value outside its union (a TS type error the mutations
// below produce) cannot be represented, nor can a negative "more" count. Such calls are not recorded.
const SESSION_UNIONS: Record<string, readonly unknown[]> = {
	status: ['starting', 'running', 'exited'], program: ['claude', 'pi', 'codex'],
	exitReason: ['stopped', 'failed', 'completed', 'interrupted'],
};
const ATTENTION_STATES: readonly unknown[] = ['unknown', 'working', 'needs-input', 'response-ended', 'failed', 'limited'];
const typedSession = (session: any): boolean => !session || typeof session !== 'object' || (
	Object.entries(SESSION_UNIONS).every(([key, values]) => session[key] === undefined || values.includes(session[key]))
	&& (session.attention === undefined || ATTENTION_STATES.includes(session.attention.state)));
const typedList = (list: unknown): boolean => !Array.isArray(list) || list.every(typedSession);
const typedGit = (context: any): boolean => !context || (context.moreCommits >= 0 && context.moreChanges >= 0);
const typed = (name: string, args: any[]): boolean => {
	switch (name) {
		case 'sessionBlocked': case 'sessionHasUnreadReply': case 'sessionNeedsAttention': case 'withReplyRead': return typedSession(args[0]);
		case 'filterSessionList': return typedList(args[0]) && (SESSION_FILTERS as unknown[]).includes(args[1]);
		case 'sessionFilterCounts': return typedList(args[0]);
		case 'handoffGitLines': return typedGit(args[0]);
		case 'handoffMarkdown': return typedSession(args[0]) && typedGit(args[2]);
		default: return true;
	}
};
const call = (name: string, fn: Function, args: unknown[]) => {
	if (!typed(name, args as any[]) || !clean(fn, args)) return;
	try { recorder.call(name, fn, args); } catch {}
};

// Named callbacks for filterSessionList's taskTitle: Rust dispatches on the name.
const byTitle = registerFn('taskTitleById', (session: any) => session.id === 'a' ? 'Ship OAuth' : session.id === 'b' ? '' : undefined);
const always = registerFn('taskTitleConst', () => 'refactor 中文 😀');

// 1. Tap the existing tests (only the functions this unit ports).
const MINE = ['filterForKey', 'sessionBlocked', 'sessionHasUnreadReply', 'sessionNeedsAttention', 'withReplyRead', 'filterSessionList', 'sessionFilterCounts', 'handoffGitLines', 'handoffMarkdown'];
recorder.addTap(await runTap({tests: ['tests/sessionFeatures.test.ts'], tap: MINE.map(name => `sessionFeatures:${name}`)}), {module: 'sessionFeatures'});

// 2. Generators.
const STATES = ['unknown', 'working', 'needs-input', 'response-ended', 'failed', 'limited'] as const;
const text = fc.array(fc.constantFrom('a', 'B', ' ', '\n', '\t', '-', '/', 'é', '😀', '中', '~', 'x y', 'auth', 'Foo', 'main'), {maxLength: 6}).map(parts => parts.join(''));
const stamp = fc.constantFrom('2026-10-19T10:00:00.000Z', '2026-10-19T09:59:59.000Z', '2026-10-19T10:05:00.000Z', 'now', '', '2025-01-01T00:00:00.000Z');
const sessionArb = (id: string, ids: string[]) => fc.record({
	id: fc.constant(id), title: text, program: fc.constantFrom('claude', 'pi', 'codex'), cwd: fc.constantFrom('/repo', '/wt/auth', '/wt/日本', '/r/😀'),
	status: fc.constantFrom('starting', 'running', 'exited'),
}).chain(base => fc.record({
	notes: fc.option(text, {nil: undefined}),
	sharedNotes: fc.option(fc.record({text: text}), {nil: undefined}),
	worktree: fc.option(fc.record({branch: fc.option(text, {nil: undefined})}, {requiredKeys: []}), {nil: undefined}),
	attention: fc.option(fc.record({state: fc.constantFrom(...STATES), event: fc.constant('Stop'), at: stamp}), {nil: undefined}),
	replyRead: fc.option(fc.boolean(), {nil: undefined}),
	exitReason: fc.option(fc.constantFrom('stopped', 'failed', 'completed', 'interrupted'), {nil: undefined}),
	archivedAt: fc.option(stamp, {nil: undefined}),
	parentSessionId: fc.option(fc.constantFrom(...ids), {nil: undefined}),
}, {requiredKeys: []}).map(extra => ({...base, ...extra} as any)));
const listArb = fc.integer({min: 0, max: 6}).chain(n => {
	const ids = ['a', 'b', 'c', 'd', 'e', 'f', 'zz'];
	return fc.tuple(...Array.from({length: n}, (_, i) => sessionArb(['a', 'b', 'c', 'd', 'e', 'f'][i]!, ids)));
});
const queryArb = fc.oneof(fc.constantFrom('', ' ', 'auth', 'AUTH  foo', 'ship', 'oauth', '中', '😀', 'main x', 'refactor'), text);

// 3. Single-session predicates and withReplyRead.
const oneSession = sessionArb('s', ['s', 'p']);
for (const session of sample(oneSession, 220, 11)) {
	call('sessionBlocked', sessionBlocked, [session]);
	call('sessionHasUnreadReply', sessionHasUnreadReply, [session]);
	call('sessionNeedsAttention', sessionNeedsAttention, [session]);
	for (const readReplies of [undefined, {}, {s: '2026-10-19T10:00:00.000Z'}, {s: '2026-10-19T10:05:00.000Z'}, {s: '2026-10-19T09:59:59.000Z'}, {other: '2030-01-01T00:00:00.000Z'}, {s: ''}]) {
		call('withReplyRead', withReplyRead, readReplies === undefined ? [session] : [session, readReplies]);
	}
}
for (const state of STATES) for (const readReplies of [{s: '2026-10-19T10:00:00.000Z'}, {s: 'zzz'}]) {
	call('withReplyRead', withReplyRead, [{id: 's', title: 't', attention: {state, event: 'Stop', at: '2026-10-19T10:00:00.000Z'}}, readReplies]);
}

// 4. filterForKey: every key, case variants, empties, multi-char and unicode input.
for (const key of [...Object.values(SESSION_FILTER_KEYS), 'x', 'a', 'A', 'R', 'E', '', '!!', ' ', '*', '😀', '中', '1', 'all', 'active']) call('filterForKey', filterForKey, [key]);

// 5. Lists: filterSessionList and sessionFilterCounts.
let listIndex = 0;
for (const list of sample(listArb, 160, 5)) {
	const query = sample(queryArb, 1, ++listIndex)[0]!;
	for (const filter of SESSION_FILTERS) {
		call('filterSessionList', filterSessionList, [list, filter, query]);
		if (list.length && filter === 'active') call('filterSessionList', filterSessionList, [list, filter, query, byTitle]);
	}
	call('filterSessionList', filterSessionList, [list, 'all', 'refactor', always]);
	call('sessionFilterCounts', sessionFilterCounts, [list, query]);
	call('sessionFilterCounts', sessionFilterCounts, [list, 'ship', byTitle]);
}
// Cycles and dangling parents; duplicate ids.
const cyc = (id: string, parent: string, extra: object = {}) => ({id, title: id, parentSessionId: parent, program: 'claude', cwd: '/r', status: 'running', ...extra});
for (const filter of SESSION_FILTERS) {
	call('filterSessionList', filterSessionList, [[cyc('a', 'b', {archivedAt: 'x'}), cyc('b', 'a'), cyc('c', 'zzz'), cyc('d', 'd')], filter, '']);
	call('filterSessionList', filterSessionList, [[cyc('a', 'b'), cyc('a', 'a', {title: 'dup'})], filter, 'dup']);
}
// Tapped and mutated variants.
const fns: Record<string, Function> = {filterSessionList, sessionFilterCounts, withReplyRead, sessionBlocked, sessionNeedsAttention};
for (const name of Object.keys(fns)) {
	for (const args of mutate(recorder.argsOf(name), {variants: 3})) call(name, fns[name]!, args);
}

// 6. handoffGitLines / handoffMarkdown.
const lineText = fc.array(fc.constantFrom('a', ' ', '\n', '\t', '\r', '\u0000', '\u007f', ' ', ' ', '"', '\\', 'é', '😀', '中', 'x', '/'), {maxLength: 6}).map(parts => parts.join(''));
const gitArb = fc.record({
	baseRef: fc.option(lineText, {nil: undefined}),
	commits: fc.array(lineText, {maxLength: 4}), moreCommits: fc.constantFrom(0, 0, 3, 120),
	changes: fc.array(fc.record({status: fc.constantFrom('M', '??', 'A', 'R', 'D'), path: lineText}), {maxLength: 4}), moreChanges: fc.constantFrom(0, 0, 2, 50),
	diff: fc.option(fc.record({files: fc.nat(60), insertions: fc.nat(500), deletions: fc.nat(500), names: fc.array(fc.record({path: lineText, additions: fc.option(fc.nat(99), {nil: undefined}), deletions: fc.option(fc.nat(99), {nil: undefined})}, {requiredKeys: ['path']}), {maxLength: 4})}), {nil: undefined}),
	error: fc.option(lineText, {nil: undefined}),
}, {requiredKeys: ['commits', 'moreCommits', 'changes', 'moreChanges']});
for (const context of sample(gitArb, 160, 21)) call('handoffGitLines', handoffGitLines, [context]);
const manyNames = Array.from({length: 53}, (_, i) => ({path: `f${i}`, additions: i, deletions: 1}));
call('handoffGitLines', handoffGitLines, [{baseRef: 'main', commits: [], moreCommits: 0, changes: [], moreChanges: 0, diff: {files: 53, insertions: 1, deletions: 2, names: manyNames}}]);
call('handoffGitLines', handoffGitLines, [{baseRef: 'main', commits: ['abc1234 subject'], moreCommits: 4, changes: [{status: 'M', path: 'a b'}], moreChanges: 1, diff: {files: 0, insertions: 0, deletions: 0, names: []}}]);
call('handoffGitLines', handoffGitLines, [{baseRef: '', commits: ['x'], moreCommits: 1, changes: [], moreChanges: 0, diff: {files: 1, insertions: 1, deletions: 1, names: [{path: 'p'}]}}]);

const taskArb = fc.record({title: lineText, body: lineText, done: fc.boolean()});
const previewArb = fc.oneof(
	text.map(t => (t + '~~ ').repeat(2)), fc.constantFrom('', 'plain output', '~~~ x\n~~~~~ y', '~'.repeat(7), 'a'.repeat(20005) + '~~', 'é😀'.repeat(10003)),
	fc.array(fc.constantFrom('~', '~~', '~~~', 'a', '\n', '中'), {maxLength: 12}).map(parts => parts.join('')),
);
const handoffSession = fc.tuple(sessionArb('s', ['p']), fc.record({doneAt: fc.option(stamp, {nil: undefined}), lastPreview: fc.option(previewArb, {nil: undefined})}, {requiredKeys: []}), fc.option(fc.record({text: lineText}), {nil: undefined}))
	.map(([session, extra, shared]: any) => ({...session, ...extra, ...(shared ? {sharedNotes: shared} : {})}));
for (const [session, includeOutput, git, task] of sample(fc.tuple(handoffSession, fc.boolean(), fc.option(gitArb, {nil: undefined}), fc.option(taskArb, {nil: undefined})), 220, 31)) {
	call('handoffMarkdown', handoffMarkdown, [session, includeOutput, git, task]);
}
const base: any = {id: 'root', title: 'Parent', cwd: '/repo', program: 'claude', status: 'running', notes: '', repoRoot: '/repo'};
call('handoffMarkdown', handoffMarkdown, [base]);
call('handoffMarkdown', handoffMarkdown, [{...base, notes: '  trimmed \n', sharedNotes: {text: '\n shared  '}, doneAt: '2026-10-05T12:00:00.000Z', worktree: {mode: 'managed', branch: 'feat/x'}}]);
call('handoffMarkdown', handoffMarkdown, [{...base, lastPreview: 'a~~b~~~~c'}, true]);
call('handoffMarkdown', handoffMarkdown, [{...base, lastPreview: 'x'.repeat(30000)}, true]);
call('handoffMarkdown', handoffMarkdown, [{...base, lastPreview: 'a'.repeat(19998) + '~~~~~'}, true]);
call('handoffMarkdown', handoffMarkdown, [{...base, notes: 'n'}, false, undefined, {title: 'Add OAuth', body: 'with PKCE', done: false}]);
call('handoffMarkdown', handoffMarkdown, [base, false, undefined, {title: 'line\nbreak', body: '  \n ', done: true}]);
call('handoffMarkdown', handoffMarkdown, [base, true, {baseRef: 'main', commits: ['a b'], moreCommits: 1, changes: [], moreChanges: 0, error: 'bad\nthing'}]);
for (const args of mutate(recorder.argsOf('handoffMarkdown'), {variants: 3})) call('handoffMarkdown', handoffMarkdown, args);
for (const args of mutate(recorder.argsOf('handoffGitLines'), {variants: 3})) call('handoffGitLines', handoffGitLines, args);

await recorder.write({prune: true});
