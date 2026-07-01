import React from 'react';
import {Box, Text} from 'ink';
import type {SessionRecord} from './types.js';
import {countHiddenSessionDescendants, countSessionDescendants, sessionDepth, sessionHasChildren} from './sessionOrder.js';
import {THEME, displaySessionTitle, programGlyph, statusColor, statusGlyph, truncate} from './ui.js';

interface SidebarProps {
	sessions: SessionRecord[];
	allSessions?: SessionRecord[];
	selectedId?: string;
	width: number;
	height: number;
	spinnerFrame: string;
	collapsedSessionIds?: ReadonlySet<string>;
	hiddenSessionIds?: ReadonlySet<string>;
}

function visibleSessions(sessions: SessionRecord[], selectedIndex: number, availableRows: number): SessionRecord[] {
	if (availableRows <= 0 || sessions.length <= availableRows) return sessions;
	const half = Math.floor(availableRows / 2);
	let start = Math.max(0, selectedIndex - half);
	const maxStart = Math.max(0, sessions.length - availableRows);
	if (start > maxStart) start = maxStart;
	return sessions.slice(start, start + availableRows);
}

interface RowParts {
	main: string;
	suffix: string;
}

function subtleCount(count: number): string {
	return `+${count}`;
}

function renderRow(
	session: SessionRecord,
	allSessions: SessionRecord[],
	index: number,
	indexWidth: number,
	active: boolean,
	width: number,
	spinnerFrame: string,
	collapsedSessionIds: ReadonlySet<string>,
	hiddenSessionIds: ReadonlySet<string>,
): RowParts {
	const cursor = active ? '›' : ' ';
	const idxText = String(index);
	const idx = `[${idxText}]`;
	const idxPadding = ' '.repeat(Math.max(0, indexWidth - idxText.length));
	const devGlyph = session.devRunning ? '▶ ' : '';
	const mergedGlyph = session.worktree?.mergedAt ? '✓' : '';
	const forkGlyph = session.subSessionKind === 'forked' ? '⑂ ' : session.subSessionKind === 'clean' ? '↳ ' : '';
	const depth = sessionDepth(session, allSessions);
	const indent = '  '.repeat(Math.min(depth, 4));
	const hasChildren = sessionHasChildren(session.id, allSessions);
	const collapsed = collapsedSessionIds.has(session.id);
	const branchGlyph = hasChildren ? (collapsed ? '▸ ' : '▾ ') : '  ';
	const childCount = collapsed && hasChildren
		? countSessionDescendants(session.id, allSessions)
		: countHiddenSessionDescendants(session.id, allSessions, hiddenSessionIds);
	const suffix = [mergedGlyph, childCount > 0 ? subtleCount(childCount) : ''].filter(Boolean).join(' ');
	const glyph = `${statusGlyph(session, spinnerFrame)} ${devGlyph}${programGlyph(session.program)}`;
	const prefix = `${cursor} ${idx}${idxPadding} ${indent}${branchGlyph}${forkGlyph}${glyph} `;
	const titleSpace = Math.max(0, width - prefix.length - suffix.length);
	const title = truncate(displaySessionTitle(session, allSessions), titleSpace);
	const filled = `${prefix}${title}`;
	if (!suffix) {
		return {main: filled.length >= width ? truncate(filled, width) : filled + ' '.repeat(width - filled.length), suffix: ''};
	}
	if (filled.length + suffix.length >= width) {
		if (suffix.length >= width) {
			return {main: truncate(`${filled}${suffix}`, width), suffix: ''};
		}
		const mainWidth = width - suffix.length;
		const main = truncate(filled, mainWidth);
		return {main: main.length >= mainWidth ? main : main + ' '.repeat(mainWidth - main.length), suffix};
	}
	return {main: filled + ' '.repeat(width - filled.length - suffix.length), suffix};
}

export function Sidebar({sessions, allSessions = sessions, selectedId, width, height, spinnerFrame, collapsedSessionIds = new Set<string>(), hiddenSessionIds = new Set<string>()}: SidebarProps) {
	const selectedIndex = Math.max(0, sessions.findIndex(session => session.id === selectedId));
	const contentWidth = Math.max(1, width - 4);
	const rowsForSessions = Math.max(1, height - 3);
	const visible = visibleSessions(sessions, selectedIndex, rowsForSessions);
	const visibleStart = Math.max(0, sessions.indexOf(visible[0] ?? sessions[0]));
	const indexWidth = String(Math.max(1, sessions.length)).length;

	return (
		<Box flexDirection="column" width={width} height={height} borderStyle="round" borderColor={THEME.border} paddingX={1}>
			<Box justifyContent="space-between" width={contentWidth}>
				<Text color={THEME.accent} bold>Sessions</Text>
				<Text color={THEME.muted}>{sessions.length === allSessions.length ? sessions.length : `${sessions.length}/${allSessions.length}`}</Text>
			</Box>
			{sessions.length === 0 ? (
				<Box flexDirection="column" marginTop={1}>
					<Text color={THEME.muted}>{truncate('No sessions yet.', contentWidth)}</Text>
					<Text color={THEME.active}>{truncate('Press n to create.', contentWidth)}</Text>
				</Box>
			) : (
				visible.map((session, index) => {
					const actualIndex = visibleStart + index + 1;
					const active = session.id === selectedId;
					const row = renderRow(session, allSessions, actualIndex, indexWidth, active, contentWidth, spinnerFrame, collapsedSessionIds, hiddenSessionIds);
					return (
						<Box key={session.id} width={contentWidth}>
							<Text
								inverse={active}
								color={active ? THEME.active : statusColor(session)}
								bold={active}
							>
								{row.main}
							</Text>
							{row.suffix ? (
								<Text inverse={active} color={active ? THEME.active : THEME.muted} dimColor={!active}>
									{row.suffix}
								</Text>
							) : null}
						</Box>
					);
				})
			)}
		</Box>
	);
}
