//! Port of cli.ts (`deckhand hooks`).
//!
//! Prints the hooks config a user saves into an agent's own settings (Codex: `~/.codex/hooks.json`, see
//! `deckhand hooks codex`). The JSON is `JSON.stringify(hookSettings(), null, 2)` with plain (not async) entries,
//! minus the events the target lacks. Its command must survive rebuilds and reinstalls, so it names the `deckhand`
//! found on `$PATH` when that is this very executable (same device and inode, symlinks followed), else the running
//! executable itself: `'<executable>' 'hook'`, each part shell-quoted as TS hookCommand does.

// Wave 0 skeleton: the binary does not reach these items until their unit wires them; drop this then.
#![allow(dead_code)]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use deckhand_core::agent_signals::HookSettings;
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
        let _ = arg;
        todo!("B1")
    }

    /// The events this target leaves out ([`CODEX_OMITTED_EVENTS`] or [`DEFAULT_OMITTED_EVENTS`]).
    pub fn omitted_events(self) -> &'static [&'static str] {
        todo!("B1")
    }
}

/// `command -v deckhand` without a shell: the first `$PATH` entry (`path_env`, colon-separated; empty entries
/// skipped) holding an executable regular file named `name` (symlinks followed).
pub fn find_on_path(name: &str, path_env: Option<&OsStr>) -> Option<PathBuf> {
    let _ = (name, path_env);
    todo!("B1")
}

/// Whether `a` and `b` are the same file (device and inode, symlinks followed); `false` when either is missing.
pub fn same_file(a: &Path, b: &Path) -> bool {
    let _ = (a, b);
    todo!("B1")
}

/// The pure half of [`hooks_executable`]: the `deckhand` on `path_env` when it is `current_exe`'s file, else
/// `current_exe`.
pub fn hooks_executable_from(current_exe: &Path, path_env: Option<&OsStr>) -> PathBuf {
    let _ = (current_exe, path_env);
    todo!("B1")
}

/// The executable the printed hooks call: [`hooks_executable_from`] with `std::env::current_exe()` and `$PATH`.
pub fn hooks_executable() -> Result<PathBuf, DhError> {
    todo!("B1")
}

/// TS: agentSignals.ts hookCommand for the printed config: `'<executable>' 'hook'` (each part `shell_quote`d).
pub fn printed_hook_command(executable: &Path) -> String {
    let _ = executable;
    todo!("B1")
}

/// TS: cli.ts `hookSettings()` (plain entries) with `target`'s omitted events deleted, keeping HOOK_EVENTS order.
pub fn hooks_settings(target: HooksTarget, command: &str) -> HookSettings {
    let _ = (target, command);
    todo!("B1")
}

/// TS: cli.ts `${JSON.stringify(settings, null, 2)}\n`.
pub fn render_hooks(settings: &HookSettings) -> Result<String, DhError> {
    let _ = settings;
    todo!("B1")
}

/// TS: cli.ts the `hooks` branch: prints [`render_hooks`] of [`hooks_settings`] for [`hooks_executable`].
pub fn run_hooks(target: HooksTarget) -> Result<(), DhError> {
    let _ = target;
    todo!("B1")
}
