// Regenerates string_decoder_utf8.json: Node's StringDecoder('utf8') outputs for chunked byte streams, the oracle
// for deckhand_term::utf8::Utf8Decoder (node-pty decodes PTY output with it via socket.setEncoding('utf8')).
//
//   node crates/term/tests/data/gen_string_decoder.mjs crates/term/tests/data/string_decoder_utf8.json
//
// Deterministic (seeded PRNG). Each case: chunks (hex), the string each write() returned, and end().
import fs from 'node:fs';
import {StringDecoder} from 'node:string_decoder';

const hex = bytes => Buffer.from(bytes).toString('hex');

function run(chunks) {
	const decoder = new StringDecoder('utf8');
	const writes = chunks.map(chunk => decoder.write(Buffer.from(chunk)));
	return {chunks: chunks.map(hex), writes, end: decoder.end()};
}

const enc = text => [...Buffer.from(text, 'utf8')];
const cases = [];
const add = (name, chunks) => cases.push({name, ...run(chunks)});

// Hand-written edge cases.
add('ascii', [enc('hello')]);
add('empty-chunks', [[], enc('a'), [], []]);
for (const ch of ['é', '€', '😀', '中文', 'a€b😀c']) {
	const bytes = enc(ch);
	for (let cut = 1; cut < bytes.length; cut++) add(`split-${ch}-${cut}`, [bytes.slice(0, cut), bytes.slice(cut)]);
	add(`bytewise-${ch}`, bytes.map(b => [b]));
}
add('truncated-3-at-end', [[0xe2, 0x82]]);
add('truncated-4-at-end', [[0xf0, 0x9f, 0x98]]);
add('truncated-then-ascii', [[0xe2, 0x82], enc('A')]);
add('truncated-then-lead', [[0xe2], [0xe2, 0x82, 0xac]]);
add('truncated-then-partial-lead', [[0xf0, 0x9f], [0x98, 0x41]]);
add('lone-continuation', [[0x80]]);
add('lone-continuations', [[0x80, 0x81, 0x82, 0x83, 0x84]]);
add('continuations-at-end', [enc('ab'), [0x80, 0x80]]);
add('overlong-c0', [[0xc0, 0x80]]);
add('overlong-c1-held', [[0xc1], [0xbf]]);
add('overlong-e0', [[0xe0, 0x80, 0x80]]);
add('overlong-f0', [[0xf0, 0x80, 0x80, 0x80]]);
add('surrogate-ed-a0', [[0xed, 0xa0, 0x80]]);
add('surrogate-ed-split', [[0xed], [0xa0], [0x80]]);
add('max-f4-8f', [[0xf4, 0x8f, 0xbf, 0xbf]]);
add('too-big-f4-90', [[0xf4, 0x90, 0x80, 0x80]]);
add('f5-held', [[0xf5], [0x80, 0x80, 0x80]]);
add('f5-end', [[0xf5]]);
add('f8-ff', [[0xf8, 0xfc, 0xfe, 0xff]]);
add('ff-end', [enc('x'), [0xff]]);
add('lead-then-lead', [[0xc3, 0xc3, 0xa9]]);
add('lead-split-lead', [[0xc3], [0xc3, 0xa9]]);
add('four-continuations-after-lead', [[0xe2, 0x82, 0xac, 0x80, 0x80]]);
add('split-after-extra-continuation', [[0xe2, 0x82, 0xac, 0x80], [0x80]]);
add('incomplete-across-three', [[0xf0], [0x9f], [0x98], [0x80]]);
add('incomplete-across-three-broken', [[0xf0], [0x9f], [0x41], [0x80]]);
add('esc-sequence', [enc('\x1b[31mred\x1b[0m')]);
add('end-with-c0-alone', [[0xc0]]);
add('end-with-e0-80', [[0xe0], [0x80]]);
add('mixed-invalid', [[0x41, 0xc3, 0x28, 0xa0, 0xa1, 0xe2, 0x28, 0xa1, 0xe2, 0x82, 0x28, 0xf0, 0x28, 0x8c, 0xbc]]);

// Seeded random streams from an alphabet heavy in boundary bytes.
let seed = 0x5eed1234;
const rand = () => { seed = (Math.imul(seed, 1103515245) + 12345) & 0x7fffffff; return seed / 0x80000000; };
const pick = list => list[Math.floor(rand() * list.length)];
const atoms = [
	[0x41], [0x7f], [0x1b], [0x80], [0xbf], [0xc0], [0xc1], [0xc2], [0xdf], [0xe0], [0xed], [0xef], [0xf0], [0xf4], [0xf5], [0xf8], [0xff],
	[0xa0], [0x9f], [0x90], [0x8f],
	enc('é'), enc('€'), enc('😀'), enc('中'), enc('\u{10ffff}'), enc('\u{ffff}'), enc('abc'),
];
for (let index = 0; index < 600; index++) {
	const bytes = [];
	const count = 1 + Math.floor(rand() * 14);
	for (let n = 0; n < count; n++) bytes.push(...pick(atoms));
	const chunks = [];
	let offset = 0;
	while (offset < bytes.length) {
		const length = Math.floor(rand() * 5);
		chunks.push(bytes.slice(offset, offset + length));
		offset += length;
	}
	add(`random-${index}`, chunks);
}

const out = process.argv[2];
if (!out) throw new Error('usage: gen_string_decoder.mjs <out.json>');
const body = cases.map(item => JSON.stringify(item)).join(',\n');
fs.writeFileSync(out, `{"node": ${JSON.stringify(process.version)}, "cases": [\n${body}\n]}\n`);
console.log(`wrote ${cases.length} cases to ${out}`);
