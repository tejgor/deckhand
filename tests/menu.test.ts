import assert from 'node:assert/strict';
import {test} from 'node:test';
import {fitHint, menuWindowStart, scrolledListTop, wrapWords} from '../src/menu.js';

test('fitHint keeps one line: full texts, then short forms, then drops the least important parts', () => {
	const parts = [{text: '↑↓ move', drop: 2}, {text: 'enter/e open the file that sets it', short: 'enter open'}, 'esc back'];
	assert.equal(fitHint(parts, 80), '↑↓ move · enter/e open the file that sets it · esc back');
	assert.equal(fitHint(parts, 31), '↑↓ move · enter open · esc back');
	assert.equal(fitHint(parts, 22), 'enter open · esc back');
	assert.ok(fitHint(parts, 5).length <= 5);
});

test('fitHint can join with another separator', () => {
	assert.equal(fitHint(['o attach', {text: 'f filter', drop: 2}, '? help'], 80, ' • '), 'o attach • f filter • ? help');
	assert.equal(fitHint(['o attach', {text: 'f filter', drop: 2}, '? help'], 20, ' • '), 'o attach • ? help');
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

test('scrolledListTop: moving up onto a row shows the headings right above it, keeping the selection visible', () => {
	// Rows 0 and 1 are headings (a group's name and its note's name), then selectable lines; 5 rows on screen.
	const headings = new Set([0, 1, 6]);
	const selectable = (index: number) => !headings.has(index);
	// Scrolled down, then back up onto the first line: both headings come back (the reported bug kept only row 2).
	assert.equal(scrolledListTop(4, 2, 5, 12, selectable), 0);
	// Up onto the first line of the second group: its heading (6) shows too.
	assert.equal(scrolledListTop(8, 7, 5, 12, selectable), 6);
	// Never so far that the selection leaves the screen.
	assert.equal(scrolledListTop(4, 2, 2, 12, selectable), 1);
	// Moving down scrolls just enough; the end of the list is not passed; no selection keeps the position.
	assert.equal(scrolledListTop(0, 7, 5, 12, selectable), 3);
	assert.equal(scrolledListTop(20, 11, 5, 12, selectable), 7);
	assert.equal(scrolledListTop(3, -1, 5, 12, selectable), 3);
	assert.equal(scrolledListTop(3, 1, 5, 3, selectable), 0);
});
