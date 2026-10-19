//! Port of agentSignals.ts (pure parts).
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
    let trimmed = input.trim_matches(|ch: char| ch <= ' ');
    let text: String = trimmed.chars().filter(|ch| !matches!(ch, '\t' | '\n' | '\r')).collect();
    let colon = text.find(':')?;
    let scheme = &text[..colon];
    let mut chars = scheme.chars();
    if !chars.next().is_some_and(|ch| ch.is_ascii_alphabetic())
        || !chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
    {
        return None;
    }
    let scheme = scheme.to_ascii_lowercase();
    let rest = &text[colon + 1..];
    let default_port = match scheme.as_str() {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        "ftp" => Some(21),
        _ => None,
    };
    let is_slash = |ch: char| ch == '/' || ch == '\\';
    if scheme == "file" {
        let mut slashes = rest.chars();
        if !(slashes.next().is_some_and(is_slash) && slashes.next().is_some_and(is_slash)) {
            return Some(String::new());
        }
        let after = &rest[2..];
        let end = after.find(['/', '\\', '?', '#']).unwrap_or(after.len());
        let authority = &after[..end];
        let bytes = authority.as_bytes();
        let drive = bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && matches!(bytes[1], b':' | b'|');
        if authority.is_empty() || drive {
            return Some(String::new());
        }
        let host = parse_host(authority, true)?;
        return Some(if host == "localhost" { String::new() } else { host });
    }
    let special = default_port.is_some();
    let after = if special {
        rest.trim_start_matches(is_slash)
    } else if let Some(after) = rest.strip_prefix("//") {
        after
    } else {
        // An opaque path: no host.
        return Some(String::new());
    };
    let end = if special {
        after.find(['/', '\\', '?', '#'])
    } else {
        after.find(['/', '?', '#'])
    }
    .unwrap_or(after.len());
    let authority = &after[..end];
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, host_port)| host_port);
    let (host, port) = split_port(host_port);
    if host.is_empty() {
        // Special URLs need a host; an empty one with a port or credentials fails too.
        return if special || port.is_some() || authority.contains('@') {
            None
        } else {
            Some(String::new())
        };
    }
    let port = match port {
        None | Some("") => None,
        Some(digits) if digits.bytes().all(|byte| byte.is_ascii_digit()) => {
            let significant = digits.trim_start_matches('0');
            let value = if significant.len() > 5 {
                return None;
            } else {
                significant.parse::<u32>().unwrap_or(0)
            };
            if value > 65535 {
                return None;
            }
            (Some(value) != default_port).then_some(value)
        }
        Some(_) => return None,
    };
    let host = parse_host(host, special)?;
    Some(match port {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

/// `host[:port]` split at the last `:` outside `[...]`.
fn split_port(host_port: &str) -> (&str, Option<&str>) {
    let mut inside = false;
    let mut split = None;
    for (index, ch) in host_port.char_indices() {
        match ch {
            '[' => inside = true,
            ']' => inside = false,
            ':' if !inside => {
                split = Some(index);
                break;
            }
            _ => {}
        }
    }
    match split {
        Some(index) => (&host_port[..index], Some(&host_port[index + 1..])),
        None => (host_port, None),
    }
}

const FORBIDDEN_HOST: &[char] = &[
    '\0', '\t', '\n', '\r', ' ', '#', '/', ':', '<', '>', '?', '@', '[', '\\', ']', '^', '|',
];

fn parse_host(input: &str, special: bool) -> Option<String> {
    if let Some(inner) = input.strip_prefix('[') {
        return parse_ipv6(inner.strip_suffix(']')?).map(|ip| format!("[{ip}]"));
    }
    if !special {
        // An opaque host: forbidden code points fail, the rest is kept (non-ASCII percent-encoded).
        if input.chars().any(|ch| FORBIDDEN_HOST.contains(&ch)) {
            return None;
        }
        let mut out = String::new();
        for ch in input.chars() {
            if ch.is_ascii() && !ch.is_ascii_control() {
                out.push(ch);
            } else {
                let mut buffer = [0u8; 4];
                for byte in ch.encode_utf8(&mut buffer).bytes() {
                    out.push_str(&format!("%{byte:02X}"));
                }
            }
        }
        return Some(out);
    }
    let decoded = String::from_utf8_lossy(&percent_decode(input)).into_owned();
    let ascii = domain_to_ascii(&decoded)?;
    if ascii
        .chars()
        .any(|ch| FORBIDDEN_HOST.contains(&ch) || ch.is_ascii_control() || ch == '%' || ch == '\u{7F}')
    {
        return None;
    }
    if ends_in_number(&ascii) {
        return parse_ipv4(&ascii);
    }
    Some(ascii)
}

fn percent_decode(input: &str) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(pair) = bytes.get(index + 1..index + 3)
            && let Ok(text) = std::str::from_utf8(pair)
            && let Ok(byte) = u8::from_str_radix(text, 16)
            && pair.iter().all(u8::is_ascii_hexdigit)
        {
            out.push(byte);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    out
}

/// UTS #46 ToASCII, simplified: the IDNA full stops, lowercase, and punycode for non-ASCII labels.
fn domain_to_ascii(domain: &str) -> Option<String> {
    let mapped: String = domain
        .chars()
        .map(|ch| {
            if matches!(ch, '\u{3002}' | '\u{FF0E}' | '\u{FF61}') {
                '.'
            } else {
                ch
            }
        })
        .collect::<String>()
        .to_lowercase();
    let labels: Option<Vec<String>> = mapped
        .split('.')
        .map(|label| {
            if label.is_ascii() {
                Some(label.to_owned())
            } else {
                punycode(label).map(|encoded| format!("xn--{encoded}"))
            }
        })
        .collect();
    let ascii = labels?.join(".");
    (!ascii.is_empty()).then_some(ascii)
}

/// RFC 3492 punycode encoding of one label.
fn punycode(label: &str) -> Option<String> {
    const BASE: u32 = 36;
    const T_MIN: u32 = 1;
    const T_MAX: u32 = 26;
    const SKEW: u32 = 38;
    const DAMP: u32 = 700;
    let digit = |value: u32| -> char {
        let byte = if value < 26 {
            b'a' + value as u8
        } else {
            b'0' + (value - 26) as u8
        };
        byte as char
    };
    let adapt = |mut delta: u32, points: u32, first: bool| -> u32 {
        delta /= if first { DAMP } else { 2 };
        delta += delta / points;
        let mut k = 0;
        while delta > ((BASE - T_MIN) * T_MAX) / 2 {
            delta /= BASE - T_MIN;
            k += BASE;
        }
        k + (BASE - T_MIN + 1) * delta / (delta + SKEW)
    };
    let points: Vec<u32> = label.chars().map(u32::from).collect();
    let mut output: String = label.chars().filter(char::is_ascii).collect();
    let basic = output.len() as u32;
    let mut handled = basic;
    if basic > 0 {
        output.push('-');
    }
    let (mut n, mut delta, mut bias) = (128u32, 0u32, 72u32);
    while (handled as usize) < points.len() {
        let next = points.iter().copied().filter(|&point| point >= n).min()?;
        delta = delta.checked_add((next - n).checked_mul(handled + 1)?)?;
        n = next;
        for &point in &points {
            if point < n {
                delta = delta.checked_add(1)?;
            }
            if point == n {
                let mut q = delta;
                let mut k = BASE;
                loop {
                    let t = if k <= bias {
                        T_MIN
                    } else if k >= bias + T_MAX {
                        T_MAX
                    } else {
                        k - bias
                    };
                    if q < t {
                        break;
                    }
                    output.push(digit(t + (q - t) % (BASE - t)));
                    q = (q - t) / (BASE - t);
                    k += BASE;
                }
                output.push(digit(q));
                bias = adapt(delta, handled + 1, handled == basic);
                delta = 0;
                handled += 1;
            }
        }
        delta = delta.checked_add(1)?;
        n = n.checked_add(1)?;
    }
    Some(output)
}

fn ipv4_number(part: &str) -> Option<u64> {
    if part.is_empty() {
        return None;
    }
    let (digits, radix) = if let Some(hex) = part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
        (hex, 16)
    } else if part.len() > 1 && part.starts_with('0') {
        (&part[1..], 8)
    } else {
        (part, 10)
    };
    if digits.is_empty() {
        return Some(0);
    }
    if !digits.chars().all(|ch| ch.is_digit(radix)) {
        return None;
    }
    // Saturate: anything this large is out of range anyway.
    Some(u64::from_str_radix(digits, radix).unwrap_or(u64::MAX))
}

fn ends_in_number(host: &str) -> bool {
    let mut parts: Vec<&str> = host.split('.').collect();
    if parts.last() == Some(&"") {
        if parts.len() == 1 {
            return false;
        }
        parts.pop();
    }
    let Some(last) = parts.last() else {
        return false;
    };
    if !last.is_empty() && last.bytes().all(|byte| byte.is_ascii_digit()) {
        return true;
    }
    last.strip_prefix("0x")
        .or_else(|| last.strip_prefix("0X"))
        .is_some_and(|hex| hex.chars().all(|ch| ch.is_ascii_hexdigit()))
}

fn parse_ipv4(host: &str) -> Option<String> {
    let mut parts: Vec<&str> = host.split('.').collect();
    if parts.last() == Some(&"") && parts.len() > 1 {
        parts.pop();
    }
    if parts.len() > 4 {
        return None;
    }
    let numbers: Option<Vec<u64>> = parts.iter().map(|part| ipv4_number(part)).collect();
    let numbers = numbers?;
    let (last, init) = numbers.split_last()?;
    if init.iter().any(|&number| number > 255) || *last >= 256u64.pow(5 - numbers.len() as u32) {
        return None;
    }
    let mut address = *last;
    for (index, number) in init.iter().enumerate() {
        address += number * 256u64.pow(3 - index as u32);
    }
    Some(format!(
        "{}.{}.{}.{}",
        (address >> 24) & 255,
        (address >> 16) & 255,
        (address >> 8) & 255,
        address & 255
    ))
}

/// An IPv6 address serialized as WHATWG does: lowercase hex, the first longest run of two or more zero pieces as `::`.
fn parse_ipv6(text: &str) -> Option<String> {
    let address: std::net::Ipv6Addr = text.parse().ok()?;
    let pieces = address.segments();
    let mut best: Option<(usize, usize)> = None;
    let mut index = 0;
    while index < 8 {
        if pieces[index] == 0 {
            let start = index;
            while index < 8 && pieces[index] == 0 {
                index += 1;
            }
            let length = index - start;
            if length > 1 && best.is_none_or(|(_, best_length)| length > best_length) {
                best = Some((start, length));
            }
        } else {
            index += 1;
        }
    }
    let hex = |range: &[u16]| {
        range
            .iter()
            .map(|piece| format!("{piece:x}"))
            .collect::<Vec<_>>()
            .join(":")
    };
    Some(match best {
        Some((start, length)) => format!("{}::{}", hex(&pieces[..start]), hex(&pieces[start + length..])),
        None => hex(&pieces),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use deckhand_parity::{Case, Outcome, replay, tags};
    use serde_json::json;

    const UNIT: &str = "P1";

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

    #[test]
    fn normalize_hook_parity() {
        replay(UNIT, "agentSignals-P1/normalizeHook", |case: &Case| {
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
        replay(
            UNIT,
            "agentSignals-P1/answersPrompt",
            |case: &Case| -> Result<bool, String> { Ok(answers_prompt(case.arg_str(0)?)) },
        );
    }

    #[test]
    fn hook_payload_fields_parity() {
        replay(UNIT, "agentSignals-P1/hookPayloadFields", |case: &Case| {
            hook_payload_fields(&tags::untag(case.arg(0)))
        });
    }

    #[test]
    fn hooks_enabled_parity() {
        replay(
            UNIT,
            "agentSignals-P1/hooksEnabled",
            |case: &Case| -> Result<bool, String> { Ok(hooks_enabled(case.arg_as(0)?, case.arg_as(1)?)) },
        );
    }

    #[test]
    fn attention_message_parity() {
        replay(
            UNIT,
            "agentSignals-P1/attentionMessage",
            |case: &Case| -> Result<String, String> {
                let reason: Option<AttentionReason> = case.arg_as(1)?;
                Ok(attention_message(case.arg_as(0)?, reason.as_ref()))
            },
        );
    }

    #[test]
    fn shell_quote_parity() {
        replay(
            UNIT,
            "agentSignals-P1/shellQuote",
            |case: &Case| -> Result<String, String> { Ok(shell_quote(case.arg_str(0)?)) },
        );
    }

    #[test]
    fn codex_resume_from_output_parity() {
        replay(
            UNIT,
            "agentSignals-P1/codexResumeFromOutput",
            |case: &Case| match case.arg_str(0) {
                Ok(output) => optional(codex_resume_from_output(output)),
                Err(error) => Outcome::throws(error),
            },
        );
    }

    #[test]
    fn needs_attention_parity() {
        replay(
            UNIT,
            "agentSignals-P1/needsAttention",
            |case: &Case| -> Result<bool, String> { Ok(needs_attention(case.arg_as(0)?)) },
        );
    }
}
