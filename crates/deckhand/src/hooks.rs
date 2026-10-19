//! Port of cli.ts (`deckhand hooks`).
//!
//! Prints the hooks config a user saves into an agent's own settings (Codex: `~/.codex/hooks.json`, see
//! `deckhand hooks codex`). The JSON is `JSON.stringify(hookSettings(), null, 2)` with plain (not async) entries,
//! minus the events the target lacks. Its command must survive rebuilds and reinstalls, so it names the `deckhand`
//! found on `$PATH` when that is this very executable (same device and inode, symlinks followed), else the running
//! executable itself: `'<executable>' 'hook'`, each part shell-quoted as TS hookCommand does.

use std::ffi::OsStr;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use deckhand_core::agent_signals::{HookSettings, hook_settings_for, shell_quote};
use deckhand_core::error::DhError;

/// The executable name looked up on `$PATH`.
pub const EXECUTABLE_NAME: &str = "deckhand";
/// TS: cli.ts events removed for Codex (`hooks codex`).
pub const CODEX_OMITTED_EVENTS: [&str; 2] = ["Notification", "StopFailure"];
/// TS: cli.ts events removed otherwise (Claude doesn't expose Codex's Interrupt event).
pub const DEFAULT_OMITTED_EVENTS: [&str; 1] = ["Interrupt"];

/// Which agent the printed hooks are for (`deckhand hooks` vs `deckhand hooks codex`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HooksTarget {
    /// `deckhand hooks` (Claude-style settings: everything but Interrupt).
    Default,
    /// `deckhand hooks codex`.
    Codex,
}

impl HooksTarget {
    /// TS: cli.ts `process.argv[3] === 'codex'` (anything else is the default).
    pub fn from_arg(arg: Option<&str>) -> HooksTarget {
        if arg == Some("codex") {
            HooksTarget::Codex
        } else {
            HooksTarget::Default
        }
    }

    /// The events this target leaves out ([`CODEX_OMITTED_EVENTS`] or [`DEFAULT_OMITTED_EVENTS`]).
    pub fn omitted_events(self) -> &'static [&'static str] {
        match self {
            HooksTarget::Codex => &CODEX_OMITTED_EVENTS,
            HooksTarget::Default => &DEFAULT_OMITTED_EVENTS,
        }
    }
}

/// Whether `path` is a regular file (symlinks followed) this process may execute (`command -v`'s test).
fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
        && nix::unistd::access(path, nix::unistd::AccessFlags::X_OK).is_ok()
}

/// `command -v deckhand` without a shell: the first `$PATH` entry (`path_env`, colon-separated) holding an
/// executable regular file named `name` (symlinks followed). Empty and relative entries are skipped: the found path
/// is printed into hook settings that run from any directory.
pub fn find_on_path(name: &str, path_env: Option<&OsStr>) -> Option<PathBuf> {
    path_env?
        .as_bytes()
        .split(|byte| *byte == b':')
        .map(|entry| Path::new(OsStr::from_bytes(entry)))
        .filter(|entry| entry.is_absolute())
        .map(|entry| entry.join(name))
        .find(|candidate| is_executable_file(candidate))
}

/// Whether `a` and `b` are the same file (device and inode, symlinks followed); `false` when either is missing.
pub fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// The pure half of [`hooks_executable`]: the `deckhand` on `path_env` when it is `current_exe`'s file, else
/// `current_exe` made absolute against the current directory (TS prints the always-absolute `process.execPath`).
pub fn hooks_executable_from(current_exe: &Path, path_env: Option<&OsStr>) -> PathBuf {
    match find_on_path(EXECUTABLE_NAME, path_env) {
        Some(on_path) if same_file(&on_path, current_exe) => on_path,
        _ => std::path::absolute(current_exe).unwrap_or_else(|_| current_exe.to_path_buf()),
    }
}

/// The executable the printed hooks call: [`hooks_executable_from`] with `std::env::current_exe()` and `$PATH`.
pub fn hooks_executable() -> Result<PathBuf, DhError> {
    let current_exe = std::env::current_exe().map_err(|error| DhError::msg(error.to_string()))?;
    Ok(hooks_executable_from(&current_exe, std::env::var_os("PATH").as_deref()))
}

/// TS: agentSignals.ts hookCommand for the printed config: `'<executable>' 'hook'` (each part `shell_quote`d).
pub fn printed_hook_command(executable: &Path) -> String {
    format!("{} {}", shell_quote(&executable.to_string_lossy()), shell_quote("hook"))
}

/// TS: cli.ts `hookSettings()` (plain entries) with `target`'s omitted events deleted, keeping HOOK_EVENTS order.
pub fn hooks_settings(target: HooksTarget, command: &str) -> HookSettings {
    let mut settings = hook_settings_for(command, false);
    for event in target.omitted_events() {
        settings.hooks.shift_remove(*event);
    }
    settings
}

/// TS: cli.ts `${JSON.stringify(settings, null, 2)}\n`.
pub fn render_hooks(settings: &HookSettings) -> Result<String, DhError> {
    let json = serde_json::to_string_pretty(settings).map_err(|error| DhError::msg(error.to_string()))?;
    Ok(format!("{json}\n"))
}

/// TS: cli.ts the `hooks` branch: prints [`render_hooks`] of [`hooks_settings`] for [`hooks_executable`].
pub fn run_hooks(target: HooksTarget) -> Result<(), DhError> {
    let command = printed_hook_command(&hooks_executable()?);
    let text = render_hooks(&hooks_settings(target, &command))?;
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
        .map_err(|error| DhError::msg(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn write_file(path: &Path, mode: u32) {
        assert!(std::fs::write(path, "#!/bin/sh\n").is_ok());
        assert!(std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).is_ok());
    }

    fn temp_root() -> tempfile::TempDir {
        match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("tempdir: {error}"),
        }
    }

    #[test]
    fn target_from_arg_and_omitted_events() {
        assert_eq!(HooksTarget::from_arg(Some("codex")), HooksTarget::Codex);
        assert_eq!(HooksTarget::from_arg(Some("Codex")), HooksTarget::Default);
        assert_eq!(HooksTarget::from_arg(None), HooksTarget::Default);
        assert_eq!(HooksTarget::Codex.omitted_events(), ["Notification", "StopFailure"]);
        assert_eq!(HooksTarget::Default.omitted_events(), ["Interrupt"]);
    }

    #[test]
    fn find_on_path_takes_the_first_executable_regular_file() {
        let root = temp_root();
        let dirs: Vec<PathBuf> = ["a", "b", "c", "d"].iter().map(|name| root.path().join(name)).collect();
        for dir in &dirs {
            assert!(std::fs::create_dir(dir).is_ok());
        }
        // a: not executable; b: a directory; c: executable; d: executable too, but later.
        write_file(&dirs[0].join("deckhand"), 0o644);
        assert!(std::fs::create_dir(dirs[1].join("deckhand")).is_ok());
        write_file(&dirs[2].join("deckhand"), 0o755);
        write_file(&dirs[3].join("deckhand"), 0o755);
        let joined: Vec<String> = dirs.iter().map(|dir| dir.display().to_string()).collect();
        let path = format!("::{}", joined.join(":"));
        assert_eq!(
            find_on_path("deckhand", Some(OsStr::new(&path))),
            Some(dirs[2].join("deckhand"))
        );
        assert_eq!(find_on_path("deckhand", None), None);
        assert_eq!(find_on_path("deckhand", Some(OsStr::new(""))), None);
        assert_eq!(find_on_path("missing", Some(OsStr::new(&path))), None);
    }

    #[test]
    fn hooks_executable_prefers_the_path_entry_only_when_it_is_this_file() {
        let root = temp_root();
        let real = root.path().join("real");
        let bin = root.path().join("bin");
        let other = root.path().join("other");
        for dir in [&real, &bin, &other] {
            assert!(std::fs::create_dir(dir).is_ok());
        }
        let exe = real.join("deckhand");
        write_file(&exe, 0o755);
        assert!(symlink(&exe, bin.join("deckhand")).is_ok());
        write_file(&other.join("deckhand"), 0o755);

        assert!(same_file(&bin.join("deckhand"), &exe));
        assert!(!same_file(&other.join("deckhand"), &exe));
        assert!(!same_file(&root.path().join("missing"), &exe));

        let on_path = bin.display().to_string();
        assert_eq!(
            hooks_executable_from(&exe, Some(OsStr::new(&on_path))),
            bin.join("deckhand")
        );
        let elsewhere = format!("{}:{}", other.display(), bin.display());
        assert_eq!(hooks_executable_from(&exe, Some(OsStr::new(&elsewhere))), exe);
        assert_eq!(hooks_executable_from(&exe, None), exe);
    }

    // The printed command runs from whatever directory the agent is in, so it is always absolute (TS prints
    // `process.execPath`): a relative `$PATH` entry is never what gets printed.
    #[test]
    fn hooks_executable_is_absolute_even_with_relative_path_entries() {
        let root = temp_root();
        let exe = root.path().join("deckhand");
        write_file(&exe, 0o755);
        let bin = root.path().join("bin");
        assert!(std::fs::create_dir(&bin).is_ok());
        assert!(symlink(&exe, bin.join("deckhand")).is_ok());
        // Nextest runs each test in its own process: changing the directory affects no other test.
        assert!(std::env::set_current_dir(root.path()).is_ok());

        let picked = hooks_executable_from(&exe, Some(OsStr::new(".")));
        assert!(
            picked.is_absolute(),
            "printed hook executable is relative: {}",
            picked.display()
        );
        assert!(same_file(&picked, &exe));
        // A relative entry is skipped; a later absolute entry naming this file still wins.
        let path = format!(".:bin:{}", bin.display());
        assert_eq!(
            hooks_executable_from(&exe, Some(OsStr::new(&path))),
            bin.join("deckhand")
        );
        assert_eq!(find_on_path("deckhand", Some(OsStr::new(".:bin"))), None);
        // A relative running executable is made absolute against the current directory.
        let picked = hooks_executable_from(Path::new("deckhand"), None);
        assert!(
            picked.is_absolute(),
            "printed hook executable is relative: {}",
            picked.display()
        );
        assert!(same_file(&picked, &exe));
    }
}
