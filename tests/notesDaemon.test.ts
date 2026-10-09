import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import net from 'node:net';
import {once} from 'node:events';
import {spawn} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import {attachJsonParser, request, writeMessage} from '../src/client.js';
import {getSocketPath} from '../src/paths.js';
import type {ClientRequest, NoteSaveResult, ServerMessage, SessionRecord} from '../src/types.js';
import {cli, repo, waitFor, withEnv, fakeAgent, stop} from './helpers.js';
import {loadState, saveState} from '../src/storage.js';

// Notes against a real daemon with fake agents (its own daemon and state directory, so it runs beside the others).
test('notes are Markdown files: legacy migration, revision-checked saves, editor edits picked up, shared worktree and repository notes, lifetimes', {timeout: 150000}, async t => {
	const root = await repo();
	const home = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-notes-'));
	const bin = path.join(home, 'bin'); await fs.mkdir(bin);
	for (const provider of ['claude', 'pi', 'codex']) await fs.writeFile(path.join(bin, provider), fakeAgent, {mode: 0o755});
	withEnv(t, {DECKHAND_HOME: home});
	const env = {...process.env, HOME: home, DECKHAND_HOME: home, DECKHAND_DEV: '0', DECKHAND_AGENT_LATEST: '{}', PATH: `${bin}${path.delimiter}${process.env.PATH}`, SHELL: '/bin/sh', TEST_CLI: cli};
	const launch = () => { const child = spawn(process.execPath, [cli, '--daemon'], {cwd: root, env, stdio: ['ignore', 'ignore', 'pipe']}); child.stderr?.on('data', () => {}); return child; };
	const call = <T>(message: Omit<Extract<ClientRequest, {requestId: string}>, 'requestId'>) => request<T>({...message, requestId: randomUUID()} as Extract<ClientRequest, {requestId: string}>);
	const ready = () => waitFor(async () => { try { return await call<{ok: boolean}>({type: 'ping'}); } catch { return {ok: false}; } }, result => result.ok);
	const state = (id: string) => call<SessionRecord[]>({type: 'list'}).then(items => items.find(item => item.id === id)!);
	const create = (title: string, worktreeMode: 'none' | 'new' = 'none', extra: Record<string, unknown> = {}) => call<SessionRecord>({type: 'create', input: {title, program: 'claude', cwd: root, repoRoot: root, cols: 80, rows: 24, worktreeMode, ...extra}} as any);
	const running = (id: string) => waitFor(() => state(id), item => item.status === 'running');
	const killAndWait = async (id: string) => { await call({type: 'kill', sessionId: id} as any); await waitFor(() => state(id), item => item.status === 'exited'); };
	const save = async (id: string, section: 'session' | 'shared', text: string, revision?: string) => {
		const current = await state(id);
		const base = revision ?? (section === 'session' ? current.notesFile!.revision : current.sharedNotes!.revision);
		return call<NoteSaveResult>({type: 'save-note', sessionId: id, section, noteId: section === 'shared' ? `${current.sharedNotes!.kind}:${current.sharedNotes!.id}` : undefined, text, revision: base} as any);
	};
	const read = (file: string) => fs.readFile(file, 'utf8').catch(() => undefined);
	const exists = (file: string) => fs.access(file).then(() => true, () => false);

	// State written by an older version: the note lives in state.json.
	const legacyId = randomUUID(), at = new Date().toISOString();
	await saveState({sessions: [{id: legacyId, title: 'legacy', program: 'claude', command: 'claude', cwd: root, repoRoot: root, launchWorktreeRoot: root, worktree: {mode: 'none'}, status: 'exited', createdAt: at, updatedAt: at, notes: 'legacy note\n- [ ] carry over'} as SessionRecord], worktrees: []});
	let daemon = launch();
	t.after(async () => { await stop(daemon); await fs.rm(root, {recursive: true, force: true}); await fs.rm(home, {recursive: true, force: true}); });
	await ready();

	await t.test('legacy notes move into their file once, and state.json stops carrying notes', async () => {
		const legacy = await state(legacyId);
		const file = path.join(home, 'notes', 'sessions', `${legacyId}.md`);
		assert.equal(legacy.notes, 'legacy note\n- [ ] carry over');
		assert.equal(legacy.notesFile?.path, file);
		assert.equal(await read(file), 'legacy note\n- [ ] carry over');
		assert.equal((await loadState()).sessions[0]!.notes, undefined);
		assert.doesNotMatch(await fs.readFile(path.join(home, 'state.json'), 'utf8'), /legacy note|notesFile|sharedNotes/);
		// Idempotent: a state.json that still has (older) notes never overwrites the file, which wins.
		await fs.writeFile(file, 'edited since');
		await stop(daemon);
		const stored = await loadState();
		await saveState({...stored, sessions: stored.sessions.map(session => ({...session, notes: 'stale copy'}))});
		daemon = launch(); await ready();
		assert.equal((await state(legacyId)).notes, 'edited since');
		assert.equal(await read(file), 'edited since');
		assert.equal((await loadState()).sessions[0]!.notes, undefined);
	});

	const socket = net.createConnection(getSocketPath()); await once(socket, 'connect');
	const events: ServerMessage[] = []; attachJsonParser(socket, message => void events.push(message));
	t.after(() => socket.destroy());
	const subscribeId = randomUUID();
	writeMessage(socket, {type: 'subscribe', requestId: subscribeId, repoRoot: root});
	await waitFor(async () => events.some(event => event.type === 'response' && event.requestId === subscribeId), Boolean);
	const updated = (id: string, matches: (session: SessionRecord) => boolean) => waitFor(async () => events.some(event => event.type === 'session-updated' && event.session.id === id && matches(event.session)), Boolean);

	await t.test('saves are revision-checked and broadcast; an editor\'s edit (in place or by rename) shows up; a stale save is refused', async () => {
		const file = (await state(legacyId)).notesFile!.path;
		events.length = 0;
		const saved = await save(legacyId, 'session', '# Plan\n- [ ] one');
		assert.equal(saved.saved, true); assert.equal(saved.session.notes, '# Plan\n- [ ] one');
		assert.equal(await read(file), '# Plan\n- [ ] one');
		await updated(legacyId, session => session.notes === '# Plan\n- [ ] one');
		const base = (await state(legacyId)).notesFile!.revision;
		// An editor writing in place, then one saving through a temporary file and a rename.
		await fs.writeFile(file, 'edited in VS Code');
		await updated(legacyId, session => session.notes === 'edited in VS Code');
		const temporary = path.join(path.dirname(file), 'vim-swap.tmp');
		await fs.writeFile(temporary, 'saved via rename'); await fs.rename(temporary, file);
		await updated(legacyId, session => session.notes === 'saved via rename');
		// Deckhand's draft was based on the old revision: refused, nothing written, the file as it is comes back.
		const stale = await save(legacyId, 'session', 'clobber', base);
		assert.equal(stale.saved, false); assert.equal(stale.session.notes, 'saved via rename');
		assert.equal(await read(file), 'saved via rename');
		// Text is capped; a file made longer elsewhere is shown cut and only editable in an editor.
		assert.equal((await save(legacyId, 'session', 'x'.repeat(50_010))).session.notes?.length, 50_000);
		await fs.writeFile(file, 'y'.repeat(60_000));
		await updated(legacyId, session => Boolean(session.notesFile?.tooLarge));
		await assert.rejects(save(legacyId, 'session', 'short'), /longer than/);
		assert.equal((await read(file))?.length, 60_000);
		await fs.writeFile(file, 'back to normal');
		await updated(legacyId, session => session.notes === 'back to normal' && !session.notesFile?.tooLarge);
	});

	let first: SessionRecord, attached: SessionRecord, child: SessionRecord;
	await t.test('every session of a worktree shares its note (attached and mode-none sub-sessions too); handoffs include it', async () => {
		first = await running((await create('notes-a', 'new')).id);
		attached = await running((await create('notes-b', 'existing', {existingWorktreePath: first.cwd})).id);
		child = await running((await call<SessionRecord>({type: 'create', input: {title: 'notes-child', program: 'pi', cwd: first.cwd, repoRoot: root, cols: 80, rows: 24, parentSessionId: first.id, subSessionKind: 'clean'}} as any)).id);
		const recordId = first.worktree!.id!;
		assert.deepEqual([first, attached, child].map(session => [session.sharedNotes?.kind, session.sharedNotes?.id]), [['worktree', recordId], ['worktree', recordId], ['worktree', recordId]]);
		events.length = 0;
		assert.equal((await save(attached.id, 'shared', 'shared plan\n- [ ] review')).saved, true);
		for (const id of [first.id, child.id]) await updated(id, session => session.sharedNotes?.text === 'shared plan\n- [ ] review');
		assert.equal(await read(path.join(home, 'notes', 'worktrees', `${recordId}.md`)), 'shared plan\n- [ ] review');
		// The UI names the note it edited: a save for another note is refused.
		await assert.rejects(call({type: 'save-note', sessionId: first.id, section: 'shared', noteId: 'worktree:other', text: 'x', revision: (await state(first.id)).sharedNotes!.revision} as any), /another note/);
		// A session in a worktree has one note, the worktree's: it has no note of its own to write.
		await assert.rejects(save(child.id, 'session', 'child only'), /worktree’s note/);
		const handoff = await fs.readFile(await call<string>({type: 'export-handoff', sessionId: child.id} as any), 'utf8');
		assert.match(handoff, /## Notes\n\n\(No notes recorded\.\)\n\n## Worktree notes\n\nshared plan\n- \[ \] review/);
		// open-note creates a missing file for the editor.
		const ownFile = await call<string>({type: 'open-note', sessionId: first.id, section: 'session'} as any);
		assert.equal(ownFile, path.join(home, 'notes', 'sessions', `${first.id}.md`)); assert.equal(await read(ownFile), '');
	});

	await t.test('main-checkout sessions share the repository note, which outlives them; a session note goes with its session', async () => {
		const one = await running((await create('main-1')).id), two = await running((await create('main-2')).id);
		assert.equal(one.sharedNotes?.kind, 'repo'); assert.equal(two.sharedNotes?.id, one.sharedNotes?.id);
		await save(one.id, 'shared', 'repo-wide');
		await updated(two.id, session => session.sharedNotes?.text === 'repo-wide');
		await save(one.id, 'session', 'mine');
		const ownFile = (await state(one.id)).notesFile!.path, repoFile = one.sharedNotes!.path;
		assert.equal(await read(ownFile), 'mine');
		for (const session of [one, two]) { await killAndWait(session.id); await call({type: 'remove', sessionId: session.id} as any); }
		assert.equal(await exists(ownFile), false);
		assert.equal(await read(repoFile), 'repo-wide');
		// It went to the notes trash, named after its session (an empty note is just deleted).
		const trash = path.join(home, 'notes', 'trash');
		const trashed = (await fs.readdir(trash)).filter(name => name.includes('_session_main-1_'));
		assert.equal(trashed.length, 1);
		assert.equal(await read(path.join(trash, trashed[0]!)), 'mine');
		assert.ok(!(await fs.readdir(trash)).some(name => name.includes('_session_main-2_')));
	});

	await t.test('a deleted worktree\'s note stays read-only; a new worktree at the path starts empty; the note goes with the record', async () => {
		await killAndWait(attached.id); await killAndWait(child.id);
		await call({type: 'kill', sessionId: first.id, deleteWorktree: true, allowDataLoss: true} as any);
		await waitFor(() => state(first.id), item => Boolean(item.worktree?.deletedAt));
		const old = await state(child.id);
		assert.equal(old.sharedNotes?.readOnly, true); assert.equal(old.sharedNotes?.text, 'shared plan\n- [ ] review');
		await assert.rejects(save(child.id, 'shared', 'nope'), /read-only/);
		const again = await running((await create('notes-a', 'new')).id);
		assert.equal(again.cwd, first.cwd); assert.notEqual(again.sharedNotes?.id, old.sharedNotes?.id);
		assert.equal(again.sharedNotes?.text, ''); assert.equal(again.sharedNotes?.readOnly, undefined);
		const oldFile = old.sharedNotes!.path;
		await call({type: 'remove', sessionId: first.id} as any); await call({type: 'remove', sessionId: attached.id} as any);
		assert.equal(await read(oldFile), 'shared plan\n- [ ] review');
		await call({type: 'remove', sessionId: child.id} as any);
		assert.equal(await exists(oldFile), false);
		assert.ok((await fs.readdir(path.join(home, 'notes', 'trash'))).some(name => name.includes('_worktree_')), 'the worktree note went to the trash');
		await killAndWait(again.id);
	});

	await t.test('at start, notes of sessions in a worktree are merged into its note (under their titles) and go to the trash', async () => {
		const owner = await running((await create('merge-a', 'new')).id);
		const helper = await running((await call<SessionRecord>({type: 'create', input: {title: 'merge-b', program: 'pi', cwd: owner.cwd, repoRoot: root, cols: 80, rows: 24, parentSessionId: owner.id, subSessionKind: 'clean'}} as any)).id);
		const lone = await running((await create('merge-lone', 'new')).id);
		await save(owner.id, 'shared', 'worktree plan');
		for (const session of [owner, helper, lone]) await killAndWait(session.id);
		// Notes written by an older version: each session's own.
		await stop(daemon);
		const own = (session: SessionRecord) => path.join(home, 'notes', 'sessions', `${session.id}.md`);
		await fs.writeFile(own(owner), '- [ ] from the owner\n');
		await fs.writeFile(own(helper), 'helper context');
		await fs.writeFile(own(lone), '- [ ] the only note');
		daemon = launch(); await ready();
		const merged = (await state(owner.id)).sharedNotes!.text;
		assert.match(merged, /^worktree plan\n\n## .*merge-a\n- \[ \] from the owner\n\n## .*merge-b\nhelper context$/);
		assert.equal((await state(helper.id)).sharedNotes!.text, merged);
		// A lone note goes into an empty worktree note as it is.
		assert.equal((await state(lone.id)).sharedNotes!.text, '- [ ] the only note');
		for (const session of [owner, helper, lone]) { assert.equal(await exists(own(session)), false); assert.equal((await state(session.id)).notes, ''); }
		assert.equal((await fs.readdir(path.join(home, 'notes', 'trash'))).filter(name => name.includes('merged-into-its-worktree-note')).length, 3);
	});
});
