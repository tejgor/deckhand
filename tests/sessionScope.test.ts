import assert from 'node:assert/strict';
import {test} from 'node:test';
import {sessionMatchesScope} from '../src/sessionScope.js';
import type {SessionRecord} from '../src/types.js';

const session = {
	repoRoot: '/repos/main',
	cwd: '/worktrees/feature',
	worktree: {path: '/worktrees/feature', mode: 'managed'},
} as SessionRecord;

test('main repo still includes sessions created in its worktrees', () => {
	assert.equal(sessionMatchesScope(session, '/repos/main'), true);
});

test('worktree includes sessions created from the main repo', () => {
	assert.equal(sessionMatchesScope(session, '/worktrees/feature'), true);
	assert.equal(sessionMatchesScope(session, '/worktrees/feature/'), true);
});

test('unrelated and similarly named worktrees are excluded', () => {
	assert.equal(sessionMatchesScope(session, '/worktrees/other'), false);
	assert.equal(sessionMatchesScope(session, '/worktrees/feat'), false);
});

test('sessions without worktree metadata match their cwd, including subdirectories', () => {
	assert.equal(sessionMatchesScope({...session, worktree: undefined}, '/worktrees/feature'), true);
	assert.equal(sessionMatchesScope({...session, cwd: '/worktrees/feature/src', worktree: undefined}, '/worktrees/feature'), true);
});
