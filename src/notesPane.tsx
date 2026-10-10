import React from 'react';
import {Box, Text} from 'ink';
import {budgetSections, checklistLabel, fitFirst, fitReadRows, noteReadRows, visibleText, type NoteSection, type TaskLinkLookup} from './notes.js';
import {scrollTopFor, wrappedEditorLines} from './textEditor.js';
import {THEME, truncate} from './ui.js';

// The Notes tab: the worktree's shared note above the session's own, each with a header, separated by a rule. Read
// mode shows rendered checklists from the top (`+N more lines` when cut); the section being edited shows the raw
// Markdown with a cursor, scrolled to keep it visible. Layout is pure (`notesLayout`), so it is unit-tested.

export interface NotesSectionInput {
	/** Header, longest first; the first that fits (with the checklist count) is shown. */
	titles: string[];
	text: string;
	/** The one muted line of an empty section that is not being edited. */
	empty: string;
	/** Shown after the title (warning color): read-only, too large. */
	flag?: string;
	/** Being edited: the cursor and the first row shown before this render. */
	editing?: {cursor: number; scrollTop: number};
	/** The section E / Ctrl+O open (header in bold). */
	active?: boolean;
}

export interface NotesPart {text: string; color?: string; bold?: boolean; dim?: boolean; inverse?: boolean}
export type NotesRow = NotesPart[];

function header(section: NotesSectionInput, width: number, focused: boolean): NotesRow {
	const count = checklistLabel(section.text);
	const tail = [section.flag, count].filter(Boolean).join(' · ');
	const titleWidth = tail ? Math.max(1, width - tail.length - 3) : width;
	const title = fitFirst(section.titles, titleWidth);
	const parts: NotesRow = [{text: title, color: focused ? THEME.active : THEME.accentSoft, bold: focused || section.active}];
	if (tail && title.length + 3 + tail.length <= width) {
		if (section.flag) parts.push({text: ' · ', color: THEME.muted}, {text: section.flag, color: THEME.warn});
		if (count) parts.push({text: ' · ', color: THEME.muted}, {text: count, color: THEME.muted});
	}
	return parts;
}

/** Body rows a section needs at `width`: the editor's rows, one for an empty section, else its rendered rows. */
function need(section: NotesSectionInput, width: number, links?: TaskLinkLookup): number {
	if (section.editing) return wrappedEditorLines({text: section.text, cursor: section.editing.cursor}, width).lines.length;
	return section.text.trim() ? noteReadRows(section.text, width, links).length : 1;
}

function body(section: NotesSectionInput, width: number, rows: number, links?: TaskLinkLookup): {rows: NotesRow[]; scrollTop?: number} {
	if (rows <= 0) return {rows: []};
	if (section.editing) {
		const {lines, cursorRow} = wrappedEditorLines({text: section.text, cursor: section.editing.cursor}, width);
		const scrollTop = scrollTopFor(section.editing.scrollTop, cursorRow, rows, lines.length);
		const shown = lines.slice(scrollTop, scrollTop + rows).map((line): NotesRow => [
			{text: visibleText(line.before)},
			...line.cursor !== undefined ? [{text: visibleText(line.cursor), inverse: true}] : [],
			...line.after ? [{text: visibleText(line.after)}] : [],
		]);
		return {rows: shown, scrollTop};
	}
	if (!section.text.trim()) return {rows: [[{text: truncate(section.empty, width), color: THEME.muted}]]};
	return {rows: fitReadRows(noteReadRows(section.text, width, links), rows).map((row): NotesRow => [
		row.kind === 'done' ? {text: row.text, color: THEME.muted} : row.kind === 'link' ? {text: row.text, color: THEME.accentSoft} : row.kind === 'more' ? {text: row.text, color: THEME.muted, dim: true} : row.kind === 'heading' ? {text: row.text, bold: true} : {text: row.text},
	])};
}

/**
 * The tab's rows, exactly `height` of them: the shared section (when the session has one), a rule, the session's
 * (when it shows one: a session in a worktree has only the worktree's note).
 * `scrollTop` is the edited section's first row, for the next render; `bodies` the rows each section's body got.
 */
export function notesLayout({shared, session, width, height, focus, links}: {shared?: NotesSectionInput; session?: NotesSectionInput; width: number; height: number; focus?: NoteSection; /** Whether each linked task is open, done or gone (`↗` lines say so). */ links?: TaskLinkLookup}): {rows: NotesRow[]; scrollTop?: number; bodies: {shared: number; session: number}} {
	const columns = Math.max(1, width);
	// A session in a worktree shows only the worktree's note (no session section).
	const headers = shared && session ? 3 : 1;
	const budget = shared && !session ? {shared: Math.max(0, height - headers), session: 0}
		: budgetSections(Math.max(0, height - headers), shared && need(shared, columns, links), need(session!, columns, links), focus);
	const rows: NotesRow[] = [];
	let scrollTop: number | undefined;
	const add = (section: NotesSectionInput, which: NoteSection, count: number) => {
		rows.push(header(section, columns, focus === which));
		const shown = body(section, columns, count, links);
		if (shown.scrollTop !== undefined) scrollTop = shown.scrollTop;
		rows.push(...shown.rows);
		for (let index = shown.rows.length; index < count; index++) rows.push([{text: ' '}]);
	};
	if (shared) {
		add(shared, 'shared', budget.shared);
		if (session) rows.push([{text: '─'.repeat(columns), color: THEME.border}]);
	}
	if (session) add(session, 'session', budget.session);
	return {rows: rows.slice(0, Math.max(1, height)), scrollTop, bodies: budget};
}

export function NotesRows({rows, width, height}: {rows: NotesRow[]; width: number; height: number}) {
	return (
		<Box flexDirection="column" width={width} height={height}>
			{rows.map((row, index) => (
				<Text key={`notes-row-${index}`} wrap="truncate-end">
					{/* Ink drops an empty row, so a blank one renders as a space. */}
					{row.some(part => part.text) ? row.map((part, at) => <Text key={at} color={part.color} bold={part.bold} dimColor={part.dim} inverse={part.inverse}>{part.text}</Text>) : ' '}
				</Text>
			))}
		</Box>
	);
}

export function NotesMessage({text, width, height}: {text: string; width: number; height: number}) {
	return <Box flexDirection="column" width={width} height={height}><Text color={THEME.muted}>{truncate(text, width)}</Text></Box>;
}
