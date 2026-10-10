import assert from 'node:assert/strict';
import {test} from 'node:test';
import {fitPath, lostFileCount, lostFileRows} from '../src/cleanupView.js';
import {cleanupOverrideText} from '../src/detailTexts.js';
import type {CleanupInspection} from '../src/types.js';

const inspection = (files: number, counted = files): CleanupInspection => ({
	safe: false, reasons: [`${counted} untracked file(s)`], dirtyFiles: 0, untrackedFiles: counted, ignoredFiles: 0,
	files: Array.from({length: files}, (_, index) => ({path: `notes/${index}.md`, state: 'untracked' as const})),
});

test('deletion confirmations list the files that would be lost, cut to the rows they have', () => {
	// All fit: every file, nothing more.
	assert.deepEqual(lostFileRows(inspection(3), 5), {files: inspection(3).files, more: 0});
	// Too many: the last row says how many more (the counts are complete even past the listed ones).
	assert.deepEqual(lostFileRows(inspection(10), 4).files.map(file => file.path), ['notes/0.md', 'notes/1.md', 'notes/2.md']);
	assert.equal(lostFileRows(inspection(10), 4).more, 7);
	assert.equal(lostFileRows(inspection(100, 250), 4).more, 247);
	assert.equal(lostFileCount(inspection(100, 250)), 250);
	// Paths are cut from the front, so the file's name stays.
	assert.equal(fitPath('src/components/deeply/nested/Button.tsx', 20), '…y/nested/Button.tsx');
	assert.equal(fitPath('a.txt', 20), 'a.txt');
	// The typed-DELETE screen of x on a session lists every one (that pane scrolls).
	const text = cleanupOverrideText({...inspection(2, 3), files: [{path: 'x.ts', state: 'changed'}, {path: 'secrets', state: 'ignored'}]});
	assert.match(text, /3 untracked file\(s\)\n\nFiles \(M changed · \? untracked · ! ignored\):\n  M x\.ts\n  ! secrets\n  \+1 more\n\nMain\/current/);
	assert.match(cleanupOverrideText(undefined), /Safety could not be verified/);
});
