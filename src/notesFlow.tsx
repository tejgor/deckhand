import React, {useCallback, useEffect, useRef, useState} from 'react';
import path from 'node:path';
import type {Key} from 'ink';
import type {LiveClient} from './client.js';
import type {SessionRecord, TasksDoc} from './types.js';
import {MAX_NOTES_CHARS, MAX_NOTES_LABEL, continueChecklist, insertChecklistItem, noteKey, parseChecklistLine, showsOwnNote, toggleChecklist, type NoteSection, type TaskLinkLookup} from './notes.js';
import {NotesMessage, NotesRows, notesLayout, type NotesSectionInput} from './notesPane.js';
import {cleanInsertedText, editText, lineEnd, lineStart, replaceRange, type EditOptions, type EditorState} from './textEditor.js';
import {openInEditor} from './desktop.js';
import {fitHint} from './menu.js';
import {displaySessionTitle, errorMessage} from './ui.js';

// The Notes tab's state: one draft per note (a shared note is one draft for every session showing it, so its cursor
// and unsaved text follow it), debounced revision-checked saves, reloads when the file changed elsewhere, and the keys
// of notes focus (Enter). The daemon owns the files (src/notesStore.ts); records carry their text and revision.

const SAVE_DEBOUNCE_MS = 300;

interface Draft {
	/** The session and section it is saved through (the last one edited from). */
	sessionId: string;
	section: NoteSection;
	/** For a shared note: `kind:id`, so a save never lands in another note. */
	noteId?: string;
	text: string;
	cursor: number;
	/** The file's text and revision as last known; `text` differs from `saved` while unsaved. */
	saved: string;
	revision: string;
	/** The text of the save in flight, and its promise. */
	inFlight?: string;
	pending?: Promise<void>;
	scrollTop: number;
}

interface NoteDoc {text: string; revision: string; readOnly?: boolean; tooLarge?: boolean}
function docOf(session: SessionRecord | undefined, section: NoteSection): NoteDoc | undefined {
	if (!session) return undefined;
	if (section === 'shared') return session.sharedNotes;
	return session.notesFile ? {text: session.notes ?? '', revision: session.notesFile.revision, tooLarge: session.notesFile.tooLarge} : undefined;
}

interface NotesFlowOptions {
	client?: LiveClient;
	session?: SessionRecord;
	sessions: SessionRecord[];
	focused: boolean;
	onExit: () => void;
	setError: (error: string | undefined) => void;
	setStatusMessage: (message: string | undefined) => void;
	/** Ctrl+P sent a checklist item to Tasks: the task list as it is now. */
	onTasks?: (tasks: TasksDoc) => void;
	/** Whether each task a `↗` line links to is open, done or gone (undefined until the list is loaded). */
	links?: TaskLinkLookup;
}
export interface NotesFlow {
	/** Enter / →: start editing (the section last edited, the session's by default). */
	focus(): void;
	handleInput(input: string, key: Partial<Key>): void;
	/** E (browse) / Ctrl+O (editing): the active section's file in Cursor / VS Code. */
	openActiveInEditor(): void;
	/** o (browse): saves the active section and resolves to its file, for the terminal editor; undefined if unavailable. */
	activeFile(): Promise<string | undefined>;
	render(width: number, height: number): React.ReactNode;
	hint(width: number): string;
}

const plural = (count: number, word: string) => `${count} ${word}${count === 1 ? '' : 's'}`;

export function useNotesFlow({client, session, sessions, focused, onExit, setError, setStatusMessage, onTasks, links}: NotesFlowOptions): NotesFlow {
	const drafts = useRef(new Map<string, Draft>());
	const timers = useRef(new Map<string, NodeJS.Timeout>());
	const [, setVersion] = useState(0);
	const rerender = useCallback(() => setVersion(version => version + 1), []);
	// The section notes focus edits (and E opens); kept while switching sessions.
	const [section, setSection] = useState<NoteSection>('session');
	// The editor's wrap width and body height from the last render, for Up/Down and PageUp/PageDown.
	const viewport = useRef({width: 40, rows: 10});
	const pasting = useRef(false);
	const latest = useRef({client, setError});
	latest.current = {client, setError};

	const hasShared = Boolean(session?.sharedNotes);
	const sharedEditable = Boolean(session?.sharedNotes && !session.sharedNotes.readOnly);
	// A session in a worktree has one note, the worktree's; a main-checkout session has its own and the main checkout's.
	const ownNote = !session || showsOwnNote(session);
	const activeSection: NoteSection = hasShared && (section === 'shared' || !ownNote) ? 'shared' : 'session';

	// Applies a newer version of the file: silently when nothing is unsaved, else with a message (the daemon refuses the
	// stale save anyway; external edits are never overwritten).
	const reconcile = (draft: Draft, doc: NoteDoc) => {
		if (doc.revision === draft.revision) return;
		if (draft.inFlight !== undefined && doc.text === draft.inFlight) { draft.saved = doc.text; draft.revision = doc.revision; return; }
		const unsaved = draft.text !== draft.saved || draft.inFlight !== undefined;
		const discarded = unsaved && draft.text !== doc.text;
		draft.text = doc.text; draft.saved = doc.text; draft.revision = doc.revision;
		draft.cursor = Math.min(draft.cursor, doc.text.length);
		// Reconciling may happen while rendering: report it after.
		if (discarded) queueMicrotask(() => latest.current.setError('These notes changed outside Deckhand (an editor?): reloaded them; your last unsaved edit was not saved'));
	};

	const draftFor = (target: SessionRecord, which: NoteSection): Draft | undefined => {
		const key = noteKey(target, which), doc = docOf(target, which);
		if (!key || !doc) return undefined;
		let draft = drafts.current.get(key);
		if (!draft) {
			draft = {sessionId: target.id, section: which, noteId: which === 'shared' ? key : undefined, text: doc.text, cursor: doc.text.length, saved: doc.text, revision: doc.revision, scrollTop: 0};
			drafts.current.set(key, draft);
		} else reconcile(draft, doc);
		return draft;
	};

	const flush = (key: string): Promise<void> => {
		clearTimeout(timers.current.get(key));
		timers.current.delete(key);
		const draft = drafts.current.get(key);
		const {client: current} = latest.current;
		if (!draft || !current) return Promise.resolve();
		if (draft.inFlight !== undefined) return draft.pending ?? Promise.resolve();
		if (draft.text === draft.saved) return Promise.resolve();
		const text = draft.text;
		draft.inFlight = text;
		draft.pending = current.saveNote(draft.sessionId, draft.section, text, draft.revision, draft.noteId).then(result => {
			draft.inFlight = undefined;
			const doc = docOf(result.session, draft.section);
			if (result.saved && doc) { draft.saved = doc.text; draft.revision = doc.revision; }
			else if (doc) reconcile(draft, doc);
			rerender();
			// Typed while it was saving: save that too.
			if (draft.text !== draft.saved) return flush(key);
		}, error => {
			draft.inFlight = undefined;
			latest.current.setError(`Notes not saved: ${errorMessage(error)}`);
			rerender();
		});
		return draft.pending;
	};

	const schedule = (key: string) => {
		clearTimeout(timers.current.get(key));
		timers.current.set(key, setTimeout(() => void flush(key), SAVE_DEBOUNCE_MS));
	};

	// Leaving notes focus saves at once.
	useEffect(() => {
		if (focused) return;
		pasting.current = false;
		for (const key of drafts.current.keys()) void flush(key);
	}, [focused]);
	useEffect(() => () => { for (const timer of timers.current.values()) clearTimeout(timer); }, []);

	// Saved first, so the editor opens what is shown; the daemon creates the file if needed.
	const fileOf = (which: NoteSection): Promise<string> | undefined => {
		const {client: current} = latest.current;
		if (!current || !session) return undefined;
		const key = noteKey(session, which);
		const sessionId = session.id;
		return (key ? flush(key) : Promise.resolve()).then(() => current.openNote(sessionId, which));
	};

	const openSection = (which: NoteSection) => {
		void fileOf(which)?.then(file => {
			const label = openInEditor(file, message => setError(`${message}; the note is ${file}`));
			if (label) setStatusMessage(`Opened ${file} in ${label}; edits there show up here`);
		}, error => setError(errorMessage(error)));
	};

	// Ctrl+P: the cursor line's open checklist item becomes a task of the repository; the line becomes a ↗ link to it.
	// Saved first, so the daemon finds the line as it is shown.
	const promote = (which: NoteSection) => {
		const {client: current} = latest.current;
		if (!current || !session) return;
		const key = noteKey(session, which);
		const draft = key ? draftFor(session, which) : undefined;
		if (!key || !draft) return;
		const line = draft.text.slice(0, draft.cursor).split('\n').length - 1;
		const sessionId = session.id;
		void flush(key).then(() => {
			if (draft.text !== draft.saved) throw new Error('These notes are not saved yet; try again');
			const sent = draft.text;
			return current.promoteNoteItem(sessionId, which, line, draft.revision, draft.noteId).then(result => ({result, sent}));
		}).then(({result, sent}) => {
			const doc = docOf(result.session, which);
			if (doc && draft.text === sent) { draft.text = doc.text; draft.saved = doc.text; draft.revision = doc.revision; draft.cursor = Math.min(draft.cursor, doc.text.length); }
			else if (doc) reconcile(draft, doc);
			onTasks?.(result.tasks);
			setStatusMessage('Sent to Tasks (b shows it); the note keeps a ↗ link');
			rerender();
		}, error => setError(errorMessage(error)));
	};

	const edit = (draft: Draft, key: string, next: EditorState, target: SessionRecord, which: NoteSection) => {
		if (next.message) { setError(next.message); return; }
		if (next.text === draft.text && next.cursor === draft.cursor) return;
		const changed = next.text !== draft.text;
		draft.text = next.text; draft.cursor = next.cursor;
		if (changed) { draft.sessionId = target.id; draft.section = which; schedule(key); }
		rerender();
	};

	const handleInput = (input: string, key: Partial<Key>) => {
		if (!session) { onExit(); return; }
		const which = activeSection;
		const draft = draftFor(session, which), draftKey = noteKey(session, which);
		const doc = docOf(session, which);
		// Bracketed paste (enabled while editing): everything up to the end marker is text, Tab and Enter included.
		if (input === '[200~') { pasting.current = true; return; }
		if (input === '[201~') { pasting.current = false; return; }
		const options: EditOptions = {selectAll: false, maxChars: MAX_NOTES_CHARS, limitMessage: `Notes are limited to ${MAX_NOTES_LABEL}; that edit was not applied.`, tab: false, wrapWidth: viewport.current.width, pageRows: Math.max(1, viewport.current.rows - 1)};
		const editable = draft && draftKey && !doc?.readOnly && !doc?.tooLarge;
		if (pasting.current) {
			if (!editable) return;
			const text = key.return ? '\n' : key.tab ? '\t' : cleanInsertedText(input);
			if (text) edit(draft, draftKey, replaceRange(draft, draft.cursor, draft.cursor, text, options), session, which);
			return;
		}
		if (key.escape) { onExit(); return; }
		if (key.tab) {
			if (!ownNote) { setStatusMessage('In a worktree there is one note, shared by its sessions'); return; }
			if (!hasShared) { setStatusMessage('This session has no worktree note yet: its worktree is not ready'); return; }
			if (which === 'session' && !sharedEditable) { setStatusMessage('Its worktree was deleted: the worktree note is read-only'); return; }
			setSection(which === 'shared' ? 'session' : 'shared');
			return;
		}
		if (key.ctrl && input === 'o') { openSection(which); return; }
		if (!draft || !draftKey) return;
		if (key.ctrl && input === 'p') {
			if (!editable) { setError(doc?.tooLarge ? 'This note is too large to change here' : 'Its worktree was deleted: the worktree note is read-only'); return; }
			promote(which);
			return;
		}
		const state: EditorState = {text: draft.text, cursor: draft.cursor};
		const navigation = key.leftArrow || key.rightArrow || key.upArrow || key.downArrow || key.home || key.end || key.pageUp || key.pageDown;
		if (!editable && !navigation) {
			setError(doc?.tooLarge ? `This note is longer than ${MAX_NOTES_LABEL}: edit it in your editor (Ctrl+O)` : 'Its worktree was deleted: the worktree note is read-only');
			return;
		}
		let next: EditorState;
		if (key.ctrl && input === 'x') next = toggleChecklist(state);
		else if (key.ctrl && input === 't') next = insertChecklistItem(state);
		else if (key.return && !key.meta) next = continueChecklist(state) ?? editText(state, '', {return: true}, options);
		else next = editText(state, input, key, options);
		if (next.text.length > MAX_NOTES_CHARS && next.text.length > draft.text.length) next = {...state, message: options.limitMessage};
		edit(draft, draftKey, next, session, which);
	};

	const titleOf = (target: SessionRecord) => displaySessionTitle(target, sessions) || '(untitled)';

	const render = (width: number, height: number): React.ReactNode => {
		if (!session) return <NotesMessage text="Select a session from the sidebar." width={width} height={height} />;
		const editingSection = focused ? activeSection : undefined;
		const own = draftFor(session, 'session');
		const shared = hasShared ? draftFor(session, 'shared') : undefined;
		const sharedNote = session.sharedNotes;
		const sharedKey = noteKey(session, 'shared');
		const sharing = sharedKey ? sessions.filter(other => noteKey(other, 'shared') === sharedKey) : [];
		const count = Math.max(1, sharing.length);
		let sharedInput: NotesSectionInput | undefined;
		if (sharedNote && shared) {
			const branch = session.worktree?.branch || sharing.find(other => other.worktree?.branch && !other.worktree.isMain)?.worktree?.branch;
			const where = sharedNote.kind === 'repo' ? 'main checkout' : branch || (session.worktree?.path ? path.basename(session.worktree.path) : 'worktree');
			const what = sharedNote.kind === 'repo' ? 'No main checkout notes' : 'No worktree notes';
			sharedInput = {
				titles: sharedNote.kind === 'repo' ? [`Main checkout (shared by ${plural(count, 'session')})`, 'Main checkout'] : [`Worktree · ${where} (shared by ${plural(count, 'session')})`, `Worktree · ${where}`, 'Worktree'],
				text: shared.text,
				empty: sharedNote.readOnly ? `${what} (its worktree was deleted)` : !ownNote ? (focused ? `${what} · type to add` : `${what} · enter to add`) : focused ? `${what} · tab to add` : `${what} · enter, then tab to add`,
				flag: sharedNote.readOnly ? 'deleted, read-only' : sharedNote.tooLarge ? 'too large, E to edit' : undefined,
				editing: editingSection === 'shared' ? {cursor: shared.cursor, scrollTop: shared.scrollTop} : undefined,
				active: !focused && activeSection === 'shared',
			};
		}
		const sessionInput: NotesSectionInput | undefined = !ownNote ? undefined : {
			titles: [`This session · ${titleOf(session)}`, 'This session'],
			text: own?.text ?? '',
			empty: focused ? 'No notes for this session · tab to add' : 'No notes for this session · enter to add',
			flag: session.notesFile?.tooLarge ? 'too large, E to edit' : undefined,
			editing: editingSection === 'session' && own ? {cursor: own.cursor, scrollTop: own.scrollTop} : undefined,
			active: !focused && activeSection === 'session' && hasShared,
		};
		const layout = notesLayout({shared: sharedInput, session: sessionInput, width, height, focus: editingSection, links});
		const edited = editingSection === 'shared' ? shared : editingSection === 'session' ? own : undefined;
		if (edited && layout.scrollTop !== undefined) edited.scrollTop = layout.scrollTop;
		// The editor's wrap width and body rows, for Up/Down and PageUp/PageDown.
		viewport.current = {width: Math.max(1, width), rows: Math.max(1, editingSection ? layout.bodies[editingSection] : 1)};
		return <NotesRows rows={layout.rows} width={width} height={height} />;
	};

	// On an open checklist item, Ctrl+P (send it to Tasks) takes the word keys' place.
	const onOpenItem = () => {
		const draft = session ? draftFor(session, activeSection) : undefined;
		if (!draft) return false;
		const line = draft.text.slice(lineStart(draft.text, draft.cursor), lineEnd(draft.text, draft.cursor));
		const item = parseChecklistLine(line);
		return Boolean(item && !item.checked && item.text.trim());
	};
	const hint = (width: number): string => fitHint([
		'notes edit', 'esc done',
		...hasShared && ownNote ? [{text: activeSection === 'shared' ? 'tab this session' : session?.sharedNotes?.kind === 'repo' ? 'tab main checkout notes' : 'tab worktree notes', short: 'tab switch', drop: 1}] : [],
		{text: 'ctrl+x toggle ☐', short: 'ctrl+x ☐', drop: 1},
		{text: 'ctrl+t new item', drop: 2},
		{text: 'ctrl+o open in editor', short: 'ctrl+o editor', drop: 2},
		onOpenItem() ? {text: 'ctrl+p → tasks', drop: 2} : {text: 'alt+←/→ words', drop: 3},
	], width, ' • ');

	return {
		focus: () => { if (section === 'shared' && !sharedEditable) setSection('session'); },
		handleInput,
		openActiveInEditor: () => openSection(activeSection),
		activeFile: () => {
			// A deleted worktree's note is read-only: edit the session's instead.
			const which = activeSection === 'shared' && !sharedEditable && ownNote ? 'session' : activeSection;
			return fileOf(which)?.catch(error => { setError(errorMessage(error)); return undefined; }) ?? Promise.resolve(undefined);
		},
		render,
		hint,
	};
}
