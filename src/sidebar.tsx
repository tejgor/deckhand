import React from 'react';
import {Box, Text} from 'ink';
import type {SessionRecord} from './types.js';
import {THEME, programGlyph, statusColor, statusGlyph, truncate} from './ui.js';

interface SidebarProps {
	sessions: SessionRecord[];
	selectedId?: string;
	width: number;
	height: number;
	spinnerFrame: string;
}

function visibleSessions(sessions: SessionRecord[], selectedIndex: number, availableRows: number): SessionRecord[] {
	if (availableRows <= 0 || sessions.length <= availableRows) return sessions;
	const half = Math.floor(availableRows / 2);
	let start = Math.max(0, selectedIndex - half);
	const maxStart = Math.max(0, sessions.length - availableRows);
	if (start > maxStart) start = maxStart;
	return sessions.slice(start, start + availableRows);
}

function renderRow(session: SessionRecord, index: number, active: boolean, width: number, spinnerFrame: string): string {
	const cursor = active ? '›' : ' ';
	const idx = `[${index}]`;
	const devGlyph = session.devRunning ? ' ▹' : '';
	const glyph = `${statusGlyph(session, spinnerFrame)} ${programGlyph(session.program)}${devGlyph}`;
	const prefix = `${cursor} ${idx} ${glyph} `;
	const titleSpace = Math.max(0, width - prefix.length);
	const title = truncate(session.title, titleSpace);
	const filled = `${prefix}${title}`;
	if (filled.length >= width) return truncate(filled, width);
	return filled + ' '.repeat(width - filled.length);
}

export function Sidebar({sessions, selectedId, width, height, spinnerFrame}: SidebarProps) {
	const selectedIndex = Math.max(0, sessions.findIndex(session => session.id === selectedId));
	const contentWidth = Math.max(1, width - 4);
	const rowsForSessions = Math.max(1, height - 3);
	const visible = visibleSessions(sessions, selectedIndex, rowsForSessions);
	const visibleStart = Math.max(0, sessions.indexOf(visible[0] ?? sessions[0]));

	return (
		<Box flexDirection="column" width={width} height={height} borderStyle="round" borderColor={THEME.border} paddingX={1}>
			<Box justifyContent="space-between" width={contentWidth}>
				<Text color={THEME.accent} bold>Sessions</Text>
				<Text color={THEME.muted}>{sessions.length}</Text>
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
					return (
						<Text
							key={session.id}
							inverse={active}
							color={active ? THEME.active : statusColor(session)}
							bold={active}
						>
							{renderRow(session, actualIndex, active, contentWidth, spinnerFrame)}
						</Text>
					);
				})
			)}
		</Box>
	);
}
