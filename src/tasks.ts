import {createHash, randomBytes} from 'node:crypto';
import {parseChecklistLine} from './notes.js';

// Tasks as pure data: one Markdown file per repository (the daemon keeps it beside the notes, src/notesStore.ts kind
// `tasks`), parsed here into tasks and changed only through TaskOps, which edit the task blocks in place and keep every
// other line (headings, prose) as written. A task is a top-level checklist item; the indented lines under it are its
// body. Deckhand's bookkeeping rides at the end of the item line in a hidden comment:
//   - [ ] Fix stuck footer <!-- dh:t=3fa9c1d2 wt=<worktree record id> added=2026-10-09 -->
// t: the task's ID (given on Deckhand's first write of the file); wt / s: the linked worktree incarnation or (main
// checkout) session doing it; done: when it was ticked; auto: ticked by a merge or by D, so undoing that reopens it;
// tried: the branch of linked work that was dropped unmerged; added: when it was added.

export const MAX_TASK_TITLE = 300;
export const MAX_TASK_BODY = 4000;
/** Done tasks the board lists in full; older ones fold into one row. */
export const RECENT_DONE_DAYS = 7;

export interface TaskMeta {t?: string; wt?: string; s?: string; done?: string; auto?: 'merge' | 'done'; tried?: string; added?: string}
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
	/** `id`: given by the daemon when a note line links to the new task. */
	| {type: 'add'; title: string; body?: string; link?: TaskLink; id?: string}
	| {type: 'edit'; id: string; title: string; body: string}
	| {type: 'toggle'; id: string}
	| {type: 'remove'; id: string}
	/** Moves the task to just before (or `after`) the `target` task. */
	| {type: 'move'; id: string; target: string; after?: boolean}
	| {type: 'link'; id: string; link: TaskLink}
	/** A merge (`merge`) or D (`done`) finished the linked work: its open tasks are ticked. */
	| {type: 'tick-linked'; link: TaskLink; auto: 'merge' | 'done'}
	/** That merge or done was undone: the tasks it ticked reopen. */
	| {type: 'reopen-linked'; link: TaskLink; auto: 'merge' | 'done'}
	/** The linked work is gone unmerged: its open tasks go back to the backlog, remembering the branch. */
	| {type: 'abandon-linked'; link: TaskLink; tried?: string};

type Block = {kind: 'raw'; lines: string[]} | {kind: 'task'; bullet: string; done: boolean; title: string; body: string[]; meta: TaskMeta};

const META = /[ \t]*<!--[ \t]*dh:([^>]*?)[ \t]*-->[ \t]*$/;
const META_KEYS = ['t', 'wt', 's', 'done', 'auto', 'tried', 'added'] as const;

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
			target = {kind: 'task', bullet: '-', done: false, title, body: cleanBody(op.body ?? '').map(line => `  ${line}`), meta: {t: op.id ?? newTaskId(), ...op.link, added: date}};
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
			delete target.meta.wt; delete target.meta.s; delete target.meta.tried;
			Object.assign(target.meta, op.link);
			changed = 1;
			break;
		}
		case 'tick-linked':
			for (const block of tasks) if (!block.done && linkMatches(block.meta, op.link)) { block.done = true; block.meta.done = date; block.meta.auto = op.auto; changed++; }
			break;
		case 'reopen-linked':
			for (const block of tasks) if (block.done && block.meta.auto === op.auto && linkMatches(block.meta, op.link)) { block.done = false; delete block.meta.done; delete block.meta.auto; changed++; }
			break;
		case 'abandon-linked':
			for (const block of tasks) {
				if (block.done || !linkMatches(block.meta, op.link)) continue;
				delete block.meta.wt; delete block.meta.s;
				if (op.tried) block.meta.tried = op.tried;
				changed++;
			}
			break;
	}
	return {text: serializeBlocks(blocks), changed, ...target && op.type !== 'remove' ? {task: taskOf(target, 0)} : {}};
}

/** The ops a client may send (`task-op`), checked; links and the linked-work ops are the daemon's own. */
export function clientTaskOp(raw: unknown): TaskOp {
	const op = (raw && typeof raw === 'object' ? raw : {}) as Record<string, unknown>;
	const text = (key: string) => { if (typeof op[key] !== 'string') throw new Error(`Invalid task change: ${key}`); return op[key] as string; };
	switch (op.type) {
		case 'add': return {type: 'add', title: text('title'), ...typeof op.body === 'string' ? {body: op.body} : {}};
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

/** The first message a session started from a task gets: its title and body. */
export function taskPrompt(task: Pick<Task, 'title' | 'body'>): string {
	return [`Task: ${task.title}`, ...task.body.trim() ? ['', task.body.trim()] : []].join('\n');
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

/** The unchecked checklist items of a note, as task titles. */
export function openNoteItems(noteText: string | undefined): string[] {
	return (noteText ?? '').split('\n').map(line => parseChecklistLine(line)).filter(item => item && !item.checked).map(item => cleanTaskTitle(item!.text)).filter(Boolean);
}

/** Hidden `<!-- dh:… -->` comments removed, for showing a line. */
export const withoutTaskMeta = (line: string) => line.replace(/[ \t]*<!--[ \t]*dh:[^>]*?-->/g, '');
