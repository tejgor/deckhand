//! Deckhand's plugin for Antigravity (`agy`), installed by hand with `deckhand plugin agy` (not in the TS).
//!
//! agy takes no MCP server, skill or instructions for one launch: `--settings` refuses `mcpServers` and plugin keys,
//! and MCP servers come only from its global `~/.gemini/config/mcp_config.json` or from a plugin, found in a
//! customization root (`~/.gemini/config/plugins/<name>/`, or a repository's `.agents/plugins/`). So this one global
//! plugin, which you install once, carries what Deckhand tells agy:
//!
//! - `skills/deckhand/SKILL.md`: the `deckhand` skill (the guide), whose name and description agy keeps in context;
//! - `mcp_config.json`: the `deckhand` MCP server (`deckhand mcp`, the one Codex gets at launch), started by agy with
//!   its own environment, so with the session's `DECKHAND_*` variables and a `PATH` whose first `deckhand` is the
//!   launching build. Outside a Deckhand session its tool only says it works inside one;
//! - `plugin.json`, whose author is Deckhand: the marker. Only a folder carrying it is ever written or removed.
//!
//! agy namespaces a plugin's servers by the plugin's folder, so the tool is `deckhand_deckhand/deckhand`
//! ([`TOOL_RULE`]), which each launch may pre-allow in its `--settings` ([`launch_settings`]). A symlinked folder would
//! be found too, but agy then asks before the agent reads the skill (the files resolve outside its customization
//! root), so the files are copied, and refreshed at agy launches when they differ from this build's.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use deckhand_core::agent_guide::{AGY_TOOL_NOTE, GUIDE, SKILL_DESCRIPTION, SKILL_NAME};
use deckhand_core::error::{DhError, Result};

use crate::paths::home_dir;
use crate::storage::write_atomic;

/// The plugin's folder name under `plugins/`, which agy also uses to namespace its MCP server.
pub const PLUGIN_NAME: &str = "deckhand";

/// The marker in `plugin.json`: the author's name.
const AUTHOR: &str = "Deckhand";

/// The permission rule for the plugin's tool (`mcp(<folder>_<server>/<tool>)`).
pub const TOOL_RULE: &str = "mcp(deckhand_deckhand/deckhand)";

/// The plugin's description (agy lists it with the plugin).
const DESCRIPTION: &str = "The deckhand commands, for agents running in a Deckhand session.";

/// agy's global customization root: `~/.gemini/config`.
pub fn config_root() -> PathBuf {
    home_dir().join(".gemini").join("config")
}

/// `<root>/plugins/deckhand`.
pub fn plugin_dir(root: &Path) -> PathBuf {
    root.join("plugins").join(PLUGIN_NAME)
}

/// The plugin's files, relative to its folder.
pub fn plugin_files() -> [(PathBuf, String); 3] {
    let manifest = json!({
        "name": PLUGIN_NAME,
        "description": DESCRIPTION,
        "author": {"name": AUTHOR},
    });
    // The bare name: agy starts it with the session's PATH, which names the launching build's `deckhand` first.
    let servers = json!({"mcpServers": {"deckhand": {"command": "deckhand", "args": ["mcp"]}}});
    let pretty = |value: &Value| format!("{}\n", serde_json::to_string_pretty(value).unwrap_or_default());
    [
        (PathBuf::from("plugin.json"), pretty(&manifest)),
        (PathBuf::from("mcp_config.json"), pretty(&servers)),
        (Path::new("skills").join(SKILL_NAME).join("SKILL.md"), skill_markdown()),
    ]
}

/// `SKILL.md`: the skill Claude and Pi get (agy keeps its name and description in context), its body starting with
/// [`AGY_TOOL_NOTE`].
pub fn skill_markdown() -> String {
    // A JSON string is a YAML double-quoted scalar, so the description's colons cannot break the frontmatter.
    let description = serde_json::to_string(SKILL_DESCRIPTION).unwrap_or_default();
    format!("---\nname: {SKILL_NAME}\ndescription: {description}\n---\n\n{AGY_TOOL_NOTE}\n\n{GUIDE}")
}

/// What is at `<root>/plugins/deckhand`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginState {
    Absent,
    /// Deckhand's plugin (its `plugin.json` names Deckhand as the author).
    Ours,
    /// Something else with that name: never written or removed.
    Foreign,
}

/// What is at `<root>/plugins/deckhand`.
pub async fn plugin_state(root: &Path) -> PluginState {
    let dir = plugin_dir(root);
    if tokio::fs::symlink_metadata(&dir).await.is_err() {
        return PluginState::Absent;
    }
    let Ok(text) = tokio::fs::read_to_string(dir.join("plugin.json")).await else {
        return PluginState::Foreign;
    };
    let ours = serde_json::from_str::<Value>(&text)
        .ok()
        .is_some_and(|manifest| manifest["author"]["name"] == AUTHOR && manifest["name"] == PLUGIN_NAME);
    if ours && tokio::fs::symlink_metadata(&dir).await.is_ok_and(|meta| meta.is_dir()) {
        PluginState::Ours
    } else {
        PluginState::Foreign
    }
}

/// Writes each file that differs from this build's; whether any did.
async fn write_files(dir: &Path) -> Result<bool> {
    let mut wrote = false;
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
        wrote = true;
    }
    Ok(wrote)
}

fn foreign(root: &Path) -> DhError {
    DhError::msg(format!(
        "{} exists and is not Deckhand's plugin; it was left as it is",
        plugin_dir(root).display()
    ))
}

/// Installs (or brings up to date) the plugin under `root`; refuses a folder of that name that isn't Deckhand's.
pub async fn install(root: &Path) -> Result<()> {
    if plugin_state(root).await == PluginState::Foreign {
        return Err(foreign(root));
    }
    write_files(&plugin_dir(root)).await.map(|_| ())
}

/// At an agy launch: brings an installed plugin up to date; whether it is installed. A missing or foreign folder is
/// left alone.
pub async fn refresh(root: &Path) -> Result<bool> {
    match plugin_state(root).await {
        PluginState::Ours => write_files(&plugin_dir(root)).await.map(|_| true),
        PluginState::Absent | PluginState::Foreign => Ok(false),
    }
}

/// Removes the plugin: only the files Deckhand writes, then the folders they leave empty. Whether there was one;
/// refuses a folder that isn't Deckhand's.
pub async fn remove(root: &Path) -> Result<bool> {
    match plugin_state(root).await {
        PluginState::Absent => return Ok(false),
        PluginState::Foreign => return Err(foreign(root)),
        PluginState::Ours => {}
    }
    let dir = plugin_dir(root);
    for (relative, _) in plugin_files() {
        let path = dir.join(&relative);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(DhError::io(&error, "unlink", &path.to_string_lossy())),
        }
        // Its folders, innermost first, while they are empty (a folder something else added to stays).
        let mut parent = relative.parent();
        while let Some(folder) = parent.filter(|folder| !folder.as_os_str().is_empty()) {
            let _ = tokio::fs::remove_dir(dir.join(folder)).await;
            parent = folder.parent();
        }
    }
    let _ = tokio::fs::remove_dir(&dir).await;
    Ok(true)
}

/// The `--settings` JSON for one agy launch: the launch directory trusted (as given and canonical: agy matches the
/// exact path), and with `allow_tool` the plugin's tool pre-allowed. `user_permissions` is the `permissions` of agy's
/// own `settings.json`; the allow rule is added to it rather than replacing it, so the user's rules (deny ones
/// included) still apply whether agy merges `--settings` into its settings or replaces the key.
pub fn launch_settings(
    cwd: &str,
    canonical: Option<&str>,
    allow_tool: bool,
    user_permissions: Option<&Value>,
) -> Value {
    let mut trusted = vec![cwd];
    if let Some(canonical) = canonical.filter(|canonical| *canonical != cwd) {
        trusted.push(canonical);
    }
    let mut settings = Map::new();
    settings.insert("trustedWorkspaces".into(), json!(trusted));
    if allow_tool {
        let mut permissions = user_permissions.and_then(Value::as_object).cloned().unwrap_or_default();
        let mut allow = permissions
            .get("allow")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if !allow.iter().any(|rule| rule == TOOL_RULE) {
            allow.push(json!(TOOL_RULE));
        }
        permissions.insert("allow".into(), Value::Array(allow));
        settings.insert("permissions".into(), Value::Object(permissions));
    }
    Value::Object(settings)
}

/// The `permissions` of agy's own `settings.json` under `data_dir`, if any.
pub async fn user_permissions(data_dir: &Path) -> Option<Value> {
    let text = tokio::fs::read_to_string(data_dir.join("settings.json")).await.ok()?;
    let settings: Value = serde_json::from_str(&text).ok()?;
    settings.get("permissions").filter(|value| value.is_object()).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
    }

    #[test]
    fn the_plugin_carries_the_skill_and_the_mcp_server() {
        let files = plugin_files();
        let manifest: Value = serde_json::from_str(&files[0].1).expect("plugin.json");
        assert_eq!(manifest["name"], "deckhand");
        assert_eq!(manifest["author"]["name"], "Deckhand");
        let servers: Value = serde_json::from_str(&files[1].1).expect("mcp_config.json");
        assert_eq!(
            servers,
            json!({"mcpServers": {"deckhand": {"command": "deckhand", "args": ["mcp"]}}})
        );
        assert_eq!(files[2].0, Path::new("skills/deckhand/SKILL.md"));
        assert!(files[2].1.starts_with("---\nname: deckhand\n"));
        assert!(files[2].1.contains(&format!("---\n\n{AGY_TOOL_NOTE}\n\n# deckhand: ")));
    }

    #[test]
    fn install_refresh_and_remove_touch_only_deckhands_folder() {
        let root = tempfile::tempdir().expect("tempdir");
        let root = root.path();
        let runtime = runtime();
        let dir = plugin_dir(root);
        assert_eq!(runtime.block_on(plugin_state(root)), PluginState::Absent);
        assert!(!runtime.block_on(refresh(root)).expect("refresh"), "nothing installed");
        assert!(!dir.exists(), "refresh never installs");
        runtime.block_on(install(root)).expect("install");
        assert_eq!(runtime.block_on(plugin_state(root)), PluginState::Ours);
        // A stale file is brought up to date at the next launch.
        std::fs::write(dir.join("mcp_config.json"), "{}").expect("stale");
        assert!(runtime.block_on(refresh(root)).expect("refresh"));
        assert_eq!(
            std::fs::read_to_string(dir.join("mcp_config.json")).expect("read"),
            plugin_files()[1].1
        );
        // Something the user added in the folder stays, with the folder.
        std::fs::write(dir.join("notes.txt"), "mine").expect("extra");
        assert!(runtime.block_on(remove(root)).expect("remove"));
        assert!(dir.join("notes.txt").exists() && !dir.join("plugin.json").exists());
        assert!(!dir.join("skills").exists());
        std::fs::remove_file(dir.join("notes.txt")).expect("cleanup");
        std::fs::remove_dir(&dir).expect("cleanup");
        assert!(!runtime.block_on(remove(root)).expect("absent"));

        // A plugin of that name that isn't Deckhand's: never written, refreshed or removed.
        std::fs::create_dir_all(&dir).expect("foreign dir");
        std::fs::write(dir.join("plugin.json"), r#"{"name": "deckhand", "author": "someone"}"#).expect("foreign");
        assert_eq!(runtime.block_on(plugin_state(root)), PluginState::Foreign);
        assert!(runtime.block_on(install(root)).is_err());
        assert!(!runtime.block_on(refresh(root)).expect("refresh"));
        assert!(runtime.block_on(remove(root)).is_err());
        assert!(!dir.join("mcp_config.json").exists());
    }

    #[test]
    fn launch_settings_trust_the_directory_and_add_the_tool_to_the_users_rules() {
        assert_eq!(
            launch_settings("/w/t", None, false, None),
            json!({"trustedWorkspaces": ["/w/t"]})
        );
        assert_eq!(
            launch_settings("/tmp/w", Some("/private/tmp/w"), true, None),
            json!({"trustedWorkspaces": ["/tmp/w", "/private/tmp/w"], "permissions": {"allow": [TOOL_RULE]}})
        );
        // The user's own rules stay, deny ones included; the tool is added once.
        let mine = json!({"allow": ["command(ls)", TOOL_RULE], "deny": ["command(rm)"]});
        assert_eq!(
            launch_settings("/w", Some("/w"), true, Some(&mine))["permissions"],
            json!({"allow": ["command(ls)", TOOL_RULE], "deny": ["command(rm)"]})
        );
        let theirs = json!({"deny": ["mcp(deckhand_deckhand/deckhand)"]});
        assert_eq!(
            launch_settings("/w", None, true, Some(&theirs))["permissions"],
            json!({"deny": [TOOL_RULE], "allow": [TOOL_RULE]})
        );
    }

    #[test]
    fn reads_the_users_permissions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let runtime = runtime();
        assert_eq!(runtime.block_on(user_permissions(dir.path())), None);
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"trustedWorkspaces": [], "permissions": {"deny": ["command(rm)"]}}"#,
        )
        .expect("settings");
        assert_eq!(
            runtime.block_on(user_permissions(dir.path())),
            Some(json!({"deny": ["command(rm)"]}))
        );
    }
}
