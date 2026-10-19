//! Port of setup.ts (doctor, stale Codex hook detection).
//!
//! In TS `deckhand doctor` is `deckhand setup`. The port adds one check: Codex's own hook config (`hooks.json` and
//! `config.toml` in `$CODEX_HOME`, else `~/.codex`) may still call the TS bridge (`node ... cli.js hook`), which no
//! longer exists after switching to the Rust binary, or another Deckhand install. Doctor only READS those files and
//! prints how to regenerate them (`deckhand hooks codex`); it never writes to the Codex home.

// B1 wires `deckhand doctor` into the binary; until then only the tests reach these items.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use deckhand_core::error::DhError;
use deckhand_protocol::data::CodexHookState;
use deckhand_sys::settings_info::{codex_hook_commands, codex_hook_state};

/// The environment variable naming Codex's home (TS: settingsInfo.ts readCodexHooks).
pub const CODEX_HOME_ENV: &str = "CODEX_HOME";
/// Codex's hooks file in its home.
pub const CODEX_HOOKS_FILE: &str = "hooks.json";
/// Codex's config file in its home (hook `command` keys are read from it too).
pub const CODEX_CONFIG_FILE: &str = "config.toml";

/// What doctor found overall. TS doctor is setup and always exits 0, so both map to exit status 0 (parity);
/// `Problems` only says the Codex hooks warning was printed.
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
    match std::env::var_os(CODEX_HOME_ENV) {
        Some(home) if !home.is_empty() => PathBuf::from(home),
        _ => deckhand_sys::paths::home_dir().join(".codex"),
    }
}

/// POSIX shell words of `command` (quotes removed, `'\''` joined), as the agent's `sh -c` would split it before
/// expansion. `None` for an unterminated quote or trailing backslash.
fn shell_words(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut chars = command.chars();
    while let Some(ch) = chars.next() {
        match ch {
            ' ' | '\t' | '\n' => words.extend(word.take()),
            '\'' => {
                let current = word.get_or_insert_with(String::new);
                loop {
                    match chars.next()? {
                        '\'' => break,
                        other => current.push(other),
                    }
                }
            }
            '"' => {
                let current = word.get_or_insert_with(String::new);
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => match chars.next()? {
                            escaped @ ('$' | '`' | '"' | '\\') => current.push(escaped),
                            '\n' => {}
                            other => {
                                current.push('\\');
                                current.push(other);
                            }
                        },
                        other => current.push(other),
                    }
                }
            }
            '\\' => match chars.next()? {
                '\n' => {}
                escaped => word.get_or_insert_with(String::new).push(escaped),
            },
            other => word.get_or_insert_with(String::new).push(other),
        }
    }
    words.extend(word);
    Some(words)
}

/// The last `/`-separated part of a path.
fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Node options that take the next word as their value (`--import <loader>` in dev).
const NODE_VALUE_OPTIONS: [&str; 5] = ["--import", "--require", "-r", "--loader", "--experimental-loader"];

/// Whether a hook `command` is the TS-era bridge: it runs `node` (any path, optionally `--import <loader>`) on a
/// `cli.js` (or dev `cli.ts`) with the final argument `hook`, quoted or not (TS hookCommand's shape:
/// `'<node>' '<root>/dist/cli.js' 'hook'`).
pub fn is_stale_node_hook(command: &str) -> bool {
    let Some(words) = shell_words(command) else {
        return false;
    };
    let [program, middle @ .., last] = words.as_slice() else {
        return false;
    };
    if last != "hook" || !matches!(basename(program), "node" | "nodejs") {
        return false;
    }
    // Node options, then the entry script, which must be the word right before `hook`.
    let mut rest = middle;
    loop {
        match rest {
            [option, _value, tail @ ..] if NODE_VALUE_OPTIONS.contains(&option.as_str()) => rest = tail,
            [option, tail @ ..] if option.starts_with('-') => rest = tail,
            [entry] => return matches!(basename(entry), "cli.js" | "cli.ts"),
            _ => return false,
        }
    }
}

/// The [`is_stale_node_hook`] commands of `commands`, in order.
pub fn stale_node_hooks(commands: &[String]) -> Vec<String> {
    commands
        .iter()
        .filter(|command| is_stale_node_hook(command))
        .cloned()
        .collect()
}

/// `fs.readFile(file, 'utf8')`, `None` on any error.
async fn read_text(file: &Path) -> Option<String> {
    tokio::fs::read(file)
        .await
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// Reads (never writes) `codex_home`'s `hooks.json` and `config.toml` and checks them against `expected` (this
/// Deckhand's printed hook command, `crate::hooks::printed_hook_command`). `None` when `codex_home` is not a
/// directory: Codex isn't used here.
pub async fn read_codex_hook_report(codex_home: &Path, expected: &str) -> Option<CodexHookReport> {
    if !tokio::fs::metadata(codex_home).await.is_ok_and(|meta| meta.is_dir()) {
        return None;
    }
    let file = codex_home.join(CODEX_HOOKS_FILE);
    let config = codex_home.join(CODEX_CONFIG_FILE);
    let (hooks_json, config_toml) = tokio::join!(read_text(&file), read_text(&config));
    let commands = codex_hook_commands(hooks_json.as_deref(), config_toml.as_deref());
    let state = codex_hook_state(&commands, expected);
    let stale = stale_node_hooks(&commands);
    Some(CodexHookReport {
        file,
        file_exists: hooks_json.is_some(),
        commands,
        state,
        stale,
    })
}

/// Whether the report needs action (stale bridge commands, or no hook calling this Deckhand).
pub fn codex_hook_report_has_problems(report: &CodexHookReport) -> bool {
    !report.stale.is_empty() || report.state.is_some()
}

/// The report as text: each stale command, the state (`missing`/`other`), and the fix: save the output of
/// `regenerate` (`deckhand hooks codex`) as `<file>` (merge it into the `hooks` object when the file exists), then
/// trust it with `/hooks` in Codex.
pub fn format_codex_hook_report(report: &CodexHookReport, regenerate: &str) -> String {
    let file = report.file.to_string_lossy();
    if !codex_hook_report_has_problems(report) {
        return format!("✓ Codex hooks call this Deckhand ({file})\n");
    }
    let mut text = format!("Codex hooks ({file}):\n");
    for command in &report.stale {
        text.push_str(&format!(
            "• Stale hook (the TypeScript bridge, node ... cli.js hook, which no longer exists): {command}\n"
        ));
    }
    match report.state {
        Some(CodexHookState::Other) => text.push_str("• Codex hooks call another Deckhand install.\n"),
        Some(CodexHookState::Missing) => text.push_str("• No Codex hook calls this Deckhand.\n"),
        None => {}
    }
    let fix = match (report.state, report.file_exists) {
        // This Deckhand's hook is already there: only the stale entries must go.
        (None, _) => format!("Fix: remove the stale hooks from {file}"),
        (Some(_), true) => format!("Fix: merge the `hooks` object printed by `{regenerate}` into {file}"),
        (Some(_), false) => format!("Fix: save the output of `{regenerate}` as {file}"),
    };
    text.push_str(&fix);
    if !report.stale.is_empty() && report.state.is_some() {
        text.push_str(", replacing the stale hooks");
    }
    if !report.stale.is_empty() {
        text.push_str(&format!(" (and from {CODEX_CONFIG_FILE}, if any are there)"));
    }
    text.push_str(", then trust them with /hooks in Codex.\n");
    text
}

/// The command that prints the Codex hooks config for this channel (TS: settingsInfo.ts readCodexHooks `command`).
pub fn regenerate_command(dev_channel: bool) -> &'static str {
    if dev_channel {
        "node scripts/deckhand-dev.mjs hooks codex"
    } else {
        "deckhand hooks codex"
    }
}

/// The Codex part of doctor: prints the report for `codex_home` (nothing when Codex isn't used here).
async fn codex_hook_check(codex_home: &Path, expected: &str, regenerate: &str) -> (DoctorStatus, Option<String>) {
    let Some(report) = read_codex_hook_report(codex_home, expected).await else {
        return (DoctorStatus::Healthy, None);
    };
    let status = if codex_hook_report_has_problems(&report) {
        DoctorStatus::Problems
    } else {
        DoctorStatus::Healthy
    };
    (status, Some(format_codex_hook_report(&report, regenerate)))
}

/// `deckhand doctor [--check] [--yes|-y]`: `crate::setup::run_setup(args)`, then the Codex hooks check.
pub async fn run_doctor(args: &[String]) -> Result<DoctorStatus, DhError> {
    crate::setup::run_setup(args).await?;
    let home = codex_home();
    // Only look up this executable's hook command when there is a Codex config to compare it with.
    if !tokio::fs::metadata(&home).await.is_ok_and(|meta| meta.is_dir()) {
        return Ok(DoctorStatus::Healthy);
    }
    let expected = crate::hooks::printed_hook_command(&crate::hooks::hooks_executable()?);
    let regenerate = regenerate_command(deckhand_sys::paths::is_dev_channel());
    let (status, text) = codex_hook_check(&home, &expected, regenerate).await;
    if let Some(text) = text {
        crate::setup::write_stdout(&text);
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPECTED: &str = "'/opt/bin/deckhand' 'hook'";
    const STALE: &str = "'/usr/local/bin/node' '/Users/me/deckhand/dist/cli.js' 'hook'";

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
    }

    fn hooks_json(commands: &[&str]) -> String {
        let entries: Vec<String> = commands
            .iter()
            .map(|command| {
                format!(
                    r#"[{{"hooks":[{{"type":"command","command":{},"timeout":2}}]}}]"#,
                    serde_json::Value::String((*command).to_owned())
                )
            })
            .collect();
        let events = ["SessionStart", "Stop", "Interrupt"];
        let body: Vec<String> = entries
            .iter()
            .zip(events)
            .map(|(entry, event)| format!(r#""{event}":{entry}"#))
            .collect();
        format!(r#"{{"hooks":{{{}}}}}"#, body.join(","))
    }

    fn report(dir: &Path) -> Option<CodexHookReport> {
        runtime().block_on(read_codex_hook_report(dir, EXPECTED))
    }

    #[test]
    fn recognizes_the_typescript_bridge_in_every_quoting() {
        for command in [
            STALE,
            "node /x/dist/cli.js hook",
            "/usr/bin/nodejs \"/x/dist/cli.js\" hook",
            "'/opt/node/bin/node' '--import' '/x/node_modules/tsx/dist/loader.mjs' '/x/src/cli.ts' 'hook'",
            "node --import=/x/loader.mjs --no-warnings /x/cli.js hook",
            "'/Users/o'\\''b/node' '/Users/o'\\''b/deckhand/dist/cli.js' 'hook'",
            "  node\t/x/cli.js   hook  ",
            "node /x/cli\\.js \"h\"ook",
        ] {
            assert!(is_stale_node_hook(command), "{command}");
        }
    }

    #[test]
    fn leaves_everything_else_alone() {
        for command in [
            EXPECTED,
            "deckhand hook",
            "node /x/dist/cli.js hooks",
            "node /x/dist/cli.js",
            "node /x/dist/other.js hook",
            "node /x/dist/cli.js hook extra",
            "node hook",
            "node --import /x/cli.js hook",
            "python /x/cli.js hook",
            "/x/node-wrapper /x/cli.js hook",
            "'/usr/bin/node' '/x/cli.js' 'hook",
            "node /x/cli.js hook\\",
            "",
        ] {
            assert!(!is_stale_node_hook(command), "{command}");
        }
    }

    #[test]
    fn stale_hooks_keep_their_order() {
        let commands: Vec<String> = ["a hook", STALE, EXPECTED, "node /y/cli.ts hook"]
            .iter()
            .map(|command| (*command).to_owned())
            .collect();
        assert_eq!(stale_node_hooks(&commands), [STALE, "node /y/cli.ts hook"]);
    }

    #[test]
    fn no_codex_home_means_no_report() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(report(&dir.path().join("missing")), None);
        let file = dir.path().join("file");
        std::fs::write(&file, "x").expect("write");
        assert_eq!(report(&file), None);
        let (status, text) = runtime().block_on(codex_hook_check(&file, EXPECTED, "deckhand hooks codex"));
        assert_eq!((status, text), (DoctorStatus::Healthy, None));
    }

    #[test]
    fn reports_stale_bridges_from_hooks_json_and_config_toml() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(CODEX_HOOKS_FILE), hooks_json(&[STALE, "echo hi"])).expect("write");
        std::fs::write(
            dir.path().join(CODEX_CONFIG_FILE),
            "[[hooks.Stop]]\ncommand = 'node /old/cli.js hook'\n",
        )
        .expect("write");
        let found = report(dir.path()).expect("report");
        assert_eq!(
            found,
            CodexHookReport {
                file: dir.path().join(CODEX_HOOKS_FILE),
                file_exists: true,
                commands: vec![STALE.into(), "echo hi".into(), "node /old/cli.js hook".into()],
                state: Some(CodexHookState::Other),
                stale: vec![STALE.into(), "node /old/cli.js hook".into()],
            }
        );
        assert!(codex_hook_report_has_problems(&found));
        let file = found.file.to_string_lossy().into_owned();
        assert_eq!(
            format_codex_hook_report(&found, "deckhand hooks codex"),
            format!(
                "Codex hooks ({file}):\n\
                 • Stale hook (the TypeScript bridge, node ... cli.js hook, which no longer exists): {STALE}\n\
                 • Stale hook (the TypeScript bridge, node ... cli.js hook, which no longer exists): node /old/cli.js hook\n\
                 • Codex hooks call another Deckhand install.\n\
                 Fix: merge the `hooks` object printed by `deckhand hooks codex` into {file}, replacing the stale \
                 hooks (and from config.toml, if any are there), then trust them with /hooks in Codex.\n"
            )
        );
        let (status, _) = runtime().block_on(codex_hook_check(dir.path(), EXPECTED, "deckhand hooks codex"));
        assert_eq!(status, DoctorStatus::Problems);
    }

    #[test]
    fn current_hooks_are_healthy() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(CODEX_HOOKS_FILE), hooks_json(&[EXPECTED, EXPECTED])).expect("write");
        let found = report(dir.path()).expect("report");
        assert_eq!(found.state, None);
        assert!(found.stale.is_empty());
        assert!(!codex_hook_report_has_problems(&found));
        assert_eq!(
            format_codex_hook_report(&found, "deckhand hooks codex"),
            format!("✓ Codex hooks call this Deckhand ({})\n", found.file.to_string_lossy())
        );
        let (status, text) = runtime().block_on(codex_hook_check(dir.path(), EXPECTED, "deckhand hooks codex"));
        assert_eq!(status, DoctorStatus::Healthy);
        assert!(text.is_some_and(|text| text.starts_with('✓')));
    }

    #[test]
    fn missing_hooks_file_says_to_save_the_printed_config() {
        let dir = tempfile::tempdir().expect("tempdir");
        let found = report(dir.path()).expect("report");
        assert!(!found.file_exists);
        assert_eq!(found.state, Some(CodexHookState::Missing));
        let file = found.file.to_string_lossy().into_owned();
        assert_eq!(
            format_codex_hook_report(&found, regenerate_command(true)),
            format!(
                "Codex hooks ({file}):\n\
                 • No Codex hook calls this Deckhand.\n\
                 Fix: save the output of `node scripts/deckhand-dev.mjs hooks codex` as {file}, then trust them with \
                 /hooks in Codex.\n"
            )
        );
        // Unreadable JSON counts as no hooks, but the file is there to merge into.
        std::fs::write(dir.path().join(CODEX_HOOKS_FILE), "{not json").expect("write");
        let found = report(dir.path()).expect("report");
        assert!(found.file_exists && found.commands.is_empty());
        assert!(format_codex_hook_report(&found, "deckhand hooks codex").contains("Fix: merge the `hooks` object"));
    }

    #[test]
    fn stale_entries_next_to_the_current_hook_only_need_removing() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join(CODEX_HOOKS_FILE),
            hooks_json(&[EXPECTED, "node /x/cli.js hook"]),
        )
        .expect("write");
        let found = report(dir.path()).expect("report");
        assert_eq!(found.state, None);
        let file = found.file.to_string_lossy().into_owned();
        assert_eq!(
            format_codex_hook_report(&found, "deckhand hooks codex"),
            format!(
                "Codex hooks ({file}):\n\
                 • Stale hook (the TypeScript bridge, node ... cli.js hook, which no longer exists): node /x/cli.js hook\n\
                 Fix: remove the stale hooks from {file} (and from config.toml, if any are there), then trust them \
                 with /hooks in Codex.\n"
            )
        );
    }

    #[test]
    fn regenerate_command_follows_the_channel() {
        assert_eq!(regenerate_command(false), "deckhand hooks codex");
        assert_eq!(regenerate_command(true), "node scripts/deckhand-dev.mjs hooks codex");
    }
}
