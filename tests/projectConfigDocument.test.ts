import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import path from 'node:path';
import {readProjectConfigDocument, saveProjectConfigDocument, STARTER_PROJECT_CONFIG} from '../src/projectConfigDocument.js';
import {isProjectTrusted, loadProjectConfig, trustProjectConfig} from '../src/projectConfig.js';
import {loadAppConfig, updateAppConfig} from '../src/storage.js';
import {git, repo, tempDir, withEnv} from './helpers.js';

test('opening a missing config is side-effect free; explicit save creates a valid starter', async t => {
	const cwd = await repo(); t.after(() => fs.rm(cwd, {recursive: true, force: true}));
	const document = await readProjectConfigDocument(cwd);
	assert.equal(document.exists, false); assert.equal(document.revision, null); assert.equal(document.raw, STARTER_PROJECT_CONFIG);
	await assert.rejects(fs.access(document.path));
	const saved = await saveProjectConfigDocument(cwd, document.raw, document.revision);
	assert.equal(saved.exists, true); assert.equal(saved.raw, await fs.readFile(saved.path, 'utf8'));
	assert.equal((await loadProjectConfig(cwd)).config.defaultWorkspace, 'new');
});
test('malformed JSON can be opened/repaired; invalid drafts never overwrite it', async t => {
	const cwd = await repo(); t.after(() => fs.rm(cwd, {recursive: true, force: true}));
	const file = path.join(cwd, 'deckhand.json'); await fs.writeFile(file, '{');
	const document = await readProjectConfigDocument(cwd); assert.equal(document.raw, '{');
	await assert.rejects(saveProjectConfigDocument(cwd, '{', document.revision));
	await assert.rejects(saveProjectConfigDocument(cwd, '{"unknown":true}', document.revision), /Unknown/);
	assert.equal(await fs.readFile(file, 'utf8'), '{');
	await saveProjectConfigDocument(cwd, '{"actions":{}}\n', document.revision);
	assert.deepEqual((await loadProjectConfig(cwd)).config.actions, {});
});
test('saving keeps the file permissions', async t => {
	const cwd = await repo(); t.after(() => fs.rm(cwd, {recursive: true, force: true}));
	const file = path.join(cwd, 'deckhand.json'); await fs.writeFile(file, '{}\n'); await fs.chmod(file, 0o660);
	const document = await readProjectConfigDocument(cwd);
	await saveProjectConfigDocument(cwd, '{"defaultAgent":"pi"}\n', document.revision);
	assert.equal((await fs.stat(file)).mode & 0o777, 0o660);
});
test('a stale draft cannot replace an externally changed, created or removed file', async t => {
	const cwd = await repo(); t.after(() => fs.rm(cwd, {recursive: true, force: true}));
	const missing = await readProjectConfigDocument(cwd);
	await fs.writeFile(missing.path, '{}');
	await assert.rejects(saveProjectConfigDocument(cwd, STARTER_PROJECT_CONFIG, missing.revision), /changed on disk/);
	const document = await readProjectConfigDocument(cwd);
	await fs.writeFile(document.path, '{"defaultAgent":"pi"}');
	await assert.rejects(saveProjectConfigDocument(cwd, '{}', document.revision), /changed on disk/);
	assert.equal(await fs.readFile(document.path, 'utf8'), '{"defaultAgent":"pi"}');
	await fs.rm(document.path);
	await assert.rejects(saveProjectConfigDocument(cwd, '{}', document.revision), /changed on disk/);
	await assert.rejects(fs.access(document.path));
	assert.deepEqual((await fs.readdir(cwd)).filter(name => name.startsWith('.deckhand.json.')), []);
});
test('simultaneous saves with one revision have exactly one winner', async t => {
	const cwd = await repo(); t.after(() => fs.rm(cwd, {recursive: true, force: true}));
	const document = await readProjectConfigDocument(cwd);
	const results = await Promise.allSettled([saveProjectConfigDocument(cwd, '{"defaultAgent":"pi"}', document.revision), saveProjectConfigDocument(cwd, '{"defaultAgent":"codex"}', document.revision)]);
	assert.equal(results.filter(result => result.status === 'fulfilled').length, 1);
	assert.equal(results.filter(result => result.status === 'rejected').length, 1);
	assert.deepEqual((await fs.readdir(cwd)).filter(name => name.startsWith('.deckhand.json.')), []);
});
test('from a linked worktree, the editor opens and saves the main checkout\'s file', async t => {
	const cwd = await repo(); t.after(() => fs.rm(cwd, {recursive: true, force: true}));
	const worktree = path.join(cwd, 'linked'); await git(cwd, 'worktree', 'add', '-b', 'linked', worktree);
	await fs.writeFile(path.join(worktree, 'deckhand.json'), '{"devCommand":"printf ignored"}');
	const document = await readProjectConfigDocument(worktree);
	assert.equal(document.path, path.join(await fs.realpath(cwd), 'deckhand.json')); assert.equal(document.exists, false);
	await saveProjectConfigDocument(worktree, '{"devCommand":"printf shared"}', document.revision);
	assert.equal((await loadProjectConfig(worktree)).config.devCommand, 'printf shared');
	assert.equal(await fs.readFile(path.join(worktree, 'deckhand.json'), 'utf8'), '{"devCommand":"printf ignored"}');
});
test('symlinked and oversized configs are rejected without modifying their targets', async t => {
	const cwd = await repo(), target = await repo(); t.after(async () => { await fs.rm(cwd, {recursive: true, force: true}); await fs.rm(target, {recursive: true, force: true}); });
	const file = path.join(cwd, 'deckhand.json'), outside = path.join(target, 'deckhand.json');
	await fs.writeFile(outside, '{}'); await fs.symlink(outside, file);
	await assert.rejects(readProjectConfigDocument(cwd), /symlink/);
	await assert.rejects(saveProjectConfigDocument(cwd, '{}', null), /symlink/);
	assert.equal(await fs.readFile(outside, 'utf8'), '{}');
	await fs.rm(file); await fs.writeFile(file, ' '.repeat(65537));
	await assert.rejects(readProjectConfigDocument(cwd), /64 KiB/);
	await assert.rejects(saveProjectConfigDocument(cwd, ' '.repeat(65537), null), /64 KiB/);
});

test('saves through Deckhand keep a trusted (or new) deckhand.json trusted; unreviewed bytes, outside edits and creation hooks still need review', async t => {
	const home = await tempDir(t, 'deckhand-save-trust-'); withEnv(t, {DECKHAND_HOME: home});
	const cwd = await repo(); t.after(() => fs.rm(cwd, {recursive: true, force: true}));
	const keepTrust = {keepTrust: true};
	const trusted = async () => { const user = await loadAppConfig(); return isProjectTrusted(await loadProjectConfig(cwd, user), user); };
	const save = async (raw: string) => saveProjectConfigDocument(cwd, raw, (await readProjectConfigDocument(cwd)).revision, keepTrust);
	const trustNow = async () => { const project = await loadProjectConfig(cwd, await loadAppConfig()); await updateAppConfig(config => trustProjectConfig(project, config)); };
	// No file and no hook: nothing to review, so the created file is trusted; saving over trusted bytes keeps it.
	const absent = await readProjectConfigDocument(cwd);
	let saved = await saveProjectConfigDocument(cwd, '{"devCommand":"one"}\n', absent.revision, keepTrust);
	assert.equal(saved.trust, 'created'); assert.equal(await trusted(), true);
	saved = await saveProjectConfigDocument(cwd, '{"devCommand":"two"}\n', saved.revision, keepTrust);
	assert.equal(saved.trust, 'kept'); assert.equal(await trusted(), true);
	// A stale revision is still rejected, and grants nothing.
	await assert.rejects(saveProjectConfigDocument(cwd, '{"devCommand":"stale"}\n', absent.revision, keepTrust), /changed on disk/);
	// An edit from outside Deckhand needs review again; a Deckhand save over those unreviewed bytes still does.
	await fs.writeFile(saved.path, '{"devCommand":"outside"}\n');
	assert.equal(await trusted(), false);
	assert.equal((await save('{"devCommand":"three"}\n')).trust, 'unreviewed'); assert.equal(await trusted(), false);
	// Without keepTrust (not a Deckhand edit) trust is untouched.
	await updateAppConfig(config => ({...config, trustedProjects: undefined}));
	assert.equal((await saveProjectConfigDocument(cwd, '{}\n', (await readProjectConfigDocument(cwd)).revision)).trust, undefined);

	// Creation hook. Trusted without the hook (hook: false), a save that enables the hook needs review.
	const hook = path.join(cwd, '.claude', 'scripts', 'create-worktree.sh'); await fs.mkdir(path.dirname(hook), {recursive: true}); await fs.writeFile(hook, '#!/bin/bash\necho one\n');
	await saveProjectConfigDocument(cwd, '{"worktree":{"hook":false}}\n', (await readProjectConfigDocument(cwd)).revision);
	await trustNow();
	assert.equal(await trusted(), true);
	assert.equal((await save('{"devCommand":"hooked"}\n')).trust, 'hook'); assert.equal(await trusted(), false);
	// Once the file and hook are trusted together, settings saves keep them trusted (the hook's bytes are unchanged).
	await trustNow();
	assert.equal((await save('{"devCommand":"four"}\n')).trust, 'kept'); assert.equal(await trusted(), true);
	// The hook edited since that trust: the bundle needs review, and a settings save never trusts the new hook.
	await fs.writeFile(hook, '#!/bin/bash\necho changed\n');
	assert.equal((await save('{"devCommand":"five"}\n')).trust, 'unreviewed'); assert.equal(await trusted(), false);
	// No file yet but an untrusted hook: creating the file does not trust the hook.
	const other = await repo(); t.after(() => fs.rm(other, {recursive: true, force: true}));
	await fs.mkdir(path.join(other, '.claude', 'scripts'), {recursive: true}); await fs.writeFile(path.join(other, '.claude', 'scripts', 'create-worktree.sh'), 'echo x\n');
	assert.equal((await saveProjectConfigDocument(other, '{}\n', null, keepTrust)).trust, 'hook');
	const user = await loadAppConfig();
	assert.equal(isProjectTrusted(await loadProjectConfig(other, user), user), false);
});
