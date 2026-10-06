import fs from 'node:fs/promises';
import path from 'node:path';
import net from 'node:net';
import {execFile, fork, spawn, type ChildProcess} from 'node:child_process';
import {promisify} from 'node:util';
import {randomUUID} from 'node:crypto';
import {StringDecoder} from 'node:string_decoder';
import {getConfigDir, getCliEntryPath, getDaemonLogPath, getDaemonPidPath, getSocketPath, getWorkerDir, getWorkerLogPath, getWorkerPidPath} from './paths.js';
import {createWorktreeForSession, currentBranch, deleteLocalBranch, findGitCommonDir, findRepoRoot, headSha, listWorktrees, mergeWorktreeIntoCurrent, removeWorktree, sanitizeWorktreeName} from './git.js';
import {ensureNodePtyReady} from './nodePty.js';
import {ensureConfigDir, loadAppConfig, type AppConfig, markAllNonExitedSessionsExited, saveState, sortSessionsNewestFirst, updateAppConfig} from './storage.js';
import {liveWorktreeRecord, MERGE_MARKERS, ownWorktreePath, projectWorktree, storedSession, withoutMarkers} from './worktreeRecords.js';
import {compareSessionOrder, sortSessionsForSidebar} from './sessionOrder.js';
import {isPathInside, sessionMatchesScope} from './sessionScope.js';
import {noWorkspaceReason, workspaceKey, workspaceWorkerId} from './workspace.js';
import {errorMessage} from './ui.js';
import type {WorktreeSettings} from './worktreeLinks.js';
import {loadProjectConfig, isProjectTrusted, projectNeedsReview, resolveDevCommand, resolveSettings, resolveSetupCommand, trustProjectConfig, type LoadedProject} from './projectConfig.js';
import {saveGlobalDefaultsDocument, saveProjectConfigDocument} from './projectConfigDocument.js';
import {readCandidateSizes, readSettingsInfo, readWorktreeCandidates} from './settingsInfo.js';
import {createPullRequest, getHandoffGitContext, getWorkspaceSummary, inspectWorkspaceCleanup, type WorkspaceSummary, type CleanupInspection} from './workspaceGit.js';
import {applyStage, readChangeDiff, readChanges, type ChangesSnapshot, type UntrackedCounts} from './changesGit.js';
import {emptyChanges, findChange, type ChangesRecord} from './changesModel.js';
import {normalizeHook, integrationArgs, needsAttention} from './agentSignals.js';
import {AGENTS, agentSpec, launchArgs, newAgentRef, relaunchPlan, sameAgentSessionRef, type LaunchPlan} from './agents.js';
import {AgentVersionChecker, findOnPath} from './agentVersionCheck.js';
import {exportHandoff} from './sessionFeatures.js';
import {PROTOCOL_VERSION} from './types.js';
import type {AgentActivityStatus, AgentSessionRef, AgentUpdateResult, AgentVersions, ClientRequest, CreateSessionInput, DevRecord, GitRecord, PreviewRecord, ProjectInfo, RestartMode, ServerMessage, ServerResponse, SessionCleanupInspection, SessionRecord, SessionWorktreeRecord, TerminalRecord, WorktreeMarkers, WorktreeRecord} from './types.js';

const execFileAsync = promisify(execFile);
const DEFAULT_PREVIEW_COLS = 80;
const DEFAULT_PREVIEW_ROWS = 24;
const WORKER_REQUEST_TIMEOUT_MS = 10_000;
const WORKER_KILL_GRACE_MS = 2000;
const MAX_REQUEST_LINE = 256_000;
const SETUP_OUTPUT_LIVE_LIMIT = 16_000;
const SETUP_OUTPUT_STORED_LIMIT = 4096;
const SUMMARY_TTL_MS = 4000;
const WORKSPACE_SHUTDOWN_TIMEOUT_MS = 3000;
const CHANGES_POLL_MS = 2000;
// A diff request reuses the last status read when it is this recent instead of reading again.
const CHANGES_FRESH_MS = 2500;
// How often the daemon asks whether the latest agent releases are due for a lookup (they are every LATEST_MAX_AGE_MS).
const AGENT_VERSIONS_TICK_MS = 30 * 60_000;

interface ClientSubscription {
	repoRoot?: string;
	watchedPreviewSessionId?: string;
	watchedTerminalSessionId?: string;
	watchedGitSessionId?: string;
	watchedDevSessionId?: string;
	watchedChangesSessionId?: string;
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

// An IPC child the daemon sends requests to: a session worker or a workspace worker.
interface WorkerChannel {
	process: ChildProcess;
	pending: Map<string, {resolve: (value: unknown) => void; reject: (error: Error) => void; timer: NodeJS.Timeout}>;
	exited?: boolean;
}

// A session worker owns only the agent PTY; Terminal/Git/Dev live in the session's workspace worker.
interface WorkerRuntime extends WorkerChannel {
	attached: {agent?: net.Socket};
	deleteWorktreeOnExit?: boolean;
	deleteBranchOnExit?: boolean;
	hookToken: string;
	launchId: string;
	allowDataLoss?: boolean;
}

// Panes a workspace worker hosts for every session in one worktree (sessionWorker.ts WORKSPACE_PANES).
type WorkspacePane = 'terminal' | 'git' | 'dev';
// One record shape serves all three panes (TerminalRecord and GitRecord are DevRecord without `command`).
type WorkspacePaneRecord = DevRecord;
const PANE_LABELS: Record<WorkspacePane, string> = {terminal: 'Terminal', git: 'Git', dev: 'Dev'};
const PANE_ATTACH_NAMES: Record<WorkspacePane, string> = {terminal: 'terminal', git: 'git', dev: 'dev command'};

// The workspace worker of one workspace (workspaceKey: the worktree root), started on demand. Requests name a
// session; the daemon resolves its workspace, and stamps each viewer's session ID on the shared pane records.
interface WorkspaceRuntime extends WorkerChannel {
	key: string;
	attached: Partial<Record<WorkspacePane, {socket: net.Socket; sessionId: string}>>;
	/** The last record each pane reported; `dev.live` drives every workspace session's devRunning. */
	records: Partial<Record<WorkspacePane, WorkspacePaneRecord>>;
	/** Pane requests in flight; an idle worker is retired only when none is pending. */
	busy: number;
}

type WorkerResponse =
	| {type: 'response'; requestId: string; ok: true; data?: unknown}
	| {type: 'response'; requestId: string; ok: false; error: string};

type WorkerEvent =
	| WorkerResponse
	| {type: 'running'; pid: number}
	| {type: 'exit'; exitCode: number | null; exitSignal: number | null; lastPreview: string}
	| {type: 'agent-status'; agentStatus: AgentActivityStatus}
	| {type: 'preview-updated'; preview: PreviewRecord}
	| {type: 'output'; target: 'agent'; data: string};

type WorkspaceEvent =
	| WorkerResponse
	| {type: 'terminal-updated'; terminal: TerminalRecord}
	| {type: 'git-updated'; git: GitRecord}
	| {type: 'dev-updated'; dev: DevRecord}
	| {type: 'output'; target: WorkspacePane; data: string};

// The Git tab's Changes view of one workspace: no worker or PTY, just Git run by the daemon. Polled while any client
// watches a session of the workspace; every Git run for it (status, diff, stage) is serialized through `queue`.
interface ChangesWatch {
	key: string;
	timer?: NodeJS.Timeout;
	queue: Promise<unknown>;
	/** A status read queued behind `queue` and not started yet: callers share it. */
	queued?: Promise<ChangesSnapshot>;
	running: boolean;
	last?: {snapshot: ChangesSnapshot; at: number};
	/** The last record pushed to watchers (JSON), so polls push only changes. */
	published?: string;
	untracked: UntrackedCounts;
}

function paneUpdatedMessage(pane: WorkspacePane, record: WorkspacePaneRecord): ServerMessage {
	if (pane === 'terminal') return {type: 'terminal-updated', terminal: record};
	if (pane === 'git') return {type: 'git-updated', git: record};
	return {type: 'dev-updated', dev: record};
}

function updatedPaneRecord(message: Exclude<WorkspaceEvent, WorkerResponse | {type: 'output'}>): [WorkspacePane, WorkspacePaneRecord] {
	if (message.type === 'terminal-updated') return ['terminal', message.terminal];
	if (message.type === 'git-updated') return ['git', message.git];
	return ['dev', message.dev];
}

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

	// Only a found binary is cached, so an agent installed while the daemon runs is found by the next launch.
	const found = await findOnPath(program);
	if (found) programCommandCache.set(program, found);
	return found ?? program;
}

function buildDeckhandAgentName(title: string, sessionId: string, suffix?: string): string {
	const safeTitle = sanitizeWorktreeName(title)
		.replace(/_*\/+_*/g, '-')
		.slice(0, 40)
		.replace(/^[-_]+|[-_]+$/g, '') || 'session';
	return `dh-${safeTitle}-${sessionId.slice(0, 8)}${suffix ? `-${suffix}` : ''}`;
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

// What an exited agent's last screen says about its conversation: the ref to keep (`ref`), whether the launch failed,
// and a note appended to the preview saying what s/S will do. A forked child never takes its parent's identity; a fork
// whose parent (or whose own new conversation) the agent cannot find stores the parent's ref, so s forks it again.
function readAgentExit(session: SessionRecord, output: string): {ref?: AgentSessionRef; failed: boolean; note?: string} {
	const agent = agentSpec(session.program);
	const forkParent = session.subSessionKind === 'forked' ? session.forkedFromAgentSessionRef : undefined;
	const missing = agent.missingConversation?.(output);
	if (forkParent && (agent.forkFailed?.(output) || (missing && (missing === forkParent.value || missing === session.agentSessionRef?.value)))) {
		return {ref: forkParent, failed: true, note: `${agent.label} found no saved conversation to fork (a conversation is saved once it has a message). Press s to fork the parent again, or S to start a fresh conversation.`};
	}
	// Never start fresh behind the user's back: say how to (S) instead.
	if (missing) return {failed: true, note: `${agent.label} has no saved conversation ${missing}. Press S to start a fresh conversation.`};
	const ref = agent.exitRef?.(output);
	if (ref && forkParent && ref.value === forkParent.value) return {failed: false};
	if (!ref && forkParent && !session.agentSessionRef && !agent.idAtLaunch) {
		return {failed: false, note: `${agent.label} did not report this fork's conversation ID, so it cannot be reopened. Press s to fork the parent again, or S to start a fresh conversation.`};
	}
	return {ref, failed: false};
}

async function prepareAgentSessionRef(ref: AgentSessionRef | undefined): Promise<void> {
	if (ref?.kind === 'path') {
		await fs.mkdir(path.dirname(ref.value), {recursive: true});
	}
}

function resolveShellCommand(): string {
	return process.env.SHELL || '/bin/sh';
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

// A fork copies the parent's conversation, whose paths all point into the parent's worktree. When the child runs in
// another worktree, its first message says where it now is so it does not keep working on the parent's files.
function movedForkPrompt(child: SessionRecord, parentRoot: string, childRoot: string): string {
	const branch = child.worktree?.branch ? ` on branch ${child.worktree.branch}` : '';
	return `Deckhand note: this conversation was forked from another session into a different worktree. You are now in ${childRoot}${branch}. Earlier messages refer to files under ${parentRoot}, which is the parent session's worktree: do not read or edit anything there. Work only in ${childRoot}. Changes the parent had not committed are not in this worktree. Reply briefly to confirm, then wait for instructions.`;
}

async function realpathOrResolve(target: string): Promise<string> {
	return fs.realpath(target).catch(() => path.resolve(target));
}

/** Whether a checkout root is a linked worktree (not the main checkout). Git reports both paths the same way. */
async function isLinkedWorktreeRoot(root: string): Promise<boolean> {
	const worktrees = await listWorktrees(root).catch(() => []);
	return worktrees.some(item => !item.isMain && path.resolve(item.path) === path.resolve(root));
}

// Sessions as the daemon holds and sends them: setting one projects its worktree record's markers into it (merged,
// deleted; src/worktreeRecords.ts), so every reader sees the markers its worktree's sessions share. persist() strips them.
class SessionMap extends Map<string, SessionRecord> {
	constructor(private readonly worktrees: ReadonlyMap<string, WorktreeRecord>) { super(); }
	override set(id: string, session: SessionRecord): this {
		return super.set(id, projectWorktree(session, this.worktrees));
	}
}

export class InkDaemon {
	/** Linked worktree incarnations by ID (persisted with the sessions); the only copy of their merge/deleted markers. */
	private readonly worktrees = new Map<string, WorktreeRecord>();
	private readonly sessions = new SessionMap(this.worktrees);
	private readonly workers = new Map<string, WorkerRuntime>();
	private readonly workspaces = new Map<string, WorkspaceRuntime>();
	private readonly workspaceStarts = new Map<string, Promise<WorkspaceRuntime>>();
	private readonly clients = new Map<net.Socket, ClientSubscription>();
	private server?: net.Server;
	private shuttingDown = false;
	private persistInFlight: Promise<void> = Promise.resolve();
	private persistQueued?: Promise<void>;
	private readonly setupProcesses = new Map<string, ChildProcess>();
	private readonly cleanupWorktrees = new Set<string>();
	private readonly preparingSessions = new Set<string>();
	private readonly summaries = new Map<string, {key: string; at: number; result: Promise<WorkspaceSummary>}>();
	private readonly changeWatches = new Map<string, ChangesWatch>();
	private readonly agentVersions = new AgentVersionChecker();
	/** The last `agent-versions-updated` sent (JSON), so only changes are broadcast. */
	private publishedAgentVersions?: string;
	private agentVersionsTimer?: NodeJS.Timeout;

	async start(): Promise<void> {
		await ensureConfigDir();
		await this.log(`starting daemon pid=${process.pid}`);
		await this.assertNoLiveDaemonFromPidFile();
		await ensureNodePtyReady();
		// If this daemon is starting, any previously-running sessions belonged to a
		// different daemon process and their node-pty handles are gone. Mark them
		// exited as crash/restart recovery, not as normal frontend quit behavior.
		const stored = await markAllNonExitedSessionsExited();
		for (const record of stored.worktrees) this.worktrees.set(record.id, record);
		for (const session of stored.sessions) {
			this.sessions.set(session.id, session);
		}

		await this.prepareSocket();
		await this.listen();
		await fs.chmod(getSocketPath(), 0o600);
		await this.writePidFile();
		this.setupProcessHandlers();
		await this.log(`daemon ready socket=${getSocketPath()}`);
		// Latest agent releases: looked up in the background now, then again once they are LATEST_MAX_AGE_MS old.
		void this.refreshAgentVersions(false);
		this.agentVersionsTimer = setInterval(() => void this.refreshAgentVersions(false), AGENT_VERSIONS_TICK_MS);
		this.agentVersionsTimer.unref();
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
		if (this.agentVersionsTimer) clearInterval(this.agentVersionsTimer);
		for (const child of this.setupProcesses.values()) { try { if (child.pid) process.kill(-child.pid, 'SIGTERM'); } catch {} }
		await this.log('cleanup start');
		for (const socket of this.clients.keys()) {
			socket.destroy();
		}
		this.clients.clear();

		for (const key of [...this.changeWatches.keys()]) this.stopChangesWatch(key);
		// Every Terminal, Git and Dev pane lives in a workspace worker.
		await Promise.all([...this.workspaces.keys()].map(key => this.retireWorkspace(key)));

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
				const worker = this.workers.get(attachedSessionId);
				if (worker?.attached.agent === socket) {
					worker.attached.agent = undefined;
					this.sendWorkerEvent(attachedSessionId, {type: 'detach', target: 'agent'});
				}
				attachedSessionId = undefined;
			}
			for (const workspace of this.workspaces.values()) {
				for (const pane of Object.keys(workspace.attached) as WorkspacePane[]) {
					if (workspace.attached[pane]?.socket !== socket) continue;
					workspace.attached[pane] = undefined;
					this.sendChannelEvent(workspace, {type: 'detach', target: pane});
				}
			}
			this.clients.delete(socket);
			this.syncChangeWatches();
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
				case 'save-config': {
					// Saving never runs anything. A repository file saved here stays trusted when the version it replaces was
					// trusted (or absent), never newly trusting a creation hook; see savedProjectTrust.
					const saved = message.target === 'global' ? await saveGlobalDefaultsDocument(message.raw, message.revision) : await saveProjectConfigDocument(message.cwd, message.raw, message.revision, {keepTrust: true});
					sendMessage(socket, response(message.requestId, saved)); return;
				}
				case 'settings-info': sendMessage(socket, response(message.requestId, await readSettingsInfo(message.cwd))); return;
				case 'worktree-candidates': sendMessage(socket, response(message.requestId, await readWorktreeCandidates(message.cwd))); return;
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
					// Only an in-flight setup is cancelled; pending/failed setup stays retryable as-is.
					const setupRunning = Boolean(child) || session.setup?.state === 'running';
					await this.saveSession({...session, status: 'exited', exitReason: 'stopped', lastPreview: setupRunning ? 'Setup cancelled; worktree retained.' : 'Startup cancelled. In-flight worktree preparation may finish, but the agent will not launch.', setup: setupRunning && session.setup ? {...session.setup, state: 'cancelled', output: session.setup.output.slice(-SETUP_OUTPUT_STORED_LIMIT)} : session.setup});
					sendMessage(socket, response(message.requestId, {ok: true})); return;
				}
				case 'run-action': {
					// Actions run in the workspace's shared Dev pane, so like Dev they need a workspace, not a running agent.
					const {session, key} = this.requireWorkspace(message.sessionId, 'dev');
					// Untrusted repository actions are never run, only global defaults' actions.
					const config = await loadAppConfig(), project = await loadProjectConfig(session.cwd, config);
					const actions = resolveSettings(project, config).actions ?? {};
					const command = Object.hasOwn(actions, message.action) ? actions[message.action] : undefined;
					if (!command && Object.hasOwn(project.config.actions ?? {}, message.action)) throw new Error('Review and trust deckhand.json first (press e or T)');
					if (!command) throw new Error('Unknown project action');
					if (this.workspaceDevLive(key)) throw new Error('Stop the current Dev/action command before starting another');
					sendMessage(socket, response(message.requestId, await this.startWorkspaceDev(session.id, key, command, message.cols, message.rows))); return;
				}
				case 'agent-versions': {
					if (message.refresh) await this.agentVersions.refreshLatest(true);
					sendMessage(socket, response(message.requestId, await this.publishAgentVersions()));
					return;
				}
				case 'update-agent': sendMessage(socket, response(message.requestId, await this.updateAgent(message.program))); return;
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
				// Terminal, Git and Dev requests name a session; they act on the one pane shared by every session in its workspace.
				case 'watch-terminal': {
					const client = this.getClient(socket);
					client.watchedTerminalSessionId = message.sessionId;
					client.terminalCols = clampSize(message.cols, client.terminalCols);
					client.terminalRows = clampSize(message.rows, client.terminalRows);
					const terminal = await this.watchWorkspacePane('terminal', message.sessionId, client.terminalCols, client.terminalRows);
					sendMessage(socket, response(message.requestId, terminal));
					return;
				}
				case 'watch-git': {
					const client = this.getClient(socket);
					client.watchedGitSessionId = message.sessionId;
					client.gitCols = clampSize(message.cols, client.gitCols);
					client.gitRows = clampSize(message.rows, client.gitRows);
					const git = await this.watchWorkspacePane('git', message.sessionId, client.gitCols, client.gitRows);
					sendMessage(socket, response(message.requestId, git));
					return;
				}
				case 'watch-dev': {
					const client = this.getClient(socket);
					client.watchedDevSessionId = message.sessionId;
					client.devCols = clampSize(message.cols, client.devCols);
					client.devRows = clampSize(message.rows, client.devRows);
					const dev = await this.watchWorkspacePane('dev', message.sessionId, client.devCols, client.devRows);
					sendMessage(socket, response(message.requestId, dev));
					return;
				}
				// The Git tab's Changes view: computed here from the workspace (no worker), pushed while watched.
				case 'watch-changes': {
					this.getClient(socket).watchedChangesSessionId = message.sessionId;
					this.syncChangeWatches();
					sendMessage(socket, response(message.requestId, await this.watchChanges(message.sessionId)));
					return;
				}
				case 'changes-diff': {
					const {key} = this.requireWorkspace(message.sessionId, 'git');
					const watch = this.changesWatch(key);
					const recent = watch.last && Date.now() - watch.last.at < CHANGES_FRESH_MS ? watch.last.snapshot : await this.refreshChanges(key);
					if (recent.record.error) throw new Error(recent.record.error);
					const entry = findChange(recent.all, message.group, message.path);
					if (!entry) throw new Error(`${message.path} is not in the current changes`);
					sendMessage(socket, response(message.requestId, await this.changesExclusive(watch, () => readChangeDiff(key, entry))));
					this.syncChangeWatches();
					return;
				}
				case 'change-stage': {
					const {session, key} = this.requireWorkspace(message.sessionId, 'git');
					const watch = this.changesWatch(key);
					const target = message.path !== undefined && message.group ? {group: message.group, path: message.path} : undefined;
					try {
						const result = await this.changesExclusive(watch, async () => {
							this.assertWorkspaceAvailable(key);
							// Validated against a status read inside the same exclusive section, so the paths are current.
							const current = await readChanges(key, watch.untracked);
							return applyStage(key, current, message.mode, target);
						});
						const snapshot = await this.refreshChanges(key);
						sendMessage(socket, response(message.requestId, {...result, changes: {...snapshot.record, sessionId: session.id, workspace: key}}));
					} finally { this.syncChangeWatches(); }
					return;
				}
				case 'start-dev': {
					const {session, key} = this.requireWorkspace(message.sessionId, 'dev');
					const command = await this.resolveSessionDevCommand(session);
					sendMessage(socket, response(message.requestId, await this.startWorkspaceDev(session.id, key, command, message.cols, message.rows)));
					return;
				}
				case 'stop-dev': {
					const key = this.workspaceKeyOf(this.requireSession(message.sessionId));
					if (key) await this.stopWorkspaceDev(key);
					sendMessage(socket, response(message.requestId, {ok: true}));
					return;
				}
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
				case 'attach-terminal':
				case 'attach-git':
				case 'attach-dev': {
					// Workspace pane attaches are released by scanning the workspaces when the socket closes.
					const pane = message.type === 'attach-terminal' ? 'terminal' : message.type === 'attach-git' ? 'git' : 'dev';
					await this.attachWorkspacePane(socket, message.requestId, pane, message.sessionId, message.cols, message.rows);
					return;
				}
				case 'terminal-input':
				case 'git-input':
				case 'dev-input': {
					const workspace = this.workspaceOfSession(message.sessionId);
					if (workspace) this.sendChannelEvent(workspace, {type: 'input', target: message.type.slice(0, -'-input'.length), data: message.data});
					return;
				}
				case 'terminal-resize':
				case 'git-resize':
				case 'dev-resize': {
					const workspace = this.workspaceOfSession(message.sessionId);
					if (workspace) this.sendChannelEvent(workspace, {type: 'resize', target: message.type.slice(0, -'-resize'.length), cols: message.cols, rows: message.rows});
					return;
				}
				case 'terminal-detach':
				case 'git-detach':
				case 'dev-detach': {
					const pane = message.type.slice(0, -'-detach'.length) as WorkspacePane;
					const workspace = this.workspaceOfSession(message.sessionId);
					if (workspace?.attached[pane]?.socket === socket) {
						workspace.attached[pane] = undefined;
						this.sendChannelEvent(workspace, {type: 'detach', target: pane});
					}
					sendMessage(socket, {type: `${pane}-detached`, sessionId: message.sessionId} as ServerMessage);
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
		this.setSessionInWorkspace(session);
		await this.persist();
		this.broadcastSessionUpdated(session);
		// A session that joined or left a workspace (prepared, worktree deleted) moves its Changes watchers.
		this.syncChangeWatches();
		return this.requireSession(session.id);
	}

	// devRunning mirrors the workspace's shared Dev (see syncDevRunning), also for sessions that just joined or left it.
	private setSessionInWorkspace(session: SessionRecord): void {
		this.sessions.set(session.id, session);
		const projected = this.requireSession(session.id);
		const devRunning = this.workspaceDevLive(this.workspaceKeyOf(projected));
		if (Boolean(projected.devRunning) !== devRunning) this.sessions.set(session.id, {...projected, devRunning});
	}

	// Merges onto the latest record so work that awaited in between never writes back a stale copy.
	private patchSession(sessionId: string, patch: Partial<SessionRecord>): SessionRecord | undefined {
		const current = this.sessions.get(sessionId);
		if (!current) return undefined;
		this.sessions.set(sessionId, {...current, ...patch});
		return this.sessions.get(sessionId);
	}

	/** The record of the linked worktree incarnation the session runs in, if any (main-checkout sessions have none). */
	private worktreeRecordOf(session: SessionRecord): WorktreeRecord | undefined {
		return session.worktree?.id ? this.worktrees.get(session.worktree.id) : undefined;
	}

	// Saves a worktree record and re-projects it into every session of that incarnation: they all change together
	// (merged, unmarked, deleted), and every client hears about each of them.
	private async saveWorktreeRecord(record: WorktreeRecord): Promise<void> {
		this.worktrees.set(record.id, record);
		const now = new Date().toISOString();
		const affected = [...this.sessions.values()].filter(session => session.worktree?.id === record.id);
		for (const session of affected) this.setSessionInWorkspace({...session, updatedAt: now});
		await this.persist();
		for (const session of affected) this.broadcastSessionUpdated(session);
		this.syncChangeWatches();
	}

	/**
	 * The incarnation a session that now runs in the linked worktree at `worktreePath` joins: the live one there, unless
	 * Deckhand just created the worktree (`created`), which starts a new one. A live incarnation still recorded at that
	 * path was then removed outside Deckhand: it is marked deleted, so its sessions never share the new worktree.
	 * Synchronous from lookup to the caller's set, so the record cannot be dropped as unreferenced in between.
	 */
	private joinWorktree(worktreePath: string, created: boolean): {id: string; superseded?: WorktreeRecord} {
		const live = liveWorktreeRecord(this.worktrees.values(), worktreePath);
		if (live && !created) return {id: live.id};
		const record: WorktreeRecord = {id: randomUUID(), path: path.resolve(worktreePath), createdAt: new Date().toISOString()};
		this.worktrees.set(record.id, record);
		const superseded = live && {...live, deletedAt: record.createdAt};
		// Marked at once (the caller saves and broadcasts it), so there is never a second live record at the path.
		if (superseded) this.worktrees.set(superseded.id, superseded);
		return {id: record.id, superseded};
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
		// Deckhand chose this conversation's ID at launch (forks included); another ID (e.g. after /clear) is not its identity.
		if (agentSpec(session.program).idAtLaunch && session.agentSessionRef?.kind === 'id') return undefined;
		if (session.subSessionKind === 'forked') {
			// A fork (e.g. a Codex child, which reports its own ID, or a legacy /branch child) never adopts its parent's identity.
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
		// The version this launch runs (read beside the spawn; cached per binary, so usually instant).
		const launchVersion = this.agentVersions.versionOf(session.command).then(result => result.version, () => undefined);
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
		const agentVersion = await launchVersion;
		this.patchSession(session.id, {agentVersion});
		// A launch may be the first to see a new installed version (updated outside Deckhand).
		void this.publishAgentVersions();
		return {...started, agentVersion};
	}

	private stopWorker(worker: WorkerRuntime): void {
		try { if (worker.process.connected) worker.process.send?.({type: 'kill', requestId: randomUUID(), force: true}); } catch {}
		setTimeout(() => { if (worker.process.exitCode === null && worker.process.signalCode === null) { try { worker.process.kill('SIGKILL'); } catch {} } }, WORKER_KILL_GRACE_MS).unref?.();
	}

	private sendWorkerRequest<T>(sessionId: string, payload: Record<string, unknown>): Promise<T> {
		const worker = this.workers.get(sessionId);
		if (!worker) return Promise.reject(new Error('session worker is not running'));
		return this.requestChannel<T>(worker, payload);
	}

	private requestChannel<T>(worker: WorkerChannel, payload: Record<string, unknown>): Promise<T> {
		if (worker.exited || !worker.process.connected) {
			return Promise.reject(new Error('worker is not running'));
		}
		const requestId = randomUUID();
		return new Promise<T>((resolve, reject) => {
			const timer = setTimeout(() => {
				worker.pending.delete(requestId);
				reject(new Error('worker request timed out'));
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
		if (worker) this.sendChannelEvent(worker, payload);
	}

	private sendChannelEvent(worker: WorkerChannel, payload: Record<string, unknown>): void {
		if (!worker.exited && worker.process.connected) {
			worker.process.send?.(payload);
		}
	}

	private settleResponse(worker: WorkerChannel, message: WorkerResponse): void {
		const pending = worker.pending.get(message.requestId);
		if (!pending) return;
		clearTimeout(pending.timer);
		worker.pending.delete(message.requestId);
		message.ok ? pending.resolve(message.data) : pending.reject(new Error(message.error));
	}

	private rejectPending(worker: WorkerChannel, reason: string): void {
		for (const pending of worker.pending.values()) {
			clearTimeout(pending.timer);
			pending.reject(new Error(reason));
		}
		worker.pending.clear();
	}


	private async handleWorkerMessage(sessionId: string, message: WorkerEvent, worker: WorkerRuntime): Promise<void> {
		if (message.type === 'response') {
			this.settleResponse(worker, message);
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
		if (message.type === 'output') {
			const attached = worker.attached.agent;
			if (attached && !attached.destroyed) sendMessage(attached, {type: 'output', sessionId, data: message.data});
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
		const agentExit = readAgentExit(existing, lastPreview);
		// node-pty reports a signal death (e.g. SIGKILL) as {exitCode: 0, signal: 9}: that is not a completion.
		const exitReason = existing.exitReason === 'stopped' ? 'stopped' : agentExit.failed ? 'failed' : exitCode === null || exitSignal ? 'interrupted' : exitCode !== 0 ? 'failed' : 'completed';
		this.sessions.set(sessionId, {
			...existing,
			...(agentExit.ref ? {agentSessionRef: agentExit.ref} : {}),
			status: 'exited',
			agentStatus: 'idle',
			agentStatusUpdatedAt: now,
			updatedAt: now,
			pid: undefined,
			exitCode,
			exitSignal,
			lastPreview: agentExit.note ? `${lastPreview}\n\n${agentExit.note}` : lastPreview,
			exitReason,
		});
		await fs.rm(getWorkerPidPath(sessionId), {force: true}).catch(() => {});
		const worktree = existing.worktree;
		if (worker?.deleteWorktreeOnExit && worktree?.path) {
			const cleanupKey = path.resolve(worktree.path);
			this.cleanupWorktrees.add(cleanupKey);
			// Every write below patches the latest record: notes/archive edits may land during these awaits. The deletion
			// marks the worktree's incarnation, so every session of it loses its workspace and restart/merge.
			const markDeleted = async () => {
				const record = this.worktreeRecordOf(this.requireSession(sessionId));
				if (record) await this.saveWorktreeRecord({...record, deletedAt: new Date().toISOString()});
				else this.patchSession(sessionId, {worktree: {...(this.sessions.get(sessionId)?.worktree ?? worktree), deletedAt: new Date().toISOString()}, updatedAt: new Date().toISOString()});
			};
			try {
				const repoCwd = existing.launchWorktreeRoot ?? existing.repoRoot;
				await this.assertCleanupAllowed(sessionId, Boolean(worker.deleteBranchOnExit), worker.allowDataLoss ?? false);
				// The workspace's shared Terminal/Git/Dev run inside the worktree: stop them (and wait) before removal.
				const workspace = workspaceKey(existing);
				if (workspace) { this.stopChangesWatch(workspace); await this.retireWorkspace(workspace); }
				await removeWorktree(worktree.path, repoCwd, worktree.name);
				await markDeleted();
				if (worker.deleteBranchOnExit && worktree.branch) await deleteLocalBranch(repoCwd, worktree.branch);
			} catch (error) {
				const gone = await fs.stat(worktree.path).then(() => false, statError => (statError as NodeJS.ErrnoException).code === 'ENOENT');
				if (gone && !this.sessions.get(sessionId)?.worktree?.deletedAt) await markDeleted();
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

	/** Every agent's versions; broadcast to subscribed clients when they changed since the last broadcast. */
	private async publishAgentVersions(): Promise<AgentVersions> {
		const versions = await this.agentVersions.snapshot(this.sessions.values());
		const json = JSON.stringify(versions);
		if (json !== this.publishedAgentVersions) {
			this.publishedAgentVersions = json;
			for (const [socket, client] of this.clients.entries()) {
				if (client.repoRoot) sendMessage(socket, {type: 'agent-versions-updated', versions});
			}
		}
		return versions;
	}

	private async refreshAgentVersions(force: boolean): Promise<void> {
		try {
			await this.agentVersions.refreshLatest(force);
			await this.publishAgentVersions();
		} catch (error) {
			await this.log(`agent version check failed: ${errorMessage(error)}`);
		}
	}

	/** Runs the agent's update command and reports the new versions; sessions are never touched (they keep running their version). */
	private async updateAgent(program: SessionRecord['program']): Promise<AgentUpdateResult> {
		if (!Object.hasOwn(AGENTS, program)) throw new Error(`Unknown agent ${String(program)}`);
		const outcome = await this.agentVersions.update(program, () => void this.publishAgentVersions());
		await this.log(`agent update ${outcome.command}: exit ${outcome.exitCode} (${outcome.before ?? '?'} -> ${outcome.after ?? '?'})`);
		return {program, ...outcome, versions: await this.publishAgentVersions()};
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

	/** The session's workspace (its worktree root), or undefined while it has none (deleted, or still being prepared). */
	private workspaceKeyOf(session: SessionRecord): string | undefined {
		return this.preparingSessions.has(session.id) ? undefined : workspaceKey(session);
	}

	private requireWorkspace(sessionId: string, pane: WorkspacePane): {session: SessionRecord; key: string} {
		const session = this.requireSession(sessionId);
		const key = this.workspaceKeyOf(session);
		if (!key) throw new Error(`${PANE_LABELS[pane]} is unavailable: ${noWorkspaceReason(session)}`);
		return {session, key};
	}

	private workspaceOfSession(sessionId: string): WorkspaceRuntime | undefined {
		const session = this.sessions.get(sessionId);
		const key = session ? this.workspaceKeyOf(session) : undefined;
		return key ? this.workspaces.get(key) : undefined;
	}

	private workspaceDevLive(key: string | undefined): boolean {
		return Boolean(key && this.workspaces.get(key)?.records.dev?.live);
	}

	/** The workspace's worker, started on demand; refuses a worktree that is being deleted or is gone from disk. */
	private async openWorkspace(key: string): Promise<WorkspaceRuntime> {
		this.assertWorkspaceAvailable(key);
		const existing = this.workspaces.get(key);
		if (existing) return existing;
		const present = await fs.stat(key).then(stat => stat.isDirectory(), () => false);
		if (!present) throw new Error(`Worktree directory is missing: ${key}`);
		this.assertWorkspaceAvailable(key);
		return this.ensureWorkspace(key);
	}

	private async ensureWorkspace(key: string): Promise<WorkspaceRuntime> {
		const existing = this.workspaces.get(key);
		if (existing) return existing;
		const pending = this.workspaceStarts.get(key);
		if (pending) return pending;
		const start = this.startWorkspaceWorker(key);
		this.workspaceStarts.set(key, start);
		try {
			return await start;
		} finally {
			if (this.workspaceStarts.get(key) === start) this.workspaceStarts.delete(key);
		}
	}

	private async startWorkspaceWorker(key: string): Promise<WorkspaceRuntime> {
		await fs.mkdir(getWorkerDir(), {recursive: true});
		const id = workspaceWorkerId(key);
		const child = fork(getCliEntryPath(), ['--workspace-worker'], {stdio: ['ignore', 'pipe', 'pipe', 'ipc']});
		const workspace: WorkspaceRuntime = {key, process: child, pending: new Map(), attached: {}, records: {}, busy: 0};
		// Wire every listener before the first await so no worker event can be missed.
		const logPath = getWorkerLogPath(id);
		child.stdout?.on('data', chunk => void fs.appendFile(logPath, chunk).catch(() => {}));
		child.stderr?.on('data', chunk => void fs.appendFile(logPath, chunk).catch(() => {}));
		child.on('message', message => void this.handleWorkspaceMessage(workspace, message as WorkspaceEvent));
		child.on('exit', () => void this.handleWorkspaceExit(workspace));
		this.workspaces.set(key, workspace);
		try {
			await fs.writeFile(getWorkerPidPath(id), String(child.pid ?? ''), 'utf8').catch(() => {});
			await this.requestChannel(workspace, {type: 'start', cwd: key});
		} catch (error) {
			if (this.workspaces.get(key) === workspace) this.workspaces.delete(key);
			try { child.kill('SIGKILL'); } catch {}
			throw error;
		}
		await this.log(`workspace worker pid=${child.pid ?? '?'} started for ${key}`);
		return workspace;
	}

	/** A pane request (snapshot, attach, start-dev). A failed one (e.g. lazygit is not installed) may leave the worker empty: retire it then. */
	private async requestPane<T>(workspace: WorkspaceRuntime, payload: Record<string, unknown>): Promise<T> {
		workspace.busy += 1;
		try {
			return await this.requestChannel<T>(workspace, payload);
		} catch (error) {
			void this.retireIfIdle(workspace);
			throw error;
		} finally {
			workspace.busy -= 1;
		}
	}

	// A worker is retired once nothing lives in it (no pane, live or exited with output, and none starting) and no
	// pane request is in flight. Its own panes never make it idle: a shell never exits on its own, and an exited
	// pane keeps its output visible.
	private async retireIfIdle(workspace: WorkspaceRuntime, idle?: boolean): Promise<void> {
		if (this.workspaces.get(workspace.key) !== workspace) return;
		idle ??= (await this.requestChannel<{idle: boolean}>(workspace, {type: 'idle'}).catch(() => ({idle: false}))).idle;
		if (!idle || workspace.busy > 0 || this.workspaces.get(workspace.key) !== workspace) return;
		await this.retireWorkspace(workspace.key).catch(error => this.log(`stopping workspace ${workspace.key} failed: ${errorMessage(error)}`));
	}

	private async handleWorkspaceMessage(workspace: WorkspaceRuntime, message: WorkspaceEvent): Promise<void> {
		if (message.type === 'response') {
			this.settleResponse(workspace, message);
			return;
		}
		if (this.workspaces.get(workspace.key) !== workspace) return;
		if (message.type === 'output') {
			const attached = workspace.attached[message.target];
			if (attached && !attached.socket.destroyed) sendMessage(attached.socket, {type: `${message.target}-output`, sessionId: attached.sessionId, data: message.data} as ServerMessage);
			return;
		}
		const [pane, record] = updatedPaneRecord(message);
		workspace.records[pane] = record;
		this.publishWorkspacePane(workspace.key, pane, record);
		if (pane === 'dev') await this.syncDevRunning(workspace.key);
	}

	private async handleWorkspaceExit(workspace: WorkspaceRuntime): Promise<void> {
		workspace.exited = true;
		this.rejectPending(workspace, 'workspace worker exited');
		// A retired worker is finished up by retireWorkspace.
		if (this.workspaces.get(workspace.key) !== workspace) return;
		this.workspaces.delete(workspace.key);
		await fs.rm(getWorkerPidPath(workspaceWorkerId(workspace.key)), {force: true}).catch(() => {});
		await this.log(`workspace worker for ${workspace.key} exited unexpectedly`);
		this.detachWorkspaceClients(workspace);
		// Viewers keep the last output, marked stopped; viewing Terminal/Git again starts a new worker.
		for (const [pane, last] of Object.entries(workspace.records) as Array<[WorkspacePane, WorkspacePaneRecord]>) {
			this.publishWorkspacePane(workspace.key, pane, {content: last.content, live: false, cwd: workspace.key, command: last.command});
		}
		await this.syncDevRunning(workspace.key);
	}

	/** Stops a workspace's worker and every pane in it (Terminal, Git, Dev), waiting (bounded) until it exited. */
	private async retireWorkspace(key: string): Promise<void> {
		const workspace = this.workspaces.get(key) ?? await this.workspaceStarts.get(key)?.catch(() => undefined);
		if (!workspace || this.workspaces.get(key) !== workspace) return;
		this.workspaces.delete(key);
		const exited = new Promise<void>(resolve => {
			if (workspace.process.exitCode !== null || workspace.process.signalCode !== null) return resolve();
			const timer = setTimeout(() => { try { workspace.process.kill('SIGKILL'); } catch {} resolve(); }, WORKSPACE_SHUTDOWN_TIMEOUT_MS);
			workspace.process.once('exit', () => { clearTimeout(timer); resolve(); });
		});
		this.sendChannelEvent(workspace, {type: 'shutdown', requestId: randomUUID()});
		await exited;
		workspace.exited = true;
		this.rejectPending(workspace, 'workspace worker stopped');
		await fs.rm(getWorkerPidPath(workspaceWorkerId(key)), {force: true}).catch(() => {});
		this.detachWorkspaceClients(workspace);
		// Panes started again meanwhile run in a new worker; leave their state alone.
		if (this.shuttingDown || this.workspaces.has(key)) return;
		for (const pane of Object.keys(workspace.records) as WorkspacePane[]) this.publishWorkspacePane(key, pane, {content: '', live: false, cwd: key});
		await this.syncDevRunning(key);
	}

	private detachWorkspaceClients(workspace: WorkspaceRuntime): void {
		const attached = workspace.attached;
		workspace.attached = {};
		for (const [pane, viewer] of Object.entries(attached) as Array<[WorkspacePane, {socket: net.Socket; sessionId: string} | undefined]>) {
			if (viewer && !viewer.socket.destroyed) sendMessage(viewer.socket, {type: `${pane}-detached`, sessionId: viewer.sessionId} as ServerMessage);
		}
	}

	// Terminal and Git start on first view or attach (a shell or lazygit that exited starts again on the next view);
	// Dev only through start-dev/run-action, so a workspace without a worker has no Dev to show.
	private async watchWorkspacePane(pane: WorkspacePane, sessionId: string | undefined, cols: number, rows: number): Promise<WorkspacePaneRecord> {
		if (!sessionId) return {content: '', live: false};
		const session = this.sessions.get(sessionId);
		if (!session) return {sessionId, content: '', live: false};
		const key = this.workspaceKeyOf(session);
		// No `workspace` on the record tells the pane there is none yet (or any more).
		if (!key) return {sessionId, content: '', live: false, cwd: session.cwd};
		const workspace = pane === 'dev' ? this.workspaces.get(key) : await this.openWorkspace(key);
		if (!workspace) return {sessionId, content: '', live: false, cwd: key, workspace: key};
		// Last viewer to size the shared pane wins.
		return {...await this.requestPane<WorkspacePaneRecord>(workspace, {type: 'snapshot', target: pane, cols, rows}), sessionId, workspace: key};
	}

	// One attach per workspace pane; output goes to the attaching socket under its session ID.
	private async attachWorkspacePane(socket: net.Socket, requestId: string, pane: WorkspacePane, sessionId: string, cols?: number, rows?: number): Promise<void> {
		const {session, key} = this.requireWorkspace(sessionId, pane);
		const workspace = pane === 'dev' ? this.workspaces.get(key) : await this.openWorkspace(key);
		if (!workspace) throw new Error('no running dev command; press d to start it');
		const current = workspace.attached[pane];
		if (current && current.socket !== socket && !current.socket.destroyed) throw new Error(`${PANE_ATTACH_NAMES[pane]} is already attached elsewhere`);
		const attachData = await this.requestPane<object & {initialFrame?: string}>(workspace, {type: 'attach', target: pane, cols: clampSize(cols ?? DEFAULT_PREVIEW_COLS, DEFAULT_PREVIEW_COLS), rows: clampSize(rows ?? DEFAULT_PREVIEW_ROWS, DEFAULT_PREVIEW_ROWS)});
		workspace.attached[pane] = {socket, sessionId: session.id};
		// attachData carries terminalModes, so attach mode can mirror the pane's bracketed-paste state.
		sendMessage(socket, response(requestId, {...session, ...attachData}));
		sendMessage(socket, {type: `${pane}-attached`, sessionId: session.id} as ServerMessage);
		if (attachData.initialFrame) sendMessage(socket, {type: `${pane}-output`, sessionId: session.id, data: attachData.initialFrame} as ServerMessage);
	}

	private async startWorkspaceDev(sessionId: string, key: string, command: string, cols: number, rows: number): Promise<DevRecord> {
		const workspace = await this.openWorkspace(key);
		const dev = await this.requestPane<DevRecord>(workspace, {type: 'start-dev', command, cols: clampSize(cols, DEFAULT_PREVIEW_COLS), rows: clampSize(rows, DEFAULT_PREVIEW_ROWS)});
		if (this.workspaces.get(key) === workspace) {
			workspace.records.dev = dev;
			this.publishWorkspacePane(key, 'dev', dev);
			await this.syncDevRunning(key);
		}
		return {...dev, sessionId, workspace: key};
	}

	private async stopWorkspaceDev(key: string): Promise<void> {
		const workspace = this.workspaces.get(key);
		if (!workspace) return;
		const {idle} = await this.requestChannel<{idle: boolean}>(workspace, {type: 'stop-dev'});
		// Nothing else (Terminal, Git) lives in the worker: retire it (its PTYs were already signalled).
		void this.retireIfIdle(workspace, idle);
	}

	// Every client watching any session of the workspace sees the one shared pane, stamped with the session it watches.
	private publishWorkspacePane(key: string, pane: WorkspacePane, record: WorkspacePaneRecord): void {
		for (const [socket, client] of this.clients.entries()) {
			const watched = pane === 'terminal' ? client.watchedTerminalSessionId : pane === 'git' ? client.watchedGitSessionId : client.watchedDevSessionId;
			const session = watched ? this.sessions.get(watched) : undefined;
			if (session && this.workspaceKeyOf(session) === key) sendMessage(socket, paneUpdatedMessage(pane, {...record, sessionId: session.id, workspace: key}));
		}
	}

	// The Git tab's Changes view. A workspace is polled (every CHANGES_POLL_MS, pushing only when the result changed)
	// while at least one client watches one of its sessions; one queue per workspace keeps its Git runs from overlapping.
	private changesWatch(key: string): ChangesWatch {
		let watch = this.changeWatches.get(key);
		if (!watch) { watch = {key, queue: Promise.resolve(), running: false, untracked: new Map()}; this.changeWatches.set(key, watch); }
		return watch;
	}

	private changesExclusive<T>(watch: ChangesWatch, run: () => Promise<T>): Promise<T> {
		const next = watch.queue.then(run, run);
		watch.queue = next.catch(() => {});
		return next;
	}

	/** A fresh status read (shared with a read already queued and not started), pushed to watchers when it changed. */
	private refreshChanges(key: string): Promise<ChangesSnapshot> {
		const watch = this.changesWatch(key);
		if (watch.queued) return watch.queued;
		const queued: Promise<ChangesSnapshot> = this.changesExclusive(watch, async () => {
			if (watch.queued === queued) watch.queued = undefined;
			watch.running = true;
			try {
				let snapshot: ChangesSnapshot;
				try {
					this.assertWorkspaceAvailable(key);
					snapshot = await readChanges(key, watch.untracked);
				} catch (error) {
					snapshot = {record: emptyChanges({loaded: true, error: errorMessage(error).split('\n')[0]}), all: [], hasHead: false};
				}
				watch.last = {snapshot, at: Date.now()};
				this.publishChanges(watch, snapshot.record);
				return snapshot;
			} finally { watch.running = false; }
		});
		watch.queued = queued;
		return queued;
	}

	private publishChanges(watch: ChangesWatch, record: ChangesRecord): void {
		const json = JSON.stringify(record);
		if (json === watch.published) return;
		watch.published = json;
		for (const [socket, client] of this.clients.entries()) {
			const session = client.watchedChangesSessionId ? this.sessions.get(client.watchedChangesSessionId) : undefined;
			if (session && this.workspaceKeyOf(session) === watch.key) sendMessage(socket, {type: 'changes-updated', changes: {...record, sessionId: session.id, workspace: watch.key}});
		}
	}

	private async watchChanges(sessionId: string | undefined): Promise<ChangesRecord> {
		if (!sessionId) return emptyChanges();
		const session = this.sessions.get(sessionId);
		if (!session) return emptyChanges({sessionId});
		const key = this.workspaceKeyOf(session);
		// No `workspace` on the record tells the pane there is none yet (or any more).
		if (!key) return emptyChanges({sessionId, loaded: true});
		return {...(await this.refreshChanges(key)).record, sessionId, workspace: key};
	}

	/** Polls exactly the workspaces some client watches; the others stop (and are forgotten). */
	private syncChangeWatches(): void {
		const watched = new Set<string>();
		for (const client of this.clients.values()) {
			const session = client.watchedChangesSessionId ? this.sessions.get(client.watchedChangesSessionId) : undefined;
			const key = session ? this.workspaceKeyOf(session) : undefined;
			if (key) watched.add(key);
		}
		for (const key of [...this.changeWatches.keys()]) if (!watched.has(key)) this.stopChangesWatch(key);
		if (this.shuttingDown) return;
		for (const key of watched) {
			const watch = this.changesWatch(key);
			if (watch.timer) continue;
			watch.timer = setInterval(() => { if (!watch.queued && !watch.running) void this.refreshChanges(key); }, CHANGES_POLL_MS);
			watch.timer.unref();
		}
	}

	/** Stops polling a workspace (no watchers left, its worktree is being deleted, or the daemon stops). */
	private stopChangesWatch(key: string): void {
		const watch = this.changeWatches.get(key);
		if (!watch) return;
		if (watch.timer) clearInterval(watch.timer);
		this.changeWatches.delete(key);
	}

	// devRunning mirrors the workspace's Dev on every session in it, exited ones included (Dev outlives agents).
	// Sessions that join or leave a workspace are brought in line by saveSession.
	private async syncDevRunning(key: string): Promise<void> {
		const devRunning = this.workspaceDevLive(key);
		const changed: SessionRecord[] = [];
		for (const session of this.sessions.values()) {
			if (Boolean(session.devRunning) === devRunning || this.workspaceKeyOf(session) !== key) continue;
			const updated = {...session, devRunning};
			this.sessions.set(session.id, updated);
			changed.push(updated);
		}
		if (!changed.length) return;
		await this.persist();
		for (const session of changed) this.broadcastSessionUpdated(session);
	}

	// An unreadable deckhand.json is treated as untrusted: fall back to the global dev command.
	private async resolveSessionDevCommand(session: SessionRecord): Promise<string> {
		const appConfig = await loadAppConfig();
		return resolveDevCommand(await loadProjectConfig(session.cwd, appConfig).catch(() => undefined), appConfig);
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
			const agent = agentSpec(parentSession.program);
			if (!agent.forks) {
				throw new Error(`forked sub-sessions are not supported for ${agent.label}`);
			}
			if (input.program !== parentSession.program) {
				throw new Error('forked sub-session must use the parent session program');
			}
			if (!parentSession.agentSessionRef) {
				throw new Error(agent.idAtLaunch
					? 'parent session does not have a resumable agent reference'
					: `${agent.label} has not reported the parent's conversation ID yet (it comes from the SessionStart hook or the exit hint), so it cannot be forked`);
			}
			// Checked here so a fork never fails at agent launch: this agent may reopen the fork in the parent's directory.
			if (!agent.forksAcrossDirectories && ((input.worktreeMode ?? 'none') !== 'none' || path.resolve(input.cwd) !== path.resolve(parentSession.cwd))) {
				throw new Error(`${agent.label} forks stay in the parent's worktree; create a clean sub-session to work in another one`);
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

		const program = startingSession.program;
		const agentName = buildDeckhandAgentName(title, sessionId);
		// A fork copies the parent's conversation as saved when the child launches (a turn in progress is not included).
		const forkFrom = startingSession.subSessionKind === 'forked' ? startingSession.forkedFromAgentSessionRef : undefined;
		const agentSessionRef = startingSession.agentSessionRef ?? newAgentRef(program);
		const plan: LaunchPlan = forkFrom ? {kind: 'fork', parent: forkFrom, ref: agentSessionRef, name: agentName} : {kind: 'new', ref: agentSessionRef, name: agentName};
		const baseRef = worktree.baseRef ?? (await currentBranch(launchWorktreeRoot) || await headSha(launchWorktreeRoot));
		// The linked worktree the session runs in shares its markers with every session there: its own, or for a session
		// without one (e.g. a sub-session) the linked worktree it was launched in. The main checkout has none.
		const linkedRoot = ownWorktreePath(worktree) ?? (requestedWorktreeMode === 'none' && await isLinkedWorktreeRoot(launchWorktreeRoot) ? launchWorktreeRoot : undefined);
		const preparedSession: SessionRecord = {
			...startingSession,
			cwd: sessionCwd,
			args: launchArgs(program, plan),
			agentSessionRef,
			launchWorktreeRoot,
			worktree: {...worktree, baseRef},
			updatedAt: new Date().toISOString(),
		};
		const current = this.sessions.get(sessionId);
		const cancelled = current?.status !== 'starting';
		if (cancelled && !(current && worktree.path)) return;
		// No await from joining the incarnation until the session that references it is set.
		const joined = linkedRoot ? this.joinWorktree(linkedRoot, worktree.origin === 'created') : undefined;
		if (joined) preparedSession.worktree = {...preparedSession.worktree!, id: joined.id};
		if (cancelled) await this.saveSession({...current!, cwd: sessionCwd, launchWorktreeRoot, worktree: preparedSession.worktree, updatedAt: new Date().toISOString()});
		else {
			this.sessions.set(sessionId, preparedSession);
			this.broadcastSessionUpdated(preparedSession);
			this.preparingSessions.delete(sessionId);
		}
		if (joined?.superseded) await this.saveWorktreeRecord(joined.superseded);
		if (cancelled) return;

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
		preparedSession.args = [...(preparedSession.args ?? []), ...await integrationArgs(preparedSession.program, preparedSession.command, appConfig.agent_hooks === true), ...this.firstMessageArgs(preparedSession, plan, true)];
		const launchSession = {...this.requireSession(sessionId), args: preparedSession.args, handoffPath: preparedSession.handoffPath};
		this.sessions.set(sessionId, launchSession);
		const runningSession = await this.startWorker(launchSession, input.cols, input.rows);
		await this.saveSession({...runningSession, ...this.requireSession(sessionId), status: 'running', pid: runningSession.pid});
	}

	private async failStartingSession(sessionId: string, error: unknown, launchId?: string): Promise<void> {
		const session = this.sessions.get(sessionId);
		if (!session || session.status !== 'starting' || (launchId && session.launchId !== launchId)) {
			return;
		}
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

	/** The first message a launch sends: a handoff child's document (first launch only), or the note for a fork into another worktree (every fork). */
	private firstMessageArgs(session: SessionRecord, plan: LaunchPlan, firstLaunch: boolean): string[] {
		if (firstLaunch && session.handoffPath) return ['--', handoffPrompt(session.handoffPath)];
		if (plan.kind !== 'fork') return [];
		const parent = session.forkedFromSessionId ? this.sessions.get(session.forkedFromSessionId) : undefined;
		const parentRoot = parent && workspaceKey(parent), childRoot = workspaceKey(session);
		return parentRoot && childRoot && parentRoot !== childRoot ? ['--', movedForkPrompt(session, parentRoot, childRoot)] : [];
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
		const parsedAgentSessionRef = mode === 'resume' && existing.lastPreview ? readAgentExit(existing, existing.lastPreview).ref : undefined;
		const restartSource = parsedAgentSessionRef ? {...existing, agentSessionRef: parsedAgentSessionRef} : existing;
		const neverStarted = Boolean(existing.launchId) && !existing.agentStartedAt;
		if (neverStarted && existing.requestedWorktreeMode && existing.requestedWorktreeMode !== 'none' && !existing.worktree?.path) throw new Error('Worktree preparation did not complete. Create a new session to retry instead of launching in the original checkout.');
		const agent = agentSpec(existing.program);
		const agentName = buildDeckhandAgentName(existing.title, existing.id, mode === 'fresh' ? `fresh-${Date.now().toString(36)}` : undefined);
		// s resumes the session's own conversation; a fork without one forks its parent again (new child ID); S starts fresh.
		const plan = relaunchPlan(restartSource, mode === 'fresh' ? 'fresh' : 'resume', neverStarted, agentName);
		if (!plan) throw new Error(`${agent.label} conversation ID is unknown. Use S for a fresh session${agent.idAtLaunch ? '' : `, or enable trusted ${agent.label} hooks before starting new sessions`}.`);
		const missingConversation = agent.missingConversation?.(existing.lastPreview ?? '');
		if (plan.kind === 'resume' && missingConversation && missingConversation === plan.ref.value) throw new Error(`${agent.label} has no saved conversation ${missingConversation}. Use S for a fresh session.`);
		const starting: SessionRecord = {
			...restartSource,
			launchId: randomUUID(),
			agentStartedAt: mode === 'fresh' ? undefined : existing.agentStartedAt,
			archivedAt: undefined,
			attention: undefined,
			exitReason: undefined,
			cleanupError: undefined,
			args: launchArgs(existing.program, plan),
			agentSessionRef: plan.ref,
			// A fresh conversation is no longer a copy of the parent's, so s never forks the parent for it.
			...(mode === 'fresh' ? {forkedFromAgentSessionRef: undefined} : {}),
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
			starting.args = [...(starting.args ?? []), ...await integrationArgs(starting.program, starting.command, config.agent_hooks === true), ...this.firstMessageArgs(starting, plan, neverStarted)];
			await prepareAgentSessionRef(starting.agentSessionRef);
			const runningSession = await this.startWorker({...this.requireSession(sessionId), args: starting.args}, cols, rows);
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
			await this.saveMergeMarkers(sessionId, {
				mergedAt: new Date().toISOString(),
				mergeMode: mode,
				mergeTargetBranch: result.targetBranch,
				mergeSourceRef: result.sourceRef,
			});
			await this.log(`${mode} merged ${session.title} (${result.sourceRef}) into ${result.targetBranch}`);
		}
		return result;
	}

	// Replaces the merge markers (undefined: clears them) where they live: on the worktree record of a linked worktree
	// (every session of it changes), else on the session (under `worktree` for the main worktree, else top-level).
	private async saveMergeMarkers(sessionId: string, markers: WorktreeMarkers | undefined): Promise<SessionRecord> {
		const session = this.requireSession(sessionId);
		const record = this.worktreeRecordOf(session);
		if (record) {
			await this.saveWorktreeRecord({...withoutMarkers(record, MERGE_MARKERS), ...markers});
			return this.requireSession(sessionId);
		}
		const updated: SessionRecord = {...withoutMarkers(session, MERGE_MARKERS), updatedAt: new Date().toISOString()};
		const worktree: SessionWorktreeRecord | undefined = session.worktree && withoutMarkers(session.worktree, MERGE_MARKERS);
		if (worktree?.path && worktree.mode !== 'none') updated.worktree = {...worktree, ...markers};
		else Object.assign(updated, markers && {mergedAt: markers.mergedAt, mergeTargetBranch: markers.mergeTargetBranch, mergeSourceRef: markers.mergeSourceRef, mergeMarkedManually: markers.mergeMarkedManually});
		return this.saveSession(updated);
	}

	private async markSessionMerged(sessionId: string, targetCwd: string): Promise<SessionRecord> {
		const session = this.sessions.get(sessionId);
		if (!session) {
			throw new Error('session does not exist');
		}
		const worktree = session.worktree;
		const worktreePath = worktree?.mode !== 'none' ? worktree?.path : undefined;
		if (worktree?.mergedAt || session.mergedAt) {
			const updated = await this.saveMergeMarkers(sessionId, undefined);
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
		const updated = await this.saveMergeMarkers(sessionId, {mergedAt: new Date().toISOString(), mergeTargetBranch: targetBranch, mergeSourceRef: sourceRef, mergeMarkedManually: true});
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
		this.summaries.forEach((_entry, slot) => { if (slot.startsWith(`${sessionId}\0`)) this.summaries.delete(slot); });
		const workspace = this.workspaceKeyOf(existing);
		this.sessions.delete(sessionId);
		// A worktree record lives as long as a session references it.
		const worktreeId = existing.worktree?.id;
		if (worktreeId && ![...this.sessions.values()].some(session => session.worktree?.id === worktreeId)) this.worktrees.delete(worktreeId);
		await this.persist();
		this.broadcastSessionRemoved(existing);
		this.syncChangeWatches();
		// Workspace panes outlive agents but not their workspace's sessions: with the last one gone, nothing could show
		// or stop them (a shell never exits on its own).
		if (workspace && ![...this.sessions.values()].some(session => this.workspaceKeyOf(session) === workspace)) {
			void this.retireWorkspace(workspace).catch(error => this.log(`stopping workspace ${workspace} failed: ${errorMessage(error)}`));
		}
	}

	// Coalesced: at most one write in flight plus one queued. Every caller arriving before the
	// queued write snapshots the sessions shares it, so bursts of updates cost one write. Worktree markers are written
	// only in their records (storedSession drops the projection).
	private persist(): Promise<void> {
		if (this.persistQueued) return this.persistQueued;
		const operation = this.persistInFlight.catch(() => {}).then(() => {
			this.persistQueued = undefined;
			return saveState({sessions: sortSessionsNewestFirst([...this.sessions.values()]).map(storedSession), worktrees: [...this.worktrees.values()]});
		});
		this.persistQueued = operation;
		this.persistInFlight = operation;
		return operation;
	}
}
