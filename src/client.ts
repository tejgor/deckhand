import fs from 'node:fs/promises';
import {closeSync, openSync} from 'node:fs';
import net from 'node:net';
import {spawn} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import {StringDecoder} from 'node:string_decoder';
import {
	getCliEntryPath,
	getConfigDir,
	getDaemonLogPath,
	getDaemonPidPath,
	getProjectRoot,
	getSocketPath,
	getTsxLoaderPath,
	isDevRuntime,
} from './paths.js';
import {PROTOCOL_VERSION} from './types.js';
import type {NoteSection} from './notes.js';
import type {TaskOp} from './tasks.js';
import type {ActionRecord, AgentUpdateResult, AgentVersions, BranchList, MergePreview, TasksDoc, NoteSaveResult, ChangeDiff, ChangeGroup, ChangesRecord, ClientRequest, CreateSessionInput, DevRecord, PreviewRecord, ProgramKey, RestartMode, ServerMessage, SessionRecord, TerminalRecord, WorktreeInfoRecord, WorktreeMergeMode, WorktreeMergeResult, ProjectInfo, WorkspaceSummary, CreatePrResult, SessionCleanupInspection, SavedConfigDocument, ConfigTargetKind, SettingsInfo, WorktreeCandidates} from './types.js';

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
	const decoder = new StringDecoder('utf8');
	const handleData = (chunk: Buffer | string) => {
		buffer += Buffer.isBuffer(chunk) ? decoder.write(chunk) : chunk;
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
		decoder.end();
	};
}

export async function request<T = unknown>(message: Extract<ClientRequest, {requestId: string}>, timeoutMs = 15000): Promise<T> {
	const socket = await createConnection();
	return new Promise<T>((resolve, reject) => {
		let done = false;
		const timer = setTimeout(() => { done = true; socket.destroy(); reject(new Error('daemon request timed out')); }, timeoutMs);
		const cleanup = attachJsonParser(socket, payload => {
			if (payload.type !== 'response' || payload.requestId !== message.requestId) {
				return;
			}
			done = true;
			clearTimeout(timer);
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
			clearTimeout(timer);
			if (!done) {
				reject(new Error('daemon connection closed before response'));
			}
		});
		writeMessage(socket, message);
	});
}

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
	const args = isDevRuntime() ? ['--import', getTsxLoaderPath(), cliPath, '--daemon'] : [cliPath, '--daemon'];
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

export async function openPersistentConnection(): Promise<net.Socket> {
	await ensureDaemonRunning();
	return createConnection();
}

interface LiveClientHandlers {
	onSessionUpdated?: (session: SessionRecord) => void;
	onSessionRemoved?: (sessionId: string) => void;
	onPreviewUpdated?: (preview: PreviewRecord) => void;
	onTerminalUpdated?: (terminal: TerminalRecord) => void;
	onChangesUpdated?: (changes: ChangesRecord) => void;
	onDevUpdated?: (dev: DevRecord) => void;
	onActionUpdated?: (action: ActionRecord) => void;
	onAgentVersionsUpdated?: (versions: AgentVersions) => void;
	onTasksUpdated?: (tasks: TasksDoc) => void;
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
					case 'changes-updated':
						this.handlers.onChangesUpdated?.(message.changes);
						return;
					case 'dev-updated':
						this.handlers.onDevUpdated?.(message.dev);
						return;
					case 'action-updated':
						this.handlers.onActionUpdated?.(message.action);
						return;
					case 'agent-versions-updated':
						this.handlers.onAgentVersionsUpdated?.(message.versions);
						return;
					case 'tasks-updated':
						this.handlers.onTasksUpdated?.(message.tasks);
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

	watchPreview(sessionId: string | undefined, cols: number, rows: number, scrollOffset = 0): Promise<PreviewRecord> {
		return this.request<PreviewRecord>({
			type: 'watch-preview',
			requestId: randomUUID(),
			sessionId,
			cols,
			rows,
			scrollOffset,
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

	// The Git tab's Changes view (lazygit itself is only attached, with o). No sessionId stops watching.
	watchChanges(sessionId: string | undefined): Promise<ChangesRecord> {
		return this.request<ChangesRecord>({type: 'watch-changes', requestId: randomUUID(), sessionId});
	}

	changesDiff(sessionId: string, group: ChangeGroup, path: string): Promise<ChangeDiff> {
		return this.request<ChangeDiff>({type: 'changes-diff', requestId: randomUUID(), sessionId, group, path});
	}

	/** Stages/unstages one entry, or everything without `entry`. */
	changeStage(sessionId: string, mode: 'stage' | 'unstage', entry?: {group: ChangeGroup; path: string}): Promise<{changed: number; skippedConflicts: number; changes: ChangesRecord}> {
		return this.request({type: 'change-stage', requestId: randomUUID(), sessionId, mode, group: entry?.group, path: entry?.path});
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

	reorderSession(sessionId: string, direction: 'up' | 'down'): Promise<SessionRecord[]> {
		return this.request<SessionRecord[]>({type: 'reorder-session', requestId: randomUUID(), sessionId, direction});
	}

	listWorktrees(cwd: string): Promise<WorktreeInfoRecord[]> {
		return this.request<WorktreeInfoRecord[]>({type: 'list-worktrees', requestId: randomUUID(), cwd});
	}

	restartSession(sessionId: string, cols: number, rows: number, mode: RestartMode = 'resume', projectFingerprint?: string): Promise<SessionRecord> {
		return this.request<SessionRecord>({type: 'restart', requestId: randomUUID(), sessionId, cols, rows, mode, projectFingerprint});
	}

	killSession(sessionId: string, deleteWorktree = false, deleteBranch = false, force = false, allowDataLoss = false): Promise<void> {
		return this.request({type: 'kill', requestId: randomUUID(), sessionId, deleteWorktree, deleteBranch, force, allowDataLoss});
	}

	mergePreview(sessionId: string, targetCwd: string): Promise<MergePreview> {
		return this.request<MergePreview>({type: 'merge-preview', requestId: randomUUID(), sessionId, targetCwd});
	}

	mergeWorktree(sessionId: string, mode: WorktreeMergeMode, targetCwd: string, commitFirst = false, tickTaskIds: string[] = []): Promise<WorktreeMergeResult> {
		return this.request<WorktreeMergeResult>({type: 'merge-worktree', requestId: randomUUID(), sessionId, mode, targetCwd, commitFirst, ...tickTaskIds.length ? {tickTaskIds} : {}});
	}

	resolveMerge(sessionId: string, targetCwd: string, action: 'keep' | 'abort'): Promise<SessionRecord> {
		return this.request<SessionRecord>({type: 'resolve-merge', requestId: randomUUID(), sessionId, targetCwd, action});
	}

	setSessionDone(sessionId: string, done: boolean): Promise<SessionRecord> {
		return this.request<SessionRecord>({type: 'set-session-done', requestId: randomUUID(), sessionId, done});
	}

	markSessionMerged(sessionId: string, targetCwd: string): Promise<SessionRecord> {
		return this.request<SessionRecord>({type: 'mark-session-merged', requestId: randomUUID(), sessionId, targetCwd});
	}

	/** `moveOpenItems`: first send the open checklist items of the notes removed with it to the repository's tasks. */
	removeSession(sessionId: string, moveOpenItems = false): Promise<void> {
		return this.request({type: 'remove', requestId: randomUUID(), sessionId, ...moveOpenItems ? {moveOpenItems: true} : {}});
	}

	/** The task list of `cwd`'s repository; `tasks-updated` follows its changes. */
	watchTasks(cwd: string): Promise<TasksDoc> { return this.request({type: 'watch-tasks', requestId: randomUUID(), cwd}); }
	taskOp(cwd: string, op: TaskOp): Promise<TasksDoc> { return this.request({type: 'task-op', requestId: randomUUID(), cwd, op}); }
	openTasks(cwd: string): Promise<string> { return this.request({type: 'open-tasks', requestId: randomUUID(), cwd}); }
	returnTaskToNote(cwd: string, taskId: string): Promise<TasksDoc> { return this.request({type: 'return-task-to-note', requestId: randomUUID(), cwd, taskId}); }
	promoteNoteItem(sessionId: string, section: NoteSection, line: number, revision: string, noteId?: string): Promise<{session: SessionRecord; tasks: TasksDoc}> {
		return this.request({type: 'promote-note-item', requestId: randomUUID(), sessionId, section, line, revision, ...noteId ? {noteId} : {}});
	}
	listBranches(cwd: string): Promise<BranchList> { return this.request({type: 'list-branches', requestId: randomUUID(), cwd}); }

	/** Saves a note if its file still has `revision`; `saved` false (and the file as it is in `session`) when it changed. */
	saveNote(sessionId: string, section: NoteSection, text: string, revision: string, noteId?: string): Promise<NoteSaveResult> {
		return this.request<NoteSaveResult>({type: 'save-note', requestId: randomUUID(), sessionId, section, noteId, text, revision});
	}

	/** The note's file (created if missing), to open it in an editor. */
	openNote(sessionId: string, section: NoteSection): Promise<string> {
		return this.request<string>({type: 'open-note', requestId: randomUUID(), sessionId, section});
	}

	saveConfig(target: ConfigTargetKind, cwd: string, raw: string, revision: string | null): Promise<SavedConfigDocument> { return this.request({type: 'save-config', requestId: randomUUID(), target, cwd, raw, revision}); }
	settingsInfo(cwd: string): Promise<SettingsInfo> { return this.request({type: 'settings-info', requestId: randomUUID(), cwd}); }
	worktreeCandidates(cwd: string): Promise<WorktreeCandidates> { return this.request({type: 'worktree-candidates', requestId: randomUUID(), cwd}); }
	worktreeCandidateSizes(cwd: string, paths: string[]): Promise<Record<string, number | null>> { return this.request({type: 'worktree-candidate-sizes', requestId: randomUUID(), cwd, paths}); }
	// The repository of `cwd` (its main checkout's deckhand.json), with trust state and effective settings.
	projectInfo(cwd: string): Promise<ProjectInfo> { return this.request({type: 'project-info', requestId: randomUUID(), cwd}); }
	trustProject(cwd: string, fingerprint: string): Promise<ProjectInfo> { return this.request({type: 'trust-project', requestId: randomUUID(), cwd, fingerprint}); }
	workspaceSummary(sessionId: string, includePr = false): Promise<WorkspaceSummary> { return this.request({type: 'workspace-summary', requestId: randomUUID(), sessionId, includePr}); }
	createPr(sessionId: string, branch?: string): Promise<CreatePrResult> { return this.request({type: 'create-pr', requestId: randomUUID(), sessionId, branch}); }
	inspectCleanup(sessionId: string, deleteBranch = true): Promise<SessionCleanupInspection> { return this.request({type: 'inspect-cleanup', requestId: randomUUID(), sessionId, deleteBranch}); }
	archiveSession(sessionId: string, archived: boolean): Promise<SessionRecord> { return this.request({type: 'archive-session', requestId: randomUUID(), sessionId, archived}); }
	exportHandoff(sessionId: string, includeOutput = false): Promise<string> { return this.request({type: 'export-handoff', requestId: randomUUID(), sessionId, includeOutput}); }
	runAction(sessionId: string, action: string, cols: number, rows: number): Promise<ActionRecord> { return this.request({type: 'run-action', requestId: randomUUID(), sessionId, action, cols, rows}); }
	watchAction(sessionId: string | undefined, cols: number, rows: number): Promise<ActionRecord> { return this.request({type: 'watch-action', requestId: randomUUID(), sessionId, cols, rows}); }
	stopAction(sessionId: string): Promise<void> { return this.request({type: 'stop-action', requestId: randomUUID(), sessionId}); }
	cancelStart(sessionId: string): Promise<void> { return this.request({type: 'cancel-start', requestId: randomUUID(), sessionId}); }
	/** Installed and latest version of every agent; `refresh` looks the latest up again first (up to ~10 s). */
	agentVersions(refresh = false): Promise<AgentVersions> { return this.request({type: 'agent-versions', requestId: randomUUID(), refresh}); }
	/** Runs the agent's own update command (may take minutes); resolves with its exit code and output. */
	updateAgent(program: ProgramKey): Promise<AgentUpdateResult> { return this.request({type: 'update-agent', requestId: randomUUID(), program}); }

	sendAgentInput(sessionId: string, data: string): void {
		if (this.closed || this.socket.destroyed) {
			return;
		}
		writeMessage(this.socket, {type: 'input', sessionId, data});
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
