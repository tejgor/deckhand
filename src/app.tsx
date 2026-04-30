import React, {useCallback, useEffect, useMemo, useRef, useState} from 'react';
import {Box, Text, useApp, useInput} from 'ink';
import {LiveClient, createLiveClient} from './client.js';
import {DevPane} from './devPane.js';
import {GitPane} from './gitPane.js';
import {PreviewPane} from './preview.js';
import {Sidebar} from './sidebar.js';
import {TabBar} from './tabs.js';
import {TerminalPane} from './terminalPane.js';
import type {DevRecord, GitRecord, PreviewRecord, ProgramKey, RightPaneTab, SessionRecord, TerminalRecord, UiExitResult, WorktreeInfoRecord, WorktreeMergeMode, WorktreeMode} from './types.js';
import {THEME, compactPath, truncate} from './ui.js';

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

const ANSI_ESCAPE_PATTERN = /\u001B(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~]|\][^\u0007]*(?:\u0007|\u001B\\))/g;
const CONTROL_CHARACTER_PATTERN = /[\u0000-\u001F\u007F-\u009F]/g;
const ORPHAN_TERMINAL_SEQUENCE_PATTERN = /^(?:\[(?:[ABCDHFIOZ]|\d+(?:;\d+)*[~ABCDHF])|O[ABCDHF])$/;
const ORPHAN_MOUSE_SEQUENCE_PATTERN = /^(?:\[?<\d*(?:;\d*){0,2}[mM]?|\[?\d+;\d*(?:;\d*)?[mM]?|\[?M[\s\S]{0,3})$/;
const ALLOWED_NAME_INPUT_PATTERN = /[^a-zA-Z0-9 _\-/.:[\]()#]/g;

function sanitizeNameInput(input: string): string {
	const cleaned = input
		.replace(ANSI_ESCAPE_PATTERN, '')
		.replace(CONTROL_CHARACTER_PATTERN, '');

	if (ORPHAN_TERMINAL_SEQUENCE_PATTERN.test(cleaned) || ORPHAN_MOUSE_SEQUENCE_PATTERN.test(cleaned)) {
		return '';
	}

	return cleaned.replace(ALLOWED_NAME_INPUT_PATTERN, '');
}

type Mode = 'browse' | 'pick-program' | 'enter-name' | 'pick-worktree' | 'confirm-kill' | 'confirm-merge' | 'help';

interface AppProps {
	repoRoot: string;
	cwd: string;
	initialSelectedId?: string;
	initialActiveTab?: RightPaneTab;
	initialSidebarWidth?: number;
	onSelectedIdChange?: (sessionId: string | undefined) => void;
	onActiveTabChange?: (tab: RightPaneTab) => void;
	onSidebarWidthChange?: (width: number) => void;
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
	return [...sessions].sort((a, b) => {
		if (a.status !== b.status) {
			if (a.status === 'running') {
				return -1;
			}
			if (b.status === 'running') {
				return 1;
			}
			if (a.status === 'starting') {
				return -1;
			}
			if (b.status === 'starting') {
				return 1;
			}
		}
		return a.createdAt.localeCompare(b.createdAt);
	});
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

function CreatePane({
	mode,
	programIndex,
	draftName,
	worktreeMode,
	width,
}: {
	mode: 'pick-program' | 'enter-name';
	programIndex: number;
	draftName: string;
	worktreeMode: WorktreeMode;
	width: number;
}) {
	return (
		<Box flexDirection="column" width={width} borderStyle="round" borderColor={THEME.borderActive} paddingX={1} paddingY={0}>
			<Text color={THEME.accent} bold>
				{mode === 'pick-program'
					? 'New session'
					: `New ${PROGRAMS[programIndex]!.label} session`}
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
					</>
				) : (
					<>
						<Text>Name: <Text color={draftName ? THEME.active : THEME.muted}>{draftName || '█'}</Text></Text>
						<Text>Workspace: <Text color={THEME.accent}>{WORKTREE_MODES.find(item => item.key === worktreeMode)?.label}</Text></Text>
					</>
				)}
			</Box>
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

function MergeConfirmPane({session, selectedIndex, width}: {session?: SessionRecord; selectedIndex: number; width: number}) {
	const options = ['Merge into current branch without committing', 'Squash merge into current branch without committing', 'Cancel'];
	const contentWidth = Math.max(1, width - 4);
	return (
		<Box flexDirection="column" width={width} borderStyle="round" borderColor={THEME.borderActive} paddingX={1}>
			<Text color={THEME.accent} bold>Merge {session ? `"${session.title}"` : 'worktree'}?</Text>
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
		</Box>
	);
}

function KillConfirmPane({session, selectedIndex, canDelete, canDeleteBranch, force, width}: {session?: SessionRecord; selectedIndex: number; canDelete: boolean; canDeleteBranch: boolean; force: boolean; width: number}) {
	const options = canDelete
		? ['Kill only, keep worktree', 'Kill and delete worktree', 'Cancel', ...(canDeleteBranch ? ['Delete worktree and branch'] : [])]
		: ['Kill session', 'Cancel'];
	const contentWidth = Math.max(1, width - 4);
	return (
		<Box flexDirection="column" width={width} borderStyle="round" borderColor={THEME.borderDanger} paddingX={1}>
			<Text color={THEME.error} bold>{force ? 'Force kill' : 'Kill'} {session ? `"${session.title}"` : 'session'}?</Text>
			{session?.worktree?.path ? (
				<Text color={THEME.muted}>{truncate(compactPath(session.worktree.path, contentWidth), contentWidth)}</Text>
			) : null}
			<Box marginTop={1} flexDirection="column">
				{options.map((option, index) => {
					const selected = index === selectedIndex;
					const isCancel = option === 'Cancel';
					const color = selected ? (isCancel ? THEME.muted : THEME.error) : undefined;
					return (
						<Text key={option} inverse={selected} color={color} bold={selected}>
							{selected ? '›' : ' '} {option}
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

function HelpPane({width}: {width: number}) {
	const rows: Array<[string, string]> = [
		['tab', 'cycle Preview / Terminal / Git / Dev'],
		['o', 'attach active pane'],
		['Ctrl+Space', 'return from attach'],
		['n', 'new session'],
		['j/k', 'move selection'],
		['h/l', 'resize sidebar'],
		['m', 'merge selected worktree into current branch'],
		['x / X', 'kill running session / force kill'],
		['s', 'restart exited session'],
		['d', 'start/stop dev command'],
		['backspace', 'remove exited session'],
		['r', 'refresh sessions'],
		['q', 'quit'],
		['esc/?', 'close help'],
	];
	return (
		<Box flexDirection="column" width={width} borderStyle="round" borderColor={THEME.borderActive} paddingX={1}>
			<Text color={THEME.accent} bold>Keyboard shortcuts</Text>
			<Box marginTop={1} flexDirection="column">
				{rows.map(([key, description]) => (
					<Text key={key}>
						<Text color={THEME.active} bold>{key.padEnd(12)}</Text>
						<Text color={THEME.muted}>{description}</Text>
					</Text>
				))}
			</Box>
		</Box>
	);
}

function footerHint(mode: Mode, activeTab: RightPaneTab, session?: SessionRecord): string {
	if (mode === 'browse') {
		const attach = session?.status === 'running' ? 'o attach' : undefined;
		const lifecycle = session?.status === 'exited' ? 's restart • backspace remove' : session?.status === 'running' ? 'x kill • X force kill' : undefined;
		const dev = session?.status === 'running' ? 'd dev' : undefined;
		const merge = session?.worktree?.path && session.worktree.mode !== 'none' ? 'm merge' : undefined;
		return ['tab pane', attach, dev, merge, 'j/k move', 'h/l resize', lifecycle, '? help', 'q quit'].filter(Boolean).join(' • ');
	}
	if (mode === 'pick-program') {
		return 'enter continue • esc cancel • j/k switch';
	}
	if (mode === 'enter-name') {
		return 'tab worktree mode • enter create • esc back • backspace delete';
	}
	if (mode === 'pick-worktree') {
		return 'type search • enter select • esc back • ↑↓ move • backspace delete';
	}
	if (mode === 'confirm-kill' || mode === 'confirm-merge') {
		return 'enter choose • esc cancel • j/k move';
	}
	return `${activeTab} shortcuts • esc/? close`;
}

export function App({repoRoot, cwd, initialSelectedId, initialActiveTab, initialSidebarWidth, onSelectedIdChange, onActiveTabChange, onSidebarWidthChange}: AppProps) {
	const {exit} = useApp();
	const [mode, setMode] = useState<Mode>('browse');
	const [sessions, setSessions] = useState<SessionRecord[]>([]);
	const [selectedId, setSelectedId] = useState<string | undefined>(initialSelectedId);
	const [programIndex, setProgramIndex] = useState(0);
	const [draftName, setDraftName] = useState('');
	const [worktreeMode, setWorktreeMode] = useState<WorktreeMode>('none');
	const [worktrees, setWorktrees] = useState<WorktreeInfoRecord[]>([]);
	const [worktreeQuery, setWorktreeQuery] = useState('');
	const [worktreeIndex, setWorktreeIndex] = useState(0);
	const [killConfirmIndex, setKillConfirmIndex] = useState(0);
	const [killConfirmForce, setKillConfirmForce] = useState(false);
	const [mergeConfirmIndex, setMergeConfirmIndex] = useState(0);
	const [activeTab, setActiveTab] = useState<RightPaneTab>(initialActiveTab ?? 'preview');
	const [preview, setPreview] = useState<PreviewRecord>(EMPTY_PREVIEW);
	const [terminal, setTerminal] = useState<TerminalRecord>(EMPTY_TERMINAL);
	const [git, setGit] = useState<GitRecord>(EMPTY_GIT);
	const [dev, setDev] = useState<DevRecord>(EMPTY_DEV);
	const [error, setError] = useState<string | undefined>();
	const [statusMessage, setStatusMessage] = useState<string | undefined>();
	const [busy, setBusy] = useState(false);
	const [client, setClient] = useState<LiveClient | undefined>();
	const [connectionEpoch, setConnectionEpoch] = useState(0);
	const [terminalSize, setTerminalSize] = useState<TerminalSize>(getTerminalSize());
	const [sidebarWidthOverride, setSidebarWidthOverride] = useState<number | undefined>(initialSidebarWidth);
	const [spinnerIndex, setSpinnerIndex] = useState(0);
	const selectedIdRef = useRef<string | undefined>(selectedId);
	const sessionsRef = useRef<SessionRecord[]>(sessions);

	useEffect(() => {
		selectedIdRef.current = selectedId;
		onSelectedIdChange?.(selectedId);
	}, [onSelectedIdChange, selectedId]);

	useEffect(() => {
		sessionsRef.current = sessions;
	}, [sessions]);

	useEffect(() => {
		onActiveTabChange?.(activeTab);
	}, [activeTab, onActiveTabChange]);

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
		session => session.status === 'starting' || (session.status === 'running' && session.agentStatus === 'active'),
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
						if (session.repoRoot !== repoRoot) {
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
				setError(undefined);
			} catch (nextError) {
				if (!cancelled) {
					setError(nextError instanceof Error ? nextError.message : String(nextError));
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
		setSelectedId(currentId => {
			if (sessions.length === 0) {
				return currentId;
			}
			if (currentId && sessions.some(session => session.id === currentId)) {
				return currentId;
			}
			return sessions[0]?.id;
		});
	}, [sessions]);

	const selectedIndex = useMemo(() => {
		if (!selectedId) {
			return 0;
		}
		const index = sessions.findIndex(session => session.id === selectedId);
		return index >= 0 ? index : 0;
	}, [selectedId, sessions]);

	const selectedSession = sessions[selectedIndex];
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
		selectedSession?.worktree?.mode === 'managed' &&
		selectedSession.worktree.branch &&
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
		const footerLines = error ? 3 : 2;
		const contentHeight = Math.max(8, totalHeight - 2 - footerLines);
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
	}, [error, sidebarWidthOverride, terminalSize.cols, terminalSize.rows]);

	const moveSelection = useCallback(
		(delta: number) => {
			if (sessions.length === 0) {
				return;
			}
			const nextIndex = (selectedIndex + delta + sessions.length) % sessions.length;
			setSelectedId(sessions[nextIndex]?.id);
		},
		[selectedIndex, sessions],
	);

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
			.watchPreview(selectedId, layout.previewCols, layout.previewRows)
			.then(nextPreview => {
				if (cancelled) {
					return;
				}
				if (nextPreview.sessionId && nextPreview.sessionId !== selectedId) {
					return;
				}
				setPreview(nextPreview);
			})
			.catch(nextError => {
				if (!cancelled) {
					setError(nextError instanceof Error ? nextError.message : String(nextError));
				}
			});
		return () => {
			cancelled = true;
		};
	}, [client, layout.previewCols, layout.previewRows, selectedId]);

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
					setError(nextError instanceof Error ? nextError.message : String(nextError));
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
					setError(nextError instanceof Error ? nextError.message : String(nextError));
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
					setError(nextError instanceof Error ? nextError.message : String(nextError));
				}
			});
		return () => {
			cancelled = true;
		};
	}, [activeTab, client, layout.previewCols, layout.previewRows, selectedId]);

	const toggleDevSelected = useCallback(async () => {
		if (!client || !selectedSession || selectedSession.status !== 'running') {
			return;
		}
		setBusy(true);
		setError(undefined);
		try {
			if (dev.sessionId === selectedSession.id && dev.live) {
				await client.stopDev(selectedSession.id);
				setDev({...EMPTY_DEV, sessionId: selectedSession.id, cwd: selectedSession.cwd});
			} else {
				const nextDev = await client.startDev(selectedSession.id, layout.previewCols, layout.previewRows);
				setDev(nextDev);
				setActiveTab('dev');
			}
		} catch (nextError) {
			setError(nextError instanceof Error ? nextError.message : String(nextError));
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
			const created = await client.createSession({
				title,
				program: PROGRAMS[programIndex]!.key,
				cwd,
				repoRoot,
				cols: layout.previewCols,
				rows: layout.previewRows,
				worktreeMode,
				existingWorktreePath,
			});
			setDraftName('');
			setWorktreeMode('none');
			setMode('browse');
			setSelectedId(created.id);
			setSessions(current => upsertSession(current, created));
		} catch (nextError) {
			setError(nextError instanceof Error ? nextError.message : String(nextError));
		} finally {
			setBusy(false);
		}
	}, [client, cwd, draftName, layout.previewCols, layout.previewRows, programIndex, repoRoot, worktreeMode]);

	const killSelected = useCallback(async (deleteWorktree = false, deleteBranch = false, force = false) => {
		if (!client || !selectedSession || selectedSession.status !== 'running') {
			return;
		}
		setBusy(true);
		setError(undefined);
		try {
			const killedSessionId = selectedSession.id;
			await client.killSession(killedSessionId, deleteWorktree || deleteBranch, deleteBranch, force);
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
			setError(nextError instanceof Error ? nextError.message : String(nextError));
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
			setError(nextError instanceof Error ? nextError.message : String(nextError));
		} finally {
			setBusy(false);
		}
	}, [client, selectedSession]);

	const restartSelected = useCallback(async () => {
		if (!client || !selectedSession || selectedSession.status !== 'exited') {
			return;
		}
		setBusy(true);
		setError(undefined);
		try {
			const restarted = await client.restartSession(selectedSession.id, layout.previewCols, layout.previewRows);
			setSelectedId(restarted.id);
			setSessions(current => upsertSession(current, restarted));
		} catch (nextError) {
			setError(nextError instanceof Error ? nextError.message : String(nextError));
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
			setStatusMessage(`${mergeMode === 'squash' ? 'Squash applied' : 'Merge applied without commit'} from ${result.sourceRef} into ${result.targetBranch}`);
		} catch (nextError) {
			setError(nextError instanceof Error ? nextError.message : String(nextError));
		} finally {
			setBusy(false);
		}
	}, [client, cwd, selectedSession]);

	useInput((input, key) => {
		if (busy) {
			return;
		}

		if (mode === 'help') {
			if (key.escape || input === '?') {
				setMode('browse');
			}
			return;
		}

		if (mode === 'browse') {
			if (input === 'q') {
				exit({kind: 'quit'} satisfies UiExitResult);
				return;
			}
			if (input === '?') {
				setMode('help');
				return;
			}
			if (input === 'n') {
				setProgramIndex(0);
				setDraftName('');
				setWorktreeMode('none');
				setMode('pick-program');
				return;
			}
			if (input === 'r') {
				void refreshSessions().catch(nextError =>
					setError(nextError instanceof Error ? nextError.message : String(nextError)),
				);
				return;
			}
			if (key.tab) {
				setActiveTab(tab => (tab === 'preview' ? 'terminal' : tab === 'terminal' ? 'git' : tab === 'git' ? 'dev' : 'preview'));
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
			if (input === 'm' && selectedSession?.worktree?.path && selectedSession.worktree.mode !== 'none') {
				setMergeConfirmIndex(0);
				setMode('confirm-merge');
				return;
			}
			if ((input === 'x' || input === 'X') && selectedSession?.status === 'running') {
				const force = input === 'X';
				if (selectedSession.worktree?.path && selectedSession.worktree.mode !== 'none') {
					setKillConfirmIndex(0);
					setKillConfirmForce(force);
					setMode('confirm-kill');
				} else {
					void killSelected(false, false, force);
				}
				return;
			}
			if (input === 'd' && selectedSession?.status === 'running') {
				void toggleDevSelected();
				return;
			}
			if ((key.backspace || key.delete) && selectedSession?.status === 'exited') {
				void removeSelected();
				return;
			}
			if (input === 's' && selectedSession?.status === 'exited') {
				void restartSelected();
				return;
			}
			if (input === 'o' && selectedSession?.status === 'running') {
				if (activeTab === 'dev' && !(dev.sessionId === selectedSession.id && dev.live)) {
					setError('start the dev command with d before attaching');
					return;
				}
				exit({
					kind: 'attach',
					sessionId: selectedSession.id,
					target: activeTab === 'terminal' ? 'terminal' : activeTab === 'git' ? 'git' : activeTab === 'dev' ? 'dev' : 'agent',
					title: selectedSession.title,
					cwd: selectedSession.cwd,
				} satisfies UiExitResult);
			}
			return;
		}

		if (mode === 'pick-program') {
			if (key.escape) {
				setMode('browse');
				return;
			}
			if (key.leftArrow || key.upArrow || input === 'k' || input === 'h') {
				setProgramIndex(index => (index - 1 + PROGRAMS.length) % PROGRAMS.length);
				return;
			}
			if (key.rightArrow || key.downArrow || input === 'j' || input === 'l') {
				setProgramIndex(index => (index + 1) % PROGRAMS.length);
				return;
			}
			if (key.return) {
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
					setBusy(true);
					void client
						.listWorktrees(cwd)
						.then(items => {
							setWorktrees(items);
							setWorktreeQuery('');
							setWorktreeIndex(0);
							setMode('pick-worktree');
						})
						.catch(nextError => setError(nextError instanceof Error ? nextError.message : String(nextError)))
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
			const optionCount = selectedCanDeleteWorktree ? (selectedCanDeleteBranch ? 4 : 3) : 2;
			if (key.escape) {
				setMode('browse');
				return;
			}
			if (key.upArrow || input === 'k') {
				setKillConfirmIndex(index => (index - 1 + optionCount) % optionCount);
				return;
			}
			if (key.downArrow || input === 'j') {
				setKillConfirmIndex(index => (index + 1) % optionCount);
				return;
			}
			if (key.return) {
				if (selectedCanDeleteWorktree) {
					if (killConfirmIndex === 0) void killSelected(false, false, killConfirmForce);
					else if (killConfirmIndex === 1) void killSelected(true, false, killConfirmForce);
					else if (killConfirmIndex === 3 && selectedCanDeleteBranch) void killSelected(true, true, killConfirmForce);
					else setMode('browse');
				} else {
					if (killConfirmIndex === 0) void killSelected(false, false, killConfirmForce);
					else setMode('browse');
				}
				return;
			}
		}
	});

	return (
		<Box flexDirection="column">
			<Box justifyContent="space-between" width={terminalSize.cols}>
				<Text color={THEME.accent} bold>deckhand</Text>
				<Text color={connectionColor(client)}>● {describeConnection(client)}</Text>
			</Box>
			<Box justifyContent="space-between" width={terminalSize.cols}>
				<Text color={THEME.muted}>{truncate(compactPath(repoRoot, Math.max(10, terminalSize.cols - 16)), Math.max(10, terminalSize.cols - 16))}</Text>
				<Text color={THEME.muted}>{sessions.length} session{sessions.length === 1 ? '' : 's'}</Text>
			</Box>
			<Box flexDirection="row">
				<Sidebar
					sessions={sessions}
					selectedId={selectedSession?.id}
					width={layout.sidebarWidth}
					height={layout.contentHeight}
					spinnerFrame={spinnerFrame}
				/>
				<Box width={1} />
				{mode === 'browse' ? (
					<Box
						flexDirection="column"
						width={layout.previewWidth}
						height={layout.contentHeight}
						borderStyle="round"
						borderColor={THEME.border}
						paddingX={1}
					>
						<TabBar activeTab={activeTab} width={layout.paneInnerWidth} />
						<Box height={1} />
						{activeTab === 'preview' ? (
							<PreviewPane
								session={selectedSession}
								preview={preview}
								width={layout.paneInnerWidth}
								height={layout.paneInnerHeight}
								spinnerFrame={spinnerFrame}
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
						) : (
							<DevPane session={selectedSession} dev={dev} width={layout.paneInnerWidth} height={layout.paneInnerHeight} />
						)}
					</Box>
				) : mode === 'help' ? (
					<HelpPane width={layout.previewWidth} />
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
						selectedIndex={killConfirmIndex}
						canDelete={selectedCanDeleteWorktree}
						canDeleteBranch={selectedCanDeleteBranch}
						force={killConfirmForce}
						width={layout.previewWidth}
					/>
				) : mode === 'confirm-merge' ? (
					<MergeConfirmPane session={selectedSession} selectedIndex={mergeConfirmIndex} width={layout.previewWidth} />
				) : (
					<CreatePane
						mode={mode}
						programIndex={programIndex}
						draftName={draftName}
						worktreeMode={worktreeMode}
						width={layout.previewWidth}
					/>
				)}
			</Box>
			<Text color={THEME.muted}>{footerHint(mode, activeTab, selectedSession)}</Text>
			{busy ? <Text color={THEME.warn}>Working…</Text> : null}
			{statusMessage ? <Text color={THEME.success}>{statusMessage}</Text> : null}
			{error ? <Text color={THEME.error}>Error: {error}</Text> : null}
		</Box>
	);
}
