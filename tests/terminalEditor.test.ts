import assert from 'node:assert/strict';
import {test} from 'node:test';
import {resolveTerminalEditor, terminalEditorArgs} from '../src/desktop.js';

// o on Notes: which terminal editor opens the note, and the vim-only notes script (tests/notesVim.test.ts).

test('the terminal editor is $VISUAL, then $EDITOR (with its arguments), else a vi found on PATH', () => {
	assert.deepEqual(resolveTerminalEditor({VISUAL: 'sh -x', EDITOR: 'nope'}), {command: 'sh', args: ['-x'], label: 'sh'});
	assert.deepEqual(resolveTerminalEditor({EDITOR: '  /bin/sh  '}), {command: '/bin/sh', args: [], label: 'sh'});
	assert.equal(resolveTerminalEditor({EDITOR: 'deckhand-no-such-editor'}), undefined);
	// Unset: nvim, vim or vi, whichever this machine has.
	const fallback = resolveTerminalEditor({PATH: process.env.PATH});
	if (fallback) assert.match(fallback.label, /^(nvim|vim|vi)$/);
});

test('vim-family editors source the notes script after the note loads; others only get the file', () => {
	assert.deepEqual(terminalEditorArgs({command: '/usr/bin/vim', args: [], label: 'vim'}, '/notes/a.md', '/dh/notes.vim'), ['/notes/a.md', '-S', '/dh/notes.vim']);
	assert.deepEqual(terminalEditorArgs({command: 'nvim', args: ['--clean'], label: 'nvim'}, 'n.md', 's.vim'), ['--clean', 'n.md', '-S', 's.vim']);
	assert.deepEqual(terminalEditorArgs({command: 'nvim', args: [], label: 'nvim'}, 'n.md'), ['n.md']);
	assert.deepEqual(terminalEditorArgs({command: 'nano', args: ['-l'], label: 'nano'}, 'n.md', 's.vim'), ['-l', 'n.md']);
});
