import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {exec, launcher, terminalUi, waitFor} from './helpers.js';

test('isolated terminal UI reviews repository config inline on n, edits config with raw terminal keys, persists search/filter and quits leaving the dev daemon alive', {timeout: 30000}, async t => {
	const home = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-ui-'));
	const {ui, env} = terminalUi(t, {args: [launcher, '--sandbox'], cwd: home, home});
	const {screen, press} = ui;
	const readJson = async (name: string) => { try { return JSON.parse(await fs.readFile(path.join(home, name), 'utf8')); } catch { return {}; } };
	const file = path.join(home, 'sandbox', 'deckhand.json');
	const openConfig = async () => { press('C'); await screen('Edit configuration'); press('j'); await screen('› Repository'); press('\r'); await screen('Repository config'); };
	const replace = async (text: string, marker: string) => { press('\x01'); await screen('all selected'); press(text); await screen(marker); };
	await screen('DEV (isolated)'); await screen('● ready');

	// The sandbox's deckhand.json (defaultWorkspace "new") is untrusted: n reviews it inline before the program picker.
	const pickWorkspace = async (label: string) => { await screen('Choose an agent'); press('\r'); await screen(`Workspace: ${label}`); press('\x1b'); await screen('Choose an agent'); press('\x1b'); await screen('C config'); };
	press('n'); await screen('Review repository configuration'); await screen('s continue without it');
	press('s'); await pickWorkspace('no worktree'); // Skipped: global defaults only.
	assert.equal(Object.keys((await readJson('config.json')).trustedProjects ?? {}).length, 0);
	press('n'); await screen('enter trust & continue'); press('\r');
	await pickWorkspace('new worktree');
	await waitFor(() => readJson('config.json'), config => Object.keys(config.trustedProjects ?? {}).length === 1);
	press('n'); await pickWorkspace('new worktree'); // Trusted: no review this time.

	// Global defaults are stored in config.json under "defaults", keeping the trust entry.
	press('C'); await screen('Edit configuration'); press('\r'); await screen('Global defaults');
	press('\x01'); await screen('all selected'); press('{"devCommand":"printf global"}'); await screen('printf global');
	press('\x13'); await screen('Saved global defaults');
	const config = await readJson('config.json');
	assert.deepEqual(config.defaults, {devCommand: 'printf global'}); assert.equal(Object.keys(config.trustedProjects).length, 1);

	// Raw key decoding (useTerminalInput): DEL and Kitty Backspace delete left, forward Delete deletes right, key releases are ignored.
	await openConfig();
	const direction = '{"devCommand":"abcd"}';
	await replace(direction, 'abcd');
	press('\x1b[H' + '\x1b[C'.repeat(direction.indexOf('abcd') + 2)); await screen(direction);
	press('\x7f'); await screen('{"devCommand":"acd"}');
	press('\x1b[127;1u'); await screen('{"devCommand":"cd"}');
	press('\x1b[3~'); await screen('{"devCommand":"d"}');
	ui.write('\x1b[127;1:3u');
	press('\x13'); await screen('Saved deckhand.json');
	assert.equal(await fs.readFile(file, 'utf8'), '{"devCommand":"d"}');

	// A dirty draft asks before discarding; the first Ctrl+C behaves like Esc here instead of quitting.
	await openConfig();
	await replace('{"defaultAgent":"pi"}', 'defaultAgent');
	press('\x03'); await screen('Discard unsaved configuration?');
	press('\x1b'); await screen('Repository config');
	press('\x1b'); await screen('Discard unsaved configuration?');
	press('\r'); await screen('C config');
	assert.equal(await fs.readFile(file, 'utf8'), '{"devCommand":"d"}');

	const preferences = async () => Object.values(await readJson('ui-state.json'))[0] as {sessionFilter?: string; sessionQuery?: string} | undefined;
	press('f'); await waitFor(preferences, state => state?.sessionFilter === 'archived');
	press('/'); await screen('Search: /');
	press('marker'); await waitFor(preferences, state => state?.sessionQuery === 'marker');
	press('\r'); await screen('C config');
	// Ctrl+C again at the discard prompt quits (so the editor can never trap the UI).
	await openConfig();
	await replace('{"defaultAgent":"pi"}', 'defaultAgent');
	press('\x03'); await screen('Discard unsaved configuration?');
	press('\x03'); assert.equal(await ui.ended, 0);
	assert.equal(await fs.readFile(file, 'utf8'), '{"devCommand":"d"}');
	assert.match((await exec(process.execPath, [launcher, 'status'], {env})).stdout, /Isolated dev daemon running/);
});
