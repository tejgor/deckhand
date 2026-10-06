import assert from 'node:assert/strict';
import {test} from 'node:test';
import {cursorRowIndex, editText, editorLines, moveVisual, scrollTopFor, wordLeft, wordRight, wrapRows, wrappedEditorLines, type EditorState} from '../src/textEditor.js';
import {normalizeTerminalKey} from '../src/terminalKeys.js';

test('editor inserts, selects/replaces, deletes and indents', () => {
	let state: EditorState = {text: 'abcd', cursor: 2};
	state = editText(state, 'X', {}); assert.equal(state.text, 'abXcd');
	state = editText(state, '', {backspace: true}); assert.equal(state.text, 'abcd');
	state = editText(state, '', {delete: true}); assert.equal(state.text, 'abd');
	state = editText(state, 'a', {ctrl: true}); assert.equal(state.selectAll, true);
	state = editText(state, '{}', {}); assert.equal(state.text, '{}'); assert.equal(state.cursor, 2);
	state = editText(state, '', {return: true}); state = editText(state, '', {tab: true}); assert.equal(state.text, '{}\n  ');
});
test('arrows and home/end navigate multiline text and Unicode without splitting surrogate pairs', () => {
	let state: EditorState = {text: 'abc\nxy\nlong', cursor: 2};
	state = editText(state, '', {downArrow: true}); assert.equal(state.cursor, 6);
	state = editText(state, '', {downArrow: true}); assert.equal(state.cursor, 9);
	state = editText(state, '', {home: true}); assert.equal(state.cursor, 7);
	state = editText(state, '', {end: true}); assert.equal(state.cursor, 11);
	state = editText(state, '', {home: true, ctrl: true}); assert.equal(state.cursor, 0);
	state = {text: 'a😀b', cursor: 3};
	state = editText(state, '', {leftArrow: true}); assert.equal(state.cursor, 1);
	state = editText(state, '', {rightArrow: true}); assert.equal(state.cursor, 3);
	state = editText(state, '', {backspace: true}); assert.equal(state.text, 'ab');
});
test('paste normalizes newlines and strips terminal control sequences; oversized paste is not applied', () => {
	const state = editText({text: '', cursor: 0}, '\x1b[200~{\r\n}\x1b[201~', {});
	assert.equal(state.text, '{\n}');
	const limited = editText(state, 'x'.repeat(65536), {});
	assert.equal(limited.text, state.text); assert.match(limited.message ?? '', /64 KiB/);
});
test('rendering keeps the cursor visible on wrapped boundaries and empty lines', () => {
	const wrapped = editorLines({text: 'abcd', cursor: 4}, 4);
	assert.equal(wrapped.cursorRow, 1); assert.equal(wrapped.lines[1]?.cursor, ' ');
	const middle = editorLines({text: 'abcd', cursor: 2}, 4);
	assert.equal(middle.lines[0]?.before, 'ab'); assert.equal(middle.lines[0]?.cursor, 'c'); assert.equal(middle.lines[0]?.after, 'd');
	const empty = editorLines({text: '\n', cursor: 0}, 10);
	assert.equal(empty.cursorRow, 0); assert.equal(empty.lines[0]?.cursor, ' ');
});

const notesOptions = {selectAll: false, maxChars: 20, limitMessage: 'too long', tab: false} as const;
test('word jumps (Alt/Ctrl+arrows, Alt+B/F), word deletion, and line start/end on Ctrl+A/E without select-all', () => {
	const text = 'fix the  login-bug\nnext';
	assert.equal(wordLeft(text, 18), 15); assert.equal(wordLeft(text, 15), 9); assert.equal(wordLeft(text, 9), 4); assert.equal(wordLeft(text, 0), 0);
	assert.equal(wordRight(text, 0), 3); assert.equal(wordRight(text, 3), 7); assert.equal(wordRight(text, 18), 23);
	let state: EditorState = {text, cursor: 18};
	state = editText(state, '', {leftArrow: true, meta: true}); assert.equal(state.cursor, 15);
	state = editText(state, '', {leftArrow: true, ctrl: true}); assert.equal(state.cursor, 9);
	state = editText(state, 'f', {meta: true}); assert.equal(state.cursor, 14);
	state = editText(state, 'b', {meta: true}); assert.equal(state.cursor, 9);
	state = editText(state, '', {rightArrow: true, ctrl: true}); assert.equal(state.cursor, 14);
	// Alt+Backspace (ESC DEL) and Ctrl+W delete the word before the cursor; Alt+Delete the one after.
	assert.deepEqual(editText(state, '', normalizeTerminalKey({delete: true, meta: true}, '\x1b\x7f')), {text: 'fix the  -bug\nnext', cursor: 9});
	assert.deepEqual(editText(state, 'w', {ctrl: true}), {text: 'fix the  -bug\nnext', cursor: 9});
	assert.deepEqual(editText({text, cursor: 9}, '', {delete: true, meta: true}), {text: 'fix the  -bug\nnext', cursor: 9});
	// Notes options: Ctrl+A/Ctrl+E go to the line's start/end; Tab is left to the caller.
	assert.deepEqual(editText({text, cursor: 21}, 'a', {ctrl: true}, notesOptions), {text, cursor: 19});
	assert.deepEqual(editText({text, cursor: 2}, 'e', {ctrl: true}, notesOptions), {text, cursor: 18});
	assert.equal(editText({text: 'a', cursor: 1}, '', {tab: true}, notesOptions).text, 'a');
});
test('insertions and deletions anywhere; the character limit refuses growth but allows shrinking', () => {
	let state: EditorState = {text: 'hello world', cursor: 5};
	state = editText(state, ',', {}, notesOptions); assert.deepEqual(state, {text: 'hello, world', cursor: 6});
	state = editText({...state, cursor: 0}, '', {delete: true}, notesOptions); assert.deepEqual(state, {text: 'ello, world', cursor: 0});
	state = editText({...state, cursor: 4}, '', {return: true}, notesOptions); assert.deepEqual(state, {text: 'ello\n, world', cursor: 5});
	state = editText(state, 'x\r\ny', {}, notesOptions); assert.equal(state.text, 'ello\nx\ny, world');
	assert.equal(editText(state, '0123456789', {}, notesOptions).message, 'too long');
	assert.equal(editText({text: 'x'.repeat(30), cursor: 30}, '', {backspace: true}, notesOptions).text.length, 29);
});
test('soft wrap: words break at spaces, long words mid-word, the cursor gets a row at an exact fit; Up/Down move by visual rows', () => {
	const text = 'the quick brown fox\nabcdefghij';
	const rows = wrapRows(text, 10, true).map(row => text.slice(row.start, row.end));
	assert.deepEqual(rows, ['the quick ', 'brown fox', 'abcdefghij', '']);
	assert.deepEqual(wrapRows('abcdefghij', 10).map(row => [row.start, row.end, row.last]), [[0, 10, true]]);
	const view = wrappedEditorLines({text, cursor: 10}, 10);
	// The cursor at a wrap boundary shows at the start of the next row.
	assert.equal(view.cursorRow, 1); assert.deepEqual(view.lines[1], {before: '', cursor: 'b', after: 'rown fox'});
	assert.equal(wrappedEditorLines({text, cursor: text.length}, 10).cursorRow, 3);
	assert.equal(cursorRowIndex(wrapRows(text, 10, true), 19), 1);
	// Up/Down keep the column within the wrapped paragraph instead of jumping a whole line.
	assert.equal(moveVisual(text, 2, 10, 1), 12);
	assert.equal(moveVisual(text, 12, 10, 1), 22);
	assert.equal(moveVisual(text, 22, 10, -1), 12);
	assert.equal(moveVisual(text, 4, 10, -1), 0);
	assert.equal(moveVisual(text, 25, 10, 1), text.length);
	// A wrapped row's end is the next row's start: Down from column 4 onto a full 5-column row stays on that row.
	assert.equal(moveVisual('aaaa bbbb\ncccccccccccc', 9, 5, 1), 14);
	// The cursor after a line that fills its row sits on an extra row (column 0), so Down goes to column 0.
	assert.equal(moveVisual('aaaaa\nbbbbbbb', 5, 5, 1), 6);
	assert.equal(editText({text, cursor: 2}, '', {downArrow: true}, {wrapWidth: 10}).cursor, 12);
	assert.equal(editText({text, cursor: 2}, '', {pageDown: true}, {wrapWidth: 10, pageRows: 2}).cursor, 22);
});
test('scrolling follows the cursor: only as far as needed, and never past the end', () => {
	assert.equal(scrollTopFor(0, 3, 5, 20), 0);
	assert.equal(scrollTopFor(0, 7, 5, 20), 3);
	assert.equal(scrollTopFor(6, 2, 5, 20), 2);
	assert.equal(scrollTopFor(18, 19, 5, 20), 15);
	assert.equal(scrollTopFor(4, 1, 5, 3), 0);
});
