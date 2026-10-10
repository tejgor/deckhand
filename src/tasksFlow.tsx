import React, {useRef, useState} from 'react';
import path from 'node:path';
import {Box, Text, type Key} from 'ink';
import type {LiveClient} from './client.js';
import type {SessionRecord, TasksDoc} from './types.js';
import {MAX_TASK_BODY, MAX_TASK_TITLE, appendNoteItem, linkKey, linkOfKey, type Task} from './tasks.js';
import {blockItems, boardRows, hasMainTask, leadSession, noteGroupLabel, noteSteps, notesViewRows, pickerCounts, pickerRows, pickerViewOf, rowKey, selectableRow, taskLinkLookup, taskOrigin, taskSessions, taskState, workLabel, workNote, workOptions, workState, type BoardRow, type NoteItem, type PickerView, type WorkOption} from './tasksBoard.js';
import {toggleChecklistLine} from './notes.js';
import {editText, wrapRows, wrappedEditorLines, type EditOptions, type EditorState} from './textEditor.js';
import {openInEditor} from './desktop.js';
import {fitHint, scrolledListTop} from './menu.js';
import {THEME, errorMessage, stripTerminalControls, truncate} from './ui.js';
import {attentionReasonLines} from './sidebarModel.js';

// The Tasks board (b): the repository's task list in the right pane, grouped by the worktree (or main-checkout
// session) each open task is on, then the backlog; v narrows it to the work of the session it was opened from. Also the
// open note items not yet tasks (tab), a small editor for adding and editing a task (title, then body), and the w
// picker that moves a task to a worktree or a main-checkout session (two views, Tab switches; / searches, as in the sidebar), back to
// the backlog, or back to the note it was sent from. The daemon owns the file; every change is an op
// (src/tasks.ts) applied to the file as it is, so an editor's edits are never overwritten.

const DETAIL_ROWS = 5;
const EDITOR_BODY_ROWS = 4;
const PICKER_ROWS = 8;
const capitalize = (text: string) => (text ? `${text[0]!.toUpperCase()}${text.slice(1)}` : text);

/**
 * The w menu: the task, where it is now, the note it came from (if any), the view, the search (`typing` after /, until
 * Enter keeps it or Esc clears it) and the selected row.
 */
interface TaskPicker {taskId: string; title: string; current?: string; origin?: string; view: PickerView; query: string; typing: boolean; index: number}

interface TaskEditor {
	/** `step`: a new checklist item of `section`'s note (that work has its task already), title only. */
	kind: 'add' | 'edit' | 'step';
	id?: string;
	/** A new task's work key (it becomes that work's task, ◆), or the work whose note a step goes into; undefined: the backlog. */
	section?: string;
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
	/** b: the board's task view, the selection on the first open task of `scope` (the selected session's work, which v shows alone). */
	open(scope?: string): void;
}

const TITLE_OPTIONS: EditOptions = {selectAll: false, maxChars: MAX_TASK_TITLE, limitMessage: `A title is at most ${MAX_TASK_TITLE} characters`, tab: false};
const BODY_OPTIONS: EditOptions = {selectAll: false, maxChars: MAX_TASK_BODY, limitMessage: `A task's details are at most ${MAX_TASK_BODY} characters`, tab: false};

export function useTasksFlow({client, repoRoot, doc, tasks, sessions, spinnerFrame, onExit, onStart, onGoTo, onOpenNote, onDoc, onSession, setError, setStatusMessage}: TasksFlowOptions): TasksFlow {
	const [view, setView] = useState<'tasks' | 'notes'>('tasks');
	const [selected, setSelected] = useState<{key?: string; index: number}>({index: 0});
	const [showOlder, setShowOlder] = useState(false);
	const [editor, setEditor] = useState<TaskEditor | undefined>();
	const [pendingDelete, setPendingDelete] = useState<string | undefined>();
	const [scope, setScope] = useState<string | undefined>();
	const [scoped, setScoped] = useState(false);
	const [picker, setPicker] = useState<TaskPicker | undefined>();
	// The Notes view (tab): every note, or (f) only their open checklist items.
	const [itemsOnly, setItemsOnly] = useState(false);
	const top = useRef(0);
	const busy = useRef(false);
	const bodyWidth = useRef(40);

	// A work's steps: its note's checklist (a worktree's note, a main-checkout session's own), under its task.
	const stepsOf = (section: string) => noteSteps(workNote(section, sessions)?.text);
	const rows: BoardRow[] = view === 'notes'
		? notesViewRows(sessions, {itemsOnly, scope: scoped ? scope : undefined, links: doc ? taskLinkLookup(tasks) : undefined})
		: boardRows(tasks, showOlder, undefined, scoped ? scope : undefined, stepsOf);
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
		if (!title) { setError(state.kind === 'step' ? 'A step needs some text' : 'A task needs a title'); return; }
		if (state.kind === 'step') { addStep(state.section!, title, () => setEditor(undefined)); return; }
		const before = new Set(tasks.map(task => task.id));
		const link = state.section ? linkOfKey(state.section) : undefined;
		if (state.kind === 'add') apply({type: 'add', title, body: state.body.text, ...link ? {link} : {}}, next => {
			setEditor(undefined);
			const added = parseAdded(next, before);
			if (added) setSelected({key: `task:${added}`, index: 0});
			setStatusMessage(state.section ? `Added as the task of ${workLabel(state.section, sessions)} (◆)` : 'Added to the backlog');
		});
		else if (state.id) apply({type: 'edit', id: state.id, title, body: state.body.text}, () => { setEditor(undefined); setStatusMessage('Saved the task'); });
	};

	const editorInput = (input: string, key: Partial<Key>, state: TaskEditor) => {
		if (key.escape) { setEditor(undefined); return; }
		if (key.ctrl && input === 's') { save(state); return; }
		if (key.tab) { if (state.kind !== 'step') setEditor({...state, field: state.field === 'title' ? 'body' : 'title'}); return; }
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

	// A new step at the end of a work's note (revision-checked, as if typed in its Notes tab).
	const addStep = (section: string, title: string, then?: () => void) => {
		const note = workNote(section, sessions);
		if (!note) { setStatusMessage('Its note is not in this list (archived or filtered?)'); return; }
		if (note.readOnly) { setStatusMessage('Its worktree was deleted: its note is read-only'); return; }
		run(() => client!.saveNote(note.sessionId, note.section, appendNoteItem(note.text, title), note.revision, note.noteId), result => {
			onSession(result.session);
			if (!result.saved) { setStatusMessage('The note changed meanwhile; try again'); return; }
			then?.();
			setStatusMessage(`Added a step to ${workLabel(section, sessions)}: ${truncate(title, 40)}`);
		});
	};

	// Onto work without a task, w makes the task that work's (◆); onto work with one, a step in its note.
	const becomesStep = (state: TaskPicker, section: string | undefined) => Boolean(section) && hasMainTask(section!, tasks, state.taskId);
	const pickerRowsOf = (state: TaskPicker) => pickerRows(workOptions(sessions, state.current), state.view, state.query, state.origin);
	// Where the selection starts in a view: where the task is now (without a search), else the first listed match.
	const pickerStart = (state: TaskPicker) => {
		const rows = pickerRowsOf(state);
		const listed = (option: WorkOption) => option.kind === 'worktree' || option.kind === 'session';
		const here = state.query ? -1 : rows.findIndex(option => listed(option) && option.section === state.current);
		return here >= 0 ? here : Math.max(0, rows.findIndex(listed));
	};
	const openPicker = (state: Omit<TaskPicker, 'index'>) => setPicker({...state, index: pickerStart({...state, index: 0})});

	const pickerInput = (input: string, key: Partial<Key>, state: TaskPicker) => {
		const rows = pickerRowsOf(state);
		const to = (index: number) => setPicker({...state, index: Math.max(0, Math.min(rows.length - 1, index))});
		const switchView = () => openPicker({...state, view: state.view === 'worktrees' ? 'sessions' : 'worktrees'});
		if (key.tab) { switchView(); return; }
		if (key.upArrow) { to(state.index - 1); return; }
		if (key.downArrow) { to(state.index + 1); return; }
		if (key.pageUp) { to(state.index - PICKER_ROWS); return; }
		if (key.pageDown) { to(state.index + PICKER_ROWS); return; }
		// Searching (/): keys are text until Enter keeps the search or Esc clears it, as in the sidebar's search.
		if (state.typing) {
			if (key.escape) { openPicker({...state, query: '', typing: false}); return; }
			if (key.return) { setPicker({...state, typing: false}); return; }
			if (key.backspace || key.delete) { openPicker({...state, query: state.query.slice(0, -1)}); return; }
			const typed = key.ctrl || key.meta ? '' : stripTerminalControls(input);
			if (typed) openPicker({...state, query: (state.query + typed).slice(0, 64)});
			return;
		}
		if (key.escape) { if (state.query) openPicker({...state, query: ''}); else setPicker(undefined); return; }
		if (input === '/') { setPicker({...state, typing: true}); return; }
		if (key.leftArrow || key.rightArrow || input === 'h' || input === 'l') { switchView(); return; }
		if (input === 'k') { to(state.index - 1); return; }
		if (input === 'j') { to(state.index + 1); return; }
		if (key.home || input === 'g') { to(0); return; }
		if (key.end || input === 'G') { to(rows.length - 1); return; }
		if (!key.return) return;
		const option = rows[state.index];
		if (!option) return;
		if (option.kind === 'note') {
			run(() => client!.returnTaskToNote(repoRoot, state.taskId), next => {
				onDoc(next);
				setPicker(undefined);
				setStatusMessage(`Back in the note of ${state.origin}: ${truncate(state.title, 40)}`);
			});
			return;
		}
		if (option.section === state.current) { setPicker(undefined); setStatusMessage('It is already there'); return; }
		if (becomesStep(state, option.section)) {
			run(() => client!.taskToNote(repoRoot, state.taskId, option.section!), next => {
				onDoc(next);
				setPicker(undefined);
				setStatusMessage(`Now a step in the note of ${option.label} (it has its task already): ${truncate(state.title, 40)}`);
			});
			return;
		}
		const link = option.section ? linkOfKey(option.section) : undefined;
		apply({type: 'assign', id: state.taskId, ...link ? {link} : {}}, () => {
			setPicker(undefined);
			setSelected({key: `task:${state.taskId}`, index: 0});
			setStatusMessage(!option.section ? 'Moved to the backlog' : `Now the task of ${option.label} (◆): its note's checklist shows as its steps, merging or finishing it ticks it`);
		});
	};

	// Space on a step: ticks it in its note (revision-checked, like typing in the Notes tab).
	const toggleStep = (row: Extract<BoardRow, {kind: 'step'}>) => {
		const note = workNote(row.section, sessions);
		if (!note) { setStatusMessage('Its note is not in this list (archived or filtered?)'); return; }
		if (note.readOnly) { setStatusMessage('Its worktree was deleted: its note is read-only'); return; }
		const text = toggleChecklistLine(note.text, row.step.line);
		if (text === undefined) { setStatusMessage('That line of the note changed; try again'); return; }
		run(() => client!.saveNote(note.sessionId, note.section, text, note.revision, note.noteId), result => {
			onSession(result.session);
			setStatusMessage(!result.saved ? 'The note changed meanwhile; try again' : row.step.done ? `Reopened: ${truncate(row.step.text, 50)}` : `Ticked: ${truncate(row.step.text, 50)}`);
		});
	};

	const firstTaskOf = (section: string | undefined) => (section ? tasks.find(task => !task.done && linkKey(task.meta) === section) : undefined);

	const handleInput = (input: string, key: Partial<Key>) => {
		if (editor) { editorInput(input, key, editor); return; }
		if (picker) { pickerInput(input, key, picker); return; }
		const deleting = pendingDelete;
		if (deleting) setPendingDelete(undefined);
		if (key.upArrow || input === 'k') { move(-1); return; }
		if (key.downArrow || input === 'j') { move(1); return; }
		if ((input === 'g' || key.home) && selectable.length) { select(selectable[0]!); return; }
		if ((input === 'G' || key.end) && selectable.length) { select(selectable.at(-1)!); return; }
		// v: only the work of the session the board was opened from (its worktree), in either view.
		if (input === 'v') {
			if (!scope) { setStatusMessage(`Select a session first: v shows the ${view === 'notes' ? 'notes' : 'tasks'} of its worktree`); return; }
			const first = view === 'tasks' ? firstTaskOf(scope) : undefined;
			setScoped(on => !on);
			setSelected(first ? {key: `task:${first.id}`, index: 0} : {index: 0});
			return;
		}
		if (view === 'notes') {
			if (key.escape || key.tab) { setView('tasks'); setSelected({index: 0}); return; }
			if (input === 'f') { setItemsOnly(on => !on); setSelected({index: 0}); return; }
			const line = currentRow?.kind === 'noteline' ? currentRow : undefined;
			const block = currentRow?.kind === 'notehead' || currentRow?.kind === 'noteline' ? currentRow.block : undefined;
			if (input === 'a') {
				if (line?.item) promote(line.item);
				else setStatusMessage('Select an open checklist item (☐) to add it as a task');
				return;
			}
			// A: every open item of the selected note at once.
			if (input === 'A') {
				if (!block) { setStatusMessage('Select a note first (j/k)'); return; }
				const open = blockItems(block, sessions).length;
				if (!open) { setStatusMessage('No open items in this note'); return; }
				run(() => client!.sendOpenItems(block.sessionId, block.section, block.revision, block.noteId), result => {
					onSession(result.session);
					onDoc(result.tasks);
					setStatusMessage(`Sent ${result.sent} item${result.sent === 1 ? '' : 's'} to Tasks (the note keeps a ↗ link for each)`);
				});
				return;
			}
			if (key.return && block) { onOpenNote(block.sessionId); return; }
			return;
		}
		if (key.escape) { onExit(); return; }
		if (key.tab) { setView('notes'); setSelected({index: 0}); return; }
		if (input === 'a') {
			const section = addSection();
			setEditor({kind: section && hasMainTask(section, tasks) ? 'step' : 'add', section, field: 'title', title: {text: '', cursor: 0}, body: {text: '', cursor: 0}});
			return;
		}
		if (input === 'E') {
			run(() => client!.openTasks(repoRoot), file => {
				const label = openInEditor(file, message => setError(`${message}; the task list is ${file}`));
				if (label) setStatusMessage(`Opened the task list in ${label}; edits there show up here`);
			});
			return;
		}
		if (currentRow?.kind === 'older' && (key.return || input === ' ')) { setShowOlder(shown => !shown); return; }
		if (currentRow?.kind === 'step') {
			if (input === ' ') { toggleStep(currentRow); return; }
			const note = workNote(currentRow.section, sessions);
			if (key.return) { if (note) onOpenNote(note.sessionId); else setStatusMessage('Its note is not in this list (archived or filtered?)'); return; }
			if (input === 'n' || input === 'o' || input === 'x' || input === 'w' || input === 'J' || input === 'K') setStatusMessage('A step from the note: space ticks it, enter opens the note to edit it');
			return;
		}
		if (!currentTask) {
			if (input === 'n' || input === 'o' || input === 'x' || input === 'w' || input === ' ' || key.return) setStatusMessage('Select a task first (j/k)');
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
		if (input === 'w') {
			if (task.done) { setStatusMessage('That task is done; space reopens it'); return; }
			const current = linkKey(task.meta);
			openPicker({taskId: task.id, title: task.title, current, origin: taskOrigin(task, sessions), view: pickerViewOf(current), query: '', typing: false});
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
			const neighbor = neighborInGroup(rows, current, direction, currentTask.section);
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
		const narrowed = scoped && scope;
		const name = narrowed ? workLabel(scope, sessions) : path.basename(repoRoot) || repoRoot;
		const openHere = narrowed ? tasks.filter(task => !task.done && linkKey(task.meta) === scope).length : 0;
		const openItems = rows.filter(row => row.kind === 'noteline' && row.item).length;
		const right = view === 'notes' ? `${openItems} open item${openItems === 1 ? '' : 's'}${itemsOnly ? ' · items only' : ''}${narrowed ? ' · v all' : ''}`
			: narrowed ? `${openHere} open · v all tasks` : `${open} open · ${inProgress} in progress`;
		const lower = picker ? pickerLines(picker, inner) : editor ? editorLines(editor, inner) : detailLines(currentRow, sessions, spinnerFrame, inner);
		// Border (2), title and status lines (2), the `+N more` line (1), then the rule and the lower part.
		const listRows = Math.max(1, height - 2 - 3 - (lower.length ? lower.length + 1 : 0));
		top.current = scrolledListTop(top.current, current, listRows, rows.length, index => selectableRow(rows[index]!));
		const shown = rows.slice(top.current, top.current + listRows);
		return (
			<Box flexDirection="column" width={width} height={height} borderStyle="round" borderColor={THEME.borderActive} paddingX={1}>
				<Box justifyContent="space-between" width={inner}>
					<Text wrap="truncate-end"><Text color={THEME.active} bold>{view === 'notes' ? '✎ Notes' : '☐ Tasks'}</Text><Text color={THEME.muted}> · {truncate(name, Math.max(4, inner - right.length - 12))}</Text></Text>
					<Text color={THEME.muted}>{right}</Text>
				</Box>
				{doc?.tooLarge ? <Text color={THEME.warn} wrap="truncate-end">The task list is too large to change here; E opens it in your editor</Text> : <Text> </Text>}
				<Box flexDirection="column" height={listRows}>
					{shown.map((row, index) => <BoardLine key={rowKey(row) ?? `row-${top.current + index}`} row={row} selected={top.current + index === current && !editor && !picker} width={inner} sessions={sessions} spinnerFrame={spinnerFrame} />)}
				</Box>
				{rows.length > top.current + listRows ? <Text color={THEME.muted}>{`  +${rows.length - top.current - listRows} more`}</Text> : <Text> </Text>}
				{lower.length ? <Text color={THEME.border}>{'─'.repeat(inner)}</Text> : null}
				{lower.map((line, index) => <React.Fragment key={`lower-${index}`}>{line}</React.Fragment>)}
			</Box>
		);
	};

	const pickerLines = (state: TaskPicker, width: number): React.ReactNode[] => {
		const options = workOptions(sessions, state.current);
		const rows = pickerRows(options, state.view, state.query, state.origin);
		const counts = pickerCounts(options, state.query);
		const start = Math.max(0, Math.min(state.index - PICKER_ROWS + 1, rows.length - PICKER_ROWS));
		const tab = (view: PickerView, text: string) => <Text inverse={state.view === view} bold={state.view === view} color={state.view === view ? THEME.active : THEME.muted}>{` ${text} `}</Text>;
		const listed = rows.some(option => option.kind === 'worktree' || option.kind === 'session');
		const none = state.query ? `Nothing in ${state.view} matches “${state.query}”` : state.view === 'worktrees' ? 'No worktrees' : 'No sessions in the main checkout';
		return [
			<Text key="head" wrap="truncate-end"><Text color={THEME.accent} bold>Move </Text>{truncate(`“${state.title}”`, Math.max(4, width - 12))}<Text color={THEME.accent} bold> to</Text></Text>,
			<Text key="views" wrap="truncate-end">{tab('worktrees', `Worktrees ${counts.worktrees}`)} {tab('sessions', `Sessions ${counts.sessions}`)}<Text color={state.query || state.typing ? THEME.active : THEME.muted}>{`   ${state.typing ? `search: ${state.query}▏` : state.query ? `search: ${state.query}` : '/ search'}`}</Text></Text>,
			...rows.slice(start, start + PICKER_ROWS).map((option, offset) => {
				const selected = start + offset === state.index;
				// Worktrees and sessions say what the task would be there: its task (◆), or a step in its note.
				const role = (option.kind === 'worktree' || option.kind === 'session') && option.section !== state.current ? (becomesStep(state, option.section) ? '→ a step in its note' : '◆ its task') : '';
				const note = [option.note, role].filter(Boolean).length ? ` ${[option.note, role].filter(Boolean).join(' · ')}` : '';
				return <Text key={option.section ?? option.kind} inverse={selected} bold={selected} color={selected ? THEME.active : undefined} wrap="truncate-end">
					{`${selected ? '›' : ' '} ${truncate(option.label, Math.max(1, width - note.length - 2))}`.padEnd(Math.max(0, width - note.length))}<Text color={selected ? undefined : THEME.muted}>{note}</Text>
				</Text>;
			}),
			...listed ? [] : [<Text key="none" color={THEME.muted} wrap="truncate-end">{`  ${none}`}</Text>],
		];
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
			<Text key="head" wrap="truncate-end"><Text color={THEME.accent} bold>{state.kind === 'step' ? 'New step' : state.kind === 'add' ? 'New task' : 'Edit task'}</Text>{state.kind !== 'edit' ? <Text color={THEME.muted}>{` · ${state.section ? `${state.kind === 'step' ? 'the note of ' : 'the task of '}${workLabel(state.section, sessions)}` : 'backlog'}`}</Text> : null}</Text>,
			<Text key="title" wrap="truncate-end">{label('Title  ', state.field === 'title')}{state.field === 'title' ? cursorOf(titleLine, true) : truncate(state.title.text, titleWidth)}</Text>,
			<Text key="body-label">{label('Details', state.field === 'body')}<Text color={THEME.muted}>{state.body.text || state.field === 'body' ? '' : " (optional; typed into the agent's input with the title)"}</Text></Text>,
			...bodyRows.map((line, index) => <Text key={`body-${index}`} wrap="truncate-end">{'  '}{cursorOf(line, state.field === 'body' && start + index === body.cursorRow)}</Text>),
		];
	};

	// A new task joins the group the selection is in (a worktree's, or the backlog); in the v view, that work's.
	// a in a work's group: a step in its note (it has its task), or its task when it has none; elsewhere the backlog.
	const addSection = () => scoped ? scope : currentTask?.group === 'progress' ? currentTask.section : currentRow?.kind === 'step' ? currentRow.section : undefined;

	const hint = (width: number): string => {
		const addTo = addSection(), addsStep = Boolean(addTo && hasMainTask(addTo, tasks));
		if (editor) return fitHint(editor.kind === 'step' ? ['enter add to the note', 'esc cancel'] : editor.field === 'title'
			? ['enter save', 'tab details', 'esc cancel']
			: ['enter new line', 'ctrl+s save', 'tab title', 'esc cancel'], width, ' • ');
		if (picker) return fitHint(picker.typing
			? ['type to search', 'enter done', '↑↓ choose', 'esc clear']
			: ['enter move here', {text: 'j/k choose', short: 'j/k'}, picker.query ? '/ edit search' : '/ search', {text: 'tab worktrees / sessions', short: 'tab view'}, picker.query ? 'esc clear search' : 'esc cancel'], width, ' • ');
		if (view === 'notes') {
			const onItem = currentRow?.kind === 'noteline' && Boolean(currentRow.item);
			const onNote = currentRow?.kind === 'notehead' || currentRow?.kind === 'noteline';
			return fitHint([
				...onItem ? ['a add as a task'] : [],
				...onNote ? [{text: 'A send all open items', short: 'A send all', drop: 1}] : [],
				{text: 'enter open the note', short: 'enter open'},
				{text: itemsOnly ? 'f every note' : 'f open items only', short: 'f filter', drop: 1},
				...scope ? [{text: scoped ? 'v all notes' : 'v this worktree', short: 'v view', drop: 1}] : [],
				{text: 'j/k move', drop: 2},
				'tab tasks', 'esc tasks',
			], width, ' • ');
		}
		if (currentRow?.kind === 'step') return fitHint(['j/k move', {text: currentRow.step.done ? 'space reopen' : 'space tick'}, {text: 'enter open the note', short: 'enter note'}, {text: 'a add step', short: 'a step'}, 'esc back'], width, ' • ');
		if (currentRow?.kind === 'empty') return fitHint(['j/k move', {text: 'a add task', short: 'a add'}, 'esc back'], width, ' • ');
		return fitHint([
			'j/k move', addsStep ? {text: 'a add step', short: 'a step'} : {text: 'a add task', short: 'a add'},
			{text: 'n start session', short: 'n start'},
			{text: 'w move to a worktree', short: 'w move'},
			{text: 'enter edit', drop: 1},
			{text: 'space done', short: 'space ✓'},
			...scope ? [{text: scoped ? 'v all tasks' : 'v this worktree', short: 'v view', drop: 1}] : [],
			{text: 'o open its session', short: 'o session', drop: 1},
			{text: 'J/K reorder', drop: 2},
			{text: 'x delete', drop: 2},
			{text: 'tab notes', drop: 1},
			{text: 'E editor', drop: 3},
			'esc back',
		], width, ' • ');
	};

	return {
		handleInput,
		render,
		hint,
		open: (next?: string) => {
			setView('tasks'); setEditor(undefined); setPicker(undefined); setPendingDelete(undefined);
			setScope(next); setScoped(false);
			const first = firstTaskOf(next);
			if (first) setSelected({key: `task:${first.id}`, index: 0});
		},
	};
}

/** The ID of the task `next` has that `before` did not (the one just added). */
function parseAdded(next: TasksDoc, before: Set<string>): string | undefined {
	const ids = [...next.text.matchAll(/<!--[ \t]*dh:t=([0-9a-f]+)/g)].map(match => match[1]!);
	return ids.find(id => !before.has(id));
}

/** The task row next to `index` in `direction` within the same group (a worktree's, the backlog, done), if any. */
function neighborInGroup(rows: BoardRow[], index: number, direction: -1 | 1, section: string): Task | undefined {
	const row = rows[index + direction];
	return row?.kind === 'task' && row.section === section ? row.task : undefined;
}

function BoardLine({row, selected, width, sessions, spinnerFrame}: {row: BoardRow; selected: boolean; width: number; sessions: SessionRecord[]; spinnerFrame: string}) {
	if (row.kind === 'heading') return <Text wrap="truncate-end"><Text color={THEME.muted} bold>{row.text}</Text>{row.count ? <Text color={THEME.muted}>{` · ${row.count}`}</Text> : null}</Text>;
	if (row.kind === 'empty') return <Text inverse={selected} color={selected ? THEME.active : THEME.muted} wrap="truncate-end">{`${selected ? '›' : ' '} ${row.text}`.padEnd(width)}</Text>;
	if (row.kind === 'work') {
		// The work's name and open count, its most urgent session's state on the right.
		const work = row.section === 'main' ? undefined : workState(row.section, sessions, spinnerFrame);
		const state = work ? `${work.glyph} ${work.text}` : '';
		const label = truncate(row.label ?? workLabel(row.section, sessions), Math.max(4, width - state.length - 8));
		return <Box justifyContent="space-between" width={width}>
			<Text wrap="truncate-end"><Text color={THEME.accentSoft} bold>{label}</Text>{row.count ? <Text color={THEME.muted}>{` · ${row.count}`}</Text> : null}</Text>
			{work ? <Text color={work.color}>{state}</Text> : null}
		</Box>;
	}
	const marker = selected ? '›' : ' ';
	if (row.kind === 'older') {
		const text = `${marker} ${row.shown ? '▾' : '▸'} ${row.count} older done · enter ${row.shown ? 'hides' : 'shows'} them`;
		return <Text inverse={selected} color={selected ? THEME.active : THEME.muted} wrap="truncate-end">{text.padEnd(width)}</Text>;
	}
	// The Notes view: a note's name (and its open items), then its lines, indented under it.
	if (row.kind === 'notehead') {
		const right = row.open ? `${row.open} open` : '';
		return <Text inverse={selected} bold color={selected ? THEME.active : THEME.accentSoft} wrap="truncate-end">
			{`${marker} ${truncate(capitalize(row.block.label), Math.max(1, width - right.length - 4))}`.padEnd(Math.max(0, width - right.length))}<Text color={selected ? undefined : THEME.muted} bold={false}>{right}</Text>
		</Text>;
	}
	if (row.kind === 'noteline') {
		const color = selected ? THEME.active : row.style === 'done' ? THEME.muted : row.style === 'link' ? THEME.accentSoft : undefined;
		return <Text inverse={selected} bold={selected || row.style === 'heading'} color={color} wrap="truncate-end">{`${marker}   ${truncate(row.text, Math.max(1, width - 4))}`.padEnd(width)}</Text>;
	}
	// A step: an item of the work's note, under the task it was started for.
	if (row.kind === 'step') {
		const color = selected ? THEME.active : row.step.done ? THEME.muted : undefined;
		return <Text inverse={selected} bold={selected} color={color} wrap="truncate-end">{`${marker}   ${row.step.done ? '☑' : '☐'} ${truncate(row.step.text, Math.max(1, width - 6))}`.padEnd(width)}</Text>;
	}
	// In a worktree's group its heading shows the state: the task is ◆, counting its steps (the work note's checklist).
	const inWork = row.group === 'progress';
	const state = taskState(row.task, sessions, spinnerFrame);
	const glyph = inWork ? {text: '◆', color: THEME.accentSoft} : {text: state.glyph, color: state.color};
	const right = inWork ? (row.steps ? `${row.steps.done}/${row.steps.total}` : '') : truncate([state.where, state.text].filter(Boolean).join('  '), Math.floor(width / 2));
	const title = truncate(row.task.title, Math.max(1, width - right.length - 6));
	return (
		<Text inverse={selected} bold={selected} color={selected ? THEME.active : row.task.done ? THEME.muted : undefined} wrap="truncate-end">
			{`${marker} `}<Text color={selected ? undefined : glyph.color}>{glyph.text}</Text>{` ${title}`.padEnd(Math.max(0, width - right.length - 3))}<Text color={selected ? undefined : THEME.muted}>{right}</Text>
		</Text>
	);
}

/** The selected row's details: its title, where its work is, and its details text. */
function detailLines(row: BoardRow | undefined, sessions: SessionRecord[], spinnerFrame: string, width: number): React.ReactNode[] {
	if (row?.kind === 'notehead' || row?.kind === 'noteline') {
		const {block} = row;
		const where = `${block.section === 'shared' ? `The ${block.label}` : `The notes of ${block.label}`} · ${noteGroupLabel(block.group, sessions)}`;
		if (row.kind === 'notehead') return [
			<Text key="t" bold wrap="truncate-end">{where}</Text>,
			<Text key="s" color={THEME.muted} wrap="truncate-end">{`${row.open ? `${row.open} open item${row.open === 1 ? '' : 's'} · ` : ''}enter opens it in its session’s Notes tab`}</Text>,
		];
		const text = wrapRows(row.text.trim(), Math.max(1, width - 2)).map(range => row.text.trim().slice(range.start, range.end));
		return [
			...text.slice(0, DETAIL_ROWS - 1).map((line, index) => <Text key={`t-${index}`} bold wrap="truncate-end">{line}</Text>),
			<Text key="s" color={THEME.muted} wrap="truncate-end">{`${where} · ${row.item ? 'a adds it as a task (on that work), the note keeps a ↗ link · ' : ''}enter opens the note`}</Text>,
		];
	}
	if (row?.kind === 'step') {
		const work = workLabel(row.section, sessions);
		return [
			<Text key="t" bold wrap="truncate-end">{row.step.text}</Text>,
			<Text key="w" color={THEME.muted} wrap="truncate-end">{`A step: a checklist item in the note of ${work} · space ${row.step.done ? 'reopens' : 'ticks'} it · enter opens the note to edit it`}</Text>,
		];
	}
	if (row?.kind !== 'task') return [];
	const {task, group} = row;
	const state = taskState(task, sessions, spinnerFrame);
	const where = group === 'progress' ? `The task of ${workLabel(linkKey(task.meta)!, sessions)}: merging or marking it done ticks it · o opens it` : group === 'backlog' ? `${state.text ? `${state.text} · ` : ''}n starts a session for it` : `${state.text}${state.where ? ` ${state.where}` : ''} · space reopens it`;
	// Sent from a note that still links it: w can put it back there.
	const origin = taskOrigin(task, sessions);
	const back = origin ? ` · w: back to the note of ${origin}` : '';
	const body = task.body ? wrapRows(task.body, Math.max(1, width - 2)).map(range => task.body.slice(range.start, range.end)) : [];
	// What its work's most urgent session asks or said (agent signals), before the body.
	const lead = group === 'progress' ? leadSession(taskSessions(task, sessions)) : undefined;
	const reason = lead ? attentionReasonLines(lead, width, 1)[0]?.[0] : undefined;
	const room = Math.max(0, DETAIL_ROWS - 2 - (reason ? 1 : 0));
	return [
		<Text key="t" bold wrap="truncate-end">{task.title}</Text>,
		<Text key="w" color={THEME.muted} wrap="truncate-end">{where}{back}</Text>,
		...reason ? [<Text key="r" color={reason.color} wrap="truncate-end">{reason.text}</Text>] : [],
		...body.slice(0, room).map((line, index) => <Text key={`b-${index}`} wrap="truncate-end">{`  ${index === room - 1 && body.length > room ? `${line.slice(0, Math.max(0, width - 6))} …` : line}`}</Text>),
	];
}

/** The Notes tab's first line for a session doing a task: the task, how many more are open on its work, its state, and the board's key. */
export function TaskBanner({task, more = 0, steps, sessions, spinnerFrame, width}: {task: Task; more?: number; /** The work note's checklist, done of total (the started task's steps). */ steps?: {done: number; total: number}; sessions: SessionRecord[]; spinnerFrame: string; width: number}) {
	const state = taskState(task, sessions, spinnerFrame);
	const right = `${steps?.total ? `${steps.done}/${steps.total} steps · ` : ''}${more ? `+${more} open here · ` : ''}${state.text ? `${state.text} · ` : ''}b board`;
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
