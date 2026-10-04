import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import path from 'node:path';
import {loadProjectConfig, isProjectTrusted, trustProjectConfig} from '../src/projectConfig.js';
import {createWorktreeForSession} from '../src/git.js';
import {repo, tempDir, withEnv} from './helpers.js';
test('legacy creation hooks are inspectable, content-trusted and never run by default', async t => {
	const cwd = await repo(); t.after(() => fs.rm(cwd, {recursive: true, force: true}));
	const home = await tempDir(t, 'deckhand-hook-trust-'); withEnv(t, {DECKHAND_HOME: home});
	const file = path.join(cwd, '.claude', 'scripts', 'create-worktree.sh'); await fs.mkdir(path.dirname(file), {recursive: true});
	await fs.writeFile(file, '#!/bin/bash\ntouch "$CLAUDE_PROJECT_DIR/hook-ran"\nexit 1\n');
	const project = await loadProjectConfig(cwd); assert.match(project.creationHook?.content ?? '', /hook-ran/);
	const config = trustProjectConfig(project, {});
	const worktree = await createWorktreeForSession('untrusted', cwd); assert.equal(worktree.creator, 'fallback');
	assert.ok(worktree.path.startsWith(await fs.realpath(home))); await assert.rejects(fs.access(path.join(cwd, 'hook-ran')));
	await assert.rejects(createWorktreeForSession('approved', cwd, project.creationHook), /failed with exit code/);
	await fs.access(path.join(cwd, 'hook-ran')); await fs.rm(path.join(cwd, 'hook-ran'));
	// Swapping the file after review invalidates trust and cannot change what runs: only the reviewed bytes execute.
	await fs.writeFile(file, '#!/bin/bash\ntouch "$CLAUDE_PROJECT_DIR/swapped"\nexit 1\n');
	assert.equal(isProjectTrusted(await loadProjectConfig(cwd), config), false);
	await assert.rejects(createWorktreeForSession('verified', cwd, project.creationHook), /failed with exit code/);
	await fs.access(path.join(cwd, 'hook-ran')); await assert.rejects(fs.access(path.join(cwd, 'swapped'))); await fs.rm(path.join(cwd, 'hook-ran'));
});
