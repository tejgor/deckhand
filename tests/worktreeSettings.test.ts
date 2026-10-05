import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import path from 'node:path';
import {applyWorktreeLinks, expandWorktreeTemplate, userSlug, worktreeLocation} from '../src/worktreeLinks.js';
import {isProjectTrusted, loadProjectConfig, projectNeedsReview, resolveSettings, trustProjectConfig, validateProjectConfig, type LoadedProject} from '../src/projectConfig.js';
import {createWorktreeForSession, removeWorktree} from '../src/git.js';
import {getHandoffGitContext, inspectWorkspaceCleanup} from '../src/workspaceGit.js';
import {handoffMarkdown} from '../src/sessionFeatures.js';
import type {SessionRecord} from '../src/types.js';
import {git, repo, tempDir, withEnv} from './helpers.js';

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

test('branchFrom current/default/origin and branchName templates decide the new branch; baseRef records the base; fetch failures fail creation', async t => {
	for (const worktree of [{branchFrom: 'upstream'}, {branchName: 'feat/x'}, {branchName: 'bad name/{name}'}, {branchName: '{name}..x'}, {branchName: '{name}.lock'}, {branchName: '{nope}/{name}'}, {hook: 'no'}]) assert.throws(() => validateProjectConfig({worktree}), undefined, JSON.stringify(worktree));
	assert.deepEqual(validateProjectConfig({worktree: {branchFrom: 'default', branchName: 'feat/{user}-{name}', hook: false}}).worktree, {branchFrom: 'default', branchName: 'feat/{user}-{name}', hook: false});
	const home = await tempDir(t, 'deckhand-branch-'); withEnv(t, {DECKHAND_HOME: home});
	const cwd = await repo(); t.after(() => fs.rm(cwd, {recursive: true, force: true}));
	const remote = path.join(home, 'origin.git'), other = path.join(home, 'other');
	await git(home, 'clone', '--bare', cwd, remote); await git(cwd, 'remote', 'add', 'origin', remote); await git(cwd, 'fetch', 'origin'); await git(cwd, 'remote', 'set-head', 'origin', 'main');
	// origin/main gets a commit the local main does not have; the launch checkout is on a feature branch.
	await git(home, 'clone', remote, other); await git(other, 'config', 'user.email', 'o@example.invalid'); await git(other, 'config', 'user.name', 'O');
	await fs.writeFile(path.join(other, 'file.txt'), 'remote\n'); await git(other, 'commit', '-am', 'remote change'); await git(other, 'push', 'origin', 'main');
	const remoteHead = await git(other, 'rev-parse', 'HEAD'), localMain = await git(cwd, 'rev-parse', 'main');
	await git(cwd, 'checkout', '-b', 'feature'); await fs.writeFile(path.join(cwd, 'file.txt'), 'feature\n'); await git(cwd, 'commit', '-am', 'feature');
	const feature = await git(cwd, 'rev-parse', 'HEAD');

	const current = await createWorktreeForSession('from current', cwd);
	assert.equal(current.branch, 'from_current'); assert.equal(current.head, feature); assert.equal(current.baseRef, 'feature');
	const local = await createWorktreeForSession('from default', cwd, undefined, {branchFrom: 'default', branchName: '{user}/{name}'});
	assert.equal(local.branch, `${userSlug()}/from_default`); assert.equal(local.head, localMain); assert.equal(local.baseRef, 'main');
	const fetched = await createWorktreeForSession('from origin', cwd, undefined, {branchFrom: 'origin', branchName: 'feat/{name}'});
	assert.equal(fetched.branch, 'feat/from_origin'); assert.equal(fetched.head, remoteHead); assert.equal(fetched.baseRef, 'origin/main');
	assert.equal(await git(cwd, 'config', '--get', 'branch.feat/from_origin.remote').catch(() => ''), ''); // --no-track
	// Reuse: an existing worktree path is reused as before, without a base.
	const reused = await createWorktreeForSession('from origin', cwd, undefined, {branchFrom: 'origin', branchName: 'feat/{name}'});
	assert.equal(reused.origin, 'existing'); assert.equal(reused.baseRef, undefined);
	await assert.rejects(createWorktreeForSession('bad', cwd, undefined, {branchName: '{name}..x'}), /Invalid branch name/);
	await git(cwd, 'remote', 'set-url', 'origin', path.join(home, 'missing.git'));
	await assert.rejects(createWorktreeForSession('offline', cwd, undefined, {branchFrom: 'origin'}), /git fetch origin main failed/);
	await assert.rejects(fs.access(path.join(home, 'worktrees', 'offline')));
	assert.equal(await git(cwd, 'branch', '--list', 'offline'), '');
});

test('worktree.hook false: the hook never runs, is not fingerprinted or reviewed; untrusted false disables, untrusted true never enables', async t => {
	const cwd = await repo(); t.after(() => fs.rm(cwd, {recursive: true, force: true}));
	const home = await tempDir(t, 'deckhand-hook-off-'); withEnv(t, {DECKHAND_HOME: home});
	const bare = await loadProjectConfig(cwd);
	const file = path.join(cwd, '.claude', 'scripts', 'create-worktree.sh'); await fs.mkdir(path.dirname(file), {recursive: true});
	await fs.writeFile(file, '#!/bin/bash\ntouch "$CLAUDE_PROJECT_DIR/hook-ran"\nexit 1\n');
	assert.equal(projectNeedsReview(await loadProjectConfig(cwd), {}), true);
	// Global defaults switch it off: nothing to trust, the same fingerprint as no hook, and creation falls back.
	const off = {defaults: {worktree: {hook: false}}};
	const globalOff = await loadProjectConfig(cwd, off);
	assert.equal(globalOff.creationHook, undefined); assert.deepEqual(globalOff.disabledHook, {file: await fs.realpath(file).catch(() => file), by: 'global'});
	assert.equal(globalOff.fingerprint, bare.fingerprint); assert.equal(projectNeedsReview(globalOff, off), false);
	assert.equal((await createWorktreeForSession('no hook', cwd, globalOff.creationHook)).creator, 'fallback');
	await assert.rejects(fs.access(path.join(cwd, 'hook-ran')));
	// An untrusted repository may switch it off...
	await fs.writeFile(path.join(cwd, 'deckhand.json'), '{"worktree":{"hook":false}}');
	const repoOff = await loadProjectConfig(cwd);
	assert.equal(repoOff.creationHook, undefined); assert.equal(repoOff.disabledHook?.by, 'repository');
	assert.equal(isProjectTrusted(repoOff, {}), false); assert.equal(resolveSettings(repoOff, {}).worktree?.hook, false);
	// ...but never on: with global false, an untrusted hook: true is reviewed with the hook and not effective until trusted.
	await fs.writeFile(path.join(cwd, 'deckhand.json'), '{"worktree":{"hook":true}}');
	const repoOn = await loadProjectConfig(cwd, off);
	assert.ok(repoOn.creationHook); assert.equal(projectNeedsReview(repoOn, off), true); assert.equal(resolveSettings(repoOn, off).worktree?.hook, false);
	assert.equal(resolveSettings(repoOn, trustProjectConfig(repoOn, off)).worktree?.hook, true);
});
