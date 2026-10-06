import React from 'react';
import {Box, Text} from 'ink';
import type {ProgramKey, SessionRecord} from './types.js';
import type {SessionFilter} from './sessionFeatures.js';
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
}

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

function partStyle(part: RowPart, row: SidebarRow): {color?: string; dimColor?: boolean} {
	// The selected row is one inverse highlight; a dimmed row is muted throughout, ▣ kept readable.
	if (row.selected) return {};
	if (row.dimmed) return part.role === 'archived' ? {color: THEME.muted} : {color: THEME.muted, dimColor: true};
	switch (part.role) {
		case 'gutter': return {color: THEME.active, dimColor: true};
		case 'number': case 'archived': case 'merged': case 'count': case 'outdated': return {color: THEME.muted, dimColor: true};
		case 'dev': return {color: THEME.success};
		case 'cleanup': return {color: THEME.error};
		case 'agent': return {color: THEME.muted};
		case 'tree': case 'status': case 'title': return {color: row.color};
		default: return {};
	}
}

export function Sidebar({sessions, allSessions = sessions, selectedId, width, height, spinnerFrame, collapsedSessionIds = new Set<string>(), hiddenSessionIds = new Set<string>(), loaded = true, filter = 'active', query = '', now = Date.now(), installedVersions}: SidebarProps) {
	const selectedIndex = Math.max(0, sessions.findIndex(session => session.id === selectedId));
	const contentWidth = Math.max(1, width - 4);
	// Rows start in the left padding column: it holds the cursor (›) and the shared-workspace marker.
	const rowWidth = contentWidth + 1;
	const rowsForSessions = Math.max(1, height - 3);
	const visible = visibleSessions(sessions, selectedIndex, rowsForSessions);
	const visibleStart = Math.max(0, sessions.indexOf(visible[0] ?? sessions[0]));
	const [emptyTitle, emptyHint] = emptyMessage(allSessions, loaded);
	const header = sidebarHeader({width: contentWidth, filter, query, shown: sessions.length, total: allSessions.length, allSessions});
	const rows = sidebarRows({
		rows: visible, allSessions, firstNumber: visibleStart + 1, numberWidth: String(Math.max(1, sessions.length)).length,
		selectedId, width: rowWidth, spinnerFrame, filter, collapsedSessionIds, hiddenSessionIds, installedVersions,
	});
	// The list has priority: the details block only takes the rows it leaves free.
	const details = sessions.length ? sessionDetails(sessions.find(session => session.id === selectedId), allSessions, contentWidth, rowsForSessions - visible.length, now, installedVersions) : [];

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
				rows.map(row => (
					<Box key={row.id} width={rowWidth}>
						<Text wrap="truncate-end" inverse={row.selected} color={row.selected ? THEME.active : undefined} bold={row.selected}>
							{row.parts.map((part, index) => <Text key={index} {...partStyle(part, row)}>{part.text}</Text>)}
						</Text>
					</Box>
				))
			)}
			{details.length ? <>
				<Box flexGrow={1} />
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
