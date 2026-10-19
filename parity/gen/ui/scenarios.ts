// The screens, in order. Each scenario starts a fresh isolated app (see harness.ts) and walks it with keys; `snap`
// records a screen once its marker is shown and the screen is idle. Markers are the replay contract: a replaying harness
// presses the same keys and waits for the same text before comparing.
import fs from 'node:fs/promises';
import path from 'node:path';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {KEY, type Session} from './harness.ts';

const exec = promisify(execFile);

export type Scenario = {name: string; about: string; run: (s: Session) => Promise<void>};

/** n (N: a sub-session) → agent → name → (tab presses) → enter, then waits until the new session's agent is idle. */
async function create(s: Session, name: string, options: {tabs?: number; sub?: boolean; down?: number; pick?: string} = {}): Promise<void> {
	await s.step(options.sub ? 'N' : 'n', 'Choose an agent');
	for (let index = 0; index < (options.down ?? 0); index++) s.press(KEY.down);
	// Enter reads the picked row from the last render: wait for it (keys in one burst would race it).
	if (options.pick) await s.wait(options.pick);
	await s.step(KEY.enter, 'Name: ');
	await s.step(name, `Name: ${name}`);
	for (let index = 0; index < (options.tabs ?? 0); index++) await s.step(KEY.tab, index === 0 ? 'new worktree' : 'existing worktree');
	s.press(KEY.enter);
	await settle(s, name);
}

/**
 * Waits for the selected session `title` to run with its agent idle: the details under the list name it and say
 * `· idle ·`, and no spinner is left (the fake agent prints once, so it goes idle 5 s after starting).
 */
async function settle(s: Session, title: string): Promise<void> {
	await s.wait(`│ ${title} `);
	await s.wait('· idle ·');
	await s.waitGone('⠋');
}

const dashboard: Scenario = {
	name: 'dashboard',
	about: 'No sessions: the empty dashboard and every overlay reachable from it.',
	async run(s) {
		await s.snap('empty-dashboard', '● ready');
		s.press('?'); await s.snap('help', 'Help · Start here');
		s.press('j'); await s.snap('help-topic', 'Help · Sessions');
		await s.step('/', 'type to search'); s.press('merge'); await s.snap('help-search', '/ merge');
		await s.step(KEY.esc, 'Help · Sessions'); await s.step(KEY.esc, '? help');
		await s.step('/', 'Search: /'); s.press('zz'); await s.snap('search', 'Search: /zz');
		await s.step(KEY.esc, '? help');
		s.press('f'); await s.snap('filter-menu', 'f back to active');
		await s.step(KEY.esc, '? help');
		s.press('C'); await s.snap('settings', 'Settings · demo');
		s.press(KEY.enter); await s.snap('settings-edit', 'Default agent ·');
		await s.step(KEY.esc, 'Settings · demo');
		s.press(KEY.left); await s.wait('Built-in default'); s.press('j'); await s.wait('❯ Default workspace'); s.press('j'); await s.snap('settings-global', '❯ Dev command');
		s.press('E'); await s.snap('config-editor-global', 'config.json "defaults"');
		await s.step(KEY.esc, 'Settings · demo'); await s.step(KEY.right, 'This repo · ');
		s.press('E'); await s.snap('config-editor', 'Repository config');
		s.press(KEY.ctrlA); await s.wait('all selected'); s.press('{"devCommand":"echo changed"}');
		await s.snap('config-editor-edited', 'echo changed');
		s.press(KEY.esc); await s.snap('config-discard', 'Discard unsaved configuration?');
		await s.step(KEY.enter, 'Settings · demo'); await s.step(KEY.esc, '? help');
		s.press('U'); await s.snap('agents', 'Agents');
		await s.step(KEY.esc, '? help');
		s.press('b'); await s.snap('tasks-empty', 'No tasks yet');
		s.press('a'); await s.wait('New task'); s.press('Write the docs'); await s.snap('task-new', 'Write the docs');
		s.press(KEY.tab); await s.wait('ctrl+s save'); s.press('why: people ask'); await s.snap('task-new-details', 'why: people ask');
		s.press(KEY.ctrlS); await s.snap('tasks-board', 'Added to the backlog');
		await s.waitGone('Added to the backlog');
		s.press(KEY.enter); await s.snap('task-edit', 'Edit task');
		await s.step(KEY.esc, 'a add');
		s.press('w'); await s.snap('task-move', 'Move “Write the docs” to');
		await s.step(KEY.esc, 'a add');
		s.press(KEY.tab); await s.snap('tasks-notes', '✎ Notes');
		await s.step(KEY.tab, '☐ Tasks'); await s.step(KEY.esc, '? help');
		s.press('W'); await s.snap('worktrees-empty', 'Worktrees');
		await s.step(KEY.esc, '? help');
		s.press('n'); await s.snap('create-pick-agent', 'Choose an agent');
		s.press(KEY.enter); await s.wait('Name: '); s.press('gamma'); await s.snap('create-name', 'Name: gamma');
		s.press(KEY.tab); await s.snap('create-worktree-new', 'new worktree');
		s.press(KEY.tab); await s.snap('create-worktree-existing', 'existing worktree');
		s.press(KEY.enter); await s.snap('pick-worktree', 'Existing worktree');
		await s.step(KEY.esc, 'existing worktree'); await s.step(KEY.esc, 'Choose an agent'); await s.step(KEY.esc, '? help');
		await s.resize(60, 36); await s.snap('narrow-60', '? help');
		await s.resize(130, 36); await s.snap('resize-back-130', 'q quit');
	},
};

const sessions: Scenario = {
	name: 'sessions',
	about: 'Several sessions (main checkout, worktree, sub-session, fork, waiting, unread reply, archived) and every tab, focus and confirmation on them.',
	async run(s) {
		const writeFile = (relative: string, content: string) => s.external('write-file', {path: `<TMP>/${relative}`, content}, () => fs.writeFile(path.join(s.base, relative), content));
		const hook = (title: string, id: string, payload: Record<string, unknown>) => s.external('hook', {session: title, payload}, () => s.hook(id, payload));
		await s.wait('● ready');
		await create(s, 'alpha');
		await create(s, 'beta', {tabs: 1});
		s.press('N'); await s.snap('create-subsession-pick', 'Choose an agent');
		s.press(KEY.esc); await s.wait('? help');
		await create(s, 'gamma', {sub: true});
		s.press(KEY.up); await s.wait('│ beta ');
		await create(s, 'delta', {sub: true, down: 3, pick: '❯ ⑂ Fork parent'});
		await create(s, 'eps', {tabs: 1});
		const ids = Object.fromEntries((await s.traces()).map((trace, index) => [['alpha', 'beta', 'gamma', 'delta', 'eps'][index], trace.sessionId]));
		// alpha waits on a permission prompt (?), gamma has an unread reply (◆).
		await hook('alpha', ids.alpha!, {hook_event_name: 'Notification', notification_type: 'permission_prompt', message: 'Claude needs your permission to use Bash'});
		await hook('gamma', ids.gamma!, {hook_event_name: 'Stop', last_assistant_message: 'All done: the tests pass.'});
		await s.wait('1 ? alpha'); await s.wait('↳ ◆ gamma');
		s.press('1'); await s.snap('sidebar-sessions', '│ alpha ');
		s.press('2'); await s.snap('tab-preview', '│ beta ');
		await writeFile('home/worktrees/beta/notes.md', 'scratch\n');
		await writeFile('home/worktrees/beta/README.md', '# Demo\n\nEdited in the worktree.\n');
		s.press('t'); await s.snap('tab-terminal', '$');
		s.press('v'); await s.snap('error-footer', 'Error: No action has run in this worktree yet');
		await s.waitGone('Error: ');
		s.press('g'); await s.snap('tab-git', 'Untracked 1');
		s.press(KEY.right); await s.snap('changes-focus', 'space stage/unstage');
		await s.step(KEY.esc, 'enter lazygit');
		s.press('d'); await s.snap('tab-dev', 'd d start Dev');
		s.press('a'); await s.snap('tab-notes', 'enter edit notes');
		s.press(KEY.enter); await s.wait('esc done'); s.press('Remember the tests'); await s.snap('notes-focus', 'Remember the tests');
		await s.step(KEY.esc, 'enter edit notes');
		s.press('p'); await s.wait('scroll'); s.press(KEY.right); await s.snap('preview-focus', 'preview focus');
		await s.step(KEY.esc, 'enter open');
		s.press('i'); await s.snap('workspace-info', 'Workspace overview');
		s.press('c'); await s.snap('workspace-create-pr', 'Create pull request');
		await s.step(KEY.esc, 'Workspace overview'); await s.step(KEY.esc, 'enter open');
		s.press('r'); await s.snap('run-list', 'needs trust');
		s.press('j'); await s.wait('Selected command'); s.press(KEY.enter); await s.snap('review', 'Review repository configuration');
		await s.step(KEY.esc, 'needs trust'); await s.step(KEY.esc, 'enter open');
		s.press('m'); await s.snap('confirm-merge', 'Merge "beta"?');
		await s.step(KEY.esc, 'enter open');
		// beta's worktree is shared with gamma and delta: deleting it is blocked.
		s.press('x'); await s.snap('confirm-kill-blocked', 'Worktree deletion is blocked');
		await s.step(KEY.esc, 'enter open');
		await writeFile('home/worktrees/eps/draft.txt', 'unsaved work\n');
		s.press('5'); await s.wait('│ eps ');
		s.press('x'); await s.snap('confirm-kill', 'Deleting the worktree would lose');
		s.press('j'); await s.wait('❯ Kill and delete worktree'); s.press(KEY.enter); await s.snap('confirm-loss', 'Type DELETE then enter');
		s.press('DEL'); await s.snap('confirm-loss-typing', 'Type DELETE then enter: DEL');
		await s.step(KEY.esc, 'enter open');
		s.press('W'); await s.snap('worktrees', 'Worktrees');
		s.press('x'); await s.snap('worktrees-cleanup', 'Delete eps?');
		await s.step(KEY.esc, 'Worktrees'); await s.step(KEY.esc, 'enter open');
		s.press('b'); await s.snap('tasks-board-sessions', 'Tasks · demo');
		await s.step(KEY.esc, 'enter open');
		s.press('1'); await s.wait('│ alpha ');
		s.press('X'); await s.wait('s resume');
		s.press(KEY.backspace); await s.wait('Archived alpha');
		await s.waitGone('Archived alpha');
		await s.snap('sidebar-after-archive', '│ beta ');
		s.press('f'); await s.wait('f back to active'); s.press('A'); await s.snap('sidebar-archived', 'alpha');
		s.press(KEY.backspace); await s.snap('confirm-remove', 'for good?');
		await s.step(KEY.esc, 'backspace remove');
		s.press('f'); await s.wait('f back to active'); s.press('f'); await s.wait('│ beta ');
		await s.resize(60, 36); await s.snap('narrow-sessions', 'beta');
		await s.resize(130, 36); await s.snap('resize-back-sessions', 'q quit');
	},
};

const merge: Scenario = {
	name: 'merge',
	about: 'A worktree session whose branch conflicts with main: the merge confirmation with a commit to merge, the conflict screen, and aborting it.',
	async run(s) {
		const writeFile = (relative: string, content: string) => s.external('write-file', {path: `<TMP>/${relative}`, content}, () => fs.writeFile(path.join(s.base, relative), content));
		// Fixed dates keep the commits identical from run to run.
		const commit = (relative: string, message: string) => s.external('git', {cwd: `<TMP>/${relative}`, args: ['commit', '-am', message], env: FIXED_GIT_DATES}, () => git(path.join(s.base, relative), FIXED_GIT_DATES, 'commit', '-am', message));
		await s.wait('● ready');
		await create(s, 'feat', {tabs: 1});
		await writeFile('home/worktrees/feat/file.txt', 'from feat\n');
		await commit('home/worktrees/feat', 'feat change');
		await writeFile('demo/file.txt', 'from main\n');
		await commit('demo', 'main change');
		s.press('m'); await s.wait('Merge "feat"?'); await s.snap('confirm-merge-commits', '1 commit');
		s.press(KEY.enter); await s.snap('merge-conflicts', 'file.txt');
		s.press('x'); await s.snap('merge-aborted', 'Merge aborted');
	},
};

const FIXED_GIT_DATES = {GIT_AUTHOR_DATE: '2026-01-01T00:00:00Z', GIT_COMMITTER_DATE: '2026-01-01T00:00:00Z'};

async function git(cwd: string, env: Record<string, string>, ...args: string[]): Promise<void> {
	await exec('git', ['-C', cwd, ...args], {env: {...process.env, GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1', ...env}});
}

export const SCENARIOS: Scenario[] = [dashboard, sessions, merge];
