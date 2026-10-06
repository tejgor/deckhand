import fs from 'node:fs/promises';
import {watch, type FSWatcher} from 'node:fs';
import path from 'node:path';
import {createHash, randomUUID} from 'node:crypto';
import {getNotesDir} from './paths.js';
import {MAX_NOTES_CHARS, MAX_NOTES_LABEL, noteRevision} from './notes.js';

// The daemon's notes: one Markdown file per note under the state directory (the single source of truth), held in
// memory and projected into the session records it sends. Files are written atomically (temp file + rename), saves are
// revision-checked against the file as it is on disk, and the directories are watched so edits made in an editor show
// up in Deckhand (and are never overwritten by it).

export type NoteKind = 'session' | 'worktree' | 'repo';
/** A note: a session's own (`id` = session ID), a linked worktree incarnation's (record ID), the main checkout's (repo hash). */
export interface NoteId {kind: NoteKind; id: string}
export interface StoredNote {text: string; revision: string; tooLarge?: boolean}

const DIRECTORIES: Record<NoteKind, string> = {session: 'sessions', worktree: 'worktrees', repo: 'repos'};
const KINDS = Object.keys(DIRECTORIES) as NoteKind[];
// Bytes read from one file at most; a longer note is shown cut and only edited in an editor.
const READ_LIMIT = 1024 * 1024;
// Editors save in bursts (truncate + write, or temp file + rename): changes settle this long before a re-read.
const WATCH_DEBOUNCE_MS = 120;
const WATCH_RETRY_MS = 1000;
const EMPTY: StoredNote = {text: '', revision: noteRevision('')};

/** The file name (without `.md`) of a note: its ID when that is file-name-safe (UUIDs, hashes), else a hash of it. */
export function noteFileStem(id: string): string {
	return /^[A-Za-z0-9_-]{1,128}$/.test(id) ? id : `x-${createHash('sha256').update(id).digest('hex').slice(0, 32)}`;
}

export function noteFilePath(note: NoteId): string {
	return path.join(getNotesDir(), DIRECTORIES[note.kind], `${noteFileStem(note.id)}.md`);
}

const cacheKey = (kind: NoteKind, stem: string) => `${kind}:${stem}`;

async function readNoteFile(file: string): Promise<StoredNote> {
	let handle: fs.FileHandle;
	try { handle = await fs.open(file, 'r'); }
	catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') return EMPTY; throw error; }
	try {
		const {size} = await handle.stat();
		const buffer = Buffer.alloc(Math.min(size, READ_LIMIT));
		const {bytesRead} = await handle.read(buffer, 0, buffer.length, 0);
		const text = buffer.subarray(0, bytesRead).toString('utf8');
		const tooLarge = size > READ_LIMIT || text.length > MAX_NOTES_CHARS;
		return tooLarge ? {text: text.slice(0, MAX_NOTES_CHARS), revision: `${noteRevision(text)}-${size}`, tooLarge} : {text, revision: noteRevision(text)};
	} finally { await handle.close(); }
}

async function writeAtomically(file: string, text: string): Promise<void> {
	const temporary = path.join(path.dirname(file), `.${path.basename(file)}.tmp-${process.pid}-${randomUUID().slice(0, 8)}`);
	try {
		await fs.writeFile(temporary, text, {encoding: 'utf8', mode: 0o600});
		await fs.rename(temporary, file);
	} finally { await fs.rm(temporary, {force: true}); }
}

const sameNote = (left: StoredNote | undefined, right: StoredNote) => (left ?? EMPTY).revision === right.revision && Boolean(left?.tooLarge) === Boolean(right.tooLarge);

export class NotesStore {
	private readonly cache = new Map<string, StoredNote>();
	/** Per note: saves and re-reads run one at a time, so a watcher re-read never interleaves with a write. */
	private readonly queues = new Map<string, Promise<unknown>>();
	private readonly timers = new Map<string, NodeJS.Timeout>();
	private readonly watchers = new Map<NoteKind, FSWatcher>();
	private closed = false;

	/** `onChange` hears about notes whose file changed outside a save (an editor, a deletion). */
	constructor(private readonly onChange: (kind: NoteKind, stem: string) => void) {}

	async ensureDirectories(): Promise<void> {
		for (const kind of KINDS) await fs.mkdir(path.join(getNotesDir(), DIRECTORIES[kind]), {recursive: true, mode: 0o700});
	}

	/** Reads every note file into memory. */
	async load(): Promise<void> {
		await this.ensureDirectories();
		for (const kind of KINDS) {
			const directory = path.join(getNotesDir(), DIRECTORIES[kind]);
			for (const name of await fs.readdir(directory).catch(() => [] as string[])) {
				const stem = noteStem(name);
				if (!stem) continue;
				try { this.remember(kind, stem, await readNoteFile(path.join(directory, name))); } catch {}
			}
		}
	}

	/**
	 * Moves notes stored in state.json by older versions into their files. A file that already exists wins (a previous
	 * migration wrote it, or it was edited since), so running it again changes nothing. Returns whether any session
	 * still carried notes, i.e. whether state.json must be written again without them.
	 */
	async migrate(sessions: Array<{id: string; notes?: unknown}>): Promise<boolean> {
		await this.ensureDirectories();
		let legacy = false;
		for (const session of sessions) {
			if (session.notes === undefined) continue;
			legacy = true;
			if (typeof session.notes !== 'string' || !session.notes) continue;
			const file = noteFilePath({kind: 'session', id: session.id});
			const exists = await fs.access(file).then(() => true, () => false);
			if (!exists) await writeAtomically(file, session.notes.slice(0, MAX_NOTES_CHARS));
		}
		return legacy;
	}

	get(note: NoteId): StoredNote {
		return this.cache.get(cacheKey(note.kind, noteFileStem(note.id))) ?? EMPTY;
	}

	/**
	 * Writes `text` (cut to MAX_NOTES_CHARS) when the file still has `revision`; otherwise nothing is written and the
	 * file as it is now is cached (`saved` false). `changed`: the cached note changed (its sessions must hear about it).
	 */
	save(note: NoteId, text: string, revision: string): Promise<{saved: boolean; changed: boolean}> {
		return this.serialized(note.kind, noteFileStem(note.id), async () => {
			const file = noteFilePath(note), stem = noteFileStem(note.id);
			const current = await readNoteFile(file);
			const changed = this.remember(note.kind, stem, current);
			if (current.tooLarge) throw new Error(`This note is longer than ${MAX_NOTES_LABEL}; edit it in your editor (E)`);
			if (current.revision !== revision) return {saved: false, changed};
			const next = text.slice(0, MAX_NOTES_CHARS);
			if (next === current.text) return {saved: true, changed};
			await fs.mkdir(path.dirname(file), {recursive: true, mode: 0o700});
			await writeAtomically(file, next);
			this.remember(note.kind, stem, {text: next, revision: noteRevision(next)});
			return {saved: true, changed: true};
		});
	}

	/** The note's file, created (empty) if missing, e.g. to open it in an editor. */
	async ensureFile(note: NoteId): Promise<string> {
		const file = noteFilePath(note);
		await fs.mkdir(path.dirname(file), {recursive: true, mode: 0o700});
		await fs.writeFile(file, '', {flag: 'wx', mode: 0o600}).catch(error => { if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error; });
		return file;
	}

	/** Deletes the note's file (its session was removed, or its worktree record dropped). */
	remove(note: NoteId): Promise<void> {
		const stem = noteFileStem(note.id);
		return this.serialized(note.kind, stem, async () => {
			this.cache.delete(cacheKey(note.kind, stem));
			await fs.rm(noteFilePath(note), {force: true});
		});
	}

	/** Watches the note directories; a changed file is re-read after a short debounce (editors that save via rename too). */
	watch(): void {
		for (const kind of KINDS) this.watchDirectory(kind);
	}

	close(): void {
		this.closed = true;
		for (const watcher of this.watchers.values()) watcher.close();
		this.watchers.clear();
		for (const timer of this.timers.values()) clearTimeout(timer);
		this.timers.clear();
	}

	private watchDirectory(kind: NoteKind): void {
		if (this.closed || this.watchers.has(kind)) return;
		const directory = path.join(getNotesDir(), DIRECTORIES[kind]);
		const retry = () => {
			this.watchers.get(kind)?.close();
			this.watchers.delete(kind);
			if (this.closed) return;
			setTimeout(() => void fs.mkdir(directory, {recursive: true, mode: 0o700}).then(() => this.watchDirectory(kind), () => {}), WATCH_RETRY_MS).unref();
		};
		try {
			const watcher = watch(directory, {persistent: false}, (_event, name) => {
				const stem = name ? noteStem(String(name)) : undefined;
				if (stem) this.schedule(kind, stem);
				else if (!name) void this.rescan(kind);
			});
			watcher.on('error', retry);
			this.watchers.set(kind, watcher);
		} catch { retry(); }
	}

	private schedule(kind: NoteKind, stem: string): void {
		const key = cacheKey(kind, stem);
		clearTimeout(this.timers.get(key));
		this.timers.set(key, setTimeout(() => {
			this.timers.delete(key);
			void this.refresh(kind, stem);
		}, WATCH_DEBOUNCE_MS));
	}

	/** Without a file name from the watcher: every file of the directory, and every cached note of it (deletions). */
	private async rescan(kind: NoteKind): Promise<void> {
		const names = await fs.readdir(path.join(getNotesDir(), DIRECTORIES[kind])).catch(() => [] as string[]);
		const stems = new Set(names.map(noteStem).filter((stem): stem is string => Boolean(stem)));
		for (const key of this.cache.keys()) if (key.startsWith(`${kind}:`)) stems.add(key.slice(kind.length + 1));
		for (const stem of stems) this.schedule(kind, stem);
	}

	private refresh(kind: NoteKind, stem: string): Promise<void> {
		return this.serialized(kind, stem, async () => {
			if (this.closed) return;
			const file = path.join(getNotesDir(), DIRECTORIES[kind], `${stem}.md`);
			let current: StoredNote;
			try { current = await readNoteFile(file); } catch { return; }
			if (this.remember(kind, stem, current)) this.onChange(kind, stem);
		}).catch(() => {});
	}

	/** Caches a note as read or written; true when that changed what the sessions show. */
	private remember(kind: NoteKind, stem: string, note: StoredNote): boolean {
		const key = cacheKey(kind, stem);
		const changed = !sameNote(this.cache.get(key), note);
		if (note.revision === EMPTY.revision && !note.tooLarge) this.cache.delete(key);
		else this.cache.set(key, note);
		return changed;
	}

	private serialized<T>(kind: NoteKind, stem: string, run: () => Promise<T>): Promise<T> {
		const key = cacheKey(kind, stem);
		const operation = (this.queues.get(key) ?? Promise.resolve()).catch(() => {}).then(run);
		const settled = operation.catch(() => {});
		this.queues.set(key, settled);
		void settled.then(() => { if (this.queues.get(key) === settled) this.queues.delete(key); });
		return operation;
	}
}

/** The note stem of a directory entry: `<stem>.md`, not a hidden/temporary file. */
function noteStem(name: string): string | undefined {
	return !name.startsWith('.') && name.endsWith('.md') && name.length > 3 ? name.slice(0, -3) : undefined;
}
