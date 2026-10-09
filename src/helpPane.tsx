import React, {useRef, useState} from 'react';
import {Box, Text, type Key} from 'ink';
import {HELP_TOPICS, type HelpLine, type HelpTopic} from './help.js';
import {SELECTION_MARKER, fitHint, fitParts, wrapWords, type DetailPart} from './menu.js';
import {THEME, truncate} from './ui.js';

// The ? guide: a topic list beside the selected topic, each topic a table of key → what it does with a few notes;
// / searches every topic. Narrow panes drop the list and show "◂ topic ▸" in the title instead.

/** From this inner width the topic list is shown beside the content. */
const LIST_WIDTH = 64;
const MAX_KEY_WIDTH = 15;
type Row = DetailPart[];

const matches = (line: HelpLine, query: string) => 'heading' in line ? false : ('note' in line ? line.note : `${line.keys} ${line.text}`).toLowerCase().includes(query);
/** The topics with their lines matching `query` (case-insensitive), in order; headings are dropped. */
export function searchHelp(query: string, topics: readonly HelpTopic[] = HELP_TOPICS): HelpTopic[] {
	const needle = query.trim().toLowerCase();
	if (!needle) return [];
	return topics.map(topic => ({title: topic.title, lines: topic.lines.filter(line => matches(line, needle) || topic.title.toLowerCase().includes(needle) && !('heading' in line))})).filter(topic => topic.lines.length);
}

/**
 * Pure: a topic's lines laid out at `width` columns. Keys sit in one aligned, highlighted column (a key too long for it
 * gets its own row); descriptions and notes wrap with a hanging indent; a blank row separates notes from key rows.
 */
export function helpRows(lines: readonly HelpLine[], width: number): Row[] {
	const keyRows = lines.filter((line): line is Extract<HelpLine, {keys: string}> => 'keys' in line);
	// Sized by the keys that fit the column; a longer one gets its own row instead of widening every row.
	const keyWidth = Math.max(1, ...keyRows.map(line => line.keys.length).filter(length => length <= MAX_KEY_WIDTH));
	const textWidth = Math.max(10, width - keyWidth - 2);
	const rows: Row[] = [];
	let previous: 'key' | 'note' | 'heading' | undefined;
	for (const line of lines) {
		const kind = 'keys' in line ? 'key' : 'note' in line ? 'note' : 'heading';
		if (rows.length && (kind === 'heading' || (previous !== 'heading' && previous !== kind))) rows.push([{text: ''}]);
		previous = kind;
		if ('heading' in line) { rows.push([{text: line.heading, color: THEME.accentSoft, bold: true}]); continue; }
		if ('note' in line) { for (const text of wrapWords(line.note, width)) rows.push([{text, color: THEME.muted}]); continue; }
		const text = wrapWords(line.text, textWidth);
		const indent = ' '.repeat(keyWidth + 2);
		if (line.keys.length > keyWidth) rows.push([{text: line.keys, color: THEME.active, bold: true}]);
		else rows.push([{text: line.keys.padEnd(keyWidth + 2), color: THEME.active, bold: true}, {text: text.shift() ?? ''}]);
		for (const rest of text) rows.push([{text: indent}, {text: rest}]);
	}
	return rows;
}
/** Search results: each matching topic's title (with its number), then its matching lines. */
function searchRows(query: string, width: number): Row[] {
	const found = searchHelp(query);
	if (!found.length) return [[{text: query.trim() ? `Nothing matches "${query.trim()}".` : 'Type to search every topic.', color: THEME.muted}]];
	return found.flatMap((topic, index) => [
		...index ? [[{text: ''}]] : [],
		[{text: `${HELP_TOPICS.findIndex(entry => entry.title === topic.title) + 1}  ${topic.title}`, color: THEME.accent, bold: true}],
		...helpRows(topic.lines, width),
	]);
}

export interface HelpView {
	/** ? : open on the first topic (or `topic`), not searching. */
	open(topic?: number): void;
	/** Keys while the guide is open; 'close' when it should close. */
	handleInput(input: string, key: Partial<Key>): 'close' | undefined;
	render(width: number, height: number): React.ReactNode;
}
export function useHelp(): HelpView {
	const [topic, setTopic] = useState(0);
	const [scroll, setScroll] = useState(0);
	// undefined: browsing topics. A string: search results (typing while `typing`).
	const [query, setQuery] = useState<string>();
	const [typing, setTyping] = useState(false);
	// The last render's scroll range and page size, so keys can clamp.
	const view = useRef({max: 0, page: 1});

	const open = (index = 0) => { setTopic(index); setScroll(0); setQuery(undefined); setTyping(false); };
	const pick = (index: number) => { setTopic((index + HELP_TOPICS.length) % HELP_TOPICS.length); setScroll(0); };
	const scrollBy = (delta: number) => setScroll(current => Math.max(0, Math.min(view.current.max, Math.min(current, view.current.max) + delta)));

	const handleInput = (input: string, key: Partial<Key>): 'close' | undefined => {
		if (key.pageDown || (input === ' ' && !typing)) { scrollBy(view.current.page); return; }
		if (key.pageUp) { scrollBy(-view.current.page); return; }
		if (query !== undefined) {
			if (key.escape) { setQuery(undefined); setTyping(false); setScroll(0); return; }
			if (key.upArrow) { scrollBy(-1); return; }
			if (key.downArrow) { scrollBy(1); return; }
			if (typing) {
				if (key.return || key.tab) setTyping(false);
				else if (key.backspace || key.delete) { setQuery(query.slice(0, -1)); setScroll(0); }
				else if (input && !key.ctrl && !key.meta && !/[\u0000-\u001f]/.test(input)) { setQuery(query + input); setScroll(0); }
				return;
			}
			if (input === 'k') scrollBy(-1);
			else if (input === 'j') scrollBy(1);
			else if (input === '/') setTyping(true);
			else if (input === '?') return 'close';
			return;
		}
		if (key.escape || input === '?') return 'close';
		if (input === '/') { setQuery(''); setTyping(true); setScroll(0); return; }
		if (key.upArrow || key.leftArrow || input === 'k' || input === 'h' || (key.tab && key.shift)) { pick(topic - 1); return; }
		if (key.downArrow || key.rightArrow || input === 'j' || input === 'l' || key.tab) { pick(topic + 1); return; }
		if (key.home) { pick(0); return; }
		if (key.end) { pick(HELP_TOPICS.length - 1); return; }
		if (/^[1-9]$/.test(input) && Number(input) <= HELP_TOPICS.length) { pick(Number(input) - 1); return; }
		if (input === '0' && HELP_TOPICS.length >= 10) pick(9);
		return;
	};

	const render = (width: number, height: number) => <HelpPane topic={topic} scroll={scroll} query={query} typing={typing} width={width} height={height} onLayout={layout => { view.current = layout; }} />;
	return {open, handleInput, render};
}

export function HelpPane({topic, scroll, query, typing, width, height, onLayout}: {topic: number; scroll: number; query?: string; typing?: boolean; width: number; height: number; onLayout?: (layout: {max: number; page: number}) => void}) {
	const inner = Math.max(1, width - 4);
	const list = inner >= LIST_WIDTH;
	// Marker, a two-digit number, a space, the title, then the list's right border and one column of air.
	const listWidth = list ? Math.max(...HELP_TOPICS.map(entry => entry.title.length)) + 6 : 0;
	// The list's right border (1) and a gap (1) separate it from the content.
	const contentWidth = Math.max(10, inner - (list ? listWidth + 2 : 0));
	const current = HELP_TOPICS[topic]!;
	const searching = query !== undefined;
	const rows = searching ? searchRows(query, contentWidth) : helpRows(current.lines, contentWidth);
	// Border (2), title, hint; the search line when searching.
	const page = Math.max(1, height - 4 - (searching ? 2 : 0));
	const max = Math.max(0, rows.length - page);
	const start = Math.min(scroll, max);
	onLayout?.({max, page});

	const position = max ? `${start + page < rows.length ? '↓ ' : ''}${Math.min(rows.length, start + page)}/${rows.length}` : '';
	const title = searching ? 'Help · search' : list ? `Help · ${current.title}` : `Help · ◂ ${current.title} (${topic + 1}/${HELP_TOPICS.length}) ▸`;
	const heading = truncate(title, Math.max(1, inner - (position ? position.length + 2 : 0)));
	const hint = searching
		? typing ? ['type to search', {text: '↑↓ scroll', drop: 1}, 'enter done', 'esc clear'] : [{text: '↑↓/PgUp/PgDn scroll', short: '↑↓ scroll'}, '/ edit search', 'esc clear', '? close']
		: [{text: list ? '↑↓ topic' : '↑↓/←→ topic', short: '↑↓ topic'}, {text: '1-9 jump', drop: 2}, ...max ? [{text: 'PgUp/PgDn/space scroll', short: 'PgDn scroll', drop: 1}] : [], '/ search', 'esc close'];
	const content = rows.slice(start, start + page).map((row, index) => <Text key={index} wrap="truncate-end">{fitParts(row, contentWidth).map((part, at) => <Text key={at} color={part.color} bold={part.bold}>{part.text}</Text>)}{row.length && row.every(part => !part.text) ? ' ' : ''}</Text>);
	const searchLine = searching ? [
		<Text key="search" wrap="truncate-end"><Text color={THEME.accent}>/ </Text>{truncate(query, Math.max(1, contentWidth - 4))}{typing ? <Text inverse> </Text> : null}</Text>,
		<Text key="gap"> </Text>,
	] : [];
	const found = searching ? new Set(searchHelp(query).map(entry => entry.title)) : undefined;

	return <Box flexDirection="column" width={width} height={height} borderStyle="round" borderColor={THEME.borderActive} paddingX={1}>
		<Text wrap="truncate-end"><Text color={THEME.accent} bold>{heading}</Text>{position ? <Text color={THEME.muted}>{' '.repeat(Math.max(2, inner - heading.length - position.length))}{position}</Text> : null}</Text>
		<Box flexGrow={1}>
			{list ? <Box flexDirection="column" width={listWidth} borderStyle="single" borderColor={THEME.border} borderTop={false} borderBottom={false} borderLeft={false} marginRight={1}>
				{HELP_TOPICS.map((entry, index) => {
					const selected = !searching && index === topic;
					const dim = found ? !found.has(entry.title) : false;
					return <Text key={entry.title} wrap="truncate-end" bold={selected} inverse={selected} color={selected ? THEME.active : dim ? THEME.muted : undefined}>
						{selected ? SELECTION_MARKER : ' '}{`${index + 1}`.padStart(2)} {truncate(entry.title, Math.max(1, listWidth - 6)).padEnd(Math.max(1, listWidth - 6))}
					</Text>;
				})}
			</Box> : null}
			<Box flexDirection="column" width={contentWidth}>
				{searchLine}
				{content}
			</Box>
		</Box>
		<Text color={THEME.muted} wrap="truncate-end">{fitHint(hint, inner)}</Text>
	</Box>;
}
