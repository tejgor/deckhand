import type {SessionRecord} from './types.js';
import {findNoteTaskLink, groupTasks, isAssigned, isLinked, linkKey, linkOfKey, openNoteItems, parseNoteTaskLink, type Task} from './tasks.js';
import {noteKey, parseChecklistLine} from './notes.js';
import {statusWords} from './sidebarModel.js';
import {THEME, displaySessionTitle, statusColor, statusGlyph} from './ui.js';
import {workspaceKey} from './workspace.js';
import {sortSessionsForSidebar} from './sessionOrder.js';

// The Tasks board (b) as pure data: its rows (one group per worktree (or main-checkout session) with open tasks, the
// backlog, done, the folded older done ones, and the open note items not yet tasks), each task's linked sessions and
// the state shown for it. A group is keyed by its work (`wt:<record id>` / `s:<session id>`, src/tasks.ts linkKey).
// src/tasksFlow.tsx keeps the selection and keys.

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
	/** The note it is in: `worktree note`, `main checkout note`, or the session's title. */
	source: string;
	/** Its group on the board: the worktree (`wt:<record id>`) or `main` (the main checkout). */
	group: string;
}

export type BoardRow =
	| {kind: 'heading'; text: string; count?: string}
	/** The heading of a worktree's (or main-checkout session's) group: `section` is its work's key (`main`: the main checkout's notes, in the note list); `label` replaces the work's name. */
	| {kind: 'work'; section: string; count: number; label?: string}
	/** `section`: the work's key for tasks in progress, else `backlog` or `done`. */
	| {kind: 'task'; task: Task; group: TaskGroup; section: string}
	| {kind: 'older'; count: number; shown: boolean}
	| {kind: 'empty'; text: string}
	| {kind: 'note'; item: NoteItem};

export const selectableRow = (row: BoardRow) => row.kind === 'task' || row.kind === 'older' || row.kind === 'note';
export const rowKey = (row: BoardRow): string | undefined => row.kind === 'task' ? `task:${row.task.id}` : row.kind === 'older' ? 'older' : row.kind === 'note' ? `note:${row.item.key}` : undefined;

/**
 * The board's rows: a group per worktree (or main-checkout session) with open tasks, in the order its first task
 * appears in the file, then the backlog and done this week (older ones folded unless `showOlder`). With `scope` (v),
 * only that work's group (shown even when empty) and its done tasks.
 */
export function boardRows(tasks: Task[], showOlder: boolean, now = new Date(), scope?: string): BoardRow[] {
	const groups = groupTasks(scope ? tasks.filter(task => linkKey(task.meta) === scope) : tasks, now);
	const rows: BoardRow[] = [];
	const section = (text: string, list: Task[], group: TaskGroup, empty?: string) => {
		rows.push({kind: 'heading', text, count: list.length ? String(list.length) : undefined});
		for (const task of list) rows.push({kind: 'task', task, group, section: group});
		if (!list.length && empty) rows.push({kind: 'empty', text: empty});
	};
	const works = [...new Set(groups.progress.map(task => linkKey(task.meta)!))];
	if (scope && !works.includes(scope)) works.push(scope);
	for (const work of works) {
		const list = groups.progress.filter(task => linkKey(task.meta) === work);
		rows.push({kind: 'work', section: work, count: list.length});
		for (const task of list) rows.push({kind: 'task', task, group: 'progress', section: work});
		if (!list.length) rows.push({kind: 'empty', text: 'No open tasks here · a adds one'});
	}
	if (!works.length) section('IN PROGRESS', [], 'progress', 'n on a backlog task starts a session for it · w assigns it to a worktree');
	if (!scope) section('BACKLOG', groups.backlog, 'backlog', tasks.length ? 'Nothing waiting' : 'No tasks yet · a adds one');
	if (groups.done.length || groups.olderDone.length) {
		section('DONE · this week', groups.done, 'done');
		if (groups.olderDone.length) {
			rows.push({kind: 'older', count: groups.olderDone.length, shown: showOlder});
			if (showOlder) for (const task of groups.olderDone) rows.push({kind: 'task', task, group: 'done', section: 'done'});
		}
	}
	return rows;
}

/** The sessions doing a task: every session of its linked worktree incarnation, or its linked (main checkout) session. */
export function taskSessions(task: Pick<Task, 'meta'>, sessions: SessionRecord[]): SessionRecord[] {
	const section = linkKey(task.meta);
	return section ? workSessions(section, sessions) : [];
}

/** The sessions of a work key (`wt:<id>`: every session of that worktree incarnation; `s:<id>`: that session). */
export function workSessions(section: string, sessions: SessionRecord[]): SessionRecord[] {
	const link = linkOfKey(section);
	if (!link) return [];
	return 'wt' in link ? sessions.filter(session => session.worktree?.id === link.wt) : sessions.filter(session => session.id === link.s);
}

/** The work key a session's tasks are linked to: its worktree incarnation's, else (main checkout) its own. */
export function workKeyOf(session: SessionRecord): string {
	return session.worktree?.id ? `wt:${session.worktree.id}` : `s:${session.id}`;
}

/** How a work key reads: `⎇ <branch>`, or `main checkout · <session>`. */
export function workLabel(section: string, sessions: SessionRecord[]): string {
	const working = workSessions(section, sessions);
	if (section.startsWith('wt:')) {
		const branch = working.find(session => session.worktree?.branch)?.worktree?.branch;
		return branch ? `⎇ ${branch}` : working.length ? '⎇ worktree' : '⎇ a worktree not listed';
	}
	return working[0] ? `main checkout · ${displaySessionTitle(working[0], sessions)}` : 'a session not listed';
}

export interface WorkOption {
	/** The work's key, or undefined for the backlog and the note. */
	section?: string;
	kind: 'backlog' | 'note' | 'worktree' | 'session';
	label: string;
	/** `merged`, `here` (where the task is now), or both. */
	note: string;
	/** What search matches (lowercase): the label, branch and the titles of the work's sessions. */
	search: string;
}

/** The w menu's two lists: worktrees, and sessions in the main checkout. */
export type PickerView = 'worktrees' | 'sessions';
export const pickerViewOf = (section?: string): PickerView => (section?.startsWith('s:') ? 'sessions' : 'worktrees');

/**
 * Where w can move a task: the backlog, then every worktree incarnation and main-checkout session in sidebar order
 * (not deleted worktrees, nor ones whose sessions are all archived).
 */
export function workOptions(sessions: SessionRecord[], current?: string): WorkOption[] {
	const options: WorkOption[] = [{kind: 'backlog', label: 'Backlog · no worktree', note: current ? '' : 'here', search: ''}];
	const seen = new Set<string>();
	for (const session of sortSessionsForSidebar(sessions)) {
		if (!workspaceKey(session)) continue;
		const section = workKeyOf(session);
		if (seen.has(section)) continue;
		const working = workSessions(section, sessions);
		if (working.every(other => other.archivedAt)) continue;
		seen.add(section);
		const merged = Boolean(session.worktree?.id && working.some(other => other.worktree?.mergedAt));
		const label = workLabel(section, sessions);
		options.push({
			section, kind: section.startsWith('wt:') ? 'worktree' : 'session', label,
			note: [merged ? '✓ merged' : '', section === current ? 'here' : ''].filter(Boolean).join(' · '),
			search: [label, ...working.map(other => displaySessionTitle(other, sessions))].join(' ').toLowerCase(),
		});
	}
	return options;
}

/**
 * The w menu's rows in `view`: the backlog and (when the task came from a note) the way back to it always first,
 * then that view's options matching every word of `query`.
 */
export function pickerRows(options: WorkOption[], view: PickerView, query: string, origin?: string): WorkOption[] {
	const terms = query.toLowerCase().trim().split(/\s+/).filter(Boolean);
	const kind = view === 'worktrees' ? 'worktree' : 'session';
	return [
		...options.filter(option => option.kind === 'backlog'),
		...origin ? [{kind: 'note' as const, label: `↩ Back to its note · ${origin}`, note: '', search: ''}] : [],
		...options.filter(option => option.kind === kind && terms.every(term => option.search.includes(term))),
	];
}

/** How many options each view lists for `query` (the menu's tabs). */
export function pickerCounts(options: WorkOption[], query: string): Record<PickerView, number> {
	const count = (view: PickerView) => pickerRows(options, view, query).filter(option => option.kind !== 'backlog').length;
	return {worktrees: count('worktrees'), sessions: count('sessions')};
}

/**
 * Where a task sent from a note came from (the note still holding its `↗` line, among the sessions in view), for w's
 * way back: not a done task, nor one a session was started for, nor a deleted worktree's (read-only) note.
 */
export function taskOrigin(task: Task, sessions: SessionRecord[]): string | undefined {
	if (task.done || (isLinked(task) && !isAssigned(task)) || !task.meta.t) return undefined;
	for (const session of sessions) {
		if (findNoteTaskLink(session.notes ?? '', task.id) >= 0) return displaySessionTitle(session, sessions) || 'session';
		const shared = session.sharedNotes;
		if (shared && !shared.readOnly && findNoteTaskLink(shared.text, task.id) >= 0) return shared.kind === 'repo' ? 'main checkout' : `⎇ ${session.worktree?.branch || 'worktree'}`;
	}
	return undefined;
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

/** Every task linked to the session's work (its worktree incarnation, else the session), in file order. */
export function linkedTasks(tasks: Task[], session: SessionRecord | undefined): Task[] {
	if (!session) return [];
	return tasks.filter(task => isLinked(task) && (task.meta.wt ? task.meta.wt === session.worktree?.id : task.meta.s === session.id));
}

/** The task the session's work is about: the open one it was started for, else an open assigned one, else a done one. */
export function linkedTask(tasks: Task[], session: SessionRecord | undefined): Task | undefined {
	const linked = linkedTasks(tasks, session);
	return linked.find(task => !task.done && !isAssigned(task)) ?? linked.find(task => !task.done) ?? linked.find(task => !isAssigned(task)) ?? linked[0];
}

/** How many other open tasks the session's work has besides `linkedTask`. */
export function otherOpenTasks(tasks: Task[], session: SessionRecord | undefined): number {
	const main = linkedTask(tasks, session);
	return linkedTasks(tasks, session).filter(task => !task.done && task !== main).length;
}

export interface TaskState {glyph: string; color: string; text: string; where?: string}

/** What a task row shows on its right: its work's state and branch, where it came from, or when it was done. */
export function taskState(task: Task, sessions: SessionRecord[], spinnerFrame: string, allSessions: SessionRecord[] = sessions): TaskState {
	if (task.done) return {glyph: '✓', color: THEME.success, text: task.meta.auto === 'merge' ? 'merged' : task.meta.auto === 'done' ? 'done (D)' : 'done', ...task.meta.done ? {where: task.meta.done} : {}};
	if (!isLinked(task)) return {glyph: '○', color: THEME.muted, text: task.meta.tried ? `tried in ⎇ ${task.meta.tried}` : task.meta.from ? `left open in ⎇ ${task.meta.from}` : task.meta.added ? `added ${task.meta.added}` : ''};
	return workState(linkKey(task.meta)!, sessions, spinnerFrame, allSessions);
}

/** The state of a worktree's (or main-checkout session's) work: its most urgent session's. */
export function workState(section: string, sessions: SessionRecord[], spinnerFrame: string, allSessions: SessionRecord[] = sessions): TaskState {
	const lead = leadSession(workSessions(section, sessions));
	if (!lead) return {glyph: '◌', color: THEME.muted, text: 'session not shown'};
	const where = lead.worktree?.branch ? `⎇ ${lead.worktree.branch}` : displaySessionTitle(lead, allSessions);
	return {glyph: statusGlyph(lead, spinnerFrame), color: statusColor(lead), text: urgency(lead) === 3 ? 'needs you' : statusWords(lead), where};
}

/**
 * Open checklist items of every session's notes (each shared note once), not linked to tasks yet, grouped by worktree
 * (its shared note first, then its sessions' notes) and then the main checkout, groups in sidebar order.
 */
export function noteItems(sessions: SessionRecord[]): NoteItem[] {
	const items: NoteItem[] = [];
	const seen = new Set<string>();
	const add = (session: SessionRecord, section: 'session' | 'shared', text: string | undefined, revision: string | undefined, source: string, noteId?: string) => {
		const key = noteKey(session, section);
		if (!key || !text || !revision || seen.has(key)) return;
		seen.add(key);
		const group = session.worktree?.id ? `wt:${session.worktree.id}` : 'main';
		text.split('\n').forEach((line, index) => {
			const item = parseChecklistLine(line);
			if (!item || item.checked || !item.text.trim() || parseNoteTaskLink(line)) return;
			items.push({key: `${key}:${index}`, title: item.text.trim(), sessionId: session.id, section, line: index, revision, ...noteId ? {noteId} : {}, source, group});
		});
	};
	const ordered = sortSessionsForSidebar(sessions);
	for (const session of ordered) {
		const shared = session.sharedNotes;
		if (shared && !shared.readOnly) add(session, 'shared', shared.text, shared.revision, shared.kind === 'repo' ? 'main checkout note' : 'worktree note', `${shared.kind}:${shared.id}`);
	}
	for (const session of ordered) add(session, 'session', session.notes, session.notesFile?.revision, displaySessionTitle(session, sessions) || 'session');
	// Worktrees in sidebar order, the main checkout last; within a group, shared notes first.
	const groups = [...new Set([...ordered.filter(session => session.worktree?.id).map(session => `wt:${session.worktree!.id}`), 'main'])];
	return groups.flatMap(group => items.filter(item => item.group === group));
}

/** A note group's heading: `⎇ <branch>`, or `main checkout`. */
export function noteGroupLabel(group: string, sessions: SessionRecord[]): string {
	return group === 'main' ? 'main checkout' : workLabel(group, sessions);
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
