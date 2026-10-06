import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';

export function getConfigDir(): string {
	return process.env.DECKHAND_HOME ? path.resolve(process.env.DECKHAND_HOME) : path.join(os.homedir(), '.deckhand');
}

/** Compares a reported Deckhand home with ours by realpath (e.g. macOS /var vs /private/var). */
export function isSameConfigDir(other: string): boolean {
	const canonical = (dir: string) => { try { return fs.realpathSync(dir); } catch { return path.resolve(dir); } };
	return canonical(other) === canonical(getConfigDir());
}

export function getSocketPath(): string {
	return path.join(getConfigDir(), 'daemon.sock');
}

export function getDaemonPidPath(): string {
	return path.join(getConfigDir(), 'daemon.pid');
}

export function getDaemonLogPath(): string {
	return path.join(getConfigDir(), 'daemon.log');
}

export function getWorkerDir(): string {
	return path.join(getConfigDir(), 'workers');
}

export function getWorkerPidPath(sessionId: string): string {
	return path.join(getWorkerDir(), `${sessionId}.pid`);
}

export function getWorkerLogPath(sessionId: string): string {
	return path.join(getWorkerDir(), `${sessionId}.log`);
}

export function getAgentSessionDir(program: string): string {
	return path.join(getConfigDir(), 'agent-sessions', program);
}

export function getStatePath(): string {
	return path.join(getConfigDir(), 'state.json');
}

/** Notes files: `sessions/<session id>.md`, `worktrees/<worktree record id>.md`, `repos/<hash of the main checkout root>.md`. */
export function getNotesDir(): string {
	return path.join(getConfigDir(), 'notes');
}

export function getConfigPath(): string {
	return path.join(getConfigDir(), 'config.json');
}

export function getProjectRoot(): string {
	const thisFile = fileURLToPath(import.meta.url);
	return path.resolve(path.dirname(thisFile), '..');
}

export function getCliEntryPath(): string {
	const root = getProjectRoot();
	if (process.env.DECKHAND_DEV === '1') {
		return path.join(root, 'src', 'cli.ts');
	}
	return path.join(root, 'dist', 'cli.js');
}

export function getTsxLoaderPath(): string {
	return import.meta.resolve('tsx');
}

export function isDevRuntime(): boolean {
	return process.env.DECKHAND_DEV === '1';
}
