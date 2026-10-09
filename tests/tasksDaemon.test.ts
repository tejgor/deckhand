import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import {readFileSync} from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import net from 'node:net';
import {once} from 'node:events';
import {spawn} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import {attachJsonParser, request, writeMessage} from '../src/client.js';
import {getSocketPath} from '../src/paths.js';
import {parseTasks} from '../src/tasks.js';
import type {BranchList, ClientRequest, NoteSaveResult, ServerMessage, SessionRecord, TasksDoc} from '../src/types.js';
import {cli, git, repo, waitFor, withEnv, fakeAgent, stop} from './helpers.js';

/** The end of the daemon's log, printed if it dies (its crash handlers write the cause there, not to stderr). */
const daemonLogTail = (home: string) => { try { return readFileSync(path.join(home, 'daemon.log'), 'utf8').split('\n').slice(-40).join('\n'); } catch (error) { return String(error); } };

// The repository task list against a real daemon with fake agents: ops, editor edits, starting a session from a task
// (link, its text typed into the agent's input but not sent, base branch), merged/done ticking and reopening, abandoned work, notes sent to Tasks.
test('tasks: one list per repository, linked to the work started from it', {timeout: 150000}, async t => {
	const root = await repo();
	await git(root, 'branch', 'release');
	await fs.writeFile(path.join(root, 'main-only.txt'), 'main\n'); await git(root, 'add', '.'); await git(root, 'commit', '-m', 'main only');
	const home = await fs.mkdtemp(path.join(os.tmpdir(), 'deckhand-tasks-'));
	const bin = path.join(home, 'bin'); await fs.mkdir(bin);
	for (const provider of ['claude', 'pi', 'codex']) await fs.writeFile(path.join(bin, provider), fakeAgent, {mode: 0o755});
	withEnv(t, {DECKHAND_HOME: home});
	const env = {...process.env, HOME: home, DECKHAND_HOME: home, DECKHAND_DEV: '0', DECKHAND_AGENT_LATEST: '{}', PATH: `${bin}${path.delimiter}${process.env.PATH}`, SHELL: '/bin/sh', TEST_CLI: cli};
	const daemon = spawn(process.execPath, [cli, '--daemon'], {cwd: root, env, stdio: ['ignore', 'ignore', 'pipe']});
	let daemonErr = ''; daemon.stderr?.on('data', chunk => { daemonErr += String(chunk); });
	daemon.on('exit', (code, signal) => { if (code !== 0 && signal !== 'SIGTERM') console.error(`DAEMON EXIT ${code} ${signal}\n${daemonErr}\n${daemonLogTail(home)}`); });
	t.after(async () => { await stop(daemon); await fs.rm(root, {recursive: true, force: true}); await fs.rm(home, {recursive: true, force: true}); });
	const call = <T>(message: Omit<Extract<ClientRequest, {requestId: string}>, 'requestId'>) => request<T>({...message, requestId: randomUUID()} as Extract<ClientRequest, {requestId: string}>);
	await waitFor(async () => { try { return await call<{ok: boolean}>({type: 'ping'}); } catch { return {ok: false}; } }, result => result.ok);
	const state = (id: string) => call<SessionRecord[]>({type: 'list'}).then(items => items.find(item => item.id === id)!);
	const create = (title: string, worktreeMode: 'none' | 'new', extra: Record<string, unknown> = {}) => call<SessionRecord>({type: 'create', input: {title, program: 'claude', cwd: root, repoRoot: root, cols: 80, rows: 24, worktreeMode, ...extra}} as any);
	const running = (id: string) => waitFor(() => state(id), item => item.status === 'running');
	const op = (value: Record<string, unknown>) => call<TasksDoc>({type: 'task-op', cwd: root, op: value} as any);
	const tasksOf = (doc: TasksDoc) => parseTasks(doc.text);
	const current = async () => tasksOf(await call<TasksDoc>({type: 'watch-tasks', cwd: root} as any));
	const byTitle = async (title: string) => (await current()).find(task => task.title === title)!;

	const socket = net.createConnection(getSocketPath()); await once(socket, 'connect');
	const events: ServerMessage[] = []; attachJsonParser(socket, message => void events.push(message));
	t.after(() => socket.destroy());
	const watchId = randomUUID();
	writeMessage(socket, {type: 'watch-tasks', requestId: watchId, cwd: root});
	await waitFor(async () => events.some(event => event.type === 'response' && event.requestId === watchId), Boolean);
	const pushed = (matches: (doc: TasksDoc) => boolean) => waitFor(async () => events.some(event => event.type === 'tasks-updated' && matches(event.tasks)), Boolean);

	let doc: TasksDoc;
	await t.test('ops change the file and reach watching clients; an editor\'s edit shows up; daemon-only ops are refused', async () => {
		doc = await op({type: 'add', title: 'First task', body: 'why it matters'});
		doc = await op({type: 'add', title: 'Second task'});
		assert.equal(doc.path, path.join(home, 'notes', 'tasks', `${doc.key}.md`));
		assert.deepEqual(tasksOf(doc).map(task => [task.title, task.body]), [['First task', 'why it matters'], ['Second task', '']]);
		await pushed(next => tasksOf(next).length === 2);
		await fs.appendFile(doc.path, '- [ ] From the editor\n');
		await pushed(next => tasksOf(next).some(task => task.title === 'From the editor'));
		// A task added in the editor has no ID yet; its provisional one still addresses it.
		const editorTask = await byTitle('From the editor');
		assert.match(editorTask.id, /^~/);
		doc = await op({type: 'toggle', id: editorTask.id});
		assert.equal(tasksOf(doc).find(task => task.title === 'From the editor')!.done, true);
		assert.ok(tasksOf(doc).every(task => /^[0-9a-f]{8}$/.test(task.id)));
		await assert.rejects(op({type: 'link', id: tasksOf(doc)[0]!.id, link: {s: 'x'}}), /Unknown task change/);
		await assert.rejects(op({type: 'toggle', id: 'gone'}), /no longer in the list/);
		// A linked worktree of the same repository shares the list.
		const other = path.join(home, 'other-worktree');
		await git(root, 'worktree', 'add', '-b', 'other', other);
		assert.equal((await call<TasksDoc>({type: 'watch-tasks', cwd: other} as any)).key, doc.key);
	});

	await t.test('branches to start from; a session started from a task links it, finds the task typed (not sent) in its agent\'s input and starts at the chosen base', async () => {
		const branches = await call<BranchList>({type: 'list-branches', cwd: root} as any);
		assert.equal(branches.current, 'main'); assert.equal(branches.defaultBranch, 'main'); assert.equal(branches.branchFrom, 'current');
		assert.ok(branches.branches.includes('release'));
		const first = await byTitle('First task');
		await assert.rejects(create('wrong base', 'none', {baseBranch: 'release'}), /only to a new worktree/);
		const session = await create('First task', 'new', {taskId: first.id, baseBranch: 'release'});
		await running(session.id);
		const started = await state(session.id);
		const trace = JSON.parse(await waitFor(() => fs.readFile(path.join(home, `trace-${session.id}.json`), 'utf8').catch(() => ''), Boolean)) as {args: string[]};
		assert.ok(!trace.args.includes('--'), 'nothing is passed as a first message');
		// Once the agent's screen settles (idle), the task arrives as one bracketed paste, without Enter.
		const typed = await waitFor(() => fs.readFile(path.join(home, `input-${session.id}`), 'utf8').catch(() => ''), Boolean, 20000);
		assert.equal(typed, '\x1b[200~First task: why it matters\x1b[201~');
		await waitFor(() => state(session.id), next => next.startPrompt === undefined);
		assert.equal(started.worktree?.baseRef, 'release');
		await assert.rejects(fs.access(path.join(started.cwd, 'main-only.txt')));
		assert.deepEqual((await byTitle('First task')).meta.wt, started.worktree?.id);
		await assert.rejects(create('again', 'new', {taskId: first.id}), /already being worked on/);
		await assert.rejects(create('done one', 'new', {taskId: (await byTitle('From the editor')).id}), /is done/);
	});

	await t.test('merged ticks the worktree\'s tasks, unmarking reopens them; D on the last session ticks, un-D reopens', async () => {
		const [session] = (await call<SessionRecord[]>({type: 'list'})).filter(item => item.title === 'First task');
		await call({type: 'mark-session-merged', sessionId: session!.id, targetCwd: root} as any);
		await waitFor(() => byTitle('First task'), task => task.done && task.meta.auto === 'merge');
		await call({type: 'mark-session-merged', sessionId: session!.id, targetCwd: root} as any);
		await waitFor(() => byTitle('First task'), task => !task.done && !task.meta.auto);
		// A second session in the same worktree: D ticks only once both are done.
		const child = await call<SessionRecord>({type: 'create', input: {title: 'helper', program: 'claude', cwd: session!.cwd, repoRoot: root, cols: 80, rows: 24, worktreeMode: 'none', parentSessionId: session!.id, subSessionKind: 'clean'}} as any);
		await running(child.id);
		await call({type: 'set-session-done', sessionId: session!.id, done: true} as any);
		await new Promise(resolve => setTimeout(resolve, 300));
		assert.equal((await byTitle('First task')).done, false);
		await call({type: 'set-session-done', sessionId: child.id, done: true} as any);
		await waitFor(() => byTitle('First task'), task => task.done && task.meta.auto === 'done');
		await call({type: 'set-session-done', sessionId: child.id, done: false} as any);
		await waitFor(() => byTitle('First task'), task => !task.done);
	});

	await t.test('work deleted unmerged goes back to the backlog, naming the branch it was tried in', async () => {
		const sessions = (await call<SessionRecord[]>({type: 'list'})).filter(item => item.title === 'First task' || item.title.endsWith('helper'));
		for (const item of sessions) { await call({type: 'kill', sessionId: item.id} as any); await waitFor(() => state(item.id), next => next.status === 'exited'); }
		const child = sessions.find(item => item.title.endsWith('helper'))!, parent = sessions.find(item => item.title === 'First task')!;
		await call({type: 'remove', sessionId: child.id} as any);
		assert.ok((await byTitle('First task')).meta.wt, 'still linked while a session refers to the worktree');
		await call({type: 'remove', sessionId: parent.id} as any);
		const task = await waitFor(() => byTitle('First task'), next => !next.meta.wt);
		assert.equal(task.meta.tried, 'first_task');
		// A main-checkout session's task: linked to the session, back to the backlog when it is removed.
		const second = await byTitle('Second task');
		const plain = await create('Second task', 'none', {taskId: second.id});
		await running(plain.id);
		// Typing into the agent before it settles wins: the draft is dropped, never mixed into your text.
		writeMessage(socket, {type: 'input', sessionId: plain.id, data: 'my own words'});
		await waitFor(() => state(plain.id), next => next.startPrompt === undefined, 20000);
		await new Promise(resolve => setTimeout(resolve, 6500));
		assert.equal(await fs.readFile(path.join(home, `input-${plain.id}`), 'utf8'), 'my own words');
		await waitFor(() => byTitle('Second task'), next => next.meta.s === plain.id);
		await call({type: 'kill', sessionId: plain.id} as any); await waitFor(() => state(plain.id), next => next.status === 'exited');
		await call({type: 'remove', sessionId: plain.id} as any);
		await waitFor(() => byTitle('Second task'), next => !next.meta.s);
	});

	await t.test('a note\'s checklist item goes to Tasks and leaves a link; removing a session can move its open items first', async () => {
		const session = await create('notes holder', 'none');
		await running(session.id);
		const fresh = await state(session.id);
		const saved = await call<NoteSaveResult>({type: 'save-note', sessionId: session.id, section: 'session', text: 'scratch\n- [ ] Promote me\n- [x] Ticked\n- [ ] Keep me', revision: fresh.notesFile!.revision} as any);
		const revision = saved.session.notesFile!.revision;
		await assert.rejects(call({type: 'promote-note-item', sessionId: session.id, section: 'session', line: 0, revision} as any), /checklist item/);
		await assert.rejects(call({type: 'promote-note-item', sessionId: session.id, section: 'session', line: 2, revision} as any), /already ticked/);
		await assert.rejects(call({type: 'promote-note-item', sessionId: session.id, section: 'session', line: 1, revision: 'stale'} as any), /changed meanwhile/);
		const promoted = await call<{session: SessionRecord; tasks: TasksDoc}>({type: 'promote-note-item', sessionId: session.id, section: 'session', line: 1, revision} as any);
		const task = tasksOf(promoted.tasks).find(item => item.title === 'Promote me')!;
		assert.equal(promoted.session.notes, `scratch\n- ↗ Promote me <!-- dh:t=${task.id} -->\n- [x] Ticked\n- [ ] Keep me`);
		await call({type: 'kill', sessionId: session.id} as any); await waitFor(() => state(session.id), next => next.status === 'exited');
		await call({type: 'remove', sessionId: session.id, moveOpenItems: true} as any);
		const moved = await byTitle('Keep me');
		assert.equal(moved.body, 'From the notes of notes holder (removed)');
		assert.equal((await current()).filter(item => item.title === 'Ticked').length, 0);
	});
});
