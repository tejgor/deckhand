import fs from 'node:fs/promises';
import {constants as fsConstants} from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import net from 'node:net';
import {execFile, fork, type ChildProcess} from 'node:child_process';
import {promisify} from 'node:util';
import {randomUUID} from 'node:crypto';
import pty, {type IPty} from 'node-pty';
import {getCliEntryPath, getDaemonLogPath, getDaemonPidPath, getSocketPath, getWorkerDir, getWorkerLogPath, getWorkerPidPath} from './paths.js';
import {createWorktreeForSession, deleteLocalBranch, findRepoRoot, listWorktrees, mergeWorktreeIntoCurrent, removeWorktree, sanitizeWorktreeName} from './git.js';
import {ensureNodePtyReady} from './nodePty.js';
import {ensureConfigDir, loadAppConfig, markAllNonExitedSessionsExited, saveSessions, sortSessionsNewestFirst} from './storage.js';
import {TerminalPreview} from './terminalPreview.js';
import type {AgentActivityStatus, AgentSessionRef, AttachTarget, ClientRequest, CreateSessionInput, DevRecord, GitRecord, PreviewRecord, ServerMessage, ServerResponse, SessionRecord, TerminalRecord} from './types.js';

const execFileAsync = promisify(execFile);
const SCROLLBACK_LIMIT = 200_000;
const DEFAULT_PREVIEW_COLS = 80;
const DEFAULT_PREVIEW_ROWS = 24;
const PREVIEW_BROADCAST_DELAY_MS = 75;
const ACTIVITY_EVALUATION_DELAY_MS = 150;
const ACTIVITY_WINDOW_MS = 3000;
const IDLE_AFTER_MS = 5000;
const ACTIVE_MIN_CHANGED_CHARS = 1;
const RESIZE_ACTIVITY_SUPPRESSION_MS = 750;
const PROTOCOL_VERSION = 15;
const WORKER_REQUEST_TIMEOUT_MS = 10_000;

interface RuntimeSession {
	term: IPty;
	scrollback: string;
	preview: TerminalPreview;
	attachedSocket?: net.Socket;
	previewBroadcastTimer?: NodeJS.Timeout;
	activityEvaluationTimer?: NodeJS.Timeout;
	activityIdleTimer?: NodeJS.Timeout;
	suppressActivityUntil?: number;
	lastPreviewSnapshot: string;
	previewChangeEvents: Array<{at: number; changedChars: number}>;
	deleteWorktreeOnExit?: boolean;
	deleteBranchOnExit?: boolean;
}

interface RuntimeTerminal {
	term: IPty;
	scrollback: string;
	preview: TerminalPreview;
	attachedSocket?: net.Socket;
	broadcastTimer?: NodeJS.Timeout;
	cwd: string;
	exited: boolean;
	exitCode?: number | null;
	exitSignal?: number | null;
}

interface ClientSubscription {
	repoRoot?: string;
	watchedPreviewSessionId?: string;
	watchedTerminalSessionId?: string;
	watchedGitSessionId?: string;
	watchedDevSessionId?: string;
	previewCols: number;
	previewRows: number;
	terminalCols: number;
	terminalRows: number;
	gitCols: number;
	gitRows: number;
	devCols: number;
	devRows: number;
}

interface WorkerRuntime {
	process: ChildProcess;
	pending: Map<string, {resolve: (value: unknown) => void; reject: (error: Error) => void; timer: NodeJS.Timeout}>;
	attached: Partial<Record<AttachTarget, net.Socket>>;
	deleteWorktreeOnExit?: boolean;
	deleteBranchOnExit?: boolean;
	exited?: boolean;
}

type WorkerEvent =
	| {type: 'response'; requestId: string; ok: true; data?: unknown}
	| {type: 'response'; requestId: string; ok: false; error: string}
	| {type: 'running'; pid: number}
	| {type: 'exit'; exitCode: number | null; exitSignal: number | null; lastPreview: string}
	| {type: 'agent-status'; agentStatus: AgentActivityStatus}
	| {type: 'preview-updated'; preview: PreviewRecord}
	| {type: 'terminal-updated'; terminal: TerminalRecord}
	| {type: 'git-updated'; git: GitRecord}
	| {type: 'dev-updated'; dev: DevRecord}
	| {type: 'output'; target: AttachTarget; data: string};

function sendMessage(socket: net.Socket, message: ServerMessage): void {
	if (!socket.destroyed) {
		socket.write(`${JSON.stringify(message)}\n`);
	}
}

function response<T>(requestId: string, data: T): ServerResponse<T> {
	return {type: 'response', requestId, ok: true, data};
}

function failure(requestId: string, error: unknown): ServerResponse {
	return {
		type: 'response',
		requestId,
		ok: false,
		error: error instanceof Error ? error.message : String(error),
	};
}

const programCommandCache = new Map<SessionRecord['program'], string>();

async function resolveProgramCommand(program: SessionRecord['program']): Promise<string> {
	const cached = programCommandCache.get(program);
	if (cached) {
		return cached;
	}

	if (program.includes('/')) {
		programCommandCache.set(program, program);
		return program;
	}

	const pathValue = process.env.PATH || '';
	for (const directory of pathValue.split(path.delimiter)) {
		if (!directory) {
			continue;
		}
		const candidate = path.join(directory, program);
		try {
			await fs.access(candidate, fsConstants.X_OK);
			programCommandCache.set(program, candidate);
			return candidate;
		} catch {
			// Try the next PATH entry.
		}
	}

	programCommandCache.set(program, program);
	return program;
}

function buildDeckhandAgentName(title: string, sessionId: string): string {
	const safeTitle = sanitizeWorktreeName(title).replace(/\//g, '-').slice(0, 40).replace(/^[-_]+|[-_]+$/g, '') || 'session';
	return `dh-${safeTitle}-${sessionId.slice(0, 8)}`;
}

function piSessionPath(cwd: string, name: string, sessionId: string): string {
	const encodedCwd = cwd.replace(/\//g, '-');
	const projectDir = `-${encodedCwd}-`;
	const timestamp = new Date().toISOString().replace(/[:.]/g, '-');
	return path.join(os.homedir(), '.pi', 'agent', 'sessions', projectDir, `${timestamp}_${name}_${sessionId}.jsonl`);
}

function buildAgentSessionRef(program: SessionRecord['program'], title: string, sessionId: string, cwd: string): AgentSessionRef | undefined {
	const name = buildDeckhandAgentName(title, sessionId);
	if (program === 'claude') {
		return {provider: program, kind: 'name', value: name};
	}
	if (program === 'pi') {
		return {provider: program, kind: 'path', value: piSessionPath(cwd, name, sessionId)};
	}
	return undefined;
}

function buildAgentArgs(session: Pick<SessionRecord, 'program' | 'agentSessionRef'>, mode: 'create' | 'resume'): string[] {
	const ref = session.agentSessionRef;
	if (!ref) {
		return [];
	}
	if (session.program === 'claude' && ref.kind === 'name') {
		return mode === 'resume' ? ['--resume', ref.value] : ['--name', ref.value];
	}
	if (session.program === 'pi' && ref.kind === 'path') {
		return ['--session', ref.value];
	}
	return [];
}

async function prepareAgentSessionRef(ref: AgentSessionRef | undefined): Promise<void> {
	if (ref?.kind === 'path') {
		await fs.mkdir(path.dirname(ref.value), {recursive: true});
	}
}

function resolveShellCommand(): string {
	return process.env.SHELL || '/bin/sh';
}

async function resolveLazyGitCommand(): Promise<string> {
	const shell = process.env.SHELL || '/bin/bash';
	try {
		const {stdout} = await execFileAsync(shell, ['-ic', 'command -v lazygit']);
		const resolved = stdout.trim();
		if (resolved) {
			return resolved;
		}
	} catch {
		// handled below
	}
	throw new Error('lazygit is not installed or not on PATH');
}

function clampScrollback(value: string): string {
	return value.length <= SCROLLBACK_LIMIT ? value : value.slice(-SCROLLBACK_LIMIT);
}

function clampSize(value: number, fallback: number): number {
	if (!Number.isFinite(value)) {
		return fallback;
	}
	return Math.max(1, Math.floor(value));
}

function signalPtyProcess(term: IPty, signal: NodeJS.Signals, forceGroup = false): void {
	if (forceGroup && process.platform !== 'win32') {
		try {
			// node-pty children are normally session/process-group leaders. Signalling
			// the group catches shells that spawned the actual agent as a child.
			process.kill(-term.pid, signal);
			return;
		} catch {
			// Fall back to node-pty's direct child signalling below.
		}
	}

	try {
		term.kill(signal);
	} catch {
		// The process may already be gone; the onExit handler will clean up state.
	}
}

export class InkDaemon {
	private readonly sessions = new Map<string, SessionRecord>();
	private readonly runtime = new Map<string, RuntimeSession>();
	private readonly terminals = new Map<string, RuntimeTerminal>();
	private readonly gits = new Map<string, RuntimeTerminal>();
	private readonly devs = new Map<string, RuntimeTerminal & {command: string}>();
	private readonly workers = new Map<string, WorkerRuntime>();
	private readonly clients = new Map<net.Socket, ClientSubscription>();
	private server?: net.Server;
	private shuttingDown = false;

	async start(): Promise<void> {
		await ensureConfigDir();
		await this.log(`starting daemon pid=${process.pid}`);
		await this.assertNoLiveDaemonFromPidFile();
		await ensureNodePtyReady();
		// If this daemon is starting, any previously-running sessions belonged to a
		// different daemon process and their node-pty handles are gone. Mark them
		// exited as crash/restart recovery, not as normal frontend quit behavior.
		const stored = await markAllNonExitedSessionsExited();
		for (const session of stored) {
			this.sessions.set(session.id, session);
		}

		await this.prepareSocket();
		await this.listen();
		await this.writePidFile();
		this.setupProcessHandlers();
		await this.log(`daemon ready socket=${getSocketPath()}`);
	}

	private async log(message: string): Promise<void> {
		try {
			await fs.appendFile(getDaemonLogPath(), `[${new Date().toISOString()}] daemon ${message}\n`, 'utf8');
		} catch {
			// Logging must never keep the daemon from starting or shutting down.
		}
	}

	private isProcessAlive(pid: number): boolean {
		try {
			process.kill(pid, 0);
			return true;
		} catch {
			return false;
		}
	}

	private async assertNoLiveDaemonFromPidFile(): Promise<void> {
		try {
			const raw = await fs.readFile(getDaemonPidPath(), 'utf8');
			const pid = Number.parseInt(raw.trim(), 10);
			if (Number.isFinite(pid) && pid > 0 && pid !== process.pid && this.isProcessAlive(pid)) {
				throw new Error(`daemon already appears to be running as pid ${pid}`);
			}
		} catch (error) {
			const err = error as NodeJS.ErrnoException;
			if (err.code === 'ENOENT') {
				return;
			}
			throw error;
		}
	}

	private async writePidFile(): Promise<void> {
		await fs.writeFile(getDaemonPidPath(), `${process.pid}\n`, 'utf8');
	}

	private async removePidFile(): Promise<void> {
		try {
			const raw = await fs.readFile(getDaemonPidPath(), 'utf8');
			if (Number.parseInt(raw.trim(), 10) !== process.pid) {
				return;
			}
			await fs.unlink(getDaemonPidPath());
		} catch {
			// ignore pid cleanup failures
		}
	}

	private async prepareSocket(): Promise<void> {
		try {
			await fs.unlink(getSocketPath());
			await this.log('removed stale socket before listen');
		} catch (error) {
			const err = error as NodeJS.ErrnoException;
			if (err.code !== 'ENOENT') {
				throw error;
			}
		}
	}

	private async listen(): Promise<void> {
		this.server = net.createServer(socket => this.handleConnection(socket));
		await new Promise<void>((resolve, reject) => {
			this.server?.once('error', reject);
			this.server?.listen(getSocketPath(), () => resolve());
		});
	}

	private setupProcessHandlers(): void {
		const shutdown = async (signal: NodeJS.Signals) => {
			try {
				await this.log(`received ${signal}; shutting down`);
				await this.cleanup();
			} finally {
				process.exit(0);
			}
		};
		process.on('SIGINT', () => void shutdown('SIGINT'));
		process.on('SIGTERM', () => void shutdown('SIGTERM'));
		process.on('SIGHUP', () => {
			void this.log('received SIGHUP; keeping daemon alive');
		});
		process.on('uncaughtException', error => {
			void (async () => {
				try {
					await this.log(`uncaught exception: ${error.stack || error.message}`);
					await this.cleanup();
				} finally {
					process.exit(1);
				}
			})();
		});
		process.on('unhandledRejection', reason => {
			void (async () => {
				try {
					await this.log(`unhandled rejection: ${reason instanceof Error ? reason.stack || reason.message : String(reason)}`);
					await this.cleanup();
				} finally {
					process.exit(1);
				}
			})();
		});
	}

	private async cleanup(): Promise<void> {
		if (this.shuttingDown) {
			return;
		}
		this.shuttingDown = true;
		await this.log('cleanup start');
		for (const socket of this.clients.keys()) {
			socket.destroy();
		}
		this.clients.clear();

		for (const [sessionId, runtime] of this.runtime.entries()) {
			this.clearRuntimeActivityTimers(runtime);
			if (runtime.previewBroadcastTimer) {
				clearTimeout(runtime.previewBroadcastTimer);
			}
			const existing = this.sessions.get(sessionId);
			if (existing && existing.status !== 'exited') {
				this.sessions.set(sessionId, {
					...existing,
					lastPreview: await runtime.preview.getSnapshot(),
				});
			}
			try {
				runtime.term.kill();
			} catch {
				// ignore shutdown errors
			}
			runtime.preview.dispose();
		}
		this.runtime.clear();
		for (const terminal of this.terminals.values()) {
			if (terminal.broadcastTimer) {
				clearTimeout(terminal.broadcastTimer);
			}
			try {
				terminal.term.kill();
			} catch {
				// ignore shutdown errors
			}
			terminal.preview.dispose();
		}
		this.terminals.clear();
		for (const git of this.gits.values()) {
			if (git.broadcastTimer) {
				clearTimeout(git.broadcastTimer);
			}
			try {
				git.term.kill();
			} catch {
				// ignore shutdown errors
			}
			git.preview.dispose();
		}
		this.gits.clear();
		for (const dev of this.devs.values()) {
			if (dev.broadcastTimer) {
				clearTimeout(dev.broadcastTimer);
			}
			try {
				dev.term.kill();
			} catch {
				// ignore shutdown errors
			}
			dev.preview.dispose();
		}
		this.devs.clear();

		for (const [sessionId, worker] of this.workers.entries()) {
			for (const pending of worker.pending.values()) {
				clearTimeout(pending.timer);
				pending.reject(new Error('daemon shutting down'));
			}
			worker.pending.clear();
			try {
				if (worker.process.connected) {
					worker.process.send?.({type: 'kill', requestId: randomUUID(), force: true});
				}
			} catch {
				// Fall through to direct worker termination.
			}
			setTimeout(() => {
				try { worker.process.kill('SIGKILL'); } catch {}
			}, 500).unref?.();
			await fs.rm(getWorkerPidPath(sessionId), {force: true}).catch(() => {});
		}
		this.workers.clear();
		await this.persist();

		if (this.server) {
			this.server.close();
		}
		try {
			await fs.unlink(getSocketPath());
		} catch {
			// ignore socket cleanup failures
		}
		await this.removePidFile();
		await this.log('cleanup complete');
	}

	private handleConnection(socket: net.Socket): void {
		void this.log('client connected');
		this.clients.set(socket, {
			previewCols: DEFAULT_PREVIEW_COLS,
			previewRows: DEFAULT_PREVIEW_ROWS,
			terminalCols: DEFAULT_PREVIEW_COLS,
			terminalRows: DEFAULT_PREVIEW_ROWS,
			gitCols: DEFAULT_PREVIEW_COLS,
			gitRows: DEFAULT_PREVIEW_ROWS,
			devCols: DEFAULT_PREVIEW_COLS,
			devRows: DEFAULT_PREVIEW_ROWS,
		});

		let buffer = '';
		let attachedSessionId: string | undefined;

		const cleanup = () => {
			if (attachedSessionId) {
				const runtime = this.runtime.get(attachedSessionId);
				if (runtime?.attachedSocket === socket) {
					runtime.attachedSocket = undefined;
				}
				const terminal = this.terminals.get(attachedSessionId);
				if (terminal?.attachedSocket === socket) {
					terminal.attachedSocket = undefined;
				}
				const git = this.gits.get(attachedSessionId);
				if (git?.attachedSocket === socket) {
					git.attachedSocket = undefined;
				}
				const dev = this.devs.get(attachedSessionId);
				if (dev?.attachedSocket === socket) {
					dev.attachedSocket = undefined;
				}
				const worker = this.workers.get(attachedSessionId);
				if (worker) {
					for (const target of ['agent', 'terminal', 'git', 'dev'] as const) {
						if (worker.attached[target] === socket) {
							worker.attached[target] = undefined;
							this.sendWorkerEvent(attachedSessionId, {type: 'detach', target});
						}
					}
				}
				attachedSessionId = undefined;
			}
			this.clients.delete(socket);
			void this.log('client disconnected');
		};

		socket.on('data', chunk => {
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
				const message = JSON.parse(line) as ClientRequest;
				void this.handleRequest(socket, message, sessionId => {
					attachedSessionId = sessionId;
				});
			}
		});

		socket.on('close', cleanup);
		socket.on('error', cleanup);
	}

	private getClient(socket: net.Socket): ClientSubscription {
		const client = this.clients.get(socket);
		if (!client) {
			const created: ClientSubscription = {
				previewCols: DEFAULT_PREVIEW_COLS,
				previewRows: DEFAULT_PREVIEW_ROWS,
				terminalCols: DEFAULT_PREVIEW_COLS,
				terminalRows: DEFAULT_PREVIEW_ROWS,
				gitCols: DEFAULT_PREVIEW_COLS,
				gitRows: DEFAULT_PREVIEW_ROWS,
				devCols: DEFAULT_PREVIEW_COLS,
				devRows: DEFAULT_PREVIEW_ROWS,
			};
			this.clients.set(socket, created);
			return created;
		}
		return client;
	}

	private async handleRequest(
		socket: net.Socket,
		message: ClientRequest,
		setAttachedSessionId: (sessionId: string | undefined) => void,
	): Promise<void> {
		try {
			switch (message.type) {
				case 'ping':
					sendMessage(socket, response(message.requestId, {ok: true, version: PROTOCOL_VERSION}));
					return;
				case 'list':
					sendMessage(socket, response(message.requestId, sortSessionsNewestFirst([...this.sessions.values()])));
					return;
				case 'subscribe': {
					const client = this.getClient(socket);
					client.repoRoot = message.repoRoot;
					sendMessage(socket, response(message.requestId, this.sessionsForRepo(message.repoRoot)));
					return;
				}
				case 'list-worktrees': {
					sendMessage(socket, response(message.requestId, await listWorktrees(message.cwd)));
					return;
				}
				case 'watch-preview': {
					const client = this.getClient(socket);
					client.watchedPreviewSessionId = message.sessionId;
					client.previewCols = clampSize(message.cols, client.previewCols);
					client.previewRows = clampSize(message.rows, client.previewRows);
					const preview = await this.getPreviewRecord(message.sessionId, client.previewCols, client.previewRows);
					sendMessage(socket, response(message.requestId, preview));
					return;
				}
				case 'watch-terminal': {
					const client = this.getClient(socket);
					client.watchedTerminalSessionId = message.sessionId;
					client.terminalCols = clampSize(message.cols, client.terminalCols);
					client.terminalRows = clampSize(message.rows, client.terminalRows);
					const terminal = await this.getTerminalRecord(message.sessionId, client.terminalCols, client.terminalRows);
					sendMessage(socket, response(message.requestId, terminal));
					return;
				}
				case 'watch-git': {
					const client = this.getClient(socket);
					client.watchedGitSessionId = message.sessionId;
					client.gitCols = clampSize(message.cols, client.gitCols);
					client.gitRows = clampSize(message.rows, client.gitRows);
					const git = await this.getGitRecord(message.sessionId, client.gitCols, client.gitRows);
					sendMessage(socket, response(message.requestId, git));
					return;
				}
				case 'watch-dev': {
					const client = this.getClient(socket);
					client.watchedDevSessionId = message.sessionId;
					client.devCols = clampSize(message.cols, client.devCols);
					client.devRows = clampSize(message.rows, client.devRows);
					const dev = await this.getDevRecord(message.sessionId, client.devCols, client.devRows);
					sendMessage(socket, response(message.requestId, dev));
					return;
				}
				case 'start-dev': {
					if (this.workers.has(message.sessionId)) {
						sendMessage(socket, response(message.requestId, await this.sendWorkerRequest<DevRecord>(message.sessionId, {type: 'start-dev', cols: clampSize(message.cols, DEFAULT_PREVIEW_COLS), rows: clampSize(message.rows, DEFAULT_PREVIEW_ROWS)})));
						return;
					}
					const dev = await this.startDev(message.sessionId, clampSize(message.cols, DEFAULT_PREVIEW_COLS), clampSize(message.rows, DEFAULT_PREVIEW_ROWS));
					sendMessage(socket, response(message.requestId, this.buildDevRecord(message.sessionId, dev)));
					return;
				}
				case 'stop-dev':
					if (this.workers.has(message.sessionId)) {
						sendMessage(socket, response(message.requestId, await this.sendWorkerRequest(message.sessionId, {type: 'stop-dev'})));
						return;
					}
					this.cleanupDev(message.sessionId);
					await this.broadcastDev(message.sessionId);
					sendMessage(socket, response(message.requestId, {ok: true}));
					return;
				case 'create': {
					const session = await this.createSession(message.input);
					sendMessage(socket, response(message.requestId, session));
					return;
				}
				case 'restart': {
					const session = await this.restartSession(message.sessionId, message.cols, message.rows);
					sendMessage(socket, response(message.requestId, session));
					return;
				}
				case 'kill':
					await this.killSession(message.sessionId, message.deleteWorktree ?? false, message.deleteBranch ?? false, message.force ?? false);
					sendMessage(socket, response(message.requestId, {ok: true}));
					return;
				case 'merge-worktree':
					sendMessage(socket, response(message.requestId, await this.mergeSessionWorktree(message.sessionId, message.mode, message.targetCwd)));
					return;
				case 'remove':
					await this.removeSession(message.sessionId);
					sendMessage(socket, response(message.requestId, {ok: true}));
					return;
				case 'attach': {
					if (this.workers.has(message.sessionId)) {
						const session = this.sessions.get(message.sessionId);
						const worker = this.workers.get(message.sessionId)!;
						if (!session || session.status === 'exited') throw new Error('session is not running');
						if (worker.attached.agent && worker.attached.agent !== socket && !worker.attached.agent.destroyed) throw new Error('session is already attached elsewhere');
						worker.attached.agent = socket;
						setAttachedSessionId(session.id);
						const cols = clampSize(message.cols ?? DEFAULT_PREVIEW_COLS, DEFAULT_PREVIEW_COLS);
						const rows = clampSize(message.rows ?? DEFAULT_PREVIEW_ROWS, DEFAULT_PREVIEW_ROWS);
						sendMessage(socket, response(message.requestId, session));
						sendMessage(socket, {type: 'attached', sessionId: session.id});
						await this.sendWorkerRequest(message.sessionId, {type: 'attach', target: 'agent', cols, rows});
						return;
					}
					const session = this.sessions.get(message.sessionId);
					const runtime = this.runtime.get(message.sessionId);
					if (!session || !runtime) {
						throw new Error('session is not running');
					}
					if (runtime.attachedSocket && runtime.attachedSocket !== socket && !runtime.attachedSocket.destroyed) {
						throw new Error('session is already attached elsewhere');
					}
					const cols = clampSize(message.cols ?? DEFAULT_PREVIEW_COLS, DEFAULT_PREVIEW_COLS);
					const rows = clampSize(message.rows ?? DEFAULT_PREVIEW_ROWS, DEFAULT_PREVIEW_ROWS);
					runtime.term.resize(cols, rows);
					await runtime.preview.resize(cols, rows);
					await this.suppressResizeActivity(runtime);
					runtime.attachedSocket = socket;
					setAttachedSessionId(session.id);
					sendMessage(socket, response(message.requestId, session));
					sendMessage(socket, {type: 'attached', sessionId: session.id});
					sendMessage(socket, {type: 'output', sessionId: session.id, data: await runtime.preview.getAnsiFrame()});
					return;
				}
				case 'input': {
					if (this.workers.has(message.sessionId)) {
						this.sendWorkerEvent(message.sessionId, {type: 'input', target: 'agent', data: message.data});
						return;
					}
					const runtime = this.runtime.get(message.sessionId);
					if (runtime) {
						runtime.term.write(message.data);
					}
					return;
				}
				case 'resize': {
					if (this.workers.has(message.sessionId)) {
						this.sendWorkerEvent(message.sessionId, {type: 'resize', target: 'agent', cols: message.cols, rows: message.rows});
						return;
					}
					const runtime = this.runtime.get(message.sessionId);
					if (runtime) {
						const cols = Math.max(1, message.cols);
						const rows = Math.max(1, message.rows);
						runtime.term.resize(cols, rows);
						await runtime.preview.resize(cols, rows);
						await this.suppressResizeActivity(runtime);
						this.schedulePreviewBroadcast(message.sessionId);
					}
					return;
				}
				case 'detach': {
					if (this.workers.has(message.sessionId)) {
						const worker = this.workers.get(message.sessionId)!;
						if (worker.attached.agent === socket) worker.attached.agent = undefined;
						this.sendWorkerEvent(message.sessionId, {type: 'detach', target: 'agent'});
						setAttachedSessionId(undefined);
						sendMessage(socket, {type: 'detached', sessionId: message.sessionId});
						return;
					}
					const runtime = this.runtime.get(message.sessionId);
					if (runtime?.attachedSocket === socket) {
						runtime.attachedSocket = undefined;
					}
					setAttachedSessionId(undefined);
					sendMessage(socket, {type: 'detached', sessionId: message.sessionId});
					return;
				}
				case 'attach-terminal': {
					if (this.workers.has(message.sessionId)) {
						const session = this.sessions.get(message.sessionId);
						const worker = this.workers.get(message.sessionId)!;
						if (!session || session.status === 'exited') throw new Error('session is not running');
						if (worker.attached.terminal && worker.attached.terminal !== socket && !worker.attached.terminal.destroyed) throw new Error('terminal is already attached elsewhere');
						worker.attached.terminal = socket;
						setAttachedSessionId(message.sessionId);
						sendMessage(socket, response(message.requestId, session));
						sendMessage(socket, {type: 'terminal-attached', sessionId: message.sessionId});
						await this.sendWorkerRequest(message.sessionId, {type: 'attach', target: 'terminal', cols: clampSize(message.cols ?? DEFAULT_PREVIEW_COLS, DEFAULT_PREVIEW_COLS), rows: clampSize(message.rows ?? DEFAULT_PREVIEW_ROWS, DEFAULT_PREVIEW_ROWS)});
						return;
					}
					const terminal = await this.ensureTerminal(
						message.sessionId,
						clampSize(message.cols ?? DEFAULT_PREVIEW_COLS, DEFAULT_PREVIEW_COLS),
						clampSize(message.rows ?? DEFAULT_PREVIEW_ROWS, DEFAULT_PREVIEW_ROWS),
					);
					if (terminal.attachedSocket && terminal.attachedSocket !== socket && !terminal.attachedSocket.destroyed) {
						throw new Error('terminal is already attached elsewhere');
					}
					terminal.attachedSocket = socket;
					setAttachedSessionId(message.sessionId);
					sendMessage(socket, response(message.requestId, this.sessions.get(message.sessionId)));
					sendMessage(socket, {type: 'terminal-attached', sessionId: message.sessionId});
					sendMessage(socket, {type: 'terminal-output', sessionId: message.sessionId, data: await terminal.preview.getAnsiFrame()});
					return;
				}
				case 'terminal-input': {
					if (this.workers.has(message.sessionId)) { this.sendWorkerEvent(message.sessionId, {type: 'input', target: 'terminal', data: message.data}); return; }
					const terminal = this.terminals.get(message.sessionId);
					if (terminal && !terminal.exited) {
						terminal.term.write(message.data);
					}
					return;
				}
				case 'terminal-resize': {
					if (this.workers.has(message.sessionId)) { this.sendWorkerEvent(message.sessionId, {type: 'resize', target: 'terminal', cols: message.cols, rows: message.rows}); return; }
					const terminal = this.terminals.get(message.sessionId);
					if (terminal && !terminal.exited) {
						const cols = Math.max(1, message.cols);
						const rows = Math.max(1, message.rows);
						terminal.term.resize(cols, rows);
						await terminal.preview.resize(cols, rows);
						this.scheduleTerminalBroadcast(message.sessionId);
					}
					return;
				}
				case 'terminal-detach': {
					if (this.workers.has(message.sessionId)) { const worker = this.workers.get(message.sessionId)!; if (worker.attached.terminal === socket) worker.attached.terminal = undefined; this.sendWorkerEvent(message.sessionId, {type: 'detach', target: 'terminal'}); setAttachedSessionId(undefined); sendMessage(socket, {type: 'terminal-detached', sessionId: message.sessionId}); return; }
					const terminal = this.terminals.get(message.sessionId);
					if (terminal?.attachedSocket === socket) {
						terminal.attachedSocket = undefined;
					}
					setAttachedSessionId(undefined);
					sendMessage(socket, {type: 'terminal-detached', sessionId: message.sessionId});
					return;
				}
				case 'attach-git': {
					if (this.workers.has(message.sessionId)) {
						const session = this.sessions.get(message.sessionId);
						const worker = this.workers.get(message.sessionId)!;
						if (!session || session.status === 'exited') throw new Error('session is not running');
						if (worker.attached.git && worker.attached.git !== socket && !worker.attached.git.destroyed) throw new Error('git is already attached elsewhere');
						worker.attached.git = socket;
						setAttachedSessionId(message.sessionId);
						sendMessage(socket, response(message.requestId, session));
						sendMessage(socket, {type: 'git-attached', sessionId: message.sessionId});
						await this.sendWorkerRequest(message.sessionId, {type: 'attach', target: 'git', cols: clampSize(message.cols ?? DEFAULT_PREVIEW_COLS, DEFAULT_PREVIEW_COLS), rows: clampSize(message.rows ?? DEFAULT_PREVIEW_ROWS, DEFAULT_PREVIEW_ROWS)});
						return;
					}
					const git = await this.ensureGit(
						message.sessionId,
						clampSize(message.cols ?? DEFAULT_PREVIEW_COLS, DEFAULT_PREVIEW_COLS),
						clampSize(message.rows ?? DEFAULT_PREVIEW_ROWS, DEFAULT_PREVIEW_ROWS),
					);
					if (git.attachedSocket && git.attachedSocket !== socket && !git.attachedSocket.destroyed) {
						throw new Error('git is already attached elsewhere');
					}
					git.attachedSocket = socket;
					setAttachedSessionId(message.sessionId);
					sendMessage(socket, response(message.requestId, this.sessions.get(message.sessionId)));
					sendMessage(socket, {type: 'git-attached', sessionId: message.sessionId});
					sendMessage(socket, {type: 'git-output', sessionId: message.sessionId, data: await git.preview.getAnsiFrame()});
					return;
				}
				case 'git-input': {
					if (this.workers.has(message.sessionId)) { this.sendWorkerEvent(message.sessionId, {type: 'input', target: 'git', data: message.data}); return; }
					const git = this.gits.get(message.sessionId);
					if (git && !git.exited) {
						git.term.write(message.data);
					}
					return;
				}
				case 'git-resize': {
					if (this.workers.has(message.sessionId)) { this.sendWorkerEvent(message.sessionId, {type: 'resize', target: 'git', cols: message.cols, rows: message.rows}); return; }
					const git = this.gits.get(message.sessionId);
					if (git && !git.exited) {
						const cols = Math.max(1, message.cols);
						const rows = Math.max(1, message.rows);
						git.term.resize(cols, rows);
						await git.preview.resize(cols, rows);
						this.scheduleGitBroadcast(message.sessionId);
					}
					return;
				}
				case 'git-detach': {
					if (this.workers.has(message.sessionId)) { const worker = this.workers.get(message.sessionId)!; if (worker.attached.git === socket) worker.attached.git = undefined; this.sendWorkerEvent(message.sessionId, {type: 'detach', target: 'git'}); setAttachedSessionId(undefined); sendMessage(socket, {type: 'git-detached', sessionId: message.sessionId}); return; }
					const git = this.gits.get(message.sessionId);
					if (git?.attachedSocket === socket) {
						git.attachedSocket = undefined;
					}
					setAttachedSessionId(undefined);
					sendMessage(socket, {type: 'git-detached', sessionId: message.sessionId});
					return;
				}
				case 'attach-dev': {
					if (this.workers.has(message.sessionId)) {
						const session = this.sessions.get(message.sessionId);
						const worker = this.workers.get(message.sessionId)!;
						if (!session || session.status === 'exited') throw new Error('session is not running');
						if (worker.attached.dev && worker.attached.dev !== socket && !worker.attached.dev.destroyed) throw new Error('dev command is already attached elsewhere');
						worker.attached.dev = socket;
						setAttachedSessionId(message.sessionId);
						sendMessage(socket, response(message.requestId, session));
						sendMessage(socket, {type: 'dev-attached', sessionId: message.sessionId});
						await this.sendWorkerRequest(message.sessionId, {type: 'attach', target: 'dev', cols: clampSize(message.cols ?? DEFAULT_PREVIEW_COLS, DEFAULT_PREVIEW_COLS), rows: clampSize(message.rows ?? DEFAULT_PREVIEW_ROWS, DEFAULT_PREVIEW_ROWS)});
						return;
					}
					const dev = this.devs.get(message.sessionId);
					if (!dev || dev.exited) {
						throw new Error('no running dev command; press d to start it');
					}
					if (dev.attachedSocket && dev.attachedSocket !== socket && !dev.attachedSocket.destroyed) {
						throw new Error('dev command is already attached elsewhere');
					}
					dev.term.resize(clampSize(message.cols ?? DEFAULT_PREVIEW_COLS, DEFAULT_PREVIEW_COLS), clampSize(message.rows ?? DEFAULT_PREVIEW_ROWS, DEFAULT_PREVIEW_ROWS));
					dev.attachedSocket = socket;
					setAttachedSessionId(message.sessionId);
					sendMessage(socket, response(message.requestId, this.sessions.get(message.sessionId)));
					sendMessage(socket, {type: 'dev-attached', sessionId: message.sessionId});
					sendMessage(socket, {type: 'dev-output', sessionId: message.sessionId, data: await dev.preview.getAnsiFrame()});
					return;
				}
				case 'dev-input': {
					if (this.workers.has(message.sessionId)) { this.sendWorkerEvent(message.sessionId, {type: 'input', target: 'dev', data: message.data}); return; }
					const dev = this.devs.get(message.sessionId);
					if (dev && !dev.exited) {
						dev.term.write(message.data);
					}
					return;
				}
				case 'dev-resize': {
					if (this.workers.has(message.sessionId)) { this.sendWorkerEvent(message.sessionId, {type: 'resize', target: 'dev', cols: message.cols, rows: message.rows}); return; }
					const dev = this.devs.get(message.sessionId);
					if (dev && !dev.exited) {
						const cols = Math.max(1, message.cols);
						const rows = Math.max(1, message.rows);
						dev.term.resize(cols, rows);
						await dev.preview.resize(cols, rows);
						this.scheduleDevBroadcast(message.sessionId);
					}
					return;
				}
				case 'dev-detach': {
					if (this.workers.has(message.sessionId)) { const worker = this.workers.get(message.sessionId)!; if (worker.attached.dev === socket) worker.attached.dev = undefined; this.sendWorkerEvent(message.sessionId, {type: 'detach', target: 'dev'}); setAttachedSessionId(undefined); sendMessage(socket, {type: 'dev-detached', sessionId: message.sessionId}); return; }
					const dev = this.devs.get(message.sessionId);
					if (dev?.attachedSocket === socket) {
						dev.attachedSocket = undefined;
					}
					setAttachedSessionId(undefined);
					sendMessage(socket, {type: 'dev-detached', sessionId: message.sessionId});
					return;
				}
			}
		} catch (error) {
			if ('requestId' in message) {
				sendMessage(socket, failure(message.requestId, error));
			}
		}
	}

	private async startWorker(session: SessionRecord, cols: number, rows: number): Promise<SessionRecord> {
		await fs.mkdir(getWorkerDir(), {recursive: true});
		const child = fork(getCliEntryPath(), ['--session-worker'], {
			env: {...process.env},
			stdio: ['ignore', 'pipe', 'pipe', 'ipc'],
		});
		const worker: WorkerRuntime = {process: child, pending: new Map(), attached: {}};
		this.workers.set(session.id, worker);
		await fs.writeFile(getWorkerPidPath(session.id), String(child.pid ?? ''), 'utf8').catch(() => {});
		const logPath = getWorkerLogPath(session.id);
		child.stdout?.on('data', chunk => void fs.appendFile(logPath, chunk).catch(() => {}));
		child.stderr?.on('data', chunk => void fs.appendFile(logPath, chunk).catch(() => {}));
		child.on('message', message => void this.handleWorkerMessage(session.id, message as WorkerEvent));
		child.on('exit', () => void this.handleWorkerProcessExit(session.id));
		try {
			return await this.sendWorkerRequest<SessionRecord>(session.id, {type: 'start', session, cols, rows});
		} catch (error) {
			this.workers.delete(session.id);
			await fs.rm(getWorkerPidPath(session.id), {force: true}).catch(() => {});
			try { child.kill('SIGKILL'); } catch {}
			throw error;
		}
	}

	private sendWorkerRequest<T>(sessionId: string, payload: Record<string, unknown>): Promise<T> {
		const worker = this.workers.get(sessionId);
		if (!worker || worker.exited || !worker.process.connected) {
			return Promise.reject(new Error('session worker is not running'));
		}
		const requestId = randomUUID();
		return new Promise<T>((resolve, reject) => {
			const timer = setTimeout(() => {
				worker.pending.delete(requestId);
				reject(new Error('session worker request timed out'));
			}, WORKER_REQUEST_TIMEOUT_MS);
			timer.unref?.();
			worker.pending.set(requestId, {resolve: value => resolve(value as T), reject, timer});
			worker.process.send?.({...payload, requestId}, error => {
				if (error) {
					const pending = worker.pending.get(requestId);
					if (pending) clearTimeout(pending.timer);
					worker.pending.delete(requestId);
					reject(error);
				}
			});
		});
	}

	private sendWorkerEvent(sessionId: string, payload: Record<string, unknown>): void {
		const worker = this.workers.get(sessionId);
		if (worker && !worker.exited && worker.process.connected) {
			worker.process.send?.(payload);
		}
	}

	private async handleWorkerMessage(sessionId: string, message: WorkerEvent): Promise<void> {
		const worker = this.workers.get(sessionId);
		if (message.type === 'response') {
			const pending = worker?.pending.get(message.requestId);
			if (pending) {
				clearTimeout(pending.timer);
				worker?.pending.delete(message.requestId);
				message.ok ? pending.resolve(message.data) : pending.reject(new Error(message.error));
			}
			return;
		}
		if (message.type === 'running') {
			const session = this.sessions.get(sessionId);
			if (session && session.status !== 'exited') {
				const updated = {...session, status: 'running' as const, pid: message.pid, updatedAt: new Date().toISOString()};
				this.sessions.set(sessionId, updated);
				await this.persist();
				this.broadcastSessionUpdated(updated);
			}
			return;
		}
		if (message.type === 'exit') {
			await this.handleWorkerSessionExit(sessionId, message.exitCode, message.exitSignal, message.lastPreview);
			return;
		}
		if (message.type === 'agent-status') {
			await this.setWorkerAgentStatus(sessionId, message.agentStatus);
			return;
		}
		if (message.type === 'preview-updated') {
			for (const [socket, client] of this.clients.entries()) if (client.watchedPreviewSessionId === sessionId) sendMessage(socket, {type: 'preview-updated', preview: message.preview});
			return;
		}
		if (message.type === 'terminal-updated') {
			for (const [socket, client] of this.clients.entries()) if (client.watchedTerminalSessionId === sessionId) sendMessage(socket, {type: 'terminal-updated', terminal: message.terminal});
			return;
		}
		if (message.type === 'git-updated') {
			for (const [socket, client] of this.clients.entries()) if (client.watchedGitSessionId === sessionId) sendMessage(socket, {type: 'git-updated', git: message.git});
			return;
		}
		if (message.type === 'dev-updated') {
			await this.updateSessionDevRunning(sessionId, message.dev.live);
			for (const [socket, client] of this.clients.entries()) if (client.watchedDevSessionId === sessionId) sendMessage(socket, {type: 'dev-updated', dev: message.dev});
			return;
		}
		if (message.type === 'output') {
			const attached = worker?.attached[message.target];
			if (attached && !attached.destroyed) {
				const eventType = message.target === 'agent' ? 'output' : `${message.target}-output`;
				sendMessage(attached, {type: eventType, sessionId, data: message.data} as ServerMessage);
			}
		}
	}

	private async handleWorkerProcessExit(sessionId: string): Promise<void> {
		const worker = this.workers.get(sessionId);
		if (worker) {
			worker.exited = true;
			for (const pending of worker.pending.values()) {
				clearTimeout(pending.timer);
				pending.reject(new Error('session worker exited'));
			}
			worker.pending.clear();
		}
		await fs.rm(getWorkerPidPath(sessionId), {force: true}).catch(() => {});
		const session = this.sessions.get(sessionId);
		if (session && session.status !== 'exited') {
			await this.handleWorkerSessionExit(sessionId, null, null, session.lastPreview ?? 'Session worker exited unexpectedly');
		}
	}

	private async setWorkerAgentStatus(sessionId: string, agentStatus: AgentActivityStatus): Promise<void> {
		const session = this.sessions.get(sessionId);
		if (!session || session.status === 'exited' || session.agentStatus === agentStatus) return;
		const updated = {...session, agentStatus, agentStatusUpdatedAt: new Date().toISOString(), updatedAt: session.updatedAt};
		this.sessions.set(sessionId, updated);
		await this.persist();
		this.broadcastSessionUpdated(updated);
	}

	private async handleWorkerSessionExit(sessionId: string, exitCode: number | null, exitSignal: number | null, lastPreview: string): Promise<void> {
		const existing = this.sessions.get(sessionId);
		if (!existing || existing.status === 'exited') return;
		const worker = this.workers.get(sessionId);
		this.workers.delete(sessionId);
		await fs.rm(getWorkerPidPath(sessionId), {force: true}).catch(() => {});
		const now = new Date().toISOString();
		const updated: SessionRecord = {...existing, status: 'exited', agentStatus: 'idle', agentStatusUpdatedAt: now, updatedAt: now, pid: undefined, exitCode, exitSignal, lastPreview, devRunning: false};
		this.sessions.set(sessionId, updated);
		if (worker?.deleteWorktreeOnExit && existing.worktree?.path) {
			try {
				const repoCwd = existing.launchWorktreeRoot ?? existing.repoRoot;
				const branch = existing.worktree.branch;
				await removeWorktree(existing.worktree.path, repoCwd);
				if (worker.deleteBranchOnExit && branch) await deleteLocalBranch(repoCwd, branch);
			} catch (error) {
				await this.log(`failed to remove worktree/branch for session ${existing.title}: ${error instanceof Error ? error.message : String(error)}`);
			}
		}
		await this.persist();
		this.broadcastSessionUpdated(updated);
		for (const [socket, client] of this.clients.entries()) {
			if (client.watchedPreviewSessionId === sessionId) sendMessage(socket, {type: 'preview-updated', preview: this.buildPreviewRecord(updated, updated.lastPreview ?? '')});
		}
	}

	private sessionsForRepo(repoRoot: string): SessionRecord[] {
		return sortSessionsNewestFirst([...this.sessions.values()].filter(session => session.repoRoot === repoRoot));
	}

	private broadcastSessionUpdated(session: SessionRecord): void {
		for (const [socket, client] of this.clients.entries()) {
			if (client.repoRoot === session.repoRoot) {
				sendMessage(socket, {type: 'session-updated', session});
			}
		}
	}

	private broadcastSessionRemoved(sessionId: string, repoRoot: string): void {
		for (const [socket, client] of this.clients.entries()) {
			if (client.repoRoot === repoRoot) {
				sendMessage(socket, {type: 'session-removed', sessionId});
			}
		}
	}

	private async updateSessionDevRunning(sessionId: string, devRunning: boolean): Promise<void> {
		const session = this.sessions.get(sessionId);
		if (!session || session.status === 'exited' || Boolean(session.devRunning) === devRunning) return;
		const updated = {...session, devRunning, updatedAt: session.updatedAt};
		this.sessions.set(sessionId, updated);
		await this.persist();
		this.broadcastSessionUpdated(updated);
	}

	private schedulePreviewBroadcast(sessionId: string): void {
		const runtime = this.runtime.get(sessionId);
		if (!runtime || runtime.previewBroadcastTimer) {
			return;
		}
		runtime.previewBroadcastTimer = setTimeout(() => {
			runtime.previewBroadcastTimer = undefined;
			void this.broadcastPreview(sessionId);
		}, PREVIEW_BROADCAST_DELAY_MS);
	}

	private clearRuntimeActivityTimers(runtime: RuntimeSession): void {
		if (runtime.activityEvaluationTimer) {
			clearTimeout(runtime.activityEvaluationTimer);
			runtime.activityEvaluationTimer = undefined;
		}
		if (runtime.activityIdleTimer) {
			clearTimeout(runtime.activityIdleTimer);
			runtime.activityIdleTimer = undefined;
		}
	}

	private changedCharacterCount(previous: string, next: string): number {
		const maxLength = Math.max(previous.length, next.length);
		let changed = Math.abs(previous.length - next.length);
		const sharedLength = Math.min(previous.length, next.length);
		for (let index = 0; index < sharedLength; index += 1) {
			if (previous[index] !== next[index]) {
				changed += 1;
			}
		}
		return Math.min(changed, maxLength);
	}

	private scheduleActivityEvaluation(sessionId: string): void {
		const runtime = this.runtime.get(sessionId);
		if (!runtime || runtime.activityEvaluationTimer || Date.now() < (runtime.suppressActivityUntil ?? 0)) {
			return;
		}
		runtime.activityEvaluationTimer = setTimeout(() => {
			runtime.activityEvaluationTimer = undefined;
			void this.evaluatePreviewActivity(sessionId);
		}, ACTIVITY_EVALUATION_DELAY_MS);
	}

	private async evaluatePreviewActivity(sessionId: string): Promise<void> {
		const session = this.sessions.get(sessionId);
		const runtime = this.runtime.get(sessionId);
		if (!session || !runtime || session.status !== 'running' || Date.now() < (runtime.suppressActivityUntil ?? 0)) {
			return;
		}

		const snapshot = await runtime.preview.getSnapshot();
		const changedChars = this.changedCharacterCount(runtime.lastPreviewSnapshot, snapshot);
		runtime.lastPreviewSnapshot = snapshot;
		if (changedChars < ACTIVE_MIN_CHANGED_CHARS) {
			return;
		}

		const now = Date.now();
		runtime.previewChangeEvents = runtime.previewChangeEvents
			.filter(event => now - event.at <= ACTIVITY_WINDOW_MS)
			.concat({at: now, changedChars});
		const recentChangedChars = runtime.previewChangeEvents.reduce((total, event) => total + event.changedChars, 0);
		if (recentChangedChars >= ACTIVE_MIN_CHANGED_CHARS) {
			await this.setAgentStatus(sessionId, 'active');
		}

		if (runtime.activityIdleTimer) {
			clearTimeout(runtime.activityIdleTimer);
		}
		runtime.activityIdleTimer = setTimeout(() => {
			runtime.activityIdleTimer = undefined;
			runtime.previewChangeEvents = [];
			void this.setAgentStatus(sessionId, 'idle');
		}, IDLE_AFTER_MS);
	}

	private async suppressResizeActivity(runtime: RuntimeSession): Promise<void> {
		runtime.suppressActivityUntil = Date.now() + RESIZE_ACTIVITY_SUPPRESSION_MS;
		runtime.lastPreviewSnapshot = await runtime.preview.getSnapshot();
	}

	private async setAgentStatus(sessionId: string, agentStatus: AgentActivityStatus): Promise<void> {
		const session = this.sessions.get(sessionId);
		if (!session || session.status === 'exited' || session.agentStatus === agentStatus) {
			return;
		}
		const updated: SessionRecord = {
			...session,
			agentStatus,
			agentStatusUpdatedAt: new Date().toISOString(),
			updatedAt: session.updatedAt,
		};
		this.sessions.set(sessionId, updated);
		await this.persist();
		this.broadcastSessionUpdated(updated);
		await this.broadcastPreview(sessionId);
	}

	private async broadcastPreview(sessionId: string): Promise<void> {
		const session = this.sessions.get(sessionId);
		if (!session) {
			return;
		}
		const runtime = this.runtime.get(sessionId);
		const snapshot = runtime ? await runtime.preview.getSnapshot() : session.lastPreview ?? '';
		for (const [socket, client] of this.clients.entries()) {
			if (client.watchedPreviewSessionId !== sessionId) {
				continue;
			}
			const preview = this.buildPreviewRecord(session, snapshot);
			sendMessage(socket, {type: 'preview-updated', preview});
		}
	}

	private buildPreviewRecord(session: SessionRecord, content: string): PreviewRecord {
		return {
			sessionId: session.id,
			content,
			live: session.status === 'running',
			status: session.status,
			agentStatus: session.agentStatus,
		};
	}

	private async getPreviewRecord(sessionId: string | undefined, cols: number, rows: number): Promise<PreviewRecord> {
		if (!sessionId) {
			return {
				content: '',
				live: false,
			};
		}

		const session = this.sessions.get(sessionId);
		if (!session) {
			return {
				sessionId,
				content: '',
				live: false,
			};
		}

		if (this.workers.has(sessionId)) {
			return await this.sendWorkerRequest<PreviewRecord>(sessionId, {type: 'snapshot', target: 'agent', cols, rows});
		}

		const runtime = this.runtime.get(sessionId);
		if (runtime) {
			runtime.term.resize(cols, rows);
			await runtime.preview.resize(cols, rows);
			await this.suppressResizeActivity(runtime);
			return this.buildPreviewRecord(session, await runtime.preview.getSnapshot());
		}

		return this.buildPreviewRecord(session, session.lastPreview ?? '');
	}

	private buildTerminalRecord(sessionId: string | undefined, terminal?: RuntimeTerminal): TerminalRecord {
		return {
			sessionId,
			content: terminal?.preview.getCachedSnapshot() ?? '',
			live: Boolean(terminal && !terminal.exited),
			cwd: terminal?.cwd,
			exitCode: terminal?.exitCode,
			exitSignal: terminal?.exitSignal,
		};
	}

	private scheduleTerminalBroadcast(sessionId: string): void {
		const terminal = this.terminals.get(sessionId);
		if (!terminal || terminal.broadcastTimer) {
			return;
		}
		terminal.broadcastTimer = setTimeout(() => {
			terminal.broadcastTimer = undefined;
			void this.broadcastTerminal(sessionId);
		}, PREVIEW_BROADCAST_DELAY_MS);
	}

	private async broadcastTerminal(sessionId: string): Promise<void> {
		const terminal = this.terminals.get(sessionId);
		if (terminal) {
			await terminal.preview.getSnapshot();
		}
		for (const [socket, client] of this.clients.entries()) {
			if (client.watchedTerminalSessionId !== sessionId) {
				continue;
			}
			sendMessage(socket, {type: 'terminal-updated', terminal: this.buildTerminalRecord(sessionId, terminal)});
		}
	}

	private async ensureTerminal(sessionId: string, cols: number, rows: number): Promise<RuntimeTerminal> {
		const session = this.sessions.get(sessionId);
		if (!session) {
			throw new Error('session does not exist');
		}
		if (session.status === 'exited') {
			throw new Error('cannot start terminal for exited session');
		}

		const existing = this.terminals.get(sessionId);
		if (existing) {
			if (existing.exited) {
				this.cleanupTerminal(sessionId);
			} else {
				existing.term.resize(Math.max(1, cols), Math.max(1, rows));
				await existing.preview.resize(cols, rows);
				return existing;
			}
		}

		const command = resolveShellCommand();
		const term = pty.spawn(command, [], {
			name: 'xterm-256color',
			cwd: session.cwd,
			env: {...process.env},
			cols: Math.max(1, cols),
			rows: Math.max(1, rows),
		});
		const terminal: RuntimeTerminal = {
			term,
			scrollback: '',
			preview: new TerminalPreview(cols, rows),
			cwd: session.cwd,
			exited: false,
		};
		this.terminals.set(sessionId, terminal);

		term.onData(output => {
			terminal.scrollback = clampScrollback(terminal.scrollback + output);
			void terminal.preview.write(output);
			this.scheduleTerminalBroadcast(sessionId);
			if (terminal.attachedSocket && !terminal.attachedSocket.destroyed) {
				sendMessage(terminal.attachedSocket, {type: 'terminal-output', sessionId, data: output});
			}
		});

		term.onExit(({exitCode, signal}) => {
			terminal.exited = true;
			terminal.exitCode = exitCode ?? null;
			terminal.exitSignal = signal ?? null;
			void this.broadcastTerminal(sessionId);
		});

		this.scheduleTerminalBroadcast(sessionId);
		return terminal;
	}

	private cleanupTerminal(sessionId: string): void {
		const terminal = this.terminals.get(sessionId);
		if (!terminal) {
			return;
		}
		this.terminals.delete(sessionId);
		if (terminal.broadcastTimer) {
			clearTimeout(terminal.broadcastTimer);
		}
		try {
			terminal.term.kill();
		} catch {
			// ignore cleanup errors
		}
		terminal.preview.dispose();
	}

	private async getTerminalRecord(sessionId: string | undefined, cols: number, rows: number): Promise<TerminalRecord> {
		if (!sessionId) {
			return {content: '', live: false};
		}
		const session = this.sessions.get(sessionId);
		if (!session) {
			return {sessionId, content: '', live: false};
		}
		if (session.status === 'exited') {
			return {sessionId, content: '', live: false, cwd: session.cwd};
		}
		if (this.workers.has(sessionId)) {
			return await this.sendWorkerRequest<TerminalRecord>(sessionId, {type: 'snapshot', target: 'terminal', cols, rows});
		}
		const terminal = await this.ensureTerminal(sessionId, cols, rows);
		await terminal.preview.resize(cols, rows);
		await terminal.preview.getSnapshot();
		return this.buildTerminalRecord(sessionId, terminal);
	}

	private buildGitRecord(sessionId: string | undefined, git?: RuntimeTerminal): GitRecord {
		return {
			sessionId,
			content: git?.preview.getCachedSnapshot() ?? '',
			live: Boolean(git && !git.exited),
			cwd: git?.cwd,
			exitCode: git?.exitCode,
			exitSignal: git?.exitSignal,
		};
	}

	private scheduleGitBroadcast(sessionId: string): void {
		const git = this.gits.get(sessionId);
		if (!git || git.broadcastTimer) {
			return;
		}
		git.broadcastTimer = setTimeout(() => {
			git.broadcastTimer = undefined;
			void this.broadcastGit(sessionId);
		}, PREVIEW_BROADCAST_DELAY_MS);
	}

	private async broadcastGit(sessionId: string): Promise<void> {
		const git = this.gits.get(sessionId);
		if (git) {
			await git.preview.getSnapshot();
		}
		for (const [socket, client] of this.clients.entries()) {
			if (client.watchedGitSessionId !== sessionId) {
				continue;
			}
			sendMessage(socket, {type: 'git-updated', git: this.buildGitRecord(sessionId, git)});
		}
	}

	private async ensureGit(sessionId: string, cols: number, rows: number): Promise<RuntimeTerminal> {
		const session = this.sessions.get(sessionId);
		if (!session) {
			throw new Error('session does not exist');
		}
		if (session.status === 'exited') {
			throw new Error('cannot start lazygit for exited session');
		}

		const existing = this.gits.get(sessionId);
		if (existing) {
			if (existing.exited) {
				this.cleanupGit(sessionId);
			} else {
				existing.term.resize(Math.max(1, cols), Math.max(1, rows));
				await existing.preview.resize(cols, rows);
				return existing;
			}
		}

		const command = await resolveLazyGitCommand();
		const term = pty.spawn(command, [], {
			name: 'xterm-256color',
			cwd: session.cwd,
			env: {...process.env},
			cols: Math.max(1, cols),
			rows: Math.max(1, rows),
		});
		const git: RuntimeTerminal = {
			term,
			scrollback: '',
			preview: new TerminalPreview(cols, rows),
			cwd: session.cwd,
			exited: false,
		};
		this.gits.set(sessionId, git);

		term.onData(output => {
			git.scrollback = clampScrollback(git.scrollback + output);
			void git.preview.write(output);
			this.scheduleGitBroadcast(sessionId);
			if (git.attachedSocket && !git.attachedSocket.destroyed) {
				sendMessage(git.attachedSocket, {type: 'git-output', sessionId, data: output});
			}
		});

		term.onExit(({exitCode, signal}) => {
			git.exited = true;
			git.exitCode = exitCode ?? null;
			git.exitSignal = signal ?? null;
			void this.broadcastGit(sessionId);
		});

		this.scheduleGitBroadcast(sessionId);
		return git;
	}

	private cleanupGit(sessionId: string): void {
		const git = this.gits.get(sessionId);
		if (!git) {
			return;
		}
		this.gits.delete(sessionId);
		if (git.broadcastTimer) {
			clearTimeout(git.broadcastTimer);
		}
		try {
			git.term.kill();
		} catch {
			// ignore cleanup errors
		}
		git.preview.dispose();
	}

	private async getGitRecord(sessionId: string | undefined, cols: number, rows: number): Promise<GitRecord> {
		if (!sessionId) {
			return {content: '', live: false};
		}
		const session = this.sessions.get(sessionId);
		if (!session) {
			return {sessionId, content: '', live: false};
		}
		if (session.status === 'exited') {
			return {sessionId, content: '', live: false, cwd: session.cwd};
		}
		if (this.workers.has(sessionId)) {
			return await this.sendWorkerRequest<GitRecord>(sessionId, {type: 'snapshot', target: 'git', cols, rows});
		}
		const git = await this.ensureGit(sessionId, cols, rows);
		await git.preview.resize(cols, rows);
		await git.preview.getSnapshot();
		return this.buildGitRecord(sessionId, git);
	}

	private buildDevRecord(sessionId: string | undefined, dev?: RuntimeTerminal & {command: string}): DevRecord {
		return {
			sessionId,
			content: dev?.preview.getCachedSnapshot() ?? '',
			live: Boolean(dev && !dev.exited),
			cwd: dev?.cwd,
			command: dev?.command,
			exitCode: dev?.exitCode,
			exitSignal: dev?.exitSignal,
		};
	}

	private scheduleDevBroadcast(sessionId: string): void {
		const dev = this.devs.get(sessionId);
		if (!dev || dev.broadcastTimer) return;
		dev.broadcastTimer = setTimeout(() => {
			dev.broadcastTimer = undefined;
			void this.broadcastDev(sessionId);
		}, PREVIEW_BROADCAST_DELAY_MS);
	}

	private async broadcastDev(sessionId: string): Promise<void> {
		const dev = this.devs.get(sessionId);
		if (dev) await dev.preview.getSnapshot();
		const record = this.buildDevRecord(sessionId, dev);
		await this.updateSessionDevRunning(sessionId, record.live);
		for (const [socket, client] of this.clients.entries()) {
			if (client.watchedDevSessionId !== sessionId) continue;
			sendMessage(socket, {type: 'dev-updated', dev: record});
		}
	}

	private async startDev(sessionId: string, cols: number, rows: number): Promise<RuntimeTerminal & {command: string}> {
		const session = this.sessions.get(sessionId);
		if (!session) throw new Error('session does not exist');
		if (session.status === 'exited') throw new Error('cannot start dev command for exited session');

		const existing = this.devs.get(sessionId);
		if (existing) {
			if (existing.exited) this.cleanupDev(sessionId);
			else {
				existing.term.resize(Math.max(1, cols), Math.max(1, rows));
				await existing.preview.resize(cols, rows);
				return existing;
			}
		}

		const config = await loadAppConfig();
		const command = config.dev_command?.trim() || 'dev';
		const shell = resolveShellCommand();
		const term = pty.spawn(shell, ['-ic', command], {
			name: 'xterm-256color',
			cwd: session.cwd,
			env: {...process.env},
			cols: Math.max(1, cols),
			rows: Math.max(1, rows),
		});
		const dev: RuntimeTerminal & {command: string} = {
			term,
			scrollback: '',
			preview: new TerminalPreview(cols, rows),
			cwd: session.cwd,
			exited: false,
			command,
		};
		this.devs.set(sessionId, dev);

		term.onData(output => {
			dev.scrollback = clampScrollback(dev.scrollback + output);
			void dev.preview.write(output);
			this.scheduleDevBroadcast(sessionId);
			if (dev.attachedSocket && !dev.attachedSocket.destroyed) {
				sendMessage(dev.attachedSocket, {type: 'dev-output', sessionId, data: output});
			}
		});

		term.onExit(({exitCode, signal}) => {
			dev.exited = true;
			dev.exitCode = exitCode ?? null;
			dev.exitSignal = signal ?? null;
			void this.broadcastDev(sessionId);
		});

		this.scheduleDevBroadcast(sessionId);
		return dev;
	}

	private cleanupDev(sessionId: string): void {
		const dev = this.devs.get(sessionId);
		if (!dev) return;
		this.devs.delete(sessionId);
		if (dev.broadcastTimer) clearTimeout(dev.broadcastTimer);
		try {
			dev.term.kill();
		} catch {
			// ignore cleanup errors
		}
		dev.preview.dispose();
	}

	private async getDevRecord(sessionId: string | undefined, cols: number, rows: number): Promise<DevRecord> {
		if (!sessionId) return {content: '', live: false};
		const session = this.sessions.get(sessionId);
		if (!session) return {sessionId, content: '', live: false};
		if (session.status === 'exited') return {sessionId, content: '', live: false, cwd: session.cwd};
		if (this.workers.has(sessionId)) {
			return await this.sendWorkerRequest<DevRecord>(sessionId, {type: 'snapshot', target: 'dev', cols, rows});
		}
		const dev = this.devs.get(sessionId);
		if (!dev) return {sessionId, content: '', live: false, cwd: session.cwd};
		await dev.preview.resize(cols, rows);
		await dev.preview.getSnapshot();
		return this.buildDevRecord(sessionId, dev);
	}

	private async createSession(input: CreateSessionInput): Promise<SessionRecord> {
		const title = input.title.trim();
		if (!title) {
			throw new Error('title cannot be empty');
		}
		if (title.length > 64) {
			throw new Error('title cannot be longer than 64 characters');
		}
		const conflict = [...this.sessions.values()].find(
			session => session.repoRoot === input.repoRoot && session.title === title && session.status !== 'exited',
		);
		if (conflict) {
			throw new Error(`an active session named "${title}" already exists in this repo`);
		}

		const command = await resolveProgramCommand(input.program);
		const sessionId = randomUUID();
		const now = new Date().toISOString();
		const baseSession: SessionRecord = {
			id: sessionId,
			title,
			program: input.program,
			command,
			args: [],
			cwd: input.cwd,
			repoRoot: input.repoRoot,
			launchCwd: input.cwd,
			launchWorktreeRoot: input.cwd,
			worktree: {mode: 'none'},
			status: 'starting',
			agentStatus: 'unknown',
			agentStatusUpdatedAt: now,
			createdAt: now,
			updatedAt: now,
			lastPreview: '',
		};

		this.sessions.set(baseSession.id, baseSession);
		this.broadcastSessionUpdated(baseSession);
		void this.persist().catch(error => console.error('failed to persist starting session', error));
		void this.finishCreateSession(baseSession.id, input).catch(error => this.failStartingSession(baseSession.id, error));
		return baseSession;
	}

	private async finishCreateSession(sessionId: string, input: CreateSessionInput): Promise<void> {
		const startingSession = this.sessions.get(sessionId);
		if (!startingSession || startingSession.status !== 'starting') {
			return;
		}

		const title = startingSession.title;
		const launchCwd = input.cwd;
		const launchWorktreeRoot = await findRepoRoot(launchCwd);
		let sessionCwd = launchCwd;
		let worktree: SessionRecord['worktree'] = {mode: 'none'};
		const requestedWorktreeMode = input.worktreeMode ?? 'none';
		if (requestedWorktreeMode === 'new') {
			const created = await createWorktreeForSession(title, launchCwd);
			sessionCwd = created.path;
			worktree = {
				mode: created.origin === 'created' ? 'managed' : 'attached',
				path: created.path,
				branch: created.branch,
				head: created.head,
				isMain: created.isMain,
				origin: created.origin,
				creator: created.creator,
				name: created.name,
			};
		} else if (requestedWorktreeMode === 'existing') {
			if (!input.existingWorktreePath) {
				throw new Error('existing worktree path is required');
			}
			const selectedPath = input.existingWorktreePath;
			const worktrees = await listWorktrees(launchCwd);
			const selected = worktrees.find(item => item.path === selectedPath);
			if (!selected) {
				throw new Error(`selected path is not a git worktree: ${selectedPath}`);
			}
			sessionCwd = selected.path;
			worktree = {
				mode: 'attached',
				path: selected.path,
				branch: selected.branch,
				head: selected.head,
				isMain: selected.isMain,
				origin: 'selected',
				creator: 'picker',
				name: selected.branch || selected.path.split('/').at(-1),
			};
		}

		const agentSessionRef = startingSession.agentSessionRef ?? buildAgentSessionRef(startingSession.program, title, sessionId, sessionCwd);
		const preparedSession: SessionRecord = {
			...startingSession,
			cwd: sessionCwd,
			args: buildAgentArgs({program: startingSession.program, agentSessionRef}, 'create'),
			agentSessionRef,
			launchWorktreeRoot,
			worktree,
			updatedAt: new Date().toISOString(),
		};
		this.sessions.set(sessionId, preparedSession);
		this.broadcastSessionUpdated(preparedSession);

		await prepareAgentSessionRef(preparedSession.agentSessionRef);
		const runningSession = await this.startWorker(preparedSession, input.cols, input.rows);
		this.sessions.set(sessionId, runningSession);
		await this.persist();
		this.broadcastSessionUpdated(runningSession);
	}

	private async failStartingSession(sessionId: string, error: unknown): Promise<void> {
		const session = this.sessions.get(sessionId);
		if (!session || session.status !== 'starting') {
			return;
		}
		const message = error instanceof Error ? error.message : String(error);
		const failedSession: SessionRecord = {
			...session,
			status: 'exited',
			updatedAt: new Date().toISOString(),
			pid: undefined,
			exitCode: null,
			exitSignal: null,
			lastPreview: `Failed to start session: ${message}`,
		};
		this.sessions.set(sessionId, failedSession);
		await this.persist();
		this.broadcastSessionUpdated(failedSession);
	}

	private async restartSession(sessionId: string, cols: number, rows: number): Promise<SessionRecord> {
		const existing = this.sessions.get(sessionId);
		if (!existing) {
			throw new Error('session does not exist');
		}
		if (this.runtime.has(sessionId) || existing.status !== 'exited') {
			throw new Error('session is already running');
		}

		const now = new Date().toISOString();
		const startingAgentSessionRef = existing.agentSessionRef;
		const starting: SessionRecord = {
			...existing,
			args: buildAgentArgs({program: existing.program, agentSessionRef: startingAgentSessionRef}, 'resume'),
			agentSessionRef: startingAgentSessionRef,
			status: 'starting',
			agentStatus: 'unknown',
			agentStatusUpdatedAt: now,
			updatedAt: now,
			pid: undefined,
			exitCode: undefined,
			exitSignal: undefined,
			lastPreview: '',
		};
		this.sessions.set(sessionId, starting);
		await this.persist();
		this.broadcastSessionUpdated(starting);

		try {
			await prepareAgentSessionRef(starting.agentSessionRef);
			const runningSession = await this.startWorker(starting, cols, rows);
			this.sessions.set(sessionId, runningSession);
			await this.persist();
			this.broadcastSessionUpdated(runningSession);
			return runningSession;
		} catch (error) {
			this.sessions.set(sessionId, existing);
			await this.persist();
			this.broadcastSessionUpdated(existing);
			throw error;
		}
	}

	private canDeleteSessionWorktree(session: SessionRecord): {ok: true} | {ok: false; reason: string} {
		const worktree = session.worktree;
		const worktreePath = worktree?.path;
		if (!worktreePath || !worktree || worktree.mode === 'none') {
			return {ok: false, reason: 'session does not have a worktree'};
		}
		if (worktree.isMain) {
			return {ok: false, reason: 'cannot delete the main worktree'};
		}
		if (session.launchWorktreeRoot && path.resolve(worktreePath) === path.resolve(session.launchWorktreeRoot)) {
			return {ok: false, reason: 'cannot delete the current worktree'};
		}
		const other = [...this.sessions.values()].find(candidate => {
			const candidatePath = candidate.worktree?.path;
			return (
				candidate.id !== session.id &&
				candidate.status !== 'exited' &&
				Boolean(candidatePath) &&
				path.resolve(candidatePath!) === path.resolve(worktreePath)
			);
		});
		if (other) {
			return {ok: false, reason: `worktree is in use by session "${other.title}"`};
		}
		return {ok: true};
	}

	private canDeleteSessionBranch(session: SessionRecord): {ok: true} | {ok: false; reason: string} {
		const worktree = session.worktree;
		const branch = worktree?.branch;
		if (!worktree || worktree.mode === 'none') {
			return {ok: false, reason: 'can only delete branches for worktree-backed sessions'};
		}
		if (!branch) {
			return {ok: false, reason: 'worktree is not on a local branch'};
		}
		if (branch === 'main' || branch === 'master') {
			return {ok: false, reason: `refusing to delete protected branch ${branch}`};
		}
		return this.canDeleteSessionWorktree(session);
	}

	private async mergeSessionWorktree(sessionId: string, mode: 'merge' | 'squash', targetCwd: string) {
		const session = this.sessions.get(sessionId);
		if (!session) {
			throw new Error('session does not exist');
		}
		const worktreePath = session.worktree?.path;
		if (!worktreePath || session.worktree?.mode === 'none') {
			throw new Error('session does not have a worktree to merge');
		}
		const result = await mergeWorktreeIntoCurrent(worktreePath, targetCwd, mode);
		if (result.skipped) {
			await this.log(`${mode} merge skipped for ${session.title} (${result.sourceRef}) into ${result.targetBranch}: ${result.reason ?? 'no new commits'}`);
		} else {
			await this.log(`${mode} merged ${session.title} (${result.sourceRef}) into ${result.targetBranch}`);
		}
		return result;
	}

	private async killSession(sessionId: string, deleteWorktree: boolean, deleteBranch: boolean, force: boolean): Promise<void> {
		const session = this.sessions.get(sessionId);
		const worker = this.workers.get(sessionId);
		if (worker) {
			if (!session || session.status === 'exited') throw new Error('session is not running');
			if (deleteBranch) {
				const allowed = this.canDeleteSessionBranch(session);
				if (!allowed.ok) throw new Error(allowed.reason);
				worker.deleteWorktreeOnExit = true;
				worker.deleteBranchOnExit = true;
			} else if (deleteWorktree) {
				const allowed = this.canDeleteSessionWorktree(session);
				if (!allowed.ok) throw new Error(allowed.reason);
				worker.deleteWorktreeOnExit = true;
			}
			await this.sendWorkerRequest(sessionId, {type: 'kill', force});
			return;
		}

		const runtime = this.runtime.get(sessionId);
		if (!session || !runtime) {
			throw new Error('session is not running');
		}
		if (deleteBranch) {
			const allowed = this.canDeleteSessionBranch(session);
			if (!allowed.ok) {
				throw new Error(allowed.reason);
			}
			runtime.deleteWorktreeOnExit = true;
			runtime.deleteBranchOnExit = true;
		} else if (deleteWorktree) {
			const allowed = this.canDeleteSessionWorktree(session);
			if (!allowed.ok) {
				throw new Error(allowed.reason);
			}
			runtime.deleteWorktreeOnExit = true;
		}
		this.cleanupTerminal(sessionId);
		this.cleanupGit(sessionId);
		this.cleanupDev(sessionId);
		signalPtyProcess(runtime.term, 'SIGTERM', force);
		if (force) {
			const forceKillTimer = setTimeout(() => {
				const current = this.runtime.get(sessionId);
				if (current === runtime) {
					signalPtyProcess(runtime.term, 'SIGKILL', true);
				}
			}, 1000);
			forceKillTimer.unref?.();
		}
	}

	private async removeSession(sessionId: string): Promise<void> {
		const existing = this.sessions.get(sessionId);
		if (!existing) {
			throw new Error('session does not exist');
		}
		if (this.runtime.has(sessionId) || this.workers.has(sessionId) || existing.status === 'running') {
			throw new Error('kill the session before removing it');
		}
		this.cleanupTerminal(sessionId);
		this.cleanupGit(sessionId);
		this.cleanupDev(sessionId);
		this.sessions.delete(sessionId);
		await this.persist();
		this.broadcastSessionRemoved(sessionId, existing.repoRoot);
	}

	private async handleSessionExit(sessionId: string, exitCode: number | null, exitSignal: number | null): Promise<void> {
		const existing = this.sessions.get(sessionId);
		if (!existing || existing.status === 'exited') {
			return;
		}
		const runtime = this.runtime.get(sessionId);
		this.runtime.delete(sessionId);
		this.cleanupTerminal(sessionId);
		this.cleanupGit(sessionId);
		this.cleanupDev(sessionId);
		if (runtime) {
			this.clearRuntimeActivityTimers(runtime);
		}
		if (runtime?.previewBroadcastTimer) {
			clearTimeout(runtime.previewBroadcastTimer);
		}
		const now = new Date().toISOString();
		const updated: SessionRecord = {
			...existing,
			status: 'exited',
			agentStatus: 'idle',
			agentStatusUpdatedAt: now,
			updatedAt: now,
			pid: undefined,
			exitCode,
			exitSignal,
			lastPreview: runtime ? await runtime.preview.getSnapshot() : existing.lastPreview,
		};
		this.sessions.set(sessionId, updated);
		if (runtime?.deleteWorktreeOnExit && existing.worktree?.path) {
			try {
				const repoCwd = existing.launchWorktreeRoot ?? existing.repoRoot;
				const branch = existing.worktree.branch;
				await removeWorktree(existing.worktree.path, repoCwd);
				if (runtime.deleteBranchOnExit && branch) {
					await deleteLocalBranch(repoCwd, branch);
				}
			} catch (error) {
				await this.log(`failed to remove worktree/branch for session ${existing.title}: ${error instanceof Error ? error.message : String(error)}`);
			}
		}
		void this.persist();
		runtime?.preview.dispose();
		this.broadcastSessionUpdated(updated);
		for (const [socket, client] of this.clients.entries()) {
			if (client.watchedPreviewSessionId === sessionId) {
				sendMessage(socket, {type: 'preview-updated', preview: this.buildPreviewRecord(updated, updated.lastPreview ?? '')});
			}
		}
		if (runtime?.attachedSocket && !runtime.attachedSocket.destroyed) {
			sendMessage(runtime.attachedSocket, {type: 'session-updated', session: updated});
		}
	}

	private async persist(): Promise<void> {
		await saveSessions(sortSessionsNewestFirst([...this.sessions.values()]));
	}
}
