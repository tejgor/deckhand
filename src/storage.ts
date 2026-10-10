import fs from 'node:fs/promises';
import path from 'node:path';
import {randomUUID} from 'node:crypto';
import {getConfigDir, getConfigPath, getStatePath} from './paths.js';
import type {SessionRecord, WorktreeRecord} from './types.js';
import {migrateDoneMarkers, migrateWorktreeRecords} from './worktreeRecords.js';

export interface InkState {
	sessions: SessionRecord[];
	/** Merge/deletion markers per linked worktree incarnation, referenced by `session.worktree.id` (src/worktreeRecords.ts). */
	worktrees: WorktreeRecord[];
}

export interface AppConfig {
	dev_command?: string;
	attach_scroll_sensitivity?: number;
	/** Trusted project fingerprints per trust root, newest first (see trustProjectConfig). */
	trustedProjects?: Record<string, string[]>;
	/** Agent signals: unset means on for Claude only (see hooksEnabled); true adds Codex, false turns them off. */
	agent_hooks?: boolean;
	notifications?: boolean;
	/** Global project defaults (deckhand.json schema), kept as stored; validated where used (see globalDefaults). */
	defaults?: unknown;
}

const EMPTY_STATE: InkState = {sessions: [], worktrees: []};

let privateDir: string | undefined;
export async function ensureConfigDir(): Promise<void> {
	const dir = getConfigDir();
	await fs.mkdir(dir, {recursive: true, mode: 0o700});
	if (privateDir === dir) return;
	// Tighten directories created by older versions or a permissive umask, but never someone else's.
	const stat = await fs.stat(dir);
	if (stat.mode & 0o077 && stat.uid === process.getuid?.()) await fs.chmod(dir, 0o700);
	privateDir = dir;
}

export async function loadState(): Promise<InkState> {
	return (await readState()).state;
}

/** The persisted state; `migrated` when it was written before worktree records or done markers (migrated here). */
async function readState(): Promise<{state: InkState; migrated: boolean}> {
	await ensureConfigDir();
	const statePath = getStatePath();
	try {
		const raw = await fs.readFile(statePath, 'utf8');
		if (!raw.trim()) {
			await saveState(EMPTY_STATE);
			return {state: EMPTY_STATE, migrated: false};
		}
		const parsed = JSON.parse(raw) as Partial<InkState>;
		const lifted = migrateWorktreeRecords(Array.isArray(parsed.sessions) ? parsed.sessions : [], Array.isArray(parsed.worktrees) ? parsed.worktrees : []);
		// Main-checkout sessions' old `M` markers become done markers (after the linked worktrees' moved to their records).
		const done = migrateDoneMarkers(lifted.sessions);
		return {state: {sessions: done.sessions, worktrees: lifted.worktrees}, migrated: lifted.changed || done.changed};
	} catch (error) {
		const err = error as NodeJS.ErrnoException;
		if (err.code === 'ENOENT') {
			await saveState(EMPTY_STATE);
			return {state: EMPTY_STATE, migrated: false};
		}
		if (error instanceof SyntaxError) {
			const backupPath = `${statePath}.corrupt-${Date.now()}`;
			await fs.rename(statePath, backupPath).catch(() => {});
			await saveState(EMPTY_STATE);
			return {state: EMPTY_STATE, migrated: false};
		}
		throw error;
	}
}

export async function saveState(state: InkState): Promise<void> {
	await ensureConfigDir();
	const statePath = getStatePath();
	const temporaryPath = `${statePath}.tmp-${process.pid}-${Date.now()}-${randomUUID()}`;
	await fs.writeFile(temporaryPath, `${JSON.stringify(state, null, 2)}\n`, {encoding: 'utf8', mode: 0o600});
	await fs.rename(temporaryPath, statePath);
}

// Daemon-crash/restart recovery: live PTYs are owned by the daemon process.
// If a new daemon process starts, any persisted non-exited sessions no longer
// have live node-pty handles and must be shown as exited. Normal frontend quit
// should not reach this path because the daemon should remain alive.
export async function markAllNonExitedSessionsExited(): Promise<InkState> {
	const {state, migrated} = await readState();
	// A migrated state is written back at once.
	let changed = migrated;
	const now = new Date().toISOString();
	const sessions = state.sessions.map(session => {
		if (session.status === 'exited') {
			if (session.devRunning) {
				changed = true;
				return {...session, devRunning: false};
			}
			return session;
		}
		changed = true;
		return {
			...session,
			status: 'exited' as const,
			updatedAt: now,
			pid: undefined,
			exitCode: session.exitCode ?? null,
			exitSignal: session.exitSignal ?? null,
			devRunning: false,
			exitReason: 'interrupted' as const,
			attention: {state: 'unknown' as const, event: 'DaemonRestart', at: now},
			...(session.setup?.state === 'running' ? {setup: {...session.setup, state: 'failed' as const, output: `${session.setup.output}\nInterrupted by daemon restart`}} : {}),
		};
	});
	const recovered = {...state, sessions};
	if (changed) {
		await saveState(recovered);
	}
	return recovered;
}

export function sortSessionsNewestFirst(sessions: SessionRecord[]): SessionRecord[] {
	return [...sessions].sort((a, b) => {
		if (a.status !== b.status) {
			return a.status === 'running' ? -1 : 1;
		}
		return a.createdAt.localeCompare(b.createdAt);
	});
}

type RawConfig = Record<string, unknown>;
function normalizeAppConfig(parsed: RawConfig): AppConfig {
	const trusted = parsed.trustedProjects && typeof parsed.trustedProjects === 'object' && !Array.isArray(parsed.trustedProjects) ? parsed.trustedProjects as Record<string, unknown> : {};
	return {
		dev_command: typeof parsed.dev_command === 'string' ? parsed.dev_command : undefined,
		attach_scroll_sensitivity: typeof parsed.attach_scroll_sensitivity === 'number' ? parsed.attach_scroll_sensitivity : undefined,
		// Older versions stored a single fingerprint string per root.
		trustedProjects: Object.fromEntries(Object.entries(trusted).flatMap(([root, value]) => {
			const list = typeof value === 'string' ? [value] : Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : [];
			return list.length ? [[root, list]] : [];
		})),
		agent_hooks: typeof parsed.agent_hooks === 'boolean' ? parsed.agent_hooks : undefined,
		notifications: parsed.notifications === true,
		defaults: parsed.defaults,
	};
}
async function readRawConfig(): Promise<RawConfig> {
	try {
		const parsed: unknown = JSON.parse(await fs.readFile(getConfigPath(), 'utf8'));
		if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) throw new Error(`${getConfigPath()} must contain a JSON object`);
		return parsed as RawConfig;
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code === 'ENOENT') return {};
		throw error;
	}
}

export async function loadAppConfig(): Promise<AppConfig> {
	await ensureConfigDir();
	const raw = await readRawConfig();
	return Object.keys(raw).length ? normalizeAppConfig(raw) : {};
}

async function writeRawConfig(config: RawConfig): Promise<void> {
	const configPath = getConfigPath();
	const temporaryPath = `${configPath}.tmp-${process.pid}-${Date.now()}-${randomUUID()}`;
	try {
		await fs.writeFile(temporaryPath, `${JSON.stringify(config, null, 2)}\n`, {encoding: 'utf8', mode: 0o600});
		await fs.rename(temporaryPath, configPath);
	} finally { await fs.rm(temporaryPath, {force: true}); }
}

const LOCK_STALE_MS = 5000, LOCK_TIMEOUT_MS = 10_000;
// The UI and daemon processes both write config.json; an exclusive lockfile serializes their read-modify-write.
async function withConfigLock<T>(operation: () => Promise<T>): Promise<T> {
	const lock = `${getConfigPath()}.lock`;
	const token = `${process.pid}:${randomUUID()}`;
	const deadline = Date.now() + LOCK_TIMEOUT_MS;
	const readToken = (file: string) => fs.readFile(file, 'utf8').catch(() => undefined);
	for (let attempt = 0; ; attempt++) {
		try {
			const handle = await fs.open(lock, 'wx', 0o600);
			try { await handle.writeFile(token); } finally { await handle.close(); }
			break;
		} catch (error) {
			if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error;
			const stat = await fs.stat(lock).catch(() => undefined);
			if (stat && Date.now() - stat.mtimeMs > LOCK_STALE_MS) {
				// A crashed writer left it behind. Checking and then deleting by path could delete a lock another
				// writer created in between, so move it aside atomically and only discard it if it is still stale.
				const aside = `${lock}.${randomUUID()}.stale`;
				if (await fs.rename(lock, aside).then(() => true, () => false)) {
					const moved = await fs.stat(aside).catch(() => undefined);
					// A live lock we raced with is put back (link fails rather than overwrite a newer lock).
					if (moved && Date.now() - moved.mtimeMs <= LOCK_STALE_MS) await fs.link(aside, lock).catch(() => {});
					await fs.rm(aside, {force: true});
				}
				continue;
			}
			if (Date.now() > deadline) throw new Error(`Timed out waiting for ${lock}`);
			await new Promise(resolve => setTimeout(resolve, Math.min(5 * 2 ** attempt, 100) + Math.random() * 10));
		}
	}
	try { return await operation(); }
	// Only release our own lock: if it was ever judged stale and replaced, the new holder's lock stays.
	finally { if (await readToken(lock) === token) await fs.rm(lock, {force: true}); }
}

let configQueue: Promise<unknown> = Promise.resolve();
/**
 * Read-modify-write of config.json, serialized in-process and across processes. The patch/updater sees the
 * normalized view; only the keys it changes are written back, so unknown keys and absent defaults are preserved.
 */
export function updateAppConfig(patchOrUpdater: AppConfig | ((current: AppConfig) => AppConfig)): Promise<AppConfig> {
	const operation = configQueue.then(async () => {
		await ensureConfigDir();
		return withConfigLock(async () => {
			const raw = await readRawConfig();
			const current = normalizeAppConfig(raw);
			const next = typeof patchOrUpdater === 'function' ? patchOrUpdater(current) : {...current, ...patchOrUpdater};
			const written: RawConfig = {...raw};
			const known = current as Record<string, unknown>, updated = next as Record<string, unknown>;
			for (const key of new Set([...Object.keys(known), ...Object.keys(updated)])) {
				if (JSON.stringify(known[key]) === JSON.stringify(updated[key])) continue;
				if (updated[key] === undefined) delete written[key];
				else written[key] = updated[key];
			}
			await writeRawConfig(written);
			return normalizeAppConfig(written);
		});
	});
	configQueue = operation.catch(() => {});
	return operation;
}

export function stateFileDisplayPath(): string {
	return path.relative(process.cwd(), getStatePath()) || getStatePath();
}
