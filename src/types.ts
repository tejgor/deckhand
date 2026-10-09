import type {AttentionState} from './agentSignals.js';
import type {WorkspaceSummary, CleanupInspection, CreatePrResult} from './workspaceGit.js';
import type {LoadedProject, ProjectConfig} from './projectConfig.js';
import type {ConfigTargetKind, ConfigTargets, ProjectConfigDocument, SavedConfigDocument} from './projectConfigDocument.js';
import type {SettingsInfo, WorktreeCandidates} from './settingsInfo.js';
import type {ChangeDiff, ChangeGroup, ChangesRecord} from './changesModel.js';
import type {TaskOp} from './tasks.js';

// Bump whenever the daemon/client request or response shape changes.
export const PROTOCOL_VERSION = 41;

export type ProgramKey = 'claude' | 'pi' | 'codex';

export type SessionStatus = 'starting' | 'running' | 'exited';
export type AgentActivityStatus = 'unknown' | 'active' | 'idle';
export type WorktreeMode = 'none' | 'new' | 'existing';
export type SessionWorktreeMode = 'none' | 'managed' | 'attached';
export type AttachTarget = 'agent' | 'terminal' | 'git' | 'dev' | 'action';
export type RightPaneTab = 'preview' | 'terminal' | 'git' | 'dev' | 'notes';
export type WorktreeMergeMode = 'merge' | 'squash';
export type RestartMode = 'resume' | 'fresh';

export interface SessionWorktreeRecord {
	mode: SessionWorktreeMode;
	/**
	 * The `WorktreeRecord` (incarnation) of the linked worktree the session runs in: set for managed/attached non-main
	 * worktrees and for sessions without their own worktree (mode `none`, e.g. sub-sessions) launched in one. Its markers
	 * below are then projected from that record by the daemon, never stored on the session (src/worktreeRecords.ts).
	 */
	id?: string;
	path?: string;
	branch?: string;
	head?: string;
	isMain?: boolean;
	origin?: 'created' | 'existing' | 'selected';
	creator?: 'script' | 'fallback' | 'picker';
	name?: string;
	mergedAt?: string;
	mergeMode?: WorktreeMergeMode;
	mergeTargetBranch?: string;
	mergeSourceRef?: string;
	/** The source commit at merge time (`mergeSourceRef` is a branch that moves): commits up to it count as integrated. */
	mergeSourceSha?: string;
	mergeMarkedManually?: boolean;
	/** The marker was set because the branch was found merged outside Deckhand (`ancestor` of the default branch, or a merged `pr`). */
	mergeDetected?: 'ancestor' | 'pr';
	baseRef?: string;
	deletedAt?: string;
	/** Worktree settings links applied when the worktree was created; notes list skipped/failed entries. */
	links?: {linked: string[]; notes: string[]};
}

/** The merge and deletion markers of a worktree (see WorktreeRecord). */
export type WorktreeMarkers = Pick<SessionWorktreeRecord, 'mergedAt' | 'mergeMode' | 'mergeTargetBranch' | 'mergeSourceRef' | 'mergeSourceSha' | 'mergeMarkedManually' | 'mergeDetected' | 'deletedAt'>;

/**
 * One incarnation of a linked worktree, shared by every session in it (state.json `worktrees`): created when Deckhand
 * creates or first attaches the worktree. A worktree later created at the same path is a new incarnation.
 */
export interface WorktreeRecord extends WorktreeMarkers {
	id: string;
	/** The worktree root, as Git reports it. */
	path: string;
	createdAt: string;
	/**
	 * The commit its branch started from (the branch's creation in its reflog, else HEAD when the record was created):
	 * merge detection needs at least one commit beyond it. Never projected into sessions.
	 */
	baseSha?: string;
	/** The branch tip when `M` cleared the marker: merge detection leaves the worktree alone until the tip moves. */
	mergeDismissedTip?: string;
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
	/** The commit that was merged (after committing the worktree's changes first). */
	sourceSha?: string;
	targetBranch: string;
	skipped?: boolean;
	conflicted?: boolean;
	/** Conflicted files (repository-relative, at most MAX_CONFLICT_PATHS) and their total. */
	conflicts?: string[];
	conflictCount?: number;
	/** The worktree's uncommitted changes were committed first (`commitFirst`). */
	committed?: {files: number; sha: string};
	reason?: string;
	stdout: string;
	stderr: string;
}

/** What `m` would merge (`merge-preview`): read-only, bounded. */
export interface MergePreview {
	/** The source worktree's branch, or its HEAD commit when detached. */
	sourceRef: string;
	sourceSha?: string;
	/** The worktree the merge goes into (Deckhand's launch checkout) and its branch (undefined: detached). */
	targetRoot: string;
	targetBranch?: string;
	/** The target is the repository's main checkout; `defaultBranch` is the repository's default (origin/HEAD, main or master). */
	targetIsMain: boolean;
	defaultBranch?: string;
	/** Commits in `<target>..<source>`: the count and the first subjects (newest first). */
	commitCount: number;
	commits: string[];
	/** Committed diff stat of `<target>...<source>`. */
	diff: {files: number; insertions: number; deletions: number};
	/** Uncommitted (changed or untracked, not ignored) files in the source worktree. */
	uncommitted: number;
	/** An operation in progress in the target, which refuses the merge. */
	inProgress?: TargetOperation;
	/** Files with uncommitted changes in the target that the merge would touch: by the commits, and by the uncommitted files. */
	overlap: {committed: string[]; uncommitted: string[]};
}
export type TargetOperation = 'merge' | 'rebase' | 'cherry-pick' | 'revert' | 'conflicts';

export interface AgentSessionRef {
	provider: ProgramKey;
	kind: 'name' | 'path' | 'id';
	value: string;
}

export type SubSessionKind = 'clean' | 'forked';

export interface SessionRecord {
	id: string;
	title: string;
	program: ProgramKey;
	command: string;
	args?: string[];
	agentSessionRef?: AgentSessionRef;
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
	/**
	 * The session's own note (`notes/sessions/<id>.md` in the state directory). Projected by the daemon from the file,
	 * never stored in state.json (older state that still has it is migrated into the file).
	 */
	notes?: string;
	/** The session note's file and revision (projected, like `notes`). */
	notesFile?: NoteFile;
	/** The note shared by every session of the session's worktree (or main checkout), projected by the daemon. */
	sharedNotes?: SharedNote;
	/** The session's workspace Dev is running (mirrored on every session in that worktree; never survives a daemon restart). */
	devRunning?: boolean;
	mergedAt?: string;
	mergeTargetBranch?: string;
	mergeSourceRef?: string;
	mergeMarkedManually?: boolean;
	parentSessionId?: string;
	subSessionKind?: SubSessionKind;
	forkedFromSessionId?: string;
	forkedFromAgentSessionRef?: AgentSessionRef;
	sidebarOrder?: number;
	archivedAt?: string;
	launchId?: string;
	agentStartedAt?: string;
	attention?: {state: AttentionState; event: string; at: string};
	exitReason?: 'stopped' | 'failed' | 'completed' | 'interrupted';
	cleanupError?: string;
	setup?: {command: string; state: 'pending' | 'running' | 'failed' | 'cancelled' | 'complete'; output: string; exitCode?: number | null};
	handoffPath?: string;
	requestedWorktreeMode?: WorktreeMode;
	/** The agent version (x.y.z) the session's agent last launched with; it stays outdated until restarted. */
	agentVersion?: string;
	/** Marked done with `D` (any session; independent of merged). */
	doneAt?: string;
	/** Started from a task: typed (not sent) into the agent's input once it first settles; cleared once typed or given up. */
	startPrompt?: string;
}

/** A note file as the daemon last read it (see src/notesStore.ts). */
export interface NoteFile {
	/** The Markdown file; created on the first save or when opened in the editor. */
	path: string;
	/** Hash of the file's text (`noteRevision`); a save names the revision it edited and is refused when the file changed. */
	revision: string;
	/** Longer than MAX_NOTES_CHARS on disk: shown cut, edited only in an editor. */
	tooLarge?: boolean;
}

/** A worktree's (or the main checkout's) note shared by its sessions. */
export interface SharedNote extends NoteFile {
	/** `worktree`: one linked worktree incarnation (`id` = its record ID); `repo`: the main checkout (`id` = hash of its root). */
	kind: 'worktree' | 'repo';
	id: string;
	text: string;
	/** Its worktree was deleted: shown, but not editable. */
	readOnly?: boolean;
}

/** A repository's task list (`watch-tasks`, `tasks-updated`): one Markdown file, parsed by src/tasks.ts. */
export interface TasksDoc {
	/** Hash of the repository's main checkout (as for its note); the same for every worktree of it. */
	key: string;
	path: string;
	text: string;
	revision: string;
	tooLarge?: boolean;
}

/** `list-branches`: local branches a new worktree can start from, most recently committed first. */
export interface BranchList {
	branches: string[];
	current?: string;
	defaultBranch?: string;
	/** The worktree.branchFrom setting in effect, i.e. what "default" means in the new-session form. */
	branchFrom: 'current' | 'default' | 'origin';
	/** A trusted creation hook decides where new worktrees start; the base cannot be chosen. */
	hook?: boolean;
}

/** `save-note`: `saved` false when the file changed since `revision` (nothing written; `session` carries the file as it is). */
export interface NoteSaveResult {saved: boolean; session: SessionRecord}

/** One agent's versions (`agent-versions`): installed (`<command> --version`), latest (npm), and its sessions. */
export interface AgentVersionInfo {
	program: ProgramKey;
	/** x.y.z of the binary found on the daemon's PATH; absent when not installed or unreadable. */
	installed?: string;
	/** x.y.z of the npm `latest` dist-tag; absent while unknown (never checked, offline, npm missing). */
	latest?: string;
	/** The resolved binary. */
	path?: string;
	/** When `latest` was last looked up (ISO). */
	checkedAt?: string;
	/** Why a version is unknown (shown as a note, never as an error). */
	error?: string;
	/** Its update command is running. */
	updating?: boolean;
	/** Running sessions of this agent, and how many of them launched with an older version than the installed one. */
	running: number;
	outdated: number;
}
export type AgentVersions = Record<ProgramKey, AgentVersionInfo>;
export interface AgentUpdateResult {
	program: ProgramKey;
	ok: boolean;
	exitCode: number | null;
	/** The command that ran, for display. */
	command: string;
	/** The last part of its combined output. */
	output: string;
	before?: string;
	after?: string;
	versions: AgentVersions;
}

export interface PreviewRecord {
	sessionId?: string;
	content: string;
	live: boolean;
	status?: SessionStatus;
	agentStatus?: AgentActivityStatus;
	scrollOffset?: number;
	maxScrollOffset?: number;
}

/** The Terminal (shell) pane shared by every session in one workspace; `sessionId` is the session it was watched for. */
export interface TerminalRecord {
	sessionId?: string;
	/** The workspace (worktree root, see src/workspace.ts) whose shell this is; absent while the session has none. */
	workspace?: string;
	content: string;
	live: boolean;
	cwd?: string;
	exitCode?: number | null;
	exitSignal?: number | null;
}

/** The Git (lazygit) pane shared by every session in one workspace; `sessionId` is the session it was watched for. */
export interface GitRecord {
	sessionId?: string;
	/** The workspace whose lazygit this is; absent while the session has none. */
	workspace?: string;
	content: string;
	live: boolean;
	cwd?: string;
	exitCode?: number | null;
	exitSignal?: number | null;
}

/** The Dev pane shared by every session in one workspace; `sessionId` is the session it was requested/watched for. */
export interface DevRecord {
	sessionId?: string;
	/** The workspace (worktree root, see src/workspace.ts) whose Dev this is; absent while the session has none. */
	workspace?: string;
	content: string;
	live: boolean;
	cwd?: string;
	command?: string;
	exitCode?: number | null;
	exitSignal?: number | null;
}

/**
 * The last action (`e`) run in one workspace, shown on the Terminal tab beside the shell: its own PTY, so it never
 * touches the shell or Dev. `name` is the action's name, `command` what ran; output and exit code stay after it exits.
 */
export interface ActionRecord extends DevRecord {
	name?: string;
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
	parentSessionId?: string;
	subSessionKind?: SubSessionKind;
	handoffFromSessionId?: string;
	/** A new worktree's branch starts at this local branch (or `origin/<name>`) instead of worktree.branchFrom. */
	baseBranch?: string;
	/** Started from this task of the repository's list: the task is linked to the new session's worktree (or the session). */
	taskId?: string;
	/** Fingerprint of the repository configuration the user reviewed; an untrusted one it matches is skipped, not refused. */
	projectFingerprint?: string;
}

export type ClientRequest =
	| {type: 'ping'; requestId: string}
	| {type: 'shutdown'; requestId: string}
	| {type: 'project-info'; requestId: string; cwd: string}
	| {type: 'save-config'; requestId: string; target: ConfigTargetKind; cwd: string; raw: string; revision: string | null}
	/** C → Settings: each effective setting with its source and pending (untrusted) value, trust state and both editable documents. Read-only. */
	| {type: 'settings-info'; requestId: string; cwd: string}
	/** Settings → Linked items: untracked/ignored entries of the main checkout plus configured links. Read-only. */
	| {type: 'worktree-candidates'; requestId: string; cwd: string}
	/** Bounded sizes (KiB, null when unknown) of candidate paths relative to the main checkout. */
	| {type: 'worktree-candidate-sizes'; requestId: string; cwd: string; paths: string[]}
	| {type: 'trust-project'; requestId: string; cwd: string; fingerprint: string}
	| {type: 'workspace-summary'; requestId: string; sessionId: string; includePr?: boolean}
	| {type: 'inspect-cleanup'; requestId: string; sessionId: string; deleteBranch?: boolean}
	/** Pushes the session's branch and opens GitHub's PR form; `branch` (as confirmed) must still be checked out. */
	| {type: 'create-pr'; requestId: string; sessionId: string; branch?: string}
	| {type: 'archive-session'; requestId: string; sessionId: string; archived: boolean}
	| {type: 'export-handoff'; requestId: string; sessionId: string; includeOutput?: boolean}
	| {type: 'agent-hook'; requestId: string; sessionId: string; launchId: string; token: string; payload: unknown}
	| {type: 'cancel-start'; requestId: string; sessionId: string}
	| {type: 'run-action'; requestId: string; sessionId: string; action: string; cols: number; rows: number}
	| {type: 'list'; requestId: string}
	/** Installed and latest version of every agent; `refresh` looks the latest up again (bounded) before answering. */
	| {type: 'agent-versions'; requestId: string; refresh?: boolean}
	/** Runs the agent's own update command (one at a time per agent); never touches sessions. */
	| {type: 'update-agent'; requestId: string; program: ProgramKey}
	| {type: 'subscribe'; requestId: string; repoRoot: string}
	| {type: 'list-worktrees'; requestId: string; cwd: string}
	| {type: 'watch-preview'; requestId: string; sessionId?: string; cols: number; rows: number; scrollOffset?: number}
	| {type: 'watch-terminal'; requestId: string; sessionId?: string; cols: number; rows: number}
	| {type: 'watch-git'; requestId: string; sessionId?: string; cols: number; rows: number}
	| {type: 'watch-dev'; requestId: string; sessionId?: string; cols: number; rows: number}
	| {type: 'watch-action'; requestId: string; sessionId?: string; cols: number; rows: number}
	/** The Git tab's Changes view of the session's workspace; pushes `changes-updated` while watched. No sessionId stops watching. */
	| {type: 'watch-changes'; requestId: string; sessionId?: string}
	/** One listed entry's diff preview (read-only, bounded). */
	| {type: 'changes-diff'; requestId: string; sessionId: string; group: ChangeGroup; path: string}
	/** Stages/unstages one listed entry, or everything (no `path`); responds with the refreshed ChangesRecord. */
	| {type: 'change-stage'; requestId: string; sessionId: string; mode: 'stage' | 'unstage'; group?: ChangeGroup; path?: string}
	| {type: 'start-dev'; requestId: string; sessionId: string; cols: number; rows: number}
	| {type: 'stop-dev'; requestId: string; sessionId: string}
	| {type: 'stop-action'; requestId: string; sessionId: string}
	/**
	 * Saves the session's own note or its shared note (`noteId`: `sharedNotes.kind:sharedNotes.id`, so it is the note the
	 * UI edited) when the file still has `revision`; text is cut to MAX_NOTES_CHARS.
	 */
	| {type: 'save-note'; requestId: string; sessionId: string; section: 'session' | 'shared'; noteId?: string; text: string; revision: string}
	/** Creates the note's file if missing and responds with its path (for the editor). */
	| {type: 'open-note'; requestId: string; sessionId: string; section: 'session' | 'shared'}
	/** The task list of the repository `cwd` belongs to; pushes `tasks-updated` while it is this client's list. */
	| {type: 'watch-tasks'; requestId: string; cwd: string}
	/** Applies one change to that list (src/tasks.ts TaskOp) and responds with the TasksDoc. */
	| {type: 'task-op'; requestId: string; cwd: string; op: TaskOp}
	/** Creates the task list's file if missing and responds with its path (for the editor). */
	| {type: 'open-tasks'; requestId: string; cwd: string}
	/** Sends the open checklist item on `line` of a note (at `revision`) to the repository's tasks; the line becomes a link. */
	| {type: 'promote-note-item'; requestId: string; sessionId: string; section: 'session' | 'shared'; noteId?: string; line: number; revision: string}
	| {type: 'list-branches'; requestId: string; cwd: string}
	| {type: 'create'; requestId: string; input: CreateSessionInput}
	| {type: 'reorder-session'; requestId: string; sessionId: string; direction: 'up' | 'down'}
	| {type: 'restart'; requestId: string; sessionId: string; cols: number; rows: number; mode?: RestartMode; projectFingerprint?: string}
	| {type: 'kill'; requestId: string; sessionId: string; deleteWorktree?: boolean; deleteBranch?: boolean; force?: boolean; allowDataLoss?: boolean}
	/** What `m` would merge into the worktree at `targetCwd`; read-only. */
	| {type: 'merge-preview'; requestId: string; sessionId: string; targetCwd: string}
	/** `commitFirst`: commit the source worktree's uncommitted changes (`git add -A`, message = session title) before merging. */
	| {type: 'merge-worktree'; requestId: string; sessionId: string; mode: WorktreeMergeMode; targetCwd: string; commitFirst?: boolean}
	/** After a conflicted merge: `keep` leaves it in progress and marks the worktree merged; `abort` undoes it. */
	| {type: 'resolve-merge'; requestId: string; sessionId: string; targetCwd: string; action: 'keep' | 'abort'}
	| {type: 'mark-session-merged'; requestId: string; sessionId: string; targetCwd: string}
	| {type: 'set-session-done'; requestId: string; sessionId: string; done: boolean}
	/** `moveOpenItems`: first send the open checklist items of the notes removed with it to the repository's tasks. */
	| {type: 'remove'; requestId: string; sessionId: string; moveOpenItems?: boolean}
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
	| {type: 'dev-detach'; sessionId: string}
	| {type: 'attach-action'; requestId: string; sessionId: string; cols?: number; rows?: number}
	| {type: 'action-input'; sessionId: string; data: string}
	| {type: 'action-resize'; sessionId: string; cols: number; rows: number}
	| {type: 'action-detach'; sessionId: string};

/** A repository's override plus what the inline review needs; `effective` is global defaults overlaid by the override only when trusted. */
export interface ProjectInfo extends LoadedProject {trusted: boolean; needsReview: boolean; effective: ProjectConfig}
// reasons/safe describe data that DELETE (allowDataLoss) may override; structuralBlockers
// (main/current/shared/missing worktree, protected branch) can never be overridden.
export type SessionCleanupInspection = CleanupInspection & {structuralBlockers: string[]};
export type {WorkspaceSummary, CleanupInspection, CreatePrResult, ProjectConfigDocument, SavedConfigDocument, ConfigTargets, ConfigTargetKind, SettingsInfo, WorktreeCandidates, ChangeDiff, ChangeGroup, ChangesRecord};

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
	| {type: 'action-updated'; action: ActionRecord}
	| {type: 'changes-updated'; changes: ChangesRecord}
	| {type: 'agent-versions-updated'; versions: AgentVersions}
	| {type: 'tasks-updated'; tasks: TasksDoc}
	| {type: 'terminal-output'; sessionId: string; data: string}
	| {type: 'git-output'; sessionId: string; data: string}
	| {type: 'dev-output'; sessionId: string; data: string}
	| {type: 'action-output'; sessionId: string; data: string}
	| {type: 'attached'; sessionId: string}
	| {type: 'detached'; sessionId: string}
	| {type: 'terminal-attached'; sessionId: string}
	| {type: 'terminal-detached'; sessionId: string}
	| {type: 'git-attached'; sessionId: string}
	| {type: 'git-detached'; sessionId: string}
	| {type: 'dev-attached'; sessionId: string}
	| {type: 'dev-detached'; sessionId: string}
	| {type: 'action-attached'; sessionId: string}
	| {type: 'action-detached'; sessionId: string};

export type ServerMessage = ServerResponse | ServerEvent;

export type UiExitResult =
	| {kind: 'quit'}
	| {kind: 'attach'; sessionId: string; target: AttachTarget; title?: string; cwd?: string; program?: ProgramKey};
