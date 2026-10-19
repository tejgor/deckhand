//! Port of projectConfig.ts (pure validation parts).
//!
//! deckhand.json validation, trust fingerprints and the effective settings (global defaults overlaid by the trusted
//! repository file). Reading files (`readBoundedUtf8`, `loadProjectConfig`, `savedProjectTrust`) is I/O and lives in
//! `deckhand_sys::config`, which builds on these.
//!
//! The user config is storage.ts's AppConfig (`trustedProjects`, `dev_command`, `defaults`): TS UserSettings is
//! [`UserSettings`] = `AppConfig`. Where the TS reads `getConfigDir()` / `getConfigPath()` (for messages and the
//! built-in worktree location), callers pass `config_dir` (DECKHAND_HOME).

use indexmap::IndexMap;
use serde::Serialize;
use serde_json::Value;
use serde_with::skip_serializing_none;

use crate::error::Result;
use deckhand_protocol::Ordered;
use deckhand_protocol::data::{ConfigTargetKind, LoadedProject, ProjectConfig, SettingRow, TemplateVars};
use deckhand_protocol::records::AppConfig;

/// TS: projectConfig.ts PROJECT_CONFIG_FILE.
pub const PROJECT_CONFIG_FILE: &str = "deckhand.json";

/// TS: projectConfig.ts MAX_TRUSTED_FINGERPRINTS (per trust root).
pub const MAX_TRUSTED_FINGERPRINTS: usize = 20;

/// TS: projectConfig.ts ProjectTrust.trustedProjects (trust root → fingerprints, newest first).
pub type TrustedProjects = IndexMap<String, Vec<String>>;

/// TS: projectConfig.ts UserSettings (the parts of config.json that feed effective settings).
pub type UserSettings = AppConfig;

/// TS: projectConfig.ts sha256 (hex).
pub fn sha256(data: &[u8]) -> String {
    let _ = data;
    todo!("P10")
}

/// TS: projectConfig.ts validateProjectConfig (`label` defaults to `deckhand.json`; the result keeps the value's key
/// order).
pub fn validate_project_config(value: &Value, label: Option<&str>) -> Result<Ordered<ProjectConfig>> {
    let _ = (value, label);
    todo!("P10")
}

/// TS: projectConfig.ts parseProjectConfig (a leading BOM is ignored; JSON errors are JSON.parse's messages).
pub fn parse_project_config(raw: &str, label: Option<&str>) -> Result<Ordered<ProjectConfig>> {
    let _ = (raw, label);
    todo!("P10")
}

/// TS: projectConfig.ts hookSetting result: whether a detected creation hook is enabled, and which layer disabled it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HookSetting {
    pub enabled: bool,
    pub by: Option<ConfigTargetKind>,
}

/// TS: projectConfig.ts hookSetting (the repository's `worktree.hook`, else a global `false`; read leniently).
pub fn hook_setting(config: &ProjectConfig, user: &UserSettings) -> HookSetting {
    let _ = (config, user);
    todo!("P10")
}

/// TS: projectConfig.ts projectFingerprint (deckhand.json's text together with the enabled hook's fingerprint).
pub fn project_fingerprint(raw: &str, hook_fingerprint: Option<&str>) -> String {
    let _ = (raw, hook_fingerprint);
    todo!("P10")
}

/// TS: projectConfig.ts isProjectTrusted.
pub fn is_project_trusted(trust_root: &str, fingerprint: &str, trusted: Option<&TrustedProjects>) -> bool {
    let _ = (trust_root, fingerprint, trusted);
    todo!("P10")
}

/// TS: projectConfig.ts projectNeedsReview.
pub fn project_needs_review(project: &LoadedProject, trusted: Option<&TrustedProjects>) -> bool {
    let _ = (project, trusted);
    todo!("P10")
}

/// TS: projectConfig.ts trustProjectConfig: the new `trustedProjects` with the fingerprint first for its root
/// (deduplicated, capped; an existing root keeps its place, a new one is appended).
pub fn trust_project_config(trust_root: &str, fingerprint: &str, trusted: Option<&TrustedProjects>) -> TrustedProjects {
    let _ = (trust_root, fingerprint, trusted);
    todo!("P10")
}

/// TS: projectConfig.ts globalDefaults (config.json `defaults`, then the legacy `dev_command`). Error: `Invalid
/// "defaults" in <config_dir>/config.json: <why>. Repair it with C (Settings): ...`.
pub fn global_defaults(user: &UserSettings, config_dir: &str) -> Result<Ordered<ProjectConfig>> {
    let _ = (user, config_dir);
    todo!("P10")
}

/// TS: projectConfig.ts SUGGESTION_KEYS.
pub const SUGGESTION_KEYS: [&str; 2] = ["defaultAgent", "defaultWorkspace"];

/// TS: projectConfig.ts untrustedLayer (an untrusted repository file's suggestions).
pub fn untrusted_layer(config: &ProjectConfig) -> ProjectConfig {
    let _ = config;
    todo!("P10")
}

/// TS: projectConfig.ts resolveSettings (the effective settings; key order as the TS spreads build it).
pub fn resolve_settings(
    project: Option<&LoadedProject>,
    user: &UserSettings,
    config_dir: &str,
) -> Result<Ordered<ProjectConfig>> {
    let _ = (project, user, config_dir);
    todo!("P10")
}

/// TS: projectConfig.ts resolveDevCommand (`dev` when nothing sets one).
pub fn resolve_dev_command(project: Option<&LoadedProject>, user: &UserSettings, config_dir: &str) -> Result<String> {
    let _ = (project, user, config_dir);
    todo!("P10")
}

/// TS: projectConfig.ts resolveSetupCommand (an untrusted setupCommand that was not reviewed is an error).
pub fn resolve_setup_command(
    project: Option<&LoadedProject>,
    user: &UserSettings,
    reviewed_fingerprint: Option<&str>,
    config_dir: &str,
) -> Result<Option<String>> {
    let _ = (project, user, reviewed_fingerprint, config_dir);
    todo!("P10")
}

/// TS: projectConfig.ts ExplainContext.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExplainContext {
    /// Template placeholders for expanding location/files; without them templates are shown as written.
    pub vars: Option<TemplateVars>,
    /// `{user}` for the branch-name example.
    pub user: Option<String>,
    /// A detected create-worktree.sh when the project could not be loaded.
    pub hook_file: Option<String>,
    /// Deckhand's state directory (TS default `getConfigDir()`): the built-in worktree location and the config path
    /// in messages.
    pub config_dir: String,
}

/// TS: projectConfig.ts SettingsExplanation.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsExplanation {
    pub rows: Vec<SettingRow>,
    pub global_error: Option<String>,
}

/// TS: projectConfig.ts explainSettings (row by row with sources; pending values of an untrusted file).
pub fn explain_settings(
    project: Option<&LoadedProject>,
    user: &UserSettings,
    context: &ExplainContext,
) -> SettingsExplanation {
    let _ = (project, user, context);
    todo!("P10")
}
