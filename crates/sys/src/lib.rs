//! deckhand-sys.

pub mod agent_path;
pub mod agent_version_check;
pub mod async_util;
pub mod changes_git;
pub mod claude_transcripts;
pub mod config;
pub mod config_document;
pub mod desktop;
pub mod git;
pub mod notes_store;
pub mod notes_vim;
#[cfg(target_os = "macos")]
pub mod notifier_app;
pub mod paths;
pub mod proc;
pub mod process_tree;
pub mod session_features;
pub mod settings_info;
pub mod storage;
pub mod ui_state;
pub mod workspace_git;
pub mod worktree_links;
