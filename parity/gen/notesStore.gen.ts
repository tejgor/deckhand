/**
 * Fixtures for src/notesStore.ts. Regenerate with `node --import tsx parity/gen/notesStore.gen.ts`.
 *
 * Pure functions: trashFileName, noteFileStem, noteFilePath (recorded with DECKHAND_HOME=/parity-home).
 * NotesStore (load, migrate, save, modify, ensureFile, remove, get, entries) as directory cases: DECKHAND_HOME is the
 * temp root. File watching (watch/close, onChange) is timing based and is covered by ported tests, not recorded.
 */
import fs from 'node:fs/promises';
import path from 'node:path';
import {Recorder, arb, fc, installDeterminism, mutate, recordDirCase, registerFn, runTap, sample} from '../lib.ts';
import {NotesStore, noteFilePath, noteFileStem, trashFileName, type NoteId, type NoteKind} from '../../src/notesStore.ts';
import {noteRevision} from '../../src/notes.ts';

const MODULE = 'notesStore';
installDeterminism();
const recorder = new Recorder(MODULE);
const HOME_VAR = 'DECKHAND_HOME';
const environment = process['env'];
environment[HOME_VAR] = '/parity-home';

const KINDS: NoteKind[] = ['session', 'worktree', 'repo', 'tasks'];

// 1. Tap the existing tests.
recorder.addTap(await runTap({tests: ['tests/notes.test.ts'], tap: ['notesStore:trashFileName', 'notesStore:noteFileStem', 'notesStore:noteFilePath']}), {module: 'notesStore'});

// 2. trashFileName.
const labels: Array<string | undefined> = [undefined, '', '   ', 'school-doc-upload / prompt-audit', 'Fix: the BUG!!', '日本語のタイトル', '😀 party 😀', 'a'.repeat(100), '--x--', 'é ü ñ', 'ABC_def.ghi', '\n\t', 'x'.repeat(59) + '-yy'];
const stems = ['af7fc4e8', 'x-0123', 'w1', ''];
const dates = [new Date('2026-10-09T14:02:11.500Z'), new Date('2026-10-09T14:02:11Z'), new Date('1999-12-31T23:59:59.999Z'), new Date('2026-01-01T00:00:00.000Z'), new Date('0099-01-01T00:00:00Z'), new Date('+012345-06-07T08:09:10Z')];
for (const label of labels) for (const kind of KINDS) recorder.call('trashFileName', trashFileName, [kind, label, 'af7fc4e8', dates[0]]);
for (const [index, date] of dates.entries()) recorder.call('trashFileName', trashFileName, ['repo', labels[index], stems[index % stems.length], date]);
for (const stem of stems) recorder.call('trashFileName', trashFileName, ['tasks', 'label', stem]); // default `now`: the fixed clock
recorder.call('trashFileName', trashFileName, ['session', undefined, 'abc']);
for (const args of mutate(recorder.argsOf('trashFileName'), {variants: 3})) recorder.call('trashFileName', trashFileName, args);
for (const [label, stem] of sample(fc.tuple(fc.option(arb.text(40), {nil: undefined}), arb.text(12)), 120, 11)) recorder.call('trashFileName', trashFileName, ['worktree', label, stem, dates[1]]);

// 3. noteFileStem: safe IDs pass through, everything else becomes x-<sha256/32>.
const ids = ['', 'a', 'af7fc4e8-1c2d-4e5f-8a9b-0c1d2e3f4a5b', 'A_b-C', 'with space', 'dot.md', 'a/b', '../etc', 'é', '日本語', '😀', 'x'.repeat(128), 'x'.repeat(129), 'a\n', '\n', 'a\u0000b', '\ud800', '-', '_', 'repo hash'];
for (const id of ids) recorder.call('noteFileStem', noteFileStem, [id]);
for (const args of mutate(recorder.argsOf('noteFileStem'), {variants: 4})) recorder.call('noteFileStem', noteFileStem, args);
for (const id of sample(arb.text(30), 150, 21)) recorder.call('noteFileStem', noteFileStem, [id]);
for (const id of sample(fc.stringMatching(/^[A-Za-z0-9_-]{0,130}$/), 80, 22)) recorder.call('noteFileStem', noteFileStem, [id]);

// 4. noteFilePath (reads DECKHAND_HOME: recorded with the fixed value /parity-home).
for (const kind of KINDS) for (const id of ids) recorder.call('noteFilePath', noteFilePath, [{kind, id}]);
for (const id of sample(arb.text(20), 60, 31)) recorder.call('noteFilePath', noteFilePath, [{kind: KINDS[id.length % 4], id}]);

await recorder.write({prune: true});
delete environment[HOME_VAR];

// 5. NotesStore as directory cases. DECKHAND_HOME is the case root while the operation runs.
const SESSION = '11111111-2222-4333-8444-555555555555';
const note = (kind: NoteKind, id: string): NoteId => ({kind, id});
const dir = (root: string, kind: NoteKind) => path.join(root, 'notes', {session: 'sessions', worktree: 'worktrees', repo: 'repos', tasks: 'tasks'}[kind]);
const put = async (root: string, kind: NoteKind, stem: string, text: string | Buffer) => {
	await fs.mkdir(dir(root, kind), {recursive: true, mode: 0o700});
	await fs.writeFile(path.join(dir(root, kind), `${stem}.md`), text);
};
const withHome = async <T>(root: string, run: () => Promise<T>): Promise<T> => {
	const previous = environment[HOME_VAR];
	environment[HOME_VAR] = root;
	try { return await run(); } finally { if (previous === undefined) delete environment[HOME_VAR]; else environment[HOME_VAR] = previous; }
};
const bigText = 'abcdefghij'.repeat(5001); // 50 010 characters
const noop = () => {};

// A save cut inside a surrogate pair caches a lone surrogate, which a Rust string cannot hold (it keeps U+FFFD, the
// character the file and the revision hash get in both): the cached text is recorded well-formed. Identity for every
// other case.
const wellFormed = (note: ReturnType<NotesStore['get']>) => ({...note, text: note.text.toWellFormed()});
async function save(root: string, id: NoteId, text: string, revision: string) {
	return withHome(root, async () => {
		const store = new NotesStore(noop);
		await store.load();
		const result = await store.save(id, text, revision);
		return {...result, after: wellFormed(store.get(id))};
	});
}
async function saveCold(root: string, id: NoteId, text: string, revision: string) {
	return withHome(root, async () => {
		const store = new NotesStore(noop);
		const result = await store.save(id, text, revision);
		return {...result, after: wellFormed(store.get(id))};
	});
}
const append = registerFn('append-line', (text: string) => ({text: text + 'more\n', result: text.length}));
const identity = registerFn('identity', (text: string) => ({text, result: 'same'}));
const huge = registerFn('make-huge', (text: string) => ({text: text + 'x'.repeat(50_000), result: null}));
const boom = registerFn('throw-boom', (_text: string): {text: string; result: unknown} => { throw new Error('boom'); });
const upper = registerFn('uppercase', (text: string) => ({text: text.toUpperCase(), result: [text.length, text.split('\n').length]}));
const clear = registerFn('clear', (_text: string) => ({text: '', result: 'cleared'}));
async function modify(root: string, id: NoteId, transform: (text: string) => {text: string; result: unknown}) {
	return withHome(root, async () => {
		const store = new NotesStore(noop);
		const outcome = await store.modify(id, transform);
		return {...outcome, get: store.get(id)};
	});
}
async function remove(root: string, id: NoteId, label?: string) {
	return withHome(root, async () => {
		const store = new NotesStore(noop);
		await store.load();
		await store.remove(id, label);
		return {get: store.get(id), entries: store.entries(id.kind)};
	});
}
async function migrate(root: string, sessions: Array<{id: string; notes?: unknown}>) {
	return withHome(root, async () => new NotesStore(noop).migrate(sessions));
}
async function ensureFile(root: string, id: NoteId) {
	return withHome(root, async () => new NotesStore(noop).ensureFile(id));
}
async function load(root: string) {
	return withHome(root, async () => {
		const store = new NotesStore(noop);
		await store.load();
		const out: Record<string, Array<[string, unknown]>> = {};
		for (const kind of KINDS) out[kind] = store.entries(kind).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
		return out;
	});
}
const none = async () => {};
const rev = noteRevision;

// save
const sid = note('session', SESSION);
await recordDirCase(MODULE, 'save-new-file', {setup: none, fn: 'NotesStore.save', args: root => [root, sid, 'hello\n', rev('')], run: save});
await recordDirCase(MODULE, 'save-new-file-wrong-revision', {setup: none, fn: 'NotesStore.save', args: root => [root, sid, 'hello\n', 'deadbeef'], run: save});
await recordDirCase(MODULE, 'save-matching-revision', {setup: root => put(root, 'session', SESSION, 'old\n'), fn: 'NotesStore.save', args: root => [root, sid, 'new\n', rev('old\n')], run: save});
await recordDirCase(MODULE, 'save-stale-revision', {setup: root => put(root, 'session', SESSION, 'disk\n'), fn: 'NotesStore.save', args: root => [root, sid, 'mine\n', rev('older\n')], run: save});
await recordDirCase(MODULE, 'save-same-text', {setup: root => put(root, 'session', SESSION, 'same\n'), fn: 'NotesStore.save', args: root => [root, sid, 'same\n', rev('same\n')], run: save});
await recordDirCase(MODULE, 'save-same-text-cold-cache', {setup: root => put(root, 'session', SESSION, 'same\n'), fn: 'NotesStore.save', args: root => [root, sid, 'same\n', rev('same\n')], run: saveCold});
await recordDirCase(MODULE, 'save-to-empty', {setup: root => put(root, 'repo', 'abc123', 'something'), fn: 'NotesStore.save', args: root => [root, note('repo', 'abc123'), '', rev('something')], run: save});
await recordDirCase(MODULE, 'save-truncates-long-text', {setup: none, fn: 'NotesStore.save', args: root => [root, note('worktree', 'wt-1'), bigText, rev('')], run: save});
await recordDirCase(MODULE, 'save-truncates-surrogate-boundary', {setup: none, fn: 'NotesStore.save', args: root => [root, note('tasks', 'r1'), 'a'.repeat(49_999) + '😀tail', rev('')], run: save});
await recordDirCase(MODULE, 'save-existing-too-large-throws', {setup: root => put(root, 'session', SESSION, bigText), fn: 'NotesStore.save', args: root => [root, sid, 'x', 'any'], run: save});
await recordDirCase(MODULE, 'save-unicode-and-hashed-id', {setup: none, fn: 'NotesStore.save', args: root => [root, note('session', 'séance 日本語/😀'), 'こんにちは 😀\r\nline2\n', rev('')], run: save});
await recordDirCase(MODULE, 'save-hashed-id-update', {setup: root => put(root, 'repo', noteFileStem('/Users/me/repo'), 'v1'), fn: 'NotesStore.save', args: root => [root, note('repo', '/Users/me/repo'), 'v2', rev('v1')], run: save});
await recordDirCase(MODULE, 'save-invalid-utf8-on-disk', {setup: root => put(root, 'session', SESSION, Buffer.from([0x61, 0xff, 0xfe, 0x62])), fn: 'NotesStore.save', args: root => [root, sid, 'fixed', rev('a��b')], run: save});
await recordDirCase(MODULE, 'save-exactly-max-chars', {setup: none, fn: 'NotesStore.save', args: root => [root, sid, 'é'.repeat(50_000), rev('')], run: save});

// modify
await recordDirCase(MODULE, 'modify-append-new-note', {setup: none, fn: 'NotesStore.modify', args: root => [root, note('tasks', 'repo1'), append], run: modify});
await recordDirCase(MODULE, 'modify-append-existing', {setup: root => put(root, 'tasks', 'repo1', '- [ ] a\n'), fn: 'NotesStore.modify', args: root => [root, note('tasks', 'repo1'), append], run: modify});
await recordDirCase(MODULE, 'modify-identity-writes-nothing', {setup: root => put(root, 'tasks', 'repo1', 'keep\n'), fn: 'NotesStore.modify', args: root => [root, note('tasks', 'repo1'), identity], run: modify});
await recordDirCase(MODULE, 'modify-identity-missing-file', {setup: none, fn: 'NotesStore.modify', args: root => [root, note('tasks', 'repo1'), identity], run: modify});
await recordDirCase(MODULE, 'modify-too-long-result-throws', {setup: root => put(root, 'tasks', 'repo1', 'x\n'), fn: 'NotesStore.modify', args: root => [root, note('tasks', 'repo1'), huge], run: modify});
await recordDirCase(MODULE, 'modify-existing-too-large-throws', {setup: root => put(root, 'tasks', 'repo1', bigText), fn: 'NotesStore.modify', args: root => [root, note('tasks', 'repo1'), append], run: modify});
await recordDirCase(MODULE, 'modify-transform-throws', {setup: root => put(root, 'tasks', 'repo1', 'a\n'), fn: 'NotesStore.modify', args: root => [root, note('tasks', 'repo1'), boom], run: modify});
await recordDirCase(MODULE, 'modify-unicode', {setup: root => put(root, 'worktree', 'w', 'é中😀\nline\n'), fn: 'NotesStore.modify', args: root => [root, note('worktree', 'w'), upper], run: modify});
await recordDirCase(MODULE, 'modify-to-empty', {setup: root => put(root, 'worktree', 'w', 'gone\n'), fn: 'NotesStore.modify', args: root => [root, note('worktree', 'w'), clear], run: modify});

// remove
await recordDirCase(MODULE, 'remove-with-text-to-trash', {setup: root => put(root, 'session', SESSION, 'keep me\n'), fn: 'NotesStore.remove', args: root => [root, sid, 'School doc upload / prompt-audit'], run: remove});
await recordDirCase(MODULE, 'remove-without-label', {setup: root => put(root, 'worktree', 'wt-9', 'text'), fn: 'NotesStore.remove', args: root => [root, note('worktree', 'wt-9')], run: remove});
await recordDirCase(MODULE, 'remove-unicode-label', {setup: root => put(root, 'repo', 'h1', '日本語'), fn: 'NotesStore.remove', args: root => [root, note('repo', 'h1'), '日本語のタイトル 😀'], run: remove});
await recordDirCase(MODULE, 'remove-empty-deletes', {setup: root => put(root, 'session', SESSION, ''), fn: 'NotesStore.remove', args: root => [root, sid, 'label'], run: remove});
await recordDirCase(MODULE, 'remove-whitespace-only-deletes', {setup: root => put(root, 'session', SESSION, ' \n\t\n'), fn: 'NotesStore.remove', args: root => [root, sid, 'label'], run: remove});
await recordDirCase(MODULE, 'remove-missing-file', {setup: none, fn: 'NotesStore.remove', args: root => [root, sid, 'label'], run: remove});
await recordDirCase(MODULE, 'remove-hashed-id', {setup: root => put(root, 'repo', noteFileStem('a/b c'), 'x'), fn: 'NotesStore.remove', args: root => [root, note('repo', 'a/b c'), 'br'], run: remove});
await recordDirCase(MODULE, 'remove-leaves-other-notes', {setup: async root => { await put(root, 'session', 's1', 'one'); await put(root, 'session', 's2', 'two'); await put(root, 'worktree', 's1', 'three'); }, fn: 'NotesStore.remove', args: root => [root, note('session', 's1'), 'same'], run: remove});

// migrate
await recordDirCase(MODULE, 'migrate-writes-legacy-notes', {setup: none, fn: 'NotesStore.migrate', args: root => [root, [{id: 'a1', notes: 'hello'}, {id: 'b2', notes: '日本語\n'}, {id: 'c3'}]], run: migrate});
await recordDirCase(MODULE, 'migrate-existing-file-wins', {setup: root => put(root, 'session', 'a1', 'edited'), fn: 'NotesStore.migrate', args: root => [root, [{id: 'a1', notes: 'legacy'}]], run: migrate});
await recordDirCase(MODULE, 'migrate-no-notes-property', {setup: none, fn: 'NotesStore.migrate', args: root => [root, [{id: 'a1'}, {id: 'b2'}]], run: migrate});
await recordDirCase(MODULE, 'migrate-empty-and-nonstring', {setup: none, fn: 'NotesStore.migrate', args: root => [root, [{id: 'a1', notes: ''}, {id: 'b2', notes: 5}, {id: 'c3', notes: null}, {id: 'd4', notes: ['x']}]], run: migrate});
await recordDirCase(MODULE, 'migrate-truncates', {setup: none, fn: 'NotesStore.migrate', args: root => [root, [{id: 'a1', notes: bigText}, {id: 'weird id/1', notes: 'z'}]], run: migrate});
await recordDirCase(MODULE, 'migrate-empty-list', {setup: none, fn: 'NotesStore.migrate', args: root => [root, []], run: migrate});

// ensureFile
await recordDirCase(MODULE, 'ensure-file-creates-empty', {setup: none, fn: 'NotesStore.ensureFile', args: root => [root, sid], run: ensureFile});
await recordDirCase(MODULE, 'ensure-file-keeps-existing', {setup: root => put(root, 'session', SESSION, 'content\n'), fn: 'NotesStore.ensureFile', args: root => [root, sid], run: ensureFile});
await recordDirCase(MODULE, 'ensure-file-hashed-id', {setup: none, fn: 'NotesStore.ensureFile', args: root => [root, note('tasks', 'x y')], run: ensureFile});

// load + get + entries
await recordDirCase(MODULE, 'load-empty-home', {setup: none, fn: 'NotesStore.load', args: root => [root], run: load});
await recordDirCase(MODULE, 'load-mixed-files', {
	setup: async root => {
		await put(root, 'session', 'a', 'alpha\n');
		await put(root, 'session', 'empty', '');
		await put(root, 'session', '.hidden', 'x');
		await fs.writeFile(path.join(dir(root, 'session'), '.a.md.tmp-1-abcd'), 'x');
		await fs.writeFile(path.join(dir(root, 'session'), 'notes.txt'), 'not md');
		await fs.writeFile(path.join(dir(root, 'session'), '.md'), 'bare');
		await put(root, 'worktree', 'w-plain', 'unicode stem');
		await put(root, 'repo', 'big', bigText);
		await put(root, 'repo', 'bad-utf8', Buffer.from([0xe3, 0x81, 0x61]));
		await put(root, 'tasks', 'r', '- [ ] one\n- [x] two\n');
		await fs.mkdir(path.join(dir(root, 'tasks'), 'subdir.md'));
		await fs.mkdir(path.join(root, 'notes', 'trash'), {recursive: true});
		await fs.writeFile(path.join(root, 'notes', 'trash', '2026-01-01T00-00-00_session_x_a.md'), 'trashed');
	},
	fn: 'NotesStore.load', args: root => [root], run: load,
});
await recordDirCase(MODULE, 'load-legacy-home-v45', {
	setup: async root => { await fs.cp(path.resolve(import.meta.dirname, '../../fixtures/state/home-v45/notes'), path.join(root, 'notes'), {recursive: true}); },
	fn: 'NotesStore.load', args: root => [root], run: load,
});
