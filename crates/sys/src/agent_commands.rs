//! What the agents Deckhand launches get for the `deckhand` commands (not in the TS): the skill, the always-loaded
//! trigger and the pre-allowed commands, as launch arguments that depend on what the installed agent's `--help` offers.
//!
//! - **Claude** loads skills lazily (only the name and description stay in context until used), so it gets a plugin
//!   directory carrying the skill (`--plugin-dir`); without that flag, the short trigger (`--append-system-prompt`).
//!   With `allow_deckhand_commands` (unless false) also `--allowedTools` [`DECKHAND_COMMANDS_RULE`].
//! - **Pi** gets the same skill directory (`--skill`); without it, the trigger (`--append-system-prompt`). Pi never
//!   asks before running a command, so there is nothing to pre-allow.
//! - **Codex** has no skill or allowlist flag: the trigger goes in as `developer_instructions` (`-c`), after the
//!   user's own `developer_instructions` from `$CODEX_HOME/config.toml`, which the override would otherwise replace.
//!   Its sandbox keeps a shell command from reaching the daemon's socket (Seatbelt counts an AF_UNIX connect as
//!   network), so a Codex new enough ([`CODEX_MCP_MIN_VERSION`]) also gets the `deckhand` MCP server (`deckhand
//!   mcp`, one `-c mcp_servers.deckhand={…}`): Codex starts it as its own child, outside the sandbox, and its tool
//!   runs the same commands. Its trigger is then [`CODEX_TRIGGER`]. The server's environment comes from Codex's own
//!   through `env_vars` (names only, so the hook token never appears in an argument); with
//!   `allow_deckhand_commands` (unless false) the tool runs without asking (`default_tools_approval_mode`).
//!
//! - **The orchestrator** (a Claude session with role `orchestrator`) also gets its guide, always loaded
//!   ([`ORCHESTRATOR_GUIDE`], `--append-system-prompt`, after the trigger when that goes there too: Claude takes the
//!   flag once), and the file-editing tools denied ([`ORCHESTRATOR_DISALLOWED_TOOLS`], `--disallowedTools`), so it
//!   plans and dispatches but cannot implement.
//!
//! The plugin lives in the state directory (`<DECKHAND_HOME>/agent-plugin`) and is rewritten only when its content
//! differs from this build's.

use std::path::{Path, PathBuf};

use deckhand_core::agent_guide::{CODEX_TRIGGER, GUIDE, SKILL_DESCRIPTION, SKILL_NAME, TRIGGER};
use deckhand_core::agent_versions::compare_versions;
use deckhand_core::error::{DhError, Result};
use deckhand_core::orchestrator_guide::ORCHESTRATOR_GUIDE;
use deckhand_protocol::data::{ProgramKey, SessionRole};

use crate::agent_path::program_help;
use crate::paths;
use crate::storage::write_atomic;

/// The plugin's directory in the state directory.
pub const PLUGIN_DIR_NAME: &str = "agent-plugin";

/// The permission rule that lets Claude run `deckhand` commands without asking.
pub const DECKHAND_COMMANDS_RULE: &str = "Bash(deckhand:*)";

/// The tools an orchestrator may not use: it plans and dispatches, the sessions it starts implement.
pub const ORCHESTRATOR_DISALLOWED_TOOLS: [&str; 3] = ["Edit", "Write", "NotebookEdit"];

/// The name of Deckhand's MCP server in Codex's `mcp_servers` (its tool shows as `deckhand` from server `deckhand`).
pub const CODEX_MCP_SERVER: &str = "deckhand";

/// The first Codex given the MCP server: older ones lack `default_tools_approval_mode` (and may not know
/// `env_vars`), so their tool would ask every time or not reach the right session. Older Codex keeps the trigger only.
pub const CODEX_MCP_MIN_VERSION: &str = "0.122.0";

/// What the MCP server takes from Codex's environment (Codex passes a server only `HOME`, `PATH`, `TMPDIR` and a few
/// others by default): the caller, as for hooks, and which Deckhand home and channel to reach.
pub const CODEX_MCP_ENV_VARS: [&str; 6] = [
    "DECKHAND_SESSION_ID",
    "DECKHAND_LAUNCH_ID",
    "DECKHAND_HOOK_TOKEN",
    paths::HOME_ENV,
    paths::CHANNEL_ENV,
    paths::ALLOW_PROD_HOME_ENV,
];

/// Codex's timeout for one call of the tool (s): above the server's own limit on one command
/// (`deckhand mcp`, 60 s), so the server answers first.
pub const CODEX_MCP_TOOL_TIMEOUT_SECS: u64 = 90;

/// The plugin's description (Claude lists it with the plugin; it is not loaded into the conversation).
const PLUGIN_DESCRIPTION: &str = "The deckhand commands, for agents running in a Deckhand session.";

/// `<config dir>/agent-plugin`.
pub fn plugin_dir(config_dir: &Path) -> PathBuf {
    config_dir.join(PLUGIN_DIR_NAME)
}

/// The skill's directory inside the plugin (`skills/deckhand`), which Pi loads by itself.
pub fn skill_dir(plugin_dir: &Path) -> PathBuf {
    plugin_dir.join("skills").join(SKILL_NAME)
}

/// `SKILL.md`: the frontmatter Claude and Pi keep in context (name, description), then the guide.
pub fn skill_markdown() -> String {
    // A JSON string is a YAML double-quoted scalar, so the description's colons cannot break the frontmatter.
    let description = serde_json::to_string(SKILL_DESCRIPTION).unwrap_or_default();
    format!("---\nname: {SKILL_NAME}\ndescription: {description}\n---\n\n{GUIDE}")
}

/// `.claude-plugin/plugin.json`.
pub fn plugin_manifest() -> String {
    let manifest = serde_json::json!({
        "name": SKILL_NAME,
        "version": deckhand_protocol::BUILD_VERSION,
        "description": PLUGIN_DESCRIPTION,
        "author": {"name": "Deckhand"},
    });
    format!("{}\n", serde_json::to_string_pretty(&manifest).unwrap_or_default())
}

/// The plugin's files, relative to its directory.
pub fn plugin_files() -> [(PathBuf, String); 2] {
    [
        (Path::new(".claude-plugin").join("plugin.json"), plugin_manifest()),
        (Path::new("skills").join(SKILL_NAME).join("SKILL.md"), skill_markdown()),
    ]
}

/// Writes the plugin into `dir`, each file only when it differs from this build's.
pub async fn write_plugin(dir: &Path) -> Result<()> {
    for (relative, contents) in plugin_files() {
        let path = dir.join(relative);
        if tokio::fs::read_to_string(&path).await.ok().as_deref() == Some(contents.as_str()) {
            continue;
        }
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| DhError::io(&error, "mkdir", &parent.to_string_lossy()))?;
        }
        write_atomic(&path, contents.as_bytes(), 0o644).await?;
    }
    Ok(())
}

/// `<plugin dir>/bin/deckhand`: the link [`link_executable`] makes, which Codex starts as the MCP server.
pub fn linked_executable(plugin_dir: &Path) -> PathBuf {
    plugin_dir.join("bin").join("deckhand")
}

/// `<plugin dir>/bin`, holding `deckhand` as a link to `exe` (replaced when it points elsewhere). Put first on an
/// agent's `PATH`, the `deckhand` commands it runs are this build's, whichever `deckhand` the `PATH` names (a dev
/// build's agents would otherwise run the installed one).
pub fn link_executable(plugin_dir: &Path, exe: &Path) -> Result<PathBuf> {
    let link = linked_executable(plugin_dir);
    let bin = plugin_dir.join("bin");
    if std::fs::read_link(&link).is_ok_and(|target| target == exe) {
        return Ok(bin);
    }
    std::fs::create_dir_all(&bin).map_err(|error| DhError::io(&error, "mkdir", &bin.to_string_lossy()))?;
    // A link beside it, renamed over the old one, so a launch never sees none.
    let temporary = bin.join(format!(".deckhand-{}", std::process::id()));
    let _ = std::fs::remove_file(&temporary);
    std::os::unix::fs::symlink(exe, &temporary)
        .map_err(|error| DhError::io(&error, "symlink", &temporary.to_string_lossy()))?;
    std::fs::rename(&temporary, &link).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        DhError::io(&error, "rename", &link.to_string_lossy())
    })?;
    Ok(bin)
}

/// `bin` first on `path` (the daemon's `PATH`; none: `bin` alone).
pub fn path_with(bin: &Path, path: Option<&std::ffi::OsStr>) -> String {
    let bin = bin.to_string_lossy();
    match path.map(|path| path.to_string_lossy()).filter(|path| !path.is_empty()) {
        Some(path) => format!("{bin}:{path}"),
        None => bin.into_owned(),
    }
}

/// The top-level `developer_instructions` of a Codex `config.toml` (none when unreadable as TOML or empty).
pub fn developer_instructions_in(config_toml: &str) -> Option<String> {
    let table: toml::Table = config_toml.parse().ok()?;
    table
        .get("developer_instructions")
        .and_then(toml::Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .map(str::to_owned)
}

/// The user's own Codex `developer_instructions` (`$CODEX_HOME/config.toml`, else `~/.codex/config.toml`).
async fn codex_user_instructions() -> Option<String> {
    let codex_home = std::env::var_os("CODEX_HOME")
        .filter(|home| !home.is_empty())
        .map_or_else(|| paths::home_dir().join(".codex"), PathBuf::from);
    let text = tokio::fs::read_to_string(codex_home.join("config.toml")).await.ok()?;
    developer_instructions_in(&text)
}

/// A TOML basic string (`-c key=<value>` parses the value as TOML): JSON's escapes are all valid TOML ones.
fn toml_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

/// Whether a Codex of `version` gets the MCP server (unknown: no, as for an old one).
pub fn codex_takes_mcp(version: Option<&str>) -> bool {
    version.is_some_and(|version| compare_versions(version, CODEX_MCP_MIN_VERSION) >= 0)
}

/// The `-c` value adding the MCP server: `mcp_servers.deckhand={command=…, args=["mcp"], env_vars=[…], …}`, one TOML
/// inline table. Codex merges a dotted override into its config table by table, so the user's own servers stay.
pub fn codex_mcp_override(deckhand: &Path, allow_commands: bool) -> String {
    let env_vars = CODEX_MCP_ENV_VARS.map(toml_string).join(", ");
    let approval = if allow_commands {
        ", default_tools_approval_mode=\"approve\""
    } else {
        ""
    };
    format!(
        "mcp_servers.{CODEX_MCP_SERVER}={{command={}, args=[\"mcp\"], env_vars=[{env_vars}], \
         tool_timeout_sec={CODEX_MCP_TOOL_TIMEOUT_SECS}{approval}}}",
        toml_string(&deckhand.to_string_lossy()),
    )
}

/// What the launch arguments depend on besides the agent (see [`command_args_for`]).
#[derive(Clone, Copy, Debug, Default)]
pub struct CommandInputs<'a> {
    /// The installed agent's `--help` text.
    pub help: &'a str,
    /// The plugin directory, when it could be written.
    pub plugin: Option<&'a Path>,
    /// Pre-allow the commands (`allow_deckhand_commands` unless false).
    pub allow_commands: bool,
    /// The user's own Codex `developer_instructions`.
    pub codex_instructions: Option<&'a str>,
    /// The installed Codex's version.
    pub codex_version: Option<&'a str>,
    /// The `deckhand` Codex starts as its MCP server (none: no server).
    pub deckhand: Option<&'a Path>,
    /// The session's role (the orchestrator gets its guide and no file-editing tools).
    pub role: Option<SessionRole>,
}

/// The launch arguments that tell an agent about the `deckhand` commands (see the module docs). `plugin`: the
/// plugin directory, when it could be written; `allow_commands`: pre-allow them; `deckhand`: the executable Codex
/// starts as its MCP server; `version`: the agent's version; `role`: the session's.
pub async fn command_args(
    program: ProgramKey,
    command: &str,
    plugin: Option<&Path>,
    allow_commands: bool,
    deckhand: Option<&Path>,
    version: Option<&str>,
    role: Option<SessionRole>,
) -> Vec<String> {
    let help = program_help(command).await;
    let codex_instructions = if program == ProgramKey::Codex {
        codex_user_instructions().await
    } else {
        None
    };
    command_args_for(
        program,
        &CommandInputs {
            help: &help,
            plugin,
            allow_commands,
            codex_instructions: codex_instructions.as_deref(),
            codex_version: version,
            deckhand,
            role,
        },
    )
}

/// [`command_args`] with the agent's `--help` text, the user's Codex instructions and its version given.
pub fn command_args_for(program: ProgramKey, inputs: &CommandInputs) -> Vec<String> {
    let CommandInputs {
        help,
        plugin,
        allow_commands,
        codex_instructions,
        codex_version,
        deckhand,
        role,
    } = *inputs;
    let mut args = Vec::new();
    match program {
        ProgramKey::Claude => {
            let orchestrator = role == Some(SessionRole::Orchestrator);
            // `--allowedTools` and `--disallowedTools` take several values: the next option ends them, so they go
            // first.
            if allow_commands && help.contains("--allowedTools") {
                args.extend(["--allowedTools".to_owned(), DECKHAND_COMMANDS_RULE.to_owned()]);
            }
            if orchestrator && help.contains("--disallowedTools") {
                args.push("--disallowedTools".to_owned());
                args.extend(ORCHESTRATOR_DISALLOWED_TOOLS.map(str::to_owned));
            }
            // Claude takes `--append-system-prompt` once: the trigger (without the plugin) and the orchestrator's
            // guide share it.
            let mut appended = Vec::new();
            match plugin {
                Some(plugin) if help.contains("--plugin-dir") => {
                    args.extend(["--plugin-dir".to_owned(), plugin.to_string_lossy().into_owned()]);
                }
                _ => appended.push(TRIGGER),
            }
            if orchestrator {
                appended.push(ORCHESTRATOR_GUIDE.trim_end());
            }
            if help.contains("--append-system-prompt") && !appended.is_empty() {
                args.extend(["--append-system-prompt".to_owned(), appended.join("\n\n")]);
            }
        }
        ProgramKey::Pi => match plugin {
            Some(plugin) if help.contains("--skill") => {
                args.extend(["--skill".to_owned(), skill_dir(plugin).to_string_lossy().into_owned()]);
            }
            _ if help.contains("--append-system-prompt") => {
                args.extend(["--append-system-prompt".to_owned(), TRIGGER.to_owned()]);
            }
            _ => {}
        },
        ProgramKey::Codex => {
            if help.contains("--config") {
                let mcp = deckhand.filter(|_| codex_takes_mcp(codex_version));
                let trigger = if mcp.is_some() { CODEX_TRIGGER } else { TRIGGER };
                let instructions = match codex_instructions {
                    Some(own) => format!("{own}\n\n{trigger}"),
                    None => trigger.to_owned(),
                };
                args.extend([
                    "-c".to_owned(),
                    format!("developer_instructions={}", toml_string(&instructions)),
                ]);
                if let Some(deckhand) = mcp {
                    args.extend(["-c".to_owned(), codex_mcp_override(deckhand, allow_commands)]);
                }
            }
        }
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE_HELP: &str = "  --allowedTools, --allowed-tools <tools...>\n  --append-system-prompt <prompt>\n  \
        --plugin-dir <path>\n  --settings <file-or-json>\n";
    const PI_HELP: &str = "  --append-system-prompt <text>\n  --skill <path>\n";
    const CODEX_HELP: &str = "  -c, --config <key=value>\n";

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    /// The inputs with only the help text and the plugin given.
    fn inputs<'a>(help: &'a str, plugin: Option<&'a Path>, allow_commands: bool) -> CommandInputs<'a> {
        CommandInputs {
            help,
            plugin,
            allow_commands,
            ..CommandInputs::default()
        }
    }

    #[test]
    fn claude_gets_the_plugin_and_the_pre_allowed_commands() {
        let plugin = Path::new("/h/agent-plugin");
        let claude = |inputs: CommandInputs| command_args_for(ProgramKey::Claude, &inputs);
        assert_eq!(
            claude(inputs(CLAUDE_HELP, Some(plugin), true)),
            strings(&["--allowedTools", "Bash(deckhand:*)", "--plugin-dir", "/h/agent-plugin"])
        );
        // Turned off: the skill alone.
        assert_eq!(
            claude(inputs(CLAUDE_HELP, Some(plugin), false)),
            strings(&["--plugin-dir", "/h/agent-plugin"])
        );
        // No plugin (unwritable) or no --plugin-dir: the short trigger instead.
        assert_eq!(
            claude(inputs(CLAUDE_HELP, None, false)),
            strings(&["--append-system-prompt", TRIGGER])
        );
        assert_eq!(
            claude(inputs("--allowedTools --append-system-prompt", Some(plugin), true)),
            strings(&["--allowedTools", "Bash(deckhand:*)", "--append-system-prompt", TRIGGER])
        );
        // An agent whose help offers none of them (a fake agent, a failed probe): nothing.
        assert!(claude(inputs("", Some(plugin), true)).is_empty());
        // The MCP server is Codex's alone.
        let with_mcp = CommandInputs {
            codex_version: Some("0.163.0"),
            deckhand: Some(Path::new("/h/agent-plugin/bin/deckhand")),
            ..inputs(CLAUDE_HELP, Some(plugin), true)
        };
        assert_eq!(claude(with_mcp), claude(inputs(CLAUDE_HELP, Some(plugin), true)));
    }

    #[test]
    fn the_orchestrator_gets_its_guide_and_no_editing_tools() {
        let plugin = Path::new("/h/agent-plugin");
        let orchestrator = |help: &str, plugin: Option<&Path>| {
            command_args_for(
                ProgramKey::Claude,
                &CommandInputs {
                    role: Some(SessionRole::Orchestrator),
                    ..inputs(help, plugin, true)
                },
            )
        };
        let help = format!("{CLAUDE_HELP}  --disallowedTools, --disallowed-tools <tools...>\n");
        let guide = ORCHESTRATOR_GUIDE.trim_end();
        assert_eq!(
            orchestrator(&help, Some(plugin)),
            strings(&[
                "--allowedTools",
                "Bash(deckhand:*)",
                "--disallowedTools",
                "Edit",
                "Write",
                "NotebookEdit",
                "--plugin-dir",
                "/h/agent-plugin",
                "--append-system-prompt",
                guide,
            ])
        );
        // Without the plugin the trigger goes in the same --append-system-prompt, first.
        let combined = format!("{TRIGGER}\n\n{guide}");
        assert_eq!(
            orchestrator(&help, None),
            strings(&[
                "--allowedTools",
                "Bash(deckhand:*)",
                "--disallowedTools",
                "Edit",
                "Write",
                "NotebookEdit",
                "--append-system-prompt",
                &combined,
            ])
        );
        // Each flag only when the installed Claude lists it.
        assert_eq!(
            orchestrator(CLAUDE_HELP, Some(plugin)),
            strings(&[
                "--allowedTools",
                "Bash(deckhand:*)",
                "--plugin-dir",
                "/h/agent-plugin",
                "--append-system-prompt",
                guide,
            ])
        );
        assert_eq!(
            orchestrator("--disallowedTools --plugin-dir", Some(plugin)),
            strings(&[
                "--disallowedTools",
                "Edit",
                "Write",
                "NotebookEdit",
                "--plugin-dir",
                "/h/agent-plugin"
            ])
        );
        assert!(orchestrator("", Some(plugin)).is_empty());
        // Any other session gets neither.
        let plain = command_args_for(ProgramKey::Claude, &inputs(&help, Some(plugin), true));
        assert!(
            !plain
                .iter()
                .any(|arg| arg == "--disallowedTools" || arg == "--append-system-prompt")
        );
    }

    #[test]
    fn pi_gets_the_skill_directory_and_nothing_to_allow() {
        let plugin = Path::new("/h/agent-plugin");
        let pi = |inputs: CommandInputs| command_args_for(ProgramKey::Pi, &inputs);
        let skill = ["--skill", "/h/agent-plugin/skills/deckhand"];
        assert_eq!(pi(inputs(PI_HELP, Some(plugin), true)), strings(&skill));
        assert_eq!(pi(inputs(PI_HELP, Some(plugin), false)), strings(&skill));
        assert_eq!(
            pi(inputs(PI_HELP, None, true)),
            strings(&["--append-system-prompt", TRIGGER])
        );
        assert!(pi(inputs("", Some(plugin), true)).is_empty());
    }

    /// The value of `-c <key>=<value>` as Codex reads it: a TOML value.
    fn override_value(assignment: &str, key: &str) -> toml::Value {
        let value = assignment
            .strip_prefix(key)
            .and_then(|rest| rest.strip_prefix('='))
            .unwrap_or_else(|| panic!("{assignment}"));
        let parsed: toml::Table = format!("v = {value}").parse().unwrap();
        parsed["v"].clone()
    }

    /// A Codex's `-c` values, by key.
    fn codex_overrides(args: &[String]) -> Vec<(String, toml::Value)> {
        assert_eq!(args.len() % 2, 0, "{args:?}");
        args.chunks(2)
            .map(|pair| {
                assert_eq!(pair[0], "-c", "{args:?}");
                let key = pair[1].split('=').next().unwrap().to_owned();
                (key.clone(), override_value(&pair[1], &key))
            })
            .collect()
    }

    const DECKHAND: &str = "/h/agent-plugin/bin/deckhand";

    fn codex_inputs(version: Option<&str>, allow_commands: bool) -> CommandInputs<'_> {
        CommandInputs {
            help: CODEX_HELP,
            allow_commands,
            codex_version: version,
            deckhand: Some(Path::new(DECKHAND)),
            ..CommandInputs::default()
        }
    }

    #[test]
    fn an_old_codex_gets_the_trigger_after_the_users_own_developer_instructions() {
        let instructions = |inputs: CommandInputs| -> String {
            let overrides = codex_overrides(&command_args_for(ProgramKey::Codex, &inputs));
            assert_eq!(overrides.len(), 1, "{overrides:?}");
            assert_eq!(overrides[0].0, "developer_instructions");
            overrides[0].1.as_str().unwrap().to_owned()
        };
        assert_eq!(instructions(inputs(CODEX_HELP, None, true)), TRIGGER);
        let own = "Answer in \"French\".\nAlways\ttest.";
        assert_eq!(
            instructions(CommandInputs {
                codex_instructions: Some(own),
                ..inputs(CODEX_HELP, None, false)
            }),
            format!("{own}\n\n{TRIGGER}")
        );
        // Older than the MCP server's minimum, or of unknown version: no server, the shell trigger.
        assert_eq!(instructions(codex_inputs(Some("0.121.9"), true)), TRIGGER);
        assert_eq!(instructions(codex_inputs(None, true)), TRIGGER);
        assert!(command_args_for(ProgramKey::Codex, &inputs("", None, true)).is_empty());
        assert!(
            command_args_for(
                ProgramKey::Codex,
                &CommandInputs {
                    help: "",
                    ..codex_inputs(Some("0.163.0"), true)
                }
            )
            .is_empty()
        );
    }

    #[test]
    fn a_new_codex_gets_the_mcp_server_and_the_tool_trigger() {
        let args = command_args_for(
            ProgramKey::Codex,
            &CommandInputs {
                codex_instructions: Some("Be brief"),
                ..codex_inputs(Some("0.163.0"), true)
            },
        );
        let overrides = codex_overrides(&args);
        assert_eq!(overrides.len(), 2, "{overrides:?}");
        assert_eq!(overrides[0].0, "developer_instructions");
        assert_eq!(
            overrides[0].1.as_str(),
            Some(format!("Be brief\n\n{CODEX_TRIGGER}").as_str())
        );
        assert_eq!(overrides[1].0, "mcp_servers.deckhand");
        let server = overrides[1].1.as_table().unwrap();
        assert_eq!(server["command"].as_str(), Some(DECKHAND));
        assert_eq!(server["args"], toml::Value::Array(vec!["mcp".into()]));
        let env_vars: Vec<&str> = server["env_vars"]
            .as_array()
            .unwrap()
            .iter()
            .map(|name| name.as_str().unwrap())
            .collect();
        assert_eq!(
            env_vars,
            [
                "DECKHAND_SESSION_ID",
                "DECKHAND_LAUNCH_ID",
                "DECKHAND_HOOK_TOKEN",
                "DECKHAND_HOME",
                "DECKHAND_CHANNEL",
                "DECKHAND_ALLOW_PROD_HOME"
            ]
        );
        // The environment goes by name only: no `env` table, whose values would be arguments.
        assert!(!server.contains_key("env"));
        assert_eq!(server["tool_timeout_sec"].as_integer(), Some(90));
        assert_eq!(server["default_tools_approval_mode"].as_str(), Some("approve"));
        // The exact argument, as `ps` shows it.
        assert_eq!(
            args[3],
            "mcp_servers.deckhand={command=\"/h/agent-plugin/bin/deckhand\", args=[\"mcp\"], env_vars=[\
             \"DECKHAND_SESSION_ID\", \"DECKHAND_LAUNCH_ID\", \"DECKHAND_HOOK_TOKEN\", \"DECKHAND_HOME\", \
             \"DECKHAND_CHANNEL\", \"DECKHAND_ALLOW_PROD_HOME\"], tool_timeout_sec=90, \
             default_tools_approval_mode=\"approve\"}"
        );
        // From the first version that has it on.
        assert_eq!(
            codex_overrides(&command_args_for(
                ProgramKey::Codex,
                &codex_inputs(Some("0.122.0"), true)
            ))
            .len(),
            2
        );
    }

    #[test]
    fn with_deckhand_commands_not_allowed_codex_asks_before_the_tool_runs() {
        let args = command_args_for(ProgramKey::Codex, &codex_inputs(Some("0.163.0"), false));
        let overrides = codex_overrides(&args);
        assert_eq!(overrides[0].1.as_str(), Some(CODEX_TRIGGER));
        let server = overrides[1].1.as_table().unwrap();
        assert!(!server.contains_key("default_tools_approval_mode"), "{server:?}");
        assert_eq!(server["command"].as_str(), Some(DECKHAND));
    }

    #[test]
    fn the_mcp_server_command_is_quoted_for_toml_and_its_environment_goes_by_name() {
        let odd = Path::new("/Users/a \"b\"\\c/agent-plugin/bin/deckhand");
        let value = override_value(&codex_mcp_override(odd, true), "mcp_servers.deckhand");
        assert_eq!(value["command"].as_str(), Some(odd.to_str().unwrap()));
        // Nothing but these keys: the environment goes by name (`env_vars`), never as values (`env`), so the hook
        // token is no argument (`ps` shows arguments to every user).
        let mut keys: Vec<&str> = value.as_table().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "args",
                "command",
                "default_tools_approval_mode",
                "env_vars",
                "tool_timeout_sec"
            ]
        );
    }

    #[test]
    fn codex_takes_the_mcp_server_from_its_minimum_version() {
        assert!(codex_takes_mcp(Some("0.122.0")));
        assert!(codex_takes_mcp(Some("0.163.0")));
        assert!(codex_takes_mcp(Some("1.0.0")));
        assert!(!codex_takes_mcp(Some("0.121.3")));
        assert!(!codex_takes_mcp(Some("0.99.0")));
        assert!(!codex_takes_mcp(None));
    }

    #[test]
    fn the_users_codex_instructions_are_read_from_the_top_level() {
        assert_eq!(
            developer_instructions_in("model = \"o3\"\ndeveloper_instructions = \"Be brief\"\n").as_deref(),
            Some("Be brief")
        );
        assert_eq!(
            developer_instructions_in("[profiles.x]\ndeveloper_instructions = \"no\"\n"),
            None
        );
        assert_eq!(developer_instructions_in("developer_instructions = \"  \"\n"), None);
        assert_eq!(developer_instructions_in("not toml ="), None);
    }

    #[test]
    fn the_skill_keeps_only_its_name_and_description_in_the_frontmatter() {
        let skill = skill_markdown();
        let mut lines = skill.lines();
        assert_eq!(lines.next(), Some("---"));
        assert_eq!(lines.next(), Some("name: deckhand"));
        let description = lines.next().unwrap().strip_prefix("description: ").unwrap();
        assert_eq!(serde_json::from_str::<String>(description).unwrap(), SKILL_DESCRIPTION);
        assert_eq!(lines.next(), Some("---"));
        assert!(skill.ends_with(GUIDE));
        let manifest: serde_json::Value = serde_json::from_str(&plugin_manifest()).unwrap();
        assert_eq!(manifest["name"], "deckhand");
        assert_eq!(manifest["version"], deckhand_protocol::BUILD_VERSION);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_plugin_is_rewritten_only_when_it_differs() {
        let temp = tempfile::tempdir().unwrap();
        let dir = plugin_dir(temp.path());
        write_plugin(&dir).await.unwrap();
        let skill = skill_dir(&dir).join("SKILL.md");
        assert_eq!(std::fs::read_to_string(&skill).unwrap(), skill_markdown());
        assert_eq!(
            std::fs::read_to_string(dir.join(".claude-plugin/plugin.json")).unwrap(),
            plugin_manifest()
        );
        // Unchanged: left alone (a rewrite renames a new file over it).
        let inode = || std::fs::metadata(&skill).unwrap().ino();
        let before = inode();
        write_plugin(&dir).await.unwrap();
        assert_eq!(inode(), before);
        // Changed (an older build's): replaced.
        std::fs::write(&skill, "old").unwrap();
        write_plugin(&dir).await.unwrap();
        assert_eq!(std::fs::read_to_string(&skill).unwrap(), skill_markdown());
        assert_ne!(inode(), before);
    }

    #[test]
    fn the_agents_deckhand_is_a_link_to_this_executable_first_on_its_path() {
        let temp = tempfile::tempdir().unwrap();
        let dir = plugin_dir(temp.path());
        let (first, second) = (temp.path().join("first"), temp.path().join("second"));
        let bin = link_executable(&dir, &first).unwrap();
        assert_eq!(bin, dir.join("bin"));
        assert_eq!(linked_executable(&dir), bin.join("deckhand"));
        assert_eq!(std::fs::read_link(bin.join("deckhand")).unwrap(), first);
        // Another build: the link follows it.
        link_executable(&dir, &second).unwrap();
        assert_eq!(std::fs::read_link(bin.join("deckhand")).unwrap(), second);
        assert_eq!(std::fs::read_dir(&bin).unwrap().count(), 1);
        assert_eq!(
            path_with(&bin, Some(std::ffi::OsStr::new("/usr/bin:/bin"))),
            format!("{}:/usr/bin:/bin", bin.display())
        );
        assert_eq!(path_with(&bin, None), bin.to_string_lossy());
    }

    use std::os::unix::fs::MetadataExt;
}
