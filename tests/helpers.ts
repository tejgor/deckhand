import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {execFile} from 'node:child_process';
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
export type TerminalUi = {screen(text: string): Promise<string>; press(key: string): void; write(data: string): void; ended: Promise<number>};
/** Runs Deckhand in a real PTY. Cleanup kills it, stops the dev daemon in `home`, then removes `home` and `remove`. */
export function terminalUi(t: TestContext, options: {args: string[]; cwd: string; home: string; env?: Record<string, string>; remove?: string[]}): {ui: TerminalUi; env: Record<string, string>} {
	const env = {...process.env, DECKHAND_DEV_HOME: options.home, TERM: 'xterm-256color', ...options.env} as Record<string, string>;
	for (const key of ['DECKHAND_SESSION_ID', 'DECKHAND_LAUNCH_ID', 'DECKHAND_HOOK_TOKEN']) delete env[key];
	const term = pty.spawn(process.execPath, options.args, {cwd: options.cwd, env, cols: 130, rows: 36});
	let output = '', exited = false;
	term.onData(data => { output = `${output}${data}`.slice(-200000); });
	const ended = new Promise<number>(resolve => term.onExit(event => { exited = true; resolve(event.exitCode); }));
	t.after(async () => {
		if (!exited) { try { term.kill(); } catch {} }
		await stopDevDaemon(options.home, env);
		for (const directory of [options.home, ...options.remove ?? []]) await fs.rm(directory, {recursive: true, force: true});
	});
	const ui: TerminalUi = {
		screen: text => waitFor(async () => output.replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, ''), value => value.includes(text)),
		// Clearing first means the next screen() only matches output produced after this key.
		press: key => { output = ''; term.write(key); },
		write: data => term.write(data),
		ended,
	};
	return {ui, env};
}
