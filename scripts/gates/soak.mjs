#!/usr/bin/env node
// G8 soak gate: runs the deckhand daemon (release build by default) for SOAK_MINUTES with five output-heavy fake-agent
// sessions in a sandbox repository, a protocol subscriber connected throughout, a second protocol client that attaches,
// detaches and resizes, and a real-PTY UI client for half of the cycles that selects sessions, switches tabs and opens
// sessions full screen. Every SOAK_SAMPLE seconds it records RSS, CPU time, open fds, process state and process-group
// sizes of the daemon and every worker (ps, lsof), then stops the daemon and checks that nothing is left behind.
//
// Isolation (docs/port/CONVENTIONS.md): everything lives under one mkdtemp directory in $TMPDIR (DECKHAND_HOME, HOME,
// the repository, the agents, a private copy of the binary); the real claude/codex/pi are never run (the PATH starts
// with the fake agent, scripts/gates/soak.agent.cjs); ~/.deckhand, ~/.codex, ~/.claude are never touched.
//
// Usage: scripts/gates/soak.sh            (builds the release binary, runs 60 minutes, writes docs/port/gates/G8.md)
//        SOAK_MINUTES=5 SOAK_CYCLE=60 SOAK_SAMPLE=10 SOAK_DOC= node scripts/gates/soak.mjs   (a quick smoke run)
// Env: SOAK_BIN (deckhand binary), SOAK_OUT (raw output directory; default a new directory in $TMPDIR), SOAK_DOC
// (markdown report to write; empty: only <out>/G8.md), SOAK_WARMUP_CYCLES (default 2 = 10 minutes at the default cycle).
// Exit status: 0 when every criterion passes, 1 when one fails, 2 when the run itself broke.
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import net from 'node:net';
import {execFile, spawn, execFileSync} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import {once} from 'node:events';
import {createRequire} from 'node:module';
import {fileURLToPath} from 'node:url';
import {promisify} from 'node:util';

const require = createRequire(import.meta.url);
const execFileP = promisify(execFile);
const here = path.dirname(fileURLToPath(import.meta.url));
const projectRoot = path.resolve(here, '..', '..');
let pty;
try { pty = require('node-pty'); } catch { console.error('soak: node-pty is required for the UI client (npm ci in the repository)'); process.exit(2); }
let HeadlessTerminal;
try { HeadlessTerminal = require('@xterm/headless').Terminal; } catch {}

// ---------------------------------------------------------------------------------------------------------- options
const env = process.env;
const MINUTES = Number(env.SOAK_MINUTES || 60);
const CYCLE = Number(env.SOAK_CYCLE || 300); // seconds; busy then idle
const SAMPLE = Number(env.SOAK_SAMPLE || 30);
const SCALE = CYCLE / 300;
const BUSY_START = 5 * SCALE, IDLE_START = 195 * SCALE;
const CYCLES = Math.round(MINUTES * 60 / CYCLE);
const SAMPLES = Math.round(MINUTES * 60 / SAMPLE);
const WARMUP_CYCLES = Number(env.SOAK_WARMUP_CYCLES || 2);
const BIN = path.resolve(projectRoot, env.SOAK_BIN || 'target/release/deckhand');
const OUT = env.SOAK_OUT ? path.resolve(env.SOAK_OUT) : fs.mkdtempSync(path.join(os.tmpdir(), 'deckhand-soak-out-'));
const DOC = env.SOAK_DOC === undefined ? path.join(projectRoot, 'docs', 'port', 'gates', 'G8.md') : env.SOAK_DOC;
const UI_COLS = 130, UI_ROWS = 36;
// Pass thresholds.
const MEMORY_GROWTH_LIMIT = 0.20;
const IDLE_CPU_LIMIT = 1.0; // percent of one core, averaged over a fully idle sample interval
if (!(CYCLE % SAMPLE === 0 && CYCLES >= WARMUP_CYCLES + 2)) { console.error('soak: SOAK_CYCLE must be a multiple of SOAK_SAMPLE and the run at least warm-up + 2 cycles long'); process.exit(2); }
if (!fs.existsSync(BIN)) { console.error(`soak: ${BIN} missing (cargo build --release -p deckhand)`); process.exit(2); }

// ------------------------------------------------------------------------------------------------------- sandbox
const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'deckhand-soak-')));
const home = path.join(root, 'deckhand-home');
const userHome = path.join(root, 'user-home');
const repo = path.join(root, 'repo');
const agentBin = path.join(root, 'agent-bin');
const bin = path.join(root, 'bin', 'deckhand');
for (const dir of [home, userHome, repo, agentBin, path.dirname(bin), OUT]) fs.mkdirSync(dir, {recursive: true});
fs.copyFileSync(BIN, bin); fs.chmodSync(bin, 0o755);
for (const provider of ['claude', 'codex', 'pi']) {
	fs.copyFileSync(path.join(here, 'soak.agent.cjs'), path.join(agentBin, provider));
	fs.chmodSync(path.join(agentBin, provider), 0o755);
}
const baseEnv = Object.fromEntries(Object.entries(env).filter(([key]) => !key.startsWith('DECKHAND_') && !key.startsWith('SOAK_') && !key.startsWith('CLAUDE') && !key.startsWith('CODEX')));
const runEnv = {
	...baseEnv, HOME: userHome, DECKHAND_HOME: home, DECKHAND_DEV: '0', DECKHAND_AGENT_LATEST: '{}', SHELL: '/bin/sh', TERM: 'xterm-256color',
	PATH: `${agentBin}${path.delimiter}${env.PATH}`, TEST_CLI: bin, CLAUDE_CONFIG_DIR: path.join(userHome, '.claude'), CODEX_HOME: path.join(userHome, '.codex'),
	GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1',
};
const git = (...args) => execFileSync('git', ['-C', repo, ...args], {env: runEnv, stdio: ['ignore', 'pipe', 'pipe']}).toString().trim();
git('init', '-b', 'main'); git('config', 'user.name', 'Deckhand Soak'); git('config', 'user.email', 'soak@example.invalid');
git('config', 'commit.gpgsign', 'false'); git('config', 'core.hooksPath', path.join(repo, '.git', 'no-hooks'));
fs.writeFileSync(path.join(repo, 'file.txt'), 'first\n'); fs.writeFileSync(path.join(repo, 'README.md'), '# soak sandbox\n');
git('add', '.'); git('commit', '-m', 'initial');

const log = (...args) => { const line = `[${new Date().toISOString()}] ${args.join(' ')}`; console.log(line); fs.appendFileSync(path.join(OUT, 'soak.log'), line + '\n'); };
const sleep = ms => new Promise(resolve => setTimeout(resolve, Math.max(0, ms)));
let epoch = 0; // ms; cycle 0 starts here
const sleepUntil = seconds => sleep(epoch + seconds * 1000 - Date.now());
const elapsed = () => (Date.now() - epoch) / 1000;

// --------------------------------------------------------------------------------------------- protocol clients
class Client {
	constructor(name) { this.name = name; this.pending = new Map(); this.counts = {}; this.outputBytes = 0; this.attention = {}; this.closed = false; this.onEvent = null; }
	async open() {
		this.socket = net.createConnection(path.join(home, 'daemon.sock'));
		await once(this.socket, 'connect');
		let buffer = Buffer.alloc(0);
		this.socket.on('data', chunk => {
			buffer = buffer.length ? Buffer.concat([buffer, chunk]) : chunk;
			let start = 0;
			for (let at = buffer.indexOf(10, start); at !== -1; at = buffer.indexOf(10, start)) {
				const text = buffer.subarray(start, at).toString('utf8'); start = at + 1;
				if (text.trim()) this.handle(JSON.parse(text));
			}
			buffer = buffer.subarray(start);
		});
		this.socket.on('error', () => {});
		this.socket.on('close', () => { this.closed = true; for (const {reject} of this.pending.values()) reject(new Error(`${this.name}: connection closed`)); this.pending.clear(); });
		return this;
	}
	handle(message) {
		if (message.type === 'response') {
			const waiter = this.pending.get(message.requestId);
			if (!waiter) return;
			this.pending.delete(message.requestId); clearTimeout(waiter.timer);
			if (message.ok) waiter.resolve(message.data); else waiter.reject(new Error(`${this.name}: ${message.error}`));
			return;
		}
		this.counts[message.type] = (this.counts[message.type] ?? 0) + 1;
		if (typeof message.data === 'string') this.outputBytes += Buffer.byteLength(message.data);
		const event = message.session?.attention?.event;
		if (message.type === 'session-updated' && event) {
			const key = `${message.session.id}:${message.session.attention.at ?? ''}:${event}`;
			if (key !== this.lastAttention?.[message.session.id]) { (this.lastAttention ??= {})[message.session.id] = key; this.attention[event] = (this.attention[event] ?? 0) + 1; }
		}
		this.onEvent?.(message);
	}
	send(message) { if (!this.closed) this.socket.write(JSON.stringify(message) + '\n'); }
	request(message, timeoutMs = 15000) {
		const requestId = randomUUID();
		return new Promise((resolve, reject) => {
			const timer = setTimeout(() => { this.pending.delete(requestId); reject(new Error(`${this.name}: ${message.type} timed out`)); }, timeoutMs);
			this.pending.set(requestId, {resolve, reject, timer});
			this.send({...message, requestId});
		});
	}
	close() { this.socket?.destroy(); }
}
const call = async (message, timeoutMs) => { const client = await new Client('call').open(); try { return await client.request(message, timeoutMs); } finally { client.close(); } };

// ---------------------------------------------------------------------------------------------------- UI client
class Ui {
	constructor() { this.term = null; this.bytes = 0; this.launches = 0; this.keys = 0; this.attached = false; this.screens = []; }
	get running() { return Boolean(this.term); }
	launch() {
		const uiEnv = {...runEnv};
		this.model = HeadlessTerminal ? new HeadlessTerminal({cols: UI_COLS, rows: UI_ROWS, scrollback: 0, allowProposedApi: true}) : null;
		this.term = pty.spawn(bin, [], {name: 'xterm-256color', cwd: repo, env: uiEnv, cols: UI_COLS, rows: UI_ROWS});
		this.pid = this.term.pid; this.launches++; this.attached = false; uiWindows.push([elapsed()]);
		const term = this.term;
		term.onData(data => { this.bytes += Buffer.byteLength(data); this.model?.write(data); });
		this.exited = new Promise(resolve => term.onExit(event => {
			if (this.term === term) { this.term = null; const window = uiWindows.at(-1); if (window && window[1] === undefined) window[1] = elapsed(); }
			resolve(event);
		}));
		log(`ui: launched pid ${this.pid}`);
	}
	press(key) { if (this.term) { this.term.write(key); this.keys++; } }
	async screen() {
		if (!this.model) return '';
		await new Promise(resolve => this.model.write('', resolve));
		const buffer = this.model.buffer.active, rows = [];
		for (let row = 0; row < UI_ROWS; row++) rows.push(buffer.getLine(buffer.viewportY + row)?.translateToString(true) ?? '');
		return rows.join('\n').replace(/\n+$/, '');
	}
	async capture(label) { if (this.term) this.screens.push({label, at: elapsed(), text: await this.screen()}); }
	// Back to the session list on the Preview tab, so an idle stretch starts from the same screen.
	async settle() {
		if (!this.term) return;
		if (this.attached) { this.press('\x1d'); this.attached = false; await sleep(800); }
		this.press('\x1b'); await sleep(300); this.press('p'); await sleep(300);
	}
	async quit() {
		if (!this.term) return;
		const term = this.term;
		await this.settle(); await this.capture('before quit');
		this.press('q');
		const ended = await Promise.race([this.exited, sleep(5000).then(() => null)]);
		if (!ended) { log('ui: did not quit on q; killing'); try { term.kill(); } catch {} await this.exited; }
		log(`ui: exited ${JSON.stringify(ended)}`);
		this.term = null; this.model?.dispose(); this.model = null;
		const window = uiWindows.at(-1); if (window && window[1] === undefined) window[1] = elapsed();
	}
}

// ---------------------------------------------------------------------------------------------------- sampling
const samples = []; // {t, k, cycle, phase, ui, role, label, pid, ppid, pgid, stat, rss, cpuS, cpuPct, fds, group (members of its process group), sub (descendants)}
const totals = []; // {t, k, cycle, phase, ui, procs, zombies, zombiePids, rss}
const seen = new Map(); // pid -> command of every process that belonged to the soak
const prevCpu = new Map(); // pid -> {cpuS, t}
let prevSampleT = 0, daemonPid = 0, daemonChild = null;
const ui = new Ui();
const sessions = []; // {id, title, program, mode}
const uiWindows = []; // [start, end] seconds
const parseCpuTime = text => {
	let days = 0, rest = text;
	if (text.includes('-')) { [days, rest] = text.split('-'); days = Number(days); }
	return days * 86400 + rest.split(':').reduce((sum, part) => sum * 60 + Number(part), 0);
};
async function ps() {
	const {stdout} = await execFileP('ps', ['-A', '-ww', '-o', 'pid=,ppid=,pgid=,stat=,rss=,time=,command='], {maxBuffer: 64 * 1024 * 1024});
	return stdout.split('\n').map(line => line.trim().match(/^(\d+)\s+(\d+)\s+(\d+)\s+(\S+)\s+(\d+)\s+(\S+)\s+(.*)$/)).filter(Boolean)
		.map(([, pid, ppid, pgid, stat, rss, time, command]) => ({pid: +pid, ppid: +ppid, pgid: +pgid, stat, rss: +rss, cpuS: parseCpuTime(time), command}));
}
async function fdCounts(pids) {
	if (!pids.length) return new Map();
	const {stdout} = await execFileP('lsof', ['-n', '-P', '-w', '-p', pids.join(',')], {maxBuffer: 64 * 1024 * 1024}).catch(error => ({stdout: error.stdout ?? ''}));
	const counts = new Map(pids.map(pid => [pid, 0]));
	for (const line of stdout.split('\n').slice(1)) {
		const columns = line.trim().split(/\s+/);
		if (columns.length > 3 && /^\d+/.test(columns[3])) counts.set(+columns[1], (counts.get(+columns[1]) ?? 0) + 1);
	}
	return counts;
}
// macOS phys_footprint (what Activity Monitor shows as Memory): RSS also counts pages malloc has already given back
// with MADV_FREE until the kernel reclaims them, so it swings with load; the footprint does not.
async function footprints(pids) {
	const result = new Map();
	if (process.platform !== 'darwin' || !pids.length) return result;
	const {stdout} = await execFileP('footprint', ['--noCategories', '-f', 'bytes', ...pids.flatMap(pid => ['--pid', String(pid)])], {maxBuffer: 16 * 1024 * 1024}).catch(error => ({stdout: error.stdout ?? ''}));
	let current = 0;
	for (const line of stdout.split('\n')) {
		const header = line.match(/\[(\d+)\]: .*Footprint:/);
		if (header) current = +header[1];
		const value = line.match(/^\s*phys_footprint: (\d+) B/);
		if (value && current) result.set(current, Math.round(+value[1] / 1024));
	}
	return result;
}
const readPid = file => { try { return Number.parseInt(fs.readFileSync(file, 'utf8'), 10) || 0; } catch { return 0; } };
function workerSessions() {
	const map = new Map();
	try { for (const file of fs.readdirSync(path.join(home, 'workers'))) if (file.endsWith('.pid')) map.set(readPid(path.join(home, 'workers', file)), file.slice(0, -4)); } catch {}
	return map;
}
function phaseOf(t0, t1) {
	const cycle = Math.floor((t1 - 0.001) / CYCLE), off0 = t0 - cycle * CYCLE, off1 = t1 - cycle * CYCLE;
	if (off0 >= IDLE_START && off1 <= CYCLE + BUSY_START) return 'idle';
	if (off0 >= BUSY_START && off1 <= IDLE_START) return 'busy';
	return 'mixed';
}
const uiThroughout = (t0, t1) => uiWindows.some(([start, end]) => start <= t0 && (end === undefined || end >= t1)) ? 'on' : uiWindows.some(([start, end]) => start < t1 && (end === undefined || end > t0)) ? 'part' : 'off';
async function sample(k) {
	const t = elapsed();
	const rows = await ps();
	daemonPid = readPid(path.join(home, 'daemon.pid')) || daemonPid;
	const bySid = workerSessions(), titles = new Map(sessions.map(session => [session.id, session.title]));
	const children = new Map();
	for (const row of rows) { if (!children.has(row.ppid)) children.set(row.ppid, []); children.get(row.ppid).push(row); }
	const rootsPids = new Set([process.pid, daemonPid, ...rows.filter(row => row.command.startsWith(bin)).map(row => row.pid)]);
	const tree = new Map();
	const visit = pid => { for (const child of children.get(pid) ?? []) if (!tree.has(child.pid)) { tree.set(child.pid, child); visit(child.pid); } };
	for (const row of rows) if (rootsPids.has(row.pid) && row.pid !== process.pid) tree.set(row.pid, row);
	for (const pid of rootsPids) visit(pid);
	// The sampler's own ps/lsof are not the soak's.
	for (const [pid, row] of tree) if (row.ppid === process.pid && /^(ps|lsof|\/bin\/ps|\/usr\/sbin\/lsof)\b/.test(row.command)) tree.delete(pid);
	const owner = pid => { for (let row = tree.get(pid); row; row = tree.get(row.ppid)) { const sid = bySid.get(row.pid); if (sid) return titles.get(sid) ?? sid.slice(0, 8); } return ''; };
	const roleOf = row => row.pid === daemonPid ? 'daemon' : row.command.includes('--session-worker') ? 'worker' : row.command.includes('--workspace-worker') ? 'ws-worker'
		: row.pid === ui.pid && ui.running ? 'ui' : row.command.includes(agentBin) ? 'agent' : row.command.startsWith(`${bin} hook`) ? 'hook' : row.ppid === process.pid ? 'harness-child' : 'other';
	const measured = [...tree.values()].filter(row => ['daemon', 'worker', 'ws-worker', 'ui', 'agent'].includes(roleOf(row)));
	const fds = await fdCounts(measured.map(row => row.pid));
	const footprint = await footprints(measured.map(row => row.pid));
	const phase = k === 0 ? 'start' : phaseOf(prevSampleT, t), uiState = k === 0 ? (ui.running ? 'on' : 'off') : uiThroughout(prevSampleT, t);
	const cycle = Math.floor((t - 0.001) / CYCLE);
	const pgCount = pgid => rows.filter(row => row.pgid === pgid && tree.has(row.pid)).length;
	const descendants = pid => (children.get(pid) ?? []).filter(child => tree.has(child.pid)).reduce((sum, child) => sum + 1 + descendants(child.pid), 0);
	let zombies = [], rss = 0;
	for (const row of tree.values()) {
		seen.set(row.pid, row.command);
		rss += row.rss;
		if (row.stat.startsWith('Z')) zombies.push(`${row.pid}(${row.ppid})`);
		const role = roleOf(row);
		if (!['daemon', 'worker', 'ws-worker', 'ui', 'agent'].includes(role)) continue;
		const prev = prevCpu.get(row.pid);
		const cpuPct = prev && t > prev.t ? (row.cpuS - prev.cpuS) / (t - prev.t) * 100 : null;
		prevCpu.set(row.pid, {cpuS: row.cpuS, t});
		const label = role === 'daemon' ? 'daemon' : role === 'ui' ? `ui#${ui.launches}` : role === 'ws-worker' ? `ws:${row.pid}` : owner(row.pid) || bySid.get(row.pid) || '?';
		samples.push({t, k, cycle, phase, ui: uiState, role, label, pid: row.pid, ppid: row.ppid, pgid: row.pgid, stat: row.stat, rss: row.rss, foot: footprint.get(row.pid) ?? null, cpuS: row.cpuS, cpuPct, fds: fds.get(row.pid) ?? null, group: pgCount(row.pgid), sub: descendants(row.pid)});
	}
	totals.push({t, k, cycle, phase, ui: uiState, procs: tree.size, zombies: zombies.length, zombiePids: zombies.join(' '), rss, others: [...tree.values()].filter(row => !['daemon', 'worker', 'ws-worker', 'ui', 'agent'].includes(roleOf(row))).map(row => row.command.split(' ')[0].split('/').pop()).join(' ')});
	prevSampleT = t;
	const daemonRow = samples.findLast(row => row.k === k && row.role === 'daemon');
	log(`sample ${k}/${SAMPLES} t=${t.toFixed(1)}s ${phase} ui=${uiState} procs=${tree.size} zombies=${zombies.length} daemon rss=${daemonRow?.rss}KB cpu=${daemonRow?.cpuPct?.toFixed(2)}% fds=${daemonRow?.fds}`);
}

// --------------------------------------------------------------------------------------------------- the drivers
const cycleStart = cycle => cycle * CYCLE;
async function uiTrack() {
	const steps = [
		['select', () => ui.press(String(1 + (ui.keys % 5)))],
		['next tab', () => ui.press('\t')],
		['open full screen', () => { ui.press('p'); setTimeout(() => { ui.press('\r'); ui.attached = true; }, 300); }],
		['back from full screen', () => { if (ui.attached) { ui.press('\x1d'); ui.attached = false; } }],
		['terminal tab', () => ui.press('t')],
		['git tab', () => ui.press('g')],
		['notes tab', () => ui.press('a')],
		['next tab ]', () => ui.press(']')],
		['previous tab [', () => ui.press('[')],
		['preview, scroll', () => { ui.press('p'); setTimeout(() => { ui.press('\x1b[C'); for (let i = 0; i < 3; i++) ui.press('k'); ui.press('\x1b'); }, 300); }],
	];
	let step = 0;
	for (let cycle = 0; cycle < CYCLES && !stopping; cycle++) {
		await sleepUntil(cycleStart(cycle) + BUSY_START + 5 * SCALE);
		if (stopping) return;
		if (cycle % 2 === 0) { ui.launch(); await sleep(2000); await ui.capture(`cycle ${cycle} start`); }
		const until = cycleStart(cycle) + (cycle % 2 === 0 ? IDLE_START - 10 * SCALE : 100 * SCALE);
		while (!stopping && elapsed() + 12 * SCALE < until && ui.running) {
			const [name, action] = steps[step++ % steps.length];
			action();
			if (cycle < 2 && [3, 5, 6, 7].includes(step % steps.length)) await sleep(1500).then(() => ui.capture(`cycle ${cycle}: after ${name}`));
			await sleepUntil(Math.min(until, elapsed() + 12 * SCALE));
		}
		if (cycle % 2 === 1 || cycle === CYCLES - 1) await ui.quit();
		else { await ui.settle(); await ui.capture(`cycle ${cycle} before idle`); }
	}
}
// Attaches to one session after another (with resizes), like a UI opening sessions full screen, during busy stretches.
const churnStats = {attaches: 0, refused: 0, failed: 0};
// The main checkout and the two new worktrees: at most one workspace worker each.
const WORKSPACES = 3;
async function churnTrack(client) {
	let current = null, n = 0;
	const detach = () => { if (current) { client.send({type: 'detach', sessionId: current}); current = null; } };
	for (let cycle = 0; cycle < CYCLES && !stopping; cycle++) {
		await sleepUntil(cycleStart(cycle) + BUSY_START + 3 * SCALE);
		while (!stopping && elapsed() < cycleStart(cycle) + IDLE_START - 8 * SCALE) {
			detach();
			const session = sessions[n++ % sessions.length];
			try { await client.request({type: 'attach', sessionId: session.id, cols: 100, rows: 30}); current = session.id; churnStats.attaches++; } catch (error) {
				// The UI holds that session full screen: attaching is exclusive, so this is the expected refusal.
				if (/already attached/.test(error.message)) churnStats.refused++; else { churnStats.failed++; log(`churn: attach failed: ${error.message}`); }
			}
			await sleep(5000 * SCALE);
			if (current && n % 2 === 0) client.send({type: 'resize', sessionId: current, cols: 80 + (n % 3) * 20, rows: 24 + (n % 2) * 12});
			await sleep(5000 * SCALE);
		}
		detach();
	}
}
// The subscriber: subscribed for the whole run; switches what it watches (Preview / Git / Terminal of a session),
// as the UI's tabs do, during busy stretches and stays on one session's preview while idle.
async function watchTrack(client) {
	let n = 0;
	const kinds = ['watch-preview', 'watch-preview', 'watch-git', 'watch-terminal'];
	for (let cycle = 0; cycle < CYCLES && !stopping; cycle++) {
		await sleepUntil(cycleStart(cycle) + BUSY_START + 1 * SCALE);
		while (!stopping && elapsed() < cycleStart(cycle) + IDLE_START - 6 * SCALE) {
			const kind = kinds[n % kinds.length], session = sessions[Math.floor(n / kinds.length) % sessions.length]; n++;
			try { await client.request({type: kind, sessionId: session.id, cols: 120, rows: 30}); } catch (error) { log(`subscriber: ${kind} failed: ${error.message}`); }
			await sleep(15000 * SCALE);
		}
		try { await client.request({type: 'watch-git', cols: 120, rows: 30}); await client.request({type: 'watch-terminal', cols: 120, rows: 30}); await client.request({type: 'watch-preview', sessionId: sessions[0].id, cols: 120, rows: 30}); } catch (error) { log(`subscriber: idle watch failed: ${error.message}`); }
	}
}

// ------------------------------------------------------------------------------------------------------- the run
let stopping = false;
const cleanup = {};
async function run() {
	log(`soak: ${MINUTES} min, cycle ${CYCLE}s (busy ${BUSY_START}-${IDLE_START}s), sample every ${SAMPLE}s, root ${root}, out ${OUT}`);
	const daemonLog = fs.openSync(path.join(OUT, 'daemon.stderr.log'), 'a');
	// detached (setsid), as the CLI's autostart does: the daemon leads its own group, which its workers share.
	daemonChild = spawn(bin, ['--daemon'], {cwd: repo, env: runEnv, stdio: ['ignore', daemonLog, daemonLog], detached: true});
	for (const deadline = Date.now() + 15000; ; await sleep(100)) {
		try { if ((await call({type: 'ping'}, 2000))?.ok !== false) break; } catch { if (Date.now() > deadline) throw new Error('daemon never answered ping'); }
	}
	daemonPid = daemonChild.pid;
	epoch = Date.now() + 15000;
	fs.writeFileSync(path.join(agentBin, 'soak.json'), JSON.stringify({epoch, cycle: CYCLE, busyStart: BUSY_START, idleStart: IDLE_START}));
	const subscriber = await new Client('subscriber').open();
	await subscriber.request({type: 'subscribe', repoRoot: repo});
	const plan = [['soak-1', 'claude', 'none'], ['soak-2', 'claude', 'new'], ['soak-3', 'codex', 'none'], ['soak-4', 'pi', 'new'], ['soak-5', 'claude', 'none']];
	for (const [title, program, worktreeMode] of plan) {
		const record = await call({type: 'create', input: {title, program, cwd: repo, repoRoot: repo, cols: 120, rows: 32, worktreeMode}}, 60000);
		sessions.push({id: record.id, title, program, mode: worktreeMode});
		log(`created ${title} (${program}, ${worktreeMode}) ${record.id}`);
	}
	for (const deadline = Date.now() + 30000; ; await sleep(250)) {
		const list = await call({type: 'list'});
		const ready = sessions.every(session => list.find(item => item.id === session.id)?.status === 'running' && fs.existsSync(path.join(home, `soak-agent-${session.id}.json`)));
		if (ready) break;
		if (Date.now() > deadline) throw new Error(`sessions never all ran: ${JSON.stringify(list.map(item => [item.title, item.status, item.error]))}`);
	}
	await subscriber.request({type: 'watch-preview', sessionId: sessions[0].id, cols: 120, rows: 30});
	const churn = await new Client('churn').open();
	log(`all sessions running; cycle 0 starts in ${((epoch - Date.now()) / 1000).toFixed(1)}s`);
	await sample(0);
	const tracks = [uiTrack(), churnTrack(churn), watchTrack(subscriber)].map(track => track.catch(error => { log(`track failed: ${error.stack}`); cleanup.trackErrors = (cleanup.trackErrors ?? 0) + 1; }));
	for (let k = 1; k <= SAMPLES; k++) { await sleepUntil(k * SAMPLE); await sample(k); }
	stopping = true;
	await ui.quit();
	await Promise.race([Promise.all(tracks), sleep(20000)]);
	cleanup.subscriber = {connected: !subscriber.closed, events: subscriber.counts, outputBytes: subscriber.outputBytes, attention: subscriber.attention};
	cleanup.churn = {connected: !churn.closed, events: churn.counts, outputBytes: churn.outputBytes, ...churnStats};
	cleanup.ui = {launches: ui.launches, keys: ui.keys, bytes: ui.bytes};
	cleanup.finalList = (await call({type: 'list'})).map(item => ({title: item.title, status: item.status, attention: item.attention?.state, event: item.attention?.event}));
	subscriber.close(); churn.close();
	await stopAll();
}
async function stopAll() {
	// The stop: SIGTERM, as `stop()` in tests/helpers.ts and a logout do; the daemon kills its workers itself.
	const groups = new Set(samples.filter(row => ['worker', 'ws-worker', 'agent'].includes(row.role)).map(row => row.pgid));
	if (ui.running) await ui.quit().catch(() => {});
	const started = Date.now();
	if (daemonChild && daemonChild.exitCode === null && daemonChild.signalCode === null) {
		const ended = once(daemonChild, 'exit');
		daemonChild.kill('SIGTERM');
		const result = await Promise.race([ended, sleep(15000).then(() => null)]);
		cleanup.daemonExit = result ? {code: result[0], signal: result[1], ms: Date.now() - started} : 'timeout (SIGKILL)';
		if (!result) { daemonChild.kill('SIGKILL'); await ended; }
	}
	await sleep(3000);
	const rows = await ps();
	const ownGroup = rows.find(row => row.pid === process.pid)?.pgid;
	const leftovers = rows.filter(row => row.pid !== process.pid && row.pgid !== ownGroup && ((seen.get(row.pid) === row.command) || row.command.includes(root) || groups.has(row.pgid)) && !/^(ps|lsof)\b/.test(row.command));
	cleanup.leftovers = leftovers.map(row => `${row.pid} ${row.stat} ${row.command.slice(0, 120)}`);
	cleanup.files = ['daemon.sock', 'daemon.pid'].filter(file => fs.existsSync(path.join(home, file)));
	try { cleanup.workerPidFiles = fs.readdirSync(path.join(home, 'workers')).filter(file => file.endsWith('.pid')); } catch { cleanup.workerPidFiles = []; }
	log(`stopped: ${JSON.stringify(cleanup.daemonExit)} leftovers=${leftovers.length} files=${cleanup.files} workerPidFiles=${cleanup.workerPidFiles}`);
	for (const row of leftovers) { try { process.kill(row.pid, 'SIGKILL'); } catch {} }
	// Keep the logs, drop the sandbox.
	fs.mkdirSync(path.join(OUT, 'logs'), {recursive: true});
	let workerLogs = [];
	try { workerLogs = fs.readdirSync(path.join(home, 'workers')).filter(file => file.endsWith('.log')).map(file => path.join('workers', file)); } catch {}
	for (const file of ['daemon.log', ...workerLogs]) {
		try { fs.copyFileSync(path.join(home, file), path.join(OUT, 'logs', file.replace('/', '-'))); } catch {}
	}
	await fsp.rm(root, {recursive: true, force: true});
}

// --------------------------------------------------------------------------------------------------- the verdict
const median = values => { const sorted = [...values].sort((a, b) => a - b); return sorted.length ? (sorted.length % 2 ? sorted[(sorted.length - 1) / 2] : (sorted[sorted.length / 2 - 1] + sorted[sorted.length / 2]) / 2) : NaN; };
const max = values => values.length ? Math.max(...values) : NaN;
const mb = kb => (kb / 1024).toFixed(1);
const fmt = (value, digits = 2) => value === null || value === undefined || Number.isNaN(value) ? '' : Number(value).toFixed(digits);
function slopeMbPerHour(points) {
	if (points.length < 2) return NaN;
	const mx = points.reduce((s, [x]) => s + x, 0) / points.length, my = points.reduce((s, [, y]) => s + y, 0) / points.length;
	const num = points.reduce((s, [x, y]) => s + (x - mx) * (y - my), 0), den = points.reduce((s, [x]) => s + (x - mx) ** 2, 0);
	return den ? num / den * 3600 / 1024 : NaN;
}
function verdict() {
	const checks = [];
	const A = row => row.cycle >= WARMUP_CYCLES && row.cycle < WARMUP_CYCLES + 2; // the 10 minutes right after warm-up
	const B = row => row.cycle >= CYCLES - 2 && row.k > 0; // the last 10 minutes
	const longLived = [...new Set(samples.filter(row => ['daemon', 'worker', 'agent'].includes(row.role)).map(row => `${row.role}|${row.label}|${row.pid}`))];
	const memoryRows = [];
	let memoryOk = true, fdOk = true, groupOk = true;
	for (const key of longLived) {
		const [role, label, pid] = key.split('|');
		const rows = samples.filter(row => row.pid === +pid && row.role === role);
		const a = rows.filter(A), b = rows.filter(B);
		if (!a.length || !b.length) { memoryOk = false; memoryRows.push({role, label, note: 'not present for the whole run'}); continue; }
		const growthMax = max(b.map(row => row.rss)) / max(a.map(row => row.rss)) - 1, growthMedian = median(b.map(row => row.rss)) / median(a.map(row => row.rss)) - 1;
		const feet = rows => rows.map(row => row.foot).filter(value => value !== null);
		const footA = median(feet(a)), footB = median(feet(b)), footGrowth = footB / footA - 1, footMaxA = max(feet(a)), footMaxB = max(feet(b));
		const idleFdsA = max(a.filter(row => row.phase === 'idle').map(row => row.fds)), idleFdsB = max(b.filter(row => row.phase === 'idle').map(row => row.fds));
		// Idle samples after warm-up: the daemon's group (daemon, session and workspace workers), a worker's descendants
		// (its agent; hooks only while busy), an agent's own group.
		const idlePost = post => post.filter(row => row.phase === 'idle');
		const groupOf = row => role === 'daemon' || role === 'agent' ? row.group : row.sub;
		const groupA = max(idlePost(a).map(groupOf)), groupB = max(idlePost(b).map(groupOf)), groupMax = max(idlePost(rows.filter(row => row.cycle >= WARMUP_CYCLES)).map(groupOf));
		const post = rows.filter(row => row.cycle >= WARMUP_CYCLES);
		const memOk = growthMedian <= MEMORY_GROWTH_LIMIT, fOk = role === 'agent' || idleFdsB <= idleFdsA, gOk = groupMax <= (role === 'daemon' ? 1 + sessions.length + WORKSPACES : 1);
		groupOk &&= gOk;
		if (role !== 'agent') { memoryOk &&= memOk; fdOk &&= fOk; }
		memoryRows.push({role, label, pid, warmEnd: rows.filter(row => row.cycle < WARMUP_CYCLES).at(-1)?.rss, aMax: max(a.map(row => row.rss)), bMax: max(b.map(row => row.rss)), aMed: median(a.map(row => row.rss)), bMed: median(b.map(row => row.rss)), growthMax, growthMedian, footA, footB, footGrowth, footMaxA, footMaxB, slope: slopeMbPerHour(post.filter(row => row.phase === 'idle').map(row => [row.t, row.rss])), idleFdsA, idleFdsB, groupA, groupB, groupMax, memOk, fOk, gOk});
	}
	checks.push({name: 'Memory bounded', criterion: `daemon and every session worker: median RSS of the last ${2 * CYCLE / 60} min ≤ ${MEMORY_GROWTH_LIMIT * 100}% above the median of the ${2 * CYCLE / 60} min after warm-up (max RSS and phys_footprint reported below)`, ok: memoryOk,
		measured: memoryRows.filter(row => row.role !== 'agent').map(row => row.note ? `${row.label}: ${row.note}` : `${row.label} ${row.growthMedian >= 0 ? '+' : ''}${(row.growthMedian * 100).toFixed(1)}%`).join(', ')});
	const zombieSamples = totals.filter(row => row.zombies > 0);
	const persistent = [];
	for (let i = 1; i < totals.length; i++) for (const pid of totals[i].zombiePids.split(' ').filter(Boolean)) if (totals[i - 1].zombiePids.split(' ').includes(pid)) persistent.push(pid);
	checks.push({name: 'No zombies', criterion: 'no process of the soak in state Z in any sample', ok: zombieSamples.length === 0,
		measured: zombieSamples.length ? `${zombieSamples.length} samples with zombies (${zombieSamples.map(row => `t=${row.t.toFixed(0)}s: ${row.zombiePids}`).join('; ')}); persisting across samples: ${persistent.length ? persistent.join(' ') : 'none'}` : `0 in ${totals.length} samples`});
	checks.push({name: 'No fd growth', criterion: 'daemon and every worker: max open fds in idle samples of the last 10 min ≤ the 10 min after warm-up', ok: fdOk,
		measured: memoryRows.filter(row => row.role !== 'agent' && !row.note).map(row => `${row.label} ${row.idleFdsA}→${row.idleFdsB}`).join(', ')});
	checks.push({name: 'Process groups bounded', criterion: `idle samples after warm-up: the daemon's group (daemon + session workers + workspace workers) ≤ ${1 + sessions.length + WORKSPACES}; each session worker has at most one descendant (its agent); each agent's group is the agent alone`, ok: groupOk,
		measured: memoryRows.filter(row => !row.note).map(row => `${row.role === 'daemon' ? 'daemon group' : `${row.label} ${row.role === 'worker' ? 'descendants' : 'agent group'}`} max ${row.groupMax}`).join(', ')});
	const idle = samples.filter(row => row.phase === 'idle' && row.cycle >= 1 && row.cpuPct !== null);
	const idleBy = role => idle.filter(row => row.role === role);
	const idleWorst = rows => rows.reduce((worst, row) => row.cpuPct > (worst?.cpuPct ?? -1) ? row : worst, null);
	const idleOk = ['daemon', 'worker', 'ws-worker'].every(role => idleBy(role).every(row => row.cpuPct <= IDLE_CPU_LIMIT));
	checks.push({name: 'Idle CPU near 0%', criterion: `daemon, session and workspace workers: CPU ≤ ${IDLE_CPU_LIMIT}% of a core over every fully idle 30 s interval (from cycle 1)`, ok: idleOk && idle.length > 0,
		measured: ['daemon', 'worker', 'ws-worker', 'agent', 'ui'].map(role => { const rows = idleBy(role), worst = idleWorst(rows); return rows.length ? `${role} mean ${fmt(rows.reduce((s, row) => s + row.cpuPct, 0) / rows.length)}% max ${fmt(worst.cpuPct)}% (${worst.label}, t=${worst.t.toFixed(0)}s)` : `${role}: no idle samples`; }).join('; ')});
	const cleanOk = cleanup.leftovers?.length === 0 && cleanup.files?.length === 0 && cleanup.workerPidFiles?.length === 0 && typeof cleanup.daemonExit === 'object';
	checks.push({name: 'Clean stop', criterion: 'after SIGTERM to the daemon (+3 s): no process of the soak left, socket, pid and worker pid files removed', ok: cleanOk,
		measured: `daemon exit ${JSON.stringify(cleanup.daemonExit)}; leftovers ${cleanup.leftovers?.length ?? '?'}${cleanup.leftovers?.length ? ` (${cleanup.leftovers.join('; ')})` : ''}; files left ${JSON.stringify(cleanup.files)} ${JSON.stringify(cleanup.workerPidFiles)}`});
	const runOk = !cleanup.trackErrors && !churnStats.failed && cleanup.subscriber?.connected && samples.length > 0;
	checks.push({name: 'Run completed', criterion: `${MINUTES} min, ${SAMPLES} samples, subscriber connected to the end, no driver errors`, ok: Boolean(runOk),
		measured: `${totals.length - 1} samples; subscriber connected: ${cleanup.subscriber?.connected}; driver errors: ${cleanup.trackErrors ?? 0}; churn attaches ${churnStats.attaches} (refused while the UI held the session: ${churnStats.refused}, failed: ${churnStats.failed})`});
	return {checks, memoryRows, ok: checks.every(check => check.ok)};
}
function csv() {
	const header = 't_s,cycle,phase,ui,role,label,pid,ppid,pgid,stat,rss_kb,footprint_kb,cpu_s,cpu_pct,fds,group_procs,descendants';
	const lines = samples.map(row => [row.t.toFixed(1), row.cycle, row.phase, row.ui, row.role, row.label, row.pid, row.ppid, row.pgid, row.stat, row.rss, row.foot ?? '', row.cpuS.toFixed(2), fmt(row.cpuPct), row.fds ?? '', row.group, row.sub].join(','));
	const totalLines = totals.map(row => [row.t.toFixed(1), row.cycle, row.phase, row.ui, 'total', `zombies=${row.zombies}${row.zombiePids ? ` [${row.zombiePids}]` : ''}`, '', '', '', '', row.rss, '', '', '', '', row.procs, ''].join(','));
	return [header, ...[...lines, ...totalLines].sort((a, b) => parseFloat(a) - parseFloat(b) || (a.includes(',total,') ? 1 : 0) - (b.includes(',total,') ? 1 : 0))].join('\n') + '\n';
}
function bar(value, scaleMax, width = 20) { const n = scaleMax > 0 ? Math.round(value / scaleMax * width) : 0; return '█'.repeat(Math.max(0, Math.min(width, n))) || '▏'; }
function markdown(result) {
	const rev = (() => { try { return execFileSync('git', ['-C', projectRoot, 'rev-parse', '--short', 'HEAD']).toString().trim(); } catch { return '?'; } })();
	const host = `${os.type()} ${os.release()} ${os.arch()}, ${os.cpus().length} × ${os.cpus()[0]?.model ?? '?'}, ${(os.totalmem() / 2 ** 30).toFixed(0)} GiB`;
	const out = [];
	out.push(`# G8 — soak (${MINUTES} min, ${path.basename(path.dirname(BIN))} build)`, '');
	out.push(`**Verdict: ${result.ok ? 'PASS' : 'FAIL'}** — run ${new Date(epoch).toISOString()}, ${MINUTES} min, commit \`${rev}\`, \`${path.relative(projectRoot, BIN)}\`, ${host}.`, '');
	out.push('Generated by `scripts/gates/soak.sh` (driver `scripts/gates/soak.mjs`, fake agent `scripts/gates/soak.agent.cjs`); rerun it to regenerate this file.', '');
	out.push('## Setup', '');
	out.push(`- Sandbox: one \`mkdtemp\` directory in \`$TMPDIR\` holding \`DECKHAND_HOME\`, \`HOME\`, a fresh git repository and a private copy of the binary; removed afterwards. \`~/.deckhand\`, \`~/.codex\`, \`~/.claude\` untouched; the real agents never run (\`PATH\` starts with the fake one).`);
	out.push(`- 5 sessions: ${sessions.map(session => `${session.title} (${session.program}, worktree ${session.mode})`).join(', ')}.`);
	out.push(`- Fake agent: ${CYCLES} cycles of ${CYCLE} s — busy ${BUSY_START}–${IDLE_START} s (16 colored 256-color lines with background, a cursor-save/up/clear/restore status line every 40 ms, a full-screen redraw every 3 s, absolute cursor jumps, two 1 MiB bursts per session per cycle at staggered times, PreToolUse/PostToolUse hooks every 25 s, UserPromptSubmit at the start and Stop at the end, all through \`$TEST_CLI hook\`), then idle until the next cycle (no output, one timer).`);
	out.push(`- Subscriber: one protocol connection for the whole run: \`subscribe\` to the repository, and during busy stretches it switches \`watch-preview\`/\`watch-git\`/\`watch-terminal\` between sessions every 15 s (the protocol side of switching tabs).`);
	out.push(`- Churn client: a second connection that, during busy stretches, \`attach\`es to the next session every 10 s (resizing every other time) and \`detach\`es.`);
	out.push(`- UI client: \`deckhand\` in a real PTY (node-pty, ${UI_COLS}×${UI_ROWS}) launched in even cycles and quit with \`q\` 100 s into the following odd cycle (so half of the idle stretches have a UI attached); every 12 s of busy time it selects a session (1–5), switches tabs (Tab, \`]\`, \`[\`, \`t\`, \`g\`, \`a\`, \`p\`), scrolls the preview, or opens the agent full screen (Enter) and returns (Ctrl+]). ${ui.launches} launches, ${ui.keys} keys, ${(ui.bytes / 2 ** 20).toFixed(1)} MiB of UI output.`);
	out.push(`- Sampling every ${SAMPLE} s (\`ps -A -o pid,ppid,pgid,stat,rss,time,command\`, \`lsof -n -P -p\` counting numeric fds) over the daemon's and the harness's process trees and every process running the binary: RSS, macOS \`phys_footprint\` (\`footprint --pid\`), CPU time (CPU % = CPU-time delta / wall-time delta of the interval), open fds, state, the members of its process group (\`group_procs\`: the daemon's group holds the daemon and its workers; an agent leads its own) and its descendants (a session worker's: its agent).`);
	out.push(`- Phases: an interval is \`idle\` when it lies entirely inside an idle stretch (the first idle interval starts 15 s after the agents stop), \`busy\` when entirely inside a busy one, \`mixed\` otherwise. Warm-up: the first ${WARMUP_CYCLES} cycles (${WARMUP_CYCLES * CYCLE / 60} min); "after warm-up" = minutes ${WARMUP_CYCLES * CYCLE / 60}–${(WARMUP_CYCLES + 2) * CYCLE / 60}, "end" = the last ${2 * CYCLE / 60} min.`);
	out.push('', '## Criteria', '', '| Criterion | Rule | Measured | Result |', '|---|---|---|---|');
	for (const check of result.checks) out.push(`| ${check.name} | ${check.criterion} | ${check.measured.replace(/\|/g, '\\|')} | ${check.ok ? 'PASS' : '**FAIL**'} |`);
	out.push('', '## Memory (RSS) per long-lived process', '', '| Process | pid | RSS end of warm-up | after warm-up: median / max | end: median / max | RSS growth (median) | RSS growth (max) | footprint after warm-up → end: median (max) | footprint growth (median) | idle-sample RSS trend after warm-up | idle fds after warm-up → end | idle group / descendants (after warm-up → end) |', '|---|---|---|---|---|---|---|---|---|---|---|---|');
	for (const row of result.memoryRows) {
		if (row.note) { out.push(`| ${row.role} ${row.label} | | ${row.note} | | | | | | | | | |`); continue; }
		out.push(`| ${row.role} ${row.label} | ${row.pid} | ${mb(row.warmEnd ?? NaN)} MiB | ${mb(row.aMed)} / ${mb(row.aMax)} MiB | ${mb(row.bMed)} / ${mb(row.bMax)} MiB | ${(row.growthMedian * 100).toFixed(1)}% | ${(row.growthMax * 100).toFixed(1)}% | ${mb(row.footA)} (${mb(row.footMaxA)}) → ${mb(row.footB)} (${mb(row.footMaxB)}) MiB | ${(row.footGrowth * 100).toFixed(1)}% | ${fmt(row.slope, 1)} MiB/h | ${row.role === 'agent' ? '' : `${row.idleFdsA} → ${row.idleFdsB}`} | ${row.groupA} → ${row.groupB} |`);
	}
	// Chart-as-table: one row per sample, daemon RSS with a bar, worker RSS, fds and CPU.
	const workers = sessions.map(session => session.title);
	const ks = [...new Set(samples.map(row => row.k))];
	const daemonMax = max(samples.filter(row => row.role === 'daemon').map(row => row.rss));
	out.push('', '## Timeline (every sample)', '', 'RSS in MiB, CPU in % of one core over the preceding interval; `ws` = workspace workers (Terminal/Git panes), `procs` = every process of the soak.', '');
	out.push(`| min | phase | UI | daemon RSS | | daemon CPU | daemon fds | ${workers.map(title => `${title} RSS / CPU / fds`).join(' | ')} | ws RSS / CPU | agents CPU (sum) | procs | zombies |`);
	out.push(`|---|---|---|---|---|---|---|${workers.map(() => '---').join('|')}|---|---|---|---|`);
	for (const k of ks) {
		const rows = samples.filter(row => row.k === k), total = totals.find(row => row.k === k), daemon = rows.find(row => row.role === 'daemon');
		const cells = workers.map(title => { const w = rows.find(row => row.role === 'worker' && row.label === title); return w ? `${mb(w.rss)} / ${fmt(w.cpuPct, 1)} / ${w.fds}` : '—'; });
		const ws = rows.filter(row => row.role === 'ws-worker'), agents = rows.filter(row => row.role === 'agent');
		out.push(`| ${(total.t / 60).toFixed(1)} | ${total.phase} | ${total.ui} | ${daemon ? mb(daemon.rss) : '—'} | ${daemon ? bar(daemon.rss, daemonMax) : ''} | ${fmt(daemon?.cpuPct, 1)} | ${daemon?.fds ?? ''} | ${cells.join(' | ')} | ${ws.length ? `${mb(ws.reduce((s, row) => s + row.rss, 0))} / ${fmt(ws.reduce((s, row) => s + (row.cpuPct ?? 0), 0), 1)} (${ws.length})` : '—'} | ${fmt(agents.reduce((s, row) => s + (row.cpuPct ?? 0), 0), 1)} | ${total.procs} | ${total.zombies} |`);
	}
	// Chart-as-table: RSS at the last fully idle sample of every cycle (same phase, same load), the clearest view of growth.
	const idleEnd = (cycle, test) => samples.filter(row => row.cycle === cycle && row.phase === 'idle' && test(row)).at(-1);
	const idleDaemonMax = max(samples.filter(row => row.phase === 'idle' && row.role === 'daemon').map(row => row.rss));
	out.push('', '## RSS at the end of each idle stretch (MiB)', '', `| cycle | min | UI | daemon | | ${workers.join(' | ')} | workspace workers | agents (sum) | procs |`, `|---|---|---|---|---|${workers.map(() => '---').join('|')}|---|---|---|`);
	for (let cycle = 0; cycle < CYCLES; cycle++) {
		const daemon = idleEnd(cycle, row => row.role === 'daemon');
		if (!daemon) continue;
		const at = samples.filter(row => row.k === daemon.k);
		out.push(`| ${cycle}${cycle < WARMUP_CYCLES ? ' (warm-up)' : ''} | ${(daemon.t / 60).toFixed(1)} | ${daemon.ui} | ${mb(daemon.rss)} | ${bar(daemon.rss, idleDaemonMax)} | ${workers.map(title => { const w = at.find(row => row.role === 'worker' && row.label === title); return w ? mb(w.rss) : '—'; }).join(' | ')} | ${mb(at.filter(row => row.role === 'ws-worker').reduce((s, row) => s + row.rss, 0))} (${at.filter(row => row.role === 'ws-worker').length}) | ${mb(at.filter(row => row.role === 'agent').reduce((s, row) => s + row.rss, 0))} | ${totals.find(row => row.k === daemon.k)?.procs} |`);
	}
	out.push('', '## Idle stretches (fully idle intervals only)', '', '| cycle | UI | intervals | daemon CPU mean / max | workers CPU max | ws-workers CPU max | agents CPU max | UI CPU max | daemon fds | daemon RSS |', '|---|---|---|---|---|---|---|---|---|---|');
	for (let cycle = 0; cycle < CYCLES; cycle++) {
		const rows = samples.filter(row => row.cycle === cycle && row.phase === 'idle' && row.cpuPct !== null);
		if (!rows.length) continue;
		const of = role => rows.filter(row => row.role === role).map(row => row.cpuPct);
		const daemon = of('daemon');
		out.push(`| ${cycle} | ${[...new Set(rows.map(row => row.ui))].join('/')} | ${new Set(rows.map(row => row.k)).size} | ${fmt(daemon.reduce((s, v) => s + v, 0) / daemon.length)} / ${fmt(max(daemon))} | ${fmt(max(of('worker')))} | ${fmt(max(of('ws-worker')))} | ${fmt(max(of('agent')))} | ${fmt(max(of('ui')))} | ${[...new Set(rows.filter(row => row.role === 'daemon').map(row => row.fds))].join('/')} | ${mb(max(rows.filter(row => row.role === 'daemon').map(row => row.rss)))} |`);
	}
	out.push('', '## Traffic and stop', '');
	out.push(`- Subscriber events: ${JSON.stringify(cleanup.subscriber?.events ?? {})}; hook signals seen (attention events): ${JSON.stringify(cleanup.subscriber?.attention ?? {})}.`);
	out.push(`- Churn client events: ${JSON.stringify(cleanup.churn?.events ?? {})}; ${(cleanup.churn?.outputBytes / 2 ** 20 || 0).toFixed(1)} MiB of agent output received while attached.`);
	out.push(`- Sessions at the end: ${JSON.stringify(cleanup.finalList ?? [])}.`);
	out.push(`- Stop: daemon exit ${JSON.stringify(cleanup.daemonExit)}; leftover processes ${JSON.stringify(cleanup.leftovers ?? [])}; files left ${JSON.stringify([...(cleanup.files ?? []), ...(cleanup.workerPidFiles ?? [])])}.`);
	if (ui.screens.length) {
		out.push('', '## UI screens (headless xterm of the PTY)', '');
		for (const screen of ui.screens.slice(0, 4)) out.push(`<details><summary>${screen.label} (t=${screen.at.toFixed(0)} s)</summary>`, '', '```text', screen.text, '```', '', '</details>', '');
	}
	out.push('', '## Raw samples (CSV)', '', `One row per measured process per sample (\`role\` daemon / worker / ws-worker / agent / ui) and one \`total\` row per sample (\`label\` = zombie count and pids, \`rss_kb\` = RSS of every process of the soak, \`group_procs\` = their number).`, '');
	out.push('<details><summary>samples.csv</summary>', '', '```csv', csv().trimEnd(), '```', '', '</details>', '');
	return out.join('\n');
}

let exitCode = 2;
const onSignal = signal => { log(`received ${signal}; stopping`); stopping = true; stopAll().finally(() => process.exit(130)); };
process.once('SIGINT', onSignal); process.once('SIGTERM', onSignal);
try {
	await run();
	const result = verdict();
	fs.writeFileSync(path.join(OUT, 'samples.csv'), csv());
	fs.writeFileSync(path.join(OUT, 'screens.json'), JSON.stringify(ui.screens, null, 1));
	const text = markdown(result);
	fs.writeFileSync(path.join(OUT, 'G8.md'), text);
	if (DOC) { fs.mkdirSync(path.dirname(DOC), {recursive: true}); fs.writeFileSync(DOC, text); }
	for (const check of result.checks) log(`${check.ok ? 'PASS' : 'FAIL'} ${check.name}: ${check.measured}`);
	log(`verdict: ${result.ok ? 'PASS' : 'FAIL'}; raw output in ${OUT}${DOC ? `, report ${DOC}` : ''}`);
	exitCode = result.ok ? 0 : 1;
} catch (error) {
	log(`soak broke: ${error.stack}`);
	stopping = true;
	await stopAll().catch(inner => log(`cleanup failed: ${inner.stack}`));
	try { fs.writeFileSync(path.join(OUT, 'samples.csv'), csv()); } catch {}
}
process.exit(exitCode);
