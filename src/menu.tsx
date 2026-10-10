import React from 'react';
import {Box, Text} from 'ink';
import {THEME, truncate} from './ui.js';

// The one selection style for every picker: a marker plus a full-width inverse, bold bar.
export const SELECTION_MARKER = '❯';

export interface RowStatus {text: string; color?: string}

/**
 * One single-line picker row. `text` is cut to `width` (never wrapped) and padded so the selection bar spans the
 * row; `status` stays visible at the right edge, in its own color unless the row is selected.
 */
export function SelectableRow({selected, text, parts, width, color, selectedColor = THEME.active, dim, status, statusWidth, wrap}: {selected: boolean; text: string; /** Styled segments of `text` (their texts join to it), cut like it; colors apply unless selected. */ parts?: DetailPart[]; width: number; color?: string; selectedColor?: string; dim?: boolean; status?: RowStatus; /** A fixed status column (left-aligned in it) instead of right-aligning the status. */ statusWidth?: number; /** Wrap long text onto aligned continuation rows (for options that must be read in full) instead of cutting it. */ wrap?: boolean}) {
	const inner = Math.max(1, width - 2);
	if (wrap && !status && text.length > inner) {
		return <Box flexDirection="column" width={width}>
			{wrapWords(text, inner).map((line, index) => <Text key={index} inverse={selected} bold={selected} color={selected ? selectedColor : color} dimColor={!selected && dim}>{index === 0 && selected ? SELECTION_MARKER : ' '} {line.padEnd(inner)}</Text>)}
		</Box>;
	}
	const tail = status?.text ? statusWidth !== undefined ? truncate(status.text, Math.min(statusWidth, inner - 1)).padEnd(Math.min(statusWidth, inner - 1)) : truncate(status.text, Math.max(0, Math.floor(inner / 2))) : '';
	const body = truncate(text, Math.max(1, inner - (tail ? tail.length + 2 : 0)));
	const gap = ' '.repeat(Math.max(tail ? 2 : 0, inner - body.length - tail.length));
	return <Text wrap="truncate-end" inverse={selected} bold={selected} color={selected ? selectedColor : color} dimColor={!selected && dim}>
		{selected ? SELECTION_MARKER : ' '} {parts ? fitParts(parts, body.length).map((part, index) => <Text key={index} color={selected ? undefined : part.color} bold={part.bold} italic={part.italic}>{part.text}</Text>) : body}{gap}{tail ? <Text color={selected ? undefined : status?.color}>{tail}</Text> : null}
	</Text>;
}

/**
 * One cell of a grid row (the Settings grid): `parts` cut and padded to `width`. Selected, it is SelectableRow's
 * inverse bold bar, limited to the cell, so the cursor is the cell rather than the row.
 */
export function SelectableCell({selected, parts, width, selectedColor = THEME.active}: {selected: boolean; parts: DetailPart[]; width: number; selectedColor?: string}) {
	const shown = fitParts(parts, width);
	const used = shown.reduce((sum, part) => sum + part.text.length, 0);
	return <Text inverse={selected} bold={selected} color={selected ? selectedColor : undefined}>
		{shown.map((part, index) => <Text key={index} color={selected ? undefined : part.color} bold={part.bold} italic={part.italic}>{part.text}</Text>)}{' '.repeat(Math.max(0, width - used))}
	</Text>;
}

export interface MenuItem {key: string; label: string; description?: string; status?: RowStatus; color?: string; selectedColor?: string; dim?: boolean}

/** First visible index that keeps `selected` on screen within `rows`. */
export function menuWindowStart(selected: number, count: number, rows: number): number {
	if (count <= rows) return 0;
	return Math.max(0, Math.min(count - rows, selected - Math.floor(rows / 2)));
}

/**
 * The first visible row of a scrolled list of `total` rows showing `rows` at a time, from the last one (`top`), so the
 * selection (`current`, -1 for none) stays on screen. Moving up onto a row also shows the rows right above it that
 * cannot be selected (its group's heading), as far as the selection stays visible: scrolling only to the selected row
 * hid the heading of the first group after scrolling down and back up.
 */
export function scrolledListTop(top: number, current: number, rows: number, total: number, selectable: (index: number) => boolean): number {
	if (current >= 0) {
		if (current < top) {
			let first = current;
			while (first > 0 && !selectable(first - 1)) first--;
			top = Math.max(first, current - rows + 1);
		}
		if (current >= top + rows) top = current - rows + 1;
	}
	return Math.max(0, Math.min(top, Math.max(0, total - rows)));
}

/**
 * A vertical list of SelectableRows: an aligned label column, then the description cut to fit. With `rows`, only a
 * window around the selection is shown (with ↑/↓ more markers), so short terminals clip instead of overflowing.
 */
export function MenuList({items, selected, width, rows, labelWidth}: {items: MenuItem[]; selected: number; width: number; rows?: number; labelWidth?: number}) {
	const column = labelWidth ?? Math.min(Math.max(0, ...items.map(item => item.label.length)), Math.max(8, Math.floor(width / 3)));
	const limit = rows === undefined ? items.length : Math.max(1, rows);
	// The ↑/↓ more markers take a row each, so they appear only when at least one item row remains beside them.
	const windowed = items.length > limit && limit >= 3;
	const visibleRows = windowed ? limit - 2 : limit;
	const start = menuWindowStart(selected, items.length, visibleRows);
	const shown = items.slice(start, start + visibleRows);
	return <Box flexDirection="column" width={width}>
		{windowed ? <Text color={THEME.muted}>{start > 0 ? `  ↑ ${start} more` : ' '}</Text> : null}
		{shown.map((item, offset) => {
			// A description cut to a few words reads worse than none (the details box has the full text).
			const room = width - 2 - column - 2 - (item.status?.text ? item.status.text.length + 2 : 0);
			const label = item.description && (item.description.length <= room || room >= 24) ? `${truncate(item.label, column).padEnd(column)}  ${item.description}` : item.label;
			return <SelectableRow key={item.key} selected={start + offset === selected} text={label} width={width} color={item.color} selectedColor={item.selectedColor} dim={item.dim} status={item.status} />;
		})}
		{windowed ? <Text color={THEME.muted}>{start + visibleRows < items.length ? `  ↓ ${items.length - start - visibleRows} more` : ' '}</Text> : null}
	</Box>;
}

/** Word-wraps `text` to `width` columns (long words are cut), so prose never runs past a box edge. */
export function wrapWords(text: string, width: number): string[] {
	const columns = Math.max(1, width), lines: string[] = [];
	let line = '';
	for (const word of text.split(/\s+/).filter(Boolean)) {
		if (!line) line = word;
		else if (line.length + 1 + word.length <= columns) line = `${line} ${word}`;
		else { lines.push(line); line = word; }
		while (line.length > columns) { lines.push(line.slice(0, columns)); line = line.slice(columns); }
	}
	return line ? [...lines, line] : lines.length ? lines : [''];
}

export interface DetailPart {text: string; color?: string; bold?: boolean; italic?: boolean}
export interface DetailLine {text: string; color?: string; /** Paths and values are cut instead of wrapped. */ nowrap?: boolean; /** Colored segments of a nowrap line (their texts join to `text`). */ parts?: DetailPart[]}
/** Segments cut to `width` columns in total, in order. */
export function fitParts(parts: DetailPart[], width: number): DetailPart[] {
	const shown: DetailPart[] = [];
	let room = Math.max(0, width);
	for (const part of parts) {
		if (room <= 0) break;
		const text = part.text.length > room ? truncate(part.text, room) : part.text;
		shown.push({...part, text}); room -= text.length;
	}
	return shown;
}
/** One detail line: plain text in one color, or its colored segments. */
export function DetailText({line, width, color}: {line: Pick<DetailLine, 'text' | 'color' | 'parts'>; width: number; color?: string}) {
	if (!line.parts) return <Text color={line.color ?? color ?? THEME.muted} wrap="truncate-end">{truncate(line.text, width)}</Text>;
	return <Text color={line.color ?? color ?? THEME.muted} wrap="truncate-end">{fitParts(line.parts, width).map((part, index) => <Text key={index} color={part.color} bold={part.bold} italic={part.italic}>{part.text}</Text>)}</Text>;
}
/** A titled box describing the selected menu item. Prose wraps inside the box; `rows` (border included) caps it. */
export function MenuDetails({title, lines, width, rows}: {title: string; lines: DetailLine[]; width: number; rows?: number}) {
	const inner = Math.max(1, width - 4);
	const wrapped = lines.flatMap(line => line.parts ? [line] : (line.nowrap ? [truncate(line.text, inner)] : wrapWords(line.text, inner)).map(text => ({text, color: line.color})));
	const shown = rows === undefined ? wrapped : wrapped.slice(0, Math.max(0, rows - 3));
	return <Box flexDirection="column" width={width} borderStyle="round" borderColor={THEME.border} paddingX={1}>
		<Text color={THEME.accentSoft} bold wrap="truncate-end">{truncate(title, inner)}</Text>
		{shown.map((line, index) => <DetailText key={index} line={line} width={inner} />)}
	</Box>;
}

export interface HintPart {text: string; short?: string; /** Higher numbers are dropped first when the hint does not fit. */ drop?: number}
/** One key-hint line that fits `width`: full texts, then short forms, then dropping the least important parts. */
export function fitHint(parts: Array<string | HintPart>, width: number, separator = ' · '): string {
	let current = parts.map((part, order) => typeof part === 'string' ? {text: part, order} : {...part, order});
	const join = (list: typeof current, short: boolean) => list.map(part => short ? part.short ?? part.text : part.text).join(separator);
	if (join(current, false).length <= width) return join(current, false);
	while (current.length > 1 && join(current, true).length > width) {
		const victim = current.reduce((worst, part) => (part.drop ?? 0) > (worst.drop ?? 0) || ((part.drop ?? 0) === (worst.drop ?? 0) && part.order > worst.order) ? part : worst);
		current = current.filter(part => part !== victim);
	}
	return truncate(join(current, true), width);
}

export interface MenuPaneLine {text: string; color?: string}
/** Rows a MenuDetails box needs for `lines` at `width` (border and title included). */
export function menuDetailsRows(lines: DetailLine[], width: number): number {
	const inner = Math.max(1, width - 4);
	return 3 + lines.reduce((sum, line) => sum + (line.nowrap || line.parts ? 1 : wrapWords(line.text, inner).length), 0);
}
/**
 * A boxed picker: title, optional subtitle lines, the menu, a details box for the selected item and one hint line.
 * Rows are budgeted from `height`: the details box shrinks (or disappears) and the menu scrolls before anything overflows.
 */
export function MenuPane({title, subtitle = [], items, selected, details, empty, hint, width, height, labelWidth}: {title: string; subtitle?: MenuPaneLine[]; items: MenuItem[]; selected: number; details?: {title: string; lines: DetailLine[]}; empty?: string; hint: Array<string | HintPart>; width: number; height: number; labelWidth?: number}) {
	const inner = Math.max(1, width - 4);
	// Border (2), title and hint rows are fixed.
	const free = Math.max(1, height - 4);
	const head = subtitle.slice(0, Math.max(0, free - 1));
	// A blank row separates the subtitle from the menu when the menu keeps at least one row.
	const spacer = head.length > 0 && free - head.length > 1 ? 1 : 0;
	const menuRows = Math.max(1, Math.min(items.length || 1, free - head.length - spacer));
	// A blank row, the details box border (2) and its title, then as many lines as fit.
	const detailsRows = details ? free - head.length - spacer - menuRows - 1 : 0;
	const showDetails = Boolean(details) && detailsRows >= 4;
	return <Box flexDirection="column" width={width} height={height} borderStyle="round" borderColor={THEME.borderActive} paddingX={1}>
		<Text color={THEME.accent} bold wrap="truncate-end">{truncate(title, inner)}</Text>
		{head.map((line, index) => <Text key={`s${index}`} color={line.color ?? THEME.muted} wrap="truncate-end">{truncate(line.text, inner)}</Text>)}
		{spacer ? <Text> </Text> : null}
		{items.length
			? <MenuList items={items} selected={selected} width={inner} rows={menuRows} labelWidth={labelWidth} />
			: <Text color={THEME.muted} wrap="truncate-end">{truncate(empty ?? 'Nothing to choose.', inner)}</Text>}
		{showDetails && details ? <Box marginTop={1}><MenuDetails title={details.title} lines={details.lines} width={inner} rows={Math.min(menuDetailsRows(details.lines, inner), detailsRows)} /></Box> : null}
		<Box flexGrow={1} />
		<Text color={THEME.muted} wrap="truncate-end">{fitHint(hint, inner)}</Text>
	</Box>;
}
