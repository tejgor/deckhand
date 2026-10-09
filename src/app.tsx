import React, {useCallback, useEffect, useMemo, useRef, useState} from 'react';
import {Box, Text, useApp} from 'ink';
import {useTerminalInput} from './useTerminalInput.js';
import {LiveClient, createLiveClient} from './client.js';
import {loadAppConfig, updateAppConfig} from './storage.js';
import {DevPane} from './devPane.js';
import {DetailsPane, detailsViewport, scrollDetails} from './detailsPane.js';
import {cleanupOverrideText, createPrConfirmText, projectActions, trustReviewText, workspaceSummaryText} from './detailTexts.js';
import {openInEditor, openUrl} from './desktop.js';
import {MenuList, MenuPane, SelectableRow, fitHint, type HintPart} from './menu.js';
import {isSettingsFlowMode, useSettingsFlow} from './settingsFlow.js';
import {useHelp} from './helpPane.js';
import {useAgentsFlow} from './agentsFlow.js';
import {installedVersions, updateHint} from './agentVersions.js';
import {filterCycleMessage, filterSessionList, nextSessionFilter, sessionNeedsAttention, type SessionFilter} from './sessionFeatures.js';
import {useChangesFlow} from './changesFlow.js';
import {emptyChanges, type ChangesRecord} from './changesModel.js';
import {useNotesFlow} from './notesFlow.js';
import {TaskBanner, useTasksFlow} from './tasksFlow.js';
import {parseTasks, type Task} from './tasks.js';
import {linkedTask, openItemsRemovedWith, taskCountLabel} from './tasksBoard.js';
import {PreviewPane} from './preview.js';
import {sessionMatchesScope} from './sessionScope.js';
import {noWorkspaceReason, workspaceKey} from './workspace.js';
import {Sidebar} from './sidebar.js';
import {msUntilAgeChanges, statusSince} from './sidebarModel.js';
import {conflictView, mergeConfirmLayout, type MergeLine, type MergeNoteEntry} from './mergeModel.js';
import {filterCollapsedSessions, sessionDescendants, sessionHasChildren, sortSessionsForSidebar} from './sessionOrder.js';
import {TabBar} from './tabs.js';
import {TerminalPane, actionStatus, hasAction, type TerminalView} from './terminalPane.js';
import {AGENTS} from './agents.js';
import type {ActionRecord, AgentVersions, AttachTarget, BranchList, TasksDoc, DevRecord, MergePreview, PreviewRecord, ProgramKey, RestartMode, RightPaneTab, SessionRecord, SubSessionKind, TerminalRecord, UiExitResult, WorktreeInfoRecord, WorktreeMergeMode, WorktreeMergeResult, WorktreeMode, ProjectInfo, WorkspaceSummary, SessionCleanupInspection} from './types.js';
import {THEME, compactPath, displaySessionTitle, errorMessage, stripTerminalControls, truncate} from './ui.js';

const RIGHT_TABS: RightPaneTab[] = ['preview', 'terminal', 'git', 'dev', 'notes'];
const EMPTY_ID_SET: ReadonlySet<string> = new Set();

const PROGRAMS: Array<{key: ProgramKey; label: string; glyph: string}> = [
	{key: 'claude', label: 'Claude', glyph: '✶'},
	{key: 'pi', label: 'Pi', glyph: 'π'},
	{key: 'codex', label: 'Codex', glyph: '◇'},
];

const EMPTY_PREVIEW: PreviewRecord = {
	content: '',
	live: false,
};

const EMPTY_TERMINAL: TerminalRecord = {
	content: '',
	live: false,
};

const EMPTY_CHANGES: ChangesRecord = emptyChanges();

const EMPTY_DEV: DevRecord = {
	content: '',
	live: false,
};

const EMPTY_ACTION: ActionRecord = {
	content: '',
	live: false,
};

const SPINNER_FRAMES = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const WORKTREE_MODES: Array<{key: WorktreeMode; label: string}> = [
	{key: 'none', label: 'no worktree'},
	{key: 'new', label: 'new worktree'},
	{key: 'existing', label: 'existing worktree'},
];
const DEFAULT_SCROLL_SENSITIVITY = 0.12;
const SCROLL_SENSITIVITY_STEP = 0.04;
const STATUS_MESSAGE_AUTO_HIDE_MS = 5000;
const HEADER_ROWS = 2;
// The footer always renders exactly these rows: the key hint and one combined
// message line. Ink repaints the whole screen once output reaches the terminal
// height, so one spare row is kept below the footer.
const FOOTER_ROWS = 2;
const SPARE_ROWS = 1;
const ERROR_MESSAGE_AUTO_HIDE_MS = 8000;

const ORPHAN_TERMINAL_SEQUENCE_PATTERN = /^(?:\[(?:[ABCDHFIOZ]|\d+(?:;\d+)*[~ABCDHF])|O[ABCDHF])$/;
const ORPHAN_MOUSE_SEQUENCE_PATTERN = /^(?:\[?<\d*(?:;\d*){0,2}[mM]?|\[?\d+;\d*(?:;\d*)?[mM]?|\[?M[\s\S]{0,3})$/;
const ALLOWED_NAME_INPUT_PATTERN = /[^a-zA-Z0-9 _\-/.:[\]()#]/g;

function normalizeScrollSensitivity(value: number | undefined): number {
	if (value === undefined || !Number.isFinite(value)) {
		return DEFAULT_SCROLL_SENSITIVITY;
	}
	return Math.max(0, Math.min(1, value));
}

function formatScrollSensitivity(value: number): string {
	return value.toFixed(2).replace(/0+$/, '').replace(/\.$/, '');
}

function mouseWheelSequence(direction: 'up' | 'down', cols: number, rows: number, count = 1): string {
	const button = direction === 'up' ? 64 : 65;
	const x = Math.max(1, Math.floor(cols / 2));
	const y = Math.max(1, Math.floor(rows / 2));
	return `\u001B[<${button};${x};${y}M`.repeat(Math.max(1, count));
}

function parseMouseWheel(input: string): {direction: 'up' | 'down'; count: number} | undefined {
	let up = 0;
	let down = 0;

	for (const match of input.matchAll(/\u001B\[<(\d+);\d+;\d+(?:;\d+;\d+)?[mM]/g)) {
		const button = Number(match[1]);
		if ((button & 64) === 64) {
			if ((button & 1) === 1) down += 1;
			else up += 1;
		}
	}

	let legacyIndex = input.indexOf('\u001B[M');
	while (legacyIndex >= 0 && input.length >= legacyIndex + 6) {
		const button = input.charCodeAt(legacyIndex + 3) - 32;
		if ((button & 64) === 64) {
			if ((button & 1) === 1) down += 1;
			else up += 1;
		}
		legacyIndex = input.indexOf('\u001B[M', legacyIndex + 6);
	}

	if (up === 0 && down === 0) {
		return undefined;
	}
	return up > down ? {direction: 'up', count: up - down} : {direction: 'down', count: down - up};
}

function sanitizeNameInput(input: string): string {
	const cleaned = stripTerminalControls(input);

	if (ORPHAN_TERMINAL_SEQUENCE_PATTERN.test(cleaned) || ORPHAN_MOUSE_SEQUENCE_PATTERN.test(cleaned)) {
		return '';
	}

	return cleaned.replace(ALLOWED_NAME_INPUT_PATTERN, '');
}

type Mode = 'browse' | 'preview-focus' | 'changes-focus' | 'notes-focus' | 'pick-program' | 'enter-name' | 'pick-worktree' | 'confirm-kill' | 'confirm-merge' | 'merge-conflicts' | 'help' | 'settings' | 'edit-project' | 'discard-project' | 'search' | 'workspace-info' | 'review-project' | 'pick-action' | 'confirm-loss' | 'agents' | 'tasks' | 'confirm-remove';

interface AppProps {
	repoRoot: string;
	cwd: string;
	initialSelectedId?: string;
	initialActiveTab?: RightPaneTab;
	initialSidebarWidth?: number;
	initialSessionTabs?: Record<string, RightPaneTab>;
	initialCollapsedSessionIds?: string[];
	initialHiddenExitedSessionIds?: string[];
	initialSessionFilter?: SessionFilter;
	initialSessionQuery?: string;
	/** The Terminal tab's view (shell or last action), kept across attaches by the caller. */
	initialTerminalView?: TerminalView;
	onTerminalViewChange?: (view: TerminalView) => void;
	onSelectedIdChange?: (sessionId: string | undefined) => void;
	onActiveTabChange?: (tab: RightPaneTab) => void;
	onSessionTabChange?: (sessionId: string, tab: RightPaneTab) => void;
	onSidebarWidthChange?: (width: number) => void;
	onCollapsedSessionIdsChange?: (sessionIds: string[]) => void;
	onHiddenExitedSessionIdsChange?: (sessionIds: string[]) => void;
	onSessionVisibilityChange?: (filter: SessionFilter, query: string) => void;
}

interface TerminalSize {
	cols: number;
	rows: number;
}

function getTerminalSize(): TerminalSize {
	return {
		cols: process.stdout.columns || 80,
		rows: process.stdout.rows || 24,
	};
}

function sidebarWidth(totalWidth: number): number {
	if (totalWidth <= 0) {
		return 24;
	}
	let width = Math.floor(totalWidth * 0.24);
	width = Math.max(24, Math.min(34, width));
	return clampSidebarWidth(width, totalWidth);
}

function clampSidebarWidth(width: number, totalWidth: number): number {
	const minWidth = Math.min(18, Math.max(10, totalWidth - 23));
	const maxWidth = Math.max(minWidth, Math.min(Math.floor(totalWidth * 0.5), totalWidth - 23));
	return Math.max(minWidth, Math.min(maxWidth, Math.floor(width)));
}

function sortSessions(sessions: SessionRecord[]): SessionRecord[] {
	return sortSessionsForSidebar(sessions);
}

function upsertSession(existing: SessionRecord[], session: SessionRecord): SessionRecord[] {
	const next = existing.filter(item => item.id !== session.id);
	next.push(session);
	return sortSessions(next);
}

function describeConnection(client: LiveClient | undefined): string {
	return client ? 'ready' : 'connecting…';
}

function connectionColor(client: LiveClient | undefined): string {
	return client ? THEME.success : THEME.warn;
}

function supportsForkedSubSession(session: SessionRecord | undefined): boolean {
	return Boolean(session && AGENTS[session.program].forks);
}

// Forks of an agent that may reopen them in the parent's directory always run there (the daemon refuses other modes).
function forkStaysInParent(program: ProgramKey | undefined, subSessionKind: SubSessionKind | undefined): boolean {
	return subSessionKind === 'forked' && Boolean(program) && !AGENTS[program!].forksAcrossDirectories;
}

function parentWorkspaceLabel(session: SessionRecord | undefined, width: number): string | undefined {
	if (!session) {
		return undefined;
	}
	const worktree = session.worktree;
	if (worktree?.path && worktree.mode !== 'none') {
		const name = worktree.branch || worktree.name || compactPath(worktree.path, Math.max(8, width - 18));
		return `parent worktree: ${truncate(name, Math.max(8, width - 18))}`;
	}
	return `parent dir: ${compactPath(session.cwd, Math.max(8, width - 12))}`;
}

/** A session title (at most 64 characters) from a task's: cut at a word where it can be. */
function sessionTitleFromTask(title: string): string {
	const clean = sanitizeNameInput(title).replace(/\s+/g, ' ').trim();
	if (clean.length <= 64) return clean;
	const cut = clean.slice(0, 64);
	const space = cut.lastIndexOf(' ');
	return (space >= 32 ? cut.slice(0, space) : cut).trim();
}

/** Where a new worktree's branch can start (↑↓ in the form): the worktree.branchFrom setting first, then local branches. */
function baseOptions(list?: BranchList): Array<{label: string; value?: string}> {
	if (!list) return [{label: 'loading branches…'}];
	if (list.hook) return [{label: "decided by the repository's worktree hook"}];
	const setting = list.branchFrom === 'default' ? `${list.defaultBranch ?? 'the default branch'} (default branch)`
		: list.branchFrom === 'origin' ? `origin/${list.defaultBranch ?? 'main'} (fetched first)`
		: `current checkout${list.current ? ` (${list.current})` : ''}`;
	return [{label: setting}, ...list.branches.map(name => ({label: name, value: name}))];
}

function RemoveConfirmPane({session, items, width}: {session?: SessionRecord; items: string[]; width: number}) {
	const inner = Math.max(10, width - 4);
	const shown = items.slice(0, 8);
	return (
		<Box flexDirection="column" width={width} borderStyle="round" borderColor={THEME.borderActive} paddingX={1}>
			<Text color={THEME.accent} bold wrap="truncate-end">Remove “{session?.title ?? 'session'}” from the list?</Text>
			<Box marginTop={1} flexDirection="column">
				<Text wrap="truncate-end">Its notes have <Text color={THEME.warn}>{items.length} unchecked item{items.length === 1 ? '' : 's'}</Text>, deleted with them:</Text>
				{shown.map((item, index) => <Text key={index} wrap="truncate-end">{`  ☐ ${truncate(item, inner - 4)}`}</Text>)}
				{items.length > shown.length ? <Text color={THEME.muted}>{`  +${items.length - shown.length} more`}</Text> : null}
			</Box>
			<Box marginTop={1}><Text color={THEME.muted} wrap="truncate-end">m move them to Tasks, then remove · enter remove anyway · esc cancel</Text></Box>
		</Box>
	);
}

function CreatePane({
	mode,
	programIndex,
	draftName,
	worktreeMode,
	width,
	parentTitle,
	parentWorkspaceLabel,
	subSessionKind,
	showForkOption,
	taskTitle,
	base,
}: {
	mode: 'pick-program' | 'enter-name';
	programIndex: number;
	draftName: string;
	worktreeMode: WorktreeMode;
	width: number;
	parentTitle?: string;
	parentWorkspaceLabel?: string;
	subSessionKind?: SubSessionKind;
	showForkOption?: boolean;
	/** Started from this task (n on the board). */
	taskTitle?: string;
	/** The new worktree's base: the option shown, its position and the number of options. */
	base?: {label: string; index: number; count: number};
}) {
	const forkSelected = mode === 'pick-program' && showForkOption && programIndex === PROGRAMS.length;
	const contentWidth = Math.max(1, width - 4);
	const program = PROGRAMS[programIndex]?.key;
	const staysInParent = forkStaysInParent(program, subSessionKind);
	const workspaceLabel = parentWorkspaceLabel && (worktreeMode === 'none' || staysInParent)
		? parentWorkspaceLabel
		: WORKTREE_MODES.find(item => item.key === worktreeMode)?.label;
	return (
		<Box flexDirection="column" width={width} borderStyle="round" borderColor={THEME.borderActive} paddingX={1} paddingY={0}>
			<Text color={THEME.accent} bold>
				{mode === 'pick-program'
					? parentTitle ? `New sub-session under ${parentTitle}` : 'New session'
					: `New ${PROGRAMS[programIndex]!.label}${parentTitle ? ` ${subSessionKind ?? 'clean'} sub-` : ' '}session`}
			</Text>
			<Box marginTop={1} flexDirection="column">
				{mode === 'pick-program' ? (
					<>
						<Text color={THEME.muted}>Choose an agent</Text>
						{PROGRAMS.map((program, index) => <SelectableRow key={program.key} selected={index === programIndex} text={`${program.glyph} ${program.label}`} width={contentWidth} />)}
						{showForkOption ? <SelectableRow selected={Boolean(forkSelected)} text="⑂ Fork parent" width={contentWidth} /> : null}
					</>
				) : (
					<>
						{taskTitle ? <Text wrap="truncate-end">Task: <Text color={THEME.accentSoft}>{taskTitle}</Text></Text> : null}
						<Text>Name: <Text color={draftName ? THEME.active : THEME.muted}>{draftName || '█'}</Text></Text>
						<Text>Workspace: <Text color={THEME.accent}>{workspaceLabel}</Text></Text>
						{base && worktreeMode === 'new' && !staysInParent ? <Text wrap="truncate-end">Base: <Text color={THEME.active}>{base.label}</Text>{base.count > 1 ? <Text color={THEME.muted}>{`  ↑↓ ${base.index + 1}/${base.count}`}</Text> : null}</Text> : null}
						{taskTitle ? <Text color={THEME.muted} wrap="truncate-end">The agent starts with the task as its first message.</Text> : null}
						{staysInParent ? <Text color={THEME.muted}>{truncate(`${PROGRAMS[programIndex]!.label} forks stay in the parent's worktree`, contentWidth)}</Text> : null}
					</>
				)}
			</Box>
			{parentTitle ? <Text color={THEME.muted}>Parent: {truncate(parentTitle, Math.max(8, width - 12))}</Text> : null}
			<Box marginTop={1}>
				<Text color={THEME.muted}>
					{mode === 'pick-program' ? 'enter continue · esc cancel · ↑↓ switch' : staysInParent ? 'enter create · esc back' : worktreeMode === 'new' && base && base.count > 1 ? 'tab worktree · ↑↓ base · enter create · esc back' : 'tab worktree · enter create · esc back'}
				</Text>
			</Box>
		</Box>
	);
}

function worktreeLabel(worktree: WorktreeInfoRecord, width: number): string {
	const branch = worktree.branch || '(detached)';
	const prefix = worktree.isMain ? 'main · ' : '';
	const pathBudget = Math.max(8, width - prefix.length - branch.length - 2);
	return truncate(`${prefix}${branch}  ${compactPath(worktree.path, pathBudget)}`, width);
}

function WorktreePickerPane({
	worktrees,
	selectedIndex,
	query,
	totalCount,
	width,
	height,
}: {
	worktrees: WorktreeInfoRecord[];
	selectedIndex: number;
	query: string;
	totalCount: number;
	width: number;
	height: number;
}) {
	const contentWidth = Math.max(1, width - 4);
	const countLabel = query ? `${worktrees.length}/${totalCount}` : String(totalCount);
	return (
		<Box flexDirection="column" width={width} height={height} borderStyle="round" borderColor={THEME.borderActive} paddingX={1}>
			<Text color={THEME.accent} bold>Existing worktree</Text>
			<Text>
				Search: <Text color={query ? THEME.active : THEME.muted}>{query || 'type to filter'}</Text>{' '}
				<Text color={THEME.muted}>({countLabel})</Text>
			</Text>
			<Box marginTop={1} flexDirection="column">
				{totalCount === 0 ? <Text color={THEME.muted}>No worktrees found.</Text> : null}
				{totalCount > 0 && worktrees.length === 0 ? <Text color={THEME.muted}>No matching worktrees.</Text> : null}
				{worktrees.length > 0 ? <MenuList items={worktrees.map(worktree => ({key: worktree.path, label: worktreeLabel(worktree, contentWidth - 2)}))} selected={selectedIndex} width={contentWidth} rows={Math.max(1, height - 7)} /> : null}
			</Box>
			<Box marginTop={1}>
				<Text color={THEME.muted}>type search · enter select · esc back · ↑↓ move · backspace delete</Text>
			</Box>
		</Box>
	);
}

/** The notes shown on the merge confirmation: the worktree's shared note first, then the session's and its sub-sessions'. */
function mergeNoteEntries(session: SessionRecord | undefined, sessions: SessionRecord[]): MergeNoteEntry[] {
	const noteSessions = session ? [session, ...sessionDescendants(session.id, sessions)] : [];
	const entries = noteSessions
		.map(noteSession => ({key: noteSession.id, title: displaySessionTitle(noteSession, sessions), lines: (noteSession.notes?.trim() ?? '').split('\n').filter(Boolean)}))
		.filter(entry => entry.lines.length > 0);
	// The worktree's shared note comes first: it describes the worktree being merged.
	const worktreeLines = (session?.sharedNotes?.text.trim() ?? '').split('\n').filter(Boolean);
	if (session && worktreeLines.length) entries.unshift({key: 'worktree', title: `Worktree notes${session.worktree?.branch ? ` · ${session.worktree.branch}` : ''}`, lines: worktreeLines});
	return entries;
}

function MergeLines({lines}: {lines: MergeLine[]}) {
	return <>{lines.map((line, index) => <Text key={index} color={line.color} bold={line.bold} wrap="truncate-end">{line.text}</Text>)}</>;
}

export function MergeConfirmPane({session, sessions, flow, selectedIndex, width, height, tasks}: {session?: SessionRecord; sessions: SessionRecord[]; flow?: MergeFlow; selectedIndex: number; width: number; height: number; tasks?: string[]}) {
	const contentWidth = Math.max(1, width - 4);
	const layout = mergeConfirmLayout({
		title: session ? displaySessionTitle(session, sessions) : 'worktree',
		preview: flow?.preview, previewError: flow?.previewError, commitFirst: flow?.commitFirst ?? true,
		commitMessage: session?.title.trim() ?? '', error: flow?.error, notes: mergeNoteEntries(session, sessions), tasks, width: contentWidth, height,
	});
	return (
		<Box flexDirection="column" width={width} height={height} borderStyle="round" borderColor={THEME.borderActive} paddingX={1}>
			<Text color={THEME.accent} bold wrap="truncate-end">{layout.title}</Text>
			<MergeLines lines={layout.details} />
			<Box marginTop={1} flexDirection="column">
				{layout.options.map((option, index) => <SelectableRow key={option} selected={index === selectedIndex} text={option} width={contentWidth} selectedColor={option === 'Cancel' ? THEME.muted : THEME.active} />)}
			</Box>
			<Box marginTop={1}><Text color={THEME.muted} wrap="truncate-end">{layout.hint}</Text></Box>
			{layout.error.length ? <Box marginTop={1} flexDirection="column"><MergeLines lines={layout.error} /></Box> : null}
			{layout.notes.length ? <Box marginTop={1} flexDirection="column"><MergeLines lines={layout.notes} /></Box> : null}
		</Box>
	);
}

export function MergeConflictPane({result, width}: {result: WorktreeMergeResult; width: number}) {
	const contentWidth = Math.max(1, width - 4);
	const view = conflictView(result, contentWidth);
	return (
		<Box flexDirection="column" width={width} borderStyle="round" borderColor={THEME.warn} paddingX={1}>
			<Text color={THEME.warn} bold wrap="truncate-end">{view.title}</Text>
			{view.files.map((file, index) => <Text key={index} color={THEME.muted} wrap="truncate-end">{file}</Text>)}
			<Box marginTop={1} flexDirection="column">
				{view.choices.map(choice => <Text key={choice.key} wrap="truncate-end"><Text color={THEME.active} bold>{choice.key.padEnd(6)}</Text>{truncate(choice.text, Math.max(1, contentWidth - 6))}</Text>)}
			</Box>
		</Box>
	);
}

interface FooterMessage {text: string; color: string}

type KillOptionKind = 'kill' | 'delete' | 'delete-branch' | 'cancel';
interface KillOption {kind: KillOptionKind; label: string}

function structuralBlockers(inspection: SessionCleanupInspection | undefined): string[] {
	return inspection?.structuralBlockers ?? [];
}

// Structural blockers (main/current/shared worktrees, branch changed/protected)
// cannot be overridden, so the blocked deletion is not offered at all. Branch
// deletion has its own inspection whose blockers can exceed the worktree's.
function killOptions(canDelete: boolean, canDeleteBranch: boolean, worktreeInspection: SessionCleanupInspection | undefined, branchInspection: SessionCleanupInspection | undefined): KillOption[] {
	const cancel: KillOption = {kind: 'cancel', label: 'Cancel'};
	if (!canDelete) return [{kind: 'kill', label: 'Kill session'}, cancel];
	const keep: KillOption = {kind: 'kill', label: 'Kill only, keep worktree (restartable)'};
	if (structuralBlockers(worktreeInspection).length > 0) return [keep, cancel];
	const offerBranch = canDeleteBranch && structuralBlockers(branchInspection).length === 0;
	return [
		keep,
		{kind: 'delete', label: 'Kill and delete worktree (not restartable)'},
		...(offerBranch ? [{kind: 'delete-branch', label: 'Kill, delete worktree and branch (not restartable)'} satisfies KillOption] : []),
		cancel,
	];
}

function cleanupSummary(inspection: SessionCleanupInspection | undefined): string {
	if (!inspection) return 'Checking cleanup safety…';
	const blockers = structuralBlockers(inspection);
	if (blockers.length > 0) return 'Worktree deletion is blocked:';
	return inspection.safe ? 'Local cleanup checks passed' : inspection.reasons.join('; ');
}

function KillConfirmPane({session, sessions, options, selectedIndex, force, width, inspection}: {session?: SessionRecord; sessions: SessionRecord[]; options: KillOption[]; selectedIndex: number; force: boolean; width: number; inspection?: SessionCleanupInspection}) {
	const contentWidth = Math.max(1, width - 4);
	return (
		<Box flexDirection="column" width={width} borderStyle="round" borderColor={THEME.borderDanger} paddingX={1}>
			<Text color={THEME.warn}>{truncate(cleanupSummary(inspection), contentWidth)}</Text>
			{structuralBlockers(inspection).map((blocker, index) => (
				<Text key={`blocker-${index}`} color={THEME.warn}>{truncate(`  ${blocker}`, contentWidth)}</Text>
			))}
			<Text color={THEME.error} bold>{force ? 'Force kill' : 'Kill'} {session ? `"${displaySessionTitle(session, sessions)}"` : 'session'}?</Text>
			{session?.worktree?.path ? (
				<Text color={THEME.muted}>{truncate(compactPath(session.worktree.path, contentWidth), contentWidth)}</Text>
			) : null}
			<Box marginTop={1} flexDirection="column">
				{options.map((option, index) => <SelectableRow key={option.kind} selected={index === selectedIndex} text={option.label} width={contentWidth} selectedColor={option.kind === 'cancel' ? THEME.muted : THEME.error} wrap />)}
			</Box>
			<Box marginTop={1}>
				<Text color={THEME.muted}>enter choose · esc cancel · j/k move</Text>
			</Box>
		</Box>
	);
}

interface ReviewLabels {enter?: string | HintPart; skip?: string | HintPart}
/** What creating a new worktree would take from an untrusted repository config (empty: nothing to review). */
function untrustedCreationParts(project: ProjectInfo): string[] {
	const {setupCommand, worktree = {}} = project.config;
	const fields = (['location', 'branchFrom', 'branchName', 'symlink', 'files'] as const).filter(field => worktree[field] !== undefined);
	return [
		...setupCommand ? [`setupCommand (${setupCommand})`] : [],
		...project.creationHook ? [`creation hook (${project.creationHook.file})`] : [],
		...fields.length ? [`worktree settings (${fields.join(', ')})`] : [],
	];
}

function ActionPickerPane({project, running, selectedIndex, width, height}: {project?: ProjectInfo; running?: ActionRecord; selectedIndex: number; width: number; height: number}) {
	const actions = projectActions(project);
	const selected = actions[selectedIndex];
	const untrusted = actions.some(action => action.needsTrust);
	const runningName = running?.live ? running.name ?? 'action' : undefined;
	return <MenuPane
		title="Actions"
		subtitle={[untrusted ? {text: '"needs trust": from deckhand.json, reviewed first', color: THEME.warn} : {text: 'Global defaults and repository actions'}]}
		items={actions.map(action => ({key: action.name, label: action.name, description: action.command, ...action.needsTrust ? {status: {text: '· needs trust', color: THEME.warn}} : {}}))}
		selected={selectedIndex}
		empty="No actions configured (C → Settings › Actions adds one)"
		details={selected ? {title: 'Selected command', lines: [
			{text: selected.command, color: THEME.active, nowrap: true},
			...selected.needsTrust ? [{text: `From this repo's deckhand.json, not trusted yet: Enter shows it for review before anything runs${selected.fallback ? ` (s runs the global ${selected.name} instead: ${selected.fallback})` : ''}.`, color: THEME.warn}] : [],
			runningName ? {text: `${runningName} is still running in this worktree: x stops it, then run another.`, color: THEME.warn}
				: {text: 'Runs on the Terminal tab beside your shell (v switches), one action at a time per worktree; Dev keeps running.'},
		]} : undefined}
		hint={[{text: 'j/k choose', drop: 1}, 'enter run', ...runningName ? [`x stop ${runningName}`] : [], 'esc cancel']}
		width={width}
		height={height}
	/>;
}

/** The merge confirmation (m) of one session: its preview, the commit-first toggle and a failed attempt's output. */
interface MergeFlow {sessionId: string; preview?: MergePreview; previewError?: string; commitFirst: boolean; error?: string}

function hasMergedMarker(session?: SessionRecord): boolean {
	return Boolean(session?.worktree?.mergedAt || session?.mergedAt);
}

function mergedTargetBranch(session: SessionRecord): string | undefined {
	return session.worktree?.mergeTargetBranch ?? session.mergeTargetBranch;
}

function footerHint(mode: Mode, activeTab: RightPaneTab, width: number, session?: SessionRecord, scrollSensitivity = DEFAULT_SCROLL_SENSITIVITY, attachReady = true, notesHint?: string, terminalAction?: {switchHint: string; finished: boolean}): string {
	switch (mode) {
		// Every other mode replaces the right pane with a screen that shows its own (single) hint line.
		case 'help': case 'settings': case 'agents':
		case 'edit-project': case 'discard-project': case 'workspace-info': case 'review-project': case 'confirm-loss':
		case 'pick-action': case 'pick-program': case 'enter-name': case 'pick-worktree': case 'confirm-kill': case 'confirm-merge': case 'merge-conflicts':
		case 'tasks': case 'confirm-remove':
			return '';
		case 'preview-focus': {
			const method = session?.program === 'claude' ? 'mouse wheel' : 'scrollback';
			return `preview focus (${method}) • wheel scroll ×${formatScrollSensitivity(scrollSensitivity)} • [/] adjust • j/k scroll • g/G top/bottom • esc/v return`;
		}
		case 'changes-focus': return 'changes • esc/v back • j/k select • space stage/unstage • a/A stage/unstage all • enter/e open in editor • J/K scroll diff • o lazygit';
		case 'notes-focus': return notesHint ?? 'notes edit • esc done';
		case 'search': return 'type to search • enter keep search • esc clear';
		case 'browse': {
			// Keep this short; everything else is listed in ? help.
			const running = session?.status === 'running';
			// Terminal, Git and Dev belong to the session's workspace, so they are usable whether or not the agent runs.
			const hasWorkspace = Boolean(session && workspaceKey(session));
			const attach = activeTab === 'dev' ? (attachReady ? 'o attach' : undefined)
				: activeTab === 'git' ? (hasWorkspace ? (attachReady ? 'v changes • o lazygit' : 'loading…') : undefined)
				// A finished action shown on the Terminal tab cannot be attached (not "loading": it will not become ready).
				: activeTab === 'terminal' ? (hasWorkspace && !terminalAction?.finished ? (attachReady ? 'o attach' : 'loading…') : undefined)
					: running && activeTab === 'preview' ? (attachReady ? 'o attach' : 'loading…') : undefined;
			const pane = activeTab === 'notes' ? (session ? 'o edit notes • E open in editor' : undefined)
				: activeTab === 'dev' && session && workspaceKey(session) ? 'd start/stop'
				: activeTab === 'terminal' && terminalAction ? terminalAction.switchHint
					: activeTab === 'preview' && running ? 'v scroll' : undefined;
			const lifecycle = session?.status === 'exited'
				? (session.worktree?.deletedAt ? 'backspace remove' : 's resume • S fresh')
				: running ? 'x kill' : session?.status === 'starting' ? 'x cancel start' : undefined;
			// Archiving is offered where it is the likely next step (finished sessions), and undoing it where it applies.
			const archive: HintPart | undefined = session?.archivedAt ? {text: 'A unarchive', drop: 2}
				: session?.status === 'exited' ? {text: 'A archive', drop: 2} : undefined;
			// Higher drop numbers go first when the line is too narrow; ? help always stays.
			const parts: Array<string | HintPart | undefined> = [attach, pane, lifecycle, archive, '? help', {text: 'n new', drop: 1}, {text: 'C settings', drop: 1}, {text: 'i info', drop: 3}, {text: 'e actions', drop: 3}, {text: '/ search', drop: 2}, {text: 'f filter', drop: 2}, {text: 'q quit', drop: 1}];
			return fitHint(parts.filter((part): part is string | HintPart => Boolean(part)), width, ' • ');
		}
	}
}

export function App({repoRoot, cwd, initialSelectedId, initialActiveTab, initialTerminalView, onTerminalViewChange, initialSidebarWidth, initialSessionTabs, initialCollapsedSessionIds, initialHiddenExitedSessionIds, initialSessionFilter, initialSessionQuery, onSessionVisibilityChange, onSelectedIdChange, onActiveTabChange, onSessionTabChange, onSidebarWidthChange, onCollapsedSessionIdsChange, onHiddenExitedSessionIdsChange}: AppProps) {
	const {exit} = useApp();
	const [mode, setMode] = useState<Mode>('browse');
	const [sessionFilter, setSessionFilter] = useState<SessionFilter>(initialSessionFilter ?? 'active');
	const [sessionQuery, setSessionQuery] = useState(initialSessionQuery ?? '');
	// The inline repository-config review: what it shows, the cwd it was resolved for, and the action it gates
	// (resumed after Enter trusts or s skips; none for an explicit T review).
	// back: where closing the review returns (Settings' T); otherwise browse.
	// skip: what s does (default: resume with the override ignored; null: s cancels). labels/purpose describe the gated step.
	const [review, setReview] = useState<{project: ProjectInfo; cwd: string; resume?: (project: ProjectInfo) => void; skip?: ((project: ProjectInfo) => void) | null; labels?: ReviewLabels; purpose?: string; back?: () => void}>();
	// The project (and session) the action picker was opened for.
	const [actionProject, setActionProject] = useState<{project: ProjectInfo; cwd: string; sessionId?: string}>();
	// Async results are keyed by session (and a request counter) so a late
	// response for one session never renders in, or authorizes, another.
	const [workspaceInfo, setWorkspaceInfo] = useState<{sessionId: string; summary?: WorkspaceSummary; prLoading?: boolean; confirmPr?: boolean; creatingPr?: boolean}>();
	const workspaceRequestRef = useRef(0);
	// Deleting only the worktree keeps the branch (and its commits), so it is
	// inspected separately from deleting the worktree and branch.
	const [cleanupCheck, setCleanupCheck] = useState<{sessionId: string; worktree?: SessionCleanupInspection; branch?: SessionCleanupInspection}>();
	const cleanupRequestRef = useRef(0);
	const [detailsScroll, setDetailsScroll] = useState(0);
	const [confirmationDraft, setConfirmationDraft] = useState('');
	const [actionIndex, setActionIndex] = useState(0);
	const [pendingDeleteBranch, setPendingDeleteBranch] = useState(false);
	const [handoffFromId, setHandoffFromId] = useState<string>();
	const [sessions, setSessions] = useState<SessionRecord[]>([]);
	const [sessionsLoaded, setSessionsLoaded] = useState(false);
	const [collapsedSessionIds, setCollapsedSessionIds] = useState<Set<string>>(() => new Set(initialCollapsedSessionIds ?? []));
	const [hiddenExitedSessionIds, setHiddenExitedSessionIds] = useState<Set<string>>(() => new Set(initialHiddenExitedSessionIds ?? []));
	const [selectedId, setSelectedId] = useState<string | undefined>(initialSelectedId);
	const [programIndex, setProgramIndex] = useState(0);
	const [draftName, setDraftName] = useState('');
	const [createParentId, setCreateParentId] = useState<string | undefined>();
	const [createSubSessionKind, setCreateSubSessionKind] = useState<SubSessionKind | undefined>();
	const [worktreeMode, setWorktreeMode] = useState<WorktreeMode>('none');
	const [worktrees, setWorktrees] = useState<WorktreeInfoRecord[]>([]);
	const [worktreeQuery, setWorktreeQuery] = useState('');
	const [worktreeIndex, setWorktreeIndex] = useState(0);
	const [killConfirmIndex, setKillConfirmIndex] = useState(0);
	const [killConfirmForce, setKillConfirmForce] = useState(false);
	const [mergeConfirmIndex, setMergeConfirmIndex] = useState(0);
	const [mergeFlow, setMergeFlow] = useState<MergeFlow>();
	const mergeRequestRef = useRef(0);
	// A conflicted merge's result, until it is kept or aborted.
	const [mergeConflict, setMergeConflict] = useState<{sessionId: string; result: WorktreeMergeResult}>();
	const sessionTabsRef = useRef<Record<string, RightPaneTab>>({
		...(initialSessionTabs ?? {}),
		...(initialSelectedId && initialActiveTab ? {[initialSelectedId]: initialActiveTab} : {}),
	});
	const [activeTab, setActiveTab] = useState<RightPaneTab>(initialSelectedId ? sessionTabsRef.current[initialSelectedId] ?? 'preview' : initialActiveTab ?? 'preview');
	const [previewScrollOffset, setPreviewScrollOffset] = useState(0);
	const [previewScrollSensitivity, setPreviewScrollSensitivity] = useState(DEFAULT_SCROLL_SENSITIVITY);
	const previewWheelAccumulatorRef = useRef(0);
	const [preview, setPreview] = useState<PreviewRecord>(EMPTY_PREVIEW);
	const [terminal, setTerminal] = useState<TerminalRecord>(EMPTY_TERMINAL);
	const [changes, setChanges] = useState<ChangesRecord>(EMPTY_CHANGES);
	// Whether this UI asked the daemon to watch (poll) a workspace's changes, so leaving the Git tab stops it.
	const changesWatchedRef = useRef(false);
	const [dev, setDev] = useState<DevRecord>(EMPTY_DEV);
	// The workspace's last action and whether the Terminal tab shows it instead of the shell.
	const [action, setAction] = useState<ActionRecord>(EMPTY_ACTION);
	const [terminalView, setTerminalView] = useState<TerminalView>(initialTerminalView ?? 'shell');
	useEffect(() => { onTerminalViewChange?.(terminalView); }, [onTerminalViewChange, terminalView]);
	// Every agent's installed/latest version (agent-versions, then agent-versions-updated): the ↑ marker, the header hint, U.
	const [agentVersions, setAgentVersions] = useState<AgentVersions>();
	// The repository's task list (watch-tasks, then tasks-updated), parsed once per change.
	const [tasksDoc, setTasksDoc] = useState<TasksDoc>();
	const tasks = useMemo(() => parseTasks(tasksDoc?.text ?? ''), [tasksDoc?.text]);
	// A session started from a task (n on the board), and the new worktree's base branch (↑↓ in the form; 0: the setting).
	const [taskStart, setTaskStart] = useState<{id: string; title: string}>();
	const [branchList, setBranchList] = useState<BranchList>();
	const [baseIndex, setBaseIndex] = useState(0);
	// Removing a session whose notes still have open items asks first (m moves them to Tasks).
	const [removeItems, setRemoveItems] = useState<string[]>([]);
	const [error, setError] = useState<string | undefined>();
	const [statusMessage, setStatusMessage] = useState<string | undefined>();
	const [numericSelection, setNumericSelection] = useState('');
	const [busy, setBusy] = useState(false);
	const [client, setClient] = useState<LiveClient | undefined>();
	const [connectionEpoch, setConnectionEpoch] = useState(0);
	const [terminalSize, setTerminalSize] = useState<TerminalSize>(getTerminalSize());
	const [sidebarWidthOverride, setSidebarWidthOverride] = useState<number | undefined>(initialSidebarWidth);
	const [spinnerIndex, setSpinnerIndex] = useState(0);
	const selectedIdRef = useRef<string | undefined>(selectedId);
	const sessionsRef = useRef<SessionRecord[]>(sessions);
	// Search and nondefault filters reveal matches regardless of collapse state.
	const collapseApplied = !sessionQuery && sessionFilter === 'active';
	const visibleSessions = useMemo(
		() => {
			const filtered = filterSessionList(sessions, sessionFilter, sessionQuery, session => linkedTask(tasks, session)?.title);
			return collapseApplied ? filterCollapsedSessions(filtered, collapsedSessionIds, hiddenExitedSessionIds) : filtered;
		},
		[collapseApplied, collapsedSessionIds, hiddenExitedSessionIds, sessions, sessionFilter, sessionQuery, tasks],
	);

	useEffect(() => { onSessionVisibilityChange?.(sessionFilter, sessionQuery); }, [onSessionVisibilityChange, sessionFilter, sessionQuery]);

	useEffect(() => {
		if (!statusMessage) {
			return;
		}
		const currentMessage = statusMessage;
		const timer = setTimeout(() => {
			setStatusMessage(message => (message === currentMessage ? undefined : message));
		}, STATUS_MESSAGE_AUTO_HIDE_MS);
		return () => clearTimeout(timer);
	}, [statusMessage]);

	useEffect(() => {
		if (!error) {
			return;
		}
		const currentError = error;
		const timer = setTimeout(() => {
			setError(message => (message === currentError ? undefined : message));
		}, ERROR_MESSAGE_AUTO_HIDE_MS);
		return () => clearTimeout(timer);
	}, [error]);

	useEffect(() => {
		selectedIdRef.current = selectedId;
		onSelectedIdChange?.(selectedId);
		setPreviewScrollOffset(0);
		if (selectedId) {
			const nextTab = sessionTabsRef.current[selectedId] ?? 'preview';
			setActiveTab(current => (current === nextTab ? current : nextTab));
		}
	}, [onSelectedIdChange, selectedId]);

	useEffect(() => {
		sessionsRef.current = sessions;
	}, [sessions]);

	useEffect(() => {
		onCollapsedSessionIdsChange?.([...collapsedSessionIds]);
	}, [collapsedSessionIds, onCollapsedSessionIdsChange]);

	useEffect(() => {
		onHiddenExitedSessionIdsChange?.([...hiddenExitedSessionIds]);
	}, [hiddenExitedSessionIds, onHiddenExitedSessionIdsChange]);

	useEffect(() => {
		void loadAppConfig()
			.then(config => setPreviewScrollSensitivity(normalizeScrollSensitivity(config.attach_scroll_sensitivity)))
			.catch(() => setPreviewScrollSensitivity(DEFAULT_SCROLL_SENSITIVITY));
	}, []);

	useEffect(() => {
		const sessionId = selectedIdRef.current;
		if (sessionId) {
			sessionTabsRef.current[sessionId] = activeTab;
			onSessionTabChange?.(sessionId, activeTab);
		}
		onActiveTabChange?.(activeTab);
		if (activeTab !== 'preview') {
			setPreviewScrollOffset(0);
			setMode(current => (current === 'preview-focus' ? 'browse' : current));
		}
		if (activeTab !== 'notes') {
			setMode(current => (current === 'notes-focus' ? 'browse' : current));
		}
		if (activeTab !== 'git') {
			setMode(current => (current === 'changes-focus' ? 'browse' : current));
		}
	}, [activeTab, onActiveTabChange, onSessionTabChange]);

	// Notes focus: bracketed paste, so a multi-line paste (Tab and Enter included) arrives as text.
	useEffect(() => {
		if (mode !== 'notes-focus' || !process.stdout.isTTY) return;
		process.stdout.write('\u001B[?2004h');
		return () => { process.stdout.write('\u001B[?2004l'); };
	}, [mode]);

	useEffect(() => {
		if (mode !== 'preview-focus') {
			return;
		}
		process.stdout.write('\u001B[?1000h\u001B[?1002h\u001B[?1003h\u001B[?1006h\u001B[?1015h\u001B[?1016h');
		return () => {
			process.stdout.write('\u001B[?1016l\u001B[?1015l\u001B[?1006l\u001B[?1003l\u001B[?1002l\u001B[?1000l');
		};
	}, [mode]);


	useEffect(() => {
		const onResize = () => {
			if (process.stdout.isTTY) {
				process.stdout.write('\x1b[2J\x1b[H');
			}
			setTerminalSize(getTerminalSize());
		};
		process.stdout.on('resize', onResize);
		return () => {
			process.stdout.off('resize', onResize);
		};
	}, []);

	const shouldAnimateStatus = sessions.some(
		session => session.status === 'starting' || (session.status === 'running' && (session.attention?.state === 'working' || ((!session.attention || session.attention.state === 'unknown') && session.agentStatus === 'active'))),
	);

	useEffect(() => {
		if (!shouldAnimateStatus) {
			return;
		}
		const timer = setInterval(() => {
			setSpinnerIndex(index => (index + 1) % SPINNER_FRAMES.length);
		}, 120);
		return () => {
			clearInterval(timer);
		};
	}, [shouldAnimateStatus]);

	useEffect(() => {
		let cancelled = false;
		let reconnectScheduled = false;
		let reconnectTimer: NodeJS.Timeout | undefined;
		let currentClient: LiveClient | undefined;

		const scheduleReconnect = () => {
			if (cancelled || reconnectScheduled) {
				return;
			}
			reconnectScheduled = true;
			reconnectTimer = setTimeout(() => {
				setConnectionEpoch(value => value + 1);
			}, 500);
		};

		void (async () => {
			try {
				const nextClient = await createLiveClient({
					onSessionUpdated: session => {
						if (!sessionMatchesScope(session, repoRoot)) {
							return;
						}
						setSessions(current => upsertSession(current, session));
					},
					onSessionRemoved: sessionId => {
						setSessions(current => current.filter(session => session.id !== sessionId));
						if (selectedIdRef.current === sessionId) {
							setPreview(EMPTY_PREVIEW);
							setTerminal(EMPTY_TERMINAL);
							setChanges(EMPTY_CHANGES);
							setDev(EMPTY_DEV);
							setAction(EMPTY_ACTION);
						}
					},
					onPreviewUpdated: nextPreview => {
						if (nextPreview.sessionId && nextPreview.sessionId !== selectedIdRef.current) {
							return;
						}
						if (!nextPreview.sessionId && selectedIdRef.current) {
							return;
						}
						setPreview(nextPreview);
						if (typeof nextPreview.maxScrollOffset === 'number') {
							setPreviewScrollOffset(offset => Math.min(offset, nextPreview.maxScrollOffset ?? 0));
						}
					},
					onTerminalUpdated: nextTerminal => {
						if (nextTerminal.sessionId && nextTerminal.sessionId !== selectedIdRef.current) {
							return;
						}
						if (!nextTerminal.sessionId && selectedIdRef.current) {
							return;
						}
						setTerminal(nextTerminal);
					},
					onChangesUpdated: nextChanges => {
						if (nextChanges.sessionId !== selectedIdRef.current) {
							return;
						}
						setChanges(nextChanges);
					},
					onDevUpdated: nextDev => {
						if (nextDev.sessionId && nextDev.sessionId !== selectedIdRef.current) {
							return;
						}
						if (!nextDev.sessionId && selectedIdRef.current) {
							return;
						}
						setDev(nextDev);
					},
					onActionUpdated: nextAction => {
						if (nextAction.sessionId !== selectedIdRef.current) {
							return;
						}
						setAction(nextAction);
					},
					onAgentVersionsUpdated: setAgentVersions,
					onTasksUpdated: next => setTasksDoc(current => (!current || current.key === next.key ? next : current)),
					onError: nextError => {
						setError(nextError.message);
					},
					onClose: () => {
						setClient(undefined);
						scheduleReconnect();
					},
				});
				if (cancelled) {
					nextClient.close();
					return;
				}
				currentClient = nextClient;
				setClient(nextClient);
				const initialSessions = await nextClient.subscribe(repoRoot);
				if (cancelled) {
					nextClient.close();
					return;
				}
				setSessions(sortSessions(initialSessions));
				setSessionsLoaded(true);
				setError(undefined);
				// The daemon's cached versions; it looks the latest releases up in the background.
				void nextClient.agentVersions().then(versions => { if (!cancelled) setAgentVersions(versions); }).catch(() => {});
				void nextClient.watchTasks(repoRoot).then(doc => { if (!cancelled) setTasksDoc(doc); }).catch(() => {});
			} catch (nextError) {
				if (!cancelled) {
					setError(errorMessage(nextError));
					scheduleReconnect();
				}
			}
		})();

		return () => {
			cancelled = true;
			if (reconnectTimer) {
				clearTimeout(reconnectTimer);
			}
			if (currentClient) {
				currentClient.close();
			}
			setClient(current => (current === currentClient ? undefined : current));
		};
	}, [connectionEpoch, repoRoot]);

	useEffect(() => {
		// Sessions arrive asynchronously; until then keep the persisted (or
		// pre-attach) selection instead of resetting it to the first session.
		if (!sessionsLoaded) {
			return;
		}
		setSelectedId(currentId => {
			if (currentId && visibleSessions.some(session => session.id === currentId)) {
				return currentId;
			}
			if (visibleSessions.length === 0) {
				// A filter/search that temporarily matches nothing keeps the selection.
				return currentId && sessions.some(session => session.id === currentId) ? currentId : undefined;
			}
			return visibleSessions[0]?.id;
		});
	}, [sessions, sessionsLoaded, visibleSessions]);

	const selectedIndex = useMemo(() => {
		if (!selectedId) {
			return 0;
		}
		const index = visibleSessions.findIndex(session => session.id === selectedId);
		return index >= 0 ? index : 0;
	}, [selectedId, visibleSessions]);

	// Only a visible session is actionable; a hidden selection is kept for later.
	const selectedSession = selectedId ? visibleSessions.find(session => session.id === selectedId) : undefined;

	// The sidebar details show the selected session's age. Every render reads the clock; while nothing else
	// re-renders (no spinner), one render exactly when the shown age would change keeps it current.
	const [clockTick, setClockTick] = useState(0);
	const selectedSince = selectedSession ? statusSince(selectedSession) : undefined;
	const selectedDoneAt = selectedSession?.doneAt;
	useEffect(() => {
		// The state's age and the `done 2d ago` marker: re-render when the first of them changes.
		const ages = [selectedSince, selectedDoneAt].map(at => (at ? Date.parse(at) : NaN)).filter(Number.isFinite);
		if (shouldAnimateStatus || !ages.length) return;
		const timer = setTimeout(() => setClockTick(tick => tick + 1), Math.min(...ages.map(at => msUntilAgeChanges(Date.now() - at))));
		return () => clearTimeout(timer);
	}, [clockTick, selectedDoneAt, selectedSince, shouldAnimateStatus]);

	const currentWorkspaceInfo = workspaceInfo && workspaceInfo.sessionId === selectedSession?.id ? workspaceInfo : undefined;
	const currentCleanupCheck = cleanupCheck && cleanupCheck.sessionId === selectedSession?.id ? cleanupCheck : undefined;
	const cleanupInspectionFor = (deleteBranch: boolean) => (deleteBranch ? currentCleanupCheck?.branch : currentCleanupCheck?.worktree);
	const showingAction = activeTab === 'terminal' && terminalView === 'action' && hasAction(selectedSession, action);
	const activeAttachTarget: AttachTarget = showingAction ? 'action' : activeTab === 'terminal' ? 'terminal' : activeTab === 'git' ? 'git' : activeTab === 'dev' ? 'dev' : 'agent';
	// The workspace's shared panes (Terminal, Git, Dev) may run while this session's agent does not.
	const selectedWorkspace = selectedSession ? workspaceKey(selectedSession) : undefined;
	const activePaneReadyForAttach = Boolean(
		selectedSession && (
			(activeAttachTarget === 'agent' && selectedSession.status === 'running') ||
			(activeAttachTarget === 'terminal' && terminal.sessionId === selectedSession.id && terminal.live) ||
			// lazygit starts on attach; the Changes record confirms the daemon sees the workspace.
			(activeAttachTarget === 'git' && changes.sessionId === selectedSession.id && Boolean(changes.workspace)) ||
			(activeAttachTarget === 'dev' && dev.sessionId === selectedSession.id && dev.live) ||
			(activeAttachTarget === 'action' && action.live)
		),
	);
	// Workspace panes are watched again when the selected session gains (or loses) its workspace, e.g. once its
	// worktree is prepared, and on lifecycle changes (the daemon reports a session still preparing as having none).
	const selectedPaneScope = selectedSession ? `${selectedWorkspace ?? ''}\0${selectedSession.status}` : undefined;

	const filteredWorktrees = useMemo(() => {
		const terms = worktreeQuery
			.toLowerCase()
			.trim()
			.split(/\s+/)
			.filter(Boolean);
		if (terms.length === 0) {
			return worktrees;
		}
		return worktrees.filter(worktree => {
			const haystack = `${worktree.branch} ${worktree.path}`.toLowerCase();
			return terms.every(term => haystack.includes(term));
		});
	}, [worktreeQuery, worktrees]);
	const selectedCanDeleteWorktree = Boolean(
		selectedSession?.worktree?.path &&
		selectedSession.worktree.mode !== 'none' &&
		!selectedSession.worktree.deletedAt &&
		!selectedSession.worktree.isMain &&
		(!selectedSession.launchWorktreeRoot || selectedSession.worktree.path !== selectedSession.launchWorktreeRoot) &&
		!sessions.some(
			session =>
				session.id !== selectedSession.id &&
				session.status !== 'exited' &&
				session.worktree?.path === selectedSession.worktree?.path,
		),
	);
	const selectedCanDeleteBranch = Boolean(
		selectedCanDeleteWorktree &&
		selectedSession?.worktree?.branch &&
		selectedSession.worktree.branch !== 'main' &&
		selectedSession.worktree.branch !== 'master',
	);

	useEffect(() => {
		setWorktreeIndex(index => Math.min(index, Math.max(0, filteredWorktrees.length - 1)));
	}, [filteredWorktrees.length]);

	useEffect(() => {
		if (!selectedSession) {
			setPreview(EMPTY_PREVIEW);
			setTerminal(EMPTY_TERMINAL);
			setChanges(EMPTY_CHANGES);
			setDev(EMPTY_DEV);
			setAction(EMPTY_ACTION);
			return;
		}
		setTerminal(current => (current.sessionId === selectedSession.id ? current : EMPTY_TERMINAL));
		setChanges(current => (current.sessionId === selectedSession.id ? current : EMPTY_CHANGES));
		setDev(current => (current.sessionId === selectedSession.id ? current : EMPTY_DEV));
		setAction(current => (current.sessionId === selectedSession.id ? current : EMPTY_ACTION));
		setPreview(current => {
			const sameSession = current.sessionId === selectedSession.id;
			const content =
				selectedSession.status === 'exited'
					? selectedSession.lastPreview ?? current.content
					: sameSession
						? current.content
						: '';
			return {
				sessionId: selectedSession.id,
				content,
				live: sameSession ? current.live : false,
				status: selectedSession.status,
				agentStatus: selectedSession.agentStatus,
			};
		});
	}, [selectedSession]);

	const spinnerFrame = SPINNER_FRAMES[spinnerIndex] ?? SPINNER_FRAMES[0]!;

	const layout = useMemo(() => {
		const totalWidth = terminalSize.cols;
		const totalHeight = terminalSize.rows;
		const leftWidth = clampSidebarWidth(sidebarWidthOverride ?? sidebarWidth(totalWidth), totalWidth);
		const separatorWidth = 1;
		const rightWidth = Math.max(20, totalWidth - leftWidth - separatorWidth);
		const contentHeight = Math.max(8, totalHeight - HEADER_ROWS - FOOTER_ROWS - SPARE_ROWS);
		// Right pane wrapper consumes 4 cols (border 2 + paddingX 2) and 4 rows
		// (border 2 + tabbar 1 + spacer 1) before the sub-pane content begins.
		const paneInnerWidth = Math.max(10, rightWidth - 4);
		const paneInnerHeight = Math.max(4, contentHeight - 4);
		const previewRows = Math.max(1, paneInnerHeight - 1);
		return {
			sidebarWidth: leftWidth,
			previewWidth: rightWidth,
			contentHeight,
			paneInnerWidth,
			paneInnerHeight,
			previewCols: paneInnerWidth,
			previewRows,
		};
	}, [sidebarWidthOverride, terminalSize.cols, terminalSize.rows]);

	const confirmNumericSelection = useCallback((value: string) => {
		if (!value) {
			return;
		}
		const targetIndex = Number.parseInt(value, 10) - 1;
		setNumericSelection('');
		if (!Number.isInteger(targetIndex) || targetIndex < 0 || targetIndex >= visibleSessions.length) {
			setError(`no visible session ${value}`);
			return;
		}
		setError(undefined);
		setSelectedId(visibleSessions[targetIndex]?.id);
	}, [visibleSessions]);

	useEffect(() => {
		if (!numericSelection) {
			return;
		}
		const timer = setTimeout(() => {
			confirmNumericSelection(numericSelection);
		}, 300);
		return () => {
			clearTimeout(timer);
		};
	}, [confirmNumericSelection, numericSelection]);

	const moveSelection = useCallback(
		(delta: number) => {
			if (visibleSessions.length === 0) {
				return;
			}
			const nextIndex = (selectedIndex + delta + visibleSessions.length) % visibleSessions.length;
			setSelectedId(visibleSessions[nextIndex]?.id);
		},
		[selectedIndex, visibleSessions],
	);

	const toggleSelectedCollapse = useCallback(() => {
		if (!selectedSession || !sessionHasChildren(selectedSession.id, sessions)) {
			setError('selected session has no sub-sessions');
			return;
		}

		const descendants = sessionDescendants(selectedSession.id, sessions);
		const descendantIds = new Set(descendants.map(session => session.id));
		const exitedDescendants = descendants.filter(session => session.status === 'exited');
		const exitedDescendantIds = new Set(exitedDescendants.map(session => session.id));
		const selectedCollapsed = collapsedSessionIds.has(selectedSession.id);
		const exitedAlreadyHidden = exitedDescendants.length > 0 && exitedDescendants.every(session => hiddenExitedSessionIds.has(session.id));

		setError(undefined);
		if (selectedCollapsed) {
			setCollapsedSessionIds(current => {
				const next = new Set(current);
				next.delete(selectedSession.id);
				for (const id of descendantIds) next.delete(id);
				return next;
			});
			setHiddenExitedSessionIds(current => {
				const next = new Set(current);
				for (const id of exitedDescendantIds) next.delete(id);
				return next;
			});
			setStatusMessage('expanded all sub-sessions');
			return;
		}

		if (exitedDescendants.length > 0 && !exitedAlreadyHidden) {
			setHiddenExitedSessionIds(current => {
				const next = new Set(current);
				for (const id of exitedDescendantIds) next.add(id);
				return next;
			});
			setStatusMessage(`collapsed ${exitedDescendants.length} exited sub-session${exitedDescendants.length === 1 ? '' : 's'}`);
			return;
		}

		setCollapsedSessionIds(current => {
			const next = new Set(current);
			next.add(selectedSession.id);
			return next;
		});
		setStatusMessage('collapsed all sub-sessions');
	}, [collapsedSessionIds, hiddenExitedSessionIds, selectedSession, sessions]);

	const reorderSelected = useCallback(async (direction: 'up' | 'down') => {
		if (!client || !selectedSession) {
			return;
		}
		setBusy(true);
		setError(undefined);
		try {
			const reordered = await client.reorderSession(selectedSession.id, direction);
			setSessions(sortSessions(reordered));
		} catch (nextError) {
			setError(errorMessage(nextError));
		} finally {
			setBusy(false);
		}
	}, [client, selectedSession]);

	const resizeSidebar = useCallback(
		(delta: number) => {
			setSidebarWidthOverride(current => {
				const baseWidth = current ?? sidebarWidth(terminalSize.cols);
				const nextWidth = clampSidebarWidth(baseWidth + delta, terminalSize.cols);
				onSidebarWidthChange?.(nextWidth);
				return nextWidth;
			});
		},
		[onSidebarWidthChange, terminalSize.cols],
	);

	const adjustScrollSensitivity = useCallback((delta: number) => {
		setPreviewScrollSensitivity(current => {
			const next = normalizeScrollSensitivity(Math.round((current + delta) * 100) / 100);
			previewWheelAccumulatorRef.current = 0;
			setStatusMessage(`Scroll multiplier ${formatScrollSensitivity(next)} (saved)`);
			void updateAppConfig({attach_scroll_sensitivity: next}).catch(nextError => {
				setError(errorMessage(nextError));
			});
			return next;
		});
	}, []);

	useEffect(() => {
		if (mode !== 'preview-focus') {
			return;
		}
		const onData = (chunk: Buffer) => {
			const wheel = parseMouseWheel(chunk.toString('utf8'));
			if (!wheel) {
				return;
			}
			let scaledCount = 0;
			for (let index = 0; index < wheel.count; index += 1) {
				previewWheelAccumulatorRef.current += previewScrollSensitivity;
				if (previewWheelAccumulatorRef.current >= 1) {
					scaledCount += 1;
					previewWheelAccumulatorRef.current -= 1;
				}
			}
			if (scaledCount === 0) {
				return;
			}
			if (client && selectedSession?.program === 'claude' && selectedSession.status === 'running') {
				client.sendAgentInput(selectedSession.id, mouseWheelSequence(wheel.direction, layout.previewCols, layout.previewRows, scaledCount));
				setPreviewScrollOffset(0);
				return;
			}
			if (wheel.direction === 'up') {
				setPreviewScrollOffset(offset => Math.min((preview.maxScrollOffset ?? offset + scaledCount), offset + scaledCount));
			} else {
				setPreviewScrollOffset(offset => Math.max(0, offset - scaledCount));
			}
		};
		process.stdin.on('data', onData);
		return () => {
			process.stdin.off('data', onData);
		};
	}, [client, layout.previewCols, layout.previewRows, mode, preview.maxScrollOffset, previewScrollSensitivity, selectedSession]);

	const refreshSessions = useCallback(async () => {
		if (!client) {
			throw new Error('still connecting to daemon');
		}
		const latest = await client.subscribe(repoRoot);
		setSessions(sortSessions(latest));
	}, [client, repoRoot]);

	useEffect(() => {
		if (!client) {
			return;
		}
		let cancelled = false;
		void client
			.watchPreview(selectedId, layout.previewCols, layout.previewRows, previewScrollOffset)
			.then(nextPreview => {
				if (cancelled) {
					return;
				}
				if (nextPreview.sessionId && nextPreview.sessionId !== selectedId) {
					return;
				}
				setPreview(nextPreview);
				if (typeof nextPreview.maxScrollOffset === 'number') {
					setPreviewScrollOffset(offset => Math.min(offset, nextPreview.maxScrollOffset ?? 0));
				}
			})
			.catch(nextError => {
				if (!cancelled) {
					setError(errorMessage(nextError));
				}
			});
		return () => {
			cancelled = true;
		};
	}, [client, layout.previewCols, layout.previewRows, previewScrollOffset, selectedId]);

	useEffect(() => {
		if (!client || activeTab !== 'terminal') {
			return;
		}
		let cancelled = false;
		void client
			.watchTerminal(selectedId, layout.previewCols, layout.previewRows)
			.then(nextTerminal => {
				if (cancelled) {
					return;
				}
				if (nextTerminal.sessionId && nextTerminal.sessionId !== selectedId) {
					return;
				}
				setTerminal(nextTerminal);
			})
			.catch(nextError => {
				if (!cancelled) {
					setError(errorMessage(nextError));
				}
			});
		return () => {
			cancelled = true;
		};
	}, [activeTab, client, layout.previewCols, layout.previewRows, selectedId, selectedPaneScope]);

	// The Terminal tab also shows the workspace's last action (its state in the header, its output on v). Watching it
	// never starts one.
	useEffect(() => {
		if (!client || activeTab !== 'terminal') {
			return;
		}
		let cancelled = false;
		void client
			.watchAction(selectedId, layout.previewCols, layout.previewRows)
			.then(nextAction => {
				if (!cancelled && nextAction.sessionId === selectedId) {
					setAction(nextAction);
				}
			})
			.catch(nextError => {
				if (!cancelled) {
					setError(errorMessage(nextError));
				}
			});
		return () => {
			cancelled = true;
		};
	}, [activeTab, client, layout.previewCols, layout.previewRows, selectedId, selectedPaneScope]);

	// The Git tab shows the workspace's Changes (lazygit only on attach). The daemon polls a watched workspace, so
	// leaving the tab stops watching.
	useEffect(() => {
		if (!client) {
			changesWatchedRef.current = false;
			return;
		}
		if (activeTab !== 'git') {
			if (changesWatchedRef.current) {
				changesWatchedRef.current = false;
				void client.watchChanges(undefined).catch(() => {});
			}
			return;
		}
		let cancelled = false;
		changesWatchedRef.current = true;
		void client
			.watchChanges(selectedId)
			.then(nextChanges => {
				if (!cancelled && nextChanges.sessionId === selectedId) {
					setChanges(nextChanges);
				}
			})
			.catch(nextError => {
				if (!cancelled) {
					setError(errorMessage(nextError));
				}
			});
		return () => {
			cancelled = true;
		};
	}, [activeTab, client, selectedId, selectedPaneScope]);

	useEffect(() => {
		if (!client || activeTab !== 'dev') {
			return;
		}
		let cancelled = false;
		void client
			.watchDev(selectedId, layout.previewCols, layout.previewRows)
			.then(nextDev => {
				if (cancelled) {
					return;
				}
				if (nextDev.sessionId && nextDev.sessionId !== selectedId) {
					return;
				}
				setDev(nextDev);
			})
			.catch(nextError => {
				if (!cancelled) {
					setError(errorMessage(nextError));
				}
			});
		return () => {
			cancelled = true;
		};
	}, [activeTab, client, layout.previewCols, layout.previewRows, selectedId, selectedPaneScope]);

	const openSelectedInEditor = useCallback(() => {
		if (!selectedSession) {
			setError('no session selected');
			return;
		}
		const targetPath = selectedSession.worktree?.path ?? selectedSession.cwd;
		const label = openInEditor(targetPath, setError);
		if (label) {
			setStatusMessage(`Opened ${targetPath} in ${label}`);
		}
	}, [selectedSession]);

	const help = useHelp();
	const agents = useAgentsFlow({client, versions: agentVersions, setVersions: setAgentVersions, setMode, setStatusMessage});
	const sidebarVersions = useMemo(() => installedVersions(agentVersions), [agentVersions]);
	const changesFlow = useChangesFlow({
		client, session: selectedSession, changes, focused: mode === 'changes-focus',
		onChanges: next => { if (next.sessionId === selectedIdRef.current) setChanges(next); },
		onExit: () => setMode('browse'),
		onAttach: () => { if (selectedSession) attachTo(selectedSession, 'git'); },
		setBusy, setError, setStatusMessage,
	});
	const notesFlow = useNotesFlow({client, session: selectedSession, sessions, focused: mode === 'notes-focus', onExit: () => setMode('browse'), setError, setStatusMessage, onTasks: setTasksDoc});
	const tasksFlow = useTasksFlow({
		client, repoRoot, doc: tasksDoc, tasks, sessions, spinnerFrame,
		onExit: () => setMode('browse'),
		onStart: task => startFromTask(task),
		onGoTo: sessionId => {
			const target = sessions.find(session => session.id === sessionId);
			if (target?.archivedAt) setSessionFilter('all');
			setSessionQuery('');
			setSelectedId(sessionId);
			setMode('browse');
		},
		onOpenNote: sessionId => {
			sessionTabsRef.current[sessionId] = 'notes';
			setSelectedId(sessionId);
			setActiveTab('notes');
			setMode('browse');
		},
		onDoc: setTasksDoc,
		onSession: session => setSessions(current => upsertSession(current, session)),
		setError, setStatusMessage,
	});
	const attachTo = (session: SessionRecord, target: AttachTarget) => exit({
		kind: 'attach',
		sessionId: session.id,
		target,
		title: displaySessionTitle(session, sessions),
		cwd: session.cwd,
		program: session.program,
	} satisfies UiExitResult);
	const settingsFlow = useSettingsFlow({client, mode, setMode, setBusy, setError, setStatusMessage, onReview: (reviewCwd, back) => reviewThen(reviewCwd, undefined, {back})});

	// Resolves the repository config for `targetCwd` and runs `resume` with it, showing the inline review first when
	// `gate` says the action depends on an untrusted override. If the config cannot be read, the error is shown and
	// `resume` still runs without it (the daemon enforces trust and reports problems) unless `required`.
	// Without `resume` (T) the review is shown on its own.
	// Trust gates running, not choosing: callers review right before something from the repository would run.
	const reviewThen = (targetCwd: string, resume: ((project?: ProjectInfo) => void) | undefined, {gate = (project: ProjectInfo) => project.needsReview, required = false, back, skip, labels, purpose}: {gate?: (project: ProjectInfo) => boolean; required?: boolean; back?: () => void; skip?: ((project: ProjectInfo) => void) | null; labels?: ReviewLabels | ((project: ProjectInfo) => ReviewLabels); purpose?: (project: ProjectInfo) => string} = {}) => {
		if (!client) return;
		setBusy(true); setError(undefined);
		void client.projectInfo(targetCwd).then(project => {
			setBusy(false);
			if (resume && !gate(project)) { resume(project); return; }
			setReview({project, cwd: targetCwd, resume, skip, labels: typeof labels === 'function' ? labels(project) : labels, purpose: purpose?.(project), back}); setDetailsScroll(0); setMode('review-project');
		}, error => {
			setBusy(false); setError(errorMessage(error));
			if (!required) resume?.(undefined);
		});
	};

	// n on a backlog task: the usual new-session form (agent, then name and workspace), filled in from the task, in a
	// new worktree by default; the session it creates is linked to the task and gets it as its first message.
	const startFromTask = (task: Task) => {
		reviewThen(cwd, project => {
			const defaults = project?.effective;
			setHandoffFromId(undefined); setCreateParentId(undefined); setCreateSubSessionKind(undefined);
			setSessionFilter('active'); setSessionQuery('');
			setProgramIndex(Math.max(0, PROGRAMS.findIndex(program => program.key === (defaults?.defaultAgent ?? 'claude'))));
			setDraftName(sessionTitleFromTask(task.title));
			setTaskStart({id: task.id, title: task.title});
			setBranchList(undefined); setBaseIndex(0);
			setWorktreeMode('new');
			setMode('pick-program');
		}, {gate: () => false});
	};

	// Backspace: removing a session deletes its note (and its worktree's, when it is the last there); open items ask first.
	const requestRemove = () => {
		const items = selectedSession?.status === 'exited' ? openItemsRemovedWith(selectedSession, sessions) : [];
		if (!items.length) { void removeSelected(); return; }
		setRemoveItems(items);
		setMode('confirm-remove');
	};

	// Creating a new worktree reviews the untrusted repository parts it would use (setup, creation hook, worktree
	// settings) right before creating; s creates with global settings only, Esc returns to the form.
	const confirmCreate = () => {
		if (!draftName.trim()) { setError('title cannot be empty'); return; }
		const parent = createParentId ? sessions.find(session => session.id === createParentId) : undefined;
		reviewThen(parent?.cwd ?? cwd, () => void submitCreate(), {
			gate: project => project.needsReview && untrustedCreationParts(project).length > 0,
			back: () => setMode('enter-name'), skip: project => void submitCreate(undefined, project.fingerprint),
			labels: {enter: {text: 'enter trust & create', short: 'enter trust'}, skip: {text: "s create without the repo's settings (global only)", short: 's without them'}},
			purpose: project => `About to create a new worktree using this repository's ${untrustedCreationParts(project).join(', ')}.`,
		});
	};

	const quit = useCallback(() => {
		exit({kind: 'quit'} satisfies UiExitResult);
	}, [exit]);

	// Title, text and footer for the scrollable details modes. Built only when
	// one of those modes is active (render and key handling).
	const detailsContent = (): {title: string; text: string; footer: Array<string | HintPart>; scroll: number} | undefined => {
		switch (mode) {
			case 'workspace-info':
				if (currentWorkspaceInfo?.confirmPr && currentWorkspaceInfo.summary) return {title: 'Create pull request', text: createPrConfirmText(currentWorkspaceInfo.summary), footer: ['enter push & open PR form', 'esc cancel'], scroll: detailsScroll};
				return {title: 'Workspace overview', text: workspaceSummaryText(currentWorkspaceInfo?.summary, currentWorkspaceInfo?.prLoading, {creatingPr: currentWorkspaceInfo?.creatingPr, links: selectedSession?.worktree?.links}), footer: [{text: 'P fetch PR status', short: 'P PR status'}, 'b open PR', 'c create PR', 'g git tab', 'esc close'], scroll: detailsScroll};
			case 'review-project':
				return {
					title: 'Review repository configuration',
					text: review ? trustReviewText(review.project, review.purpose) : 'No repository configuration loaded.',
					footer: review?.resume ? [review.labels?.enter ?? {text: 'enter trust & continue', short: 'enter trust'}, review.labels?.skip ?? (review.skip === null ? 's cancel' : {text: 's continue without it (global defaults only)', short: 's continue without it'}), review.back ? 'esc back' : 'esc cancel'] : review?.project.needsReview ? ['enter trust', 'esc close'] : ['esc close'],
					scroll: detailsScroll,
				};
			case 'confirm-loss':
				return {title: 'Destructive cleanup override', text: cleanupOverrideText(cleanupInspectionFor(pendingDeleteBranch)?.reasons), footer: [`Type DELETE then enter: ${confirmationDraft}`, 'esc cancel'], scroll: detailsScroll};
			default:
				return undefined;
		}
	};

	// Shared scrolling for details modes; true when the key scrolled.
	const killConfirmOptions = killOptions(selectedCanDeleteWorktree, selectedCanDeleteBranch, currentCleanupCheck?.worktree, currentCleanupCheck?.branch);
	const killConfirmIndexClamped = Math.min(killConfirmIndex, killConfirmOptions.length - 1);
	const killConfirmInspection = cleanupInspectionFor(killConfirmOptions[killConfirmIndexClamped]?.kind === 'delete-branch');

	const scrollDetailsPane = (input: string, key: Parameters<typeof scrollDetails>[2]): boolean => {
		const content = detailsContent();
		if (!content) return false;
		const next = scrollDetails(detailsScroll, input, key, detailsViewport(content.text, layout.previewWidth, layout.contentHeight));
		if (next === undefined) return false;
		setDetailsScroll(next);
		return true;
	};

	// Dev is shared by every session in the selected session's workspace; starting/stopping acts on that one process.
	const toggleDevSelected = useCallback(async () => {
		if (!client || !selectedSession || !workspaceKey(selectedSession)) {
			return;
		}
		setBusy(true);
		setError(undefined);
		try {
			if (selectedSession.devRunning || (dev.sessionId === selectedSession.id && dev.live)) {
				await client.stopDev(selectedSession.id);
				setDev({...EMPTY_DEV, sessionId: selectedSession.id, cwd: selectedSession.cwd});
				setSessions(current => upsertSession(current, {...(current.find(session => session.id === selectedSession.id) ?? selectedSession), devRunning: false}));
			} else {
				const nextDev = await client.startDev(selectedSession.id, layout.previewCols, layout.previewRows);
				setDev(nextDev);
				setSessions(current => upsertSession(current, {...(current.find(session => session.id === selectedSession.id) ?? selectedSession), devRunning: nextDev.live}));
				setActiveTab('dev');
			}
		} catch (nextError) {
			setError(errorMessage(nextError));
		} finally {
			setBusy(false);
		}
	}, [client, dev.live, dev.sessionId, layout.previewCols, layout.previewRows, selectedSession]);

	// projectFingerprint: the repository config reviewed and skipped for this creation (see CreateSessionInput).
	const submitCreate = useCallback(async (existingWorktreePath?: string, projectFingerprint?: string) => {
		const title = draftName.trim();
		if (!title) {
			setError('title cannot be empty');
			return;
		}
		if (!client) {
			setError('still connecting to daemon');
			return;
		}
		setBusy(true);
		setError(undefined);
		try {
			const parent = createParentId ? sessions.find(session => session.id === createParentId) : undefined;
			const sessionCwd = parent?.cwd ?? cwd;
			const sessionRepoRoot = parent?.repoRoot ?? repoRoot;
			const created = await client.createSession({
				title,
				program: PROGRAMS[programIndex]!.key,
				cwd: sessionCwd,
				repoRoot: sessionRepoRoot,
				cols: layout.previewCols,
				rows: layout.previewRows,
				worktreeMode,
				existingWorktreePath,
				parentSessionId: createParentId,
				subSessionKind: createParentId ? createSubSessionKind ?? 'clean' : undefined,
				handoffFromSessionId: handoffFromId,
				projectFingerprint,
				...worktreeMode === 'new' && baseOptions(branchList)[baseIndex]?.value ? {baseBranch: baseOptions(branchList)[baseIndex]!.value} : {},
				...taskStart && !createParentId ? {taskId: taskStart.id} : {},
			});
			setDraftName('');
			setTaskStart(undefined);
			setCreateParentId(undefined);
			setCreateSubSessionKind(undefined);
			setHandoffFromId(undefined);
			setWorktreeMode('none');
			setMode('browse');
			setSelectedId(created.id);
			setSessions(current => upsertSession(current, created));
		} catch (nextError) {
			setError(errorMessage(nextError));
		} finally {
			setBusy(false);
		}
	}, [client, createParentId, createSubSessionKind, cwd, draftName, layout.previewCols, layout.previewRows, programIndex, repoRoot, sessions, worktreeMode, handoffFromId, branchList, baseIndex, taskStart]);

	// The new worktree's base branches, loaded once the form shows a new worktree.
	useEffect(() => {
		if (mode !== 'enter-name' || worktreeMode !== 'new' || branchList || !client) return;
		let cancelled = false;
		const parent = createParentId ? sessions.find(session => session.id === createParentId) : undefined;
		void client.listBranches(parent?.cwd ?? cwd).then(list => { if (!cancelled) setBranchList(list); }).catch(() => {});
		return () => { cancelled = true; };
	}, [mode, worktreeMode, branchList, client, cwd, createParentId, sessions]);

	const killSelected = useCallback(async (deleteWorktree = false, deleteBranch = false, force = false, allowDataLoss = false) => {
		if (!client || !selectedSession || selectedSession.status !== 'running') {
			return;
		}
		setBusy(true);
		setError(undefined);
		try {
			const killedSessionId = selectedSession.id;
			await client.killSession(killedSessionId, deleteWorktree || deleteBranch, deleteBranch, force, allowDataLoss);
			setMode('browse');
			if (!force) {
				setTimeout(() => {
					const session = sessionsRef.current.find(item => item.id === killedSessionId);
					if (session?.status === 'running') {
						setError('session still running; press X to force kill');
					}
				}, 1500).unref?.();
			}
		} catch (nextError) {
			setError(errorMessage(nextError));
		} finally {
			setBusy(false);
		}
	}, [client, selectedSession]);

	const removeSelected = useCallback(async (moveOpenItems = false) => {
		if (!client || !selectedSession || selectedSession.status !== 'exited') {
			return;
		}
		setBusy(true);
		setError(undefined);
		try {
			await client.removeSession(selectedSession.id, moveOpenItems);
			if (moveOpenItems) setStatusMessage('Moved its open note items to Tasks (b)');
			setPreview(EMPTY_PREVIEW);
			setTerminal(EMPTY_TERMINAL);
			setChanges(EMPTY_CHANGES);
			setDev(EMPTY_DEV);
			setAction(EMPTY_ACTION);
		} catch (nextError) {
			setError(errorMessage(nextError));
		} finally {
			setBusy(false);
		}
	}, [client, selectedSession]);

	const restartSelected = useCallback(async (restartMode: RestartMode = 'resume', projectFingerprint?: string) => {
		if (!client || !selectedSession || selectedSession.status !== 'exited') {
			return;
		}
		if (selectedSession.worktree?.deletedAt) {
			setError('cannot restart session because its worktree was deleted');
			return;
		}
		setBusy(true);
		setError(undefined);
		try {
			const restarted = await client.restartSession(selectedSession.id, layout.previewCols, layout.previewRows, restartMode, projectFingerprint);
			setSelectedId(restarted.id);
			setSessions(current => upsertSession(current, restarted));
		} catch (nextError) {
			setError(errorMessage(nextError));
		} finally {
			setBusy(false);
		}
	}, [client, layout.previewCols, layout.previewRows, selectedSession]);

	const mergeSelected = useCallback(async (mergeMode: WorktreeMergeMode) => {
		if (!client || !selectedSession?.worktree?.path || selectedSession.worktree.mode === 'none') {
			return;
		}
		const sessionId = selectedSession.id;
		const flow = mergeFlow?.sessionId === sessionId ? mergeFlow : undefined;
		// The toggle only applies when there is something uncommitted to commit.
		const commitFirst = Boolean(flow?.preview?.uncommitted) && (flow?.commitFirst ?? true);
		setBusy(true);
		setError(undefined);
		setMergeFlow(current => (current?.sessionId === sessionId ? {...current, error: undefined} : current));
		try {
			const result = await client.mergeWorktree(sessionId, mergeMode, cwd, commitFirst);
			const committed = result.committed ? `Committed ${result.committed.files} file${result.committed.files === 1 ? '' : 's'}, then ` : '';
			if (result.conflicted) {
				setMergeConflict({sessionId, result});
				setMode('merge-conflicts');
				return;
			}
			setMode('browse');
			if (result.skipped) {
				setStatusMessage(`Skipped merge: no new commits from ${result.sourceRef} into ${result.targetBranch}`);
			} else {
				const applied = `${mergeMode === 'squash' ? 'squash applied' : 'merge applied without commit'} from ${result.sourceRef} into ${result.targetBranch}`;
				setStatusMessage(committed ? `${committed}${applied}` : `${applied[0]!.toUpperCase()}${applied.slice(1)}`);
			}
		} catch (nextError) {
			// Shown on the confirmation (a failed commit's hook output can be long); nothing was merged.
			setMergeFlow(current => (current?.sessionId === sessionId ? {...current, error: errorMessage(nextError)} : current));
		} finally {
			setBusy(false);
		}
	}, [client, cwd, mergeFlow, selectedSession]);

	const resolveConflictedMerge = useCallback(async (action: 'keep' | 'abort') => {
		if (!client || !mergeConflict) return;
		const {sessionId, result} = mergeConflict;
		setBusy(true);
		setError(undefined);
		try {
			const updated = await client.resolveMerge(sessionId, cwd, action);
			setSessions(current => upsertSession(current, updated));
			setMergeConflict(undefined);
			setMode('browse');
			const count = result.conflictCount ?? result.conflicts?.length ?? 0;
			setStatusMessage(action === 'keep'
				? `Merge kept with conflicts in ${count} file${count === 1 ? '' : 's'} into ${result.targetBranch}: resolve them, then commit (marked merged)`
				: `Merge aborted; ${result.targetBranch} is as it was`);
		} catch (nextError) {
			setError(errorMessage(nextError));
		} finally {
			setBusy(false);
		}
	}, [client, cwd, mergeConflict]);

	const toggleSelectedDone = useCallback(async () => {
		if (!client || !selectedSession) return;
		const done = !selectedSession.doneAt;
		setError(undefined);
		try {
			const updated = await client.setSessionDone(selectedSession.id, done);
			setSessions(current => upsertSession(current, updated));
			setStatusMessage(done ? `Marked done: ${displaySessionTitle(updated, sessionsRef.current)}` : `No longer done: ${displaySessionTitle(updated, sessionsRef.current)}`);
		} catch (nextError) {
			setError(errorMessage(nextError));
		}
	}, [client, selectedSession]);

	const markSelectedMerged = useCallback(async () => {
		if (!client || !selectedSession) {
			return;
		}
		const wasMerged = hasMergedMarker(selectedSession);
		setBusy(true);
		setError(undefined);
		try {
			const updated = await client.markSessionMerged(selectedSession.id, cwd);
			setSessions(current => upsertSession(current, updated));
			setStatusMessage(wasMerged ? 'Unmarked' : `Marked into ${mergedTargetBranch(updated) ?? 'target branch'}`);
		} catch (nextError) {
			setError(errorMessage(nextError));
		} finally {
			setBusy(false);
		}
	}, [client, cwd, selectedSession]);

	useTerminalInput((input, key) => {
		// Ink's exitOnCtrlC is disabled (see cli.ts) so a first Ctrl+C acts like Esc
		// while a Settings edit or JSON draft is open (asking before discarding it). Ctrl+C at the
		// discard prompt, while a save is in flight, or anywhere else quits like q.
		if (key.ctrl && input === 'c') {
			if (settingsFlow.cancelable() && !busy) {
				settingsFlow.handleInput('', {escape: true});
				return;
			}
			quit();
			return;
		}

		if (busy) {
			return;
		}

		if (isSettingsFlowMode(mode)) {
			settingsFlow.handleInput(input, key);
			return;
		}
		if (mode === 'agents') {
			agents.handleInput(input, key);
			return;
		}
		if (mode === 'help') {
			if (help.handleInput(input, key) === 'close') setMode('browse');
			return;
		}

		if (mode === 'preview-focus') {
			const sendClaudeWheel = (direction: 'up' | 'down', count = 1) => {
				if (client && selectedSession?.program === 'claude' && selectedSession.status === 'running') {
					client.sendAgentInput(selectedSession.id, mouseWheelSequence(direction, layout.previewCols, layout.previewRows, count));
					setPreviewScrollOffset(0);
					return true;
				}
				return false;
			};
			const scrollPreview = (direction: 'up' | 'down', count = 1) => {
				if (sendClaudeWheel(direction, count)) {
					return;
				}
				if (direction === 'up') {
					setPreviewScrollOffset(offset => Math.min((preview.maxScrollOffset ?? offset + count), offset + count));
				} else {
					setPreviewScrollOffset(offset => Math.max(0, offset - count));
				}
			};
			if (key.escape || input === 'v') {
				setMode('browse');
				return;
			}
			if (input === '[' || input === ']') {
				adjustScrollSensitivity(input === ']' ? SCROLL_SENSITIVITY_STEP : -SCROLL_SENSITIVITY_STEP);
				return;
			}
			if (input === 'k') {
				scrollPreview('up');
				return;
			}
			if (input === 'j') {
				scrollPreview('down');
				return;
			}
			if (input === 'g') {
				// Claude scrolls its own view, so only the wheel fallback applies there.
				if (!sendClaudeWheel('up', 12)) setPreviewScrollOffset(offset => preview.maxScrollOffset ?? offset + layout.previewRows);
				return;
			}
			if (input === 'G') {
				if (!sendClaudeWheel('down', 12)) setPreviewScrollOffset(0);
				return;
			}
			return;
		}

		if (mode === 'tasks') {
			tasksFlow.handleInput(input, key);
			return;
		}

		if (mode === 'confirm-remove') {
			if (key.escape) { setMode('browse'); return; }
			if (input === 'm') { setMode('browse'); void removeSelected(true); return; }
			if (key.return) { setMode('browse'); void removeSelected(false); return; }
			return;
		}

		if (mode === 'changes-focus') {
			changesFlow.handleInput(input, key);
			return;
		}

		if (mode === 'notes-focus') {
			notesFlow.handleInput(input, key);
			return;
		}

		if (mode === 'search') {
			if (key.escape) { setSessionQuery(''); setMode('browse'); return; }
			if (key.return) { setMode('browse'); return; }
			if (key.backspace || key.delete) setSessionQuery(value => value.slice(0, -1));
			else if (!key.ctrl && !key.meta) setSessionQuery(value => (value + stripTerminalControls(input)).slice(0, 256));
			return;
		}
		if (mode === 'review-project') {
			// Enter trusts these exact bytes and resumes the gated action; s resumes it with the override ignored.
			if (!review) { setMode('browse'); return; }
			const {project, cwd: reviewCwd, resume, skip, back} = review;
			const close = () => { setReview(undefined); setMode('browse'); back?.(); };
			if (key.escape) { close(); return; }
			if (scrollDetailsPane(input, key)) return;
			if (input === 's') { close(); if (skip !== null) (skip ?? resume)?.(project); return; }
			if (!key.return || !client) return;
			if (!project.needsReview) { close(); setStatusMessage(project.trusted ? 'Already trusted' : 'Nothing to trust'); resume?.(project); return; }
			setBusy(true); setError(undefined);
			void client.trustProject(reviewCwd, project.fingerprint).then(trusted => {
				setBusy(false); close(); setStatusMessage('Repository configuration trusted');
				resume?.(trusted);
			}, async error => {
				// Changed since this review: show the new bytes instead.
				setError(errorMessage(error));
				const next = await client.projectInfo(reviewCwd).catch(() => undefined);
				if (next) setReview(current => (current ? {...current, project: next} : current));
				setBusy(false);
			});
			return;
		}
		if (mode === 'confirm-loss') {
			if (key.escape) { setMode('browse'); return; }
			if (scrollDetailsPane(input, key)) return;
			if (key.backspace || key.delete) { setConfirmationDraft(value => value.slice(0, -1)); return; }
			if (key.return) {
				if (confirmationDraft === 'DELETE') {
					const inspection = cleanupInspectionFor(pendingDeleteBranch);
					if (inspection && !inspection.safe && structuralBlockers(inspection).length === 0) void killSelected(true, pendingDeleteBranch, killConfirmForce, true);
				}
				return;
			}
			if (!key.ctrl && !key.meta) setConfirmationDraft(value => (value + sanitizeNameInput(input)).slice(0, 10));
			return;
		}
		if (mode === 'workspace-info') {
			if (currentWorkspaceInfo?.confirmPr) {
				// Pushing is outward-facing: only Enter on this confirmation runs it.
				const {sessionId, summary} = currentWorkspaceInfo;
				if (key.escape) { setWorkspaceInfo(current => (current?.sessionId === sessionId ? {...current, confirmPr: false} : current)); return; }
				if (scrollDetailsPane(input, key)) return;
				if (!key.return || !client || !summary) return;
				setDetailsScroll(0); setError(undefined);
				setWorkspaceInfo(current => (current?.sessionId === sessionId ? {...current, confirmPr: false, creatingPr: true} : current));
				const done = () => setWorkspaceInfo(current => (current?.sessionId === sessionId ? {...current, creatingPr: false} : current));
				void client.createPr(sessionId, summary.branch).then(result => {
					done();
					setStatusMessage(result.existing ? `Pushed ${result.branch} to ${result.remote}; opened its PR` : `Pushed ${result.branch} to ${result.remote}; opened GitHub's new-PR form${result.base ? ` (base ${result.base})` : ''}`);
					// Refresh the counts; P fetches the PR now that the cache was invalidated.
					const requestId = ++workspaceRequestRef.current;
					void client.workspaceSummary(sessionId).then(next => { if (workspaceRequestRef.current === requestId) setWorkspaceInfo(current => (current?.sessionId === sessionId ? {...current, summary: next} : current)); }).catch(() => {});
				}, error => { done(); setError(errorMessage(error)); });
				return;
			}
			if (key.escape) { setMode('browse'); return; }
			if (scrollDetailsPane(input, key)) return;
			if (input === 'c' && currentWorkspaceInfo?.summary && !currentWorkspaceInfo.creatingPr) {
				if (currentWorkspaceInfo.summary.branch === '(detached)') { setError('HEAD is detached; check out a branch before creating a PR'); return; }
				const {sessionId} = currentWorkspaceInfo;
				setDetailsScroll(0);
				setWorkspaceInfo(current => (current?.sessionId === sessionId ? {...current, confirmPr: true} : current));
				return;
			}
			if (input === 'P' && client && currentWorkspaceInfo?.summary && !currentWorkspaceInfo.prLoading && !currentWorkspaceInfo.creatingPr) {
				const {sessionId} = currentWorkspaceInfo;
				const requestId = ++workspaceRequestRef.current;
				setWorkspaceInfo(current => (current?.sessionId === sessionId ? {...current, prLoading: true} : current));
				void client.workspaceSummary(sessionId, true).then(summary => {
					if (workspaceRequestRef.current === requestId) setWorkspaceInfo({sessionId, summary});
				}).catch(error => {
					if (workspaceRequestRef.current !== requestId) return;
					setWorkspaceInfo(current => (current?.sessionId === sessionId ? {...current, prLoading: false} : current));
					setError(errorMessage(error));
				});
				return;
			}
			if (input === 'b' && currentWorkspaceInfo?.summary?.pr) { openUrl(currentWorkspaceInfo.summary.pr.url, setError); return; }
			if (input === 'g') { setActiveTab('git'); setMode('browse'); }
			return;
		}
		if (mode === 'pick-action') {
			const actions = projectActions(actionProject?.project);
			const reviewSessionId = actionProject?.sessionId;
			if (key.escape) { setMode('browse'); return; }
			if (key.upArrow || input === 'k') setActionIndex(index => Math.max(0, index - 1));
			if (key.downArrow || input === 'j') setActionIndex(index => Math.min(Math.max(0, actions.length - 1), index + 1));
			const picked = actions[actionIndex];
			// x stops the worktree's running action (the picker says which); one runs at a time per worktree.
			if (input === 'x' && client && reviewSessionId && action.sessionId === reviewSessionId && action.live) {
				setBusy(true);
				void client.stopAction(reviewSessionId).then(() => setStatusMessage(`Stopped ${action.name ?? 'the action'}`))
					.catch(error => setError(errorMessage(error))).finally(() => setBusy(false));
				return;
			}
			if (key.return && picked && actionProject) {
				if (!client || !reviewSessionId) { setError('select a session to run project actions'); return; }
				const run = () => {
					setBusy(true);
					void client.runAction(reviewSessionId, picked.name, layout.previewCols, layout.previewRows).then(record => {
						// The action runs in its own process; the Terminal tab shows it until v switches back to the shell.
						setAction(record); setActiveTab('terminal'); setTerminalView('action');
						setMode(current => (current === 'pick-action' ? 'browse' : current));
					}).catch(error => setError(errorMessage(error))).finally(() => setBusy(false));
				};
				if (!picked.needsTrust) { run(); return; }
				// An untrusted repository action is reviewed now, right before it would run; Esc returns to the list.
				reviewThen(actionProject.cwd, run, {
					back: () => setMode('pick-action'), skip: picked.fallback === undefined ? null : run,
					labels: {enter: {text: 'enter trust & run', short: 'enter trust'}, skip: picked.fallback === undefined ? {text: 's cancel this run', short: 's cancel'} : {text: `s run the global ${picked.name} instead`, short: 's run global'}},
					purpose: () => `About to run the repository action ${picked.name}: ${picked.command}`,
				});
			}
			return;
		}
		if (mode === 'browse') {
			if (input === '[' || input === ']') {
				adjustScrollSensitivity(input === ']' ? SCROLL_SENSITIVITY_STEP : -SCROLL_SENSITIVITY_STEP);
				return;
			}
			if (numericSelection) {
				if (/^\d$/.test(input)) {
					setNumericSelection(value => value + input);
					return;
				}
				if (key.return) {
					confirmNumericSelection(numericSelection);
					return;
				}
				if (key.escape) {
					setNumericSelection('');
					return;
				}
				if (key.backspace || key.delete) {
					setNumericSelection(value => value.slice(0, -1));
					return;
				}
				setNumericSelection('');
			}
			if (/^\d$/.test(input)) {
				if (visibleSessions.length <= 10) {
					confirmNumericSelection(input === '0' ? '10' : input);
				} else {
					setNumericSelection(input);
				}
				return;
			}
			if (input === 'q') {
				quit();
				return;
			}
			if (input === '?') {
				help.open(); setMode('help');
				return;
			}
			if (input === '/') { setMode('search'); return; }
			if (input === 'f') {
				const next = nextSessionFilter(sessionFilter);
				setSessionFilter(next); setStatusMessage(filterCycleMessage(next));
				return;
			}
			if (input === 'A' && client && selectedSession) {
				const {id, archivedAt} = selectedSession, title = displaySessionTitle(selectedSession, sessions);
				// Archiving hides the session from the default view, so say where it went.
				void client.archiveSession(id, !archivedAt).then(() => setStatusMessage(archivedAt ? `Unarchived ${title}`
					: `Archived ${title}${sessionFilter === 'active' ? ' · hidden here, press f for the archived view' : ''}`)).catch(error => setError(errorMessage(error)));
				return;
			}
			if (input === '!') {
				const targets = sessions.filter(session => !session.archivedAt && sessionNeedsAttention(session));
				const next = targets[(targets.findIndex(session => session.id === selectedId) + 1) % Math.max(1, targets.length)];
				if (next) { setSessionFilter('attention'); setSessionQuery(''); setSelectedId(next.id); } else setStatusMessage('No known attention requests');
				return;
			}
			if (input === 'i' && client && selectedSession) {
				const sessionId = selectedSession.id;
				const requestId = ++workspaceRequestRef.current;
				setWorkspaceInfo({sessionId}); setDetailsScroll(0); setMode('workspace-info');
				void client.workspaceSummary(sessionId).then(summary => {
					if (workspaceRequestRef.current === requestId) setWorkspaceInfo({sessionId, summary});
				}).catch(error => {
					if (workspaceRequestRef.current === requestId) setError(errorMessage(error));
				});
				return;
			}
			if (input === 'C' && client) {
				settingsFlow.open(selectedSession?.cwd ?? cwd);
				return;
			}
			if (input === 'U' && client) {
				agents.open();
				return;
			}
			if (input === 'e') {
				// Every action of the session's repository is listed; untrusted repository ones are reviewed when chosen.
				const sessionId = selectedSession?.id, actionCwd = selectedSession?.cwd ?? cwd;
				// The picker says whether an action is still running in this worktree (x stops it), whichever tab is shown.
				if (client && sessionId && selectedWorkspace) void client.watchAction(sessionId, layout.previewCols, layout.previewRows).then(record => { if (record.sessionId === selectedIdRef.current) setAction(record); }).catch(() => {});
				reviewThen(actionCwd, project => {
					if (!project) return;
					setActionProject({project, cwd: actionCwd, sessionId}); setActionIndex(0); setMode('pick-action');
				}, {required: true, gate: () => false});
				return;
			}
			if (input === 'b') {
				tasksFlow.open();
				setMode('tasks');
				return;
			}
			if (input === 'H' && client && selectedSession) {
				void client.exportHandoff(selectedSession.id).then(file => {
					setStatusMessage(`Handoff: ${file}`);
					openInEditor(file, setError);
				}).catch(error => setError(errorMessage(error)));
				return;
			}
			if (input === 'F' && selectedSession) {
				if (!selectedSession.handoffPath) { setError('Export and review a handoff with H before creating a handoff child'); return; }
				const parent = selectedSession;
				setSessionFilter('active'); setSessionQuery('');
				setHandoffFromId(parent.id); setCreateParentId(parent.id); setCreateSubSessionKind('clean'); setDraftName(''); setWorktreeMode('none');
				setProgramIndex(Math.max(0, PROGRAMS.findIndex(program => program.key === parent.program)));
				setMode('pick-program');
				return;
			}
			if ((input === 'x' || input === 'X') && selectedSession?.status === 'starting' && client) { void client.cancelStart(selectedSession.id).catch(error => setError(errorMessage(error))); return; }
			if (input === 'n' || input === 'N') {
				// No review here: the repository's defaults only preselect the picker (trusted or not). Creating a new
				// worktree reviews whatever it would run (see confirmCreate). Children keep their parent's agent and directory.
				const parent = input === 'N' ? selectedSession : undefined;
				reviewThen(parent?.cwd ?? cwd, project => {
					const defaults = parent ? undefined : project?.effective;
					setHandoffFromId(undefined);
					setSessionFilter('active'); setSessionQuery('');
					setProgramIndex(Math.max(0, PROGRAMS.findIndex(program => program.key === (parent?.program ?? defaults?.defaultAgent ?? 'claude'))));
					setDraftName('');
					setTaskStart(undefined); setBranchList(undefined); setBaseIndex(0);
					setCreateParentId(parent?.id);
					setCreateSubSessionKind(parent ? 'clean' : undefined);
					setWorktreeMode(defaults?.defaultWorkspace ?? 'none');
					setMode('pick-program');
				}, {gate: () => false});
				return;
			}
			if (input === 'r') {
				void refreshSessions().catch(nextError =>
					setError(errorMessage(nextError)),
				);
				return;
			}
			if (key.tab) {
				setPreviewScrollOffset(0);
				setActiveTab(tab => RIGHT_TABS[(RIGHT_TABS.indexOf(tab) + 1) % RIGHT_TABS.length] ?? 'preview');
				return;
			}
			if (input === 'p') {
				setPreviewScrollOffset(0);
				setActiveTab('preview');
				return;
			}
			if (input === 't') {
				setPreviewScrollOffset(0);
				setActiveTab('terminal');
				return;
			}
			if (input === 'g') {
				setPreviewScrollOffset(0);
				setActiveTab('git');
				return;
			}
			if (input === 'a') {
				setPreviewScrollOffset(0);
				setActiveTab('notes');
				return;
			}
			if (input === 'd') {
				if (activeTab !== 'dev') {
					setPreviewScrollOffset(0);
					setActiveTab('dev');
					return;
				}
				if (selectedSession && workspaceKey(selectedSession)) {
					// Starting Dev reviews an untrusted repository devCommand first (skipping uses the global one).
					if (selectedSession.devRunning || (dev.sessionId === selectedSession.id && dev.live)) void toggleDevSelected();
					else reviewThen(selectedSession.cwd, () => void toggleDevSelected(), {
						gate: project => project.needsReview && Boolean(project.config.devCommand),
						// Skipping starts what applies until trusted: the global (or legacy) Dev command, else the built-in `dev`.
						labels: project => ({enter: {text: 'enter trust & start', short: 'enter trust'}, skip: {text: `s start ${project.effective.devCommand?.trim() || 'dev'} instead`, short: 's start fallback'}}),
						purpose: project => `About to start Dev with the repository devCommand: ${project.config.devCommand} (until trusted, d runs ${project.effective.devCommand?.trim() ? `the global ${project.effective.devCommand.trim()}` : 'the built-in fallback dev'})`,
					});
				} else if (selectedSession) {
					setError(`Dev is unavailable: ${noWorkspaceReason(selectedSession)}`);
				}
				return;
			}
			if (input === 'v' && activeTab === 'preview' && selectedSession?.status === 'running') {
				setMode('preview-focus');
				return;
			}
			// The Terminal tab switches between the shell and the worktree's last action.
			if (input === 'v' && activeTab === 'terminal' && selectedSession) {
				if (hasAction(selectedSession, action)) setTerminalView(view => (view === 'action' && showingAction ? 'shell' : 'action'));
				else setError('No action has run in this worktree yet; e runs one');
				return;
			}
			if (input === 'v' && activeTab === 'git' && selectedSession) {
				if (!selectedWorkspace) setError(`Git is unavailable: ${noWorkspaceReason(selectedSession)}`);
				else if (changes.sessionId === selectedSession.id && changes.workspace) setMode('changes-focus');
				else setError('Changes are still loading; try again in a moment');
				return;
			}
			if (input === 'c') {
				toggleSelectedCollapse();
				return;
			}
			if (input === 'K') {
				void reorderSelected('up');
				return;
			}
			if (input === 'J') {
				void reorderSelected('down');
				return;
			}
			if (input === 'k') {
				moveSelection(-1);
				return;
			}
			if (input === 'j') {
				moveSelection(1);
				return;
			}
			if (key.leftArrow || input === 'h') {
				resizeSidebar(-2);
				return;
			}
			if (key.rightArrow || input === 'l') {
				resizeSidebar(2);
				return;
			}
			if (input === 'm' && selectedSession?.worktree?.path && selectedSession.worktree.mode !== 'none' && !selectedSession.worktree.deletedAt) {
				const sessionId = selectedSession.id;
				const requestId = ++mergeRequestRef.current;
				setMergeConfirmIndex(0);
				setMergeFlow({sessionId, commitFirst: true});
				setMode('confirm-merge');
				// What would be merged, checked fresh each time the confirmation opens.
				client?.mergePreview(sessionId, cwd).then(preview => {
					if (mergeRequestRef.current === requestId) setMergeFlow(current => (current?.sessionId === sessionId ? {...current, preview} : current));
				}).catch(nextError => {
					if (mergeRequestRef.current === requestId) setMergeFlow(current => (current?.sessionId === sessionId ? {...current, previewError: errorMessage(nextError)} : current));
				});
				return;
			}
			if (input === 'M' && selectedSession) {
				// Merged belongs to worktrees; a main-checkout session can still be marked done.
				if (!selectedSession.worktree?.id && !hasMergedMarker(selectedSession)) setStatusMessage(selectedSession.requestedWorktreeMode && selectedSession.requestedWorktreeMode !== 'none' && !workspaceKey(selectedSession) ? 'Its worktree is not ready yet' : 'Not in a worktree, so there is nothing to mark merged. Use D to mark it done');
				else void markSelectedMerged();
				return;
			}
			if (input === 'D' && selectedSession) {
				void toggleSelectedDone();
				return;
			}
			if ((input === 'x' || input === 'X') && selectedSession?.status === 'running') {
				const force = input === 'X';
				if (selectedSession.worktree?.path && selectedSession.worktree.mode !== 'none') {
					const sessionId = selectedSession.id;
					const requestId = ++cleanupRequestRef.current;
					setKillConfirmIndex(0);
					setKillConfirmForce(force);
					setCleanupCheck({sessionId});
					setMode('confirm-kill');
					if (client) {
						const inspect = (deleteBranch: boolean) => {
							const settle = (inspection: SessionCleanupInspection) => {
								if (cleanupRequestRef.current === requestId) setCleanupCheck(current => (current?.sessionId === sessionId ? {...current, [deleteBranch ? 'branch' : 'worktree']: inspection} : current));
							};
							void client.inspectCleanup(sessionId, deleteBranch).then(settle).catch(error => settle({safe: false, reasons: [errorMessage(error)], dirtyFiles: 0, untrackedFiles: 0, ignoredFiles: 0, structuralBlockers: []}));
						};
						inspect(false);
						if (selectedCanDeleteBranch) inspect(true);
					}
				} else {
					void killSelected(false, false, force);
				}
				return;
			}
			if ((key.backspace || key.delete) && selectedSession?.status === 'exited') {
				requestRemove();
				return;
			}
			if ((input === 's' || input === 'S') && selectedSession?.status === 'exited') {
				const restartMode = input === 'S' ? 'fresh' : 'resume';
				// Setup still owed (failed, cancelled or refused as untrusted) depends on the repository config.
				if (selectedSession.setup && selectedSession.setup.state !== 'complete') reviewThen(selectedSession.cwd, project => void restartSelected(restartMode, project?.fingerprint), {
					gate: project => project.needsReview && Boolean(project.config.setupCommand),
					labels: {enter: {text: 'enter trust & retry setup', short: 'enter trust'}, skip: {text: "s retry without the repo's setup", short: 's without it'}},
					purpose: project => `About to run the repository setupCommand: ${project.config.setupCommand}`,
				});
				else void restartSelected(restartMode);
				return;
			}
			if (input === 'O') {
				openSelectedInEditor();
				return;
			}
			if (input === 'o' && activeTab === 'notes' && selectedSession) {
				notesFlow.focus();
				setMode('notes-focus');
				return;
			}
			// The Notes tab's active section (the session's, or the worktree's after editing that) in Cursor / VS Code.
			if (input === 'E' && activeTab === 'notes' && selectedSession) {
				notesFlow.openActiveInEditor();
				return;
			}
			const workspaceTab = activeTab === 'terminal' || activeTab === 'git' || activeTab === 'dev';
			if (input === 'o' && selectedSession && (selectedSession.status === 'running' || workspaceTab)) {
				if (workspaceTab && !selectedWorkspace) {
					setError(`${activeTab === 'git' ? 'Git' : activeTab === 'terminal' ? 'Terminal' : 'Dev'} is unavailable: ${noWorkspaceReason(selectedSession)}`);
					return;
				}
				if (activeTab === 'dev' && !selectedSession.devRunning && !(dev.sessionId === selectedSession.id && dev.live)) {
					setError('start the dev command with d before attaching');
					return;
				}
				if (activeAttachTarget === 'action' && !action.live) {
					setError(`${action.name ?? 'The action'} has finished; e runs an action again (v shows the shell)`);
					return;
				}
				if (!activePaneReadyForAttach) {
					setError(`${activeAttachTarget === 'git' ? 'Git' : activeAttachTarget === 'terminal' ? 'Terminal' : 'Dev'} tab is still loading; wait for it to appear before attaching`);
					return;
				}
				attachTo(selectedSession, activeAttachTarget);
			}
			return;
		}

		if (mode === 'pick-program') {
			if (key.escape) {
				setCreateParentId(undefined);
				setCreateSubSessionKind(undefined);
				if (taskStart) { setTaskStart(undefined); setMode('tasks'); return; }
				setMode('browse');
				return;
			}
			const parent = createParentId ? sessions.find(session => session.id === createParentId) : undefined;
			const optionCount = PROGRAMS.length + (parent && !handoffFromId && supportsForkedSubSession(parent) ? 1 : 0);
			if (key.leftArrow || key.upArrow || input === 'k' || input === 'h') {
				setProgramIndex(index => (index - 1 + optionCount) % optionCount);
				return;
			}
			if (key.rightArrow || key.downArrow || input === 'j' || input === 'l') {
				setProgramIndex(index => (index + 1) % optionCount);
				return;
			}
			if (key.return) {
				if (parent && !handoffFromId && supportsForkedSubSession(parent) && programIndex === PROGRAMS.length) {
					setCreateSubSessionKind('forked');
					setProgramIndex(Math.max(0, PROGRAMS.findIndex(program => program.key === parent.program)));
					if (forkStaysInParent(parent.program, 'forked')) setWorktreeMode('none');
				} else {
					setCreateSubSessionKind(parent ? 'clean' : undefined);
				}
				setMode('enter-name');
			}
			return;
		}

		if (mode === 'enter-name') {
			if (key.escape) {
				setMode('pick-program');
				return;
			}
			if (key.return) {
				if (worktreeMode === 'existing') {
					if (!draftName.trim()) {
						setError('title cannot be empty');
						return;
					}
					if (!client) {
						setError('still connecting to daemon');
						return;
					}
					const parent = createParentId ? sessions.find(session => session.id === createParentId) : undefined;
					setBusy(true);
					void client
						.listWorktrees(parent?.cwd ?? cwd)
						.then(items => {
							setWorktrees(items);
							setWorktreeQuery('');
							setWorktreeIndex(0);
							setMode('pick-worktree');
						})
						.catch(nextError => setError(errorMessage(nextError)))
						.finally(() => setBusy(false));
					return;
				}
				if (worktreeMode === 'new') { confirmCreate(); return; }
				void submitCreate();
				return;
			}
			if (key.backspace || key.delete) {
				setDraftName(value => value.slice(0, -1));
				return;
			}
			if (key.tab) {
				if (forkStaysInParent(PROGRAMS[programIndex]?.key, createSubSessionKind)) return;
				setWorktreeMode(current => {
					const index = WORKTREE_MODES.findIndex(item => item.key === current);
					return WORKTREE_MODES[(index + 1) % WORKTREE_MODES.length]!.key;
				});
				return;
			}
			if ((key.upArrow || key.downArrow) && worktreeMode === 'new' && !forkStaysInParent(PROGRAMS[programIndex]?.key, createSubSessionKind)) {
				const count = baseOptions(branchList).length;
				if (count > 1) setBaseIndex(index => (index + (key.downArrow ? 1 : count - 1)) % count);
				return;
			}
			if (key.ctrl || key.meta || key.upArrow || key.downArrow || key.leftArrow || key.rightArrow) {
				return;
			}
			if (input) {
				const text = sanitizeNameInput(input);
				if (text) {
					setDraftName(value => value + text);
				}
			}
			return;
		}

		if (mode === 'pick-worktree') {
			if (key.escape) {
				setMode('enter-name');
				return;
			}
			if (key.upArrow) {
				setWorktreeIndex(index => Math.max(0, index - 1));
				return;
			}
			if (key.downArrow) {
				setWorktreeIndex(index => Math.min(Math.max(0, filteredWorktrees.length - 1), index + 1));
				return;
			}
			if (key.return && filteredWorktrees[worktreeIndex]) {
				void submitCreate(filteredWorktrees[worktreeIndex]!.path);
				return;
			}
			if (key.backspace || key.delete) {
				setWorktreeQuery(value => value.slice(0, -1));
				setWorktreeIndex(0);
				return;
			}
			if (key.ctrl || key.meta || key.leftArrow || key.rightArrow || key.tab) {
				return;
			}
			if (input) {
				const text = sanitizeNameInput(input);
				if (text) {
					setWorktreeQuery(value => value + text);
					setWorktreeIndex(0);
				}
			}
			return;
		}

		if (mode === 'merge-conflicts') {
			// Only two choices: keep the merge in progress (marked merged), or abort it.
			if (key.return) void resolveConflictedMerge('keep');
			else if (input === 'a') void resolveConflictedMerge('abort');
			return;
		}

		if (mode === 'confirm-merge') {
			const optionCount = 3;
			if (key.escape) {
				setMode('browse');
				return;
			}
			if (input === ' ') {
				if (mergeFlow?.preview?.uncommitted) setMergeFlow(current => (current ? {...current, commitFirst: !current.commitFirst} : current));
				return;
			}
			if (key.upArrow || input === 'k') {
				setMergeConfirmIndex(index => (index - 1 + optionCount) % optionCount);
				return;
			}
			if (key.downArrow || input === 'j') {
				setMergeConfirmIndex(index => (index + 1) % optionCount);
				return;
			}
			if (key.return) {
				// The preview decides what the toggle applies to; until it (or its error) arrives, only Cancel works.
				const pending = mergeConfirmIndex < 2 && !mergeFlow?.preview && !mergeFlow?.previewError;
				if (pending) setStatusMessage('Still checking what will be merged…');
				else if (mergeConfirmIndex === 0) void mergeSelected('merge');
				else if (mergeConfirmIndex === 1) void mergeSelected('squash');
				else setMode('browse');
				return;
			}
		}

		if (mode === 'confirm-kill') {
			const options = killConfirmOptions;
			const selectedIndex = killConfirmIndexClamped;
			if (key.escape) {
				setMode('browse');
				return;
			}
			if (key.upArrow || input === 'k') {
				setKillConfirmIndex((selectedIndex - 1 + options.length) % options.length);
				return;
			}
			if (key.downArrow || input === 'j') {
				setKillConfirmIndex((selectedIndex + 1) % options.length);
				return;
			}
			if (key.return) {
				const option = options[selectedIndex]!;
				if (option.kind === 'kill') void killSelected(false, false, killConfirmForce);
				else if (option.kind === 'delete' || option.kind === 'delete-branch') {
					// Wait for this session's inspection; never authorize from another session's result.
					const deleteBranch = option.kind === 'delete-branch';
					const inspection = cleanupInspectionFor(deleteBranch);
					if (!inspection) return;
					const blockers = structuralBlockers(inspection);
					if (blockers.length > 0) setError(blockers.join('; '));
					else if (inspection.safe) void killSelected(true, deleteBranch, killConfirmForce);
					else { setPendingDeleteBranch(deleteBranch); setConfirmationDraft(''); setDetailsScroll(0); setMode('confirm-loss'); }
				} else setMode('browse');
				return;
			}
		}
	});

	// The sidebar header shows the filter, search and counts, so the repo path gets the whole row.
	const repoLabelWidth = Math.max(1, terminalSize.cols);
	const candidateMessages: Array<FooterMessage | undefined> = [
		error ? {text: `Error: ${error}`, color: THEME.error} : undefined,
		busy ? {text: 'Working…', color: THEME.warn} : undefined,
		numericSelection ? {text: `Select session: ${numericSelection}`, color: THEME.active} : undefined,
		statusMessage ? {text: statusMessage, color: THEME.success} : undefined,
		selectedSession?.cleanupError ? {text: selectedSession.cleanupError, color: THEME.error} : undefined,
	];
	const footerMessages = candidateMessages.filter((message): message is FooterMessage => Boolean(message));
	// Exactly FOOTER_ROWS rows, each truncated, so the layout above never shifts.
	const footerRows = [
		<Text key="hint" color={mode === 'search' ? THEME.active : THEME.muted} wrap="truncate-end">
			{mode === 'search' ? `Search: /${sessionQuery} · enter keep · esc clear` : mode === 'tasks' ? tasksFlow.hint(terminalSize.cols) : footerHint(mode, activeTab, terminalSize.cols, selectedSession, previewScrollSensitivity, activePaneReadyForAttach, mode === 'notes-focus' ? notesFlow.hint(terminalSize.cols) : undefined, hasAction(selectedSession, action) ? {switchHint: showingAction ? 'v shell' : `v ${action.name ?? 'action'} ${actionStatus(action).text}`, finished: showingAction && !action.live} : undefined)}
		</Text>,
		<Text key="messages" wrap="truncate-end">
			{footerMessages.length > 0
				? footerMessages.map((message, index) => <Text key={index} color={message.color}>{index > 0 ? ' · ' : ''}{message.text.replace(/\s*\n\s*/g, ' ')}</Text>)
				: ' '}
		</Text>,
	];
	const details = detailsContent();
	// Settings (C) take the full width: the grid shows both layers side by side.
	const settingsOpen = isSettingsFlowMode(mode) && !details;
	// Muted, only when an installed agent has a newer release (nothing while all are current or the latest is unknown).
	const agentUpdateHint = updateHint(agentVersions);
	// Open tasks in the header, so a forgotten backlog stays in sight (the first thing to go when narrow).
	const taskCount = terminalSize.cols >= 70 ? taskCountLabel(tasks) : '';
	const selectedTask = linkedTask(tasks, selectedSession);

	return (
		<Box flexDirection="column">
			<Box justifyContent="space-between" width={terminalSize.cols}>
				<Text color={THEME.accent} bold>{process.env.DECKHAND_CHANNEL === 'dev' ? 'deckhand · DEV (isolated)' : 'deckhand'}</Text>
				<Text wrap="truncate-start">
					{taskCount ? <Text color={THEME.muted}>{taskCount}   </Text> : null}
					{agentUpdateHint ? <Text color={THEME.muted}>{agentUpdateHint}   </Text> : null}
					<Text color={connectionColor(client)}>● {describeConnection(client)}</Text>
				</Text>
			</Box>
			<Box width={terminalSize.cols}>
				<Text color={THEME.muted} wrap="truncate-end">{truncate(compactPath(repoRoot, repoLabelWidth), repoLabelWidth)}</Text>
			</Box>
			<Box flexDirection="row">
				{settingsOpen ? settingsFlow.render(terminalSize.cols, layout.contentHeight) : mode === 'help' ? help.render(terminalSize.cols, layout.contentHeight) : mode === 'agents' ? agents.render(terminalSize.cols, layout.contentHeight) : <>
				<Sidebar
					sessions={visibleSessions}
					allSessions={sessions}
					selectedId={selectedSession?.id}
					width={layout.sidebarWidth}
					height={layout.contentHeight}
					spinnerFrame={spinnerFrame}
					collapsedSessionIds={collapseApplied ? collapsedSessionIds : EMPTY_ID_SET}
					hiddenSessionIds={collapseApplied ? hiddenExitedSessionIds : EMPTY_ID_SET}
					loaded={sessionsLoaded}
					filter={sessionFilter}
					query={sessionQuery}
					now={Date.now()}
					installedVersions={sidebarVersions}
					taskOf={session => linkedTask(tasks, session)}
				/>
				<Box width={1} />
				{mode === 'browse' || mode === 'preview-focus' || mode === 'changes-focus' || mode === 'notes-focus' || mode === 'search' ? (
					<Box
						flexDirection="column"
						width={layout.previewWidth}
						height={layout.contentHeight}
						borderStyle="round"
						borderColor={THEME.border}
						paddingX={1}
					>
						<TabBar activeTab={activeTab} width={layout.paneInnerWidth} devRunning={selectedSession?.devRunning} />
						<Box height={1} />
						{activeTab === 'preview' ? (
							<PreviewPane
								session={selectedSession}
								preview={preview}
								width={layout.paneInnerWidth}
								height={layout.paneInnerHeight}
								spinnerFrame={spinnerFrame}
								focused={mode === 'preview-focus'}
							/>
						) : activeTab === 'terminal' ? (
							<TerminalPane
								session={selectedSession}
								terminal={terminal}
								action={action}
								view={terminalView}
								width={layout.paneInnerWidth}
								height={layout.paneInnerHeight}
							/>
						) : activeTab === 'git' ? (
							changesFlow.render(layout.paneInnerWidth, layout.paneInnerHeight)
						) : activeTab === 'dev' ? (
							<DevPane session={selectedSession} dev={dev} width={layout.paneInnerWidth} height={layout.paneInnerHeight} />
						) : selectedTask ? (
							<Box flexDirection="column">
								<TaskBanner task={selectedTask} sessions={sessions} spinnerFrame={spinnerFrame} width={layout.paneInnerWidth} />
								{notesFlow.render(layout.paneInnerWidth, Math.max(1, layout.paneInnerHeight - 2))}
							</Box>
						) : (
							notesFlow.render(layout.paneInnerWidth, layout.paneInnerHeight)
						)}
					</Box>
				) : mode === 'tasks' ? (
					tasksFlow.render(layout.previewWidth, layout.contentHeight)
				) : mode === 'confirm-remove' ? (
					<RemoveConfirmPane session={selectedSession} items={removeItems} width={layout.previewWidth} />
				) : details ? (
					<DetailsPane title={details.title} text={details.text} footer={details.footer} width={layout.previewWidth} height={layout.contentHeight} scroll={details.scroll} />
				) : mode === 'pick-action' ? (
					<ActionPickerPane project={actionProject?.project} running={action.sessionId === actionProject?.sessionId ? action : undefined} selectedIndex={actionIndex} width={layout.previewWidth} height={layout.contentHeight} />
				) : mode === 'pick-worktree' ? (
					<WorktreePickerPane
						worktrees={filteredWorktrees}
						selectedIndex={worktreeIndex}
						query={worktreeQuery}
						totalCount={worktrees.length}
						width={layout.previewWidth}
						height={layout.contentHeight}
					/>
				) : mode === 'confirm-kill' ? (
					<KillConfirmPane
						session={selectedSession}
						sessions={sessions}
						options={killConfirmOptions}
						selectedIndex={killConfirmIndexClamped}
						force={killConfirmForce}
						width={layout.previewWidth}
						inspection={killConfirmInspection}
					/>
				) : mode === 'confirm-merge' ? (
					<MergeConfirmPane session={selectedSession} sessions={sessions} flow={mergeFlow?.sessionId === selectedSession?.id ? mergeFlow : undefined} selectedIndex={mergeConfirmIndex} width={layout.previewWidth} height={layout.contentHeight} tasks={tasks.filter(task => !task.done && task.meta.wt && task.meta.wt === selectedSession?.worktree?.id).map(task => task.title)} />
				) : mode === 'merge-conflicts' && mergeConflict ? (
					<MergeConflictPane result={mergeConflict.result} width={layout.previewWidth} />
				) : mode === 'pick-program' || mode === 'enter-name' ? (
					<CreatePane
						mode={mode}
						programIndex={programIndex}
						draftName={draftName}
						worktreeMode={worktreeMode}
						width={layout.previewWidth}
						parentTitle={createParentId ? sessions.find(session => session.id === createParentId)?.title : undefined}
						parentWorkspaceLabel={parentWorkspaceLabel(createParentId ? sessions.find(session => session.id === createParentId) : undefined, layout.previewWidth)}
						subSessionKind={createSubSessionKind}
						taskTitle={taskStart && !createParentId ? taskStart.title : undefined}
						base={{label: baseOptions(branchList)[Math.min(baseIndex, baseOptions(branchList).length - 1)]!.label, index: Math.min(baseIndex, baseOptions(branchList).length - 1), count: baseOptions(branchList).length}}
						showForkOption={createParentId && !handoffFromId ? supportsForkedSubSession(sessions.find(session => session.id === createParentId)) : false}
					/>
				) : null}
				</>}
			</Box>
			{footerRows}
		</Box>
	);
}
