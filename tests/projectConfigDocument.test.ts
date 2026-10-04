import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import path from 'node:path';
import {readProjectConfigDocument, saveProjectConfigDocument, STARTER_PROJECT_CONFIG} from '../src/projectConfigDocument.js';
import {loadProjectConfig} from '../src/projectConfig.js';
import {git, repo} from './helpers.js';

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
