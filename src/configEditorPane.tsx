import React, {memo, useMemo, useRef} from 'react';
import os from 'node:os';
import {Box, Text} from 'ink';
import {editorLines, type EditorState} from './textEditor.js';
import {DetailsPane} from './detailsPane.js';
import type {ConfigTargetKind, ConfigTargets, EffectiveSettingsInfo, ProjectConfigDocument} from './types.js';
import type {SettingRow} from './projectConfig.js';
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
		`${selected === 3 ? '›' : ' '} Effective settings · what applies here now and which layer sets each value (read-only)`,
		'  Enter on a row opens the file that sets it; T reviews/trusts deckhand.json', '',
		'The repository file overrides global defaults field by field (actions merge by name; worktree per field).',
		'Copies of deckhand.json inside linked worktrees are ignored. Nothing is written until Ctrl+S.',
	];
	return lines.join('\n');
}

export function ConfigTargetPane({targets, selected, width, height}: {targets: ConfigTargets; selected: number; width: number; height: number}) {
	return <DetailsPane title="Edit configuration" text={targetText(targets, selected)} footer="j/k/arrows choose · enter open · esc cancel" width={width} height={height} />;
}

/** The layer Enter opens for a row: the repository file when it sets (or would set) the value, else global defaults. */
export function settingLayer(row: SettingRow): ConfigTargetKind {
	return row.source === 'repo' || (row.source === 'not set' && row.pending) ? 'repository' : 'global';
}
const home = (value: string) => { const dir = os.homedir(); return dir && (value === dir || value.startsWith(`${dir}/`)) ? `~${value.slice(dir.length)}` : value; };
// Pending-only rows (set only by the untrusted repository file) show the pending value in place.
const pendingOnly = (row: SettingRow) => row.source === 'not set' && Boolean(row.pending);
function rowValue(row: SettingRow): string {
	const value = home(pendingOnly(row) ? row.pending!.value : row.value);
	if (!row.entry) return value;
	if (row.key === 'actions') return `${row.entry}  ${value}`;
	if (row.key === 'worktree.files') return `${row.entry} ← ${value}`;
	return row.entry;
}
// Paths keep their distinctive tail; everything else is cut at the end.
const fit = (row: SettingRow, text: string, width: number) => row.key === 'worktree.location' && text.length > width ? `…${text.slice(text.length - width + 1)}` : truncate(text, width);
function repositoryLine(info: EffectiveSettingsInfo): {text: string; color?: string} {
	const {state, error} = info.repository;
	const hook = info.needsReview && state !== 'untrusted' ? ' · creation hook not trusted (T)' : '';
	if (state === 'trusted') return {text: `Repository deckhand.json: trusted ✓${hook}`, color: THEME.success};
	if (state === 'untrusted') return {text: 'Repository deckhand.json: not trusted — repo values ignored until you trust it (T)', color: THEME.warn};
	if (state === 'absent') return {text: `Repository deckhand.json: not present${hook}`};
	if (state === 'bare') return {text: 'Repository deckhand.json: none (bare repository: global defaults only)'};
	if (state === 'none') return {text: error ?? 'Not a Git repository: global defaults only', color: THEME.warn};
	return {text: `Repository deckhand.json: not valid: ${error ?? 'unknown error'}`, color: THEME.error};
}
interface EffectiveLine {text: string; row?: number; color?: string; muted?: boolean}
function effectiveLines(info: EffectiveSettingsInfo, selected: number, inner: number): EffectiveLine[] {
	const lines: EffectiveLine[] = [];
	const keyWidth = 20, sourceWidth = 20, valueWidth = Math.max(10, inner - keyWidth - sourceWidth - 4);
	const indent = ' '.repeat(keyWidth + 3);
	info.rows.forEach((row, index) => {
		const key = index > 0 && info.rows[index - 1]!.key === row.key ? '' : row.key;
		const only = pendingOnly(row);
		lines.push({text: `${index === selected ? '›' : ' '} ${key.padEnd(keyWidth)} ${fit(row, rowValue(row), valueWidth).padEnd(valueWidth)} ${only ? 'repo, pending trust' : row.source}`, row: index, color: only ? THEME.warn : undefined, muted: row.source === 'not set' && !only});
		const raw = only ? row.pending!.raw : row.raw;
		const detail = [typeof raw === 'string' && raw !== (only ? row.pending!.value : row.value) ? `(${raw})` : '', row.note ?? ''].filter(Boolean).join(' · ');
		if (detail) lines.push({text: `${indent}${home(detail)}`, row: index, muted: true});
		if (row.pending && !only) {
			const raw = typeof row.pending.raw === 'string' && row.pending.raw !== row.pending.value ? `  (${row.pending.raw})` : '';
			lines.push({text: `${indent}(repo, pending trust) ${home(row.pending.value)}${raw}`, row: index, color: THEME.warn});
		}
	});
	return lines;
}

export function EffectiveSettingsPane({info, selected, width, height}: {info: EffectiveSettingsInfo; selected: number; width: number; height: number}) {
	const inner = Math.max(1, width - 4);
	const header = [repositoryLine(info), ...info.globalError ? [{text: `Global defaults: not valid: ${info.globalError}`, color: THEME.error}] : []];
	const bodyRows = Math.max(1, height - 5 - header.length);
	const lines = effectiveLines(info, selected, inner);
	const scroll = useRef(0);
	const first = lines.findIndex(line => line.row === selected), last = first < 0 ? -1 : first + lines.slice(first).filter(line => line.row === selected).length - 1;
	if (first >= 0 && first < scroll.current) scroll.current = first;
	else if (last >= scroll.current + bodyRows) scroll.current = last - bodyRows + 1;
	scroll.current = Math.max(0, Math.min(scroll.current, Math.max(0, lines.length - bodyRows)));
	const target = info.rows[selected] ? settingLayer(info.rows[selected]!) : 'global';
	return <Box flexDirection="column" width={width} height={height} borderStyle="round" borderColor={THEME.borderActive} paddingX={1}>
		<Text color={THEME.accent} bold>{truncate(visible(`Effective settings · ${info.repo}`), inner)}</Text>
		{header.map((line, index) => <Text key={`h${index}`} color={line.color}>{truncate(visible(line.text), inner)}</Text>)}
		{Array.from({length: bodyRows}, (_, index) => {
			const line = lines[scroll.current + index];
			return <Text key={index} color={line?.row === selected && !line.muted && !line.color ? THEME.accent : line?.color} bold={line?.row === selected && !line.muted} dimColor={line?.muted}>{line ? truncate(visible(line.text), inner) : ' '}</Text>;
		})}
		<Text color={THEME.muted}>{truncate(`Enter opens ${target === 'repository' ? 'deckhand.json' : 'global defaults'} (the layer that sets this row). Read-only view.`, inner)}</Text>
		<Text color={THEME.muted}>{truncate('↑↓ move · enter/e open the file that sets it · T review/trust · esc back', inner)}</Text>
	</Box>;
}
