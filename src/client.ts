import fs from 'node:fs/promises';
import {closeSync, openSync} from 'node:fs';
import net from 'node:net';
import {spawn} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import {
	getCliEntryPath,
	getConfigDir,
	getDaemonLogPath,
	getDaemonPidPath,
	getProjectRoot,
	getSocketPath,
	isDevRuntime,
} from './paths.js';
import type {ClientRequest, CreateSessionInput, DevRecord, GitRecord, PreviewRecord, ServerMessage, SessionRecord, TerminalRecord, WorktreeInfoRecord, WorktreeMergeMode, WorktreeMergeResult} from './types.js';

function createConnection(): Promise<net.Socket> {
	const socketPath = getSocketPath();
	return new Promise((resolve, reject) => {
		const socket = net.createConnection(socketPath);
		socket.once('connect', () => resolve(socket));
		socket.once('error', reject);
	});
}

function writeMessage(socket: net.Socket, message: ClientRequest): void {
	socket.write(`${JSON.stringify(message)}\n`);
}

function attachJsonParser(socket: net.Socket, onMessage: (message: ServerMessage) => void): () => void {
	let buffer = '';
	const handleData = (chunk: Buffer | string) => {
		buffer += chunk.toString();
		while (true) {
			const newlineIndex = buffer.indexOf('\n');
			if (newlineIndex === -1) {
				break;
			}
			const line = buffer.slice(0, newlineIndex).trim();
			buffer = buffer.slice(newlineIndex + 1);
			if (!line) {
				continue;
			}
			onMessage(JSON.parse(line) as ServerMessage);
		}
	};
	socket.on('data', handleData);
	return () => {
		socket.off('data', handleData);
	};
}

export async function request<T = unknown>(message: Extract<ClientRequest, {requestId: string}>): Promise<T> {
	const socket = await createConnection();
	return new Promise<T>((resolve, reject) => {
		let done = false;
		const cleanup = attachJsonParser(socket, payload => {
			if (payload.type !== 'response' || payload.requestId !== message.requestId) {
				return;
			}
			done = true;
			socket.end();
			if (!payload.ok) {
				reject(new Error(payload.error || 'daemon request failed'));
				return;
			}
			resolve(payload.data as T);
		});
		socket.once('error', error => {
			if (!done) {
				reject(error);
			}
		});
		socket.once('close', () => {
			cleanup();
			if (!done) {
				reject(new Error('daemon connection closed before response'));
			}
		});
		writeMessage(socket, message);
	});
}

const PROTOCOL_VERSION = 12;

class ProtocolMismatchError extends Error {}

function describeError(error: unknown): string {
	return error instanceof Error ? error.message : String(error);
}

async function ping(): Promise<void> {
	const response = await request<{ok: true; version?: number}>({type: 'ping', requestId: randomUUID()});
	if (response.version !== PROTOCOL_VERSION) {
		throw new ProtocolMismatchError(
			`daemon protocol mismatch: expected v${PROTOCOL_VERSION}, got ${String(response.version)}`,
		);
	}
}

async function readDaemonPid(): Promise<number | undefined> {
	try {
		const raw = await fs.readFile(getDaemonPidPath(), 'utf8');
		const pid = Number.parseInt(raw.trim(), 10);
		return Number.isFinite(pid) && pid > 0 ? pid : undefined;
	} catch {
		return undefined;
	}
}

function isProcessAlive(pid: number | undefined): boolean {
	if (!pid) {
		return false;
	}
	try {
		process.kill(pid, 0);
		return true;
	} catch {
		return false;
	}
}

async function appendClientLog(message: string): Promise<void> {
	await fs.mkdir(getConfigDir(), {recursive: true});
	await fs.appendFile(getDaemonLogPath(), `[${new Date().toISOString()}] client ${message}\n`, 'utf8');
}

function spawnDaemon(): void {
	const cliPath = getCliEntryPath();
	const args = isDevRuntime() ? ['--import', 'tsx', cliPath, '--daemon'] : [cliPath, '--daemon'];
	const stdoutFd = openSync(getDaemonLogPath(), 'a');
	const stderrFd = openSync(getDaemonLogPath(), 'a');
	try {
		const child = spawn(process.execPath, args, {
			cwd: getProjectRoot(),
			env: {...process.env, DECKHAND_DAEMON: '1'},
			detached: true,
			stdio: ['ignore', stdoutFd, stderrFd],
		});
		child.unref();
	} finally {
		closeSync(stdoutFd);
		closeSync(stderrFd);
	}
}

async function waitForDaemon(deadlineMs: number): Promise<void> {
	const deadline = Date.now() + deadlineMs;
	let lastError: unknown;
	while (Date.now() < deadline) {
		try {
			await ping();
			return;
		} catch (error) {
			lastError = error;
			await new Promise(resolve => setTimeout(resolve, 150));
		}
	}
	throw new Error(`failed to start daemon: ${describeError(lastError)}`);
}

export async function ensureDaemonRunning(): Promise<void> {
	try {
		await ping();
		return;
	} catch (initialError) {
		const pid = await readDaemonPid();
		if (initialError instanceof ProtocolMismatchError && isProcessAlive(pid)) {
			const restartHint = `stop the old daemon first: kill ${pid} (or kill $(cat ${getDaemonPidPath()}))`;
			await appendClientLog(
				`refusing to replace live daemon pid ${pid} after protocol mismatch: ${initialError.message}; ${restartHint}`,
			);
			throw new Error(`${initialError.message}; ${restartHint}`);
		}

		if (isProcessAlive(pid)) {
			await appendClientLog(
				`ping failed while daemon pid ${pid} is still alive; retrying before replacement: ${describeError(initialError)}`,
			);
			try {
				await waitForDaemon(2000);
				return;
			} catch (retryError) {
				throw new Error(
					`daemon pid ${pid} is alive but did not respond; see ${getDaemonLogPath()}: ${describeError(retryError)}`,
				);
			}
		}

		await appendClientLog(`starting daemon after ping failure: ${describeError(initialError)}`);
		try {
			await fs.unlink(getSocketPath());
		} catch {
			// ignore stale socket cleanup failures
		}
		spawnDaemon();
	}

	await waitForDaemon(5000);
}

export async function listSessions(): Promise<SessionRecord[]> {
	await ensureDaemonRunning();
	return request<SessionRecord[]>({type: 'list', requestId: randomUUID()});
}

export async function createSession(input: CreateSessionInput): Promise<SessionRecord> {
	await ensureDaemonRunning();
	return request<SessionRecord>({type: 'create', requestId: randomUUID(), input});
}

export async function restartSession(sessionId: string, cols: number, rows: number): Promise<SessionRecord> {
	await ensureDaemonRunning();
	return request<SessionRecord>({type: 'restart', requestId: randomUUID(), sessionId, cols, rows});
}

export async function killSession(sessionId: string, deleteWorktree = false, deleteBranch = false, force = false): Promise<void> {
	await ensureDaemonRunning();
	await request({type: 'kill', requestId: randomUUID(), sessionId, deleteWorktree, deleteBranch, force});
}

export async function mergeWorktree(sessionId: string, mode: WorktreeMergeMode, targetCwd: string): Promise<WorktreeMergeResult> {
	await ensureDaemonRunning();
	return request<WorktreeMergeResult>({type: 'merge-worktree', requestId: randomUUID(), sessionId, mode, targetCwd});
}

export async function listWorktrees(cwd: string): Promise<WorktreeInfoRecord[]> {
	await ensureDaemonRunning();
	return request<WorktreeInfoRecord[]>({type: 'list-worktrees', requestId: randomUUID(), cwd});
}

export async function removeSession(sessionId: string): Promise<void> {
	await ensureDaemonRunning();
	await request({type: 'remove', requestId: randomUUID(), sessionId});
}

export async function openPersistentConnection(): Promise<net.Socket> {
	await ensureDaemonRunning();
	return createConnection();
}

interface LiveClientHandlers {
	onSessionUpdated?: (session: SessionRecord) => void;
	onSessionRemoved?: (sessionId: string) => void;
	onPreviewUpdated?: (preview: PreviewRecord) => void;
	onTerminalUpdated?: (terminal: TerminalRecord) => void;
	onGitUpdated?: (git: GitRecord) => void;
	onDevUpdated?: (dev: DevRecord) => void;
	onError?: (error: Error) => void;
	onClose?: () => void;
}

export class LiveClient {
	private readonly socket: net.Socket;
	private readonly handlers: LiveClientHandlers;
	private readonly pending = new Map<string, {resolve: (value: unknown) => void; reject: (error: Error) => void}>();
	private readonly stopParsing: () => void;
	private closed = false;

	constructor(socket: net.Socket, handlers: LiveClientHandlers = {}) {
		this.socket = socket;
		this.handlers = handlers;
		this.stopParsing = attachJsonParser(socket, message => {
			try {
				if (message.type === 'response') {
					const pending = this.pending.get(message.requestId);
					if (!pending) {
						return;
					}
					this.pending.delete(message.requestId);
					if (!message.ok) {
						pending.reject(new Error(message.error || 'daemon request failed'));
						return;
					}
					pending.resolve(message.data);
					return;
				}

				switch (message.type) {
					case 'session-updated':
						this.handlers.onSessionUpdated?.(message.session);
						return;
					case 'session-removed':
						this.handlers.onSessionRemoved?.(message.sessionId);
						return;
					case 'preview-updated':
						this.handlers.onPreviewUpdated?.(message.preview);
						return;
					case 'terminal-updated':
						this.handlers.onTerminalUpdated?.(message.terminal);
						return;
					case 'git-updated':
						this.handlers.onGitUpdated?.(message.git);
						return;
					case 'dev-updated':
						this.handlers.onDevUpdated?.(message.dev);
						return;
					default:
						return;
				}
			} catch (error) {
				this.handlers.onError?.(error instanceof Error ? error : new Error(String(error)));
			}
		});

		socket.on('error', error => {
			this.rejectAll(error instanceof Error ? error : new Error(String(error)));
			this.handlers.onError?.(error instanceof Error ? error : new Error(String(error)));
		});
		socket.on('close', () => {
			this.closed = true;
			this.stopParsing();
			this.rejectAll(new Error('daemon connection closed'));
			this.handlers.onClose?.();
		});
	}

	private rejectAll(error: Error): void {
		for (const pending of this.pending.values()) {
			pending.reject(error);
		}
		this.pending.clear();
	}

	request<T>(message: Extract<ClientRequest, {requestId: string}>): Promise<T> {
		if (this.closed || this.socket.destroyed) {
			return Promise.reject(new Error('daemon connection is closed'));
		}
		return new Promise<T>((resolve, reject) => {
			this.pending.set(message.requestId, {
				resolve: value => resolve(value as T),
				reject,
			});
			writeMessage(this.socket, message);
		});
	}

	subscribe(repoRoot: string): Promise<SessionRecord[]> {
		return this.request<SessionRecord[]>({type: 'subscribe', requestId: randomUUID(), repoRoot});
	}

	watchPreview(sessionId: string | undefined, cols: number, rows: number): Promise<PreviewRecord> {
		return this.request<PreviewRecord>({
			type: 'watch-preview',
			requestId: randomUUID(),
			sessionId,
			cols,
			rows,
		});
	}

	watchTerminal(sessionId: string | undefined, cols: number, rows: number): Promise<TerminalRecord> {
		return this.request<TerminalRecord>({
			type: 'watch-terminal',
			requestId: randomUUID(),
			sessionId,
			cols,
			rows,
		});
	}

	watchGit(sessionId: string | undefined, cols: number, rows: number): Promise<GitRecord> {
		return this.request<GitRecord>({
			type: 'watch-git',
			requestId: randomUUID(),
			sessionId,
			cols,
			rows,
		});
	}

	watchDev(sessionId: string | undefined, cols: number, rows: number): Promise<DevRecord> {
		return this.request<DevRecord>({
			type: 'watch-dev',
			requestId: randomUUID(),
			sessionId,
			cols,
			rows,
		});
	}

	startDev(sessionId: string, cols: number, rows: number): Promise<DevRecord> {
		return this.request<DevRecord>({type: 'start-dev', requestId: randomUUID(), sessionId, cols, rows});
	}

	stopDev(sessionId: string): Promise<void> {
		return this.request({type: 'stop-dev', requestId: randomUUID(), sessionId});
	}

	listSessions(): Promise<SessionRecord[]> {
		return this.request<SessionRecord[]>({type: 'list', requestId: randomUUID()});
	}

	createSession(input: CreateSessionInput): Promise<SessionRecord> {
		return this.request<SessionRecord>({type: 'create', requestId: randomUUID(), input});
	}

	listWorktrees(cwd: string): Promise<WorktreeInfoRecord[]> {
		return this.request<WorktreeInfoRecord[]>({type: 'list-worktrees', requestId: randomUUID(), cwd});
	}

	restartSession(sessionId: string, cols: number, rows: number): Promise<SessionRecord> {
		return this.request<SessionRecord>({type: 'restart', requestId: randomUUID(), sessionId, cols, rows});
	}

	killSession(sessionId: string, deleteWorktree = false, deleteBranch = false, force = false): Promise<void> {
		return this.request({type: 'kill', requestId: randomUUID(), sessionId, deleteWorktree, deleteBranch, force});
	}

	mergeWorktree(sessionId: string, mode: WorktreeMergeMode, targetCwd: string): Promise<WorktreeMergeResult> {
		return this.request<WorktreeMergeResult>({type: 'merge-worktree', requestId: randomUUID(), sessionId, mode, targetCwd});
	}

	removeSession(sessionId: string): Promise<void> {
		return this.request({type: 'remove', requestId: randomUUID(), sessionId});
	}

	close(): void {
		if (this.closed) {
			return;
		}
		this.closed = true;
		this.stopParsing();
		this.rejectAll(new Error('daemon connection closed'));
		if (!this.socket.destroyed) {
			this.socket.end();
		}
	}
}

export async function createLiveClient(handlers: LiveClientHandlers = {}): Promise<LiveClient> {
	const socket = await openPersistentConnection();
	return new LiveClient(socket, handlers);
}

export {attachJsonParser, writeMessage};
