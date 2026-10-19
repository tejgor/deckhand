// Replays the synthetic v45 Deckhand home (fixtures/state/home-v45/) through the TS daemon and records what it leaves
// behind after starting, serving a few read requests and stopping cleanly:
//   fixtures/state/home-v45.after-idle/          the DECKHAND_HOME tree (file contents byte-exact after normalisation)
//   fixtures/state/home-v45.after-idle.modes.json  every path's mode in it
//   fixtures/state/read-responses.jsonl          one {request, response} per line
//
//   npm run build && node --import tsx parity/gen/state.gen.ts            # regenerate
//   node --import tsx parity/gen/state.gen.ts --check                     # exit 1 when the outputs on disk differ
//   node --import tsx parity/gen/state.gen.ts --keep                      # keep the sandbox (path printed)
//
// Isolation: everything runs in a fresh sandbox under $TMPDIR (DECKHAND_HOME, HOME, XDG dirs, the git repositories,
// fake claude/codex/pi that only answer --version). The real ~/.deckhand is never read. See fixtures/state/README.md for
// the placeholder root, the sandbox description (fixtures/state/sandbox.json) and the normalisations applied.

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {createHash, randomUUID} from 'node:crypto';
import {execFileSync, spawn, type ChildProcess} from 'node:child_process';
import {fileURLToPath} from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const STATE_FIXTURES = path.join(ROOT, 'fixtures', 'state');
const HOME_FIXTURE = path.join(STATE_FIXTURES, 'home-v45');
const HOME_MODES = path.join(STATE_FIXTURES, 'home-v45.modes.json');
const AFTER_DIR = path.join(STATE_FIXTURES, 'home-v45.after-idle');
const AFTER_MODES = path.join(STATE_FIXTURES, 'home-v45.after-idle.modes.json');
const RESPONSES = path.join(STATE_FIXTURES, 'read-responses.jsonl');
const CLI = path.join(ROOT, 'dist', 'cli.js');

interface Sandbox {
	placeholder: string;
	deckhandHome: string;
	home: string;
	bin: string;
	git: {name: string; email: string; start: string; stepSeconds: number};
	steps: Array<
		| {op: 'init'; repo: string}
		| {op: 'write'; path: string; text: string}
		| {op: 'mkdir'; path: string}
		| {op: 'commit'; repo: string; message: string}
		| {op: 'worktree'; repo: string; path: string; branch: string}
		| {op: 'merge'; repo: string; branch: string; message: string}
	>;
	expect: Record<string, Record<string, string>>;
	agents: Record<string, string>;
	agentLatest: Record<string, string>;
}

const sandboxSpec = JSON.parse(fs.readFileSync(path.join(STATE_FIXTURES, 'sandbox.json'), 'utf8')) as Sandbox;
const P = sandboxSpec.placeholder;

// Files whose content the daemon writes with wall-clock data and in an order that depends on timing: listed with
// their mode only (see README).
const VOLATILE_FILES = new Set(['daemon.log']);

// ── Sandbox ─────────────────────────────────────────────────────────────────────────────────────────────────────

/** Paths of every directory a repository note or task list can be keyed by (hash of the resolved root). */
function sandboxRoots(): string[] {
	const roots = new Set<string>();
	for (const step of sandboxSpec.steps) {
		if (step.op === 'init') roots.add(step.repo);
		if (step.op === 'worktree') roots.add(step.path);
		if (step.op === 'mkdir') roots.add(step.path);
	}
	return [...roots];
}

const noteHash = (root: string) => createHash('sha256').update(path.resolve(root)).digest('hex').slice(0, 16);

function buildSandbox(real: string): Record<string, Record<string, string>> {
	let tick = 0;
	const start = Date.parse(sandboxSpec.git.start);
	const git = (cwd: string, ...args: string[]) => {
		const date = new Date(start + tick * sandboxSpec.git.stepSeconds * 1000).toISOString().replace('.000Z', 'Z');
		return execFileSync('git', args, {
			cwd,
			encoding: 'utf8',
			env: {
				PATH: '/usr/bin:/bin:/usr/sbin:/sbin', HOME: path.join(real, sandboxSpec.home), GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1',
				GIT_AUTHOR_NAME: sandboxSpec.git.name, GIT_AUTHOR_EMAIL: sandboxSpec.git.email, GIT_COMMITTER_NAME: sandboxSpec.git.name, GIT_COMMITTER_EMAIL: sandboxSpec.git.email,
				GIT_AUTHOR_DATE: date, GIT_COMMITTER_DATE: date, TZ: 'UTC', LANG: 'C', LC_ALL: 'C',
			},
		}).trim();
	};
	for (const step of sandboxSpec.steps) {
		switch (step.op) {
			case 'init': {
				const repo = path.join(real, step.repo);
				fs.mkdirSync(repo, {recursive: true});
				git(repo, 'init', '-q', '-b', 'main');
				git(repo, 'config', 'commit.gpgsign', 'false');
				git(repo, 'config', 'core.hooksPath', path.join(repo, '.git', 'no-hooks'));
				break;
			}
			case 'write':
				fs.mkdirSync(path.dirname(path.join(real, step.path)), {recursive: true});
				fs.writeFileSync(path.join(real, step.path), step.text);
				break;
			case 'mkdir':
				fs.mkdirSync(path.join(real, step.path), {recursive: true});
				break;
			case 'commit':
				tick++;
				git(path.join(real, step.repo), 'add', '-A');
				git(path.join(real, step.repo), 'commit', '-q', '-m', step.message);
				break;
			case 'worktree':
				tick++;
				fs.mkdirSync(path.dirname(path.join(real, step.path)), {recursive: true});
				git(path.join(real, step.repo), 'worktree', 'add', '-q', '-b', step.branch, path.join(real, step.path), 'HEAD');
				break;
			case 'merge':
				tick++;
				git(path.join(real, step.repo), 'merge', '-q', '--no-ff', '-m', step.message, step.branch);
				break;
		}
	}
	const refs: Record<string, Record<string, string>> = {};
	for (const [repo, branches] of Object.entries(sandboxSpec.expect)) {
		refs[repo] = Object.fromEntries(Object.keys(branches).map(branch => [branch, git(path.join(real, repo), 'rev-parse', `refs/heads/${branch}`)]));
	}
	if (process.argv.includes('--sandbox-only')) {
		for (const repo of Object.keys(sandboxSpec.expect)) console.log(`${repo}:\n${git(path.join(real, repo), 'log', '--all', '--format=%H %s')}\n${git(path.join(real, repo), 'reflog', '--all', '--format=%H %gd %gs')}`);
	}
	// Fake agents: answer `--version` like the real ones and do nothing else (the daemon asks for versions on start).
	const bin = path.join(real, sandboxSpec.bin);
	fs.mkdirSync(bin, {recursive: true});
	for (const [program, version] of Object.entries(sandboxSpec.agents)) {
		fs.writeFileSync(path.join(bin, program), `#!/bin/sh\nif [ "$1" = "--version" ]; then echo '${version}'; exit 0; fi\necho 'fixture agent: refusing to run' >&2\nexit 64\n`, {mode: 0o755});
	}
	return refs;
}

// ── Placeholder rewriting ───────────────────────────────────────────────────────────────────────────────────────

interface Rewriter {
	toReal(text: string): string;
	toPlaceholder(text: string): string;
	nameToReal(name: string): string;
	nameToPlaceholder(name: string): string;
}

function rewriter(real: string): Rewriter {
	const unresolved = [path.join(os.tmpdir(), path.basename(real))].filter(candidate => candidate !== real);
	const hashes = sandboxRoots().map(root => ({fixture: noteHash(path.join(P, root)), real: noteHash(path.join(real, root))}));
	const swapHashes = (text: string, from: 'fixture' | 'real', to: 'fixture' | 'real') => hashes.reduce((value, pair) => value.split(pair[from]).join(pair[to]), text);
	return {
		toReal: text => swapHashes(text.split(P).join(real), 'fixture', 'real'),
		toPlaceholder: text => swapHashes([real, ...unresolved].reduce((value, candidate) => value.split(candidate).join(P), text), 'real', 'fixture'),
		nameToReal: name => swapHashes(name, 'fixture', 'real'),
		nameToPlaceholder: name => swapHashes(name, 'real', 'fixture'),
	};
}

// ── Trees and modes ─────────────────────────────────────────────────────────────────────────────────────────────

type Modes = Record<string, string>;
const octal = (mode: number) => `0${(mode & 0o777).toString(8)}`;

function walk(dir: string, relative = ''): string[] {
	const out: string[] = [];
	for (const entry of fs.readdirSync(path.join(dir, relative), {withFileTypes: true}).sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0))) {
		const rel = relative ? `${relative}/${entry.name}` : entry.name;
		if (entry.isDirectory()) { out.push(`${rel}/`); out.push(...walk(dir, rel)); }
		else out.push(rel);
	}
	return out;
}

/** Copies home-v45 into the sandbox: placeholder paths and path hashes rewritten, modes and empty directories from the manifest. */
function installHome(target: string, rw: Rewriter): void {
	const modes = JSON.parse(fs.readFileSync(HOME_MODES, 'utf8')) as Modes;
	fs.mkdirSync(target, {recursive: true});
	for (const rel of Object.keys(modes).filter(key => key.endsWith('/') && key !== './')) fs.mkdirSync(path.join(target, rw.nameToReal(rel)), {recursive: true});
	for (const rel of walk(HOME_FIXTURE)) {
		if (rel.endsWith('/')) { fs.mkdirSync(path.join(target, rw.nameToReal(rel)), {recursive: true}); continue; }
		if (!(rel in modes)) throw new Error(`home-v45.modes.json lacks ${rel}`);
		const bytes = fs.readFileSync(path.join(HOME_FIXTURE, rel));
		fs.writeFileSync(path.join(target, rw.nameToReal(rel)), rw.toReal(bytes.toString('utf8')));
	}
	for (const rel of Object.keys(modes)) {
		if (rel !== './' && !rel.endsWith('/') && !fs.existsSync(path.join(HOME_FIXTURE, rel))) throw new Error(`home-v45.modes.json lists missing ${rel}`);
	}
	// Deepest first, so a directory's own mode never blocks setting its children.
	for (const rel of Object.keys(modes).sort((a, b) => b.length - a.length)) {
		fs.chmodSync(rel === './' ? target : path.join(target, rw.nameToReal(rel)), Number.parseInt(modes[rel]!, 8));
	}
}

// ── Normalisation ───────────────────────────────────────────────────────────────────────────────────────────────

const ISO = /\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z/g;
const TRASH_DATE = /^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}_/;
const TASK_DATE = /\b(done|added)=(\d{4}-\d{2}-\d{2})\b/g;
const TASK_ID = /\bt=([0-9a-f]{8})\b/g;
const USER_SLUG = (() => {
	let name = '';
	try { name = os.userInfo().username; } catch {}
	return name.toLowerCase().replace(/[^a-z0-9._-]+/g, '-').replace(/^[-.]+|[-.]+$/g, '').replace(/\.{2,}/g, '.') || 'user';
})();
const UUID = /[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}/g;

interface Normaliser {
	text(text: string): string;
	name(name: string): string;
	value(value: unknown): unknown;
}

function normaliser(rw: Rewriter, window: {start: number; end: number}, known: Set<string>): Normaliser {
	const inWindow = (ms: number) => ms >= window.start - 1000 && ms <= window.end + 1000;
	const fresh = new Map<string, string>();
	const uuid = (id: string) => {
		if (known.has(id)) return id;
		if (!fresh.has(id)) fresh.set(id, `<uuid-${fresh.size + 1}>`);
		return fresh.get(id)!;
	};
	// Task IDs Deckhand gives (src/tasks.ts newTaskId: 4 random bytes) to tasks that had none.
	const freshTasks = new Map<string, string>();
	const taskId = (id: string) => {
		if (!freshTasks.has(id)) freshTasks.set(id, `<task-${freshTasks.size + 1}>`);
		return freshTasks.get(id)!;
	};
	// Task bookkeeping dates (`done=`/`added=` in `<!-- dh:… -->`, src/tasks.ts `today`) are UTC days.
	const today = new Set([window.start, window.end].map(ms => new Date(ms).toISOString().slice(0, 10)));
	const text = (input: string) => rw.toPlaceholder(input)
		.replace(ISO, stamp => (inWindow(Date.parse(stamp)) ? '<now>' : stamp))
		.replace(TASK_DATE, (whole, key: string, day: string) => (today.has(day) ? `${key}=<today>` : whole))
		.replace(UUID, uuid)
		.replace(TASK_ID, (whole, id: string) => (known.has(`t=${id}`) ? whole : `t=${taskId(id)}`));
	// A note's revision hashes its text as on disk (src/notes.ts noteRevision); when normalising changed the text (a
	// fresh task ID, today's date), the revision is recomputed from the normalised text so the pair stays consistent.
	const revision = (input: string) => createHash('sha256').update(input).digest('hex').slice(0, 16);
	const revised = (original: string, stored: unknown): unknown => {
		if (typeof stored !== 'string' || stored !== revision(original)) return value(stored);
		const normalised = text(original);
		return normalised === original ? stored : revision(normalised);
	};
	const value = (input: unknown): unknown => {
		if (typeof input === 'string') return text(input);
		if (typeof input === 'number') return Number.isInteger(input) && input > 1e12 && inWindow(input) ? '<now-ms>' : input;
		if (Array.isArray(input)) return input.map(value);
		if (input && typeof input === 'object') {
			const record = input as Record<string, unknown>;
			return Object.fromEntries(Object.entries(record).map(([key, item]) => {
				if (key === 'revision' && typeof record.text === 'string') return [key, revised(record.text, item)];
				// settings-info's `user` is the OS account (src/worktreeLinks.ts userSlug, from getpwuid, not $USER).
				if (key === 'user' && item === USER_SLUG) return [key, '<user>'];
				if (key === 'notesFile' && typeof record.notes === 'string' && item && typeof item === 'object') {
					const file = item as Record<string, unknown>;
					return [key, Object.fromEntries(Object.entries(file).map(([inner, field]) => [inner, inner === 'revision' ? revised(record.notes as string, field) : value(field)]))];
				}
				return [text(key), value(item)];
			}));
		}
		return input;
	};
	return {
		text,
		name: input => {
			const renamed = rw.nameToPlaceholder(input).replace(UUID, uuid);
			const base = path.posix.basename(renamed);
			const date = TRASH_DATE.exec(base);
			if (!date) return renamed;
			const stamp = date[0].slice(0, -1).replace(/T(\d{2})-(\d{2})-(\d{2})$/, 'T$1:$2:$3.000Z');
			// Trash names carry the second they were trashed (src/notesStore.ts trashFileName): `NOW_` in file names.
			return inWindow(Date.parse(stamp)) || inWindow(Date.parse(stamp) + 999) ? `${path.posix.dirname(renamed) === '.' ? '' : `${path.posix.dirname(renamed)}/`}NOW_${base.slice(date[0].length)}` : renamed;
		},
		value,
	};
}

// ── Daemon ──────────────────────────────────────────────────────────────────────────────────────────────────────

const sleep = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));

async function waitFor<T>(read: () => Promise<T>, done: (value: T) => boolean, timeoutMs: number, what: string): Promise<T> {
	const deadline = Date.now() + timeoutMs;
	let last: T | undefined;
	for (;;) {
		try { last = await read(); if (done(last)) return last; } catch {}
		if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}: ${JSON.stringify(last)?.slice(0, 500)}`);
		await sleep(50);
	}
}

interface Run {responses: Array<{request: Record<string, unknown>; response: unknown}>; window: {start: number; end: number}}

async function runDaemon(real: string): Promise<Run> {
	const deckhandHome = path.join(real, sandboxSpec.deckhandHome);
	const home = path.join(real, sandboxSpec.home);
	fs.mkdirSync(home, {recursive: true});
	const env: Record<string, string> = {
		PATH: `${path.join(real, sandboxSpec.bin)}:/usr/bin:/bin:/usr/sbin:/sbin`,
		HOME: home,
		DECKHAND_HOME: deckhandHome,
		DECKHAND_DEV: '0',
		DECKHAND_AGENT_LATEST: JSON.stringify(sandboxSpec.agentLatest),
		SHELL: '/bin/sh',
		TERM: 'xterm-256color',
		LANG: 'en_US.UTF-8',
		TZ: 'UTC',
		TMPDIR: path.join(real, 'tmp'),
		XDG_CONFIG_HOME: path.join(home, '.config'),
		XDG_STATE_HOME: path.join(home, '.local', 'state'),
		XDG_CACHE_HOME: path.join(home, '.cache'),
		XDG_DATA_HOME: path.join(home, '.local', 'share'),
		CLAUDE_CONFIG_DIR: path.join(home, '.claude'),
		CODEX_HOME: path.join(home, '.codex'),
		GIT_CONFIG_GLOBAL: '/dev/null',
		GIT_CONFIG_NOSYSTEM: '1',
	};
	fs.mkdirSync(env.TMPDIR!, {recursive: true});
	// The client below talks to this daemon's socket (src/paths.ts reads DECKHAND_HOME at call time).
	process.env.DECKHAND_HOME = deckhandHome;
	const {request} = await import('../../src/client.js');
	const call = <T>(message: Record<string, unknown>) => request<T>({...message, requestId: randomUUID()} as Parameters<typeof request>[0], 20_000);
	const start = Date.now();
	// The daemon inherits this umask; files it creates without an explicit mode (daemon.log, workers/) depend on it.
	const previousUmask = process.umask(0o022);
	const daemon: ChildProcess = spawn(process.execPath, [CLI, '--daemon'], {cwd: path.join(real, 'repos', 'app'), env, stdio: ['ignore', 'pipe', 'pipe']});
	process.umask(previousUmask);
	let stderr = '';
	daemon.stderr?.on('data', data => { stderr += String(data); });
	daemon.stdout?.on('data', () => {});
	const exited = new Promise<number | null>(resolve => daemon.once('exit', code => resolve(code)));
	const responses: Run['responses'] = [];
	try {
		await waitFor(() => call<{ok: boolean}>({type: 'ping'}), result => result.ok, 20_000, 'the daemon to answer ping');
		// Background work started with the daemon: merge detection (feature-merged gets its detected marker; feature-a's
		// record its baseSha first, as records are checked in order). Wait for it so every run captures the same state.
		await waitFor(() => call<Array<{id: string; worktree?: {mergedAt?: string}}>>({type: 'list'}), sessions => Boolean(sessions.find(session => session.id === MERGED_SESSION)?.worktree?.mergedAt), 20_000, 'merge detection');
		// The detected merge ticks the merged worktree's tasks after persisting the record.
		await waitFor(() => call<{text: string}>({type: 'watch-tasks', cwd: path.join(real, 'repos', 'app')}), doc => MERGED_TASK_TICKED.test(doc.text), 20_000, 'the merged worktree\'s task to be ticked');
		await sleep(300);
		const app = path.join(real, 'repos', 'app'), lib = path.join(real, 'repos', 'lib');
		const reads: Array<Record<string, unknown>> = [
			{type: 'ping'},
			{type: 'list'},
			{type: 'subscribe', repoRoot: app},
			{type: 'subscribe', repoRoot: lib},
			{type: 'agent-versions'},
			{type: 'list-worktrees', cwd: app},
			{type: 'worktree-overview', cwd: app},
			{type: 'watch-tasks', cwd: app},
			{type: 'watch-tasks', cwd: lib},
			{type: 'project-info', cwd: app},
			{type: 'project-info', cwd: lib},
			{type: 'settings-info', cwd: app},
			{type: 'list-branches', cwd: app},
			{type: 'watch-preview', sessionId: PREVIEW_SESSION, cols: 80, rows: 24},
			{type: 'list'},
		];
		for (const message of reads) {
			let response: unknown;
			try { response = {ok: true, data: await call(message)}; }
			catch (error) { response = {ok: false, error: error instanceof Error ? error.message : String(error)}; }
			responses.push({request: message, response});
		}
	} finally {
		daemon.kill('SIGTERM');
		const code = await Promise.race([exited, sleep(10_000).then(() => 'timeout' as const)]);
		if (code === 'timeout') { daemon.kill('SIGKILL'); throw new Error(`daemon did not stop on SIGTERM\n${stderr}`); }
		if (code !== 0) throw new Error(`daemon exited with ${code}\n${stderr}`);
	}
	return {responses, window: {start, end: Date.now()}};
}

// Sessions the run looks at (ids from home-v45/state.json).
const MERGED_SESSION = '5e550000-0000-4000-8000-000000000006';
const PREVIEW_SESSION = '5e550000-0000-4000-8000-000000000002';
const MERGED_TASK_TICKED = /- \[x\] Merged feature task <!-- dh:t=7c3d4e5f [^>]*auto=merge/;

// ── Capture ─────────────────────────────────────────────────────────────────────────────────────────────────────

interface Captured {files: Map<string, Buffer>; modes: Modes; responses: string}

function capture(real: string, run: Run, rw: Rewriter): Captured {
	const deckhandHome = path.join(real, sandboxSpec.deckhandHome);
	const known = new Set<string>();
	for (const rel of walk(HOME_FIXTURE)) {
		for (const match of rel.matchAll(UUID)) known.add(match[0]);
		if (rel.endsWith('/')) continue;
		const content = fs.readFileSync(path.join(HOME_FIXTURE, rel), 'utf8');
		for (const match of content.matchAll(UUID)) known.add(match[0]);
		for (const match of content.matchAll(TASK_ID)) known.add(`t=${match[1]}`);
	}
	const norm = normaliser(rw, run.window, known);
	// state.json first: fresh UUIDs are numbered in the order the persisted state lists them.
	const entries = walk(deckhandHome).sort((a, b) => (a === 'state.json' ? -1 : b === 'state.json' ? 1 : 0));
	const files = new Map<string, Buffer>();
	const modes: Modes = {'./': octal(fs.statSync(deckhandHome).mode)};
	for (const rel of entries) {
		const name = norm.name(rel);
		modes[name] = octal(fs.lstatSync(path.join(deckhandHome, rel)).mode);
		if (rel.endsWith('/') || VOLATILE_FILES.has(rel)) continue;
		files.set(name, Buffer.from(norm.text(fs.readFileSync(path.join(deckhandHome, rel), 'utf8')), 'utf8'));
	}
	const sortedModes = Object.fromEntries(Object.entries(modes).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)));
	const responses = run.responses.map(entry => JSON.stringify({request: norm.value(entry.request), response: norm.value(entry.response)})).join('\n') + '\n';
	return {files, modes: sortedModes, responses};
}

function writeOutputs(captured: Captured): void {
	fs.rmSync(AFTER_DIR, {recursive: true, force: true});
	for (const [rel, bytes] of captured.files) {
		fs.mkdirSync(path.dirname(path.join(AFTER_DIR, rel)), {recursive: true});
		fs.writeFileSync(path.join(AFTER_DIR, rel), bytes);
	}
	fs.writeFileSync(AFTER_MODES, `${JSON.stringify(captured.modes, null, 2)}\n`);
	fs.writeFileSync(RESPONSES, captured.responses);
}

function compareOutputs(captured: Captured): string[] {
	const problems: string[] = [];
	const onDisk = fs.existsSync(AFTER_DIR) ? walk(AFTER_DIR).filter(rel => !rel.endsWith('/')) : [];
	for (const rel of new Set([...onDisk, ...captured.files.keys()])) {
		const current = fs.existsSync(path.join(AFTER_DIR, rel)) ? fs.readFileSync(path.join(AFTER_DIR, rel)) : undefined;
		const next = captured.files.get(rel);
		if (!current || !next || !current.equals(next)) problems.push(`home-v45.after-idle/${rel}`);
	}
	if (!fs.existsSync(AFTER_MODES) || fs.readFileSync(AFTER_MODES, 'utf8') !== `${JSON.stringify(captured.modes, null, 2)}\n`) problems.push('home-v45.after-idle.modes.json');
	if (!fs.existsSync(RESPONSES) || fs.readFileSync(RESPONSES, 'utf8') !== captured.responses) problems.push('read-responses.jsonl');
	return problems;
}

async function main(): Promise<void> {
	const check = process.argv.includes('--check');
	const keep = process.argv.includes('--keep');
	if (!fs.existsSync(CLI)) throw new Error('dist/cli.js is missing: run `npm run build` first');
	// Short: the daemon's socket (<sandbox>/deckhand-home/daemon.sock) must fit sun_path (104 bytes on macOS).
	const real = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'dhs-')));
	if (Buffer.byteLength(path.join(real, sandboxSpec.deckhandHome, 'daemon.sock')) > 103) throw new Error(`$TMPDIR is too long for a Unix socket path under ${real}`);
	try {
		const refs = buildSandbox(real);
		if (process.argv.includes("--sandbox-only")) {
			// For editing the fixture: the SHAs and the placeholder note hashes it must use.
			console.log(JSON.stringify({refs, noteHashes: Object.fromEntries(sandboxRoots().map(root => [root, noteHash(path.join(P, root))]))}, null, 2));
			return;
		}
		const mismatched = Object.entries(sandboxSpec.expect).flatMap(([repo, branches]) => Object.entries(branches).filter(([branch, sha]) => refs[repo]![branch] !== sha).map(([branch, sha]) => `${repo} ${branch}: expected ${sha || '(unset)'}, got ${refs[repo]![branch]}`));
		if (mismatched.length) throw new Error(`the sandbox repositories differ from fixtures/state/sandbox.json "expect" (the fixture's SHAs depend on them):\n${mismatched.join('\n')}`);
		const rw = rewriter(real);
		installHome(path.join(real, sandboxSpec.deckhandHome), rw);
		const run = await runDaemon(real);
		const captured = capture(real, run, rw);
		if (check) {
			const problems = compareOutputs(captured);
			if (problems.length) { console.log(`stale:\n${problems.join('\n')}`); process.exitCode = 1; }
			else console.log('state fixtures are up to date');
		} else {
			writeOutputs(captured);
			console.log(`wrote ${path.relative(process.cwd(), AFTER_DIR)}/ (${captured.files.size} files), ${path.relative(process.cwd(), AFTER_MODES)}, ${path.relative(process.cwd(), RESPONSES)} (${run.responses.length} responses)`);
		}
	} finally {
		if (keep) console.log(`sandbox kept at ${real}`);
		else fs.rmSync(real, {recursive: true, force: true});
	}
}

await main();
