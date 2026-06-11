import React from 'react';
import {Box, Text} from 'ink';
import type {SessionRecord} from './types.js';
import {THEME, fitLines} from './ui.js';

interface NotesPaneProps {
	session?: SessionRecord;
	notes: string;
	width: number;
	height: number;
	focused?: boolean;
}

function fitEditableNotes(text: string, width: number, height: number): string[] {
	const cursor = '▌';
	const rawLines = text.split('\n');
	rawLines[rawLines.length - 1] = `${rawLines[rawLines.length - 1] ?? ''}${cursor}`;
	const visibleLines = rawLines.slice(-height);
	const lines = visibleLines.map((line, index) => {
		const isCursorLine = index === visibleLines.length - 1;
		if (line.length <= width) return line;
		if (!isCursorLine) return width === 1 ? line.slice(0, 1) : `${line.slice(0, width - 1)}…`;
		return width <= 1 ? cursor : `…${line.slice(-(width - 1))}`;
	});
	return lines.length >= height ? lines : [...lines, ...Array.from({length: height - lines.length}, () => ' ')];
}

export function NotesPane({session, notes, width, height, focused = false}: NotesPaneProps) {
	const header = session ? (focused ? 'notes edit · esc stop editing' : 'notes · press o to edit') : 'No session selected.';
	const body = session ? notes || 'No notes yet.' : 'Select a session from the sidebar.';
	const lines = focused && session ? fitEditableNotes(notes, width, Math.max(1, height - 1)) : fitLines(body, width, Math.max(1, height - 1));
	const empty = !notes && !focused;

	return (
		<Box flexDirection="column" width={width} height={height}>
			<Text color={focused ? THEME.active : THEME.muted}>{header}</Text>
			{lines.map((line, index) => (
				<Text key={`notes-line-${index}`} color={empty ? THEME.muted : undefined}>
					{line || ' '}
				</Text>
			))}
		</Box>
	);
}
