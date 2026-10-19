/** Regenerate: node --import tsx parity/gen/textEditor.gen.ts (fixtures/parity/textEditor/). */
import {Recorder, arb, fc, installDeterminism, mutate, runTap, sample} from '../lib.ts';
import {cleanInsertedText, cursorRowIndex, editText, editorLines, lineEnd, lineStart, moveVisual, replaceRange, scrollTopFor, wordLeft, wordRight, wrapRows, wrappedEditorLines} from '../../src/textEditor.ts';

installDeterminism();
const recorder = new Recorder('textEditor');
const fns = {lineStart, lineEnd, wordLeft, wordRight, cleanInsertedText, replaceRange, editText, editorLines, wrapRows, cursorRowIndex, moveVisual, wrappedEditorLines, scrollTopFor} as const;
type Name = keyof typeof fns;

recorder.addTap(await runTap({tests: ['tests/textEditor.test.ts', 'tests/terminalKeys.test.ts'], tap: ['textEditor:*']}), {module: 'textEditor'});

// Offsets are Rust byte offsets: keep only offsets on code-point boundaries (not inside a surrogate pair).
const boundary = (text: unknown, offset: unknown): boolean => {
	if (typeof text !== 'string' || typeof offset !== 'number' || !Number.isInteger(offset)) return true;
	if (offset < 0 || offset > text.length) return true;
	return !(offset > 0 && offset < text.length && /[\uD800-\uDBFF]/.test(text[offset - 1]!) && /[\uDC00-\uDFFF]/.test(text[offset]!));
};
const ok = (name: string, args: unknown[]): boolean => {
	const a = args as any[];
	switch (name) {
		case 'lineStart': case 'lineEnd': case 'wordLeft': case 'wordRight': case 'moveVisual': return boundary(a[0], a[1]);
		case 'editText': case 'editorLines': case 'wrappedEditorLines': return boundary(a[0]?.text, a[0]?.cursor);
		case 'replaceRange': return boundary(a[0]?.text, a[0]?.cursor) && boundary(a[0]?.text, a[1]) && boundary(a[0]?.text, a[2]);
		default: return true;
	}
};
const rec = (name: Name, args: unknown[]) => { if (ok(name, args)) { try { recorder.call(name, fns[name] as any, args); } catch {} } };

for (const name of Object.keys(fns) as Name[]) for (const args of mutate(recorder.argsOf(name), {variants: 4})) rec(name, args);

// Fast-check generated cases.
for (const [text, cursor] of sample(arb.textWithBoundary(20), 150, 11)) {
	for (const name of ['lineStart', 'lineEnd', 'wordLeft', 'wordRight'] as const) rec(name, [text, cursor]);
}
const junk = ['\x1b[200~', '\x1b[201~', '\x1b]0;title\x07', '\x1b]8;;u\x1b\\', '\r\n', '\r', '\x00', '\x7f', '\x85', 'x', '😀', '\t', '\n', 'é'];
for (const input of sample(fc.oneof(arb.text(16), fc.array(fc.constantFrom(...junk), {maxLength: 8}).map(p => p.join(''))), 150, 12)) rec('cleanInsertedText', [input]);

const flags = ['ctrl', 'meta', 'shift', 'leftArrow', 'rightArrow', 'upArrow', 'downArrow', 'pageUp', 'pageDown', 'home', 'end', 'backspace', 'delete', 'return', 'tab', 'escape'] as const;
const keyArb = fc.subarray([...flags], {maxLength: 3}).map(set => Object.fromEntries(set.map(flag => [flag, true])));
const inputArb = fc.oneof(fc.constantFrom('', 'a', 'b', 'e', 'f', 'w', 'x', '😀', '中', ' ', '\r', '\x1b[200~hi\x1b[201~', 'é\n'), arb.text(6));
const stateArb = arb.textWithBoundary(18).chain(([text, cursor]) => fc.boolean().map(selectAll => (selectAll ? {text, cursor, selectAll: true} : {text, cursor})));
const optionsArb = fc.oneof(
	fc.constant({}),
	fc.record({selectAll: fc.boolean()}),
	fc.record({tab: fc.boolean(), wrapWidth: fc.integer({min: 1, max: 8}), pageRows: fc.integer({min: 0, max: 3})}),
	fc.record({maxChars: fc.integer({min: 0, max: 24}), limitMessage: fc.constant('Too long'), selectAll: fc.boolean()}),
	fc.record({maxChars: fc.integer({min: 4, max: 24}), wrapWidth: fc.integer({min: 2, max: 6})}),
);
for (const [state, input, key, options] of sample(fc.tuple(stateArb, inputArb, keyArb, optionsArb), 700, 13)) rec('editText', [state, input, key, options]);
for (const [state, input, key] of sample(fc.tuple(stateArb, inputArb, keyArb), 100, 14)) rec('editText', [state, input, key]);
// Size limit (config default is 64 KiB, bytes) and message state carried through.
rec('editText', [{text: 'x'.repeat(65536), cursor: 65536}, 'y', {}]);
rec('editText', [{text: 'x'.repeat(65535), cursor: 3}, 'é', {}]);
rec('editText', [{text: 'x'.repeat(65536), cursor: 3, message: 'old'}, '', {backspace: true}]);
rec('editText', [{text: 'abc', cursor: 1, message: 'old'}, 'z', {}]);

for (const [[text, cursor], [text2, a]] of sample(fc.tuple(arb.textWithBoundary(14), arb.textWithBoundary(14)), 150, 15)) {
	const state = {text, cursor};
	const end = text.length;
	rec('replaceRange', [state, cursor, end, 'é😀', {}]);
	rec('replaceRange', [state, 0, cursor, '', {maxChars: 3}]);
	rec('replaceRange', [state, cursor, cursor, 'abcd', {maxChars: 3, limitMessage: 'cap'}]);
	rec('replaceRange', [{...state, selectAll: true, message: 'm'}, 0, end, text2.slice(0, a), {maxChars: 5}]);
}

for (const [[text, cursor], width, direction, count] of sample(fc.tuple(arb.textWithBoundary(26), fc.integer({min: -1, max: 9}), fc.constantFrom(-1, 1), fc.integer({min: 1, max: 4})), 350, 16)) {
	rec('moveVisual', [text, cursor, width, direction, count]);
	rec('moveVisual', [text, cursor, width, direction]);
}
for (const [text, width, room] of sample(fc.tuple(arb.text(30, ['a', 'b', ' ', ' ', '\n', '😀', '中', 'é']), fc.oneof(fc.integer({min: -1, max: 10}), fc.double({min: 0.5, max: 8, noNaN: true})), fc.boolean()), 400, 17)) {
	rec('wrapRows', [text, width, room]);
	rec('wrapRows', [text, width]);
}
for (const [[text, cursor], width] of sample(fc.tuple(arb.textWithBoundary(26), fc.integer({min: 0, max: 9})), 300, 18)) {
	const state = {text, cursor};
	rec('editorLines', [state, width]);
	rec('wrappedEditorLines', [state, width]);
	rec('cursorRowIndex', [wrapRows(text, Math.max(1, width), true), cursor]);
	rec('cursorRowIndex', [wrapRows(text, Math.max(1, width)), cursor + 2]);
}
rec('cursorRowIndex', [[], 0]);
rec('cursorRowIndex', [[], 5]);
for (const args of sample(fc.tuple(fc.integer({min: -2, max: 30}), fc.integer({min: -2, max: 30}), fc.integer({min: 0, max: 10}), fc.integer({min: 0, max: 40})), 250, 19)) rec('scrollTopFor', args);
for (const text of ['', '\n', 'a', '😀', '\n\n', 'hello world', 'a\n\nb', '中文 字', 'éx', '  ', '😀😀😀😀']) {
	for (const width of [1, 2, 3, 80]) { rec('wrapRows', [text, width, true]); rec('wrappedEditorLines', [{text, cursor: text.length}, width]); rec('editorLines', [{text, cursor: 0}, width]); }
}

await recorder.write({prune: true});
