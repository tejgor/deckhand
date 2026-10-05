import React, {useMemo} from 'react';
import {Box, Text, type Key} from 'ink';
import {fitHint, type HintPart} from './menu.js';
import {DISPLAY_CONTROL_PATTERN, THEME, truncate} from './ui.js';

// Border (2) + title + hint rows around the scrollable body.
const DETAILS_CHROME_ROWS = 4;
// Border (2) + paddingX (2) columns.
const DETAILS_CHROME_COLUMNS = 4;

// Bidi overrides/isolates, zero-width and line-separator characters can make
// reviewed text read differently from the bytes that execute. Show them.
const INVISIBLE_PATTERN = /[\u061C\u180E\u200B-\u200F\u2028-\u202E\u2060-\u2064\u2066-\u2069\uFEFF]/g;

function codePointEscape(character: string): string {
	return `<U+${character.codePointAt(0)!.toString(16).toUpperCase().padStart(4, '0')}>`;
}

// Never silently drop characters: details panes include the trust review.
export function visibleDetailText(text: string): string {
	return text.replace(/\t/g, '    ').replace(DISPLAY_CONTROL_PATTERN, codePointEscape).replace(INVISIBLE_PATTERN, codePointEscape);
}

// Wrap by code points so surrogate pairs are never split across rows.
export function detailLines(text: string, width: number): string[] {
	const columns = Math.max(1, width);
	return visibleDetailText(text).split('\n').flatMap(line => {
		const characters = Array.from(line);
		if (characters.length === 0) return [' '];
		const result: string[] = [];
		for (let index = 0; index < characters.length; index += columns) result.push(characters.slice(index, index + columns).join(''));
		return result;
	});
}

export interface DetailsViewport {lines: string[]; rows: number; max: number}

export function detailsViewport(text: string, width: number, height: number): DetailsViewport {
	const lines = detailLines(text, Math.max(1, width - DETAILS_CHROME_COLUMNS));
	const rows = Math.max(1, height - DETAILS_CHROME_ROWS);
	return {lines, rows, max: Math.max(0, lines.length - rows)};
}

export function clampScroll(scroll: number, max: number): number {
	return Math.max(0, Math.min(max, scroll));
}

// Shared details-pane scrolling: ↑↓ j k, PgUp/PgDn, Home/End. Returns
// undefined when the key is not a scroll key so callers can handle it.
export function scrollDetails(scroll: number, input: string, key: Partial<Key>, viewport: Pick<DetailsViewport, 'rows' | 'max'>): number | undefined {
	const current = clampScroll(scroll, viewport.max);
	if (key.home) return 0;
	if (key.end) return viewport.max;
	if (key.pageUp) return clampScroll(current - viewport.rows, viewport.max);
	if (key.pageDown) return clampScroll(current + viewport.rows, viewport.max);
	if (key.upArrow || (input === 'k' && !key.ctrl && !key.meta)) return clampScroll(current - 1, viewport.max);
	if (key.downArrow || (input === 'j' && !key.ctrl && !key.meta)) return clampScroll(current + 1, viewport.max);
	return undefined;
}

/** A scrollable text pane with one hint line; `footer` lists its keys (each screen's only hint line). */
export function DetailsPane({title, text, footer, width, height, scroll = 0}: {title: string; text: string; footer: string | Array<string | HintPart>; width: number; height: number; scroll?: number}) {
	const viewport = useMemo(() => detailsViewport(text, width, height), [text, width, height]);
	const {lines, rows, max} = viewport;
	const start = clampScroll(scroll, max);
	const inner = Math.max(1, width - DETAILS_CHROME_COLUMNS);
	const scrollable = lines.length > rows;
	// The position sits at the right of the title; the hint names the scroll keys unless the footer already does.
	const position = scrollable ? `${Math.min(lines.length, start + rows)}/${lines.length}` : '';
	const parts = typeof footer === 'string' ? [footer] : footer;
	const hint = fitHint(scrollable && !parts.some(part => /scroll/.test(typeof part === 'string' ? part : part.text)) ? [...parts, {text: '↑↓/PgUp/PgDn scroll', short: '↑↓ scroll', drop: 1}] : parts, inner);
	const heading = truncate(title, Math.max(1, inner - (position ? position.length + 2 : 0)));
	return <Box flexDirection="column" width={width} height={height} borderStyle="round" borderColor={THEME.borderActive} paddingX={1}>
		<Text wrap="truncate-end"><Text color={THEME.accent} bold>{heading}</Text>{position ? <Text color={THEME.muted}>{' '.repeat(Math.max(2, inner - heading.length - position.length))}{position}</Text> : null}</Text>
		{lines.slice(start, start + rows).map((line, index) => <Text key={index}>{line}</Text>)}
		<Box flexGrow={1} />
		<Text color={THEME.muted} wrap="truncate-end">{hint}</Text>
	</Box>;
}
