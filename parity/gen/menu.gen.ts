/**
 * Parity fixtures for the pure helpers of src/menu.tsx (fixtures/parity/menu/). Regenerate with
 *
 *   node --import tsx parity/gen/menu.gen.ts
 *
 * Recorded: SELECTION_MARKER, menuWindowStart, scrolledListTop (the `selectable` predicate is one of the named
 * predicates below: the Rust replay dispatches on the `$fn` name), wrapWords, fitParts, fitHint, menuDetailsRows.
 * The Ink components (SelectableRow, MenuList, MenuPane, ...) are rendering, covered by the UI screen goldens.
 * Widths count UTF-16 code units, like TS; a result with a lone surrogate (a cut inside a pair) is not recorded.
 */
import {Recorder, arb, encode, fc, installDeterminism, mutate, registerFn, runTap, sample} from '../lib.ts';
import * as menu from '../../src/menu.tsx';

installDeterminism();
const recorder = new Recorder('menu', {cap: 700});
const rec = (name: string, fn: Function, args: unknown[]) => {
	let result: unknown;
	try { result = fn(...args); } catch { result = undefined; }
	if (!JSON.stringify(encode(result)).includes('$utf16')) recorder.call(name, fn, args);
};

recorder.call('SELECTION_MARKER', () => menu.SELECTION_MARKER, []);

// 1. Tap tests/menu.test.ts (fitHint, menuWindowStart, scrolledListTop, wrapWords). The predicates it passes are
//    inline arrows, recorded as {"$fn":"anonymous"}; those cases are re-recorded below with named predicates instead.
const tapped = await runTap({tests: ['tests/menu.test.ts'], tap: ['menu:fitHint', 'menu:menuWindowStart', 'menu:wrapWords']});
recorder.addTap(tapped, {module: 'menu'});

// 2. menuWindowStart: exhaustive small grid, plus random and mutated.
for (let count = 0; count <= 8; count++) for (let rows = 0; rows <= 6; rows++) for (let selected = -1; selected <= count; selected++) recorder.call('menuWindowStart', menu.menuWindowStart, [selected, count, rows]);
for (const args of sample(fc.tuple(fc.integer({min: 0, max: 200}), fc.integer({min: 0, max: 200}), fc.integer({min: 0, max: 60})), 150, 21)) recorder.call('menuWindowStart', menu.menuWindowStart, args);
for (const args of mutate(recorder.argsOf('menuWindowStart').slice(0, 60))) recorder.call('menuWindowStart', menu.menuWindowStart, args);

// 3. scrolledListTop(top, current, rows, total, selectable).
const predicates: Record<string, (index: number) => boolean> = {
	all: registerFn('all', () => true),
	none: registerFn('none', () => false),
	even: registerFn('even', (index: number) => index % 2 === 0),
	notEvery4: registerFn('notEvery4', (index: number) => index % 4 !== 0),
	notEvery3: registerFn('notEvery3', (index: number) => index % 3 !== 0),
	notFirstTwo: registerFn('notFirstTwo', (index: number) => index > 1),
	notEvery5: registerFn('notEvery5', (index: number) => index % 5 !== 0),
};
for (const [name, predicate] of Object.entries(predicates)) {
	for (let total = 0; total <= 9; total += 3) for (let rows = 1; rows <= 5; rows += 2) for (let top = 0; top <= total; top += 2) for (let current = -1; current < total; current++) {
		void name;
		recorder.call('scrolledListTop', menu.scrolledListTop, [top, current, rows, total, predicate]);
	}
	for (const args of sample(fc.tuple(fc.integer({min: 0, max: 40}), fc.integer({min: -1, max: 40}), fc.integer({min: 0, max: 12}), fc.integer({min: 0, max: 45})), 40, 22 + name.length)) recorder.call('scrolledListTop', menu.scrolledListTop, [...args, predicate]);
}
// The scenario of tests/menu.test.ts: headings at 0 and 3 are not selectable.
const headings = registerFn('headings03', (index: number) => index !== 0 && index !== 3);
for (const [top, current] of [[5, 4], [5, 3], [5, 2], [5, 1], [5, 0], [0, 6], [2, 9], [4, 4], [0, 0], [3, 4]]) recorder.call('scrolledListTop', menu.scrolledListTop, [top, current, 4, 10, headings]);

// 4. wrapWords(text, width).
const wrapAlphabet = ['a', 'b', 'Zz', ' ', ' ', ' ', '  ', '\n', '\t', '-', '/', 'é', '😀', '中', 'word', 'longerword', ' ', ' '];
const proseCases = ['', ' ', '   ', 'one', 'one two', 'one  two\tthree\nfour', 'Applies to every repository and never needs trust.', 'abcdefghij', 'a bcdefghij k', '  leading and trailing  ', '中文 中文字符 词', '😀😀😀 😀', 'x'.repeat(30), 'longword '.repeat(6), 'line\n\nbreak', 'a b c d'];
for (const text of proseCases) for (const width of [-2, 0, 1, 2, 3, 5, 8, 10, 20, 80]) rec('wrapWords', menu.wrapWords, [text, width]);
for (const text of sample(arb.text(40, wrapAlphabet), 250, 23)) for (const width of sample(fc.integer({min: 0, max: 25}), 2, text.length + 3)) rec('wrapWords', menu.wrapWords, [text, width]);
for (const args of mutate(recorder.argsOf('wrapWords').slice(0, 80), {variants: 2})) rec('wrapWords', menu.wrapWords, args);

// 5. fitParts(parts, width).
const partTexts = ['', 'a', 'abc', 'hello', 'é', '😀', '中文', 'ab😀', 'longer text here'];
const partArb = fc.record({text: fc.constantFrom(...partTexts)}, {requiredKeys: ['text']}).chain(base => fc.record({color: fc.option(fc.constantFrom('red', 'gray', 'cyan'), {nil: undefined}), bold: fc.option(fc.boolean(), {nil: undefined}), italic: fc.option(fc.boolean(), {nil: undefined})}).map(extra => {
	const part: Record<string, unknown> = {...base};
	for (const [key, value] of Object.entries(extra)) if (value !== undefined) part[key] = value;
	return part;
}));
const fixedParts = [
	[{text: 'abc', color: 'red'}, {text: 'defg', bold: true}, {text: 'hi', italic: true, color: 'gray'}],
	[{text: ''}, {text: 'x'}], [], [{text: '中文字符', color: 'cyan'}, {text: '😀😀'}], [{text: 'a'}, {text: 'b'}, {text: 'c'}],
];
for (const parts of fixedParts) for (const width of [-1, 0, 1, 2, 3, 4, 5, 7, 9, 12, 50]) rec('fitParts', menu.fitParts, [parts, width]);
for (const parts of sample(fc.array(partArb, {maxLength: 4}), 150, 24)) for (const width of sample(fc.integer({min: -1, max: 22}), 2, parts.length + 5)) rec('fitParts', menu.fitParts, [parts, width]);

// 6. fitHint(parts, width, separator?).
const hintParts: unknown[][] = [
	[], ['esc back'], ['a', 'b', 'c'],
	[{text: '↑↓ move', drop: 2}, {text: 'enter/e open the file that sets it', short: 'enter open'}, 'esc back'],
	['o attach', {text: 'f filter', drop: 2}, '? help'],
	[{text: 'one', drop: 1}, {text: 'two', drop: 1}, {text: 'three', drop: 1}],
	[{text: 'long form', short: 'short', drop: 3}, {text: 'another long form', short: 'alt', drop: 2}, {text: 'keep', drop: 0}],
	[{text: '中文提示', short: '中'}, {text: '😀 emoji', drop: 5}, 'é'],
	[{text: 'x', drop: 0}, {text: 'y', drop: 0}, {text: 'z', drop: 0}], [{text: 'no short', drop: 9}], ['', 'b'],
	[{text: 'a', short: ''}, {text: 'b', short: '', drop: 1}],
];
for (const parts of hintParts) for (const width of [-1, 0, 1, 3, 5, 10, 15, 20, 25, 31, 40, 60, 80]) {
	rec('fitHint', menu.fitHint, [parts, width]);
	rec('fitHint', menu.fitHint, [parts, width, ' | ']);
	rec('fitHint', menu.fitHint, [parts, width, '']);
}
const hintPartArb = fc.oneof(fc.constantFrom('go', 'esc back', '中', 'é😀'), fc.record({text: fc.constantFrom('move', 'open it now', '中文', '😀 x'), short: fc.option(fc.constantFrom('', 'mv', 'o'), {nil: undefined}), drop: fc.option(fc.integer({min: 0, max: 4}), {nil: undefined})}).map(part => Object.fromEntries(Object.entries(part).filter(([, value]) => value !== undefined))));
for (const parts of sample(fc.array(hintPartArb, {maxLength: 5}), 200, 25)) for (const width of sample(fc.integer({min: 0, max: 50}), 2, parts.length + 7)) rec('fitHint', menu.fitHint, [parts, width]);
for (const args of mutate(recorder.argsOf('fitHint').slice(0, 80), {variants: 2})) rec('fitHint', menu.fitHint, args);

// 7. menuDetailsRows(lines, width).
const detailLineArb = fc.record({text: fc.oneof(fc.constantFrom('', 'short', 'A longer sentence that needs wrapping across lines', '中文字符 and 😀'), arb.text(30, wrapAlphabet)), nowrap: fc.option(fc.boolean(), {nil: undefined}), withParts: fc.boolean()}).map(({text, nowrap, withParts}) => {
	const line: Record<string, unknown> = {text};
	if (nowrap !== undefined) line.nowrap = nowrap;
	if (withParts) line.parts = [{text: text.slice(0, 3), color: 'red'}, {text: text.slice(3)}];
	return line;
});
for (const lines of [[], [{text: ''}], [{text: 'one two three four five six'}], [{text: 'x'.repeat(40), nowrap: true}], [{text: 'ab', parts: [{text: 'a'}, {text: 'b'}]}], [{text: 'aaa bbb ccc'}, {text: 'ddd eee', nowrap: false}, {text: 'path/to/file', nowrap: true}]]) for (const width of [-1, 0, 4, 5, 6, 8, 10, 14, 20, 80]) recorder.call('menuDetailsRows', menu.menuDetailsRows, [lines, width]);
for (const lines of sample(fc.array(detailLineArb, {maxLength: 5}), 150, 26)) for (const width of sample(fc.integer({min: 0, max: 40}), 2, lines.length + 9)) recorder.call('menuDetailsRows', menu.menuDetailsRows, [lines, width]);

await recorder.write({prune: true});
