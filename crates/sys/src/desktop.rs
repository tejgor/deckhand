//! Port of desktop.ts.
//!
//! Opening things outside the terminal UI: a file in Cursor or VS Code (at a line), an https URL in the browser, and
//! choosing the terminal editor for notes (o on Notes) with its arguments. GUI helpers are spawned detached (their own
//! session, stdio ignored) so their lifetime is never tied to the UI; failures are reported through `on_error`.

use std::collections::HashMap;
use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use deckhand_core::error::errno_code;
use deckhand_core::jsstr::{is_js_whitespace, js_split_whitespace, js_trim};
use deckhand_protocol::data::EditorCommand;

/// TS: desktop.ts resolveEditorCommand (`command -v cursor || command -v code` in a login shell; on macOS `open -a
/// Cursor` as a fallback).
pub fn resolve_editor_command() -> Option<EditorCommand> {
    let found = Command::new("sh")
        .args(["-lc", "command -v cursor || command -v code"])
        .stdin(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.code() == Some(0))
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned());
    let command = found
        .as_deref()
        .map(js_trim)
        .and_then(|text| text.split('\n').next())
        .unwrap_or("");
    if !command.is_empty() {
        let label = if command.contains("cursor") { "Cursor" } else { "Code" };
        return Some(EditorCommand {
            command: command.to_owned(),
            args: Vec::new(),
            label: label.to_owned(),
        });
    }
    if cfg!(target_os = "macos") {
        return Some(EditorCommand {
            command: "open".to_owned(),
            args: vec!["-a".to_owned(), "Cursor".to_owned()],
            label: "Cursor".to_owned(),
        });
    }
    None
}

/// Node's message for a failed `spawn`: `spawn <file> <CODE>` for a system error.
fn spawn_error_message(command: &str, error: &io::Error) -> String {
    match errno_code(error) {
        Some(code) => format!("spawn {command} {code}"),
        None => error.to_string(),
    }
}

/// TS: desktop.ts spawnDetached (true when started; a spawn failure goes to `on_error`).
///
/// Node reports a system error (`ENOENT`: not installed) through the child's `error` event after `spawn` returned,
/// so that case still returns true; only an invalid argument (a NUL byte, which Node rejects synchronously) returns
/// false.
pub fn spawn_detached(command: &str, args: &[String], on_error: &dyn Fn(String)) -> bool {
    let mut child = Command::new(command);
    child
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe and touches no memory of the parent (Node `detached: true`).
    unsafe {
        child.pre_exec(|| nix::unistd::setsid().map(|_| ()).map_err(io::Error::from));
    }
    match child.spawn() {
        Ok(mut child) => {
            // `child.unref()`: nothing waits for it, but it is still reaped (libuv does that for Node).
            let _ = std::thread::Builder::new()
                .name("deckhand-detached-reaper".to_owned())
                .spawn(move || {
                    let _ = child.wait();
                });
            true
        }
        Err(error) => {
            let synchronous = error.raw_os_error().is_none();
            on_error(spawn_error_message(command, &error));
            !synchronous
        }
    }
}

/// TS: desktop.ts openInEditor (Cursor or VS Code; with `line`, `-g <path>:<line>`; the macOS `open -a` fallback
/// just opens the file). Returns the editor's label when started; `could not find cursor or code command on PATH`
/// goes to `on_error`.
pub fn open_in_editor(target: &str, on_error: &dyn Fn(String), line: Option<i64>) -> Option<String> {
    let Some(editor) = resolve_editor_command() else {
        on_error("could not find cursor or code command on PATH".to_owned());
        return None;
    };
    let cli = editor.command != "open";
    let target_args = match line {
        Some(line) if cli => vec!["-g".to_owned(), format!("{target}:{}", line.max(1))],
        _ => vec![target.to_owned()],
    };
    let args: Vec<String> = editor.args.iter().cloned().chain(target_args).collect();
    spawn_detached(&editor.command, &args, on_error).then_some(editor.label)
}

/// `/^https:\/\/[^\s]+$/`.
fn is_https_url(url: &str) -> bool {
    url.strip_prefix("https://")
        .is_some_and(|rest| !rest.is_empty() && !rest.chars().any(is_js_whitespace))
}

/// TS: desktop.ts openUrl (https only: `refusing to open a non-https URL`; `open` on macOS, else `xdg-open`).
pub fn open_url(url: &str, on_error: &dyn Fn(String)) -> bool {
    if !is_https_url(url) {
        on_error("refusing to open a non-https URL".to_owned());
        return false;
    }
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    spawn_detached(opener, &[url.to_owned()], on_error)
}

/// `command -v "$1"` with this process's environment: whether `command` names something runnable.
fn command_exists(command: &str) -> bool {
    Command::new("sh")
        .args(["-c", "command -v \"$1\"", "sh", command])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|output| {
            output.status.code() == Some(0) && !js_trim(&String::from_utf8_lossy(&output.stdout)).is_empty()
        })
}

/// TS: desktop.ts resolveTerminalEditor: `$VISUAL`, then `$EDITOR` (either may carry arguments, split on
/// whitespace), else the first of nvim, vim and vi; each checked with `command -v` (with this process's PATH).
/// `env`: where VISUAL and EDITOR are read (default the process environment).
pub fn resolve_terminal_editor(env: Option<&HashMap<String, String>>) -> Option<EditorCommand> {
    let var = |name: &str| -> String {
        match env {
            Some(env) => env.get(name).cloned().unwrap_or_default(),
            None => std::env::var_os(name)
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default(),
        }
    };
    let visual = var("VISUAL");
    let configured_raw = if visual.is_empty() { var("EDITOR") } else { visual };
    let configured = js_trim(&configured_raw);
    let candidates: Vec<Vec<&str>> = if configured.is_empty() {
        vec![vec!["nvim"], vec!["vim"], vec!["vi"]]
    } else {
        vec![js_split_whitespace(configured)]
    };
    for candidate in candidates {
        let Some((command, args)) = candidate.split_first() else {
            continue;
        };
        if command.is_empty() {
            continue;
        }
        if command_exists(command) {
            return Some(EditorCommand {
                command: (*command).to_owned(),
                args: args.iter().map(|arg| (*arg).to_owned()).collect(),
                label: command.rsplit('/').next().unwrap_or(command).to_owned(),
            });
        }
    }
    None
}

/// TS: desktop.ts isVimFamily (label `nvim`, `vim` or `vi`).
pub fn is_vim_family(editor: &EditorCommand) -> bool {
    matches!(editor.label.as_str(), "nvim" | "vim" | "vi")
}

/// TS: desktop.ts terminalEditorArgs (the editor's own arguments, the file, and for a vim-family editor `-S
/// <vim_script>`).
pub fn terminal_editor_args(editor: &EditorCommand, file: &str, vim_script: Option<&str>) -> Vec<String> {
    let mut args = editor.args.clone();
    args.push(file.to_owned());
    if let Some(script) = vim_script.filter(|script| !script.is_empty())
        && is_vim_family(editor)
    {
        args.push("-S".to_owned());
        args.push(script.to_owned());
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use deckhand_testkit::{Case, replay, tags};
    use std::cell::RefCell;

    fn editor(command: &str, args: &[&str], label: &str) -> EditorCommand {
        EditorCommand {
            command: command.to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            label: label.to_owned(),
        }
    }

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    // covers: B-I8-01 the terminal editor is $VISUAL, then $EDITOR (with its arguments), else a vi found on PATH
    #[test]
    fn terminal_editor_is_visual_then_editor_else_vi() {
        assert_eq!(
            resolve_terminal_editor(Some(&vars(&[("VISUAL", "sh -x"), ("EDITOR", "nope")]))),
            Some(editor("sh", &["-x"], "sh"))
        );
        assert_eq!(
            resolve_terminal_editor(Some(&vars(&[("EDITOR", "  /bin/sh  ")]))),
            Some(editor("/bin/sh", &[], "sh"))
        );
        assert_eq!(
            resolve_terminal_editor(Some(&vars(&[("EDITOR", "deckhand-no-such-editor")]))),
            None
        );
        // Unset: nvim, vim or vi, whichever this machine has.
        let path = std::env::var("PATH").unwrap_or_default();
        if let Some(fallback) = resolve_terminal_editor(Some(&vars(&[("PATH", &path)]))) {
            assert!(matches!(fallback.label.as_str(), "nvim" | "vim" | "vi"), "{fallback:?}");
        }
    }

    // covers: B-I8-02 vim-family editors source the notes script after the note loads; others only get the file
    #[test]
    fn vim_family_editors_source_the_notes_script() {
        assert_eq!(
            terminal_editor_args(
                &editor("/usr/bin/vim", &[], "vim"),
                "/notes/a.md",
                Some("/dh/notes.vim")
            ),
            ["/notes/a.md", "-S", "/dh/notes.vim"]
        );
        assert_eq!(
            terminal_editor_args(&editor("nvim", &["--clean"], "nvim"), "n.md", Some("s.vim")),
            ["--clean", "n.md", "-S", "s.vim"]
        );
        assert_eq!(
            terminal_editor_args(&editor("nvim", &[], "nvim"), "n.md", None),
            ["n.md"]
        );
        assert_eq!(
            terminal_editor_args(&editor("nano", &["-l"], "nano"), "n.md", Some("s.vim")),
            ["-l", "n.md"]
        );
    }

    #[test]
    fn open_url_refuses_non_https_urls() {
        let errors = RefCell::new(Vec::new());
        let on_error = |message: String| errors.borrow_mut().push(message);
        for url in [
            "http://example.com",
            "https://",
            "https://a b",
            "file:///etc/passwd",
            " https://x",
        ] {
            assert!(!open_url(url, &on_error), "{url}");
        }
        assert_eq!(errors.borrow().len(), 5);
        assert!(
            errors
                .borrow()
                .iter()
                .all(|message| message == "refusing to open a non-https URL")
        );
        assert!(is_https_url("https://example.com/a?b=c"));
        assert!(!is_https_url("https://x\n"));
    }

    #[test]
    fn spawn_detached_reports_a_missing_program_after_starting() {
        let errors = RefCell::new(Vec::new());
        let on_error = |message: String| errors.borrow_mut().push(message);
        assert!(spawn_detached("/no/such/deckhand-helper", &[], &on_error));
        assert_eq!(*errors.borrow(), ["spawn /no/such/deckhand-helper ENOENT"]);
        errors.borrow_mut().clear();
        assert!(spawn_detached(
            "/bin/sh",
            &["-c".to_owned(), "exit 0".to_owned()],
            &on_error
        ));
        assert!(errors.borrow().is_empty());
        assert!(!spawn_detached("/bin/sh", &["a\0b".to_owned()], &on_error));
        assert_eq!(errors.borrow().len(), 1);
    }

    fn editor_arg(case: &Case) -> Result<EditorCommand, String> {
        case.arg_as::<EditorCommand>(0)
    }

    #[test]
    fn is_vim_family_parity() {
        replay("desktop/isVimFamily", |case: &Case| -> Result<bool, String> {
            Ok(is_vim_family(&editor_arg(case)?))
        });
    }

    #[test]
    fn terminal_editor_args_parity() {
        replay(
            "desktop/terminalEditorArgs",
            |case: &Case| -> Result<Vec<String>, String> {
                let script = if case.arg_is_undefined(2) {
                    None
                } else {
                    Some(case.arg_str(2)?)
                };
                Ok(terminal_editor_args(&editor_arg(case)?, case.arg_str(1)?, script))
            },
        );
    }

    #[test]
    fn resolve_terminal_editor_parity() {
        replay(
            "desktop/resolveTerminalEditor",
            |case: &Case| -> Result<serde_json::Value, String> {
                let env = case.arg_as::<HashMap<String, String>>(0)?;
                Ok(match resolve_terminal_editor(Some(&env)) {
                    Some(editor) => serde_json::to_value(editor).map_err(|error| error.to_string())?,
                    None => tags::undefined(),
                })
            },
        );
    }
}
