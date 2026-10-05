import React, {useCallback, useEffect, useMemo, useRef, useState} from 'react';
import {Box, Text, useApp} from 'ink';
import {useTerminalInput} from './useTerminalInput.js';
import {LiveClient, createLiveClient} from './client.js';
import {loadAppConfig, updateAppConfig} from './storage.js';
import {DevPane} from './devPane.js';
import {DetailsPane, detailsViewport, scrollDetails} from './detailsPane.js';
import {actionPickerText, cleanupOverrideText, createPrConfirmText, projectActionNames, trustReviewText, workspaceSummaryText} from './detailTexts.js';
import {openInEditor, openUrl} from './desktop.js';
import {isConfigFlowMode, useProjectConfigFlow} from './projectConfigFlow.js';
import {HELP_TEXT} from './help.js';
import {filterSessionList, sessionNeedsAttention, SESSION_FILTERS, type SessionFilter} from './sessionFeatures.js';
import {GitPane} from './gitPane.js';
import {NotesPane} from './notesPane.js';
import {PreviewPane} from './preview.js';
import {sessionMatchesScope} from './sessionScope.js';
import {Sidebar} from './sidebar.js';
import {filterCollapsedSessions, sessionDescendants, sessionHasChildren, sortSessionsForSidebar} from './sessionOrder.js';
import {TabBar} from './tabs.js';
import {TerminalPane} from './terminalPane.js';
import type {AttachTarget, DevRecord, GitRecord, PreviewRecord, ProgramKey, RestartMode, RightPaneTab, SessionRecord, SubSessionKind, TerminalRecord, UiExitResult, WorktreeInfoRecord, WorktreeMergeMode, WorktreeMode, ProjectInfo, WorkspaceSummary, SessionCleanupInspection} from './types.js';
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

const EMPTY_GIT: GitRecord = {
	content: '',
	live: false,
};

const EMPTY_DEV: DevRecord = {
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

type Mode = 'browse' | 'preview-focus' | 'notes-focus' | 'pick-program' | 'enter-name' | 'pick-worktree' | 'confirm-kill' | 'confirm-merge' | 'help' | 'pick-config' | 'edit-project' | 'discard-project' | 'worktree-setup' | 'discard-worktree-setup' | 'effective-settings' | 'search' | 'workspace-info' | 'review-project' | 'pick-action' | 'confirm-loss';

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
	return session?.program === 'claude' || session?.program === 'pi';
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
}) {
	const forkSelected = mode === 'pick-program' && showForkOption && programIndex === PROGRAMS.length;
	const workspaceLabel = parentWorkspaceLabel && worktreeMode === 'none'
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
						{PROGRAMS.map((program, index) => {
							const selected = index === programIndex;
							return (
								<Text key={program.key} inverse={selected} color={selected ? THEME.active : undefined} bold={selected}>
									{selected ? '›' : ' '} {program.glyph} {program.label}
								</Text>
							);
						})}
						{showForkOption ? (
							<Text inverse={forkSelected} color={forkSelected ? THEME.active : undefined} bold={forkSelected}>
								{forkSelected ? '›' : ' '} ⑂ Fork parent
							</Text>
						) : null}
					</>
				) : (
					<>
						<Text>Name: <Text color={draftName ? THEME.active : THEME.muted}>{draftName || '█'}</Text></Text>
						<Text>Workspace: <Text color={THEME.accent}>{workspaceLabel}</Text></Text>
					</>
				)}
			</Box>
			{parentTitle ? <Text color={THEME.muted}>Parent: {truncate(parentTitle, Math.max(8, width - 12))}</Text> : null}
			<Box marginTop={1}>
				<Text color={THEME.muted}>
					{mode === 'pick-program' ? 'enter continue · esc cancel · ↑↓ switch' : 'tab worktree · enter create · esc back'}
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
}: {
	worktrees: WorktreeInfoRecord[];
	selectedIndex: number;
	query: string;
	totalCount: number;
	width: number;
}) {
	const contentWidth = Math.max(1, width - 4);
	const countLabel = query ? `${worktrees.length}/${totalCount}` : String(totalCount);
	return (
		<Box flexDirection="column" width={width} borderStyle="round" borderColor={THEME.borderActive} paddingX={1}>
			<Text color={THEME.accent} bold>Existing worktree</Text>
			<Text>
				Search: <Text color={query ? THEME.active : THEME.muted}>{query || 'type to filter'}</Text>{' '}
				<Text color={THEME.muted}>({countLabel})</Text>
			</Text>
			<Box marginTop={1} flexDirection="column">
				{totalCount === 0 ? <Text color={THEME.muted}>No worktrees found.</Text> : null}
				{totalCount > 0 && worktrees.length === 0 ? <Text color={THEME.muted}>No matching worktrees.</Text> : null}
				{worktrees.map((worktree, index) => {
					const selected = index === selectedIndex;
					return (
						<Text key={worktree.path} inverse={selected} color={selected ? THEME.active : undefined}>
							{selected ? '›' : ' '} {worktreeLabel(worktree, contentWidth - 2)}
						</Text>
					);
				})}
			</Box>
			<Box marginTop={1}>
				<Text color={THEME.muted}>type search · enter select · esc back · ↑↓ move · backspace delete</Text>
			</Box>
		</Box>
	);
}

function MergeConfirmPane({session, sessions, selectedIndex, width}: {session?: SessionRecord; sessions: SessionRecord[]; selectedIndex: number; width: number}) {
	const options = ['Merge into current branch without committing', 'Squash merge into current branch without committing', 'Cancel'];
	const contentWidth = Math.max(1, width - 4);
	const noteSessions = session ? [session, ...sessionDescendants(session.id, sessions)] : [];
	const noteEntries = noteSessions
		.map(noteSession => ({session: noteSession, lines: (noteSession.notes?.trim() ?? '').split('\n').filter(Boolean)}))
		.filter(entry => entry.lines.length > 0);
	return (
		<Box flexDirection="column" width={width} borderStyle="round" borderColor={THEME.borderActive} paddingX={1}>
			<Text color={THEME.accent} bold>Merge {session ? `"${displaySessionTitle(session, sessions)}"` : 'worktree'}?</Text>
			{session?.worktree?.path ? (
				<Text color={THEME.muted}>{truncate(compactPath(session.worktree.path, contentWidth), contentWidth)}</Text>
			) : null}
			<Box marginTop={1} flexDirection="column">
				{options.map((option, index) => {
					const selected = index === selectedIndex;
					const isCancel = option === 'Cancel';
					return (
						<Text key={option} inverse={selected} color={selected ? (isCancel ? THEME.muted : THEME.active) : undefined} bold={selected}>
							{selected ? '›' : ' '} {option}
						</Text>
					);
				})}
			</Box>
			<Box marginTop={1}>
				<Text color={THEME.muted}>enter choose · esc cancel · j/k move</Text>
			</Box>
			<Box marginTop={1} flexDirection="column">
				<Text color={THEME.accentSoft} bold>Notes</Text>
				{noteEntries.length > 0 ? noteEntries.map(entry => (
					<Box key={entry.session.id} flexDirection="column">
						<Text color={THEME.muted} bold>{truncate(displaySessionTitle(entry.session, sessions), contentWidth)}</Text>
						{entry.lines.map((line, index) => (
							<Text key={`${entry.session.id}-${index}`} color={THEME.muted}>{truncate(`  ${line}`, contentWidth)}</Text>
						))}
					</Box>
				)) : (
					<Text color={THEME.muted}>No notes for this session or its sub-sessions.</Text>
				)}
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
				{options.map((option, index) => {
					const selected = index === selectedIndex;
					const isCancel = option.kind === 'cancel';
					const color = selected ? (isCancel ? THEME.muted : THEME.error) : undefined;
					return (
						<Text key={option.kind} inverse={selected} color={color} bold={selected}>
							{selected ? '›' : ' '} {option.label}
						</Text>
					);
				})}
			</Box>
			<Box marginTop={1}>
				<Text color={THEME.muted}>enter choose · esc cancel · j/k move</Text>
			</Box>
		</Box>
	);
}

function hasMergedMarker(session?: SessionRecord): boolean {
	return Boolean(session?.worktree?.mergedAt || session?.mergedAt);
}

function mergedTargetBranch(session: SessionRecord): string | undefined {
	return session.worktree?.mergeTargetBranch ?? session.mergeTargetBranch;
}

function footerHint(mode: Mode, activeTab: RightPaneTab, session?: SessionRecord, scrollSensitivity = DEFAULT_SCROLL_SENSITIVITY, attachReady = true): string {
	switch (mode) {
		case 'help': return 'help & workflows • j/k/arrows/PgUp/PgDn scroll • Home/End jump • esc/? close';
		case 'pick-config': return 'j/k/arrows choose global defaults, repository, worktree setup or effective settings • enter open • esc cancel';
		case 'effective-settings': return 'effective settings • ↑↓ move • enter/e open the file that sets it • T review/trust • esc back';
		case 'worktree-setup': return 'worktree setup • ↑↓ move • space link/skip • ←→ option • t target • h hook • Ctrl+S save • e JSON • esc cancel';
		case 'discard-worktree-setup': return 'enter discard unsaved worktree setup • esc keep editing';
		case 'edit-project': return 'config editor • Ctrl+S save • Ctrl+F format • Ctrl+A select all • esc cancel';
		case 'discard-project': return 'enter discard unsaved draft • esc keep editing';
		case 'preview-focus': {
			const method = session?.program === 'claude' ? 'mouse wheel' : 'scrollback';
			return `preview focus (${method}) • wheel scroll ×${formatScrollSensitivity(scrollSensitivity)} • [/] adjust • j/k scroll • g/G top/bottom • esc/v return`;
		}
		case 'notes-focus': return 'notes edit • type to edit • enter newline • esc stop editing';
		case 'search': return 'type to search • enter keep search • esc clear';
		case 'workspace-info': return 'workspace overview • P PR status • b open PR • c create PR • g lazygit • j/k/PgUp/PgDn scroll • esc close';
		case 'review-project': return 'review repository config • ↑↓/PgUp/PgDn scroll';
		case 'confirm-loss': return 'type DELETE then enter to authorize data loss • ↑↓/PgUp/PgDn scroll • esc cancel';
		case 'pick-action': return 'j/k choose action • enter run in Dev pane • esc cancel';
		case 'pick-program': return 'enter continue • esc cancel • j/k switch';
		case 'enter-name': return 'tab worktree mode • enter create • esc back • backspace delete';
		case 'pick-worktree': return 'type search • enter select • esc back • ↑↓ move • backspace delete';
		case 'confirm-kill':
		case 'confirm-merge': return 'enter choose • esc cancel • j/k move';
		case 'browse': {
			// Keep this short; everything else is listed in ? help.
			const running = session?.status === 'running';
			const attach = running && activeTab !== 'notes' ? (attachReady ? 'o attach' : 'loading…') : undefined;
			const pane = activeTab === 'notes' ? (session ? 'o edit notes' : undefined)
				: activeTab === 'dev' && running ? 'd start/stop'
					: activeTab === 'preview' && running ? 'v scroll' : undefined;
			const lifecycle = session?.status === 'exited'
				? (session.worktree?.deletedAt ? 'backspace remove' : 's resume • S fresh')
				: running ? 'x kill' : session?.status === 'starting' ? 'x cancel start' : undefined;
			return [attach, pane, lifecycle, '? help', 'n new', 'C config', 'T trust', 'i info', 'e actions', '/ search', 'f filter', 'q quit'].filter(Boolean).join(' • ');
		}
	}
}

export function App({repoRoot, cwd, initialSelectedId, initialActiveTab, initialSidebarWidth, initialSessionTabs, initialCollapsedSessionIds, initialHiddenExitedSessionIds, initialSessionFilter, initialSessionQuery, onSessionVisibilityChange, onSelectedIdChange, onActiveTabChange, onSessionTabChange, onSidebarWidthChange, onCollapsedSessionIdsChange, onHiddenExitedSessionIdsChange}: AppProps) {
	const {exit} = useApp();
	const [mode, setMode] = useState<Mode>('browse');
	const [sessionFilter, setSessionFilter] = useState<SessionFilter>(initialSessionFilter ?? 'active');
	const [sessionQuery, setSessionQuery] = useState(initialSessionQuery ?? '');
	// The inline repository-config review: what it shows, the cwd it was resolved for, and the action it gates
	// (resumed after Enter trusts or s skips; none for an explicit T review).
	// back: where closing the review returns (Effective settings' T); otherwise browse.
	const [review, setReview] = useState<{project: ProjectInfo; cwd: string; resume?: (project: ProjectInfo) => void; back?: () => void}>();
	// The project (and session) the action picker was opened for.
	const [actionProject, setActionProject] = useState<{project: ProjectInfo; sessionId?: string}>();
	// The repository config fingerprint reviewed for the session being created (see CreateSessionInput).
	const [createProjectFingerprint, setCreateProjectFingerprint] = useState<string>();
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
	const sessionTabsRef = useRef<Record<string, RightPaneTab>>({
		...(initialSessionTabs ?? {}),
		...(initialSelectedId && initialActiveTab ? {[initialSelectedId]: initialActiveTab} : {}),
	});
	const [activeTab, setActiveTab] = useState<RightPaneTab>(initialSelectedId ? sessionTabsRef.current[initialSelectedId] ?? 'preview' : initialActiveTab ?? 'preview');
	const [notesDraft, setNotesDraft] = useState('');
	const lastSavedNotesRef = useRef<Record<string, string>>({});
	const [previewScrollOffset, setPreviewScrollOffset] = useState(0);
	const [previewScrollSensitivity, setPreviewScrollSensitivity] = useState(DEFAULT_SCROLL_SENSITIVITY);
	const previewWheelAccumulatorRef = useRef(0);
	const [preview, setPreview] = useState<PreviewRecord>(EMPTY_PREVIEW);
	const [terminal, setTerminal] = useState<TerminalRecord>(EMPTY_TERMINAL);
	const [git, setGit] = useState<GitRecord>(EMPTY_GIT);
	const [dev, setDev] = useState<DevRecord>(EMPTY_DEV);
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
			const filtered = filterSessionList(sessions, sessionFilter, sessionQuery);
			return collapseApplied ? filterCollapsedSessions(filtered, collapsedSessionIds, hiddenExitedSessionIds) : filtered;
		},
		[collapseApplied, collapsedSessionIds, hiddenExitedSessionIds, sessions, sessionFilter, sessionQuery],
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
	}, [activeTab, onActiveTabChange, onSessionTabChange]);

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
							setGit(EMPTY_GIT);
							setDev(EMPTY_DEV);
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
					onGitUpdated: nextGit => {
						if (nextGit.sessionId && nextGit.sessionId !== selectedIdRef.current) {
							return;
						}
						if (!nextGit.sessionId && selectedIdRef.current) {
							return;
						}
						setGit(nextGit);
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
	const currentWorkspaceInfo = workspaceInfo && workspaceInfo.sessionId === selectedSession?.id ? workspaceInfo : undefined;
	const currentCleanupCheck = cleanupCheck && cleanupCheck.sessionId === selectedSession?.id ? cleanupCheck : undefined;
	const cleanupInspectionFor = (deleteBranch: boolean) => (deleteBranch ? currentCleanupCheck?.branch : currentCleanupCheck?.worktree);
	const activeAttachTarget: AttachTarget = activeTab === 'terminal' ? 'terminal' : activeTab === 'git' ? 'git' : activeTab === 'dev' ? 'dev' : 'agent';
	const activePaneReadyForAttach = Boolean(
		selectedSession?.status === 'running' && (
			activeAttachTarget === 'agent' ||
			(activeAttachTarget === 'terminal' && terminal.sessionId === selectedSession.id && terminal.live) ||
			(activeAttachTarget === 'git' && git.sessionId === selectedSession.id && git.live) ||
			(activeAttachTarget === 'dev' && dev.sessionId === selectedSession.id && dev.live)
		),
	);

	useEffect(() => {
		const notes = selectedSession?.notes ?? '';
		if (selectedSession && lastSavedNotesRef.current[selectedSession.id] === undefined) {
			lastSavedNotesRef.current[selectedSession.id] = notes;
		}
		setNotesDraft(notes);
	}, [selectedSession?.id]);

	useEffect(() => {
		if (!client || !selectedSession) {
			return;
		}
		const sessionId = selectedSession.id;
		if ((lastSavedNotesRef.current[sessionId] ?? '') === notesDraft) {
			return;
		}
		const timer = setTimeout(() => {
			void client.updateSessionNotes(sessionId, notesDraft).then(updated => {
				lastSavedNotesRef.current[sessionId] = updated.notes ?? '';
				setSessions(current => upsertSession(current, updated));
			}).catch(nextError => {
				setError(errorMessage(nextError));
			});
		}, 300);
		return () => clearTimeout(timer);
	}, [client, notesDraft, selectedSession?.id]);

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
			setGit(EMPTY_GIT);
			setDev(EMPTY_DEV);
			return;
		}
		setTerminal(current => (current.sessionId === selectedSession.id ? current : EMPTY_TERMINAL));
		setGit(current => (current.sessionId === selectedSession.id ? current : EMPTY_GIT));
		setDev(current => (current.sessionId === selectedSession.id ? current : EMPTY_DEV));
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
	}, [activeTab, client, layout.previewCols, layout.previewRows, selectedId]);

	useEffect(() => {
		if (!client || activeTab !== 'git') {
			return;
		}
		let cancelled = false;
		void client
			.watchGit(selectedId, layout.previewCols, layout.previewRows)
			.then(nextGit => {
				if (cancelled) {
					return;
				}
				if (nextGit.sessionId && nextGit.sessionId !== selectedId) {
					return;
				}
				setGit(nextGit);
			})
			.catch(nextError => {
				if (!cancelled) {
					setError(errorMessage(nextError));
				}
			});
		return () => {
			cancelled = true;
		};
	}, [activeTab, client, layout.previewCols, layout.previewRows, selectedId]);

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
	}, [activeTab, client, layout.previewCols, layout.previewRows, selectedId]);

	const updateNotesDraft = useCallback((updater: (current: string) => string) => {
		if (!selectedSession) {
			return;
		}
		setNotesDraft(current => {
			const next = updater(current).slice(0, 50_000);
			setSessions(sessions => {
				const currentSession = sessions.find(session => session.id === selectedSession.id) ?? selectedSession;
				return upsertSession(sessions, {...currentSession, notes: next});
			});
			return next;
		});
	}, [selectedSession]);

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

	const configFlow = useProjectConfigFlow({client, mode, setMode, setBusy, setError, setStatusMessage, onReview: (reviewCwd, back) => reviewThen(reviewCwd, undefined, {back})});

	// Resolves the repository config for `targetCwd` and runs `resume` with it, showing the inline review first when
	// `gate` says the action depends on an untrusted override. If the config cannot be read, the error is shown and
	// `resume` still runs without it (the daemon enforces trust and reports problems) unless `required`.
	// Without `resume` (T) the review is shown on its own.
	const reviewThen = (targetCwd: string, resume: ((project?: ProjectInfo) => void) | undefined, {gate = (project: ProjectInfo) => project.needsReview, required = false, back}: {gate?: (project: ProjectInfo) => boolean; required?: boolean; back?: () => void} = {}) => {
		if (!client) return;
		setBusy(true); setError(undefined);
		void client.projectInfo(targetCwd).then(project => {
			setBusy(false);
			if (resume && !gate(project)) { resume(project); return; }
			setReview({project, cwd: targetCwd, resume, back}); setDetailsScroll(0); setMode('review-project');
		}, error => {
			setBusy(false); setError(errorMessage(error));
			if (!required) resume?.(undefined);
		});
	};

	const quit = useCallback(() => {
		exit({kind: 'quit'} satisfies UiExitResult);
	}, [exit]);

	// Title, text and footer for the scrollable details modes. Built only when
	// one of those modes is active (render and key handling).
	const detailsContent = (): {title: string; text: string; footer: string; scroll: number} | undefined => {
		switch (mode) {
			case 'help':
				return {title: 'Help & workflows', text: HELP_TEXT, footer: 'j/k/arrows/PgUp/PgDn scroll · Home/End jump · ? close', scroll: detailsScroll};
			case 'workspace-info':
				if (currentWorkspaceInfo?.confirmPr && currentWorkspaceInfo.summary) return {title: 'Create pull request', text: createPrConfirmText(currentWorkspaceInfo.summary), footer: 'enter push & open PR form · esc cancel', scroll: detailsScroll};
				return {title: 'Workspace overview', text: workspaceSummaryText(currentWorkspaceInfo?.summary, currentWorkspaceInfo?.prLoading, {creatingPr: currentWorkspaceInfo?.creatingPr, links: selectedSession?.worktree?.links}), footer: 'P fetch PR status · b open PR · c create PR · g lazygit', scroll: detailsScroll};
			case 'review-project':
				return {
					title: 'Review repository configuration',
					text: review ? trustReviewText(review.project) : 'No repository configuration loaded.',
					footer: review?.resume ? 'enter trust & continue • s continue without it (global defaults only) • esc cancel' : review?.project.needsReview ? 'enter trust • esc close' : 'esc close',
					scroll: detailsScroll,
				};
			case 'confirm-loss':
				return {title: 'Destructive cleanup override', text: cleanupOverrideText(cleanupInspectionFor(pendingDeleteBranch)?.reasons), footer: `Type DELETE then enter: ${confirmationDraft}`, scroll: detailsScroll};
			case 'pick-action':
				return {title: 'Actions', text: actionPickerText(actionProject?.project, actionIndex), footer: 'j/k choose · enter run in Dev pane', scroll: Math.max(0, actionIndex - 1)};
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

	const toggleDevSelected = useCallback(async () => {
		if (!client || !selectedSession || selectedSession.status !== 'running') {
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

	const submitCreate = useCallback(async (existingWorktreePath?: string) => {
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
				projectFingerprint: createProjectFingerprint,
			});
			setDraftName('');
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
	}, [client, createParentId, createSubSessionKind, cwd, draftName, layout.previewCols, layout.previewRows, programIndex, repoRoot, sessions, worktreeMode, handoffFromId, createProjectFingerprint]);

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

	const removeSelected = useCallback(async () => {
		if (!client || !selectedSession || selectedSession.status !== 'exited') {
			return;
		}
		setBusy(true);
		setError(undefined);
		try {
			await client.removeSession(selectedSession.id);
			setPreview(EMPTY_PREVIEW);
			setTerminal(EMPTY_TERMINAL);
			setGit(EMPTY_GIT);
			setDev(EMPTY_DEV);
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
		setBusy(true);
		setError(undefined);
		try {
			const result = await client.mergeWorktree(selectedSession.id, mergeMode, cwd);
			setMode('browse');
			if (result.skipped) {
				setStatusMessage(`Skipped merge: no new commits from ${result.sourceRef} into ${result.targetBranch}`);
			} else if (result.conflicted) {
				setStatusMessage(`${mergeMode === 'squash' ? 'Squash merge' : 'Merge'} has conflicts to resolve from ${result.sourceRef} into ${result.targetBranch}`);
			} else {
				setStatusMessage(`${mergeMode === 'squash' ? 'Squash applied' : 'Merge applied without commit'} from ${result.sourceRef} into ${result.targetBranch}`);
			}
		} catch (nextError) {
			setError(errorMessage(nextError));
		} finally {
			setBusy(false);
		}
	}, [client, cwd, selectedSession]);

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
		// in the config editor (asking before discarding a draft). Ctrl+C at the
		// discard prompt, while a save is in flight, or anywhere else quits like q.
		if (key.ctrl && input === 'c') {
			if ((mode === 'edit-project' || mode === 'worktree-setup') && !busy) {
				configFlow.handleInput('', {escape: true});
				return;
			}
			quit();
			return;
		}

		if (busy) {
			return;
		}

		if (isConfigFlowMode(mode)) {
			configFlow.handleInput(input, key);
			return;
		}
		if (mode === 'help') {
			if (key.escape || input === '?') { setMode('browse'); return; }
			scrollDetailsPane(input, key);
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

		if (mode === 'notes-focus') {
			if (key.escape) {
				setMode('browse');
				return;
			}
			if (key.ctrl || key.meta) {
				return;
			}
			if (key.backspace || key.delete) {
				updateNotesDraft(value => value.slice(0, -1));
				return;
			}
			if (key.return) {
				updateNotesDraft(value => `${value}\n`);
				return;
			}
			if (input) {
				const cleaned = input.replace(/\r\n?|\n/g, '\n').replace(/[\u0000-\u0008\u000B-\u001F\u007F]/g, '');
				if (cleaned) {
					updateNotesDraft(value => value + cleaned);
				}
				return;
			}
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
			const {project, cwd: reviewCwd, resume, back} = review;
			const close = () => { setReview(undefined); setMode('browse'); back?.(); };
			if (key.escape) { close(); return; }
			if (scrollDetailsPane(input, key)) return;
			if (input === 's') { close(); resume?.(project); return; }
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
			const actions = projectActionNames(actionProject?.project);
			const reviewSessionId = actionProject?.sessionId;
			if (key.escape) { setMode('browse'); return; }
			if (key.upArrow || input === 'k') setActionIndex(index => Math.max(0, index - 1));
			if (key.downArrow || input === 'j') setActionIndex(index => Math.min(Math.max(0, actions.length - 1), index + 1));
			if (key.return && actions[actionIndex]) {
				if (!client || !reviewSessionId) { setError('select a running session to run project actions'); return; }
				setBusy(true);
				void client.runAction(reviewSessionId, actions[actionIndex]!, layout.previewCols, layout.previewRows).then(record => {
					setDev(record); setActiveTab('dev');
					setMode(current => (current === 'pick-action' ? 'browse' : current));
				}).catch(error => setError(errorMessage(error))).finally(() => setBusy(false));
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
				setDetailsScroll(0); setMode('help');
				return;
			}
			if (input === '/') { setMode('search'); return; }
			if (input === 'f') { setSessionFilter(filter => SESSION_FILTERS[(SESSION_FILTERS.indexOf(filter) + 1) % SESSION_FILTERS.length]!); return; }
			if (input === 'A' && client && selectedSession) { void client.archiveSession(selectedSession.id, !selectedSession.archivedAt).catch(error => setError(errorMessage(error))); return; }
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
				configFlow.open(selectedSession?.cwd ?? cwd);
				return;
			}
			if (input === 'T') {
				reviewThen(selectedSession?.cwd ?? cwd, undefined);
				return;
			}
			if (input === 'e') {
				// Actions come from the session's repository: global defaults, plus repository actions once trusted.
				const sessionId = selectedSession?.id;
				reviewThen(selectedSession?.cwd ?? cwd, project => {
					if (!project) return;
					setActionProject({project, sessionId}); setActionIndex(0); setMode('pick-action');
				}, {required: true});
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
				reviewThen(parent.cwd, project => {
					setSessionFilter('active'); setSessionQuery('');
					setHandoffFromId(parent.id); setCreateParentId(parent.id); setCreateSubSessionKind('clean'); setDraftName(''); setWorktreeMode('none');
					setCreateProjectFingerprint(project?.fingerprint);
					setProgramIndex(Math.max(0, PROGRAMS.findIndex(program => program.key === parent.program)));
					setMode('pick-program');
				});
				return;
			}
			if ((input === 'x' || input === 'X') && selectedSession?.status === 'starting' && client) { void client.cancelStart(selectedSession.id).catch(error => setError(errorMessage(error))); return; }
			if (input === 'n' || input === 'N') {
				// Review first: the repository override can set the defaults and the new worktree's setup.
				// Children keep their parent's agent and directory.
				const parent = input === 'N' ? selectedSession : undefined;
				reviewThen(parent?.cwd ?? cwd, project => {
					const defaults = parent ? undefined : project?.effective;
					setHandoffFromId(undefined);
					setSessionFilter('active'); setSessionQuery('');
					setProgramIndex(Math.max(0, PROGRAMS.findIndex(program => program.key === (parent?.program ?? defaults?.defaultAgent ?? 'claude'))));
					setDraftName('');
					setCreateParentId(parent?.id);
					setCreateSubSessionKind(parent ? 'clean' : undefined);
					setCreateProjectFingerprint(project?.fingerprint);
					setWorktreeMode(defaults?.defaultWorkspace ?? 'none');
					setMode('pick-program');
				});
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
				if (selectedSession?.status === 'running') {
					// Starting Dev reviews an untrusted repository devCommand first (skipping uses the global one).
					if (selectedSession.devRunning || (dev.sessionId === selectedSession.id && dev.live)) void toggleDevSelected();
					else reviewThen(selectedSession.cwd, () => void toggleDevSelected(), {gate: project => project.needsReview && Boolean(project.config.devCommand)});
				} else {
					setError('session must be running to start dev');
				}
				return;
			}
			if (input === 'v' && activeTab === 'preview' && selectedSession?.status === 'running') {
				setMode('preview-focus');
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
				setMergeConfirmIndex(0);
				setMode('confirm-merge');
				return;
			}
			if (input === 'M') {
				void markSelectedMerged();
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
				void removeSelected();
				return;
			}
			if ((input === 's' || input === 'S') && selectedSession?.status === 'exited') {
				const restartMode = input === 'S' ? 'fresh' : 'resume';
				// Setup still owed (failed, cancelled or refused as untrusted) depends on the repository config.
				if (selectedSession.setup && selectedSession.setup.state !== 'complete') reviewThen(selectedSession.cwd, project => void restartSelected(restartMode, project?.fingerprint));
				else void restartSelected(restartMode);
				return;
			}
			if (input === 'O') {
				openSelectedInEditor();
				return;
			}
			if (input === 'o' && activeTab === 'notes' && selectedSession) {
				setMode('notes-focus');
				return;
			}
			if (input === 'o' && selectedSession?.status === 'running') {
				if (activeTab === 'dev' && !selectedSession.devRunning && !(dev.sessionId === selectedSession.id && dev.live)) {
					setError('start the dev command with d before attaching');
					return;
				}
				if (!activePaneReadyForAttach) {
					setError(`${activeAttachTarget === 'git' ? 'Git' : activeAttachTarget === 'terminal' ? 'Terminal' : 'Dev'} tab is still loading; wait for it to appear before attaching`);
					return;
				}
				exit({
					kind: 'attach',
					sessionId: selectedSession.id,
					target: activeAttachTarget,
					title: displaySessionTitle(selectedSession, sessions),
					cwd: selectedSession.cwd,
					program: selectedSession.program,
				} satisfies UiExitResult);
			}
			return;
		}

		if (mode === 'pick-program') {
			if (key.escape) {
				setCreateParentId(undefined);
				setCreateSubSessionKind(undefined);
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
				void submitCreate();
				return;
			}
			if (key.backspace || key.delete) {
				setDraftName(value => value.slice(0, -1));
				return;
			}
			if (key.tab) {
				setWorktreeMode(current => {
					const index = WORKTREE_MODES.findIndex(item => item.key === current);
					return WORKTREE_MODES[(index + 1) % WORKTREE_MODES.length]!.key;
				});
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

		if (mode === 'confirm-merge') {
			const optionCount = 3;
			if (key.escape) {
				setMode('browse');
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
				if (mergeConfirmIndex === 0) void mergeSelected('merge');
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

	const visibilityLabel = truncate(`${sessionFilter}${sessionQuery ? ` · /${sessionQuery}` : ''} · ${visibleSessions.length}/${sessions.length} sessions`, Math.max(10, Math.floor(terminalSize.cols / 2)));
	const repoLabelWidth = Math.max(1, terminalSize.cols - visibilityLabel.length - 1);
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
			{mode === 'search' ? `Search: /${sessionQuery} · enter keep · esc clear` : mode === 'workspace-info' && currentWorkspaceInfo?.confirmPr ? 'create pull request • enter push & open PR form • esc cancel' : footerHint(mode, activeTab, selectedSession, previewScrollSensitivity, activePaneReadyForAttach)}
		</Text>,
		<Text key="messages" wrap="truncate-end">
			{footerMessages.length > 0
				? footerMessages.map((message, index) => <Text key={index} color={message.color}>{index > 0 ? ' · ' : ''}{message.text.replace(/\s*\n\s*/g, ' ')}</Text>)
				: ' '}
		</Text>,
	];
	const details = detailsContent();

	return (
		<Box flexDirection="column">
			<Box justifyContent="space-between" width={terminalSize.cols}>
				<Text color={THEME.accent} bold>{process.env.DECKHAND_CHANNEL === 'dev' ? 'deckhand · DEV (isolated)' : 'deckhand'}</Text>
				<Text color={connectionColor(client)}>● {describeConnection(client)}</Text>
			</Box>
			<Box justifyContent="space-between" width={terminalSize.cols}>
				<Text color={THEME.muted} wrap="truncate-end">{truncate(compactPath(repoRoot, repoLabelWidth), repoLabelWidth)}</Text>
				<Text color={THEME.muted} wrap="truncate-end">{visibilityLabel}</Text>
			</Box>
			<Box flexDirection="row">
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
				/>
				<Box width={1} />
				{mode === 'browse' || mode === 'preview-focus' || mode === 'notes-focus' || mode === 'search' ? (
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
								width={layout.paneInnerWidth}
								height={layout.paneInnerHeight}
							/>
						) : activeTab === 'git' ? (
							<GitPane session={selectedSession} git={git} width={layout.paneInnerWidth} height={layout.paneInnerHeight} />
						) : activeTab === 'dev' ? (
							<DevPane session={selectedSession} dev={dev} width={layout.paneInnerWidth} height={layout.paneInnerHeight} />
						) : (
							<NotesPane session={selectedSession} notes={notesDraft} width={layout.paneInnerWidth} height={layout.paneInnerHeight} focused={mode === 'notes-focus'} />
						)}
					</Box>
				) : details ? (
					<DetailsPane title={details.title} text={details.text} footer={details.footer} width={layout.previewWidth} height={layout.contentHeight} scroll={details.scroll} />
				) : isConfigFlowMode(mode) ? (
					configFlow.render(layout.previewWidth, layout.contentHeight)
				) : mode === 'pick-worktree' ? (
					<WorktreePickerPane
						worktrees={filteredWorktrees}
						selectedIndex={worktreeIndex}
						query={worktreeQuery}
						totalCount={worktrees.length}
						width={layout.previewWidth}
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
					<MergeConfirmPane session={selectedSession} sessions={sessions} selectedIndex={mergeConfirmIndex} width={layout.previewWidth} />
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
						showForkOption={createParentId && !handoffFromId ? supportsForkedSubSession(sessions.find(session => session.id === createParentId)) : false}
					/>
				) : null}
			</Box>
			{footerRows}
		</Box>
	);
}
