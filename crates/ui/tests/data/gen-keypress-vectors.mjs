// Generates keypress-vectors.json: what Ink 6 (node_modules/ink) makes of raw stdin bytes, replayed by
// crates/ui/tests/keypress_vectors.rs against deckhand_ui::input.
//
//   node crates/ui/tests/data/gen-keypress-vectors.mjs > crates/ui/tests/data/keypress-vectors.json
//
// The pipeline is Ink's own: stdin.setEncoding('utf8') (a StringDecoder over the bytes), App's input parser
// (input-parser.js: one event per key, a trailing lone escape held until the next tick), parseKeypress and the
// Key/input mapping of hooks/use-input.js (copied below: it lives inside a React hook). A `null` chunk is the
// setImmediate that flushes a held escape when no more input arrived; after the last chunk the flush always runs.
import {StringDecoder} from 'node:string_decoder';

const ink = new URL('../../../../node_modules/ink/build/', import.meta.url);
const {createInputParser} = await import(new URL('input-parser.js', ink));
const {default: parseKeypress, nonAlphanumericKeys} = await import(new URL('parse-keypress.js', ink));

// hooks/use-input.js handleData, verbatim apart from returning instead of calling the handler.
function useInputMapping(data) {
	const keypress = parseKeypress(data);
	const key = {
		upArrow: keypress.name === 'up',
		downArrow: keypress.name === 'down',
		leftArrow: keypress.name === 'left',
		rightArrow: keypress.name === 'right',
		pageDown: keypress.name === 'pagedown',
		pageUp: keypress.name === 'pageup',
		home: keypress.name === 'home',
		end: keypress.name === 'end',
		return: keypress.name === 'return',
		escape: keypress.name === 'escape',
		ctrl: keypress.ctrl,
		shift: keypress.shift,
		tab: keypress.name === 'tab',
		backspace: keypress.name === 'backspace',
		delete: keypress.name === 'delete',
		meta: keypress.meta || keypress.name === 'escape' || keypress.option,
		super: keypress.super ?? false,
		hyper: keypress.hyper ?? false,
		capsLock: keypress.capsLock ?? false,
		numLock: keypress.numLock ?? false,
		eventType: keypress.eventType,
	};
	let input;
	if (keypress.isKittyProtocol) {
		if (keypress.isPrintable) {
			input = keypress.text ?? keypress.name;
		} else if (keypress.ctrl && keypress.name.length === 1) {
			input = keypress.name;
		} else {
			input = '';
		}
	} else if (keypress.ctrl) {
		input = keypress.name;
	} else {
		input = keypress.sequence;
	}
	if (!keypress.isKittyProtocol && nonAlphanumericKeys.includes(keypress.name)) {
		input = '';
	}
	if (input.startsWith('\u001B')) {
		input = input.slice(1);
	}
	if (input.length === 1 && typeof input[0] === 'string' && /[A-Z]/.test(input[0])) {
		key.shift = true;
	}
	return {input, key};
}

const b = text => [...Buffer.from(text, 'utf8')];
const E = '\u001B';

// [name, chunks]: a chunk is a string (its UTF-8 bytes), an array of byte values, or null (the flush tick).
const cases = [
	// Printable keys, one per chunk.
	...['a', 'z', 'A', 'Z', '0', '9', ' ', '~', '!', '?', '/', '\\', '[', ']', '<', '>', '+', '=', '-', '.', ',', '\'', '"', '`', '{', '|', '}']
		.map(text => [`printable ${JSON.stringify(text)}`, [text]]),
	['two-byte utf-8', ['é']],
	['three-byte utf-8', ['日']],
	['four-byte utf-8 emoji', ['😀']],
	['emoji with skin tone', ['👍🏽']],
	['combining accent', ['é']],
	['zwj family emoji', ['👨‍👩‍👧']],
	// Controls.
	['return', ['\r']],
	['linefeed', ['\n']],
	['crlf', ['\r\n']],
	['tab', ['\t']],
	['backspace ctrl-h', ['\b']],
	['delete 0x7f', ['\x7f']],
	['lone escape flushed', [E]],
	['lone escape then flush tick', [E, null]],
	['ctrl-space nul', ['\x00']],
	...Array.from({length: 26}, (_, index) => [`ctrl-${String.fromCharCode(97 + index)}`, [String.fromCharCode(index + 1)]]),
	['ctrl-backslash', ['\x1c']],
	['ctrl-close-bracket', ['\x1d']],
	['ctrl-caret', ['\x1e']],
	['ctrl-underscore', ['\x1f']],
	// Cursor and editing keys.
	...[['up', 'A'], ['down', 'B'], ['right', 'C'], ['left', 'D'], ['clear', 'E'], ['end', 'F'], ['home', 'H']]
		.flatMap(([name, letter]) => [[`csi ${name}`, [`${E}[${letter}`]], [`ss3 ${name}`, [`${E}O${letter}`]]]),
	...[1, 2, 3, 4, 5, 6, 7, 8].map(number => [`csi ${number}~`, [`${E}[${number}~`]]),
	...[11, 12, 13, 14, 15, 17, 18, 19, 20, 21, 23, 24].map(number => [`function key ${number}~`, [`${E}[${number}~`]]),
	...['P', 'Q', 'R', 'S'].map(letter => [`ss3 function ${letter}`, [`${E}O${letter}`]]),
	...['A', 'B', 'C', 'D', 'E'].map(letter => [`cygwin function [[${letter}`, [`${E}[[${letter}`]]),
	['putty pageup', [`${E}[[5~`]],
	['shift-tab', [`${E}[Z`]],
	// Modified keys (xterm modifier parameter, rxvt forms).
	...[2, 3, 4, 5, 6, 7, 8, 9, 10].flatMap(modifier => [
		[`up modifier ${modifier}`, [`${E}[1;${modifier}A`]],
		[`delete modifier ${modifier}`, [`${E}[3;${modifier}~`]],
	]),
	['ctrl-right', [`${E}[1;5C`]],
	['ctrl-left', [`${E}[1;5D`]],
	['alt-right', [`${E}[1;3C`]],
	['ctrl-home', [`${E}[1;5H`]],
	['ctrl-end', [`${E}[1;5F`]],
	['shift-pagedown', [`${E}[6;2~`]],
	...['a', 'b', 'c', 'd', 'e'].map(letter => [`rxvt shift ${letter}`, [`${E}[${letter}`]]),
	...['a', 'b', 'c', 'd', 'e'].map(letter => [`rxvt ctrl O${letter}`, [`${E}O${letter}`]]),
	...['2$', '3$', '5$', '6$', '7$', '8$', '2^', '3^', '5^', '6^', '7^', '8^'].map(code => [`rxvt ${code}`, [`${E}[${code}`]]),
	// Meta (Alt / Option) combinations.
	...['a', 'b', 'f', 'B', 'F', 'Z', '1', '0'].map(letter => [`meta ${letter}`, [`${E}${letter}`]]),
	['meta dot', [`${E}.`]],
	['meta space', [`${E} `]],
	['meta return', [`${E}\r`]],
	['meta backspace 0x7f', [`${E}\x7f`]],
	['meta backspace ctrl-h', [`${E}\b`]],
	['meta tab', [`${E}\t`]],
	['meta ctrl-a', [`${E}\x01`]],
	['meta two-byte', [`${E}é`]],
	['meta emoji', [`${E}😀`]],
	['meta bracket', [`${E}[`]],
	['meta O', [`${E}O`]],
	['double escape', [`${E}${E}`]],
	['triple escape', [`${E}${E}${E}`]],
	['option up', [`${E}${E}[A`]],
	['option ss3 up', [`${E}${E}OA`]],
	['option pagedown', [`${E}${E}[6~`]],
	['escape then letter split', [E, 'a']],
	['escape then letter flushed between', [E, null, 'a']],
	// Several keys in one chunk.
	['typed fast', ['abc']],
	['jj', ['jj']],
	['dd', ['dd']],
	['fr', ['fr']],
	['text with newline', ['hello\nworld']],
	['two arrows', [`${E}[A${E}[B`]],
	['text around arrow', [`a${E}[Ab`]],
	['arrow then text', [`${E}[Cxyz`]],
	['escape then arrow', [`${E}${E}[B${E}`]],
	['meta then text', [`${E}bxyz`]],
	['many arrows', [`${E}[A`.repeat(5)]],
	['controls run together', ['\r\r\t\x7f']],
	// Bracketed paste.
	['bracketed paste', [`${E}[200~pasted text${E}[201~`]],
	['bracketed paste multi-line', [`${E}[200~line one\r\nline two\tend${E}[201~`]],
	['bracketed paste split', [`${E}[200~first`, ' half', ` second${E}[2`, '01~']],
	['bracketed paste with escape inside', [`${E}[200~a${E}b${E}[201~`]],
	['plain paste', ['pasted without brackets, a long line']],
	// Mouse reports (preview focus enables them).
	['sgr wheel up', [`${E}[<64;10;5M`]],
	['sgr wheel down twice', [`${E}[<65;10;5M${E}[<65;10;5M`]],
	['sgr release', [`${E}[<0;3;4m`]],
	['legacy mouse', [`${E}[M\`!!`]],
	['urxvt mouse', [`${E}[96;10;5M`]],
	// Focus, replies and odd sequences.
	['focus in', [`${E}[I`]],
	['focus out', [`${E}[O`]],
	['device attributes reply', [`${E}[?1;2c`]],
	['cursor position reply', [`${E}[12;40R`]],
	['csi with bell inside', [`${E}[1\x07x`]],
	['osc-looking', [`${E}]0;title\x07`]],
	['incomplete csi flushed', [`${E}[1;5`]],
	['incomplete csi completed', [`${E}[1;`, '5C']],
	['split csi', [`${E}[`, 'A']],
	['split escape', [E, '[A']],
	['escape bracket flushed', [`${E}[`]],
	['escape O flushed', [`${E}O`]],
	['ss3 non-final', [`${E}O1`]],
	['escape escape bracket split', [`${E}${E}`, '[A']],
	// UTF-8 decoding across chunks and invalid bytes.
	['utf-8 split 2+1', [[0xe6, 0x97], [0xa5]]],
	['utf-8 split 1+3', [[0xf0], [0x9f, 0x98, 0x80]]],
	['utf-8 split byte by byte', [[0xf0], [0x9f], [0x98], [0x80]]],
	['invalid byte', [[0xff]]],
	['lone continuation', [[0x80]]],
	['truncated sequence then ascii', [[0xe6, 0x41]]],
	['high byte alt (macOS meta sends)', [[0xe9]]],
	['latin-1 meta f', [[0xe6]]],
	// Kitty keyboard protocol (Ink parses it whenever it arrives).
	['kitty a', [`${E}[97u`]],
	['kitty ctrl-a', [`${E}[97;5u`]],
	['kitty shift-enter', [`${E}[13;2u`]],
	['kitty enter', [`${E}[13u`]],
	['kitty space', [`${E}[32u`]],
	['kitty escape', [`${E}[27u`]],
	['kitty tab', [`${E}[9u`]],
	['kitty backspace', [`${E}[127u`]],
	['kitty ctrl code 3', [`${E}[3;5u`]],
	['kitty super-a', [`${E}[97;9u`]],
	['kitty caps-lock a', [`${E}[97;65u`]],
	['kitty release a', [`${E}[97;1:3u`]],
	['kitty repeat a', [`${E}[97;1:2u`]],
	['kitty text field', [`${E}[97;2;65u`]],
	['kitty keypad 0', [`${E}[57399u`]],
	['kitty invalid codepoint', [`${E}[1114112u`]],
	['kitty surrogate codepoint', [`${E}[55296u`]],
	['kitty special up press', [`${E}[1;1:1A`]],
	['kitty special up release', [`${E}[1;1:3A`]],
	['kitty special delete repeat', [`${E}[3;5:2~`]],
	['kitty special unknown', [`${E}[9;1:1~`]],
	['kitty uppercase', [`${E}[65;2u`]],
	['kitty emoji', [`${E}[128512u`]],
];

const out = [];
for (const [name, chunks] of cases) {
	const decoder = new StringDecoder('utf8');
	const parser = createInputParser();
	const events = [];
	const emit = sequence => events.push({sequence, ...useInputMapping(sequence)});
	const flush = () => { const pending = parser.flushPendingEscape(); if (pending) emit(pending); };
	const bytes = [];
	for (const chunk of chunks) {
		if (chunk === null) { flush(); bytes.push(null); continue; }
		const raw = typeof chunk === 'string' ? b(chunk) : chunk;
		bytes.push(raw);
		for (const sequence of parser.push(decoder.write(Buffer.from(raw)))) emit(sequence);
	}
	flush();
	out.push({name, chunks: bytes, events});
}
process.stdout.write(`${JSON.stringify(out, null, '\t')}\n`);
