//! Port of types.ts (client->daemon requests).
//!
//! [`Request`] is types.ts ClientRequest, internally tagged on `type`. Every variant is a named struct whose field
//! order is the order client.ts (or attach.ts / cli.ts) writes the keys, so a Rust client writes what the TS client
//! writes. Requests with a `requestId` get exactly one response ([`crate::events::ServerResponse`]); the input,
//! resize and detach messages of an attach get none.
//!
//! Decoding is as lenient as daemon.ts, which never validates a request: a case reads `message.<field>` and whatever
//! the client sent (nothing, `null`, another type) flows on to the code that uses it. So no field of a request fails
//! to decode; each is decoded into what the daemon needs to behave as TS does with that value:
//! - IDs and paths that are only looked up ([`lenient::string`]): a non-string never names a record, as in TS.
//! - Values TS checks itself (`typeof x === 'string'`, `Number.isInteger`, `=== true`, truthiness, `?? default`): the
//!   decoded form keeps exactly what that check needs ([`lenient`]).
//! - Values whose exact JS value matters (an unknown enum string the daemon reports, `cwd` passed on as given, a
//!   template-literal message): [`Loose`], which keeps absent, the known type or the raw JSON apart.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use serde_with::skip_serializing_none;

use crate::data::{
    ChangeGroup, ConfigTargetKind, JsNumber, MoveDirection, NoteSection, ProgramKey, ResolveMergeAction, RestartMode,
    StageMode, WorktreeMergeMode,
};

/// A request field as daemon.ts reads it: `undefined` ([`Loose::Absent`]), a value of the expected type
/// ([`Loose::Known`]), or anything else the client sent, `null` included ([`Loose::Other`]). Declare such fields with
/// `#[serde(default, skip_serializing_if = "Loose::is_absent")]`.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Loose<T> {
    #[default]
    Absent,
    Known(T),
    Other(Value),
}

impl<T> Loose<T> {
    /// The value when it has the expected type.
    pub fn known(&self) -> Option<&T> {
        match self {
            Loose::Known(value) => Some(value),
            _ => None,
        }
    }

    /// TS `=== undefined`.
    pub fn is_absent(&self) -> bool {
        matches!(self, Loose::Absent)
    }
}

impl<T: Serialize> Loose<T> {
    /// The JSON value (`None` when absent).
    pub fn to_value(&self) -> Option<Value> {
        match self {
            Loose::Absent => None,
            Loose::Known(value) => serde_json::to_value(value).ok(),
            Loose::Other(value) => Some(value.clone()),
        }
    }

    /// JS `String(value)` (`undefined` when absent), as a template literal or a string conversion writes it.
    pub fn js_string(&self) -> String {
        self.to_value()
            .map_or_else(|| "undefined".to_owned(), |value| js_string(&value))
    }

    /// JS truthiness.
    pub fn truthy(&self) -> bool {
        self.to_value().is_some_and(|value| js_truthy(&value))
    }
}

impl<T> From<T> for Loose<T> {
    fn from(value: T) -> Self {
        Loose::Known(value)
    }
}

impl From<&str> for Loose<String> {
    fn from(value: &str) -> Self {
        Loose::Known(value.to_owned())
    }
}

impl<T: Serialize> Serialize for Loose<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Loose::Absent => serializer.serialize_none(),
            Loose::Known(value) => value.serialize(serializer),
            Loose::Other(value) => value.serialize(serializer),
        }
    }
}

impl<'de, T: serde::de::DeserializeOwned> Deserialize<'de> for Loose<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(match T::deserialize(&value) {
            Ok(known) => Loose::Known(known),
            Err(_) => Loose::Other(value),
        })
    }
}

/// JS `String(value)` of a JSON value.
pub fn js_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => match (number.as_i64(), number.as_u64(), number.as_f64()) {
            (Some(int), _, _) => int.to_string(),
            (_, Some(int), _) => int.to_string(),
            (_, _, Some(float)) => JsNumber(float).to_js_string(),
            _ => number.to_string(),
        },
        Value::String(text) => text.clone(),
        // `Array.prototype.join(',')`: null and undefined elements are empty.
        Value::Array(items) => items
            .iter()
            .map(|item| if item.is_null() { String::new() } else { js_string(item) })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_owned(),
    }
}

/// JS truthiness of a JSON value.
pub fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// Node's `ERR_INVALID_ARG_TYPE` description of a value that is not the string an fs/path API wanted (`Received
/// undefined`, `Received type number (5)`, `Received an instance of Object`), `None` for absent.
pub fn node_received(value: Option<&Value>) -> String {
    match value {
        None => "Received undefined".to_owned(),
        Some(Value::Null) => "Received null".to_owned(),
        Some(Value::Bool(flag)) => format!("Received type boolean ({flag})"),
        Some(Value::Number(_)) => format!("Received type number ({})", js_string(value.unwrap_or(&Value::Null))),
        Some(Value::String(text)) => format!("Received type string ('{text}')"),
        Some(Value::Array(_)) => "Received an instance of Array".to_owned(),
        Some(Value::Object(_)) => "Received an instance of Object".to_owned(),
    }
}

/// `deserialize_with` decoders for request fields (each with `#[serde(default)]`, which stands for `undefined`).
pub mod lenient {
    use super::{Deserialize, Deserializer, Value, js_string, js_truthy};
    use crate::data::{ConfigTargetKind, MoveDirection, ResolveMergeAction};

    /// An ID or path that is only looked up or compared: a string as sent; any other truthy value as JS `String()`
    /// (which names no record, as the raw value does not in TS); a falsy one is empty (`undefined` here).
    pub fn string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(match value {
            Value::String(text) => text,
            other if js_truthy(&other) => js_string(&other),
            _ => String::new(),
        })
    }

    /// [`string`] for an optional ID (`if (!sessionId)`): a falsy value is `None`.
    pub fn opt_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
        string(deserializer).map(|text| Some(text).filter(|text| !text.is_empty()))
    }

    /// The `requestId` echoed in the response: a string as sent, anything else as JS `String()`.
    pub fn request_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(match value {
            Value::String(text) => text,
            other => js_string(&other),
        })
    }

    /// A value TS checks with `typeof x === 'string'` (or only uses when it is one): `None` for anything else.
    pub fn opt_string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(match value {
            Value::String(text) => Some(text),
            _ => None,
        })
    }

    /// An array TS checks element by element (`Array.isArray(paths) && paths.every(...)`): `None` unless it is an
    /// array of strings.
    pub fn string_list<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Vec<String>>, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(Vec::<String>::deserialize(&value).ok())
    }

    /// A flag used with `??` and then as a boolean: `None` for `undefined`/`null`, else its JS truthiness.
    pub fn truthy<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<bool>, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok((!value.is_null()).then(|| js_truthy(&value)))
    }

    /// A flag only tested for truthiness (`message.done ? ... : ...`).
    pub fn flag<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
        Value::deserialize(deserializer).map(|value| js_truthy(&value))
    }

    /// A flag tested with `=== true`.
    pub fn strict_true<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<bool>, D::Error> {
        Value::deserialize(deserializer).map(|value| Some(value == Value::Bool(true)))
    }

    /// A size TS gives to clampSize/clampNonNegative (`Number.isFinite(value) ? Math.floor(value) : fallback`):
    /// a number, floored; `None` for anything else (the fallback).
    pub fn size<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<i64>, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(match value {
            Value::Number(number) => number.as_i64().or_else(|| {
                number.as_f64().map(|float| {
                    // Saturating: a size beyond i64 is clamped by the daemon anyway.
                    #[allow(clippy::cast_possible_truncation)]
                    let floored = float.floor() as i64;
                    floored
                })
            }),
            _ => None,
        })
    }

    /// A number TS checks with `Number.isInteger`: `None` unless it is integral.
    pub fn integer<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<i64>, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(match value {
            Value::Number(number) => number.as_i64().or_else(|| {
                number
                    .as_f64()
                    .filter(|float| float.fract() == 0.0 && float.abs() < 9.0e15)
                    // Integral and within ±2^53: exact.
                    .map(|float| {
                        #[allow(clippy::cast_possible_truncation)]
                        let int = float as i64;
                        int
                    })
            }),
            _ => None,
        })
    }

    /// Any value present (`null` included); `None` only for `undefined`.
    pub fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
        Value::deserialize(deserializer).map(Some)
    }

    /// save-config `target`: `message.target === 'global'` picks the global file, anything else the repository's.
    pub fn config_target<'de, D: Deserializer<'de>>(deserializer: D) -> Result<ConfigTargetKind, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(if value == "global" {
            ConfigTargetKind::Global
        } else {
            ConfigTargetKind::Repository
        })
    }

    /// reorder-session `direction`: `direction === 'up'` moves up, anything else down.
    pub fn direction<'de, D: Deserializer<'de>>(deserializer: D) -> Result<MoveDirection, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(if value == "up" {
            MoveDirection::Up
        } else {
            MoveDirection::Down
        })
    }

    /// resolve-merge `action`: `action === 'abort'` aborts, anything else keeps.
    pub fn merge_action<'de, D: Deserializer<'de>>(deserializer: D) -> Result<ResolveMergeAction, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(if value == "abort" {
            ResolveMergeAction::Abort
        } else {
            ResolveMergeAction::Keep
        })
    }

    /// `undefined` for the defaults above.
    pub fn repository() -> ConfigTargetKind {
        ConfigTargetKind::Repository
    }

    /// `undefined` direction: down.
    pub fn down() -> MoveDirection {
        MoveDirection::Down
    }

    /// `undefined` action: keep.
    pub fn keep() -> ResolveMergeAction {
        ResolveMergeAction::Keep
    }
}

/// A request naming only its ID (`ping`, `shutdown`, `list`).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestOnly {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
}

/// A request about the repository or checkout `cwd` belongs to (passed on as given: see the daemon's cases).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CwdRequest {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub cwd: Loose<String>,
}

/// A request about one session.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRequest {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveConfig {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default = "lenient::repository", deserialize_with = "lenient::config_target")]
    pub target: ConfigTargetKind,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub cwd: Loose<String>,
    /// Only text is saved (`Configuration must be text of at most 64 KiB` otherwise).
    #[serde(default, deserialize_with = "lenient::opt_string")]
    pub raw: Option<String>,
    /// `string | null` (`Loose::Other(null)`: the file must not exist yet); anything else is an invalid revision.
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub revision: Loose<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeCandidateSizes {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub cwd: Loose<String>,
    /// `None` unless an array of strings (refused as TS refuses it).
    #[serde(default, deserialize_with = "lenient::string_list")]
    pub paths: Option<Vec<String>>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustProject {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub cwd: Loose<String>,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub fingerprint: Loose<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSummaryRequest {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
    #[serde(default, deserialize_with = "lenient::truthy")]
    pub include_pr: Option<bool>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectCleanup {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
    #[serde(default, deserialize_with = "lenient::truthy")]
    pub delete_branch: Option<bool>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePr {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
    /// The branch the UI showed; a non-string is no expectation.
    #[serde(default, deserialize_with = "lenient::opt_string")]
    pub branch: Option<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveSession {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
    #[serde(default, deserialize_with = "lenient::flag")]
    pub archived: bool,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportHandoff {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
    #[serde(default, deserialize_with = "lenient::truthy")]
    pub include_output: Option<bool>,
}

/// cli.ts `hook`: an agent signal relayed from the agent's hook (`payload` is the reduced hook input).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentHook {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub launch_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub token: String,
    #[serde(default)]
    pub payload: Value,
    #[serde(default, deserialize_with = "lenient::size")]
    pub sent_at: Option<i64>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunAction {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
    /// Looked up as an own key of the actions (`Object.hasOwn` converts it with `String()`).
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub action: Loose<String>,
    #[serde(default, deserialize_with = "lenient::size")]
    pub cols: Option<i64>,
    #[serde(default, deserialize_with = "lenient::size")]
    pub rows: Option<i64>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentVersionsRequest {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::truthy")]
    pub refresh: Option<bool>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAgent {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    /// Checked by the daemon (`Unknown agent <program>`).
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub program: Loose<ProgramKey>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscribe {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub repo_root: Loose<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectWorktree {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub cwd: Loose<String>,
    /// Must be an absolute path (`Invalid worktree path`).
    #[serde(default, deserialize_with = "lenient::string")]
    pub path: String,
    #[serde(default, deserialize_with = "lenient::truthy")]
    pub delete_branch: Option<bool>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteWorktree {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub cwd: Loose<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub path: String,
    #[serde(default, deserialize_with = "lenient::opt_string")]
    pub branch: Option<String>,
    #[serde(default, deserialize_with = "lenient::truthy")]
    pub delete_branch: Option<bool>,
    #[serde(default, deserialize_with = "lenient::truthy")]
    pub stop_sessions: Option<bool>,
    #[serde(default, deserialize_with = "lenient::truthy")]
    pub allow_data_loss: Option<bool>,
    #[serde(default, deserialize_with = "lenient::truthy")]
    pub send_open_items: Option<bool>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchPreview {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::opt_id")]
    pub session_id: Option<String>,
    #[serde(default, deserialize_with = "lenient::size")]
    pub cols: Option<i64>,
    #[serde(default, deserialize_with = "lenient::size")]
    pub rows: Option<i64>,
    #[serde(default, deserialize_with = "lenient::size")]
    pub scroll_offset: Option<i64>,
}

/// `watch-terminal`, `watch-git`, `watch-dev`, `watch-action`: no `sessionId` stops watching.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchPane {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::opt_id")]
    pub session_id: Option<String>,
    #[serde(default, deserialize_with = "lenient::size")]
    pub cols: Option<i64>,
    #[serde(default, deserialize_with = "lenient::size")]
    pub rows: Option<i64>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchChanges {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::opt_id")]
    pub session_id: Option<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangesDiff {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub group: Loose<ChangeGroup>,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub path: Loose<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeStage {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub mode: Loose<StageMode>,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub group: Loose<ChangeGroup>,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub path: Loose<String>,
}

/// `start-dev`.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSizeRequest {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
    #[serde(default, deserialize_with = "lenient::size")]
    pub cols: Option<i64>,
    #[serde(default, deserialize_with = "lenient::size")]
    pub rows: Option<i64>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveNote {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
    /// `'session'` is the session's own note; anything else its shared one (`'shared'` adds its checks).
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub section: Loose<NoteSection>,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub note_id: Loose<String>,
    /// `Invalid note` unless text.
    #[serde(default, deserialize_with = "lenient::opt_string")]
    pub text: Option<String>,
    #[serde(default, deserialize_with = "lenient::opt_string")]
    pub revision: Option<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenNote {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub section: Loose<NoteSection>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskOpRequest {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub cwd: Loose<String>,
    /// tasks.ts clientTaskOp checks it (after the repository is resolved).
    #[serde(default)]
    pub op: Value,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendOpenItems {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub section: Loose<NoteSection>,
    /// Absent: no revision check; any other value must equal the note's.
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub revision: Loose<String>,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub note_id: Loose<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromoteNoteItem {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub section: Loose<NoteSection>,
    /// `Invalid line` unless an integer >= 0.
    #[serde(default, deserialize_with = "lenient::integer")]
    pub line: Option<i64>,
    /// Must equal the note's revision (a non-string never does).
    #[serde(default, deserialize_with = "lenient::opt_string")]
    pub revision: Option<String>,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub note_id: Loose<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReturnTaskToNote {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub cwd: Loose<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub task_id: String,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskToNote {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub cwd: Loose<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub task_id: String,
    /// The work whose note gets the task: `wt:<id>` or `s:<id>` (`Invalid work` unless text).
    #[serde(default, deserialize_with = "lenient::opt_string")]
    pub section: Option<String>,
}

/// `create`: `input` is checked by the daemon in createSession's order (a [`crate::data::CreateSessionInput`] when
/// well formed).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Create {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::present")]
    pub input: Option<Value>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReorderSession {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
    #[serde(default = "lenient::down", deserialize_with = "lenient::direction")]
    pub direction: MoveDirection,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Restart {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
    #[serde(default, deserialize_with = "lenient::size")]
    pub cols: Option<i64>,
    #[serde(default, deserialize_with = "lenient::size")]
    pub rows: Option<i64>,
    /// `mode ?? 'resume'`; an unknown mode is neither resume nor fresh to restartSession.
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub mode: Loose<RestartMode>,
    #[serde(default, deserialize_with = "lenient::opt_string")]
    pub project_fingerprint: Option<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Kill {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
    #[serde(default, deserialize_with = "lenient::truthy")]
    pub delete_worktree: Option<bool>,
    #[serde(default, deserialize_with = "lenient::truthy")]
    pub delete_branch: Option<bool>,
    #[serde(default, deserialize_with = "lenient::truthy")]
    pub force: Option<bool>,
    #[serde(default, deserialize_with = "lenient::truthy")]
    pub allow_data_loss: Option<bool>,
    #[serde(default, deserialize_with = "lenient::strict_true")]
    pub send_open_items: Option<bool>,
}

/// `merge-preview`, `mark-session-merged`.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTargetRequest {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub target_cwd: Loose<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeWorktree {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub mode: Loose<WorktreeMergeMode>,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub target_cwd: Loose<String>,
    #[serde(default, deserialize_with = "lenient::truthy")]
    pub commit_first: Option<bool>,
    #[serde(default, deserialize_with = "lenient::strict_true")]
    pub send_open_items: Option<bool>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveMerge {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub target_cwd: Loose<String>,
    #[serde(default = "lenient::keep", deserialize_with = "lenient::merge_action")]
    pub action: ResolveMergeAction,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSessionDone {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
    #[serde(default, deserialize_with = "lenient::flag")]
    pub done: bool,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Remove {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
    #[serde(default, deserialize_with = "lenient::truthy")]
    pub move_open_items: Option<bool>,
}

/// `attach`, `attach-terminal`, `attach-git`, `attach-dev`, `attach-action` (attach.ts literal order).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attach {
    #[serde(default, deserialize_with = "lenient::request_id")]
    pub request_id: String,
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
    #[serde(default, deserialize_with = "lenient::size")]
    pub cols: Option<i64>,
    #[serde(default, deserialize_with = "lenient::size")]
    pub rows: Option<i64>,
}

/// Keys typed into an attached PTY (no response).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
    /// Forwarded as sent; the agent's input case also reads it as text.
    #[serde(default, skip_serializing_if = "Loose::is_absent")]
    pub data: Loose<String>,
}

/// An attached terminal's new size (no response; forwarded as sent, absent sizes omitted).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resize {
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
    #[serde(default, deserialize_with = "lenient::size")]
    pub cols: Option<i64>,
    #[serde(default, deserialize_with = "lenient::size")]
    pub rows: Option<i64>,
}

/// Ends an attach (answered by a `*detached` event, not a response).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Detach {
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: String,
}

/// types.ts ClientRequest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Request {
    #[serde(rename = "ping")]
    Ping(RequestOnly),
    #[serde(rename = "shutdown")]
    Shutdown(RequestOnly),
    #[serde(rename = "project-info")]
    ProjectInfo(CwdRequest),
    #[serde(rename = "save-config")]
    SaveConfig(SaveConfig),
    #[serde(rename = "settings-info")]
    SettingsInfo(CwdRequest),
    #[serde(rename = "worktree-candidates")]
    WorktreeCandidates(CwdRequest),
    #[serde(rename = "global-link-candidates")]
    GlobalLinkCandidates(CwdRequest),
    #[serde(rename = "worktree-candidate-sizes")]
    WorktreeCandidateSizes(WorktreeCandidateSizes),
    #[serde(rename = "trust-project")]
    TrustProject(TrustProject),
    #[serde(rename = "workspace-summary")]
    WorkspaceSummary(WorkspaceSummaryRequest),
    #[serde(rename = "inspect-cleanup")]
    InspectCleanup(InspectCleanup),
    #[serde(rename = "create-pr")]
    CreatePr(CreatePr),
    #[serde(rename = "archive-session")]
    ArchiveSession(ArchiveSession),
    #[serde(rename = "export-handoff")]
    ExportHandoff(ExportHandoff),
    #[serde(rename = "agent-hook")]
    AgentHook(AgentHook),
    #[serde(rename = "cancel-start")]
    CancelStart(SessionRequest),
    #[serde(rename = "run-action")]
    RunAction(RunAction),
    #[serde(rename = "list")]
    List(RequestOnly),
    #[serde(rename = "agent-versions")]
    AgentVersions(AgentVersionsRequest),
    #[serde(rename = "update-agent")]
    UpdateAgent(UpdateAgent),
    #[serde(rename = "subscribe")]
    Subscribe(Subscribe),
    #[serde(rename = "list-worktrees")]
    ListWorktrees(CwdRequest),
    #[serde(rename = "worktree-overview")]
    WorktreeOverview(CwdRequest),
    #[serde(rename = "inspect-worktree")]
    InspectWorktree(InspectWorktree),
    #[serde(rename = "delete-worktree")]
    DeleteWorktree(DeleteWorktree),
    #[serde(rename = "watch-preview")]
    WatchPreview(WatchPreview),
    #[serde(rename = "watch-terminal")]
    WatchTerminal(WatchPane),
    #[serde(rename = "watch-git")]
    WatchGit(WatchPane),
    #[serde(rename = "watch-dev")]
    WatchDev(WatchPane),
    #[serde(rename = "watch-action")]
    WatchAction(WatchPane),
    #[serde(rename = "watch-changes")]
    WatchChanges(WatchChanges),
    #[serde(rename = "changes-diff")]
    ChangesDiff(ChangesDiff),
    #[serde(rename = "change-stage")]
    ChangeStage(ChangeStage),
    #[serde(rename = "start-dev")]
    StartDev(SessionSizeRequest),
    #[serde(rename = "stop-dev")]
    StopDev(SessionRequest),
    #[serde(rename = "stop-action")]
    StopAction(SessionRequest),
    #[serde(rename = "save-note")]
    SaveNote(SaveNote),
    #[serde(rename = "open-note")]
    OpenNote(OpenNote),
    #[serde(rename = "watch-tasks")]
    WatchTasks(CwdRequest),
    #[serde(rename = "task-op")]
    TaskOp(TaskOpRequest),
    #[serde(rename = "open-tasks")]
    OpenTasks(CwdRequest),
    #[serde(rename = "send-open-items")]
    SendOpenItems(SendOpenItems),
    #[serde(rename = "promote-note-item")]
    PromoteNoteItem(PromoteNoteItem),
    #[serde(rename = "return-task-to-note")]
    ReturnTaskToNote(ReturnTaskToNote),
    #[serde(rename = "task-to-note")]
    TaskToNote(TaskToNote),
    #[serde(rename = "list-branches")]
    ListBranches(CwdRequest),
    #[serde(rename = "create")]
    Create(Create),
    #[serde(rename = "reorder-session")]
    ReorderSession(ReorderSession),
    #[serde(rename = "restart")]
    Restart(Restart),
    #[serde(rename = "kill")]
    Kill(Kill),
    #[serde(rename = "merge-preview")]
    MergePreview(SessionTargetRequest),
    #[serde(rename = "merge-worktree")]
    MergeWorktree(MergeWorktree),
    #[serde(rename = "resolve-merge")]
    ResolveMerge(ResolveMerge),
    #[serde(rename = "mark-session-merged")]
    MarkSessionMerged(SessionTargetRequest),
    #[serde(rename = "set-session-done")]
    SetSessionDone(SetSessionDone),
    #[serde(rename = "remove")]
    Remove(Remove),
    #[serde(rename = "attach")]
    Attach(Attach),
    #[serde(rename = "input")]
    Input(Input),
    #[serde(rename = "resize")]
    Resize(Resize),
    #[serde(rename = "detach")]
    Detach(Detach),
    #[serde(rename = "attach-terminal")]
    AttachTerminal(Attach),
    #[serde(rename = "terminal-input")]
    TerminalInput(Input),
    #[serde(rename = "terminal-resize")]
    TerminalResize(Resize),
    #[serde(rename = "terminal-detach")]
    TerminalDetach(Detach),
    #[serde(rename = "attach-git")]
    AttachGit(Attach),
    #[serde(rename = "git-input")]
    GitInput(Input),
    #[serde(rename = "git-resize")]
    GitResize(Resize),
    #[serde(rename = "git-detach")]
    GitDetach(Detach),
    #[serde(rename = "attach-dev")]
    AttachDev(Attach),
    #[serde(rename = "dev-input")]
    DevInput(Input),
    #[serde(rename = "dev-resize")]
    DevResize(Resize),
    #[serde(rename = "dev-detach")]
    DevDetach(Detach),
    #[serde(rename = "attach-action")]
    AttachAction(Attach),
    #[serde(rename = "action-input")]
    ActionInput(Input),
    #[serde(rename = "action-resize")]
    ActionResize(Resize),
    #[serde(rename = "action-detach")]
    ActionDetach(Detach),
}

macro_rules! request_table {
    ($($variant:ident => $tag:literal),+ $(,)?) => {
        impl Request {
            /// Every `type` tag, in types.ts order.
            pub const TYPES: &'static [&'static str] = &[$($tag),+];

            /// The request's `type` tag.
            pub fn type_name(&self) -> &'static str {
                match self { $(Request::$variant(_) => $tag,)+ }
            }
        }
    };
}

request_table! {
    Ping => "ping", Shutdown => "shutdown", ProjectInfo => "project-info", SaveConfig => "save-config",
    SettingsInfo => "settings-info", WorktreeCandidates => "worktree-candidates",
    GlobalLinkCandidates => "global-link-candidates", WorktreeCandidateSizes => "worktree-candidate-sizes",
    TrustProject => "trust-project", WorkspaceSummary => "workspace-summary", InspectCleanup => "inspect-cleanup",
    CreatePr => "create-pr", ArchiveSession => "archive-session", ExportHandoff => "export-handoff",
    AgentHook => "agent-hook", CancelStart => "cancel-start", RunAction => "run-action", List => "list",
    AgentVersions => "agent-versions", UpdateAgent => "update-agent", Subscribe => "subscribe",
    ListWorktrees => "list-worktrees", WorktreeOverview => "worktree-overview", InspectWorktree => "inspect-worktree",
    DeleteWorktree => "delete-worktree", WatchPreview => "watch-preview", WatchTerminal => "watch-terminal",
    WatchGit => "watch-git", WatchDev => "watch-dev", WatchAction => "watch-action", WatchChanges => "watch-changes",
    ChangesDiff => "changes-diff", ChangeStage => "change-stage", StartDev => "start-dev", StopDev => "stop-dev",
    StopAction => "stop-action", SaveNote => "save-note", OpenNote => "open-note", WatchTasks => "watch-tasks",
    TaskOp => "task-op", OpenTasks => "open-tasks", SendOpenItems => "send-open-items",
    PromoteNoteItem => "promote-note-item", ReturnTaskToNote => "return-task-to-note", TaskToNote => "task-to-note",
    ListBranches => "list-branches", Create => "create", ReorderSession => "reorder-session", Restart => "restart",
    Kill => "kill", MergePreview => "merge-preview", MergeWorktree => "merge-worktree",
    ResolveMerge => "resolve-merge", MarkSessionMerged => "mark-session-merged", SetSessionDone => "set-session-done",
    Remove => "remove", Attach => "attach", Input => "input", Resize => "resize", Detach => "detach",
    AttachTerminal => "attach-terminal", TerminalInput => "terminal-input", TerminalResize => "terminal-resize",
    TerminalDetach => "terminal-detach", AttachGit => "attach-git", GitInput => "git-input", GitResize => "git-resize",
    GitDetach => "git-detach", AttachDev => "attach-dev", DevInput => "dev-input", DevResize => "dev-resize",
    DevDetach => "dev-detach", AttachAction => "attach-action", ActionInput => "action-input",
    ActionResize => "action-resize", ActionDetach => "action-detach",
}

impl Request {
    /// The request's ID; `None` for the attach input/resize/detach messages, which get no response.
    pub fn request_id(&self) -> Option<&str> {
        use Request as R;
        Some(match self {
            R::Ping(r) | R::Shutdown(r) | R::List(r) => &r.request_id,
            R::ProjectInfo(r)
            | R::SettingsInfo(r)
            | R::WorktreeCandidates(r)
            | R::GlobalLinkCandidates(r)
            | R::ListWorktrees(r)
            | R::WorktreeOverview(r)
            | R::WatchTasks(r)
            | R::OpenTasks(r)
            | R::ListBranches(r) => &r.request_id,
            R::SaveConfig(r) => &r.request_id,
            R::WorktreeCandidateSizes(r) => &r.request_id,
            R::TrustProject(r) => &r.request_id,
            R::WorkspaceSummary(r) => &r.request_id,
            R::InspectCleanup(r) => &r.request_id,
            R::CreatePr(r) => &r.request_id,
            R::ArchiveSession(r) => &r.request_id,
            R::ExportHandoff(r) => &r.request_id,
            R::AgentHook(r) => &r.request_id,
            R::CancelStart(r) | R::StopDev(r) | R::StopAction(r) => &r.request_id,
            R::RunAction(r) => &r.request_id,
            R::AgentVersions(r) => &r.request_id,
            R::UpdateAgent(r) => &r.request_id,
            R::Subscribe(r) => &r.request_id,
            R::InspectWorktree(r) => &r.request_id,
            R::DeleteWorktree(r) => &r.request_id,
            R::WatchPreview(r) => &r.request_id,
            R::WatchTerminal(r) | R::WatchGit(r) | R::WatchDev(r) | R::WatchAction(r) => &r.request_id,
            R::WatchChanges(r) => &r.request_id,
            R::ChangesDiff(r) => &r.request_id,
            R::ChangeStage(r) => &r.request_id,
            R::StartDev(r) => &r.request_id,
            R::SaveNote(r) => &r.request_id,
            R::OpenNote(r) => &r.request_id,
            R::TaskOp(r) => &r.request_id,
            R::SendOpenItems(r) => &r.request_id,
            R::PromoteNoteItem(r) => &r.request_id,
            R::ReturnTaskToNote(r) => &r.request_id,
            R::TaskToNote(r) => &r.request_id,
            R::Create(r) => &r.request_id,
            R::ReorderSession(r) => &r.request_id,
            R::Restart(r) => &r.request_id,
            R::Kill(r) => &r.request_id,
            R::MergePreview(r) | R::MarkSessionMerged(r) => &r.request_id,
            R::MergeWorktree(r) => &r.request_id,
            R::ResolveMerge(r) => &r.request_id,
            R::SetSessionDone(r) => &r.request_id,
            R::Remove(r) => &r.request_id,
            R::Attach(r) | R::AttachTerminal(r) | R::AttachGit(r) | R::AttachDev(r) | R::AttachAction(r) => {
                &r.request_id
            }
            R::Input(_)
            | R::Resize(_)
            | R::Detach(_)
            | R::TerminalInput(_)
            | R::TerminalResize(_)
            | R::TerminalDetach(_)
            | R::GitInput(_)
            | R::GitResize(_)
            | R::GitDetach(_)
            | R::DevInput(_)
            | R::DevResize(_)
            | R::DevDetach(_)
            | R::ActionInput(_)
            | R::ActionResize(_)
            | R::ActionDetach(_) => return None,
        })
    }
}

/// daemon.ts accepts a line as a request when it parses to an object with a string `type` (anything else closes the
/// connection); an unknown `type` is then ignored without a response. This returns that `type`.
pub fn request_type(value: &Value) -> Option<&str> {
    value.as_object()?.get("type")?.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_the_client_key_order() {
        let request = Request::Restart(Restart {
            request_id: "r".into(),
            session_id: "s".into(),
            cols: Some(80),
            rows: Some(24),
            mode: RestartMode::Fresh.into(),
            project_fingerprint: None,
        });
        assert_eq!(
            serde_json::to_string(&request).unwrap(),
            r#"{"type":"restart","requestId":"r","sessionId":"s","cols":80,"rows":24,"mode":"fresh"}"#
        );
        assert_eq!(request.type_name(), "restart");
        assert_eq!(request.request_id(), Some("r"));
    }

    #[test]
    fn save_config_writes_a_null_revision() {
        let request = Request::SaveConfig(SaveConfig {
            request_id: "r".into(),
            target: ConfigTargetKind::Global,
            cwd: "/x".into(),
            raw: Some("{}\n".into()),
            revision: Loose::Other(Value::Null),
        });
        let text =
            r#"{"type":"save-config","requestId":"r","target":"global","cwd":"/x","raw":"{}\n","revision":null}"#;
        assert_eq!(serde_json::to_string(&request).unwrap(), text);
        assert_eq!(serde_json::from_str::<Request>(text).unwrap(), request);
    }

    #[test]
    fn known_types_decode_whatever_fields_they_carry() {
        use serde_json::json;
        // TS reads `message.<field>` and lets any value through: no known type is refused for its fields.
        for kind in Request::TYPES {
            for body in [
                json!({"type": kind}),
                json!({"type": kind, "requestId": 5, "sessionId": 5, "cwd": 5, "path": 5, "repoRoot": 5,
                       "cols": "x", "rows": null, "section": "bogus", "mode": "bogus", "group": "bogus",
                       "direction": "bogus", "action": "bogus", "program": "bogus", "target": "bogus", "op": "x",
                       "line": 1.5, "done": "yes", "archived": 1, "paths": "x", "input": 5, "payload": 5,
                       "data": 5, "sentAt": "x", "deleteBranch": [], "sendOpenItems": 1}),
            ] {
                let request = serde_json::from_value::<Request>(body.clone());
                assert!(request.is_ok(), "{kind}: {request:?}");
            }
        }
    }

    #[test]
    fn fields_keep_what_the_ts_checks_need() {
        use serde_json::json;
        let decode = |body: Value| serde_json::from_value::<Request>(body).unwrap();
        // An unknown agent is kept for `Unknown agent <program>`.
        let Request::UpdateAgent(update) = decode(json!({"type": "update-agent", "requestId": "r", "program": "nope"}))
        else {
            panic!()
        };
        assert_eq!(update.program, Loose::Other(json!("nope")));
        assert_eq!(update.program.js_string(), "nope");
        let Request::UpdateAgent(update) = decode(json!({"type": "update-agent", "requestId": "r"})) else {
            panic!()
        };
        assert_eq!(update.program.js_string(), "undefined");
        // Number.isInteger: a fraction is no line (not floored).
        let Request::PromoteNoteItem(promote) = decode(json!({"type": "promote-note-item", "line": 1.5})) else {
            panic!()
        };
        assert_eq!(promote.line, None);
        let Request::PromoteNoteItem(promote) = decode(json!({"type": "promote-note-item", "line": 2.0})) else {
            panic!()
        };
        assert_eq!(promote.line, Some(2));
        // clampSize floors; a non-number is the fallback.
        let Request::WatchPreview(watch) =
            decode(json!({"type": "watch-preview", "cols": 80.7, "rows": "24", "scrollOffset": 1.5}))
        else {
            panic!()
        };
        assert_eq!((watch.cols, watch.rows, watch.scroll_offset), (Some(80), None, Some(1)));
        // `=== true`, truthiness and `?? default` stay apart.
        let Request::Kill(kill) =
            decode(json!({"type": "kill", "sendOpenItems": 1, "deleteWorktree": "x", "force": null}))
        else {
            panic!()
        };
        assert_eq!(
            (kill.send_open_items, kill.delete_worktree, kill.force),
            (Some(false), Some(true), None)
        );
        // The task change is checked by the daemon, after the repository.
        let Request::TaskOp(op) = decode(json!({"type": "task-op", "cwd": 5})) else {
            panic!()
        };
        assert_eq!((op.cwd, op.op), (Loose::Other(json!(5)), Value::Null));
        // A missing requestId is empty (a success is answered without one).
        let Request::List(list) = decode(json!({"type": "list"})) else {
            panic!()
        };
        assert_eq!(list.request_id, "");
        // Anything but 'global' saves the repository file; anything but 'up' moves down; anything but 'abort' keeps.
        let Request::SaveConfig(save) = decode(json!({"type": "save-config", "target": "bogus"})) else {
            panic!()
        };
        assert_eq!(save.target, ConfigTargetKind::Repository);
        let Request::ReorderSession(reorder) = decode(json!({"type": "reorder-session", "direction": "bogus"})) else {
            panic!()
        };
        assert_eq!(reorder.direction, MoveDirection::Down);
        let Request::ResolveMerge(resolve) = decode(json!({"type": "resolve-merge", "action": 5})) else {
            panic!()
        };
        assert_eq!(resolve.action, ResolveMergeAction::Keep);
    }

    #[test]
    fn js_conversions_match_javascript() {
        use serde_json::json;
        assert_eq!(js_string(&json!(null)), "null");
        assert_eq!(js_string(&json!(5)), "5");
        assert_eq!(js_string(&json!(1.5)), "1.5");
        assert_eq!(js_string(&json!([1, null, "a"])), "1,,a");
        assert_eq!(js_string(&json!({"a": 1})), "[object Object]");
        assert!(!js_truthy(&json!(0)) && !js_truthy(&json!("")) && js_truthy(&json!([])));
        assert_eq!(node_received(None), "Received undefined");
        assert_eq!(node_received(Some(&json!(5))), "Received type number (5)");
        // Absent fields are not written.
        let request = Request::UpdateAgent(UpdateAgent {
            request_id: "r".into(),
            program: Loose::Absent,
        });
        assert_eq!(
            serde_json::to_string(&request).unwrap(),
            r#"{"type":"update-agent","requestId":"r"}"#
        );
    }

    #[test]
    fn every_variant_has_a_distinct_tag() {
        let mut tags = Request::TYPES.to_vec();
        tags.sort_unstable();
        tags.dedup();
        assert_eq!(tags.len(), Request::TYPES.len());
        assert_eq!(Request::TYPES.len(), 76);
    }
}
