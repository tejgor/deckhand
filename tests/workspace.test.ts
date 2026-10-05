import assert from 'node:assert/strict';
import {test} from 'node:test';
import {noWorkspaceReason, workspaceKey, workspaceWorkerId} from '../src/workspace.js';
import type {SessionRecord} from '../src/types.js';

test('workspace key: the worktree a session runs in, none for deleted or unprepared worktrees', () => {
	const plain = {cwd: '/repos/app', launchWorktreeRoot: '/repos/app', worktree: {mode: 'none'}, requestedWorktreeMode: 'none'} as SessionRecord;
	const managed = {...plain, cwd: '/wt/feature', worktree: {mode: 'managed', path: '/wt/feature/'}, requestedWorktreeMode: 'new'} as SessionRecord;
	const cases: [string, SessionRecord, string | undefined][] = [
		['no worktree: the launch checkout root', plain, '/repos/app'],
		['no worktree launched in a linked worktree: that worktree', {...plain, cwd: '/wt/other', launchWorktreeRoot: '/wt/other'}, '/wt/other'],
		['no worktree, older record without launchWorktreeRoot: cwd', {...plain, launchWorktreeRoot: undefined}, '/repos/app'],
		['managed worktree: its path, resolved', managed, '/wt/feature'],
		['attached worktree (here the main checkout): shares the plain session workspace', {...plain, worktree: {mode: 'attached', path: '/repos/app', isMain: true}, requestedWorktreeMode: 'existing'} as SessionRecord, '/repos/app'],
		['launch root never wins over the worktree', {...managed, launchWorktreeRoot: '/repos/app'}, '/wt/feature'],
		['deleted worktree: no workspace (a new worktree at that path is a new workspace)', {...managed, worktree: {...managed.worktree!, deletedAt: '2026-01-01T00:00:00.000Z'}}, undefined],
		['new worktree still being created: not the launch checkout', {...plain, requestedWorktreeMode: 'new'}, undefined],
		['existing worktree not selected yet', {...plain, requestedWorktreeMode: 'existing'}, undefined],
		['worktree record without a path', {...managed, worktree: {mode: 'managed'}} as SessionRecord, undefined],
	];
	for (const [name, session, expected] of cases) assert.equal(workspaceKey(session), expected, name);
	assert.match(noWorkspaceReason(cases[6]![1]), /deleted/);
	assert.match(noWorkspaceReason(cases[7]![1]), /not ready/);
	assert.equal(workspaceWorkerId('/wt/feature'), workspaceWorkerId('/wt/feature'));
	assert.notEqual(workspaceWorkerId('/wt/feature'), workspaceWorkerId('/wt/other'));
	assert.match(workspaceWorkerId('/wt/feature'), /^workspace-[0-9a-f]{16}$/);
});
