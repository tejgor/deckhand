import React, {memo, useMemo, useRef} from 'react';
import {Box, Text} from 'ink';
import {editorLines, type EditorState} from './textEditor.js';
import type {ProjectConfigDocument} from './types.js';
import {fitHint} from './menu.js';
import {DISPLAY_CONTROL_PATTERN, THEME, compactPath, truncate} from './ui.js';

// One row per editor line: CR/tab become spaces, other controls and line separators '?'.
function visible(text: string): string { return text.replace(/\r/g, ' ').replace(DISPLAY_CONTROL_PATTERN, '?').replace(/[\u2028\u2029]/g, '?').replace(/\t/g, ' '); }

// Scroll only as far as needed to keep the cursor row on screen.
function scrollTopFor(previous: number, cursorRow: number, rows: number, lineCount: number): number {
	let top = previous;
	if (cursorRow < top) top = cursorRow;
	else if (cursorRow >= top + rows) top = cursorRow - rows + 1;
	return Math.max(0, Math.min(top, Math.max(0, lineCount - rows)));
}

export const ConfigEditorPane = memo(function ConfigEditorPane({document, state, error, width, height}: {document: ProjectConfigDocument; state: EditorState; error?: string; width: number; height: number}) {
	const inner = Math.max(1, width - 4), rows = Math.max(1, height - 6);
	const {lines, cursorRow} = useMemo(() => editorLines({text: state.text, cursor: state.cursor}, inner), [state.text, state.cursor, inner]);
	const scrollTop = useRef(0);
	scrollTop.current = scrollTopFor(scrollTop.current, cursorRow, rows, lines.length);
	const start = scrollTop.current;
	const body = lines.slice(start, start + rows);
	const dirty = state.text !== document.raw;
	const lineNumber = useMemo(() => state.text.slice(0, state.cursor).split('\n').length, [state.text, state.cursor]);
	return <Box flexDirection="column" width={width} height={height} borderStyle="round" borderColor={THEME.borderActive} paddingX={1}>
		<Text color={THEME.accent} bold>{truncate(`${document.kind === 'global' ? 'Global defaults · config.json "defaults"' : 'Repository config · deckhand.json'}${!document.exists ? ' · new' : dirty ? ' · unsaved' : ''}`, inner)}</Text>
		<Text color={THEME.muted}>{compactPath(visible(document.path), inner)}</Text>
		{Array.from({length: rows}, (_, index) => {
			const line = body[index];
			return <Text key={index} inverse={state.selectAll}>
				{/* Ink drops an empty row, so a blank line renders as a space. */}{line && (line.before || line.cursor !== undefined || line.after) ? visible(line.before) : ' '}{line?.cursor !== undefined ? <Text inverse bold>{visible(line.cursor)}</Text> : null}{line ? visible(line.after) : ''}
			</Text>;
		})}
		<Text wrap="truncate-end"><Text color={THEME.muted}>Line {lineNumber}{state.selectAll ? ' · all selected' : ''} · </Text><Text color={error || state.message ? THEME.error : THEME.muted}>{visible(error ?? state.message ?? (document.kind === 'repository' ? 'Save validates JSON/schema and never runs anything; a trusted file stays trusted.' : 'Save validates JSON/schema; it never runs commands.'))}</Text></Text>
		<Text color={THEME.muted} wrap="truncate-end">{fitHint(['Ctrl+S save', 'Ctrl+F format', {text: 'Ctrl+A select all', drop: 1}, {text: 'arrows/home/end edit', drop: 3}, {text: 'enter newline', drop: 2}, 'esc cancel'], inner)}</Text>
	</Box>;
});
