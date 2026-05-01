import pty, {type IPty} from 'node-pty';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {ensureNodePtyReady} from './nodePty.js';
import {loadAppConfig} from './storage.js';
import {TerminalPreview} from './terminalPreview.js';
import type {AgentActivityStatus, AttachTarget, DevRecord, GitRecord, PreviewRecord, SessionRecord, TerminalRecord} from './types.js';

const execFileAsync = promisify(execFile);
const DEFAULT_COLS = 80;
const DEFAULT_ROWS = 24;
const PREVIEW_BROADCAST_DELAY_MS = 75;
const ACTIVITY_EVALUATION_DELAY_MS = 150;
const ACTIVITY_WINDOW_MS = 3000;
const IDLE_AFTER_MS = 5000;
const ACTIVE_MIN_CHANGED_CHARS = 1;
const RESIZE_ACTIVITY_SUPPRESSION_MS = 750;

type WorkerCommand =
	| {type: 'start'; requestId: string; session: SessionRecord; cols: number; rows: number}
	| {type: 'kill'; requestId: string; force?: boolean}
	| {type: 'snapshot'; requestId: string; target: AttachTarget; cols: number; rows: number}
	| {type: 'start-dev'; requestId: string; cols: number; rows: number}
	| {type: 'stop-dev'; requestId: string}
	| {type: 'attach'; requestId: string; target: AttachTarget; cols: number; rows: number}
	| {type: 'detach'; target: AttachTarget}
	| {type: 'input'; target: AttachTarget; data: string}
	| {type: 'resize'; target: AttachTarget; cols: number; rows: number};

type WorkerMessage =
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

interface RuntimePty {
	term: IPty;
	preview: TerminalPreview;
	cwd: string;
	exited: boolean;
	exitCode?: number | null;
	exitSignal?: number | null;
	broadcastTimer?: NodeJS.Timeout;
	command?: string;
}

interface AgentRuntime extends RuntimePty {
	activityEvaluationTimer?: NodeJS.Timeout;
	activityIdleTimer?: NodeJS.Timeout;
	suppressActivityUntil?: number;
	lastPreviewSnapshot: string;
	previewChangeEvents: Array<{at: number; changedChars: number}>;
}

function post(message: WorkerMessage): void {
	if (process.send) process.send(message);
}

function ok(requestId: string, data?: unknown): void {
	post({type: 'response', requestId, ok: true, data});
}

function fail(requestId: string, error: unknown): void {
	post({type: 'response', requestId, ok: false, error: error instanceof Error ? error.message : String(error)});
}

function size(value: number, fallback: number): number {
	return Number.isFinite(value) ? Math.max(1, Math.floor(value)) : fallback;
}

function shellCommand(): string {
	return process.env.SHELL || '/bin/sh';
}

async function lazyGitCommand(): Promise<string> {
	const shell = process.env.SHELL || '/bin/bash';
	try {
		const {stdout} = await execFileAsync(shell, ['-ic', 'command -v lazygit']);
		const resolved = stdout.trim();
		if (resolved) return resolved;
	} catch {}
	throw new Error('lazygit is not installed or not on PATH');
}

function changedCharacterCount(previous: string, next: string): number {
	const maxLength = Math.max(previous.length, next.length);
	let changed = Math.abs(previous.length - next.length);
	const sharedLength = Math.min(previous.length, next.length);
	for (let i = 0; i < sharedLength; i += 1) if (previous[i] !== next[i]) changed += 1;
	return Math.min(changed, maxLength);
}

function signalPtyProcess(term: IPty, signal: NodeJS.Signals): void {
	if (process.platform !== 'win32') {
		try { process.kill(-term.pid, signal); return; } catch {}
	}
	try { term.kill(signal); } catch {}
}

class SessionWorker {
	private session?: SessionRecord;
	private agent?: AgentRuntime;
	private terminal?: RuntimePty;
	private git?: RuntimePty;
	private dev?: RuntimePty;
	private attached = new Set<AttachTarget>();

	async start(): Promise<void> {
		await ensureNodePtyReady();
		process.on('message', message => void this.handle(message as WorkerCommand));
	}

	private async handle(command: WorkerCommand): Promise<void> {
		try {
			switch (command.type) {
				case 'start': ok(command.requestId, await this.startAgent(command.session, command.cols, command.rows)); return;
				case 'kill': this.kill(command.force ?? false); ok(command.requestId, {ok: true}); return;
				case 'snapshot': ok(command.requestId, await this.snapshot(command.target, command.cols, command.rows)); return;
				case 'start-dev': ok(command.requestId, await this.startDev(command.cols, command.rows)); return;
				case 'stop-dev': this.cleanup('dev'); post({type: 'dev-updated', dev: this.buildDevRecord(undefined)}); ok(command.requestId, {ok: true}); return;
				case 'attach': this.attached.add(command.target); ok(command.requestId, await this.attach(command.target, command.cols, command.rows)); return;
				case 'detach': this.attached.delete(command.target); return;
				case 'input': this.getExisting(command.target)?.term.write(command.data); return;
				case 'resize': await this.resize(command.target, command.cols, command.rows); return;
			}
		} catch (error) {
			if ('requestId' in command) fail(command.requestId, error);
		}
	}

	private async startAgent(session: SessionRecord, cols: number, rows: number): Promise<SessionRecord> {
		this.session = session;
		const term = pty.spawn(session.command, session.args ?? [], {name: 'xterm-256color', cwd: session.cwd, env: {...process.env}, cols: size(cols, DEFAULT_COLS), rows: size(rows, DEFAULT_ROWS)});
		const runtime: AgentRuntime = {term, preview: new TerminalPreview(cols, rows), cwd: session.cwd, exited: false, lastPreviewSnapshot: '', previewChangeEvents: []};
		this.agent = runtime;
		post({type: 'running', pid: term.pid});
		runtime.activityIdleTimer = setTimeout(() => { runtime.activityIdleTimer = undefined; void this.setAgentStatus('idle'); }, IDLE_AFTER_MS);
		term.onData(output => {
			void runtime.preview.write(output);
			this.scheduleActivityEvaluation();
			this.schedulePreviewBroadcast();
			if (this.attached.has('agent')) post({type: 'output', target: 'agent', data: output});
		});
		term.onExit(({exitCode, signal}) => void this.handleAgentExit(exitCode ?? null, signal ?? null));
		this.schedulePreviewBroadcast();
		return {...session, status: 'running', pid: term.pid};
	}

	private async handleAgentExit(exitCode: number | null, exitSignal: number | null): Promise<void> {
		const agent = this.agent;
		if (!agent || agent.exited) return;
		agent.exited = true;
		this.clearActivityTimers(agent);
		if (agent.broadcastTimer) clearTimeout(agent.broadcastTimer);
		const lastPreview = await agent.preview.getSnapshot();
		this.cleanup('terminal'); this.cleanup('git'); this.cleanup('dev');
		agent.preview.dispose();
		this.agent = undefined;
		post({type: 'exit', exitCode, exitSignal, lastPreview});
		setTimeout(() => process.exit(0), 25).unref?.();
	}

	private kill(force: boolean): void {
		if (!this.agent || this.agent.exited) throw new Error('session is not running');
		this.cleanup('terminal'); this.cleanup('git'); this.cleanup('dev');
		// Always signal the PTY process group. Coding agents often run below a
		// shell/bootstrap process, and killing only the direct PTY child can leave
		// the actual agent alive and the session stuck in running state.
		signalPtyProcess(this.agent.term, 'SIGTERM');
		if (force) setTimeout(() => { if (this.agent && !this.agent.exited) signalPtyProcess(this.agent.term, 'SIGKILL'); }, 1000).unref?.();
	}

	private async ensureTerminal(cols: number, rows: number): Promise<RuntimePty> {
		if (!this.session) throw new Error('session does not exist');
		if (this.terminal && !this.terminal.exited) return this.resizeRuntime(this.terminal, cols, rows);
		this.cleanup('terminal');
		this.terminal = this.spawnPane('terminal', shellCommand(), [], this.session.cwd, cols, rows);
		return this.terminal;
	}

	private async ensureGit(cols: number, rows: number): Promise<RuntimePty> {
		if (!this.session) throw new Error('session does not exist');
		if (this.git && !this.git.exited) return this.resizeRuntime(this.git, cols, rows);
		this.cleanup('git');
		this.git = this.spawnPane('git', await lazyGitCommand(), [], this.session.cwd, cols, rows);
		return this.git;
	}

	private async startDev(cols: number, rows: number): Promise<DevRecord> {
		if (!this.session) throw new Error('session does not exist');
		if (this.dev && !this.dev.exited) return this.buildDevRecord(await this.resizeRuntime(this.dev, cols, rows));
		this.cleanup('dev');
		const config = await loadAppConfig();
		const command = config.dev_command?.trim() || 'dev';
		this.dev = this.spawnPane('dev', shellCommand(), ['-ic', command], this.session.cwd, cols, rows, command);
		return this.buildDevRecord(this.dev);
	}

	private spawnPane(target: 'terminal' | 'git' | 'dev', command: string, args: string[], cwd: string, cols: number, rows: number, label?: string): RuntimePty {
		const term = pty.spawn(command, args, {name: 'xterm-256color', cwd, env: {...process.env}, cols: size(cols, DEFAULT_COLS), rows: size(rows, DEFAULT_ROWS)});
		const runtime: RuntimePty = {term, preview: new TerminalPreview(cols, rows), cwd, exited: false, command: label};
		term.onData(output => {
			void runtime.preview.write(output);
			this.schedulePaneBroadcast(target, runtime);
			if (this.attached.has(target)) post({type: 'output', target, data: output});
		});
		term.onExit(({exitCode, signal}) => { runtime.exited = true; runtime.exitCode = exitCode ?? null; runtime.exitSignal = signal ?? null; this.schedulePaneBroadcast(target, runtime); });
		this.schedulePaneBroadcast(target, runtime);
		return runtime;
	}

	private async attach(target: AttachTarget, cols: number, rows: number): Promise<unknown> {
		const record = await this.snapshot(target, cols, rows);
		const runtime = this.getExisting(target);
		if (runtime) post({type: 'output', target, data: await runtime.preview.getAnsiFrame()});
		return record;
	}

	private async snapshot(target: AttachTarget, cols: number, rows: number): Promise<unknown> {
		if (target === 'agent') {
			if (!this.agent || !this.session) return {content: '', live: false};
			this.agent.term.resize(size(cols, DEFAULT_COLS), size(rows, DEFAULT_ROWS));
			await this.agent.preview.resize(cols, rows);
			await this.suppressResizeActivity();
			return {sessionId: this.session.id, content: await this.agent.preview.getSnapshot(), live: true, status: 'running', agentStatus: this.session.agentStatus} satisfies PreviewRecord;
		}
		if (target === 'terminal') return this.buildTerminalRecord(await this.ensureTerminal(cols, rows));
		if (target === 'git') return this.buildGitRecord(await this.ensureGit(cols, rows));
		return this.dev ? this.buildDevRecord(await this.resizeRuntime(this.dev, cols, rows)) : {sessionId: this.session?.id, content: '', live: false, cwd: this.session?.cwd};
	}

	private async resize(target: AttachTarget, cols: number, rows: number): Promise<void> {
		const runtime = this.getExisting(target);
		if (!runtime || runtime.exited) return;
		await this.resizeRuntime(runtime, cols, rows);
		if (target === 'agent') { await this.suppressResizeActivity(); this.schedulePreviewBroadcast(); }
		else this.schedulePaneBroadcast(target as 'terminal' | 'git' | 'dev', runtime);
	}

	private async resizeRuntime<T extends RuntimePty>(runtime: T, cols: number, rows: number): Promise<T> {
		runtime.term.resize(size(cols, DEFAULT_COLS), size(rows, DEFAULT_ROWS));
		await runtime.preview.resize(cols, rows);
		return runtime;
	}

	private getExisting(target: AttachTarget): RuntimePty | undefined {
		if (target === 'agent') return this.agent;
		if (target === 'terminal') return this.terminal;
		if (target === 'git') return this.git;
		return this.dev;
	}

	private cleanup(target: AttachTarget): void {
		const runtime = this.getExisting(target);
		if (!runtime) return;
		if (runtime.broadcastTimer) clearTimeout(runtime.broadcastTimer);
		try { runtime.term.kill(); } catch {}
		runtime.preview.dispose();
		if (target === 'terminal') this.terminal = undefined;
		else if (target === 'git') this.git = undefined;
		else if (target === 'dev') this.dev = undefined;
	}

	private buildTerminalRecord(terminal?: RuntimePty): TerminalRecord { return {sessionId: this.session?.id, content: terminal?.preview.getCachedSnapshot() ?? '', live: Boolean(terminal && !terminal.exited), cwd: terminal?.cwd ?? this.session?.cwd, exitCode: terminal?.exitCode, exitSignal: terminal?.exitSignal}; }
	private buildGitRecord(git?: RuntimePty): GitRecord { return {sessionId: this.session?.id, content: git?.preview.getCachedSnapshot() ?? '', live: Boolean(git && !git.exited), cwd: git?.cwd ?? this.session?.cwd, exitCode: git?.exitCode, exitSignal: git?.exitSignal}; }
	private buildDevRecord(dev?: RuntimePty): DevRecord { return {sessionId: this.session?.id, content: dev?.preview.getCachedSnapshot() ?? '', live: Boolean(dev && !dev.exited), cwd: dev?.cwd ?? this.session?.cwd, command: dev?.command, exitCode: dev?.exitCode, exitSignal: dev?.exitSignal}; }

	private schedulePreviewBroadcast(): void {
		const runtime = this.agent;
		if (!runtime || runtime.broadcastTimer) return;
		runtime.broadcastTimer = setTimeout(async () => {
			runtime.broadcastTimer = undefined;
			if (!this.session || !this.agent) return;
			post({type: 'preview-updated', preview: {sessionId: this.session.id, content: await runtime.preview.getSnapshot(), live: true, status: 'running', agentStatus: this.session.agentStatus}});
		}, PREVIEW_BROADCAST_DELAY_MS);
	}

	private schedulePaneBroadcast(target: 'terminal' | 'git' | 'dev', runtime: RuntimePty): void {
		if (runtime.broadcastTimer) return;
		runtime.broadcastTimer = setTimeout(async () => {
			runtime.broadcastTimer = undefined;
			await runtime.preview.getSnapshot();
			if (target === 'terminal') post({type: 'terminal-updated', terminal: this.buildTerminalRecord(runtime)});
			else if (target === 'git') post({type: 'git-updated', git: this.buildGitRecord(runtime)});
			else post({type: 'dev-updated', dev: this.buildDevRecord(runtime)});
		}, PREVIEW_BROADCAST_DELAY_MS);
	}

	private scheduleActivityEvaluation(): void {
		const runtime = this.agent;
		if (!runtime || runtime.activityEvaluationTimer || Date.now() < (runtime.suppressActivityUntil ?? 0)) return;
		runtime.activityEvaluationTimer = setTimeout(() => { runtime.activityEvaluationTimer = undefined; void this.evaluatePreviewActivity(); }, ACTIVITY_EVALUATION_DELAY_MS);
	}

	private async evaluatePreviewActivity(): Promise<void> {
		const runtime = this.agent;
		if (!runtime || Date.now() < (runtime.suppressActivityUntil ?? 0)) return;
		const snapshot = await runtime.preview.getSnapshot();
		const changedChars = changedCharacterCount(runtime.lastPreviewSnapshot, snapshot);
		runtime.lastPreviewSnapshot = snapshot;
		if (changedChars < ACTIVE_MIN_CHANGED_CHARS) return;
		const now = Date.now();
		runtime.previewChangeEvents = runtime.previewChangeEvents.filter(e => now - e.at <= ACTIVITY_WINDOW_MS).concat({at: now, changedChars});
		if (runtime.previewChangeEvents.reduce((t, e) => t + e.changedChars, 0) >= ACTIVE_MIN_CHANGED_CHARS) await this.setAgentStatus('active');
		if (runtime.activityIdleTimer) clearTimeout(runtime.activityIdleTimer);
		runtime.activityIdleTimer = setTimeout(() => { runtime.activityIdleTimer = undefined; runtime.previewChangeEvents = []; void this.setAgentStatus('idle'); }, IDLE_AFTER_MS);
	}

	private async suppressResizeActivity(): Promise<void> {
		if (!this.agent) return;
		this.agent.suppressActivityUntil = Date.now() + RESIZE_ACTIVITY_SUPPRESSION_MS;
		this.agent.lastPreviewSnapshot = await this.agent.preview.getSnapshot();
	}

	private async setAgentStatus(agentStatus: AgentActivityStatus): Promise<void> {
		if (!this.session || this.session.agentStatus === agentStatus) return;
		this.session = {...this.session, agentStatus, agentStatusUpdatedAt: new Date().toISOString()};
		post({type: 'agent-status', agentStatus});
		this.schedulePreviewBroadcast();
	}

	private clearActivityTimers(runtime: AgentRuntime): void {
		if (runtime.activityEvaluationTimer) clearTimeout(runtime.activityEvaluationTimer);
		if (runtime.activityIdleTimer) clearTimeout(runtime.activityIdleTimer);
	}
}

export async function runSessionWorker(): Promise<void> {
	process.title = 'deckhand-session-worker';
	await new SessionWorker().start();
	await new Promise(() => {});
}
