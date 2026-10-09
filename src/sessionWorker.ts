import pty, {type IPty} from 'node-pty';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {ensureNodePtyReady} from './nodePty.js';
import {killSurvivors, snapshotDescendants, survivors} from './processTree.js';
import {loadAppConfig} from './storage.js';
import {TerminalPreview} from './terminalPreview.js';
import type {ActionRecord, AgentActivityStatus, AttachTarget, DevRecord, GitRecord, PreviewRecord, SessionRecord, TerminalRecord} from './types.js';

const execFileAsync = promisify(execFile);
const DEFAULT_COLS = 80;
const DEFAULT_ROWS = 24;
const PREVIEW_BROADCAST_DELAY_MS = 75;
const ACTIVITY_EVALUATION_DELAY_MS = 150;
const ACTIVITY_WINDOW_MS = 3000;
const IDLE_AFTER_MS = 5000;
const ACTIVE_MIN_CHANGED_CHARS = 1;
const RESIZE_ACTIVITY_SUPPRESSION_MS = 750;
const PANE_KILL_GRACE_MS = 2000;
// Root-session credentials stay with the agent; nested agents started from a pane must not reuse them.
const ROOT_SESSION_ENV = ['DECKHAND_HOOK_TOKEN', 'DECKHAND_LAUNCH_ID', 'DECKHAND_SESSION_ID'];

type PaneTarget = Exclude<AttachTarget, 'agent'>;

// Pane commands are shared by both worker kinds: a session worker serves only 'agent', a workspace worker its panes.
type PaneCommand =
	| {type: 'snapshot'; requestId: string; target: AttachTarget; cols: number; rows: number; scrollOffset?: number}
	| {type: 'attach'; requestId: string; target: AttachTarget; cols: number; rows: number}
	| {type: 'detach'; target: AttachTarget}
	| {type: 'input'; target: AttachTarget; data: string}
	| {type: 'resize'; target: AttachTarget; cols: number; rows: number};

type WorkerCommand =
	| PaneCommand
	| {type: 'start'; requestId: string; session: SessionRecord; cols: number; rows: number}
	| {type: 'kill'; requestId: string; force?: boolean};

type WorkspaceWorkerCommand =
	| PaneCommand
	| {type: 'start'; requestId: string; cwd: string}
	| {type: 'start-dev'; requestId: string; cols: number; rows: number; command: string}
	| {type: 'stop-dev'; requestId: string}
	| {type: 'start-action'; requestId: string; cols: number; rows: number; command: string; name: string}
	| {type: 'stop-action'; requestId: string}
	| {type: 'idle'; requestId: string}
	| {type: 'shutdown'; requestId: string};

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
	| {type: 'action-updated'; action: ActionRecord}
	| {type: 'output'; target: AttachTarget; data: string};

interface TerminalModes {
	bracketedPaste: boolean;
}

interface RuntimePty {
	term: IPty;
	preview: TerminalPreview;
	cwd: string;
	exited: boolean;
	terminalModes: TerminalModes;
	exitCode?: number | null;
	exitSignal?: number | null;
	broadcastTimer?: NodeJS.Timeout;
	command?: string;
	/** An action's name (the `action` pane). */
	name?: string;
}

interface AgentRuntime extends RuntimePty {
	activityEvaluationTimer?: NodeJS.Timeout;
	activityIdleTimer?: NodeJS.Timeout;
	suppressActivityUntil?: number;
	lastPreviewSnapshot: string;
	previewChangeEvents: Array<{at: number; changedChars: number}>;
	lastDataAt?: number;
}

// node-pty can report exit before the final output chunks are delivered. Exit-time parsing (resume hints,
// "No conversation found") needs those last lines, so wait briefly for output to go quiet first.
const EXIT_OUTPUT_QUIET_MS = 60, EXIT_OUTPUT_MAX_WAIT_MS = 400;
async function waitForQuietOutput(runtime: AgentRuntime): Promise<void> {
	const deadline = Date.now() + EXIT_OUTPUT_MAX_WAIT_MS;
	while (Date.now() < deadline && Date.now() - (runtime.lastDataAt ?? 0) < EXIT_OUTPUT_QUIET_MS) await new Promise(resolve => setTimeout(resolve, 20));
}

function post(message: WorkerMessage): void {
	if (!process.connected || !process.send) return;
	try { process.send(message); } catch { /* The daemon disconnected; the disconnect handler shuts down. */ }
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

function updateTerminalModes(modes: TerminalModes, output: string): void {
	const bracketedPastePattern = /\x1b\[\?2004([hl])/g;
	for (const match of output.matchAll(bracketedPastePattern)) {
		modes.bracketedPaste = match[1] === 'h';
	}
}

// The companion panes a workspace worker hosts: shared by every session in one worktree and outliving any agent
// (see src/workspace.ts). A session worker hosts only its agent PTY. `action` is the last action run (shown on the
// Terminal tab), a PTY of its own so it never touches the shell or Dev.
const WORKSPACE_PANES: ReadonlySet<PaneTarget> = new Set(['terminal', 'git', 'dev', 'action']);

type PaneRecord = TerminalRecord & GitRecord & ActionRecord;

function paneUpdated(target: PaneTarget, record: PaneRecord): WorkerMessage {
	if (target === 'terminal') return {type: 'terminal-updated', terminal: record};
	if (target === 'git') return {type: 'git-updated', git: record};
	if (target === 'action') return {type: 'action-updated', action: record};
	return {type: 'dev-updated', dev: record};
}

// The companion pane PTYs of one workspace and their headless previews. Records carry no session: the daemon
// stamps each viewing session's ID (and the workspace) on them.
class PaneHost {
	private readonly panes: Partial<Record<PaneTarget, RuntimePty>> = {};
	private readonly startPromises = new Map<PaneTarget, Promise<RuntimePty>>();
	private readonly attached = new Set<PaneTarget>();

	constructor(private readonly targets: ReadonlySet<PaneTarget>, private readonly owner: () => {cwd?: string}) {}

	/** True when no pane runtime (live or exited with output) remains and none is starting. */
	get idle(): boolean {
		return this.startPromises.size === 0 && Object.values(this.panes).every(runtime => !runtime);
	}

	private hosted(target: AttachTarget): PaneTarget {
		if (target === 'agent' || !this.targets.has(target)) throw new Error(`${target} pane is not hosted by this worker`);
		return target;
	}

	private cwd(): string {
		const cwd = this.owner().cwd;
		if (!cwd) throw new Error('workspace is not started');
		return cwd;
	}

	private async ensureTerminal(cols: number, rows: number): Promise<RuntimePty> {
		const existing = this.panes.terminal;
		if (existing && !existing.exited) return this.resizeRuntime(existing, cols, rows);
		void this.stop('terminal');
		return this.spawnPane('terminal', shellCommand(), [], this.cwd(), cols, rows);
	}

	private async ensureGit(cols: number, rows: number): Promise<RuntimePty> {
		const existing = this.panes.git;
		if (existing && !existing.exited) return this.resizeRuntime(existing, cols, rows);
		const pending = this.startPromises.get('git');
		if (pending) return this.resizeRuntime(await pending, cols, rows);

		const start = (async () => {
			void this.stop('git');
			const command = await lazyGitCommand();
			const current = this.panes.git;
			if (current && !current.exited) return current;
			return this.spawnPane('git', command, [], this.cwd(), cols, rows);
		})();
		this.startPromises.set('git', start);
		try {
			return await start;
		} finally {
			if (this.startPromises.get('git') === start) this.startPromises.delete('git');
		}
	}

	async startDev(cols: number, rows: number, requestedCommand?: string): Promise<DevRecord> {
		this.hosted('dev');
		const reuse = async (runtime: RuntimePty) => {
			if (requestedCommand?.trim() && requestedCommand.trim() !== runtime.command) throw new Error('Stop the current Dev command before starting another');
			return this.record('dev', await this.resizeRuntime(runtime, cols, rows));
		};
		const existing = this.panes.dev;
		if (existing && !existing.exited) return reuse(existing);
		const pending = this.startPromises.get('dev');
		if (pending) return reuse(await pending);

		const start = (async () => {
			void this.stop('dev');
			const config = await loadAppConfig();
			const command = requestedCommand?.trim() || config.dev_command?.trim() || 'dev';
			const current = this.panes.dev;
			if (current && !current.exited) return current;
			return this.spawnPane('dev', shellCommand(), ['-ic', command], this.cwd(), cols, rows, command);
		})();
		this.startPromises.set('dev', start);
		try {
			return this.record('dev', await start);
		} finally {
			if (this.startPromises.get('dev') === start) this.startPromises.delete('dev');
		}
	}

	// One action at a time per workspace; a finished one is replaced (its output and exit code stay until then).
	async startAction(cols: number, rows: number, command: string, name: string): Promise<ActionRecord> {
		this.hosted('action');
		const existing = this.panes.action;
		if (this.startPromises.has('action') || (existing && !existing.exited)) throw new Error(`The ${existing?.name ?? 'previous'} action is still running; stop it first (r, then x)`);
		const start = (async () => {
			await this.stop('action');
			const runtime = this.spawnPane('action', shellCommand(), ['-ic', command], this.cwd(), cols, rows, command);
			runtime.name = name;
			return runtime;
		})();
		this.startPromises.set('action', start);
		try {
			return this.record('action', await start);
		} finally {
			if (this.startPromises.get('action') === start) this.startPromises.delete('action');
		}
	}

	private spawnPane(target: PaneTarget, command: string, args: string[], cwd: string, cols: number, rows: number, label?: string): RuntimePty {
		const env = {...process.env};
		for (const key of ROOT_SESSION_ENV) delete env[key];
		const term = pty.spawn(command, args, {name: 'xterm-256color', cwd, env, cols: size(cols, DEFAULT_COLS), rows: size(rows, DEFAULT_ROWS)});
		const runtime: RuntimePty = {term, preview: new TerminalPreview(cols, rows), cwd, exited: false, terminalModes: {bracketedPaste: false}, command: label};
		this.panes[target] = runtime;
		// A replaced/stopped pane may keep emitting until it dies; only the current one reports.
		const current = () => this.panes[target] === runtime;
		term.onData(output => {
			if (!current()) return;
			updateTerminalModes(runtime.terminalModes, output);
			void runtime.preview.write(output);
			this.scheduleBroadcast(target, runtime);
			if (this.attached.has(target)) post({type: 'output', target, data: output});
		});
		term.onExit(({exitCode, signal}) => { runtime.exited = true; runtime.exitCode = exitCode ?? null; runtime.exitSignal = signal ?? null; if (current()) this.scheduleBroadcast(target, runtime); });
		this.scheduleBroadcast(target, runtime);
		return runtime;
	}

	async snapshot(target: AttachTarget, cols: number, rows: number): Promise<PaneRecord> {
		const pane = this.hosted(target);
		if (pane === 'terminal') return this.record(pane, await this.ensureTerminal(cols, rows));
		if (pane === 'git') return this.record(pane, await this.ensureGit(cols, rows));
		// Dev and actions start only on request; viewing never starts them.
		const runtime = this.panes[pane];
		return this.record(pane, runtime ? await this.resizeRuntime(runtime, cols, rows) : undefined);
	}

	async attach(target: AttachTarget, cols: number, rows: number): Promise<unknown> {
		const pane = this.hosted(target);
		this.attached.add(pane);
		const record = await this.snapshot(pane, cols, rows);
		const runtime = this.panes[pane];
		return runtime ? {...record, terminalModes: runtime.terminalModes, initialFrame: await runtime.preview.getAnsiFrame()} : record;
	}

	detach(target: AttachTarget): void {
		this.attached.delete(this.hosted(target));
	}

	input(target: AttachTarget, data: string): void {
		this.panes[this.hosted(target)]?.term.write(data);
	}

	async resize(target: AttachTarget, cols: number, rows: number): Promise<void> {
		const pane = this.hosted(target);
		const runtime = this.panes[pane];
		if (!runtime || runtime.exited) return;
		await this.resizeRuntime(runtime, cols, rows);
		this.scheduleBroadcast(pane, runtime);
	}

	private async resizeRuntime(runtime: RuntimePty, cols: number, rows: number): Promise<RuntimePty> {
		if (!runtime.exited) runtime.term.resize(size(cols, DEFAULT_COLS), size(rows, DEFAULT_ROWS));
		await runtime.preview.resize(cols, rows);
		await runtime.preview.getSnapshot();
		return runtime;
	}

	/** Stops and forgets a pane; resolves once its process exited (SIGKILL after a grace period). */
	stop(target: PaneTarget): Promise<void> {
		const runtime = this.panes[target];
		if (!runtime) return Promise.resolve();
		delete this.panes[target];
		if (runtime.broadcastTimer) clearTimeout(runtime.broadcastTimer);
		// Taken before signalling: once the shell dies its children are reparented and can't be found from it.
		const descendants = runtime.exited ? new Map() : snapshotDescendants(runtime.term.pid);
		// Interactive shells (`$SHELL -ic cmd`) ignore SIGTERM; SIGHUP is what a closed terminal sends.
		signalPtyProcess(runtime.term, 'SIGHUP');
		runtime.preview.dispose();
		if (runtime.exited) return Promise.resolve();
		// After the grace period, SIGKILL the group plus descendants that survived, including jobs the shell moved to
		// their own process group (which the group signal cannot reach). The shell exiting first ends the wait only when
		// nothing survived it, so a worker that exits right after stopping its panes still kills them.
		return new Promise(resolve => {
			const timer = setTimeout(() => { if (!runtime.exited) signalPtyProcess(runtime.term, 'SIGKILL'); killSurvivors(descendants); resolve(); }, PANE_KILL_GRACE_MS);
			runtime.term.onExit(() => { if (survivors(descendants).size === 0) { clearTimeout(timer); resolve(); } });
		});
	}

	stopAll(): Promise<void> {
		return Promise.all([...this.targets].map(target => this.stop(target))).then(() => {});
	}

	record(target: PaneTarget, runtime?: RuntimePty): PaneRecord {
		const owner = this.owner();
		return {content: runtime?.preview.getCachedSnapshot() ?? '', live: Boolean(runtime && !runtime.exited), cwd: runtime?.cwd ?? owner.cwd, ...(target === 'dev' || target === 'action' ? {command: runtime?.command} : {}), ...(target === 'action' ? {name: runtime?.name} : {}), exitCode: runtime?.exitCode, exitSignal: runtime?.exitSignal};
	}

	private scheduleBroadcast(target: PaneTarget, runtime: RuntimePty): void {
		if (runtime.broadcastTimer) return;
		runtime.broadcastTimer = setTimeout(async () => {
			runtime.broadcastTimer = undefined;
			await runtime.preview.getSnapshot();
			post(paneUpdated(target, this.record(target, runtime)));
		}, PREVIEW_BROADCAST_DELAY_MS);
	}
}

async function handlePaneCommand(host: PaneHost, command: PaneCommand): Promise<void> {
	switch (command.type) {
		case 'snapshot': ok(command.requestId, await host.snapshot(command.target, command.cols, command.rows)); return;
		case 'attach': ok(command.requestId, await host.attach(command.target, command.cols, command.rows)); return;
		case 'detach': host.detach(command.target); return;
		case 'input': host.input(command.target, command.data); return;
		case 'resize': await host.resize(command.target, command.cols, command.rows); return;
	}
}

// Owns one session's agent PTY; its companion panes belong to the session's workspace worker.
class SessionWorker {
	private session?: SessionRecord;
	private agent?: AgentRuntime;
	private agentAttached = false;

	async start(): Promise<void> {
		await ensureNodePtyReady();
		process.on('message', message => void this.handle(message as WorkerCommand));
		process.on('disconnect', () => {
			if (this.agent) signalPtyProcess(this.agent.term, 'SIGTERM');
			setTimeout(() => { if (this.agent) signalPtyProcess(this.agent.term, 'SIGKILL'); process.exit(0); }, 1000).unref?.();
		});
	}

	private async handle(command: WorkerCommand): Promise<void> {
		try {
			switch (command.type) {
				case 'start': ok(command.requestId, await this.startAgent(command.session, command.cols, command.rows)); return;
				case 'kill': this.kill(command.force ?? false); ok(command.requestId, {ok: true}); return;
			}
			if (command.target !== 'agent') throw new Error(`${command.target} pane is hosted by the workspace worker`);
			switch (command.type) {
				case 'snapshot': ok(command.requestId, await this.snapshotAgent(command.cols, command.rows, command.scrollOffset)); return;
				case 'attach': this.agentAttached = true; ok(command.requestId, await this.attachAgent(command.cols, command.rows)); return;
				case 'detach': this.agentAttached = false; return;
				case 'input': this.agent?.term.write(command.data); return;
				case 'resize': await this.resizeAgent(command.cols, command.rows); return;
			}
		} catch (error) {
			if ('requestId' in command) fail(command.requestId, error);
		}
	}

	private async startAgent(session: SessionRecord, cols: number, rows: number): Promise<SessionRecord> {
		this.session = session;
		const term = pty.spawn(session.command, session.args ?? [], {name: 'xterm-256color', cwd: session.cwd, env: {...process.env}, cols: size(cols, DEFAULT_COLS), rows: size(rows, DEFAULT_ROWS)});
		const runtime: AgentRuntime = {term, preview: new TerminalPreview(cols, rows), cwd: session.cwd, exited: false, terminalModes: {bracketedPaste: false}, lastPreviewSnapshot: '', previewChangeEvents: []};
		this.agent = runtime;
		post({type: 'running', pid: term.pid});
		runtime.activityIdleTimer = setTimeout(() => { runtime.activityIdleTimer = undefined; void this.setAgentStatus('idle'); }, IDLE_AFTER_MS);
		term.onData(output => {
			runtime.lastDataAt = Date.now();
			updateTerminalModes(runtime.terminalModes, output);
			void runtime.preview.write(output);
			this.scheduleActivityEvaluation();
			this.schedulePreviewBroadcast();
			if (this.agentAttached) post({type: 'output', target: 'agent', data: output});
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
		await waitForQuietOutput(agent);
		const lastPreview = await agent.preview.getSnapshot();
		agent.preview.dispose();
		this.agent = undefined;
		post({type: 'exit', exitCode, exitSignal, lastPreview});
		setTimeout(() => process.exit(0), 25).unref?.();
	}

	private kill(force: boolean): void {
		if (!this.agent || this.agent.exited) throw new Error('session is not running');
		// Always signal the PTY process group. Coding agents often run below a
		// shell/bootstrap process, and killing only the direct PTY child can leave
		// the actual agent alive and the session stuck in running state.
		signalPtyProcess(this.agent.term, 'SIGTERM');
		if (force) setTimeout(() => { if (this.agent && !this.agent.exited) signalPtyProcess(this.agent.term, 'SIGKILL'); }, 1000).unref?.();
	}

	private async attachAgent(cols: number, rows: number): Promise<unknown> {
		const record = await this.snapshotAgent(cols, rows);
		return this.agent ? {...record, terminalModes: this.agent.terminalModes, initialFrame: await this.agent.preview.getAnsiFrame()} : record;
	}

	private async snapshotAgent(cols: number, rows: number, scrollOffset = 0): Promise<PreviewRecord> {
		if (!this.agent || !this.session) return {content: '', live: false};
		const nextCols = size(cols, DEFAULT_COLS);
		const nextRows = size(rows, DEFAULT_ROWS);
		const resized = this.agent.term.cols !== nextCols || this.agent.term.rows !== nextRows;
		this.agent.term.resize(nextCols, nextRows);
		await this.agent.preview.resize(cols, rows);
		if (resized) await this.suppressResizeActivity();
		const content = await this.agent.preview.getSnapshot(scrollOffset);
		const scrollInfo = await this.agent.preview.getScrollInfo(scrollOffset);
		return {sessionId: this.session.id, content, live: true, status: 'running', agentStatus: this.session.agentStatus, ...scrollInfo};
	}

	private async resizeAgent(cols: number, rows: number): Promise<void> {
		const runtime = this.agent;
		if (!runtime || runtime.exited) return;
		runtime.term.resize(size(cols, DEFAULT_COLS), size(rows, DEFAULT_ROWS));
		await runtime.preview.resize(cols, rows);
		await runtime.preview.getSnapshot();
		await this.suppressResizeActivity();
		this.schedulePreviewBroadcast();
	}

	private schedulePreviewBroadcast(): void {
		const runtime = this.agent;
		if (!runtime || runtime.broadcastTimer) return;
		runtime.broadcastTimer = setTimeout(async () => {
			runtime.broadcastTimer = undefined;
			if (!this.session || !this.agent) return;
			const content = await runtime.preview.getSnapshot();
			const scrollInfo = await runtime.preview.getScrollInfo();
			post({type: 'preview-updated', preview: {sessionId: this.session.id, content, live: true, status: 'running', agentStatus: this.session.agentStatus, ...scrollInfo}});
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

// Hosts the panes shared by every session in one workspace (worktree): Terminal, Git, Dev and the last action. The
// daemon starts it on demand (first Terminal/Git view or attach, Dev start, an action run) and retires it once nothing
// is left in it (after a Dev or action stop, or a failed pane start), before the worktree is deleted, when the
// workspace's last session is removed, or on shutdown. Its own panes never retire it: a shell never exits on its own,
// and exited panes keep their output.
class WorkspaceWorker {
	private cwd?: string;
	private stopping = false;
	private readonly panes = new PaneHost(WORKSPACE_PANES, () => ({cwd: this.cwd}));

	async start(): Promise<void> {
		await ensureNodePtyReady();
		process.on('message', message => void this.handle(message as WorkspaceWorkerCommand));
		process.on('disconnect', () => void this.panes.stopAll().finally(() => process.exit(0)));
	}

	private async handle(command: WorkspaceWorkerCommand): Promise<void> {
		try {
			// Nothing may start in a worker that is shutting down (its worktree may be about to be removed).
			if (this.stopping && command.type !== 'shutdown' && command.type !== 'detach') throw new Error('workspace worker is stopping');
			switch (command.type) {
				case 'start': this.cwd = command.cwd; ok(command.requestId, {ok: true}); return;
				case 'start-dev': ok(command.requestId, await this.panes.startDev(command.cols, command.rows, command.command)); return;
				case 'stop-dev': await this.stopPane('dev'); ok(command.requestId, {idle: this.panes.idle}); return;
				case 'start-action': ok(command.requestId, await this.panes.startAction(command.cols, command.rows, command.command, command.name)); return;
				case 'stop-action': await this.stopPane('action'); ok(command.requestId, {idle: this.panes.idle}); return;
				case 'idle': ok(command.requestId, {idle: this.panes.idle}); return;
				case 'shutdown': this.stopping = true; await this.panes.stopAll(); ok(command.requestId, {ok: true}); setTimeout(() => process.exit(0), 25).unref?.(); return;
			}
			await handlePaneCommand(this.panes, command);
		} catch (error) {
			if ('requestId' in command) fail(command.requestId, error);
		}
	}

	// Responds once the pane is signalled, not once it exited.
	private async stopPane(target: PaneTarget): Promise<void> {
		void this.panes.stop(target);
		post(paneUpdated(target, this.panes.record(target)));
	}
}

export async function runWorkspaceWorker(): Promise<void> {
	process.title = 'deckhand-workspace-worker';
	await new WorkspaceWorker().start();
	await new Promise(() => {});
}
