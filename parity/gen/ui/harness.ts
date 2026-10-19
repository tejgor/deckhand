// UI golden harness: drives the built TS app (dist/cli.js) in a real PTY, renders its output with @xterm/headless and
// snapshots the screen (text, cursor, per-row style runs) once it is idle. Every run is isolated: DECKHAND_HOME, HOME,
// the fake agents and the demo repository live under one temp directory, which is removed afterwards.
import fs from 'node:fs/promises';
import {mkdtempSync, realpathSync} from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {fileURLToPath} from 'node:url';
import pty from 'node-pty';
import xtermHeadless from '@xterm/headless';
import {fakeAgent} from '../../../tests/helpers.ts';
import {normaliseRow, normaliseText, type Glyph, type Style} from './normalise.ts';

const {Terminal} = xtermHeadless as typeof import('@xterm/headless');
const exec = promisify(execFile);

export const PROJECT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..');
export const CLI = path.join(PROJECT, 'dist', 'cli.js');
export const COLS = 130, ROWS = 36;
/** A screen is idle once its normalised content has not changed for this long. */
const STABLE_MS = 700;
const WAIT_MS = 30_000;

export const KEY = {
	esc: '\x1b', enter: '\r', tab: '\t', shiftTab: '\x1b[Z', backspace: '\x7f',
	up: '\x1b[A', down: '\x1b[B', right: '\x1b[C', left: '\x1b[D', home: '\x1b[H', end: '\x1b[F',
	ctrlA: '\x01', ctrlC: '\x03', ctrlP: '\x10', ctrlS: '\x13', ctrlT: '\x14',
} as const;

/** One replayable step of a screen's script. */
export type Step =
	| {press: string}
	| {wait: string}
	| {waitGone: string}
	| {resize: {cols: number; rows: number}}
	| {external: string; args: Record<string, unknown>};

export type StyleRun = {start: number; end: number} & Partial<Style>;
export type Screen = {
	name: string; scenario: string; cols: number; rows: number;
	keys: string[]; script: Step[];
	text: string[];
	cursor: {x: number; y: number; visible: boolean};
	styles: Array<{row: number; runs: StyleRun[]}>;
};

const gitEnv = {GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1'};
export async function git(cwd: string, ...args: string[]): Promise<string> {
	return (await exec('git', ['-C', cwd, ...args], {env: {...process.env, ...gitEnv}, maxBuffer: 1024 * 1024})).stdout.trim();
}

/** The demo repository every scenario starts in: `<TMP>/demo`, one commit on main plus an untracked-free tree. */
async function demoRepo(directory: string): Promise<void> {
	await fs.mkdir(directory, {recursive: true});
	await git(directory, 'init', '-b', 'main');
	await git(directory, 'config', 'user.name', 'Deckhand Test');
	await git(directory, 'config', 'user.email', 'deckhand@example.invalid');
	await git(directory, 'config', 'commit.gpgsign', 'false');
	await git(directory, 'config', 'core.hooksPath', path.join(directory, '.git', 'no-hooks'));
	await fs.writeFile(path.join(directory, 'file.txt'), 'first\n');
	await fs.writeFile(path.join(directory, 'README.md'), '# Demo\n\nA repository for UI goldens.\n');
	await fs.writeFile(path.join(directory, 'deckhand.json'), `${JSON.stringify({devCommand: 'echo dev-ready', actions: {test: 'echo tests-ok'}}, null, 2)}\n`);
	await git(directory, 'add', '.'); await git(directory, 'commit', '-m', 'initial');
}

function styleOf(cell: import('@xterm/headless').IBufferCell): Style {
	const color = (mode: 'fg' | 'bg'): number | string | null => {
		const isDefault = mode === 'fg' ? cell.isFgDefault() : cell.isBgDefault();
		if (isDefault) return null;
		const value = mode === 'fg' ? cell.getFgColor() : cell.getBgColor();
		const rgb = mode === 'fg' ? cell.isFgRGB() : cell.isBgRGB();
		return rgb ? `#${value.toString(16).padStart(6, '0')}` : value;
	};
	return {fg: color('fg'), bg: color('bg'), bold: Boolean(cell.isBold()), dim: Boolean(cell.isDim()), italic: Boolean(cell.isItalic()), underline: Boolean(cell.isUnderline()), inverse: Boolean(cell.isInverse())};
}

export class Session {
	readonly name: string;
	readonly base: string;
	readonly home: string;
	readonly userHome: string;
	readonly bin: string;
	readonly repo: string;
	readonly shim: string;
	private term!: pty.IPty;
	private xterm!: InstanceType<typeof Terminal>;
	private pending = Promise.resolve();
	private cols = COLS;
	private rows = ROWS;
	private script: Step[] = [];
	private keys: string[] = [];
	private exited = false;
	readonly screens: Screen[] = [];
	private bases: string[];

	constructor(name: string) {
		this.name = name;
		// realpath: macOS reports /private/var for $TMPDIR's /var; the app shows the resolved form.
		this.base = realpathSync(mkdtempSync(path.join(os.tmpdir(), 'dh-ui-')));
		this.home = path.join(this.base, 'home');
		this.userHome = path.join(this.base, 'user');
		this.bin = path.join(this.base, 'bin');
		this.repo = path.join(this.base, 'demo');
		this.shim = path.join(this.bin, 'deckhand-cli.mjs');
		this.bases = [...new Set([this.base, this.base.replace(/^\/private\//, '/')])];
	}

	async start(): Promise<void> {
		await fs.mkdir(this.home, {recursive: true, mode: 0o700});
		await fs.mkdir(this.userHome, {recursive: true});
		await fs.mkdir(this.bin, {recursive: true});
		for (const agent of ['claude', 'pi', 'codex']) await fs.writeFile(path.join(this.bin, agent), fakeAgent, {mode: 0o755});
		// TEST_CLI: the fake agents report lifecycle hooks through `node $TEST_CLI hook`; a Rust harness swaps this shim.
		await fs.writeFile(this.shim, `import ${JSON.stringify(CLI)};\n`);
		await demoRepo(this.repo);
		const gitDir = path.dirname((await exec('sh', ['-c', 'command -v git'])).stdout.trim());
		const env: Record<string, string> = {
			PATH: [this.bin, path.dirname(process.execPath), gitDir, '/usr/bin', '/bin', '/usr/sbin', '/sbin'].join(path.delimiter),
			HOME: this.userHome, DECKHAND_HOME: this.home, DECKHAND_DEV: '0', DECKHAND_AGENT_LATEST: '{}',
			SHELL: '/bin/sh', TERM: 'xterm-256color', LANG: 'en_US.UTF-8', TZ: 'UTC', USER: 'deckhand', LOGNAME: 'deckhand',
			TEST_CLI: this.shim, CLAUDE_CONFIG_DIR: path.join(this.userHome, '.claude'), CODEX_HOME: path.join(this.userHome, '.codex'),
			EDITOR: 'true', VISUAL: 'true', ...gitEnv,
		};
		if (process.env.TMPDIR) env.TMPDIR = process.env.TMPDIR;
		this.xterm = new Terminal({cols: this.cols, rows: this.rows, allowProposedApi: true, scrollback: 0});
		this.term = pty.spawn(process.execPath, [CLI], {name: 'xterm-256color', cwd: this.repo, env, cols: this.cols, rows: this.rows});
		this.term.onData(data => { this.pending = this.pending.then(() => new Promise<void>(resolve => this.xterm.write(data, resolve))); });
		this.term.onExit(() => { this.exited = true; });
	}

	private record(step: Step): void { this.script.push(step); }

	press(key: string): void {
		this.record({press: key}); this.keys.push(key);
		this.term.write(key);
	}

	async resize(cols: number, rows: number): Promise<void> {
		this.record({resize: {cols, rows}});
		this.cols = cols; this.rows = rows;
		this.term.resize(cols, rows);
		await this.pending;
		this.xterm.resize(cols, rows);
	}

	/** A replayable change made outside the UI (see fixtures/ui/README.md). */
	async external(kind: string, args: Record<string, unknown>, run: () => Promise<void>): Promise<void> {
		this.record({external: kind, args}); await run();
	}

	private glyphRows(): Glyph[][] {
		const buffer = this.xterm.buffer.active;
		const rows: Glyph[][] = [];
		for (let y = 0; y < this.rows; y++) {
			const line = buffer.getLine(buffer.viewportY + y);
			const glyphs: Glyph[] = [];
			for (let x = 0; x < this.cols; x++) {
				const cell = line?.getCell(x);
				if (!cell) { glyphs.push({ch: ' ', width: 1, style: {fg: null, bg: null, bold: false, dim: false, italic: false, underline: false, inverse: false}}); continue; }
				const width = cell.getWidth();
				if (width === 0) continue; // the right half of a wide character
				glyphs.push({ch: cell.getChars() || ' ', width, style: styleOf(cell)});
			}
			rows.push(glyphs);
		}
		return rows;
	}

	/** Normalised rows (glyphs) of what is on screen now. */
	private async current(): Promise<Glyph[][]> {
		await this.pending;
		return this.glyphRows().map(row => normaliseRow(row, this.bases));
	}

	private static key(rows: Glyph[][]): string {
		return rows.map(row => row.map(glyph => `${glyph.ch}\u0000${JSON.stringify(glyph.style)}`).join('\u0001')).join('\n');
	}

	text(rows: Glyph[][]): string[] { return rows.map(row => row.map(glyph => glyph.ch).join('').replace(/\s+$/, '')); }

	/** Waits until `marker` is on screen (normalised text), recording the wait so a replay waits too. */
	async wait(marker: string, record = true): Promise<void> {
		if (record) this.record({wait: marker});
		const deadline = Date.now() + WAIT_MS;
		while (Date.now() < deadline) {
			if (this.exited) throw new Error(`[${this.name}] app exited while waiting for ${JSON.stringify(marker)}; screen:\n${this.text(await this.current()).join('\n')}`);
			if (this.text(await this.current()).join('\n').includes(marker)) return;
			await new Promise(resolve => setTimeout(resolve, 40));
		}
		throw new Error(`[${this.name}] timed out waiting for ${JSON.stringify(marker)}; screen:\n${this.text(await this.current()).join('\n')}`);
	}

	async waitGone(marker: string): Promise<void> {
		this.record({waitGone: marker});
		const deadline = Date.now() + WAIT_MS;
		while (Date.now() < deadline) {
			if (!this.text(await this.current()).join('\n').includes(marker)) return;
			await new Promise(resolve => setTimeout(resolve, 60));
		}
		throw new Error(`[${this.name}] timed out waiting for ${JSON.stringify(marker)} to go; screen:\n${this.text(await this.current()).join('\n')}`);
	}

	/** Presses `key` and waits for `marker`. */
	async step(key: string, marker: string): Promise<void> { this.press(key); await this.wait(marker); }

	/** Waits for idle (normalised screen unchanged for STABLE_MS) and records the screen. */
	async snap(name: string, marker?: string): Promise<Screen> {
		if (marker) await this.wait(marker);
		const deadline = Date.now() + WAIT_MS;
		let last = '', since = Date.now(), rows = await this.current();
		while (Date.now() < deadline) {
			rows = await this.current();
			const key = Session.key(rows);
			if (key !== last) { last = key; since = Date.now(); }
			else if (Date.now() - since >= STABLE_MS) break;
			await new Promise(resolve => setTimeout(resolve, 50));
		}
		if (marker && !this.text(rows).join('\n').includes(marker)) throw new Error(`[${this.name}] ${name}: ${JSON.stringify(marker)} went away before the screen settled`);
		const buffer = this.xterm.buffer.active;
		const core = (this.xterm as unknown as {_core?: {coreService?: {isCursorHidden?: boolean}}})._core;
		const styles: Screen['styles'] = [];
		rows.forEach((row, index) => {
			const runs: StyleRun[] = [];
			let column = 0, previousKey = '';
			for (const glyph of row) {
				const start = column; column += glyph.width;
				const {fg, bg, bold, dim, italic, underline, inverse} = glyph.style;
				if (fg === null && bg === null && !bold && !dim && !italic && !underline && !inverse) continue;
				const style: Partial<Style> = {};
				if (fg !== null) style.fg = fg; if (bg !== null) style.bg = bg;
				if (bold) style.bold = true; if (dim) style.dim = true; if (italic) style.italic = true; if (underline) style.underline = true; if (inverse) style.inverse = true;
				const previous = runs.at(-1), styleKey = JSON.stringify(style);
				if (previous && previous.end === start && previousKey === styleKey) previous.end = column;
				else runs.push({start, end: column, ...style});
				previousKey = styleKey;
			}
			if (runs.length) styles.push({row: index, runs});
		});
		const screen: Screen = {
			name, scenario: this.name, cols: this.cols, rows: this.rows,
			keys: [...this.keys], script: [...this.script],
			text: this.text(rows),
			cursor: {x: buffer.cursorX, y: buffer.cursorY, visible: !(core?.coreService?.isCursorHidden ?? false)},
			styles,
		};
		this.screens.push(screen);
		return screen;
	}

	/** Normalises a string the same way screens are (for diagnostics). */
	normalise(text: string): string { return normaliseText(text, this.bases); }

	/** IDs and hook credentials of the sessions whose fake agent has started, newest last. */
	async traces(): Promise<Array<{sessionId: string; args: string[]; token: string; launchId: string; mtime: number}>> {
		const names = (await fs.readdir(this.home).catch(() => [] as string[])).filter(name => name.startsWith('trace-') && name.endsWith('.json'));
		const traces = await Promise.all(names.map(async name => {
			const file = path.join(this.home, name);
			const [raw, stat] = await Promise.all([fs.readFile(file, 'utf8'), fs.stat(file)]);
			return {sessionId: name.slice('trace-'.length, -'.json'.length), ...JSON.parse(raw), mtime: stat.birthtimeMs};
		}));
		return traces.sort((a, b) => a.mtime - b.mtime);
	}

	/** Sends a lifecycle hook as the fake agent of `sessionId` would (`node $TEST_CLI hook`). */
	async hook(sessionId: string, payload: Record<string, unknown>): Promise<void> {
		const trace = (await this.traces()).find(item => item.sessionId === sessionId);
		if (!trace) throw new Error(`no trace for ${sessionId}`);
		const flag = (name: string) => { const at = trace.args.indexOf(name); return at >= 0 ? trace.args[at + 1] : undefined; };
		const nativeId = flag('--session-id') ?? flag('--resume') ?? 'fixture-native-claude';
		await new Promise<void>((resolve, reject) => {
			const child = execFile(process.execPath, [this.shim, 'hook'], {env: {...process.env, DECKHAND_HOME: this.home, HOME: this.userHome, DECKHAND_SESSION_ID: sessionId, DECKHAND_LAUNCH_ID: trace.launchId, DECKHAND_HOOK_TOKEN: trace.token}}, error => (error ? reject(error) : resolve()));
			child.stdin?.end(JSON.stringify({session_id: nativeId, ...payload}));
		});
	}

	async stop(): Promise<void> {
		if (!this.exited) { try { this.term.kill(); } catch {} }
		// The UI started a detached daemon in DECKHAND_HOME; SIGTERM makes it stop its workers, then wait for it.
		const pid = Number.parseInt(await fs.readFile(path.join(this.home, 'daemon.pid'), 'utf8').catch(() => ''), 10);
		const alive = (id: number) => { try { process.kill(id, 0); return true; } catch { return false; } };
		if (Number.isFinite(pid) && pid > 0 && alive(pid)) {
			try { process.kill(pid, 'SIGTERM'); } catch {}
			const deadline = Date.now() + 8000;
			while (alive(pid) && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 50));
			if (alive(pid)) { try { process.kill(pid, 'SIGKILL'); } catch {} }
		}
		// Anything left running from this home (workers, agents): kill it by its recorded pid.
		const workers = path.join(this.home, 'workers');
		for (const name of await fs.readdir(workers).catch(() => [] as string[])) {
			const worker = Number.parseInt(await fs.readFile(path.join(workers, name), 'utf8').catch(() => ''), 10);
			if (Number.isFinite(worker) && worker > 0 && alive(worker)) { try { process.kill(-worker, 'SIGKILL'); } catch {} try { process.kill(worker, 'SIGKILL'); } catch {} }
		}
		for (const trace of await this.traces().catch(() => [])) {
			const agent = (trace as {pid?: number}).pid;
			if (agent && alive(agent)) { try { process.kill(agent, 'SIGKILL'); } catch {} }
		}
		this.xterm.dispose();
		await fs.rm(this.base, {recursive: true, force: true});
	}
}
