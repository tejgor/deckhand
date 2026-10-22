//! Agent signals: the pure parts (the process and PATH lookups are in deckhand_sys::agent_path).
//!
//! Agent hook payloads → attention signals, and the hook settings Deckhand passes to agents. The parts that run
//! processes or know the deckhand binary (`hookCommand`, `hookSettings`, `integrationArgs` and its `--help` probe
//! cache) live in `deckhand_sys::agent_path`, which builds on [`hook_settings_for`].

use std::sync::LazyLock;

use indexmap::IndexMap;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use serde_with::skip_serializing_none;

use crate::agents::JS_SPACE;
use crate::error::{DhError, Result};
use crate::jsstr;
use deckhand_protocol::data::{
    AgentSessionRef, AgentSessionRefKind, AttentionReason, AttentionReasonKind, AttentionState, ProgramKey,
};

/// TS: agentSignals.ts AgentSignal. Key order: a subagent's `{state, event, reason?, fromSubagent}`, otherwise
/// `{state, event, nativeRef, reason?, idle?}` (an undefined `nativeRef` is dropped by JSON.stringify).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSignal {
    pub state: AttentionState,
    pub event: String,
    pub native_ref: Option<AgentSessionRef>,
    pub reason: Option<AttentionReason>,
    pub from_subagent: Option<bool>,
    /// Claude's idle prompt, which only ends a working or unknown state (never hides a question or a failure).
    pub idle: Option<bool>,
}

/// TS: agentSignals.ts MAX_REASON_LENGTH (UTF-16 units of a reason line).
pub const MAX_REASON_LENGTH: usize = 300;

/// TS: agentSignals.ts MAX_HOOK_FIELD_LENGTH.
pub const MAX_HOOK_FIELD_LENGTH: usize = 256;

/// TS: agentSignals.ts MAX_HOOK_TEXT_LENGTH.
pub const MAX_HOOK_TEXT_LENGTH: usize = 2000;

/// The events hook settings register (TS: the list in agentSignals.ts hookSettings), in order.
pub const HOOK_EVENTS: [&str; 9] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "Stop",
    "StopFailure",
    "Notification",
    "Interrupt",
];

// Tools that stop the agent to ask you something. Claude's AskUserQuestion and ExitPlanMode are documented hook
// targets; Codex's request_user_input is assumed to use the same `questions` shape (not verified against a real payload).
const QUESTION_TOOLS: [&str; 2] = ["AskUserQuestion", "request_user_input"];
const PLAN_TOOLS: [&str; 1] = ["ExitPlanMode"];
// Events a native subagent may report: a permission prompt or question it raises blocks the root session's TUI, and
// its tool use after that shows the prompt was answered. Its other events (Stop, SessionStart, ...) are not the root's.
const SUBAGENT_EVENTS: [&str; 4] = ["PreToolUse", "PostToolUse", "PermissionRequest", "Notification"];
const EDIT_TOOLS: [&str; 5] = ["Edit", "Write", "MultiEdit", "NotebookEdit", "apply_patch"];
const PROMPT_NOTIFICATIONS: [&str; 4] = [
    "permission_prompt",
    "elicitation_dialog",
    "elicitation_url_dialog",
    "agent_needs_input",
];

const INPUT_ERROR: &str = "Hook input must be an object";

static CSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").expect("CSI is a valid regex"));
static CSI_STRICT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]").expect("CSI_STRICT is a valid regex"));
static CONTROLS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\x00-\x1F\x7F-\u{9F}]+").expect("CONTROLS is a valid regex"));
static SPACES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("[{JS_SPACE}]+")).expect("SPACES is a valid regex"));
static MARKDOWN_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^[{s}]*(?:#{{1,6}}[{s}]+|[-*+>][{s}]+|[0-9]+[.)][{s}]+)+",
        s = JS_SPACE
    ))
    .expect("MARKDOWN_PREFIX is a valid regex")
});
static EMPHASIS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*\*|__|`").expect("EMPHASIS is a valid regex"));
static SESSION_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9_-]{0,127}$").expect("SESSION_ID is a valid regex"));
static MCP_TOOL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^mcp__(.+?)__(.+)$").expect("MCP_TOOL is a valid regex"));
static PASTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)\x1b\[200~.*?\x1b\[201~").expect("PASTE is a valid regex"));
// JS `.` stops at line terminators.
static ESCAPE_PAIR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b[^\n\r\u{2028}\u{2029}]").expect("ESCAPE_PAIR is a valid regex"));
static ANSWER_KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\r\n0-9]").expect("ANSWER_KEY is a valid regex"));
static CODEX_RESUME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"codex[{s}]+resume[{s}]+([a-zA-Z0-9][a-zA-Z0-9_-]{{7,127}})(?:[{s}]|$)",
        s = JS_SPACE
    ))
    .expect("CODEX_RESUME is a valid regex")
});

/// JS truthiness of a JSON value (a missing property is `None`).
fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|number| number != 0.0),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(_) | Value::Object(_)) => true,
    }
}

/// `String(number)` for a JSON number.
fn js_number_string(number: &serde_json::Number) -> String {
    if let Some(integer) = number.as_i64() {
        return integer.to_string();
    }
    let value = number.as_f64().unwrap_or(f64::NAN);
    let text = deckhand_protocol::data::JsNumber(value).to_js_string();
    match text.split_once('e') {
        Some((mantissa, exponent)) if !exponent.starts_with('-') => format!("{mantissa}e+{exponent}"),
        _ => text,
    }
}

/// `String(value)` for a JSON value (`undefined` is the caller's `None`).
fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_owned(),
        Some(Value::Null) => "null".to_owned(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(Value::Number(number)) => js_number_string(number),
        Some(Value::String(text)) => text.clone(),
        // Array.prototype.join: null and undefined items are empty.
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                if item.is_null() {
                    String::new()
                } else {
                    js_string(Some(item))
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_owned(),
    }
}

fn string_of<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    map.get(key).and_then(Value::as_str)
}

static EMPTY: LazyLock<Map<String, Value>> = LazyLock::new(Map::new);

/// TS: agentSignals.ts record (a plain object, else `{}`).
fn record(value: Option<&Value>) -> &Map<String, Value> {
    match value {
        Some(Value::Object(map)) => map,
        _ => &EMPTY,
    }
}

/// TS: agentSignals.ts normalizeHook (`Ok(None)`: an event that says nothing about attention). Errors:
/// `Hook input must be an object`, `Hook event is missing`.
pub fn normalize_hook(program: ProgramKey, payload: &Value) -> Result<Option<AgentSignal>> {
    let Value::Object(raw) = payload else {
        return Err(DhError::msg(INPUT_ERROR));
    };
    let Some(event) = string_of(raw, "hook_event_name") else {
        return Err(DhError::msg("Hook event is missing"));
    };
    let from_subagent = truthy(raw.get("agent_id")) || truthy(raw.get("parent_session_id"));
    if from_subagent && !SUBAGENT_EVENTS.contains(&event) {
        return Ok(None);
    }
    let mut reason = None;
    let mut idle = false;
    let state = match event {
        "SessionStart" | "Interrupt" => AttentionState::Unknown,
        "UserPromptSubmit" | "PostToolUse" => AttentionState::Working,
        // A question or plan approval arrives as a tool call: the agent waits on you, it is not working.
        "PreToolUse" => {
            reason = ask_reason(raw);
            if reason.is_some() {
                AttentionState::NeedsInput
            } else {
                AttentionState::Working
            }
        }
        "PermissionRequest" => {
            reason = Some(ask_reason(raw).unwrap_or_else(|| permission_reason(raw)));
            AttentionState::NeedsInput
        }
        "Stop" => {
            reason = message_reason(raw.get("last_assistant_message"));
            AttentionState::ResponseEnded
        }
        "StopFailure" => {
            let error = string_of(raw, "error_type")
                .or_else(|| string_of(raw, "error"))
                .unwrap_or("");
            let text = match failure_text(error) {
                Some(text) => text.to_owned(),
                None if !error.is_empty() => format!("failed: {}", one_line(error)),
                None => "request failed".to_owned(),
            };
            reason = Some(AttentionReason {
                kind: AttentionReasonKind::Failure,
                text,
                options: None,
            });
            if error == "rate_limit" {
                AttentionState::Limited
            } else {
                AttentionState::Failed
            }
        }
        "Notification" => {
            // The idle prompt (a reply ~60 s old, nothing typed since) means the turn ended, not that a question
            // waits. The others come once a prompt has waited ~6 s untouched (each keystroke defers them); a
            // permission prompt's is the only signal of a sandboxed command's network request, which runs no
            // PermissionRequest hooks.
            let kind = js_string(raw.get("notification_type"));
            idle = kind == "idle_prompt";
            if !idle && !PROMPT_NOTIFICATIONS.contains(&kind.as_str()) {
                return Ok(None);
            }
            if idle && from_subagent {
                return Ok(None);
            }
            if let Some(message) = string_of(raw, "message") {
                let text = one_line(message);
                if !text.is_empty() {
                    reason = Some(AttentionReason {
                        kind: AttentionReasonKind::Notice,
                        text,
                        options: None,
                    });
                }
            }
            if idle {
                AttentionState::ResponseEnded
            } else {
                AttentionState::NeedsInput
            }
        }
        _ => return Ok(None),
    };
    if from_subagent {
        return Ok(Some(AgentSignal {
            state,
            event: event.to_owned(),
            native_ref: None,
            reason,
            from_subagent: Some(true),
            idle: None,
        }));
    }
    let native_ref = string_of(raw, "session_id")
        .filter(|id| program != ProgramKey::Pi && SESSION_ID.is_match(id))
        .map(|id| AgentSessionRef {
            provider: program,
            kind: AgentSessionRefKind::Id,
            value: id.to_owned(),
            extra: Default::default(),
        });
    Ok(Some(AgentSignal {
        state,
        event: event.to_owned(),
        native_ref,
        reason,
        from_subagent: None,
        idle: idle.then_some(true),
    }))
}

/// TS: agentSignals.ts answersPrompt (keys typed into the agent that answer the prompt on its screen).
///
/// Enter or a digit (a menu choice), or Esc alone (declining). Claude sends no hook between your answer and the end
/// of the tool it allowed. Arrow keys and the mouse only move, and pasted text is not an answer.
pub fn answers_prompt(data: &str) -> bool {
    if data == "\x1b" {
        return true;
    }
    let without_paste = PASTE.replace_all(data, "");
    let without_csi = CSI.replace_all(&without_paste, "");
    let keys = ESCAPE_PAIR.replace_all(&without_csi, "");
    ANSWER_KEY.is_match(&keys)
}

/// TS: agentSignals.ts FAILURE_TEXT.
fn failure_text(error: &str) -> Option<&'static str> {
    match error {
        "rate_limit" => Some("rate limited"),
        "overloaded" => Some("API overloaded"),
        "authentication_failed" => Some("authentication failed"),
        "billing_error" => Some("billing problem"),
        "server_error" => Some("API server error"),
        _ => None,
    }
}

/// TS: agentSignals.ts oneLine (agent text as one display line, cut to MAX_REASON_LENGTH with `…`): controls
/// (escape sequences included) and runs of whitespace become one space.
pub fn one_line(text: &str) -> String {
    let without_csi = CSI.replace_all(text, "");
    let without_controls = CONTROLS.replace_all(&without_csi, " ");
    let spaced = SPACES.replace_all(&without_controls, " ");
    let line = jsstr::js_trim(&spaced);
    if jsstr::js_len(line) > MAX_REASON_LENGTH {
        format!("{}…", jsstr::js_slice(line, 0, Some(MAX_REASON_LENGTH as i64 - 1)))
    } else {
        line.to_owned()
    }
}

/// TS: agentSignals.ts plainLine (a Markdown line without its heading, list, quote and emphasis markup).
fn plain_line(text: &str) -> String {
    let without_prefix = MARKDOWN_PREFIX.replacen(text, 1, "");
    one_line(&EMPHASIS.replace_all(&without_prefix, ""))
}

/// TS: agentSignals.ts askReason: a question tool's first question (`asks (+1): …` when it asks more) with its
/// options, or a plan to approve.
fn ask_reason(raw: &Map<String, Value>) -> Option<AttentionReason> {
    let tool = string_of(raw, "tool_name")?;
    let input = record(raw.get("tool_input"));
    if PLAN_TOOLS.contains(&tool) {
        let first =
            string_of(input, "plan").and_then(|plan| plan.split('\n').map(plain_line).find(|line| !line.is_empty()));
        return Some(AttentionReason {
            kind: AttentionReasonKind::Plan,
            text: first.map_or_else(
                || "plan ready for approval".to_owned(),
                |first| format!("plan ready: {first}"),
            ),
            options: None,
        });
    }
    if !QUESTION_TOOLS.contains(&tool) {
        return None;
    }
    let questions: Vec<&Map<String, Value>> = match input.get("questions") {
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| record(Some(item)))
            .filter(|item| string_of(item, "question").is_some_and(|question| !one_line(question).is_empty()))
            .collect(),
        _ => Vec::new(),
    };
    let Some(first) = questions.first() else {
        return Some(AttentionReason {
            kind: AttentionReasonKind::Question,
            text: "asks you a question".to_owned(),
            options: None,
        });
    };
    let options: Vec<String> = match first.get("options") {
        Some(Value::Array(items)) => items
            .iter()
            .map(|option| match option {
                Value::String(text) => one_line(text),
                other => {
                    let label = record(Some(other)).get("label").filter(|label| !label.is_null());
                    one_line(&label.map_or_else(String::new, |label| js_string(Some(label))))
                }
            })
            .filter(|option| !option.is_empty())
            .collect(),
        _ => Vec::new(),
    };
    let more = if questions.len() > 1 {
        format!(" (+{})", questions.len() - 1)
    } else {
        String::new()
    };
    let question = string_of(first, "question").unwrap_or("");
    Some(AttentionReason {
        kind: AttentionReasonKind::Question,
        text: format!("asks{more}: {}", one_line(question)),
        options: (!options.is_empty()).then_some(options),
    })
}

/// TS: agentSignals.ts permissionReason: what a permission prompt asks to do (the command it would run, the file it
/// would edit, the host or tool it would use).
fn permission_reason(raw: &Map<String, Value>) -> AttentionReason {
    let permission = |text: String| AttentionReason {
        kind: AttentionReasonKind::Permission,
        text,
        options: None,
    };
    let tool = string_of(raw, "tool_name").map(one_line).unwrap_or_default();
    let input = record(raw.get("tool_input"));
    let text = |key: &str| string_of(input, key).map(one_line).unwrap_or_default();
    let first = |keys: &[&str]| keys.iter().map(|key| text(key)).find(|value| !value.is_empty());
    if let Some(command) = first(&["command", "cmd"]) {
        return permission(format!("wants to run: {command}"));
    }
    if EDIT_TOOLS.contains(&tool.as_str()) {
        return permission(match first(&["file_path", "notebook_path", "path"]) {
            Some(file) => format!("wants to edit: {}", file.rsplit('/').next().unwrap_or("")),
            None => "wants to edit files".to_owned(),
        });
    }
    let url = text("url");
    if !url.is_empty() {
        // Not a URL (or no host): show it as given.
        let host = url_host(&url).filter(|host| !host.is_empty()).unwrap_or(url);
        return permission(format!("wants to fetch: {host}"));
    }
    if let Some(captures) = MCP_TOOL.captures(&tool) {
        let part = |index: usize| captures.get(index).map_or("", |found| found.as_str());
        return permission(format!("wants to use: {} {}", part(1), part(2)));
    }
    permission(if tool.is_empty() {
        "asks for permission".to_owned()
    } else {
        format!("wants to use: {tool}")
    })
}

/// TS: agentSignals.ts messageReason: the last line of the agent's reply, where a question to you usually is.
fn message_reason(message: Option<&Value>) -> Option<AttentionReason> {
    let message = message?.as_str()?;
    let last = message
        .split('\n')
        .rev()
        .map(plain_line)
        .find(|line| !line.is_empty())?;
    Some(AttentionReason {
        kind: AttentionReasonKind::Message,
        text: format!("said: {last}"),
        options: None,
    })
}

// What `deckhand hook` forwards to the daemon: the lifecycle fields, plus only what a reason line is made from (a
// question or plan tool's input, a permission prompt's command/file/URL, the end of the last reply, a notification's
// text), each bounded. Other tool inputs and outputs never leave the hook process.
const SIGNAL_FIELDS: [&str; 8] = [
    "hook_event_name",
    "session_id",
    "agent_id",
    "parent_session_id",
    "notification_type",
    "error",
    "error_type",
    "tool_name",
];
const PERMISSION_INPUT_FIELDS: [&str; 6] = ["command", "cmd", "file_path", "notebook_path", "path", "url"];

/// `text.slice(0, max)` in UTF-16 units.
fn head(text: &str, max: usize) -> String {
    jsstr::js_slice(text, 0, Some(max as i64))
}

/// TS: agentSignals.ts hookPayloadFields (what `deckhand hook` forwards: bounded lifecycle and reason fields).
pub fn hook_payload_fields(payload: &Value) -> Result<Map<String, Value>> {
    let Value::Object(raw) = payload else {
        return Err(DhError::msg(INPUT_ERROR));
    };
    let mut fields = Map::new();
    for field in SIGNAL_FIELDS {
        let text = match raw.get(field) {
            None | Some(Value::Null) | Some(Value::Bool(false)) => continue,
            Some(Value::String(text)) if text.is_empty() => continue,
            Some(Value::String(text)) => head(text, MAX_HOOK_FIELD_LENGTH),
            // Non-string values keep their truthiness (e.g. agent_id) without copying their contents.
            Some(Value::Object(_) | Value::Array(_)) => "[object]".to_owned(),
            other => head(&js_string(other), MAX_HOOK_FIELD_LENGTH),
        };
        fields.insert(field.to_owned(), Value::String(text));
    }
    let event = string_of(raw, "hook_event_name");
    let input = record(raw.get("tool_input"));
    let bounded = |value: Option<&Value>, max: usize| value.and_then(Value::as_str).map(|text| head(text, max));
    // An undefined property is left out, as JSON.stringify does.
    let put = |map: &mut Map<String, Value>, key: &str, value: Option<String>| {
        if let Some(value) = value {
            map.insert(key.to_owned(), Value::String(value));
        }
    };
    if let Some(tool) = string_of(raw, "tool_name")
        && matches!(event, Some("PreToolUse" | "PermissionRequest"))
    {
        if QUESTION_TOOLS.contains(&tool) {
            let items = input
                .get("questions")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice);
            let questions: Vec<Value> = items
                .iter()
                .take(4)
                .map(|item| {
                    let item = record(Some(item));
                    let mut question = Map::new();
                    put(
                        &mut question,
                        "question",
                        bounded(item.get("question"), MAX_REASON_LENGTH),
                    );
                    put(&mut question, "header", bounded(item.get("header"), 60));
                    let options = item
                        .get("options")
                        .and_then(Value::as_array)
                        .map_or(&[][..], Vec::as_slice);
                    let options: Vec<Value> = options
                        .iter()
                        .take(8)
                        .map(|option| {
                            let mut entry = Map::new();
                            if let Value::String(label) = option {
                                put(&mut entry, "label", Some(head(label, 120)));
                            } else {
                                let option = record(Some(option));
                                put(&mut entry, "label", bounded(option.get("label"), 120));
                                put(&mut entry, "description", bounded(option.get("description"), 200));
                            }
                            Value::Object(entry)
                        })
                        .collect();
                    question.insert("options".to_owned(), Value::Array(options));
                    question.insert(
                        "multiSelect".to_owned(),
                        Value::Bool(item.get("multiSelect") == Some(&Value::Bool(true))),
                    );
                    Value::Object(question)
                })
                .collect();
            let mut tool_input = Map::new();
            tool_input.insert("questions".to_owned(), Value::Array(questions));
            fields.insert("tool_input".to_owned(), Value::Object(tool_input));
        } else if PLAN_TOOLS.contains(&tool) {
            let mut tool_input = Map::new();
            put(
                &mut tool_input,
                "plan",
                bounded(input.get("plan"), MAX_HOOK_TEXT_LENGTH),
            );
            fields.insert("tool_input".to_owned(), Value::Object(tool_input));
        } else if event == Some("PermissionRequest") {
            let mut tool_input = Map::new();
            for key in PERMISSION_INPUT_FIELDS {
                put(&mut tool_input, key, bounded(input.get(key), MAX_HOOK_FIELD_LENGTH));
            }
            fields.insert("tool_input".to_owned(), Value::Object(tool_input));
        }
    }
    if event == Some("Stop")
        && let Some(message) = string_of(raw, "last_assistant_message")
    {
        let tail = jsstr::js_slice(message, -(MAX_HOOK_TEXT_LENGTH as i64), None);
        fields.insert("last_assistant_message".to_owned(), Value::String(tail));
    }
    if event == Some("Notification")
        && let Some(message) = string_of(raw, "message")
    {
        fields.insert(
            "message".to_owned(),
            Value::String(head(message, MAX_HOOK_FIELD_LENGTH)),
        );
    }
    Ok(fields)
}

/// TS: agentSignals.ts hooksEnabled: whether launches pass agent signals (the user's setting, else on for Claude
/// only: Codex needs its own hooks set up first).
pub fn hooks_enabled(setting: Option<bool>, program: ProgramKey) -> bool {
    setting.unwrap_or(program == ProgramKey::Claude)
}

/// TS: agentSignals.ts attentionMessage (the reason as a notification's text, else the state in words).
pub fn attention_message(state: AttentionState, reason: Option<&AttentionReason>) -> String {
    if let Some(reason) = reason {
        return reason.text.clone();
    }
    match state {
        AttentionState::NeedsInput => "needs input".to_owned(),
        AttentionState::ResponseEnded => "response ended".to_owned(),
        AttentionState::Limited => "rate limited".to_owned(),
        other => other.as_str().to_owned(),
    }
}

/// TS: agentSignals.ts shellQuote (`'...'` with `'\''` for quotes).
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// One hook command (TS: the `{type, command, timeout, async?}` literal in hookSettings).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookCommand {
    #[serde(rename = "type")]
    pub kind: String,
    pub command: String,
    pub timeout: i64,
    #[serde(rename = "async")]
    pub async_: Option<bool>,
}

/// One matcher entry (`{hooks: [...]}`).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookMatcher {
    pub hooks: Vec<HookCommand>,
}

/// TS: agentSignals.ts hookSettings result: `{hooks: {<event>: [{hooks: [command]}]}}` in HOOK_EVENTS order.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookSettings {
    pub hooks: IndexMap<String, Vec<HookMatcher>>,
}

/// TS: agentSignals.ts hookSettings with the hook command given (the pure half; `async_`: Claude's background
/// hooks).
///
/// `async` (Claude) runs them in the background, so the agent never waits on Deckhand: they only report, never
/// decide. Codex's hooks.json gets plain entries (its docs don't list `async`).
pub fn hook_settings_for(command: &str, async_: bool) -> HookSettings {
    let hook = HookCommand {
        kind: "command".to_owned(),
        command: command.to_owned(),
        timeout: 2,
        async_: async_.then_some(true),
    };
    HookSettings {
        hooks: HOOK_EVENTS
            .iter()
            .map(|event| {
                (
                    (*event).to_owned(),
                    vec![HookMatcher {
                        hooks: vec![hook.clone()],
                    }],
                )
            })
            .collect(),
    }
}

/// TS: agentSignals.ts codexResumeFromOutput (the `codex resume <id>` exit hint).
pub fn codex_resume_from_output(output: &str) -> Option<AgentSessionRef> {
    let plain = CSI_STRICT.replace_all(output, "");
    let id = CODEX_RESUME.captures(&plain)?.get(1)?.as_str().to_owned();
    Some(AgentSessionRef {
        provider: ProgramKey::Codex,
        kind: AgentSessionRefKind::Id,
        value: id,
        extra: Default::default(),
    })
}

/// TS: agentSignals.ts needsAttention.
pub fn needs_attention(state: Option<AttentionState>) -> bool {
    matches!(
        state,
        Some(
            AttentionState::NeedsInput
                | AttentionState::ResponseEnded
                | AttentionState::Failed
                | AttentionState::Limited
        )
    )
}

// ---------------------------------------------------------------------------------------------------------------
// `new URL(url).host` (WHATWG URL), for the permission reason's host. `None` where the TS constructor throws.

/// The `host` of a URL as the WHATWG parser serializes it (`host[:port]`, default ports dropped, IDNA labels as
/// punycode), `None` when parsing fails.
fn url_host(input: &str) -> Option<String> {
    let url = url::Url::parse(input).ok()?;
    let host = url.host_str().unwrap_or("");
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use deckhand_testkit::{Case, Outcome, replay, tags};
    use serde_json::json;

    fn optional<T: Serialize>(value: Option<T>) -> Outcome {
        match value {
            Some(value) => Outcome::ret(value),
            None => Outcome::ret(tags::undefined()),
        }
    }

    #[test]
    fn hook_settings_register_every_event() {
        let settings = serde_json::to_value(hook_settings_for("'/bin/deckhand' 'hook'", true)).expect("serializes");
        assert_eq!(
            settings["hooks"]
                .as_object()
                .map(|hooks| hooks.keys().cloned().collect::<Vec<_>>()),
            Some(HOOK_EVENTS.map(String::from).to_vec())
        );
        assert_eq!(
            settings["hooks"]["Stop"],
            json!([{"hooks": [{"type": "command", "command": "'/bin/deckhand' 'hook'", "timeout": 2, "async": true}]}])
        );
        let plain = serde_json::to_value(hook_settings_for("x", false)).expect("serializes");
        assert_eq!(
            plain["hooks"]["Interrupt"],
            json!([{"hooks": [{"type": "command", "command": "x", "timeout": 2}]}])
        );
    }

    #[test]
    fn url_hosts_follow_whatwg() {
        let cases = [
            ("https://example.com:8080/a?b=1", Some("example.com:8080")),
            ("https://Example.COM:443/", Some("example.com")),
            ("http://", None),
            ("not a url", None),
            ("file:///x/y", Some("")),
            ("https://中文.example/路径", Some("xn--fiq228c.example")),
            ("https://user:pw@host.test/x", Some("host.test")),
            ("http://0x7f.1/", Some("127.0.0.1")),
            ("http://[::1]:80/", Some("[::1]")),
            ("http://1.2.3.4.5/", None),
            ("mailto:a@b.c", Some("")),
            ("foo://Host:9/x", Some("Host:9")),
            ("https://a b/", None),
        ];
        for (input, host) in cases {
            assert_eq!(url_host(input).as_deref(), host, "{input}");
        }
    }

    /// UTS #46 mapping and validation, each expectation taken from Node 24's `new URL(input).host`.
    #[test]
    fn url_hosts_apply_idna_mapping() {
        let cases = [
            // Compatibility forms of ASCII map to ASCII.
            ("https://ｅｘａｍｐｌｅ.com/x", Some("example.com")),
            ("http://ｆｏｏ．ｃｏｍ/", Some("foo.com")),
            ("https://１２７.０.０.１/", Some("127.0.0.1")),
            ("https://ⓐ.com/", Some("a.com")),
            ("https://ⓐbc.com/", Some("abc.com")),
            ("http://Ⓩ.com/", Some("z.com")),
            ("https://%E2%93%90.com/", Some("a.com")),
            ("https://Ⅳ.com/", Some("iv.com")),
            ("http://ⅻ.com/", Some("xii.com")),
            ("http://①.com/", Some("1.com")),
            ("http://⑴.com/", Some("(1).com")),
            ("http://ﬁ.com/", Some("fi.com")),
            ("http://㎏.com/", Some("kg.com")),
            ("http://™.com/", Some("tm.com")),
            ("http://𝐞𝐱𝐚𝐦𝐩𝐥𝐞.com/", Some("example.com")),
            // Ignored code points vanish.
            ("https://a\u{AD}b.com/", Some("ab.com")),
            ("http://ab\u{FE00}c.com/", Some("abc.com")),
            ("http://a\u{200B}b.com/", Some("ab.com")),
            ("http://a\u{2060}b.com/", Some("ab.com")),
            ("http://a\u{FEFF}b.com/", Some("ab.com")),
            // NFC.
            ("https://cafe\u{301}.com/", Some("xn--caf-dma.com")),
            ("https://café.com/", Some("xn--caf-dma.com")),
            ("https://é.com/", Some("xn--9ca.com")),
            ("http://Ｅ\u{301}.com/", Some("xn--9ca.com")),
            ("http://o\u{302}\u{301}.com/", Some("xn--0lg.com")),
            ("http://a\u{323}\u{302}.com/", Some("xn--zkg.com")),
            ("http://ä\u{308}.com/", Some("xn--4ca01i.com")),
            // Lowercasing is per code point (no final sigma), and keeps deviation characters.
            ("http://ΑΣ.com/", Some("xn--mxa0b.com")),
            ("http://ß.com/", Some("xn--zca.com")),
            ("http://ς.com/", Some("xn--3xa.com")),
            ("http://İ.com/", Some("xn--i-9bb.com")),
            ("http://ↄ.com/", Some("xn--r5g.com")),
            ("http://Ↄ.com/", Some("xn--r5g.com")),
            ("http://💩.la/", Some("xn--ls8h.la")),
            // (Node accepts a mixed-direction label like "aא"; the url crate applies the Bidi rule and rejects it.)
            // Joiners only after a virama.
            ("http://\u{915}\u{94D}\u{200D}\u{937}.com/", Some("xn--11b2ezcw70k.com")),
            ("http://\u{915}\u{94D}\u{200C}\u{937}.com/", Some("xn--11b2ezcs70k.com")),
            ("http://a\u{200D}b.com/", None),
            ("http://a\u{200C}b.com/", None),
            // Disallowed code points and leading combining marks fail.
            ("https://\u{FFFF}.com/", None),
            ("http://a\u{FFFD}.com/", None),
            ("http://a\u{E000}.com/", None),
            ("http://a\u{85}b.com/", None),
            ("http://a\u{FDD0}.com/", None),
            ("http://\u{301}b.com/", None),
            // `xn--` labels must decode to valid, normalized, non-ASCII labels.
            ("http://xn--9ca.com/", Some("xn--9ca.com")),
            ("http://XN--9CA.com/", Some("xn--9ca.com")),
            ("http://xn--zca.com/", Some("xn--zca.com")),
            ("http://xn--ls8h.la/", Some("xn--ls8h.la")),
            ("http://xn--fiq228c.com/", Some("xn--fiq228c.com")),
            ("http://xn--a-1ga.com/", Some("xn--a-1ga.com")),
            ("https://xn--a.com/", None),
            ("http://xn--.com/", None),
            ("http://xn---.com/", None),
            ("http://xn--abc.com/", None),
            ("http://xn--abc-.com/", None),
            ("http://xn--abc-ll.com/", None),
            ("http://xn--ab-m1t.com/", None),
            ("http://xn--e-xbb.com/", None),
            ("http://xn--a-ecp.ru/", None),
            ("http://xn--1ug.com/", None),
            // Plain ASCII is untouched beyond lowercasing.
            ("http://-a.com/", Some("-a.com")),
            ("http://ab--c.com/", Some("ab--c.com")),
            ("http://a..b/", Some("a..b")),
        ];
        let failures: Vec<String> = cases
            .iter()
            .filter(|(input, host)| url_host(input).as_deref() != *host)
            .map(|(input, host)| format!("{input:?}: got {:?}, want {host:?}", url_host(input)))
            .collect();
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn permission_reason_names_the_mapped_host() {
        let reason = |url: &str| {
            let raw =
                json!({"hook_event_name": "PermissionRequest", "tool_name": "WebFetch", "tool_input": {"url": url}});
            normalize_hook(ProgramKey::Claude, &raw)
                .expect("normalizes")
                .and_then(|signal| signal.reason)
                .map(|reason| reason.text)
        };
        assert_eq!(
            reason("https://ｅｘａｍｐｌｅ.com/x").as_deref(),
            Some("wants to fetch: example.com")
        );
        assert_eq!(
            reason("http://xn--a.com/").as_deref(),
            Some("wants to fetch: http://xn--a.com/")
        );
        assert_eq!(
            reason("http://a\u{200D}b.com/").as_deref(),
            Some("wants to fetch: http://a\u{200D}b.com/")
        );
    }

    #[test]
    fn normalize_hook_parity() {
        replay("agentSignals-P1/normalizeHook", |case: &Case| {
            let Ok(program) = case.arg_as::<ProgramKey>(0) else {
                return Outcome::throws("parity: bad arguments");
            };
            match normalize_hook(program, &tags::untag(case.arg(1))) {
                Ok(signal) => optional(signal),
                Err(error) => Outcome::throws(error),
            }
        });
    }

    #[test]
    fn answers_prompt_parity() {
        replay("agentSignals-P1/answersPrompt", |case: &Case| -> Result<bool, String> {
            Ok(answers_prompt(case.arg_str(0)?))
        });
    }

    #[test]
    fn hook_payload_fields_parity() {
        replay("agentSignals-P1/hookPayloadFields", |case: &Case| {
            hook_payload_fields(&tags::untag(case.arg(0)))
        });
    }

    #[test]
    fn hooks_enabled_parity() {
        replay("agentSignals-P1/hooksEnabled", |case: &Case| -> Result<bool, String> {
            Ok(hooks_enabled(case.arg_as(0)?, case.arg_as(1)?))
        });
    }

    #[test]
    fn attention_message_parity() {
        replay(
            "agentSignals-P1/attentionMessage",
            |case: &Case| -> Result<String, String> {
                let reason: Option<AttentionReason> = case.arg_as(1)?;
                Ok(attention_message(case.arg_as(0)?, reason.as_ref()))
            },
        );
    }

    #[test]
    fn shell_quote_parity() {
        replay("agentSignals-P1/shellQuote", |case: &Case| -> Result<String, String> {
            Ok(shell_quote(case.arg_str(0)?))
        });
    }

    #[test]
    fn codex_resume_from_output_parity() {
        replay("agentSignals-P1/codexResumeFromOutput", |case: &Case| {
            match case.arg_str(0) {
                Ok(output) => optional(codex_resume_from_output(output)),
                Err(error) => Outcome::throws(error),
            }
        });
    }

    #[test]
    fn needs_attention_parity() {
        replay(
            "agentSignals-P1/needsAttention",
            |case: &Case| -> Result<bool, String> { Ok(needs_attention(case.arg_as(0)?)) },
        );
    }

    /// `normalizeHook` as JSON (an undefined `nativeRef` is absent, as `JSON.stringify` drops it).
    fn hook(program: ProgramKey, payload: Value) -> Option<Value> {
        normalize_hook(program, &payload)
            .expect("normalizes")
            .map(|signal| serde_json::to_value(signal).expect("serializes"))
    }

    fn claude(payload: Value) -> Option<Value> {
        hook(ProgramKey::Claude, payload)
    }

    fn state_of(signal: Option<Value>) -> Option<String> {
        signal.and_then(|signal| signal["state"].as_str().map(str::to_owned))
    }

    fn reason_of(signal: Option<Value>) -> Option<Value> {
        signal.and_then(|signal| signal.get("reason").cloned())
    }

    #[test]
    fn lifecycle_evidence_is_advisory_and_excludes_subagents() {
        let state = |payload: Value| state_of(claude(payload));
        assert_eq!(
            state(json!({"hook_event_name": "PermissionRequest"})).as_deref(),
            Some("needs-input")
        );
        assert_eq!(
            state(json!({"hook_event_name": "Stop"})).as_deref(),
            Some("response-ended")
        );
        assert_eq!(
            state(json!({"hook_event_name": "StopFailure", "error_type": "rate_limit"})).as_deref(),
            Some("limited")
        );
        let started = hook(
            ProgramKey::Codex,
            json!({"hook_event_name": "SessionStart", "session_id": "native_123"}),
        );
        assert_eq!(
            started.as_ref().map(|signal| signal["nativeRef"]["value"].clone()),
            Some(json!("native_123"))
        );
        let bad = hook(
            ProgramKey::Codex,
            json!({"hook_event_name": "SessionStart", "session_id": "--bad"}),
        );
        assert!(bad.is_some_and(|signal| signal.get("nativeRef").is_none()));
        assert_eq!(
            claude(json!({"hook_event_name": "Stop", "agent_id": "native-child"})),
            None
        );
        assert_eq!(claude(json!({"hook_event_name": "FutureEvent"})), None);
        assert!(!needs_attention(Some(AttentionState::Working)));
        assert!(needs_attention(Some(AttentionState::ResponseEnded)));
        assert!(normalize_hook(ProgramKey::Claude, &json!("invalid")).is_err());
    }

    #[test]
    fn signals_say_why_a_session_waits() {
        let ask = json!({"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion", "tool_input": {"questions": [
            {"question": "Which auth\nlibrary?", "header": "Auth", "options": [{"label": "Lucia", "description": "small"}, {"label": "Auth.js"}], "multiSelect": false},
            {"question": "Keep sessions?", "options": [{"label": "Yes"}]},
        ]}});
        // A question is no tool work: the session waits on you.
        assert_eq!(
            reason_of(claude(ask.clone())),
            Some(
                json!({"kind": "question", "text": "asks (+1): Which auth library?", "options": ["Lucia", "Auth.js"]})
            )
        );
        assert_eq!(state_of(claude(ask.clone())).as_deref(), Some("needs-input"));
        let mut no_input = ask.clone();
        no_input["tool_input"] = json!({});
        assert_eq!(
            reason_of(claude(no_input)),
            Some(json!({"kind": "question", "text": "asks you a question"}))
        );
        assert_eq!(
            reason_of(claude(
                json!({"hook_event_name": "PreToolUse", "tool_name": "ExitPlanMode",
                "tool_input": {"plan": "\n## **Plan:** move auth to `lucia`\n- step"}})
            )),
            Some(json!({"kind": "plan", "text": "plan ready: Plan: move auth to lucia"}))
        );
        assert_eq!(
            claude(json!({"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": "ls"}})),
            Some(json!({"state": "working", "event": "PreToolUse"}))
        );
        // Permission prompts: the command, the file's name, the host, the MCP server and tool.
        let permission = |tool_name: &str, tool_input: Value| {
            reason_of(claude(
                json!({"hook_event_name": "PermissionRequest", "tool_name": tool_name, "tool_input": tool_input}),
            ))
            .and_then(|reason| reason["text"].as_str().map(str::to_owned))
        };
        assert_eq!(
            permission(
                "Bash",
                json!({"command": "npm   publish\n--tag next", "description": "Publish"})
            )
            .as_deref(),
            Some("wants to run: npm publish --tag next")
        );
        assert_eq!(
            permission("Edit", json!({"file_path": "/repo/src/auth.ts"})).as_deref(),
            Some("wants to edit: auth.ts")
        );
        assert_eq!(
            permission("WebFetch", json!({"url": "https://example.com/a?b"})).as_deref(),
            Some("wants to fetch: example.com")
        );
        assert_eq!(
            permission("mcp__github__create_issue", json!({})).as_deref(),
            Some("wants to use: github create_issue")
        );
        assert_eq!(permission("Glob", json!({})).as_deref(), Some("wants to use: Glob"));
        assert_eq!(
            permission("AskUserQuestion", ask["tool_input"].clone()).as_deref(),
            Some("asks (+1): Which auth library?")
        );
        // A reply's last non-empty line, without Markdown or terminal controls; a failure in words.
        assert_eq!(
            reason_of(claude(json!({"hook_event_name": "Stop",
                "last_assistant_message": "Done.\n\n- **Should I** also update `e2e`?\u{1b}[2J\n\n"}))),
            Some(json!({"kind": "message", "text": "said: Should I also update e2e?"}))
        );
        assert_eq!(
            reason_of(claude(
                json!({"hook_event_name": "Stop", "last_assistant_message": "  \n"})
            )),
            None
        );
        assert_eq!(
            reason_of(claude(
                json!({"hook_event_name": "StopFailure", "error_type": "overloaded"})
            )),
            Some(json!({"kind": "failure", "text": "API overloaded"}))
        );
        assert_eq!(
            reason_of(claude(
                json!({"hook_event_name": "StopFailure", "error_type": "rate_limit"})
            ))
            .map(|reason| reason["text"].clone()),
            Some(json!("rate limited"))
        );
        // The idle prompt ends a turn (it never asks anything); prompts left waiting need input; other notices are not states.
        assert_eq!(
            claude(
                json!({"hook_event_name": "Notification", "notification_type": "idle_prompt",
                "message": "Claude is waiting for your input"})
            ),
            Some(json!({"state": "response-ended", "event": "Notification",
                "reason": {"kind": "notice", "text": "Claude is waiting for your input"}, "idle": true}))
        );
        for kind in ["permission_prompt", "elicitation_dialog", "elicitation_url_dialog"] {
            assert_eq!(
                state_of(claude(
                    json!({"hook_event_name": "Notification", "notification_type": kind})
                ))
                .as_deref(),
                Some("needs-input"),
                "{kind}"
            );
        }
        assert_eq!(
            claude(json!({"hook_event_name": "Notification", "notification_type": "elicitation_complete"})),
            None
        );
        assert_eq!(
            claude(json!({"hook_event_name": "Notification", "notification_type": "idle_prompt", "agent_id": "sub"})),
            None
        );
        // Keys that answer a prompt: Enter, a digit, Esc alone; not arrows, the mouse wheel, letters or a paste.
        for keys in ["\r", "1", "\u{1b}", "y\r"] {
            assert!(answers_prompt(keys), "{keys:?}");
        }
        for keys in [
            "\u{1b}[B",
            "\u{1b}[<64;10;5M",
            "y",
            "\u{1b}[200~line 1\nline 2\u{1b}[201~",
            "\u{1b}OA",
        ] {
            assert!(!answers_prompt(keys), "{keys:?}");
        }
        let long = reason_of(claude(
            json!({"hook_event_name": "Stop", "last_assistant_message": "x".repeat(500)}),
        ))
        .and_then(|reason| reason["text"].as_str().map(jsstr::js_len));
        assert_eq!(long, Some("said: ".len() + 300));
        // A native subagent's permission prompt blocks the root's TUI: reported, never with its identity.
        assert_eq!(
            claude(
                json!({"hook_event_name": "PermissionRequest", "agent_id": "sub", "session_id": "abc",
                "tool_name": "Bash", "tool_input": {"command": "rm x"}})
            ),
            Some(json!({"state": "needs-input", "event": "PermissionRequest",
                "reason": {"kind": "permission", "text": "wants to run: rm x"}, "fromSubagent": true}))
        );
        assert_eq!(
            claude(json!({"hook_event_name": "PostToolUse", "agent_id": "sub"}))
                .map(|signal| signal["fromSubagent"].clone()),
            Some(json!(true))
        );
        assert_eq!(
            claude(json!({"hook_event_name": "SessionStart", "agent_id": "sub", "session_id": "abc"})),
            None
        );
        assert_eq!(attention_message(AttentionState::NeedsInput, None), "needs input");
        let said = AttentionReason {
            kind: AttentionReasonKind::Message,
            text: "said: ok".to_owned(),
            options: None,
        };
        assert_eq!(
            attention_message(AttentionState::ResponseEnded, Some(&said)),
            "said: ok"
        );
    }

    #[test]
    fn hook_bridge_forwards_only_what_a_reason_is_made_from() {
        let big = "y".repeat(5000);
        let fields = |payload: Value| Value::Object(hook_payload_fields(&payload).expect("object"));
        assert_eq!(
            fields(json!({"hook_event_name": "PreToolUse", "tool_name": "Bash",
                "tool_input": {"command": "cat secrets.txt"}, "transcript_path": "/t", "cwd": "/c"})),
            json!({"hook_event_name": "PreToolUse", "tool_name": "Bash"})
        );
        assert_eq!(
            fields(json!({"hook_event_name": "PostToolUse", "tool_name": "Bash",
                "tool_input": {"command": "cat secrets.txt"}, "tool_response": {"stdout": "SECRET"}})),
            json!({"hook_event_name": "PostToolUse", "tool_name": "Bash"})
        );
        assert_eq!(
            fields(json!({"hook_event_name": "PermissionRequest", "tool_name": "Bash",
                "tool_input": {"command": big, "description": "d", "env": {"A": "1"}}}))["tool_input"],
            json!({"command": "y".repeat(256)})
        );
        let questions: Vec<Value> = (0..6)
            .map(|index| {
                json!({"question": format!("q{index}"), "options": (0..10)
                    .map(|option| json!({"label": format!("o{option}"), "description": big, "extra": 1}))
                    .collect::<Vec<_>>()})
            })
            .collect();
        let forwarded = fields(json!({"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion",
            "tool_input": {"questions": questions}}));
        let questions = forwarded["tool_input"]["questions"].as_array().expect("questions");
        assert_eq!(questions.len(), 4);
        let options = questions[0]["options"].as_array().expect("options");
        assert_eq!(options.len(), 8);
        assert_eq!(options[0]["description"].as_str().map(jsstr::js_len), Some(200));
        assert_eq!(
            options[0]
                .as_object()
                .map(|map| map.keys().cloned().collect::<Vec<_>>()),
            Some(vec!["label".to_owned(), "description".to_owned()])
        );
        assert_eq!(
            fields(json!({"hook_event_name": "PreToolUse", "tool_name": "ExitPlanMode", "tool_input": {"plan": big}}))
                ["tool_input"]["plan"]
                .as_str()
                .map(jsstr::js_len),
            Some(2000)
        );
        // The end of the reply is kept (that is where a question is).
        assert_eq!(
            fields(json!({"hook_event_name": "Stop", "last_assistant_message": format!("{big}END")}))["last_assistant_message"],
            json!(format!("{}END", "y".repeat(1997)))
        );
        assert_eq!(
            fields(json!({"hook_event_name": "Notification", "message": big}))["message"],
            json!("y".repeat(256))
        );
        assert_eq!(
            fields(json!({"hook_event_name": "UserPromptSubmit", "prompt": "my prompt"})).get("prompt"),
            None
        );
        // What the bridge forwards normalizes like the full payload.
        let ask = json!({"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion",
            "tool_input": {"questions": [{"question": "Which?", "options": [{"label": "A"}]}]}});
        assert_eq!(claude(fields(ask.clone())), claude(ask));
    }

    #[test]
    fn agent_signals_default_on_for_claude_only() {
        assert!(hooks_enabled(None, ProgramKey::Claude));
        assert!(!hooks_enabled(None, ProgramKey::Codex));
        assert!(!hooks_enabled(None, ProgramKey::Pi));
        assert!(hooks_enabled(Some(true), ProgramKey::Codex));
        assert!(!hooks_enabled(Some(false), ProgramKey::Claude));
        assert_eq!(
            hook_settings_for("c", true).hooks["PreToolUse"][0].hooks[0].async_,
            Some(true)
        );
        let plain = serde_json::to_value(hook_settings_for("c", false)).expect("serializes");
        assert!(plain["hooks"]["Stop"][0]["hooks"][0].get("async").is_none());
    }

    #[test]
    fn codex_exit_hints_preserve_exact_identities() {
        assert_eq!(
            codex_resume_from_output("\u{1b}[32mTo continue: codex resume 12345678-abcd\u{1b}[0m\n").map(|r| r.value),
            Some("12345678-abcd".to_owned())
        );
        assert_eq!(codex_resume_from_output("codex resume --last"), None);
    }
}
