import fs from 'node:fs/promises';
import path from 'node:path';
import {randomUUID} from 'node:crypto';
import {getConfigDir, getConfigPath, getStatePath} from './paths.js';
import type {SessionRecord} from './types.js';

interface InkState {
	sessions: SessionRecord[];
}

export interface AppConfig {
	dev_command?: string;
	attach_scroll_sensitivity?: number;
}

const EMPTY_STATE: InkState = {sessions: []};

export async function ensureConfigDir(): Promise<void> {
	await fs.mkdir(getConfigDir(), {recursive: true});
}

export async function loadState(): Promise<InkState> {
	await ensureConfigDir();
	const statePath = getStatePath();
	try {
		const raw = await fs.readFile(statePath, 'utf8');
		if (!raw.trim()) {
			await saveState(EMPTY_STATE);
			return EMPTY_STATE;
		}
		const parsed = JSON.parse(raw) as Partial<InkState>;
		return {
			sessions: Array.isArray(parsed.sessions) ? parsed.sessions : [],
		};
	} catch (error) {
		const err = error as NodeJS.ErrnoException;
		if (err.code === 'ENOENT') {
			await saveState(EMPTY_STATE);
			return EMPTY_STATE;
		}
		if (error instanceof SyntaxError) {
			const backupPath = `${statePath}.corrupt-${Date.now()}`;
			await fs.rename(statePath, backupPath).catch(() => {});
			await saveState(EMPTY_STATE);
			return EMPTY_STATE;
		}
		throw error;
	}
}

export async function saveState(state: InkState): Promise<void> {
	await ensureConfigDir();
	const statePath = getStatePath();
	const temporaryPath = `${statePath}.tmp-${process.pid}-${Date.now()}-${randomUUID()}`;
	await fs.writeFile(temporaryPath, `${JSON.stringify(state, null, 2)}\n`, 'utf8');
	await fs.rename(temporaryPath, statePath);
}

export async function saveSessions(sessions: SessionRecord[]): Promise<void> {
	await saveState({sessions});
}

// Daemon-crash/restart recovery: live PTYs are owned by the daemon process.
// If a new daemon process starts, any persisted non-exited sessions no longer
// have live node-pty handles and must be shown as exited. Normal frontend quit
// should not reach this path because the daemon should remain alive.
export async function markAllNonExitedSessionsExited(): Promise<SessionRecord[]> {
	const state = await loadState();
	let changed = false;
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
		};
	});
	if (changed) {
		await saveSessions(sessions);
	}
	return sessions;
}

export function sortSessionsNewestFirst(sessions: SessionRecord[]): SessionRecord[] {
	return [...sessions].sort((a, b) => {
		if (a.status !== b.status) {
			return a.status === 'running' ? -1 : 1;
		}
		return a.createdAt.localeCompare(b.createdAt);
	});
}

export async function loadAppConfig(): Promise<AppConfig> {
	await ensureConfigDir();
	try {
		const raw = await fs.readFile(getConfigPath(), 'utf8');
		const parsed = JSON.parse(raw) as Partial<AppConfig>;
		return {
			dev_command: typeof parsed.dev_command === 'string' ? parsed.dev_command : undefined,
			attach_scroll_sensitivity: typeof parsed.attach_scroll_sensitivity === 'number' ? parsed.attach_scroll_sensitivity : undefined,
		};
	} catch (error) {
		const err = error as NodeJS.ErrnoException;
		if (err.code === 'ENOENT') {
			return {};
		}
		throw error;
	}
}

export function stateFileDisplayPath(): string {
	return path.relative(process.cwd(), getStatePath()) || getStatePath();
}
