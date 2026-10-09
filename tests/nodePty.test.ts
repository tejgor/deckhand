import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {execFile} from 'node:child_process';
import {createRequire} from 'node:module';
import {promisify} from 'node:util';
import {signIfNeeded} from '../src/nodePty.js';

const exec = promisify(execFile);
const darwin = process.platform === 'darwin';

// The daemon and every worker check node-pty's binaries when they start, often at the same moment. Signing in place
// once let two of them corrupt spawn-helper (no agent or pane could start after that); signing now happens only when
// needed and on a copy renamed over the file.
test('concurrent starts sign node-pty\'s spawn-helper once, atomically, and leave a valid one alone', {skip: !darwin && 'macOS only'}, async t => {
	const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-pty-'));
	t.after(() => fs.rm(directory, {recursive: true, force: true}));
	const require = createRequire(import.meta.url);
	const source = path.join(path.dirname(require.resolve('node-pty/package.json')), 'prebuilds', `darwin-${process.arch}`, 'spawn-helper');
	const helper = path.join(directory, 'spawn-helper');
	await fs.copyFile(source, helper);
	await exec('codesign', ['--remove-signature', helper]);
	await assert.rejects(exec('codesign', ['--verify', helper]), 'starts unsigned');

	await Promise.all(Array.from({length: 8}, () => signIfNeeded(helper)));
	assert.match((await exec('file', [helper])).stdout, /Mach-O 64-bit executable/);
	await exec('codesign', ['--verify', helper]);
	assert.deepEqual((await fs.readdir(directory)).filter(name => name !== 'spawn-helper'), [], 'no temporary copies left');

	// Valid already: nothing is written.
	const before = (await fs.stat(helper)).mtimeMs;
	await new Promise(resolve => setTimeout(resolve, 20));
	await Promise.all(Array.from({length: 4}, () => signIfNeeded(helper)));
	assert.equal((await fs.stat(helper)).mtimeMs, before);
});
