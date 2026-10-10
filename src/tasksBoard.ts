import type {SessionRecord} from './types.js';
import {findNoteTaskLink, groupTasks, isLinked, linkKey, linkOfKey, openNoteItems, parseNoteTaskLink, type Task} from './tasks.js';
import {noteKey, parseChecklistLine, taskLinkSuffix, type TaskLinkLookup} from './notes.js';
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
	/** `section`: the work's key for tasks in progress, else `backlog` or `done`. `steps`: its work note's checklist, done/total. */
	| {kind: 'task'; task: Task; group: TaskGroup; section: string; steps?: {done: number; total: number}}
	/** A checklist item of the work's note, shown under the task the work was started for (one of its steps). */
	| {kind: 'step'; section: string; taskId: string; step: WorkStep}
	| {kind: 'older'; count: number; shown: boolean}
	| {kind: 'empty'; text: string}
	/** The Notes view (tab): a note's heading (its open items counted), then each line of it. */
	| {kind: 'notehead'; block: NoteBlock; open: number}
	| {kind: 'noteline'; block: NoteBlock; line: number; text: string; style: NoteLineStyle; item?: NoteItem};

/** How a note's line shows: an open item (actionable), a ticked one, a ↗ link to a task, a heading, or text. */
export type NoteLineStyle = 'open' | 'done' | 'link' | 'heading' | 'text';

export const selectableRow = (row: BoardRow) => row.kind === 'task' || row.kind === 'step' || row.kind === 'older' || row.kind === 'notehead' || row.kind === 'noteline';
export const rowKey = (row: BoardRow): string | undefined => row.kind === 'task' ? `task:${row.task.id}` : row.kind === 'step' ? `step:${row.section}:${row.step.line}` : row.kind === 'older' ? 'older'
	: row.kind === 'notehead' ? `notehead:${row.block.key}` : row.kind === 'noteline' ? `noteline:${row.block.key}:${row.line}` : undefined;

/** One checklist item of a work's note: the note's line, its text, ticked or not. */
export interface WorkStep {line: number; text: string; done: boolean}

/** The note a work's steps are in: a worktree's shared note, or a main-checkout session's own; how the board saves it. */
export interface WorkNote {sessionId: string; section: 'session' | 'shared'; noteId?: string; text: string; revision: string; readOnly?: boolean}

/** The note of work `section` (`wt:<id>`: the worktree's note, from any session of it; `s:<id>`: that session's own note). */
export function workNote(section: string, sessions: SessionRecord[]): WorkNote | undefined {
	const working = workSessions(section, sessions);
	if (section.startsWith('wt:')) {
		const id = section.slice(3);
		const holder = [...working.filter(session => !session.archivedAt), ...working.filter(session => session.archivedAt)]
			.find(session => session.sharedNotes?.kind === 'worktree' && session.sharedNotes.id === id);
		const shared = holder?.sharedNotes;
		return holder && shared ? {sessionId: holder.id, section: 'shared', noteId: `${shared.kind}:${shared.id}`, text: shared.text, revision: shared.revision, ...shared.readOnly ? {readOnly: true} : {}} : undefined;
	}
	const session = working[0];
	return session?.notesFile ? {sessionId: session.id, section: 'session', text: session.notes ?? '', revision: session.notesFile.revision} : undefined;
}

/** Steps done of total. */
export const stepCount = (steps: WorkStep[]) => ({done: steps.filter(step => step.done).length, total: steps.length});

/** A note's checklist items (open and ticked; not `↗` links, nor items without text), as steps. */
export function noteSteps(text: string | undefined): WorkStep[] {
	return (text ?? '').split('\n').flatMap((line, index) => {
		const item = parseChecklistLine(line);
		return item && item.text.trim() ? [{line: index, text: item.text.trim(), done: item.checked}] : [];
	});
}

/**
 * The board's rows: a group per worktree (or main-checkout session) with open tasks, in the order its first task
 * appears in the file, then the backlog and done this week (older ones folded unless `showOlder`). With `scope` (v),
 * only that work's group (shown even when empty) and its done tasks. `stepsOf`: a work's steps (its note's checklist),
 * listed under the first task it was started for, which counts them.
 */
export function boardRows(tasks: Task[], showOlder: boolean, now = new Date(), scope?: string, stepsOf?: (section: string) => WorkStep[]): BoardRow[] {
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
		const steps = stepsOf?.(work) ?? [];
		const lead = steps.length ? list[0] : undefined;
		for (const task of list) {
			if (task !== lead) { rows.push({kind: 'task', task, group: 'progress', section: work}); continue; }
			rows.push({kind: 'task', task, group: 'progress', section: work, steps: stepCount(steps)});
			for (const step of steps) rows.push({kind: 'step', section: work, taskId: task.id, step});
		}
		if (!list.length) rows.push({kind: 'empty', text: 'No open tasks here · a adds one'});
	}
	if (!works.length) section('IN PROGRESS', [], 'progress', 'n on a backlog task starts a session for it · w gives it to a worktree');
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

/** Whether work `section` has an open task (◆) other than `except`: w then puts a task into its note as a step instead. */
export function hasMainTask(section: string, tasks: Task[], except?: string): boolean {
	return tasks.some(task => !task.done && task.id !== except && linkKey(task.meta) === section);
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
 * way back: not a done task, nor one some work is for, nor a deleted worktree's (read-only) note.
 */
export function taskOrigin(task: Task, sessions: SessionRecord[]): string | undefined {
	if (task.done || isLinked(task) || !task.meta.t) return undefined;
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

/** The task the session's work is about: the open one, else a done one. */
export function linkedTask(tasks: Task[], session: SessionRecord | undefined): Task | undefined {
	const linked = linkedTasks(tasks, session);
	return linked.find(task => !task.done) ?? linked[0];
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

/** One note the Notes view (tab) lists: a worktree's, the main checkout's, or a session's own. */
export interface NoteBlock {
	/** The note's draft key (`worktree:<id>`, `repo:<id>`, `session:<id>`). */
	key: string;
	/** Its group: the worktree (`wt:<record id>`) or `main` (the main checkout). */
	group: string;
	/** `worktree note`, `main checkout note`, or the session's title. */
	label: string;
	/** A session showing it, which saves, sends items and opens it (the first not archived, in sidebar order). */
	sessionId: string;
	section: 'session' | 'shared';
	/** A shared note's `kind:id`. */
	noteId?: string;
	text: string;
	revision: string;
}

/**
 * Every note with text, grouped by worktree in sidebar order (its note first, then any session note left in it), then
 * the main checkout (its note, then each session's own). Deleted worktrees' (read-only) notes are left out.
 */
export function noteBlocks(sessions: SessionRecord[]): NoteBlock[] {
	const ordered = sortSessionsForSidebar(sessions);
	const preferred = [...ordered.filter(session => !session.archivedAt), ...ordered.filter(session => session.archivedAt)];
	const blocks: NoteBlock[] = [];
	const seen = new Set<string>();
	const groupOf = (session: SessionRecord) => (session.worktree?.id ? `wt:${session.worktree.id}` : 'main');
	for (const session of preferred) {
		const shared = session.sharedNotes, key = noteKey(session, 'shared');
		if (!shared || shared.readOnly || !key || seen.has(key) || !shared.text.trim()) continue;
		seen.add(key);
		blocks.push({key, group: groupOf(session), label: shared.kind === 'repo' ? 'main checkout note' : 'worktree note', sessionId: session.id, section: 'shared', noteId: `${shared.kind}:${shared.id}`, text: shared.text, revision: shared.revision});
	}
	for (const session of ordered) {
		const key = noteKey(session, 'session');
		if (!key || !session.notes?.trim() || !session.notesFile) continue;
		blocks.push({key, group: groupOf(session), label: displaySessionTitle(session, sessions) || 'session', sessionId: session.id, section: 'session', text: session.notes, revision: session.notesFile.revision});
	}
	// Worktrees in sidebar order, the main checkout last; within a group, shared notes first.
	const groups = [...new Set([...ordered.filter(session => session.worktree?.id).map(session => `wt:${session.worktree!.id}`), 'main'])];
	return groups.flatMap(group => blocks.filter(block => block.group === group));
}

/** The open checklist items of a note (not ↗ links to tasks), as items the board can send to Tasks. */
export function blockItems(block: NoteBlock, sessions: SessionRecord[]): NoteItem[] {
	const items: NoteItem[] = [];
	block.text.split('\n').forEach((line, index) => {
		const item = parseChecklistLine(line);
		if (!item || item.checked || !item.text.trim() || parseNoteTaskLink(line)) return;
		items.push({key: `${block.key}:${index}`, title: item.text.trim(), sessionId: block.sessionId, section: block.section, line: index, revision: block.revision, ...block.noteId ? {noteId: block.noteId} : {}, source: block.label, group: block.group});
	});
	return items;
}

/**
 * Open checklist items of every session's notes (each shared note once), not linked to tasks yet, grouped by worktree
 * (its shared note first, then its sessions' notes) and then the main checkout, groups in sidebar order.
 */
export function noteItems(sessions: SessionRecord[]): NoteItem[] {
	return noteBlocks(sessions).flatMap(block => blockItems(block, sessions));
}

/** Whether each task a note links to is open, done or gone; undefined while the list is not loaded. */
export function taskLinkLookup(tasks: Task[] | undefined): TaskLinkLookup | undefined {
	if (!tasks) return undefined;
	const done = new Map(tasks.map(task => [task.id, task.done]));
	return id => (!done.has(id) ? 'gone' : done.get(id) ? 'done' : 'open');
}

/** A note's line as the Notes view shows it (blank lines are left out: undefined). */
export function noteLine(line: string, links?: TaskLinkLookup): {text: string; style: NoteLineStyle} | undefined {
	if (!line.trim()) return undefined;
	const link = parseNoteTaskLink(line);
	if (link) return {text: `${link.indent}↗ ${link.title} ${taskLinkSuffix(links ? links(link.id) : undefined)}`, style: 'link'};
	const item = parseChecklistLine(line);
	if (item) return {text: `${item.indent}${item.checked ? '☑' : '☐'} ${item.text.trim()}`, style: item.checked ? 'done' : 'open'};
	const heading = /^#{1,6}[ \t]+(.*)$/.exec(line);
	if (heading) return {text: heading[1]!.trim(), style: 'heading'};
	return {text: line.replace(/\s+$/, ''), style: 'text'};
}

/**
 * The Notes view's rows: per group (worktree, then main checkout) its heading, then each note's heading and lines.
 * `itemsOnly` (f): only open checklist items. `scope` (v): one worktree (`wt:<id>`), or a main-checkout session
 * (`s:<id>`: its own note and the main checkout's).
 */
export function notesViewRows(sessions: SessionRecord[], {itemsOnly = false, scope, links}: {itemsOnly?: boolean; scope?: string; links?: TaskLinkLookup} = {}): BoardRow[] {
	const inScope = (block: NoteBlock) => !scope || (scope.startsWith('wt:') ? block.group === scope : block.group === 'main' && (block.section === 'shared' || block.sessionId === scope.slice(2)));
	const blocks = noteBlocks(sessions).filter(inScope).map(block => {
		const items = blockItems(block, sessions);
		const lines = block.text.split('\n').flatMap((raw, line): BoardRow[] => {
			const shown = noteLine(raw, links);
			if (!shown || (itemsOnly && shown.style !== 'open')) return [];
			const item = shown.style === 'open' ? items.find(candidate => candidate.line === line) : undefined;
			return [{kind: 'noteline', block, line, text: shown.text, style: shown.style, ...item ? {item} : {}}];
		});
		return {block, open: items.length, lines};
	}).filter(entry => entry.lines.length);
	if (!blocks.length) return [{kind: 'empty', text: itemsOnly ? 'No open checklist items in these notes · f shows every note' : scope ? 'No notes here yet · write one in a session’s Notes tab (a)' : 'No notes yet · write one in a session’s Notes tab (a)'}];
	return [...new Set(blocks.map(entry => entry.block.group))].flatMap(group => {
		const inGroup = blocks.filter(entry => entry.block.group === group);
		return [
			{kind: 'work', section: group, count: inGroup.reduce((sum, entry) => sum + entry.open, 0), label: noteGroupLabel(group, sessions)} as BoardRow,
			...inGroup.flatMap(({block, open, lines}): BoardRow[] => [{kind: 'notehead', block, open}, ...lines]),
		];
	});
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

/** The open checklist items of a worktree's note (from any session of it): what merging or deleting it can send to the backlog. */
export function worktreeOpenItems(recordId: string | undefined, sessions: SessionRecord[]): string[] {
	if (!recordId) return [];
	const note = workNote(`wt:${recordId}`, sessions);
	return note && !note.readOnly ? openNoteItems(note.text) : [];
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
