import fs from 'node:fs/promises';
import {mkdtempSync} from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {execFile, type ChildProcess} from 'node:child_process';
import {promisify} from 'node:util';
import {fileURLToPath} from 'node:url';
import type {TestContext} from 'node:test';
import pty from 'node-pty';
export const exec = promisify(execFile);
export const cli = fileURLToPath(new URL('../dist/cli.js', import.meta.url));
export const launcher = fileURLToPath(new URL('../scripts/deckhand-dev.mjs', import.meta.url));
// Fixture Git ignores the user's global/system config (signing, hooks, default branch, ...).
const gitEnv = {...process.env, GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1'};
export async function git(cwd: string, ...args: string[]): Promise<string> {
	return (await exec('git', ['-C', cwd, ...args], {env: gitEnv, maxBuffer: 1024 * 1024})).stdout.trim();
}
export async function repo(): Promise<string> {
	const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-test-'));
	await git(directory, 'init', '-b', 'main');
	await git(directory, 'config', 'user.name', 'Deckhand Test');
	await git(directory, 'config', 'user.email', 'deckhand@example.invalid');
	await git(directory, 'config', 'commit.gpgsign', 'false');
	await git(directory, 'config', 'core.hooksPath', path.join(directory, '.git', 'no-hooks'));
	await fs.writeFile(path.join(directory, 'file.txt'), 'first\n');
	await git(directory, 'add', '.'); await git(directory, 'commit', '-m', 'initial');
	return directory;
}
export async function tempDir(t: TestContext, prefix: string): Promise<string> {
	const directory = await fs.mkdtemp(path.join(os.tmpdir(), prefix));
	t.after(() => fs.rm(directory, {recursive: true, force: true}));
	return directory;
}
/** Sets process.env entries (undefined deletes) for the rest of the test and restores them afterwards. */
export function withEnv(t: TestContext, values: Record<string, string | undefined>): void {
	const previous = Object.fromEntries(Object.keys(values).map(key => [key, process.env[key]]));
	const apply = (next: Record<string, string | undefined>) => { for (const [key, value] of Object.entries(next)) { if (value === undefined) delete process.env[key]; else process.env[key] = value; } };
	apply(values); t.after(() => apply(previous));
}
export async function waitFor<T>(read: () => Promise<T>, matches: (value: T) => boolean, timeout = 8000): Promise<T> {
	const deadline = Date.now() + timeout;
	let last: T | undefined;
	while (Date.now() < deadline) { last = await read(); if (matches(last)) return last; await new Promise(resolve => setTimeout(resolve, 40)); }
	throw new Error(`Condition timed out: ${JSON.stringify(last)}`);
}
export const isAlive = (pid: number) => { try { process.kill(pid, 0); return true; } catch { return false; } };
/** Stops the isolated dev daemon of `home` and waits for its process to exit, so it cannot rewrite files after removal. */
export async function stopDevDaemon(home: string, env: NodeJS.ProcessEnv): Promise<void> {
	const pid = Number.parseInt(await fs.readFile(path.join(home, 'daemon.pid'), 'utf8').catch(() => ''), 10);
	await exec(process.execPath, [launcher, 'stop'], {env}).catch(() => {});
	if (!Number.isFinite(pid) || pid <= 0) return;
	try { await waitFor(async () => isAlive(pid), alive => !alive, 5000); }
	catch { try { process.kill(pid, 'SIGKILL'); } catch {} }
}
/**
 * Ceiling for a real-PTY UI wait. The waits are condition-based (they return as soon as the screen matches), so this
 * only matters under load: `npm test` runs the test files in parallel and the daemon test keeps several agents busy.
 */
export const UI_WAIT_MS = 30_000;
/** Test timeout for a real-PTY UI run: room for a few slow waits without hiding a hang. */
export const UI_TEST_TIMEOUT_MS = 120_000;
export type TerminalUi = {screen(text: string): Promise<string>; press(key: string): void; write(data: string): void; ended: Promise<number>};
/** Runs Deckhand in a real PTY. Cleanup kills it, stops the dev daemon in `home`, then removes `home` and `remove`. */
export function terminalUi(t: TestContext, options: {args: string[]; cwd: string; home: string; env?: Record<string, string>; remove?: string[]}): {ui: TerminalUi; env: Record<string, string>} {
	// A plain shell and a private HOME (beside, never inside, the dev home), so commands the UI runs (Dev, actions)
	// never load the user's shell profile.
	const userHome = mkdtempSync(path.join(os.tmpdir(), 'deckhand-ui-home-'));
	// DECKHAND_AGENT_LATEST: the daemon never asks npm for the latest agent releases in tests.
	const env = {...process.env, DECKHAND_DEV_HOME: options.home, HOME: userHome, SHELL: '/bin/sh', TERM: 'xterm-256color', DECKHAND_AGENT_LATEST: '{}', ...options.env} as Record<string, string>;
	for (const key of ['DECKHAND_SESSION_ID', 'DECKHAND_LAUNCH_ID', 'DECKHAND_HOOK_TOKEN']) delete env[key];
	const term = pty.spawn(process.execPath, options.args, {cwd: options.cwd, env, cols: 130, rows: 36});
	let output = '', exited = false;
	term.onData(data => { output = `${output}${data}`.slice(-200000); });
	const ended = new Promise<number>(resolve => term.onExit(event => { exited = true; resolve(event.exitCode); }));
	t.after(async () => {
		if (!exited) { try { term.kill(); } catch {} }
		await stopDevDaemon(options.home, env);
		for (const directory of [options.home, userHome, ...options.remove ?? []]) await fs.rm(directory, {recursive: true, force: true});
	});
	const ui: TerminalUi = {
		screen: text => waitFor(async () => output.replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, ''), value => value.includes(text), UI_WAIT_MS),
		// Clearing first means the next screen() only matches output produced after this key.
		press: key => { output = ''; term.write(key); },
		write: data => term.write(data),
		ended,
	};
	return {ui, env};
}
/**
 * A fake claude/pi/codex: records its argv and input under the state directory and reports SessionStart through
 * `deckhand hook`. `--version` prints FAKE_VERSION in the agent's own format; its update command (`claude update`,
 * `codex update`, `pi update --self`) rewrites this file with the next patch version, like a real updater replacing
 * the binary, and logs the call to updates.log.
 */
export const fakeAgent = `#!/usr/bin/env node
const fs = require('fs');
const cp = require('child_process');
const path = require('path');
const FAKE_VERSION = '1.0.0';
if (process.argv.includes('--help')) { console.log('PROMPT --settings --no-daemon resume'); process.exit(0); }
const provider = path.basename(process.argv[1]);
if (process.argv[2] === '--version') { console.log(provider === 'claude' ? FAKE_VERSION + ' (Claude Code)' : provider === 'codex' ? 'codex-cli ' + FAKE_VERSION : FAKE_VERSION); process.exit(0); }
if (process.argv[2] === 'update') {
	const [major, minor, patch] = FAKE_VERSION.split('.').map(Number), next = major + '.' + minor + '.' + (patch + 1);
	const self = fs.readFileSync(process.argv[1], 'utf8');
	fs.writeFileSync(process.argv[1], self.replace("const FAKE_VERSION = '" + FAKE_VERSION + "'", "const FAKE_VERSION = '" + next + "'"));
	fs.appendFileSync(path.join(process.env.DECKHAND_HOME, 'updates.log'), JSON.stringify({provider, args: process.argv.slice(2), stdin: fs.fstatSync(0).isFIFO() ? 'pipe' : fs.fstatSync(0).isCharacterDevice() ? 'tty-or-null' : 'other'}) + '\\n');
	console.log('Updating ' + provider + '...'); console.log('Updated ' + FAKE_VERSION + ' -> ' + next);
	process.exit(0);
}
// \`codex fork <parent>\` reports its own ID, but first and last also the parent's (a child must never adopt it).
const forkOf = provider === 'codex' && process.argv[2] === 'fork' ? process.argv[3] : undefined;
const id = forkOf ? 'fixture-child-codex' : 'fixture-native-' + provider;
fs.writeFileSync(path.join(process.env.DECKHAND_HOME, 'trace-' + process.env.DECKHAND_SESSION_ID + '.json'), JSON.stringify({args: process.argv.slice(2), token: process.env.DECKHAND_HOOK_TOKEN, launchId: process.env.DECKHAND_LAUNCH_ID, pid: process.pid}));
function hook(event, sessionId = id) { return new Promise(resolve => { const child = cp.spawn(process.execPath, [process.env.TEST_CLI, 'hook'], {stdio: ['pipe','ignore','ignore']}); child.stdin.end(JSON.stringify({hook_event_name: event, session_id: sessionId})); child.on('close', resolve); }); }
const startHooks = () => forkOf ? hook('SessionStart', forkOf).then(() => hook('SessionStart')).then(() => hook('SessionStart', forkOf)) : hook('SessionStart');
startHooks().then(() => { fs.writeFileSync(path.join(process.env.DECKHAND_HOME, 'ready-' + process.env.DECKHAND_LAUNCH_ID), ''); console.log('ready'); });
if (process.stdin.isTTY) process.stdin.setRawMode(true);
process.stdin.resume();
process.stdin.on('data', data => fs.appendFileSync(path.join(process.env.DECKHAND_HOME, 'input-' + process.env.DECKHAND_SESSION_ID), data));
setInterval(() => {}, 10000);
`;
// The fake agent without lifecycle hooks: Deckhand never learns a Codex ID from it.
export const withoutHooks = fakeAgent.replace('startHooks().then(', 'Promise.resolve().then(');
export async function stop(child: ChildProcess): Promise<void> {
	if (child.exitCode !== null || child.signalCode !== null) return;
	const ended = new Promise<void>(resolve => child.once('exit', () => resolve()));
	child.kill('SIGTERM'); await ended;
}
