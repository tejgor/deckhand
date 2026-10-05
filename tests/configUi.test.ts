import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {exec, launcher, terminalUi, waitFor} from './helpers.js';

// A stand-in agent (never a real one): stays alive and prints a marker.
const fakeAgent = `#!/usr/bin/env node
if (process.argv.includes('--help')) { console.log('PROMPT --settings --no-daemon resume'); process.exit(0); }
console.log('fake agent ready');
process.stdin.resume();
setInterval(() => {}, 10000);
`;

test('isolated terminal UI asks for trust only when repository config is about to run, edits the raw JSON (C → e) with raw terminal keys, persists search/filter and quits leaving the dev daemon alive', {timeout: 40000}, async t => {
	const home = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-ui-'));
	const bin = path.join(home, 'bin'); await fs.mkdir(bin);
	for (const agent of ['claude', 'pi', 'codex']) await fs.writeFile(path.join(bin, agent), fakeAgent, {mode: 0o755});
	const {ui, env} = terminalUi(t, {args: [launcher, '--sandbox'], cwd: home, home, env: {PATH: `${bin}${path.delimiter}${process.env.PATH}`}});
	const {screen, press} = ui;
	const readJson = async (name: string) => { try { return JSON.parse(await fs.readFile(path.join(home, name), 'utf8')); } catch { return {}; } };
	const file = path.join(home, 'sandbox', 'deckhand.json');
	// C → Settings opens on the This repo column; e opens the selected column's raw JSON (← selects Global).
	const openSettings = async () => { press('C'); await screen('Settings · sandbox'); await screen('Global (all repos)'); };
	const openConfig = async () => { await openSettings(); press('e'); await screen('Repository config'); };
	const replace = async (text: string, marker: string) => { press('\x01'); await screen('all selected'); press(text); await screen(marker); };
	await screen('DEV (isolated)'); await screen('● ready');

	// The sandbox's deckhand.json (defaultWorkspace "new") is untrusted. Trust gates running, not choosing: n opens the
	// picker at once with the repository's workspace preselected, and only creating a worktree that would run its
	// setupCommand reviews it (Esc returns to the form, s creates with global settings only).
	await fs.writeFile(file, JSON.stringify({...JSON.parse(await fs.readFile(file, 'utf8')), setupCommand: 'touch repo-setup-ran'}));
	press('n'); await screen('Choose an agent'); press('\r'); await screen('Workspace: new worktree');
	press('wt1'); await screen('Name: wt1');
	press('\r'); await screen('Review repository configuration'); await screen('setupCommand (touch repo-setup-ran)');
	press('\x1b'); await screen('Workspace: new worktree');
	press('\r'); await screen("s create without the repo's settings");
	press('s'); await screen('fake agent ready');
	assert.equal(Object.keys((await readJson('config.json')).trustedProjects ?? {}).length, 0);
	await fs.access(path.join(home, 'worktrees', 'wt1'));
	await assert.rejects(fs.access(path.join(home, 'worktrees', 'wt1', 'repo-setup-ran')));
	// e lists the untrusted repository action, marked; choosing it reviews first, Esc returns to the list.
	press('e'); await screen('· needs trust');
	press('\r'); await screen('About to run the repository action test: npm test'); await screen('s cancel');
	press('\x1b'); await screen('· needs trust');
	press('\x1b'); await screen('C settings');
	// T in Settings trusts it explicitly; n still opens the picker directly.
	press('C'); await screen('Settings ·'); press('T'); await screen('Review repository configuration'); press('\r'); await screen('trusted ✓'); press('\x1b'); await screen('C settings');
	await waitFor(() => readJson('config.json'), config => Object.keys(config.trustedProjects ?? {}).length === 1);
	press('n'); await screen('Choose an agent'); press('\r'); await screen('Workspace: new worktree'); press('\x1b'); await screen('Choose an agent'); press('\x1b'); await screen('C settings');

	// Global defaults are stored in config.json under "defaults", keeping the trust entry.
	await openSettings(); press('\x1b[D'); await screen('Built-in default'); press('e'); await screen('config.json "defaults"');
	press('\x01'); await screen('all selected'); press('{"devCommand":"printf global"}'); await screen('printf global');
	press('\x13'); await screen('Saved global defaults'); await screen('Settings · sandbox');
	press('\x1b'); await screen('C settings');
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
	press('\x13'); await screen('Saved deckhand.json · still trusted'); await screen('Settings · sandbox');
	assert.equal(await fs.readFile(file, 'utf8'), '{"devCommand":"d"}');
	press('\x1b'); await screen('C settings');

	// A dirty draft asks before discarding; the first Ctrl+C behaves like Esc here instead of quitting.
	await openConfig();
	await replace('{"defaultAgent":"pi"}', 'defaultAgent');
	press('\x03'); await screen('Discard unsaved configuration?');
	press('\x1b'); await screen('Repository config');
	press('\x1b'); await screen('Discard unsaved configuration?');
	press('\r'); await screen('Settings · sandbox');
	press('\x1b'); await screen('C settings');
	assert.equal(await fs.readFile(file, 'utf8'), '{"devCommand":"d"}');

	const preferences = async () => Object.values(await readJson('ui-state.json'))[0] as {sessionFilter?: string; sessionQuery?: string} | undefined;
	press('f'); await waitFor(preferences, state => state?.sessionFilter === 'archived');
	press('/'); await screen('Search: /');
	press('marker'); await waitFor(preferences, state => state?.sessionQuery === 'marker');
	press('\r'); await screen('C settings');
	// Ctrl+C again at the discard prompt quits (so the editor can never trap the UI).
	await openConfig();
	await replace('{"defaultAgent":"pi"}', 'defaultAgent');
	press('\x03'); await screen('Discard unsaved configuration?');
	press('\x03'); assert.equal(await ui.ended, 0);
	assert.equal(await fs.readFile(file, 'utf8'), '{"devCommand":"d"}');
	assert.match((await exec(process.execPath, [launcher, 'status'], {env})).stdout, /Isolated dev daemon running/);
});
