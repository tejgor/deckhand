//! Agent signals: the process and PATH lookups (the pure parts are in deckhand_core::agent_signals).
//!
//! What launches pass an agent so it reports its lifecycle to Deckhand: the `deckhand hook` command line (this
//! executable), the hook settings built on it (`deckhand_core::agent_signals::hook_settings_for`), and the
//! integration arguments, which depend on what the installed agent's `--help` offers (probed once per command, only
//! successful probes cached). Normalising hook payloads is pure and lives in `deckhand_core::agent_signals`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::FutureExt;
use futures::future::{LocalBoxFuture, Shared};

use deckhand_core::agent_signals::{HookSettings, hook_settings_for, shell_quote};
use deckhand_core::error::{DhError, Result};
use deckhand_protocol::data::ProgramKey;
use serde_json::Value;

use crate::paths;
use crate::proc::{self, ExecOptions};

/// TS: agentSignals.ts programHelp timeout (`<command> --help`).
pub const HELP_TIMEOUT: Duration = Duration::from_millis(5000);
/// TS: agentSignals.ts programHelp maxBuffer.
pub const HELP_MAX_BUFFER: usize = 512 * 1024;

/// The executable name looked up on `$PATH` (as `deckhand hooks` does).
const EXECUTABLE_NAME: &str = "deckhand";
/// TS: integrationArgs isolated-channel refusal.
const NO_DAEMON_MISSING: &str = "This Codex version cannot verify isolated native-daemon operation (--no-daemon missing). Upgrade Codex or test with Claude/Pi.";

/// The first `$PATH` entry holding an executable regular file named `name` (symlinks followed; empty entries
/// skipped), like `command -v`.
fn find_on_path(name: &str, path_env: Option<&OsStr>) -> Option<PathBuf> {
    let path_env = path_env?;
    std::env::split_paths(path_env)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(name))
        .find(|candidate| {
            std::fs::metadata(candidate).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

/// Same device and inode (symlinks followed); `false` when either is missing.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// The executable hooks call. TS runs `node <cli entry>`; the Rust binary is its own entry. Like `deckhand hooks`,
/// it names the `deckhand` on `$PATH` when that is this very executable (so the command survives rebuilds and
/// reinstalls the same way the printed Codex config does, and Settings compares like with like), else the running
/// executable itself.
fn hook_executable_from(current_exe: &Path, path_env: Option<&OsStr>) -> PathBuf {
    match find_on_path(EXECUTABLE_NAME, path_env) {
        Some(found) if same_file(&found, current_exe) => found,
        _ => current_exe.to_path_buf(),
    }
}

/// `[...parts, 'hook'].map(shellQuote).join(' ')`.
fn hook_command_from(parts: &[&str]) -> String {
    parts
        .iter()
        .copied()
        .chain(std::iter::once("hook"))
        .map(shell_quote)
        .collect::<Vec<_>>()
        .join(" ")
}

/// TS: agentSignals.ts hookCommand: `'<this executable>' 'hook'`, each part shell-quoted (TS: node, the tsx loader
/// in dev, the CLI entry, `hook`).
pub fn hook_command() -> Result<String> {
    let exe = paths::cli_entry_path()?;
    let executable = hook_executable_from(&exe, std::env::var_os("PATH").as_deref());
    Ok(hook_command_from(&[&executable.to_string_lossy()]))
}

/// TS: agentSignals.ts hookSettings (every supported event calls [`hook_command`]; `async_`: Claude's background
/// hooks).
pub fn hook_settings(async_: bool) -> Result<HookSettings> {
    Ok(hook_settings_for(&hook_command()?, async_))
}

type HelpProbe = Shared<LocalBoxFuture<'static, String>>;

thread_local! {
    /// Help texts per command, with an id per probe so a failed probe only removes itself.
    static HELP_CACHE: RefCell<HashMap<String, (u64, HelpProbe)>> = RefCell::new(HashMap::new());
    static NEXT_PROBE: RefCell<u64> = const { RefCell::new(0) };
}

/// TS: agentSignals.ts programHelp (`<command> --help` stdout, bounded; empty on failure; successes cached per
/// command for the process's life, concurrent probes shared).
pub async fn program_help(command: &str) -> String {
    let cached = HELP_CACHE.with(|cache| cache.borrow().get(command).map(|(_, probe)| probe.clone()));
    if let Some(probe) = cached {
        return probe.await;
    }
    let id = NEXT_PROBE.with(|next| {
        let mut next = next.borrow_mut();
        *next += 1;
        *next
    });
    let key = command.to_owned();
    let probe: HelpProbe = async move {
        let options = ExecOptions::default().timeout(HELP_TIMEOUT).max_buffer(HELP_MAX_BUFFER);
        match proc::exec(&key, &["--help"], &options).await {
            Ok(output) => output.stdout,
            Err(_) => {
                // Only successful probes are cached; a transient failure (timeout, upgrade in progress) is retried
                // next launch.
                HELP_CACHE.with(|cache| {
                    let mut cache = cache.borrow_mut();
                    if cache.get(&key).is_some_and(|(cached, _)| *cached == id) {
                        cache.remove(&key);
                    }
                });
                String::new()
            }
        }
    }
    .boxed_local()
    .shared();
    HELP_CACHE.with(|cache| cache.borrow_mut().insert(command.to_owned(), (id, probe.clone())));
    probe.await
}

/// TS: agentSignals.ts integrationArgs: Claude with `--settings` gets the hook settings (without StopFailure and
/// Interrupt); Codex gets `--no-daemon` when it has it, which the isolated dev channel requires (`This Codex version
/// cannot verify isolated native-daemon operation (--no-daemon missing). Upgrade Codex or test with Claude/Pi.`).
/// Not in the TS: with `scroll_override` (`claude_scroll_override`, hooks on or off), Claude's settings also fix its
/// scroll speed at one line per wheel event (`env` `CLAUDE_CODE_SCROLL_SPEED`) and turn its wheel acceleration off,
/// so each wheel event Deckhand forwards scrolls it the same distance. The speed goes in `--settings` because Claude
/// applies its settings files' `env` over its environment, and `/scroll-speed` saves to `~/.claude/settings.json`'s;
/// `--settings` comes after those (only managed policy overrides it). The worker's `agent_env` sets it too, for a
/// Claude without `--settings`.
pub async fn integration_args(
    program: ProgramKey,
    command: &str,
    enabled: bool,
    scroll_override: bool,
) -> Result<Vec<String>> {
    integration_args_with(
        program,
        command,
        enabled,
        scroll_override,
        paths::is_dev_channel(),
        hook_command,
    )
    .await
}

/// Claude's scroll speed with `claude_scroll_override`: one line per wheel event.
pub const CLAUDE_SCROLL_SPEED: &str = "1";

/// [`integration_args`] with the channel and the hook command given.
async fn integration_args_with(
    program: ProgramKey,
    command: &str,
    enabled: bool,
    scroll_override: bool,
    dev_channel: bool,
    hook_command: impl FnOnce() -> Result<String>,
) -> Result<Vec<String>> {
    let isolated_codex = program == ProgramKey::Codex && dev_channel;
    let claude_scroll = program == ProgramKey::Claude && scroll_override;
    if !enabled && !isolated_codex && !claude_scroll {
        return Ok(Vec::new());
    }
    let help = program_help(command).await;
    if program == ProgramKey::Codex {
        let has_no_daemon = help.contains("--no-daemon");
        if isolated_codex && !has_no_daemon {
            return Err(DhError::msg(NO_DAEMON_MISSING));
        }
        return Ok(if has_no_daemon {
            vec!["--no-daemon".to_owned()]
        } else {
            Vec::new()
        });
    }
    if program != ProgramKey::Claude || !help.contains("--settings") {
        return Ok(Vec::new());
    }
    // Additional settings only; never alter permissions or replace the native TUI.
    let mut settings = serde_json::Map::new();
    if enabled {
        let mut hooks = hook_settings_for(&hook_command()?, true);
        hooks.hooks.shift_remove("StopFailure"); // Opt-in manual configuration for version-specific failure events.
        hooks.hooks.shift_remove("Interrupt"); // Claude doesn't expose Codex's Interrupt event.
        settings.insert("hooks".to_owned(), serde_json::to_value(&hooks.hooks)?);
    }
    if claude_scroll {
        settings.insert(
            "env".to_owned(),
            serde_json::json!({ "CLAUDE_CODE_SCROLL_SPEED": CLAUDE_SCROLL_SPEED }),
        );
        settings.insert("wheelScrollAccelerationEnabled".to_owned(), Value::Bool(false));
    }
    Ok(vec!["--settings".to_owned(), serde_json::to_string(&settings)?])
}

#[cfg(test)]
mod tests {
    use super::*;
    use deckhand_testkit::{DirOp, replay, replay_dir};
    use serde_json::Value;

    /// TS records node's path as `$EXEC` and the CLI entry as `$CLI` (`$TSX`: the dev loader); the Rust binary is
    /// both, so the replay builds the command from those placeholders.
    const PROD_PARTS: [&str; 2] = ["$EXEC", "$CLI"];
    const DEV_PARTS: [&str; 4] = ["$EXEC", "--import", "$TSX", "$CLI"];

    /// JS truthiness of `options.async` (undefined options: no).
    fn async_option(case: &deckhand_testkit::Case) -> bool {
        match case.arg(0).get("async") {
            None | Some(Value::Null) | Some(Value::Bool(false)) => false,
            Some(Value::Number(n)) => n.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
            Some(Value::String(text)) => !text.is_empty(),
            Some(Value::Object(map)) => !map.contains_key("$u"),
            Some(_) => true,
        }
    }

    #[test]
    fn hook_command_parity() {
        replay("agentSignals-I7/hookCommand", |_| -> Result<String> {
            Ok(hook_command_from(&PROD_PARTS))
        });
        replay("agentSignals-I7/hookCommandDev", |_| -> Result<String> {
            Ok(hook_command_from(&DEV_PARTS))
        });
    }

    #[test]
    fn hook_settings_parity() {
        replay("agentSignals-I7/hookSettings", |case| -> Result<HookSettings> {
            Ok(hook_settings_for(&hook_command_from(&PROD_PARTS), async_option(case)))
        });
        replay("agentSignals-I7/hookSettingsDev", |case| -> Result<HookSettings> {
            Ok(hook_settings_for(&hook_command_from(&DEV_PARTS), async_option(case)))
        });
    }

    fn run_integration_case(op: &DirOp) -> std::result::Result<Vec<String>, String> {
        // `other` stands for a program that is neither Claude nor Codex; ProgramKey cannot hold it, and Pi takes the
        // same path (probe when enabled, no arguments).
        let program: ProgramKey = match op.arg_str(0)? {
            "other" => ProgramKey::Pi,
            _ => op.arg_as(0)?,
        };
        let command = op.arg_str(1)?.to_owned();
        let enabled = op.arg_bool(2)?;
        let dev_channel = op.fn_name == "integrationArgsDevChannel";
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        runtime
            .block_on(integration_args_with(
                program,
                &command,
                enabled,
                false,
                dev_channel,
                || Ok(hook_command_from(&PROD_PARTS)),
            ))
            .map_err(|error| error.to_string())
    }

    #[test]
    fn integration_args_parity() {
        let cases = deckhand_testkit::dir_cases("agentSignals-I7");
        assert!(!cases.is_empty());
        for fixture in cases {
            replay_dir(&fixture, |_root, op| run_integration_case(op));
        }
    }

    #[test]
    fn hook_command_resolves_the_path_deckhand_only_when_it_is_this_executable() {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let exe = temp.path().join("real-deckhand");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path_env = std::env::join_paths([Path::new(""), &bin]).unwrap();
        // Nothing on PATH: the running executable.
        assert_eq!(hook_executable_from(&exe, Some(&path_env)), exe);
        assert_eq!(hook_executable_from(&exe, None), exe);
        // A different deckhand on PATH: still the running executable.
        let other = bin.join("deckhand");
        std::fs::write(&other, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(hook_executable_from(&exe, Some(&path_env)), exe);
        // A symlink to this executable: the PATH name.
        std::fs::remove_file(&other).unwrap();
        std::os::unix::fs::symlink(&exe, &other).unwrap();
        assert_eq!(hook_executable_from(&exe, Some(&path_env)), other);
        assert_eq!(
            hook_command_from(&[&other.to_string_lossy()]),
            format!("'{}' 'hook'", other.display())
        );
        assert_eq!(hook_command_from(&["it's"]), r"'it'\''s' 'hook'");
    }

    #[test]
    fn dev_codex_requires_native_daemon_isolation_even_when_hooks_are_off() {
        let directory = tempfile::tempdir().unwrap();
        let supported = directory.path().join("supported");
        let unsupported = directory.path().join("unsupported");
        for (file, body) in [(&supported, "resume --no-daemon"), (&unsupported, "resume")] {
            std::fs::write(file, format!("#!/bin/sh\nprintf \"{body}\\n\"\n")).unwrap();
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let home = tempfile::tempdir().unwrap();
        let _env = crate::storage::tests::Home::set(home.path());
        let previous = std::env::var_os(paths::CHANNEL_ENV);
        // SAFETY: the Home guard holds the test environment lock.
        unsafe { std::env::set_var(paths::CHANNEL_ENV, "dev") };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (ok, err) = runtime.block_on(async {
            (
                integration_args(ProgramKey::Codex, &supported.to_string_lossy(), false, true).await,
                integration_args(ProgramKey::Codex, &unsupported.to_string_lossy(), false, true).await,
            )
        });
        // SAFETY: as above.
        unsafe {
            match previous {
                Some(value) => std::env::set_var(paths::CHANNEL_ENV, value),
                None => std::env::remove_var(paths::CHANNEL_ENV),
            }
        }
        assert_eq!(ok.unwrap(), vec!["--no-daemon".to_owned()]);
        assert!(err.unwrap_err().to_string().contains("--no-daemon missing"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn claude_scroll_override_turns_acceleration_off_with_or_without_hooks() {
        let temp = tempfile::tempdir().unwrap();
        let claude = temp.path().join("claude");
        std::fs::write(&claude, "#!/bin/sh\nprintf -- '--settings <file-or-json>\\n'\n").unwrap();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
        let command = claude.to_string_lossy().into_owned();
        let args = |program, hooks, scroll| {
            integration_args_with(program, &command, hooks, scroll, false, || Ok("hook".to_owned()))
        };
        let settings = |args: Vec<String>| -> Value {
            assert_eq!(args.len(), 2, "{args:?}");
            assert_eq!(args[0], "--settings");
            serde_json::from_str(&args[1]).unwrap()
        };
        // Hooks off: the scroll settings alone.
        assert_eq!(
            settings(args(ProgramKey::Claude, false, true).await.unwrap()),
            serde_json::json!({
                "env": {"CLAUDE_CODE_SCROLL_SPEED": "1"},
                "wheelScrollAccelerationEnabled": false
            })
        );
        // Hooks on: the hooks first, then the scroll settings.
        let both = settings(args(ProgramKey::Claude, true, true).await.unwrap());
        let keys: Vec<&String> = both.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["hooks", "env", "wheelScrollAccelerationEnabled"]);
        assert!(both["hooks"]["Stop"].is_array());
        // Opted out: as before.
        assert!(args(ProgramKey::Claude, false, false).await.unwrap().is_empty());
        assert!(
            settings(args(ProgramKey::Claude, true, false).await.unwrap())
                .get("wheelScrollAccelerationEnabled")
                .is_none()
        );
        // Other agents never get it.
        assert!(args(ProgramKey::Pi, false, true).await.unwrap().is_empty());
        assert!(args(ProgramKey::Codex, false, true).await.unwrap().is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn program_help_caches_only_successes() {
        let temp = tempfile::tempdir().unwrap();
        let agent = temp.path().join("agent");
        let write = |body: &str| {
            std::fs::write(&agent, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        let command = agent.to_string_lossy().into_owned();
        write("exit 1");
        assert_eq!(program_help(&command).await, "");
        write("echo first");
        assert_eq!(program_help(&command).await, "first\n");
        write("echo second");
        assert_eq!(program_help(&command).await, "first\n");
        let missing = temp.path().join("missing").to_string_lossy().into_owned();
        let (a, b) = tokio::join!(program_help(&missing), program_help(&missing));
        assert_eq!((a.as_str(), b.as_str()), ("", ""));
    }
}
