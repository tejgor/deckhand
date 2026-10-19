/**
 * client fixtures: the pure framing helpers of src/client.ts (writeMessage, attachJsonParser), driven through a fake
 * socket. request / ensureDaemonRunning / openPersistentConnection / LiveClient need a live daemon or spawn processes
 * and are covered by ported tests, not recorded here. No existing TS test calls the helpers directly (attach.ts
 * only reaches them through live sockets), so there is nothing to tap.
 *
 *   node --import tsx parity/gen/client.gen.ts
 *
 * client/writeMessage      args [message]              -> the exact text written to the socket
 * client/attachJsonParser  args [chunks, detachAfter?] -> {messages, error?, listeners}; a chunk is {text} (string data)
 *                          or {bytes} (Buffer data, decoded as streaming UTF-8)
 */
import {EventEmitter} from 'node:events';
import {Recorder, fc, installDeterminism, mutate, sample} from '../lib.ts';
import {attachJsonParser, writeMessage} from '../../src/client.ts';

delete process.env['DECKHAND_WIRE_LOG'];
installDeterminism();
const recorder = new Recorder('client');

type Chunk = {text: string} | {bytes: Buffer};

function writeMessageFixture(message: unknown): string {
	let written = '';
	const socket: any = {write(data: string) { written += data; return true; }};
	writeMessage(socket, message as any);
	return written;
}

function attachJsonParserFixture(chunks: Chunk[], detachAfter?: number): {messages: unknown[]; error?: string; listeners: number} {
	const socket: any = new EventEmitter();
	const messages: unknown[] = [];
	const stop = attachJsonParser(socket, message => messages.push(message));
	let error: string | undefined;
	let detached = false;
	for (let index = 0; index < chunks.length; index++) {
		const chunk = chunks[index]!;
		try { socket.emit('data', 'text' in chunk ? chunk.text : Buffer.from(chunk.bytes)); }
		catch (thrown) { error = thrown instanceof Error ? thrown.message : String(thrown); break; }
		if (detachAfter !== undefined && index === detachAfter) { stop(); detached = true; break; }
	}
	const listeners = socket.listenerCount('data');
	if (!detached) stop();
	return error === undefined ? {messages, listeners} : {messages, error, listeners};
}

// writeMessage
const messages: unknown[] = [
	{type: 'ping', requestId: 'r1'},
	{type: 'input', sessionId: 's', data: ''},
	{type: 'input', sessionId: 's', data: 'héllo 😀 日本語  \n\t"\\'},
	{type: 'input', sessionId: 's', data: '\x1b[A\x00\x7f'},
	{type: 'x', n: [1, 2.5, -0, 1e21, null], nested: {a: {b: [true, false]}, u: undefined}},
	{type: 'lone', data: '\uD800'},
	{},
];
for (const message of messages) recorder.call('writeMessage', writeMessageFixture, [message]);
for (const args of mutate(messages.map(message => [message]), {variants: 2})) recorder.call('writeMessage', writeMessageFixture, args);
for (const text of sample(fc.string({unit: 'binary', maxLength: 20}), 60)) recorder.call('writeMessage', writeMessageFixture, [{type: 'input', sessionId: 's', data: text}]);

// attachJsonParser
const T = (text: string): Chunk => ({text});
const bytes = (...values: number[]): Chunk => ({bytes: Buffer.from(values)});
const B = (text: string): Chunk => ({bytes: Buffer.from(text, 'utf8')});
const msg = (value: unknown) => JSON.stringify(value);
const cases: Array<[Chunk[], number?]> = [
	[[]],
	[[T('')]],
	[[T(`${msg({a: 1})}\n`)]],
	[[T(msg({a: 1}))]],
	[[T(`${msg({a: 1})}\n${msg({b: 2})}\n`)]],
	[[T(`${msg({a: 1})}\n${msg({b: 2})}`), T('\n')]],
	[[T('{"a":'), T('1}'), T('\n')]],
	[[T('\n\n  \n\t\n')]],
	[[T(`  ${msg({a: 1})}  \r\n`)]],
	[[T(` ${msg({a: 1})} \n﻿${msg({b: 2})}﻿\n`)]],
	[[T(`${msg('line sep')}\n`)]],
	[[T(`${msg({a: 1})}\nnot json\n${msg({b: 2})}\n`)]],
	[[T('{"a":\n')]],
	[[T('nope\n')]],
	[[T('\n')]],
	[[T(`${msg(null)}\n${msg(1)}\n${msg('s')}\n${msg([1, 2])}\n`)]],
	[[B(`${msg({text: 'héllo 😀 日本語'})}\n`)]],
	[[bytes(0xf0, 0x9f), bytes(0x98, 0x80)]],
	[[B('{"t":"'), bytes(0xf0, 0x9f), bytes(0x98, 0x80, 0x22, 0x7d, 0x0a)]],
	[[B('{"t":"'), bytes(0xe6, 0x97), bytes(0xa5, 0x22, 0x7d, 0x0a)]],
	[[bytes(0xff, 0xfe, 0x0a)]],
	[[bytes(0x7b, 0x7d, 0x0a, 0xe6, 0x97)]],
	[[T(`${msg({a: 1})}\n`), T(`${msg({b: 2})}\n`), T(`${msg({c: 3})}\n`)], 0],
	[[T(`${msg({a: 1})}\n${msg({b: 2})}\n`), T(`${msg({c: 3})}\n`)], 0],
	[[T(`${msg({a: 1})}\n`)], 5],
];
for (const [chunks, detachAfter] of cases) recorder.call('attachJsonParser', attachJsonParserFixture, detachAfter === undefined ? [chunks] : [chunks, detachAfter]);
for (const args of mutate(recorder.argsOf('attachJsonParser'), {variants: 3})) recorder.call('attachJsonParser', attachJsonParserFixture, args);

// fast-check: valid messages split at arbitrary byte boundaries (as Buffers) or whole-codepoint boundaries (as strings).
const value = fc.oneof(fc.string({unit: 'binary', maxLength: 12}).filter(text => text.isWellFormed()), fc.integer(), fc.constant(null), fc.boolean());
const message = fc.record({type: fc.constantFrom('a', 'output', 'ping'), data: value, sessionId: fc.constantFrom('s1', 'ü', '😀')});
const framed = fc.record({
	list: fc.array(fc.tuple(message, fc.constantFrom('\n', '\r\n', '\n\n', ' \n')), {maxLength: 5}),
	cuts: fc.array(fc.nat(400), {maxLength: 4}),
	asBytes: fc.boolean(),
});
for (const {list, cuts, asBytes} of sample(framed, 120)) {
	const buffer = Buffer.from(list.map(([item, sep]) => `${JSON.stringify(item)}${sep}`).join(''), 'utf8');
	const points = [...new Set(cuts.map(cut => Math.min(cut, buffer.length)))].sort((a, b) => a - b);
	const chunks: Chunk[] = [];
	let from = 0;
	for (const point of [...points, buffer.length]) {
		const slice = buffer.subarray(from, point);
		from = point;
		if (asBytes) chunks.push({bytes: Buffer.from(slice)});
		else if (slice.length) chunks.push(T(slice.toString('utf8').replaceAll('�', '')));
	}
	recorder.call('attachJsonParser', attachJsonParserFixture, [chunks]);
}

await recorder.write({prune: true});
