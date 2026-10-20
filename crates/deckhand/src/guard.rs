//! Port of debug-build refusal to use ~/.deckhand.
//!
//! Not in the TS: the Rust port's debug builds (`target/debug/deckhand`, which tests and agents run) must never touch
//! the production state directory by accident. The check itself (canonical paths, symlinks, the
//! `DECKHAND_ALLOW_PROD_HOME=1` override) lives in `deckhand_sys::paths::guard_debug_home`; this module decides which
//! commands it applies to. It is real (not a stub) from Wave 0 on, so no later unit can run against `~/.deckhand`.

use deckhand_core::error::DhError;

use crate::cli::Command;

/// Whether `command` reads or writes the state directory (`DECKHAND_HOME` / `~/.deckhand`): everything except
/// printing the hooks config, `setup`/`doctor` (which only probe the PATH and read Codex's own config), `--version`
/// and `help`.
pub fn needs_home(command: &Command) -> bool {
    !matches!(
        command,
        Command::Hooks(_) | Command::Setup(_) | Command::Doctor(_) | Command::Version | Command::Help
    )
}

/// The debug-build guard: in a debug build, an error when the resolved state directory is (or is inside)
/// `~/.deckhand` and `DECKHAND_ALLOW_PROD_HOME` is not `1`; release builds always pass.
pub fn check_home() -> Result<(), DhError> {
    deckhand_sys::paths::guard_debug_home()
}
