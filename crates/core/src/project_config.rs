//! Port of projectConfig.ts (pure validation parts).
//!
//! deckhand.json validation, trust fingerprints and the effective settings (global defaults overlaid by the trusted
//! repository file). Reading files (`readBoundedUtf8`, `loadProjectConfig`, `savedProjectTrust`) is I/O and lives in
//! `deckhand_sys::config`, which builds on these.
//!
//! The user config is storage.ts's AppConfig (`trustedProjects`, `dev_command`, `defaults`): TS UserSettings is
//! [`UserSettings`] = `AppConfig`. Where the TS reads `getConfigDir()` / `getConfigPath()` (for messages and the
//! built-in worktree location), callers pass `config_dir` (DECKHAND_HOME).
//!
//! Key order: the TS builds effective settings by spreading objects, so their JSON key order is that of the spreads
//! (and, as in every JS object, integer-like keys first). The merges here therefore work on JSON maps with JS
//! semantics and record the resulting order in the returned [`Ordered`].

use indexmap::IndexMap;
use serde::Serialize;
use serde_json::{Map, Value};
use serde_with::skip_serializing_none;
use sha2::{Digest, Sha256};

use crate::config_draft::format_config_json;
use crate::error::{DhError, Result, json_parse_message};
use crate::jsstr::{self, path};
use crate::worktree_links::{
    BranchVars, expand_branch_name, expand_worktree_template, merge_worktree_settings, validate_worktree_settings,
    worktree_location,
};
use deckhand_protocol::data::{
    ConfigTargetKind, DevCommand, LoadedProject, PendingSetting, ProjectConfig, SettingRow, SettingSource,
    StringOrBool, TemplateVars, WorktreeSettings,
};
use deckhand_protocol::records::AppConfig;
use deckhand_protocol::{KeyOrder, Ordered};

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
    Sha256::digest(data).iter().map(|byte| format!("{byte:02x}")).collect()
}

// ---------------------------------------------------------------------------------------------------------------
// JS object semantics

/// A canonical array index: V8 keeps these keys first, ascending, in every object.
fn array_index(key: &str) -> Option<u32> {
    let index: u32 = key.parse().ok()?;
    (index != u32::MAX && index.to_string() == key).then_some(index)
}

/// `map` in JS property order: array indices ascending, then the other keys in insertion order.
fn js_order_map(map: Map<String, Value>) -> Map<String, Value> {
    if !map.keys().any(|key| array_index(key).is_some()) {
        return map;
    }
    let mut indices = Vec::new();
    let mut others = Vec::new();
    for (key, value) in map {
        match array_index(&key) {
            Some(index) => indices.push((index, key, value)),
            None => others.push((key, value)),
        }
    }
    indices.sort_by_key(|(index, _, _)| *index);
    indices
        .into_iter()
        .map(|(_, key, value)| (key, value))
        .chain(others)
        .collect()
}

/// `value` with every object in JS property order (what a JS object built from it would enumerate).
fn js_order(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            js_order_map(map)
                .into_iter()
                .map(|(key, value)| (key, js_order(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(js_order).collect()),
        other => other,
    }
}

/// The keys of `map` in JS enumeration order.
fn js_keys<V>(map: &IndexMap<String, V>) -> Vec<&String> {
    let mut indices: Vec<(u32, &String)> = map
        .keys()
        .filter_map(|key| array_index(key).map(|index| (index, key)))
        .collect();
    indices.sort_by_key(|(index, _)| *index);
    let mut keys: Vec<&String> = indices.into_iter().map(|(_, key)| key).collect();
    keys.extend(map.keys().filter(|key| array_index(key).is_none()));
    keys
}

/// The keys of a JSON object in JS enumeration order.
fn js_map_keys(map: &Map<String, Value>) -> Vec<&String> {
    let mut indices: Vec<(u32, &String)> = map
        .keys()
        .filter_map(|key| array_index(key).map(|index| (index, key)))
        .collect();
    indices.sort_by_key(|(index, _)| *index);
    let mut keys: Vec<&String> = indices.into_iter().map(|(_, key)| key).collect();
    keys.extend(map.keys().filter(|key| array_index(key).is_none()));
    keys
}

/// A serialised settings object as a JSON map (in its recorded key order).
fn to_map<T: Serialize>(value: &Ordered<T>) -> Result<Map<String, Value>> {
    match value.to_value()? {
        Value::Object(map) => Ok(map),
        _ => Ok(Map::new()),
    }
}

/// A JS-built settings object as an [`Ordered`] config that serialises in the same key order.
fn ordered_config(map: Map<String, Value>) -> Result<Ordered<ProjectConfig>> {
    let value = js_order(Value::Object(map));
    let order = KeyOrder::of(&value);
    let config: ProjectConfig = serde_json::from_value(value)?;
    Ok(Ordered::with_order(config, order))
}

// ---------------------------------------------------------------------------------------------------------------
// Validation

const ALLOWED_KEYS: [&str; 6] = [
    "defaultAgent",
    "defaultWorkspace",
    "devCommand",
    "setupCommand",
    "actions",
    "worktree",
];

/// Not in the TS: the most named Dev processes (`devCommand` as a map) one layer may define.
pub const MAX_DEV_PROCESSES: usize = 8;

/// `/^[a-zA-Z0-9][a-zA-Z0-9 _.-]{0,47}$/` (ASCII only, so characters and UTF-16 units agree).
fn valid_action_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    let mut rest = 0;
    for ch in chars {
        if !(ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '_' | '.' | '-')) {
            return false;
        }
        rest += 1;
    }
    rest <= 47
}

/// A command string: non-blank, at most 8192 UTF-16 units, no NUL.
fn check_command(value: &Value, label: &str) -> Result<()> {
    match value.as_str() {
        Some(text) if !jsstr::js_trim(text).is_empty() && jsstr::js_len(text) <= 8192 && !text.contains('\0') => Ok(()),
        _ => Err(DhError::msg(format!("Invalid command: {label}"))),
    }
}

/// TS: projectConfig.ts validateProjectConfig (`label` defaults to `deckhand.json`; the result keeps the value's key
/// order).
pub fn validate_project_config(value: &Value, label: Option<&str>) -> Result<Ordered<ProjectConfig>> {
    let label = label.unwrap_or(PROJECT_CONFIG_FILE);
    if !value.is_object() {
        return Err(DhError::msg(format!("{label} must be an object")));
    }
    // Checked in the order a JS object enumerates its keys, on the value as given: an invalid one may be nested
    // deeper than a recursive copy can go (JSON.parse accepts any depth). Only a valid one is copied in JS order.
    let Some(record) = value.as_object() else {
        return Err(DhError::msg(format!("{label} must be an object")));
    };
    for key in js_map_keys(record) {
        if !ALLOWED_KEYS.contains(&key.as_str()) {
            return Err(DhError::msg(format!("Unknown {label} setting: {key}")));
        }
    }
    if let Some(agent) = record.get("defaultAgent")
        && !matches!(agent.as_str(), Some("claude" | "pi" | "codex"))
    {
        return Err(DhError::msg("Invalid defaultAgent"));
    }
    if let Some(workspace) = record.get("defaultWorkspace")
        && !matches!(workspace.as_str(), Some("none" | "new" | "existing"))
    {
        return Err(DhError::msg("Invalid defaultWorkspace"));
    }
    // Not in the TS: devCommand may also name several processes (name → command) that run side by side.
    if let Some(processes) = record.get("devCommand").and_then(Value::as_object) {
        if processes.is_empty() {
            return Err(DhError::msg("devCommand needs at least one process"));
        }
        if processes.len() > MAX_DEV_PROCESSES {
            return Err(DhError::msg(format!(
                "At most {MAX_DEV_PROCESSES} Dev processes are supported"
            )));
        }
        for name in js_map_keys(processes) {
            if !valid_action_name(name) || ["__proto__", "constructor", "prototype"].contains(&name.as_str()) {
                return Err(DhError::msg(format!("Invalid Dev process name: {name}")));
            }
            check_command(&processes[name], name)?;
        }
    } else if let Some(command) = record.get("devCommand") {
        check_command(command, "devCommand")?;
    }
    if let Some(command) = record.get("setupCommand") {
        check_command(command, "setupCommand")?;
    }
    if let Some(actions) = record.get("actions") {
        let Some(actions) = actions.as_object() else {
            return Err(DhError::msg("actions must be a command map"));
        };
        if actions.len() > 30 {
            return Err(DhError::msg("At most 30 actions are supported"));
        }
        for name in js_map_keys(actions) {
            if !valid_action_name(name) || ["__proto__", "constructor", "prototype"].contains(&name.as_str()) {
                return Err(DhError::msg(format!("Invalid action name: {name}")));
            }
            check_command(&actions[name], name)?;
        }
    }
    if let Some(worktree) = record.get("worktree") {
        validate_worktree_settings(worktree)?;
    }
    let ordered = js_order(value.clone());
    let order = KeyOrder::of(&ordered);
    let config: ProjectConfig = serde_json::from_value(ordered)?;
    Ok(Ordered::with_order(config, order))
}

/// `\uD800`-`\uDFFF` escapes that do not form a pair become `�`: a Rust string cannot hold a lone surrogate, and
/// U+FFFD is what Node writes for one when it encodes the string as UTF-8.
fn replace_lone_surrogate_escapes(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let hex4 = |at: usize| -> Option<u32> {
        let digits = chars.get(at..at + 4)?;
        digits
            .iter()
            .try_fold(0, |unit, digit| Some(unit * 16 + digit.to_digit(16)?))
    };
    let escape_at = |at: usize| chars.get(at) == Some(&'\\') && chars.get(at + 1) == Some(&'u');
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut i = 0;
    while let Some(&ch) = chars.get(i) {
        if !in_string {
            in_string = ch == '"';
            out.push(ch);
            i += 1;
            continue;
        }
        match ch {
            '"' => {
                in_string = false;
                out.push(ch);
                i += 1;
            }
            '\\' if escape_at(i) => match hex4(i + 2) {
                Some(0xD800..=0xDBFF) => {
                    let low = if escape_at(i + 6) { hex4(i + 8) } else { None };
                    if low.is_some_and(|low| (0xDC00..=0xDFFF).contains(&low)) {
                        out.extend(chars.get(i..i + 12).into_iter().flatten());
                        i += 12;
                    } else {
                        out.push_str("\\ufffd");
                        i += 6;
                    }
                }
                Some(0xDC00..=0xDFFF) => {
                    out.push_str("\\ufffd");
                    i += 6;
                }
                _ => {
                    out.extend(chars.get(i..(i + 6).min(chars.len())).into_iter().flatten());
                    i += 6;
                }
            },
            '\\' => {
                out.push(ch);
                if let Some(&next) = chars.get(i + 1) {
                    out.push(next);
                }
                i += 2;
            }
            _ => {
                out.push(ch);
                i += 1;
            }
        }
    }
    out
}

/// `JSON.parse(text)`: V8's error messages; what V8 accepts but serde_json does not (lone surrogates, numbers out of
/// range) goes through JS's own round trip first.
fn json_parse(text: &str) -> Result<Value> {
    if let Some(message) = json_parse_message(text) {
        return Err(DhError::msg(message));
    }
    if let Ok(value) = serde_json::from_str(text) {
        return Ok(value);
    }
    let normalized = format_config_json(text)?;
    parse_without_recursion(&replace_lone_surrogate_escapes(&normalized)).ok_or_else(|| DhError::json_parse(text))
}

/// The end (exclusive) of the JSON string starting at `start` (its opening quote).
fn string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut pos = start + 1;
    loop {
        match *bytes.get(pos)? {
            b'\\' => pos += 2,
            b'"' => return Some(pos + 1),
            _ => pos += 1,
        }
    }
}

/// The value of `text`, valid JSON whose strings serde_json can decode, built with an explicit stack: serde_json's
/// own parser stops at 128 levels, where JSON.parse accepts any depth (format_config_json bounds it, as V8's stack
/// does). Duplicate keys keep their first position with the last value, as in JS.
fn parse_without_recursion(text: &str) -> Option<Value> {
    enum Open {
        Array(Vec<Value>),
        Object(Map<String, Value>, String),
    }
    let bytes = text.as_bytes();
    let skip_whitespace = |pos: &mut usize| {
        while matches!(bytes.get(*pos), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            *pos += 1;
        }
    };
    // An object key and its colon, from `pos`.
    let key = |pos: &mut usize| -> Option<String> {
        skip_whitespace(pos);
        let end = string_end(bytes, *pos)?;
        let key = serde_json::from_str(text.get(*pos..end)?).ok()?;
        *pos = end;
        skip_whitespace(pos);
        (bytes.get(*pos) == Some(&b':')).then(|| *pos += 1)?;
        Some(key)
    };
    let mut stack: Vec<Open> = Vec::new();
    let mut pos = 0;
    loop {
        skip_whitespace(&mut pos);
        let mut value = match *bytes.get(pos)? {
            b'[' => {
                pos += 1;
                skip_whitespace(&mut pos);
                if bytes.get(pos) != Some(&b']') {
                    stack.push(Open::Array(Vec::new()));
                    continue;
                }
                pos += 1;
                Value::Array(Vec::new())
            }
            b'{' => {
                pos += 1;
                skip_whitespace(&mut pos);
                if bytes.get(pos) != Some(&b'}') {
                    let first = key(&mut pos)?;
                    stack.push(Open::Object(Map::new(), first));
                    continue;
                }
                pos += 1;
                Value::Object(Map::new())
            }
            b'"' => {
                let end = string_end(bytes, pos)?;
                let value = Value::String(serde_json::from_str(text.get(pos..end)?).ok()?);
                pos = end;
                value
            }
            _ => {
                let end = bytes[pos..]
                    .iter()
                    .position(|byte| matches!(byte, b',' | b']' | b'}' | b' ' | b'\t' | b'\n' | b'\r'))
                    .map_or(bytes.len(), |length| pos + length);
                let value = serde_json::from_str(text.get(pos..end)?).ok()?;
                pos = end;
                value
            }
        };
        // Put the value in its container, closing every container it completes.
        loop {
            skip_whitespace(&mut pos);
            let separator = bytes.get(pos).copied();
            match stack.last_mut() {
                None => return (pos == bytes.len()).then_some(value),
                Some(Open::Array(items)) => {
                    items.push(value);
                    pos += 1;
                    match separator? {
                        b',' => break,
                        b']' => {}
                        _ => return None,
                    }
                }
                Some(Open::Object(map, pending)) => {
                    map.insert(std::mem::take(pending), value);
                    pos += 1;
                    match separator? {
                        b',' => {
                            *pending = key(&mut pos)?;
                            break;
                        }
                        b'}' => {}
                        _ => return None,
                    }
                }
            }
            value = match stack.pop()? {
                Open::Array(items) => Value::Array(items),
                Open::Object(map, _) => Value::Object(map),
            };
        }
    }
}

/// TS: projectConfig.ts parseProjectConfig (a leading BOM is ignored; JSON errors are JSON.parse's messages).
pub fn parse_project_config(raw: &str, label: Option<&str>) -> Result<Ordered<ProjectConfig>> {
    let text = raw.strip_prefix('\u{FEFF}').unwrap_or(raw);
    let value = json_parse(text)?;
    let config = validate_project_config(&value, label);
    drop_without_recursion(value);
    config
}

/// Drops `value` with an explicit stack: a value nested thousands of levels deep (which only fails validation)
/// would overflow the stack in the recursive drop.
fn drop_without_recursion(value: Value) {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Array(items) => pending.extend(items),
            Value::Object(map) => pending.extend(map.into_iter().map(|(_, child)| child)),
            _ => {}
        }
    }
}

/// TS: projectConfig.ts hookSetting result: whether a detected creation hook is enabled, and which layer disabled it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HookSetting {
    pub enabled: bool,
    pub by: Option<ConfigTargetKind>,
}

/// TS: projectConfig.ts hookSetting (the repository's `worktree.hook`, else a global `false`; read leniently).
///
/// `worktree.hook: false` in the repository file is honoured even while that file is untrusted: it can only stop a
/// script from running. Global defaults are read leniently because a `false` there only disables, too.
pub fn hook_setting(config: &ProjectConfig, user: &UserSettings) -> HookSetting {
    if let Some(hook) = config.worktree.as_ref().and_then(|worktree| worktree.hook) {
        return if hook {
            HookSetting {
                enabled: true,
                by: None,
            }
        } else {
            HookSetting {
                enabled: false,
                by: Some(ConfigTargetKind::Repository),
            }
        };
    }
    let global_hook = user
        .defaults
        .as_ref()
        .and_then(|defaults| defaults.get("worktree"))
        .and_then(|worktree| worktree.get("hook"));
    if global_hook == Some(&Value::Bool(false)) {
        HookSetting {
            enabled: false,
            by: Some(ConfigTargetKind::Global),
        }
    } else {
        HookSetting {
            enabled: true,
            by: None,
        }
    }
}

/// TS: projectConfig.ts projectFingerprint (deckhand.json's text together with the enabled hook's fingerprint).
pub fn project_fingerprint(raw: &str, hook_fingerprint: Option<&str>) -> String {
    sha256(format!("config\0{raw}\0creation-hook\0{}", hook_fingerprint.unwrap_or("")).as_bytes())
}

// ---------------------------------------------------------------------------------------------------------------
// Trust

/// TS: projectConfig.ts isProjectTrusted.
pub fn is_project_trusted(trust_root: &str, fingerprint: &str, trusted: Option<&TrustedProjects>) -> bool {
    trusted
        .and_then(|trusted| trusted.get(trust_root))
        .is_some_and(|list| list.iter().any(|value| value == fingerprint))
}

/// TS: projectConfig.ts projectNeedsReview (only a repository with a deckhand.json or an enabled creation hook has
/// anything to trust).
pub fn project_needs_review(project: &LoadedProject, trusted: Option<&TrustedProjects>) -> bool {
    (project.exists || project.creation_hook.is_some())
        && !is_project_trusted(&project.trust_root, &project.fingerprint, trusted)
}

/// TS: projectConfig.ts trustProjectConfig: the new `trustedProjects` with the fingerprint first for its root
/// (deduplicated, capped; an existing root keeps its place, a new one is appended).
pub fn trust_project_config(trust_root: &str, fingerprint: &str, trusted: Option<&TrustedProjects>) -> TrustedProjects {
    let previous = trusted.and_then(|trusted| trusted.get(trust_root));
    let list: Vec<String> = std::iter::once(fingerprint.to_owned())
        .chain(
            previous
                .into_iter()
                .flatten()
                .filter(|value| *value != fingerprint)
                .cloned(),
        )
        .take(MAX_TRUSTED_FINGERPRINTS)
        .collect();
    let mut next = trusted.cloned().unwrap_or_default();
    next.insert(trust_root.to_owned(), list);
    next
}

/// `{...value}` for the value of an object spread: objects copy, arrays and strings spread by index, anything else
/// adds nothing.
fn spread(value: Option<&Value>) -> Map<String, Value> {
    match value {
        Some(Value::Object(map)) => map.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(index, item)| (index.to_string(), item.clone()))
            .collect(),
        Some(Value::String(text)) => text
            .encode_utf16()
            .enumerate()
            .map(|(index, unit)| (index.to_string(), Value::String(String::from_utf16_lossy(&[unit]))))
            .collect(),
        _ => Map::new(),
    }
}

/// TS: projectConfig.ts trustProjectConfig on an arbitrary config object (`{...config, trustedProjects: {...}}`),
/// e.g. a raw config.json map: other keys, and other roots' values as they are, are kept.
pub fn trust_project_config_value(
    trust_root: &str,
    fingerprint: &str,
    config: &Map<String, Value>,
) -> Map<String, Value> {
    let trusted = config.get("trustedProjects");
    let previous = match trusted {
        Some(Value::Object(map)) => map.get(trust_root).and_then(Value::as_array),
        _ => None,
    };
    let list: Vec<Value> = std::iter::once(Value::String(fingerprint.to_owned()))
        .chain(
            previous
                .into_iter()
                .flatten()
                .filter(|value| value.as_str() != Some(fingerprint))
                .cloned(),
        )
        .take(MAX_TRUSTED_FINGERPRINTS)
        .collect();
    let mut roots = js_order_map(spread(trusted));
    roots.insert(trust_root.to_owned(), Value::Array(list));
    let mut next = config.clone();
    next.insert("trustedProjects".to_owned(), Value::Object(js_order_map(roots)));
    next
}

// ---------------------------------------------------------------------------------------------------------------
// Effective settings

/// TS: projectConfig.ts globalDefaults (config.json `defaults`, then the legacy `dev_command`). Error: `Invalid
/// "defaults" in <config_dir>/config.json: <why>. Repair it with C (Settings): ...`.
pub fn global_defaults(user: &UserSettings, config_dir: &str) -> Result<Ordered<ProjectConfig>> {
    let mut defaults = Map::new();
    if let Some(value) = &user.defaults {
        let validated = validate_project_config(value, Some("defaults")).map_err(|error| {
            DhError::msg(format!(
                "Invalid \"defaults\" in {}: {error}. Repair it with C (Settings): ← to the Global column, then E for the raw JSON.",
                path::join(&[config_dir, "config.json"])
            ))
        })?;
        defaults = to_map(&validated)?;
    }
    let legacy_dev = user
        .dev_command
        .as_deref()
        .map(jsstr::js_trim)
        .filter(|command| !command.is_empty());
    if let Some(legacy_dev) = legacy_dev
        && !defaults.contains_key("devCommand")
    {
        defaults.insert("devCommand".to_owned(), Value::String(legacy_dev.to_owned()));
    }
    ordered_config(defaults)
}

/// TS: projectConfig.ts SUGGESTION_KEYS.
pub const SUGGESTION_KEYS: [&str; 2] = ["defaultAgent", "defaultWorkspace"];

/// TS: projectConfig.ts untrustedLayer (an untrusted repository file's suggestions).
pub fn untrusted_layer(config: &ProjectConfig) -> ProjectConfig {
    ProjectConfig {
        default_agent: config.default_agent,
        default_workspace: config.default_workspace,
        ..ProjectConfig::default()
    }
}

/// mergeWorktreeSettings's literal key order: location, branchFrom, branchName, hook, symlink, files.
fn worktree_value(settings: &WorktreeSettings) -> Value {
    let mut map = Map::new();
    if let Some(location) = &settings.location {
        map.insert("location".to_owned(), Value::String(location.clone()));
    }
    if let Some(branch_from) = settings.branch_from {
        map.insert("branchFrom".to_owned(), Value::String(branch_from.as_str().to_owned()));
    }
    if let Some(branch_name) = &settings.branch_name {
        map.insert("branchName".to_owned(), Value::String(branch_name.clone()));
    }
    if let Some(hook) = settings.hook {
        map.insert("hook".to_owned(), Value::Bool(hook));
    }
    if let Some(symlink) = &settings.symlink {
        map.insert(
            "symlink".to_owned(),
            Value::Array(symlink.iter().cloned().map(Value::String).collect()),
        );
    }
    if let Some(files) = &settings.files {
        let files: Map<String, Value> = files
            .iter()
            .map(|(destination, source)| (destination.clone(), Value::String(source.clone())))
            .collect();
        map.insert("files".to_owned(), Value::Object(js_order_map(files)));
    }
    Value::Object(map)
}

/// The repository layer: the whole file while trusted, else only its suggestions.
fn repository_layer(project: Option<&LoadedProject>, user: &UserSettings) -> Ordered<ProjectConfig> {
    match project {
        None => Ordered::default(),
        Some(project)
            if is_project_trusted(
                &project.trust_root,
                &project.fingerprint,
                user.trusted_projects.as_ref(),
            ) =>
        {
            project.config.clone()
        }
        Some(project) => Ordered::new(untrusted_layer(&project.config)),
    }
}

/// TS: projectConfig.ts resolveSettings (the effective settings; key order as the TS spreads build it).
///
/// Global defaults overlaid field by field by the repository override. Everything that runs or places files applies
/// only while the override is trusted; its suggestions apply regardless, since the user confirms them in the picker.
pub fn resolve_settings(
    project: Option<&LoadedProject>,
    user: &UserSettings,
    config_dir: &str,
) -> Result<Ordered<ProjectConfig>> {
    let global = global_defaults(user, config_dir)?;
    let repository = repository_layer(project, user);
    let mut merged = to_map(&global)?;
    for (key, value) in to_map(&repository)? {
        merged.insert(key, value);
    }
    if global.actions.is_some() || repository.actions.is_some() {
        let mut actions = Map::new();
        for (name, command) in global.actions.iter().chain(repository.actions.iter()).flatten() {
            actions.insert(name.clone(), Value::String(command.clone()));
        }
        merged.insert("actions".to_owned(), Value::Object(actions));
    }
    if let Some(worktree) = merge_worktree_settings(global.worktree.as_ref(), repository.worktree.as_ref()) {
        merged.insert("worktree".to_owned(), worktree_value(&worktree));
    }
    // An untrusted repository may still switch the creation hook off (see hook_setting); it can never switch it on.
    if project.and_then(|project| project.config.worktree.as_ref()?.hook) == Some(false) {
        let mut worktree = spread(merged.get("worktree"));
        worktree.insert("hook".to_owned(), Value::Bool(false));
        merged.insert("worktree".to_owned(), Value::Object(worktree));
    }
    ordered_config(merged)
}

/// TS: projectConfig.ts resolveDevCommand (`dev` when nothing sets one).
pub fn resolve_dev_command(project: Option<&LoadedProject>, user: &UserSettings, config_dir: &str) -> Result<String> {
    let settings = resolve_settings(project, user, config_dir)?;
    let command = settings
        .dev_command_text()
        .map(jsstr::js_trim)
        .filter(|command| !command.is_empty())
        .unwrap_or("dev");
    Ok(command.to_owned())
}

/// Not in the TS: what Dev runs, from the effective settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DevPlan {
    /// One command (`devCommand`, the legacy `dev_command`, or `dev`).
    Command(String),
    /// Named commands side by side (`devCommand` as a map), in their configured order.
    Processes(Vec<(String, String)>),
}

/// Not in the TS: [`resolve_dev_command`], or the effective `devCommand`'s named processes when it names them.
pub fn resolve_dev_plan(project: Option<&LoadedProject>, user: &UserSettings, config_dir: &str) -> Result<DevPlan> {
    let settings = resolve_settings(project, user, config_dir)?;
    if let Some(processes) = settings.dev_processes().filter(|processes| !processes.is_empty()) {
        let processes = js_keys(processes)
            .into_iter()
            .map(|name| (name.clone(), jsstr::js_trim(&processes[name]).to_owned()))
            .collect();
        return Ok(DevPlan::Processes(processes));
    }
    resolve_dev_command(project, user, config_dir).map(DevPlan::Command)
}

/// TS: projectConfig.ts resolveSetupCommand (an untrusted setupCommand that was not reviewed is an error).
///
/// An untrusted repository setupCommand never runs: it is ignored when the user reviewed exactly these bytes and
/// chose to continue without trust, and refused otherwise so the session reports it.
pub fn resolve_setup_command(
    project: Option<&LoadedProject>,
    user: &UserSettings,
    reviewed_fingerprint: Option<&str>,
    config_dir: &str,
) -> Result<Option<String>> {
    let settings = resolve_settings(project, user, config_dir)?;
    if let Some(project) = project
        && project
            .config
            .setup_command
            .as_deref()
            .is_some_and(|command| !command.is_empty())
        && !is_project_trusted(
            &project.trust_root,
            &project.fingerprint,
            user.trusted_projects.as_ref(),
        )
        && reviewed_fingerprint != Some(project.fingerprint.as_str())
    {
        return Err(DhError::msg(
            "Repository deckhand.json is not trusted (unreviewed or changed since review); setup has not run. Press s to review it and retry",
        ));
    }
    Ok(settings.setup_command.clone())
}

// ---------------------------------------------------------------------------------------------------------------
// explainSettings

/// TS: projectConfig.ts ExplainContext.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExplainContext {
    /// Template placeholders for expanding location/files; without them templates are shown as written.
    pub vars: Option<TemplateVars>,
    /// `{user}` for the branch-name example.
    pub user: Option<String>,
    /// A detected create-worktree.sh when the project could not be loaded.
    pub hook_file: Option<String>,
    /// Deckhand's state directory (TS `getConfigDir()`): the config path in messages, and the built-in worktree
    /// location unless `location_config_dir` is set.
    pub config_dir: String,
    /// TS `configDir`: the state directory for the built-in worktree location only (messages keep `config_dir`).
    pub location_config_dir: Option<String>,
}

/// TS: projectConfig.ts SettingsExplanation.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsExplanation {
    pub rows: Vec<SettingRow>,
    pub global_error: Option<String>,
}

/// TS: projectConfig.ts SCALARS (key, built-in default).
const SCALARS: [(&str, Option<&str>); 4] = [
    ("defaultAgent", Some("claude")),
    ("defaultWorkspace", Some("none")),
    ("devCommand", Some("dev")),
    ("setupCommand", None),
];

/// A scalar setting of a config, as its string value.
fn scalar(config: &ProjectConfig, key: &str) -> Option<String> {
    match key {
        "defaultAgent" => config.default_agent.map(|agent| agent.as_str().to_owned()),
        "defaultWorkspace" => config.default_workspace.map(|mode| mode.as_str().to_owned()),
        "devCommand" => config.dev_command.as_ref().map(dev_command_summary),
        "setupCommand" => config.setup_command.clone(),
        _ => None,
    }
}

/// Not in the TS: `devCommand` as one string: the command, or `name (command), …` for named processes.
pub fn dev_command_summary(dev: &DevCommand) -> String {
    match dev {
        DevCommand::Command(command) => command.clone(),
        DevCommand::Processes(processes) => js_keys(processes)
            .into_iter()
            .map(|name| format!("{name} ({})", processes[name]))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// `String(value)`.
fn plain(value: &StringOrBool) -> String {
    match value {
        StringOrBool::String(text) => text.clone(),
        StringOrBool::Bool(flag) => flag.to_string(),
    }
}

/// The values of one setting in each layer.
struct Layers {
    repo: Option<StringOrBool>,
    global: Option<StringOrBool>,
    pending: Option<StringOrBool>,
}

/// One explained setting: its effective value and how to show it.
struct Pick<'a> {
    key: &'a str,
    value: Option<StringOrBool>,
    layers: Layers,
    fallback: Option<StringOrBool>,
    display: &'a dyn Fn(&StringOrBool) -> String,
    legacy: bool,
    note: Option<&'a str>,
}

/// `pendingOf`: `same` marks an untrusted value equal to the one in effect.
fn pending_of(row: &SettingRow, value: String, raw: StringOrBool) -> PendingSetting {
    let same = (row.source != SettingSource::NotSet && value == row.value).then_some(true);
    PendingSetting { value, raw, same }
}

fn pick(rows: &mut Vec<SettingRow>, pick: Pick<'_>) {
    let display = pick.display;
    let source = match &pick.value {
        None if pick.fallback.is_none() => SettingSource::NotSet,
        None => SettingSource::BuiltInDefault,
        Some(value) if pick.layers.repo.as_ref() == Some(value) => SettingSource::Repo,
        Some(_) if pick.legacy => SettingSource::LegacyDevCommand,
        Some(_) => SettingSource::Global,
    };
    let shown = match (&pick.value, &pick.fallback) {
        (Some(value), _) | (None, Some(value)) => display(value),
        (None, None) => String::from("—"),
    };
    let mut row = SettingRow {
        key: pick.key.to_owned(),
        entry: None,
        value: shown,
        source,
        raw: pick.value.clone(),
        note: None,
        pending: None,
    };
    let overrides = match (&pick.layers.global, &pick.value) {
        (Some(global), Some(value)) if source == SettingSource::Repo && global != value => {
            Some(format!("overrides global {}", display(global)))
        }
        _ => None,
    };
    let note: Vec<&str> = [overrides.as_deref(), pick.note]
        .into_iter()
        .flatten()
        .filter(|note| !note.is_empty())
        .collect();
    if !note.is_empty() {
        row.note = Some(note.join(" · "));
    }
    if let Some(pending) = pick.layers.pending {
        row.pending = Some(pending_of(&row, display(&pending), pending));
    }
    rows.push(row);
}

/// A map setting's layers (`actions`, `worktree.files`).
struct EntryLayers<'a> {
    repo: Option<&'a IndexMap<String, String>>,
    global: Option<&'a IndexMap<String, String>>,
    pending: Option<&'a IndexMap<String, String>>,
}

/// One row per entry of a map setting, then the entries only an untrusted file has, else a single unset row.
fn entries(
    rows: &mut Vec<SettingRow>,
    key: &str,
    values: Option<&IndexMap<String, String>>,
    layers: EntryLayers<'_>,
    display: &dyn Fn(&str) -> String,
) {
    let before = rows.len();
    for entry in values.map(js_keys).unwrap_or_default() {
        let Some(value) = values.and_then(|values| values.get(entry)) else {
            continue;
        };
        let repo = layers.repo.is_some_and(|repo| repo.contains_key(entry));
        let mut row = SettingRow {
            key: key.to_owned(),
            entry: Some(entry.clone()),
            value: display(value),
            source: if repo {
                SettingSource::Repo
            } else {
                SettingSource::Global
            },
            raw: Some(StringOrBool::String(value.clone())),
            note: None,
            pending: None,
        };
        let global_value = layers.global.and_then(|global| global.get(entry));
        if let Some(global_value) = global_value
            && repo
            && global_value != value
        {
            row.note = Some(format!("overrides global {}", display(global_value)));
        }
        if let Some(pending) = layers.pending.and_then(|pending| pending.get(entry)) {
            row.pending = Some(pending_of(
                &row,
                display(pending),
                StringOrBool::String(pending.clone()),
            ));
        }
        rows.push(row);
    }
    for entry in layers.pending.map(js_keys).unwrap_or_default() {
        let Some(value) = layers.pending.and_then(|pending| pending.get(entry)) else {
            continue;
        };
        if values.is_some_and(|values| values.contains_key(entry)) {
            continue;
        }
        rows.push(SettingRow {
            key: key.to_owned(),
            entry: Some(entry.clone()),
            value: String::from("—"),
            source: SettingSource::NotSet,
            raw: None,
            note: None,
            pending: Some(PendingSetting {
                value: display(value),
                raw: StringOrBool::String(value.clone()),
                same: None,
            }),
        });
    }
    if rows.len() == before {
        rows.push(unset_row(key));
    }
}

fn unset_row(key: &str) -> SettingRow {
    SettingRow {
        key: key.to_owned(),
        entry: None,
        value: String::from("—"),
        source: SettingSource::NotSet,
        raw: None,
        note: None,
        pending: None,
    }
}

const NO_HOOK: &str = "no .claude/scripts/create-worktree.sh detected";
const HOOK_UNUSED: &str = ".claude/scripts/create-worktree.sh detected, not used";
const HOOK_DECIDES: &str = ".claude/scripts/create-worktree.sh detected: decides location and branch";
const HOOK_UNTRUSTED: &str = ".claude/scripts/create-worktree.sh detected: will not run until trusted (T)";

/// TS: projectConfig.ts explainSettings (row by row with sources; pending values of an untrusted file).
///
/// Values come from resolve_settings (invalid global defaults are reported and skipped, keeping the legacy
/// dev_command), sources from the layers, pending values from an untrusted repository file. Pure; never reads files.
pub fn explain_settings(
    project: Option<&LoadedProject>,
    user: &UserSettings,
    context: &ExplainContext,
) -> SettingsExplanation {
    let config_dir = context.config_dir.as_str();
    let resolved =
        global_defaults(user, config_dir).and_then(|global| Ok((global, resolve_settings(project, user, config_dir)?)));
    let (global, effective, global_error) = match resolved {
        Ok((global, effective)) => (global, effective, None),
        Err(error) => {
            let lenient = AppConfig {
                defaults: None,
                ..user.clone()
            };
            (
                global_defaults(&lenient, config_dir).unwrap_or_default(),
                resolve_settings(project, &lenient, config_dir).unwrap_or_default(),
                Some(error.to_string()),
            )
        }
    };
    let trusted = project.is_some_and(|project| {
        is_project_trusted(
            &project.trust_root,
            &project.fingerprint,
            user.trusted_projects.as_ref(),
        )
    });
    let repository = repository_layer(project, user);
    // What trust would add: everything but the suggestions, which apply already.
    let pending_layer = match project {
        Some(project) if !trusted => ProjectConfig {
            default_agent: None,
            default_workspace: None,
            ..(*project.config).clone()
        },
        _ => ProjectConfig::default(),
    };
    let vars = context.vars.as_ref();
    let location = |template: &StringOrBool| {
        let template = plain(template);
        match vars {
            Some(vars) => worktree_location(&template, vars).unwrap_or(template),
            None => template,
        }
    };
    let source = |template: &str| match vars {
        Some(vars) => expand_worktree_template(template, vars).unwrap_or_else(|_| template.to_owned()),
        None => template.to_owned(),
    };
    let branch = |template: &StringOrBool| {
        let template = plain(template);
        let branch_vars = BranchVars {
            name: vars.map_or("<name>", |vars| vars.name.as_str()),
            user: context.user.as_deref().unwrap_or("user"),
        };
        expand_branch_name(&template, &branch_vars).unwrap_or(template)
    };
    let show = |value: &StringOrBool| match value {
        StringOrBool::Bool(true) => String::from("on"),
        StringOrBool::Bool(false) => String::from("off"),
        StringOrBool::String(text) => text.clone(),
    };
    let text = |value: Option<String>| value.map(StringOrBool::String);
    let mut rows = Vec::new();
    let hook_file = match project {
        Some(project) => project
            .creation_hook
            .as_ref()
            .map(|hook| hook.file.clone())
            .or_else(|| project.disabled_hook.as_ref().map(|hook| hook.file.clone())),
        None => context.hook_file.clone(),
    };
    let hook_active = trusted && project.is_some_and(|project| project.creation_hook.is_some());

    for (key, built_in) in SCALARS {
        let repo_value = scalar(&repository, key);
        let effective_value = scalar(&effective, key);
        let suggestion = project.is_some() && !trusted && SUGGESTION_KEYS.contains(&key) && repo_value.is_some();
        let global_dev = user.defaults.as_ref().and_then(|defaults| defaults.get("devCommand"));
        let legacy = key == "devCommand"
            && repository.dev_command.is_none()
            && effective_value.is_some()
            && (global_error.is_some() || global_dev.is_none());
        pick(
            &mut rows,
            Pick {
                key,
                value: text(effective_value),
                layers: Layers {
                    repo: text(repo_value),
                    global: text(scalar(&global, key)),
                    pending: text(scalar(&pending_layer, key)),
                },
                fallback: built_in.map(|value| StringOrBool::String(value.to_owned())),
                display: &plain,
                legacy,
                note: suggestion.then_some("applies without trust: only preselects the new-session picker"),
            },
        );
    }
    entries(
        &mut rows,
        "actions",
        effective.actions.as_ref(),
        EntryLayers {
            repo: repository.actions.as_ref(),
            global: global.actions.as_ref(),
            pending: pending_layer.actions.as_ref(),
        },
        &|value: &str| value.to_owned(),
    );
    // Not in the TS: a row per named Dev process, after the devCommand row, while one applies or is pending.
    if effective.dev_processes().is_some() || pending_layer.dev_processes().is_some() {
        entries(
            &mut rows,
            "devCommand",
            effective.dev_processes(),
            EntryLayers {
                repo: repository.dev_processes(),
                global: global.dev_processes(),
                pending: pending_layer.dev_processes(),
            },
            &|value: &str| value.to_owned(),
        );
    }

    let empty = WorktreeSettings::default();
    let tree = effective.worktree.as_ref().unwrap_or(&empty);
    let repo_tree = repository.worktree.as_ref().unwrap_or(&empty);
    let global_tree = global.worktree.as_ref().unwrap_or(&empty);
    let pending_tree = pending_layer.worktree.as_ref().unwrap_or(&empty);
    let ignored = hook_active.then_some("ignored: the creation hook decides");
    let default_location = path::join(&[
        context.location_config_dir.as_deref().unwrap_or(config_dir),
        "worktrees",
        vars.map_or("{name}", |vars| vars.name.as_str()),
    ]);
    pick(
        &mut rows,
        Pick {
            key: "worktree.location",
            value: text(tree.location.clone()),
            layers: Layers {
                repo: text(repo_tree.location.clone()),
                global: text(global_tree.location.clone()),
                pending: text(pending_tree.location.clone()),
            },
            fallback: Some(StringOrBool::String(default_location)),
            display: &location,
            legacy: false,
            note: ignored,
        },
    );
    let branch_from = |settings: &WorktreeSettings| settings.branch_from.map(|from| from.as_str().to_owned());
    pick(
        &mut rows,
        Pick {
            key: "worktree.branchFrom",
            value: text(branch_from(tree)),
            layers: Layers {
                repo: text(branch_from(repo_tree)),
                global: text(branch_from(global_tree)),
                pending: text(branch_from(pending_tree)),
            },
            fallback: Some(StringOrBool::String(String::from("current"))),
            display: &plain,
            legacy: false,
            note: ignored,
        },
    );
    pick(
        &mut rows,
        Pick {
            key: "worktree.branchName",
            value: text(tree.branch_name.clone()),
            layers: Layers {
                repo: text(repo_tree.branch_name.clone()),
                global: text(global_tree.branch_name.clone()),
                pending: text(pending_tree.branch_name.clone()),
            },
            fallback: Some(StringOrBool::String(String::from("{name}"))),
            display: &branch,
            legacy: false,
            note: ignored,
        },
    );
    // The repository's hook: false applies even untrusted (see hook_setting); only `true` can be pending.
    let hook_on = tree.hook != Some(false);
    let hook_note = if hook_file.is_none() {
        NO_HOOK
    } else if !hook_on {
        HOOK_UNUSED
    } else if hook_active {
        HOOK_DECIDES
    } else {
        HOOK_UNTRUSTED
    };
    let repository_hook_off = project.and_then(|project| project.config.worktree.as_ref()?.hook) == Some(false);
    pick(
        &mut rows,
        Pick {
            key: "worktree.hook",
            value: tree.hook.map(StringOrBool::Bool),
            layers: Layers {
                repo: if repository_hook_off {
                    Some(StringOrBool::Bool(false))
                } else {
                    repo_tree.hook.map(StringOrBool::Bool)
                },
                global: global_tree.hook.map(StringOrBool::Bool),
                pending: (pending_tree.hook == Some(true)).then_some(StringOrBool::Bool(true)),
            },
            fallback: Some(StringOrBool::Bool(true)),
            display: &show,
            legacy: false,
            note: Some(hook_note),
        },
    );

    let symlinks: &[String] = tree.symlink.as_deref().unwrap_or_default();
    let repo_links: &[String] = repo_tree.symlink.as_deref().unwrap_or_default();
    let global_links: &[String] = global_tree.symlink.as_deref().unwrap_or_default();
    let before = rows.len();
    for entry in symlinks {
        let source = if global_links.contains(entry) {
            SettingSource::Global
        } else if repo_links.contains(entry) {
            SettingSource::Repo
        } else {
            SettingSource::Global
        };
        rows.push(SettingRow {
            key: String::from("worktree.symlink"),
            entry: Some(entry.clone()),
            value: entry.clone(),
            source,
            raw: Some(StringOrBool::String(entry.clone())),
            note: None,
            pending: None,
        });
    }
    for entry in pending_tree.symlink.iter().flatten() {
        if !symlinks.contains(entry) {
            rows.push(SettingRow {
                key: String::from("worktree.symlink"),
                entry: Some(entry.clone()),
                value: String::from("—"),
                source: SettingSource::NotSet,
                raw: None,
                note: None,
                pending: Some(PendingSetting {
                    value: entry.clone(),
                    raw: StringOrBool::String(entry.clone()),
                    same: None,
                }),
            });
        }
    }
    if rows.len() == before {
        rows.push(unset_row("worktree.symlink"));
    }
    entries(
        &mut rows,
        "worktree.files",
        tree.files.as_ref(),
        EntryLayers {
            repo: repo_tree.files.as_ref(),
            global: global_tree.files.as_ref(),
            pending: pending_tree.files.as_ref(),
        },
        &source,
    );
    SettingsExplanation {
        rows,
        global_error: global_error.filter(|error| !error.is_empty()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deckhand_testkit::{Case, Outcome, Replay, replay, tags};
    use serde_json::json;

    /// `replay` that also compares object key order (these objects are written to the wire and config.json).
    fn replay_ordered(fixture: &str, port: impl FnMut(&Case) -> Outcome) {
        Replay::new(fixture).key_order(true).run(port).assert_ok();
    }

    /// The fixtures pin DECKHAND_HOME to this directory.
    const CONFIG_DIR: &str = "/deckhand-home";

    /// JSON.parse accepts any nesting depth (serde_json stops at 128): a deeply nested value reaches validation and
    /// fails there with its own message, as in TS.
    #[test]
    fn deeply_nested_json_is_parsed_like_json_parse() {
        let nested = |depth: usize, inner: &str| format!("{}{inner}{}", "[".repeat(depth), "]".repeat(depth));
        for depth in [127, 200, 5000] {
            let deep = nested(depth, "\"\\ud800x\", 1e400, {\"a\": 1, \"a\": 2}");
            let message = |text: String| parse_project_config(&text, None).unwrap_err().to_string();
            assert_eq!(message(deep.clone()), "deckhand.json must be an object");
            assert_eq!(message(format!("{{\"defaultAgent\": {deep}}}")), "Invalid defaultAgent");
            assert_eq!(
                message(format!("{{\"devCommand\": \"x\", \"extra\": {{\"k\": {deep}}}}}")),
                "Unknown deckhand.json setting: extra"
            );
        }
        let value = json_parse(&nested(300, "\"\\ud800x\", 1e400, -0, {\"a\": 1, \"b\": [], \"a\": 2}")).unwrap();
        let mut inner = &value;
        for _ in 0..299 {
            inner = &inner[0];
        }
        assert_eq!(inner, &json!(["\u{FFFD}x", null, 0, {"a": 2, "b": []}]));
    }

    /// Runs a port body whose harness steps may fail (argument decoding).
    fn port(body: impl FnOnce() -> std::result::Result<Outcome, String>) -> Outcome {
        body().unwrap_or_else(|error| Outcome::throws(format!("harness: {error}")))
    }

    fn outcome<T: Serialize>(result: Result<T>) -> Outcome {
        match result {
            Ok(value) => Outcome::ret(value),
            Err(error) => Outcome::throws(error),
        }
    }

    fn optional_str(case: &Case, index: usize) -> std::result::Result<Option<&str>, String> {
        if case.arg_is_undefined(index) {
            Ok(None)
        } else {
            case.arg_str(index).map(Some)
        }
    }

    /// `config.trustedProjects` as isProjectTrusted reads it: only array values are lists, and only their strings
    /// can equal a fingerprint.
    fn trusted_of(value: &Value) -> Option<TrustedProjects> {
        let roots = value.get("trustedProjects")?.as_object()?;
        Some(
            roots
                .iter()
                .filter_map(|(root, list)| {
                    let list = list.as_array()?;
                    Some((
                        root.clone(),
                        list.iter()
                            .filter_map(|item| item.as_str().map(str::to_owned))
                            .collect(),
                    ))
                })
                .collect(),
        )
    }

    /// A recorded LoadedProject (the generator's creation hooks carry no `content`, which nothing here reads).
    fn project_of(case: &Case, index: usize) -> std::result::Result<Option<LoadedProject>, String> {
        if case.arg_is_undefined(index) {
            return Ok(None);
        }
        let mut value = tags::untag(case.arg(index));
        if let Some(hook) = value.get_mut("creationHook").and_then(Value::as_object_mut) {
            hook.entry("content").or_insert_with(|| json!(""));
        }
        serde_json::from_value(value)
            .map(Some)
            .map_err(|error| format!("case {} project: {error}", case.index))
    }

    fn user_of(case: &Case, index: usize) -> std::result::Result<UserSettings, String> {
        if case.arg_is_undefined(index) {
            Ok(UserSettings::default())
        } else {
            case.arg_as(index)
        }
    }

    fn context_of(case: &Case, index: usize) -> std::result::Result<ExplainContext, String> {
        let value = tags::untag(case.arg(index));
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        Ok(ExplainContext {
            vars: match value.get("vars") {
                Some(vars) => Some(serde_json::from_value(vars.clone()).map_err(|error| error.to_string())?),
                None => None,
            },
            user: text("user"),
            hook_file: text("hookFile"),
            config_dir: CONFIG_DIR.to_owned(),
            location_config_dir: text("configDir"),
        })
    }

    #[test]
    fn sha256_parity() {
        replay("projectConfig-P10/sha256", |case: &Case| {
            port(|| {
                let bytes = match case.arg(0) {
                    Value::String(text) => text.as_bytes().to_vec(),
                    other => match tags::tag_of(other) {
                        Some(("$bytes", Value::String(base64))) => tags::base64_decode(base64)?,
                        _ => return Err(String::from("unsupported sha256 argument")),
                    },
                };
                Ok(Outcome::ret(sha256(&bytes)))
            })
        });
    }

    #[test]
    fn project_fingerprint_parity() {
        replay("projectConfig-P10/projectFingerprint", |case: &Case| {
            port(|| {
                Ok(Outcome::ret(project_fingerprint(
                    case.arg_str(0)?,
                    optional_str(case, 1)?,
                )))
            })
        });
    }

    #[test]
    fn validate_project_config_parity() {
        replay_ordered("projectConfig-P10/validateProjectConfig", |case: &Case| {
            port(|| {
                Ok(outcome(validate_project_config(
                    &tags::untag(case.arg(0)),
                    optional_str(case, 1)?,
                )))
            })
        });
    }

    #[test]
    fn parse_project_config_parity() {
        replay_ordered("projectConfig-P10/parseProjectConfig", |case: &Case| {
            port(|| Ok(outcome(parse_project_config(case.arg_str(0)?, optional_str(case, 1)?))))
        });
    }

    #[test]
    fn is_project_trusted_parity() {
        replay("projectConfig-P10/isProjectTrusted", |case: &Case| {
            port(|| {
                let project = tags::untag(case.arg(0));
                let field = |key: &str| project.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
                let trusted = trusted_of(&tags::untag(case.arg(1)));
                Ok(Outcome::ret(is_project_trusted(
                    &field("trustRoot"),
                    &field("fingerprint"),
                    trusted.as_ref(),
                )))
            })
        });
    }

    #[test]
    fn project_needs_review_parity() {
        replay("projectConfig-P10/projectNeedsReview", |case: &Case| {
            port(|| {
                let project = project_of(case, 0)?.ok_or("project expected")?;
                let trusted = trusted_of(&tags::untag(case.arg(1)));
                Ok(Outcome::ret(project_needs_review(&project, trusted.as_ref())))
            })
        });
    }

    #[test]
    fn trust_project_config_parity() {
        replay_ordered("projectConfig-P10/trustProjectConfig", |case: &Case| {
            port(|| {
                let project = tags::untag(case.arg(0));
                let field = |key: &str| project.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
                let (root, fingerprint) = (field("trustRoot"), field("fingerprint"));
                let config = tags::untag(case.arg(1));
                let config = config.as_object().cloned().unwrap_or_default();
                let next = trust_project_config_value(&root, &fingerprint, &config);
                // The typed form agrees wherever the config is well-formed (string lists only).
                let well_formed = match config.get("trustedProjects") {
                    None => true,
                    Some(Value::Object(roots)) => roots
                        .values()
                        .all(|list| list.as_array().is_some_and(|items| items.iter().all(Value::is_string))),
                    Some(_) => false,
                };
                if well_formed {
                    let typed = trust_project_config(&root, &fingerprint, trusted_of(&Value::Object(config)).as_ref());
                    if serde_json::to_value(&typed).ok().as_ref() != next.get("trustedProjects") {
                        return Err(String::from("typed trust_project_config disagrees"));
                    }
                }
                Ok(Outcome::ret(next))
            })
        });
    }

    #[test]
    fn global_defaults_parity() {
        replay_ordered("projectConfig-P10/globalDefaults", |case: &Case| {
            port(|| Ok(outcome(global_defaults(&user_of(case, 0)?, CONFIG_DIR))))
        });
    }

    #[test]
    fn untrusted_layer_parity() {
        replay_ordered("projectConfig-P10/untrustedLayer", |case: &Case| {
            port(|| Ok(Outcome::ret(untrusted_layer(&case.arg_as(0)?))))
        });
    }

    #[test]
    fn resolve_settings_parity() {
        replay_ordered("projectConfig-P10/resolveSettings", |case: &Case| {
            port(|| {
                Ok(outcome(resolve_settings(
                    project_of(case, 0)?.as_ref(),
                    &user_of(case, 1)?,
                    CONFIG_DIR,
                )))
            })
        });
    }

    #[test]
    fn resolve_dev_command_parity() {
        replay("projectConfig-P10/resolveDevCommand", |case: &Case| {
            port(|| {
                Ok(outcome(resolve_dev_command(
                    project_of(case, 0)?.as_ref(),
                    &user_of(case, 1)?,
                    CONFIG_DIR,
                )))
            })
        });
    }

    #[test]
    fn resolve_setup_command_parity() {
        replay("projectConfig-P10/resolveSetupCommand", |case: &Case| {
            port(|| {
                let result = resolve_setup_command(
                    project_of(case, 0)?.as_ref(),
                    &user_of(case, 1)?,
                    optional_str(case, 2)?,
                    CONFIG_DIR,
                );
                Ok(match result {
                    Ok(Some(command)) => Outcome::ret(command),
                    Ok(None) => Outcome::ret(tags::undefined()),
                    Err(error) => Outcome::throws(error),
                })
            })
        });
    }

    #[test]
    fn explain_settings_parity() {
        replay_ordered("projectConfig-P10/explainSettings", |case: &Case| {
            port(|| {
                Ok(Outcome::ret(explain_settings(
                    project_of(case, 0)?.as_ref(),
                    &user_of(case, 1)?,
                    &context_of(case, 2)?,
                )))
            })
        });
    }

    // -----------------------------------------------------------------------------------------------------------
    // Ported tests (tests/projectConfig.test.ts)

    fn config(value: Value) -> Ordered<ProjectConfig> {
        serde_json::from_value(value).unwrap()
    }

    fn user(value: Value) -> UserSettings {
        serde_json::from_value(value).unwrap()
    }

    fn project(root: &str, value: Value) -> LoadedProject {
        LoadedProject {
            root: root.to_owned(),
            trust_root: root.to_owned(),
            path: Some(format!("{root}/deckhand.json")),
            config: config(value),
            exists: true,
            fingerprint: String::from("f"),
            creation_hook: None,
            disabled_hook: None,
        }
    }

    /// `trustProjectConfig(project, user)` as an updateAppConfig updater.
    fn trust(project: &LoadedProject, user: &UserSettings) -> UserSettings {
        UserSettings {
            trusted_projects: Some(trust_project_config(
                &project.trust_root,
                &project.fingerprint,
                user.trusted_projects.as_ref(),
            )),
            ..user.clone()
        }
    }

    fn json_of<T: Serialize>(value: &T) -> Value {
        serde_json::to_value(value).unwrap()
    }

    // covers: B-P10-01 strict project schema rejects malformed/unknown commands and defaults
    #[test]
    fn strict_project_schema() {
        for value in [
            "[]",
            r#"{"unknown":true}"#,
            r#"{"defaultAgent":["pi"]}"#,
            r#"{"defaultWorkspace":null}"#,
            r#"{"setupCommand":""}"#,
            r#"{"actions":{"bad/name":"echo ok"}}"#,
            r#"{"actions":{"__proto__":"echo bad"}}"#,
        ] {
            assert!(parse_project_config(value, None).is_err(), "{value}");
        }
        assert_eq!(
            json_of(&parse_project_config(r#"{"defaultAgent":"codex","actions":{"test":"npm test"}}"#, None).unwrap()),
            json!({"defaultAgent": "codex", "actions": {"test": "npm test"}})
        );
    }

    // Not in the TS: named Dev processes, validated like actions, and one setting with devCommand across layers.
    #[test]
    fn dev_command_names_processes_validated_like_actions() {
        for value in [
            r#"{"devCommand":[]}"#,
            r#"{"devCommand":{}}"#,
            r#"{"devCommand":{"api/x":"run"}}"#,
            r#"{"devCommand":{"api":""}}"#,
            r#"{"devCommand":{"a":"1","b":"2","c":"3","d":"4","e":"5","f":"6","g":"7","h":"8","i":"9"}}"#,
            r#"{"devProcesses":{"api":"run"}}"#,
        ] {
            assert!(parse_project_config(value, None).is_err(), "{value}");
        }
        let processes = json!({"api": "cd backend && python3 main.py", "web": "cd frontend && npm run dev"});
        let plan = DevPlan::Processes(vec![
            ("api".to_owned(), "cd backend && python3 main.py".to_owned()),
            ("web".to_owned(), "cd frontend && npm run dev".to_owned()),
        ]);
        // Global processes, beside a legacy dev_command they replace.
        let global = user(json!({"dev_command": "legacy", "defaults": {"devCommand": processes}}));
        assert_eq!(resolve_dev_plan(None, &global, CONFIG_DIR).unwrap(), plan);
        // A trusted repository's devCommand replaces global processes, and its processes a global command.
        let command = project("/r", json!({"devCommand": "make dev"}));
        assert_eq!(
            resolve_dev_plan(Some(&command), &trust(&command, &global), CONFIG_DIR).unwrap(),
            DevPlan::Command("make dev".to_owned())
        );
        let named = project("/r", json!({"devCommand": processes}));
        let global_command = user(json!({"defaults": {"devCommand": "npm start"}}));
        assert_eq!(
            resolve_dev_plan(Some(&named), &trust(&named, &global_command), CONFIG_DIR).unwrap(),
            plan
        );
        // Untrusted, the repository's processes never run.
        assert_eq!(
            resolve_dev_plan(Some(&named), &global_command, CONFIG_DIR).unwrap(),
            DevPlan::Command("npm start".to_owned())
        );
        // Settings: the devCommand row sums the processes up, then a row per process.
        let context = ExplainContext {
            config_dir: CONFIG_DIR.to_owned(),
            ..ExplainContext::default()
        };
        let rows = explain_settings(Some(&named), &trust(&named, &global_command), &context).rows;
        let dev: Vec<(Option<&str>, &str, SettingSource)> = rows
            .iter()
            .filter(|row| row.key == "devCommand")
            .map(|row| (row.entry.as_deref(), row.value.as_str(), row.source))
            .collect();
        assert_eq!(
            dev,
            [
                (
                    None,
                    "api (cd backend && python3 main.py), web (cd frontend && npm run dev)",
                    SettingSource::Repo
                ),
                (Some("api"), "cd backend && python3 main.py", SettingSource::Repo),
                (Some("web"), "cd frontend && npm run dev", SettingSource::Repo),
            ]
        );
        let untrusted = explain_settings(Some(&named), &global_command, &context).rows;
        assert!(
            untrusted
                .iter()
                .filter(|row| row.key == "devCommand" && row.entry.is_some())
                .all(|row| row.pending.is_some() && row.source == SettingSource::NotSet)
        );
    }

    // covers: B-P10-02 effective settings: global defaults overlaid by the repository override only while trusted (its agent/workspace suggestions always); actions merge by name
    #[test]
    fn effective_settings() {
        let project = project(
            "/r",
            json!({"defaultAgent": "pi", "devCommand": "repo dev", "setupCommand": "repo setup", "actions": {"test": "repo test", "lint": "repo lint"}}),
        );
        let user_settings = user(
            json!({"dev_command": "legacy", "defaults": {"defaultAgent": "codex", "defaultWorkspace": "new", "setupCommand": "npm ci", "actions": {"test": "global test", "fmt": "global fmt"}}}),
        );
        // Untrusted: only the repository's defaultAgent applies (it preselects the picker and runs nothing).
        assert_eq!(
            json_of(&resolve_settings(Some(&project), &user_settings, CONFIG_DIR).unwrap()),
            json!({"defaultAgent": "pi", "defaultWorkspace": "new", "setupCommand": "npm ci", "devCommand": "legacy", "actions": {"test": "global test", "fmt": "global fmt"}})
        );
        let trusted = trust(&project, &user_settings);
        assert_eq!(
            json_of(&resolve_settings(Some(&project), &trusted, CONFIG_DIR).unwrap()),
            json!({"defaultAgent": "pi", "defaultWorkspace": "new", "setupCommand": "repo setup", "devCommand": "repo dev", "actions": {"test": "repo test", "fmt": "global fmt", "lint": "repo lint"}})
        );
        assert_eq!(
            resolve_dev_command(None, &UserSettings::default(), CONFIG_DIR).unwrap(),
            "dev"
        );
        assert_eq!(
            resolve_dev_command(
                Some(&project),
                &user(json!({"dev_command": "legacy", "defaults": {"devCommand": "npm run dev"}})),
                CONFIG_DIR
            )
            .unwrap(),
            "npm run dev"
        );
        let error = resolve_settings(
            Some(&project),
            &user(json!({"defaults": {"setupCommand": ""}})),
            CONFIG_DIR,
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.starts_with("Invalid \"defaults\"") && error.contains("Invalid command"),
            "{error}"
        );
        // An untrusted repository setupCommand is refused unless the user reviewed exactly these bytes and skipped them.
        let refused = resolve_setup_command(Some(&project), &user_settings, None, CONFIG_DIR).unwrap_err();
        assert!(refused.to_string().contains("not trusted"));
        assert_eq!(
            resolve_setup_command(Some(&project), &user_settings, Some("f"), CONFIG_DIR).unwrap(),
            Some(String::from("npm ci"))
        );
        assert_eq!(
            resolve_setup_command(Some(&project), &trusted, None, CONFIG_DIR).unwrap(),
            Some(String::from("repo setup"))
        );
    }

    /// Rebuilds a ProjectConfig from the rows that are in effect (not built-in, unset or pending).
    fn from_rows(rows: &[SettingRow]) -> Value {
        let mut config = Map::new();
        let mut worktree = Map::new();
        for row in rows {
            if matches!(row.source, SettingSource::BuiltInDefault | SettingSource::NotSet) {
                continue;
            }
            let (scope, field) = match row.key.strip_prefix("worktree.") {
                Some(field) => (&mut worktree, field),
                None => (&mut config, row.key.as_str()),
            };
            let raw = json_of(&row.raw);
            if field == "symlink" {
                let list = scope.entry("symlink").or_insert_with(|| json!([]));
                list.as_array_mut().unwrap().push(raw);
            } else if let Some(entry) = &row.entry {
                let map = scope.entry(field).or_insert_with(|| json!({}));
                map.as_object_mut().unwrap().insert(entry.clone(), raw);
            } else {
                scope.insert(field.to_owned(), raw);
            }
        }
        if !worktree.is_empty() {
            config.insert(String::from("worktree"), Value::Object(worktree));
        }
        Value::Object(config)
    }

    fn find<'a>(rows: &'a [SettingRow], key: &str, entry: Option<&str>) -> &'a SettingRow {
        rows.iter()
            .find(|row| row.key == key && row.entry.as_deref() == entry)
            .unwrap()
    }

    fn pending_value(row: &SettingRow) -> Option<&str> {
        row.pending.as_ref().map(|pending| pending.value.as_str())
    }

    // covers: B-P10-03 explainSettings: sources per row, pending untrusted values, list merges, legacy and built-in defaults; values equal resolveSettings
    #[test]
    fn explain_settings_rows() {
        let vars = TemplateVars {
            name: String::from("<name>"),
            repo: String::from("mono"),
            repo_parent: String::from("/dev"),
            repo_root: String::from("/dev/mono"),
            home: String::from("/home/me"),
        };
        let context = ExplainContext {
            vars: Some(vars),
            user: Some(String::from("tejas")),
            hook_file: None,
            config_dir: CONFIG_DIR.to_owned(),
            location_config_dir: Some(String::from("/state")),
        };
        let project = project(
            "/dev/mono",
            json!({"defaultWorkspace": "new", "devCommand": "cd frontend && npm run dev", "actions": {"test": "pytest", "lint": "repo lint"}, "worktree": {"location": "{repoParent}/worktrees/{name}", "hook": false, "symlink": ["frontend/node_modules", "node_modules"], "files": {"backend/local.cfg": "{repoParent}/backend.cfg"}}}),
        );
        let user_settings = user(
            json!({"dev_command": "legacy dev", "defaults": {"defaultAgent": "codex", "actions": {"lint": "npm run lint"}, "worktree": {"branchName": "{user}/{name}", "symlink": ["node_modules"], "files": {"backend/local.cfg": "/g/cfg"}}}}),
        );
        let trusted = trust(&project, &user_settings);
        let with_config = |value: Value| LoadedProject {
            config: config(value),
            ..project.clone()
        };
        let empty_project = with_config(json!({}));
        let start = user(json!({"defaults": {"devCommand": "npm start"}}));
        let no_user = UserSettings::default();
        let cases: [(Option<&LoadedProject>, &UserSettings); 5] = [
            (None, &no_user),
            (None, &user_settings),
            (Some(&project), &user_settings),
            (Some(&project), &trusted),
            (Some(&empty_project), &start),
        ];
        for (p, u) in cases {
            assert_eq!(
                from_rows(&explain_settings(p, u, &context).rows),
                json_of(&resolve_settings(p, u, CONFIG_DIR).unwrap())
            );
        }

        // Built-in defaults only.
        let empty = explain_settings(None, &no_user, &context).rows;
        let row = find(&empty, "defaultAgent", None);
        assert_eq!(
            (row.value.as_str(), row.source),
            ("claude", SettingSource::BuiltInDefault)
        );
        assert_eq!(find(&empty, "devCommand", None).value, "dev");
        assert_eq!(find(&empty, "setupCommand", None).source, SettingSource::NotSet);
        assert_eq!(find(&empty, "actions", None).source, SettingSource::NotSet);
        assert_eq!(find(&empty, "worktree.location", None).value, "/state/worktrees/<name>");
        let hook = find(&empty, "worktree.hook", None);
        assert_eq!(
            (hook.value.as_str(), hook.note.as_deref()),
            ("on", Some("no .claude/scripts/create-worktree.sh detected"))
        );

        // Global only (repository untrusted): legacy dev_command, repo values pending; the repository's hook: false
        // applies anyway.
        let pending = explain_settings(Some(&project), &user_settings, &context).rows;
        let dev = find(&pending, "devCommand", None);
        assert_eq!(
            (dev.source, dev.value.as_str(), pending_value(dev)),
            (
                SettingSource::LegacyDevCommand,
                "legacy dev",
                Some("cd frontend && npm run dev")
            )
        );
        // The repository's suggestions apply without trust, so they are not pending.
        let workspace = find(&pending, "defaultWorkspace", None);
        assert_eq!(
            (workspace.source, workspace.value.as_str(), workspace.pending.as_ref()),
            (SettingSource::Repo, "new", None)
        );
        assert!(
            workspace
                .note
                .as_deref()
                .unwrap_or_default()
                .contains("applies without trust")
        );
        let lint = find(&pending, "actions", Some("lint"));
        assert_eq!(
            (lint.source, pending_value(lint)),
            (SettingSource::Global, Some("repo lint"))
        );
        let test = find(&pending, "actions", Some("test"));
        assert_eq!(
            (test.source, pending_value(test)),
            (SettingSource::NotSet, Some("pytest"))
        );
        assert_eq!(
            (
                find(&pending, "worktree.symlink", Some("frontend/node_modules")).source,
                find(&pending, "worktree.symlink", Some("node_modules")).source
            ),
            (SettingSource::NotSet, SettingSource::Global)
        );
        let hook = find(&pending, "worktree.hook", None);
        assert_eq!(
            (hook.value.as_str(), hook.source, hook.pending.as_ref()),
            ("off", SettingSource::Repo, None)
        );
        let branch_name = find(&pending, "worktree.branchName", None);
        assert_eq!(
            (branch_name.value.as_str(), branch_name.raw.as_ref()),
            (
                "tejas/<name>",
                Some(&StringOrBool::String(String::from("{user}/{name}")))
            )
        );
        assert_eq!(
            pending_value(find(&pending, "worktree.location", None)),
            Some("/dev/worktrees/<name>")
        );
        assert!(
            pending
                .iter()
                .all(|row| row.pending.as_ref().is_none_or(|pending| pending.same != Some(true)))
        );
        // An untrusted value equal to the one in effect (here the built-in dev and a global action) is marked `same`.
        let same = explain_settings(
            Some(&with_config(
                json!({"devCommand": "dev", "actions": {"lint": "npm run lint"}}),
            )),
            &user_settings,
            &context,
        )
        .rows;
        let dev = find(&same, "devCommand", None);
        assert_eq!(dev.source, SettingSource::LegacyDevCommand);
        assert_eq!(json_of(&dev.pending), json!({"value": "dev", "raw": "dev"}));
        assert_eq!(
            json_of(
                &find(
                    &explain_settings(Some(&with_config(json!({"devCommand": "dev"}))), &no_user, &context).rows,
                    "devCommand",
                    None
                )
                .pending
            ),
            json!({"value": "dev", "raw": "dev", "same": true})
        );
        assert_eq!(
            json_of(&find(&same, "actions", Some("lint")).pending),
            json!({"value": "npm run lint", "raw": "npm run lint", "same": true})
        );

        // Trusted: repository wins field by field and per entry, global entries it does not override stay.
        let merged = explain_settings(Some(&project), &trusted, &context).rows;
        assert_eq!(
            [
                find(&merged, "devCommand", None).source,
                find(&merged, "defaultAgent", None).source,
                find(&merged, "worktree.branchName", None).source,
                find(&merged, "worktree.branchFrom", None).source
            ],
            [
                SettingSource::Repo,
                SettingSource::Global,
                SettingSource::Global,
                SettingSource::BuiltInDefault
            ]
        );
        let lint = find(&merged, "actions", Some("lint"));
        assert_eq!(
            (
                lint.source,
                lint.note.as_deref(),
                find(&merged, "actions", Some("test")).source
            ),
            (
                SettingSource::Repo,
                Some("overrides global npm run lint"),
                SettingSource::Repo
            )
        );
        let files = find(&merged, "worktree.files", Some("backend/local.cfg"));
        assert_eq!(
            (
                files.value.as_str(),
                files.raw.as_ref(),
                find(&merged, "worktree.location", None).value.as_str()
            ),
            (
                "/dev/backend.cfg",
                Some(&StringOrBool::String(String::from("{repoParent}/backend.cfg"))),
                "/dev/worktrees/<name>"
            )
        );
        assert!(merged.iter().all(|row| row.pending.is_none()));
        // A non-legacy global devCommand, and invalid global defaults reported while the rest still resolves.
        let default_context = ExplainContext {
            config_dir: CONFIG_DIR.to_owned(),
            ..ExplainContext::default()
        };
        let global_dev = explain_settings(
            None,
            &user(json!({"dev_command": "x", "defaults": {"devCommand": "npm start"}})),
            &default_context,
        );
        assert_eq!(find(&global_dev.rows, "devCommand", None).source, SettingSource::Global);
        let broken_user = UserSettings {
            defaults: Some(json!({"setupCommand": ""})),
            ..trusted.clone()
        };
        let broken = explain_settings(Some(&project), &broken_user, &context);
        assert!(
            broken
                .global_error
                .as_deref()
                .unwrap_or_default()
                .starts_with("Invalid \"defaults\"")
        );
        assert_eq!(
            (
                find(&broken.rows, "devCommand", None).source,
                find(&broken.rows, "defaultWorkspace", None).value.as_str()
            ),
            (SettingSource::Repo, "new")
        );
    }

    // -----------------------------------------------------------------------------------------------------------
    // Edge cases beyond the TS tests

    #[test]
    fn trust_keeps_newest_first_capped_and_root_order() {
        let mut trusted = trust_project_config("/a", "one", None);
        trusted = trust_project_config("/b", "x", Some(&trusted));
        trusted = trust_project_config("/a", "two", Some(&trusted));
        assert_eq!(trusted.keys().collect::<Vec<_>>(), ["/a", "/b"]);
        assert_eq!(trusted["/a"], ["two", "one"]);
        trusted = trust_project_config("/a", "one", Some(&trusted));
        assert_eq!(trusted["/a"], ["one", "two"]);
        for i in 0..30 {
            trusted = trust_project_config("/a", &i.to_string(), Some(&trusted));
        }
        assert_eq!(trusted["/a"].len(), MAX_TRUSTED_FINGERPRINTS);
        assert!(!is_project_trusted("/a", "one", Some(&trusted)));
        assert!(is_project_trusted("/a", "29", Some(&trusted)));
    }

    #[test]
    fn hook_setting_layers() {
        let repo_off = config(json!({"worktree": {"hook": false}}));
        let repo_on = config(json!({"worktree": {"hook": true}}));
        let global_off = user(json!({"defaults": {"worktree": {"hook": false}}}));
        assert_eq!(
            hook_setting(&repo_off, &UserSettings::default()),
            HookSetting {
                enabled: false,
                by: Some(ConfigTargetKind::Repository)
            }
        );
        assert_eq!(
            hook_setting(&repo_on, &global_off),
            HookSetting {
                enabled: true,
                by: None
            }
        );
        assert_eq!(
            hook_setting(&ProjectConfig::default(), &global_off),
            HookSetting {
                enabled: false,
                by: Some(ConfigTargetKind::Global)
            }
        );
        // Read leniently: malformed defaults never disable anything.
        assert!(hook_setting(&ProjectConfig::default(), &user(json!({"defaults": "x"}))).enabled);
    }

    #[test]
    fn js_key_order_and_lone_surrogates() {
        let parsed =
            parse_project_config(r#"{"actions":{"b":"x","10":"y","2":"z"},"defaultAgent":"pi"}"#, None).unwrap();
        assert_eq!(
            serde_json::to_string(&parsed).unwrap(),
            r#"{"actions":{"2":"z","10":"y","b":"x"},"defaultAgent":"pi"}"#
        );
        let lone = parse_project_config(r#"{"actions":{"a":"\ud83d x \ude00"}}"#, None).unwrap();
        assert_eq!(lone.actions.as_ref().unwrap()["a"], "\u{FFFD} x \u{FFFD}");
        let pair = parse_project_config(r#"{"actions":{"a":"😀"}}"#, None).unwrap();
        assert_eq!(pair.actions.as_ref().unwrap()["a"], "😀");
        assert_eq!(
            parse_project_config("{", None).unwrap_err().to_string(),
            "Expected property name or '}' in JSON at position 1 (line 1 column 2)"
        );
        // Numbers V8 reads as Infinity are still rejected by the schema, not by the JSON parser.
        assert_eq!(
            parse_project_config(r#"{"defaultAgent":1e400}"#, None)
                .unwrap_err()
                .to_string(),
            "Invalid defaultAgent"
        );
    }

    #[test]
    fn resolved_key_order_follows_the_spreads() {
        let project = project("/r", json!({"worktree": {"hook": false}, "defaultAgent": "pi"}));
        let user_settings = user(
            json!({"dev_command": "legacy", "defaults": {"worktree": {"symlink": ["a"], "location": "/g/{name}"}, "actions": {"t": "x"}}}),
        );
        let resolved = resolve_settings(Some(&project), &user_settings, CONFIG_DIR).unwrap();
        assert_eq!(
            serde_json::to_string(&resolved).unwrap(),
            r#"{"worktree":{"location":"/g/{name}","symlink":["a"],"hook":false},"actions":{"t":"x"},"devCommand":"legacy","defaultAgent":"pi"}"#
        );
    }
}
