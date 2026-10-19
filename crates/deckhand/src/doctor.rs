//! Port of setup.ts (doctor, stale Codex hook detection).
//!
//! In TS `deckhand doctor` is `deckhand setup`. The port adds one check: Codex's own hook config (`hooks.json` and
//! `config.toml` in `$CODEX_HOME`, else `~/.codex`) may still call the TS bridge (`node ... cli.js hook`), which no
//! longer exists after switching to the Rust binary, or another Deckhand install. Doctor only READS those files and
//! prints how to regenerate them (`deckhand hooks codex`); it never writes to the Codex home.

// Wave 0 skeleton: the binary does not reach these items until their unit wires them; drop this then.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use deckhand_core::error::DhError;
use deckhand_protocol::data::CodexHookState;

/// The environment variable naming Codex's home (TS: settingsInfo.ts readCodexHooks).
pub const CODEX_HOME_ENV: &str = "CODEX_HOME";
/// Codex's hooks file in its home.
pub const CODEX_HOOKS_FILE: &str = "hooks.json";
/// Codex's config file in its home (hook `command` keys are read from it too).
pub const CODEX_CONFIG_FILE: &str = "config.toml";

/// What doctor found overall (the exit status: problems exit 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoctorStatus {
    Healthy,
    Problems,
}

/// The Codex hooks check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexHookReport {
    /// `<codex home>/hooks.json`.
    pub file: PathBuf,
    /// Whether `hooks.json` could be read.
    pub file_exists: bool,
    /// Every hook command found (`deckhand_sys::settings_info::codex_hook_commands`).
    pub commands: Vec<String>,
    /// `None`: a hook calls this Deckhand; else missing or another install's
    /// (`deckhand_sys::settings_info::codex_hook_state`).
    pub state: Option<CodexHookState>,
    /// The commands that still call the TS bridge ([`is_stale_node_hook`]).
    pub stale: Vec<String>,
}

/// `$CODEX_HOME` (non-empty), else `<home>/.codex` (TS: settingsInfo.ts readCodexHooks `dir`).
pub fn codex_home() -> PathBuf {
    todo!("B2")
}

/// Whether a hook `command` is the TS-era bridge: it runs `node` (any path, optionally `--import <loader>`) on a
/// `cli.js` (or dev `cli.ts`) with the final argument `hook`, quoted or not (TS hookCommand's shape:
/// `'<node>' '<root>/dist/cli.js' 'hook'`).
pub fn is_stale_node_hook(command: &str) -> bool {
    let _ = command;
    todo!("B2")
}

/// The [`is_stale_node_hook`] commands of `commands`, in order.
pub fn stale_node_hooks(commands: &[String]) -> Vec<String> {
    let _ = commands;
    todo!("B2")
}

/// Reads (never writes) `codex_home`'s `hooks.json` and `config.toml` and checks them against `expected` (this
/// Deckhand's printed hook command, `crate::hooks::printed_hook_command`). `None` when `codex_home` is not a
/// directory: Codex isn't used here.
pub async fn read_codex_hook_report(codex_home: &Path, expected: &str) -> Option<CodexHookReport> {
    let _ = (codex_home, expected);
    todo!("B2")
}

/// Whether the report needs action (stale bridge commands, or no hook calling this Deckhand).
pub fn codex_hook_report_has_problems(report: &CodexHookReport) -> bool {
    let _ = report;
    todo!("B2")
}

/// The report as text: each stale command, the state (`missing`/`other`), and the fix: save the output of
/// `regenerate` (`deckhand hooks codex`) as `<file>` (merge it into the `hooks` object when the file exists), then
/// trust it with `/hooks` in Codex.
pub fn format_codex_hook_report(report: &CodexHookReport, regenerate: &str) -> String {
    let _ = (report, regenerate);
    todo!("B2")
}

/// The command that prints the Codex hooks config for this channel (TS: settingsInfo.ts readCodexHooks `command`).
pub fn regenerate_command(dev_channel: bool) -> &'static str {
    let _ = dev_channel;
    todo!("B2")
}

/// `deckhand doctor [--check] [--yes|-y]`: `crate::setup::run_setup(args)`, then the Codex hooks check.
pub async fn run_doctor(args: &[String]) -> Result<DoctorStatus, DhError> {
    let _ = args;
    todo!("B2")
}
