import fs from 'node:fs/promises';
import {constants as fsConstants} from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {spawn} from 'node:child_process';
import {AGENTS} from './agents.js';
import {AGENT_PROGRAMS, parseVersion, sessionOutdated} from './agentVersions.js';
import type {AgentVersions, ProgramKey, SessionRecord} from './types.js';

// The daemon's agent version I/O: the installed version (`<binary> --version`, cached by the binary's realpath and
// mtime), the latest release (`npm view <package> version`, cached, refreshed at most every LATEST_MAX_AGE_MS in the
// background or on demand), and running an agent's own update command. Everything runs with the daemon's environment,
// the one agents are launched with. Failures make a version unknown; nothing here throws except update().

const VERSION_TIMEOUT_MS = 10_000;
const LATEST_TIMEOUT_MS = 10_000;
export const LATEST_MAX_AGE_MS = 6 * 60 * 60_000;
const UPDATE_TIMEOUT_MS = 5 * 60_000;
const UPDATE_OUTPUT_LIMIT = 8000;
const KILL_GRACE_MS = 2000;

/** The first executable `program` on `pathValue` (the daemon's PATH, as agent launches resolve it). */
export async function findOnPath(program: string, pathValue = process.env.PATH || ''): Promise<string | undefined> {
	for (const directory of pathValue.split(path.delimiter)) {
		if (!directory) continue;
		const candidate = path.join(directory, program);
		try {
			await fs.access(candidate, fsConstants.X_OK);
			if ((await fs.stat(candidate)).isFile()) return candidate;
		} catch {
			// Try the next PATH entry.
		}
	}
	return undefined;
}

export interface RunResult {exitCode: number | null; output: string; timedOut: boolean; error?: string}
// Runs are detached (their own process group), so they would outlive a daemon that exits mid-run; stop them on exit.
const runningGroups = new Set<number>();
process.on('exit', () => { for (const pid of runningGroups) try { process.kill(-pid, 'SIGKILL'); } catch {} });
/** Runs `file args` with stdin closed and the daemon's environment; combined output keeps its last `limit` characters. */
export function runBounded(file: string, args: string[], {timeout, limit, cwd = os.homedir()}: {timeout: number; limit: number; cwd?: string}): Promise<RunResult> {
	return new Promise(resolve => {
		let output = '', timedOut = false, settled = false;
		const finish = (result: RunResult) => { if (child.pid) runningGroups.delete(child.pid); if (!settled) { settled = true; clearTimeout(timer); resolve(result); } };
		// Its own process group, so a timeout also stops what it started (an updater's npm, a download).
		const child = spawn(file, args, {cwd, env: process.env, stdio: ['ignore', 'pipe', 'pipe'], detached: true});
		if (child.pid) runningGroups.add(child.pid);
		const kill = (signal: NodeJS.Signals) => { try { if (child.pid) process.kill(-child.pid, signal); } catch { try { child.kill(signal); } catch {} } };
		const append = (chunk: Buffer) => { output = `${output}${chunk.toString('utf8')}`.slice(-limit); };
		child.stdout?.on('data', append);
		child.stderr?.on('data', append);
		const timer = setTimeout(() => { timedOut = true; kill('SIGTERM'); setTimeout(() => kill('SIGKILL'), KILL_GRACE_MS).unref(); }, timeout);
		child.on('error', error => finish({exitCode: null, output, timedOut, error: error.message}));
		child.on('close', code => finish({exitCode: code, output, timedOut}));
	});
}

/** The latest release of `program`, or undefined/throws when unknown. */
export type LatestLookup = (program: ProgramKey, npmPackage: string) => Promise<string | undefined>;

const firstLine = (text: string) => text.split('\n').map(line => line.trim()).find(Boolean) ?? '';

/**
 * `npm view <package> version` (the `latest` dist-tag). DECKHAND_AGENT_LATEST (JSON, program → version) replaces the
 * lookup entirely, so tests never reach the network; a program missing from it is unknown.
 */
export const npmLatest: LatestLookup = async (program, npmPackage) => {
	const override = process.env.DECKHAND_AGENT_LATEST;
	if (override !== undefined) {
		const value = (JSON.parse(override || '{}') as Record<string, unknown>)[program];
		if (typeof value !== 'string') throw new Error('not listed in DECKHAND_AGENT_LATEST');
		return parseVersion(value);
	}
	const result = await runBounded('npm', ['view', npmPackage, 'version'], {timeout: LATEST_TIMEOUT_MS, limit: 4096});
	if (result.exitCode !== 0) throw new Error(result.timedOut ? 'npm view timed out' : result.error ?? (firstLine(result.output) || `npm view exited with ${result.exitCode}`));
	const version = parseVersion(result.output);
	if (!version) throw new Error('npm view printed no version');
	return version;
};

export interface InstalledVersion {path?: string; version?: string; error?: string}
export interface UpdateOutcome {ok: boolean; exitCode: number | null; command: string; output: string; before?: string; after?: string}

export class AgentVersionChecker {
	/** `--version` reads by realpath; reused while the binary's mtime and size stay the same. Only successes are kept. */
	private readonly binaries = new Map<string, {stamp: string; version: Promise<string | undefined>}>();
	private readonly latest = new Map<ProgramKey, {version?: string; checkedAt: number; error?: string}>();
	private latestRun?: Promise<void>;
	private readonly updates = new Map<ProgramKey, Promise<UpdateOutcome>>();

	constructor(private readonly lookup: LatestLookup = npmLatest) {}

	/** The version of the binary `command` names (a path, or a name looked up on PATH). */
	async versionOf(command: string): Promise<InstalledVersion> {
		const file = command.includes('/') ? command : await findOnPath(command);
		if (!file) return {error: 'not found on PATH'};
		let real: string, stamp: string;
		try {
			real = await fs.realpath(file);
			const stat = await fs.stat(real);
			stamp = `${stat.mtimeMs}:${stat.size}`;
		} catch {
			return {error: `${file} is missing`};
		}
		let cached = this.binaries.get(real);
		if (cached?.stamp !== stamp) {
			cached = {stamp, version: runBounded(file, ['--version'], {timeout: VERSION_TIMEOUT_MS, limit: 2000}).then(result => parseVersion(result.output))};
			this.binaries.set(real, cached);
		}
		const version = await cached.version;
		if (!version && this.binaries.get(real) === cached) this.binaries.delete(real);
		return version ? {path: file, version} : {path: file, error: `${path.basename(file)} --version printed no version`};
	}

	installed(program: ProgramKey): Promise<InstalledVersion> {
		return this.versionOf(program);
	}

	/** Looks every agent's latest release up again: always when `force`, else only once LATEST_MAX_AGE_MS passed. One run at a time. */
	refreshLatest(force: boolean): Promise<void> {
		if (this.latestRun) return this.latestRun;
		const now = Date.now();
		if (!force && AGENT_PROGRAMS.every(program => now - (this.latest.get(program)?.checkedAt ?? -Infinity) < LATEST_MAX_AGE_MS)) return Promise.resolve();
		this.latestRun = Promise.all(AGENT_PROGRAMS.map(async program => {
			try {
				this.latest.set(program, {version: await this.lookup(program, AGENTS[program].version.npmPackage), checkedAt: Date.now()});
			} catch (error) {
				this.latest.set(program, {checkedAt: Date.now(), error: `latest unknown: ${error instanceof Error ? error.message : String(error)}`});
			}
		})).then(() => undefined).finally(() => { this.latestRun = undefined; });
		return this.latestRun;
	}

	updating(program: ProgramKey): boolean {
		return this.updates.has(program);
	}

	/** Every agent's versions, with the running sessions of each and how many launched with an older version. */
	async snapshot(sessions: Iterable<SessionRecord>): Promise<AgentVersions> {
		const all = [...sessions];
		const entries = await Promise.all(AGENT_PROGRAMS.map(async program => {
			const installed = await this.installed(program);
			const latest = this.latest.get(program);
			const running = all.filter(session => session.program === program && session.status === 'running');
			return [program, {
				program,
				installed: installed.version,
				latest: latest?.version,
				path: installed.path,
				checkedAt: latest ? new Date(latest.checkedAt).toISOString() : undefined,
				error: installed.path && !installed.version ? installed.error : latest?.error,
				updating: this.updating(program) || undefined,
				running: running.length,
				outdated: running.filter(session => sessionOutdated(session, installed.version)).length,
			}] as const;
		}));
		return Object.fromEntries(entries) as AgentVersions;
	}

	/**
	 * Runs the agent's own update command (stdin closed, daemon environment, home directory, UPDATE_TIMEOUT_MS) and
	 * re-reads the installed version. A second request while one runs gets the same result. Throws when not installed.
	 */
	async update(program: ProgramKey, onStart?: () => void): Promise<UpdateOutcome> {
		const spec = AGENTS[program];
		const running = this.updates.get(program);
		if (running) return running;
		const before = await this.installed(program);
		const started = this.updates.get(program);
		if (started) return started;
		if (!before.path) throw new Error(`${spec.label} is not installed; run deckhand setup to install it`);
		const command = `${program} ${spec.version.updateArgs.join(' ')}`;
		const run = (async (): Promise<UpdateOutcome> => {
			const result = await runBounded(before.path!, spec.version.updateArgs, {timeout: UPDATE_TIMEOUT_MS, limit: UPDATE_OUTPUT_LIMIT});
			// The updater may have replaced the binary in place; read it again.
			this.binaries.clear();
			const after = await this.installed(program);
			const note = result.timedOut ? `\n(stopped after ${UPDATE_TIMEOUT_MS / 60_000} minutes)` : result.error ? `\n${result.error}` : '';
			return {ok: result.exitCode === 0, exitCode: result.exitCode, command, output: `${result.output.trimEnd()}${note}`.trim(), before: before.version, after: after.version};
		})().finally(() => { this.updates.delete(program); });
		this.updates.set(program, run);
		onStart?.();
		return run;
	}
}
