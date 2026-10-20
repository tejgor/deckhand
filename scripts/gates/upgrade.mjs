#!/usr/bin/env node
// G6 — upgrade path gate (one-off acceptance check of the Rust port; results in docs/port/gates/G6.md).
//
//   cargo build --release -p deckhand
//   node scripts/gates/upgrade.mjs [--keep] [--report <file.json>]
//
// 1. Builds the v45 TS daemon from the `main` branch in a temporary git worktree (`git worktree add --detach`,
//    `npm ci`, `npm run build`; nothing is installed globally).
// 2. Starts it on a temp DECKHAND_HOME/HOME and creates two sessions with a fake agent: one left running, one stopped.
// 3. Runs the Rust client (target/release/deckhand, the UI, in a PTY: the UI is the only command that pings the
//    daemon) against that home: it must refuse to replace the live v45 daemon with
//    `daemon protocol mismatch: expected v46, got 45; stop the old daemon first: kill <pid> (or kill $(cat …))`,
//    leaving the old daemon and its session alive.
// 4. `kill <pid>` (SIGTERM, as the message says), then runs the Rust client again: it starts the Rust daemon, which
//    adopts state.json and reports the previously running session `exited` / `exitReason: interrupted` with the
//    `DaemonRestart` attention, the stopped one unchanged.
// 5. Stops everything, checks the state the Rust daemon left on disk, removes the sandbox and the temp worktree
//    (`git worktree remove --force`: node_modules/ and dist/ are untracked).
//
// Isolation: temp DECKHAND_HOME and HOME under $TMPDIR; claude/codex/pi on PATH are fake node scripts.

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import net from 'node:net';
import {randomUUID} from 'node:crypto';
import {execFileSync, spawn} from 'node:child_process';
import {createRequire} from 'node:module';
import {fileURLToPath} from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const RS_BIN = path.resolve(ROOT, process.env.RS_BIN ?? 'target/release/deckhand');
const KEEP = process.argv.includes('--keep');
const REPORT = (() => { const at = process.argv.indexOf('--report'); return at > 0 ? path.resolve(process.argv[at + 1]) : undefined; })();
const pty = createRequire(path.join(ROOT, 'package.json'))('node-pty');
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const isAlive = pid => { try { process.kill(pid, 0); return true; } catch { return false; } };

const report = {startedAt: new Date().toISOString(), steps: [], checks: []};
let failed = false;
function check(name, ok, evidence) {
	if (!ok) failed = true;
	report.checks.push({name, ok, evidence});
	console.log(`${ok ? 'PASS' : 'FAIL'} ${name}${evidence === undefined ? '' : `\n  ${String(typeof evidence === 'string' ? evidence : JSON.stringify(evidence)).replaceAll('\n', '\n  ')}`}`);
}
function step(text) { report.steps.push(text); console.log(`-- ${text}`); }

function run(command, args, options = {}) {
	return execFileSync(command, args, {encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 64 * 1024 * 1024, ...options});
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
		await sleep(100);
	}
}

// The fake agent of tests/helpers.ts (fakeAgent), trimmed: answers --version/--help, reports SessionStart through
// `$TEST_CLI hook`, writes ready-<session>-<launch> to $FAKE_AGENT_STATE (outside DECKHAND_HOME), then idles.
const fakeAgent = version => `#!${process.execPath}
const fs = require('fs'), cp = require('child_process'), path = require('path');
const STATE = process.env.FAKE_AGENT_STATE;
if (process.argv.includes('--help')) { console.log('PROMPT --settings --no-daemon resume'); process.exit(0); }
const provider = path.basename(process.argv[1]);
if (process.argv[2] === '--version') { console.log(${JSON.stringify(version)}); process.exit(0); }
const flag = name => { const at = process.argv.indexOf(name); return at > 1 ? process.argv[at + 1] : undefined; };
const id = provider === 'claude' ? flag('--session-id') ?? flag('--resume') ?? 'fixture-native-claude' : 'fixture-native-' + provider;
fs.writeFileSync(path.join(STATE, 'pid-' + process.pid), '');
const child = cp.spawn(process.env.TEST_CLI, ['hook'], {stdio: ['pipe', 'ignore', 'ignore']});
child.stdin.end(JSON.stringify({hook_event_name: 'SessionStart', session_id: id}));
child.on('close', () => { fs.writeFileSync(path.join(STATE, 'ready-' + process.env.DECKHAND_SESSION_ID + '-' + process.env.DECKHAND_LAUNCH_ID), ''); console.log('ready'); });
if (process.stdin.isTTY) process.stdin.setRawMode(true);
process.stdin.resume();
setInterval(() => {}, 10000);
`;

/**
 * Runs a client UI (`command args`) in a PTY until it exits, `stopWhen(output)` holds or `untilMs` passes; returns its
 * output and exit code (undefined when the gate closed it). 300 columns, so the status line never truncates the error.
 */
function runClient(command, args, cwd, env, {untilMs, stopWhen}) {
	return new Promise(resolve => {
		let exitCode, finished = false;
		const term = pty.spawn(command, args, {cwd, env, cols: 300, rows: 30, name: 'xterm-256color'});
		let output = '', closedByGate = false;
		const finish = () => { if (finished) return; finished = true; clearTimeout(timer); clearInterval(poll); if (exitCode === undefined) { closedByGate = true; try { term.kill(); } catch {} } setTimeout(() => resolve({output, exitCode: closedByGate ? undefined : exitCode}), 300); };
		term.onData(data => { output += data; });
		term.onExit(event => { if (!finished) exitCode = event.exitCode; finish(); });
		const timer = setTimeout(finish, untilMs);
		const poll = setInterval(async () => { if (stopWhen && await stopWhen(output)) finish(); }, 250);
	});
}
const plain = text => text.replace(/\x1b\[[0-9;?]*[ -/]*[@-~]/g, '').replace(/\x1b[\]P^_][^\x07\x1b]*(\x07|\x1b\\)/g, '').replace(/\x1b[()][0-9A-Za-z]/g, '').replace(/\x1b[=>78DEHM]/g, '');

async function main() {
	if (!fs.existsSync(RS_BIN)) throw new Error(`${RS_BIN} is missing: run \`cargo build --release -p deckhand\` first`);
	// Short: <sandbox>/home/daemon.sock must fit sun_path.
	const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'dhu-')));
	const checkout = path.join(tmp, 'v45');
	const sandbox = path.join(tmp, 's');
	report.tmp = tmp;
	let v45Pid, rustDaemonPid;
	let worktreeAdded = false;
	try {
		// ── 1. v45 TS daemon from main ──────────────────────────────────────────────────────────────────────────────
		step(`git worktree add --detach ${checkout} main`);
		run('git', ['-C', ROOT, 'worktree', 'add', '--detach', checkout, 'main']);
		worktreeAdded = true;
		report.mainCommit = run('git', ['-C', checkout, 'rev-parse', 'HEAD']).trim();
		const protocol = /PROTOCOL_VERSION = (\d+)/.exec(fs.readFileSync(path.join(checkout, 'src', 'types.ts'), 'utf8'))?.[1];
		check('main is the v45 TS daemon (src/types.ts PROTOCOL_VERSION)', protocol === '45', `main ${report.mainCommit}: PROTOCOL_VERSION = ${protocol}`);
		step('npm ci && npm run build (in the temp worktree; nothing global)');
		run('npm', ['ci', '--no-audit', '--no-fund', '--prefer-offline'], {cwd: checkout});
		run('npm', ['run', 'build'], {cwd: checkout});
		const v45Cli = path.join(checkout, 'dist', 'cli.js');

		// ── 2. Sandbox and a running v45 session ────────────────────────────────────────────────────────────────────
		const home = path.join(sandbox, 'home'), deckhandHome = path.join(sandbox, 'dh'), repo = path.join(sandbox, 'repo');
		const bin = path.join(sandbox, 'bin'), agentState = path.join(sandbox, 'agent-state');
		for (const dir of [home, bin, agentState, repo, path.join(sandbox, 'tmp')]) fs.mkdirSync(dir, {recursive: true});
		const gitEnv = {...process.env, GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1', HOME: home};
		run('git', ['-C', repo, 'init', '-q', '-b', 'main'], {env: gitEnv});
		for (const [key, value] of [['user.name', 'Deckhand Gate'], ['user.email', 'gate@example.invalid'], ['commit.gpgsign', 'false'], ['core.hooksPath', path.join(repo, '.git', 'no-hooks')]]) run('git', ['-C', repo, 'config', key, value], {env: gitEnv});
		fs.writeFileSync(path.join(repo, 'README.md'), '# upgrade gate\n');
		run('git', ['-C', repo, 'add', '-A'], {env: gitEnv});
		run('git', ['-C', repo, 'commit', '-q', '-m', 'initial'], {env: gitEnv});
		for (const [program, version] of [['claude', '2.0.14 (Claude Code)'], ['codex', 'codex-cli 0.46.0'], ['pi', '0.9.3']]) fs.writeFileSync(path.join(bin, program), fakeAgent(version), {mode: 0o755});
		const v45Shim = path.join(sandbox, 'deckhand-v45');
		fs.writeFileSync(v45Shim, `#!/bin/sh\nexec '${process.execPath}' '${v45Cli}' "$@"\n`, {mode: 0o755});
		const baseEnv = {
			PATH: `${bin}:/usr/bin:/bin:/usr/sbin:/sbin`, HOME: home, DECKHAND_HOME: deckhandHome, DECKHAND_DEV: '0',
			DECKHAND_AGENT_LATEST: '{}', SHELL: '/bin/sh', TERM: 'xterm-256color', LANG: 'en_US.UTF-8', TZ: 'UTC',
			TMPDIR: path.join(sandbox, 'tmp'), XDG_CONFIG_HOME: path.join(home, '.config'), XDG_STATE_HOME: path.join(home, '.local', 'state'),
			XDG_CACHE_HOME: path.join(home, '.cache'), XDG_DATA_HOME: path.join(home, '.local', 'share'),
			CLAUDE_CONFIG_DIR: path.join(home, '.claude'), CODEX_HOME: path.join(home, '.codex'),
			GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1', FAKE_AGENT_STATE: agentState,
		};
		step(`start the v45 daemon: node ${path.relative(tmp, v45Cli)} --daemon (detached, DECKHAND_HOME=${deckhandHome})`);
		const log = fs.openSync(path.join(sandbox, 'v45-daemon.out'), 'a');
		const v45 = spawn(process.execPath, [v45Cli, '--daemon'], {cwd: repo, env: {...baseEnv, TEST_CLI: v45Shim, DECKHAND_DAEMON: '1'}, detached: true, stdio: ['ignore', log, log]});
		v45.unref();
		fs.closeSync(log);
		const ping45 = await waitFor(() => request(deckhandHome, {type: 'ping'}, 2000), result => result.ok, 30_000, 'the v45 daemon');
		v45Pid = Number.parseInt(fs.readFileSync(path.join(deckhandHome, 'daemon.pid'), 'utf8'), 10);
		check('v45 daemon answers ping with version 45', ping45.data.version === 45 && v45Pid === v45.pid, {ping: ping45.data, pidFile: v45Pid, spawned: v45.pid});
		const input = title => ({title, program: 'claude', cwd: repo, repoRoot: repo, cols: 100, rows: 30, worktreeMode: 'none'});
		const running = await request(deckhandHome, {type: 'create', input: input('Running across the upgrade')}, 60_000);
		const stopped = await request(deckhandHome, {type: 'create', input: input('Stopped before the upgrade')}, 60_000);
		if (!running.ok || !stopped.ok) throw new Error(`v45 create failed: ${running.error ?? stopped.error}`);
		const readyOf = async id => { const session = (await request(deckhandHome, {type: 'list'})).data.find(item => item.id === id); return session?.launchId && fs.existsSync(path.join(agentState, `ready-${id}-${session.launchId}`)) && session.agentSessionRef ? session : undefined; };
		await waitFor(() => readyOf(running.data.id), Boolean, 30_000, 'the running session\'s fake agent');
		await waitFor(() => readyOf(stopped.data.id), Boolean, 30_000, 'the second session\'s fake agent');
		const killed = await request(deckhandHome, {type: 'kill', sessionId: stopped.data.id}, 30_000);
		const before = await waitFor(() => request(deckhandHome, {type: 'list'}), result => result.ok && result.data.find(item => item.id === stopped.data.id)?.status === 'exited', 20_000, 'the stopped session to exit');
		const runningBefore = before.data.find(item => item.id === running.data.id);
		const stoppedBefore = before.data.find(item => item.id === stopped.data.id);
		check('v45 state before the upgrade: one running session, one stopped', killed.ok && runningBefore.status === 'running' && stoppedBefore.exitReason === 'stopped',
			{running: {id: runningBefore.id, status: runningBefore.status, pid: runningBefore.pid}, stopped: {id: stoppedBefore.id, status: stoppedBefore.status, exitReason: stoppedBefore.exitReason}});
		const agentPid = runningBefore.pid;

		// ── 3. The Rust client refuses to replace it ────────────────────────────────────────────────────────────────
		// The UI is the only client command that pings (ensureDaemonRunning); it reports the failure in its status line
		// (`Error: <message>`) and stays open. The v46 TS client of this tree is run the same way as the oracle.
		const expected = `daemon protocol mismatch: expected v46, got 45; stop the old daemon first: kill ${v45Pid} (or kill $(cat ${path.join(deckhandHome, 'daemon.pid')}))`;
		const shows = output => plain(output).includes(`Error: ${expected}`);
		step(`oracle: run the v46 TS client (node dist/cli.js, this tree) in a PTY in the sandbox repository`);
		const tsRefused = await runClient(process.execPath, [path.join(ROOT, 'dist', 'cli.js')], repo, {...baseEnv, TEST_CLI: path.join(ROOT, 'dist', 'cli.js')}, {untilMs: 20_000, stopWhen: shows});
		check('oracle: the v46 TS client shows the kill <pid> message', shows(tsRefused.output), {exitCode: tsRefused.exitCode ?? '(still open; closed by the gate)', expected});
		step(`run ${path.relative(ROOT, RS_BIN)} (UI) in a PTY in the sandbox repository`);
		const clientEnv = {...baseEnv, TEST_CLI: RS_BIN};
		// Only what the Rust client appends counts (the TS client above logged its own refusal).
		const logOffset = fs.statSync(path.join(deckhandHome, 'daemon.log')).size;
		const refused = await runClient(RS_BIN, [], repo, clientEnv, {untilMs: 20_000, stopWhen: shows});
		const text = plain(refused.output);
		report.refusalScreen = text.slice(text.indexOf('Error: '), text.indexOf('Error: ') + expected.length + 7);
		check('the Rust client refuses with the kill <pid> message (as the TS client does)', shows(refused.output), {exitCode: refused.exitCode ?? '(still open; closed by the gate)', shown: report.refusalScreen, expected});
		const stillUp = await request(deckhandHome, {type: 'ping'}, 3000);
		const stillRunning = (await request(deckhandHome, {type: 'list'})).data?.find(item => item.id === running.data.id);
		check('the v45 daemon and its running session are untouched', isAlive(v45Pid) && stillUp.ok && stillUp.data.version === 45 && stillRunning?.status === 'running' && isAlive(agentPid),
			{v45Alive: isAlive(v45Pid), ping: stillUp.data, session: stillRunning?.status, agentAlive: isAlive(agentPid)});
		const daemonLog = fs.readFileSync(path.join(deckhandHome, 'daemon.log')).subarray(logOffset).toString('utf8');
		const logLine = daemonLog.split('\n').find(line => line.includes('client refusing to replace live daemon pid'));
		check('the Rust client logs the refusal to daemon.log', Boolean(logLine?.includes(`pid ${v45Pid} after protocol mismatch: daemon protocol mismatch: expected v46, got 45`)), logLine);

		// ── 4. kill <pid>, then the Rust daemon adopts the state ─────────────────────────────────────────────────────
		step(`kill ${v45Pid} (SIGTERM, as the message says)`);
		process.kill(v45Pid, 'SIGTERM');
		await waitFor(async () => !isAlive(v45Pid), Boolean, 15_000, 'the v45 daemon to exit');
		await waitFor(async () => !isAlive(agentPid), Boolean, 10_000, 'the v45 session\'s agent to exit').catch(() => {});
		const persisted = JSON.parse(fs.readFileSync(path.join(deckhandHome, 'state.json'), 'utf8'));
		const persistedRunning = persisted.sessions.find(item => item.id === running.data.id);
		report.v45StateAfterKill = {running: {status: persistedRunning?.status, pid: persistedRunning?.pid}, socket: fs.existsSync(path.join(deckhandHome, 'daemon.sock')), pidFile: fs.existsSync(path.join(deckhandHome, 'daemon.pid'))};
		check('v45 exited, leaving the session persisted as running', !isAlive(v45Pid) && persistedRunning?.status === 'running', report.v45StateAfterKill);

		step(`run ${path.relative(ROOT, RS_BIN)} (UI) again: it starts the Rust daemon`);
		const adopted = await runClient(RS_BIN, [], repo, clientEnv, {
			untilMs: 30_000,
			stopWhen: async () => { const ping = await request(deckhandHome, {type: 'ping'}, 1000); return ping.ok && ping.data.version === 46; },
		});
		const ping46 = await waitFor(() => request(deckhandHome, {type: 'ping'}, 2000), result => result.ok, 10_000, 'the Rust daemon');
		rustDaemonPid = Number.parseInt(fs.readFileSync(path.join(deckhandHome, 'daemon.pid'), 'utf8'), 10);
		const command = rustDaemonPid ? run('ps', ['-o', 'command=', '-p', String(rustDaemonPid)]).trim() : '';
		check('the Rust client started the Rust daemon (protocol 46)', ping46.data.version === 46 && command.startsWith(RS_BIN) && command.includes('--daemon'), {ping: ping46.data, pid: rustDaemonPid, command, uiExit: adopted.exitCode ?? '(still running; closed by the gate)'});
		const after = (await request(deckhandHome, {type: 'list'})).data;
		const runningAfter = after.find(item => item.id === running.data.id);
		const stoppedAfter = after.find(item => item.id === stopped.data.id);
		check('the previously running session is marked interrupted',
			runningAfter?.status === 'exited' && runningAfter.exitReason === 'interrupted' && runningAfter.attention?.state === 'unknown' && runningAfter.attention?.event === 'DaemonRestart' && runningAfter.pid === undefined,
			{status: runningAfter?.status, exitReason: runningAfter?.exitReason, attention: runningAfter?.attention, pid: runningAfter?.pid, exitCode: runningAfter?.exitCode, agentSessionRef: runningAfter?.agentSessionRef});
		check('the stopped session is unchanged', stoppedAfter?.status === 'exited' && stoppedAfter.exitReason === 'stopped' && stoppedAfter.updatedAt === stoppedBefore.updatedAt,
			{status: stoppedAfter?.status, exitReason: stoppedAfter?.exitReason, updatedAt: [stoppedBefore.updatedAt, stoppedAfter?.updatedAt]});
		check('no other session appeared or vanished', after.length === 2, after.map(item => `${item.title}: ${item.status}`));

		// ── 5. Stop and check what the Rust daemon left on disk ─────────────────────────────────────────────────────
		// `shutdown` is refused outside the dev channel (TS and Rust alike): stop it as a user would.
		process.kill(rustDaemonPid, 'SIGTERM');
		await waitFor(async () => !isAlive(rustDaemonPid), Boolean, 15_000, 'the Rust daemon to stop');
		const finalState = JSON.parse(fs.readFileSync(path.join(deckhandHome, 'state.json'), 'utf8'));
		const finalRunning = finalState.sessions.find(item => item.id === running.data.id);
		const mode = file => `0${(fs.statSync(path.join(deckhandHome, file)).mode & 0o777).toString(8)}`;
		check('state.json after the Rust daemon stopped: interrupted session persisted, 0600/0700',
			finalRunning?.exitReason === 'interrupted' && finalRunning.status === 'exited' && mode('state.json') === '0600' && mode('.') === '0700',
			{session: {status: finalRunning?.status, exitReason: finalRunning?.exitReason, attention: finalRunning?.attention}, modes: {'state.json': mode('state.json'), '.': mode('.')}});
		report.rustDaemonLog = fs.readFileSync(path.join(deckhandHome, 'daemon.log'), 'utf8').split('\n').filter(line => !line.includes('] client ') || line.includes('refusing') || line.includes('starting daemon')).slice(-30);
	} catch (error) {
		failed = true;
		report.error = error instanceof Error ? error.stack : String(error);
		console.error(report.error);
	} finally {
		for (const pid of [v45Pid, rustDaemonPid]) if (pid && isAlive(pid)) { try { process.kill(pid, 'SIGKILL'); } catch {} }
		try { for (const file of fs.readdirSync(path.join(sandbox, 'agent-state')).filter(name => name.startsWith('pid-'))) { const pid = Number(file.slice(4)); if (isAlive(pid)) process.kill(pid, 'SIGKILL'); } } catch {}
		if (worktreeAdded && !KEEP) {
			step(`git worktree remove --force ${checkout} (--force: untracked node_modules/ and dist/)`);
			try { run('git', ['-C', ROOT, 'worktree', 'remove', '--force', checkout]); }
			catch (error) { failed = true; report.cleanupError = String(error); }
			report.worktreeRemoved = !run('git', ['-C', ROOT, 'worktree', 'list', '--porcelain']).includes(checkout);
			check('temp worktree removed', report.worktreeRemoved);
		}
		if (KEEP) console.log(`kept: ${tmp}`);
		else fs.rmSync(tmp, {recursive: true, force: true});
		report.ok = !failed;
		if (REPORT) fs.writeFileSync(REPORT, `${JSON.stringify(report, null, 2)}\n`);
		console.log(`\n${report.checks.filter(item => item.ok).length} passed, ${report.checks.filter(item => !item.ok).length} failed`);
		process.exitCode = failed ? 1 : 0;
	}
}

await main();
