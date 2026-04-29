import os from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';

export function getConfigDir(): string {
	return path.join(os.homedir(), '.deckhand');
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

export function getStatePath(): string {
	return path.join(getConfigDir(), 'state.json');
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

export function isDevRuntime(): boolean {
	return process.env.DECKHAND_DEV === '1';
}
