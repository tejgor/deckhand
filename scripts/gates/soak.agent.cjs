#!/usr/bin/env node
// G8 soak: an output-heavy fake claude/codex/pi (copied by soak.mjs into the soak's agent-bin as all three names).
//
// It follows the soak clock in soak.json beside it: every cycle is a busy stretch (continuous colored lines, cursor
// movement, two 1 MiB bursts, tool hooks) and then an idle stretch (no output, one timer to the next busy start), so
// all five sessions are idle at the same time and the daemon's and workers' idle CPU can be measured. Hooks go through
// `$TEST_CLI hook`, as the real agents' hooks do. Like tests/helpers.ts fakeAgent it answers --help / --version and
// reports SessionStart; it never exits on its own (Deckhand stops it).
'use strict';
const fs = require('fs');
const cp = require('child_process');
const path = require('path');

const provider = path.basename(process.argv[1]);
if (process.argv.includes('--help')) { console.log('PROMPT --settings --no-daemon resume'); process.exit(0); }
if (process.argv[2] === '--version') {
	console.log(provider === 'claude' ? '1.0.0 (Claude Code)' : provider === 'codex' ? 'codex-cli 1.0.0' : '1.0.0');
	process.exit(0);
}

const config = JSON.parse(fs.readFileSync(path.join(path.dirname(process.argv[1]), 'soak.json'), 'utf8'));
const home = process.env.DECKHAND_HOME;
const sessionId = process.env.DECKHAND_SESSION_ID || 'unknown';
const scale = config.cycle / 300;
// Staggers the bursts of the five sessions (the agent does not know its title).
const lane = [...sessionId].reduce((hash, char) => (hash * 31 + char.charCodeAt(0)) >>> 0, 7) % 5;
const nativeId = provider === 'claude' ? (flag('--session-id') || flag('--resume') || 'soak-native-claude') : 'soak-native-' + provider + '-' + sessionId.slice(0, 8);

function flag(name) { const at = process.argv.indexOf(name); return at > 1 ? process.argv[at + 1] : undefined; }

try {
	fs.writeFileSync(path.join(home, 'soak-agent-' + sessionId + '.json'), JSON.stringify({provider, pid: process.pid, launchId: process.env.DECKHAND_LAUNCH_ID, lane}));
} catch {}

function hook(payload) {
	try {
		const child = cp.spawn(process.env.TEST_CLI, ['hook'], {stdio: ['pipe', 'ignore', 'ignore']});
		child.on('error', () => {});
		child.stdin.on('error', () => {});
		child.stdin.end(JSON.stringify({session_id: nativeId, ...payload}));
	} catch {}
}

process.stdout.on('error', () => process.exit(0));
process.stdin.on('error', () => process.exit(0));
process.stdin.on('end', () => process.exit(0));
if (process.stdin.isTTY) process.stdin.setRawMode(true);
process.stdin.resume();
process.stdin.on('data', () => {});

const words = ['refactor', 'daemon', 'worker', 'snapshot', 'scrollback', 'résumé', 'naïve', '日本語', 'ok ✓', 'warn ⚠', 'diff', 'hunk', 'tokio', 'LocalSet', 'pty', 'ESC[', 'grapheme', 'width'];
let frame = 0, writing = false, timers = [], ticker = null;

function line(n) {
	const color = (n * 7) % 231 + 1;
	let text = '';
	for (let i = 0; text.length < 70; i++) text += words[(n + i * 5) % words.length] + ' ';
	return `\x1b[38;5;${color}m●\x1b[0m \x1b[1m${provider}\x1b[22m ${String(n).padStart(7, '0')} \x1b[48;5;${(color + 100) % 231 + 1}m${text}\x1b[0m\r\n`;
}

function writeFrame() {
	if (writing) return;
	frame++;
	let out = '';
	for (let i = 0; i < 16; i++) out += line(frame * 16 + i);
	// Cursor movement: rewrite a status line three rows up, then come back.
	out += `\x1b7\x1b[3A\r\x1b[2K\x1b[36m${'⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏'[frame % 10]} working… frame ${frame}\x1b[0m\x1b8`;
	if (frame % 75 === 0) out += '\x1b[?25l\x1b[2J\x1b[H\x1b[1;33m── redraw ──\x1b[0m\r\n\x1b[?25h';
	if (frame % 20 === 0) out += `\x1b[10;5H\x1b[K\x1b[35mcursor jump ${frame}\x1b[0m\x1b[999;1H`;
	process.stdout.write(out);
}

// 1 MiB of colored lines, written in chunks that honour backpressure.
function burst() {
	if (writing) return;
	writing = true;
	let remaining = 1024 * 1024, n = 0;
	const next = () => {
		while (remaining > 0) {
			let chunk = '';
			while (chunk.length < 64 * 1024) chunk += line(1_000_000 + n++);
			const bytes = Buffer.from(chunk.slice(0, Math.min(chunk.length, remaining)));
			remaining -= bytes.length;
			if (!process.stdout.write(bytes)) { process.stdout.once('drain', next); return; }
		}
		writing = false;
	};
	next();
}

function phaseAt(now) {
	const t = (now - config.epoch) / 1000;
	if (t < 0) return {busy: false, next: config.epoch + config.busyStart * 1000};
	const cycle = Math.floor(t / config.cycle), offset = t - cycle * config.cycle, start = config.epoch + cycle * config.cycle * 1000;
	if (offset >= config.busyStart && offset < config.idleStart) return {busy: true, offset, next: start + config.idleStart * 1000};
	return {busy: false, next: offset < config.busyStart ? start + config.busyStart * 1000 : start + (config.cycle + config.busyStart) * 1000};
}

function at(delaySeconds, fn) { if (delaySeconds >= 0) timers.push(setTimeout(fn, delaySeconds * 1000)); }

function enterBusy(phase) {
	hook({hook_event_name: 'UserPromptSubmit', prompt: 'soak'});
	ticker = setInterval(writeFrame, 40);
	const elapsed = phase.offset - config.busyStart;
	for (const start of [30, 110]) at((start + lane * 9) * scale - elapsed, burst);
	for (let s = 20; s < (config.idleStart - config.busyStart) / scale - 10; s += 25) {
		at(s * scale - elapsed, () => hook({hook_event_name: 'PreToolUse', tool_name: 'Bash', tool_input: {command: 'ls'}}));
		at((s + 2) * scale - elapsed, () => hook({hook_event_name: 'PostToolUse', tool_name: 'Bash'}));
	}
	timers.push(setTimeout(enterIdle, phase.next - Date.now()));
}

function enterIdle() {
	clearInterval(ticker); ticker = null;
	for (const timer of timers) clearTimeout(timer);
	timers = [];
	process.stdout.write('\r\n\x1b[32m✓ Done.\x1b[0m\r\n> ');
	hook({hook_event_name: 'Stop', last_assistant_message: 'Done with soak cycle.'});
	schedule();
}

function schedule() {
	const phase = phaseAt(Date.now());
	if (phase.busy) enterBusy(phase);
	else timers.push(setTimeout(schedule, Math.max(10, phase.next - Date.now())));
}

hook({hook_event_name: 'SessionStart', source: 'startup'});
process.stdout.write('ready\r\n');
schedule();
