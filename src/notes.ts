import path from 'node:path';
import {createHash} from 'node:crypto';
import type {SessionRecord} from './types.js';
import {lineEnd, lineStart, wrapRows, type EditorState} from './textEditor.js';
import {workspaceKey} from './workspace.js';
import {DISPLAY_CONTROL_PATTERN, truncate} from './ui.js';

// Notes as pure data: identities, revisions, Markdown checklists and the Notes tab's layout. The daemon keeps the
// files (src/notesStore.ts); the UI edits them (src/notesFlow.tsx) and renders them (src/notesPane.tsx).

export const MAX_NOTES_CHARS = 50_000;
export const MAX_NOTES_LABEL = '50 000 characters';

/** Which note of the Notes tab: the one shared by the session's worktree (or main checkout), or its own. */
export type NoteSection = 'shared' | 'session';

/** A note's revision: a hash of its text as on disk (a missing file is the empty text). Saves name the one they edited. */
export function noteRevision(text: string): string {
	return createHash('sha256').update(text).digest('hex').slice(0, 16);
}

/** File-name-safe key of the main checkout's (repository) note. */
export function repoNoteId(root: string): string {
	return createHash('sha256').update(path.resolve(root)).digest('hex').slice(0, 16);
}

/**
 * The note shared by every session in the session's workspace: its linked worktree incarnation's (also for a deleted
 * worktree, while the record exists), else the main checkout's repository note; none while its worktree is prepared.
 */
export function sharedNoteIdentity(session: Pick<SessionRecord, 'cwd' | 'launchWorktreeRoot' | 'worktree' | 'requestedWorktreeMode'>): {kind: 'worktree' | 'repo'; id: string} | undefined {
	if (session.worktree?.id) return {kind: 'worktree', id: session.worktree.id};
	const key = workspaceKey(session);
	return key ? {kind: 'repo', id: repoNoteId(key)} : undefined;
}

/** The UI's draft key of a note (shared notes are the same draft for every session showing them). */
export function noteKey(session: SessionRecord, section: NoteSection): string | undefined {
	if (section === 'session') return `session:${session.id}`;
	return session.sharedNotes ? `${session.sharedNotes.kind}:${session.sharedNotes.id}` : undefined;
}

// ── Checklists ────────────────────────────────────────────────────────────────────────────────────────────────

const CHECKLIST = /^([ \t]*)([-*+])([ \t]+)\[([ xX])\](?:[ \t]|$)/;
const BULLET = /^([ \t]*)([-*+])[ \t]+/;

/** `- [ ] item` / `* [x] item` / `+ [X] item`, indented or not. `prefix` is the marker's length (its trailing space too). */
export interface ChecklistItem {indent: string; bullet: string; checked: boolean; prefix: number; text: string; box: number}
export function parseChecklistLine(line: string): ChecklistItem | undefined {
	const match = CHECKLIST.exec(line);
	if (!match) return undefined;
	const [whole, indent, bullet, gap, mark] = match as unknown as [string, string, string, string, string];
	return {indent, bullet, checked: mark !== ' ', prefix: whole.length, text: line.slice(whole.length), box: indent.length + bullet.length + gap.length + 1};
}

export function checklistCounts(text: string | undefined): {open: number; done: number} {
	let open = 0, done = 0;
	for (const line of (text ?? '').split('\n')) {
		const item = parseChecklistLine(line);
		if (item) { if (item.checked) done++; else open++; }
	}
	return {open, done};
}

/** Ctrl+X: checks/unchecks the cursor line's item; a line without one becomes an open item (a plain bullet keeps its bullet). */
export function toggleChecklist(state: EditorState): EditorState {
	const {text, cursor} = state;
	const start = lineStart(text, cursor), line = text.slice(start, lineEnd(text, cursor));
	const item = parseChecklistLine(line);
	if (item) {
		const at = start + item.box;
		return {text: `${text.slice(0, at)}${item.checked ? ' ' : 'x'}${text.slice(at + 1)}`, cursor};
	}
	const bullet = BULLET.exec(line);
	const insertAt = start + (bullet ? bullet[0].length : /^[ \t]*/.exec(line)![0].length);
	const inserted = bullet ? '[ ] ' : '- [ ] ';
	return {text: `${text.slice(0, insertAt)}${inserted}${text.slice(insertAt)}`, cursor: cursor >= insertAt ? cursor + inserted.length : cursor};
}

/** Ctrl+T: a new open item below the cursor's line, with that line's indent (and bullet, for an item). */
export function insertChecklistItem(state: EditorState): EditorState {
	const {text, cursor} = state;
	const start = lineStart(text, cursor), end = lineEnd(text, cursor), line = text.slice(start, end);
	const item = parseChecklistLine(line);
	const indent = item?.indent ?? /^[ \t]*/.exec(line)![0];
	// An empty line takes the item itself.
	if (!line.trim()) return {text: `${text.slice(0, start)}${indent}- [ ] ${text.slice(end)}`, cursor: start + indent.length + 6};
	const inserted = `\n${indent}${item?.bullet ?? '-'} [ ] `;
	return {text: `${text.slice(0, end)}${inserted}${text.slice(end)}`, cursor: end + inserted.length};
}

/**
 * Enter on a checklist item (after its marker) continues the list: a new open item with the rest of the line. Enter on
 * an empty item ends the list (the marker is removed). Undefined: a plain newline.
 */
export function continueChecklist(state: EditorState): EditorState | undefined {
	const {text, cursor} = state;
	const start = lineStart(text, cursor), end = lineEnd(text, cursor);
	const item = parseChecklistLine(text.slice(start, end));
	if (!item || cursor - start < item.prefix) return undefined;
	if (!item.text.trim()) return {text: `${text.slice(0, start)}${text.slice(end)}`, cursor: start};
	const inserted = `\n${item.indent}${item.bullet} [ ] `;
	return {text: `${text.slice(0, cursor)}${inserted}${text.slice(cursor)}`, cursor: cursor + inserted.length};
}

// ── Read mode ─────────────────────────────────────────────────────────────────────────────────────────────────

/** One row of a note as the Notes tab shows it outside editing. */
export interface NoteRow {text: string; kind?: 'open' | 'done' | 'heading' | 'more'}

/** Display text: tabs become spaces, other controls `?`. */
export function visibleText(text: string): string {
	return text.replace(/\t/g, ' ').replace(DISPLAY_CONTROL_PATTERN, '?').replace(/[\u2028\u2029]/g, '?');
}

function wrapped(text: string, width: number): string[] {
	return wrapRows(text, width).map(row => text.slice(row.start, row.end));
}

/** The note word-wrapped at `width`: checklist items as `☐ item` / `☑ item` (hanging indent), headings marked. */
export function noteReadRows(text: string, width: number): NoteRow[] {
	const columns = Math.max(1, width);
	const rows: NoteRow[] = [];
	for (const raw of visibleText(text).split('\n')) {
		const item = parseChecklistLine(raw);
		if (item) {
			const marker = `${item.indent}${item.checked ? '☑' : '☐'} `;
			const kind = item.checked ? 'done' : 'open';
			// Continuation rows hang under the item's text while that leaves a useful width.
			const hang = columns - marker.length >= 8 ? marker.length : 0;
			const body = hang ? wrapped(item.text, columns - hang) : wrapped(`${marker}${item.text}`, columns);
			body.forEach((line, index) => rows.push({text: hang ? `${index ? ' '.repeat(hang) : marker}${line}` : line, kind}));
			continue;
		}
		const heading = /^#{1,6}[ \t]/.test(raw);
		for (const line of wrapped(raw, columns)) rows.push(heading ? {text: line, kind: 'heading'} : {text: line});
	}
	return rows;
}

/** Rows within `height`: from the top; when they do not fit, the last row says how many more there are. */
export function fitReadRows(rows: NoteRow[], height: number): NoteRow[] {
	const room = Math.max(0, height);
	if (rows.length <= room) return rows;
	if (room === 0) return [];
	const shown = rows.slice(0, room - 1);
	const more = rows.length - shown.length;
	return [...shown, {text: `+${more} more line${more === 1 ? '' : 's'}`, kind: 'more'}];
}

// ── Layout ────────────────────────────────────────────────────────────────────────────────────────────────────

/** The smallest body a section with more content than fits gets (unless the pane itself is smaller). */
export const MIN_SECTION_ROWS = 3;

/**
 * Body rows for the two sections of the Notes tab, out of `total`: each section's `need` (its content rows; 1 for an
 * empty, collapsed one). When both fit, each gets its need and the session section the rest; otherwise each gets at
 * least MIN_SECTION_ROWS (or its need), and the rest is split in proportion to what is still missing, the focused
 * section's share counting double. Without a shared section, the session section gets everything.
 */
export function budgetSections(total: number, shared: number | undefined, session: number, focus?: NoteSection): {shared: number; session: number} {
	const rows = Math.max(0, Math.floor(total));
	if (shared === undefined) return {shared: 0, session: rows};
	const needShared = Math.max(1, shared), needSession = Math.max(1, session);
	if (needShared + needSession <= rows) return {shared: needShared, session: rows - needShared};
	const minShared = Math.min(needShared, MIN_SECTION_ROWS), minSession = Math.min(needSession, MIN_SECTION_ROWS);
	if (minShared + minSession > rows) {
		// A tiny pane: the focused section (else the session's) first, one row for the other while there are two.
		const first: NoteSection = focus ?? 'session';
		const otherMin = first === 'shared' ? minSession : minShared;
		const other = rows >= 2 ? Math.max(1, Math.min(otherMin, rows - Math.ceil(rows / 2))) : 0;
		return first === 'shared' ? {shared: rows - other, session: other} : {shared: other, session: rows - other};
	}
	const rest = rows - minShared - minSession;
	const extraShared = needShared - minShared, extraSession = needSession - minSession;
	const weightShared = extraShared * (focus === 'shared' ? 2 : 1), weightSession = extraSession * (focus === 'session' ? 2 : 1);
	let giveShared = weightShared + weightSession > 0 ? Math.min(extraShared, Math.round(rest * weightShared / (weightShared + weightSession))) : 0;
	const giveSession = Math.min(extraSession, rest - giveShared);
	giveShared = Math.min(extraShared, rest - giveSession);
	return {shared: minShared + giveShared, session: minSession + giveSession};
}

/** `☐ 2 open` / `☑ 3 done` for a section header (empty without checklist items). */
export function checklistLabel(text: string | undefined): string {
	const {open, done} = checklistCounts(text);
	if (open) return `☐ ${open} open`;
	return done ? `☑ ${done} done` : '';
}

/** The first of `candidates` that fits `width`, else the last one cut. */
export function fitFirst(candidates: string[], width: number): string {
	return candidates.find(candidate => candidate.length <= width) ?? truncate(candidates.at(-1) ?? '', width);
}

/**
 * The sidebar details' checklist line for the selected session: open items of its note plus its worktree's
 * (`☐ 3 open (2 worktree)`), shortened to fit; undefined without open items.
 */
export function openChecklistText(session: Pick<SessionRecord, 'notes' | 'sharedNotes'>, width: number): string | undefined {
	const own = checklistCounts(session.notes).open, shared = checklistCounts(session.sharedNotes?.text).open;
	const total = own + shared;
	if (!total || width < 1) return undefined;
	return fitFirst([
		...shared ? [`☐ ${total} open (${shared} worktree)`, `☐ ${total} open (${shared} wt)`] : [],
		`☐ ${total} open`, `☐ ${total}`,
	], width);
}
