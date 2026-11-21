//! `deckhand plugin agy [--remove]` (not in the TS): installs Deckhand's plugin for Antigravity in its global
//! customization root (`~/.gemini/config/plugins/deckhand`), or removes it. Antigravity takes no MCP server or skill for
//! one launch, so this is the one place Deckhand writes into an agent's own configuration, and only when you run it.
//! See `deckhand_sys::antigravity_plugin`.

use std::path::Path;

use deckhand_core::error::DhError;
use deckhand_sys::antigravity_plugin;

use crate::cli::print_line;

/// `deckhand plugin --help`.
pub const USAGE: &str = "\
Usage: deckhand plugin agy [--remove]

Installs Deckhand's plugin for Antigravity in ~/.gemini/config/plugins/deckhand: the deckhand skill and the deckhand
MCP tool, for agy sessions Deckhand starts (each launch allows the tool unless allow_deckhand_commands is false).
Outside a Deckhand session the tool only says it works inside one. Deckhand keeps the plugin up to date at agy
launches. --remove removes it. A folder of that name that isn't Deckhand's is never touched.";

/// What `plugin` was asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginAction {
    Install,
    Remove,
    Help,
}

/// Parses the words after `plugin`.
pub fn parse(args: &[String]) -> Result<PluginAction, DhError> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["agy"] => Ok(PluginAction::Install),
        ["agy", "--remove"] | ["--remove", "agy"] => Ok(PluginAction::Remove),
        [] | ["--help" | "-h" | "help"] | ["agy", "--help" | "-h"] => Ok(PluginAction::Help),
        _ => Err(DhError::msg(format!(
            "Unknown plugin arguments: {} (only agy has a plugin)\n\n{USAGE}",
            args.join(" ")
        ))),
    }
}

/// Runs `deckhand plugin <args>`.
pub async fn run_plugin(args: &[String]) -> Result<(), DhError> {
    run_in(&antigravity_plugin::config_root(), parse(args)?).await
}

async fn run_in(root: &Path, action: PluginAction) -> Result<(), DhError> {
    let dir = antigravity_plugin::plugin_dir(root);
    match action {
        PluginAction::Help => print_line(USAGE),
        PluginAction::Install => {
            antigravity_plugin::install(root).await?;
            print_line(&format!(
                "Installed Deckhand's Antigravity plugin in {}. agy sessions Deckhand starts from now on get the \
                 deckhand skill and tool (running ones: restart them). Remove it with deckhand plugin agy --remove.",
                dir.display()
            ));
        }
        PluginAction::Remove => {
            if antigravity_plugin::remove(root).await? {
                print_line(&format!(
                    "Removed Deckhand's Antigravity plugin from {}.",
                    dir.display()
                ));
            } else {
                print_line("Deckhand's Antigravity plugin is not installed.");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn parses_install_remove_and_help() {
        assert_eq!(parse(&args(&["agy"])).unwrap(), PluginAction::Install);
        assert_eq!(parse(&args(&["agy", "--remove"])).unwrap(), PluginAction::Remove);
        assert_eq!(parse(&args(&[])).unwrap(), PluginAction::Help);
        assert_eq!(parse(&args(&["agy", "--help"])).unwrap(), PluginAction::Help);
        let error = parse(&args(&["codex"])).unwrap_err().to_string();
        assert!(
            error.starts_with("Unknown plugin arguments: codex (only agy has a plugin)"),
            "{error}"
        );
    }

    #[test]
    fn installs_and_removes_under_the_root() {
        let root = tempfile::tempdir().expect("tempdir");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime
            .block_on(run_in(root.path(), PluginAction::Install))
            .expect("install");
        let dir = antigravity_plugin::plugin_dir(root.path());
        assert!(dir.join("plugin.json").exists() && dir.join("mcp_config.json").exists());
        runtime
            .block_on(run_in(root.path(), PluginAction::Remove))
            .expect("remove");
        assert!(!dir.exists());
        runtime
            .block_on(run_in(root.path(), PluginAction::Remove))
            .expect("absent");
    }
}
