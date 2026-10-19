// Shared format of the terminal stream corpus (fixtures/term/streams/<name>.bin + <name>.json).
//
// A stream is the raw bytes a PTY master produced, in the chunks they arrived in (node-pty `onData` granularity), plus
// events that happened between chunks: a resize of the terminal, or a checkpoint at which the goldens observe the
// screen. parity/gen/term.gen.ts replays a stream into the TS oracle; the Rust port (unit T1) replays the same bytes.

import fs from 'node:fs';
import path from 'node:path';

export type StreamSource = 'sh' | 'lazygit' | 'synthetic';

export type StreamEvent =
	/** The terminal (PTY and preview) was resized after `at` bytes. */
	| {at: number; type: 'resize'; cols: number; rows: number}
	/** The goldens observe the preview and the screen after `at` bytes. */
	| {at: number; type: 'checkpoint'; label: string};

export interface StreamMeta {
	name: string;
	source: StreamSource;
	description: string;
	/** The size the preview is constructed with (TerminalPreview's constructor clamps it like the TS code does). */
	cols: number;
	rows: number;
	/** Byte length of each chunk, in arrival order; they sum to the .bin file's size. */
	chunks: number[];
	/** In stream order; every `at` lies on a chunk boundary. Events at the same offset apply in the listed order. */
	events: StreamEvent[];
	/** Scroll offsets probed at every checkpoint (getSnapshot/getScrollInfo); defaults to DEFAULT_SCROLL_OFFSETS. */
	scrollOffsets?: number[];
	/** How it was recorded (commands, keys, environment), for a human re-recording it. */
	recording?: Record<string, unknown>;
}

/** Offsets that cover the clamp paths of normalizeScrollOffset: zero, small, fractional, negative and far too large. */
export const DEFAULT_SCROLL_OFFSETS = [0, 1, 2.5, 7, -3, 1_000_000];

export const ROOT = path.resolve(path.dirname(new URL(import.meta.url).pathname), '..', '..', '..');
export const TERM_FIXTURES = path.join(ROOT, 'fixtures', 'term');
export const STREAMS_DIR = path.join(TERM_FIXTURES, 'streams');

/** Builds a stream chunk by chunk. Strings are UTF-8 encoded; Buffers are taken as raw bytes (invalid UTF-8 allowed). */
export class StreamBuilder {
	private readonly parts: Buffer[] = [];
	private readonly events: StreamEvent[] = [];
	private offset = 0;

	constructor(readonly name: string, readonly source: StreamSource, readonly description: string, readonly cols: number, readonly rows: number, readonly scrollOffsets?: number[]) {}

	/** One chunk (one node-pty `onData`). Empty data is ignored. */
	chunk(data: string | Buffer): this {
		const bytes = typeof data === 'string' ? Buffer.from(data, 'utf8') : data;
		if (!bytes.length) return this;
		this.parts.push(bytes);
		this.offset += bytes.length;
		return this;
	}

	/** One chunk per element. */
	chunks(...data: Array<string | Buffer>): this {
		for (const item of data) this.chunk(item);
		return this;
	}

	/** `data` cut into chunks at the given byte offsets (to split UTF-8 sequences or escape sequences). */
	split(data: string | Buffer, cuts: number[]): this {
		const bytes = typeof data === 'string' ? Buffer.from(data, 'utf8') : data;
		let start = 0;
		for (const cut of [...cuts].sort((a, b) => a - b)) {
			if (cut <= start || cut >= bytes.length) continue;
			this.chunk(bytes.subarray(start, cut));
			start = cut;
		}
		return this.chunk(bytes.subarray(start));
	}

	/** `data` cut into chunks of `size` bytes (sequences end up split wherever the cut falls). */
	every(data: string | Buffer, size: number): this {
		const bytes = typeof data === 'string' ? Buffer.from(data, 'utf8') : data;
		for (let start = 0; start < bytes.length; start += size) this.chunk(bytes.subarray(start, start + size));
		return this;
	}

	resize(cols: number, rows: number): this {
		this.events.push({at: this.offset, type: 'resize', cols, rows});
		return this;
	}

	checkpoint(label: string): this {
		this.events.push({at: this.offset, type: 'checkpoint', label});
		return this;
	}

	build(recording?: Record<string, unknown>): {meta: StreamMeta; bytes: Buffer} {
		return {
			meta: {name: this.name, source: this.source, description: this.description, cols: this.cols, rows: this.rows, chunks: this.parts.map(part => part.length), events: this.events, ...this.scrollOffsets ? {scrollOffsets: this.scrollOffsets} : {}, ...recording ? {recording} : {}},
			bytes: Buffer.concat(this.parts),
		};
	}
}

export function writeStream(meta: StreamMeta, bytes: Buffer): void {
	validateStream(meta, bytes.length);
	fs.mkdirSync(STREAMS_DIR, {recursive: true});
	fs.writeFileSync(path.join(STREAMS_DIR, `${meta.name}.bin`), bytes);
	fs.writeFileSync(path.join(STREAMS_DIR, `${meta.name}.json`), `${JSON.stringify(meta, null, 2)}\n`);
}

export function readStream(name: string): {meta: StreamMeta; bytes: Buffer} {
	const meta = JSON.parse(fs.readFileSync(path.join(STREAMS_DIR, `${name}.json`), 'utf8')) as StreamMeta;
	const bytes = fs.readFileSync(path.join(STREAMS_DIR, `${name}.bin`));
	validateStream(meta, bytes.length);
	return {meta, bytes};
}

export function listStreams(): string[] {
	if (!fs.existsSync(STREAMS_DIR)) return [];
	return fs.readdirSync(STREAMS_DIR).filter(name => name.endsWith('.json')).map(name => name.slice(0, -5)).sort();
}

export function validateStream(meta: StreamMeta, size: number): void {
	const total = meta.chunks.reduce((sum, length) => sum + length, 0);
	if (total !== size) throw new Error(`${meta.name}: chunks sum to ${total}, file has ${size} bytes`);
	if (meta.chunks.some(length => !Number.isInteger(length) || length <= 0)) throw new Error(`${meta.name}: chunk lengths must be positive integers`);
	const boundaries = new Set<number>([0]);
	let offset = 0;
	for (const length of meta.chunks) boundaries.add(offset += length);
	let previous = 0;
	for (const event of meta.events) {
		if (!boundaries.has(event.at)) throw new Error(`${meta.name}: event at ${event.at} is not on a chunk boundary`);
		if (event.at < previous) throw new Error(`${meta.name}: events out of order at ${event.at}`);
		previous = event.at;
	}
}
