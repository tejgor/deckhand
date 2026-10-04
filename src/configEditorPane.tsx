import React, {memo, useMemo, useRef} from 'react';
import {Box, Text} from 'ink';
import {editorLines, type EditorState} from './textEditor.js';
import {DetailsPane} from './detailsPane.js';
import type {ConfigTargets, ProjectConfigDocument} from './types.js';
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
	const inner = Math.max(1, width - 4), rows = Math.max(1, height - 8);
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
				{line ? visible(line.before) : ' '}{line?.cursor !== undefined ? <Text inverse bold>{visible(line.cursor)}</Text> : null}{line ? visible(line.after) : ''}
			</Text>;
		})}
		<Text color={error || state.message ? THEME.error : THEME.muted}>{truncate(visible(error ?? state.message ?? 'Save validates JSON/schema; it never trusts or runs commands.'), inner)}</Text>
		<Text color={THEME.muted}>{truncate('Ctrl+S save · Ctrl+F format · Ctrl+A select all', inner)}</Text>
		<Text color={THEME.muted}>{truncate('arrows/home/end edit · enter newline · esc cancel', inner)}</Text>
		<Text color={THEME.muted}>Line {lineNumber}{state.selectAll ? ' · all selected' : ''}</Text>
	</Box>;
});

function targetText(targets: ConfigTargets, selected: number): string {
	const state = (document: ProjectConfigDocument) => document.exists ? 'edit existing' : 'create (starter draft)';
	const lines = [
		`${selected === 0 ? '›' : ' '} Global defaults · all repositories, never needs trust · ${targets.global ? state(targets.global) : 'unavailable'}`,
		`  ${targets.global ? `${targets.global.path} ("defaults")` : targets.globalError}`, '',
		`${selected === 1 ? '›' : ' '} Repository · deckhand.json in the main checkout, applies once trusted · ${targets.repository ? state(targets.repository) : 'unavailable'}`,
		`  ${targets.repository?.path ?? targets.repositoryError}`, '',
		`${selected === 2 ? '›' : ' '} Worktree setup · where new worktrees go, their branch, the creation hook and which untracked files are linked`,
		'  Edits the worktree section of the repository file or global defaults (t toggles), with suggestions from the main checkout', '',
		'The repository file overrides global defaults field by field (actions merge by name; worktree per field).',
		'Copies of deckhand.json inside linked worktrees are ignored. Nothing is written until Ctrl+S.',
	];
	return lines.join('\n');
}

export function ConfigTargetPane({targets, selected, width, height}: {targets: ConfigTargets; selected: number; width: number; height: number}) {
	return <DetailsPane title="Edit configuration" text={targetText(targets, selected)} footer="j/k/arrows choose · enter open · esc cancel" width={width} height={height} />;
}
