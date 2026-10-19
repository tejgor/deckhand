//! Port of notesVim.ts.
//!
//! o on Notes in vim/nvim: the in-app editor's note keys as a vimscript sourced after the note loads (`-S`). Ctrl+]
//! and Ctrl+Space (Deckhand's detach keys) save and quit; Ctrl+X, Ctrl+T and Enter mirror toggleChecklist,
//! insertChecklistItem and continueChecklist (deckhand_core::notes) in the note's buffer only. Ctrl+P (send to
//! Tasks) is left to the Tasks view, so vim never needs the daemon.

use std::path::PathBuf;

use tokio::io::AsyncWriteExt;

use crate::notes_store::write_error;
use crate::paths;
use deckhand_core::error::{DhError, Result};

/// TS: notesVim.ts NOTES_VIM_SCRIPT (byte-identical to the TS `String.raw` template).
pub const NOTES_VIM_SCRIPT: &str = r#"" Deckhand: keys for a note opened with o on the Notes tab. Written by Deckhand on each launch (src/notesVim.ts).

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
"#;

/// TS: notesVim.ts writeNotesVimScript (`<home>/notes.vim`, rewritten on each launch so it follows the installed
/// version; directory 0700, file 0600). Returns its path.
pub async fn write_notes_vim_script() -> Result<PathBuf> {
    let file = paths::config_dir().join("notes.vim");
    if let Some(directory) = file.parent() {
        tokio::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .await
            .map_err(|error| DhError::io(&error, "mkdir", &directory.to_string_lossy()))?;
    }
    // `fs.writeFile(file, text, {mode})`: the mode only applies when the file is created.
    let mut handle = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&file)
        .await
        .map_err(|error| DhError::io(&error, "open", &file.to_string_lossy()))?;
    handle
        .write_all(NOTES_VIM_SCRIPT.as_bytes())
        .await
        .map_err(|error| write_error(&error))?;
    handle.flush().await.map_err(|error| write_error(&error))?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::{Command, ExitStatus, Stdio};
    use std::time::{Duration, Instant};

    use deckhand_parity::{Case, DirOp, replay, replay_dir};

    use super::*;
    use crate::notes_store::tests::Home;

    const UNIT: &str = "I2";

    #[test]
    fn script_matches_the_ts_source() {
        let source = include_str!("../../../src/notesVim.ts");
        let start = source.find("String.raw`").map(|at| at + "String.raw`".len()).unwrap();
        let end = start + source[start..].find("`;").unwrap();
        assert_eq!(NOTES_VIM_SCRIPT, &source[start..end]);
    }

    #[test]
    fn notes_vim_script_parity() {
        replay(UNIT, "notesVim/NOTES_VIM_SCRIPT", |_: &Case| {
            Ok::<_, String>(NOTES_VIM_SCRIPT)
        });
    }

    fn replay_write(name: &str) {
        replay_dir(
            UNIT,
            &format!("notesVim/{name}"),
            |root: &Path, op: &DirOp| -> Result<String, String> {
                if op.fn_name != "writeNotesVimScript" {
                    return Err(format!("unknown op {}", op.fn_name));
                }
                let _home = Home::set(root);
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| error.to_string())
                    .and_then(|runtime| {
                        runtime
                            .block_on(write_notes_vim_script())
                            .map_err(|error| error.to_string())
                    })
                    .map(|file| file.to_string_lossy().into_owned())
            },
        );
    }

    #[test]
    fn write_fresh_home_parity() {
        replay_write("write-fresh-home");
    }

    #[test]
    fn write_overwrites_stale_script_parity() {
        replay_write("write-overwrites-stale-script");
    }

    #[test]
    fn write_shorter_than_existing_parity() {
        replay_write("write-shorter-than-existing");
    }

    // o on Notes in vim: the note keys behave like the in-app editor's (deckhand_core::notes), checked in a real
    // headless vim.

    /// `command -v vim` (the vim tests are skipped without one).
    fn vim() -> Option<String> {
        let output = Command::new("sh").args(["-c", "command -v vim"]).output().ok()?;
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!path.is_empty()).then_some(path)
    }

    /// Runs `command` for at most 5 s (spawnSync's `timeout`); `None` when it had to be killed.
    fn run(command: &mut Command) -> Option<ExitStatus> {
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn vim");
        let deadline = Instant::now() + Duration::from_millis(5000);
        loop {
            if let Some(status) = child.try_wait().expect("wait for vim") {
                return Some(status);
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// A vim run on `note.md` with the script sourced (`-S`) and `commands` after it (`-c`).
    fn vim_command(vim: &str, note: &Path, script: &Path, commands: &[String]) -> Command {
        let mut command = Command::new(vim);
        command
            .args(["-N", "-u", "NONE", "-i", "NONE", "-n", "-es"])
            .arg(note)
            .arg("-S")
            .arg(script);
        for line in commands {
            command.arg("-c").arg(line);
        }
        command
    }

    /// `text` after running `keys` (`:normal` notation, e.g. `\<C-x>`) in vim with the script sourced.
    fn after_keys(vim: &str, text: &str, keys: &str) -> String {
        let directory = tempfile::Builder::new()
            .prefix("deckhand-vim-")
            .tempdir()
            .expect("tempdir");
        let note = directory.path().join("note.md");
        let script = directory.path().join("notes.vim");
        let out = directory.path().join("out.md");
        std::fs::write(&note, text).expect("write note");
        std::fs::write(&script, NOTES_VIM_SCRIPT).expect("write script");
        let commands = [format!("exe \"normal {keys}\""), format!("wq! {}", out.display())];
        let status = run(&mut vim_command(vim, &note, &script, &commands));
        assert!(status.is_some(), "vim timed out");
        std::fs::read_to_string(&out).expect("read out")
    }

    // covers: B-I2-01 Ctrl+X checks and unchecks an item, or makes the line one (a bullet keeps its bullet)
    #[test]
    fn ctrl_x_checks_and_unchecks_an_item() {
        let Some(vim) = vim() else { return };
        assert_eq!(after_keys(&vim, "- [ ] a\n", r"\<C-x>"), "- [x] a\n");
        assert_eq!(after_keys(&vim, "  * [X] a\n", r"\<C-x>"), "  * [ ] a\n");
        assert_eq!(after_keys(&vim, "  * b\n", r"\<C-x>"), "  * [ ] b\n");
        assert_eq!(after_keys(&vim, "plain\n", r"\<C-x>"), "- [ ] plain\n");
        // In insert mode the cursor keeps its place in the text.
        assert_eq!(after_keys(&vim, "foo\n", r"A\<C-x>bar\<Esc>"), "- [ ] foobar\n");
    }

    // covers: B-I2-02 Ctrl+T adds an open item below with the line's indent and bullet, ready to type
    #[test]
    fn ctrl_t_adds_an_open_item_below() {
        let Some(vim) = vim() else { return };
        assert_eq!(
            after_keys(&vim, "  + [x] a\n", r"\<C-t>b\<Esc>"),
            "  + [x] a\n  + [ ] b\n"
        );
        assert_eq!(after_keys(&vim, "text\n", r"A\<C-t>b\<Esc>"), "text\n- [ ] b\n");
        assert_eq!(after_keys(&vim, "\n", r"\<C-t>q\<Esc>"), "- [ ] q\n");
    }

    // covers: B-I2-03 Enter on an item continues the list, on an empty one ends it, elsewhere is a plain newline
    #[test]
    fn enter_on_an_item_continues_the_list() {
        let Some(vim) = vim() else { return };
        assert_eq!(
            after_keys(&vim, "- [x] ab\n", r"0fbi\<CR>c\<Esc>"),
            "- [x] a\n- [ ] cb\n"
        );
        assert_eq!(
            after_keys(&vim, "  - [ ] a\n", r"A\<CR>b\<Esc>"),
            "  - [ ] a\n  - [ ] b\n"
        );
        assert_eq!(after_keys(&vim, "- [ ] \n", r"A\<CR>z\<Esc>"), "z\n");
        assert_eq!(after_keys(&vim, "x\n", r"A\<CR>y\<Esc>"), "x\ny\n");
        // Inside the marker: a plain newline.
        assert_eq!(after_keys(&vim, "- [ ] a\n", r"0i\<CR>\<Esc>"), "\n- [ ] a\n");
    }

    // covers: B-I2-04 Ctrl+], Ctrl+Space save and quit
    #[test]
    fn ctrl_bracket_and_ctrl_space_save_and_quit() {
        let Some(vim) = vim() else { return };
        for key in [r"\<C-]>", r"\<C-@>"] {
            let directory = tempfile::Builder::new()
                .prefix("deckhand-vim-")
                .tempdir()
                .expect("tempdir");
            let note = directory.path().join("note.md");
            let script = directory.path().join("notes.vim");
            std::fs::write(&note, "a\n").expect("write note");
            std::fs::write(&script, NOTES_VIM_SCRIPT).expect("write script");
            // No wq afterwards: only the mapping can have written the file.
            let status = run(&mut vim_command(
                &vim,
                &note,
                &script,
                &[format!("exe \"normal Ab{key}\"")],
            ));
            assert!(status.is_some(), "vim did not quit");
            assert_eq!(std::fs::read_to_string(&note).expect("read note"), "ab\n");
        }
    }
}
