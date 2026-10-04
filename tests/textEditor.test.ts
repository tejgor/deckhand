import assert from 'node:assert/strict';
import {test} from 'node:test';
import {editText, editorLines, type EditorState} from '../src/textEditor.js';

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
