/**
 * Parity fixtures for src/workspace.ts (unit P2). Regenerate with
 *   node --import tsx parity/gen/workspace.gen.ts
 * Paths are absolute (path.resolve against the process cwd is not recorded).
 */
import {Recorder, arb, fc, installDeterminism, mutate, runTap, sample} from '../lib.ts';
import {noWorkspaceReason, workspaceKey, workspacePaneUnavailable, workspaceWorkerId} from '../../src/workspace.ts';

const MODULE = 'workspace';
installDeterminism();
const recorder = new Recorder(MODULE);

const tapped = await runTap({tests: ['tests/workspace.test.ts', 'tests/worktreeRecords.test.ts'], tap: ['workspace:*']});
recorder.addTap(tapped, {module: 'workspace'});

const PATHS = ['/r', '/r/', '/r/wt-a', '/r/wt-a/', '/r/wt-a/../wt-b', '/r/./wt-a', '/work/中文', '/work/😀 emoji', '/work/é', '/', '//r//wt-a', '/r/wt a/\ttab', '/work/\u{1F600}‍\u{1F468}'];
const PATH = fc.constantFrom(...PATHS);
const opt = <T>(arbitrary: fc.Arbitrary<T>) => fc.option(arbitrary, {nil: undefined, freq: 3});
const MODES = fc.constantFrom('managed', 'attached', 'none');
const worktree = fc.record({mode: MODES, path: opt(fc.oneof(PATH, fc.constant(''))), deletedAt: opt(fc.constantFrom('2026-01-02T00:00:00.000Z', '')), id: opt(fc.constant('w1'))}, {requiredKeys: ['mode']});
const sessionLike = fc.record({id: fc.constantFrom('s1', 's2', 'é', '中'), cwd: PATH, launchWorktreeRoot: opt(PATH), worktree: opt(worktree), requestedWorktreeMode: opt(MODES)}, {requiredKeys: ['id', 'cwd']});

for (const sess of sample(sessionLike, 250, 21)) {
	recorder.call('workspaceKey', workspaceKey, [sess]);
	recorder.call('noWorkspaceReason', noWorkspaceReason, [sess]);
}
// Hand-picked: every branch.
const s = (extra: Record<string, unknown> = {}) => ({id: 's1', cwd: '/r/sub', ...extra});
for (const sess of [s(), s({launchWorktreeRoot: '/r/'}), s({worktree: {mode: 'managed', path: '/r/wt/'}}), s({worktree: {mode: 'managed'}}), s({worktree: {mode: 'managed', path: ''}}),
	s({worktree: {mode: 'attached', path: '/r', deletedAt: '2026-01-02T00:00:00.000Z'}}), s({worktree: {mode: 'none', deletedAt: '2026-01-02T00:00:00.000Z'}}), s({worktree: {mode: 'none'}, requestedWorktreeMode: 'managed'}),
	s({requestedWorktreeMode: 'attached'}), s({requestedWorktreeMode: 'none'}), s({requestedWorktreeMode: 'managed', worktree: {mode: 'managed', path: '/r/wt'}}), s({worktree: {mode: 'none'}, launchWorktreeRoot: '/r/x/../y'}), s({worktree: {mode: 'managed', path: '/work/中文/😀'}}), s({cwd: '/'}), s({worktree: {mode: 'managed', path: '/a', deletedAt: ''}})]) {
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
for (const sess of [s(), s({worktree: {mode: 'managed', path: '/r/wt'}}), s({worktree: {mode: 'none', deletedAt: '2026-01-02T00:00:00.000Z'}}), s({requestedWorktreeMode: 'managed'}), s({id: 's2'})]) {
	for (const record of records) recorder.call('workspacePaneUnavailable', workspacePaneUnavailable, [sess, record]);
}

// workspaceWorkerId(workspace)
for (const text of [...PATHS, '', ' ', '\0', 'workspace', 'é', 'é', '😀', '\u{1F600}'.repeat(40), 'a'.repeat(5000), '中文/路径/😀', '/r\n', 'ß', 'İ', '‍']) recorder.call('workspaceWorkerId', workspaceWorkerId, [text]);
for (const text of sample(arb.text(30), 80, 24)) recorder.call('workspaceWorkerId', workspaceWorkerId, [text]);
for (const args of mutate(recorder.argsOf('workspaceWorkerId'), {variants: 2})) recorder.call('workspaceWorkerId', workspaceWorkerId, args);

await recorder.write({prune: true});
