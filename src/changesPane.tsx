import React from 'react';
import {Box, Text} from 'ink';
import type {SessionRecord} from './types.js';
import {CHANGE_GROUP_TITLES, changeLabel, changeRows, classifyDiff, lineCountsText, totalChanges, type ChangeDiff, type ChangeEntry, type ChangeRow, type ChangesRecord, type DiffLineKind} from './changesModel.js';
import {SELECTION_MARKER, fitParts, menuWindowStart, type DetailPart} from './menu.js';
import {DISPLAY_CONTROL_PATTERN, THEME, compactPath, truncate} from './ui.js';
import {workspacePaneUnavailable} from './workspace.js';

// Rendering of the Git tab: the Changes list of the session's workspace (browse: the list alone, read-only; focus:
// a selectable list plus the selected entry's diff, side by side when wide enough, stacked otherwise). State and
// keys live in changesFlow.ts; lazygit is only attached (o).

/** Side by side from this many columns of pane width. */
const SIDE_BY_SIDE_WIDTH = 100;

export interface ChangesLayout {side: boolean; listWidth: number; listRows: number; diffWidth: number; diffRows: number}
/** How focus mode splits the pane (after its header row) between the list and the diff (whose title takes a row). */
export function changesLayout(width: number, height: number, listLength: number): ChangesLayout {
	const body = Math.max(2, height - 1);
	if (width >= SIDE_BY_SIDE_WIDTH) {
		const listWidth = Math.max(36, Math.min(64, Math.floor(width * 0.42)));
		return {side: true, listWidth, listRows: body, diffWidth: Math.max(10, width - listWidth - 3), diffRows: Math.max(1, body - 1)};
	}
	// Stacked: the list takes what it needs up to 40% (at least 3 rows), a rule, then the diff.
	const listRows = Math.max(Math.min(3, body), Math.min(listLength, Math.floor(body * 0.4)));
	return {side: false, listWidth: width, listRows, diffWidth: width, diffRows: Math.max(1, body - listRows - 2)};
}

const STATUS_COLORS: Record<string, string> = {M: THEME.warn, T: THEME.warn, A: THEME.success, '?': THEME.success, D: THEME.error, U: THEME.error, R: THEME.active, C: THEME.active};
const DIFF_COLORS: Record<DiffLineKind, string | undefined> = {add: THEME.success, del: THEME.error, hunk: THEME.active, meta: THEME.muted, context: undefined};
const visible = (text: string) => text.replace(/\t/g, '    ').replace(DISPLAY_CONTROL_PATTERN, '?');

function EntryRow({entry, selected, width}: {entry: ChangeEntry; selected: boolean; width: number}) {
	const inner = Math.max(1, width - 2);
	const {name, dir} = changeLabel(entry);
	const counts = lineCountsText(entry);
	const tail: DetailPart[] = !counts ? [] : entry.binary ? [{text: counts, color: THEME.muted}] : [{text: `+${entry.additions}`, color: THEME.success}, {text: ` −${entry.deletions}`, color: THEME.error}];
	const tailLength = counts.length;
	const shownTail = tailLength + 2 <= Math.floor(inner / 2) ? tail : [];
	const room = inner - (shownTail.length ? tailLength + 1 : 0);
	const body = fitParts([{text: `${entry.status} `, color: STATUS_COLORS[entry.status], bold: true}, {text: visible(name)}, ...(dir ? [{text: `  ${visible(dir)}`, color: THEME.muted}] : [])], room);
	const used = body.reduce((sum, part) => sum + part.text.length, 0);
	return <Text wrap="truncate-end" inverse={selected} bold={selected} color={selected ? THEME.active : undefined}>
		{selected ? SELECTION_MARKER : ' '} {body.map((part, index) => <Text key={index} color={selected ? undefined : part.color} bold={part.bold}>{part.text}</Text>)}
		{' '.repeat(Math.max(0, room - used + (shownTail.length ? 1 : 0)))}
		{shownTail.map((part, index) => <Text key={`t${index}`} color={selected ? undefined : part.color}>{part.text}</Text>)}
	</Text>;
}

function ListRow({row, selected, width}: {row: ChangeRow; selected: number; width: number}) {
	if (row.kind === 'entry') return <EntryRow entry={row.entry} selected={row.index === selected} width={width} />;
	if (row.kind === 'more') return <Text color={THEME.muted} wrap="truncate-end">{truncate(`    +${row.count} more`, width)}</Text>;
	const title = CHANGE_GROUP_TITLES[row.group];
	return <Text wrap="truncate-end"><Text color={row.group === 'conflicts' ? THEME.error : THEME.accentSoft} bold>{truncate(title, Math.max(1, width - 6))}</Text><Text color={THEME.muted}> {row.count}</Text></Text>;
}

/** The rows shown in `rows` lines: from the top (browse), or around the selection with ↑/↓ more markers (focus). */
function listWindow(rows: ChangeRow[], selected: number, height: number, focused: boolean): Array<ChangeRow | {kind: 'marker'; text: string}> {
	if (rows.length <= height) return rows;
	if (!focused) return [...rows.slice(0, Math.max(0, height - 1)), {kind: 'marker', text: `  ↓ ${rows.length - height + 1} more · v to browse`}];
	const at = Math.max(0, rows.findIndex(row => row.kind === 'entry' && row.index === selected));
	if (height < 3) { const start = menuWindowStart(at, rows.length, height); return rows.slice(start, start + height); }
	const visibleRows = height - 2;
	// Keep the selection's group header in view when it fits.
	const start = menuWindowStart(at, rows.length, visibleRows);
	const anchored = at - start === 0 && rows[at - 1]?.kind === 'header' && start > 0 ? start - 1 : start;
	const shown = rows.slice(anchored, anchored + visibleRows);
	const below = rows.length - anchored - shown.length;
	return [{kind: 'marker', text: anchored > 0 ? `  ↑ ${anchored} more` : ' '}, ...shown, {kind: 'marker', text: below > 0 ? `  ↓ ${below} more` : ' '}];
}

function ChangesList({rows, selected, width, height, focused}: {rows: ChangeRow[]; selected: number; width: number; height: number; focused: boolean}) {
	const shown = listWindow(rows, focused ? selected : -1, height, focused);
	return <Box flexDirection="column" width={width} height={height}>
		{shown.map((row, index) => row.kind === 'marker'
			? <Text key={`m${index}`} color={THEME.muted} wrap="truncate-end">{truncate(row.text, width)}</Text>
			: <ListRow key={row.kind === 'entry' ? `e${row.index}` : `${row.kind}${row.group}`} row={row} selected={focused ? selected : -1} width={width} />)}
	</Box>;
}

export interface DiffState {key: string; diff?: ChangeDiff; error?: string}
/** Diff lines (for scrolling): classified, the cut notice included. */
export function diffLines(diff: ChangeDiff | undefined): Array<{kind: DiffLineKind | 'note'; text: string}> {
	if (!diff) return [];
	if (diff.binary) return [{kind: 'note', text: 'Binary file; no preview.'}];
	if (!diff.text.trim()) return [{kind: 'note', text: 'No textual changes (mode, directory or empty file).'}];
	const lines: Array<{kind: DiffLineKind | 'note'; text: string}> = classifyDiff(diff.text);
	if (diff.truncated) lines.push({kind: 'note', text: '… preview cut here; open the file (enter) or lazygit (o) for the rest'});
	return lines;
}

function DiffView({entry, state, scroll, width, height}: {entry?: ChangeEntry; state?: DiffState; scroll: number; width: number; height: number}) {
	const lines = state?.diff ? diffLines(state.diff) : [];
	const rows = Math.max(1, height - 1);
	const start = Math.max(0, Math.min(scroll, lines.length - rows));
	const position = lines.length > rows ? `${start + 1}-${Math.min(lines.length, start + rows)}/${lines.length}` : '';
	const title = entry ? `${entry.group === 'staged' ? 'staged' : entry.group === 'untracked' ? 'untracked' : entry.group === 'conflicts' ? 'conflict' : 'unstaged'} · ${visible(entry.path)}` : 'No selection';
	const message = !entry ? 'Select a file to preview its diff.' : state?.error ? `Diff unavailable: ${state.error}` : !state?.diff ? 'Loading diff…' : undefined;
	const head = truncate(title, Math.max(1, width - (position ? position.length + 2 : 0)));
	return <Box flexDirection="column" width={width} height={height}>
		<Text wrap="truncate-end"><Text color={THEME.muted} bold>{head}</Text>{position ? <Text color={THEME.muted}>{' '.repeat(Math.max(2, width - head.length - position.length))}{position}</Text> : null}</Text>
		{message
			? <Text color={state?.error ? THEME.error : THEME.muted} wrap="truncate-end">{truncate(message, width)}</Text>
			: lines.slice(start, start + rows).map((line, index) => <Text key={index} color={line.kind === 'note' ? THEME.muted : DIFF_COLORS[line.kind]} dimColor={line.kind === 'meta'} wrap="truncate-end">{truncate(visible(line.text), width) || ' '}</Text>)}
	</Box>;
}

function summary(changes: ChangesRecord): {text: string; color: string} {
	if (changes.error) return {text: '○ error', color: THEME.error};
	if (!changes.loaded) return {text: '…', color: THEME.muted};
	const {conflicts, staged, unstaged, untracked} = changes.counts;
	if (!totalChanges(changes.counts)) return {text: '● clean', color: THEME.success};
	const parts = [conflicts ? `${conflicts} conflicted` : '', staged ? `${staged} staged` : '', unstaged ? `${unstaged} changed` : '', untracked ? `${untracked} untracked` : ''].filter(Boolean);
	return {text: parts.join(' · '), color: conflicts ? THEME.error : THEME.muted};
}

function fallbackMessage(session: SessionRecord | undefined, changes: ChangesRecord): {text: string; color?: string} | undefined {
	if (!session) return {text: 'No session selected.'};
	const unavailable = workspacePaneUnavailable(session, changes);
	if (unavailable) return {text: `Git is unavailable: ${unavailable}.`};
	if (changes.sessionId !== session.id || !changes.loaded) return {text: 'Reading changes…'};
	if (changes.error) return {text: `Git status failed: ${changes.error}`, color: THEME.error};
	if (!totalChanges(changes.counts)) return {text: 'No changes. o opens lazygit for history, branches and commits.'};
	return undefined;
}

export function ChangesPane({session, changes, focused, selected, diff, diffScroll, width, height}: {session?: SessionRecord; changes: ChangesRecord; focused: boolean; selected: number; diff?: DiffState; diffScroll: number; width: number; height: number}) {
	const ready = changes.sessionId === session?.id && changes.workspace;
	const status = ready ? summary(changes) : {text: '', color: THEME.muted};
	const branch = ready && changes.branch ? ` · ${changes.branch}` : '';
	const pathBudget = Math.max(8, width - status.text.length - 1);
	const where = truncate(`${compactPath(changes.workspace ?? session?.worktree?.path ?? session?.cwd ?? '', Math.max(8, pathBudget - branch.length))}${branch}`, pathBudget);
	const header = <Box justifyContent="space-between" width={width}>
		<Text color={THEME.muted} wrap="truncate-end">{where}</Text>
		<Text color={status.color} wrap="truncate-end">{status.text}</Text>
	</Box>;
	const message = fallbackMessage(session, changes);
	const body = Math.max(1, height - 1);
	if (message) return <Box flexDirection="column" width={width} height={height}>{header}<Text color={message.color} wrap="wrap">{message.text}</Text></Box>;
	const rows = changeRows(changes);
	if (!focused) return <Box flexDirection="column" width={width} height={height}>{header}<ChangesList rows={rows} selected={-1} width={width} height={body} focused={false} /></Box>;
	const layout = changesLayout(width, height, rows.length);
	const entry = selected >= 0 ? changes.entries[selected] : undefined;
	const diffView = <DiffView entry={entry} state={diff} scroll={diffScroll} width={layout.diffWidth} height={layout.diffRows + 1} />;
	return <Box flexDirection="column" width={width} height={height}>
		{header}
		{layout.side
			? <Box flexDirection="row" width={width} height={body}>
				<ChangesList rows={rows} selected={selected} width={layout.listWidth} height={layout.listRows} focused />
				<Box width={3} height={body} flexDirection="column">{Array.from({length: body}, (_, index) => <Text key={index} color={THEME.border}> │ </Text>)}</Box>
				{diffView}
			</Box>
			: <>
				<ChangesList rows={rows} selected={selected} width={width} height={layout.listRows} focused />
				<Text color={THEME.border}>{'─'.repeat(Math.max(1, width))}</Text>
				{diffView}
			</>}
	</Box>;
}
