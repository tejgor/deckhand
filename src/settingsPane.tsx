import React, {useRef} from 'react';
import {Box, Text} from 'ink';
import type {ConfigTargetKind} from './projectConfigDocument.js';
import type {SettingsInfo, WorktreeCandidate, WorktreeCandidates} from './settingsInfo.js';
import {ACTIONS_INTRO, COLUMNS, COLUMN_TITLES, MAX_ACTIONS, NEEDS_TRUST_DETAIL, SETTINGS, actionCount, cellDetail, homePath, layerActions, layerName, settingsGrid, type ChoiceOption, type GridCell, type GridRow, type SettingId} from './settingsModel.js';
import type {EditorState} from './textEditor.js';
import {DetailText, SELECTION_MARKER, SelectableCell, SelectableRow, fitHint, wrapWords, type DetailLine, type DetailPart, type HintPart} from './menu.js';
import {DISPLAY_CONTROL_PATTERN, THEME, truncate} from './ui.js';

// Rendering of the Settings screen (C): the grid (one row per setting, one column per layer, the cursor a cell), the
// Actions list and the link picker of one layer, each with a details box (or an inline edit control) under it and one
// hint line. State and keys live in settingsFlow.tsx.

export type SettingsView = 'main' | 'actions' | 'links';
export type SettingsEdit =
	| {kind: 'choice'; id: SettingId; label: string; target: ConfigTargetKind; options: ChoiceOption[]; index: number; /** The option this layer stores (◉), -1 for none. */ current: number}
	| {kind: 'text'; id: SettingId; label: string; target: ConfigTargetKind; state: EditorState; error?: string; entry?: string; /** Adding an action: its name first, then its command. */ step?: 'name' | 'command'; /** Shown while the input is empty: what applies without a value here. */ placeholder?: string}
	| {kind: 'clear'; id: SettingId; label: string; target: ConfigTargetKind; entry?: string}
	| {kind: 'discard-links'};
export interface LinksState {data: WorktreeCandidates; links: Record<string, boolean>; initial: Record<string, boolean>; target: ConfigTargetKind; row: number}
export interface Notice {text: string; error?: boolean}

const visible = (text: string) => text.replace(DISPLAY_CONTROL_PATTERN, '?').replace(/[\n\t]/g, ' ');
const layerShort = (kind: ConfigTargetKind) => kind === 'global' ? 'global defaults' : 'this repo';
const LAYER_COLOR: Record<ConfigTargetKind, string> = {global: THEME.active, repository: THEME.success};
// Paths keep their distinctive tail; everything else is cut at the end.
const tail = (text: string, width: number) => width <= 1 ? text.slice(0, width) : text.length <= width ? text : `…${text.slice(text.length - width + 1)}`;
const partsLength = (parts: DetailPart[]) => parts.reduce((sum, part) => sum + part.text.length, 0);
const partsLine = (parts: DetailPart[]): DetailLine => ({text: parts.map(part => part.text).join(''), parts, nowrap: true});

function repositoryState(info: SettingsInfo): {text: string; color: string} {
	const hook = info.needsReview && info.repository.state !== 'untrusted';
	switch (info.repository.state) {
		case 'trusted': return hook ? {text: 'deckhand.json: trusted ✓ · hook needs review', color: THEME.warn} : {text: 'deckhand.json: trusted ✓', color: THEME.success};
		case 'untrusted': return {text: 'deckhand.json: needs trust', color: THEME.warn};
		case 'absent': return hook ? {text: 'creation hook needs review', color: THEME.warn} : {text: 'deckhand.json: not present', color: THEME.muted};
		case 'invalid': return {text: 'deckhand.json: invalid', color: THEME.error};
		case 'bare': return {text: 'bare repo: global defaults only', color: THEME.muted};
		case 'none': return {text: 'not a Git repo: global defaults only', color: THEME.warn};
	}
}
function headerLines(info: SettingsInfo): Notice[] {
	const lines: Notice[] = [];
	if (info.repository.state === 'invalid') lines.push({text: `deckhand.json invalid: ${info.repository.error ?? 'unknown error'} (→ This repo, then e repairs it)`, error: true});
	if (info.globalError) lines.push({text: `Global defaults invalid: ${/: (.*?)\. Repair it/.exec(info.globalError)?.[1] ?? info.globalError} (← Global, then e repairs it)`, error: true});
	return lines;
}

interface ListLine {text: string; /** Styled segments of `text`. */ parts?: DetailPart[]; /** Rendered as is (a grid row). */ element?: React.ReactNode; row?: number; head?: boolean; selected?: boolean; color?: string; bold?: boolean; status?: {text: string; color?: string}; statusWidth?: number; /** Explanation above the list, dropped when the list must scroll. */ intro?: boolean; /** A section heading: shown above its first row when it fits, never the scroll anchor. */ section?: boolean}
/**
 * Keeps the selected row in view (its section heading too when it fits), with ↑/↓ more markers on the sides that
 * have more. `scroll` persists the first shown line between renders.
 */
function windowLines(lines: ListLine[], selected: number, rows: number, scroll: {current: number}): ListLine[] {
	if (lines.length <= rows) { scroll.current = 0; return lines; }
	const head = Math.max(0, lines.findIndex(line => line.row === selected && !line.section));
	let last = head;
	while (lines[last + 1]?.row === selected && !lines[last + 1]!.section) last++;
	const place = (room: number) => {
		const top = lines[head - 1]?.section && last - head + 2 <= room ? head - 1 : head;
		let start = scroll.current;
		if (top < start) start = top;
		else if (last >= start + room) start = Math.min(head, last - room + 1);
		return Math.max(0, Math.min(start, lines.length - room));
	};
	// Too short for a marker beside the rows (two markers need a third row): rows only.
	if (rows < 3) { scroll.current = place(Math.max(1, rows)); return lines.slice(scroll.current, scroll.current + Math.max(1, rows)); }
	const marker = (text: string): ListLine => ({text, color: THEME.muted});
	// One marker when the window touches either end, otherwise both.
	let start = place(rows - 1);
	if (start === 0) { scroll.current = 0; return [...lines.slice(0, rows - 1), marker(`  ↓ ${lines.length - rows + 1} more`)]; }
	if (start + rows - 1 >= lines.length) { scroll.current = start; return [marker(`  ↑ ${start} more`), ...lines.slice(start)]; }
	start = place(Math.max(1, rows - 2));
	scroll.current = start;
	const room = Math.max(1, rows - 2);
	const shown = lines.slice(start, start + room);
	const below = lines.length - start - room;
	return [marker(start > 0 ? `  ↑ ${start} more` : ' '), ...shown, marker(below > 0 ? `  ↓ ${below} more` : ' ')];
}

// The grid ------------------------------------------------------------------------------------------------------

/** From this inner width both value columns are shown; below it only the selected one, with a ◂ ▸ indicator. */
const TWO_COLUMN_WIDTH = 64;
const LABEL_WIDTH = Math.max(...SETTINGS.map(def => def.label.length));
interface GridLayout {two: boolean; label: number; cell: number}
const MAX_CELL = 40;
/** Marker (1) + space + label + gap (2), then one or two cells separated by a space; wide panes keep cells compact. */
function gridLayout(inner: number): GridLayout {
	const label = Math.min(LABEL_WIDTH, Math.max(8, Math.floor(inner / 3)));
	const two = inner >= TWO_COLUMN_WIDTH;
	const room = Math.max(4, inner - label - 4);
	return {two, label, cell: Math.min(MAX_CELL, two ? Math.floor((room - 1) / 2) : room)};
}
/**
 * A cell: "● " when its value applies now, then the value (paths keep their tail): bold when in effect, muted when
 * overridden or unset, italic for the built-in or legacy value, yellow with ⚠ (and "needs trust" when it fits) when it
 * applies only once trusted.
 */
function cellParts(cell: GridCell, width: number, path: boolean): {parts: DetailPart[]; cut: boolean} {
	const lead: DetailPart = cell.effective ? {text: '● ', color: THEME.active, bold: true} : {text: '  '};
	const room = Math.max(1, width - 2);
	const text = visible(cell.text);
	const [long, short] = cell.needsTrust ? [' ⚠ needs trust', ' ⚠'] : cell.builtIn ? [' (built-in)', ' (built-in)'] : cell.legacy ? [' (legacy)', ' (legacy)'] : ['', ''];
	// The long form when the value fits beside it. Otherwise ⚠ always stays (cutting the value); "(built-in)" goes when
	// the value alone fits (its style and the details line still say so), else stays while the value keeps 8 columns.
	const suffix = text.length + long.length <= room ? long : cell.needsTrust ? short : text.length <= room || room - short.length < 8 ? '' : short;
	const body = (path ? tail : truncate)(text, Math.max(1, room - suffix.length));
	const style: Partial<DetailPart> = cell.needsTrust ? {color: THEME.warn} : cell.builtIn || cell.legacy ? {color: THEME.muted, italic: true} : cell.set && cell.effective ? {bold: true} : {color: THEME.muted};
	return {parts: [lead, {text: body, ...style}, ...suffix ? [{text: suffix, ...cell.needsTrust ? {color: THEME.warn} : {color: THEME.muted, italic: true}}] : []], cut: body.length < text.length};
}
const isPath = (row: GridRow) => row.def.id === 'worktree.location';
function gridRowElement(row: GridRow, selected: boolean, column: ConfigTargetKind, layout: GridLayout): React.ReactNode {
	const columns = layout.two ? COLUMNS : [column];
	return <Text wrap="truncate-end">
		<Text color={THEME.active} bold>{selected ? SELECTION_MARKER : ' '}</Text>{' '}
		<Text bold={selected}>{truncate(row.def.label, layout.label).padEnd(layout.label)}</Text>{'  '}
		{columns.map((kind, index) => <React.Fragment key={kind}>{index ? ' ' : ''}<SelectableCell selected={selected && kind === column} parts={cellParts(row.cells[kind], layout.cell, isPath(row)).parts} width={layout.cell} /></React.Fragment>)}
	</Text>;
}
/** The column headings (aligned with the values), or "◂ Global | This repo ▸" when only the selected column shows. */
function ColumnHeader({column, layout}: {column: ConfigTargetKind; layout: GridLayout}) {
	const style = (kind: ConfigTargetKind) => kind === column ? {color: THEME.accent, bold: true} : {color: THEME.muted};
	const indent = ' '.repeat(layout.label + 4);
	if (layout.two) {
		return <Text wrap="truncate-end">{indent}{COLUMNS.map((kind, index) => {
			const title = COLUMN_TITLES[kind].find(text => text.length + 2 <= layout.cell) ?? COLUMN_TITLES[kind].at(-1)!;
			return <React.Fragment key={kind}>{index ? ' ' : ''}<Text {...style(kind)}>{truncate(`  ${title}`, layout.cell).padEnd(index ? 0 : layout.cell)}</Text></React.Fragment>;
		})}</Text>;
	}
	return <Text wrap="truncate-end">{indent}<Text color={THEME.muted}>{'◂ '}</Text><Text {...style('global')}>Global</Text><Text color={THEME.muted}>{' | '}</Text><Text {...style('repository')}>This repo</Text><Text color={THEME.muted}>{' ▸'}</Text></Text>;
}
function gridLines(grid: GridRow[], selected: number, column: ConfigTargetKind, layout: GridLayout): ListLine[] {
	const lines: ListLine[] = [];
	grid.forEach((row, index) => {
		if (index === 0 || grid[index - 1]!.def.section !== row.def.section) lines.push({text: ` ${row.def.section}`, section: true, bold: true, color: THEME.accentSoft});
		lines.push({text: row.def.label, row: index, element: gridRowElement(row, index === selected, column, layout)});
	});
	return lines;
}
/**
 * The details box of the selected cell, one line: "<Layer> · <file> · <how it relates to the other layer>" (the file
 * is cut from the front, or dropped when the rest needs the room, then the line wraps), plus the full value when the
 * cell cut it, a template, or a note (hook state, ignored while a hook is active).
 */
function gridDetails(info: SettingsInfo, row: GridRow, column: ConfigTargetKind, layout: GridLayout, width: number): DetailLine[] {
	const detail = cellDetail(info, row, column);
	const color = detail.error ? THEME.error : detail.warn ? THEME.warn : undefined;
	// "Built-in default" becomes "Built-in" when that keeps the explanation on one line (the cell says built-in too).
	const headText = !detail.layer && detail.head.length + detail.relation.length + 3 > width ? 'Built-in' : detail.head;
	const head: DetailPart = {text: headText, color: detail.layer ? LAYER_COLOR[detail.layer] : THEME.muted, bold: true};
	const relation: DetailPart = {text: detail.relation, ...color ? {color} : {}};
	const file = detail.layer ? homePath(info.targets[detail.layer]?.path ?? '', info.vars?.home ?? process.env.HOME) : '';
	const room = width - partsLength([head, relation]) - 6;
	const lines: DetailLine[] = file && room >= Math.min(file.length, 14) ? [partsLine([head, {text: ' · '}, {text: tail(file, room)}, {text: ' · '}, relation])]
		: partsLength([head, relation]) + 3 <= width ? [partsLine([head, {text: ' · '}, relation])]
		: [{text: `${headText} · ${detail.relation}`, ...color ? {color} : {}}];
	const cell = row.cells[column];
	const cut = cellParts(cell, layout.cell, isPath(row)).cut || (row.def.control === 'links' && cell.set);
	const extra = cell.template && cell.template !== cell.text ? `Template: ${cell.template}` : cut && cell.full ? cell.full : row.note;
	// A second line only while the first did not wrap: the box stays at two lines.
	if (extra && (lines[0]!.parts || wrapWords(lines[0]!.text, width).length === 1)) lines.push({text: visible(extra), nowrap: true, color: THEME.muted});
	return lines;
}

// Lists (Actions, Linked items) -----------------------------------------------------------------------------------

/** Column layout for label/value/status rows: marker (2) + label + gap + value + gap + status. */
function columns(inner: number, labels: string[], statuses: string[]) {
	const statusWidth = Math.min(Math.max(0, ...statuses.map(status => status.length)), Math.floor(inner / 3));
	const labelWidth = Math.min(Math.max(4, ...labels.map(label => label.length)), Math.max(8, Math.floor(inner / 3)));
	const valueWidth = Math.max(6, inner - 2 - 1 - labelWidth - 2 - 2 - statusWidth);
	return {labelWidth, valueWidth, statusWidth};
}
type Layout = ReturnType<typeof columns>;
function valueLine(layout: Layout, row: number | undefined, label: string, value: string, status: string | undefined, {selected = false, muted = false, statusColor = THEME.muted}: {selected?: boolean; muted?: boolean; statusColor?: string} = {}): ListLine {
	const text = ` ${truncate(visible(label), layout.labelWidth).padEnd(layout.labelWidth)}  ${truncate(visible(value), layout.valueWidth).padEnd(layout.valueWidth)}`;
	return {text, row, head: row !== undefined, selected, ...muted ? {color: THEME.muted} : {}, status: {text: status ?? '', color: statusColor}, statusWidth: layout.statusWidth};
}
function actionLines(info: SettingsInfo, column: ConfigTargetKind, selected: number, inner: number): {lines: ListLine[]; valueWidth: number} {
	const {own, context} = layerActions(info, column);
	const all = [...own, ...context];
	const layout = columns(inner, all.map(action => action.name), all.map(action => action.status ?? ''));
	// What actions are, above the list (it scrolls away with a long list).
	const lines: ListLine[] = wrapWords(ACTIONS_INTRO, Math.max(1, inner - 1)).map(text => ({text: ` ${text}`, color: THEME.muted, intro: true}));
	own.forEach((action, index) => lines.push(valueLine(layout, index, action.name, action.command, action.status, {selected: index === selected, statusColor: action.needsTrust ? THEME.warn : THEME.muted})));
	lines.push({text: ' + Add action', row: own.length, head: true, color: THEME.active, selected: selected === own.length});
	if (context.length) {
		lines.push({text: column === 'repository' ? ' Also in effect here, from global defaults:' : ' This repo also has:', color: THEME.muted});
		for (const action of context) lines.push(valueLine(layout, undefined, action.name, action.command, action.status, {muted: true}));
	}
	return {lines, valueWidth: layout.valueWidth};
}
function actionDetails(info: SettingsInfo, column: ConfigTargetKind, selected: number, valueWidth: number): DetailLine[] {
	const action = layerActions(info, column).own[selected];
	const head: DetailPart = {text: layerName(column), color: LAYER_COLOR[column], bold: true};
	if (!action) {
		const lines: DetailLine[] = [{text: 'a (or Enter) adds an action: first its name, then its command.'}];
		if (actionCount(info, column) >= MAX_ACTIONS) lines.push({text: `${layerShort(column)} has ${MAX_ACTIONS} actions, the most allowed: remove one (x) to add another.`, color: THEME.warn});
		return lines;
	}
	const relation = action.needsTrust ? NEEDS_TRUST_DETAIL : action.status === 'overrides global' ? `overrides the global ${action.name} action` : action.status ? `this repo's own ${action.name} action wins there` : 'run it with e on a session';
	const lines: DetailLine[] = [{text: `${head.text} · ${relation}`, ...action.needsTrust ? {color: THEME.warn} : {}}];
	if (action.command.length > valueWidth) lines.push({text: action.command, nowrap: true, color: THEME.active});
	return lines;
}

function formatSize(kib: number | null | undefined, kind?: WorktreeCandidate['kind']): string {
	if (kind === 'symlink') return '';
	if (kib === undefined) return '…';
	if (kib === null) return '?';
	if (kib < 1024) return `${kib} KB`;
	const mib = kib / 1024;
	return mib < 1024 ? `${mib < 10 ? mib.toFixed(1) : Math.round(mib)} MB` : `${(mib / 1024).toFixed(1)} GB`;
}
function candidateAction(candidate: WorktreeCandidate): string {
	return candidate.missing ? 'missing in checkout'
		: candidate.configured === 'files' ? `files → ${candidate.source}`
		: candidate.configured ? `configured${candidate.layers?.length ? ` (${candidate.layers.map(layer => layer === 'global' ? 'global' : 'repo').join(', ')})` : ''}`
		: candidate.reason ? `${candidate.suggestion === 'link' ? 'suggested: ' : ''}${candidate.reason}` : '';
}
// Narrow rows drop the "suggested: " prefix ([link]/[skip] already shows the suggestion); the details box has it.
const fitAction = (text: string, width: number) => text.length > width ? text.replace(/^suggested: /, '') : text;
function linkLines(state: LinksState, sizes: Record<string, number | null>, home: string | undefined, inner: number): ListLine[] {
	const {candidates, moreCandidates, candidatesError} = state.data;
	const lines: ListLine[] = [];
	if (candidatesError) lines.push({text: `Could not list untracked/ignored files: ${candidatesError}`, color: THEME.error});
	else if (!candidates.length) lines.push({text: 'No untracked or ignored entries in the main checkout.', color: THEME.muted});
	// Marker (2), "[link] " (7), name, gap, size, gap, then the reason in the rest.
	const sizeWidth = 7, fixed = 2 + 7 + 2 + sizeWidth + 2;
	const names = candidates.map(candidate => `${candidate.path}${candidate.kind === 'dir' ? '/' : ''}${candidate.target ? ` → ${tail(homePath(candidate.target, home), 30)}` : candidate.kind === 'symlink' ? ' → (broken link)' : ''}`);
	const nameWidth = Math.max(8, Math.min(Math.max(8, ...names.map(name => name.length)), inner - fixed - 12));
	const actionWidth = Math.max(1, inner - fixed - nameWidth);
	candidates.forEach((candidate, index) => {
		const linked = candidate.configured === 'files' || state.links[candidate.path];
		const text = `[${linked ? 'link' : 'skip'}] ${truncate(visible(names[index]!), nameWidth).padEnd(nameWidth)}  ${formatSize(sizes[candidate.path] ?? (candidate.missing ? null : undefined), candidate.kind).padStart(sizeWidth)}  ${truncate(visible(fitAction(candidateAction(candidate), actionWidth)), actionWidth)}`;
		lines.push({text, row: index, head: true, color: linked ? THEME.success : THEME.muted, selected: index === state.row});
	});
	if (moreCandidates) lines.push({text: `  +${moreCandidates} more (edit raw JSON)`, color: THEME.muted});
	return lines;
}

// Edit controls ---------------------------------------------------------------------------------------------------

/** Text input on one row: the cursor is shown inverse and the view scrolls horizontally to keep it visible. */
function InputLine({state, width, placeholder}: {state: EditorState; width: number; placeholder?: string}) {
	const characters = Array.from(state.text);
	const cursor = Array.from(state.text.slice(0, state.cursor)).length;
	const room = Math.max(2, width - 3);
	const from = cursor < room ? 0 : cursor - room + 1;
	const shown = characters.slice(from, from + room);
	const at = cursor - from;
	// Ctrl+A: the whole text is selected (typing replaces it).
	if (state.selectAll && characters.length) return <Text wrap="truncate-end"><Text color={THEME.accent}>{'› '}</Text><Text inverse>{visible(characters.slice(0, room).join(''))}</Text></Text>;
	if (!characters.length && placeholder) return <Text wrap="truncate-end"><Text color={THEME.accent}>{'› '}</Text><Text inverse bold> </Text><Text color={THEME.muted} italic>{truncate(` ${visible(placeholder)}`, Math.max(0, room - 1))}</Text></Text>;
	return <Text wrap="truncate-end"><Text color={THEME.accent}>{'› '}</Text>{visible(shown.slice(0, at).join(''))}<Text inverse bold>{visible(shown[at] ?? ' ')}</Text>{visible(shown.slice(at + 1).join(''))}</Text>;
}

/** Help prose wrapped at its " · " separators where it can (so an example or rule is not split), else at words. */
function wrapChunks(text: string, width: number): string[] {
	const lines: string[] = [];
	let line = '';
	for (const chunk of text.split(' · ')) {
		const joined = line ? `${line} · ${chunk}` : chunk;
		if (joined.length <= width) { line = joined; continue; }
		if (line) lines.push(line);
		const words = wrapWords(chunk, width);
		line = words.pop() ?? '';
		lines.push(...words);
	}
	const chunked = line ? [...lines, line] : lines;
	const words = wrapWords(text, width);
	return chunked.length <= words.length ? chunked : words;
}
/** A text edit's messages: its error (wrapped, up to 3 lines), then the help lines (prose wraps, paths and previews are cut). */
function editMessage(edit: Extract<SettingsEdit, {kind: 'text'}>, help: DetailLine[] | undefined, inner: number): DetailLine[] {
	const lines: DetailLine[] = [
		...edit.error ? wrapWords(visible(edit.error), inner).slice(0, 3).map(text => ({text, color: THEME.error})) : [],
		// Wrapped help continues indented, so each rule or example list reads as one item.
		...(help ?? []).flatMap(line => (line.nowrap ? [truncate(visible(line.text), inner)] : wrapChunks(visible(line.text), Math.max(1, inner - 2)).map((text, index) => index ? `  ${text}` : text)).map(text => ({text, color: line.color ?? THEME.muted}))),
	];
	return lines.length ? lines : [{text: ' '}];
}
/** The title of an edit: "<Setting> · <Layer>"; adding an action is two numbered steps (shortened to keep the name). */
function editTitle(edit: SettingsEdit, width: number): string {
	if (edit.kind === 'discard-links') return 'Discard link changes?';
	if (edit.kind === 'clear') return `Clear ${edit.label} in ${layerShort(edit.target)}?`;
	if (edit.kind === 'choice' || edit.id !== 'actions') return `${edit.label} · ${layerName(edit.target)}`;
	const name = visible(edit.entry ?? '');
	const titles = edit.step === 'name' ? ['New action · step 1 of 2: name']
		: edit.step === 'command' ? [`New action · step 2 of 2: command for ${name}`, `Step 2 of 2: command for ${name}`, `2/2: command for ${name}`]
		: [`Action ${name}: command`, `${name}: command`];
	return titles.find(title => title.length <= width) ?? titles.at(-1)!;
}
// An option's warning sits on its own lines under it (aligned with the label, at most two), so it never cuts the path.
const WARNING_INDENT = 4;
function warningLines(option: ChoiceOption, inner: number): string[] {
	if (!option.warning) return [];
	const room = Math.max(1, inner - WARNING_INDENT);
	const lines = wrapWords(visible(option.warning), room);
	return (lines.length > 2 ? [lines[0]!, truncate(lines.slice(1).join(' '), room)] : lines).map(text => `${' '.repeat(Math.min(WARNING_INDENT, Math.max(0, inner - room)))}${text}`);
}
function editRows(edit: SettingsEdit, help: DetailLine[] | undefined, inner: number): number {
	if (edit.kind === 'choice') return edit.options.reduce((sum, option) => sum + 1 + warningLines(option, inner).length, 0);
	if (edit.kind === 'text') return 1 + editMessage(edit, help, inner).length;
	return 1 + (help?.length ?? 0);
}
function EditBox({edit, help, width, rows, bordered}: {edit: SettingsEdit; help?: DetailLine[]; width: number; rows: number; bordered: boolean}) {
	const inner = Math.max(1, width - (bordered ? 4 : 0));
	const content: React.ReactNode[] = [];
	if (edit.kind === 'choice') {
		const labelWidth = Math.max(...edit.options.map(option => option.label.length));
		// Paths keep their tail (the end says where worktrees go); everything else is cut at the end.
		const room = Math.max(1, inner - 2 - 2 - labelWidth - 2);
		const detail = (option: ChoiceOption) => option.detail ? (option.path ? tail : truncate)(visible(option.detail), room) : '';
		const blocks = edit.options.map((option, index) => [
			<SelectableRow key={index} selected={index === edit.index} text={`${index === edit.current ? '◉' : '○'} ${option.label.padEnd(labelWidth)}${option.detail ? `  ${detail(option)}` : ''}`} width={inner} color={index === edit.current ? THEME.active : undefined} />,
			...warningLines(option, inner).map((text, line) => <Text key={`${index}w${line}`} color={THEME.warn} wrap="truncate-end">{text}</Text>),
		]);
		// When the options do not all fit, show a window that keeps the highlighted one (and its warning) visible.
		const free = Math.max(1, rows - (bordered ? 2 : 0) - 1);
		const size = (from: number, to: number) => blocks.slice(from, to + 1).reduce((sum, block) => sum + block.length, 0);
		let start = 0, used = 0;
		while (start < edit.index && size(start, edit.index) > free) start++;
		for (const block of blocks.slice(start)) { if (used && used + block.length > free) break; content.push(...block); used += block.length; }
	} else if (edit.kind === 'text') {
		content.push(<InputLine key="input" state={edit.state} width={inner} placeholder={edit.placeholder} />);
		editMessage(edit, help, inner).forEach((line, index) => content.push(<Text key={`help${index}`} color={line.color ?? THEME.muted} wrap="truncate-end">{line.text}</Text>));
	} else {
		const text = edit.kind === 'clear'
			? edit.target === 'repository' ? 'Removes it from deckhand.json; global defaults (or the built-in default) apply instead.' : 'Removes it from global defaults; the built-in default applies unless deckhand.json sets it.'
			: 'Nothing has been saved. Enter discards your link changes.';
		content.push(<Text key="text" color={THEME.muted} wrap="truncate-end">{truncate(text, inner)}</Text>);
		(help ?? []).forEach((line, index) => content.push(<Text key={`help${index}`} color={line.color ?? THEME.warn} wrap="truncate-end">{truncate(visible(line.text), inner)}</Text>));
	}
	const body = [<Text key="title" color={THEME.accent} bold wrap="truncate-end">{truncate(visible(editTitle(edit, inner)), inner)}</Text>, ...content].slice(0, Math.max(1, rows - (bordered ? 2 : 0)));
	return bordered
		? <Box flexDirection="column" width={width} borderStyle="round" borderColor={THEME.borderActive} paddingX={1}>{body}</Box>
		: <Box flexDirection="column" width={width}>{body}</Box>;
}
function editHint(edit: SettingsEdit): Array<string | HintPart> {
	if (edit.kind === 'choice') return [{text: '↑↓ choose', drop: 1}, 'enter save', 'esc cancel'];
	if (edit.kind === 'text') return [{text: '←→/home/end move', short: '←→ move', drop: 1}, edit.step === 'name' ? 'enter next' : 'enter save', 'esc cancel'];
	if (edit.kind === 'clear') return ['enter clear', 'esc keep'];
	return ['enter discard', 'esc keep editing'];
}

/** Rows a details box (border, no title) needs for `lines` at `width`. */
function infoRows(lines: DetailLine[], width: number): number {
	const inner = Math.max(1, width - 4);
	return 2 + lines.reduce((sum, line) => sum + (line.nowrap || line.parts ? 1 : wrapWords(line.text, inner).length), 0);
}
/** The details box under a list: wrapped prose, cut values, at most `rows` rows (border included). */
function InfoBox({lines, width, rows}: {lines: DetailLine[]; width: number; rows: number}) {
	const inner = Math.max(1, width - 4);
	const wrapped = lines.flatMap(line => line.parts || line.nowrap ? [line] : wrapWords(line.text, inner).map(text => ({text, color: line.color})));
	return <Box flexDirection="column" width={width} borderStyle="round" borderColor={THEME.border} paddingX={1}>
		{wrapped.slice(0, Math.max(1, rows - 2)).map((line, index) => <DetailText key={index} line={line} width={inner} />)}
	</Box>;
}

interface SettingsPaneProps {
	info: SettingsInfo;
	view: SettingsView;
	row: number;
	actionRow: number;
	/** The selected column: the layer the cursor's cell (and the Actions/Linked items sub-editors) edit. */
	column: ConfigTargetKind;
	edit?: SettingsEdit;
	/** The edit control's help lines (live check, rules, examples, preview), computed by the flow. */
	editHelp?: DetailLine[];
	links?: LinksState;
	sizes: Record<string, number | null>;
	notice?: Notice;
	width: number;
	height: number;
}
export function SettingsPane({info, view, row, actionRow, column, edit, editHelp, links, sizes, notice, width, height}: SettingsPaneProps) {
	const inner = Math.max(1, width - 4);
	const scroll = useRef<Record<SettingsView, number>>({main: 0, actions: 0, links: 0});
	const header = headerLines(info);
	const layout = gridLayout(inner);
	// Border (2), title, header lines, the column headings (grid) and the hint line are fixed; the list and the box below share the rest.
	const body = Math.max(1, height - 2 - 1 - header.length - (view === 'main' ? 1 : 0) - 1);
	const layerTitle = layerName(view === 'links' && links ? links.target : column);
	const title = view === 'main' ? `Settings · ${info.repo}` : view === 'actions' ? `Settings › Actions · ${layerTitle}` : `Settings › Linked items · ${layerTitle}`;
	const state = view === 'main' ? repositoryState(info) : {text: `saves to ${layerShort(view === 'links' && links ? links.target : column)}`, color: LAYER_COLOR[view === 'links' && links ? links.target : column]};
	const heading = truncate(visible(title), Math.max(8, inner - state.text.length - 2));
	const right = truncate(state.text, Math.max(0, inner - heading.length - 2));

	let lines: ListLine[], selected: number, details: DetailLine[] = [], hint: Array<string | HintPart>;
	if (view === 'links' && links) {
		lines = linkLines(links, sizes, info.vars?.home, inner);
		selected = links.row;
		hint = [{text: '↑↓ move', drop: 2}, 'space link/skip', 'enter save', {text: 'e raw JSON', short: 'e JSON', drop: 1}, 'esc cancel'];
		const current = links.data.candidates[links.row];
		if (current) details.push({text: `${current.path}${current.kind === 'dir' ? '/' : ''}: ${current.configured === 'files' ? `copied from ${current.source} by worktree.files (edit in raw JSON: e)` : candidateAction(current) || (links.links[current.path] ? 'linked' : 'not linked')}`});
		if (JSON.stringify(links.links) !== JSON.stringify(links.initial)) details.push({text: 'Unsaved changes: Enter saves them, Esc discards.', color: THEME.warn});
	} else if (view === 'actions') {
		const list = actionLines(info, column, actionRow, inner);
		lines = list.lines; selected = actionRow;
		details = actionDetails(info, column, actionRow, list.valueWidth);
		hint = [{text: '↑↓ move', drop: 3}, 'enter edit', 'a add', {text: 'x remove', drop: 1}, {text: 'e raw JSON', short: 'e JSON', drop: 2}, {text: 'esc back', short: 'esc'}];
	} else {
		const grid = settingsGrid(info);
		const current = Math.min(row, grid.length - 1);
		lines = gridLines(grid, current, column, layout); selected = current;
		details = gridDetails(info, grid[current]!, column, layout, inner - 4);
		hint = [{text: '↑↓ setting', drop: 4}, {text: '←→ global/repo', short: '←→ column'}, 'enter edit', {text: 'x clear', drop: 3}, {text: 'e JSON', drop: 1}, {text: 'T trust', drop: 2}, 'esc'];
	}
	if (notice) details = [{text: notice.text, color: notice.error ? THEME.error : THEME.warn}, ...details];
	if (!details.length) details = [{text: ' '}];
	if (edit) hint = editHint(edit);

	// The edit control (or details box) sits under the list. When it does not fit beside at least one list row, the
	// edit control takes the whole body and the details box becomes one line.
	let list = body, bottom: React.ReactNode = null;
	if (edit) {
		const needed = 2 + 1 + editRows(edit, editHelp, inner - 4);
		const bordered = body - needed >= 1;
		list = bordered ? Math.max(1, Math.min(lines.length, body - needed)) : 0;
		bottom = <EditBox edit={edit} help={editHelp} width={inner} rows={bordered ? needed : body} bordered={bordered} />;
	} else {
		// The details box takes what it needs (a notice can make it three lines) while the list keeps three rows; else one line.
		const rows = Math.min(infoRows(details, inner), 2 + (notice ? 3 : 2));
		if (body - rows >= 3) { list = body - rows; bottom = <InfoBox lines={details} width={inner} rows={rows} />; }
		else { list = Math.max(1, body - 1); bottom = <DetailText line={details[0]!} width={inner} />; }
	}
	if (lines.length > list) lines = lines.filter(line => !line.intro);
	const shown = list > 0 ? windowLines(lines, selected, list, {get current() { return scroll.current[view]; }, set current(value: number) { scroll.current[view] = value; }}) : [];
	return <Box flexDirection="column" width={width} height={height} borderStyle="round" borderColor={THEME.borderActive} paddingX={1}>
		<Text wrap="truncate-end"><Text color={THEME.accent} bold>{heading}</Text>{right ? <Text color={state.color}>{' '.repeat(Math.max(2, inner - heading.length - right.length))}{right}</Text> : null}</Text>
		{header.map((line, index) => <Text key={`h${index}`} color={THEME.error} wrap="truncate-end">{truncate(visible(line.text), inner)}</Text>)}
		{view === 'main' ? <ColumnHeader column={column} layout={layout} /> : null}
		{shown.map((line, index) => line.element ? <React.Fragment key={index}>{line.element}</React.Fragment>
			: line.head || line.status
				? <SelectableRow key={index} selected={Boolean(line.selected)} text={visible(line.text)} parts={line.parts} width={inner} color={line.color} status={line.status} statusWidth={line.statusWidth} />
				: <Text key={index} color={line.color} bold={line.bold} wrap="truncate-end">{truncate(visible(line.text), inner) || ' '}</Text>)}
		{bottom}
		<Box flexGrow={1} />
		<Text color={THEME.muted} wrap="truncate-end">{fitHint(hint, inner)}</Text>
	</Box>;
}
