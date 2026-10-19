/**
 * Parity fixtures for src/sessionScope.ts (unit P2). Regenerate with
 *   node --import tsx parity/gen/sessionScope.gen.ts
 * All paths are absolute (a relative path resolves against the process cwd, which is not recorded). Separator is POSIX.
 */
import {Recorder, fc, installDeterminism, mutate, runTap, sample} from '../lib.ts';
import {isPathInside, sessionMatchesScope} from '../../src/sessionScope.ts';

const MODULE = 'sessionScope';
installDeterminism();
const recorder = new Recorder(MODULE);

const tapped = await runTap({tests: ['tests/sessionScope.test.ts'], tap: ['sessionScope:*']});
recorder.addTap(tapped, {module: 'sessionScope'});

const SEGMENTS = ['a', 'b', 'repo', '..', '.', '', '...', '..x', 'x..', '中文', '😀', 'é', 'é', 'sp ace', '-'];
const abs = fc.array(fc.constantFrom(...SEGMENTS), {maxLength: 5}).map(parts => '/' + parts.join('/'));
const absWithTrailing = fc.tuple(abs, fc.boolean()).map(([value, slash]) => slash ? value + '/' : value);
const absolute = (value: unknown) => typeof value === 'string' && value.startsWith('/');

const fixed = ['/', '/a', '/a/', '/a/b', '/a/b/', '/a/b/../c', '/a/../b', '/a//b', '/a/./b', '/ab', '/a/bc', '/a/..b', '/a/b..', '/..', '/../a', '/a/b/c/d', '/中文', '/中文/😀', '/é', '/é', '/a b', '//a', '///', '/a/b/..', '/a/b/../..', '/a/b/../../..'];
for (const parent of fixed) for (const child of fixed) recorder.call('isPathInside', isPathInside, [parent, child]);
for (const [parent, child] of sample(fc.tuple(absWithTrailing, absWithTrailing), 400, 31)) recorder.call('isPathInside', isPathInside, [parent, child]);
for (const args of mutate(recorder.argsOf('isPathInside').filter(([parent, child]) => absolute(parent) && absolute(child)), {variants: 1}).filter(([parent, child]) => absolute(parent) && absolute(child))) recorder.call('isPathInside', isPathInside, args);

const rec = (extra: Record<string, unknown>) => ({id: 's', title: 't', program: 'claude', command: 'claude', status: 'running', createdAt: '2026-01-01T00:00:00.000Z', updatedAt: '2026-01-01T00:00:00.000Z', repoRoot: '/a', cwd: '/a', ...extra});
const roots = ['/a', '/a/', '/a/b', '/a/wt', '/中文', '/a/../a'];
const opt = <T>(arbitrary: fc.Arbitrary<T>) => fc.option(arbitrary, {nil: undefined, freq: 2});
const session = fc.record({repoRoot: fc.constantFrom(...roots, '/other'), cwd: fc.constantFrom(...roots, '/other', '/a/b/c', '/a/wt/sub', '/a/b/../wt'), worktree: opt(fc.record({mode: fc.constantFrom('managed', 'attached', 'none'), path: opt(fc.constantFrom(...roots, '/other', '/a/wt/'))}, {requiredKeys: ['mode']}))}).map(extra => rec(extra));
for (const sess of sample(session, 300, 32)) for (const root of ['/a', '/a/wt/', '/中文', '/other', '/a/b', '/']) recorder.call('sessionMatchesScope', sessionMatchesScope, [sess, root]);
for (const [repoRoot, cwd, worktree, root] of [
	['/a', '/x', undefined, '/a'], ['/x', '/x', {mode: 'managed', path: '/a/'}, '/a'], ['/x', '/a/deep/er', undefined, '/a'], ['/x', '/ab', undefined, '/a'], ['/x', '/x', {mode: 'none'}, '/a'],
	['/x', '/x', {mode: 'attached', path: '/a/../a'}, '/a'], ['/x', '/x', {mode: 'attached', path: '/a/sub'}, '/a'], ['/a/', '/x', undefined, '/a/.'], ['/x', '/中文/😀', undefined, '/中文'], ['/x', '/', undefined, '/'], ['/x', '/x', undefined, '/'],
] as const) recorder.call('sessionMatchesScope', sessionMatchesScope, [rec({repoRoot, cwd, ...(worktree ? {worktree} : {})}), root]);
for (const args of mutate(recorder.argsOf('sessionMatchesScope'), {variants: 1}).filter(([sess, root]) => {
	const value = sess as {repoRoot?: unknown; cwd?: unknown; worktree?: {path?: unknown}};
	return absolute(root) && absolute(value.repoRoot) && absolute(value.cwd) && (value.worktree?.path === undefined || absolute(value.worktree.path));
})) recorder.call('sessionMatchesScope', sessionMatchesScope, args);

await recorder.write({prune: true});
