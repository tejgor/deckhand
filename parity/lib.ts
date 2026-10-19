/**
 * Parity fixture recording: runs the TypeScript oracle and writes `fixtures/parity/<module>/<fn>.jsonl` (one case per
 * line) or before/op/after directory cases, which `crates/parity` replays against the Rust port. See parity/README.md.
 *
 * Everything here is deterministic on purpose: identical inputs give byte-identical fixture files, so
 * `scripts/verify-unit` can regenerate fixtures and diff them against the committed ones.
 */
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {spawn} from 'node:child_process';
import {createRequire, syncBuiltinESMExports} from 'node:module';
import {fileURLToPath, pathToFileURL} from 'node:url';
import fc from 'fast-check';

export {fc};
export type Json = null | boolean | number | string | Json[] | {[key: string]: Json};

export const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
/** Where fixtures are written: `$PARITY_FIXTURES_OUT` (verify-unit regenerates into a temp dir) or fixtures/parity. */
export function fixturesRoot(): string {
	return process.env.PARITY_FIXTURES_OUT ? path.resolve(process.env.PARITY_FIXTURES_OUT) : path.join(REPO_ROOT, 'fixtures', 'parity');
}
export const DEFAULT_SEED = 0x5eed;
export const DEFAULT_CAP = 400;

// ---------------------------------------------------------------------------------------------------------------
// Tagged encoding of non-JSON values. Each tag is an object with exactly one `$`-key; crates/parity decodes them.
// ---------------------------------------------------------------------------------------------------------------

export const TAGS = ['$u', '$num', '$bigint', '$date', '$map', '$set', '$fn', '$screen', '$bytes', '$error', '$regexp', '$utf16', '$symbol', '$cycle', '$obj'] as const;
const TAG_SET = new Set<string>(TAGS);

const fnToName = new Map<Function, string>();
const nameToFn = new Map<string, Function>();
/** Names a function so it records as `{"$fn": name}` and decodes back to it; the Rust side resolves the same name. */
export function registerFn<T extends Function>(name: string, fn: T): T {
	fnToName.set(fn, name); nameToFn.set(name, fn);
	return fn;
}
export function registerFns(fns: Record<string, Function>): void { for (const [name, fn] of Object.entries(fns)) registerFn(name, fn); }

export interface EncodeOptions {
	/**
	 * `omit` (default): an object property holding `undefined` is dropped, as JSON.stringify and serde's
	 * skip_serializing_none do. `tag`: kept as `{"$u":1}` (use when `key in object` matters).
	 */
	undefinedProps?: 'omit' | 'tag';
}

/** An @xterm/headless Terminal (or anything shaped like one). */
function isScreen(value: any): boolean {
	return typeof value?.cols === 'number' && typeof value?.rows === 'number' && typeof value?.buffer?.active?.getLine === 'function';
}
/** Plain snapshot of a terminal screen: size, cursor and every buffer line (right-trimmed), scrollback included. */
export function screenSnapshot(terminal: any): Json {
	const buffer = terminal.buffer.active;
	const lines: string[] = [];
	for (let index = 0; index < buffer.length; index++) lines.push(buffer.getLine(index)?.translateToString(true) ?? '');
	return {cols: terminal.cols, rows: terminal.rows, cursor: {x: buffer.cursorX, y: buffer.cursorY}, baseY: buffer.baseY, viewportY: buffer.viewportY, type: buffer.type, lines};
}

export function encode(value: unknown, options: EncodeOptions = {}): Json {
	const ancestors = new Set<object>();
	const walk = (item: unknown): Json => {
		switch (typeof item) {
			case 'undefined': return {$u: 1};
			case 'boolean': return item;
			case 'number':
				if (Number.isNaN(item)) return {$num: 'NaN'};
				if (item === Infinity) return {$num: 'Infinity'};
				if (item === -Infinity) return {$num: '-Infinity'};
				return item === 0 ? 0 : item; // -0 records as 0, like JSON
			case 'string': return item.isWellFormed() ? item : {$utf16: Array.from({length: item.length}, (_, index) => item.charCodeAt(index))};
			case 'bigint': return {$bigint: item.toString()};
			case 'symbol': return {$symbol: item.description ?? ''};
			case 'function': return {$fn: fnToName.get(item) ?? (item.name || 'anonymous')};
		}
		if (item === null) return null;
		const object = item as any;
		if (ancestors.has(object)) return {$cycle: 1};
		ancestors.add(object);
		try {
			if (Array.isArray(object)) return Array.from(object, walk);
			if (object instanceof Date) return {$date: Number.isNaN(object.getTime()) ? null : object.toISOString()};
			if (object instanceof Map) return {$map: [...object].map(([key, entry]) => [walk(key), walk(entry)])};
			if (object instanceof Set) return {$set: [...object].map(walk)};
			if (object instanceof RegExp) return {$regexp: String(object)};
			if (object instanceof Error) {
				const error: {[key: string]: Json} = {name: object.name, message: object.message};
				if ((object as any).code !== undefined) error.code = walk((object as any).code);
				return {$error: error};
			}
			if (ArrayBuffer.isView(object)) return {$bytes: Buffer.from(object.buffer, object.byteOffset, object.byteLength).toString('base64')};
			if (object instanceof ArrayBuffer) return {$bytes: Buffer.from(object).toString('base64')};
			if (isScreen(object)) return {$screen: screenSnapshot(object)};
			if (typeof object.toJSON === 'function') return walk(object.toJSON());
			const out: {[key: string]: Json} = {};
			for (const key of Object.keys(object)) {
				const entry = object[key];
				if (entry === undefined && options.undefinedProps !== 'tag') continue;
				out[key] = walk(entry);
			}
			// A real object that looks like a tag is escaped so decoding is unambiguous.
			const keys = Object.keys(out);
			return keys.length === 1 && TAG_SET.has(keys[0]!) ? {$obj: out} : out;
		} finally {
			ancestors.delete(object);
		}
	};
	return walk(value);
}

export function decode(json: Json): unknown {
	if (json === null || typeof json !== 'object') return json;
	if (Array.isArray(json)) return json.map(decode);
	const keys = Object.keys(json);
	if (keys.length === 1 && TAG_SET.has(keys[0]!)) {
		const tag = keys[0]!, body = json[tag] as any;
		switch (tag) {
			case '$u': return undefined;
			case '$num': return Number(body);
			case '$bigint': return BigInt(body);
			case '$date': return new Date(body === null ? Number.NaN : body);
			case '$map': return new Map((body as Json[][]).map(([key, entry]) => [decode(key!), decode(entry!)]));
			case '$set': return new Set((body as Json[]).map(decode));
			case '$fn': {
				const fn = nameToFn.get(body);
				if (!fn) throw new Error(`parity: function "${body}" is not registered (registerFn)`);
				return fn;
			}
			case '$screen': return body;
			case '$bytes': return Buffer.from(body, 'base64');
			case '$error': return Object.assign(new Error(body.message), body.name === 'Error' ? {} : {name: body.name}, body.code === undefined ? {} : {code: decode(body.code)});
			case '$regexp': { const match = /^\/(.*)\/([a-z]*)$/s.exec(body)!; return new RegExp(match[1]!, match[2]); }
			case '$utf16': return String.fromCharCode(...(body as number[]));
			case '$symbol': return Symbol(body);
			case '$cycle': throw new Error('parity: cannot decode a cyclic reference');
			case '$obj': return Object.fromEntries(Object.entries(body as object).map(([key, entry]) => [key, decode(entry as Json)]));
		}
	}
	return Object.fromEntries(keys.map(key => [key, decode(json[key]!)]));
}

// ---------------------------------------------------------------------------------------------------------------
// Cases and call capture.
// ---------------------------------------------------------------------------------------------------------------

/** One fixture line. Key order on disk: args, argsAfter, async, then ret or throws. */
export interface RecordedCase {
	args: Json[];
	/** The arguments after the call, present only when the call mutated them. */
	argsAfter?: Json[];
	/** The call returned a promise; ret/throws are its settled value. */
	async?: true;
	ret?: Json;
	/** `Error.message` (or String(thrown)) when the call threw / rejected. */
	throws?: string;
}

export function caseLine(entry: RecordedCase): string {
	const out: {[key: string]: Json} = {args: entry.args};
	if (entry.argsAfter) out.argsAfter = entry.argsAfter;
	if (entry.async) out.async = true;
	if (entry.throws !== undefined) out.throws = entry.throws;
	else out.ret = entry.ret === undefined ? {$u: 1} : entry.ret;
	return JSON.stringify(out);
}
function messageOf(error: unknown): string {
	return error instanceof Error ? error.message : typeof error === 'string' ? error : String(error);
}
const isThenable = (value: unknown): value is PromiseLike<unknown> => typeof (value as any)?.then === 'function';

/**
 * Calls `fn` exactly as the caller would (same return value, same exception) and reports the recorded case.
 * Arguments are encoded before the call, so mutation is visible as `argsAfter`.
 */
export function invokeAndCapture(fn: Function, thisArg: unknown, args: unknown[], report: (entry: RecordedCase) => void, options: EncodeOptions = {}): unknown {
	let before: Json[];
	try { before = encode(args, options) as Json[]; }
	catch { return fn.apply(thisArg, args); }
	const beforeText = JSON.stringify(before);
	const finish = (outcome: {ret?: unknown; throws?: string}, async: boolean) => {
		try {
			const entry: RecordedCase = {args: before};
			const after = encode(args, options) as Json[];
			if (JSON.stringify(after) !== beforeText) entry.argsAfter = after;
			if (async) entry.async = true;
			if (outcome.throws !== undefined) entry.throws = outcome.throws;
			else entry.ret = encode(outcome.ret, options);
			report(entry);
		} catch {} // Recording must never change the oracle's behaviour.
	};
	let result: unknown;
	try { result = fn.apply(thisArg, args); }
	catch (error) { finish({throws: messageOf(error)}, false); throw error; }
	if (isThenable(result)) {
		(result as Promise<unknown>).then(value => finish({ret: value}, true), error => finish({throws: messageOf(error)}, true));
		return result;
	}
	finish({ret: result}, false);
	return result;
}

// ---------------------------------------------------------------------------------------------------------------
// Recorder: collects cases per function and writes deterministic jsonl files.
// ---------------------------------------------------------------------------------------------------------------

export interface RecorderOptions {
	/** Max cases per function file (default 400); larger sets are sampled evenly after sorting. */
	cap?: number;
	/** Applied to every line before dedupe (e.g. replace a temp path with a placeholder). */
	scrub?: (line: string) => string;
	encode?: EncodeOptions;
}
export interface TapRecord {module: string; fn: string; line: string}

/**
 * Lines are deduplicated and sorted (UTF-16 code-unit order), so the file does not depend on the order calls were
 * made in, then capped by even sampling. Case indices (used by allowlists) are line numbers from 0.
 */
export function finalizeLines(lines: Iterable<string>, cap = DEFAULT_CAP): string[] {
	const sorted = [...new Set(lines)].sort();
	if (sorted.length <= cap) return sorted;
	return Array.from({length: cap}, (_, index) => sorted[Math.floor((index * sorted.length) / cap)]!);
}

export class Recorder {
	private readonly lines = new Map<string, Set<string>>();
	constructor(readonly module: string, readonly options: RecorderOptions = {}) {}

	add(fn: string, entry: RecordedCase | string): void {
		let line = typeof entry === 'string' ? entry : caseLine(entry);
		if (this.options.scrub) line = this.options.scrub(line);
		let set = this.lines.get(fn);
		if (!set) this.lines.set(fn, set = new Set());
		set.add(line);
	}
	/** Calls `impl(...args)`, records the case under `fn`, and returns what the call returned (or rethrows). */
	call(fn: string, impl: Function, args: unknown[], thisArg?: unknown): unknown {
		return invokeAndCapture(impl, thisArg, args, entry => this.add(fn, entry), this.options.encode);
	}
	/** Records every argument list, awaiting async results one at a time; exceptions are recorded, not raised. */
	async callAll(fn: string, impl: Function, argLists: unknown[][], thisArg?: unknown): Promise<void> {
		for (const args of argLists) {
			try { await this.call(fn, impl, args, thisArg); } catch {}
		}
	}
	/** Adds tap records (from runTap), optionally only for `fromModule` and the listed functions. */
	addTap(records: TapRecord[], filter: {module?: string; fns?: string[]} = {}): void {
		for (const record of records) {
			if (filter.module !== undefined && record.module !== filter.module) continue;
			if (filter.fns && !filter.fns.includes(record.fn)) continue;
			this.add(record.fn, record.line);
		}
	}
	fns(): string[] { return [...this.lines.keys()].sort(); }
	/** The recorded cases of `fn` (parsed), in file order. */
	cases(fn: string): RecordedCase[] { return finalizeLines(this.lines.get(fn) ?? [], Infinity).map(line => JSON.parse(line)); }
	/** Decoded argument lists of `fn`, e.g. as seeds for mutate(). */
	argsOf(fn: string): unknown[][] { return this.cases(fn).map(entry => decode(entry.args) as unknown[]); }

	/**
	 * Writes `<fixturesRoot>/<module>/<fn>.jsonl` for every function. With `prune`, other .jsonl files in the module
	 * directory are removed (use one Recorder per module per gen script). Returns the written paths.
	 */
	async write(options: {prune?: boolean} = {}): Promise<string[]> {
		const directory = path.join(fixturesRoot(), this.module);
		await fsp.mkdir(directory, {recursive: true});
		const written: string[] = [];
		for (const fn of this.fns()) {
			const lines = finalizeLines(this.lines.get(fn)!, this.options.cap ?? DEFAULT_CAP);
			warnNondeterministic(this.module, fn, lines);
			const file = path.join(directory, `${fn}.jsonl`);
			await fsp.writeFile(file, lines.join('\n') + '\n');
			written.push(file);
		}
		if (options.prune) {
			for (const name of await fsp.readdir(directory)) {
				if (name.endsWith('.jsonl') && !written.includes(path.join(directory, name))) await fsp.rm(path.join(directory, name));
			}
		}
		return written;
	}
}
function warnNondeterministic(module: string, fn: string, lines: string[]): void {
	const outcomes = new Map<string, number>();
	for (const line of lines) {
		const parsed = JSON.parse(line) as RecordedCase;
		const key = JSON.stringify(parsed.args);
		outcomes.set(key, (outcomes.get(key) ?? 0) + 1);
	}
	const duplicated = [...outcomes.values()].filter(count => count > 1).length;
	if (duplicated) console.warn(`parity: ${module}/${fn}: ${duplicated} argument list(s) recorded with different outcomes (nondeterministic? mutated shared state?)`);
}

/** One-shot: records `fn` over `cases` (argument lists) and writes `<module>/<fnName>.jsonl`. */
export async function recordCalls(module: string, fnName: string, fn: Function, cases: unknown[][], options: RecorderOptions = {}): Promise<string> {
	const recorder = new Recorder(module, options);
	await recorder.callAll(fnName, fn, cases);
	return (await recorder.write())[0] ?? '';
}

// ---------------------------------------------------------------------------------------------------------------
// Determinism: fixed clock, seeded Math.random / randomUUID / randomBytes.
// ---------------------------------------------------------------------------------------------------------------

export function prng(seed = DEFAULT_SEED): () => number {
	let state = seed >>> 0;
	return () => { // mulberry32
		state = (state + 0x6d2b79f5) >>> 0;
		let t = state;
		t = Math.imul(t ^ (t >>> 15), t | 1);
		t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
		return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
	};
}
export interface DeterminismOptions {
	/** Start of the fixed clock (default 2026-01-01T00:00:00.000Z). */
	now?: string | number;
	/** Milliseconds the clock advances on every read (default 0: frozen). Use 1 for code that waits for time to pass. */
	tickMs?: number;
	seed?: number;
}
/** Installs a fixed clock and seeded randomness process-wide; returns a function that restores the originals. */
export function installDeterminism(options: DeterminismOptions = {}): () => void {
	const RealDate = Date;
	let clock = typeof options.now === 'number' ? options.now : RealDate.parse(options.now ?? '2026-01-01T00:00:00.000Z');
	const tick = options.tickMs ?? 0;
	const read = () => { const value = clock; clock += tick; return value; };
	class FixedDate extends RealDate {
		constructor(...args: any[]) {
			if (args.length === 0) super(read());
			else super(...(args as [any]));
		}
		static override now(): number { return read(); }
		static [Symbol.hasInstance](value: unknown): boolean { return value instanceof RealDate; }
	}
	const random = prng(options.seed ?? DEFAULT_SEED);
	const bytes = (size: number) => Buffer.from(Array.from({length: size}, () => Math.floor(random() * 256)));
	const uuid = () => {
		const b = bytes(16);
		b[6] = (b[6]! & 0x0f) | 0x40; b[8] = (b[8]! & 0x3f) | 0x80;
		const hex = b.toString('hex');
		return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
	};
	const crypto = createRequire(import.meta.url)('node:crypto');
	const saved = {date: globalThis.Date, random: Math.random, uuid: crypto.randomUUID, randomBytes: crypto.randomBytes, webUuid: globalThis.crypto?.randomUUID};
	globalThis.Date = FixedDate as DateConstructor;
	Math.random = random;
	crypto.randomUUID = uuid;
	crypto.randomBytes = (size: number, callback?: (error: Error | null, value: Buffer) => void) => {
		const value = bytes(size);
		if (callback) { queueMicrotask(() => callback(null, value)); return undefined; }
		return value;
	};
	if (globalThis.crypto) Object.defineProperty(globalThis.crypto, 'randomUUID', {value: uuid, configurable: true, writable: true});
	syncBuiltinESMExports(); // `import {randomUUID} from 'node:crypto'` sees the seeded version
	return () => {
		globalThis.Date = saved.date; Math.random = saved.random; crypto.randomUUID = saved.uuid; crypto.randomBytes = saved.randomBytes;
		if (globalThis.crypto) Object.defineProperty(globalThis.crypto, 'randomUUID', {value: saved.webUuid, configurable: true, writable: true});
		syncBuiltinESMExports();
	};
}
export async function withDeterminism<T>(run: () => T | Promise<T>, options: DeterminismOptions = {}): Promise<T> {
	const restore = installDeterminism(options);
	try { return await run(); } finally { restore(); }
}

// ---------------------------------------------------------------------------------------------------------------
// fast-check helpers (fixed seeds: same fast-check version => same samples) and input mutation.
// ---------------------------------------------------------------------------------------------------------------

export function sample<T>(arbitrary: fc.Arbitrary<T>, count: number, seed = DEFAULT_SEED): T[] {
	return fc.sample(arbitrary, {seed, numRuns: count});
}
/**
 * Characters that stress UTF-16 vs byte offsets and word/line logic: ASCII, whitespace, newline, a precomposed and a
 * combining accent, a surrogate pair, CJK (wide), ZWJ.
 */
export const EDGE_ALPHABET = ['a', 'b', 'Z', '0', '_', ' ', '\n', '\t', '-', '.', '/', 'é', 'é', '😀', '中', 'ß', '‍'];
export const arb = {
	text: (maxLength = 24, alphabet: string[] = EDGE_ALPHABET) => fc.array(fc.constantFrom(...alphabet), {maxLength}).map(parts => parts.join('')),
	/** A text and a UTF-16 offset into it (0..=length, possibly inside a surrogate pair). */
	textWithOffset: (maxLength = 24, alphabet: string[] = EDGE_ALPHABET) => arb.text(maxLength, alphabet).chain(text => fc.tuple(fc.constant(text), fc.integer({min: 0, max: text.length}))),
	/** Like textWithOffset but the offset is on a code-point boundary. */
	textWithBoundary: (maxLength = 24, alphabet: string[] = EDGE_ALPHABET) => arb.text(maxLength, alphabet).chain(text => {
		const boundaries = [0];
		for (const character of text) boundaries.push(boundaries.at(-1)! + character.length);
		return fc.tuple(fc.constant(text), fc.constantFrom(...boundaries));
	}),
};

function clone(value: unknown): unknown {
	if (Array.isArray(value)) return value.map(clone);
	if (value && typeof value === 'object' && Object.getPrototypeOf(value) === Object.prototype) return Object.fromEntries(Object.entries(value).map(([key, entry]) => [key, clone(entry)]));
	return value; // functions, dates, maps... are shared, not mutated
}
/**
 * Deterministic variants of recorded argument lists: each variant changes one leaf (string edit with edge characters,
 * integer ±1/0/×2, boolean flip, optional property dropped). Returns de-duplicated new argument lists only.
 */
export function mutate(inputs: unknown[][], options: {seed?: number; variants?: number; alphabet?: string[]} = {}): unknown[][] {
	const random = prng(options.seed ?? DEFAULT_SEED);
	const pick = <T>(items: T[]): T => items[Math.floor(random() * items.length)]!;
	const alphabet = options.alphabet ?? EDGE_ALPHABET;
	const seen = new Set(inputs.map(args => JSON.stringify(encode(args))));
	const out: unknown[][] = [];
	for (const args of inputs) {
		for (let variant = 0; variant < (options.variants ?? 3); variant++) {
			const copy = clone(args) as unknown[];
			const leaves: {parent: any; key: string | number}[] = [];
			const collect = (parent: any) => {
				for (const key of Array.isArray(parent) ? parent.keys() : Object.keys(parent)) {
					const value = parent[key];
					if (value && typeof value === 'object' && (Array.isArray(value) || Object.getPrototypeOf(value) === Object.prototype)) collect(value);
					else if (['string', 'number', 'boolean'].includes(typeof value)) leaves.push({parent, key});
				}
			};
			collect(copy);
			if (!leaves.length) continue;
			const {parent, key} = pick(leaves);
			const value = parent[key];
			if (typeof value === 'string') {
				const characters = Array.from(value);
				const at = Math.floor(random() * (characters.length + 1));
				const edit = pick(['insert', 'delete', 'duplicate', 'empty']);
				if (edit === 'insert' || !characters.length) characters.splice(at, 0, pick(alphabet));
				else if (edit === 'delete') characters.splice(Math.min(at, characters.length - 1), 1);
				else if (edit === 'duplicate') characters.push(...characters);
				else characters.length = 0;
				parent[key] = characters.join('');
			} else if (typeof value === 'number') {
				parent[key] = Number.isInteger(value) ? pick([value + 1, value - 1, 0, value * 2]) : value + 0.5;
			} else if (typeof value === 'boolean') {
				if (!Array.isArray(parent) && random() < 0.3) delete parent[key];
				else parent[key] = !value;
			}
			const signature = JSON.stringify(encode(copy));
			if (!seen.has(signature)) { seen.add(signature); out.push(copy); }
		}
	}
	return out;
}

// ---------------------------------------------------------------------------------------------------------------
// Tap: record the calls the existing TS tests make (see parity/tap.ts).
// ---------------------------------------------------------------------------------------------------------------

const TAP_BUFFER = Symbol.for('deckhand.parity.tap');
const TAP_LIMIT = 20_000; // distinct lines per function and process
type TapBuffer = Map<string, Set<string>>;
function tapBuffer(): TapBuffer {
	return ((globalThis as any)[TAP_BUFFER] ??= new Map()) as TapBuffer;
}
/** Used by the generated wrapper modules: a recorded call of an exported function. */
export function tapCall(module: string, fn: string, impl: Function, thisArg: unknown, args: unknown[]): unknown {
	return invokeAndCapture(impl, thisArg, args, entry => {
		const key = `${module}\t${fn}`;
		const buffer = tapBuffer();
		let lines = buffer.get(key);
		if (!lines) buffer.set(key, lines = new Set());
		if (lines.size < TAP_LIMIT) lines.add(caseLine(entry));
	});
}
/** Wraps an exported value if it is a plain (non-class) function; anything else is returned as is. */
export function tapWrap(module: string, fn: string, value: unknown): unknown {
	if (typeof value !== 'function' || /^class[\s{]/.test(Function.prototype.toString.call(value))) return value;
	return function (this: unknown, ...args: unknown[]) { return tapCall(module, fn, value, this, args); };
}
/** Writes this process's tap buffer to `<dir>/<pid>.tap` (called on exit by parity/tap.ts). */
export function flushTap(directory: string): void {
	const lines: string[] = [];
	for (const [key, set] of tapBuffer()) for (const line of set) lines.push(`${key}\t${line}`);
	if (lines.length) fs.writeFileSync(path.join(directory, `${process.pid}.tap`), lines.join('\n') + '\n');
}

export interface TapOptions {
	/** Test files, relative to the repo root (e.g. tests/textEditor.test.ts). */
	tests: string[];
	/** `module:fn` or `module:*` entries (module = src/<module>.ts|tsx). */
	tap: string[];
	/** Extra environment for the test process. */
	env?: Record<string, string>;
	/** Record even if the tests fail (default: throw). */
	allowFailures?: boolean;
	/** Install the fixed clock / seeded randomness inside the test processes (tickMs 1). */
	determinism?: boolean;
}
/**
 * Runs the TS tests with the tap loader and returns every recorded call. The test process gets a fresh temp HOME and
 * DECKHAND_HOME; tests run serially (--test-concurrency=1).
 */
export async function runTap(options: TapOptions): Promise<TapRecord[]> {
	const scratch = await fsp.realpath(await fsp.mkdtemp(path.join(os.tmpdir(), 'parity-tap-')));
	const spool = path.join(scratch, 'spool'), home = path.join(scratch, 'home');
	await fsp.mkdir(spool); await fsp.mkdir(home); await fsp.mkdir(path.join(scratch, 'deckhand-home'));
	try {
		const args = ['--import', 'tsx', '--import', pathToFileURL(path.join(REPO_ROOT, 'parity', 'tap.ts')).href, '--test', '--test-concurrency=1', ...options.tests];
		const env: NodeJS.ProcessEnv = {...process.env, HOME: home, DECKHAND_HOME: path.join(scratch, 'deckhand-home'), PARITY_TAP: options.tap.join(','), PARITY_TAP_OUT: spool, ...(options.determinism ? {PARITY_TAP_DETERMINISM: '1'} : {}), ...options.env};
		for (const key of ['DECKHAND_SESSION_ID', 'DECKHAND_LAUNCH_ID', 'DECKHAND_HOOK_TOKEN', 'NODE_TEST_CONTEXT']) delete env[key];
		const child = spawn(process.execPath, args, {cwd: REPO_ROOT, env, stdio: ['ignore', 'pipe', 'pipe']});
		let output = '';
		child.stdout.on('data', chunk => { output += chunk; });
		child.stderr.on('data', chunk => { output += chunk; });
		const code = await new Promise<number>((resolve, reject) => { child.once('error', reject); child.once('close', status => resolve(status ?? 1)); });
		if (code !== 0 && !options.allowFailures) throw new Error(`parity: tapped tests failed (exit ${code}):\n${output.slice(-4000)}`);
		const records: TapRecord[] = [];
		for (const name of (await fsp.readdir(spool)).sort()) {
			for (const raw of (await fsp.readFile(path.join(spool, name), 'utf8')).split('\n')) {
				if (!raw) continue;
				const [module, fn] = raw.split('\t', 2) as [string, string];
				records.push({module, fn, line: raw.slice(module.length + fn.length + 2)});
			}
		}
		if (!records.length) throw new Error(`parity: tap recorded nothing for ${options.tap.join(',')} (check PARITY_TAP names)\n${output.slice(-2000)}`);
		return records;
	} finally {
		await fsp.rm(scratch, {recursive: true, force: true});
	}
}

// ---------------------------------------------------------------------------------------------------------------
// File-based modules: fixtures/parity/<module>/<case>/{before/, after/, tree.json, op.json}.
// ---------------------------------------------------------------------------------------------------------------

export const ROOT_PLACEHOLDER = '$ROOT';
export interface TreeEntry {path: string; type: 'dir' | 'file' | 'symlink'; mode: string; target?: string}
export interface Tree {entries: TreeEntry[]; files: Map<string, Buffer>}

/**
 * Deterministic listing of `root` (root itself excluded): relative '/'-separated paths in code-unit order, octal
 * permission bits, symlink targets, and file bytes. Occurrences of `root` in contents/targets become `$ROOT`.
 */
export async function serializeTree(root: string): Promise<Tree> {
	const entries: TreeEntry[] = [], files = new Map<string, Buffer>();
	const walk = async (directory: string, prefix: string) => {
		for (const name of (await fsp.readdir(directory)).sort()) {
			const full = path.join(directory, name), relative = prefix ? `${prefix}/${name}` : name;
			const stat = await fsp.lstat(full);
			const mode = (stat.mode & 0o7777).toString(8).padStart(4, '0');
			if (stat.isSymbolicLink()) entries.push({path: relative, type: 'symlink', mode, target: scrubString(await fsp.readlink(full), root)});
			else if (stat.isDirectory()) { entries.push({path: relative, type: 'dir', mode}); await walk(full, relative); }
			else { entries.push({path: relative, type: 'file', mode}); files.set(relative, scrubBuffer(await fsp.readFile(full), root)); }
		}
	};
	await walk(root, '');
	entries.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
	return {entries, files};
}
export function scrubString(value: string, root: string): string { return value.split(root).join(ROOT_PLACEHOLDER); }
export function scrubBuffer(value: Buffer, root: string): Buffer {
	const needle = Buffer.from(root), parts: Buffer[] = [];
	let from = 0, at: number;
	while ((at = value.indexOf(needle, from)) >= 0) { parts.push(value.subarray(from, at), Buffer.from(ROOT_PLACEHOLDER)); from = at + needle.length; }
	if (!parts.length) return value;
	parts.push(value.subarray(from));
	return Buffer.concat(parts);
}
function scrubJson(value: Json, roots: string[]): Json {
	if (typeof value === 'string') return roots.reduce((text, root) => scrubString(text, root), value);
	if (Array.isArray(value)) return value.map(entry => scrubJson(entry, roots));
	if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([key, entry]) => [key, scrubJson(entry, roots)]));
	return value;
}
async function writeTree(directory: string, tree: Tree): Promise<void> {
	await fsp.mkdir(directory, {recursive: true});
	for (const entry of tree.entries) {
		// Directories and modes live in tree.json (Git keeps neither empty dirs nor modes); only file bytes go here.
		if (entry.type !== 'file') continue;
		await fsp.mkdir(path.dirname(path.join(directory, entry.path)), {recursive: true});
		await fsp.writeFile(path.join(directory, entry.path), tree.files.get(entry.path)!);
	}
}

export interface DirCaseSpec {
	/** Builds the "before" state inside the empty temp root. */
	setup: (root: string) => void | Promise<void>;
	/** Name the Rust replay dispatches on (op.json "fn"). */
	fn: string;
	/** Arguments of the operation (absolute paths under root become "$ROOT/..." in op.json). */
	args: (root: string) => unknown[];
	/** The operation under test. */
	run: (...args: any[]) => unknown;
	determinism?: DeterminismOptions | false;
}
/**
 * Records one file-based case into `<fixturesRoot>/<module>/<caseName>/`: before/ and after/ (file bytes, `$ROOT`
 * substituted), tree.json ({before, after} listings with modes/dirs/symlinks) and op.json ({fn, args, ret|throws}).
 * Runs with umask 022 and (by default) the fixed clock.
 */
export async function recordDirCase(module: string, caseName: string, spec: DirCaseSpec): Promise<string> {
	const tempRoot = await fsp.mkdtemp(path.join(os.tmpdir(), 'parity-dir-'));
	const root = await fsp.realpath(tempRoot);
	const previousUmask = process.umask(0o022);
	const restore = spec.determinism === false ? () => {} : installDeterminism(spec.determinism ?? {});
	try {
		await spec.setup(root);
		const before = await serializeTree(root);
		const args = spec.args(root);
		let entry: RecordedCase | undefined;
		try { await invokeAndCapture(spec.run, undefined, args, recorded => { entry = recorded; }); } catch {}
		await new Promise(resolve => setImmediate(resolve)); // let an async capture settle
		if (!entry) throw new Error(`parity: ${module}/${caseName}: operation was not captured`);
		const after = await serializeTree(root);
		const directory = path.join(fixturesRoot(), module, caseName);
		await fsp.rm(directory, {recursive: true, force: true});
		await writeTree(path.join(directory, 'before'), before);
		await writeTree(path.join(directory, 'after'), after);
		await fsp.writeFile(path.join(directory, 'tree.json'), JSON.stringify({before: before.entries, after: after.entries}, null, 2) + '\n');
		const scrubbed = scrubJson(JSON.parse(caseLine(entry)), [root, tempRoot]) as {[key: string]: Json};
		await fsp.writeFile(path.join(directory, 'op.json'), JSON.stringify({fn: spec.fn, ...scrubbed}, null, 2) + '\n');
		return directory;
	} finally {
		restore();
		process.umask(previousUmask);
		await fsp.rm(tempRoot, {recursive: true, force: true});
	}
}
