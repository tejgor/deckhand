#!/usr/bin/env node
// G5 — state compatibility gate (one-off acceptance check of the Rust port; results in docs/port/gates/G5.md).
//
//   npm run build && cargo build --release -p deckhand
//   node scripts/gates/state-compat.mjs [--keep] [--report <file.json>]
//
// Two identical sandboxes are built from fixtures/state (sandbox.json + home-v45, rewritten as parity/gen/state.gen.ts
// does: placeholder root -> sandbox root, path-hash note names, modes from the manifest). The TS daemon
// (node dist/cli.js --daemon) runs on one, the Rust daemon (target/release/deckhand --daemon) on the other, each with
// its own HOME/XDG/TMPDIR inside its sandbox. Both sandboxes' git repositories are created by the same deterministic
// steps, so every SHA is identical. Then:
//   (a) every read request type is sent to both daemons; normalised responses must be equal;
//   (b) after that idle start + SIGTERM the two DECKHAND_HOME trees must be byte-identical (contents and modes);
//   (c) both daemons are restarted and the same sequence of writing operations is applied to both; the normalised
//       responses and the full trees are compared after every operation;
//   (d) every unknown key the fixture plants survives in the Rust-written tree;
//   (e) the TS daemon is started on the Rust-written tree (and, as the reference, on the TS-written one): every read
//       succeeds and matches, and the trees it leaves stay identical.
// Exit code 0 only when every comparison passed. The JSON report lists each check with a diff excerpt on failure.
//
// Isolation: nothing outside the two sandboxes under $TMPDIR is read or written (no ~/.deckhand, no real agents:
// claude/codex/pi on PATH are fake node scripts adapted from tests/helpers.ts fakeAgent).

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import net from 'node:net';
import {createHash, randomUUID} from 'node:crypto';
import {execFileSync, spawn} from 'node:child_process';
import {fileURLToPath} from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const STATE_FIXTURES = path.join(ROOT, 'fixtures', 'state');
const HOME_FIXTURE = path.join(STATE_FIXTURES, 'home-v45');
const HOME_MODES = path.join(STATE_FIXTURES, 'home-v45.modes.json');
const TS_CLI = path.join(ROOT, 'dist', 'cli.js');
const RS_BIN = path.resolve(ROOT, process.env.RS_BIN ?? 'target/release/deckhand');
const KEEP = process.argv.includes('--keep');
const REPORT = (() => { const at = process.argv.indexOf('--report'); return at > 0 ? path.resolve(process.argv[at + 1]) : undefined; })();

const sandboxSpec = JSON.parse(fs.readFileSync(path.join(STATE_FIXTURES, 'sandbox.json'), 'utf8'));
const P = sandboxSpec.placeholder;
const RUN_START = Date.now();
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

// ── Sandbox (same steps as parity/gen/state.gen.ts) ───────────────────────────────────────────────────────────────

function sandboxRoots() {
	const roots = new Set();
	for (const step of sandboxSpec.steps) {
		if (step.op === 'init') roots.add(step.repo);
		if (step.op === 'worktree' || step.op === 'mkdir') roots.add(step.path);
	}
	return [...roots];
}
const noteHash = root => createHash('sha256').update(path.resolve(root)).digest('hex').slice(0, 16);

// The fake agent of tests/helpers.ts (`fakeAgent`), with three changes: --version prints the sandbox.json version
// string, the shebang is this node (the daemon's PATH has no node), and its own bookkeeping files (trace-*, ready-*,
// input-*) go to $FAKE_AGENT_STATE instead of DECKHAND_HOME so they never enter the compared tree.
function fakeAgent(version) {
	return `#!${process.execPath}
const fs = require('fs');
const cp = require('child_process');
const path = require('path');
const STATE = process.env.FAKE_AGENT_STATE;
if (process.argv.includes('--help')) { console.log('PROMPT --settings --no-daemon resume'); process.exit(0); }
const provider = path.basename(process.argv[1]);
if (process.argv[2] === '--version') { console.log(${JSON.stringify(version)}); process.exit(0); }
const forkOf = provider === 'codex' && process.argv[2] === 'fork' ? process.argv[3] : undefined;
const flag = name => { const at = process.argv.indexOf(name); return at > 1 ? process.argv[at + 1] : undefined; };
const id = forkOf ? 'fixture-child-codex' : provider === 'claude' ? flag('--session-id') ?? flag('--resume') ?? 'fixture-native-claude' : 'fixture-native-' + provider;
fs.writeFileSync(path.join(STATE, 'trace-' + process.env.DECKHAND_SESSION_ID + '.json'), JSON.stringify({args: process.argv.slice(2), launchId: process.env.DECKHAND_LAUNCH_ID, pid: process.pid}));
function hook(event, sessionId = id) { return new Promise(resolve => { const child = cp.spawn(process.env.TEST_CLI, ['hook'], {stdio: ['pipe', 'ignore', 'ignore']}); child.stdin.end(JSON.stringify({hook_event_name: event, session_id: sessionId})); child.on('close', resolve); }); }
const startHooks = () => forkOf ? hook('SessionStart', forkOf).then(() => hook('SessionStart')).then(() => hook('SessionStart', forkOf)) : hook('SessionStart');
startHooks().then(() => { fs.writeFileSync(path.join(STATE, 'ready-' + process.env.DECKHAND_SESSION_ID + '-' + process.env.DECKHAND_LAUNCH_ID), ''); console.log('ready'); });
if (process.stdin.isTTY) process.stdin.setRawMode(true);
process.stdin.resume();
process.stdin.on('data', data => fs.appendFileSync(path.join(STATE, 'input-' + process.env.DECKHAND_SESSION_ID), data));
setInterval(() => {}, 10000);
`;
}

function buildSandbox(real) {
	let tick = 0;
	const start = Date.parse(sandboxSpec.git.start);
	const git = (cwd, ...args) => {
		const date = new Date(start + tick * sandboxSpec.git.stepSeconds * 1000).toISOString().replace('.000Z', 'Z');
		return execFileSync('git', args, {
			cwd, encoding: 'utf8',
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
			case 'mkdir': fs.mkdirSync(path.join(real, step.path), {recursive: true}); break;
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
	for (const [repo, branches] of Object.entries(sandboxSpec.expect)) {
		for (const [branch, sha] of Object.entries(branches)) {
			const actual = git(path.join(real, repo), 'rev-parse', `refs/heads/${branch}`);
			if (actual !== sha) throw new Error(`sandbox ${repo} ${branch}: expected ${sha}, got ${actual}`);
		}
	}
	const bin = path.join(real, sandboxSpec.bin);
	fs.mkdirSync(bin, {recursive: true});
	for (const [program, version] of Object.entries(sandboxSpec.agents)) fs.writeFileSync(path.join(bin, program), fakeAgent(version), {mode: 0o755});
	fs.mkdirSync(path.join(real, 'agent-state'), {recursive: true});
	// The TS side's `deckhand` for the fake agent's `$TEST_CLI hook` (the Rust side uses the Rust binary itself).
	fs.mkdirSync(path.join(real, 'cli'), {recursive: true});
	const quote = value => `'${value.replaceAll("'", `'\\''`)}'`;
	fs.writeFileSync(path.join(real, 'cli', 'deckhand'), `#!/bin/sh\nexec ${quote(process.execPath)} ${quote(TS_CLI)} "$@"\n`, {mode: 0o755});
}

function rewriter(real) {
	const unresolved = [path.join(os.tmpdir(), path.basename(real))].filter(candidate => candidate !== real);
	const hashes = sandboxRoots().map(root => ({fixture: noteHash(path.join(P, root)), real: noteHash(path.join(real, root))}));
	const swap = (text, from, to) => hashes.reduce((value, pair) => value.split(pair[from]).join(pair[to]), text);
	return {
		toReal: text => swap(text.split(P).join(real), 'fixture', 'real'),
		toPlaceholder: text => swap([real, ...unresolved].reduce((value, candidate) => value.split(candidate).join(P), text), 'real', 'fixture'),
		nameToReal: name => swap(name, 'fixture', 'real'),
		nameToPlaceholder: name => swap(name, 'real', 'fixture'),
	};
}

function walk(dir, relative = '') {
	const out = [];
	for (const entry of fs.readdirSync(path.join(dir, relative), {withFileTypes: true}).sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0))) {
		const rel = relative ? `${relative}/${entry.name}` : entry.name;
		if (entry.isDirectory()) { out.push(`${rel}/`); out.push(...walk(dir, rel)); } else out.push(rel);
	}
	return out;
}

function installHome(target, rw) {
	const modes = JSON.parse(fs.readFileSync(HOME_MODES, 'utf8'));
	fs.mkdirSync(target, {recursive: true});
	for (const rel of Object.keys(modes).filter(key => key.endsWith('/') && key !== './')) fs.mkdirSync(path.join(target, rw.nameToReal(rel)), {recursive: true});
	for (const rel of walk(HOME_FIXTURE)) {
		if (rel.endsWith('/')) { fs.mkdirSync(path.join(target, rw.nameToReal(rel)), {recursive: true}); continue; }
		fs.writeFileSync(path.join(target, rw.nameToReal(rel)), rw.toReal(fs.readFileSync(path.join(HOME_FIXTURE, rel), 'utf8')));
	}
	for (const rel of Object.keys(modes).sort((a, b) => b.length - a.length)) fs.chmodSync(rel === './' ? target : path.join(target, rw.nameToReal(rel)), Number.parseInt(modes[rel], 8));
}

// ── Normalisation ─────────────────────────────────────────────────────────────────────────────────────────────────
// Only values that are volatile by construction (each documented in docs/port/gates/G5.md):
//   sandbox root -> placeholder (and path-hash note names), ISO timestamps within the run -> <now>, epoch-ms within
//   the run -> <now-ms>, trash file date prefix within the run -> NOW_, task dates today -> <today>, UUIDs not in the
//   fixture -> <uuid-N> (numbered per tree by first appearance), fresh task IDs -> <task-N>, note revisions recomputed
//   after the above, pids (`pid` keys, daemon.pid, workers/*.pid) -> <pid>, the OS user slug -> <user>, the
//   implementation's own executable path -> <deckhand-cli>, the random part of handoff file names -> <handoff-N>,
//   8-hex prefixes of fresh session IDs (agent session names) -> <uuid-N:8>.

const ISO = /\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z/g;
const EPOCH_MS = /\b1\d{12}\b/g;
const TRASH_DATE = /^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}_/;
const TASK_DATE = /\b(done|added)=(\d{4}-\d{2}-\d{2})\b/g;
const TASK_ID = /\bt=([0-9a-f]{8})\b/g;
const HANDOFF = /(handoffs\/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}-)([0-9a-f]{8})\.md/g;
const KNOWN_HANDOFFS = new Set(fs.readdirSync(path.join(HOME_FIXTURE, 'handoffs')).map(name => /-([0-9a-f]{8})\.md$/.exec(name)?.[1]).filter(Boolean));
const UUID = /[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}/g;
const USER_SLUG = (() => {
	let name = '';
	try { name = os.userInfo().username; } catch {}
	return name.toLowerCase().replace(/[^a-z0-9._-]+/g, '-').replace(/^[-.]+|[-.]+$/g, '').replace(/\.{2,}/g, '.') || 'user';
})();

const KNOWN = (() => {
	const known = new Set();
	for (const rel of walk(HOME_FIXTURE)) {
		for (const match of rel.matchAll(UUID)) known.add(match[0]);
		if (rel.endsWith('/')) continue;
		const content = fs.readFileSync(path.join(HOME_FIXTURE, rel), 'utf8');
		for (const match of content.matchAll(UUID)) known.add(match[0]);
		for (const match of content.matchAll(TASK_ID)) known.add(`t=${match[1]}`);
	}
	return known;
})();

function normaliser(real) {
	const rw = rewriter(real);
	const inWindow = ms => ms >= RUN_START - 1000 && ms <= Date.now() + 1000;
	const fresh = new Map();
	const uuid = id => {
		if (KNOWN.has(id)) return id;
		if (!fresh.has(id)) fresh.set(id, `<uuid-${fresh.size + 1}>`);
		return fresh.get(id);
	};
	const freshTasks = new Map();
	const taskId = id => {
		if (!freshTasks.has(id)) freshTasks.set(id, `<task-${freshTasks.size + 1}>`);
		return freshTasks.get(id);
	};
	// Handoff files are `<session id>-<randomUUID().slice(0, 8)>.md` (src/sessionFeatures.ts:97).
	const freshHandoffs = new Map();
	const handoff = (whole, prefix, random) => {
		if (KNOWN_HANDOFFS.has(random)) return whole;
		if (!freshHandoffs.has(random)) freshHandoffs.set(random, `<handoff-${freshHandoffs.size + 1}>`);
		return `${prefix}${freshHandoffs.get(random)}.md`;
	};
	const cliPaths = [RS_BIN, TS_CLI, path.join(real, 'cli', 'deckhand'), process.execPath].sort((a, b) => b.length - a.length);
	const text = input => {
		let out = rw.toPlaceholder(input);
		for (const cli of cliPaths) out = out.split(cli).join('<deckhand-cli>');
		const days = new Set([new Date(RUN_START).toISOString().slice(0, 10), new Date().toISOString().slice(0, 10)]);
		// The agent hook command is the implementation's own executable: `'node' 'dist/cli.js' 'hook'` vs `'deckhand' 'hook'`.
		out = out.split(`'<deckhand-cli>' '<deckhand-cli>'`).join(`'<deckhand-cli>'`);
		out = out
			.replace(HANDOFF, handoff)
			.replace(ISO, stamp => (inWindow(Date.parse(stamp)) ? '<now>' : stamp))
			.replace(EPOCH_MS, ms => (inWindow(Number(ms)) ? '<now-ms>' : ms))
			.replace(TASK_DATE, (whole, key, day) => (days.has(day) ? `${key}=<today>` : whole))
			.replace(UUID, uuid)
			.replace(TASK_ID, (whole, id) => (KNOWN.has(`t=${id}`) ? whole : `t=${taskId(id)}`))
			// Persisted process ids (a running session's `pid`).
			.replace(/"pid": \d+/g, '"pid": "<pid>"');
		// Agent session names embed the session ID's first 8 hex digits (src/daemon.ts:267 `dh-<title>-<id[0..8]>`).
		for (const [id, token] of fresh) out = out.split(id.slice(0, 8)).join(`${token.slice(0, -1)}:8>`);
		return out;
	};
	const revision = input => createHash('sha256').update(input).digest('hex').slice(0, 16);
	const revised = (original, stored) => {
		if (typeof stored !== 'string' || stored !== revision(original)) return value(stored);
		const normalised = text(original);
		return normalised === original ? stored : revision(normalised);
	};
	const value = input => {
		if (typeof input === 'string') return text(input);
		if (typeof input === 'number') return Number.isInteger(input) && input > 1e12 && inWindow(input) ? '<now-ms>' : input;
		if (Array.isArray(input)) return input.map(value);
		if (input && typeof input === 'object') {
			return Object.fromEntries(Object.entries(input).map(([key, item]) => {
				if (key === 'revision' && typeof input.text === 'string') return [key, revised(input.text, item)];
				if (key === 'user' && item === USER_SLUG) return [key, '<user>'];
				if (key === 'pid' && typeof item === 'number') return [key, '<pid>'];
				if (key === 'notesFile' && typeof input.notes === 'string' && item && typeof item === 'object') {
					return [key, Object.fromEntries(Object.entries(item).map(([inner, field]) => [inner, inner === 'revision' ? revised(input.notes, field) : value(field)]))];
				}
				return [text(key), value(item)];
			}));
		}
		return input;
	};
	const name = input => {
		const renamed = rw.nameToPlaceholder(input).replace(HANDOFF, handoff).replace(UUID, uuid);
		const base = path.posix.basename(renamed);
		const date = TRASH_DATE.exec(base);
		if (!date) return renamed;
		const stamp = date[0].slice(0, -1).replace(/T(\d{2})-(\d{2})-(\d{2})$/, 'T$1:$2:$3.000Z');
		return inWindow(Date.parse(stamp)) || inWindow(Date.parse(stamp) + 999) ? `${path.posix.dirname(renamed) === '.' ? '' : `${path.posix.dirname(renamed)}/`}NOW_${base.slice(date[0].length)}` : renamed;
	};
	return {text, value, name};
}

// ── Trees ─────────────────────────────────────────────────────────────────────────────────────────────────────────

// Content never compared (mode still is): the daemon log has wall-clock lines, pids and timing-dependent order.
const VOLATILE_CONTENT = new Set(['daemon.log']);
const PID_FILE = rel => rel === 'daemon.pid' || /^workers\/[^/]+\.pid$/.test(rel);

function snapshot(side) {
	const home = side.home;
	const norm = side.norm;
	// state.json first: fresh UUIDs are numbered in the order the persisted state lists them.
	const entries = walk(home).sort((a, b) => (a === 'state.json' ? -1 : b === 'state.json' ? 1 : 0));
	const tree = new Map();
	tree.set('./', {mode: `0${(fs.statSync(home).mode & 0o777).toString(8)}`, content: null});
	for (const rel of entries) {
		let stat;
		try { stat = fs.lstatSync(path.join(home, rel)); } catch { continue; }
		const mode = `0${(stat.mode & 0o777).toString(8)}`;
		const kind = stat.isSocket() ? 'socket' : stat.isSymbolicLink() ? 'symlink' : stat.isDirectory() ? 'dir' : 'file';
		let content = null;
		if (kind === 'symlink') content = `-> ${norm.text(fs.readlinkSync(path.join(home, rel)))}`;
		else if (kind === 'file') {
			if (VOLATILE_CONTENT.has(rel)) content = '<volatile>';
			else {
				let raw;
				try { raw = fs.readFileSync(path.join(home, rel), 'utf8'); } catch { continue; }
				content = PID_FILE(rel) ? raw.replace(/^\d+(\n?)$/, '<pid>$1') : norm.text(raw);
			}
		}
		tree.set(norm.name(rel), {mode, kind, content});
	}
	return tree;
}

function lineDiff(a, b, context = 2, max = 30) {
	const left = a.split('\n'), right = b.split('\n');
	let top = 0;
	while (top < left.length && top < right.length && left[top] === right[top]) top++;
	let bottom = 0;
	while (bottom < left.length - top && bottom < right.length - top && left[left.length - 1 - bottom] === right[right.length - 1 - bottom]) bottom++;
	const out = [];
	for (let i = Math.max(0, top - context); i < top; i++) out.push(`  ${left[i]}`);
	for (let i = top; i < left.length - bottom; i++) out.push(`- ${left[i]}`);
	for (let i = top; i < right.length - bottom; i++) out.push(`+ ${right[i]}`);
	for (let i = left.length - bottom; i < Math.min(left.length, left.length - bottom + context); i++) out.push(`  ${left[i]}`);
	return out.length > max ? [...out.slice(0, max), `  … ${out.length - max} more lines`].join('\n') : out.join('\n');
}

function compareTrees(left, right, leftName, rightName) {
	const problems = [];
	for (const rel of [...new Set([...left.keys(), ...right.keys()])].sort()) {
		const a = left.get(rel), b = right.get(rel);
		if (!a) { problems.push(`${rel}: only in ${rightName}`); continue; }
		if (!b) { problems.push(`${rel}: only in ${leftName}`); continue; }
		if (a.mode !== b.mode) problems.push(`${rel}: mode ${leftName} ${a.mode} vs ${rightName} ${b.mode}`);
		if (a.kind !== b.kind) problems.push(`${rel}: kind ${leftName} ${a.kind} vs ${rightName} ${b.kind}`);
		else if (a.content !== b.content) problems.push(`${rel}: content differs (- ${leftName}, + ${rightName})\n${lineDiff(a.content ?? '', b.content ?? '')}`);
	}
	return problems;
}

const sameTree = (a, b) => a.size === b.size && [...a].every(([rel, entry]) => { const other = b.get(rel); return other && other.mode === entry.mode && other.kind === entry.kind && other.content === entry.content; });

// ── Daemons ───────────────────────────────────────────────────────────────────────────────────────────────────────

function envFor(real, cli) {
	const home = path.join(real, sandboxSpec.home);
	fs.mkdirSync(home, {recursive: true});
	fs.mkdirSync(path.join(real, 'tmp'), {recursive: true});
	return {
		PATH: `${path.join(real, sandboxSpec.bin)}:/usr/bin:/bin:/usr/sbin:/sbin`,
		HOME: home,
		DECKHAND_HOME: path.join(real, sandboxSpec.deckhandHome),
		DECKHAND_DEV: '0',
		DECKHAND_AGENT_LATEST: JSON.stringify(sandboxSpec.agentLatest),
		SHELL: '/bin/sh', TERM: 'xterm-256color', LANG: 'en_US.UTF-8', TZ: 'UTC',
		TMPDIR: path.join(real, 'tmp'),
		XDG_CONFIG_HOME: path.join(home, '.config'), XDG_STATE_HOME: path.join(home, '.local', 'state'),
		XDG_CACHE_HOME: path.join(home, '.cache'), XDG_DATA_HOME: path.join(home, '.local', 'share'),
		CLAUDE_CONFIG_DIR: path.join(home, '.claude'), CODEX_HOME: path.join(home, '.codex'),
		GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1',
		TEST_CLI: cli, FAKE_AGENT_STATE: path.join(real, 'agent-state'),
	};
}

function request(home, message, timeoutMs = 20_000) {
	return new Promise(resolve => {
		const requestId = randomUUID();
		const socket = net.createConnection(path.join(home, 'daemon.sock'));
		let buffer = '', done = false;
		const finish = result => { if (done) return; done = true; clearTimeout(timer); socket.destroy(); resolve(result); };
		const timer = setTimeout(() => finish({ok: false, error: 'gate: request timed out'}), timeoutMs);
		socket.on('connect', () => socket.write(`${JSON.stringify({...message, requestId})}\n`));
		socket.on('data', chunk => {
			buffer += chunk.toString('utf8');
			for (let at = buffer.indexOf('\n'); at >= 0; at = buffer.indexOf('\n')) {
				const line = buffer.slice(0, at).trim();
				buffer = buffer.slice(at + 1);
				if (!line) continue;
				let parsed;
				try { parsed = JSON.parse(line); } catch { continue; }
				if (parsed.type === 'response' && parsed.requestId === requestId) finish(parsed.ok ? {ok: true, data: parsed.data} : {ok: false, error: parsed.error});
			}
		});
		socket.on('error', error => finish({ok: false, error: `gate: ${error.message}`}));
		socket.on('close', () => finish({ok: false, error: 'gate: connection closed before response'}));
	});
}

async function waitFor(read, done, timeoutMs, what) {
	const deadline = Date.now() + timeoutMs;
	let last;
	for (;;) {
		try { last = await read(); if (done(last)) return last; } catch {}
		if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}: ${JSON.stringify(last)?.slice(0, 400)}`);
		await sleep(50);
	}
}

/** Starts `impl` ('ts' | 'rs') on `tree`'s sandbox. */
async function startDaemon(tree, impl) {
	const command = impl === 'ts' ? [process.execPath, TS_CLI, '--daemon'] : [RS_BIN, '--daemon'];
	const env = envFor(tree.real, impl === 'ts' ? path.join(tree.real, 'cli', 'deckhand') : RS_BIN);
	const previous = process.umask(0o022);
	const child = spawn(command[0], command.slice(1), {cwd: path.join(tree.real, 'repos', 'app'), env, stdio: ['ignore', 'pipe', 'pipe']});
	process.umask(previous);
	let output = '';
	child.stdout.on('data', data => { output += String(data); });
	child.stderr.on('data', data => { output += String(data); });
	const exited = new Promise(resolve => child.once('exit', (code, signal) => resolve({code, signal})));
	const daemon = {impl, tree, child, exited, output: () => output};
	await waitFor(() => request(tree.home, {type: 'ping'}, 2000), result => result.ok, 30_000, `${impl} daemon on ${tree.name} to answer ping`);
	return daemon;
}

async function stopDaemon(daemon) {
	daemon.child.kill('SIGTERM');
	const result = await Promise.race([daemon.exited, sleep(15_000).then(() => 'timeout')]);
	if (result === 'timeout') { daemon.child.kill('SIGKILL'); return `${daemon.impl} daemon on ${daemon.tree.name} did not stop on SIGTERM\n${daemon.output()}`; }
	if (result.code !== 0) return `${daemon.impl} daemon on ${daemon.tree.name} exited with ${result.code ?? result.signal}\n${daemon.output()}`;
	return undefined;
}

/** Waits until both trees stopped changing (three equal snapshots 250 ms apart). */
async function settle(trees, timeoutMs = 20_000) {
	const deadline = Date.now() + timeoutMs;
	const last = trees.map(() => undefined), stable = trees.map(() => 0);
	while (Date.now() < deadline) {
		trees.forEach((tree, index) => {
			const now = snapshot(tree);
			stable[index] = last[index] && sameTree(last[index], now) ? stable[index] + 1 : 0;
			last[index] = now;
		});
		if (stable.every(count => count >= 3)) return last;
		await sleep(250);
	}
	return last;
}

function killLeftovers(tree) {
	const workers = path.join(tree.home, 'workers');
	let killed = 0;
	try {
		for (const file of fs.readdirSync(workers).filter(name => name.endsWith('.pid'))) {
			const pid = Number.parseInt(fs.readFileSync(path.join(workers, file), 'utf8'), 10);
			if (Number.isFinite(pid) && pid > 0 && pid !== 4194301) { try { process.kill(pid, 'SIGKILL'); killed++; } catch {} }
		}
	} catch {}
	// Fake agents record their pid in agent-state/trace-*.json.
	try {
		for (const file of fs.readdirSync(path.join(tree.real, 'agent-state')).filter(name => name.startsWith('trace-'))) {
			const pid = JSON.parse(fs.readFileSync(path.join(tree.real, 'agent-state', file), 'utf8')).pid;
			if (Number.isFinite(pid) && pid > 0) { try { process.kill(pid, 'SIGKILL'); killed++; } catch {} }
		}
	} catch {}
	return killed;
}

// ── Requests ──────────────────────────────────────────────────────────────────────────────────────────────────────

const S = n => `5e550000-0000-4000-8000-0000000000${n}`;
const MERGED_TASK_TICKED = /- \[x\] Merged feature task <!-- dh:t=7c3d4e5f [^>]*auto=merge/;

/** Every read request type of src/types.ts ClientRequest (side-effect free), plus a few error paths. */
function readRequests(real) {
	const app = path.join(real, 'repos', 'app'), lib = path.join(real, 'repos', 'lib'), plain = path.join(real, 'plain');
	const featureA = path.join(real, 'worktrees', 'app', 'feature-a');
	return [
		{type: 'ping'},
		{type: 'list'},
		{type: 'subscribe', repoRoot: app},
		{type: 'subscribe', repoRoot: lib},
		{type: 'agent-versions'},
		{type: 'list-worktrees', cwd: app},
		{type: 'list-worktrees', cwd: lib},
		{type: 'worktree-overview', cwd: app},
		{type: 'worktree-overview', cwd: lib},
		{type: 'inspect-worktree', cwd: app, path: featureA, deleteBranch: true},
		{type: 'inspect-worktree', cwd: app, path: path.join(real, 'worktrees', 'app', 'attached'), deleteBranch: false},
		{type: 'watch-tasks', cwd: app},
		{type: 'watch-tasks', cwd: lib},
		{type: 'project-info', cwd: app},
		{type: 'project-info', cwd: lib},
		{type: 'project-info', cwd: plain},
		{type: 'settings-info', cwd: app},
		{type: 'settings-info', cwd: lib},
		{type: 'settings-info', cwd: plain},
		{type: 'worktree-candidates', cwd: app},
		{type: 'global-link-candidates', cwd: app},
		{type: 'worktree-candidate-sizes', cwd: app, paths: ['.env', 'README.md', 'src', 'missing']},
		{type: 'list-branches', cwd: app},
		{type: 'list-branches', cwd: lib},
		{type: 'watch-preview', sessionId: S('02'), cols: 80, rows: 24},
		{type: 'workspace-summary', sessionId: S('04'), includePr: false},
		{type: 'workspace-summary', sessionId: S('01'), includePr: false},
		{type: 'inspect-cleanup', sessionId: S('04'), deleteBranch: true},
		{type: 'inspect-cleanup', sessionId: S('07'), deleteBranch: false},
		{type: 'merge-preview', sessionId: S('04'), targetCwd: app},
		{type: 'watch-changes', sessionId: S('04')},
		{type: 'changes-diff', sessionId: S('04'), group: 'unstaged', path: 'src/a.ts'},
		{type: 'workspace-summary', sessionId: 'no-such-session', includePr: false},
		{type: 'list-worktrees', cwd: plain},
		{type: 'list'},
	];
}

// ── Gate ──────────────────────────────────────────────────────────────────────────────────────────────────────────

const report = {startedAt: new Date(RUN_START).toISOString(), checks: []};
let failed = false;
function check(name, problems, detail) {
	const ok = problems.length === 0;
	if (!ok) failed = true;
	report.checks.push({name, ok, ...(ok ? {} : {problems}), ...(detail ? {detail} : {})});
	console.log(`${ok ? 'PASS' : 'FAIL'} ${name}${ok ? '' : `\n${problems.map(problem => `  ${problem.replaceAll('\n', '\n  ')}`).join('\n')}`}`);
	return ok;
}

// worktree-candidate-sizes: TS fills the map as its concurrent size reads finish (src/settingsInfo.ts:275), so its key
// order is timing-dependent; keys are sorted before comparing.
const sortedKeys = value => (value && typeof value === 'object' && !Array.isArray(value) ? Object.fromEntries(Object.entries(value).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))) : value);
function compareResponses(name, leftNorm, left, rightNorm, right, leftName = 'ts', rightName = 'rs', unordered = false) {
	const shape = response => (unordered && response.ok ? {...response, data: sortedKeys(response.data)} : response);
	const a = JSON.stringify(leftNorm.value(shape(left)), null, 2), b = JSON.stringify(rightNorm.value(shape(right)), null, 2);
	return check(name, a === b ? [] : [`response differs (- ${leftName}, + ${rightName})\n${lineDiff(a, b)}`]);
}

function makeTree(name) {
	const real = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'dhg-')));
	if (Buffer.byteLength(path.join(real, sandboxSpec.deckhandHome, 'daemon.sock')) > 103) throw new Error(`$TMPDIR is too long for a Unix socket path under ${real}`);
	buildSandbox(real);
	installHome(path.join(real, sandboxSpec.deckhandHome), rewriter(real));
	return {name, real, home: path.join(real, sandboxSpec.deckhandHome), norm: normaliser(real)};
}

async function bothRead(ts, rs, message) {
	const toTs = translate(message, rs.tree.real, ts.tree.real);
	return Promise.all([request(ts.tree.home, toTs), request(rs.tree.home, message)]);
}
/** The same request with the other sandbox's root. */
const translate = (message, from, to) => JSON.parse(JSON.stringify(message).split(from).join(to));

async function waitForStartupWork(daemon) {
	const home = daemon.tree.home, app = path.join(daemon.tree.real, 'repos', 'app');
	await waitFor(() => request(home, {type: 'list'}), result => result.ok && Boolean(result.data.find(session => session.id === S('06'))?.worktree?.mergedAt), 30_000, `${daemon.impl} merge detection`);
	await waitFor(() => request(home, {type: 'watch-tasks', cwd: app}), result => result.ok && MERGED_TASK_TICKED.test(result.data.text), 30_000, `${daemon.impl} merged task tick`);
}

async function runReads(label, left, right, leftName, rightName) {
	for (const message of readRequests(right.tree.real)) {
		const leftMessage = translate(message, right.tree.real, left.tree.real);
		const [a, b] = await Promise.all([request(left.tree.home, leftMessage), request(right.tree.home, message)]);
		const tag = `${message.type}${message.cwd ? ` ${path.relative(right.tree.real, message.cwd) || '.'}` : ''}${message.repoRoot ? ` ${path.relative(right.tree.real, message.repoRoot)}` : ''}${message.sessionId ? ` ${message.sessionId.slice(-2)}` : ''}${message.path ? ` ${path.basename(message.path)}` : ''}`;
		compareResponses(`${label}: ${tag}`, left.tree.norm, a, right.tree.norm, b, leftName, rightName, message.type === 'worktree-candidate-sizes');
		report.checks.at(-1).detail = {[leftName]: outcome(a), [rightName]: outcome(b)};
		if (label.startsWith('(e)') && !b.ok && a.ok) check(`${label}: ${tag} succeeds on the Rust-written tree`, [b.error]);
	}
}

function compareSnapshots(label, ts, rs, detail) {
	const [a, b] = [snapshot(ts), snapshot(rs)];
	check(label, compareTrees(a, b, ts.name, rs.name), typeof detail === 'function' ? detail(a, b) : detail);
	return a;
}

/** Paths whose entry changed between two snapshots of one tree (evidence that an operation did something). */
const changedPaths = (before, after) => [...new Set([...before.keys(), ...after.keys()])].filter(rel => {
	const a = before.get(rel), b = after.get(rel);
	return !a || !b || a.mode !== b.mode || a.content !== b.content;
}).sort();
const outcome = response => (response.ok ? 'ok' : `error: ${response.error}`);

// A sub-session's title is prefixed with its parent's.
const SUB_TITLE = 'Gate main session / Gate sub session';

// Writing operations, applied to both trees in turn. Each takes the side's context and returns the response to compare.
function operations() {
	const ids = {};
	const app = ctx => path.join(ctx.real, 'repos', 'app');
	const lib = ctx => path.join(ctx.real, 'repos', 'lib');
	const sessionByTitle = async (ctx, title) => (await request(ctx.home, {type: 'list'})).data.find(session => session.title === title);
	const taskId = async (ctx, title) => {
		const doc = (await request(ctx.home, {type: 'watch-tasks', cwd: app(ctx)})).data;
		const line = doc.text.split('\n').find(row => row.includes(title));
		return /t=([0-9a-f]{8})/.exec(line ?? '')?.[1] ?? 'missing';
	};
	const waitReady = async (ctx, title) => {
		const session = await sessionByTitle(ctx, title);
		if (!session) return;
		await waitFor(async () => fs.existsSync(path.join(ctx.real, 'agent-state', `ready-${session.id}-${(await sessionByTitle(ctx, title))?.launchId}`)), Boolean, 20_000, `fake agent of ${title}`).catch(() => {});
		await waitFor(() => sessionByTitle(ctx, title), current => current?.status === 'running' && Boolean(current.agentSessionRef), 10_000, `${title} running`).catch(() => {});
	};
	const create = (title, program, extra = {}) => async ctx => {
		const input = {title, program, cwd: app(ctx), repoRoot: app(ctx), cols: 100, rows: 30, ...extra};
		if (input.parentTitle) { input.parentSessionId = (await sessionByTitle(ctx, input.parentTitle))?.id; delete input.parentTitle; }
		const result = await request(ctx.home, {type: 'create', input}, 60_000);
		await waitReady(ctx, title);
		return result;
	};
	const bySession = (title, build) => async ctx => {
		const session = await sessionByTitle(ctx, title);
		return request(ctx.home, build(session?.id ?? 'missing', session, ctx), 60_000);
	};
	const config = (target, cwd, edit) => async ctx => {
		const info = (await request(ctx.home, {type: 'settings-info', cwd: cwd(ctx)})).data;
		const document = info.targets[target];
		return request(ctx.home, {type: 'save-config', target, cwd: cwd(ctx), raw: edit(document.raw), revision: document.revision});
	};
	const jsonEdit = mutate => raw => { const value = JSON.parse(raw.trim() || '{}'); mutate(value); return `${JSON.stringify(value, null, 2)}\n`; };
	return [
		['save-config global (config.json defaults: add an action, a symlink, a dev command)', config('global', app, jsonEdit(value => { value.actions = {...value.actions, gate: 'echo gate'}; value.worktree = {...value.worktree, symlink: [...value.worktree?.symlink ?? [], '.gate']}; value.devCommand = 'npm run gate'; }))],
		['save-config repository app (deckhand.json edit; re-trust)', config('repository', app, jsonEdit(value => { value.actions = {...value.actions, gate: 'echo repo-gate'}; }))],
		['save-config repository lib (creates deckhand.json)', config('repository', lib, jsonEdit(value => { value.devCommand = 'npm run lib-dev'; }))],
		['trust-project lib', async ctx => {
			const info = (await request(ctx.home, {type: 'project-info', cwd: lib(ctx)})).data;
			return request(ctx.home, {type: 'trust-project', cwd: lib(ctx), fingerprint: info.fingerprint});
		}],
		['create claude session (main checkout)', create('Gate main session', 'claude', {worktreeMode: 'none'})],
		['create codex session (new worktree)', create('Gate worktree', 'codex', {worktreeMode: 'new'})],
		['create pi sub-session (clean)', create('Gate sub session', 'pi', {worktreeMode: 'none', parentTitle: 'Gate main session', subSessionKind: 'clean'})],
		['create claude session (second new worktree)', create('Gate doomed worktree', 'claude', {worktreeMode: 'new'})],
		['reorder-session up', bySession('Gate worktree', id => ({type: 'reorder-session', sessionId: id, direction: 'up'}))],
		['reorder-session down', bySession('Gate worktree', id => ({type: 'reorder-session', sessionId: id, direction: 'down'}))],
		['reorder-session up (fixture session)', async ctx => request(ctx.home, {type: 'reorder-session', sessionId: S('0c'), direction: 'up'})],
		['archive-session S07', async ctx => request(ctx.home, {type: 'archive-session', sessionId: S('07'), archived: true})],
		['unarchive S08', async ctx => request(ctx.home, {type: 'archive-session', sessionId: S('08'), archived: false})],
		['save-note session (Gate main session)', bySession('Gate main session', (id, session) => ({type: 'save-note', sessionId: id, section: 'session', text: 'Gate note\n\n- [ ] gate item one\n- [ ] gate item two\n', revision: session?.notesFile?.revision ?? ''}))],
		['save-note shared (worktree note of Gate worktree)', bySession('Gate worktree', (id, session) => ({type: 'save-note', sessionId: id, section: 'shared', noteId: session?.sharedNotes ? `${session.sharedNotes.kind}:${session.sharedNotes.id}` : undefined, text: 'Worktree note\n- [ ] wt item\n', revision: session?.sharedNotes?.revision ?? ''}))],
		['save-note session S01 (existing note)', async ctx => {
			const session = (await request(ctx.home, {type: 'list'})).data.find(item => item.id === S('01'));
			return request(ctx.home, {type: 'save-note', sessionId: S('01'), section: 'session', text: `${session.notes}\nEdited by the gate.\n`, revision: session.notesFile.revision});
		}],
		['promote-note-item (line 3 of Gate main session note)', bySession('Gate main session', (id, session) => ({type: 'promote-note-item', sessionId: id, section: 'session', line: 3, revision: session?.notesFile?.revision ?? ''}))],
		['send-open-items (Gate main session note)', bySession('Gate main session', (id, session) => ({type: 'send-open-items', sessionId: id, section: 'session', revision: session?.notesFile?.revision ?? ''}))],
		['open-note shared S0e', async ctx => request(ctx.home, {type: 'open-note', sessionId: S('0e'), section: 'shared'})],
		['open-tasks lib', async ctx => request(ctx.home, {type: 'open-tasks', cwd: lib(ctx)})],
		['task-op add', async ctx => request(ctx.home, {type: 'task-op', cwd: app(ctx), op: {type: 'add', title: 'Gate task', body: 'body line'}})],
		['task-op add linked', async ctx => {
			const session = await sessionByTitle(ctx, 'Gate worktree');
			return request(ctx.home, {type: 'task-op', cwd: app(ctx), op: {type: 'add', title: 'Gate linked task', link: {wt: session?.worktree?.id ?? 'missing'}}});
		}],
		['task-op edit', async ctx => request(ctx.home, {type: 'task-op', cwd: app(ctx), op: {type: 'edit', id: await taskId(ctx, 'Gate task'), title: 'Gate task (edited)', body: 'new body'}})],
		['task-op toggle', async ctx => request(ctx.home, {type: 'task-op', cwd: app(ctx), op: {type: 'toggle', id: await taskId(ctx, 'Gate task (edited)')}})],
		['task-op move', async ctx => {
			const doc = (await request(ctx.home, {type: 'watch-tasks', cwd: app(ctx)})).data;
			const first = /t=([0-9a-f]{8})/.exec(doc.text)?.[1] ?? 'missing';
			return request(ctx.home, {type: 'task-op', cwd: app(ctx), op: {type: 'move', id: await taskId(ctx, 'Gate task (edited)'), target: first}});
		}],
		['task-to-note (linked task back into its worktree note)', async ctx => {
			const session = await sessionByTitle(ctx, 'Gate worktree');
			return request(ctx.home, {type: 'task-to-note', cwd: app(ctx), taskId: await taskId(ctx, 'Gate linked task'), section: `wt:${session?.worktree?.id}`});
		}],
		['task-op remove', async ctx => request(ctx.home, {type: 'task-op', cwd: app(ctx), op: {type: 'remove', id: await taskId(ctx, 'Gate task (edited)')}})],
		['set-session-done S0e', async ctx => request(ctx.home, {type: 'set-session-done', sessionId: S('0e'), done: true})],
		['set-session-done S02 false→true→false', async ctx => { await request(ctx.home, {type: 'set-session-done', sessionId: S('02'), done: true}); return request(ctx.home, {type: 'set-session-done', sessionId: S('02'), done: false}); }],
		['mark-session-merged S07 → app', async ctx => request(ctx.home, {type: 'mark-session-merged', sessionId: S('07'), targetCwd: app(ctx)})],
		['export-handoff S02', async ctx => request(ctx.home, {type: 'export-handoff', sessionId: S('02'), includeOutput: false})],
		['kill pi sub-session', bySession(SUB_TITLE, id => ({type: 'kill', sessionId: id}))],
		['remove pi sub-session', bySession(SUB_TITLE, id => ({type: 'remove', sessionId: id}))],
		['kill Gate doomed worktree (keep worktree)', bySession('Gate doomed worktree', id => ({type: 'kill', sessionId: id}))],
		['delete-worktree gate-doomed-worktree (stop sessions, delete branch)', async ctx => {
			const session = await sessionByTitle(ctx, 'Gate doomed worktree');
			return request(ctx.home, {type: 'delete-worktree', cwd: app(ctx), path: session?.worktree?.path ?? 'missing', branch: session?.worktree?.branch, deleteBranch: true, stopSessions: true, allowDataLoss: true}, 60_000);
		}],
		['kill Gate worktree with deleteWorktree+deleteBranch', bySession('Gate worktree', id => ({type: 'kill', sessionId: id, deleteWorktree: true, deleteBranch: true, allowDataLoss: true}))],
		['kill Gate main session', bySession('Gate main session', id => ({type: 'kill', sessionId: id}))],
		['restart Gate main session (resume)', async ctx => {
			const session = await sessionByTitle(ctx, 'Gate main session');
			const result = await request(ctx.home, {type: 'restart', sessionId: session?.id ?? 'missing', cols: 100, rows: 30, mode: 'resume'}, 60_000);
			await waitReady(ctx, 'Gate main session');
			return result;
		}],
		['kill Gate main session again', bySession('Gate main session', id => ({type: 'kill', sessionId: id}))],
		['remove Gate main session (moveOpenItems)', bySession('Gate main session', id => ({type: 'remove', sessionId: id, moveOpenItems: true}))],
		['remove Gate worktree', bySession('Gate worktree', id => ({type: 'remove', sessionId: id}))],
		['remove Gate doomed worktree', bySession('Gate doomed worktree', id => ({type: 'remove', sessionId: id}))],
		['delete-worktree fixture legacy (stopSessions, deleteBranch)', async ctx => request(ctx.home, {type: 'delete-worktree', cwd: app(ctx), path: path.join(ctx.real, 'worktrees', 'app', 'legacy'), branch: 'legacy', deleteBranch: true, stopSessions: true, allowDataLoss: true}, 60_000)],
		['remove S11 (non-git session)', async ctx => request(ctx.home, {type: 'remove', sessionId: S('11')})],
	].map(([name, run]) => ({name, run, ids}));
}

// Unknown keys planted in home-v45 that must survive any rewrite (fixtures/state/MIGRATIONS.md #19, config.json).
const UNKNOWN_KEYS = [
	['state.json', '"futureSessionField"'],
	['state.json', '"futureWorktreeField"'],
	['state.json', '"futureLinksField"'],
	['state.json', '"futureRefField"'],
	['state.json', '"futureSetupField"'],
	['state.json', '"futureRecordField"'],
	['state.json', '"futureAttentionField": "kept on exited sessions"'],
	['config.json', '"futureConfigKey"'],
	['ui-state.json', null],
];

function unknownKeysCheck(label, tree) {
	const problems = [];
	for (const [file, needle] of UNKNOWN_KEYS) {
		const text = fs.readFileSync(path.join(tree.home, file), 'utf8');
		if (needle === null) { if (text !== rewriter(tree.real).toReal(fs.readFileSync(path.join(HOME_FIXTURE, file), 'utf8'))) problems.push(`${file} was rewritten (the daemon must never touch it)`); }
		else if (!text.includes(needle)) problems.push(`${file} lost ${needle}`);
	}
	return check(label, problems);
}

async function main() {
	if (!fs.existsSync(TS_CLI)) throw new Error('dist/cli.js is missing: run `npm run build` first');
	if (!fs.existsSync(RS_BIN)) throw new Error(`${RS_BIN} is missing: run \`cargo build --release -p deckhand\` first`);
	const ts = makeTree('ts-tree'), rs = makeTree('rs-tree');
	report.sandboxes = {ts: ts.real, rs: rs.real};
	console.log(`sandboxes: ts ${ts.real}  rs ${rs.real}`);
	const daemons = [];
	try {
		check('(setup) sandboxes identical before any daemon', compareTrees(snapshot(ts), snapshot(rs), 'ts', 'rs'));

		// (a) + (b): idle start, reads, SIGTERM.
		let tsd = await startDaemon(ts, 'ts'), rsd = await startDaemon(rs, 'rs');
		daemons.push(tsd, rsd);
		await Promise.all([waitForStartupWork(tsd), waitForStartupWork(rsd)]);
		await settle([ts, rs]);
		await runReads('(a) read', tsd, rsd, 'ts', 'rs');
		check('(b) daemons stop cleanly on SIGTERM', [await stopDaemon(tsd), await stopDaemon(rsd)].filter(Boolean));
		compareSnapshots('(b) trees identical after idle start + stop', ts, rs);
		unknownKeysCheck('(d) unknown keys survive the Rust daemon (after idle)', rs);

		// (c) writing operations, trees compared after each.
		tsd = await startDaemon(ts, 'ts'); rsd = await startDaemon(rs, 'rs');
		daemons.push(tsd, rsd);
		await settle([ts, rs]);
		let before = compareSnapshots('(c) trees identical after restart', ts, rs);
		for (const op of operations()) {
			const [a, b] = await Promise.all([op.run(ts), op.run(rs)]);
			console.log(`  op ${op.name}: ts ${outcome(a)} | rs ${outcome(b)}`);
			compareResponses(`(c) ${op.name}: response`, ts.norm, a, rs.norm, b);
			report.checks.at(-1).detail = {ts: outcome(a), rs: outcome(b)};
			await settle([ts, rs]);
			before = compareSnapshots(`(c) ${op.name}: tree`, ts, rs, after => ({changed: changedPaths(before, after)}));
		}
		await runReads('(c) reads after the operations', tsd, rsd, 'ts', 'rs');
		check('(c) daemons stop cleanly on SIGTERM', [await stopDaemon(tsd), await stopDaemon(rsd)].filter(Boolean));
		compareSnapshots('(c) trees identical after stop', ts, rs);
		unknownKeysCheck('(d) unknown keys survive the Rust daemon (after the operations)', rs);

		// (e) the TS daemon on the Rust-written tree vs the TS daemon on the TS-written tree.
		const tsOnTs = await startDaemon(ts, 'ts'), tsOnRs = await startDaemon(rs, 'ts');
		daemons.push(tsOnTs, tsOnRs);
		await settle([ts, rs]);
		compareSnapshots('(e) TS daemon started on both trees: trees identical', ts, rs);
		await runReads('(e) TS on Rust tree', tsOnTs, tsOnRs, 'ts-on-ts', 'ts-on-rs');
		check('(e) TS daemons stop cleanly on SIGTERM', [await stopDaemon(tsOnTs), await stopDaemon(tsOnRs)].filter(Boolean));
		compareSnapshots('(e) trees identical after the TS daemon stopped', ts, rs);
		unknownKeysCheck('(d) unknown keys survive TS reading the Rust tree', rs);
		const log = fs.readFileSync(path.join(rs.home, 'daemon.log'), 'utf8');
		report.rsTreeDaemonLogTail = log.split('\n').slice(-40).join('\n');
	} catch (error) {
		failed = true;
		report.error = error instanceof Error ? error.stack : String(error);
		console.error(report.error);
	} finally {
		for (const daemon of daemons) if (daemon.child.exitCode === null && daemon.child.signalCode === null) { try { daemon.child.kill('SIGKILL'); } catch {} }
		report.leftoverProcessesKilled = killLeftovers(ts) + killLeftovers(rs);
		report.passed = report.checks.filter(item => item.ok).length;
		report.failed = report.checks.filter(item => !item.ok).length;
		report.ok = !failed;
		if (REPORT) fs.writeFileSync(REPORT, `${JSON.stringify(report, null, 2)}\n`);
		if (KEEP) console.log(`sandboxes kept: ${ts.real} ${rs.real}`);
		else for (const tree of [ts, rs]) fs.rmSync(tree.real, {recursive: true, force: true});
		console.log(`\n${report.passed} passed, ${report.failed} failed${REPORT ? ` (report: ${REPORT})` : ''}`);
		process.exitCode = failed ? 1 : 0;
	}
}

await main();
