import assert from 'node:assert/strict';
import {test} from 'node:test';
import {isPathInside, sessionMatchesScope} from '../src/sessionScope.js';
import type {SessionRecord} from '../src/types.js';

test('session scope links a main repo and its worktrees without matching lookalike paths', () => {
	const session = {repoRoot: '/repos/main', cwd: '/worktrees/feature', worktree: {path: '/worktrees/feature', mode: 'managed'}} as SessionRecord;
	const bare = {...session, worktree: undefined};
	const cases: [string, SessionRecord, string, boolean][] = [
		['main repo includes its worktree sessions', session, '/repos/main', true],
		['worktree includes sessions created from the main repo', session, '/worktrees/feature', true],
		['trailing slash is ignored', session, '/worktrees/feature/', true],
		['unrelated worktree', session, '/worktrees/other', false],
		['name prefix is not containment', session, '/worktrees/feat', false],
		['no worktree metadata: cwd matches', bare, '/worktrees/feature', true],
		['no worktree metadata: cwd subdirectory matches', {...bare, cwd: '/worktrees/feature/src'}, '/worktrees/feature', true],
	];
	for (const [name, record, scope, expected] of cases) assert.equal(sessionMatchesScope(record, scope), expected, name);
	const paths: [string, string, boolean][] = [['/a/b', '/a/b', true], ['/a/b/', '/a/b/c/d', true], ['/a/b', '/a/bc', false], ['/a/b', '/a', false], ['/a/b', '/a/b/../c', false], ['/a/b', '/a/b/..c', true]];
	for (const [parent, child, expected] of paths) assert.equal(isPathInside(parent, child), expected, `${child} inside ${parent}`);
});
