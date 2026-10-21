//! Persisted records: state.json, config.json, ui-state.json.
//!
//! TS writes all three with `JSON.stringify(value, null, 2)` plus a trailing newline ([`to_ts_file_text`]); key order
//! is insertion order, so the records keep the order they were read with ([`Ordered`], raw maps) and unknown keys
//! where TS keeps them. Where TS normalises a record on every read and write (ui-state.json entries, the AppConfig
//! view of config.json) the normalised struct has exactly TS's keys, and unknown keys are dropped as TS drops them.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_with::skip_serializing_none;

use crate::data::{
    JsNumber, MergeDetected, RightPaneTab, SessionRecord, WorktreeMarkers, WorktreeMergeMode, string_union,
};
use crate::order::Ordered;

/// `JSON.stringify(value)`.
pub fn to_ts_json<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<String> {
    serde_json::to_string(value)
}

/// `JSON.stringify(value, null, 2)` (serde_json's pretty printer uses the same two-space layout, `[]`/`{}` for
/// empty containers and the same string escapes).
pub fn to_ts_json_pretty<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<String> {
    serde_json::to_string_pretty(value)
}

/// What storage.ts/uiState.ts write to disk: `` `${JSON.stringify(value, null, 2)}\n` ``.
pub fn to_ts_file_text<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<String> {
    let mut text = to_ts_json_pretty(value)?;
    text.push('\n');
    Ok(text)
}

/// types.ts WorktreeRecord: one incarnation of a linked worktree (state.json `worktrees`). Order: the creation
/// literal (`{id, path, createdAt, baseSha?}`), then markers as they are set; carried as `Ordered<WorktreeRecord>`.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeRecord {
    pub id: String,
    pub path: String,
    pub created_at: String,
    pub base_sha: Option<String>,
    pub merged_at: Option<String>,
    pub merge_mode: Option<WorktreeMergeMode>,
    pub merge_target_branch: Option<String>,
    pub merge_source_ref: Option<String>,
    pub merge_source_sha: Option<String>,
    pub merge_marked_manually: Option<bool>,
    pub merge_detected: Option<MergeDetected>,
    pub deleted_at: Option<String>,
    pub merge_dismissed_tip: Option<String>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

impl WorktreeRecord {
    /// worktreeRecords.ts worktreeMarkers: the markers set on this record.
    pub fn markers(&self) -> WorktreeMarkers {
        WorktreeMarkers {
            merged_at: self.merged_at.clone(),
            merge_mode: self.merge_mode,
            merge_target_branch: self.merge_target_branch.clone(),
            merge_source_ref: self.merge_source_ref.clone(),
            merge_source_sha: self.merge_source_sha.clone(),
            merge_marked_manually: self.merge_marked_manually,
            merge_detected: self.merge_detected,
            deleted_at: self.deleted_at.clone(),
        }
    }
}

/// storage.ts InkState: state.json. TS rebuilds it as `{sessions, worktrees}` on every read, so other top-level keys
/// are not kept (and not written) here either.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StateFile {
    #[serde(default)]
    pub sessions: Vec<Ordered<SessionRecord>>,
    #[serde(default)]
    pub worktrees: Vec<Ordered<WorktreeRecord>>,
}

/// config.json as stored: any JSON object, in file order. storage.ts updateAppConfig writes it back with unknown keys
/// and unchanged values as they were, changed keys in place and new keys appended.
pub type RawConfig = IndexMap<String, Value>;

/// storage.ts AppConfig: the normalised view of config.json (normalizeAppConfig key order).
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    pub dev_command: Option<String>,
    /// The TS wheel multiplier (default 0.12), still read and kept as stored but no longer used: [`Self::scroll_speed`]
    /// replaced it with a new default (1), so a value tuned to the old one does not crawl.
    pub attach_scroll_sensitivity: Option<JsNumber>,
    /// Trusted project fingerprints per trust root, newest first.
    #[serde(rename = "trustedProjects")]
    pub trusted_projects: Option<IndexMap<String, Vec<String>>>,
    pub agent_hooks: Option<bool>,
    pub notifications: Option<bool>,
    /// The UI theme: `"dusk"` for the brand palette; anything else or unset: the terminal's colors (see `theme_mode`).
    pub theme: Option<String>,
    /// Not in the TS: the share of the terminal the sidebar takes, in percent (`<`/`>` save it).
    pub sidebar_width_percent: Option<JsNumber>,
    /// Not in the TS: unless false, Claude is launched scrolling one line per wheel event without its own
    /// acceleration, so the wheel feels the same for everyone and Deckhand's scroll sensitivity alone sets the speed.
    pub claude_scroll_override: Option<bool>,
    /// Not in the TS: how far the mouse wheel scrolls, when attached and in Preview focus (`deckhand_core::wheel`; 1,
    /// the default: each wheel event as the terminal sends it). Settings (`C`) and `+`/`-` in Preview focus save it.
    pub scroll_speed: Option<JsNumber>,
    /// Global project defaults (deckhand.json schema), kept as stored (`null` included).
    #[serde(default, deserialize_with = "crate::events::present_value")]
    pub defaults: Option<Value>,
}

impl AppConfig {
    /// storage.ts loadAppConfig + normalizeAppConfig: an empty file gives an empty config; otherwise wrong-typed values
    /// are ignored, a single fingerprint string (older versions) becomes a list, and `notifications` is a boolean.
    pub fn from_raw(raw: &RawConfig) -> Self {
        if raw.is_empty() {
            return AppConfig::default();
        }
        let trusted = match raw.get("trustedProjects") {
            Some(Value::Object(map)) => map
                .iter()
                .filter_map(|(root, value)| {
                    let list: Vec<String> = match value {
                        Value::String(single) => vec![single.clone()],
                        Value::Array(items) => items
                            .iter()
                            .filter_map(|item| item.as_str().map(str::to_owned))
                            .collect(),
                        _ => Vec::new(),
                    };
                    (!list.is_empty()).then(|| (root.clone(), list))
                })
                .collect(),
            _ => IndexMap::new(),
        };
        AppConfig {
            dev_command: raw.get("dev_command").and_then(Value::as_str).map(str::to_owned),
            attach_scroll_sensitivity: raw
                .get("attach_scroll_sensitivity")
                .and_then(Value::as_f64)
                .map(JsNumber),
            trusted_projects: Some(trusted),
            agent_hooks: raw.get("agent_hooks").and_then(Value::as_bool),
            notifications: Some(raw.get("notifications") == Some(&Value::Bool(true))),
            theme: raw.get("theme").and_then(Value::as_str).map(str::to_owned),
            sidebar_width_percent: raw.get("sidebar_width_percent").and_then(Value::as_f64).map(JsNumber),
            claude_scroll_override: raw.get("claude_scroll_override").and_then(Value::as_bool),
            scroll_speed: raw.get("scroll_speed").and_then(Value::as_f64).map(JsNumber),
            // `defaults: parsed.defaults` keeps even null.
            defaults: raw.get("defaults").cloned(),
        }
    }
}

string_union!(
    /// sessionFeatures.ts SessionFilter.
    SessionFilter {
        Active = "active",
        Running = "running",
        Attention = "attention",
        Exited = "exited",
        Archived = "archived",
        All = "all",
    }
);

/// uiState.ts UiState: one repository's UI preferences (normalizeUiState key order). Normalised on every read and
/// write, so unknown keys are dropped as in TS.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiState {
    pub selected_id: Option<String>,
    pub active_tab: Option<RightPaneTab>,
    pub session_filter: SessionFilter,
    pub session_query: String,
    pub sidebar_width: Option<JsNumber>,
    pub session_tabs: IndexMap<String, RightPaneTab>,
    pub collapsed_session_ids: Vec<String>,
    pub hidden_exited_session_ids: Vec<String>,
    /// Session ID -> when you last detached from its agent.
    pub read_replies: IndexMap<String, String>,
}

impl Default for UiState {
    fn default() -> Self {
        UiState::normalize(&Value::Null)
    }
}

/// The first `units` UTF-16 code units of `text` (JS `slice(0, units)`), never splitting a character: a surrogate
/// pair cut in half by JS is dropped whole here (Rust strings cannot hold a lone surrogate).
fn js_prefix(text: &str, units: usize) -> String {
    let mut used = 0;
    let mut end = 0;
    for (index, ch) in text.char_indices() {
        used += ch.len_utf16();
        if used > units {
            break;
        }
        end = index + ch.len_utf8();
    }
    text[..end].to_owned()
}

fn tab_of(value: &Value) -> Option<RightPaneTab> {
    let text = value.as_str()?;
    RightPaneTab::ALL.iter().copied().find(|tab| tab.as_str() == text)
}

impl UiState {
    /// uiState.ts normalizeUiState.
    pub fn normalize(raw: &Value) -> Self {
        let empty = serde_json::Map::new();
        let value = raw.as_object().unwrap_or(&empty);
        let strings = |key: &str| -> Vec<String> {
            match value.get(key) {
                Some(Value::Array(items)) => items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .take(10_000)
                    .collect(),
                _ => Vec::new(),
            }
        };
        let entries = |key: &str| -> Vec<(String, Value)> {
            match value.get(key) {
                Some(Value::Object(map)) => map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                // An array is an object too: Object.entries lists it by index.
                Some(Value::Array(items)) => items
                    .iter()
                    .enumerate()
                    .map(|(i, v)| (i.to_string(), v.clone()))
                    .collect(),
                _ => Vec::new(),
            }
        };
        let read: Vec<(String, String)> = entries("readReplies")
            .into_iter()
            .filter_map(|(id, at)| at.as_str().map(|at| (id, at.to_owned())))
            .collect();
        let skip = read.len().saturating_sub(1000);
        UiState {
            selected_id: value.get("selectedId").and_then(Value::as_str).map(str::to_owned),
            active_tab: value.get("activeTab").and_then(tab_of),
            session_filter: value
                .get("sessionFilter")
                .and_then(Value::as_str)
                .and_then(|text| {
                    SessionFilter::ALL
                        .iter()
                        .copied()
                        .find(|filter| filter.as_str() == text)
                })
                .unwrap_or(SessionFilter::Active),
            session_query: value
                .get("sessionQuery")
                .and_then(Value::as_str)
                .map(|text| js_prefix(text, 4096))
                .unwrap_or_default(),
            sidebar_width: value
                .get("sidebarWidth")
                .and_then(Value::as_f64)
                .filter(|n| n.is_finite())
                .map(JsNumber),
            session_tabs: entries("sessionTabs")
                .into_iter()
                .filter_map(|(id, tab)| tab_of(&tab).map(|tab| (id, tab)))
                .collect(),
            collapsed_session_ids: strings("collapsedSessionIds"),
            hidden_exited_session_ids: strings("hiddenExitedSessionIds"),
            read_replies: read.into_iter().skip(skip).collect(),
        }
    }
}

/// ui-state.json: repository root -> its UiState. Entries other than the one being saved are written back as read.
pub type UiStateFile = IndexMap<String, Value>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_text_matches_json_stringify_indent_two() {
        let value = serde_json::json!({"sessions": [], "worktrees": [{"id": "a", "list": [1, {"b": {}}]}]});
        assert_eq!(
            to_ts_file_text(&value).unwrap(),
            "{\n  \"sessions\": [],\n  \"worktrees\": [\n    {\n      \"id\": \"a\",\n      \"list\": [\n        1,\n        {\n          \"b\": {}\n        }\n      ]\n    }\n  ]\n}\n"
        );
    }

    #[test]
    fn ui_state_normalizes_like_ts() {
        let state = UiState::normalize(&serde_json::json!({
            "activeTab": "bogus", "sessionFilter": "exited", "sidebarWidth": 31.0, "extra": 1,
            "sessionTabs": {"a": "git", "b": "nope"}, "collapsedSessionIds": ["x", 3], "readReplies": {"s": "t", "u": 5}
        }));
        assert_eq!(
            to_ts_json(&state).unwrap(),
            r#"{"sessionFilter":"exited","sessionQuery":"","sidebarWidth":31,"sessionTabs":{"a":"git"},"collapsedSessionIds":["x"],"hiddenExitedSessionIds":[],"readReplies":{"s":"t"}}"#
        );
        assert_eq!(js_prefix("ab\u{1F600}c", 3), "ab");
        assert_eq!(js_prefix("ab\u{1F600}c", 4), "ab\u{1F600}");
    }

    #[test]
    fn app_config_normalizes_like_ts() {
        let raw: RawConfig = serde_json::from_str(r#"{"zeta":1,"trustedProjects":{"/r":"abc","/s":[],"/t":["x",2]},"notifications":"yes","agent_hooks":true}"#).unwrap();
        assert_eq!(
            to_ts_json(&AppConfig::from_raw(&raw)).unwrap(),
            r#"{"trustedProjects":{"/r":["abc"],"/t":["x"]},"agent_hooks":true,"notifications":false}"#
        );
        assert_eq!(to_ts_json(&AppConfig::from_raw(&RawConfig::new())).unwrap(), "{}");
    }
}
