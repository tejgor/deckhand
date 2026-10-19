//! Port of settingsModel.ts.
//!
//! The Settings screen (C) as pure data: a grid with one row per setting and one column per layer (global defaults,
//! this repo's deckhand.json), each cell that layer's own value and whether it applies (from explainSettings); the
//! options of each control; and the edits as JSON written into the selected column's document.

use indexmap::IndexMap;
use serde::Serialize;
use serde_json::{Map, Value};
use serde_with::skip_serializing_none;

use crate::error::Result;
use deckhand_protocol::data::{
    CodexHookStatus, ConfigTargetKind, ConfigTargets, ProjectConfigDocument, SaveTrust, SettingsInfo, StringOrBool,
    TemplateVars, WorktreeCandidate,
};

/// TS: settingsModel.ts SettingId.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum SettingId {
    #[serde(rename = "defaultAgent")]
    DefaultAgent,
    #[serde(rename = "defaultWorkspace")]
    DefaultWorkspace,
    #[serde(rename = "devCommand")]
    DevCommand,
    #[serde(rename = "setupCommand")]
    SetupCommand,
    #[serde(rename = "actions")]
    Actions,
    #[serde(rename = "worktree.location")]
    WorktreeLocation,
    #[serde(rename = "worktree.branchFrom")]
    WorktreeBranchFrom,
    #[serde(rename = "worktree.branchName")]
    WorktreeBranchName,
    #[serde(rename = "worktree.links")]
    WorktreeLinks,
    #[serde(rename = "worktree.hook")]
    WorktreeHook,
    #[serde(rename = "agentHooks")]
    AgentHooks,
    #[serde(rename = "notifications")]
    Notifications,
}

impl SettingId {
    /// The TS string (also the SettingRow key, for the settings explainSettings lists).
    pub fn as_str(self) -> &'static str {
        match self {
            SettingId::DefaultAgent => "defaultAgent",
            SettingId::DefaultWorkspace => "defaultWorkspace",
            SettingId::DevCommand => "devCommand",
            SettingId::SetupCommand => "setupCommand",
            SettingId::Actions => "actions",
            SettingId::WorktreeLocation => "worktree.location",
            SettingId::WorktreeBranchFrom => "worktree.branchFrom",
            SettingId::WorktreeBranchName => "worktree.branchName",
            SettingId::WorktreeLinks => "worktree.links",
            SettingId::WorktreeHook => "worktree.hook",
            SettingId::AgentHooks => "agentHooks",
            SettingId::Notifications => "notifications",
        }
    }
}

/// TS: settingsModel.ts SettingControl.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum SettingControl {
    #[serde(rename = "choice")]
    Choice,
    #[serde(rename = "text")]
    Text,
    #[serde(rename = "actions")]
    Actions,
    #[serde(rename = "links")]
    Links,
}

/// TS: settingsModel.ts SettingDef section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum SettingSection {
    General,
    Commands,
    Worktrees,
    Agents,
}

/// TS: settingsModel.ts SettingDef (`global_only`: stored in config.json itself, never set by a repository).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingDef {
    pub id: SettingId,
    pub label: &'static str,
    pub section: SettingSection,
    pub control: SettingControl,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub global_only: bool,
}

const fn def(id: SettingId, label: &'static str, section: SettingSection, control: SettingControl) -> SettingDef {
    SettingDef {
        id,
        label,
        section,
        control,
        global_only: false,
    }
}

/// TS: settingsModel.ts SETTINGS (screen order).
pub const SETTINGS: [SettingDef; 12] = [
    def(
        SettingId::DefaultAgent,
        "Default agent",
        SettingSection::General,
        SettingControl::Choice,
    ),
    def(
        SettingId::DefaultWorkspace,
        "Default workspace",
        SettingSection::General,
        SettingControl::Choice,
    ),
    def(
        SettingId::DevCommand,
        "Dev command",
        SettingSection::Commands,
        SettingControl::Text,
    ),
    def(
        SettingId::SetupCommand,
        "Setup command",
        SettingSection::Commands,
        SettingControl::Text,
    ),
    def(
        SettingId::Actions,
        "Actions",
        SettingSection::Commands,
        SettingControl::Actions,
    ),
    def(
        SettingId::WorktreeLocation,
        "Location",
        SettingSection::Worktrees,
        SettingControl::Choice,
    ),
    def(
        SettingId::WorktreeBranchFrom,
        "Branch from",
        SettingSection::Worktrees,
        SettingControl::Choice,
    ),
    def(
        SettingId::WorktreeBranchName,
        "Branch name",
        SettingSection::Worktrees,
        SettingControl::Text,
    ),
    def(
        SettingId::WorktreeLinks,
        "Linked items",
        SettingSection::Worktrees,
        SettingControl::Links,
    ),
    def(
        SettingId::WorktreeHook,
        "Creation hook",
        SettingSection::Worktrees,
        SettingControl::Choice,
    ),
    SettingDef {
        global_only: true,
        ..def(
            SettingId::AgentHooks,
            "Agent signals",
            SettingSection::Agents,
            SettingControl::Choice,
        )
    },
    SettingDef {
        global_only: true,
        ..def(
            SettingId::Notifications,
            "Notifications",
            SettingSection::Agents,
            SettingControl::Choice,
        )
    },
];

/// TS: settingsModel.ts APP_FLAG (the config.json key of a global-only setting).
pub const APP_FLAG: [(SettingId, &str); 2] = [
    (SettingId::AgentHooks, "agent_hooks"),
    (SettingId::Notifications, "notifications"),
];

/// TS: settingsModel.ts isAppFlag.
pub fn is_app_flag(id: SettingId) -> bool {
    let _ = id;
    todo!("P9")
}

/// TS: settingsModel.ts LOCATION_PRESETS.next.
pub const LOCATION_NEXT_TO_REPO: &str = "{repoParent}/worktrees/{name}";

/// TS: settingsModel.ts LOCATION_PRESETS.inside.
pub const LOCATION_INSIDE_REPO: &str = "{repoRoot}/.worktrees/{name}";

/// TS: settingsModel.ts settingPath (the JSON path a setting is stored at).
pub fn setting_path(id: SettingId, entry: Option<&str>) -> Vec<String> {
    let _ = (id, entry);
    todo!("P9")
}

/// TS: settingsModel.ts Layer (a document's settings as written).
pub type Layer = Map<String, Value>;

/// TS: settingsModel.ts documentLayer. Errors: `<layer> not valid JSON; press E to repair it first`, `<layer> not a
/// JSON object; ...`.
pub fn document_layer(document: &ProjectConfigDocument) -> Result<Layer> {
    let _ = document;
    todo!("P9")
}

/// TS: settingsModel.ts infoLayer.
pub fn info_layer(targets: &ConfigTargets, kind: ConfigTargetKind) -> Option<Layer> {
    let _ = (targets, kind);
    todo!("P9")
}

/// TS: settingsModel.ts layerSets.
pub fn layer_sets(layer: Option<&Layer>, id: SettingId, entry: Option<&str>) -> bool {
    let _ = (layer, id, entry);
    todo!("P9")
}

/// TS: settingsModel.ts otherTarget.
pub fn other_target(kind: ConfigTargetKind) -> ConfigTargetKind {
    let _ = kind;
    todo!("P9")
}

/// TS: settingsModel.ts targetName.
pub fn target_name(kind: ConfigTargetKind) -> &'static str {
    let _ = kind;
    todo!("P9")
}

/// TS: settingsModel.ts SettingChange (`value` `None` removes the key).
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SettingChange {
    pub path: Vec<String>,
    pub value: Option<Value>,
}

/// TS: settingsModel.ts applyChange (the document's JSON with one key set or removed; validated like a save).
pub fn apply_change(document: &ProjectConfigDocument, change: &SettingChange) -> Result<String> {
    let _ = (document, change);
    todo!("P9")
}

/// TS: settingsModel.ts ownValue.
pub fn own_value<'a>(layer: Option<&'a Layer>, id: SettingId, entry: Option<&str>) -> Option<&'a Value> {
    let _ = (layer, id, entry);
    todo!("P9")
}

/// TS: settingsModel.ts COLUMNS.
pub const COLUMNS: [ConfigTargetKind; 2] = [ConfigTargetKind::Global, ConfigTargetKind::Repository];

/// TS: settingsModel.ts COLUMN_TITLES (longest first).
pub fn column_titles(kind: ConfigTargetKind) -> &'static [&'static str] {
    let _ = kind;
    todo!("P9")
}

/// TS: settingsModel.ts layerName.
pub fn layer_name(kind: ConfigTargetKind) -> &'static str {
    let _ = kind;
    todo!("P9")
}

/// TS: settingsModel.ts initialColumn.
pub fn initial_column(targets: &ConfigTargets) -> ConfigTargetKind {
    let _ = targets;
    todo!("P9")
}

/// TS: settingsModel.ts columnProblem.
pub fn column_problem(targets: &ConfigTargets, column: ConfigTargetKind) -> Option<String> {
    let _ = (targets, column);
    todo!("P9")
}

/// TS: settingsModel.ts GridCell.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GridCell {
    /// The layer's value for display (`—` when unset).
    pub text: String,
    pub full: Option<String>,
    pub template: Option<String>,
    pub set: bool,
    pub effective: bool,
    pub built_in: Option<bool>,
    pub legacy: Option<bool>,
    pub needs_trust: Option<bool>,
    pub global_only: Option<bool>,
    pub warning: Option<String>,
}

/// TS: settingsModel.ts GridRow `cells` (`Record<ConfigTargetKind, GridCell>`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct GridCells {
    pub global: GridCell,
    pub repository: GridCell,
}

impl GridCells {
    /// `cells[kind]`.
    pub fn get(&self, kind: ConfigTargetKind) -> &GridCell {
        let _ = kind;
        todo!("P9")
    }
}

/// TS: settingsModel.ts GridRow.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GridRow {
    pub def: SettingDef,
    pub cells: GridCells,
    pub note: Option<String>,
    pub warn: Option<bool>,
}

/// TS: settingsModel.ts homePath (`~` for the home directory).
pub fn home_path(value: &str, home: Option<&str>) -> String {
    let _ = (value, home);
    todo!("P9")
}

/// TS: settingsModel.ts codexHookNote.
pub fn codex_hook_note(status: &CodexHookStatus, home: Option<&str>) -> String {
    let _ = (status, home);
    todo!("P9")
}

/// TS: settingsModel.ts settingsGrid.
pub fn settings_grid(info: &SettingsInfo) -> Vec<GridRow> {
    let _ = info;
    todo!("P9")
}

/// TS: settingsModel.ts NEEDS_TRUST_DETAIL.
pub const NEEDS_TRUST_DETAIL: &str = "Applies once trusted — you'll be asked the first time it runs (or press T)";

/// TS: settingsModel.ts CellDetail.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CellDetail {
    pub head: String,
    pub layer: Option<ConfigTargetKind>,
    pub relation: String,
    pub warn: Option<bool>,
    pub error: Option<bool>,
}

/// TS: settingsModel.ts cellDetail.
pub fn cell_detail(info: &SettingsInfo, row: &GridRow, column: ConfigTargetKind) -> CellDetail {
    let _ = (info, row, column);
    todo!("P9")
}

/// TS: settingsModel.ts ActionEntry.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionEntry {
    pub name: String,
    pub command: String,
    pub status: Option<String>,
    pub needs_trust: Option<bool>,
}

/// TS: settingsModel.ts layerActions result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LayerActions {
    pub own: Vec<ActionEntry>,
    pub context: Vec<ActionEntry>,
}

/// TS: settingsModel.ts layerActions.
pub fn layer_actions(info: &SettingsInfo, kind: ConfigTargetKind) -> LayerActions {
    let _ = (info, kind);
    todo!("P9")
}

/// TS: settingsModel.ts ChoiceOption.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ChoiceOption {
    pub label: String,
    pub value: Option<StringOrBool>,
    pub detail: Option<String>,
    /// `detail` is a path (cut from the front).
    pub path: Option<bool>,
    /// Shown on its own line under the option.
    pub warning: Option<String>,
    /// Opens a text input (custom location).
    pub custom: Option<bool>,
}

/// TS: settingsModel.ts previewLocation.
pub fn preview_location(template: &str, vars: Option<&TemplateVars>) -> String {
    let _ = (template, vars);
    todo!("P9")
}

/// TS: settingsModel.ts choiceOptions.
pub fn choice_options(info: &SettingsInfo, id: SettingId, target: ConfigTargetKind) -> Vec<ChoiceOption> {
    let _ = (info, id, target);
    todo!("P9")
}

/// TS: settingsModel.ts currentChoice result (TS -1: `None`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CurrentChoice {
    /// The option to highlight first.
    pub index: Option<usize>,
    /// The option this layer stores (◉).
    pub current: Option<usize>,
}

/// TS: settingsModel.ts currentChoice.
pub fn current_choice(
    info: &SettingsInfo,
    id: SettingId,
    target: ConfigTargetKind,
    options: &[ChoiceOption],
) -> CurrentChoice {
    let _ = (info, id, target, options);
    todo!("P9")
}

/// TS: settingsModel.ts choiceChange (`None` when the target already has that value).
pub fn choice_change(
    info: &SettingsInfo,
    id: SettingId,
    target: ConfigTargetKind,
    option: &ChoiceOption,
) -> Option<SettingChange> {
    let _ = (info, id, target, option);
    todo!("P9")
}

/// TS: settingsModel.ts initialText.
pub fn initial_text(targets: &ConfigTargets, id: SettingId, target: ConfigTargetKind, entry: Option<&str>) -> String {
    let _ = (targets, id, target, entry);
    todo!("P9")
}

/// TS: settingsModel.ts inheritedHint.
pub fn inherited_hint(
    info: &SettingsInfo,
    id: SettingId,
    target: ConfigTargetKind,
    entry: Option<&str>,
) -> Option<String> {
    let _ = (info, id, target, entry);
    todo!("P9")
}

/// TS: settingsModel.ts MAX_ACTIONS.
pub const MAX_ACTIONS: usize = 30;

/// TS: settingsModel.ts MAX_ACTION_NAME.
pub const MAX_ACTION_NAME: usize = 48;

/// TS: settingsModel.ts ACTIONS_INTRO.
pub const ACTIONS_INTRO: &str = "Named shell commands you run with e on a session; they run in that session's worktree and show on its Terminal tab, beside the shell.";

/// TS: settingsModel.ts ACTION_NAME_RULES.
pub const ACTION_NAME_RULES: &str =
    "letters, numbers, spaces, _ . - · starts with a letter or number · up to 48 characters";

/// TS: settingsModel.ts ACTION_NAME_EXAMPLES.
pub const ACTION_NAME_EXAMPLES: &str = "e.g. test · lint frontend · db.migrate";

/// TS: settingsModel.ts ACTION_COMMAND_HELP.
pub const ACTION_COMMAND_HELP: &str =
    "Runs with your shell in the session's worktree, like typing it in a terminal · && pipes cd env vars all work";

/// TS: settingsModel.ts ACTION_COMMAND_EXAMPLES.
pub const ACTION_COMMAND_EXAMPLES: &str = "e.g. npm test · cd backend && .venv/bin/pytest -x · make lint";

/// TS: settingsModel.ts actionNameProblem.
pub fn action_name_problem(name: &str) -> Option<String> {
    let _ = name;
    todo!("P9")
}

/// TS: settingsModel.ts actionCount.
pub fn action_count(targets: &ConfigTargets, kind: ConfigTargetKind) -> usize {
    let _ = (targets, kind);
    todo!("P9")
}

/// TS: settingsModel.ts actionNameCheck result.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct NameCheck {
    pub error: Option<String>,
    pub note: Option<String>,
}

/// TS: settingsModel.ts actionNameCheck.
pub fn action_name_check(targets: &ConfigTargets, target: ConfigTargetKind, name: &str) -> NameCheck {
    let _ = (targets, target, name);
    todo!("P9")
}

/// TS: settingsModel.ts commandProblem.
pub fn command_problem(text: &str) -> Option<String> {
    let _ = text;
    todo!("P9")
}

/// TS: settingsModel.ts textChange (`{change}` or `{error}`: the validation error to show while the input stays open).
pub fn text_change(id: SettingId, text: &str, entry: Option<&str>) -> std::result::Result<SettingChange, String> {
    let _ = (id, text, entry);
    todo!("P9")
}

/// TS: settingsModel.ts initialLinks (`suggest` defaults to true).
pub fn initial_links(candidates: &[WorktreeCandidate], suggest: bool) -> IndexMap<String, bool> {
    let _ = (candidates, suggest);
    todo!("P9")
}

/// TS: settingsModel.ts LinkSelection.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkSelection {
    pub symlink: Vec<String>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// Unlinked here but still linked by the other layer.
    pub still_linked: Vec<String>,
}

/// TS: settingsModel.ts linkSelection.
pub fn link_selection(
    target: Option<&Layer>,
    other: Option<&Layer>,
    candidates: &[WorktreeCandidate],
    links: &IndexMap<String, bool>,
) -> LinkSelection {
    let _ = (target, other, candidates, links);
    todo!("P9")
}

/// TS: settingsModel.ts trustNote.
pub fn trust_note(trust: Option<SaveTrust>) -> &'static str {
    let _ = trust;
    todo!("P9")
}

/// TS: settingsModel.ts savedMessage options.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SavedMessageOptions {
    pub cleared: bool,
    pub trust: Option<SaveTrust>,
}

/// TS: settingsModel.ts savedMessage.
pub fn saved_message(label: &str, target: ConfigTargetKind, options: SavedMessageOptions) -> String {
    let _ = (label, target, options);
    todo!("P9")
}

/// TS: settingsModel.ts actionSavedMessage.
pub fn action_saved_message(name: &str, target: ConfigTargetKind, trust: Option<SaveTrust>) -> String {
    let _ = (name, target, trust);
    todo!("P9")
}
