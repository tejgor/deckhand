// SYNTHETIC streams: byte-for-byte imitations of how the agent TUIs Deckhand hosts draw (claude's Ink renderer, codex's
// ratatui inline viewport and alternate screen, pi's differential renderer) plus edge cases a recorded shell session
// cannot produce reliably (UTF-8 split across chunks, invalid bytes, clamped resizes). The real agents are never run:
// the sequences below follow what their renderers emit, written out by hand.
//
// Bytes are PTY *master* output. Node apps (Ink, pi-tui) keep ONLCR in raw mode (libuv), so their `\n` reaches the
// master as `\r\n` (see `onlcr`); crossterm's raw mode (codex) clears OPOST, so its output is taken as written.

import {StreamBuilder} from './format.js';

const ESC = '\x1b';
const CSI = `${ESC}[`;
const OSC = `${ESC}]`;
const BEL = '\x07';
const ST = `${ESC}\\`;
const SYNC_ON = `${CSI}?2026h`;
const SYNC_OFF = `${CSI}?2026l`;

const onlcr = (text: string) => text.replace(/\r?\n/g, '\r\n');
const sgr = (...codes: Array<number | string>) => `${CSI}${codes.join(';')}m`;
const RESET = sgr(0);
const rgb = (r: number, g: number, b: number) => `38;2;${r};${g};${b}`;
const bgRgb = (r: number, g: number, b: number) => `48;2;${r};${g};${b}`;
const cup = (row: number, col: number) => `${CSI}${row};${col}H`;

// ── claude (Ink) ────────────────────────────────────────────────────────────────────────────────────────────────

/** ansi-escapes `eraseLines(count)` as Ink's log-update uses it. */
function eraseLines(count: number): string {
	let out = '';
	for (let index = 0; index < count; index++) out += `${CSI}2K${index < count - 1 ? `${CSI}1A` : ''}`;
	return count ? `${out}${CSI}G` : out;
}

/** Ink's log-update: erase the previous dynamic frame, write the new one followed by a newline (claude wraps it in DEC 2026). */
class InkRenderer {
	private previousLineCount = 0;
	constructor(private readonly synchronized: boolean) {}
	frame(lines: string[], staticLines: string[] = []): string {
		const output = `${lines.join('\n')}\n`;
		const staticOutput = staticLines.length ? `${staticLines.join('\n')}\n` : '';
		const body = `${eraseLines(this.previousLineCount)}${staticOutput}${output}`;
		this.previousLineCount = output.split('\n').length;
		return onlcr(this.synchronized ? `${SYNC_ON}${body}${SYNC_OFF}` : body);
	}
	/** Claude clears everything on resize and renders the static history again. */
	clearAll(): string {
		this.previousLineCount = 0;
		return `${CSI}2J${CSI}3J${CSI}H`;
	}
}

const ORANGE = rgb(215, 119, 87);
const GRAY = '38;5;246';

function claudeBox(width: number, inner: string[], color = GRAY): string[] {
	const top = `${sgr(color)}╭${'─'.repeat(width - 2)}╮${RESET}`;
	const bottom = `${sgr(color)}╰${'─'.repeat(width - 2)}╯${RESET}`;
	const visible = (text: string) => text.replace(/\x1b\[[0-9;]*m/g, '');
	const rows = inner.map(line => {
		const padding = Math.max(0, width - 4 - [...visible(line)].reduce((sum, char) => sum + (/[　-鿿]/.test(char) ? 2 : 1), 0));
		return `${sgr(color)}│${RESET} ${line}${' '.repeat(padding)} ${sgr(color)}│${RESET}`;
	});
	return [top, ...rows, bottom];
}

function claudePrompt(width: number, text: string, footer: string): string[] {
	return [...claudeBox(width, [text ? `> ${text}` : `> ${sgr(2)}Try "refactor src/app.tsx"${RESET}`]), `  ${sgr(2)}${footer}${RESET}`];
}

export function claudeInk(): StreamBuilder {
	const cols = 80;
	const stream = new StreamBuilder('synthetic-claude-ink', 'synthetic', 'Imitates claude (Ink): DEC 2026 frames redrawn in place with eraseLines (2K + cursor-up), static history written above the live frame, a frame taller than the screen, clear-and-rerender on resize, terminal modes on start/exit.', cols, 24);
	const ink = new InkRenderer(true);
	// Startup: modes claude enables (bracketed paste, focus events, kitty keyboard push, hidden cursor), then a title.
	stream.chunk(`${CSI}?2004h${CSI}?1004h${CSI}>1u${CSI}?25l${OSC}0;✳ Claude Code${BEL}`);
	const welcome = claudeBox(cols - 30, [`${sgr(ORANGE)}✻${RESET} Welcome to ${sgr(1)}Claude Code${RESET}!`, '', `  ${sgr(2)}/help for help, /status for your current setup${RESET}`, '', `  ${sgr(2)}cwd: /tmp/fixture/project${RESET}`], ORANGE);
	stream.chunk(ink.frame([...welcome, '', ...claudePrompt(cols, '', '? for shortcuts')]));
	stream.checkpoint('welcome');
	// Typing: each keystroke re-renders the whole live frame.
	let typed = '';
	for (const char of 'list the files') {
		typed += char;
		stream.chunk(ink.frame([...welcome, '', ...claudePrompt(cols, typed, '? for shortcuts')]));
	}
	stream.checkpoint('typed');
	// Submit: the welcome box and the prompt become static history; the spinner is the live frame.
	const history = [...welcome, '', `${sgr(GRAY)}> list the files${RESET}`, ''];
	const spinner = ['✻', '✶', '✳', '✢', '·', '✢', '✳', '✶'];
	stream.chunk(ink.frame([`${sgr(ORANGE)}${spinner[0]}${RESET} ${sgr(ORANGE)}Thinking…${RESET} ${sgr(2)}(0s · esc to interrupt)${RESET}`, '', ...claudePrompt(cols, '', 'esc to interrupt')], history));
	for (let tick = 1; tick < 12; tick++) {
		stream.chunk(ink.frame([`${sgr(ORANGE)}${spinner[tick % spinner.length]}${RESET} ${sgr(ORANGE)}Thinking…${RESET} ${sgr(2)}(${Math.floor(tick / 3)}s · ↑ ${tick * 37} tokens · esc to interrupt)${RESET}`, '', ...claudePrompt(cols, '', 'esc to interrupt')]));
	}
	stream.checkpoint('thinking');
	// A tool call and its output land in the static history.
	const tool = [`${sgr(32)}⏺${RESET} ${sgr(1)}Bash${RESET}(ls -la)`, ...Array.from({length: 14}, (_, index) => `  ${index ? ' ' : '⎿'}  ${sgr(2)}-rw-r--r--  1 user  staff  ${String(120 + index * 17).padStart(5)} Oct 19 12:0${index % 10} file-${String(index).padStart(2, '0')}.ts${RESET}`), `     ${sgr(2)}… +32 lines (ctrl+r to expand)${RESET}`, ''];
	stream.chunk(ink.frame([`${sgr(ORANGE)}✳${RESET} ${sgr(ORANGE)}Running…${RESET}`, '', ...claudePrompt(cols, '', 'esc to interrupt')], tool));
	stream.checkpoint('tool-output');
	// A live frame taller than the screen: the erase walks up only to the top row, so the excess stays in scrollback.
	const tall = Array.from({length: 26}, (_, index) => `${sgr(index % 2 ? 36 : 35)}│${RESET} streaming line ${index + 1}: ${'lorem ipsum dolor sit amet '.slice(0, 10 + (index * 7) % 40)}`);
	stream.chunk(ink.frame([...tall, ...claudePrompt(cols, '', 'esc to interrupt')]));
	stream.chunk(ink.frame([...tall.slice(0, 20).map(line => `${line} ✓`), ...claudePrompt(cols, '', 'esc to interrupt')]));
	stream.checkpoint('taller-than-screen');
	// The reply settles: the live frame shrinks back to the prompt.
	const reply = [`${sgr(37)}⏺${RESET} There are ${sgr(1)}46 files${RESET}. The largest are:`, '', `  - ${sgr(36)}src/daemon.ts${RESET} (3 309 lines)`, `  - ${sgr(36)}src/app.tsx${RESET}`, `  - 日本語のファイル名.md ${sgr(2)}(wide characters)${RESET}`, ''];
	stream.chunk(ink.frame(claudePrompt(cols, '', '? for shortcuts'), reply));
	stream.checkpoint('reply');
	// Resize: claude clears screen and scrollback and renders everything again at the new width.
	stream.resize(60, 18);
	stream.chunk(onlcr(`${SYNC_ON}${ink.clearAll()}${SYNC_OFF}`));
	const narrowWelcome = claudeBox(50, [`${sgr(ORANGE)}✻${RESET} Welcome to ${sgr(1)}Claude Code${RESET}!`], ORANGE);
	stream.chunk(ink.frame(claudePrompt(60, '', '? for shortcuts'), [...narrowWelcome, '', `${sgr(GRAY)}> list the files${RESET}`, '', ...reply]));
	stream.checkpoint('after-resize');
	// A paste arrives (bracketed paste is the input side; the echo is just a re-render), then exit restores the modes.
	stream.chunk(ink.frame(claudePrompt(60, '[Pasted text #1 +12 lines]', 'enter to send')));
	stream.chunk(ink.frame(claudePrompt(60, '', '? for shortcuts')));
	stream.chunk(onlcr(`${CSI}?25h${CSI}?2004l${CSI}?1004l${CSI}<u${OSC}0;${BEL}\n`));
	return stream;
}

// ── codex (ratatui) ─────────────────────────────────────────────────────────────────────────────────────────────

type Cell = {char: string; style: string};
type Styled = Array<[text: string, style?: string]>;

/** A minimal ratatui-style back buffer: draws whole frames and emits only the cells that changed (CUP + SGR + text). */
class RatatuiScreen {
	private previous: Cell[][];
	constructor(private width: number, private height: number, private top = 1) {
		this.previous = this.blank();
	}
	private blank(): Cell[][] {
		return Array.from({length: this.height}, () => Array.from({length: this.width}, () => ({char: ' ', style: ''})));
	}
	resize(width: number, height: number): void {
		this.width = width; this.height = height;
		this.previous = this.blank();
	}
	/** Moves the viewport (inline mode); the next draw repaints it all. */
	moveTo(top: number): void {
		this.top = top;
		this.previous = this.blank();
	}
	get viewportTop(): number { return this.top; }
	draw(lines: Styled[], cursor?: {row: number; col: number}): string {
		const next = this.blank();
		lines.slice(0, this.height).forEach((spans, row) => {
			let col = 0;
			for (const [text, style = ''] of spans) {
				for (const char of text) {
					if (col >= this.width) break;
					const wide = /[ᄀ-ᅟ⺀-꓏가-힣豈-﫿︰-﹏＀-｠￠-￦]|[\u{1f300}-\u{1faff}]/u.test(char);
					if (wide && col + 1 >= this.width) break;
					next[row]![col] = {char, style};
					if (wide) next[row]![col + 1] = {char: '', style};
					col += wide ? 2 : 1;
				}
			}
		});
		let out = '';
		let style = '';
		let at: {row: number; col: number} | undefined;
		for (let row = 0; row < this.height; row++) {
			for (let col = 0; col < this.width; col++) {
				const cell = next[row]![col]!, old = this.previous[row]![col]!;
				if (cell.char === old.char && cell.style === old.style) continue;
				if (cell.char === '') continue;
				if (!at || at.row !== row || at.col !== col) out += cup(this.top + row, col + 1);
				if (cell.style !== style) { out += `${RESET}${cell.style ? sgr(cell.style) : ''}`; style = cell.style; }
				out += cell.char;
				const wide = next[row]![col + 1]?.char === '';
				at = {row, col: col + (wide ? 2 : 1)};
			}
		}
		if (style) out += RESET;
		this.previous = next;
		if (cursor) out += `${cup(this.top + cursor.row, cursor.col + 1)}${CSI}?25h`;
		else out += `${CSI}?25l`;
		return out;
	}
}

function codexComposer(width: number, text: string, status: string): Styled[] {
	return [
		[[`▌ ${text || 'Ask Codex to do anything'}`, text ? '' : '2']],
		[[' ']],
		[[` ⏎ send   ⇧⏎ newline   ⌃T transcript   ⌃C quit`, '2'], [`   ${status}`.slice(0, Math.max(0, width - 52)), '36']],
	];
}

export function codexInline(): StreamBuilder {
	const cols = 100, rows = 30, viewportHeight = 6;
	const stream = new StreamBuilder('synthetic-codex-inline', 'synthetic', 'Imitates codex (ratatui inline viewport): crossterm modes, cursor position query, history inserted above the viewport through a scroll region (DECSTBM + CUP + CRLF), cell-diff redraws with CUP/SGR runs, the viewport reaching the bottom, a resize, exit.', cols, rows);
	// A shell prompt line was there before codex started.
	stream.chunk('$ codex\r\n');
	// crossterm: keyboard enhancement push, bracketed paste, focus change; then the cursor position query (the reply
	// is input, not output).
	stream.chunk(`${CSI}>7u${CSI}?2004h${CSI}?1004h${CSI}6n`);
	const screen = new RatatuiScreen(cols, viewportHeight, 2);
	stream.chunk(`${SYNC_ON}${screen.draw(codexComposer(cols, '', 'gpt-5-codex high · 100% context left'), {row: 0, col: 2})}${SYNC_OFF}`);
	stream.checkpoint('started');
	let historyRows = 1; // rows above the viewport in use
	const insertHistory = (lines: Styled[]) => {
		let out = SYNC_ON;
		// Room left below the viewport: move it down first (clearing where it was), else scroll the region above it.
		const free = rows - (screen.viewportTop - 1 + viewportHeight);
		const shift = Math.min(free, lines.length);
		if (shift > 0) {
			out += `${cup(screen.viewportTop, 1)}${CSI}J`;
			screen.moveTo(screen.viewportTop + shift);
		}
		const regionBottom = screen.viewportTop - 1;
		out += `${CSI}1;${regionBottom}r${cup(Math.min(historyRows, regionBottom), 1)}`;
		for (const spans of lines) {
			out += '\r\n';
			for (const [text, style] of spans) out += style ? `${sgr(style)}${text}${RESET}` : text;
			historyRows++;
		}
		out += `${CSI}r`;
		stream.chunk(out);
		return out;
	};
	insertHistory([[['>_ ', '1'], ['OpenAI Codex', '1'], [' (v0.0.0-fixture)', '2']], [[' ']], [['model: ', '2'], ['gpt-5-codex high', '36']], [['directory: ', '2'], ['/tmp/fixture/project', '']], [[' ']]]);
	stream.chunk(`${screen.draw(codexComposer(cols, '', 'gpt-5-codex high · 100% context left'), {row: 0, col: 2})}${SYNC_OFF}`);
	let typed = '';
	for (const char of 'explain the build') {
		typed += char;
		stream.chunk(`${SYNC_ON}${screen.draw(codexComposer(cols, typed, 'gpt-5-codex high · 100% context left'), {row: 0, col: 2 + typed.length})}${SYNC_OFF}`);
	}
	stream.checkpoint('typed');
	insertHistory([[['user', '1;36']], [['explain the build', '']], [[' ']]]);
	const spin = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴'];
	for (let tick = 0; tick < 6; tick++) {
		stream.chunk(`${tick ? SYNC_ON : ''}${screen.draw([[[`${spin[tick]} `, '36'], ['Working', '1'], [` (${tick}s • Esc to interrupt)`, '2']], ...codexComposer(cols, '', '98% context left')], undefined)}${SYNC_OFF}`);
	}
	stream.checkpoint('working');
	// A long answer: enough history to push the viewport to the bottom and scroll the region.
	for (let block = 0; block < 4; block++) {
		insertHistory([[['codex', '1;35']], ...Array.from({length: 6}, (_, index): Styled => [[`• step ${block * 6 + index + 1}: `, ''], ['tsc -p tsconfig.json', '33'], [` compiles src/ to dist/ — 日本語 ${'·'.repeat(index * 3)}`, '']]), [[' ']]]);
		stream.chunk(`${screen.draw([[[`${spin[block]} `, '36'], ['Working', '1'], [` (${6 + block}s • Esc to interrupt)`, '2']], ...codexComposer(cols, '', `${96 - block}% context left`)])}${SYNC_OFF}`);
	}
	stream.checkpoint('history-scrolled');
	stream.chunk(`${SYNC_ON}${screen.draw(codexComposer(cols, '', '92% context left'), {row: 0, col: 2})}${SYNC_OFF}`);
	// Resize: crossterm reports it; codex clears the viewport and repaints it at the new size.
	stream.resize(72, 20);
	screen.resize(72, viewportHeight);
	screen.moveTo(20 - viewportHeight + 1);
	stream.chunk(`${SYNC_ON}${cup(screen.viewportTop, 1)}${CSI}J${screen.draw(codexComposer(72, '', '92% context left'), {row: 0, col: 2})}${SYNC_OFF}`);
	stream.checkpoint('after-resize');
	// Exit: the viewport is cleared, modes popped, a final line printed below the history.
	stream.chunk(`${cup(screen.viewportTop, 1)}${CSI}J${CSI}<1u${CSI}?2004l${CSI}?1004l${CSI}?25h`);
	stream.chunk(`${sgr(2)}Token usage: total=1234 input=1000 output=234${RESET}\r\n`);
	return stream;
}

export function codexAlt(): StreamBuilder {
	const cols = 90, rows = 28;
	const stream = new StreamBuilder('synthetic-codex-alt', 'synthetic', 'Imitates a ratatui full-screen view (codex transcript overlay): normal-screen history, alternate screen enter (1049), full paint and cell diffs, mouse/keyboard modes, a resize repaint inside the alternate screen, leaving it restores the normal screen and cursor.', cols, rows);
	stream.chunk(onlcr(Array.from({length: 34}, (_, index) => `$ normal screen line ${index + 1}`).join('\n') + '\n$ '));
	stream.checkpoint('normal-before');
	const screen = new RatatuiScreen(cols, rows);
	stream.chunk(`${CSI}?1049h${CSI}?1000h${CSI}?1002h${CSI}?1006h${CSI}>7u${CSI}2J`);
	const transcript = (offset: number, width: number, height: number): Styled[] => {
		const body: Styled[] = Array.from({length: 80}, (_, index) => [[index % 5 === 0 ? 'user ' : 'codex ', index % 5 === 0 ? '1;36' : '1;35'], [`message ${index + 1} `, ''], [`${'═'.repeat(index % 12)}`, '38;5;208'], [index % 7 === 0 ? ' ✔ 完了' : '', '32']]);
		const visible = body.slice(offset, offset + height - 2);
		return [[[` T R A N S C R I P T `, '7'], [' '.repeat(Math.max(0, width - 40))], [`${offset + 1}/${body.length}`, '2']], ...visible, [[' ↑/↓ scroll   PgUp/PgDn page   q quit ', `${bgRgb(40, 40, 60)};${rgb(230, 230, 230)}`]]];
	};
	stream.chunk(`${SYNC_ON}${screen.draw(transcript(0, cols, rows))}${SYNC_OFF}`);
	stream.checkpoint('alt-painted');
	for (const offset of [1, 2, 3, 10, 25]) stream.chunk(`${SYNC_ON}${screen.draw(transcript(offset, cols, rows))}${SYNC_OFF}`);
	stream.checkpoint('alt-scrolled');
	stream.resize(70, 16);
	screen.resize(70, 16);
	stream.chunk(`${SYNC_ON}${CSI}2J${screen.draw(transcript(25, 70, 16))}${SYNC_OFF}`);
	stream.checkpoint('alt-resized');
	stream.chunk(`${CSI}?1006l${CSI}?1002l${CSI}?1000l${CSI}<1u${CSI}?1049l${CSI}?25h`);
	stream.checkpoint('normal-after');
	stream.chunk('echo back\r\nback\r\n$ ');
	return stream;
}

// ── pi (pi-tui differential rendering) ──────────────────────────────────────────────────────────────────────────

/** pi-tui's renderer: the first frame in full; later ones repaint from the first changed line, or everything when that line has scrolled away. */
class DiffRenderer {
	private previous: string[] = [];
	private cursorRow = 0; // row of the cursor within the rendered lines
	constructor(private height: number) {}
	resize(height: number): void { this.height = height; }
	render(lines: string[], forceFull = false): string {
		let out = SYNC_ON;
		const viewportTop = Math.max(0, this.previous.length - this.height);
		let first = lines.findIndex((line, index) => line !== this.previous[index]);
		if (first < 0) first = lines.length < this.previous.length ? lines.length : -1;
		if (first < 0 && !forceFull) return '';
		if (!this.previous.length || forceFull || first < viewportTop) {
			out += `${CSI}3J${CSI}2J${CSI}H${lines.join('\r\n')}`;
		} else {
			const delta = first - this.cursorRow;
			if (delta < 0) out += `${CSI}${-delta}A`;
			else if (delta > 0) out += `${CSI}${delta}B`;
			out += '\r';
			for (let index = first; index < lines.length; index++) {
				if (index > first) out += '\r\n';
				out += `${CSI}2K${lines[index]}`;
			}
			if (this.previous.length > lines.length) out += `\r\n${CSI}J${CSI}1A`;
		}
		this.cursorRow = Math.max(0, lines.length - 1);
		this.previous = lines;
		return `${out}${SYNC_OFF}`;
	}
}

export function piDiff(): StreamBuilder {
	const cols = 80, rows = 20;
	const stream = new StreamBuilder('synthetic-pi-diff', 'synthetic', 'Imitates pi (pi-tui): first frame in full, later frames repainted from the first changed line (cursor up/down + CR + 2K per line, J when shorter), full clear (3J 2J H) when the change is above the viewport, DEC 2026 around every frame, a resize forcing a full repaint.', cols, rows);
	const tui = new DiffRenderer(rows);
	stream.chunk(`${CSI}?25l${CSI}?2004h${OSC}0;π - fixture${BEL}`);
	const header = [`${sgr(1)}pi${RESET} ${sgr(2)}v0.0.0-fixture${RESET}`, `${sgr(2)}escape to interrupt · ctrl+c to clear · / for commands${RESET}`, ''];
	const editor = (text: string) => [`${sgr(38, 5, 240)}${'─'.repeat(cols)}${RESET}`, `${text}${sgr(7)} ${RESET}`, `${sgr(38, 5, 240)}${'─'.repeat(cols)}${RESET}`, `${sgr(2)}~/project (main)        ↑1.2k ↓300 $0.012 12.3%/200k     claude-sonnet${RESET}`];
	let messages: string[] = [];
	const frame = (text: string) => [...header, ...messages, ...editor(text)];
	stream.chunk(tui.render(frame('')));
	stream.checkpoint('first-frame');
	let typed = '';
	for (const char of 'fix the tests') { typed += char; stream.chunk(tui.render(frame(typed))); }
	stream.checkpoint('typed');
	messages = [`${sgr(48, 5, 236)} fix the tests ${RESET}`, ''];
	stream.chunk(tui.render(frame('')));
	// Streaming answer: the last line grows, lines get appended, the frame outgrows the screen.
	let answer = '';
	for (const word of 'Looking at the failing suite first. The snapshot test compares the rendered header which changed width when the version string grew; updating the fixture and the assertion fixes it.'.split(' ')) {
		answer = answer ? `${answer} ${word}` : word;
		const wrapped = answer.match(/.{1,78}(\s|$)/g)!.map(line => line.trimEnd());
		stream.chunk(tui.render([...header, ...messages, ...wrapped, ...editor('')]));
	}
	messages = [...messages, ...answer.match(/.{1,78}(\s|$)/g)!.map(line => line.trimEnd()), ''];
	for (let index = 0; index < 8; index++) {
		messages = [...messages, `${sgr(32)}✓${RESET} ${sgr(2)}edit${RESET} tests/fixture-${index}.test.ts ${sgr(2)}(+${index * 3} -${index})${RESET}`];
		stream.chunk(tui.render(frame('')));
	}
	stream.checkpoint('grown-past-screen');
	// A change above the viewport (the first message is edited): full clear and repaint.
	messages[0] = `${sgr(48, 5, 236)} fix the tests (edited) ${RESET}`;
	stream.chunk(tui.render(frame('')));
	stream.checkpoint('full-repaint');
	// Shrinking frame: the tail is erased with J.
	messages = messages.slice(0, messages.length - 5);
	stream.chunk(tui.render(frame('')));
	stream.checkpoint('shrunk');
	stream.resize(64, 14);
	tui.resize(14);
	stream.chunk(tui.render(frame('').map(line => line.replace(/─{80}/, '─'.repeat(64))), true));
	stream.checkpoint('after-resize');
	stream.chunk(`${CSI}?2004l${CSI}?25h\r\n`);
	return stream;
}

// ── Decoder and parser edge cases ───────────────────────────────────────────────────────────────────────────────

export function utf8Split(): StreamBuilder {
	const stream = new StreamBuilder('synthetic-utf8-split', 'synthetic', 'UTF-8 split across chunks at every byte position (2-, 3- and 4-byte sequences, ZWJ emoji, combining marks), escape sequences split mid-CSI/mid-OSC, invalid bytes (lone continuation, overlong, surrogate, 0xFF) and a truncated sequence at EOF (flushed as U+FFFD by the decoder).', 40, 10);
	const text = 'héllo wörld — 日本語テキスト 🎉 👩‍💻 é ñ̃\r\n';
	stream.every(text, 1);
	stream.checkpoint('one-byte-chunks');
	stream.every(text, 2);
	stream.every(text, 3);
	stream.checkpoint('two-and-three-byte-chunks');
	const emoji = Buffer.from('A🎉B', 'utf8'); // 41 F0 9F 8E 89 42
	stream.split(emoji, [2]).split(emoji, [3]).split(emoji, [4]).chunk('\r\n');
	stream.split(`${sgr(31)}red${RESET} ${OSC}0;split title${BEL}after\r\n`, [2, 3, 4, 12, 14, 20]);
	stream.checkpoint('split-sequences');
	stream.chunk(Buffer.from([0x41, 0x80, 0x42, 0xc0, 0xaf, 0x43, 0xed, 0xa0, 0x80, 0x44, 0xff, 0xfe, 0x45, 0x0d, 0x0a]));
	// A 3-byte sequence cut short by ASCII, and a 2-byte one cut by the chunk end then resumed with a non-continuation.
	stream.chunk(Buffer.from([0xe3, 0x81, 0x46, 0xc3]));
	stream.chunk(Buffer.from([0x47, 0x0d, 0x0a]));
	stream.checkpoint('invalid-bytes');
	stream.chunk(Buffer.from('tail: ', 'utf8'));
	stream.chunk(Buffer.from([0xf0, 0x9f, 0x8e]));
	return stream;
}

export function edgeCases(): StreamBuilder {
	const stream = new StreamBuilder('synthetic-edge-cases', 'synthetic', 'Small-screen parser edge cases: pending wrap at the last column, a wide char that does not fit the last column, DECAWM off, combining marks and emoji sequences, tab stops, ICH/DCH/ECH/IL/DL/REP, DECSC/DECRC, origin mode in a scroll region, RI/IND/NEL, IRM, DECALN, OSC titles and hyperlinks, DCS/APC strings, ED 3, RIS, and clamped/no-op resizes.', 20, 6, [0, 1, 3, 100]);
	stream.chunk('12345678901234567890');
	stream.checkpoint('pending-wrap');
	stream.chunk('X\r\n12345678901234567890\r\nnext\r\n');
	stream.chunk('123456789012345678日\r\n1234567890123456789日\r\n');
	stream.checkpoint('wide-at-edge');
	stream.chunk(`${CSI}?7labcdefghijklmnopqrstuvwxyz${CSI}?7h\r\n`);
	stream.chunk('é ạ̈ 👨‍👩‍👧 🇯🇵 ❤️ ☺︎ x\r\n');
	stream.checkpoint('combining');
	stream.chunk(`${CSI}2J${CSI}H${CSI}3g${CSI}1;4H${ESC}H${CSI}1;9H${ESC}H${CSI}H`);
	stream.chunk('a\tb\tc\td\r\n');
	stream.chunk(`${CSI}0g${CSI}H${CSI}2Bx${CSI}2Iy${CSI}Zz\r\n`);
	stream.chunk(`${CSI}4;1Habcdefghij${CSI}4;3H${CSI}2@${CSI}4;8H${CSI}3P${CSI}4;2H${CSI}2X`);
	stream.chunk(`${CSI}5;1H=${CSI}4b`);
	stream.checkpoint('tabs-and-editing');
	stream.chunk(`${CSI}2J${CSI}H1\r\n2\r\n3\r\n4\r\n5\r\n6`);
	stream.chunk(`${CSI}2;1H${CSI}L${CSI}5;1H${CSI}M`);
	stream.chunk(`${CSI}6;5H${ESC}7${CSI}1;1H@${ESC}8#${CSI}s${CSI}3;3H%${CSI}u&`);
	stream.checkpoint('lines-and-saves');
	stream.chunk(`${CSI}2;4r${CSI}?6h${CSI}1;1Horigin${CSI}10;1Hbottom\r\nscrolled${CSI}?6l${CSI}r`);
	stream.chunk(`${CSI}1;1H${ESC}M${ESC}Mreverse${CSI}6;1H${ESC}D${ESC}Eindex`);
	stream.checkpoint('regions');
	stream.chunk(`${CSI}4h${CSI}1;1HINS${CSI}4l${ESC}#8`);
	stream.checkpoint('decaln');
	stream.chunk(`${CSI}2J${CSI}H${OSC}2;window title${ST}${OSC}1;icon${BEL}${OSC}8;id=1;https://example.invalid/a${ST}link${OSC}8;;${ST} plain\r\n`);
	stream.chunk(`${ESC}P1$qm${ST}${ESC}_apc payload${ST}${ESC}^pm${ST}${OSC}52;c;aGVsbG8=${BEL}${CSI}?1049h${CSI}?1049l${CSI}>4;1m${CSI}=1;1u${CSI}99;99;99x still here\r\n`);
	stream.checkpoint('strings-ignored');
	stream.chunk(Array.from({length: 12}, (_, index) => `line ${index}`).join('\r\n'));
	stream.checkpoint('with-scrollback');
	stream.chunk(`${CSI}3J`);
	stream.checkpoint('scrollback-cleared');
	stream.resize(0, 0);
	stream.chunk('tiny');
	stream.checkpoint('clamped-1x1');
	stream.resize(20, 6);
	stream.resize(20, 6);
	stream.chunk('\r\nback to 20x6');
	stream.checkpoint('resized-back');
	stream.chunk(`${sgr(1, 4, 7)}styled${ESC}c after reset`);
	stream.checkpoint('after-ris');
	return stream;
}

export function c1Controls(): StreamBuilder {
	const stream = new StreamBuilder('synthetic-c1-controls', 'synthetic', 'C1 control code points arriving as UTF-8 (U+009B CSI, U+009D OSC, U+0085 NEL, U+0090 DCS) next to their 7-bit forms; kept separate so a terminal that treats them as printable diverges in one fixture only.', 30, 6);
	stream.chunk('a\u009b31mred\u009b0m b\r\n');
	stream.chunk('\u009d0;c1 title\u0007c\u0085d\r\n');
	stream.chunk('\u0090q\u009c e\r\n');
	return stream;
}

export function scrollbackReflow(): StreamBuilder {
	const stream = new StreamBuilder('synthetic-scrollback-reflow', 'synthetic', 'Scrollback and reflow: wrapped long lines, styled runs crossing the wrap, resizes narrower/wider/shorter/taller in the normal buffer (lines move between scrollback and screen), a resize inside the alternate screen, and scroll offsets across the scrollback.', 30, 8, [0, 1, 4, 10, 25, 1000]);
	for (let index = 0; index < 40; index++) {
		const long = index % 4 === 0 ? ` ${sgr(32)}${'wrapped-'.repeat(1 + (index % 9))}${RESET}` : '';
		stream.chunk(`${String(index).padStart(2, '0')} ${index % 3 ? 'short' : 'a somewhat longer line'}${long}\r\n`);
	}
	stream.chunk('$ ');
	stream.checkpoint('filled');
	stream.resize(20, 8);
	stream.checkpoint('narrower');
	stream.resize(45, 8);
	stream.checkpoint('wider');
	stream.resize(45, 4);
	stream.checkpoint('shorter');
	stream.resize(45, 12);
	stream.checkpoint('taller');
	stream.chunk(`${CSI}?1049h${CSI}H${CSI}2Jalt screen\r\nsecond line`);
	stream.checkpoint('alt');
	stream.resize(25, 6);
	stream.checkpoint('alt-resized');
	stream.chunk(`${CSI}?1049l`);
	stream.checkpoint('normal-again');
	stream.chunk('echo done\r\ndone\r\n$ ');
	return stream;
}

export const SYNTHETIC_STREAMS: Record<string, () => StreamBuilder> = {
	'synthetic-claude-ink': claudeInk,
	'synthetic-codex-inline': codexInline,
	'synthetic-codex-alt': codexAlt,
	'synthetic-pi-diff': piDiff,
	'synthetic-utf8-split': utf8Split,
	'synthetic-edge-cases': edgeCases,
	'synthetic-c1-controls': c1Controls,
	'synthetic-scrollback-reflow': scrollbackReflow,
};
