import type {SessionRecord} from './types.js';
import {groupTasks, isLinked, openNoteItems, parseNoteTaskLink, type Task} from './tasks.js';
import {noteKey, parseChecklistLine} from './notes.js';
import {statusWords} from './sidebarModel.js';
import {THEME, displaySessionTitle, statusColor, statusGlyph} from './ui.js';

// The Tasks board (b) as pure data: its rows (groups, tasks, the folded older done ones, and the open note items not
// yet tasks), each task's linked sessions and the state shown for it. src/tasksFlow.tsx keeps the selection and keys.

export type TaskGroup = 'progress' | 'backlog' | 'done';

/** An open checklist item of a note (not a task), listed under the board's `tab` so nothing hides in notes. */
export interface NoteItem {
	key: string;
	title: string;
	sessionId: string;
	section: 'session' | 'shared';
	/** The note's line of the item (`promote-note-item`) and the revision it was read at. */
	line: number;
	revision: string;
	noteId?: string;
	source: string;
}

export type BoardRow =
	| {kind: 'heading'; text: string; count?: string}
	| {kind: 'task'; task: Task; group: TaskGroup}
	| {kind: 'older'; count: number; shown: boolean}
	| {kind: 'empty'; text: string}
	| {kind: 'note'; item: NoteItem};

export const selectableRow = (row: BoardRow) => row.kind === 'task' || row.kind === 'older' || row.kind === 'note';
export const rowKey = (row: BoardRow): string | undefined => row.kind === 'task' ? `task:${row.task.id}` : row.kind === 'older' ? 'older' : row.kind === 'note' ? `note:${row.item.key}` : undefined;

/** The board's rows: in progress, backlog, done this week (older ones folded unless `showOlder`). */
export function boardRows(tasks: Task[], showOlder: boolean, now = new Date()): BoardRow[] {
	const groups = groupTasks(tasks, now);
	const rows: BoardRow[] = [];
	const section = (text: string, list: Task[], group: TaskGroup, empty?: string) => {
		rows.push({kind: 'heading', text, count: list.length ? String(list.length) : undefined});
		for (const task of list) rows.push({kind: 'task', task, group});
		if (!list.length && empty) rows.push({kind: 'empty', text: empty});
	};
	section('IN PROGRESS', groups.progress, 'progress', 'n on a backlog task starts a session for it');
	section('BACKLOG', groups.backlog, 'backlog', tasks.length ? 'Nothing waiting' : 'No tasks yet · a adds one');
	if (groups.done.length || groups.olderDone.length) {
		section('DONE · this week', groups.done, 'done');
		if (groups.olderDone.length) {
			rows.push({kind: 'older', count: groups.olderDone.length, shown: showOlder});
			if (showOlder) for (const task of groups.olderDone) rows.push({kind: 'task', task, group: 'done'});
		}
	}
	return rows;
}

/** The sessions doing a task: every session of its linked worktree incarnation, or its linked (main checkout) session. */
export function taskSessions(task: Pick<Task, 'meta'>, sessions: SessionRecord[]): SessionRecord[] {
	if (task.meta.wt) return sessions.filter(session => session.worktree?.id === task.meta.wt);
	if (task.meta.s) return sessions.filter(session => session.id === task.meta.s);
	return [];
}

function urgency(session: SessionRecord): number {
	const attention = session.status === 'running' ? session.attention?.state : undefined;
	if (attention === 'needs-input' || attention === 'failed' || attention === 'limited') return 3;
	if (session.status === 'running' || session.status === 'starting') return session.agentStatus === 'active' ? 2 : 1;
	return 0;
}

/** The session whose state a task shows: the most urgent (needs you, working, idle, exited). */
export function leadSession(sessions: SessionRecord[]): SessionRecord | undefined {
	return sessions.reduce<SessionRecord | undefined>((best, session) => !best || urgency(session) > urgency(best) ? session : best, undefined);
}

/** The open task linked to the session's work (its worktree incarnation, else the session), else a done one. */
export function linkedTask(tasks: Task[], session: SessionRecord | undefined): Task | undefined {
	if (!session) return undefined;
	const linked = tasks.filter(task => isLinked(task) && (task.meta.wt ? task.meta.wt === session.worktree?.id : task.meta.s === session.id));
	return linked.find(task => !task.done) ?? linked[0];
}

export interface TaskState {glyph: string; color: string; text: string; where?: string}

/** What a task row shows on its right: its work's state and branch, where it came from, or when it was done. */
export function taskState(task: Task, sessions: SessionRecord[], spinnerFrame: string, allSessions: SessionRecord[] = sessions): TaskState {
	if (task.done) return {glyph: '✓', color: THEME.success, text: task.meta.auto === 'merge' ? 'merged' : task.meta.auto === 'done' ? 'done (D)' : 'done', ...task.meta.done ? {where: task.meta.done} : {}};
	if (!isLinked(task)) return {glyph: '○', color: THEME.muted, text: task.meta.tried ? `tried in ⎇ ${task.meta.tried}` : task.meta.added ? `added ${task.meta.added}` : ''};
	const working = taskSessions(task, sessions);
	const lead = leadSession(working);
	if (!lead) return {glyph: '◌', color: THEME.muted, text: 'session not shown'};
	const where = lead.worktree?.branch ? `⎇ ${lead.worktree.branch}` : displaySessionTitle(lead, allSessions);
	return {glyph: statusGlyph(lead, spinnerFrame), color: statusColor(lead), text: urgency(lead) === 3 ? 'needs you' : statusWords(lead), where};
}

/** Open checklist items of the notes in view (each shared note once), not linked to tasks yet. */
export function noteItems(sessions: SessionRecord[]): NoteItem[] {
	const items: NoteItem[] = [];
	const seen = new Set<string>();
	const add = (session: SessionRecord, section: 'session' | 'shared', text: string | undefined, revision: string | undefined, source: string, noteId?: string) => {
		const key = noteKey(session, section);
		if (!key || !text || !revision || seen.has(key)) return;
		seen.add(key);
		text.split('\n').forEach((line, index) => {
			const item = parseChecklistLine(line);
			if (!item || item.checked || !item.text.trim() || parseNoteTaskLink(line)) return;
			items.push({key: `${key}:${index}`, title: item.text.trim(), sessionId: session.id, section, line: index, revision, ...noteId ? {noteId} : {}, source});
		});
	};
	for (const session of sessions) {
		const shared = session.sharedNotes;
		if (shared && !shared.readOnly) add(session, 'shared', shared.text, shared.revision, shared.kind === 'repo' ? 'main checkout' : `⎇ ${session.worktree?.branch || 'worktree'}`, `${shared.kind}:${shared.id}`);
		add(session, 'session', session.notes, session.notesFile?.revision, displaySessionTitle(session, sessions) || 'session');
	}
	return items;
}

/** `☐ 3 tasks` for the header (open tasks), empty without any. */
export function taskCountLabel(tasks: Task[]): string {
	const open = tasks.filter(task => !task.done).length;
	return open ? `☐ ${open} task${open === 1 ? '' : 's'} · b` : '';
}

/**
 * The open checklist items removing `session` would delete: its own note's, and its worktree note's when it is the
 * last session there (a worktree's note goes with its record).
 */
export function openItemsRemovedWith(session: SessionRecord, sessions: SessionRecord[]): string[] {
	const worktreeId = session.worktree?.id;
	const last = Boolean(worktreeId) && !sessions.some(other => other.id !== session.id && other.worktree?.id === worktreeId);
	return [...openNoteItems(session.notes), ...last && session.sharedNotes?.kind === 'worktree' ? openNoteItems(session.sharedNotes.text) : []];
}
