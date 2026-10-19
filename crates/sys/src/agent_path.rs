//! Port of agentSignals.ts (process/PATH parts).
//!
//! What launches pass an agent so it reports its lifecycle to Deckhand: the `deckhand hook` command line (this
//! executable), the hook settings built on it (`deckhand_core::agent_signals::hook_settings_for`), and the
//! integration arguments, which depend on what the installed agent's `--help` offers (probed once per command, only
//! successful probes cached). Normalising hook payloads is pure and lives in `deckhand_core::agent_signals`.

use std::time::Duration;

use deckhand_core::agent_signals::HookSettings;
use deckhand_core::error::Result;
use deckhand_protocol::data::ProgramKey;

/// TS: agentSignals.ts programHelp timeout (`<command> --help`).
pub const HELP_TIMEOUT: Duration = Duration::from_millis(5000);
/// TS: agentSignals.ts programHelp maxBuffer.
pub const HELP_MAX_BUFFER: usize = 512 * 1024;

/// TS: agentSignals.ts hookCommand: `'<this executable>' 'hook'`, each part shell-quoted (TS: node, the tsx loader
/// in dev, the CLI entry, `hook`).
pub fn hook_command() -> Result<String> {
    todo!("I7")
}

/// TS: agentSignals.ts hookSettings (every supported event calls [`hook_command`]; `async_`: Claude's background
/// hooks).
pub fn hook_settings(async_: bool) -> Result<HookSettings> {
    let _ = async_;
    todo!("I7")
}

/// TS: agentSignals.ts programHelp (`<command> --help` stdout, bounded; empty on failure; successes cached per
/// command for the process's life, concurrent probes shared).
pub async fn program_help(command: &str) -> String {
    let _ = command;
    todo!("I7")
}

/// TS: agentSignals.ts integrationArgs: Claude with `--settings` gets the hook settings (without StopFailure and
/// Interrupt); Codex gets `--no-daemon` when it has it, which the isolated dev channel requires (`This Codex version
/// cannot verify isolated native-daemon operation (--no-daemon missing). Upgrade Codex or test with Claude/Pi.`).
pub async fn integration_args(program: ProgramKey, command: &str, enabled: bool) -> Result<Vec<String>> {
    let _ = (program, command, enabled);
    todo!("I7")
}
