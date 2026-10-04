import assert from 'node:assert/strict';
import {test} from 'node:test';
import {normalizeTerminalKey} from '../src/terminalKeys.js';
import {editText} from '../src/textEditor.js';

test('terminal Backspace bytes and Kitty codepoint delete left, not right', () => {
	for (const sequence of ['\b', '\x7f', '\x1b[127u', '\x1b[127;1u', '\x1b[127;1:2u', '\x1b[8;1u']) {
		const key = normalizeTerminalKey({delete: true}, sequence);
		assert.equal(key.backspace, true); assert.equal(key.delete, false);
		assert.deepEqual(editText({text: 'abcd', cursor: 2}, '', key), {text: 'acd', cursor: 1});
	}
});
test('real forward Delete retains its direction and modifiers', () => {
	for (const sequence of ['\x1b[3~', '\x1b[3;2~', '\x1b[3;1:1~']) {
		const key = normalizeTerminalKey({delete: true, shift: true}, sequence);
		assert.equal(key.delete, true); assert.equal(key.shift, true);
		assert.deepEqual(editText({text: 'abcd', cursor: 2}, '', key), {text: 'abd', cursor: 2});
	}
});
