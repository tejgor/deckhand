import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import path from 'node:path';
import {applyWorktreeLinks, expandWorktreeTemplate, worktreeLocation} from '../src/worktreeLinks.js';
import {resolveSettings, trustProjectConfig, validateProjectConfig, type LoadedProject} from '../src/projectConfig.js';
import {removeWorktree} from '../src/git.js';
import {getHandoffGitContext, inspectWorkspaceCleanup} from '../src/workspaceGit.js';
import {handoffMarkdown} from '../src/sessionFeatures.js';
import type {SessionRecord} from '../src/types.js';
import {git, repo, tempDir} from './helpers.js';

test('worktree templates expand known placeholders; validation rejects escapes; merge rules and trust', () => {
	const vars = {name: 'feat/x', repo: 'app', repoParent: '/dev', repoRoot: '/dev/app', home: '/h'};
	assert.equal(worktreeLocation('{repoParent}/worktrees/{name}', vars), '/dev/worktrees/feat/x');
	assert.equal(worktreeLocation('~/wt/{repo}/{name}', vars), '/h/wt/app/feat/x');
	assert.equal(expandWorktreeTemplate('{repoRoot}-{repo}', vars), '/dev/app-app');
	assert.throws(() => expandWorktreeTemplate('/x/{nope}', vars), /Unknown placeholder/);
	assert.throws(() => worktreeLocation('wt/{name}', vars), /absolute/);
	const invalid = [{location: 'relative/{name}'}, {location: '/x/{nope}/{name}'}, {location: '/x/{name'}, {location: '/fixed'}, {symlink: ['../up']}, {symlink: ['/abs']}, {symlink: ['a/../b']},
		{symlink: ['./a']}, {symlink: ['a/']}, {symlink: ['.git/hooks']}, {symlink: 'a'}, {symlink: Array.from({length: 51}, (_, i) => `d${i}`)}, {files: {'a\0b': '/x'}}, {files: {'../x': '/x'}}, {files: {ok: ''}}, {other: true}];
	for (const worktree of invalid) assert.throws(() => validateProjectConfig({worktree}), undefined, JSON.stringify(worktree));
	const global = {location: '~/global/{name}', symlink: ['a', 'b'], files: {'x/.env': '{repoParent}/global.env', z: '/global-z'}};
	const project = {root: '/r', trustRoot: '/r', path: '/r/deckhand.json', exists: true, fingerprint: 'f', config: {worktree: {location: '{repoParent}/repo/{name}', symlink: ['b', 'c'], files: {'x/.env': 'repo.env', y: '/repo-y'}}}} as LoadedProject;
	const user = {defaults: {worktree: global}};
	assert.deepEqual(resolveSettings(project, user).worktree, global); // Untrusted: global only.
	assert.deepEqual(resolveSettings(project, trustProjectConfig(project, user)).worktree, {location: '{repoParent}/repo/{name}', symlink: ['a', 'b', 'c'], files: {'x/.env': 'repo.env', z: '/global-z', y: '/repo-y'}});
	assert.equal(resolveSettings(undefined, {}).worktree, undefined);
});

test('links resolve real targets, skip missing sources, keep real content and refuse escapes; cleanup treats them as disposable and removal keeps targets', async t => {
	const launch = await repo(); t.after(() => fs.rm(launch, {recursive: true, force: true}));
	const outside = await fs.realpath(await tempDir(t, 'dh-links-'));
	await fs.mkdir(path.join(outside, 'deps')); await fs.writeFile(path.join(outside, 'deps', 'keep.js'), 'dependency');
	await fs.writeFile(path.join(outside, 'backend.env'), 'SECRET=1');
	// The launch checkout's node_modules is itself a link: the new one points at the real directory.
	await fs.mkdir(path.join(launch, 'frontend')); await fs.symlink(path.join(outside, 'deps'), path.join(launch, 'frontend', 'node_modules'));
	await fs.writeFile(path.join(launch, 'tracked.txt'), 'real'); await fs.symlink(outside, path.join(launch, 'escape'));
	await git(launch, 'add', 'tracked.txt', 'escape'); await git(launch, 'commit', '-m', 'tracked');
	const worktree = path.join(outside, 'wt'); await git(launch, 'worktree', 'add', '-b', 'wt', worktree);
	await fs.mkdir(path.join(worktree, 'backend')); await fs.symlink('/nonexistent', path.join(worktree, 'backend', '.env')); // A stale link is replaced.
	const result = await applyWorktreeLinks(
		{symlink: ['frontend/node_modules', 'backend/.venv', 'tracked.txt'], files: {'backend/.env': '{repoParent}/backend.env', 'escape/stolen': '{repoParent}/backend.env'}},
		{launchRoot: launch, worktreeRoot: worktree, vars: {name: 'wt', repo: 'app', repoParent: outside, repoRoot: launch, home: outside}},
	);
	assert.deepEqual(result.linked, ['frontend/node_modules', 'backend/.env']);
	assert.equal(await fs.readlink(path.join(worktree, 'frontend', 'node_modules')), path.join(outside, 'deps'));
	assert.equal(await fs.readFile(path.join(worktree, 'backend', '.env'), 'utf8'), 'SECRET=1');
	assert.match(result.notes.join('\n'), /backend\/\.venv: source missing/); assert.match(result.notes.join('\n'), /tracked\.txt: exists and is not a symlink, kept/);
	assert.match(result.notes.join('\n'), /escape\/stolen: parent escape leaves the worktree/);
	assert.equal(await fs.readFile(path.join(worktree, 'tracked.txt'), 'utf8'), 'real');
	await assert.rejects(fs.access(path.join(outside, 'stolen')));
	const inspection = await inspectWorkspaceCleanup(worktree, 'main', {deleteBranch: true});
	assert.equal(inspection.safe, true, inspection.reasons.join('; '));
	await removeWorktree(worktree, launch, 'wt');
	await assert.rejects(fs.access(worktree));
	assert.equal(await fs.readFile(path.join(outside, 'deps', 'keep.js'), 'utf8'), 'dependency');
	assert.equal(await fs.readFile(path.join(outside, 'backend.env'), 'utf8'), 'SECRET=1');
});

test('handoffs list commit subjects and changed file names, never diff content', async t => {
	const cwd = await repo(); t.after(() => fs.rm(cwd, {recursive: true, force: true}));
	await git(cwd, 'checkout', '-b', 'feature');
	await fs.writeFile(path.join(cwd, 'file.txt'), 'COMMITTED_DIFF_LINE\n'); await git(cwd, 'commit', '-am', 'Add the feature');
	await fs.writeFile(path.join(cwd, 'notes.md'), 'UNCOMMITTED_CONTENT\n');
	const context = await getHandoffGitContext(cwd, 'main');
	const markdown = handoffMarkdown({id: 's', title: 'Task', program: 'claude', cwd, notes: 'n'} as SessionRecord, false, context);
	assert.match(markdown, /## Workspace changes/); assert.match(markdown, /Base: main/); assert.match(markdown, /- [0-9a-f]+ Add the feature/);
	assert.match(markdown, /- \?\? notes\.md/); assert.match(markdown, /1 file\(s\) changed, 1 insertion\(s\), 1 deletion\(s\)/); assert.match(markdown, /- file\.txt \+1 -1/);
	assert.doesNotMatch(markdown, /COMMITTED_DIFF_LINE|UNCOMMITTED_CONTENT/);
	assert.equal(await getHandoffGitContext(path.dirname(cwd)), undefined); // Outside a repository: omitted.
	assert.match(handoffMarkdown({id: 's', title: 'T', program: 'pi', cwd} as SessionRecord, false, await getHandoffGitContext(cwd, '--all')), /Git information unavailable: Invalid Git/);
});
