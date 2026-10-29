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
//!
//! The plugin lives in the state directory (`<DECKHAND_HOME>/agent-plugin`) and is rewritten only when its content
//! differs from this build's.

use std::path::{Path, PathBuf};

use deckhand_core::agent_guide::{GUIDE, SKILL_DESCRIPTION, SKILL_NAME, TRIGGER};
use deckhand_core::error::{DhError, Result};
use deckhand_protocol::data::ProgramKey;

use crate::agent_path::program_help;
use crate::paths;
use crate::storage::write_atomic;

/// The plugin's directory in the state directory.
pub const PLUGIN_DIR_NAME: &str = "agent-plugin";

/// The permission rule that lets Claude run `deckhand` commands without asking.
pub const DECKHAND_COMMANDS_RULE: &str = "Bash(deckhand:*)";

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

/// `<plugin dir>/bin`, holding `deckhand` as a link to `exe` (replaced when it points elsewhere). Put first on an
/// agent's `PATH`, the `deckhand` commands it runs are this build's, whichever `deckhand` the `PATH` names (a dev
/// build's agents would otherwise run the installed one).
pub fn link_executable(plugin_dir: &Path, exe: &Path) -> Result<PathBuf> {
    let bin = plugin_dir.join("bin");
    let link = bin.join("deckhand");
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

/// The launch arguments that tell an agent about the `deckhand` commands (see the module docs). `plugin`: the
/// plugin directory, when it could be written; `allow_commands`: pre-allow them (Claude).
pub async fn command_args(
    program: ProgramKey,
    command: &str,
    plugin: Option<&Path>,
    allow_commands: bool,
) -> Vec<String> {
    let help = program_help(command).await;
    let codex_instructions = if program == ProgramKey::Codex {
        codex_user_instructions().await
    } else {
        None
    };
    command_args_for(program, &help, plugin, allow_commands, codex_instructions.as_deref())
}

/// [`command_args`] with the agent's `--help` text and the user's Codex instructions given.
pub fn command_args_for(
    program: ProgramKey,
    help: &str,
    plugin: Option<&Path>,
    allow_commands: bool,
    codex_instructions: Option<&str>,
) -> Vec<String> {
    let mut args = Vec::new();
    match program {
        ProgramKey::Claude => {
            // `--allowedTools` takes several values: the next option ends them, so it goes first.
            if allow_commands && help.contains("--allowedTools") {
                args.extend(["--allowedTools".to_owned(), DECKHAND_COMMANDS_RULE.to_owned()]);
            }
            match plugin {
                Some(plugin) if help.contains("--plugin-dir") => {
                    args.extend(["--plugin-dir".to_owned(), plugin.to_string_lossy().into_owned()]);
                }
                _ if help.contains("--append-system-prompt") => {
                    args.extend(["--append-system-prompt".to_owned(), TRIGGER.to_owned()]);
                }
                _ => {}
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
                let instructions = match codex_instructions {
                    Some(own) => format!("{own}\n\n{TRIGGER}"),
                    None => TRIGGER.to_owned(),
                };
                args.extend([
                    "-c".to_owned(),
                    format!("developer_instructions={}", toml_string(&instructions)),
                ]);
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

    #[test]
    fn claude_gets_the_plugin_and_the_pre_allowed_commands() {
        let plugin = Path::new("/h/agent-plugin");
        assert_eq!(
            command_args_for(ProgramKey::Claude, CLAUDE_HELP, Some(plugin), true, None),
            strings(&["--allowedTools", "Bash(deckhand:*)", "--plugin-dir", "/h/agent-plugin"])
        );
        // Turned off: the skill alone.
        assert_eq!(
            command_args_for(ProgramKey::Claude, CLAUDE_HELP, Some(plugin), false, None),
            strings(&["--plugin-dir", "/h/agent-plugin"])
        );
        // No plugin (unwritable) or no --plugin-dir: the short trigger instead.
        assert_eq!(
            command_args_for(ProgramKey::Claude, CLAUDE_HELP, None, false, None),
            strings(&["--append-system-prompt", TRIGGER])
        );
        assert_eq!(
            command_args_for(
                ProgramKey::Claude,
                "--allowedTools --append-system-prompt",
                Some(plugin),
                true,
                None
            ),
            strings(&["--allowedTools", "Bash(deckhand:*)", "--append-system-prompt", TRIGGER])
        );
        // An agent whose help offers none of them (a fake agent, a failed probe): nothing.
        assert!(command_args_for(ProgramKey::Claude, "", Some(plugin), true, None).is_empty());
    }

    #[test]
    fn pi_gets_the_skill_directory_and_nothing_to_allow() {
        let plugin = Path::new("/h/agent-plugin");
        let skill = ["--skill", "/h/agent-plugin/skills/deckhand"];
        assert_eq!(
            command_args_for(ProgramKey::Pi, PI_HELP, Some(plugin), true, None),
            strings(&skill)
        );
        assert_eq!(
            command_args_for(ProgramKey::Pi, PI_HELP, Some(plugin), false, None),
            strings(&skill)
        );
        assert_eq!(
            command_args_for(ProgramKey::Pi, PI_HELP, None, true, None),
            strings(&["--append-system-prompt", TRIGGER])
        );
        assert!(command_args_for(ProgramKey::Pi, "", Some(plugin), true, None).is_empty());
    }

    #[test]
    fn codex_gets_the_trigger_after_the_users_own_developer_instructions() {
        let value = |args: Vec<String>| -> String {
            assert_eq!(args.len(), 2, "{args:?}");
            assert_eq!(args[0], "-c");
            let assignment = args[1].strip_prefix("developer_instructions=").unwrap().to_owned();
            // As Codex reads it: a TOML value.
            let parsed: toml::Table = format!("v = {assignment}").parse().unwrap();
            parsed["v"].as_str().unwrap().to_owned()
        };
        assert_eq!(
            value(command_args_for(ProgramKey::Codex, CODEX_HELP, None, true, None)),
            TRIGGER
        );
        let own = "Answer in \"French\".\nAlways\ttest.";
        assert_eq!(
            value(command_args_for(ProgramKey::Codex, CODEX_HELP, None, false, Some(own))),
            format!("{own}\n\n{TRIGGER}")
        );
        assert!(command_args_for(ProgramKey::Codex, "", None, true, None).is_empty());
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
