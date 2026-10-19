//! Port of desktop.ts.
//!
//! Opening things outside the terminal UI: a file in Cursor or VS Code (at a line), an https URL in the browser, and
//! choosing the terminal editor for notes (o on Notes) with its arguments. GUI helpers are spawned detached (their own
//! session, stdio ignored) so their lifetime is never tied to the UI; failures are reported through `on_error`.

use std::collections::HashMap;

use deckhand_protocol::data::EditorCommand;

/// TS: desktop.ts resolveEditorCommand (`command -v cursor || command -v code` in a login shell; on macOS `open -a
/// Cursor` as a fallback).
pub fn resolve_editor_command() -> Option<EditorCommand> {
    todo!("I8")
}

/// TS: desktop.ts spawnDetached (true when started; a spawn failure goes to `on_error`).
pub fn spawn_detached(command: &str, args: &[String], on_error: &dyn Fn(String)) -> bool {
    let _ = (command, args, on_error);
    todo!("I8")
}

/// TS: desktop.ts openInEditor (Cursor or VS Code; with `line`, `-g <path>:<line>`; the macOS `open -a` fallback
/// just opens the file). Returns the editor's label when started; `could not find cursor or code command on PATH`
/// goes to `on_error`.
pub fn open_in_editor(target: &str, on_error: &dyn Fn(String), line: Option<i64>) -> Option<String> {
    let _ = (target, on_error, line);
    todo!("I8")
}

/// TS: desktop.ts openUrl (https only: `refusing to open a non-https URL`; `open` on macOS, else `xdg-open`).
pub fn open_url(url: &str, on_error: &dyn Fn(String)) -> bool {
    let _ = (url, on_error);
    todo!("I8")
}

/// TS: desktop.ts resolveTerminalEditor: `$VISUAL`, then `$EDITOR` (either may carry arguments, split on
/// whitespace), else the first of nvim, vim and vi; each checked with `command -v` (with this process's PATH).
/// `env`: where VISUAL and EDITOR are read (default the process environment).
pub fn resolve_terminal_editor(env: Option<&HashMap<String, String>>) -> Option<EditorCommand> {
    let _ = env;
    todo!("I8")
}

/// TS: desktop.ts isVimFamily (label `nvim`, `vim` or `vi`).
pub fn is_vim_family(editor: &EditorCommand) -> bool {
    let _ = editor;
    todo!("I8")
}

/// TS: desktop.ts terminalEditorArgs (the editor's own arguments, the file, and for a vim-family editor `-S
/// <vim_script>`).
pub fn terminal_editor_args(editor: &EditorCommand, file: &str, vim_script: Option<&str>) -> Vec<String> {
    let _ = (editor, file, vim_script);
    todo!("I8")
}
