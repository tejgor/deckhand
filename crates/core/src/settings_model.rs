//! Port of settingsModel.ts.
//!
//! The Settings screen (C) as pure data: a grid with one row per setting and one column per layer (global defaults,
//! this repo's deckhand.json), each cell that layer's own value and whether it applies (from explainSettings); the
//! options of each control; and the edits as JSON written into the selected column's document.

use std::collections::HashSet;
use std::sync::LazyLock;

use indexmap::{IndexMap, IndexSet};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use serde_with::skip_serializing_none;

use crate::config_draft::format_config_json;
use crate::error::{DhError, Result, json_parse_message};
use crate::jsstr;
use crate::project_config::validate_project_config;
use crate::worktree_links::worktree_location;
use deckhand_protocol::data::{
    CandidateConfigured, CandidateSuggestion, CodexHookState, CodexHookStatus, ConfigTargetKind, ConfigTargets,
    JsNumber, ProjectConfigDocument, RepositoryState, SaveTrust, SettingRow, SettingSource, SettingsInfo, StringOrBool,
    TemplateVars, WorktreeCandidate,
};

/// TS: settingsModel.ts SettingId.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    APP_FLAG.iter().any(|(flag, _)| *flag == id)
}

/// TS: settingsModel.ts LOCATION_PRESETS.next.
pub const LOCATION_NEXT_TO_REPO: &str = "{repoParent}/worktrees/{name}";

/// TS: settingsModel.ts LOCATION_PRESETS.inside.
pub const LOCATION_INSIDE_REPO: &str = "{repoRoot}/.worktrees/{name}";

/// TS: settingsModel.ts settingPath (the JSON path a setting is stored at).
pub fn setting_path(id: SettingId, entry: Option<&str>) -> Vec<String> {
    match id {
        SettingId::Actions => match entry {
            None => vec![String::from("actions")],
            Some(entry) => vec![String::from("actions"), entry.to_owned()],
        },
        SettingId::WorktreeLinks => vec![String::from("worktree"), String::from("symlink")],
        _ => id.as_str().split('.').map(str::to_owned).collect(),
    }
}

/// TS: settingsModel.ts Layer (a document's settings as written).
pub type Layer = Map<String, Value>;

/// TS: settingsModel.ts documentLayer. Errors: `<layer> not valid JSON; press E to repair it first`, `<layer> not a
/// JSON object; ...`.
pub fn document_layer(document: &ProjectConfigDocument) -> Result<Layer> {
    parsed_layer(document, Surrogates::Replace)
}

/// `document_layer`, with lone surrogates either replaced by U+FFFD (display) or carried as marks (editing).
fn parsed_layer(document: &ProjectConfigDocument, surrogates: Surrogates) -> Result<Layer> {
    let label = layer_label(document.kind);
    let value = if document.exists {
        js_json_parse(&document.raw, surrogates)
            .ok_or_else(|| DhError::msg(format!("{label} not valid JSON; press E to repair it first")))?
    } else {
        Value::Object(Map::new())
    };
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(DhError::msg(format!(
            "{label} not a JSON object; press E to repair it first"
        ))),
    }
}

/// TS: settingsModel.ts infoLayer.
pub fn info_layer(targets: &ConfigTargets, kind: ConfigTargetKind) -> Option<Layer> {
    target_document(targets, kind).and_then(|document| document_layer(document).ok())
}

/// TS: settingsModel.ts layerSets.
pub fn layer_sets(layer: Option<&Layer>, id: SettingId, entry: Option<&str>) -> bool {
    if id == SettingId::WorktreeLinks {
        return value_at(layer, &["worktree", "symlink"]).is_some()
            || value_at(layer, &["worktree", "files"]).is_some();
    }
    value_at(layer, &setting_path(id, entry)).is_some()
}

/// TS: settingsModel.ts otherTarget.
pub fn other_target(kind: ConfigTargetKind) -> ConfigTargetKind {
    match kind {
        ConfigTargetKind::Global => ConfigTargetKind::Repository,
        ConfigTargetKind::Repository => ConfigTargetKind::Global,
    }
}

/// TS: settingsModel.ts targetName.
pub fn target_name(kind: ConfigTargetKind) -> &'static str {
    match kind {
        ConfigTargetKind::Global => "global defaults",
        ConfigTargetKind::Repository => "this repo's deckhand.json",
    }
}

/// TS: settingsModel.ts SettingChange (`value` `None` removes the key).
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SettingChange {
    pub path: Vec<String>,
    pub value: Option<Value>,
}

/// TS: settingsModel.ts applyChange (the document's JSON with one key set or removed; validated like a save).
///
/// A Rust string cannot hold the lone surrogates JSON.parse keeps (and JSON.stringify writes back as `\udxxx`), so the
/// layer is edited with them carried as marks (`Surrogates::Mark`) that become escapes again in the output.
pub fn apply_change(document: &ProjectConfigDocument, change: &SettingChange) -> Result<String> {
    let mut next = parsed_layer(document, Surrogates::Mark)?;
    let path: Vec<String> = change.path.iter().map(|key| escape_mark(key)).collect();
    let value = change.value.as_ref().map(|value| map_strings(value, &escape_mark));
    if let Some((last, parents_path)) = path.split_last() {
        // `parents.length` in TS: how many objects along the path exist (or were created); the root is the first.
        let mut parents = 1;
        for (depth, key) in parents_path.iter().enumerate() {
            let Some(parent) = object_at_mut(&mut next, &path[..depth]) else {
                break;
            };
            match parent.get(key) {
                // `parent[key]` finds Object.prototype's methods: not an object.
                None if inherited_function(key) => return Err(not_an_object(&change.path, parents)),
                None => {
                    if value.is_none() {
                        break;
                    }
                    parent.insert(key.clone(), Value::Object(Map::new()));
                }
                Some(Value::Object(_)) => {}
                Some(_) => return Err(not_an_object(&change.path, parents)),
            }
            parents += 1;
        }
        if parents == path.len()
            && let Some(parent) = object_at_mut(&mut next, parents_path)
        {
            match value {
                None => {
                    parent.shift_remove(last);
                }
                // Assigning `__proto__` sets the prototype instead of an own property: nothing is written.
                Some(_) if last == "__proto__" && !parent.contains_key(last) => {}
                Some(value) => {
                    parent.insert(last.clone(), value);
                }
            }
        }
        for depth in (1..parents).rev() {
            let empty = object_at_mut(&mut next, &path[..depth]).is_some_and(|object| object.is_empty());
            if empty && let Some(parent) = object_at_mut(&mut next, &path[..depth - 1]) {
                parent.shift_remove(&path[depth - 1]);
            }
        }
    }
    let next = Value::Object(next);
    let label = (document.kind == ConfigTargetKind::Global).then_some("defaults");
    // Validated as before, with each lone surrogate read as U+FFFD (one UTF-16 unit, like the surrogate itself).
    validate_project_config(
        &map_strings(&next, &|text| unmark(text, |_| String::from("\u{FFFD}"))),
        label,
    )?;
    let text = format_config_json(&serde_json::to_string(&next)?)?;
    Ok(unmark(&text, |digits| format!("\\u{digits}")))
}

/// TS: settingsModel.ts ownValue.
pub fn own_value<'a>(layer: Option<&'a Layer>, id: SettingId, entry: Option<&str>) -> Option<&'a Value> {
    value_at(layer, &setting_path(id, entry))
}

/// TS: settingsModel.ts COLUMNS.
pub const COLUMNS: [ConfigTargetKind; 2] = [ConfigTargetKind::Global, ConfigTargetKind::Repository];

/// TS: settingsModel.ts COLUMN_TITLES (longest first).
pub fn column_titles(kind: ConfigTargetKind) -> &'static [&'static str] {
    match kind {
        ConfigTargetKind::Global => &["Global (all repos)", "Global"],
        ConfigTargetKind::Repository => &["This repo"],
    }
}

/// TS: settingsModel.ts layerName.
pub fn layer_name(kind: ConfigTargetKind) -> &'static str {
    match kind {
        ConfigTargetKind::Global => "Global",
        ConfigTargetKind::Repository => "This repo",
    }
}

/// TS: settingsModel.ts initialColumn.
pub fn initial_column(targets: &ConfigTargets) -> ConfigTargetKind {
    if targets.repository.is_some() {
        ConfigTargetKind::Repository
    } else {
        ConfigTargetKind::Global
    }
}

/// TS: settingsModel.ts columnProblem.
pub fn column_problem(targets: &ConfigTargets, column: ConfigTargetKind) -> Option<String> {
    match column {
        ConfigTargetKind::Repository if targets.repository.is_none() => Some(format!(
            "No repository layer here: {}",
            targets.repository_error.as_deref().unwrap_or("no deckhand.json")
        )),
        ConfigTargetKind::Global if targets.global.is_none() => Some(
            targets
                .global_error
                .clone()
                .unwrap_or_else(|| String::from("Global defaults are unavailable")),
        ),
        _ => None,
    }
}

/// TS: settingsModel.ts GridCell.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GridCells {
    pub global: GridCell,
    pub repository: GridCell,
}

impl GridCells {
    /// `cells[kind]`.
    pub fn get(&self, kind: ConfigTargetKind) -> &GridCell {
        match kind {
            ConfigTargetKind::Global => &self.global,
            ConfigTargetKind::Repository => &self.repository,
        }
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
    match home {
        Some(home)
            if !home.is_empty()
                && (value == home || value.strip_prefix(home).is_some_and(|rest| rest.starts_with('/'))) =>
        {
            format!("~{}", &value[home.len()..])
        }
        _ => value.to_owned(),
    }
}

/// TS: settingsModel.ts codexHookNote.
pub fn codex_hook_note(status: &CodexHookStatus, home: Option<&str>) -> String {
    let file = home_path(&status.file, home);
    if status.state == CodexHookState::Other {
        return format!(
            "Codex hooks call another Deckhand install: redo {}, then /hooks in Codex",
            status.command
        );
    }
    let target = if status.file_exists {
        format!("→ merge into {file}")
    } else {
        format!("> {file}")
    };
    format!(
        "Codex hooks not set up: {} {target}, then /hooks in Codex",
        status.command
    )
}

/// TS: settingsModel.ts settingsGrid.
pub fn settings_grid(info: &SettingsInfo) -> Vec<GridRow> {
    let home = info.vars.as_ref().map(|vars| vars.home.as_str());
    let global_layer = info_layer(&info.targets, ConfigTargetKind::Global);
    let repository_layer = info_layer(&info.targets, ConfigTargetKind::Repository);
    let layers = |kind: ConfigTargetKind| match kind {
        ConfigTargetKind::Global => global_layer.as_ref(),
        ConfigTargetKind::Repository => repository_layer.as_ref(),
    };
    let hook_row = info.rows.iter().find(|row| row.key == "worktree.hook");
    SETTINGS
        .iter()
        .map(|def| {
            if is_app_flag(def.id) {
                let on = app_flag(info, def.id);
                let set = stored_flag(info, def.id).is_some();
                let codex = if def.id == SettingId::AgentHooks && info.agent_hooks == Some(true) {
                    info.codex_hooks.as_ref()
                } else {
                    None
                };
                let global = GridCell {
                    text: String::from(if on { "on" } else { "off" }),
                    set,
                    effective: true,
                    built_in: (!set).then_some(true),
                    warning: codex.map(|_| String::from("Codex")),
                    ..GridCell::default()
                };
                let repository = GridCell {
                    text: String::from("global only"),
                    set: false,
                    effective: false,
                    global_only: Some(true),
                    ..GridCell::default()
                };
                return GridRow {
                    def: *def,
                    cells: GridCells { global, repository },
                    note: codex.map(|codex| codex_hook_note(codex, home)),
                    warn: codex.map(|_| true),
                };
            }
            if matches!(def.control, SettingControl::Actions | SettingControl::Links) {
                let actions = def.control == SettingControl::Actions;
                let keys: &[&str] = if actions {
                    &["actions"]
                } else {
                    &["worktree.symlink", "worktree.files"]
                };
                let rows: Vec<&SettingRow> = info
                    .rows
                    .iter()
                    .filter(|row| keys.contains(&row.key.as_str()) && row.entry.is_some())
                    .collect();
                let cell = |kind: ConfigTargetKind| -> GridCell {
                    let layer = layers(kind);
                    let own: Vec<String> = if actions {
                        record_keys(own_value(layer, SettingId::Actions, None))
                    } else {
                        let mut own = strings(value_at(layer, &["worktree", "symlink"]));
                        own.extend(record_keys(value_at(layer, &["worktree", "files"])));
                        own
                    };
                    let text = if own.is_empty() {
                        String::from("—")
                    } else if actions {
                        own.join(", ")
                    } else {
                        format!("{} item{}", own.len(), if own.len() == 1 { "" } else { "s" })
                    };
                    let pending = kind == ConfigTargetKind::Repository && rows.iter().any(|row| row.pending.is_some());
                    GridCell {
                        text,
                        full: Some(own.join(", ")),
                        set: !own.is_empty(),
                        effective: rows.iter().any(|row| row.source.as_str() == source_of(kind)),
                        needs_trust: pending.then_some(true),
                        ..GridCell::default()
                    }
                };
                return GridRow {
                    def: *def,
                    cells: GridCells {
                        global: cell(ConfigTargetKind::Global),
                        repository: cell(ConfigTargetKind::Repository),
                    },
                    note: None,
                    warn: None,
                };
            }
            let row = info.rows.iter().find(|entry| entry.key == def.id.as_str());
            let source = row.map_or("not set", |row| row.source.as_str());
            let cell = |kind: ConfigTargetKind| -> GridCell {
                let effective = source == source_of(kind);
                if let Some(own) = own_value(layers(kind), def.id, None) {
                    let needs_trust = kind == ConfigTargetKind::Repository
                        && !effective
                        && row.is_some_and(|row| row.pending.is_some());
                    return GridCell {
                        set: true,
                        effective,
                        needs_trust: needs_trust.then_some(true),
                        ..show(info, def.id, own)
                    };
                }
                if kind == ConfigTargetKind::Global
                    && source == "legacy dev_command"
                    && let Some(StringOrBool::String(raw)) = row.and_then(|row| row.raw.as_ref())
                {
                    return GridCell {
                        set: false,
                        effective: true,
                        legacy: Some(true),
                        ..show(info, def.id, &Value::String(raw.clone()))
                    };
                }
                if kind == ConfigTargetKind::Global && source == "built-in default" {
                    if def.id == SettingId::WorktreeLocation {
                        let text = home_path(&info.default_location, home);
                        return GridCell {
                            full: Some(text.clone()),
                            text,
                            set: false,
                            effective: true,
                            built_in: Some(true),
                            ..GridCell::default()
                        };
                    }
                    if let Some(value) = built_in_value(def.id) {
                        return GridCell {
                            set: false,
                            effective: true,
                            built_in: Some(true),
                            ..show(info, def.id, &value)
                        };
                    }
                }
                GridCell {
                    text: String::from("—"),
                    set: false,
                    effective: false,
                    ..GridCell::default()
                }
            };
            let ignored = def.id.as_str().starts_with("worktree.")
                && def.id != SettingId::WorktreeHook
                && row
                    .and_then(|row| row.note.as_deref())
                    .is_some_and(|note| note.starts_with("ignored"));
            let note = if def.id == SettingId::WorktreeHook {
                if info.hook_file.as_deref().is_some_and(|file| !file.is_empty()) {
                    hook_row.and_then(|row| row.note.clone())
                } else {
                    Some(String::from("no .claude/scripts/create-worktree.sh in this repo"))
                }
            } else {
                ignored.then(|| String::from("ignored while the creation hook is active"))
            };
            GridRow {
                def: *def,
                cells: GridCells {
                    global: cell(ConfigTargetKind::Global),
                    repository: cell(ConfigTargetKind::Repository),
                },
                note: note.filter(|note| !note.is_empty()),
                warn: None,
            }
        })
        .collect()
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
    let id = row.def.id;
    let base = |relation: String| CellDetail {
        head: layer_name(column).to_owned(),
        layer: Some(column),
        relation,
        ..CellDetail::default()
    };
    if is_app_flag(id) {
        return match column {
            ConfigTargetKind::Repository => base(String::from("global only: it is about your machine, not the repo")),
            ConfigTargetKind::Global => base(app_flag_detail(info, id).to_owned()),
        };
    }
    if let Some(problem) = column_problem(&info.targets, column) {
        return CellDetail {
            error: Some(true),
            ..base(problem)
        };
    }
    if column == ConfigTargetKind::Repository && info.repository.state == RepositoryState::Invalid {
        return CellDetail {
            error: Some(true),
            ..base(format!(
                "invalid ({}): e repairs it",
                info.repository.error.as_deref().unwrap_or("unknown error")
            ))
        };
    }
    let cell = row.cells.get(column);
    let other = row.cells.get(other_target(column));
    let list = matches!(row.def.control, SettingControl::Actions | SettingControl::Links);
    let truthy = |flag: Option<bool>| flag == Some(true);
    if truthy(cell.built_in) {
        return CellDetail {
            head: String::from("Built-in default"),
            relation: built_in_detail(id).to_owned(),
            ..CellDetail::default()
        };
    }
    if truthy(cell.legacy) {
        return base(String::from(
            "the legacy dev_command in config.json; setting a global Dev command replaces it",
        ));
    }
    if truthy(cell.needs_trust) {
        return CellDetail {
            warn: Some(true),
            ..base(NEEDS_TRUST_DETAIL.to_owned())
        };
    }
    if !cell.set {
        let relation = match column {
            ConfigTargetKind::Global if other.effective => "not set; this repo sets it",
            ConfigTargetKind::Global => built_in_detail(id),
            ConfigTargetKind::Repository if truthy(other.built_in) => "not set: the built-in default applies",
            ConfigTargetKind::Repository if other.effective => "not set: inherits global",
            ConfigTargetKind::Repository => "not set",
        };
        return base(relation.to_owned());
    }
    if !cell.effective {
        return base(String::from(match column {
            ConfigTargetKind::Global => "overridden by this repo",
            ConfigTargetKind::Repository => "not in effect",
        }));
    }
    if column == ConfigTargetKind::Repository {
        let relation = if list && other.effective {
            "merged with global (same names: this repo wins)"
        } else if other.set || truthy(other.legacy) {
            "overrides global"
        } else {
            "applies here"
        };
        // What an untrusted file may still do: preselect the n picker, or switch the creation hook off.
        let untrusted = if info.repository.state == RepositoryState::Trusted {
            None
        } else if id == SettingId::WorktreeHook {
            Some("it can only switch the hook off")
        } else if row.def.section == SettingSection::General {
            Some("it only preselects n")
        } else {
            None
        };
        return base(match untrusted {
            Some(untrusted) => format!("{relation}, untrusted too: {untrusted}"),
            None => relation.to_owned(),
        });
    }
    base(String::from(if list && other.effective {
        "merged with this repo"
    } else if truthy(other.needs_trust) {
        "applies until this repo's value is trusted"
    } else {
        "inherited by this repo"
    }))
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
    let commands = |layer: ConfigTargetKind| -> Vec<(String, String)> {
        let layer = info_layer(&info.targets, layer);
        match own_value(layer.as_ref(), SettingId::Actions, None) {
            Some(Value::Object(map)) => map
                .iter()
                .filter_map(|(name, command)| command.as_str().map(|command| (name.clone(), command.to_owned())))
                .collect(),
            _ => Vec::new(),
        }
    };
    let pending: HashSet<&str> = info
        .rows
        .iter()
        .filter(|row| row.key == "actions" && row.pending.is_some())
        .filter_map(|row| row.entry.as_deref())
        .collect();
    let mine = commands(kind);
    let theirs = commands(other_target(kind));
    let has = |list: &[(String, String)], name: &str| list.iter().any(|(entry, _)| entry == name);
    let repository = kind == ConfigTargetKind::Repository;
    let own = mine
        .iter()
        .map(|(name, command)| {
            if repository && pending.contains(name.as_str()) {
                return ActionEntry {
                    name: name.clone(),
                    command: command.clone(),
                    status: Some(String::from("⚠ needs trust")),
                    needs_trust: Some(true),
                };
            }
            let status = has(&theirs, name).then(|| {
                String::from(if repository {
                    "overrides global"
                } else {
                    "this repo overrides it"
                })
            });
            ActionEntry {
                name: name.clone(),
                command: command.clone(),
                status,
                needs_trust: None,
            }
        })
        .collect();
    let context = theirs
        .iter()
        .filter(|(name, _)| !has(&mine, name))
        .map(|(name, command)| {
            let status = if repository {
                "global"
            } else if pending.contains(name.as_str()) {
                "this repo · ⚠ needs trust"
            } else {
                "this repo"
            };
            ActionEntry {
                name: name.clone(),
                command: command.clone(),
                status: Some(status.to_owned()),
                needs_trust: None,
            }
        })
        .collect();
    LayerActions { own, context }
}

/// TS: settingsModel.ts ChoiceOption.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
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
    let Some(vars) = vars else {
        return template.to_owned();
    };
    match worktree_location(template, vars) {
        Ok(location) => home_path(&location, Some(&vars.home)),
        Err(error) => format!("invalid: {error}"),
    }
}

/// TS: settingsModel.ts choiceOptions.
pub fn choice_options(info: &SettingsInfo, id: SettingId, target: ConfigTargetKind) -> Vec<ChoiceOption> {
    let option = |label: &str, value: Option<StringOrBool>, detail: Option<&str>| ChoiceOption {
        label: label.to_owned(),
        value,
        detail: detail.map(str::to_owned),
        ..ChoiceOption::default()
    };
    let text = |value: &str| Some(StringOrBool::String(value.to_owned()));
    let flag = |value: bool| Some(StringOrBool::Bool(value));
    match id {
        SettingId::DefaultAgent => ["claude", "pi", "codex"]
            .iter()
            .map(|value| option(value, text(value), None))
            .collect(),
        SettingId::DefaultWorkspace => ["none", "new", "existing"]
            .iter()
            .map(|value| option(workspace_label(value).unwrap_or(value), text(value), None))
            .collect(),
        SettingId::WorktreeBranchFrom => ["current", "default", "origin"]
            .iter()
            .map(|value| option(&branch_from_label(value, info), text(value), None))
            .collect(),
        SettingId::AgentHooks => vec![
            option(
                "on",
                flag(true),
                Some("Claude: automatic · Codex: needs its hooks set up"),
            ),
            option("off", flag(false), Some("attention from terminal activity only")),
        ],
        SettingId::Notifications => vec![
            option(
                "on",
                flag(true),
                Some("desktop notification when a session needs you or exits"),
            ),
            option("off", flag(false), None),
        ],
        SettingId::WorktreeHook => vec![
            option(
                "on",
                flag(true),
                Some(if info.hook_file.as_deref().is_some_and(|file| !file.is_empty()) {
                    "create-worktree.sh runs once trusted"
                } else {
                    "no create-worktree.sh found"
                }),
            ),
            option("off", flag(false), Some("Deckhand creates worktrees itself")),
        ],
        SettingId::WorktreeLocation => {
            let vars = info.vars.as_ref();
            let global_layer = if target == ConfigTargetKind::Repository {
                info_layer(&info.targets, ConfigTargetKind::Global)
            } else {
                None
            };
            let inherited = match own_value(global_layer.as_ref(), SettingId::WorktreeLocation, None) {
                Some(Value::String(global)) => ChoiceOption {
                    path: Some(true),
                    ..option("global default", None, Some(&preview_location(global, vars)))
                },
                _ => ChoiceOption {
                    path: Some(true),
                    ..option(
                        "Deckhand default",
                        None,
                        Some(&home_path(&info.default_location, vars.map(|vars| vars.home.as_str()))),
                    )
                },
            };
            let target_layer = info_layer(&info.targets, target);
            let custom = match own_value(target_layer.as_ref(), id, None) {
                Some(Value::String(own))
                    if !own.is_empty() && own != LOCATION_NEXT_TO_REPO && own != LOCATION_INSIDE_REPO =>
                {
                    ChoiceOption {
                        custom: Some(true),
                        path: Some(true),
                        ..option("custom…", text(own), Some(own))
                    }
                }
                _ => ChoiceOption {
                    custom: Some(true),
                    ..option("custom…", None, Some("type a template with {name}"))
                },
            };
            vec![
                inherited,
                ChoiceOption {
                    path: Some(true),
                    ..option(
                        "next to repo",
                        text(LOCATION_NEXT_TO_REPO),
                        Some(&preview_location(LOCATION_NEXT_TO_REPO, vars)),
                    )
                },
                ChoiceOption {
                    path: Some(true),
                    warning: (!info.inside_ignored).then(|| {
                        String::from("⚠ .worktrees/ is not gitignored — worktrees would show up as untracked files")
                    }),
                    ..option(
                        "inside repo",
                        text(LOCATION_INSIDE_REPO),
                        Some(&preview_location(LOCATION_INSIDE_REPO, vars)),
                    )
                },
                custom,
            ]
        }
        _ => Vec::new(),
    }
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
    let position = |found: &dyn Fn(&ChoiceOption) -> bool| options.iter().position(found);
    if is_app_flag(id) {
        let on = app_flag(info, id);
        let index = position(&|option| option.value == Some(StringOrBool::Bool(on)));
        return CurrentChoice {
            index,
            current: stored_flag(info, id).and(index),
        };
    }
    let layer = info_layer(&info.targets, target);
    let own = own_value(layer.as_ref(), id, None);
    if id == SettingId::WorktreeLocation {
        let Some(own) = own else {
            return CurrentChoice {
                index: Some(0),
                current: Some(0),
            };
        };
        let found = position(&|option| option.custom != Some(true) && same_value(option.value.as_ref(), Some(own)));
        let index = found.or_else(|| options.len().checked_sub(1));
        return CurrentChoice { index, current: index };
    }
    if let Some(own) = own {
        let index = position(&|option| same_value(option.value.as_ref(), Some(own)));
        return CurrentChoice {
            index: Some(index.unwrap_or(0)),
            current: index,
        };
    }
    let row = info.rows.iter().find(|entry| entry.key == id.as_str());
    let value = match row.and_then(|row| row.raw.as_ref()) {
        Some(raw) => Some(or_bool_value(raw)),
        None if id == SettingId::WorktreeHook => Some(Value::Bool(row.map(|row| row.value.as_str()) != Some("off"))),
        None => row.map(|row| Value::String(row.value.clone())),
    };
    let index = position(&|option| same_value(option.value.as_ref(), value.as_ref()));
    CurrentChoice {
        index: Some(index.unwrap_or(0)),
        current: None,
    }
}

/// TS: settingsModel.ts choiceChange (`None` when the target already has that value).
pub fn choice_change(
    info: &SettingsInfo,
    id: SettingId,
    target: ConfigTargetKind,
    option: &ChoiceOption,
) -> Option<SettingChange> {
    let value = option.value.as_ref().map(or_bool_value);
    if let Some(flag) = app_flag_key(id) {
        // Agent signals not set stores either choice (on adds Codex); otherwise picking the current value changes nothing.
        let unchanged = if id == SettingId::AgentHooks {
            info.agent_hooks.map(StringOrBool::Bool) == option.value
        } else {
            Some(StringOrBool::Bool(app_flag(info, id))) == option.value
        };
        return (!unchanged).then(|| SettingChange {
            path: vec![flag.to_owned()],
            value,
        });
    }
    let layer = info_layer(&info.targets, target);
    if same_value(option.value.as_ref(), own_value(layer.as_ref(), id, None)) {
        return None;
    }
    Some(SettingChange {
        path: setting_path(id, None),
        value,
    })
}

/// TS: settingsModel.ts initialText.
pub fn initial_text(targets: &ConfigTargets, id: SettingId, target: ConfigTargetKind, entry: Option<&str>) -> String {
    let layer = info_layer(targets, target);
    match own_value(layer.as_ref(), id, entry) {
        Some(Value::String(own)) => own.clone(),
        _ => String::new(),
    }
}

/// TS: settingsModel.ts inheritedHint.
pub fn inherited_hint(
    info: &SettingsInfo,
    id: SettingId,
    target: ConfigTargetKind,
    entry: Option<&str>,
) -> Option<String> {
    if target == ConfigTargetKind::Repository {
        let global_layer = info_layer(&info.targets, ConfigTargetKind::Global);
        if let Some(Value::String(global)) = own_value(global_layer.as_ref(), id, entry) {
            return Some(format!("global: {global}"));
        }
        if id == SettingId::DevCommand
            && let Some(row) = info
                .rows
                .iter()
                .find(|row| row.key == "devCommand" && row.source == SettingSource::LegacyDevCommand)
            && let Some(StringOrBool::String(raw)) = &row.raw
        {
            return Some(format!("global: {raw} (legacy)"));
        }
    }
    match id {
        SettingId::DevCommand => Some(String::from("built-in: dev")),
        SettingId::WorktreeBranchName => Some(String::from("built-in: {name}")),
        _ => None,
    }
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
    if name.is_empty() {
        return Some(String::from("type a name first"));
    }
    if name.starts_with(' ') {
        return Some(String::from("can't start with a space"));
    }
    if !name.starts_with(|ch: char| ch.is_ascii_alphanumeric()) {
        return Some(String::from("must start with a letter or number"));
    }
    if let Some(bad) = name
        .chars()
        .find(|ch| !(ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '_' | '.' | '-')))
    {
        let shown = if bad == '\t' {
            String::from("a tab")
        } else if VISIBLE_CHAR.is_match(bad.encode_utf8(&mut [0; 4])) {
            format!("\"{bad}\"")
        } else {
            format!("U+{:04X}", u32::from(bad))
        };
        return Some(format!("can't contain {shown}"));
    }
    if name.ends_with(' ') {
        return Some(String::from("can't end with a space"));
    }
    let length = jsstr::js_len(name);
    if length > MAX_ACTION_NAME {
        return Some(format!("too long: {length} characters (up to {MAX_ACTION_NAME})"));
    }
    let mut actions = Map::new();
    actions.insert(name.to_owned(), Value::String(String::from("x")));
    let mut probe = Map::new();
    probe.insert(String::from("actions"), Value::Object(actions));
    match validate_project_config(&Value::Object(probe), None) {
        Ok(_) => None,
        Err(_) => Some(format!("{name} is a reserved name")),
    }
}

/// TS: settingsModel.ts actionCount.
pub fn action_count(targets: &ConfigTargets, kind: ConfigTargetKind) -> usize {
    let layer = info_layer(targets, kind);
    match own_value(layer.as_ref(), SettingId::Actions, None) {
        Some(Value::Object(actions)) => actions.len(),
        _ => 0,
    }
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
    let other = other_target(target);
    let full = (action_count(targets, target) >= MAX_ACTIONS).then(|| {
        format!(
            "{} already has {MAX_ACTIONS} actions, the most allowed: remove one (x) first",
            layer_short(target)
        )
    });
    if name.is_empty() {
        return NameCheck {
            error: full,
            note: None,
        };
    }
    let error = |error: String| NameCheck {
        error: Some(error),
        note: None,
    };
    let note = |note: String| NameCheck {
        error: None,
        note: Some(note),
    };
    if let Some(problem) = action_name_problem(name) {
        return error(problem);
    }
    if layer_sets(info_layer(targets, target).as_ref(), SettingId::Actions, Some(name)) {
        return note(format!("replaces the existing {name} action ({})", layer_short(target)));
    }
    if let Some(full) = full {
        return error(full);
    }
    if layer_sets(info_layer(targets, other).as_ref(), SettingId::Actions, Some(name)) {
        return note(if target == ConfigTargetKind::Repository {
            format!("overrides the global {name} action in this repo")
        } else {
            format!("adds a global {name} action; this repo's {name} action still wins here")
        });
    }
    note(format!("adds a new action to {}", layer_short(target)))
}

/// TS: settingsModel.ts commandProblem.
pub fn command_problem(text: &str) -> Option<String> {
    if jsstr::js_trim(text).is_empty() {
        return Some(String::from("type a command"));
    }
    if text.contains('\0') {
        return Some(String::from("can't contain a NUL character"));
    }
    let length = jsstr::js_len(text);
    if length > 8192 {
        return Some(format!("too long: {length} characters (up to 8192)"));
    }
    None
}

/// TS: settingsModel.ts textChange (`{change}` or `{error}`: the validation error to show while the input stays open).
pub fn text_change(id: SettingId, text: &str, entry: Option<&str>) -> std::result::Result<SettingChange, String> {
    let path = setting_path(id, entry);
    let blank = jsstr::js_trim(text).is_empty();
    if id == SettingId::Actions && blank {
        return Err(String::from("Type a command for the action"));
    }
    if matches!(id, SettingId::DevCommand | SettingId::SetupCommand) && blank {
        return Err(String::from("Type a command (x clears the setting instead)"));
    }
    let probe = path.iter().rev().fold(Value::String(text.to_owned()), |inner, key| {
        let mut object = Map::new();
        object.insert(key.clone(), inner);
        Value::Object(object)
    });
    if let Err(error) = validate_project_config(&probe, None) {
        return Err(error.to_string());
    }
    Ok(SettingChange {
        path,
        value: Some(Value::String(text.to_owned())),
    })
}

/// TS: settingsModel.ts initialLinks (`suggest` defaults to true).
pub fn initial_links(candidates: &[WorktreeCandidate], suggest: bool) -> IndexMap<String, bool> {
    candidates
        .iter()
        .map(|candidate| {
            let linked =
                candidate.configured.is_some() || (suggest && candidate.suggestion == CandidateSuggestion::Link);
            (candidate.path.clone(), linked)
        })
        .collect()
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
    let list = |layer: Option<&Layer>| -> Vec<String> { strings(own_value(layer, SettingId::WorktreeLinks, None)) };
    let own = list(target);
    let others: IndexSet<String> = list(other).into_iter().collect();
    let mut known: IndexMap<String, bool> = IndexMap::new();
    for candidate in candidates
        .iter()
        .filter(|candidate| candidate.configured != Some(CandidateConfigured::Files))
    {
        known.insert(
            candidate.path.clone(),
            links.get(&candidate.path).copied().unwrap_or(false),
        );
    }
    let unlinked = |entry: &String| known.get(entry) == Some(&false);
    let kept: Vec<String> = own.iter().filter(|entry| !unlinked(entry)).cloned().collect();
    let added: Vec<String> = known
        .iter()
        .filter(|(entry, linked)| **linked && !own.contains(entry) && !others.contains(*entry))
        .map(|(entry, _)| entry.clone())
        .collect();
    LinkSelection {
        symlink: kept.into_iter().chain(added.iter().cloned()).collect(),
        removed: own.iter().filter(|entry| unlinked(entry)).cloned().collect(),
        still_linked: others.iter().filter(|entry| unlinked(entry)).cloned().collect(),
        added,
    }
}

/// TS: settingsModel.ts trustNote.
pub fn trust_note(trust: Option<SaveTrust>) -> &'static str {
    match trust {
        Some(SaveTrust::Kept) => "still trusted",
        Some(SaveTrust::Created) => "created and trusted",
        Some(SaveTrust::Unreviewed) => "review required (the file had changes you haven't reviewed)",
        Some(SaveTrust::Hook) => "review required (the creation hook hasn't been reviewed)",
        None => "reviewed before anything from it runs",
    }
}

/// TS: settingsModel.ts savedMessage options.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SavedMessageOptions {
    pub cleared: bool,
    pub trust: Option<SaveTrust>,
}

/// TS: settingsModel.ts savedMessage.
pub fn saved_message(label: &str, target: ConfigTargetKind, options: SavedMessageOptions) -> String {
    let (verb, preposition) = if options.cleared {
        ("Cleared", "in")
    } else {
        ("Saved", "to")
    };
    format!(
        "{verb} {label} {preposition} {}{}",
        layer_short(target),
        repository_trust(target, options.trust)
    )
}

/// TS: settingsModel.ts actionSavedMessage.
pub fn action_saved_message(name: &str, target: ConfigTargetKind, trust: Option<SaveTrust>) -> String {
    format!(
        "Saved action {name} to {}{} · run it with e on a session",
        layer_short(target),
        repository_trust(target, trust)
    )
}

// Helpers -------------------------------------------------------------------------------------------------------

/// A character `actionNameProblem` quotes as itself (`/^[\p{L}\p{N}\p{P}\p{S}]$/u`); others are shown as `U+XXXX`.
static VISIBLE_CHAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\p{L}\p{N}\p{P}\p{S}]$").expect("valid regex"));

/// TS WORKSPACE_LABELS.
fn workspace_label(value: &str) -> Option<&'static str> {
    match value {
        "none" => Some("no worktree"),
        "new" => Some("new worktree"),
        "existing" => Some("existing worktree"),
        _ => None,
    }
}

/// The config.json key of a global-only setting (TS `APP_FLAG[id]`).
fn app_flag_key(id: SettingId) -> Option<&'static str> {
    APP_FLAG.iter().find(|(flag, _)| *flag == id).map(|(_, key)| *key)
}

/// TS appFlag: Agent signals are on unless switched off (unset: Claude only, see hooksEnabled); Notifications are
/// off unless switched on.
fn app_flag(info: &SettingsInfo, id: SettingId) -> bool {
    if id == SettingId::AgentHooks {
        info.agent_hooks != Some(false)
    } else {
        info.notifications
    }
}

/// TS storedFlag: the value config.json stores for the flag, `None` when it stores none (Notifications off counts as
/// none).
fn stored_flag(info: &SettingsInfo, id: SettingId) -> Option<bool> {
    if id == SettingId::AgentHooks {
        info.agent_hooks
    } else {
        info.notifications.then_some(true)
    }
}

fn layer_label(kind: ConfigTargetKind) -> &'static str {
    match kind {
        ConfigTargetKind::Global => "Global defaults are",
        ConfigTargetKind::Repository => "deckhand.json is",
    }
}

fn layer_short(kind: ConfigTargetKind) -> &'static str {
    match kind {
        ConfigTargetKind::Global => "global defaults",
        ConfigTargetKind::Repository => "this repo",
    }
}

/// ` · <trust note>` after a save to the repository file.
fn repository_trust(target: ConfigTargetKind, trust: Option<SaveTrust>) -> String {
    match target {
        ConfigTargetKind::Repository => format!(" · {}", trust_note(trust)),
        ConfigTargetKind::Global => String::new(),
    }
}

/// The explainSettings `source` of a layer's values.
fn source_of(kind: ConfigTargetKind) -> &'static str {
    match kind {
        ConfigTargetKind::Global => "global",
        ConfigTargetKind::Repository => "repo",
    }
}

fn target_document(targets: &ConfigTargets, kind: ConfigTargetKind) -> Option<&ProjectConfigDocument> {
    match kind {
        ConfigTargetKind::Global => targets.global.as_ref(),
        ConfigTargetKind::Repository => targets.repository.as_ref(),
    }
}

/// `JSON.parse(raw.replace(/^﻿/, ''))` with JS object semantics (key order, duplicate keys, number
/// normalisation); `None` when JSON.parse throws.
fn js_json_parse(raw: &str, surrogates: Surrogates) -> Option<Value> {
    let text = raw.strip_prefix('\u{FEFF}').unwrap_or(raw);
    if json_parse_message(text).is_some() {
        return None;
    }
    // The normalised text writes each lone surrogate as a `\udxxx` escape and every other non-control character raw.
    let normalized = format_config_json(text).ok()?;
    serde_json::from_str(&rewrite_lone_surrogates(&normalized, surrogates)).ok()
}

/// What `js_json_parse` does with the lone surrogates JSON.parse keeps but a Rust string cannot hold.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Surrogates {
    /// Each becomes U+FFFD (lossy: for reading the layer).
    Replace,
    /// Each becomes `MARK` + its 4 lowercase hex digits, and a raw `MARK` becomes `MARK` + `fdd0`: reversible by
    /// `unmark`, and keys that differ only in their lone surrogates stay distinct.
    Mark,
}

/// A noncharacter that introduces a carried lone surrogate (or, followed by `fdd0`, stands for itself).
const MARK: char = '\u{FDD0}';

/// JSON `text` with the `\uD800`-`\uDFFF` escapes that do not form a pair rewritten per `surrogates`.
fn rewrite_lone_surrogates(text: &str, surrogates: Surrogates) -> String {
    let chars: Vec<char> = text.chars().collect();
    let unit_at = |at: usize| -> Option<u32> {
        if chars.get(at) != Some(&'\\') || chars.get(at + 1) != Some(&'u') {
            return None;
        }
        chars
            .get(at + 2..at + 6)?
            .iter()
            .try_fold(0, |unit, digit| Some(unit * 16 + digit.to_digit(16)?))
    };
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(&ch) = chars.get(i) {
        if ch != '\\' {
            if ch == MARK && surrogates == Surrogates::Mark {
                out.push_str(&format!("{MARK}fdd0"));
            } else {
                out.push(ch);
            }
            i += 1;
            continue;
        }
        let pair = matches!(unit_at(i), Some(0xD800..=0xDBFF)) && matches!(unit_at(i + 6), Some(0xDC00..=0xDFFF));
        let (take, lone) = match unit_at(i) {
            _ if pair => (12, None),
            Some(unit @ 0xD800..=0xDFFF) => (6, Some(unit)),
            Some(_) => (6, None),
            None => (2, None),
        };
        match (lone, surrogates) {
            (Some(_), Surrogates::Replace) => out.push_str("\\ufffd"),
            (Some(unit), Surrogates::Mark) => out.push_str(&format!("{MARK}{unit:04x}")),
            (None, _) => out.extend(chars.iter().skip(i).take(take)),
        }
        i += take;
    }
    out
}

/// `text` with each raw `MARK` written as `MARK` + `fdd0` (for strings that join a marked layer from outside).
fn escape_mark(text: &str) -> String {
    text.replace(MARK, &format!("{MARK}fdd0"))
}

/// `text` with marks undone: `MARK` + `fdd0` is `MARK` again; `MARK` + a surrogate's digits becomes `lone(digits)`.
fn unmark(text: &str, lone: impl Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(MARK) {
        out.push_str(&rest[..at]);
        let after = &rest[at + MARK.len_utf8()..];
        // Marks are only ever followed by 4 ASCII hex digits.
        let digits = after.get(..4).unwrap_or(after);
        if digits == "fdd0" {
            out.push(MARK);
        } else {
            out.push_str(&lone(digits));
        }
        rest = &after[digits.len()..];
    }
    out.push_str(rest);
    out
}

/// `value` with `rewrite` applied to every string and object key (key order kept).
fn map_strings(value: &Value, rewrite: &impl Fn(&str) -> String) -> Value {
    match value {
        Value::String(text) => Value::String(rewrite(text)),
        Value::Array(items) => Value::Array(items.iter().map(|item| map_strings(item, rewrite)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (rewrite(key), map_strings(item, rewrite)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// TS valueAt: the value at `keys` through own properties of plain objects only.
fn value_at<'a, S: AsRef<str>>(layer: Option<&'a Layer>, keys: &[S]) -> Option<&'a Value> {
    let (last, parents) = keys.split_last()?;
    let mut current = layer?;
    for key in parents {
        current = current.get(key.as_ref())?.as_object()?;
    }
    current.get(last.as_ref())
}

/// The object at `keys` under `root` (`root` itself for no keys).
fn object_at_mut<'a>(root: &'a mut Layer, keys: &[String]) -> Option<&'a mut Layer> {
    let mut current = root;
    for key in keys {
        current = current.get_mut(key)?.as_object_mut()?;
    }
    Some(current)
}

/// A method every JS object inherits from Object.prototype (`parent[key]` is a function, not undefined).
fn inherited_function(key: &str) -> bool {
    matches!(
        key,
        "constructor"
            | "hasOwnProperty"
            | "isPrototypeOf"
            | "propertyIsEnumerable"
            | "toString"
            | "toLocaleString"
            | "valueOf"
            | "__defineGetter__"
            | "__defineSetter__"
            | "__lookupGetter__"
            | "__lookupSetter__"
    )
}

/// TS: `${change.path.slice(0, parents.length).join('.')} is not an object; ...`.
fn not_an_object(path: &[String], parents: usize) -> DhError {
    let shown: Vec<&str> = path.iter().take(parents).map(String::as_str).collect();
    DhError::msg(format!(
        "{} is not an object; press E to repair it first",
        shown.join(".")
    ))
}

/// `Object.keys(record(value))`.
fn record_keys(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Object(map)) => map.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

/// The string entries of an array (TS `strings`).
fn strings(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

/// JS `String(value)` for a JSON value.
fn js_string(value: &Value) -> String {
    match value {
        Value::Null => String::from("null"),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => JsNumber(number.as_f64().unwrap_or(f64::NAN)).to_js_string(),
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                item => js_string(item),
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => String::from("[object Object]"),
    }
}

fn or_bool_value(value: &StringOrBool) -> Value {
    match value {
        StringOrBool::String(text) => Value::String(text.clone()),
        StringOrBool::Bool(flag) => Value::Bool(*flag),
    }
}

/// `option.value === value` (both may be undefined).
fn same_value(option: Option<&StringOrBool>, value: Option<&Value>) -> bool {
    match (option, value) {
        (None, None) => true,
        (Some(StringOrBool::String(a)), Some(Value::String(b))) => a == b,
        (Some(StringOrBool::Bool(a)), Some(Value::Bool(b))) => a == b,
        _ => false,
    }
}

fn branch_from_label(value: &str, info: &SettingsInfo) -> String {
    match value {
        "default" => format!(
            "default branch ({})",
            info.default_branch.as_deref().unwrap_or("none found")
        ),
        "origin" => match info.origin_branch.as_deref() {
            Some(branch) if !branch.is_empty() => format!("origin/{branch}, fetched"),
            _ => String::from("origin (no remote found)"),
        },
        _ => String::from("current checkout"),
    }
}

/// TS BUILT_IN_VALUE.
fn built_in_value(id: SettingId) -> Option<Value> {
    match id {
        SettingId::DefaultAgent => Some(Value::from("claude")),
        SettingId::DefaultWorkspace => Some(Value::from("none")),
        SettingId::DevCommand => Some(Value::from("dev")),
        SettingId::WorktreeBranchFrom => Some(Value::from("current")),
        SettingId::WorktreeBranchName => Some(Value::from("{name}")),
        SettingId::WorktreeHook => Some(Value::Bool(true)),
        _ => None,
    }
}

/// TS settingsGrid `show`: a value for display (`text`, `full`, `template`; the other fields default).
fn show(info: &SettingsInfo, id: SettingId, value: &Value) -> GridCell {
    let text = js_string(value);
    match id {
        SettingId::DefaultWorkspace => GridCell {
            text: workspace_label(&text).map_or(text.clone(), str::to_owned),
            ..GridCell::default()
        },
        SettingId::WorktreeBranchFrom => GridCell {
            text: branch_from_label(&text, info),
            ..GridCell::default()
        },
        SettingId::WorktreeHook => GridCell {
            text: String::from(if *value == Value::Bool(false) { "off" } else { "on" }),
            ..GridCell::default()
        },
        SettingId::WorktreeLocation => {
            let preview = preview_location(&text, info.vars.as_ref());
            GridCell {
                text: preview.clone(),
                full: Some(preview),
                template: Some(text),
                ..GridCell::default()
            }
        }
        _ => GridCell {
            text: text.clone(),
            full: Some(text),
            ..GridCell::default()
        },
    }
}

/// TS BUILT_IN: what a built-in value (or a setting nothing sets) means in practice.
fn built_in_detail(id: SettingId) -> &'static str {
    match id {
        SettingId::DefaultAgent => "Nothing sets it, so new sessions (n) start on claude.",
        SettingId::DefaultWorkspace => "Nothing sets it, so n offers \"no worktree\" (this checkout).",
        SettingId::DevCommand => "No Dev command is set, so d runs a shell command named `dev`.",
        SettingId::SetupCommand => "No setup command: new worktrees start the agent right away.",
        SettingId::Actions => "No actions yet: named commands you run with e on a session.",
        SettingId::WorktreeLocation => "Nothing sets it: new worktrees go in Deckhand's own folder.",
        SettingId::WorktreeBranchFrom => "Nothing sets it: new branches start at the current checkout.",
        SettingId::WorktreeBranchName => "Nothing sets it: new branches are named after the session.",
        SettingId::WorktreeLinks => "Nothing linked: new worktrees start without untracked files.",
        SettingId::WorktreeHook => "On by default: a trusted create-worktree.sh makes worktrees.",
        SettingId::AgentHooks => "Off: attention (! and the session markers) comes from terminal activity only.",
        SettingId::Notifications => "Off: no desktop notifications.",
    }
}

/// TS appFlagDetail: what a global-only setting does when on (Notifications also says what Agent signals adds).
fn app_flag_detail(info: &SettingsInfo, id: SettingId) -> &'static str {
    if id == SettingId::AgentHooks && info.agent_hooks.is_none() {
        return "Not set: on for Claude (its state and what it asks) · on adds Codex, with its hooks set up";
    }
    if !app_flag(info, id) {
        return built_in_detail(id);
    }
    if id == SettingId::AgentHooks {
        return "new sessions report state and what they ask · Claude automatically, Codex via its own hooks";
    }
    if app_flag(info, SettingId::AgentHooks) {
        "a desktop notification when a session needs you or exits"
    } else {
        "a desktop notification when a session exits (Agent signals adds needs input / done)"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_config::{
        ExplainContext, UserSettings, explain_settings, parse_project_config, trust_project_config,
    };
    use deckhand_parity::{Case, Outcome, replay, tags};
    use deckhand_protocol::Ordered;
    use deckhand_protocol::data::{LoadedProject, ProjectConfig, RepositoryInfo};
    use serde_json::json;

    // Fixture arguments ---------------------------------------------------------------------------------------

    /// `Pick<SettingsInfo, 'targets'>`.
    #[derive(Deserialize)]
    struct WithTargets {
        targets: ConfigTargets,
    }

    /// `Pick<ProjectConfigDocument, 'raw' | 'exists' | 'kind'>`.
    #[derive(Deserialize)]
    struct DocumentArg {
        kind: ConfigTargetKind,
        raw: String,
        exists: bool,
    }

    #[derive(Deserialize)]
    struct RowArg {
        def: RowDefArg,
        cells: GridCells,
        note: Option<String>,
        warn: Option<bool>,
    }

    #[derive(Deserialize)]
    struct RowDefArg {
        id: SettingId,
    }

    #[derive(Deserialize, Default)]
    struct SavedOptionsArg {
        cleared: Option<bool>,
        trust: Option<SaveTrust>,
    }

    fn info_arg(case: &Case, index: usize) -> std::result::Result<SettingsInfo, String> {
        case.arg_as(index)
    }

    fn targets_arg(case: &Case, index: usize) -> std::result::Result<ConfigTargets, String> {
        Ok(case.arg_as::<WithTargets>(index)?.targets)
    }

    fn kind_arg(case: &Case, index: usize) -> std::result::Result<ConfigTargetKind, String> {
        case.arg_as(index)
    }

    fn id_arg(case: &Case, index: usize) -> std::result::Result<SettingId, String> {
        case.arg_as(index)
    }

    fn opt_str_arg(case: &Case, index: usize) -> std::result::Result<Option<String>, String> {
        if case.arg_is_undefined(index) {
            Ok(None)
        } else {
            Ok(Some(case.arg_str(index)?.to_owned()))
        }
    }

    fn layer_arg(case: &Case, index: usize) -> std::result::Result<Option<Layer>, String> {
        if case.arg_is_undefined(index) {
            Ok(None)
        } else {
            case.arg_as(index).map(Some)
        }
    }

    fn row_arg(case: &Case, index: usize) -> std::result::Result<GridRow, String> {
        let row: RowArg = case.arg_as(index)?;
        let def = SETTINGS
            .iter()
            .find(|def| def.id == row.def.id)
            .copied()
            .ok_or_else(|| String::from("unknown setting"))?;
        Ok(GridRow {
            def,
            cells: row.cells,
            note: row.note,
            warn: row.warn,
        })
    }

    /// WorktreeCandidates as the gen script builds them (`reason` may be missing).
    fn candidates_arg(case: &Case, index: usize) -> std::result::Result<Vec<WorktreeCandidate>, String> {
        let mut list = tags::untag(case.arg(index));
        if let Value::Array(items) = &mut list {
            for item in items {
                if let Value::Object(map) = item {
                    map.entry("reason").or_insert_with(|| Value::from(""));
                }
            }
        }
        serde_json::from_value(list).map_err(|error| error.to_string())
    }

    fn undefined_or<T: Serialize>(value: Option<T>) -> Value {
        match value {
            Some(value) => serde_json::to_value(value).unwrap_or(Value::Null),
            None => tags::undefined(),
        }
    }

    fn index(value: Option<usize>) -> i64 {
        value.map_or(-1, |value| value as i64)
    }

    // Fixture replays -----------------------------------------------------------------------------------------

    #[test]
    fn simple_parity() {
        replay(
            "P9",
            "settingsModel/isAppFlag",
            |case: &Case| -> std::result::Result<bool, String> { Ok(is_app_flag(id_arg(case, 0)?)) },
        );
        replay(
            "P9",
            "settingsModel/settingPath",
            |case: &Case| -> std::result::Result<Vec<String>, String> {
                Ok(setting_path(id_arg(case, 0)?, opt_str_arg(case, 1)?.as_deref()))
            },
        );
        replay(
            "P9",
            "settingsModel/otherTarget",
            |case: &Case| -> std::result::Result<ConfigTargetKind, String> { Ok(other_target(kind_arg(case, 0)?)) },
        );
        replay(
            "P9",
            "settingsModel/targetName",
            |case: &Case| -> std::result::Result<&str, String> { Ok(target_name(kind_arg(case, 0)?)) },
        );
        replay(
            "P9",
            "settingsModel/layerName",
            |case: &Case| -> std::result::Result<&str, String> { Ok(layer_name(kind_arg(case, 0)?)) },
        );
        replay(
            "P9",
            "settingsModel/homePath",
            |case: &Case| -> std::result::Result<String, String> {
                Ok(home_path(case.arg_str(0)?, opt_str_arg(case, 1)?.as_deref()))
            },
        );
        replay(
            "P9",
            "settingsModel/codexHookNote",
            |case: &Case| -> std::result::Result<String, String> {
                let status: CodexHookStatus = case.arg_as(0)?;
                Ok(codex_hook_note(&status, opt_str_arg(case, 1)?.as_deref()))
            },
        );
        replay(
            "P9",
            "settingsModel/previewLocation",
            |case: &Case| -> std::result::Result<String, String> {
                let vars: Option<TemplateVars> = if case.arg_is_undefined(1) {
                    None
                } else {
                    Some(case.arg_as(1)?)
                };
                Ok(preview_location(case.arg_str(0)?, vars.as_ref()))
            },
        );
    }

    #[test]
    fn layer_parity() {
        replay(
            "P9",
            "settingsModel/infoLayer",
            |case: &Case| -> std::result::Result<Value, String> {
                Ok(undefined_or(info_layer(&targets_arg(case, 0)?, kind_arg(case, 1)?)))
            },
        );
        replay(
            "P9",
            "settingsModel/layerSets",
            |case: &Case| -> std::result::Result<bool, String> {
                let layer = layer_arg(case, 0)?;
                Ok(layer_sets(
                    layer.as_ref(),
                    id_arg(case, 1)?,
                    opt_str_arg(case, 2)?.as_deref(),
                ))
            },
        );
        replay(
            "P9",
            "settingsModel/ownValue",
            |case: &Case| -> std::result::Result<Value, String> {
                let layer = layer_arg(case, 0)?;
                Ok(undefined_or(own_value(
                    layer.as_ref(),
                    id_arg(case, 1)?,
                    opt_str_arg(case, 2)?.as_deref(),
                )))
            },
        );
        replay("P9", "settingsModel/applyChange", |case: &Case| -> Outcome {
            let run = || -> std::result::Result<Result<String>, String> {
                let document: DocumentArg = case.arg_as(0)?;
                let document = ProjectConfigDocument {
                    kind: document.kind,
                    root: String::new(),
                    path: String::new(),
                    raw: document.raw,
                    revision: None,
                    exists: document.exists,
                };
                let change: SettingChange = case.arg_as(1)?;
                Ok(apply_change(&document, &change))
            };
            match run() {
                Ok(result) => result.into(),
                Err(error) => Outcome::throws(format!("bad args: {error}")),
            }
        });
    }

    #[test]
    fn grid_parity() {
        replay(
            "P9",
            "settingsModel/settingsGrid",
            |case: &Case| -> std::result::Result<Vec<GridRow>, String> { Ok(settings_grid(&info_arg(case, 0)?)) },
        );
        replay(
            "P9",
            "settingsModel/cellDetail",
            |case: &Case| -> std::result::Result<CellDetail, String> {
                Ok(cell_detail(&info_arg(case, 0)?, &row_arg(case, 1)?, kind_arg(case, 2)?))
            },
        );
        replay(
            "P9",
            "settingsModel/layerActions",
            |case: &Case| -> std::result::Result<LayerActions, String> {
                Ok(layer_actions(&info_arg(case, 0)?, kind_arg(case, 1)?))
            },
        );
        replay(
            "P9",
            "settingsModel/actionCount",
            |case: &Case| -> std::result::Result<usize, String> {
                Ok(action_count(&targets_arg(case, 0)?, kind_arg(case, 1)?))
            },
        );
        replay(
            "P9",
            "settingsModel/columnProblem",
            |case: &Case| -> std::result::Result<Value, String> {
                Ok(undefined_or(column_problem(&targets_arg(case, 0)?, kind_arg(case, 1)?)))
            },
        );
        replay(
            "P9",
            "settingsModel/initialColumn",
            |case: &Case| -> std::result::Result<ConfigTargetKind, String> {
                Ok(initial_column(&targets_arg(case, 0)?))
            },
        );
    }

    #[test]
    fn control_parity() {
        replay(
            "P9",
            "settingsModel/choiceOptions",
            |case: &Case| -> std::result::Result<Vec<ChoiceOption>, String> {
                Ok(choice_options(
                    &info_arg(case, 0)?,
                    id_arg(case, 1)?,
                    kind_arg(case, 2)?,
                ))
            },
        );
        replay(
            "P9",
            "settingsModel/currentChoice",
            |case: &Case| -> std::result::Result<Value, String> {
                let options: Vec<ChoiceOption> = case.arg_as(3)?;
                let choice = current_choice(&info_arg(case, 0)?, id_arg(case, 1)?, kind_arg(case, 2)?, &options);
                Ok(json!({"index": index(choice.index), "current": index(choice.current)}))
            },
        );
        replay(
            "P9",
            "settingsModel/choiceChange",
            |case: &Case| -> std::result::Result<Value, String> {
                let option: ChoiceOption = case.arg_as(3)?;
                Ok(undefined_or(choice_change(
                    &info_arg(case, 0)?,
                    id_arg(case, 1)?,
                    kind_arg(case, 2)?,
                    &option,
                )))
            },
        );
        replay(
            "P9",
            "settingsModel/initialText",
            |case: &Case| -> std::result::Result<String, String> {
                Ok(initial_text(
                    &targets_arg(case, 0)?,
                    id_arg(case, 1)?,
                    kind_arg(case, 2)?,
                    opt_str_arg(case, 3)?.as_deref(),
                ))
            },
        );
        replay(
            "P9",
            "settingsModel/inheritedHint",
            |case: &Case| -> std::result::Result<Value, String> {
                Ok(undefined_or(inherited_hint(
                    &info_arg(case, 0)?,
                    id_arg(case, 1)?,
                    kind_arg(case, 2)?,
                    opt_str_arg(case, 3)?.as_deref(),
                )))
            },
        );
    }

    #[test]
    fn action_parity() {
        replay(
            "P9",
            "settingsModel/actionNameProblem",
            |case: &Case| -> std::result::Result<Value, String> {
                Ok(undefined_or(action_name_problem(case.arg_str(0)?)))
            },
        );
        replay(
            "P9",
            "settingsModel/actionNameCheck",
            |case: &Case| -> std::result::Result<NameCheck, String> {
                Ok(action_name_check(
                    &targets_arg(case, 0)?,
                    kind_arg(case, 1)?,
                    case.arg_str(2)?,
                ))
            },
        );
        replay(
            "P9",
            "settingsModel/commandProblem",
            |case: &Case| -> std::result::Result<Value, String> { Ok(undefined_or(command_problem(case.arg_str(0)?))) },
        );
        replay(
            "P9",
            "settingsModel/textChange",
            |case: &Case| -> std::result::Result<Value, String> {
                Ok(
                    match text_change(id_arg(case, 0)?, case.arg_str(1)?, opt_str_arg(case, 2)?.as_deref()) {
                        Ok(change) => json!({"change": change}),
                        Err(error) => json!({"error": error}),
                    },
                )
            },
        );
    }

    #[test]
    fn link_and_message_parity() {
        replay(
            "P9",
            "settingsModel/initialLinks",
            |case: &Case| -> std::result::Result<IndexMap<String, bool>, String> {
                let suggest = if case.arg_is_undefined(1) {
                    true
                } else {
                    case.arg_bool(1)?
                };
                Ok(initial_links(&candidates_arg(case, 0)?, suggest))
            },
        );
        replay(
            "P9",
            "settingsModel/linkSelection",
            |case: &Case| -> std::result::Result<LinkSelection, String> {
                let (target, other) = (layer_arg(case, 0)?, layer_arg(case, 1)?);
                let links: IndexMap<String, bool> = case.arg_as(3)?;
                Ok(link_selection(
                    target.as_ref(),
                    other.as_ref(),
                    &candidates_arg(case, 2)?,
                    &links,
                ))
            },
        );
        replay(
            "P9",
            "settingsModel/trustNote",
            |case: &Case| -> std::result::Result<&str, String> {
                let trust: Option<SaveTrust> = if case.arg_is_undefined(0) {
                    None
                } else {
                    Some(case.arg_as(0)?)
                };
                Ok(trust_note(trust))
            },
        );
        replay(
            "P9",
            "settingsModel/savedMessage",
            |case: &Case| -> std::result::Result<String, String> {
                let options: SavedOptionsArg = if case.arg_is_undefined(2) {
                    SavedOptionsArg::default()
                } else {
                    case.arg_as(2)?
                };
                let options = SavedMessageOptions {
                    cleared: options.cleared.unwrap_or(false),
                    trust: options.trust,
                };
                Ok(saved_message(case.arg_str(0)?, kind_arg(case, 1)?, options))
            },
        );
        replay(
            "P9",
            "settingsModel/actionSavedMessage",
            |case: &Case| -> std::result::Result<String, String> {
                let trust: Option<SaveTrust> = if case.arg_is_undefined(2) {
                    None
                } else {
                    Some(case.arg_as(2)?)
                };
                Ok(action_saved_message(case.arg_str(0)?, kind_arg(case, 1)?, trust))
            },
        );
    }

    // Ported TS tests -----------------------------------------------------------------------------------------

    use ConfigTargetKind::{Global, Repository};

    fn vars() -> TemplateVars {
        TemplateVars {
            name: String::from("<name>"),
            repo: String::from("mono"),
            repo_parent: String::from("/dev"),
            repo_root: String::from("/dev/mono"),
            home: String::from("/home/me"),
        }
    }

    /// A SettingsInfo as the daemon builds it: explainSettings rows plus both documents as written.
    fn settings_info(
        repo_raw: Option<&str>,
        defaults: Option<Value>,
        trusted: bool,
        legacy_dev: Option<&str>,
    ) -> SettingsInfo {
        let config = match repo_raw {
            Some(raw) => parse_project_config(raw, None).expect("valid deckhand.json"),
            None => Ordered::new(ProjectConfig::default()),
        };
        let project = LoadedProject {
            root: String::from("/dev/mono"),
            trust_root: String::from("/dev/mono"),
            path: Some(String::from("/dev/mono/deckhand.json")),
            config,
            exists: repo_raw.is_some(),
            fingerprint: String::from("f"),
            creation_hook: None,
            disabled_hook: None,
        };
        let mut user = UserSettings {
            defaults: defaults.clone(),
            dev_command: legacy_dev.map(str::to_owned),
            ..UserSettings::default()
        };
        if trusted {
            user.trusted_projects = Some(trust_project_config("/dev/mono", "f", user.trusted_projects.as_ref()));
        }
        let context = ExplainContext {
            vars: Some(vars()),
            user: Some(String::from("me")),
            config_dir: String::from("/state"),
            location_config_dir: Some(String::from("/state")),
            ..ExplainContext::default()
        };
        let document = |kind: ConfigTargetKind, raw: Option<String>| ProjectConfigDocument {
            kind,
            root: String::from("/r"),
            path: String::from(if kind == Global {
                "/home/me/.deckhand/config.json"
            } else {
                "/dev/mono/deckhand.json"
            }),
            revision: raw.as_ref().map(|_| "x".repeat(64)),
            exists: raw.is_some(),
            raw: raw.unwrap_or_else(|| String::from("{}")),
        };
        let state = match (repo_raw, trusted) {
            (None, _) => RepositoryState::Absent,
            (Some(_), true) => RepositoryState::Trusted,
            (Some(_), false) => RepositoryState::Untrusted,
        };
        SettingsInfo {
            cwd: String::from("/dev/mono"),
            repo: String::from("mono"),
            repository: RepositoryInfo {
                state,
                path: Some(String::from("/dev/mono/deckhand.json")),
                error: None,
            },
            needs_review: !trusted && repo_raw.is_some(),
            global_error: None,
            rows: explain_settings(Some(&project), &user, &context).rows,
            targets: ConfigTargets {
                global: Some(document(Global, defaults.map(|defaults| defaults.to_string()))),
                global_error: None,
                repository: Some(document(Repository, repo_raw.map(str::to_owned))),
                repository_error: None,
            },
            hook_file: None,
            vars: Some(vars()),
            default_location: String::from("/state/worktrees/<name>"),
            inside_ignored: false,
            default_branch: None,
            origin_branch: None,
            user: String::from("me"),
            agent_hooks: None,
            notifications: false,
            codex_hooks: None,
        }
    }

    const REPO: &str = r#"{"devCommand":"npm run dev","actions":{"test":"npm test"},"worktree":{"location":"{repoParent}/worktrees/{name}","files":{"backend/.env":"{repoParent}/b.env"}}}"#;

    fn global() -> Value {
        json!({"defaultAgent": "pi", "worktree": {"symlink": ["shared"], "branchFrom": "origin"}})
    }

    fn with(mut base: Value, key: &str, value: Value) -> Value {
        if let Value::Object(map) = &mut base {
            map.insert(key.to_owned(), value);
        }
        base
    }

    fn row(info: &SettingsInfo, id: SettingId) -> GridRow {
        settings_grid(info)
            .into_iter()
            .find(|row| row.def.id == id)
            .expect("row")
    }

    fn cell(info: &SettingsInfo, id: SettingId, kind: ConfigTargetKind) -> Value {
        serde_json::to_value(row(info, id).cells.get(kind)).expect("cell")
    }

    fn detail(info: &SettingsInfo, id: SettingId, kind: ConfigTargetKind) -> CellDetail {
        cell_detail(info, &row(info, id), kind)
    }

    fn parse(raw: &str) -> Value {
        serde_json::from_str(raw).expect("json")
    }

    fn opt(label: &str, value: Option<StringOrBool>) -> ChoiceOption {
        ChoiceOption {
            label: label.to_owned(),
            value,
            ..ChoiceOption::default()
        }
    }

    fn text_value(value: &str) -> Option<StringOrBool> {
        Some(StringOrBool::String(value.to_owned()))
    }

    // covers: B-P9-01 settings grid: each cell is that layer's own value; the one in effect (explainSettings ≡ resolveSettings) is marked, an untrusted repository value needs trust, built-ins show in the global column
    #[test]
    fn grid_cells_and_effect() {
        let info = settings_info(Some(REPO), Some(global()), false, None);
        let labels: Vec<String> = settings_grid(&info)
            .iter()
            .map(|row| {
                format!(
                    "{}/{}",
                    serde_json::to_value(row.def.section).unwrap().as_str().unwrap(),
                    row.def.label
                )
            })
            .collect();
        assert_eq!(
            labels,
            [
                "General/Default agent",
                "General/Default workspace",
                "Commands/Dev command",
                "Commands/Setup command",
                "Commands/Actions",
                "Worktrees/Location",
                "Worktrees/Branch from",
                "Worktrees/Branch name",
                "Worktrees/Linked items",
                "Worktrees/Creation hook",
                "Agents/Agent signals",
                "Agents/Notifications",
            ]
        );
        use SettingId::*;
        // Untrusted: the repository's Dev command sits in its cell (needs trust) while the built-in dev applies.
        assert_eq!(
            cell(&info, DevCommand, Repository),
            json!({"text": "npm run dev", "full": "npm run dev", "set": true, "effective": false, "needsTrust": true})
        );
        assert_eq!(
            cell(&info, DevCommand, Global),
            json!({"text": "dev", "full": "dev", "set": false, "effective": true, "builtIn": true})
        );
        assert_eq!(
            cell(&info, DefaultAgent, Global),
            json!({"text": "pi", "full": "pi", "set": true, "effective": true})
        );
        assert_eq!(
            cell(&info, DefaultAgent, Repository),
            json!({"text": "—", "set": false, "effective": false})
        );
        let setup = cell(&info, SetupCommand, Global);
        assert_eq!(
            (setup["text"].clone(), setup["effective"].clone()),
            (json!("—"), json!(false))
        );
        assert_eq!(
            cell(&info, WorktreeLocation, Repository),
            json!({"text": "/dev/worktrees/<name>", "full": "/dev/worktrees/<name>", "template": "{repoParent}/worktrees/{name}", "set": true, "effective": false, "needsTrust": true})
        );
        let location = cell(&info, WorktreeLocation, Global);
        assert_eq!(
            (location["text"].clone(), location["builtIn"].clone()),
            (json!("/state/worktrees/<name>"), json!(true))
        );
        let branch_from = cell(&info, WorktreeBranchFrom, Global);
        assert_eq!(
            (branch_from["text"].clone(), branch_from["effective"].clone()),
            (json!("origin (no remote found)"), json!(true))
        );
        // Lists: a compact summary per layer (action names, a count of linked items).
        let actions = cell(&info, Actions, Repository);
        assert_eq!(
            (
                actions["text"].clone(),
                actions["needsTrust"].clone(),
                cell(&info, Actions, Global)["text"].clone()
            ),
            (json!("test"), json!(true), json!("—"))
        );
        let (links_global, links_repo) = (
            cell(&info, WorktreeLinks, Global),
            cell(&info, WorktreeLinks, Repository),
        );
        assert_eq!(
            [
                links_global["text"].clone(),
                links_global["effective"].clone(),
                links_repo["text"].clone(),
                links_repo["full"].clone()
            ],
            [json!("1 item"), json!(true), json!("1 item"), json!("backend/.env")]
        );
        // The details line names the layer and how it relates to the other one, or what the built-in means.
        assert_eq!(
            serde_json::to_value(detail(&info, DevCommand, Repository)).unwrap(),
            json!({"head": "This repo", "layer": "repository", "relation": "Applies once trusted — you'll be asked the first time it runs (or press T)", "warn": true})
        );
        assert_eq!(
            serde_json::to_value(detail(&info, DevCommand, Global)).unwrap(),
            json!({"head": "Built-in default", "relation": "No Dev command is set, so d runs a shell command named `dev`."})
        );
        assert_eq!(detail(&info, DefaultAgent, Global).relation, "inherited by this repo");
        assert_eq!(
            detail(&info, DefaultAgent, Repository).relation,
            "not set: inherits global"
        );
        assert_eq!(detail(&info, DevCommand, Global).head, "Built-in default");

        // Trusted: the repository values apply; a global value they replace is overridden (dimmed), lists merge.
        let trusted_global = with(global(), "devCommand", json!("printf global"));
        let trusted = settings_info(Some(REPO), Some(trusted_global.clone()), true, None);
        assert_eq!(
            [
                cell(&trusted, DevCommand, Repository)["effective"].clone(),
                cell(&trusted, DevCommand, Global)["effective"].clone(),
                cell(&trusted, DevCommand, Global)["text"].clone()
            ],
            [json!(true), json!(false), json!("printf global")]
        );
        assert_eq!(detail(&trusted, DevCommand, Repository).relation, "overrides global");
        assert_eq!(detail(&trusted, DevCommand, Global).relation, "overridden by this repo");
        assert_eq!(
            [
                cell(&trusted, WorktreeLocation, Global)["text"].clone(),
                cell(&trusted, WorktreeLocation, Repository)["effective"].clone()
            ],
            [json!("—"), json!(true)]
        );
        assert_eq!(
            [
                cell(&trusted, WorktreeLinks, Global)["effective"].clone(),
                cell(&trusted, WorktreeLinks, Repository)["effective"].clone()
            ],
            [json!(true), json!(true)]
        );
        // Suggestions (and hook: false) apply untrusted; the legacy dev_command shows in the global column.
        let suggestion = settings_info(
            Some(r#"{"defaultAgent":"codex","worktree":{"hook":false}}"#),
            Some(global()),
            false,
            None,
        );
        let agent = cell(&suggestion, DefaultAgent, Repository);
        assert_eq!(
            [
                agent["effective"].clone(),
                agent.get("needsTrust").cloned().unwrap_or(Value::Null),
                cell(&suggestion, DefaultAgent, Global)["effective"].clone()
            ],
            [json!(true), Value::Null, json!(false)]
        );
        assert_eq!(
            detail(&suggestion, DefaultAgent, Repository).relation,
            "overrides global, untrusted too: it only preselects n"
        );
        let hook = cell(&suggestion, WorktreeHook, Repository);
        assert_eq!(
            [hook["text"].clone(), hook["effective"].clone()],
            [json!("off"), json!(true)]
        );
        let legacy = settings_info(Some("{}"), None, false, Some("make dev"));
        assert_eq!(
            cell(&legacy, DevCommand, Global),
            json!({"text": "make dev", "full": "make dev", "set": false, "effective": true, "legacy": true})
        );

        // Clearing a repository value: the global one takes over.
        let raw = apply_change(
            trusted.targets.repository.as_ref().unwrap(),
            &SettingChange {
                path: vec![String::from("devCommand")],
                value: None,
            },
        )
        .unwrap();
        let cleared = settings_info(Some(&raw), Some(trusted_global), true, None);
        assert_eq!(
            [
                cell(&cleared, DevCommand, Repository)["text"].clone(),
                cell(&cleared, DevCommand, Global)["effective"].clone()
            ],
            [json!("—"), json!(true)]
        );
    }

    // covers: B-P9-02 Agent signals and Notifications are global only (config.json flags); Agent signals warns when Codex's hooks do not call this Deckhand
    #[test]
    fn global_only_flags() {
        // Detection (codexHookCommands / codexHookState) is settingsInfo.ts, ported and tested with I7.
        use SettingId::*;
        // Agent signals not set: on (built in) for Claude; either choice is stored (on adds Codex).
        let unset = settings_info(Some(REPO), Some(global()), false, None);
        assert_eq!(
            serde_json::to_value(row(&unset, AgentHooks).cells).unwrap(),
            json!({"global": {"text": "on", "set": false, "effective": true, "builtIn": true}, "repository": {"text": "global only", "set": false, "effective": false, "globalOnly": true}})
        );
        assert!(
            detail(&unset, AgentHooks, Global)
                .relation
                .starts_with("Not set: on for Claude")
        );
        assert!(
            detail(&unset, AgentHooks, Repository)
                .relation
                .starts_with("global only")
        );
        let options = choice_options(&unset, AgentHooks, Global);
        assert_eq!(
            current_choice(&unset, AgentHooks, Global, &options),
            CurrentChoice {
                index: Some(0),
                current: None
            }
        );
        assert_eq!(
            choice_change(&unset, AgentHooks, Global, &opt("on", Some(StringOrBool::Bool(true)))),
            Some(SettingChange {
                path: vec![String::from("agent_hooks")],
                value: Some(json!(true))
            })
        );
        assert_eq!(
            choice_change(&unset, AgentHooks, Global, &opt("off", Some(StringOrBool::Bool(false)))),
            Some(SettingChange {
                path: vec![String::from("agent_hooks")],
                value: Some(json!(false))
            })
        );
        assert_eq!(
            choice_change(
                &unset,
                Notifications,
                Global,
                &opt("off", Some(StringOrBool::Bool(false)))
            ),
            None
        );
        // Switched off: stored, so it is no built-in.
        let off = SettingsInfo {
            agent_hooks: Some(false),
            ..unset.clone()
        };
        assert_eq!(
            serde_json::to_value(&row(&off, AgentHooks).cells.global).unwrap(),
            json!({"text": "off", "set": true, "effective": true})
        );
        assert!(detail(&off, AgentHooks, Global).relation.starts_with("Off: attention"));
        let options = choice_options(&off, AgentHooks, Global);
        assert_eq!(
            current_choice(&off, AgentHooks, Global, &options),
            CurrentChoice {
                index: Some(1),
                current: Some(1)
            }
        );
        assert_eq!(
            choice_change(&off, AgentHooks, Global, &opt("off", Some(StringOrBool::Bool(false)))),
            None
        );

        let codex = CodexHookStatus {
            state: CodexHookState::Missing,
            file: String::from("/home/me/.codex/hooks.json"),
            file_exists: false,
            command: String::from("deckhand hooks codex"),
        };
        let on = SettingsInfo {
            agent_hooks: Some(true),
            codex_hooks: Some(codex.clone()),
            ..off.clone()
        };
        assert_eq!(
            serde_json::to_value(&row(&on, AgentHooks).cells.global).unwrap(),
            json!({"text": "on", "set": true, "effective": true, "warning": "Codex"})
        );
        let on_row = row(&on, AgentHooks);
        assert_eq!(
            (on_row.note.as_deref(), on_row.warn),
            (
                Some("Codex hooks not set up: deckhand hooks codex > ~/.codex/hooks.json, then /hooks in Codex"),
                Some(true)
            )
        );
        assert_eq!(
            codex_hook_note(
                &CodexHookStatus {
                    file_exists: true,
                    ..codex
                },
                Some("/home/me")
            ),
            "Codex hooks not set up: deckhand hooks codex → merge into ~/.codex/hooks.json, then /hooks in Codex"
        );
        // Notifications alone cover exits; Agent signals adds needs input / done.
        let notify = SettingsInfo {
            notifications: true,
            ..off.clone()
        };
        assert!(
            cell_detail(&notify, &row(&off, Notifications), Global)
                .relation
                .contains("exits (Agent signals adds")
        );
    }

    // covers: B-P9-03 the grid opens on This repo when there is a repository layer, else Global; each layer's Actions list shows the other layer's as context
    #[test]
    fn initial_column_and_layer_actions() {
        let info = settings_info(Some(REPO), Some(global()), false, None);
        assert_eq!(initial_column(&info.targets), Repository);
        assert_eq!(column_problem(&info.targets, Global), None);
        // Outside a repository (or in a bare one) only global defaults exist.
        let bare = SettingsInfo {
            targets: ConfigTargets {
                global: info.targets.global.clone(),
                repository_error: Some(String::from("Bare repositories have no main checkout")),
                ..ConfigTargets::default()
            },
            ..info.clone()
        };
        assert_eq!(initial_column(&bare.targets), Global);
        assert!(
            column_problem(&bare.targets, Repository)
                .unwrap_or_default()
                .contains("Bare repositories")
        );
        assert!(
            cell_detail(&bare, &settings_grid(&bare)[2], Repository)
                .relation
                .contains("No repository layer here")
        );
        // A repository file that does not exist yet is created with just the edited key.
        let absent = settings_info(None, None, false, None);
        assert_eq!(initial_column(&absent.targets), Repository);
        assert_eq!(
            apply_change(
                absent.targets.repository.as_ref().unwrap(),
                &SettingChange {
                    path: vec![String::from("defaultAgent")],
                    value: Some(json!("codex"))
                }
            )
            .unwrap(),
            "{\n  \"defaultAgent\": \"codex\"\n}\n"
        );
        let actions = settings_info(
            Some(r#"{"actions":{"test":"npm test","lint":"npm run lint"}}"#),
            Some(json!({"actions": {"test": "npm t", "fmt": "npm run fmt"}})),
            false,
            None,
        );
        assert_eq!(
            serde_json::to_value(layer_actions(&actions, Repository)).unwrap(),
            json!({
                "own": [{"name": "test", "command": "npm test", "status": "⚠ needs trust", "needsTrust": true}, {"name": "lint", "command": "npm run lint", "status": "⚠ needs trust", "needsTrust": true}],
                "context": [{"name": "fmt", "command": "npm run fmt", "status": "global"}],
            })
        );
        assert_eq!(
            serde_json::to_value(layer_actions(&actions, Global)).unwrap(),
            json!({
                "own": [{"name": "test", "command": "npm t", "status": "this repo overrides it"}, {"name": "fmt", "command": "npm run fmt"}],
                "context": [{"name": "lint", "command": "npm run lint", "status": "this repo · ⚠ needs trust"}],
            })
        );
    }

    // covers: B-P9-04 edits become one key in one layer, preserving every other key and its place; invalid values are rejected
    #[test]
    fn edits_preserve_other_keys() {
        use SettingId::*;
        let info = settings_info(Some(REPO), Some(global()), false, None);
        let repository = info.targets.repository.clone().unwrap();
        let global_doc = info.targets.global.clone().unwrap();
        let keys = |raw: &str| -> Vec<String> { parse(raw).as_object().unwrap().keys().cloned().collect() };
        // Choice.
        let agent = choice_options(&info, DefaultAgent, Repository);
        assert_eq!(
            agent.iter().map(|option| option.label.as_str()).collect::<Vec<_>>(),
            ["claude", "pi", "codex"]
        );
        // In effect: global pi; the repo stores none.
        assert_eq!(
            current_choice(&info, DefaultAgent, Repository, &agent),
            CurrentChoice {
                index: Some(1),
                current: None
            }
        );
        assert_eq!(
            current_choice(&info, DefaultAgent, Global, &agent),
            CurrentChoice {
                index: Some(1),
                current: Some(1)
            }
        );
        let codex = choice_change(&info, DefaultAgent, Repository, &agent[2]).unwrap();
        assert_eq!(
            parse(&apply_change(&repository, &codex).unwrap()),
            with(parse(REPO), "defaultAgent", json!("codex"))
        );
        assert_eq!(
            keys(&apply_change(&repository, &codex).unwrap()),
            ["devCommand", "actions", "worktree", "defaultAgent"]
        );
        // Global already has pi.
        assert_eq!(choice_change(&info, DefaultAgent, Global, &agent[1]), None);
        assert_eq!(
            choice_options(&info, WorktreeBranchFrom, Global)
                .into_iter()
                .map(|option| option.value)
                .collect::<Vec<_>>(),
            [text_value("current"), text_value("default"), text_value("origin")]
        );
        // Hook on/off.
        let hook = choice_options(&info, WorktreeHook, Global);
        assert_eq!(
            hook.iter()
                .map(|option| (option.label.as_str(), option.detail.as_deref()))
                .collect::<Vec<_>>(),
            [
                ("on", Some("no create-worktree.sh found")),
                ("off", Some("Deckhand creates worktrees itself"))
            ]
        );
        let change = choice_change(&info, WorktreeHook, Global, &hook[1]).unwrap();
        assert_eq!(
            parse(&apply_change(&global_doc, &change).unwrap()),
            json!({"defaultAgent": "pi", "worktree": {"symlink": ["shared"], "branchFrom": "origin", "hook": false}})
        );
        // Location presets: the repository's own template is preselected; the first option removes it, custom… types one.
        let location = choice_options(&info, WorktreeLocation, Repository);
        assert_eq!(
            location
                .iter()
                .map(|option| (option.label.as_str(), option.value.clone()))
                .collect::<Vec<_>>(),
            [
                ("Deckhand default", None),
                ("next to repo", text_value("{repoParent}/worktrees/{name}")),
                ("inside repo", text_value("{repoRoot}/.worktrees/{name}")),
                ("custom…", None),
            ]
        );
        assert_eq!(
            [
                location[0].detail.as_deref(),
                location[2].detail.as_deref(),
                location[2].warning.as_deref()
            ],
            [
                Some("/state/worktrees/<name>"),
                Some("/dev/mono/.worktrees/<name>"),
                Some("⚠ .worktrees/ is not gitignored — worktrees would show up as untracked files"),
            ]
        );
        assert_eq!(
            current_choice(&info, WorktreeLocation, Repository, &location),
            CurrentChoice {
                index: Some(1),
                current: Some(1)
            }
        );
        let global_location = choice_options(&info, WorktreeLocation, Global);
        assert_eq!(
            current_choice(&info, WorktreeLocation, Global, &global_location),
            CurrentChoice {
                index: Some(0),
                current: Some(0)
            }
        );
        let change = choice_change(&info, WorktreeLocation, Repository, &location[0]).unwrap();
        assert_eq!(
            parse(&apply_change(&repository, &change).unwrap())["worktree"],
            json!({"files": {"backend/.env": "{repoParent}/b.env"}})
        );
        assert_eq!(
            initial_text(&info.targets, WorktreeLocation, Repository, None),
            "{repoParent}/worktrees/{name}"
        );
        assert!(
            text_change(WorktreeLocation, "~/wt/{repo}", None)
                .unwrap_err()
                .contains("must contain {name}")
        );
        assert_eq!(
            text_change(WorktreeLocation, "~/wt/{repo}/{name}", None),
            Ok(SettingChange {
                path: vec![String::from("worktree"), String::from("location")],
                value: Some(json!("~/wt/{repo}/{name}")),
            })
        );
        // Text with validation errors kept for the open input.
        // A text input starts from the layer's own value, empty when it stores none (the inherited value is a hint).
        assert_eq!(initial_text(&info.targets, WorktreeBranchName, Global, None), "");
        assert_eq!(
            inherited_hint(&info, WorktreeBranchName, Global, None).as_deref(),
            Some("built-in: {name}")
        );
        assert_eq!(initial_text(&info.targets, DevCommand, Repository, None), "npm run dev");
        assert_eq!(initial_text(&info.targets, DevCommand, Global, None), "");
        assert_eq!(
            inherited_hint(&info, DevCommand, Global, None).as_deref(),
            Some("built-in: dev")
        );
        let make_dev = settings_info(Some(REPO), Some(json!({"devCommand": "make dev"})), false, None);
        assert_eq!(
            inherited_hint(&make_dev, DevCommand, Repository, None).as_deref(),
            Some("global: make dev")
        );
        for bad in ["feat/x", "bad name/{name}", "{name}..x"] {
            assert!(text_change(WorktreeBranchName, bad, None).is_err(), "{bad}");
        }
        assert!(text_change(DevCommand, "  ", None).unwrap_err().contains("x clears"));
        assert_eq!(
            text_change(Actions, "", Some("lint")),
            Err(String::from("Type a command for the action"))
        );
        let dev = text_change(DevCommand, "printf global", None).unwrap();
        assert_eq!(
            parse(&apply_change(&global_doc, &dev).unwrap()),
            with(global(), "devCommand", json!("printf global"))
        );
        // Actions: add (name, then command), edit, remove; an emptied actions object is dropped.
        let long = "a".repeat(49);
        for (name, problem) in [
            ("", "type a name first"),
            (" lint", "can't start with a space"),
            ("-lint", "must start with a letter or number"),
            ("bad/name", "can't contain \"/\""),
            ("lint ", "can't end with a space"),
            (long.as_str(), "too long: 49 characters (up to 48)"),
            ("constructor", "constructor is a reserved name"),
        ] {
            assert_eq!(action_name_problem(name).as_deref(), Some(problem), "{name}");
        }
        let max = "a".repeat(48);
        for name in ["test", "lint frontend", "db.migrate", max.as_str()] {
            assert_eq!(action_name_problem(name), None, "{name}");
        }
        let added = apply_change(
            &repository,
            &text_change(Actions, "npm run lint", Some("lint")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            parse(&added)["actions"],
            json!({"test": "npm test", "lint": "npm run lint"})
        );
        let edit = SettingChange {
            path: vec![String::from("actions"), String::from("test")],
            value: Some(json!("pytest")),
        };
        assert_eq!(
            parse(&apply_change(&repository, &edit).unwrap())["actions"],
            json!({"test": "pytest"})
        );
        let remove = SettingChange {
            path: vec![String::from("actions"), String::from("test")],
            value: None,
        };
        assert_eq!(
            keys(&apply_change(&repository, &remove).unwrap()),
            ["devCommand", "worktree"]
        );
        // Saving re-validates the whole layer and refuses to edit through malformed JSON.
        let broken = ProjectConfigDocument {
            raw: String::from("{"),
            ..repository.clone()
        };
        assert!(
            apply_change(&broken, &codex)
                .unwrap_err()
                .to_string()
                .contains("press E to repair")
        );
        let array = ProjectConfigDocument {
            raw: String::from(r#"{"worktree":[]}"#),
            ..repository.clone()
        };
        let hook_off = SettingChange {
            path: vec![String::from("worktree"), String::from("hook")],
            value: Some(json!(false)),
        };
        assert!(
            apply_change(&array, &hook_off)
                .unwrap_err()
                .to_string()
                .contains("not an object")
        );
    }

    // Review R #1: JSON.parse keeps lone surrogates and JSON.stringify writes them back as `\udxxx`, so saving one
    // setting leaves every other key's lone-surrogate escapes (in values and keys) byte-for-byte as they were.
    #[test]
    fn apply_change_keeps_lone_surrogate_escapes() {
        let document = |raw: &str| ProjectConfigDocument {
            kind: ConfigTargetKind::Repository,
            root: String::from("/repo"),
            path: String::from("/repo/deckhand.json"),
            raw: raw.to_owned(),
            revision: None,
            exists: true,
        };
        let agent = SettingChange {
            path: vec![String::from("defaultAgent")],
            value: Some(json!("pi")),
        };
        assert_eq!(
            apply_change(&document(r#"{"devCommand":"a\ud800b"}"#), &agent).unwrap(),
            "{\n  \"devCommand\": \"a\\ud800b\",\n  \"defaultAgent\": \"pi\"\n}\n"
        );
        // Replayed from TS: an escaped backslash, a lone low surrogate, U+FDD0 kept raw (in the document and the new
        // value), and a lone surrogate in a key.
        let raw =
            "{\"devCommand\":\"a\\ud800b\\\\ud800\\udfff\u{FDD0}x\",\"worktree\":{\"files\":{\"x\\udc00\":\"y\"}}}";
        let action = SettingChange {
            path: vec![String::from("actions"), String::from("n")],
            value: Some(json!("go\u{FDD0}")),
        };
        assert_eq!(
            apply_change(&document(raw), &action).unwrap(),
            "{\n  \"devCommand\": \"a\\ud800b\\\\ud800\\udfff\u{FDD0}x\",\n  \"worktree\": {\n    \"files\": {\n      \"x\\udc00\": \"y\"\n    }\n  },\n  \"actions\": {\n    \"n\": \"go\u{FDD0}\"\n  }\n}\n"
        );
        // Two keys that differ only in their lone surrogate stay two keys.
        let raw = r#"{"worktree":{"files":{"a\ud800":"x","a\ud801":"y"}}}"#;
        assert_eq!(
            apply_change(&document(raw), &agent).unwrap(),
            "{\n  \"worktree\": {\n    \"files\": {\n      \"a\\ud800\": \"x\",\n      \"a\\ud801\": \"y\"\n    }\n  },\n  \"defaultAgent\": \"pi\"\n}\n"
        );
    }

    // covers: B-P9-05 adding an action: the name is checked live (rules, existing names in either layer, the per-layer limit); commands must be non-empty
    #[test]
    fn action_name_checks() {
        let info = settings_info(
            Some(r#"{"actions":{"test":"npm test"}}"#),
            Some(json!({"actions": {"lint": "npm run lint"}})),
            false,
            None,
        );
        let check = |info: &SettingsInfo, target, name| action_name_check(&info.targets, target, name);
        let note = |text: &str| NameCheck {
            error: None,
            note: Some(text.to_owned()),
        };
        assert_eq!(check(&info, Repository, ""), NameCheck::default());
        assert_eq!(
            check(&info, Repository, " x"),
            NameCheck {
                error: Some(String::from("can't start with a space")),
                note: None
            }
        );
        assert_eq!(
            check(&info, Repository, "test"),
            note("replaces the existing test action (this repo)")
        );
        assert_eq!(
            check(&info, Repository, "lint"),
            note("overrides the global lint action in this repo")
        );
        assert_eq!(
            check(&info, Global, "test"),
            note("adds a global test action; this repo's test action still wins here")
        );
        assert_eq!(
            check(&info, Global, "fmt"),
            note("adds a new action to global defaults")
        );
        let many: Map<String, Value> = (0..MAX_ACTIONS)
            .map(|index| (format!("a{index}"), json!("true")))
            .collect();
        let full = settings_info(Some(&json!({"actions": many}).to_string()), None, false, None);
        assert_eq!(
            check(&full, Repository, "").error.as_deref(),
            Some("this repo already has 30 actions, the most allowed: remove one (x) first")
        );
        assert!(
            check(&full, Repository, "new")
                .error
                .unwrap_or_default()
                .contains("30 actions")
        );
        assert_eq!(
            check(&full, Repository, "a1"),
            note("replaces the existing a1 action (this repo)")
        );
        assert_eq!(
            check(&full, Global, "new"),
            note("adds a new action to global defaults")
        );
        assert_eq!(command_problem("  ").as_deref(), Some("type a command"));
        assert_eq!(
            command_problem(&"x".repeat(8193)).as_deref(),
            Some("too long: 8193 characters (up to 8192)")
        );
        assert_eq!(command_problem("cd backend && .venv/bin/pytest -x"), None);
        assert_eq!(
            action_saved_message("lint frontend", Repository, Some(SaveTrust::Kept)),
            "Saved action lint frontend to this repo · still trusted · run it with e on a session"
        );
        assert_eq!(
            action_saved_message("lint", Global, None),
            "Saved action lint to global defaults · run it with e on a session"
        );
    }

    // covers: B-P9-06 the status after a save says whether the repository file is still trusted
    #[test]
    fn saved_messages() {
        let trust = |trust| SavedMessageOptions {
            cleared: false,
            trust: Some(trust),
        };
        assert_eq!(
            saved_message("Location", Repository, trust(SaveTrust::Kept)),
            "Saved Location to this repo · still trusted"
        );
        assert_eq!(
            saved_message("Dev command", Repository, trust(SaveTrust::Created)),
            "Saved Dev command to this repo · created and trusted"
        );
        assert_eq!(
            saved_message("Dev command", Repository, trust(SaveTrust::Unreviewed)),
            "Saved Dev command to this repo · review required (the file had changes you haven't reviewed)"
        );
        assert_eq!(
            saved_message("Location", Repository, trust(SaveTrust::Hook)),
            "Saved Location to this repo · review required (the creation hook hasn't been reviewed)"
        );
        assert_eq!(
            saved_message(
                "Default agent",
                Repository,
                SavedMessageOptions {
                    cleared: true,
                    trust: Some(SaveTrust::Kept)
                }
            ),
            "Cleared Default agent in this repo · still trusted"
        );
        assert_eq!(
            saved_message("Dev command", Global, SavedMessageOptions::default()),
            "Saved Dev command to global defaults"
        );
    }

    // covers: B-P9-07 link picker: candidates classified and preselected; the selection rewrites only the target layer's symlink list
    #[test]
    fn link_picker_selection() {
        // classifyCandidate (settingsInfo.ts) is ported and tested with I7; its results for these entries are spelled out.
        let candidate = |path: &str, kind: &str, suggestion: &str, extra: Value| -> WorktreeCandidate {
            let mut value =
                json!({"path": path, "kind": kind, "ignored": true, "suggestion": suggestion, "reason": ""});
            if let (Value::Object(map), Value::Object(extra)) = (&mut value, extra) {
                map.extend(extra);
            }
            serde_json::from_value(value).expect("candidate")
        };
        let candidates = vec![
            candidate(
                "shared",
                "dir",
                "skip",
                json!({"configured": "symlink", "layers": ["global"]}),
            ),
            candidate(
                "keep",
                "dir",
                "skip",
                json!({"configured": "symlink", "layers": ["repository"]}),
            ),
            candidate(
                "backend/.env",
                "file",
                "link",
                json!({"configured": "files", "source": "{repoParent}/b.env", "layers": ["repository"]}),
            ),
            candidate("node_modules", "dir", "link", json!({})),
            candidate("dist", "dir", "skip", json!({})),
        ];
        let links = initial_links(&candidates, true);
        assert_eq!(
            serde_json::to_value(&links).unwrap(),
            json!({"shared": true, "keep": true, "backend/.env": true, "node_modules": true, "dist": false})
        );
        let info = settings_info(
            Some(
                r#"{"devCommand":"npm run dev","worktree":{"symlink":["unknown","keep"],"files":{"backend/.env":"{repoParent}/b.env"}}}"#,
            ),
            Some(global()),
            false,
            None,
        );
        let repository = info_layer(&info.targets, Repository);
        let global_layer = info_layer(&info.targets, Global);
        // Repository target: its entries stay in order (unknown ones too), new links are appended unless global already links them.
        let mut picked = links.clone();
        picked.insert(String::from("shared"), false);
        picked.insert(String::from("dist"), true);
        let selection = link_selection(repository.as_ref(), global_layer.as_ref(), &candidates, &picked);
        assert_eq!(
            serde_json::to_value(&selection).unwrap(),
            json!({"symlink": ["unknown", "keep", "node_modules", "dist"], "added": ["node_modules", "dist"], "removed": [], "stillLinked": ["shared"]})
        );
        let change = SettingChange {
            path: vec![String::from("worktree"), String::from("symlink")],
            value: Some(json!(selection.symlink)),
        };
        let raw = apply_change(info.targets.repository.as_ref().unwrap(), &change).unwrap();
        assert_eq!(
            parse(&raw),
            json!({"devCommand": "npm run dev", "worktree": {"symlink": ["unknown", "keep", "node_modules", "dist"], "files": {"backend/.env": "{repoParent}/b.env"}}})
        );
        // Global target: unlinking its entry removes it; files entries are never written to symlink.
        let mut picked = links;
        picked.insert(String::from("shared"), false);
        picked.insert(String::from("keep"), false);
        assert_eq!(
            serde_json::to_value(link_selection(
                global_layer.as_ref(),
                repository.as_ref(),
                &candidates,
                &picked
            ))
            .unwrap(),
            json!({"symlink": ["node_modules"], "added": ["node_modules"], "removed": ["shared"], "stillLinked": ["keep"]})
        );
    }
}
