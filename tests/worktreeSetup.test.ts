import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import path from 'node:path';
import {classifyCandidate, applyWorktreeSection, initialSetupModel, worktreeSection, type WorktreeSetupInfo} from '../src/worktreeSetup.js';
import {createWorktreeForSession} from '../src/git.js';
import {userSlug} from '../src/worktreeLinks.js';
import {isProjectTrusted, loadProjectConfig, projectNeedsReview, resolveSettings, trustProjectConfig, validateProjectConfig} from '../src/projectConfig.js';
import {git, repo, tempDir, withEnv} from './helpers.js';

test('candidate classification links dependencies and env files, skips build/cache/clutter and everything else', () => {
	const link = ['node_modules', 'frontend/node_modules', 'backend/.venv', 'venv', 'vendor', 'web/.vercel', '.env', 'backend/.env', '.env.local', 'config/.env.production', 'backend.env'];
	const skip = ['dist', 'web/build', 'out', 'coverage', '.next', '.turbo', '.cache', 'pkg/__pycache__', '.pytest_cache', '.DS_Store', 'debug.log', 'notes.txt', 'environment', 'tmp'];
	for (const entry of link) assert.equal(classifyCandidate(entry).suggestion, 'link', entry);
	for (const entry of skip) assert.equal(classifyCandidate(entry).suggestion, 'skip', entry);
	assert.deepEqual(classifyCandidate('a/node_modules'), {suggestion: 'link', reason: 'dependencies'});
	assert.deepEqual(classifyCandidate('dist'), {suggestion: 'skip', reason: 'build output'});
	assert.deepEqual(classifyCandidate('notes.txt'), {suggestion: 'skip', reason: ''});
});

test('applying a setup writes only the worktree section of the chosen target, keeping other keys and files; branch settings validate', () => {
	const candidate = (entry: string, extra = {}) => ({path: entry, kind: 'dir' as const, ignored: true, ...classifyCandidate(entry), ...extra});
	const info = {
		cwd: '/r', repo: 'r', checkout: '/r', repositoryTrusted: false, repositoryNeedsReview: true, layerErrors: {}, candidates: [
			candidate('shared', {configured: 'symlink', layers: ['global']}), candidate('backend/.env', {configured: 'files', source: '{repoParent}/b.env', layers: ['repository']}),
			candidate('node_modules'), candidate('dist'),
		],
		moreCandidates: 0, vars: {name: '<name>', repo: 'r', repoParent: '/', repoRoot: '/r', home: '/h'}, defaultLocation: '/h/.deckhand/worktrees/<name>', insideIgnored: false, user: 'me',
		layers: {global: {symlink: ['shared'], branchFrom: 'origin'}, repository: {location: '{repoParent}/worktrees/{name}', files: {'backend/.env': '{repoParent}/b.env'}}},
		targets: {
			repository: {kind: 'repository', root: '/r', path: '/r/deckhand.json', raw: '{"devCommand":"npm run dev","worktree":{"location":"{repoParent}/worktrees/{name}","files":{"backend/.env":"{repoParent}/b.env"}},"actions":{"t":"npm test"}}', revision: 'x', exists: true},
			global: {kind: 'global', root: '/h', path: '/h/config.json', raw: '{}', revision: null, exists: false},
		},
	} as WorktreeSetupInfo;
	const model = initialSetupModel(info);
	assert.deepEqual(model, {target: 'repository', location: 'next', branchFrom: 'origin', branchName: '{name}', hook: true, links: {shared: true, 'backend/.env': true, node_modules: true, dist: false}});
	// Repository target: inherited values (global branchFrom origin) are not repeated; files stay; other keys keep their place.
	const raw = applyWorktreeSection(info.targets.repository!, worktreeSection({...model, links: {...model.links, dist: true}, branchName: '{user}/{name}'}, info));
	assert.equal(raw, `${JSON.stringify({devCommand: 'npm run dev', worktree: {location: '{repoParent}/worktrees/{name}', symlink: ['shared', 'node_modules', 'dist'], files: {'backend/.env': '{repoParent}/b.env'}, branchName: '{user}/{name}'}, actions: {t: 'npm test'}}, null, 2)}\n`);
	// Global target: "Deckhand default" writes no location; choices that differ from built-in defaults are explicit; no files of its own.
	const global = JSON.parse(applyWorktreeSection(info.targets.global!, worktreeSection({...model, target: 'global', location: 'default', branchFrom: 'current', hook: false}, info)));
	assert.deepEqual(global, {worktree: {symlink: ['shared', 'node_modules'], hook: false}});
	assert.throws(() => applyWorktreeSection({...info.targets.repository!, raw: '{'}, {}), /press e/);
	assert.equal(JSON.parse(applyWorktreeSection(info.targets.repository!, {})).worktree, undefined);
	for (const worktree of [{branchFrom: 'upstream'}, {branchName: 'feat/x'}, {branchName: 'bad name/{name}'}, {branchName: '{name}..x'}, {branchName: '{name}.lock'}, {branchName: '{nope}/{name}'}, {hook: 'no'}]) assert.throws(() => validateProjectConfig({worktree}), undefined, JSON.stringify(worktree));
	assert.deepEqual(validateProjectConfig({worktree: {branchFrom: 'default', branchName: 'feat/{user}-{name}', hook: false}}).worktree, {branchFrom: 'default', branchName: 'feat/{user}-{name}', hook: false});
});

test('branchFrom current/default/origin and branchName templates decide the new branch; baseRef records the base; fetch failures fail creation', async t => {
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
