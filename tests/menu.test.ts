import assert from 'node:assert/strict';
import {test} from 'node:test';
import {fitHint, menuWindowStart, wrapWords} from '../src/menu.js';

test('fitHint keeps one line: full texts, then short forms, then drops the least important parts', () => {
	const parts = [{text: '↑↓ move', drop: 2}, {text: 'enter/e open the file that sets it', short: 'enter open'}, 'esc back'];
	assert.equal(fitHint(parts, 80), '↑↓ move · enter/e open the file that sets it · esc back');
	assert.equal(fitHint(parts, 31), '↑↓ move · enter open · esc back');
	assert.equal(fitHint(parts, 22), 'enter open · esc back');
	assert.ok(fitHint(parts, 5).length <= 5);
});

test('menuWindowStart keeps the selection visible', () => {
	assert.equal(menuWindowStart(0, 3, 5), 0);
	assert.equal(menuWindowStart(9, 10, 4), 6);
	assert.equal(menuWindowStart(5, 10, 4), 3);
});

test('wrapWords wraps at spaces and cuts words longer than the width', () => {
	assert.deepEqual(wrapWords('Applies to every repository and never needs trust.', 20), ['Applies to every', 'repository and never', 'needs trust.']);
	assert.deepEqual(wrapWords('abcdefgh', 3), ['abc', 'def', 'gh']);
	assert.deepEqual(wrapWords('', 5), ['']);
});
