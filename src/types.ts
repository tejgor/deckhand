export type ProgramKey = 'claude' | 'pi' | 'codex';

export type SessionStatus = 'starting' | 'running' | 'exited';
export type AgentActivityStatus = 'unknown' | 'active' | 'idle';
export type WorktreeMode = 'none' | 'new' | 'existing';
export type SessionWorktreeMode = 'none' | 'managed' | 'attached';
export type AttachTarget = 'agent' | 'terminal' | 'git' | 'dev';
export type RightPaneTab = 'preview' | 'terminal' | 'git' | 'dev';
export type WorktreeMergeMode = 'merge' | 'squash';

export interface SessionWorktreeRecord {
	mode: SessionWorktreeMode;
	path?: string;
	branch?: string;
	head?: string;
	isMain?: boolean;
	origin?: 'created' | 'existing' | 'selected';
	creator?: 'script' | 'fallback' | 'picker';
	name?: string;
}

export interface WorktreeInfoRecord {
	path: string;
	branch: string;
	head: string;
	isMain: boolean;
}

export interface WorktreeMergeResult {
	mode: WorktreeMergeMode;
	sourceRef: string;
	targetBranch: string;
	stdout: string;
	stderr: string;
}

export interface SessionRecord {
	id: string;
	title: string;
	program: ProgramKey;
	command: string;
	cwd: string;
	repoRoot: string;
	launchCwd?: string;
	launchWorktreeRoot?: string;
	worktree?: SessionWorktreeRecord;
	status: SessionStatus;
	agentStatus?: AgentActivityStatus;
	agentStatusUpdatedAt?: string;
	createdAt: string;
	updatedAt: string;
	pid?: number;
	exitCode?: number | null;
	exitSignal?: number | null;
	lastPreview?: string;
	devRunning?: boolean;
}

export interface PreviewRecord {
	sessionId?: string;
	content: string;
	live: boolean;
	status?: SessionStatus;
	agentStatus?: AgentActivityStatus;
}

export interface TerminalRecord {
	sessionId?: string;
	content: string;
	live: boolean;
	cwd?: string;
	exitCode?: number | null;
	exitSignal?: number | null;
}

export interface GitRecord {
	sessionId?: string;
	content: string;
	live: boolean;
	cwd?: string;
	exitCode?: number | null;
	exitSignal?: number | null;
}

export interface DevRecord {
	sessionId?: string;
	content: string;
	live: boolean;
	cwd?: string;
	command?: string;
	exitCode?: number | null;
	exitSignal?: number | null;
}

export interface CreateSessionInput {
	title: string;
	program: ProgramKey;
	cwd: string;
	repoRoot: string;
	cols: number;
	rows: number;
	worktreeMode?: WorktreeMode;
	existingWorktreePath?: string;
}

export type ClientRequest =
	| {type: 'ping'; requestId: string}
	| {type: 'list'; requestId: string}
	| {type: 'subscribe'; requestId: string; repoRoot: string}
	| {type: 'list-worktrees'; requestId: string; cwd: string}
	| {type: 'watch-preview'; requestId: string; sessionId?: string; cols: number; rows: number}
	| {type: 'watch-terminal'; requestId: string; sessionId?: string; cols: number; rows: number}
	| {type: 'watch-git'; requestId: string; sessionId?: string; cols: number; rows: number}
	| {type: 'watch-dev'; requestId: string; sessionId?: string; cols: number; rows: number}
	| {type: 'start-dev'; requestId: string; sessionId: string; cols: number; rows: number}
	| {type: 'stop-dev'; requestId: string; sessionId: string}
	| {type: 'create'; requestId: string; input: CreateSessionInput}
	| {type: 'restart'; requestId: string; sessionId: string; cols: number; rows: number}
	| {type: 'kill'; requestId: string; sessionId: string; deleteWorktree?: boolean; deleteBranch?: boolean; force?: boolean}
	| {type: 'merge-worktree'; requestId: string; sessionId: string; mode: WorktreeMergeMode; targetCwd: string}
	| {type: 'remove'; requestId: string; sessionId: string}
	| {type: 'attach'; requestId: string; sessionId: string; cols?: number; rows?: number}
	| {type: 'input'; sessionId: string; data: string}
	| {type: 'resize'; sessionId: string; cols: number; rows: number}
	| {type: 'detach'; sessionId: string}
	| {type: 'attach-terminal'; requestId: string; sessionId: string; cols?: number; rows?: number}
	| {type: 'terminal-input'; sessionId: string; data: string}
	| {type: 'terminal-resize'; sessionId: string; cols: number; rows: number}
	| {type: 'terminal-detach'; sessionId: string}
	| {type: 'attach-git'; requestId: string; sessionId: string; cols?: number; rows?: number}
	| {type: 'git-input'; sessionId: string; data: string}
	| {type: 'git-resize'; sessionId: string; cols: number; rows: number}
	| {type: 'git-detach'; sessionId: string}
	| {type: 'attach-dev'; requestId: string; sessionId: string; cols?: number; rows?: number}
	| {type: 'dev-input'; sessionId: string; data: string}
	| {type: 'dev-resize'; sessionId: string; cols: number; rows: number}
	| {type: 'dev-detach'; sessionId: string};

export type ServerResponse<T = unknown> = {
	type: 'response';
	requestId: string;
	ok: boolean;
	data?: T;
	error?: string;
};

export type ServerEvent =
	| {type: 'output'; sessionId: string; data: string}
	| {type: 'session-updated'; session: SessionRecord}
	| {type: 'session-removed'; sessionId: string}
	| {type: 'preview-updated'; preview: PreviewRecord}
	| {type: 'terminal-updated'; terminal: TerminalRecord}
	| {type: 'git-updated'; git: GitRecord}
	| {type: 'dev-updated'; dev: DevRecord}
	| {type: 'terminal-output'; sessionId: string; data: string}
	| {type: 'git-output'; sessionId: string; data: string}
	| {type: 'dev-output'; sessionId: string; data: string}
	| {type: 'attached'; sessionId: string}
	| {type: 'detached'; sessionId: string}
	| {type: 'terminal-attached'; sessionId: string}
	| {type: 'terminal-detached'; sessionId: string}
	| {type: 'git-attached'; sessionId: string}
	| {type: 'git-detached'; sessionId: string}
	| {type: 'dev-attached'; sessionId: string}
	| {type: 'dev-detached'; sessionId: string};

export type ServerMessage = ServerResponse | ServerEvent;

export type UiExitResult =
	| {kind: 'quit'}
	| {kind: 'attach'; sessionId: string; target: AttachTarget; title?: string; cwd?: string};
