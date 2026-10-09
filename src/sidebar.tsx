import React from 'react';
import {Box, Text} from 'ink';
import type {ProgramKey, SessionRecord} from './types.js';
import {sessionNeedsAttention, type SessionFilter} from './sessionFeatures.js';
import type {Task} from './tasks.js';
import {sessionDetails, sidebarHeader, sidebarRows, type RowPart, type SidebarRow} from './sidebarModel.js';
import {THEME, truncate} from './ui.js';

interface SidebarProps {
	sessions: SessionRecord[];
	allSessions?: SessionRecord[];
	selectedId?: string;
	width: number;
	height: number;
	spinnerFrame: string;
	// Empty in filtered/search views, where collapse state is not applied.
	collapsedSessionIds?: ReadonlySet<string>;
	hiddenSessionIds?: ReadonlySet<string>;
	loaded?: boolean;
	filter?: SessionFilter;
	query?: string;
	/** Clock for the details block's age (ms). */
	now?: number;
	/** Installed agent versions, for the outdated marker (↑) and the details line. */
	installedVersions?: Partial<Record<ProgramKey, string>>;
	/** The task a session works on, for the details block. */
	taskOf?: (session: SessionRecord) => Pick<Task, 'title' | 'done'> | undefined;
}

// The most rows the details block can use: a rule, the title (two lines), state, location, task and checklist.
const DETAIL_ROWS = 7;

function visibleSessions(sessions: SessionRecord[], selectedIndex: number, availableRows: number): SessionRecord[] {
	if (availableRows <= 0 || sessions.length <= availableRows) return sessions;
	const half = Math.floor(availableRows / 2);
	let start = Math.max(0, selectedIndex - half);
	const maxStart = Math.max(0, sessions.length - availableRows);
	if (start > maxStart) start = maxStart;
	return sessions.slice(start, start + availableRows);
}

function emptyMessage(allSessions: SessionRecord[], loaded: boolean): [string, string] {
	if (!loaded && allSessions.length === 0) return ['Loading sessions…', ''];
	if (allSessions.length > 0) return ['No sessions match this view.', 'f filter · / search'];
	return ['No sessions yet.', 'Press n to create.'];
}

export function partStyle(part: RowPart, row: SidebarRow): {color?: string; dimColor?: boolean} {
	// The selected row is one inverse highlight; a dimmed row is muted throughout, ▣ kept readable.
	if (row.selected) return {};
	if (row.dimmed) return part.role === 'archived' || part.role === 'done' ? {color: THEME.muted} : {color: THEME.muted, dimColor: true};
	// Done: the title (and tree) muted, not dim; the status glyph keeps its color, so a signal still shows.
	if (row.done && (part.role === 'title' || part.role === 'tree')) return {color: THEME.muted};
	switch (part.role) {
		case 'gutter': return {color: THEME.active, dimColor: true};
		case 'number': case 'archived': case 'merged': case 'count': case 'outdated': return {color: THEME.muted, dimColor: true};
		case 'done': return {color: THEME.muted};
		case 'dev': return {color: THEME.success};
		case 'cleanup': return {color: THEME.error};
		case 'agent': return {color: THEME.muted};
		case 'tree': case 'status': case 'title': return {color: row.color};
		default: return {};
	}
}

/** `↑ 12 more` / `↓ 3 more · ! 1`: sessions out of view on one side of the list, and how many of them need you. */
export function moreText(arrow: string, hidden: SessionRecord[]): string {
	if (!hidden.length) return '';
	const waiting = hidden.filter(session => !session.archivedAt && sessionNeedsAttention(session)).length;
	return `${arrow} ${hidden.length} more${waiting ? ` · ! ${waiting}` : ''}`;
}

function MoreLine({arrow, hidden, width}: {arrow: string; hidden: SessionRecord[]; width: number}) {
	const text = moreText(arrow, hidden);
	const waiting = text.includes('!');
	return <Box marginLeft={1} width={width}><Text wrap="truncate-end" color={waiting ? THEME.warn : THEME.muted}>{text || ' '}</Text></Box>;
}

export function Sidebar({sessions, allSessions = sessions, selectedId, width, height, spinnerFrame, collapsedSessionIds = new Set<string>(), hiddenSessionIds = new Set<string>(), loaded = true, filter = 'active', query = '', now = Date.now(), installedVersions, taskOf}: SidebarProps) {
	const selectedIndex = Math.max(0, sessions.findIndex(session => session.id === selectedId));
	const contentWidth = Math.max(1, width - 4);
	// Rows start in the left padding column: it holds the cursor (›) and the shared-workspace marker.
	const rowWidth = contentWidth + 1;
	const rowsForSessions = Math.max(1, height - 3);
	// A list longer than the sidebar keeps rows for the selected session's details (and scrolls in the rest);
	// a short one leaves them whatever it does not use.
	const selected = sessions.find(session => session.id === selectedId);
	const task = selected && taskOf?.(selected);
	// As many rows as the largest details block of any listed session needs (at most DETAIL_ROWS), so the list keeps
	// its size while the selection moves; sessions with less leave the rest blank.
	const detailRows = sessions.length > rowsForSessions - DETAIL_ROWS && rowsForSessions >= DETAIL_ROWS * 3
		? Math.max(0, ...sessions.map(session => sessionDetails(session, allSessions, contentWidth, DETAIL_ROWS, now, installedVersions, taskOf?.(session)).length)) : 0;
	// A list that scrolls gets a line above and below it saying how many sessions are out of view (and how many of
	// those need you), kept even when empty so the rows don't jump as the selection moves.
	const scrolls = sessions.length > rowsForSessions - detailRows && rowsForSessions - detailRows >= 5;
	const listRows = rowsForSessions - detailRows - (scrolls ? 2 : 0);
	const visible = visibleSessions(sessions, selectedIndex, listRows);
	const visibleStart = Math.max(0, sessions.indexOf(visible[0] ?? sessions[0]));
	const above = sessions.slice(0, visibleStart), below = sessions.slice(visibleStart + visible.length);
	const [emptyTitle, emptyHint] = emptyMessage(allSessions, loaded);
	const header = sidebarHeader({width: contentWidth, filter, query, shown: sessions.length, total: allSessions.length, allSessions});
	const rows = sidebarRows({
		rows: visible, allSessions, firstNumber: visibleStart + 1, numberWidth: String(Math.max(1, sessions.length)).length,
		selectedId, width: rowWidth, spinnerFrame, filter, collapsedSessionIds, hiddenSessionIds, installedVersions,
	});
	// The details take the rows the list leaves free (the ones kept for them when it scrolls).
	const details = sessions.length ? sessionDetails(selected, allSessions, contentWidth, rowsForSessions - visible.length - (scrolls ? 2 : 0), now, installedVersions, task) : [];

	return (
		<Box flexDirection="column" width={width} height={height} borderStyle="round" borderColor={THEME.border} paddingRight={1}>
			<Box justifyContent="space-between" width={contentWidth} marginLeft={1}>
				<Text color={THEME.accent} bold>{header.title}</Text>
				<Text wrap="truncate-end">
					<Text color={header.highlighted ? THEME.active : THEME.muted}>{header.label}</Text>
					{header.attention ? <><Text color={THEME.muted}> · </Text><Text color={THEME.warn} bold>{header.attention}</Text></> : null}
				</Text>
			</Box>
			{sessions.length === 0 ? (
				<Box flexDirection="column" marginTop={1} marginLeft={1}>
					<Text color={THEME.muted}>{truncate(emptyTitle, contentWidth)}</Text>
					<Text color={THEME.active}>{truncate(emptyHint || ' ', contentWidth)}</Text>
				</Box>
			) : (
				<>
					{scrolls ? <MoreLine arrow="↑" hidden={above} width={contentWidth} /> : null}
					{rows.map(row => (
						<Box key={row.id} width={rowWidth}>
							<Text wrap="truncate-end" inverse={row.selected} color={row.selected ? THEME.active : undefined} bold={row.selected}>
								{row.parts.map((part, index) => <Text key={index} {...partStyle(part, row)}>{part.text}</Text>)}
							</Text>
						</Box>
					))}
					{scrolls ? <MoreLine arrow="↓" hidden={below} width={contentWidth} /> : null}
				</>
			)}
			{details.length ? <>
				{/* Under a scrolling list the details sit right below it (their rule never moves); otherwise at the bottom. */}
				{scrolls ? null : <Box flexGrow={1} />}
				<Box flexDirection="column" marginLeft={1} width={contentWidth}>
					{details.map((line, index) => (
						<Text key={index} wrap="truncate-end">
							{line.map((part, at) => <Text key={at} color={part.color} dimColor={part.dim}>{part.text}</Text>)}
						</Text>
					))}
				</Box>
			</> : null}
		</Box>
	);
}
