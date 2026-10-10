import assert from 'node:assert/strict';
import {spawnSync} from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {test} from 'node:test';
import {NOTES_VIM_SCRIPT} from '../src/notesVim.js';

// o on Notes in vim: the note keys behave like the in-app editor's (src/notes.ts), checked in a real headless vim.

const vim = spawnSync('sh', ['-c', 'command -v vim'], {encoding: 'utf8'}).stdout.trim();

/** `text` after running `keys` (`:normal` notation, e.g. `\<C-x>`) in vim with the script sourced. */
function afterKeys(text: string, keys: string): string {
	const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'deckhand-vim-'));
	try {
		const note = path.join(directory, 'note.md'), script = path.join(directory, 'notes.vim'), out = path.join(directory, 'out.md');
		fs.writeFileSync(note, text);
		fs.writeFileSync(script, NOTES_VIM_SCRIPT);
		const result = spawnSync(vim, ['-N', '-u', 'NONE', '-i', 'NONE', '-n', '-es', note, '-S', script, '-c', `exe "normal ${keys}"`, '-c', `wq! ${out}`], {encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], timeout: 5000});
		assert.equal(result.error, undefined);
		return fs.readFileSync(out, 'utf8');
	} finally {
		fs.rmSync(directory, {recursive: true, force: true});
	}
}

test('Ctrl+X checks and unchecks an item, or makes the line one (a bullet keeps its bullet)', {skip: !vim && 'no vim'}, () => {
	assert.equal(afterKeys('- [ ] a\n', '\\<C-x>'), '- [x] a\n');
	assert.equal(afterKeys('  * [X] a\n', '\\<C-x>'), '  * [ ] a\n');
	assert.equal(afterKeys('  * b\n', '\\<C-x>'), '  * [ ] b\n');
	assert.equal(afterKeys('plain\n', '\\<C-x>'), '- [ ] plain\n');
	// In insert mode the cursor keeps its place in the text.
	assert.equal(afterKeys('foo\n', 'A\\<C-x>bar\\<Esc>'), '- [ ] foobar\n');
});

test('Ctrl+T adds an open item below with the line\'s indent and bullet, ready to type', {skip: !vim && 'no vim'}, () => {
	assert.equal(afterKeys('  + [x] a\n', '\\<C-t>b\\<Esc>'), '  + [x] a\n  + [ ] b\n');
	assert.equal(afterKeys('text\n', 'A\\<C-t>b\\<Esc>'), 'text\n- [ ] b\n');
	assert.equal(afterKeys('\n', '\\<C-t>q\\<Esc>'), '- [ ] q\n');
});

test('Enter on an item continues the list, on an empty one ends it, elsewhere is a plain newline', {skip: !vim && 'no vim'}, () => {
	assert.equal(afterKeys('- [x] ab\n', '0fbi\\<CR>c\\<Esc>'), '- [x] a\n- [ ] cb\n');
	assert.equal(afterKeys('  - [ ] a\n', 'A\\<CR>b\\<Esc>'), '  - [ ] a\n  - [ ] b\n');
	assert.equal(afterKeys('- [ ] \n', 'A\\<CR>z\\<Esc>'), 'z\n');
	assert.equal(afterKeys('x\n', 'A\\<CR>y\\<Esc>'), 'x\ny\n');
	// Inside the marker: a plain newline.
	assert.equal(afterKeys('- [ ] a\n', '0i\\<CR>\\<Esc>'), '\n- [ ] a\n');
});

test('Ctrl+], Ctrl+Space save and quit', {skip: !vim && 'no vim'}, () => {
	for (const key of ['\\<C-]>', '\\<C-@>']) {
		const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'deckhand-vim-'));
		try {
			const note = path.join(directory, 'note.md'), script = path.join(directory, 'notes.vim');
			fs.writeFileSync(note, 'a\n');
			fs.writeFileSync(script, NOTES_VIM_SCRIPT);
			// No wq afterwards: only the mapping can have written the file.
			const result = spawnSync(vim, ['-N', '-u', 'NONE', '-i', 'NONE', '-n', '-es', note, '-S', script, '-c', `exe "normal Ab${key}"`], {stdio: 'ignore', timeout: 5000});
			assert.equal(result.signal, null);
			assert.equal(fs.readFileSync(note, 'utf8'), 'ab\n');
		} finally {
			fs.rmSync(directory, {recursive: true, force: true});
		}
	}
});
