import fs from 'node:fs/promises';
import {constants as fsConstants} from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import {execFile, fork, spawn, type ChildProcess} from 'node:child_process';
import {promisify} from 'node:util';
import {randomUUID} from 'node:crypto';
import {StringDecoder} from 'node:string_decoder';
import pty, {type IPty} from 'node-pty';
import {getConfigDir, getCliEntryPath, getDaemonLogPath, getDaemonPidPath, getSocketPath, getWorkerDir, getWorkerLogPath, getWorkerPidPath} from './paths.js';
import {createWorktreeForSession, currentBranch, deleteLocalBranch, findGitCommonDir, findRepoRoot, headSha, listWorktrees, mergeWorktreeIntoCurrent, removeWorktree, sanitizeWorktreeName} from './git.js';
import {ensureNodePtyReady} from './nodePty.js';
import {ensureConfigDir, loadAppConfig, type AppConfig, markAllNonExitedSessionsExited, saveSessions, sortSessionsNewestFirst, updateAppConfig} from './storage.js';
import {compareSessionOrder, sortSessionsForSidebar} from './sessionOrder.js';
import {isPathInside, sessionMatchesScope} from './sessionScope.js';
import {errorMessage} from './ui.js';
import {TerminalPreview} from './terminalPreview.js';
import type {WorktreeSettings} from './worktreeLinks.js';
import {loadProjectConfig, isProjectTrusted, readEffectiveSettings, projectNeedsReview, resolveDevCommand, resolveSettings, resolveSetupCommand, trustProjectConfig, type LoadedProject} from './projectConfig.js';
import {readConfigTargets, saveGlobalDefaultsDocument, saveProjectConfigDocument} from './projectConfigDocument.js';
import {readCandidateSizes, readWorktreeSetupInfo} from './worktreeSetup.js';
import {createPullRequest, getHandoffGitContext, getWorkspaceSummary, inspectWorkspaceCleanup, type WorkspaceSummary, type CleanupInspection} from './workspaceGit.js';
import {normalizeHook, integrationArgs, codexResumeFromOutput, needsAttention} from './agentSignals.js';
import {exportHandoff} from './sessionFeatures.js';
import {PROTOCOL_VERSION} from './types.js';
import type {AgentActivityStatus, AgentSessionRef, AttachTarget, ClientRequest, CreateSessionInput, DevRecord, GitRecord, PreviewRecord, ProjectInfo, RestartMode, ServerMessage, ServerResponse, SessionCleanupInspection, SessionRecord, TerminalRecord} from './types.js';

const execFileAsync = promisify(execFile);
const DEFAULT_PREVIEW_COLS = 80;
const DEFAULT_PREVIEW_ROWS = 24;
const PREVIEW_BROADCAST_DELAY_MS = 75;
const WORKER_REQUEST_TIMEOUT_MS = 10_000;
const WORKER_KILL_GRACE_MS = 2000;
const MAX_REQUEST_LINE = 256_000;
const SETUP_OUTPUT_LIVE_LIMIT = 16_000;
const SETUP_OUTPUT_STORED_LIMIT = 4096;
const SUMMARY_TTL_MS = 4000;

// Daemon-local panes serve sessions that are still starting (no worker yet); they are
// disposed when a worker takes over or the start fails/is cancelled.
interface RuntimeTerminal {
	term: IPty;
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
	previewScrollOffset: number;
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
	hookToken: string;
	launchId: string;
	allowDataLoss?: boolean;
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

// Effective settings may throw for invalid global defaults; that surfaces wherever they are used.
function projectInfo(project: LoadedProject, config: AppConfig): ProjectInfo {
	return {...project, trusted: isProjectTrusted(project, config), needsReview: projectNeedsReview(project, config), effective: resolveSettings(project, config)};
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

function buildDeckhandAgentName(title: string, sessionId: string, suffix?: string): string {
	const safeTitle = sanitizeWorktreeName(title)
		.replace(/_*\/+_*/g, '-')
		.slice(0, 40)
		.replace(/^[-_]+|[-_]+$/g, '') || 'session';
	return `dh-${safeTitle}-${sessionId.slice(0, 8)}${suffix ? `-${suffix}` : ''}`;
}

// Claude and Pi accept an exact conversation ID chosen at launch, so resume never depends on
// name lookup (ambiguous after renames, duplicates or forks) or on Pi's private file layout.
function buildAgentSessionRef(program: SessionRecord['program']): AgentSessionRef | undefined {
	return program === 'claude' || program === 'pi' ? {provider: program, kind: 'id', value: randomUUID()} : undefined;
}

function truncateSessionTitle(value: string, maxLength: number): string {
	if (value.length <= maxLength) {
		return value;
	}
	if (maxLength <= 1) {
		return value.slice(0, Math.max(0, maxLength));
	}
	return `${value.slice(0, maxLength - 1).trimEnd()}…`;
}

function inheritedChildTitle(parentTitle: string, childTitle: string): string {
	const normalizedParent = parentTitle.trim().replace(/\s+/g, ' ');
	const normalizedChild = childTitle.trim().replace(/\s+/g, ' ');
	if (!normalizedParent || normalizedChild === normalizedParent || normalizedChild.startsWith(`${normalizedParent} / `)) {
		return truncateSessionTitle(normalizedChild, 64);
	}
	const separator = ' / ';
	if (normalizedParent.length + separator.length + normalizedChild.length <= 64) {
		return `${normalizedParent}${separator}${normalizedChild}`;
	}
	const minPrefixLength = Math.min(12, normalizedParent.length);
	const maxChildLength = Math.max(1, 64 - separator.length - minPrefixLength);
	const child = truncateSessionTitle(normalizedChild, maxChildLength);
	const prefix = truncateSessionTitle(normalizedParent, Math.max(1, 64 - separator.length - child.length));
	return `${prefix}${separator}${child}`;
}

function supportsForkedSubSession(program: SessionRecord['program']): boolean {
	return program === 'claude' || program === 'pi';
}

function branchCommandInput(name: string): string {
	// Claude Code users may be in vim normal mode. `a` enters insert mode, and
	// backspace removes the inserted `a` when already in insert mode.
	return `a\x7f/branch ${name}\r`;
}

// `name` labels a new conversation; `forkFrom` (create mode) makes Pi copy that session into the new ID before
// its TUI starts. Claude forks resume the parent and type /branch instead (see branchCommandInput).
function buildAgentArgs(session: Pick<SessionRecord, 'program' | 'agentSessionRef'>, mode: 'create' | 'resume', name?: string, forkFrom?: AgentSessionRef): string[] {
	const ref = session.agentSessionRef;
	if (!ref) {
		return [];
	}
	const label = mode === 'create' && name ? ['--name', name] : [];
	if (session.program === 'claude' && (ref.kind === 'name' || ref.kind === 'id')) {
		// Never --session-id on resume: Claude rejects it with --resume and refuses an ID already in use.
		if (mode === 'resume') return ['--resume', ref.value];
		return ref.kind === 'id' ? ['--session-id', ref.value, ...label] : ['--name', ref.value];
	}
	if (session.program === 'pi' && ref.kind === 'id') {
		return [...(forkFrom ? ['--fork', forkFrom.value] : []), '--session-id', ref.value, ...label];
	}
	if (session.program === 'pi' && ref.kind === 'path') {
		return ['--session', ref.value];
	}
	if (session.program === 'codex' && ref.kind === 'id' && mode === 'resume') return ['resume', ref.value];
	return [];
}

function sameAgentSessionRef(left: AgentSessionRef | undefined, right: AgentSessionRef | undefined): boolean {
	return Boolean(left && right && left.provider === right.provider && left.kind === right.kind && left.value === right.value);
}

const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

function parseClaudeResumeRef(output: string): AgentSessionRef | undefined {
	const text = output.replace(/\x1b\[[0-9;?]*[ -/]*[@-~]/g, '');
	const match = text.match(/(?:^|\n)\s*claude\s+--resume(?:=|\s+)(?:"([^"]+)"|'([^']+)'|(\S+))/i);
	const value = (match?.[1] ?? match?.[2] ?? match?.[3])?.trim();
	return value ? {provider: 'claude', kind: UUID_PATTERN.test(value) ? 'id' : 'name', value} : undefined;
}

// Claude prints this and exits when `--resume <uuid>` names no saved conversation.
function missingClaudeConversation(session: SessionRecord, output: string | undefined): string | undefined {
	if (session.program !== 'claude' || !output) return undefined;
	return output.replace(/\x1b\[[0-9;?]*[ -/]*[@-~]/g, '').match(/No conversation found with session ID:\s*([0-9a-f-]{36})/i)?.[1];
}

function refFromExitOutput(session: SessionRecord, output: string): AgentSessionRef | undefined {
	if (session.program === 'claude') {
		return parseClaudeResumeRef(output);
	}
	if (session.program === 'codex') return codexResumeFromOutput(output);
	// `pi --fork` exits before creating the child when the parent has no saved session yet; forget the
	// unused child ID so the next resume forks again instead of opening an empty conversation.
	if (session.program === 'pi' && session.subSessionKind === 'forked' && /No session found matching/.test(output)) return session.forkedFromAgentSessionRef;
	return undefined;
}

function restartRefForSession(session: SessionRecord): {ref: AgentSessionRef | undefined; shouldForkParent: boolean} {
	if (session.subSessionKind !== 'forked') {
		return {ref: session.agentSessionRef, shouldForkParent: false};
	}
	if (session.agentSessionRef && !sameAgentSessionRef(session.agentSessionRef, session.forkedFromAgentSessionRef)) {
		return {ref: session.agentSessionRef, shouldForkParent: false};
	}
	return {ref: session.forkedFromAgentSessionRef ?? session.agentSessionRef, shouldForkParent: true};
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

function clampSize(value: number, fallback: number): number {
	if (!Number.isFinite(value)) {
		return fallback;
	}
	return Math.max(1, Math.floor(value));
}

function clampNonNegative(value: number): number {
	if (!Number.isFinite(value)) {
		return 0;
	}
	return Math.max(0, Math.floor(value));
}

function handoffPrompt(handoffPath: string): string {
	return `Read the handoff document at ${handoffPath}. Use it as task context, inspect the workspace, and continue the work described there. Keep all normal permission checks.`;
}

async function realpathOrResolve(target: string): Promise<string> {
	return fs.realpath(target).catch(() => path.resolve(target));
}

export class InkDaemon {
	private readonly sessions = new Map<string, SessionRecord>();
	private readonly terminals = new Map<string, RuntimeTerminal>();
	private readonly gits = new Map<string, RuntimeTerminal>();
	private readonly devs = new Map<string, RuntimeTerminal & {command: string}>();
	private readonly gitStartPromises = new Map<string, Promise<RuntimeTerminal>>();
	private readonly workers = new Map<string, WorkerRuntime>();
	private readonly clients = new Map<net.Socket, ClientSubscription>();
	private server?: net.Server;
	private shuttingDown = false;
	private persistInFlight: Promise<void> = Promise.resolve();
	private persistQueued?: Promise<void>;
	private readonly setupProcesses = new Map<string, ChildProcess>();
	private readonly cleanupWorktrees = new Set<string>();
	private readonly preparingSessions = new Set<string>();
	private readonly summaries = new Map<string, {key: string; at: number; result: Promise<WorkspaceSummary>}>();

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
		await fs.chmod(getSocketPath(), 0o600);
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
		// The socket file is created at bind time; a 0o077 umask keeps it private before the
		// explicit chmod, so no other local user can connect in between.
		const previousUmask = process.umask(0o077);
		try {
			await new Promise<void>((resolve, reject) => {
				this.server?.once('error', reject);
				this.server?.listen(getSocketPath(), () => resolve());
			});
		} finally {
			process.umask(previousUmask);
		}
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
		for (const child of this.setupProcesses.values()) { try { if (child.pid) process.kill(-child.pid, 'SIGTERM'); } catch {} }
		await this.log('cleanup start');
		for (const socket of this.clients.keys()) {
			socket.destroy();
		}
		this.clients.clear();

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

		// Detach workers first so their exit events are ignored, then wait briefly for
		// each to exit before SIGKILL; process.exit follows cleanup, so the fallback must not be deferred.
		const workers = [...this.workers.entries()];
		this.workers.clear();
		await Promise.all(workers.map(async ([sessionId, worker]) => {
			for (const pending of worker.pending.values()) {
				clearTimeout(pending.timer);
				pending.reject(new Error('daemon shutting down'));
			}
			worker.pending.clear();
			const exited = new Promise<void>(resolve => {
				if (worker.process.exitCode !== null || worker.process.signalCode !== null) return resolve();
				const timer = setTimeout(() => { try { worker.process.kill('SIGKILL'); } catch {} resolve(); }, 500);
				worker.process.once('exit', () => { clearTimeout(timer); resolve(); });
			});
			try {
				if (worker.process.connected) worker.process.send?.({type: 'kill', requestId: randomUUID(), force: true});
			} catch {
				// Fall through to direct worker termination.
			}
			await exited;
			await fs.rm(getWorkerPidPath(sessionId), {force: true}).catch(() => {});
		}));
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
			previewScrollOffset: 0,
			terminalCols: DEFAULT_PREVIEW_COLS,
			terminalRows: DEFAULT_PREVIEW_ROWS,
			gitCols: DEFAULT_PREVIEW_COLS,
			gitRows: DEFAULT_PREVIEW_ROWS,
			devCols: DEFAULT_PREVIEW_COLS,
			devRows: DEFAULT_PREVIEW_ROWS,
		});

		let buffer = '';
		const decoder = new StringDecoder('utf8');
		let attachedSessionId: string | undefined;
		// Head of an oversized line whose remainder is being discarded up to its newline.
		let oversizedHead: string | undefined;

		const cleanup = () => {
			if (attachedSessionId) {
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
			buffer += decoder.write(chunk);
			while (true) {
				const newlineIndex = buffer.indexOf('\n');
				if (oversizedHead !== undefined) {
					if (newlineIndex === -1) { buffer = buffer.slice(-4096); break; }
					this.rejectOversizedRequest(socket, `${oversizedHead}\n${buffer.slice(Math.max(0, newlineIndex - 4096), newlineIndex)}`);
					oversizedHead = undefined;
					buffer = buffer.slice(newlineIndex + 1);
					continue;
				}
				if (newlineIndex === -1) {
					if (buffer.length > MAX_REQUEST_LINE) { oversizedHead = buffer.slice(0, 4096); buffer = buffer.slice(-4096); }
					break;
				}
				const line = buffer.slice(0, newlineIndex).trim();
				buffer = buffer.slice(newlineIndex + 1);
				if (!line) {
					continue;
				}
				// The connection is multiplexed (UI + attach); one oversized line is rejected, not fatal.
				if (line.length > MAX_REQUEST_LINE) { this.rejectOversizedRequest(socket, `${line.slice(0, 4096)}\n${line.slice(-4096)}`); continue; }
				let message: ClientRequest;
				try { message = JSON.parse(line) as ClientRequest; if (!message || typeof message.type !== 'string') throw new Error('invalid request'); }
				catch { void this.log('closing client after malformed request'); socket.destroy(); return; }
				void this.handleRequest(socket, message, sessionId => {
					attachedSessionId = sessionId;
				});
			}
		});

		socket.on('close', cleanup);
		socket.on('error', cleanup);
	}

	private rejectOversizedRequest(socket: net.Socket, excerpt: string): void {
		const requestId = excerpt.match(/"requestId":"([\w-]{1,128})"/)?.[1];
		void this.log(`rejected oversized client message (>${MAX_REQUEST_LINE} chars)${requestId ? ` requestId=${requestId}` : ''}`);
		if (requestId) sendMessage(socket, failure(requestId, `Request exceeds ${MAX_REQUEST_LINE} characters`));
	}

	private getClient(socket: net.Socket): ClientSubscription {
		const client = this.clients.get(socket);
		if (!client) {
			const created: ClientSubscription = {
				previewCols: DEFAULT_PREVIEW_COLS,
				previewRows: DEFAULT_PREVIEW_ROWS,
				previewScrollOffset: 0,
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
					sendMessage(socket, response(message.requestId, {ok: true, version: PROTOCOL_VERSION, home: getConfigDir(), channel: process.env.DECKHAND_CHANNEL ?? 'stable'}));
					return;
				case 'shutdown': {
					if (process.env.DECKHAND_CHANNEL !== 'dev') throw new Error('This control is restricted to the isolated dev daemon');
					sendMessage(socket, response(message.requestId, {ok: true}));
					setTimeout(() => { void this.cleanup().finally(() => process.exit(0)); }, 25); return;
				}
				case 'config-targets': {
					sendMessage(socket, response(message.requestId, await readConfigTargets(message.cwd))); return;
				}
				case 'save-config': {
					// Saving never runs anything or grants trust.
					const saved = message.target === 'global' ? await saveGlobalDefaultsDocument(message.raw, message.revision) : await saveProjectConfigDocument(message.cwd, message.raw, message.revision);
					sendMessage(socket, response(message.requestId, saved)); return;
				}
				case 'worktree-setup-info': sendMessage(socket, response(message.requestId, await readWorktreeSetupInfo(message.cwd))); return;
				case 'effective-settings': sendMessage(socket, response(message.requestId, await readEffectiveSettings(message.cwd, await loadAppConfig()))); return;
				case 'worktree-candidate-sizes': sendMessage(socket, response(message.requestId, await readCandidateSizes(message.cwd, message.paths))); return;
				case 'project-info': {
					const config = await loadAppConfig();
					sendMessage(socket, response(message.requestId, projectInfo(await loadProjectConfig(message.cwd, config), config))); return;
				}
				case 'trust-project': {
					const project = await loadProjectConfig(message.cwd, await loadAppConfig());
					if (project.fingerprint !== message.fingerprint) throw new Error('Repository configuration changed; review it again before trusting');
					const config = await updateAppConfig(current => trustProjectConfig(project, current));
					sendMessage(socket, response(message.requestId, projectInfo(project, config))); return;
				}
				case 'workspace-summary': {
					const session = this.requireSession(message.sessionId);
					const now = Date.now();
					for (const [id, entry] of this.summaries) if (now - entry.at > SUMMARY_TTL_MS * 15 || !this.sessions.has(id.split('\0')[0]!)) this.summaries.delete(id);
					const slot = `${session.id}\0${message.includePr ? 'pr' : ''}`;
					const key = JSON.stringify([session.cwd, session.worktree?.baseRef]);
					let cached = this.summaries.get(slot);
					if (!cached || cached.key !== key || now - cached.at > SUMMARY_TTL_MS) {
						const entry = {key, at: now, result: getWorkspaceSummary(session.cwd, session.worktree?.baseRef, message.includePr)};
						this.summaries.set(slot, entry);
						entry.result.catch(() => { if (this.summaries.get(slot) === entry) this.summaries.delete(slot); });
						cached = entry;
					}
					sendMessage(socket, response(message.requestId, await cached.result)); return;
				}
				case 'create-pr': {
					// Outward-facing: the UI confirms first. Pushes (never forced) and opens GitHub's PR form via gh.
					const session = this.requireSession(message.sessionId);
					const worktree = session.worktree;
					if (!worktree || worktree.mode === 'none' || !worktree.branch || worktree.deletedAt) throw new Error('Create PR needs a session in a worktree on a branch');
					try { sendMessage(socket, response(message.requestId, await createPullRequest(session.cwd, {baseRef: worktree.baseRef, expectedBranch: message.branch}))); }
					finally { for (const slot of this.summaries.keys()) if (slot.startsWith(`${session.id}\0`)) this.summaries.delete(slot); }
					return;
				}
				case 'inspect-cleanup': sendMessage(socket, response(message.requestId, await this.inspectSessionCleanup(message.sessionId, message.deleteBranch ?? true))); return;
				case 'archive-session': {
					const session = this.requireSession(message.sessionId);
					sendMessage(socket, response(message.requestId, await this.saveSession({...session, archivedAt: message.archived ? new Date().toISOString() : undefined}))); return;
				}
				case 'export-handoff': {
					const handoffPath = await this.exportSessionHandoff(this.requireSession(message.sessionId), message.includeOutput);
					await this.saveSession({...this.requireSession(message.sessionId), handoffPath});
					sendMessage(socket, response(message.requestId, handoffPath)); return;
				}
				case 'agent-hook': await this.handleAgentHook(message); sendMessage(socket, response(message.requestId, {ok: true})); return;
				case 'cancel-start': {
					const session = this.requireSession(message.sessionId);
					if (session.status !== 'starting') throw new Error('Session is not starting');
					const child = this.setupProcesses.get(session.id);
					if (child?.pid) { try { process.kill(-child.pid, 'SIGTERM'); } catch {} }
					if (this.workers.has(session.id)) this.sendWorkerEvent(session.id, {type: 'kill', requestId: randomUUID(), force: true});
					this.cleanupLocalPanes(session.id);
					// Only an in-flight setup is cancelled; pending/failed setup stays retryable as-is.
					const setupRunning = Boolean(child) || session.setup?.state === 'running';
					await this.saveSession({...session, status: 'exited', exitReason: 'stopped', lastPreview: setupRunning ? 'Setup cancelled; worktree retained.' : 'Startup cancelled. In-flight worktree preparation may finish, but the agent will not launch.', setup: setupRunning && session.setup ? {...session.setup, state: 'cancelled', output: session.setup.output.slice(-SETUP_OUTPUT_STORED_LIMIT)} : session.setup});
					sendMessage(socket, response(message.requestId, {ok: true})); return;
				}
				case 'run-action': {
					const session = this.requireSession(message.sessionId);
					if (session.status !== 'running') throw new Error('Start the agent session before running actions');
					// Untrusted repository actions are never run, only global defaults' actions.
					const config = await loadAppConfig(), project = await loadProjectConfig(session.cwd, config);
					const actions = resolveSettings(project, config).actions ?? {};
					const command = Object.hasOwn(actions, message.action) ? actions[message.action] : undefined;
					if (!command && Object.hasOwn(project.config.actions ?? {}, message.action)) throw new Error('Review and trust deckhand.json first (press e or T)');
					if (!command) throw new Error('Unknown project action');
					if (session.devRunning) throw new Error('Stop the current Dev/action command before starting another');
					sendMessage(socket, response(message.requestId, await this.sendWorkerRequest<DevRecord>(session.id, {type: 'start-dev', command, cols: message.cols, rows: message.rows}))); return;
				}
				case 'list':
					sendMessage(socket, response(message.requestId, sortSessionsForSidebar([...this.sessions.values()])));
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
					client.previewScrollOffset = clampNonNegative(message.scrollOffset ?? 0);
					const preview = await this.getPreviewRecord(message.sessionId, client.previewCols, client.previewRows, client.previewScrollOffset);
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
						const command = await this.resolveSessionDevCommand(this.requireSession(message.sessionId));
						sendMessage(socket, response(message.requestId, await this.sendWorkerRequest<DevRecord>(message.sessionId, {type: 'start-dev', command, cols: clampSize(message.cols, DEFAULT_PREVIEW_COLS), rows: clampSize(message.rows, DEFAULT_PREVIEW_ROWS)})));
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
				case 'update-session-notes':
					sendMessage(socket, response(message.requestId, await this.updateSessionNotes(message.sessionId, message.notes)));
					return;
				case 'create': {
					const session = await this.createSession(message.input);
					sendMessage(socket, response(message.requestId, session));
					return;
				}
				case 'reorder-session':
					sendMessage(socket, response(message.requestId, await this.reorderSession(message.sessionId, message.direction)));
					return;
				case 'restart': {
					const session = await this.restartSession(message.sessionId, message.cols, message.rows, message.mode ?? 'resume', message.projectFingerprint);
					sendMessage(socket, response(message.requestId, session));
					return;
				}
				case 'kill':
					await this.killSession(message.sessionId, message.deleteWorktree ?? false, message.deleteBranch ?? false, message.force ?? false, message.allowDataLoss ?? false);
					sendMessage(socket, response(message.requestId, {ok: true}));
					return;
				case 'merge-worktree':
					sendMessage(socket, response(message.requestId, await this.mergeSessionWorktree(message.sessionId, message.mode, message.targetCwd)));
					return;
				case 'mark-session-merged':
					sendMessage(socket, response(message.requestId, await this.markSessionMerged(message.sessionId, message.targetCwd)));
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
						const cols = clampSize(message.cols ?? DEFAULT_PREVIEW_COLS, DEFAULT_PREVIEW_COLS);
						const rows = clampSize(message.rows ?? DEFAULT_PREVIEW_ROWS, DEFAULT_PREVIEW_ROWS);
						const attachData = await this.sendWorkerRequest<object & {initialFrame?: string}>(message.sessionId, {type: 'attach', target: 'agent', cols, rows});
						worker.attached.agent = socket;
						setAttachedSessionId(session.id);
						sendMessage(socket, response(message.requestId, {...session, ...attachData}));
						sendMessage(socket, {type: 'attached', sessionId: session.id});
						if (attachData.initialFrame) sendMessage(socket, {type: 'output', sessionId: session.id, data: attachData.initialFrame});
						return;
					}
					throw new Error('session is not running');
				}
				case 'input':
					this.sendWorkerEvent(message.sessionId, {type: 'input', target: 'agent', data: message.data});
					return;
				case 'resize':
					this.sendWorkerEvent(message.sessionId, {type: 'resize', target: 'agent', cols: message.cols, rows: message.rows});
					return;
				case 'detach': {
					const worker = this.workers.get(message.sessionId);
					if (worker) {
						if (worker.attached.agent === socket) worker.attached.agent = undefined;
						this.sendWorkerEvent(message.sessionId, {type: 'detach', target: 'agent'});
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
						const attachData = await this.sendWorkerRequest<object & {initialFrame?: string}>(message.sessionId, {type: 'attach', target: 'terminal', cols: clampSize(message.cols ?? DEFAULT_PREVIEW_COLS, DEFAULT_PREVIEW_COLS), rows: clampSize(message.rows ?? DEFAULT_PREVIEW_ROWS, DEFAULT_PREVIEW_ROWS)});
						worker.attached.terminal = socket;
						setAttachedSessionId(message.sessionId);
						sendMessage(socket, response(message.requestId, {...session, ...attachData}));
						sendMessage(socket, {type: 'terminal-attached', sessionId: message.sessionId});
						if (attachData.initialFrame) sendMessage(socket, {type: 'terminal-output', sessionId: message.sessionId, data: attachData.initialFrame});
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
						const attachData = await this.sendWorkerRequest<object & {initialFrame?: string}>(message.sessionId, {type: 'attach', target: 'git', cols: clampSize(message.cols ?? DEFAULT_PREVIEW_COLS, DEFAULT_PREVIEW_COLS), rows: clampSize(message.rows ?? DEFAULT_PREVIEW_ROWS, DEFAULT_PREVIEW_ROWS)});
						worker.attached.git = socket;
						setAttachedSessionId(message.sessionId);
						sendMessage(socket, response(message.requestId, {...session, ...attachData}));
						sendMessage(socket, {type: 'git-attached', sessionId: message.sessionId});
						if (attachData.initialFrame) sendMessage(socket, {type: 'git-output', sessionId: message.sessionId, data: attachData.initialFrame});
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
						const attachData = await this.sendWorkerRequest<object & {initialFrame?: string}>(message.sessionId, {type: 'attach', target: 'dev', cols: clampSize(message.cols ?? DEFAULT_PREVIEW_COLS, DEFAULT_PREVIEW_COLS), rows: clampSize(message.rows ?? DEFAULT_PREVIEW_ROWS, DEFAULT_PREVIEW_ROWS)});
						worker.attached.dev = socket;
						setAttachedSessionId(message.sessionId);
						sendMessage(socket, response(message.requestId, {...session, ...attachData}));
						sendMessage(socket, {type: 'dev-attached', sessionId: message.sessionId});
						if (attachData.initialFrame) sendMessage(socket, {type: 'dev-output', sessionId: message.sessionId, data: attachData.initialFrame});
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

	private requireSession(sessionId: string): SessionRecord {
		const session = this.sessions.get(sessionId);
		if (!session) throw new Error('Session does not exist');
		return session;
	}

	private async saveSession(session: SessionRecord): Promise<SessionRecord> {
		this.sessions.set(session.id, session);
		await this.persist();
		this.broadcastSessionUpdated(session);
		return session;
	}

	// Merges onto the latest record so work that awaited in between never writes back a stale copy.
	private patchSession(sessionId: string, patch: Partial<SessionRecord>): SessionRecord | undefined {
		const current = this.sessions.get(sessionId);
		if (!current) return undefined;
		const updated = {...current, ...patch};
		this.sessions.set(sessionId, updated);
		return updated;
	}

	private isCurrentLaunch(sessionId: string, launchId: string | undefined): boolean {
		const current = this.sessions.get(sessionId);
		return Boolean(current && current.status === 'starting' && current.launchId === launchId);
	}

	private assertCurrentLaunch(sessionId: string, launchId: string | undefined): void {
		if (!this.isCurrentLaunch(sessionId, launchId)) throw new Error('Startup cancelled');
	}

	// Reasons no DELETE/allowDataLoss override may bypass. The single source for both the
	// inspection shown to the user and enforcement at kill/exit time.
	private async cleanupBlockers(session: SessionRecord, deleteBranch: boolean): Promise<string[]> {
		const worktree = session.worktree;
		const worktreePath = worktree && worktree.mode !== 'none' ? worktree.path : undefined;
		if (!worktree || !worktreePath) return ['session does not have a worktree'];
		if (worktree.deletedAt) return ['worktree was already deleted'];
		if (worktree.isMain) return ['cannot delete the main worktree'];
		const blockers: string[] = [];
		const target = await realpathOrResolve(worktreePath);
		if (session.launchWorktreeRoot && target === await realpathOrResolve(session.launchWorktreeRoot)) blockers.push('cannot delete the current worktree');
		if (deleteBranch && !worktree.branch) blockers.push('worktree is not on a local branch');
		else if (deleteBranch && (worktree.branch === 'main' || worktree.branch === 'master')) blockers.push(`refusing to delete protected branch ${worktree.branch}`);
		for (const other of this.sessions.values()) {
			if (other.id === session.id || other.status === 'exited') continue;
			const otherWorktree = other.worktree?.path ? await realpathOrResolve(other.worktree.path) : undefined;
			if (otherWorktree === target || isPathInside(target, await realpathOrResolve(other.cwd))) { blockers.push(`worktree is in use by session "${other.title}"`); break; }
		}
		try {
			let registered: Awaited<ReturnType<typeof listWorktrees>>[number] | undefined;
			for (const item of await listWorktrees(session.launchWorktreeRoot ?? session.repoRoot)) if (await realpathOrResolve(item.path) === target) { registered = item; break; }
			if (!registered || registered.isMain) blockers.push('Worktree is missing or is the main checkout');
			else if (deleteBranch && registered.branch !== worktree.branch) blockers.push('Worktree branch changed; refresh before deletion');
		} catch (error) {
			blockers.push(`Worktree registration could not be verified: ${errorMessage(error)}`);
		}
		return blockers;
	}

	// Git context is gathered at export time, bounded and fail-soft; a deleted worktree has none.
	private async exportSessionHandoff(session: SessionRecord, includeOutput = false): Promise<string> {
		const gitContext = session.worktree?.deletedAt ? undefined : await getHandoffGitContext(session.cwd, session.worktree?.baseRef).catch(error => ({commits: [], moreCommits: 0, changes: [], moreChanges: 0, error: errorMessage(error)}));
		return exportHandoff(session, includeOutput, gitContext);
	}

	private async inspectSessionCleanup(sessionId: string, deleteBranch: boolean): Promise<SessionCleanupInspection> {
		const worktree = this.requireSession(sessionId).worktree;
		let inspection: CleanupInspection = {safe: false, reasons: [], dirtyFiles: 0, untrackedFiles: 0, ignoredFiles: 0};
		if (worktree?.path && worktree.mode !== 'none' && !worktree.deletedAt) {
			// A failed inspection is a data-loss reason (overridable), never a silent pass.
			try { inspection = await inspectWorkspaceCleanup(worktree.path, worktree.baseRef, {deleteBranch}); }
			catch (error) { inspection = {...inspection, reasons: [`Workspace safety could not be verified: ${errorMessage(error)}`]}; }
		}
		// Structural checks run after the slow git inspection, against the latest record.
		const structuralBlockers = await this.cleanupBlockers(this.requireSession(sessionId), deleteBranch);
		return {...inspection, safe: inspection.safe && structuralBlockers.length === 0, structuralBlockers};
	}

	private async assertCleanupAllowed(sessionId: string, deleteBranch: boolean, allowDataLoss: boolean): Promise<void> {
		const {structuralBlockers, safe, reasons} = await this.inspectSessionCleanup(sessionId, deleteBranch);
		if (structuralBlockers.length) throw new Error(structuralBlockers.join('; '));
		if (!safe && !allowDataLoss) throw new Error(`Deletion blocked: ${reasons.join('; ')}. An explicit data-loss override is required.`);
	}

	private async handleAgentHook(message: Extract<ClientRequest, {type: 'agent-hook'}>): Promise<void> {
		const worker = this.workers.get(message.sessionId);
		const session = this.requireSession(message.sessionId);
		if (!worker || worker.hookToken !== message.token || worker.launchId !== message.launchId || session.launchId !== message.launchId || session.status === 'exited') throw new Error('Stale or unauthorized lifecycle callback');
		const signal = normalizeHook(session.program, message.payload);
		if (!signal) return;
		const nativeRef = this.acceptedNativeRef(session, signal.nativeRef, signal.event);
		const attention = {state: signal.state, event: signal.event, at: new Date().toISOString()};
		if (!nativeRef && session.attention?.state === signal.state) {
			// Tool-use events repeat constantly without changing anything visible; keep them off disk and the wire.
			this.sessions.set(session.id, {...session, attention});
			return;
		}
		const updated = await this.saveSession({...session, ...(nativeRef ? {agentSessionRef: nativeRef} : {}), attention});
		if (needsAttention(signal.state) && session.attention?.state !== signal.state) void this.notify(updated, signal.state);
	}

	private acceptedNativeRef(session: SessionRecord, ref: AgentSessionRef | undefined, event: string): AgentSessionRef | undefined {
		if (!ref || sameAgentSessionRef(ref, session.agentSessionRef)) return undefined;
		// Identity is established at SessionStart; later events may only fill in a missing ref.
		if (event !== 'SessionStart' && session.agentSessionRef) return undefined;
		// Deckhand chose this Claude conversation's ID at launch; another ID (e.g. after /clear) is not its identity.
		if (session.program === 'claude' && session.agentSessionRef?.kind === 'id' && session.subSessionKind !== 'forked') return undefined;
		if (session.subSessionKind === 'forked') {
			// A fork first runs as its parent before branching; never adopt the parent's identity.
			const parent = session.forkedFromSessionId ? this.sessions.get(session.forkedFromSessionId) : undefined;
			if (ref.value === session.forkedFromAgentSessionRef?.value || ref.value === parent?.agentSessionRef?.value) return undefined;
		}
		return ref;
	}

	private async notify(session: SessionRecord, message: string): Promise<void> {
		try {
			if (!(await loadAppConfig()).notifications) return;
			if (process.platform === 'darwin') await execFileAsync('osascript', ['-e', 'on run argv\ndisplay notification (item 2 of argv) with title (item 1 of argv)\nend run', `Deckhand: ${session.title}`, message], {timeout: 2000});
			else await execFileAsync('notify-send', ['--', `Deckhand: ${session.title}`, message], {timeout: 2000});
		} catch { /* Headless machines and denied notification permissions are normal. */ }
	}

	private assertWorkspaceAvailable(cwd: string): void {
		const root = path.resolve(cwd);
		if ([...this.cleanupWorktrees].some(target => isPathInside(target, root))) throw new Error('Workspace cleanup is in progress');
	}

	private async runSetup(sessionId: string, command: string): Promise<void> {
		const initial = this.requireSession(sessionId);
		const launchId = initial.launchId;
		this.assertWorkspaceAvailable(initial.cwd);
		const update = (state: 'running' | 'failed' | 'complete', output: string, exitCode?: number | null) => {
			if (!this.isCurrentLaunch(sessionId, launchId)) return;
			// Output lives only in `setup` (bounded once finished, dropped on success); lastPreview is a status line.
			const stored = state === 'running' ? output : state === 'complete' ? '' : output.slice(-SETUP_OUTPUT_STORED_LIMIT);
			const updated = this.patchSession(sessionId, {setup: {command, state, output: stored, exitCode}, lastPreview: `Setup (${state}): ${command}`});
			if (updated) this.broadcastSessionUpdated(updated);
		};
		update('running', ''); await this.persist();
		this.assertCurrentLaunch(sessionId, launchId);
		const child = spawn(resolveShellCommand(), ['-c', command], {cwd: initial.cwd, env: {...process.env}, detached: true, stdio: ['ignore', 'pipe', 'pipe']});
		this.setupProcesses.set(sessionId, child);
		let output = '';
		let lastBroadcast = 0;
		const append = (text: string) => { output = `${output}${text}`.slice(-SETUP_OUTPUT_LIVE_LIMIT); if (Date.now() - lastBroadcast > 150) { lastBroadcast = Date.now(); update('running', output); } };
		for (const stream of [child.stdout, child.stderr]) {
			const decoder = new StringDecoder('utf8');
			stream?.on('data', (chunk: Buffer) => append(decoder.write(chunk)));
			stream?.on('end', () => { const rest = decoder.end(); if (rest) append(rest); });
		}
		let timedOut = false;
		const timer = setTimeout(() => { timedOut = true; try { if (child.pid) process.kill(-child.pid, 'SIGKILL'); } catch {} }, 600000);
		try {
			const code = await new Promise<number | null>((resolve, reject) => { child.once('error', reject); child.once('close', resolve); });
			this.assertCurrentLaunch(sessionId, launchId);
			update(code === 0 ? 'complete' : 'failed', `${output}${timedOut ? '\nSetup timed out after 10 minutes' : ''}`, code);
			await this.persist();
			if (code !== 0) throw new Error(timedOut ? 'Worktree setup timed out' : `Worktree setup failed (${String(code)}); press s to retry`);
		} catch (error) {
			if (this.sessions.get(sessionId)?.setup?.state === 'running') update('failed', `${output}\n${String(error)}`);
			await this.persist(); throw error;
		} finally { clearTimeout(timer); if (this.setupProcesses.get(sessionId) === child) this.setupProcesses.delete(sessionId); }
	}

	private async startWorker(session: SessionRecord, cols: number, rows: number): Promise<SessionRecord> {
		const launchId = session.launchId;
		if (!launchId) throw new Error('Session has no launch identity');
		this.assertWorkspaceAvailable(session.cwd);
		await fs.mkdir(getWorkerDir(), {recursive: true});
		this.assertWorkspaceAvailable(session.cwd);
		this.assertCurrentLaunch(session.id, launchId);
		const hookToken = randomUUID();
		const child = fork(getCliEntryPath(), ['--session-worker'], {
			env: {...process.env, DECKHAND_SESSION_ID: session.id, DECKHAND_LAUNCH_ID: launchId, DECKHAND_HOOK_TOKEN: hookToken},
			stdio: ['ignore', 'pipe', 'pipe', 'ipc'],
		});
		const worker: WorkerRuntime = {process: child, pending: new Map(), attached: {}, hookToken, launchId};
		// Wire every listener before the first await so no worker event can be missed.
		const logPath = getWorkerLogPath(session.id);
		child.stdout?.on('data', chunk => void fs.appendFile(logPath, chunk).catch(() => {}));
		child.stderr?.on('data', chunk => void fs.appendFile(logPath, chunk).catch(() => {}));
		child.on('message', message => void this.handleWorkerMessage(session.id, message as WorkerEvent, worker));
		child.on('exit', () => {
			if (this.workers.get(session.id) === worker) void this.handleWorkerProcessExit(session.id);
			else { for (const pending of worker.pending.values()) { clearTimeout(pending.timer); pending.reject(new Error('session worker exited')); } worker.pending.clear(); }
		});
		this.workers.set(session.id, worker);
		// Pane requests now route to the worker; retire any daemon-local panes from the starting phase.
		this.cleanupLocalPanes(session.id);
		let started: SessionRecord;
		try {
			await fs.writeFile(getWorkerPidPath(session.id), String(child.pid ?? ''), 'utf8').catch(() => {});
			// A cancel during the await above already sent its kill; never start an agent after it.
			this.assertCurrentLaunch(session.id, launchId);
			started = await this.sendWorkerRequest<SessionRecord>(session.id, {type: 'start', session, cols, rows});
		} catch (error) {
			if (this.workers.get(session.id) === worker) {
				this.workers.delete(session.id);
				await fs.rm(getWorkerPidPath(session.id), {force: true}).catch(() => {});
			}
			try { child.kill('SIGKILL'); } catch {}
			throw error;
		}
		const current = this.sessions.get(session.id);
		if (!current || current.status === 'exited' || current.launchId !== launchId) {
			// Cancelled while the agent was spawning: stop it. The exit handlers retire the worker.
			this.stopWorker(worker);
			throw new Error('Startup cancelled');
		}
		return started;
	}

	private stopWorker(worker: WorkerRuntime): void {
		try { if (worker.process.connected) worker.process.send?.({type: 'kill', requestId: randomUUID(), force: true}); } catch {}
		setTimeout(() => { if (worker.process.exitCode === null && worker.process.signalCode === null) { try { worker.process.kill('SIGKILL'); } catch {} } }, WORKER_KILL_GRACE_MS).unref?.();
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


	private async handleWorkerMessage(sessionId: string, message: WorkerEvent, worker: WorkerRuntime): Promise<void> {
		if (message.type === 'response') {
			const pending = worker?.pending.get(message.requestId);
			if (pending) {
				clearTimeout(pending.timer);
				worker?.pending.delete(message.requestId);
				message.ok ? pending.resolve(message.data) : pending.reject(new Error(message.error));
			}
			return;
		}
		if (this.workers.get(sessionId) !== worker) return;
		if (message.type === 'running') {
			const session = this.sessions.get(sessionId);
			if (session && session.status !== 'exited') await this.saveSession({...session, status: 'running', agentStartedAt: new Date().toISOString(), pid: message.pid, updatedAt: new Date().toISOString()});
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
			for (const [socket, client] of this.clients.entries()) {
				if (client.watchedPreviewSessionId !== sessionId) continue;
				const preview = client.previewScrollOffset > 0
					? await this.getPreviewRecord(sessionId, client.previewCols, client.previewRows, client.previewScrollOffset)
					: message.preview;
				sendMessage(socket, {type: 'preview-updated', preview});
			}
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
		} else { this.workers.delete(sessionId); }
	}

	private async setWorkerAgentStatus(sessionId: string, agentStatus: AgentActivityStatus): Promise<void> {
		const session = this.sessions.get(sessionId);
		if (!session || session.status === 'exited' || session.agentStatus === agentStatus) return;
		await this.saveSession({...session, agentStatus, agentStatusUpdatedAt: new Date().toISOString()});
	}

	private async handleWorkerSessionExit(sessionId: string, exitCode: number | null, exitSignal: number | null, lastPreview: string): Promise<void> {
		const existing = this.sessions.get(sessionId);
		if (!existing || existing.status === 'exited') return;
		const worker = this.workers.get(sessionId);
		this.workers.delete(sessionId);
		const now = new Date().toISOString();
		const parsedAgentSessionRef = refFromExitOutput(existing, lastPreview);
		const missingConversation = missingClaudeConversation(existing, lastPreview);
		// node-pty reports a signal death (e.g. SIGKILL) as {exitCode: 0, signal: 9}: that is not a completion.
		const exitReason = existing.exitReason === 'stopped' ? 'stopped' : missingConversation ? 'failed' : exitCode === null || exitSignal ? 'interrupted' : exitCode !== 0 ? 'failed' : 'completed';
		this.sessions.set(sessionId, {
			...existing,
			...(parsedAgentSessionRef ? {agentSessionRef: parsedAgentSessionRef} : {}),
			status: 'exited',
			agentStatus: 'idle',
			agentStatusUpdatedAt: now,
			updatedAt: now,
			pid: undefined,
			exitCode,
			exitSignal,
			// Never start fresh behind the user's back: say how to (S) instead.
			lastPreview: missingConversation ? `${lastPreview}\n\nClaude has no saved conversation ${missingConversation}. Press S to start a fresh conversation.` : lastPreview,
			devRunning: false,
			exitReason,
		});
		await fs.rm(getWorkerPidPath(sessionId), {force: true}).catch(() => {});
		const worktree = existing.worktree;
		if (worker?.deleteWorktreeOnExit && worktree?.path) {
			const cleanupKey = path.resolve(worktree.path);
			this.cleanupWorktrees.add(cleanupKey);
			// Every write below patches the latest record: notes/archive edits may land during these awaits.
			const markDeleted = () => this.patchSession(sessionId, {worktree: {...(this.sessions.get(sessionId)?.worktree ?? worktree), deletedAt: new Date().toISOString()}, updatedAt: new Date().toISOString()});
			try {
				const repoCwd = existing.launchWorktreeRoot ?? existing.repoRoot;
				await this.assertCleanupAllowed(sessionId, Boolean(worker.deleteBranchOnExit), worker.allowDataLoss ?? false);
				await removeWorktree(worktree.path, repoCwd, worktree.name);
				markDeleted();
				if (worker.deleteBranchOnExit && worktree.branch) await deleteLocalBranch(repoCwd, worktree.branch);
			} catch (error) {
				const gone = await fs.stat(worktree.path).then(() => false, statError => (statError as NodeJS.ErrnoException).code === 'ENOENT');
				if (gone && !this.sessions.get(sessionId)?.worktree?.deletedAt) markDeleted();
				const cleanupError = `${gone ? 'Worktree removed; cleanup incomplete' : 'Worktree retained'}: ${errorMessage(error)}`;
				this.patchSession(sessionId, {cleanupError});
				await this.log(cleanupError);
			} finally { this.cleanupWorktrees.delete(cleanupKey); }
		}
		const updated = this.requireSession(sessionId);
		await this.saveSession(updated);
		void this.notify(updated, `Agent process ${updated.exitReason ?? 'exited'}`);
		for (const [socket, client] of this.clients.entries()) {
			if (client.watchedPreviewSessionId === sessionId) sendMessage(socket, {type: 'preview-updated', preview: this.buildPreviewRecord(updated, updated.lastPreview ?? '')});
		}
	}

	private sessionsForRepo(repoRoot: string): SessionRecord[] {
		return sortSessionsForSidebar([...this.sessions.values()].filter(session => sessionMatchesScope(session, repoRoot)));
	}

	private broadcastSessionUpdated(session: SessionRecord): void {
		session = this.sessions.get(session.id) ?? session;
		for (const [socket, client] of this.clients.entries()) {
			if (client.repoRoot && sessionMatchesScope(session, client.repoRoot)) {
				sendMessage(socket, {type: 'session-updated', session});
			}
		}
	}

	private broadcastSessionRemoved(session: SessionRecord): void {
		const sessionId = session.id;
		for (const [socket, client] of this.clients.entries()) {
			if (client.repoRoot && sessionMatchesScope(session, client.repoRoot)) {
				sendMessage(socket, {type: 'session-removed', sessionId});
			}
		}
	}

	private async updateSessionDevRunning(sessionId: string, devRunning: boolean): Promise<void> {
		const session = this.sessions.get(sessionId);
		if (!session || session.status === 'exited' || Boolean(session.devRunning) === devRunning) return;
		await this.saveSession({...session, devRunning});
	}

	private async updateSessionNotes(sessionId: string, notes: string): Promise<SessionRecord> {
		const session = this.sessions.get(sessionId);
		if (!session) throw new Error('session not found');
		return this.saveSession({...session, notes: notes.slice(0, 50_000), updatedAt: new Date().toISOString()});
	}

	private buildPreviewRecord(session: SessionRecord, content: string, scrollOffset = 0, maxScrollOffset = 0): PreviewRecord {
		return {
			sessionId: session.id,
			content,
			live: session.status === 'running',
			status: session.status,
			agentStatus: session.agentStatus,
			scrollOffset,
			maxScrollOffset,
		};
	}

	private async getPreviewRecord(sessionId: string | undefined, cols: number, rows: number, scrollOffset = 0): Promise<PreviewRecord> {
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
			return await this.sendWorkerRequest<PreviewRecord>(sessionId, {type: 'snapshot', target: 'agent', cols, rows, scrollOffset});
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
			preview: new TerminalPreview(cols, rows),
			cwd: session.cwd,
			exited: false,
		};
		this.terminals.set(sessionId, terminal);

		term.onData(output => {
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

	private cleanupLocalPanes(sessionId: string): void {
		this.cleanupTerminal(sessionId);
		this.cleanupGit(sessionId);
		this.cleanupDev(sessionId);
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

		const pending = this.gitStartPromises.get(sessionId);
		if (pending) {
			const git = await pending;
			git.term.resize(Math.max(1, cols), Math.max(1, rows));
			await git.preview.resize(cols, rows);
			return git;
		}

		const start = (async () => {
			const command = await resolveLazyGitCommand();
			const current = this.gits.get(sessionId);
			if (current && !current.exited) return current;
			const term = pty.spawn(command, [], {
				name: 'xterm-256color',
				cwd: session.cwd,
				env: {...process.env},
				cols: Math.max(1, cols),
				rows: Math.max(1, rows),
			});
			const git: RuntimeTerminal = {
				term,
					preview: new TerminalPreview(cols, rows),
				cwd: session.cwd,
				exited: false,
			};
			this.gits.set(sessionId, git);

			term.onData(output => {
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
		})();
		this.gitStartPromises.set(sessionId, start);
		try {
			return await start;
		} finally {
			if (this.gitStartPromises.get(sessionId) === start) this.gitStartPromises.delete(sessionId);
		}
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

	// An unreadable deckhand.json is treated as untrusted: fall back to the global dev command.
	private async resolveSessionDevCommand(session: SessionRecord): Promise<string> {
		const appConfig = await loadAppConfig();
		return resolveDevCommand(await loadProjectConfig(session.cwd, appConfig).catch(() => undefined), appConfig);
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

		const command = await this.resolveSessionDevCommand(session);
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
			preview: new TerminalPreview(cols, rows),
			cwd: session.cwd,
			exited: false,
			command,
		};
		this.devs.set(sessionId, dev);

		term.onData(output => {
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

	private async reorderSession(sessionId: string, direction: 'up' | 'down'): Promise<SessionRecord[]> {
		const session = this.sessions.get(sessionId);
		if (!session) {
			throw new Error('session does not exist');
		}
		const siblings = [...this.sessions.values()]
			.filter(candidate => candidate.repoRoot === session.repoRoot && candidate.parentSessionId === session.parentSessionId)
			.sort(compareSessionOrder);
		const index = siblings.findIndex(candidate => candidate.id === sessionId);
		const swapIndex = direction === 'up' ? index - 1 : index + 1;
		if (index < 0 || swapIndex < 0 || swapIndex >= siblings.length) {
			return this.sessionsForRepo(session.repoRoot);
		}
		const now = new Date().toISOString();
		const normalized = siblings.map((candidate, order) => ({...candidate, sidebarOrder: order}));
		const current = normalized[index]!;
		const other = normalized[swapIndex]!;
		const currentOrder = current.sidebarOrder;
		const otherOrder = other.sidebarOrder;
		const updatedCurrent = {...current, sidebarOrder: otherOrder, updatedAt: now};
		const updatedOther = {...other, sidebarOrder: currentOrder, updatedAt: now};
		for (const candidate of normalized) {
			if (candidate.id === updatedCurrent.id) this.sessions.set(candidate.id, updatedCurrent);
			else if (candidate.id === updatedOther.id) this.sessions.set(candidate.id, updatedOther);
			else this.sessions.set(candidate.id, candidate);
		}
		await this.persist();
		this.broadcastSessionUpdated(updatedCurrent);
		this.broadcastSessionUpdated(updatedOther);
		return this.sessionsForRepo(session.repoRoot);
	}

	private async sameRepository(left: string, right: string): Promise<boolean> {
		return left === right || await findGitCommonDir(left) === await findGitCommonDir(right);
	}

	private async createSession(input: CreateSessionInput): Promise<SessionRecord> {
		if (!['claude', 'pi', 'codex'].includes(input.program)) throw new Error('Unsupported agent');
		if (input.handoffFromSessionId && input.subSessionKind === 'forked') throw new Error('Handoffs require a clean session');
		const parentSession = input.parentSessionId ? this.sessions.get(input.parentSessionId) : undefined;
		const title = parentSession ? inheritedChildTitle(parentSession.title, input.title) : input.title.trim();
		if (!title) {
			throw new Error('title cannot be empty');
		}
		if (title.length > 64) {
			throw new Error('title cannot be longer than 64 characters');
		}
		if (input.parentSessionId && (!parentSession || !await this.sameRepository(parentSession.repoRoot, input.repoRoot))) {
			throw new Error('parent session does not exist in this repo');
		}
		if (input.subSessionKind === 'forked') {
			if (!parentSession) {
				throw new Error('forked sub-session requires a parent session');
			}
			if (!supportsForkedSubSession(parentSession.program)) {
				throw new Error('forked sub-sessions are only supported for Claude and Pi');
			}
			if (input.program !== parentSession.program) {
				throw new Error('forked sub-session must use the parent session program');
			}
			if (!parentSession.agentSessionRef) {
				throw new Error('parent session does not have a resumable agent reference');
			}
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
		const siblingOrders = [...this.sessions.values()]
			.filter(session => session.repoRoot === input.repoRoot && session.parentSessionId === input.parentSessionId)
			.map(session => (typeof session.sidebarOrder === 'number' && Number.isFinite(session.sidebarOrder) ? session.sidebarOrder : 0));
		const nextSidebarOrder = siblingOrders.length === 0 ? 0 : Math.max(...siblingOrders) + 1;
		let handoffPath: string | undefined;
		if (input.handoffFromSessionId) {
			const source = this.requireSession(input.handoffFromSessionId);
			if (!await this.sameRepository(source.repoRoot, input.repoRoot)) throw new Error('Handoff source must be in the same repository');
			handoffPath = source.handoffPath ?? await this.exportSessionHandoff(source);
		}
		const baseSession: SessionRecord = {
			id: sessionId,
			launchId: randomUUID(),
			title,
			program: input.program,
			command,
			args: [],
			cwd: input.cwd,
			repoRoot: input.repoRoot,
			launchCwd: input.cwd,
			launchWorktreeRoot: input.cwd,
			worktree: {mode: 'none'},
			requestedWorktreeMode: input.worktreeMode ?? 'none',
			handoffPath,
			status: 'starting',
			agentStatus: 'unknown',
			agentStatusUpdatedAt: now,
			createdAt: now,
			updatedAt: now,
			lastPreview: '',
			parentSessionId: input.parentSessionId,
			subSessionKind: input.subSessionKind,
			forkedFromSessionId: input.subSessionKind === 'forked' ? parentSession?.id : undefined,
			forkedFromAgentSessionRef: input.subSessionKind === 'forked' ? parentSession?.agentSessionRef : undefined,
			sidebarOrder: nextSidebarOrder,
		};

		this.sessions.set(baseSession.id, baseSession);
		this.broadcastSessionUpdated(baseSession);
		void this.persist().catch(error => console.error('failed to persist starting session', error));
		this.preparingSessions.add(baseSession.id);
		void this.finishCreateSession(baseSession.id, input).catch(error => this.failStartingSession(baseSession.id, error, baseSession.launchId)).finally(() => this.preparingSessions.delete(baseSession.id));
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
		const appConfig = await loadAppConfig();
		let sessionCwd = launchCwd;
		let worktree: SessionRecord['worktree'] = {mode: 'none'};
		const requestedWorktreeMode = input.worktreeMode ?? 'none';
		// Project configuration only matters for new worktrees (creation hook + setup), so a
		// malformed deckhand.json never blocks none/existing sessions. The repository config is the
		// main checkout's; the creation hook resolves from the launch checkout (the worktree does not exist yet).
		let launchProject: LoadedProject | undefined;
		let projectError: string | undefined;
		if (requestedWorktreeMode === 'new') {
			try { launchProject = await loadProjectConfig(launchCwd, appConfig); } catch (error) { projectError = errorMessage(error); }
			// Worktree location/links: global defaults, overlaid by the repository override only when trusted.
			// Invalid global defaults are reported by setup below, so they do not block creation here.
			let worktreeSettings: WorktreeSettings | undefined;
			try { worktreeSettings = resolveSettings(launchProject, appConfig).worktree; } catch {}
			const created = await createWorktreeForSession(title, launchCwd, launchProject && isProjectTrusted(launchProject, appConfig) ? launchProject.creationHook : undefined, worktreeSettings);
			if (created.links?.notes.length) await this.log(`worktree links for ${created.path}: ${created.links.notes.join('; ')}`);
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
				...(created.links ? {links: created.links} : {}),
				// The base the new branch actually started from (branchFrom), so cleanup and create-pr --base compare against it.
				...(created.baseRef ? {baseRef: created.baseRef} : {}),
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

		const forkParent = input.subSessionKind === 'forked' && input.parentSessionId ? this.sessions.get(input.parentSessionId) : undefined;
		const program = startingSession.program;
		const agentName = buildDeckhandAgentName(title, sessionId);
		// Claude's /branch picks the child's ID, so a forked Claude child is known by the name it branches to
		// until a SessionStart hook or exit hint reports that ID. Everything else gets an exact ID now.
		const branchesClaude = Boolean(forkParent) && program === 'claude';
		const agentSessionRef = startingSession.agentSessionRef ?? (branchesClaude ? {provider: program, kind: 'name', value: agentName} : buildAgentSessionRef(program));
		const preparedSession: SessionRecord = {
			...startingSession,
			cwd: sessionCwd,
			args: branchesClaude
				? buildAgentArgs({program, agentSessionRef: forkParent?.agentSessionRef}, 'resume')
				: buildAgentArgs({program, agentSessionRef}, 'create', agentName, forkParent?.agentSessionRef),
			agentSessionRef,
			forkedFromSessionId: forkParent?.id ?? startingSession.forkedFromSessionId,
			forkedFromAgentSessionRef: forkParent?.agentSessionRef ?? startingSession.forkedFromAgentSessionRef,
			launchWorktreeRoot,
			worktree: {...worktree, baseRef: worktree.baseRef ?? (await currentBranch(launchWorktreeRoot) || await headSha(launchWorktreeRoot))},
			updatedAt: new Date().toISOString(),
		};
		if (this.sessions.get(sessionId)?.status !== 'starting') {
			const cancelled = this.sessions.get(sessionId);
			if (cancelled && worktree.path) await this.saveSession({...cancelled, cwd: sessionCwd, launchWorktreeRoot, worktree: preparedSession.worktree, updatedAt: new Date().toISOString()});
			return;
		}
		this.sessions.set(sessionId, preparedSession);
		this.broadcastSessionUpdated(preparedSession);
		this.preparingSessions.delete(sessionId);

		// Setup uses the configuration that was reviewed for this launch (the same bytes whose trust chose
		// the creation hook): global defaults, overlaid by the repository override only when trusted.
		if (worktree.origin === 'created') {
			let setupCommand: string | undefined;
			try {
				if (projectError) throw new Error(`${projectError}. Fix it with C, then press s to retry setup`);
				setupCommand = resolveSetupCommand(launchProject, appConfig, input.projectFingerprint);
			} catch (error) {
				// Unreadable or unreviewed config: setup is owed once it is fixed or reviewed.
				this.patchSession(sessionId, {setup: {command: launchProject?.config.setupCommand ?? 'configuration', state: 'failed', output: errorMessage(error)}});
				throw new Error(`Worktree retained: ${errorMessage(error)}`);
			}
			if (setupCommand) {
				// Recorded before anything can fail or be cancelled, so a restart knows setup is still owed.
				await this.saveSession({...this.requireSession(sessionId), setup: {command: setupCommand, state: 'pending', output: ''}});
				await this.runSetup(sessionId, setupCommand);
			}
		}
		this.assertCurrentLaunch(sessionId, startingSession.launchId);
		preparedSession.args = [...(preparedSession.args ?? []), ...await integrationArgs(preparedSession.program, preparedSession.command, appConfig.agent_hooks === true), ...(preparedSession.handoffPath ? ['--', handoffPrompt(preparedSession.handoffPath)] : [])];
		const launchSession = {...this.requireSession(sessionId), args: preparedSession.args, handoffPath: preparedSession.handoffPath};
		this.sessions.set(sessionId, launchSession);
		const runningSession = await this.startWorker(launchSession, input.cols, input.rows);
		if (branchesClaude) {
			setTimeout(() => { if (this.sessions.get(sessionId)?.launchId === preparedSession.launchId) this.sendWorkerEvent(sessionId, {type: 'input', target: 'agent', data: branchCommandInput(agentName)}); }, 500).unref?.();
		}
		await this.saveSession({...runningSession, ...this.requireSession(sessionId), status: 'running', pid: runningSession.pid});
	}

	private async failStartingSession(sessionId: string, error: unknown, launchId?: string): Promise<void> {
		const session = this.sessions.get(sessionId);
		if (!session || session.status !== 'starting' || (launchId && session.launchId !== launchId)) {
			return;
		}
		this.cleanupLocalPanes(sessionId);
		await this.saveSession({
			...session,
			status: 'exited',
			updatedAt: new Date().toISOString(),
			pid: undefined,
			exitCode: null,
			exitSignal: null,
			lastPreview: `Failed to start session: ${errorMessage(error)}${session.setup?.output ? `\n${session.setup.output}` : ''}`,
			exitReason: 'failed',
		});
	}

	private async restartSession(sessionId: string, cols: number, rows: number, mode: RestartMode = 'resume', projectFingerprint?: string): Promise<SessionRecord> {
		const existing = this.sessions.get(sessionId);
		if (!existing) {
			throw new Error('session does not exist');
		}
		if (this.preparingSessions.has(sessionId)) throw new Error('Worktree preparation is still finishing; wait before retrying');
		if (this.workers.has(sessionId) || existing.status !== 'exited') {
			throw new Error('session is already running');
		}
		if (existing.worktree?.deletedAt) {
			throw new Error('cannot restart session because its worktree was deleted');
		}

		const now = new Date().toISOString();
		const parsedAgentSessionRef = mode === 'resume' && existing.lastPreview ? refFromExitOutput(existing, existing.lastPreview) : undefined;
		const restartSource = parsedAgentSessionRef ? {...existing, agentSessionRef: parsedAgentSessionRef} : existing;
		const neverStarted = Boolean(existing.launchId) && !existing.agentStartedAt;
		if (neverStarted && existing.requestedWorktreeMode && existing.requestedWorktreeMode !== 'none' && !existing.worktree?.path) throw new Error('Worktree preparation did not complete. Create a new session to retry instead of launching in the original checkout.');
		if (mode === 'resume' && existing.program === 'codex' && !restartSource.agentSessionRef && !neverStarted) throw new Error('Codex conversation ID is unknown. Use S for a fresh session, or enable trusted Codex hooks before starting new sessions.');
		const missingConversation = mode === 'resume' ? missingClaudeConversation(existing, existing.lastPreview) : undefined;
		if (missingConversation && missingConversation === restartSource.agentSessionRef?.value) throw new Error(`Claude has no saved conversation ${missingConversation}. Use S for a fresh session.`);
		const agentName = buildDeckhandAgentName(existing.title, existing.id, mode === 'fresh' ? `fresh-${Date.now().toString(36)}` : undefined);
		const restartRef = mode === 'fresh' ? {ref: buildAgentSessionRef(existing.program), shouldForkParent: false} : restartRefForSession(restartSource);
		// A fork that never launched, or never got its own conversation, forks its parent again.
		const forkSource = mode !== 'fresh' && existing.subSessionKind === 'forked' && (restartRef.shouldForkParent || neverStarted) ? existing.forkedFromAgentSessionRef : undefined;
		const branchesClaude = Boolean(forkSource) && existing.program === 'claude';
		// Pi forks copy the parent into a new exact ID; a Claude fork keeps the existing reference until /branch reports one.
		const startingAgentSessionRef = forkSource && !branchesClaude ? buildAgentSessionRef(existing.program) : restartRef.ref;
		const starting: SessionRecord = {
			...restartSource,
			launchId: randomUUID(),
			agentStartedAt: mode === 'fresh' ? undefined : existing.agentStartedAt,
			archivedAt: undefined,
			attention: undefined,
			exitReason: undefined,
			cleanupError: undefined,
			args: branchesClaude
				? buildAgentArgs({program: existing.program, agentSessionRef: forkSource}, 'resume')
				: buildAgentArgs({program: existing.program, agentSessionRef: startingAgentSessionRef}, mode === 'fresh' || neverStarted || forkSource ? 'create' : 'resume', agentName, forkSource),
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
		await this.saveSession(starting);

		try {
			const config = await loadAppConfig();
			// Setup that never completed (pending, failed, untrusted, cancelled) is retried against the
			// repository's current effective configuration. Plain restarts never read deckhand.json.
			if (existing.setup && existing.setup.state !== 'complete') {
				const setupCommand = resolveSetupCommand(await loadProjectConfig(existing.cwd, config), config, projectFingerprint);
				if (!setupCommand) this.patchSession(sessionId, {setup: undefined});
				else await this.runSetup(sessionId, setupCommand);
			}
			this.assertCurrentLaunch(sessionId, starting.launchId);
			starting.args = [...(starting.args ?? []), ...await integrationArgs(starting.program, starting.command, config.agent_hooks === true), ...(neverStarted && starting.handoffPath ? ['--', handoffPrompt(starting.handoffPath)] : [])];
			await prepareAgentSessionRef(starting.agentSessionRef);
			const runningSession = await this.startWorker({...this.requireSession(sessionId), args: starting.args}, cols, rows);
			if (branchesClaude) {
				const forkName = buildDeckhandAgentName(starting.title, starting.id);
				setTimeout(() => { if (this.sessions.get(sessionId)?.launchId === starting.launchId) this.sendWorkerEvent(sessionId, {type: 'input', target: 'agent', data: branchCommandInput(forkName)}); }, 500).unref?.();
			}
			return await this.saveSession({...runningSession, ...this.requireSession(sessionId), status: 'running', pid: runningSession.pid});
		} catch (error) {
			if (this.sessions.get(sessionId)?.status === 'starting') await this.failStartingSession(sessionId, error, starting.launchId);
			throw error;
		}
	}

	private async mergeSessionWorktree(sessionId: string, mode: 'merge' | 'squash', targetCwd: string) {
		const session = this.sessions.get(sessionId);
		if (!session) {
			throw new Error('session does not exist');
		}
		const worktree = session.worktree;
		const worktreePath = worktree?.path;
		if (!worktreePath || !worktree || worktree.mode === 'none') {
			throw new Error('session does not have a worktree to merge');
		}
		if (worktree.deletedAt) {
			throw new Error('cannot merge session because its worktree was deleted');
		}
		const result = await mergeWorktreeIntoCurrent(worktreePath, targetCwd, mode);
		if (result.skipped) {
			await this.log(`${mode} merge skipped for ${session.title} (${result.sourceRef}) into ${result.targetBranch}: ${result.reason ?? 'no new commits'}`);
		} else if (result.conflicted) {
			await this.log(`${mode} merge for ${session.title} (${result.sourceRef}) into ${result.targetBranch} has conflicts to resolve`);
		} else {
			const updated: SessionRecord = {
				...session,
				worktree: {
					...worktree,
					mergedAt: new Date().toISOString(),
					mergeMode: mode,
					mergeTargetBranch: result.targetBranch,
					mergeSourceRef: result.sourceRef,
				},
				updatedAt: new Date().toISOString(),
			};
			await this.saveSession(updated);
			await this.log(`${mode} merged ${session.title} (${result.sourceRef}) into ${result.targetBranch}`);
		}
		return result;
	}

	private async markSessionMerged(sessionId: string, targetCwd: string): Promise<SessionRecord> {
		const session = this.sessions.get(sessionId);
		if (!session) {
			throw new Error('session does not exist');
		}
		const worktree = session.worktree;
		const worktreePath = worktree?.mode !== 'none' ? worktree?.path : undefined;
		if (worktree?.mergedAt) {
			const unmergedWorktree = {...worktree};
			delete unmergedWorktree.mergedAt;
			delete unmergedWorktree.mergeMode;
			delete unmergedWorktree.mergeTargetBranch;
			delete unmergedWorktree.mergeSourceRef;
			delete unmergedWorktree.mergeMarkedManually;
			const updated: SessionRecord = {
				...session,
				worktree: unmergedWorktree,
				updatedAt: new Date().toISOString(),
			};
			await this.saveSession(updated);
			await this.log(`unmarked ${session.title} as merged`);
			return updated;
		}
		if (session.mergedAt) {
			const updated: SessionRecord = {...session, updatedAt: new Date().toISOString()};
			delete updated.mergedAt;
			delete updated.mergeTargetBranch;
			delete updated.mergeSourceRef;
			delete updated.mergeMarkedManually;
			await this.saveSession(updated);
			await this.log(`unmarked ${session.title} as merged`);
			return updated;
		}
		if (worktree?.deletedAt) {
			throw new Error('cannot mark session merged because its worktree was deleted');
		}
		const sourceRoot = await findRepoRoot(worktreePath ?? session.cwd);
		const targetRoot = await findRepoRoot(targetCwd);
		const sourceRef = await currentBranch(sourceRoot) || await headSha(sourceRoot);
		const targetBranch = await currentBranch(targetRoot);
		if (!targetBranch) {
			throw new Error('target worktree is detached; checkout a branch before marking merged');
		}
		const now = new Date().toISOString();
		const updated: SessionRecord = worktreePath && worktree
			? {
				...session,
				worktree: {
					...worktree,
					mergedAt: now,
					mergeTargetBranch: targetBranch,
					mergeSourceRef: sourceRef,
					mergeMarkedManually: true,
				},
				updatedAt: now,
			}
			: {
				...session,
				mergedAt: now,
				mergeTargetBranch: targetBranch,
				mergeSourceRef: sourceRef,
				mergeMarkedManually: true,
				updatedAt: now,
			};
		await this.saveSession(updated);
		await this.log(`manually marked ${session.title} (${sourceRef}) merged into ${targetBranch}`);
		return updated;
	}

	private async killSession(sessionId: string, deleteWorktree: boolean, deleteBranch: boolean, force: boolean, allowDataLoss = false): Promise<void> {
		const deleteOnExit = deleteWorktree || deleteBranch;
		const assertRunning = () => {
			const current = this.requireSession(sessionId);
			const worker = this.workers.get(sessionId);
			if (!worker || worker.exited || current.status === 'exited') throw new Error('session is not running');
			return {current, worker};
		};
		assertRunning();
		if (deleteOnExit) await this.assertCleanupAllowed(sessionId, deleteBranch, allowDataLoss);
		// Re-read after the slow cleanup checks: the session may have exited or been edited meanwhile.
		const {current, worker} = assertRunning();
		worker.allowDataLoss = allowDataLoss;
		worker.deleteWorktreeOnExit = deleteOnExit;
		worker.deleteBranchOnExit = deleteBranch;
		this.sessions.set(sessionId, {...current, exitReason: 'stopped', cleanupError: undefined});
		await this.sendWorkerRequest(sessionId, {type: 'kill', force});
	}

	private async removeSession(sessionId: string): Promise<void> {
		const existing = this.sessions.get(sessionId);
		if (!existing) {
			throw new Error('session does not exist');
		}
		if (this.workers.has(sessionId) || existing.status === 'running') {
			throw new Error('kill the session before removing it');
		}
		this.cleanupLocalPanes(sessionId);
		this.summaries.forEach((_entry, slot) => { if (slot.startsWith(`${sessionId}\0`)) this.summaries.delete(slot); });
		this.sessions.delete(sessionId);
		await this.persist();
		this.broadcastSessionRemoved(existing);
	}

	// Coalesced: at most one write in flight plus one queued. Every caller arriving before the
	// queued write snapshots the sessions shares it, so bursts of updates cost one write.
	private persist(): Promise<void> {
		if (this.persistQueued) return this.persistQueued;
		const operation = this.persistInFlight.catch(() => {}).then(() => {
			this.persistQueued = undefined;
			return saveSessions(sortSessionsNewestFirst([...this.sessions.values()]));
		});
		this.persistQueued = operation;
		this.persistInFlight = operation;
		return operation;
	}
}
