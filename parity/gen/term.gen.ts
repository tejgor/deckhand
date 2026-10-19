// Regenerates the terminal goldens (fixtures/term/<name>.golden.json) from the recorded streams
// (fixtures/term/streams/<name>.bin + .json). Deterministic: same streams, same goldens. Recording new streams is
// parity/gen/streams/record.ts, which is not part of regeneration.
//
//   node --import tsx parity/gen/term.gen.ts            # every stream
//   node --import tsx parity/gen/term.gen.ts --check    # exit 1 when a golden on disk differs
//
// Each stream is replayed the way a Deckhand worker feeds its preview: node-pty decodes the PTY bytes with a
// streaming UTF-8 decoder (socket.setEncoding('utf8') = string_decoder) and every non-empty decoded chunk goes to
// TerminalPreview.write without awaiting; resizes go to TerminalPreview.resize. At every checkpoint the golden records
// the @xterm/headless screen (from the preview's own Terminal) and the exact results of a fixed sequence of
// TerminalPreview calls, so the call order (and the snapshot caching it exercises) is part of the contract.

import fs from 'node:fs';
import path from 'node:path';
import {StringDecoder} from 'node:string_decoder';
import {createRequire} from 'node:module';
import {pathToFileURL} from 'node:url';
import type {Terminal as HeadlessTerminal, IBufferCell, IBufferLine} from '@xterm/headless';
import {TerminalPreview} from '../../src/terminalPreview.js';
import {DEFAULT_SCROLL_OFFSETS, TERM_FIXTURES, listStreams, readStream, type StreamMeta} from './streams/format.js';

const XTERM_VERSION = (createRequire(import.meta.url)('@xterm/headless/package.json') as {version: string}).version;

type Json = null | boolean | number | string | Json[] | {[key: string]: Json};

type Call = {fn: 'getCachedSnapshot' | 'getAnsiFrame' | 'getSnapshot' | 'getScrollInfo'; args?: number[]; result: Json};

// ── Screen snapshot ─────────────────────────────────────────────────────────────────────────────────────────────

// IBufferCell colour modes (xterm's Attributes.CM_*), as returned by getFgColorMode/getBgColorMode.
const CM_P16 = 0x1000000, CM_P256 = 0x2000000, CM_RGB = 0x3000000;

function color(mode: number, value: number): string {
	if (mode === CM_P16) return `p16:${value}`;
	if (mode === CM_P256) return `p256:${value}`;
	if (mode === CM_RGB) return `rgb:${value.toString(16).padStart(6, '0')}`;
	return 'd';
}

function flags(cell: IBufferCell): string {
	return [
		cell.isBold() ? 'b' : '', cell.isDim() ? 'd' : '', cell.isItalic() ? 'i' : '', cell.isUnderline() ? 'u' : '',
		cell.isBlink() ? 'k' : '', cell.isInverse() ? 'r' : '', cell.isInvisible() ? 'h' : '', cell.isStrikethrough() ? 's' : '',
		cell.isOverline() ? 'o' : '',
	].join('');
}

/** Non-default attribute runs of a line: [x, cells, fg, bg, flags]; cells outside every run have default attributes. */
function attributeRuns(line: IBufferLine, cols: number, cell: IBufferCell): Json[] {
	const runs: Array<[number, number, string, string, string]> = [];
	for (let x = 0; x < cols; x++) {
		if (!line.getCell(x, cell)) break;
		const fg = color(cell.getFgColorMode(), cell.getFgColor()), bg = color(cell.getBgColorMode(), cell.getBgColor()), fl = flags(cell);
		if (fg === 'd' && bg === 'd' && !fl) continue;
		const last = runs.at(-1);
		if (last && last[0] + last[1] === x && last[2] === fg && last[3] === bg && last[4] === fl) last[1]++;
		else runs.push([x, 1, fg, bg, fl]);
	}
	return runs;
}

function screenSnapshot(terminal: HeadlessTerminal, title: string | null): Json {
	const buffer = terminal.buffer.active;
	const cell = buffer.getNullCell();
	const lines: string[] = [];
	const wrapped: number[] = [];
	const attrs: Record<string, Json> = {};
	const wide: Record<string, Json> = {};
	for (let y = 0; y < terminal.rows; y++) {
		const line = buffer.getLine(buffer.baseY + y);
		if (!line) { lines.push(''); continue; }
		lines.push(line.translateToString(true));
		if (line.isWrapped) wrapped.push(y);
		const runs = attributeRuns(line, terminal.cols, cell);
		if (runs.length) attrs[String(y)] = runs;
		const wideCells: number[] = [];
		for (let x = 0; x < terminal.cols; x++) if (line.getCell(x, cell) && cell.getWidth() === 2) wideCells.push(x);
		if (wideCells.length) wide[String(y)] = wideCells;
	}
	const scrollback: string[] = [];
	const scrollbackWrapped: number[] = [];
	for (let y = 0; y < buffer.baseY; y++) {
		const line = buffer.getLine(y);
		scrollback.push(line ? line.translateToString(true) : '');
		if (line?.isWrapped) scrollbackWrapped.push(y);
	}
	return {
		cols: terminal.cols,
		rows: terminal.rows,
		buffer: buffer.type,
		cursor: {x: buffer.cursorX, y: buffer.cursorY},
		baseY: buffer.baseY,
		viewportY: buffer.viewportY,
		length: buffer.length,
		lines,
		wrapped,
		attrs,
		wide,
		scrollback,
		scrollbackWrapped,
		modes: {...terminal.modes} as unknown as Json,
		title,
	};
}

// ── Replay ──────────────────────────────────────────────────────────────────────────────────────────────────────

async function observe(preview: TerminalPreview, offsets: number[]): Promise<Call[]> {
	const calls: Call[] = [];
	calls.push({fn: 'getCachedSnapshot', result: preview.getCachedSnapshot()});
	calls.push({fn: 'getAnsiFrame', result: await preview.getAnsiFrame()});
	calls.push({fn: 'getCachedSnapshot', result: preview.getCachedSnapshot()});
	for (const offset of offsets) {
		calls.push({fn: 'getSnapshot', args: [offset], result: await preview.getSnapshot(offset)});
		calls.push({fn: 'getScrollInfo', args: [offset], result: await preview.getScrollInfo(offset)});
	}
	calls.push({fn: 'getCachedSnapshot', result: preview.getCachedSnapshot()});
	calls.push({fn: 'getSnapshot', result: await preview.getSnapshot()});
	calls.push({fn: 'getScrollInfo', result: await preview.getScrollInfo()});
	return calls;
}

export async function golden(meta: StreamMeta, bytes: Buffer): Promise<Json> {
	const preview = new TerminalPreview(meta.cols, meta.rows);
	// The preview's own terminal: the screen snapshot must describe exactly what the preview read from.
	const internals = preview as unknown as {terminal: HeadlessTerminal; pending: Promise<void>};
	let title: string | null = null;
	internals.terminal.onTitleChange(next => { title = next; });
	const offsets = meta.scrollOffsets ?? DEFAULT_SCROLL_OFFSETS;
	const ops: Json[] = [{op: 'new', cols: meta.cols, rows: meta.rows}];
	const decoder = new StringDecoder('utf8');
	let eventIndex = 0;
	const checkpoint = async (label: string, at: number) => {
		// Settle the writes and resizes queued so far without touching the preview's snapshot state.
		await internals.pending;
		const screen = screenSnapshot(internals.terminal, title);
		ops.push({op: 'checkpoint', label, at, screen, calls: await observe(preview, offsets) as unknown as Json});
	};
	const eventsAt = async (offset: number) => {
		while (eventIndex < meta.events.length && meta.events[eventIndex]!.at === offset) {
			const event = meta.events[eventIndex++]!;
			if (event.type === 'resize') {
				ops.push({op: 'resize', cols: event.cols, rows: event.rows});
				void preview.resize(event.cols, event.rows);
				// What a worker does after resizing a preview: snapshot it (see sessionWorker resizeRuntime/resizeAgent).
				await checkpoint(`after-resize-${event.cols}x${event.rows}`, offset);
			} else await checkpoint(event.label, offset);
		}
	};
	let offset = 0;
	await eventsAt(0);
	for (const length of meta.chunks) {
		const data = decoder.write(bytes.subarray(offset, offset + length));
		ops.push({op: 'write', from: offset, to: offset + length, ...data ? {} : {empty: true}});
		// node-pty emits no data event for a chunk that decodes to nothing (an incomplete UTF-8 sequence).
		if (data) void preview.write(data);
		offset += length;
		await eventsAt(offset);
	}
	// EOF: the decoder flushes an incomplete trailing sequence (as U+FFFD) as one last data event.
	const tail = decoder.end();
	if (tail) { ops.push({op: 'eof'}); void preview.write(tail); }
	await checkpoint('end', offset);
	preview.dispose();
	return {
		name: meta.name,
		source: meta.source,
		description: meta.description,
		oracle: {terminalPreview: 'src/terminalPreview.ts', xtermHeadless: XTERM_VERSION, scrollback: 10000, decoder: 'string_decoder utf8'},
		cols: meta.cols,
		rows: meta.rows,
		scrollOffsets: offsets,
		ops,
	};
}

// ── Output ──────────────────────────────────────────────────────────────────────────────────────────────────────

/** JSON with 1-tab-free 2-space indentation, arrays of scalars kept on one line (smaller, readable diffs). */
export function formatJson(value: Json, indent = ''): string {
	if (value === null || typeof value !== 'object') return JSON.stringify(value);
	const inner = `${indent}  `;
	if (Array.isArray(value)) {
		if (!value.length) return '[]';
		if (value.every(item => item === null || typeof item !== 'object' || (Array.isArray(item) && item.every(leaf => leaf === null || typeof leaf !== 'object')))) {
			return `[${value.map(item => formatJson(item)).join(', ')}]`;
		}
		return `[\n${value.map(item => `${inner}${formatJson(item, inner)}`).join(',\n')}\n${indent}]`;
	}
	const entries = Object.entries(value).filter(([, item]) => item !== undefined);
	if (!entries.length) return '{}';
	return `{\n${entries.map(([key, item]) => `${inner}${JSON.stringify(key)}: ${formatJson(item, inner)}`).join(',\n')}\n${indent}}`;
}

async function main(): Promise<void> {
	const check = process.argv.includes('--check');
	const names = process.argv.slice(2).filter(arg => !arg.startsWith('--'));
	const streams = names.length ? names : listStreams();
	if (!streams.length) throw new Error('no streams in fixtures/term/streams (record them with parity/gen/streams/record.ts)');
	let stale = 0;
	for (const name of streams) {
		const {meta, bytes} = readStream(name);
		const text = `${formatJson(await golden(meta, bytes))}\n`;
		const file = path.join(TERM_FIXTURES, `${name}.golden.json`);
		if (check) {
			const current = fs.existsSync(file) ? fs.readFileSync(file, 'utf8') : undefined;
			if (current !== text) { stale++; console.log(`stale: ${path.relative(process.cwd(), file)}`); }
			continue;
		}
		fs.writeFileSync(file, text);
		console.log(`wrote ${path.relative(process.cwd(), file)} (${text.length} bytes)`);
	}
	if (check && stale) process.exit(1);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) await main();
