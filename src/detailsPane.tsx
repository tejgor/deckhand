import React, {useMemo} from 'react';
import {Box, Text, type Key} from 'ink';
import {DISPLAY_CONTROL_PATTERN, THEME} from './ui.js';

// Border (2) + title + footer + scroll status rows around the scrollable body.
const DETAILS_CHROME_ROWS = 5;
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

export function DetailsPane({title, text, footer, width, height, scroll = 0}: {title: string; text: string; footer: string; width: number; height: number; scroll?: number}) {
	const viewport = useMemo(() => detailsViewport(text, width, height), [text, width, height]);
	const {lines, rows, max} = viewport;
	const start = clampScroll(scroll, max);
	return <Box flexDirection="column" width={width} height={height} borderStyle="round" borderColor={THEME.borderActive} paddingX={1}>
		<Text color={THEME.accent} bold wrap="truncate-end">{title}</Text>
		{lines.slice(start, start + rows).map((line, index) => <Text key={index}>{line}</Text>)}
		<Text color={THEME.muted} wrap="truncate-end">{footer}</Text>
		<Text color={THEME.muted} wrap="truncate-end">{lines.length > rows ? `↑↓/PgUp/PgDn scroll (${start + 1}/${lines.length}) · ` : ''}esc return</Text>
	</Box>;
}
