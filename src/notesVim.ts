import fs from 'node:fs/promises';
import path from 'node:path';
import {getConfigDir} from './paths.js';

// o on Notes in vim/nvim: the in-app editor's note keys as a vimscript sourced after the note loads (`-S`). Ctrl+] and
// Ctrl+Space (Deckhand's detach keys) save and quit; Ctrl+X, Ctrl+T and Enter mirror toggleChecklist,
// insertChecklistItem and continueChecklist (src/notes.ts) in the note's buffer only. Ctrl+P (send to Tasks) is left
// to the Tasks view (b, Tab: a / A), so vim never needs the daemon.

export const NOTES_VIM_SCRIPT = String.raw`" Deckhand: keys for a note opened with o on the Notes tab. Written by Deckhand on each launch (src/notesVim.ts).

" Back to Deckhand: save this buffer and quit (refused while another buffer has unsaved changes).
for s:key in ['<C-]>', '<C-Space>', '<C-@>', '<Nul>']
  execute 'silent! nnoremap <silent> ' . s:key . ' :update<Bar>qall<CR>'
  execute 'silent! inoremap <silent> ' . s:key . ' <Esc>:update<Bar>qall<CR>'
endfor

" The checklist keys edit at the insert-mode cursor through <Cmd> (vim 8.2.1978+, nvim 0.5+).
if !has('nvim-0.5') && !has('patch-8.2.1978')
  finish
endif

" - [ ] item / * [x] item / + [X] item, indented or not: [1] indent, bullet and gap, [2] the mark.
let s:checklist = '^\(\s*[-*+]\s\+\)\[\([ xX]\)\]\%(\s\|$\)'

" Ctrl+X: check/uncheck the line's item; a line without one becomes an open item (a plain bullet keeps its bullet).
function! DeckhandNotesToggle() abort
  let l:line = getline('.')
  let l:item = matchlist(l:line, s:checklist)
  if !empty(l:item)
    let l:box = len(l:item[1]) + 1
    call setline('.', strpart(l:line, 0, l:box) . (l:item[2] ==# ' ' ? 'x' : ' ') . strpart(l:line, l:box + 1))
    return
  endif
  let l:bullet = matchstr(l:line, '^\s*[-*+]\s\+')
  let l:at = empty(l:bullet) ? len(matchstr(l:line, '^\s*')) : len(l:bullet)
  let l:inserted = empty(l:bullet) ? '- [ ] ' : '[ ] '
  let l:col = col('.')
  call setline('.', strpart(l:line, 0, l:at) . l:inserted . strpart(l:line, l:at))
  if l:col > l:at | call cursor(line('.'), l:col + len(l:inserted)) | endif
endfunction

" Ctrl+T: a new open item below the line, with its indent (and bullet, for an item), ready to type; an empty line
" takes the item itself.
function! DeckhandNotesNewItem() abort
  let l:line = getline('.')
  let l:item = matchlist(l:line, s:checklist)
  let l:indent = matchstr(l:line, '^\s*')
  if l:line =~# '^\s*$'
    call setline('.', l:indent . '- [ ] ')
  else
    call append('.', l:indent . (empty(l:item) ? '-' : matchstr(l:item[1], '[-*+]')) . ' [ ] ')
    call cursor(line('.') + 1, 1)
  endif
  if mode() ==# 'i' | call cursor(line('.'), len(getline('.')) + 1) | else | startinsert! | endif
endfunction

" Enter on an item, after its marker: a new open item with the rest of the line; on an empty item, ends the list.
function! DeckhandNotesEnter() abort
  if pumvisible() | return "\<CR>" | endif
  let l:item = matchlist(getline('.'), s:checklist)
  if empty(l:item) || col('.') - 1 < len(l:item[0]) | return "\<CR>" | endif
  return "\<Cmd>call DeckhandNotesContinue()\<CR>"
endfunction
function! DeckhandNotesContinue() abort
  let l:line = getline('.')
  let l:item = matchlist(l:line, s:checklist)
  if strpart(l:line, len(l:item[0])) =~# '^\s*$'
    call setline('.', '')
    call cursor(line('.'), 1)
    return
  endif
  let l:prefix = matchstr(l:item[1], '^\s*') . matchstr(l:item[1], '[-*+]') . ' [ ] '
  let l:col = col('.') - 1
  call setline('.', strpart(l:line, 0, l:col))
  call append('.', l:prefix . strpart(l:line, l:col))
  call cursor(line('.') + 1, len(l:prefix) + 1)
endfunction

nnoremap <buffer> <silent> <C-x> <Cmd>call DeckhandNotesToggle()<CR>
inoremap <buffer> <silent> <C-x> <Cmd>call DeckhandNotesToggle()<CR>
nnoremap <buffer> <silent> <C-t> <Cmd>call DeckhandNotesNewItem()<CR>
inoremap <buffer> <silent> <C-t> <Cmd>call DeckhandNotesNewItem()<CR>
inoremap <buffer> <silent> <expr> <CR> DeckhandNotesEnter()
`;

/** Writes the script to the Deckhand home (rewritten each time, so it follows the installed version) and returns its path. */
export async function writeNotesVimScript(): Promise<string> {
	const file = path.join(getConfigDir(), 'notes.vim');
	await fs.mkdir(path.dirname(file), {recursive: true, mode: 0o700});
	await fs.writeFile(file, NOTES_VIM_SCRIPT, {mode: 0o600});
	return file;
}
