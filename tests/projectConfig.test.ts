import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import path from 'node:path';
import {loadProjectConfig, parseProjectConfig, isProjectTrusted, projectNeedsReview, trustProjectConfig, resolveDevCommand, resolveSettings, resolveSetupCommand, type LoadedProject} from '../src/projectConfig.js';
import {readProjectConfigDocument} from '../src/projectConfigDocument.js';
import {git, repo} from './helpers.js';
test('strict project schema rejects malformed/unknown commands and defaults', () => {
	for (const value of ['[]', '{"unknown":true}', '{"defaultAgent":["pi"]}', '{"defaultWorkspace":null}', '{"setupCommand":""}', '{"actions":{"bad/name":"echo ok"}}', '{"actions":{"__proto__":"echo bad"}}']) assert.throws(() => parseProjectConfig(value));
	assert.deepEqual(parseProjectConfig('{"defaultAgent":"codex","actions":{"test":"npm test"}}'), {defaultAgent: 'codex', actions: {test: 'npm test'}});
});
test('effective settings: global defaults overlaid by the repository override only while trusted; actions merge by name', () => {
	const project = {root: '/r', trustRoot: '/r', path: '/r/deckhand.json', exists: true, fingerprint: 'f', config: {defaultAgent: 'pi', devCommand: 'repo dev', setupCommand: 'repo setup', actions: {test: 'repo test', lint: 'repo lint'}}} as LoadedProject;
	const user = {dev_command: 'legacy', defaults: {defaultAgent: 'codex', defaultWorkspace: 'new', setupCommand: 'npm ci', actions: {test: 'global test', fmt: 'global fmt'}}};
	assert.deepEqual(resolveSettings(project, user), {defaultAgent: 'codex', defaultWorkspace: 'new', setupCommand: 'npm ci', devCommand: 'legacy', actions: {test: 'global test', fmt: 'global fmt'}});
	const trusted = trustProjectConfig(project, user);
	assert.deepEqual(resolveSettings(project, trusted), {defaultAgent: 'pi', defaultWorkspace: 'new', setupCommand: 'repo setup', devCommand: 'repo dev', actions: {test: 'repo test', fmt: 'global fmt', lint: 'repo lint'}});
	assert.equal(resolveDevCommand(undefined, {}), 'dev');
	assert.equal(resolveDevCommand(project, {dev_command: 'legacy', defaults: {devCommand: 'npm run dev'}}), 'npm run dev');
	assert.throws(() => resolveSettings(project, {defaults: {setupCommand: ''}}), /Invalid "defaults".*Invalid command/);
	// An untrusted repository setupCommand is refused unless the user reviewed exactly these bytes and skipped them.
	assert.throws(() => resolveSetupCommand(project, user), /not trusted/);
	assert.equal(resolveSetupCommand(project, user, 'f'), 'npm ci');
	assert.equal(resolveSetupCommand(project, trusted), 'repo setup');
});
test('the repository override is the main checkout\'s live deckhand.json; worktree copies are ignored', async t => {
	const cwd = await repo(), other = await repo(); t.after(async () => { await fs.rm(cwd, {recursive: true, force: true}); await fs.rm(other, {recursive: true, force: true}); });
	assert.equal(projectNeedsReview(await loadProjectConfig(other), {}), false); // Nothing to trust.
	const raw = JSON.stringify({devCommand: 'touch DO_NOT_EXECUTE', defaultWorkspace: 'new'});
	await fs.writeFile(path.join(cwd, 'deckhand.json'), raw); await fs.writeFile(path.join(other, 'deckhand.json'), raw);
	await git(cwd, 'add', 'deckhand.json'); await git(cwd, 'commit', '-m', 'project config');
	const worktree = path.join(cwd, 'linked'); await git(cwd, 'worktree', 'add', '-b', 'linked', worktree);
	const project = await loadProjectConfig(worktree);
	assert.equal(project.path, path.join(await fs.realpath(cwd), 'deckhand.json')); assert.equal(projectNeedsReview(project, {}), true);
	assert.equal(resolveDevCommand(project, {dev_command: 'global'}), 'global');
	const config = trustProjectConfig(project, {dev_command: 'global'});
	assert.equal(resolveDevCommand(project, config), 'touch DO_NOT_EXECUTE');
	assert.equal(isProjectTrusted(await loadProjectConfig(cwd), config), true); // Same repository from any checkout.
	assert.equal(isProjectTrusted(await loadProjectConfig(other), config), false);
	await fs.writeFile(path.join(worktree, 'deckhand.json'), '{"devCommand":"printf worktree-copy"}');
	assert.equal((await loadProjectConfig(worktree)).fingerprint, project.fingerprint);
	// An uncommitted edit in the main checkout applies to worktree sessions at once, and needs a fresh review.
	await fs.writeFile(path.join(cwd, 'deckhand.json'), '{"devCommand":"printf live"}');
	const live = await loadProjectConfig(worktree);
	assert.equal(live.config.devCommand, 'printf live'); assert.equal(projectNeedsReview(live, config), true);
	assert.equal(resolveDevCommand(live, trustProjectConfig(live, config)), 'printf live');
	await assert.rejects(fs.access(path.join(cwd, 'DO_NOT_EXECUTE')));
});
test('trust keeps several fingerprints per repository, newest first and capped', async t => {
	const cwd = await repo(); t.after(() => fs.rm(cwd, {recursive: true, force: true}));
	const first = await loadProjectConfig(cwd); let config = trustProjectConfig(first, {});
	await fs.writeFile(path.join(cwd, 'deckhand.json'), '{}'); const second = await loadProjectConfig(cwd);
	config = trustProjectConfig(second, trustProjectConfig(first, config));
	assert.equal(isProjectTrusted(first, config) && isProjectTrusted(second, config), true);
	assert.deepEqual(config.trustedProjects?.[first.trustRoot], [second.fingerprint, first.fingerprint]);
	for (let i = 0; i < 30; i++) config = trustProjectConfig({...first, fingerprint: String(i)}, config);
	assert.equal(config.trustedProjects?.[first.trustRoot]?.length, 20); assert.equal(isProjectTrusted(first, config), false);
});
test('trust roots are unique for submodules and bare repositories; bare repositories have no repository override', async t => {
	const outer = await repo(), inner = await repo(); t.after(async () => { await fs.rm(outer, {recursive: true, force: true}); await fs.rm(inner, {recursive: true, force: true}); });
	await git(outer, '-c', 'protocol.file.allow=always', 'submodule', 'add', inner, 'sub');
	const submodule = await loadProjectConfig(path.join(outer, 'sub'));
	assert.equal(submodule.trustRoot, await fs.realpath(path.join(outer, 'sub'))); assert.notEqual(submodule.trustRoot, (await loadProjectConfig(outer)).trustRoot);
	const bare = path.join(outer, 'bare.git'), linked = path.join(outer, 'bare-linked');
	await git(outer, 'clone', '--bare', inner, bare); await git(bare, 'worktree', 'add', '-b', 'linked', linked);
	const project = await loadProjectConfig(linked);
	assert.equal(project.trustRoot, await fs.realpath(bare)); assert.equal(project.root, await fs.realpath(linked));
	assert.equal(project.path, undefined); assert.equal(project.exists, false);
	await assert.rejects(readProjectConfigDocument(linked), /Bare repositories/);
});
test('malformed, oversized and symlinked config errors are not hidden', async t => {
	const cwd = await repo(); t.after(() => fs.rm(cwd, {recursive: true, force: true}));
	const file = path.join(cwd, 'deckhand.json');
	await fs.writeFile(file, '{'); await assert.rejects(loadProjectConfig(cwd), /deckhand.json/);
	await fs.writeFile(file, ' '.repeat(70000)); await assert.rejects(loadProjectConfig(cwd), /64 KiB/);
	await fs.rm(file); await fs.writeFile(path.join(cwd, 'target.json'), '{}'); await fs.symlink(path.join(cwd, 'target.json'), file);
	await assert.rejects(loadProjectConfig(cwd), /must not be a symlink/);
});
