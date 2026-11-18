//! Copying text to the system clipboard through the platform's command-line tool (`pbcopy`; `wl-copy`, `xclip` or
//! `xsel`), without blocking the UI. [`Clipboard`] is what the app holds; tests give it a fake.

use std::fmt;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::process::Stdio;
use std::rc::Rc;
use std::time::Duration;

use tokio::io::AsyncWriteExt;

/// A clipboard command: the program, and the arguments that make it read the clipboard's new text from stdin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipboardTool {
    pub program: &'static str,
    pub args: &'static [&'static str],
}

const PBCOPY: ClipboardTool = ClipboardTool {
    program: "pbcopy",
    args: &[],
};
const WL_COPY: ClipboardTool = ClipboardTool {
    program: "wl-copy",
    args: &[],
};
const XCLIP: ClipboardTool = ClipboardTool {
    program: "xclip",
    args: &["-selection", "clipboard"],
};
const XSEL: ClipboardTool = ClipboardTool {
    program: "xsel",
    args: &["--clipboard", "--input"],
};

/// The tools tried on this platform, in order: `pbcopy` on macOS; elsewhere `wl-copy` (under Wayland), then `xclip`,
/// then `xsel`.
pub fn candidates(macos: bool, wayland: bool) -> Vec<ClipboardTool> {
    if macos {
        return vec![PBCOPY];
    }
    let mut tools = Vec::new();
    if wayland {
        tools.push(WL_COPY);
    }
    tools.extend([XCLIP, XSEL]);
    tools
}

/// The first of `candidates` that `installed` finds.
pub fn find_tool(candidates: &[ClipboardTool], installed: impl Fn(&str) -> bool) -> Option<ClipboardTool> {
    candidates.iter().copied().find(|tool| installed(tool.program))
}

/// Whether `program` is an executable file in a `PATH` directory.
fn on_path(program: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let file = Path::new(&dir).join(program);
        std::fs::metadata(file).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    })
}

/// Why the text was not copied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CopyError {
    /// None of the candidates is installed (their names, in order).
    NoTool(Vec<&'static str>),
    /// The tool could not be started, failed, or did not finish.
    Failed { program: &'static str, detail: String },
}

impl fmt::Display for CopyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CopyError::NoTool(names) => match names.as_slice() {
                [only] => write!(f, "Could not copy: {only} was not found"),
                names => write!(
                    f,
                    "Could not copy: no clipboard tool found (install {})",
                    or_list(names)
                ),
            },
            CopyError::Failed { program, detail } => write!(f, "Could not copy: {program} {detail}"),
        }
    }
}

/// `a`, `a or b`, `a, b or c`.
fn or_list(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [only] => (*only).to_owned(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

/// How long a clipboard tool may take before the copy counts as failed.
const COPY_TIMEOUT: Duration = Duration::from_secs(5);

/// Runs `tool` with `text` on its stdin. Its output is not read: `wl-copy` and `xclip` leave a child behind that
/// serves the clipboard and would hold a pipe open.
pub async fn copy_with(tool: ClipboardTool, text: String) -> Result<(), CopyError> {
    let failed = |detail: String| CopyError::Failed {
        program: tool.program,
        detail,
    };
    let mut child = tokio::process::Command::new(tool.program)
        .args(tool.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| failed(format!("could not start ({error})")))?;
    let run = async {
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .await
                .map_err(|error| failed(format!("did not take the text ({error})")))?;
            // Dropping stdin closes it: the tool sees the end of the text.
        }
        let status = child
            .wait()
            .await
            .map_err(|error| failed(format!("failed ({error})")))?;
        if status.success() {
            Ok(())
        } else {
            Err(failed(format!("failed ({status})")))
        }
    };
    tokio::time::timeout(COPY_TIMEOUT, run)
        .await
        .unwrap_or_else(|_| Err(failed("did not finish".to_owned())))
}

/// The result of one copy.
pub type CopyFuture = Pin<Box<dyn Future<Output = Result<(), CopyError>>>>;

/// Copies text to the clipboard: the system's ([`Clipboard::default`]) or, in tests, a fake.
#[derive(Clone)]
pub struct Clipboard(Rc<dyn Fn(String) -> CopyFuture>);

impl Clipboard {
    /// A clipboard that copies with `copy`.
    pub fn new(copy: impl Fn(String) -> CopyFuture + 'static) -> Self {
        Clipboard(Rc::new(copy))
    }

    /// Copies `text`; the future runs the tool, so the caller spawns it rather than awaiting it on the UI loop.
    pub fn copy(&self, text: String) -> CopyFuture {
        (self.0)(text)
    }
}

impl Default for Clipboard {
    /// The system clipboard: the first installed [`candidates`] tool, looked up on each copy.
    fn default() -> Self {
        Clipboard::new(|text| {
            let tools = candidates(cfg!(target_os = "macos"), std::env::var_os("WAYLAND_DISPLAY").is_some());
            match find_tool(&tools, on_path) {
                Some(tool) => Box::pin(copy_with(tool, text)),
                None => {
                    let names = tools.iter().map(|tool| tool.program).collect();
                    Box::pin(async move { Err(CopyError::NoTool(names)) })
                }
            }
        })
    }
}

impl fmt::Debug for Clipboard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Clipboard")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_platform_tries_its_tools_in_order() {
        let programs = |tools: Vec<ClipboardTool>| tools.into_iter().map(|tool| tool.program).collect::<Vec<_>>();
        assert_eq!(programs(candidates(true, false)), ["pbcopy"]);
        assert_eq!(programs(candidates(false, true)), ["wl-copy", "xclip", "xsel"]);
        assert_eq!(
            programs(candidates(false, false)),
            ["xclip", "xsel"],
            "no wl-copy outside Wayland"
        );
        let linux = candidates(false, true);
        assert_eq!(find_tool(&linux, |program| program != "wl-copy"), Some(XCLIP));
        assert_eq!(find_tool(&linux, |program| program == "xsel"), Some(XSEL));
        assert_eq!(find_tool(&linux, |_| false), None);
        assert_eq!(XCLIP.args, ["-selection", "clipboard"]);
        assert_eq!(XSEL.args, ["--clipboard", "--input"]);
    }

    #[test]
    fn a_failed_copy_says_why() {
        assert_eq!(
            CopyError::NoTool(vec!["pbcopy"]).to_string(),
            "Could not copy: pbcopy was not found"
        );
        assert_eq!(
            CopyError::NoTool(vec!["wl-copy", "xclip", "xsel"]).to_string(),
            "Could not copy: no clipboard tool found (install wl-copy, xclip or xsel)"
        );
        assert_eq!(
            CopyError::NoTool(vec!["xclip", "xsel"]).to_string(),
            "Could not copy: no clipboard tool found (install xclip or xsel)"
        );
        assert_eq!(
            CopyError::Failed {
                program: "xclip",
                detail: "failed (exit status: 1)".to_owned()
            }
            .to_string(),
            "Could not copy: xclip failed (exit status: 1)"
        );
    }

    #[test]
    fn a_tool_that_fails_or_is_missing_is_reported() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| panic!("{error}"));
        runtime.block_on(async {
            // `false` reads nothing and exits 1 (a stand-in for a clipboard tool that fails).
            let failing = ClipboardTool {
                program: "false",
                args: &[],
            };
            match copy_with(failing, "x".repeat(10)).await {
                Err(CopyError::Failed { program: "false", .. }) => {}
                other => panic!("{other:?}"),
            }
            let missing = ClipboardTool {
                program: "deckhand-no-such-clipboard-tool",
                args: &[],
            };
            let error = copy_with(missing, "x".to_owned()).await;
            assert!(
                matches!(&error, Err(CopyError::Failed { detail, .. }) if detail.starts_with("could not start")),
                "{error:?}"
            );
            let cat = ClipboardTool {
                program: "cat",
                args: &[],
            };
            assert_eq!(copy_with(cat, "copied".to_owned()).await, Ok(()));
        });
    }
}
