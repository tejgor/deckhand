import React, {useRef, useState} from 'react';
import path from 'node:path';
import {Box, Text, type Key} from 'ink';
import type {LiveClient} from './client.js';
import type {SessionRecord, TasksDoc} from './types.js';
import {MAX_TASK_BODY, MAX_TASK_TITLE, type Task} from './tasks.js';
import {boardRows, leadSession, noteItems, rowKey, selectableRow, taskSessions, taskState, type BoardRow, type NoteItem} from './tasksBoard.js';
import {editText, wrapRows, wrappedEditorLines, type EditOptions, type EditorState} from './textEditor.js';
import {openInEditor} from './desktop.js';
import {fitHint} from './menu.js';
import {THEME, errorMessage, truncate} from './ui.js';

// The Tasks board (b): the repository's task list in the right pane, the open note items not yet tasks (tab), and a
// small editor for adding and editing a task (title, then body). The daemon owns the file; every change is an op
// (src/tasks.ts) applied to the file as it is, so an editor's edits are never overwritten.

const DETAIL_ROWS = 5;
const EDITOR_BODY_ROWS = 4;

interface TaskEditor {
	kind: 'add' | 'edit';
	id?: string;
	field: 'title' | 'body';
	title: EditorState;
	body: EditorState;
}

interface TasksFlowOptions {
	client?: LiveClient;
	repoRoot: string;
	doc?: TasksDoc;
	tasks: Task[];
	sessions: SessionRecord[];
	spinnerFrame: string;
	onExit: () => void;
	/** n on a backlog task: the new-session form, filled in from it. */
	onStart: (task: Task) => void;
	/** g on a task in progress: select its session. */
	onGoTo: (sessionId: string) => void;
	/** enter on a note item: that session's Notes tab. */
	onOpenNote: (sessionId: string) => void;
	onDoc: (doc: TasksDoc) => void;
	onSession: (session: SessionRecord) => void;
	setError: (error: string | undefined) => void;
	setStatusMessage: (message: string | undefined) => void;
}
export interface TasksFlow {
	handleInput(input: string, key: Partial<Key>): void;
	render(width: number, height: number): React.ReactNode;
	hint(width: number): string;
	/** b: the board, from the top of its task view. */
	open(): void;
}

const TITLE_OPTIONS: EditOptions = {selectAll: false, maxChars: MAX_TASK_TITLE, limitMessage: `A title is at most ${MAX_TASK_TITLE} characters`, tab: false};
const BODY_OPTIONS: EditOptions = {selectAll: false, maxChars: MAX_TASK_BODY, limitMessage: `A task's details are at most ${MAX_TASK_BODY} characters`, tab: false};

export function useTasksFlow({client, repoRoot, doc, tasks, sessions, spinnerFrame, onExit, onStart, onGoTo, onOpenNote, onDoc, onSession, setError, setStatusMessage}: TasksFlowOptions): TasksFlow {
	const [view, setView] = useState<'tasks' | 'notes'>('tasks');
	const [selected, setSelected] = useState<{key?: string; index: number}>({index: 0});
	const [showOlder, setShowOlder] = useState(false);
	const [editor, setEditor] = useState<TaskEditor | undefined>();
	const [pendingDelete, setPendingDelete] = useState<string | undefined>();
	const top = useRef(0);
	const busy = useRef(false);
	const bodyWidth = useRef(40);

	const items = view === 'notes' ? noteItems(sessions) : [];
	const rows: BoardRow[] = view === 'notes'
		? [{kind: 'heading', text: 'IN NOTES · open items, not tasks', count: items.length ? String(items.length) : undefined}, ...items.length ? items.map((item): BoardRow => ({kind: 'note', item})) : [{kind: 'empty', text: 'No open checklist items in the notes here'} as BoardRow]]
		: boardRows(tasks, showOlder);
	const selectable = rows.map((row, index) => (selectableRow(row) ? index : -1)).filter(index => index >= 0);
	// The selection follows its row (by key) across updates; a row that went keeps the position.
	const keyedIndex = selected.key ? rows.findIndex(row => rowKey(row) === selected.key) : -1;
	const current = keyedIndex >= 0 ? keyedIndex : selectable.length ? selectable[Math.min(selected.index, selectable.length - 1)]! : -1;
	const currentRow = current >= 0 ? rows[current] : undefined;
	const currentTask = currentRow?.kind === 'task' ? currentRow : undefined;

	const select = (index: number) => setSelected({key: rowKey(rows[index]!), index: Math.max(0, selectable.indexOf(index))});
	const move = (direction: -1 | 1) => {
		if (!selectable.length) return;
		const at = Math.max(0, selectable.indexOf(current));
		select(selectable[Math.min(selectable.length - 1, Math.max(0, at + direction))]!);
	};

	const run = <T,>(work: () => Promise<T>, then?: (value: T) => void) => {
		if (!client) { setError('still connecting to daemon'); return; }
		if (busy.current) return;
		busy.current = true;
		void work().then(value => then?.(value), error => setError(errorMessage(error))).finally(() => { busy.current = false; });
	};
	const apply = (op: Parameters<LiveClient['taskOp']>[1], then?: (doc: TasksDoc) => void) => run(() => client!.taskOp(repoRoot, op), next => { onDoc(next); then?.(next); });

	const save = (state: TaskEditor) => {
		const title = state.title.text.trim();
		if (!title) { setError('A task needs a title'); return; }
		const before = new Set(tasks.map(task => task.id));
		if (state.kind === 'add') apply({type: 'add', title, body: state.body.text}, next => {
			setEditor(undefined);
			const added = parseAdded(next, before);
			if (added) setSelected({key: `task:${added}`, index: 0});
			setStatusMessage('Added to the backlog');
		});
		else if (state.id) apply({type: 'edit', id: state.id, title, body: state.body.text}, () => { setEditor(undefined); setStatusMessage('Saved the task'); });
	};

	const editorInput = (input: string, key: Partial<Key>, state: TaskEditor) => {
		if (key.escape) { setEditor(undefined); return; }
		if (key.ctrl && input === 's') { save(state); return; }
		if (key.tab) { setEditor({...state, field: state.field === 'title' ? 'body' : 'title'}); return; }
		if (state.field === 'title') {
			if (key.return) { save(state); return; }
			if (key.upArrow || key.downArrow) return;
			const next = editText(state.title, input, key, TITLE_OPTIONS);
			if (next.message) { setError(next.message); return; }
			setEditor({...state, title: {text: next.text.replace(/\n/g, ' '), cursor: next.cursor}});
			return;
		}
		const options: EditOptions = {...BODY_OPTIONS, wrapWidth: bodyWidth.current, pageRows: EDITOR_BODY_ROWS};
		const next = key.return ? editText(state.body, '', {return: true}, options) : editText(state.body, input, key, options);
		if (next.message) { setError(next.message); return; }
		setEditor({...state, body: {text: next.text, cursor: next.cursor}});
	};

	const handleInput = (input: string, key: Partial<Key>) => {
		if (editor) { editorInput(input, key, editor); return; }
		const deleting = pendingDelete;
		if (deleting) setPendingDelete(undefined);
		if (key.upArrow || input === 'k') { move(-1); return; }
		if (key.downArrow || input === 'j') { move(1); return; }
		if ((input === 'g' || key.home) && selectable.length) { select(selectable[0]!); return; }
		if ((input === 'G' || key.end) && selectable.length) { select(selectable.at(-1)!); return; }
		if (view === 'notes') {
			if (key.escape || key.tab) { setView('tasks'); setSelected({index: 0}); return; }
			const item = currentRow?.kind === 'note' ? currentRow.item : undefined;
			if (input === 'a' && item) { promote(item); return; }
			if (key.return && item) { onOpenNote(item.sessionId); return; }
			return;
		}
		if (key.escape) { onExit(); return; }
		if (key.tab) { setView('notes'); setSelected({index: 0}); return; }
		if (input === 'a') { setEditor({kind: 'add', field: 'title', title: {text: '', cursor: 0}, body: {text: '', cursor: 0}}); return; }
		if (input === 'E') {
			run(() => client!.openTasks(repoRoot), file => {
				const label = openInEditor(file, message => setError(`${message}; the task list is ${file}`));
				if (label) setStatusMessage(`Opened the task list in ${label}; edits there show up here`);
			});
			return;
		}
		if (currentRow?.kind === 'older' && (key.return || input === ' ')) { setShowOlder(shown => !shown); return; }
		if (!currentTask) {
			if (input === 'n' || input === 'o' || input === 'x' || input === ' ' || key.return) setStatusMessage('Select a task first (j/k)');
			return;
		}
		const {task, group} = currentTask;
		if (key.return) { setEditor({kind: 'edit', id: task.id, field: 'title', title: {text: task.title, cursor: task.title.length}, body: {text: task.body, cursor: task.body.length}}); return; }
		if (input === ' ') { apply({type: 'toggle', id: task.id}, () => setStatusMessage(task.done ? `Reopened ${task.title}` : `Done: ${task.title}`)); return; }
		if (input === 'n') {
			if (group === 'backlog') onStart(task);
			else setStatusMessage(group === 'progress' ? 'That task is already being worked on (o opens its session)' : 'That task is done; space reopens it');
			return;
		}
		if (input === 'o') {
			const lead = leadSession(taskSessions(task, sessions));
			if (lead) onGoTo(lead.id);
			else setStatusMessage(group === 'progress' ? 'Its session is not in this list (archived or filtered?)' : 'That task has no session; n starts one');
			return;
		}
		if (input === 'x') {
			if (deleting === task.id) { apply({type: 'remove', id: task.id}, () => setStatusMessage(`Deleted ${task.title}`)); return; }
			setPendingDelete(task.id);
			setStatusMessage(`x again deletes “${truncate(task.title, 40)}”`);
			return;
		}
		if ((input === 'J' || input === 'K') && group !== 'done') {
			const direction = input === 'J' ? 1 : -1;
			const neighbor = neighborInGroup(rows, current, direction, group);
			if (neighbor) apply({type: 'move', id: task.id, target: neighbor.id, ...direction > 0 ? {after: true} : {}});
			return;
		}
	};

	const promote = (item: NoteItem) => run(() => client!.promoteNoteItem(item.sessionId, item.section, item.line, item.revision, item.noteId), result => {
		onSession(result.session);
		onDoc(result.tasks);
		setStatusMessage(`Sent to Tasks: ${truncate(item.title, 40)} (the note keeps a ↗ link)`);
	});

	const render = (width: number, height: number): React.ReactNode => {
		const inner = Math.max(10, width - 4);
		const open = tasks.filter(task => !task.done).length;
		const inProgress = rows.filter(row => row.kind === 'task' && row.group === 'progress').length;
		const name = path.basename(repoRoot) || repoRoot;
		const right = view === 'notes' ? 'tab back' : `${open} open · ${inProgress} in progress`;
		const lower = editor ? editorLines(editor, inner) : detailLines(currentRow, sessions, spinnerFrame, inner);
		// Border (2), title and status lines (2), the `+N more` line (1), then the rule and the lower part.
		const listRows = Math.max(1, height - 2 - 3 - (lower.length ? lower.length + 1 : 0));
		if (current >= 0) {
			if (current < top.current) top.current = current;
			if (current >= top.current + listRows) top.current = current - listRows + 1;
		}
		top.current = Math.max(0, Math.min(top.current, Math.max(0, rows.length - listRows)));
		const shown = rows.slice(top.current, top.current + listRows);
		return (
			<Box flexDirection="column" width={width} height={height} borderStyle="round" borderColor={THEME.borderActive} paddingX={1}>
				<Box justifyContent="space-between" width={inner}>
					<Text wrap="truncate-end"><Text color={THEME.active} bold>☐ Tasks</Text><Text color={THEME.muted}> · {truncate(name, Math.max(4, inner - right.length - 12))}</Text></Text>
					<Text color={THEME.muted}>{right}</Text>
				</Box>
				{doc?.tooLarge ? <Text color={THEME.warn} wrap="truncate-end">The task list is too large to change here; E opens it in your editor</Text> : <Text> </Text>}
				<Box flexDirection="column" height={listRows}>
					{shown.map((row, index) => <BoardLine key={rowKey(row) ?? `row-${top.current + index}`} row={row} selected={top.current + index === current && !editor} width={inner} sessions={sessions} spinnerFrame={spinnerFrame} />)}
				</Box>
				{rows.length > top.current + listRows ? <Text color={THEME.muted}>{`  +${rows.length - top.current - listRows} more`}</Text> : <Text> </Text>}
				{lower.length ? <Text color={THEME.border}>{'─'.repeat(inner)}</Text> : null}
				{lower.map((line, index) => <React.Fragment key={`lower-${index}`}>{line}</React.Fragment>)}
			</Box>
		);
	};

	const editorLines = (state: TaskEditor, width: number): React.ReactNode[] => {
		const label = (text: string, active: boolean) => <Text color={active ? THEME.active : THEME.muted} bold={active}>{text}</Text>;
		const titleWidth = Math.max(4, width - 8);
		const titleRow = wrappedEditorLines(state.title, titleWidth);
		const titleLine = titleRow.lines[Math.max(0, titleRow.cursorRow)] ?? {before: state.title.text, after: ''};
		bodyWidth.current = Math.max(4, width - 2);
		const body = wrappedEditorLines(state.body, bodyWidth.current);
		const start = Math.max(0, Math.min(body.cursorRow - EDITOR_BODY_ROWS + 1, body.lines.length - EDITOR_BODY_ROWS));
		const bodyRows = body.lines.slice(start, start + EDITOR_BODY_ROWS);
		const cursorOf = (line: {before: string; cursor?: string; after: string}, active: boolean) => <Text>{line.before}{active && line.cursor !== undefined ? <Text inverse>{line.cursor}</Text> : line.cursor ?? ''}{line.after}</Text>;
		return [
			<Text key="head" color={THEME.accent} bold>{state.kind === 'add' ? 'New task' : 'Edit task'}</Text>,
			<Text key="title" wrap="truncate-end">{label('Title  ', state.field === 'title')}{state.field === 'title' ? cursorOf(titleLine, true) : truncate(state.title.text, titleWidth)}</Text>,
			<Text key="body-label">{label('Details', state.field === 'body')}<Text color={THEME.muted}>{state.body.text || state.field === 'body' ? '' : " (optional; typed into the agent's input with the title)"}</Text></Text>,
			...bodyRows.map((line, index) => <Text key={`body-${index}`} wrap="truncate-end">{'  '}{cursorOf(line, state.field === 'body' && start + index === body.cursorRow)}</Text>),
		];
	};

	const hint = (width: number): string => {
		if (editor) return fitHint(editor.field === 'title'
			? ['enter save', 'tab details', 'esc cancel']
			: ['enter new line', 'ctrl+s save', 'tab title', 'esc cancel'], width, ' • ');
		if (view === 'notes') return fitHint(['a add as a task', 'enter open the note', 'j/k move', 'tab back'], width, ' • ');
		return fitHint([
			'j/k move', 'a add',
			{text: 'n start session', short: 'n start'},
			{text: 'enter edit', drop: 1},
			{text: 'space done', short: 'space ✓'},
			{text: 'o open its session', short: 'o session', drop: 1},
			{text: 'J/K reorder', drop: 2},
			{text: 'x delete', drop: 2},
			{text: 'tab note items', short: 'tab notes', drop: 1},
			{text: 'E editor', drop: 3},
			'esc back',
		], width, ' • ');
	};

	return {
		handleInput,
		render,
		hint,
		open: () => { setView('tasks'); setEditor(undefined); setPendingDelete(undefined); },
	};
}

/** The ID of the task `next` has that `before` did not (the one just added). */
function parseAdded(next: TasksDoc, before: Set<string>): string | undefined {
	const ids = [...next.text.matchAll(/<!--[ \t]*dh:t=([0-9a-f]+)/g)].map(match => match[1]!);
	return ids.find(id => !before.has(id));
}

/** The task row next to `index` in `direction` within the same group, if any. */
function neighborInGroup(rows: BoardRow[], index: number, direction: -1 | 1, group: string): Task | undefined {
	for (let at = index + direction; at >= 0 && at < rows.length; at += direction) {
		const row = rows[at]!;
		if (row.kind !== 'task') return undefined;
		if (row.group === group) return row.task;
	}
	return undefined;
}

function BoardLine({row, selected, width, sessions, spinnerFrame}: {row: BoardRow; selected: boolean; width: number; sessions: SessionRecord[]; spinnerFrame: string}) {
	if (row.kind === 'heading') return <Text wrap="truncate-end"><Text color={THEME.muted} bold>{row.text}</Text>{row.count ? <Text color={THEME.muted}>{` · ${row.count}`}</Text> : null}</Text>;
	if (row.kind === 'empty') return <Text color={THEME.muted} wrap="truncate-end">{`  ${row.text}`}</Text>;
	const marker = selected ? '›' : ' ';
	if (row.kind === 'older') {
		const text = `${marker} ${row.shown ? '▾' : '▸'} ${row.count} older done · enter ${row.shown ? 'hides' : 'shows'} them`;
		return <Text inverse={selected} color={selected ? THEME.active : THEME.muted} wrap="truncate-end">{text.padEnd(width)}</Text>;
	}
	if (row.kind === 'note') {
		const right = truncate(row.item.source, Math.floor(width / 3));
		const title = truncate(row.item.title, Math.max(1, width - right.length - 6));
		return <Text inverse={selected} bold={selected} color={selected ? THEME.active : undefined} wrap="truncate-end">{`${marker} ☐ ${title}`.padEnd(width - right.length)}<Text color={selected ? undefined : THEME.muted}>{right}</Text></Text>;
	}
	const state = taskState(row.task, sessions, spinnerFrame);
	const right = truncate([state.where, state.text].filter(Boolean).join('  '), Math.floor(width / 2));
	const title = truncate(row.task.title, Math.max(1, width - right.length - 6));
	return (
		<Text inverse={selected} bold={selected} color={selected ? THEME.active : row.task.done ? THEME.muted : undefined} wrap="truncate-end">
			{`${marker} `}<Text color={selected ? undefined : state.color}>{state.glyph}</Text>{` ${title}`.padEnd(Math.max(0, width - right.length - 2))}<Text color={selected ? undefined : row.group === 'progress' ? state.color : THEME.muted}>{right}</Text>
		</Text>
	);
}

/** The selected row's details: its title, where its work is, and its details text. */
function detailLines(row: BoardRow | undefined, sessions: SessionRecord[], spinnerFrame: string, width: number): React.ReactNode[] {
	if (row?.kind === 'note') return [
		<Text key="t" bold wrap="truncate-end">{row.item.title}</Text>,
		<Text key="s" color={THEME.muted} wrap="truncate-end">{`In the notes of ${row.item.source}. a adds it to the backlog; the note keeps a ↗ link.`}</Text>,
	];
	if (row?.kind !== 'task') return [];
	const {task, group} = row;
	const state = taskState(task, sessions, spinnerFrame);
	const where = group === 'progress' ? `${state.where ?? ''} · ${state.text} · o opens it` : group === 'backlog' ? `${state.text ? `${state.text} · ` : ''}n starts a session for it` : `${state.text}${state.where ? ` ${state.where}` : ''} · space reopens it`;
	const body = task.body ? wrapRows(task.body, Math.max(1, width - 2)).map(range => task.body.slice(range.start, range.end)) : [];
	const room = Math.max(0, DETAIL_ROWS - 2);
	return [
		<Text key="t" bold wrap="truncate-end">{task.title}</Text>,
		<Text key="w" color={THEME.muted} wrap="truncate-end">{where}</Text>,
		...body.slice(0, room).map((line, index) => <Text key={`b-${index}`} wrap="truncate-end">{`  ${index === room - 1 && body.length > room ? `${line.slice(0, Math.max(0, width - 6))} …` : line}`}</Text>),
	];
}

/** The Notes tab's first line for a session doing a task: the task, its state, and the board's key. */
export function TaskBanner({task, sessions, spinnerFrame, width}: {task: Task; sessions: SessionRecord[]; spinnerFrame: string; width: number}) {
	const state = taskState(task, sessions, spinnerFrame);
	const right = `${state.text ? `${state.text} · ` : ''}b board`;
	const title = truncate(task.title, Math.max(4, width - right.length - 10));
	return (
		<Box flexDirection="column" width={width}>
			<Box justifyContent="space-between" width={width}>
				<Text wrap="truncate-end"><Text color={THEME.accent}>◆ Task</Text> <Text color={THEME.accentSoft} bold>{title}</Text></Text>
				<Text color={THEME.muted}>{right}</Text>
			</Box>
			<Text color={THEME.border}>{'─'.repeat(Math.max(1, width))}</Text>
		</Box>
	);
}
