/**
 * Parity fixtures for src/workspace.ts (unit P2). Regenerate with
 *   node --import tsx parity/gen/workspace.gen.ts
 * Paths are absolute (path.resolve against the process cwd is not recorded).
 */
import {Recorder, arb, caseLine, fc, installDeterminism, mutate, runTap, sample, type RecordedCase} from '../lib.ts';
import {noWorkspaceReason, workspaceKey, workspacePaneUnavailable, workspaceWorkerId} from '../../src/workspace.ts';

const MODULE = 'workspace';

// The Rust port takes typed records (deckhand_protocol SessionRecord, SessionWorktreeRecord, WorktreeRecord): values
// outside the TS types (unknown enum strings, null for optional fields, a non-integer or non-number sidebarOrder) are
// rejected when state.json / the wire is decoded, before these functions run, so such calls are not recorded. Paths
// are absolute (or empty), and a call whose line mentions the process cwd (path.resolve of a relative path) is dropped:
// its result depends on where the generator runs.
const ENUMS: Record<string, readonly string[]> = {
	program: ['claude', 'pi', 'codex'], status: ['starting', 'running', 'exited'], requestedWorktreeMode: ['none', 'new', 'existing'],
	mode: ['none', 'managed', 'attached'], mergeMode: ['merge', 'squash'], mergeDetected: ['ancestor', 'pr'],
};
const PATH_KEYS = new Set(['path', 'cwd', 'launchWorktreeRoot', 'repoRoot', 'launchCwd']);
const rustTyped = (value: unknown): boolean => {
	if (value === null) return false;
	if (Array.isArray(value)) return value.every(rustTyped);
	if (typeof value === 'number') return Number.isSafeInteger(value);
	if (!value || typeof value !== 'object') return true;
	return Object.entries(value).every(([key, entry]) => key !== '$num'
		&& (!Object.hasOwn(ENUMS, key) || ENUMS[key]!.includes(entry as string))
		&& (!PATH_KEYS.has(key) || (typeof entry === 'string' && (entry === '' || entry.startsWith('/'))))
		&& (key !== 'sidebarOrder' || typeof entry === 'number')
		&& rustTyped(entry));
};
class TypedRecorder extends Recorder {
	constructor(module: string, private readonly accept: (fn: string, args: unknown[]) => boolean = () => true) { super(module); }
	override add(fn: string, entry: RecordedCase | string): void {
		const line = typeof entry === 'string' ? entry : caseLine(entry);
		const args = (JSON.parse(line) as {args: unknown[]}).args;
		if (line.includes(process.cwd()) || !rustTyped(args) || !this.accept(fn, args)) return;
		super.add(fn, line);
	}
}
installDeterminism();
const recorder = new TypedRecorder(MODULE);

const tapped = await runTap({tests: ['tests/workspace.test.ts', 'tests/worktreeRecords.test.ts'], tap: ['workspace:*']});
recorder.addTap(tapped, {module: 'workspace'});

const PATHS = ['/r', '/r/', '/r/wt-a', '/r/wt-a/', '/r/wt-a/../wt-b', '/r/./wt-a', '/work/中文', '/work/😀 emoji', '/work/é', '/', '//r//wt-a', '/r/wt a/\ttab', '/work/\u{1F600}‍\u{1F468}'];
const PATH = fc.constantFrom(...PATHS);
const opt = <T>(arbitrary: fc.Arbitrary<T>) => fc.option(arbitrary, {nil: undefined, freq: 3});
const MODES = fc.constantFrom('managed', 'attached', 'none');
const worktree = fc.record({mode: MODES, path: opt(fc.oneof(PATH, fc.constant(''))), deletedAt: opt(fc.constantFrom('2026-01-02T00:00:00.000Z', '')), id: opt(fc.constant('w1'))}, {requiredKeys: ['mode']});
const sessionLike = fc.record({id: fc.constantFrom('s1', 's2', 'é', '中'), cwd: PATH, launchWorktreeRoot: opt(PATH), worktree: opt(worktree), requestedWorktreeMode: opt(fc.constantFrom('none', 'new', 'existing'))}, {requiredKeys: ['id', 'cwd']});

for (const sess of sample(sessionLike, 250, 21)) {
	recorder.call('workspaceKey', workspaceKey, [sess]);
	recorder.call('noWorkspaceReason', noWorkspaceReason, [sess]);
}
// Hand-picked: every branch.
const s = (extra: Record<string, unknown> = {}) => ({id: 's1', cwd: '/r/sub', ...extra});
for (const sess of [s(), s({launchWorktreeRoot: '/r/'}), s({worktree: {mode: 'managed', path: '/r/wt/'}}), s({worktree: {mode: 'managed'}}), s({worktree: {mode: 'managed', path: ''}}),
	s({worktree: {mode: 'attached', path: '/r', deletedAt: '2026-01-02T00:00:00.000Z'}}), s({worktree: {mode: 'none', deletedAt: '2026-01-02T00:00:00.000Z'}}), s({worktree: {mode: 'none'}, requestedWorktreeMode: 'new'}),
	s({requestedWorktreeMode: 'existing'}), s({requestedWorktreeMode: 'none'}), s({requestedWorktreeMode: 'new', worktree: {mode: 'managed', path: '/r/wt'}}), s({worktree: {mode: 'none'}, launchWorktreeRoot: '/r/x/../y'}), s({worktree: {mode: 'managed', path: '/work/中文/😀'}}), s({cwd: '/'}), s({worktree: {mode: 'managed', path: '/a', deletedAt: ''}})]) {
	recorder.call('workspaceKey', workspaceKey, [sess]);
	recorder.call('noWorkspaceReason', noWorkspaceReason, [sess]);
}
const absolute = (value: unknown) => value === undefined || (typeof value === 'string' && value.startsWith('/'));
const safe = ([sess]: unknown[]) => { const v = sess as {cwd?: unknown; launchWorktreeRoot?: unknown; worktree?: {path?: unknown}}; return absolute(v.cwd) && absolute(v.launchWorktreeRoot) && (v.worktree?.path === '' || absolute(v.worktree?.path)); };
for (const [name, fn] of [['workspaceKey', workspaceKey], ['noWorkspaceReason', noWorkspaceReason]] as const) {
	for (const args of mutate(recorder.argsOf(name), {variants: 2}).filter(safe)) recorder.call(name, fn as Function, args);
}

// workspacePaneUnavailable(session, record)
const records = [{}, {sessionId: 's1'}, {sessionId: 's1', workspace: '/r/wt'}, {sessionId: 's2'}, {sessionId: 's2', workspace: '/r'}, {workspace: '/r'}, {sessionId: 's1', workspace: ''}, {sessionId: ''}];
for (const sess of sample(sessionLike, 120, 22)) for (const record of sample(fc.constantFrom(...records), 2, 23)) recorder.call('workspacePaneUnavailable', workspacePaneUnavailable, [sess, record]);
for (const sess of [s(), s({worktree: {mode: 'managed', path: '/r/wt'}}), s({worktree: {mode: 'none', deletedAt: '2026-01-02T00:00:00.000Z'}}), s({requestedWorktreeMode: 'new'}), s({id: 's2'})]) {
	for (const record of records) recorder.call('workspacePaneUnavailable', workspacePaneUnavailable, [sess, record]);
}

// workspaceWorkerId(workspace)
for (const text of [...PATHS, '', ' ', '\0', 'workspace', 'é', 'é', '😀', '\u{1F600}'.repeat(40), 'a'.repeat(5000), '中文/路径/😀', '/r\n', 'ß', 'İ', '‍']) recorder.call('workspaceWorkerId', workspaceWorkerId, [text]);
for (const text of sample(arb.text(30), 80, 24)) recorder.call('workspaceWorkerId', workspaceWorkerId, [text]);
for (const args of mutate(recorder.argsOf('workspaceWorkerId'), {variants: 2})) recorder.call('workspaceWorkerId', workspaceWorkerId, args);

await recorder.write({prune: true});
