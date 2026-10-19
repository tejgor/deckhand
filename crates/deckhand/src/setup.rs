//! Port of setup.ts.
//!
//! `deckhand setup [--check] [--yes|-y]`: reports which agents (and the optional lazygit) are on the user's PATH as
//! their interactive shell sees it (`$SHELL -ic 'command -v <tool>'`), and offers to run each missing agent's
//! official installer. `--check` only reports. Tests run it with a temp `HOME`, so the shell reads scratch rc files.

// Wave 0 skeleton: the binary does not reach these items until their unit wires them; drop this then.
#![allow(dead_code)]

use deckhand_core::error::DhError;

/// TS: setup.ts ToolInstall.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolInstall {
    pub key: &'static str,
    pub label: &'static str,
    pub command: &'static str,
    /// Agents only; optional tools are never installed by setup.
    pub install_command: Option<&'static str>,
    pub description: &'static str,
    pub optional: bool,
}

/// TS: setup.ts ToolStatus (a [`ToolInstall`] plus what detection found).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolStatus {
    pub tool: ToolInstall,
    pub installed: bool,
    /// `command -v` output, trimmed (set when installed).
    pub path: Option<String>,
}

/// TS: setup.ts AGENT_TOOLS.
pub const AGENT_TOOLS: [ToolInstall; 3] = [
    ToolInstall {
        key: "claude",
        label: "Claude Code",
        command: "claude",
        install_command: Some("curl -fsSL https://claude.ai/install.sh | bash"),
        description: "Anthropic Claude Code agent",
        optional: false,
    },
    ToolInstall {
        key: "pi",
        label: "Pi",
        command: "pi",
        install_command: Some("curl -fsSL https://pi.dev/install.sh | sh"),
        description: "Pi coding agent",
        optional: false,
    },
    ToolInstall {
        key: "codex",
        label: "Codex",
        command: "codex",
        install_command: Some("curl -fsSL https://chatgpt.com/codex/install.sh | sh"),
        description: "OpenAI Codex CLI agent",
        optional: false,
    },
];

/// TS: setup.ts OPTIONAL_TOOLS.
pub const OPTIONAL_TOOLS: [ToolInstall; 1] = [ToolInstall {
    key: "lazygit",
    label: "lazygit",
    command: "lazygit",
    install_command: None,
    description: "optional Git tab UI",
    optional: true,
}];

/// TS: setup.ts runSetup's flags (`--yes`/`-y`, `--check`; anything else ignored).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SetupArgs {
    pub assume_yes: bool,
    pub check_only: bool,
}

impl SetupArgs {
    /// TS: setup.ts `args.includes('--yes') || args.includes('-y')`, `args.includes('--check')`.
    pub fn parse(args: &[String]) -> SetupArgs {
        let _ = args;
        todo!("B2")
    }
}

/// TS: setup.ts splitCommand result (a command line run by the user's shell).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellCommand {
    pub file: String,
    pub args: Vec<String>,
}

/// TS: setup.ts shell (`$SHELL`, else `/bin/bash`; empty counts as unset).
pub fn shell() -> String {
    todo!("B2")
}

/// TS: setup.ts commandPath (`<shell> -ic 'command -v <command>'`, trimmed stdout; `None` on failure or empty
/// output).
pub async fn command_path(command: &str) -> Option<String> {
    let _ = command;
    todo!("B2")
}

/// TS: setup.ts detectTools (agents then optional tools, probed concurrently, in that order).
pub async fn detect_tools() -> Vec<ToolStatus> {
    todo!("B2")
}

/// TS: setup.ts splitCommand (`{file: shell(), args: ['-ic', command]}`).
pub fn split_command(command: &str) -> ShellCommand {
    let _ = command;
    todo!("B2")
}

/// TS: setup.ts runInstall (stdio inherited; the exit code, 1 on a spawn error or a signal).
pub async fn run_install(command: &str) -> i32 {
    let _ = command;
    todo!("B2")
}

/// TS: setup.ts printStatus as text: `Deckhand setup check`, a blank line, one line per tool (`✓`/`•`, label,
/// ` optional`, description, ` (<path>)` or ` (not installed)`), and a blank line.
pub fn format_status(statuses: &[ToolStatus]) -> String {
    let _ = statuses;
    todo!("B2")
}

/// TS: setup.ts printStatus (writes [`format_status`] to stdout).
pub fn print_status(statuses: &[ToolStatus]) {
    let _ = statuses;
    todo!("B2")
}

/// TS: setup.ts promptYes (`<question> [y/N] ` on stdout; `y`/`yes`, trimmed and lowercased, is yes; `assume_yes`
/// skips the prompt).
pub async fn prompt_yes(question: &str, assume_yes: bool) -> bool {
    let _ = (question, assume_yes);
    todo!("B2")
}

/// TS: setup.ts runSetup (exported): status, then for each missing agent its install command and (unless
/// `--check`) an offer to run it, then the status after setup and what is still missing.
pub async fn run_setup(args: &[String]) -> Result<(), DhError> {
    let _ = args;
    todo!("B2")
}
