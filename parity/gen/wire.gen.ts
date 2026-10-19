// Wire corpus generator for the Rust port (crates/protocol/tests/wire_corpus.rs).
//
//   node --import tsx parity/gen/wire.gen.ts            capture (build + full TS test run) and reduce
//   node --import tsx parity/gen/wire.gen.ts <logdir>   reduce an existing DECKHAND_WIRE_LOG directory only
//
// Capture runs `npm run build` and `node --import tsx --test tests/*.test.ts` with DECKHAND_WIRE_LOG, a temp HOME and a
// temp DECKHAND_HOME under the OS temp directory. The daemon, client and both worker kinds then append every message
// they send or receive to <logdir>/<role>-<pid>.jsonl as {dir, msg} (daemon IPC lines also carry `worker`).
//
// Reduce keeps, per (dir, worker kind, message type, request type of a response), one message per distinct shape (key
// order and value kinds, recursively), the smallest one, capped per type, and writes fixtures/wire/corpus.jsonl sorted
// so the same logs always give the same file. Each line is {dir, worker?, req?, msg}: `worker` ('session' |
// 'workspace') names the IPC peer, `req` the request type a response answers (when its request was seen).
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {spawnSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';

type Dir = 'c2d' | 'd2c' | 'd2w' | 'w2d';
interface Line {dir: Dir; worker?: 'session' | 'workspace'; req?: string; msg: Record<string, unknown>}

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const OUT = path.join(root, 'fixtures', 'wire', 'corpus.jsonl');
const MAX_BYTES = 5 * 1024 * 1024;
const PER_TYPE = 24;
const DIR_ORDER: Dir[] = ['c2d', 'd2c', 'd2w', 'w2d'];

function capture(): string {
	const base = fs.mkdtempSync(path.join(os.tmpdir(), 'deckhand-wire-'));
	const logDir = path.join(base, 'log'), home = path.join(base, 'home'), deckhandHome = path.join(base, 'deckhand-home');
	for (const dir of [logDir, home, deckhandHome]) fs.mkdirSync(dir, {recursive: true});
	const env = {...process.env, DECKHAND_WIRE_LOG: logDir, HOME: home, DECKHAND_HOME: deckhandHome};
	const build = spawnSync('npm', ['run', 'build'], {cwd: root, env, stdio: 'inherit'});
	if (build.status !== 0) throw new Error('npm run build failed');
	const tests = fs.readdirSync(path.join(root, 'tests')).filter(name => name.endsWith('.test.ts')).sort().map(name => path.join('tests', name));
	const run = spawnSync(process.execPath, ['--import', 'tsx', '--test', ...tests], {cwd: root, env, stdio: 'inherit'});
	if (run.status !== 0) console.warn(`warning: the TS test run exited with ${run.status}; reducing what was logged`);
	return logDir;
}

const kind = (value: unknown): string => value === null ? 'null' : Array.isArray(value) ? 'array' : typeof value;

/** Key order and value kinds, recursively; array elements contribute their distinct shapes. */
function shape(value: unknown): string {
	if (Array.isArray(value)) return `[${[...new Set(value.map(shape))].sort().join('|')}]`;
	if (value && typeof value === 'object') return `{${Object.entries(value).map(([key, child]) => `${JSON.stringify(key)}:${shape(child)}`).join(',')}}`;
	return kind(value);
}

// Machine-specific path prefixes (the checkout, the temp directory, the home directory) become neutral ones, so the
// corpus does not depend on (or reveal) where it was captured. Only string contents change, never a message's shape.
const tmp = fs.realpathSync(os.tmpdir()).replace(/^\/private(?=\/)/, '');
const REPLACEMENTS: Array<[string, string]> = ([
	[root, '/src/deckhand'],
	[`/private${tmp}`, '/private/tmp'],
	[tmp, '/tmp'],
	[os.tmpdir().replace(/\/$/, ''), '/tmp'],
	[os.userInfo().homedir, '/home/user'],
] as Array<[string, string]>).filter(([from]) => from.length > 1).sort((a, b) => b[0].length - a[0].length);
function sanitize(text: string): string {
	for (const [from, to] of REPLACEMENTS) text = text.split(from).join(to);
	return text;
}

function readLogs(logDir: string): Line[] {
	const lines: Line[] = [];
	for (const name of fs.readdirSync(logDir).filter(file => file.endsWith('.jsonl')).sort()) {
		const role = name.replace(/-\d+\.jsonl$/, '');
		const fileWorker = role === 'worker' ? 'session' : role === 'workspace-worker' ? 'workspace' : undefined;
		// Request IDs are UUIDs, unique per file; responses are annotated with their request's type.
		const requests = new Map<string, string>();
		for (let raw of fs.readFileSync(path.join(logDir, name), 'utf8').split('\n')) {
			if (!raw.trim()) continue;
			raw = sanitize(raw);
			let entry: {dir: Dir; worker?: 'session' | 'workspace'; msg: Record<string, unknown>};
			try { entry = JSON.parse(raw); } catch { continue; }
			if (!entry.msg || typeof entry.msg !== 'object') continue;
			const worker = entry.worker ?? fileWorker;
			const {type, requestId} = entry.msg as {type?: unknown; requestId?: unknown};
			const line: Line = {dir: entry.dir, ...worker ? {worker} : {}, msg: entry.msg};
			if (typeof requestId === 'string') {
				if (type === 'response') { const req = requests.get(`${worker ?? ''}:${requestId}`); if (req) line.req = req; }
				else if (typeof type === 'string') requests.set(`${worker ?? ''}:${requestId}`, type);
			}
			lines.push(line);
		}
	}
	return lines;
}

const serialize = (line: Line): string => JSON.stringify({dir: line.dir, ...line.worker ? {worker: line.worker} : {}, ...line.req ? {req: line.req} : {}, msg: line.msg});

function reduce(lines: Line[]): string[] {
	// group key -> shape -> smallest serialized line
	const groups = new Map<string, Map<string, string>>();
	for (const line of lines) {
		const group = [line.dir, line.worker ?? '', String(line.msg.type), line.req ?? ''].join('\0');
		const shapes = groups.get(group) ?? new Map<string, string>();
		groups.set(group, shapes);
		const key = shape(line.msg), text = serialize(line), current = shapes.get(key);
		if (current === undefined || text.length < current.length || (text.length === current.length && text < current)) shapes.set(key, text);
	}
	const out: string[] = [];
	const sortedGroups = [...groups.keys()].sort((a, b) => {
		const [da] = a.split('\0'), [db] = b.split('\0');
		return DIR_ORDER.indexOf(da as Dir) - DIR_ORDER.indexOf(db as Dir) || (a < b ? -1 : a > b ? 1 : 0);
	});
	for (const group of sortedGroups) {
		// The most different shapes first (smallest texts break ties), capped per group.
		const texts = [...groups.get(group)!.values()].sort((a, b) => a.length - b.length || (a < b ? -1 : a > b ? 1 : 0));
		out.push(...texts.slice(0, PER_TYPE));
	}
	let total = out.reduce((sum, text) => sum + Buffer.byteLength(text) + 1, 0);
	// Over the budget: drop the largest samples of groups that keep at least one.
	while (total > MAX_BYTES) {
		const counts = new Map<string, number>();
		const groupOf = (text: string) => { const line = JSON.parse(text) as Line; return [line.dir, line.worker ?? '', String(line.msg.type), line.req ?? ''].join('\0'); };
		for (const text of out) counts.set(groupOf(text), (counts.get(groupOf(text)) ?? 0) + 1);
		let largest = -1;
		for (let i = 0; i < out.length; i++) if ((counts.get(groupOf(out[i]!)) ?? 0) > 1 && (largest < 0 || out[i]!.length > out[largest]!.length)) largest = i;
		if (largest < 0) break;
		total -= Buffer.byteLength(out[largest]!) + 1;
		out.splice(largest, 1);
	}
	return out;
}

/** A session as daemon.ts persistedSession stores it: no notes, no projected worktree markers, no attention reason. */
function persisted(session: Record<string, any>): Record<string, any> {
	const {notes: _notes, notesFile: _file, sharedNotes: _shared, ...stored} = session;
	if (stored.worktree?.id) {
		const worktree = {...stored.worktree};
		for (const key of WORKTREE_MARKERS) delete worktree[key];
		stored.worktree = worktree;
	}
	if (stored.attention?.reason) { const {reason: _reason, ...attention} = stored.attention; stored.attention = attention; }
	return stored;
}
const WORKTREE_MARKERS = ['mergedAt', 'mergeMode', 'mergeTargetBranch', 'mergeSourceRef', 'mergeSourceSha', 'mergeMarkedManually', 'mergeDetected', 'deletedAt'];

/**
 * fixtures/wire/records: state.json, config.json and ui-state.json exactly as storage.ts / uiState.ts write them (run
 * against a temp DECKHAND_HOME), from the corpus's sessions plus hand-made config and UI state, and TS's normalised
 * views of them, for crates/protocol/tests/wire_corpus.rs.
 */
async function writeRecords(corpus: string[]): Promise<void> {
	const home = fs.mkdtempSync(path.join(os.tmpdir(), 'deckhand-wire-records-'));
	process.env.DECKHAND_HOME = home;
	const storage = await import('../../src/storage.js');
	const uiState = await import('../../src/uiState.js');
	const dir = path.join(root, 'fixtures', 'wire', 'records');
	fs.mkdirSync(dir, {recursive: true});

	// state.json: every session the corpus shows (by shape), stored as the daemon stores it, and a worktree record per
	// incarnation they reference (markers from the projected session, in the order they were projected).
	const sessions = new Map<string, Record<string, any>>();
	const add = (session: any) => { if (session && typeof session === 'object' && typeof session.id === 'string') { const stored = persisted(session); const key = shape(stored); if (!sessions.has(key)) sessions.set(key, stored); } };
	const worktrees = new Map<string, Record<string, any>>();
	for (const text of corpus) {
		const {msg} = JSON.parse(text) as Line;
		const candidates = msg.type === 'session-updated' ? [msg.session] : msg.type === 'response' && Array.isArray(msg.data) ? msg.data : [];
		for (const session of candidates as any[]) {
			add(session);
			const worktree = session?.worktree;
			if (worktree?.id && worktree.path && !worktrees.has(worktree.id)) {
				const markers = Object.fromEntries(Object.entries(worktree).filter(([key]) => WORKTREE_MARKERS.includes(key)));
				worktrees.set(worktree.id, {id: worktree.id, path: worktree.path, createdAt: session.createdAt, ...worktree.head ? {baseSha: worktree.head} : {}, ...markers});
			}
		}
	}
	const storedSessions = [...sessions.values()].slice(0, 40);
	// Keys a newer version may add survive a round trip (the daemon spreads sessions).
	if (storedSessions[0]) storedSessions[0] = {...storedSessions[0], futureField: {kept: true}};
	const storedWorktrees = [...worktrees.values()];
	if (storedWorktrees[0]) storedWorktrees[0] = {...storedWorktrees[0], mergeDismissedTip: '0123456789abcdef0123456789abcdef01234567'};
	await storage.saveState({sessions: storedSessions as any, worktrees: storedWorktrees as any});
	fs.copyFileSync(path.join(home, 'state.json'), path.join(dir, 'state.json'));

	// config.json: a hand-made file (unknown keys, an old single-fingerprint entry, wrong-typed values) updated by TS.
	fs.writeFileSync(path.join(home, 'config.json'), `${JSON.stringify({zeta: {nested: [1, 2]}, dev_command: 'npm run dev', trustedProjects: {'/old/repo': 'abc123', '/bad': 7}, notifications: 'yes', attach_scroll_sensitivity: 1.5, defaults: {defaultAgent: 'pi', actions: {test: 'npm test'}}}, null, 2)}\n`);
	await storage.updateAppConfig(current => ({...current, agent_hooks: true, trustedProjects: {...current.trustedProjects, '/repo': ['f1', 'f0']}}));
	fs.copyFileSync(path.join(home, 'config.json'), path.join(dir, 'config.json'));
	fs.writeFileSync(path.join(dir, 'app-config.normalized.json'), `${JSON.stringify(await storage.loadAppConfig())}\n`);

	// ui-state.json: two repositories saved through saveUiState.
	await uiState.saveUiState('/repo/a', {selectedId: 's1', activeTab: 'git', sidebarWidth: 34, sessionFilter: 'running', sessionQuery: 'fix', sessionTabs: {s1: 'git', s2: 'notes'}, collapsedSessionIds: ['s3'], hiddenExitedSessionIds: [], readReplies: {s1: '2026-01-01T00:00:00.000Z'}});
	await uiState.saveUiState('/repo/b', {sessionTabs: {}, collapsedSessionIds: [], hiddenExitedSessionIds: [], readReplies: {}} as any);
	fs.copyFileSync(path.join(home, 'ui-state.json'), path.join(dir, 'ui-state.json'));
	const inputs: unknown[] = [
		null, {}, [], 'text',
		{selectedId: 7, activeTab: 'notes', sessionFilter: 'bogus', sessionQuery: 5, sidebarWidth: 12.5, extra: true},
		{activeTab: 'agent', sessionFilter: 'archived', sessionQuery: `${'q'.repeat(4095)}étail`, sidebarWidth: null},
		{sessionTabs: ['git', 'nope', 'dev'], collapsedSessionIds: 'abc', hiddenExitedSessionIds: ['a', 1, 'b', null]},
		{sessionTabs: {a: 'preview', b: 3, c: 'terminal'}, readReplies: Object.fromEntries(Array.from({length: 1003}, (_, i) => [`s${i}`, i % 7 ? `t${i}` : i]))},
		{collapsedSessionIds: Array.from({length: 10_005}, (_, i) => `c${i}`)},
	];
	fs.writeFileSync(path.join(dir, 'ui-state.normalize.jsonl'), inputs.map(input => `${JSON.stringify({input, output: uiState.normalizeUiState(input)})}\n`).join(''));
	fs.rmSync(home, {recursive: true, force: true});
}

const logDir = process.argv[2] ?? capture();
const lines = readLogs(logDir);
const reduced = reduce(lines);
fs.mkdirSync(path.dirname(OUT), {recursive: true});
fs.writeFileSync(OUT, reduced.map(text => `${text}\n`).join(''));
await writeRecords(reduced);
const types = new Set(reduced.map(text => { const line = JSON.parse(text) as Line; return `${line.dir} ${line.worker ?? ''} ${String(line.msg.type)}`; }));
console.log(`${lines.length} logged messages -> ${reduced.length} corpus lines, ${types.size} (dir, type) pairs, ${fs.statSync(OUT).size} bytes: ${path.relative(root, OUT)} (logs: ${logDir})`);
