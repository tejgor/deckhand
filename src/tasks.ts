import {createHash, randomBytes} from 'node:crypto';
import {parseChecklistLine} from './notes.js';

// Tasks as pure data: one Markdown file per repository (the daemon keeps it beside the notes, src/notesStore.ts kind
// `tasks`), parsed here into tasks and changed only through TaskOps, which edit the task blocks in place and keep every
// other line (headings, prose) as written. A task is a top-level checklist item; the indented lines under it are its
// body. Deckhand's bookkeeping rides at the end of the item line in a hidden comment:
//   - [ ] Fix stuck footer <!-- dh:t=3fa9c1d2 wt=<worktree record id> added=2026-10-09 -->
// t: the task's ID (given on Deckhand's first write of the file); wt / s: the linked worktree incarnation or (main
// checkout) session; assigned: the task was put there (w, or added there) rather than started there (n), so it is a
// follow-up the work may not have done: a merge or D ticks only started tasks, and a merge sends open assigned ones
// back to the backlog; done: when it was ticked; auto: ticked by a merge or by D, so undoing that reopens it; tried:
// the branch of started work that was dropped unmerged; from / was: the branch (and worktree record) an assigned task
// went back to the backlog from, so undoing that merge assigns it again; added: when it was added.

export const MAX_TASK_TITLE = 300;
export const MAX_TASK_BODY = 4000;
/** Done tasks the board lists in full; older ones fold into one row. */
export const RECENT_DONE_DAYS = 7;

export interface TaskMeta {t?: string; wt?: string; s?: string; assigned?: string; done?: string; auto?: 'merge' | 'done'; tried?: string; from?: string; was?: string; added?: string}
export interface Task {
	/** `meta.t`, or (before Deckhand gave it one) a provisional `~<index>:<hash of the title>` ops resolve by position. */
	id: string;
	title: string;
	done: boolean;
	/** The lines under the item, their common indent removed. */
	body: string;
	meta: TaskMeta;
}
export type TaskLink = {wt: string} | {s: string};

export type TaskOp =
	/** `id`: given by the daemon when a note line links to the new task; `link`: added already assigned there. */
	| {type: 'add'; title: string; body?: string; link?: TaskLink; id?: string; from?: string}
	| {type: 'edit'; id: string; title: string; body: string}
	| {type: 'toggle'; id: string}
	| {type: 'remove'; id: string}
	/** Moves the task to just before (or `after`) the `target` task. */
	| {type: 'move'; id: string; target: string; after?: boolean}
	/** A session was started for the task (n): it is that work's own task. */
	| {type: 'link'; id: string; link: TaskLink}
	/** w: the task is assigned to that work (a follow-up), or back to the backlog without `link`. */
	| {type: 'assign'; id: string; link?: TaskLink}
	/** A merge (`merge`) or D (`done`) finished the linked work: its open started tasks are ticked, and the assigned ones in `also`. */
	| {type: 'tick-linked'; link: TaskLink; auto: 'merge' | 'done'; also?: string[]}
	/** A merge left these assigned tasks open: they go back to the backlog, remembering the branch and the worktree. */
	| {type: 'release-linked'; link: {wt: string}; from?: string}
	/** That merge or done was undone: the tasks it ticked reopen (and a merge's released tasks are assigned again). */
	| {type: 'reopen-linked'; link: TaskLink; auto: 'merge' | 'done'}
	/** The linked work is gone unmerged: its open tasks go back to the backlog, remembering the branch. */
	| {type: 'abandon-linked'; link: TaskLink; tried?: string};

type Block = {kind: 'raw'; lines: string[]} | {kind: 'task'; bullet: string; done: boolean; title: string; body: string[]; meta: TaskMeta};

const META = /[ \t]*<!--[ \t]*dh:([^>]*?)[ \t]*-->[ \t]*$/;
const META_KEYS = ['t', 'wt', 's', 'assigned', 'done', 'auto', 'tried', 'from', 'was', 'added'] as const;

function parseMeta(raw: string): TaskMeta {
	const meta: TaskMeta = {};
	for (const pair of raw.split(/[ \t]+/)) {
		const at = pair.indexOf('=');
		if (at < 1) continue;
		const key = pair.slice(0, at), value = pair.slice(at + 1);
		if (!value || !(META_KEYS as readonly string[]).includes(key)) continue;
		if (key === 'auto') { if (value === 'merge' || value === 'done') meta.auto = value; }
		else meta[key as Exclude<keyof TaskMeta, 'auto'>] = value;
	}
	return meta;
}

function formatMeta(meta: TaskMeta): string {
	const pairs = META_KEYS.filter(key => meta[key]).map(key => `${key}=${String(meta[key]).replace(/[\s>]/g, '')}`);
	return pairs.length ? ` <!-- dh:${pairs.join(' ')} -->` : '';
}

/** A title as one safe line: whitespace collapsed, no comment markers, cut to MAX_TASK_TITLE. */
export function cleanTaskTitle(title: string): string {
	return title.replace(/[\u0000-\u001F\u007F\u2028\u2029]+/g, ' ').replace(/<!--|-->/g, '').replace(/\s+/g, ' ').trim().slice(0, MAX_TASK_TITLE);
}

function cleanBody(body: string): string[] {
	const lines = body.replace(/\r\n?/g, '\n').replace(/[\u0000-\u0008\u000B-\u001F\u007F]/g, '').slice(0, MAX_TASK_BODY).split('\n').map(line => line.replace(/\s+$/, ''));
	while (lines.length && !lines.at(-1)) lines.pop();
	while (lines.length && !lines[0]) lines.shift();
	// A blank line would end the body when the file is read again: keep it as an indented empty line.
	return lines;
}

function dedent(lines: string[]): string {
	const indents = lines.filter(line => line.trim()).map(line => /^[ \t]*/.exec(line)![0].length);
	const cut = indents.length ? Math.min(...indents) : 0;
	return lines.map(line => line.slice(Math.min(cut, /^[ \t]*/.exec(line)![0].length))).join('\n');
}

function parseBlocks(text: string): Block[] {
	const lines = text.replace(/\r\n?/g, '\n').split('\n');
	if (lines.length && lines.at(-1) === '') lines.pop();
	const blocks: Block[] = [];
	const raw = (line: string) => {
		const last = blocks.at(-1);
		if (last?.kind === 'raw') last.lines.push(line);
		else blocks.push({kind: 'raw', lines: [line]});
	};
	for (let index = 0; index < lines.length; index++) {
		const line = lines[index]!;
		const item = parseChecklistLine(line);
		if (!item || item.indent) { raw(line); continue; }
		const match = META.exec(item.text);
		const title = (match ? item.text.slice(0, match.index) : item.text).trim();
		const body: string[] = [];
		// The body: the indented lines right under the item (an indented blank line keeps it going).
		while (index + 1 < lines.length && /^[ \t]/.test(lines[index + 1]!)) body.push(lines[++index]!);
		blocks.push({kind: 'task', bullet: item.bullet, done: item.checked, title, body, meta: match ? parseMeta(match[1]!) : {}});
	}
	return blocks;
}

function serializeBlocks(blocks: Block[]): string {
	const out: string[] = [];
	for (const block of blocks) {
		if (block.kind === 'raw') { out.push(...block.lines); continue; }
		out.push(`${block.bullet} [${block.done ? 'x' : ' '}] ${block.title}${formatMeta(block.meta)}`);
		out.push(...block.body.map(line => (line ? line : '  ')));
	}
	return out.length ? `${out.join('\n')}\n` : '';
}

const shortHash = (text: string) => createHash('sha256').update(text).digest('hex').slice(0, 8);
const provisionalId = (index: number, title: string) => `~${index}:${shortHash(title)}`;
export const newTaskId = () => randomBytes(4).toString('hex');

function taskOf(block: Extract<Block, {kind: 'task'}>, index: number): Task {
	return {id: block.meta.t ?? provisionalId(index, block.title), title: block.title, done: block.done, body: dedent(block.body), meta: {...block.meta}};
}

/** The file's tasks, in file order. */
export function parseTasks(text: string): Task[] {
	return parseBlocks(text).filter((block): block is Extract<Block, {kind: 'task'}> => block.kind === 'task').map(taskOf);
}

export const linkMatches = (meta: TaskMeta, link: TaskLink) => ('wt' in link ? meta.wt === link.wt : meta.s === link.s);
export const isLinked = (task: Pick<Task, 'meta'>) => Boolean(task.meta.wt || task.meta.s);
/** Assigned to its work (w) rather than started there (n). */
export const isAssigned = (task: Pick<Task, 'meta'>) => isLinked(task) && Boolean(task.meta.assigned);
/** `wt:<id>` / `s:<id>`: the work a task is linked to (the board's group), else undefined. */
export const linkKey = (meta: TaskMeta): string | undefined => (meta.wt ? `wt:${meta.wt}` : meta.s ? `s:${meta.s}` : undefined);
export const keyOfLink = (link: TaskLink): string => ('wt' in link ? `wt:${link.wt}` : `s:${link.s}`);
export function linkOfKey(key: string): TaskLink | undefined {
	if (key.startsWith('wt:') && key.length > 3) return {wt: key.slice(3)};
	if (key.startsWith('s:') && key.length > 2) return {s: key.slice(2)};
	return undefined;
}
function clearLink(meta: TaskMeta): void {
	delete meta.wt; delete meta.s; delete meta.assigned; delete meta.tried; delete meta.from; delete meta.was;
}
const today = (now: Date) => now.toISOString().slice(0, 10);

/**
 * Applies `op` to the file's text. Tasks without an ID get one first (so every task Deckhand has written can be
 * addressed); an unknown ID throws (the file changed meanwhile). `task`: the task the op added or changed, if one.
 */
export function applyTaskOp(text: string, op: TaskOp, now = new Date()): {text: string; task?: Task; changed: number} {
	const blocks = parseBlocks(text);
	const tasks = blocks.filter((block): block is Extract<Block, {kind: 'task'}> => block.kind === 'task');
	const ids = tasks.map((block, index) => block.meta.t ?? provisionalId(index, block.title));
	for (const block of tasks) block.meta.t ??= newTaskId();
	const find = (id: string) => {
		const at = ids.indexOf(id);
		if (at < 0) throw new Error('That task is no longer in the list (it changed outside Deckhand?); try again');
		return tasks[at]!;
	};
	const date = today(now);
	let target: Extract<Block, {kind: 'task'}> | undefined;
	let changed = 0;
	switch (op.type) {
		case 'add': {
			const title = cleanTaskTitle(op.title);
			if (!title) throw new Error('A task needs a title');
			if (op.id && ids.includes(op.id)) throw new Error('A task with that ID already exists');
			target = {kind: 'task', bullet: '-', done: false, title, body: cleanBody(op.body ?? '').map(line => `  ${line}`), meta: {t: op.id ?? newTaskId(), ...op.link && {...op.link, assigned: date}, ...!op.link && op.from ? {from: op.from} : {}, added: date}};
			// New tasks go after the last open task (the backlog's end), else at the end of the file.
			let lastOpen = -1;
			blocks.forEach((block, index) => { if (block.kind === 'task' && !block.done) lastOpen = index; });
			blocks.splice(lastOpen >= 0 ? lastOpen + 1 : blocks.length, 0, target);
			changed = 1;
			break;
		}
		case 'edit': {
			target = find(op.id);
			const title = cleanTaskTitle(op.title);
			if (!title) throw new Error('A task needs a title');
			target.title = title;
			target.body = cleanBody(op.body).map(line => `  ${line}`);
			changed = 1;
			break;
		}
		case 'toggle': {
			target = find(op.id);
			target.done = !target.done;
			if (target.done) target.meta.done = date;
			else delete target.meta.done;
			delete target.meta.auto;
			changed = 1;
			break;
		}
		case 'remove': {
			target = find(op.id);
			blocks.splice(blocks.indexOf(target), 1);
			changed = 1;
			break;
		}
		case 'move': {
			target = find(op.id);
			const anchor = find(op.target);
			if (anchor === target) break;
			blocks.splice(blocks.indexOf(target), 1);
			blocks.splice(blocks.indexOf(anchor) + (op.after ? 1 : 0), 0, target);
			changed = 1;
			break;
		}
		case 'link': {
			target = find(op.id);
			clearLink(target.meta);
			Object.assign(target.meta, op.link);
			changed = 1;
			break;
		}
		case 'assign': {
			target = find(op.id);
			if (target.done) throw new Error('That task is done; space reopens it');
			// Already there (started or assigned): nothing to change.
			if (op.link ? linkMatches(target.meta, op.link) : !(target.meta.wt || target.meta.s)) break;
			clearLink(target.meta);
			if (op.link) Object.assign(target.meta, op.link, {assigned: date});
			changed = 1;
			break;
		}
		case 'tick-linked':
			for (const block of tasks) {
				if (block.done || !linkMatches(block.meta, op.link) || (block.meta.assigned && !op.also?.includes(block.meta.t!))) continue;
				block.done = true; block.meta.done = date; block.meta.auto = op.auto; changed++;
			}
			break;
		case 'release-linked':
			for (const block of tasks) {
				if (block.done || !block.meta.assigned || !linkMatches(block.meta, op.link)) continue;
				clearLink(block.meta);
				if (op.from) block.meta.from = op.from;
				block.meta.was = op.link.wt;
				changed++;
			}
			break;
		case 'reopen-linked':
			for (const block of tasks) {
				if (block.done && block.meta.auto === op.auto && linkMatches(block.meta, op.link)) { block.done = false; delete block.meta.done; delete block.meta.auto; changed++; }
				else if (op.auto === 'merge' && 'wt' in op.link && !block.done && block.meta.was === op.link.wt && !block.meta.wt && !block.meta.s) {
					clearLink(block.meta);
					Object.assign(block.meta, {wt: op.link.wt, assigned: date});
					changed++;
				}
			}
			break;
		case 'abandon-linked':
			for (const block of tasks) {
				if (block.done || !linkMatches(block.meta, op.link)) continue;
				const assigned = Boolean(block.meta.assigned);
				clearLink(block.meta);
				// Started work was tried there; an assigned follow-up merely waited there.
				if (op.tried) block.meta[assigned ? 'from' : 'tried'] = op.tried;
				changed++;
			}
			break;
	}
	return {text: serializeBlocks(blocks), changed, ...target && op.type !== 'remove' ? {task: taskOf(target, 0)} : {}};
}

function clientLink(raw: unknown): TaskLink {
	const link = (raw && typeof raw === 'object' ? raw : {}) as Record<string, unknown>;
	if (typeof link.wt === 'string' && link.wt && link.s === undefined) return {wt: link.wt};
	if (typeof link.s === 'string' && link.s && link.wt === undefined) return {s: link.s};
	throw new Error('Invalid task change: link');
}

/**
 * The ops a client may send (`task-op`), checked; the daemon also checks an `add`/`assign` link names work in this
 * repository. Starting (`link`) and the linked-work ops are the daemon's own.
 */
export function clientTaskOp(raw: unknown): TaskOp {
	const op = (raw && typeof raw === 'object' ? raw : {}) as Record<string, unknown>;
	const text = (key: string) => { if (typeof op[key] !== 'string') throw new Error(`Invalid task change: ${key}`); return op[key] as string; };
	switch (op.type) {
		case 'add': return {type: 'add', title: text('title'), ...typeof op.body === 'string' ? {body: op.body} : {}, ...op.link !== undefined ? {link: clientLink(op.link)} : {}};
		case 'assign': return {type: 'assign', id: text('id'), ...op.link !== undefined ? {link: clientLink(op.link)} : {}};
		case 'edit': return {type: 'edit', id: text('id'), title: text('title'), body: text('body')};
		case 'toggle': case 'remove': return {type: op.type, id: text('id')};
		case 'move': return {type: 'move', id: text('id'), target: text('target'), ...op.after === true ? {after: true} : {}};
		default: throw new Error('Unknown task change');
	}
}

// ── The board ────────────────────────────────────────────────────────────────────────────────────────────────────

export interface TaskGroups {progress: Task[]; backlog: Task[]; done: Task[]; olderDone: Task[]}

/** Open linked tasks, open unlinked ones, then done ones: those ticked in the last RECENT_DONE_DAYS (or undated, newest file position first) and the older. */
export function groupTasks(tasks: Task[], now = new Date()): TaskGroups {
	const cutoff = today(new Date(now.getTime() - RECENT_DONE_DAYS * 86_400_000));
	const done = tasks.filter(task => task.done);
	return {
		progress: tasks.filter(task => !task.done && isLinked(task)),
		backlog: tasks.filter(task => !task.done && !isLinked(task)),
		done: done.filter(task => (task.meta.done ?? '') >= cutoff).sort((left, right) => (right.meta.done ?? '').localeCompare(left.meta.done ?? '')),
		olderDone: done.filter(task => (task.meta.done ?? '') < cutoff),
	};
}

export const openTaskCount = (tasks: Task[]) => tasks.filter(task => !task.done).length;

/**
 * What a session started from a task finds typed into its agent's input (not sent): the title, then the details on
 * the same line. One line, so every agent shows it in full in its input box, ready to edit.
 */
export function taskPrompt(task: Pick<Task, 'title' | 'body'>): string {
	const body = task.body.trim().replace(/\s*\n\s*/g, ' ');
	return body ? `${task.title}: ${body}` : task.title;
}

// ── Links from notes ─────────────────────────────────────────────────────────────────────────────────────────────

const NOTE_LINK = /^([ \t]*)[-*+][ \t]+↗[ \t]+(.*?)[ \t]*<!--[ \t]*dh:t=([0-9a-z]+)[ \t]*-->[ \t]*$/;

/** A note line sent to Tasks: `- ↗ <title> <!-- dh:t=<id> -->`. */
export function parseNoteTaskLink(line: string): {indent: string; title: string; id: string} | undefined {
	const match = NOTE_LINK.exec(line);
	return match ? {indent: match[1]!, title: match[2]!, id: match[3]!} : undefined;
}

export const noteTaskLinkLine = (indent: string, title: string, id: string) => `${indent}- ↗ ${title} <!-- dh:t=${id} -->`;

/**
 * The checklist item on `line` (0-based) of a note, to send to Tasks: its title (the item's text) and the note with
 * that line replaced by a link to the new task (`id`). Undefined when the line is not an open or done checklist item.
 */
export function promoteNoteLine(noteText: string, line: number, id: string): {title: string; done: boolean; text: string} | undefined {
	const lines = noteText.split('\n');
	const item = lines[line] === undefined ? undefined : parseChecklistLine(lines[line]!);
	const title = item && cleanTaskTitle(item.text);
	if (!item || !title) return undefined;
	lines[line] = noteTaskLinkLine(item.indent, title, id);
	return {title, done: item.checked, text: lines.join('\n')};
}

/**
 * Every open checklist item of a note, to send to Tasks at once: each becomes its `↗` link to a new task (`newId`).
 * Items without text are left alone. `items` are in note order.
 */
export function promoteOpenNoteLines(noteText: string, newId: () => string): {text: string; items: Array<{id: string; title: string}>} {
	const items: Array<{id: string; title: string}> = [];
	const lines = noteText.split('\n').map(line => {
		const item = parseChecklistLine(line);
		const title = item && !item.checked ? cleanTaskTitle(item.text) : '';
		if (!item || !title) return line;
		const id = newId();
		items.push({id, title});
		return noteTaskLinkLine(item.indent, title, id);
	});
	return {text: lines.join('\n'), items};
}

/** The line (0-based) of a note that links to task `id` (`- ↗ … <!-- dh:t=<id> -->`), else -1. */
export function findNoteTaskLink(noteText: string, id: string): number {
	return noteText.split('\n').findIndex(line => parseNoteTaskLink(line)?.id === id);
}

/** The note with its link (on `line`) turned back into an open checklist item: the task's title, its details indented under it. */
export function returnTaskToNote(noteText: string, line: number, task: Pick<Task, 'title' | 'body'>): string {
	const lines = noteText.split('\n');
	const link = lines[line] === undefined ? undefined : parseNoteTaskLink(lines[line]!);
	if (!link) throw new Error('That note no longer links to the task');
	const body = task.body ? task.body.split('\n').map(text => (text ? `${link.indent}  ${text}` : '')) : [];
	lines.splice(line, 1, `${link.indent}- [ ] ${task.title}`, ...body);
	return lines.join('\n');
}

/** The unchecked checklist items of a note, as task titles. */
export function openNoteItems(noteText: string | undefined): string[] {
	return (noteText ?? '').split('\n').map(line => parseChecklistLine(line)).filter(item => item && !item.checked).map(item => cleanTaskTitle(item!.text)).filter(Boolean);
}

/** Hidden `<!-- dh:… -->` comments removed, for showing a line. */
export const withoutTaskMeta = (line: string) => line.replace(/[ \t]*<!--[ \t]*dh:[^>]*?-->/g, '');
