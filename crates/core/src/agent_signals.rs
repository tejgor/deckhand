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

/// UTS #46 ToASCII as WHATWG URL parsing runs it (non-transitional, STD3 rules and hyphen checks off): map each
/// code point, compose (NFC), then per label validate `xn--` labels by decoding them, check the joiner and
/// leading-mark rules, and punycode the non-ASCII labels. `None` where the TS URL constructor throws.
///
/// Without a Unicode data dependency this carries only part of the tables (see [`IDNA_ASCII`]): every mapping whose
/// result is ASCII, the ignored and disallowed code points of assigned characters, and Latin/Greek/Cyrillic
/// composition. Other characters fall back to their lowercase form.
fn domain_to_ascii(domain: &str) -> Option<String> {
    let mut mapped = String::with_capacity(domain.len());
    for ch in domain.chars() {
        match idna_map(ch) {
            IdnaMapping::Valid => mapped.push(ch),
            IdnaMapping::Ignored => {}
            IdnaMapping::Disallowed => return None,
            IdnaMapping::Mapped(text) => mapped.push_str(&text),
        }
    }
    let normalized = compose(&mapped);
    let labels: Option<Vec<String>> = normalized
        .split('.')
        .map(|label| {
            if let Some(encoded) = label.strip_prefix("xn--") {
                let decoded = punycode_decode(encoded)?;
                let valid = !decoded.is_ascii()
                    && compose(&decoded) == decoded
                    && decoded.chars().all(|ch| matches!(idna_map(ch), IdnaMapping::Valid))
                    && label_rules_hold(&decoded);
                valid.then(|| label.to_owned())
            } else if label.is_ascii() {
                Some(label.to_owned())
            } else if label_rules_hold(label) {
                punycode(label).map(|encoded| format!("xn--{encoded}"))
            } else {
                None
            }
        })
        .collect();
    let ascii = labels?.join(".");
    (!ascii.is_empty()).then_some(ascii)
}

/// One code point's UTS #46 status, as far as [`idna_map`] knows it.
enum IdnaMapping {
    Valid,
    Ignored,
    Disallowed,
    Mapped(String),
}

fn in_ranges(ranges: &[(u32, u32)], point: u32) -> bool {
    ranges
        .binary_search_by(|&(start, end)| {
            if end < point {
                std::cmp::Ordering::Less
            } else if start > point {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

fn idna_map(ch: char) -> IdnaMapping {
    let point = u32::from(ch);
    if ch.is_ascii() {
        return if ch.is_ascii_uppercase() {
            IdnaMapping::Mapped(ch.to_ascii_lowercase().to_string())
        } else {
            IdnaMapping::Valid
        };
    }
    if matches!(ch, '\u{3002}' | '\u{FF0E}' | '\u{FF61}') {
        return IdnaMapping::Mapped(".".to_owned());
    }
    if matches!(ch, '\u{200C}' | '\u{200D}') {
        // Deviation characters: kept (non-transitional), then held to the joiner rule in `label_rules_hold`.
        return IdnaMapping::Valid;
    }
    if in_ranges(IDNA_IGNORED, point) {
        return IdnaMapping::Ignored;
    }
    let private_use = matches!(point, 0xE000..=0xF8FF | 0xF0000..=0xFFFFD | 0x10_0000..=0x10_FFFD);
    let noncharacter = matches!(point, 0xFDD0..=0xFDEF) || point & 0xFFFE == 0xFFFE;
    if private_use || noncharacter || in_ranges(IDNA_DISALLOWED, point) {
        return IdnaMapping::Disallowed;
    }
    let run = IDNA_ASCII.binary_search_by(|&(start, end, _, _)| {
        if end < point {
            std::cmp::Ordering::Less
        } else if start > point {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
    if let Ok(index) = run {
        let (start, _, step, target) = IDNA_ASCII[index];
        if step == 0 {
            return IdnaMapping::Mapped(target.to_owned());
        }
        let first = target.bytes().next().map_or(0, u32::from);
        let shifted = char::from_u32(first + (point - start)).unwrap_or(ch);
        return IdnaMapping::Mapped(shifted.to_string());
    }
    let mut lower = ch.to_lowercase();
    if lower.len() == 1 && lower.next() == Some(ch) {
        IdnaMapping::Valid
    } else {
        IdnaMapping::Mapped(ch.to_lowercase().collect())
    }
}

/// Canonical composition (the C of NFC) for the pairs in [`COMPOSITIONS`], each mark composing with the character
/// right before it. Input is expected in canonical order, as typed text is.
fn compose(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        let composed = out.chars().next_back().and_then(|base| {
            let (_, bases, composites) = COMPOSITIONS.iter().find(|(mark, _, _)| *mark == ch)?;
            let index = bases.chars().position(|candidate| candidate == base)?;
            composites.chars().nth(index)
        });
        match composed {
            Some(composite) => {
                out.pop();
                out.push(composite);
            }
            None => out.push(ch),
        }
    }
    out
}

/// A combining mark that may not start a label (the common combining blocks, the marks [`COMPOSITIONS`] knows and
/// the viramas).
fn is_combining_mark(ch: char) -> bool {
    matches!(
        u32::from(ch),
        0x300..=0x36F | 0x483..=0x489 | 0x591..=0x5BD | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F
    ) || COMPOSITIONS.iter().any(|(mark, _, _)| *mark == ch)
        || VIRAMAS.contains(&u32::from(ch))
        || ch == '\u{903}'
}

/// The UTS #46 label checks beyond the mapping: no leading combining mark, and ZERO WIDTH (NON-)JOINER only right
/// after a virama (the joining-type context for ZWNJ is not modelled).
fn label_rules_hold(label: &str) -> bool {
    if label.chars().next().is_some_and(is_combining_mark) {
        return false;
    }
    let mut previous = None;
    for ch in label.chars() {
        if matches!(ch, '\u{200C}' | '\u{200D}')
            && !previous.is_some_and(|before: char| VIRAMAS.contains(&u32::from(before)))
        {
            return false;
        }
        previous = Some(ch);
    }
    true
}

/// RFC 3492 punycode encoding of one label.
fn punycode(label: &str) -> Option<String> {
    let digit = |value: u32| -> char {
        let byte = if value < 26 {
            b'a' + value as u8
        } else {
            b'0' + (value - 26) as u8
        };
        byte as char
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
                let mut k = PUNY_BASE;
                loop {
                    let t = punycode_threshold(k, bias);
                    if q < t {
                        break;
                    }
                    output.push(digit(t + (q - t) % (PUNY_BASE - t)));
                    q = (q - t) / (PUNY_BASE - t);
                    k += PUNY_BASE;
                }
                output.push(digit(q));
                bias = punycode_adapt(delta, handled + 1, handled == basic);
                delta = 0;
                handled += 1;
            }
        }
        delta = delta.checked_add(1)?;
        n = n.checked_add(1)?;
    }
    Some(output)
}

const PUNY_BASE: u32 = 36;
const PUNY_T_MIN: u32 = 1;
const PUNY_T_MAX: u32 = 26;

fn punycode_threshold(k: u32, bias: u32) -> u32 {
    if k <= bias {
        PUNY_T_MIN
    } else if k >= bias + PUNY_T_MAX {
        PUNY_T_MAX
    } else {
        k - bias
    }
}

fn punycode_adapt(mut delta: u32, points: u32, first: bool) -> u32 {
    const SKEW: u32 = 38;
    const DAMP: u32 = 700;
    delta /= if first { DAMP } else { 2 };
    delta += delta / points;
    let mut k = 0;
    while delta > ((PUNY_BASE - PUNY_T_MIN) * PUNY_T_MAX) / 2 {
        delta /= PUNY_BASE - PUNY_T_MIN;
        k += PUNY_BASE;
    }
    k + (PUNY_BASE - PUNY_T_MIN + 1) * delta / (delta + SKEW)
}

/// RFC 3492 punycode decoding of one label (without its `xn--`), `None` when it is not valid punycode.
fn punycode_decode(input: &str) -> Option<String> {
    if !input.is_ascii() {
        return None;
    }
    let (basic, digits) = match input.rfind('-') {
        Some(index) => (&input[..index], &input[index + 1..]),
        None => ("", input),
    };
    let mut output: Vec<char> = basic.chars().collect();
    let (mut n, mut i, mut bias) = (128u32, 0u32, 72u32);
    let mut digits = digits.bytes().peekable();
    while digits.peek().is_some() {
        let old_i = i;
        let mut weight = 1u32;
        let mut k = PUNY_BASE;
        loop {
            let digit = match digits.next()? {
                byte @ b'a'..=b'z' => u32::from(byte - b'a'),
                byte @ b'A'..=b'Z' => u32::from(byte - b'A'),
                byte @ b'0'..=b'9' => u32::from(byte - b'0') + 26,
                _ => return None,
            };
            i = i.checked_add(digit.checked_mul(weight)?)?;
            let t = punycode_threshold(k, bias);
            if digit < t {
                break;
            }
            weight = weight.checked_mul(PUNY_BASE - t)?;
            k += PUNY_BASE;
        }
        let length = output.len() as u32 + 1;
        bias = punycode_adapt(i - old_i, length, old_i == 0);
        n = n.checked_add(i / length)?;
        i %= length;
        output.insert(i as usize, char::from_u32(n)?);
        i += 1;
    }
    Some(output.into_iter().collect())
}

// UTS #46 data, generated by probing Node 24's `new URL("http://a" + ch + "b.com/").host` for every code point (the
// TS oracle's WHATWG parser). `IDNA_ASCII` holds every mapping whose result is ASCII as `(first, last, step, target)`:
// step 0 maps the whole run to `target`, step 1 maps `first + i` to the character `target + i`.
#[rustfmt::skip]
const IDNA_ASCII: &[(u32, u32, u8, &str)] = &[
    (0xAA, 0xAA, 0, "a"), (0xB2, 0xB3, 1, "2"), (0xB9, 0xB9, 0, "1"), (0xBA, 0xBA, 0, "o"),
    (0x132, 0x133, 0, "ij"), (0x17F, 0x17F, 0, "s"), (0x1C7, 0x1C9, 0, "lj"), (0x1CA, 0x1CC, 0, "nj"),
    (0x1F1, 0x1F3, 0, "dz"), (0x2B0, 0x2B0, 0, "h"), (0x2B2, 0x2B2, 0, "j"), (0x2B3, 0x2B3, 0, "r"),
    (0x2B7, 0x2B7, 0, "w"), (0x2B8, 0x2B8, 0, "y"), (0x2E1, 0x2E1, 0, "l"), (0x2E2, 0x2E2, 0, "s"),
    (0x2E3, 0x2E3, 0, "x"), (0x37E, 0x37E, 0, ";"), (0x1D2C, 0x1D2C, 0, "a"), (0x1D2E, 0x1D2E, 0, "b"),
    (0x1D30, 0x1D31, 1, "d"), (0x1D33, 0x1D3A, 1, "g"), (0x1D3C, 0x1D3C, 0, "o"), (0x1D3E, 0x1D3E, 0, "p"),
    (0x1D3F, 0x1D3F, 0, "r"), (0x1D40, 0x1D41, 1, "t"), (0x1D42, 0x1D42, 0, "w"), (0x1D43, 0x1D43, 0, "a"),
    (0x1D47, 0x1D47, 0, "b"), (0x1D48, 0x1D49, 1, "d"), (0x1D4D, 0x1D4D, 0, "g"), (0x1D4F, 0x1D4F, 0, "k"),
    (0x1D50, 0x1D50, 0, "m"), (0x1D52, 0x1D52, 0, "o"), (0x1D56, 0x1D56, 0, "p"), (0x1D57, 0x1D58, 1, "t"),
    (0x1D5B, 0x1D5B, 0, "v"), (0x1D62, 0x1D62, 0, "i"), (0x1D63, 0x1D63, 0, "r"), (0x1D64, 0x1D65, 1, "u"),
    (0x1D9C, 0x1D9C, 0, "c"), (0x1DA0, 0x1DA0, 0, "f"), (0x1DBB, 0x1DBB, 0, "z"), (0x1FEF, 0x1FEF, 0, "`"),
    (0x203C, 0x203C, 0, "!!"), (0x2070, 0x2070, 0, "0"), (0x2071, 0x2071, 0, "i"), (0x2074, 0x2079, 1, "4"),
    (0x207A, 0x207A, 0, "+"), (0x207C, 0x207C, 0, "="), (0x207D, 0x207E, 1, "("), (0x207F, 0x207F, 0, "n"),
    (0x2080, 0x2089, 1, "0"), (0x208A, 0x208A, 0, "+"), (0x208C, 0x208C, 0, "="), (0x208D, 0x208E, 1, "("),
    (0x2090, 0x2090, 0, "a"), (0x2091, 0x2091, 0, "e"), (0x2092, 0x2092, 0, "o"), (0x2093, 0x2093, 0, "x"),
    (0x2095, 0x2095, 0, "h"), (0x2096, 0x2099, 1, "k"), (0x209A, 0x209A, 0, "p"), (0x209B, 0x209C, 1, "s"),
    (0x20A8, 0x20A8, 0, "rs"), (0x2102, 0x2102, 0, "c"), (0x210A, 0x210B, 1, "g"), (0x210C, 0x210E, 0, "h"),
    (0x2110, 0x2111, 0, "i"), (0x2112, 0x2113, 0, "l"), (0x2115, 0x2115, 0, "n"), (0x2116, 0x2116, 0, "no"),
    (0x2119, 0x211B, 1, "p"), (0x211C, 0x211D, 0, "r"), (0x2120, 0x2120, 0, "sm"), (0x2121, 0x2121, 0, "tel"),
    (0x2122, 0x2122, 0, "tm"), (0x2124, 0x2124, 0, "z"), (0x2128, 0x2128, 0, "z"), (0x212C, 0x212D, 1, "b"),
    (0x212F, 0x2130, 0, "e"), (0x2131, 0x2131, 0, "f"), (0x2133, 0x2133, 0, "m"), (0x2134, 0x2134, 0, "o"),
    (0x2139, 0x2139, 0, "i"), (0x213B, 0x213B, 0, "fax"), (0x2145, 0x2146, 0, "d"), (0x2147, 0x2147, 0, "e"),
    (0x2148, 0x2149, 1, "i"), (0x2160, 0x2160, 0, "i"), (0x2161, 0x2161, 0, "ii"), (0x2162, 0x2162, 0, "iii"),
    (0x2163, 0x2163, 0, "iv"), (0x2164, 0x2164, 0, "v"), (0x2165, 0x2165, 0, "vi"), (0x2166, 0x2166, 0, "vii"),
    (0x2167, 0x2167, 0, "viii"), (0x2168, 0x2168, 0, "ix"), (0x2169, 0x2169, 0, "x"), (0x216A, 0x216A, 0, "xi"),
    (0x216B, 0x216B, 0, "xii"), (0x216C, 0x216C, 0, "l"), (0x216D, 0x216E, 1, "c"), (0x216F, 0x216F, 0, "m"),
    (0x2170, 0x2170, 0, "i"), (0x2171, 0x2171, 0, "ii"), (0x2172, 0x2172, 0, "iii"), (0x2173, 0x2173, 0, "iv"),
    (0x2174, 0x2174, 0, "v"), (0x2175, 0x2175, 0, "vi"), (0x2176, 0x2176, 0, "vii"), (0x2177, 0x2177, 0, "viii"),
    (0x2178, 0x2178, 0, "ix"), (0x2179, 0x2179, 0, "x"), (0x217A, 0x217A, 0, "xi"), (0x217B, 0x217B, 0, "xii"),
    (0x217C, 0x217C, 0, "l"), (0x217D, 0x217E, 1, "c"), (0x217F, 0x217F, 0, "m"), (0x2460, 0x2468, 1, "1"),
    (0x2469, 0x2469, 0, "10"), (0x246A, 0x246A, 0, "11"), (0x246B, 0x246B, 0, "12"), (0x246C, 0x246C, 0, "13"),
    (0x246D, 0x246D, 0, "14"), (0x246E, 0x246E, 0, "15"), (0x246F, 0x246F, 0, "16"), (0x2470, 0x2470, 0, "17"),
    (0x2471, 0x2471, 0, "18"), (0x2472, 0x2472, 0, "19"), (0x2473, 0x2473, 0, "20"), (0x2474, 0x2474, 0, "(1)"),
    (0x2475, 0x2475, 0, "(2)"), (0x2476, 0x2476, 0, "(3)"), (0x2477, 0x2477, 0, "(4)"), (0x2478, 0x2478, 0, "(5)"),
    (0x2479, 0x2479, 0, "(6)"), (0x247A, 0x247A, 0, "(7)"), (0x247B, 0x247B, 0, "(8)"), (0x247C, 0x247C, 0, "(9)"),
    (0x247D, 0x247D, 0, "(10)"), (0x247E, 0x247E, 0, "(11)"), (0x247F, 0x247F, 0, "(12)"),
    (0x2480, 0x2480, 0, "(13)"), (0x2481, 0x2481, 0, "(14)"), (0x2482, 0x2482, 0, "(15)"),
    (0x2483, 0x2483, 0, "(16)"), (0x2484, 0x2484, 0, "(17)"), (0x2485, 0x2485, 0, "(18)"),
    (0x2486, 0x2486, 0, "(19)"), (0x2487, 0x2487, 0, "(20)"), (0x249C, 0x249C, 0, "(a)"),
    (0x249D, 0x249D, 0, "(b)"), (0x249E, 0x249E, 0, "(c)"), (0x249F, 0x249F, 0, "(d)"), (0x24A0, 0x24A0, 0, "(e)"),
    (0x24A1, 0x24A1, 0, "(f)"), (0x24A2, 0x24A2, 0, "(g)"), (0x24A3, 0x24A3, 0, "(h)"), (0x24A4, 0x24A4, 0, "(i)"),
    (0x24A5, 0x24A5, 0, "(j)"), (0x24A6, 0x24A6, 0, "(k)"), (0x24A7, 0x24A7, 0, "(l)"), (0x24A8, 0x24A8, 0, "(m)"),
    (0x24A9, 0x24A9, 0, "(n)"), (0x24AA, 0x24AA, 0, "(o)"), (0x24AB, 0x24AB, 0, "(p)"), (0x24AC, 0x24AC, 0, "(q)"),
    (0x24AD, 0x24AD, 0, "(r)"), (0x24AE, 0x24AE, 0, "(s)"), (0x24AF, 0x24AF, 0, "(t)"), (0x24B0, 0x24B0, 0, "(u)"),
    (0x24B1, 0x24B1, 0, "(v)"), (0x24B2, 0x24B2, 0, "(w)"), (0x24B3, 0x24B3, 0, "(x)"), (0x24B4, 0x24B4, 0, "(y)"),
    (0x24B5, 0x24B5, 0, "(z)"), (0x24B6, 0x24CF, 1, "a"), (0x24D0, 0x24E9, 1, "a"), (0x24EA, 0x24EA, 0, "0"),
    (0x2A75, 0x2A75, 0, "=="), (0x2A76, 0x2A76, 0, "==="), (0x2C7C, 0x2C7C, 0, "j"), (0x2C7D, 0x2C7D, 0, "v"),
    (0x3002, 0x3002, 0, "."), (0x3250, 0x3250, 0, "pte"), (0x3251, 0x3251, 0, "21"), (0x3252, 0x3252, 0, "22"),
    (0x3253, 0x3253, 0, "23"), (0x3254, 0x3254, 0, "24"), (0x3255, 0x3255, 0, "25"), (0x3256, 0x3256, 0, "26"),
    (0x3257, 0x3257, 0, "27"), (0x3258, 0x3258, 0, "28"), (0x3259, 0x3259, 0, "29"), (0x325A, 0x325A, 0, "30"),
    (0x325B, 0x325B, 0, "31"), (0x325C, 0x325C, 0, "32"), (0x325D, 0x325D, 0, "33"), (0x325E, 0x325E, 0, "34"),
    (0x325F, 0x325F, 0, "35"), (0x32B1, 0x32B1, 0, "36"), (0x32B2, 0x32B2, 0, "37"), (0x32B3, 0x32B3, 0, "38"),
    (0x32B4, 0x32B4, 0, "39"), (0x32B5, 0x32B5, 0, "40"), (0x32B6, 0x32B6, 0, "41"), (0x32B7, 0x32B7, 0, "42"),
    (0x32B8, 0x32B8, 0, "43"), (0x32B9, 0x32B9, 0, "44"), (0x32BA, 0x32BA, 0, "45"), (0x32BB, 0x32BB, 0, "46"),
    (0x32BC, 0x32BC, 0, "47"), (0x32BD, 0x32BD, 0, "48"), (0x32BE, 0x32BE, 0, "49"), (0x32BF, 0x32BF, 0, "50"),
    (0x32CC, 0x32CC, 0, "hg"), (0x32CD, 0x32CD, 0, "erg"), (0x32CE, 0x32CE, 0, "ev"), (0x32CF, 0x32CF, 0, "ltd"),
    (0x3371, 0x3371, 0, "hpa"), (0x3372, 0x3372, 0, "da"), (0x3373, 0x3373, 0, "au"), (0x3374, 0x3374, 0, "bar"),
    (0x3375, 0x3375, 0, "ov"), (0x3376, 0x3376, 0, "pc"), (0x3377, 0x3377, 0, "dm"), (0x3378, 0x3378, 0, "dm2"),
    (0x3379, 0x3379, 0, "dm3"), (0x337A, 0x337A, 0, "iu"), (0x3380, 0x3380, 0, "pa"), (0x3381, 0x3381, 0, "na"),
    (0x3383, 0x3383, 0, "ma"), (0x3384, 0x3384, 0, "ka"), (0x3385, 0x3385, 0, "kb"), (0x3386, 0x3386, 0, "mb"),
    (0x3387, 0x3387, 0, "gb"), (0x3388, 0x3388, 0, "cal"), (0x3389, 0x3389, 0, "kcal"), (0x338A, 0x338A, 0, "pf"),
    (0x338B, 0x338B, 0, "nf"), (0x338E, 0x338E, 0, "mg"), (0x338F, 0x338F, 0, "kg"), (0x3390, 0x3390, 0, "hz"),
    (0x3391, 0x3391, 0, "khz"), (0x3392, 0x3392, 0, "mhz"), (0x3393, 0x3393, 0, "ghz"), (0x3394, 0x3394, 0, "thz"),
    (0x3396, 0x3396, 0, "ml"), (0x3397, 0x3397, 0, "dl"), (0x3398, 0x3398, 0, "kl"), (0x3399, 0x3399, 0, "fm"),
    (0x339A, 0x339A, 0, "nm"), (0x339C, 0x339C, 0, "mm"), (0x339D, 0x339D, 0, "cm"), (0x339E, 0x339E, 0, "km"),
    (0x339F, 0x339F, 0, "mm2"), (0x33A0, 0x33A0, 0, "cm2"), (0x33A1, 0x33A1, 0, "m2"), (0x33A2, 0x33A2, 0, "km2"),
    (0x33A3, 0x33A3, 0, "mm3"), (0x33A4, 0x33A4, 0, "cm3"), (0x33A5, 0x33A5, 0, "m3"), (0x33A6, 0x33A6, 0, "km3"),
    (0x33A9, 0x33A9, 0, "pa"), (0x33AA, 0x33AA, 0, "kpa"), (0x33AB, 0x33AB, 0, "mpa"), (0x33AC, 0x33AC, 0, "gpa"),
    (0x33AD, 0x33AD, 0, "rad"), (0x33B0, 0x33B0, 0, "ps"), (0x33B1, 0x33B1, 0, "ns"), (0x33B3, 0x33B3, 0, "ms"),
    (0x33B4, 0x33B4, 0, "pv"), (0x33B5, 0x33B5, 0, "nv"), (0x33B7, 0x33B7, 0, "mv"), (0x33B8, 0x33B8, 0, "kv"),
    (0x33B9, 0x33B9, 0, "mv"), (0x33BA, 0x33BA, 0, "pw"), (0x33BB, 0x33BB, 0, "nw"), (0x33BD, 0x33BD, 0, "mw"),
    (0x33BE, 0x33BE, 0, "kw"), (0x33BF, 0x33BF, 0, "mw"), (0x33C3, 0x33C3, 0, "bq"), (0x33C4, 0x33C4, 0, "cc"),
    (0x33C5, 0x33C5, 0, "cd"), (0x33C8, 0x33C8, 0, "db"), (0x33C9, 0x33C9, 0, "gy"), (0x33CA, 0x33CA, 0, "ha"),
    (0x33CB, 0x33CB, 0, "hp"), (0x33CC, 0x33CC, 0, "in"), (0x33CD, 0x33CD, 0, "kk"), (0x33CE, 0x33CE, 0, "km"),
    (0x33CF, 0x33CF, 0, "kt"), (0x33D0, 0x33D0, 0, "lm"), (0x33D1, 0x33D1, 0, "ln"), (0x33D2, 0x33D2, 0, "log"),
    (0x33D3, 0x33D3, 0, "lx"), (0x33D4, 0x33D4, 0, "mb"), (0x33D5, 0x33D5, 0, "mil"), (0x33D6, 0x33D6, 0, "mol"),
    (0x33D7, 0x33D7, 0, "ph"), (0x33D9, 0x33D9, 0, "ppm"), (0x33DA, 0x33DA, 0, "pr"), (0x33DB, 0x33DB, 0, "sr"),
    (0x33DC, 0x33DC, 0, "sv"), (0x33DD, 0x33DD, 0, "wb"), (0x33FF, 0x33FF, 0, "gal"), (0xA7F1, 0xA7F1, 0, "s"),
    (0xA7F2, 0xA7F2, 0, "c"), (0xA7F3, 0xA7F3, 0, "f"), (0xA7F4, 0xA7F4, 0, "q"), (0xFB00, 0xFB00, 0, "ff"),
    (0xFB01, 0xFB01, 0, "fi"), (0xFB02, 0xFB02, 0, "fl"), (0xFB03, 0xFB03, 0, "ffi"), (0xFB04, 0xFB04, 0, "ffl"),
    (0xFB05, 0xFB06, 0, "st"), (0xFB29, 0xFB29, 0, "+"), (0xFE10, 0xFE10, 0, ","), (0xFE14, 0xFE14, 0, ";"),
    (0xFE15, 0xFE15, 0, "!"), (0xFE33, 0xFE34, 0, "_"), (0xFE35, 0xFE36, 1, "("), (0xFE37, 0xFE37, 0, "{"),
    (0xFE38, 0xFE38, 0, "}"), (0xFE4D, 0xFE4F, 0, "_"), (0xFE50, 0xFE50, 0, ","), (0xFE54, 0xFE54, 0, ";"),
    (0xFE57, 0xFE57, 0, "!"), (0xFE59, 0xFE5A, 1, "("), (0xFE5B, 0xFE5B, 0, "{"), (0xFE5C, 0xFE5C, 0, "}"),
    (0xFE60, 0xFE60, 0, "&"), (0xFE61, 0xFE62, 1, "*"), (0xFE63, 0xFE63, 0, "-"), (0xFE66, 0xFE66, 0, "="),
    (0xFE69, 0xFE69, 0, "$"), (0xFF01, 0xFF02, 1, "!"), (0xFF04, 0xFF04, 0, "$"), (0xFF06, 0xFF0E, 1, "&"),
    (0xFF10, 0xFF19, 1, "0"), (0xFF1B, 0xFF1B, 0, ";"), (0xFF1D, 0xFF1D, 0, "="), (0xFF21, 0xFF3A, 1, "a"),
    (0xFF3F, 0xFF5B, 1, "_"), (0xFF5D, 0xFF5E, 1, "}"), (0xFF61, 0xFF61, 0, "."), (0x107A5, 0x107A5, 0, "q"),
    (0x1CCD6, 0x1CCEF, 1, "a"), (0x1CCF0, 0x1CCF9, 1, "0"), (0x1D400, 0x1D419, 1, "a"), (0x1D41A, 0x1D433, 1, "a"),
    (0x1D434, 0x1D44D, 1, "a"), (0x1D44E, 0x1D454, 1, "a"), (0x1D456, 0x1D467, 1, "i"), (0x1D468, 0x1D481, 1, "a"),
    (0x1D482, 0x1D49B, 1, "a"), (0x1D49C, 0x1D49C, 0, "a"), (0x1D49E, 0x1D49F, 1, "c"), (0x1D4A2, 0x1D4A2, 0, "g"),
    (0x1D4A5, 0x1D4A6, 1, "j"), (0x1D4A9, 0x1D4AC, 1, "n"), (0x1D4AE, 0x1D4B5, 1, "s"), (0x1D4B6, 0x1D4B9, 1, "a"),
    (0x1D4BB, 0x1D4BB, 0, "f"), (0x1D4BD, 0x1D4C3, 1, "h"), (0x1D4C5, 0x1D4CF, 1, "p"), (0x1D4D0, 0x1D4E9, 1, "a"),
    (0x1D4EA, 0x1D503, 1, "a"), (0x1D504, 0x1D505, 1, "a"), (0x1D507, 0x1D50A, 1, "d"), (0x1D50D, 0x1D514, 1, "j"),
    (0x1D516, 0x1D51C, 1, "s"), (0x1D51E, 0x1D537, 1, "a"), (0x1D538, 0x1D539, 1, "a"), (0x1D53B, 0x1D53E, 1, "d"),
    (0x1D540, 0x1D544, 1, "i"), (0x1D546, 0x1D546, 0, "o"), (0x1D54A, 0x1D550, 1, "s"), (0x1D552, 0x1D56B, 1, "a"),
    (0x1D56C, 0x1D585, 1, "a"), (0x1D586, 0x1D59F, 1, "a"), (0x1D5A0, 0x1D5B9, 1, "a"), (0x1D5BA, 0x1D5D3, 1, "a"),
    (0x1D5D4, 0x1D5ED, 1, "a"), (0x1D5EE, 0x1D607, 1, "a"), (0x1D608, 0x1D621, 1, "a"), (0x1D622, 0x1D63B, 1, "a"),
    (0x1D63C, 0x1D655, 1, "a"), (0x1D656, 0x1D66F, 1, "a"), (0x1D670, 0x1D689, 1, "a"), (0x1D68A, 0x1D6A3, 1, "a"),
    (0x1D7CE, 0x1D7D7, 1, "0"), (0x1D7D8, 0x1D7E1, 1, "0"), (0x1D7E2, 0x1D7EB, 1, "0"), (0x1D7EC, 0x1D7F5, 1, "0"),
    (0x1D7F6, 0x1D7FF, 1, "0"), (0x1F101, 0x1F101, 0, "0,"), (0x1F102, 0x1F102, 0, "1,"),
    (0x1F103, 0x1F103, 0, "2,"), (0x1F104, 0x1F104, 0, "3,"), (0x1F105, 0x1F105, 0, "4,"),
    (0x1F106, 0x1F106, 0, "5,"), (0x1F107, 0x1F107, 0, "6,"), (0x1F108, 0x1F108, 0, "7,"),
    (0x1F109, 0x1F109, 0, "8,"), (0x1F10A, 0x1F10A, 0, "9,"), (0x1F110, 0x1F110, 0, "(a)"),
    (0x1F111, 0x1F111, 0, "(b)"), (0x1F112, 0x1F112, 0, "(c)"), (0x1F113, 0x1F113, 0, "(d)"),
    (0x1F114, 0x1F114, 0, "(e)"), (0x1F115, 0x1F115, 0, "(f)"), (0x1F116, 0x1F116, 0, "(g)"),
    (0x1F117, 0x1F117, 0, "(h)"), (0x1F118, 0x1F118, 0, "(i)"), (0x1F119, 0x1F119, 0, "(j)"),
    (0x1F11A, 0x1F11A, 0, "(k)"), (0x1F11B, 0x1F11B, 0, "(l)"), (0x1F11C, 0x1F11C, 0, "(m)"),
    (0x1F11D, 0x1F11D, 0, "(n)"), (0x1F11E, 0x1F11E, 0, "(o)"), (0x1F11F, 0x1F11F, 0, "(p)"),
    (0x1F120, 0x1F120, 0, "(q)"), (0x1F121, 0x1F121, 0, "(r)"), (0x1F122, 0x1F122, 0, "(s)"),
    (0x1F123, 0x1F123, 0, "(t)"), (0x1F124, 0x1F124, 0, "(u)"), (0x1F125, 0x1F125, 0, "(v)"),
    (0x1F126, 0x1F126, 0, "(w)"), (0x1F127, 0x1F127, 0, "(x)"), (0x1F128, 0x1F128, 0, "(y)"),
    (0x1F129, 0x1F129, 0, "(z)"), (0x1F12B, 0x1F12B, 0, "c"), (0x1F12C, 0x1F12C, 0, "r"),
    (0x1F12D, 0x1F12D, 0, "cd"), (0x1F12E, 0x1F12E, 0, "wz"), (0x1F130, 0x1F149, 1, "a"),
    (0x1F14A, 0x1F14A, 0, "hv"), (0x1F14B, 0x1F14B, 0, "mv"), (0x1F14C, 0x1F14C, 0, "sd"),
    (0x1F14D, 0x1F14D, 0, "ss"), (0x1F14E, 0x1F14E, 0, "ppv"), (0x1F14F, 0x1F14F, 0, "wc"),
    (0x1F16A, 0x1F16A, 0, "mc"), (0x1F16B, 0x1F16B, 0, "md"), (0x1F16C, 0x1F16C, 0, "mr"),
    (0x1F190, 0x1F190, 0, "dj"), (0x1FBF0, 0x1FBF9, 1, "0"),
];
// Code points UTS #46 drops.
#[rustfmt::skip]
const IDNA_IGNORED: &[(u32, u32)] = &[
    (0xAD, 0xAD), (0x34F, 0x34F), (0x115F, 0x1160), (0x17B4, 0x17B5), (0x180B, 0x180F), (0x200B, 0x200B),
    (0x2060, 0x2064), (0x206A, 0x206F), (0x3164, 0x3164), (0xFE00, 0xFE0F), (0xFEFF, 0xFEFF), (0xFFA0, 0xFFA0),
    (0x1BCA0, 0x1BCA3), (0x1D173, 0x1D17A), (0xE0100, 0xE01EF),
];
// Assigned code points that fail in every context (disallowed, or mapped to a forbidden host code point).
#[rustfmt::skip]
const IDNA_DISALLOWED: &[(u32, u32)] = &[
    (0x80, 0xA0), (0xA8, 0xA8), (0xAF, 0xAF), (0xB4, 0xB4), (0xB8, 0xB8), (0x2D8, 0x2DD), (0x37A, 0x37A),
    (0x384, 0x385), (0x600, 0x605), (0x61C, 0x61C), (0x6DD, 0x6DD), (0x70F, 0x70F), (0x890, 0x891), (0x8E2, 0x8E2),
    (0x1680, 0x1680), (0x1FBD, 0x1FBD), (0x1FBF, 0x1FC1), (0x1FCD, 0x1FCF), (0x1FDD, 0x1FDF), (0x1FED, 0x1FEE),
    (0x1FFD, 0x1FFE), (0x2000, 0x200A), (0x200C, 0x200F), (0x2017, 0x2017), (0x2024, 0x2026), (0x2028, 0x202F),
    (0x203E, 0x203E), (0x2047, 0x2049), (0x205F, 0x205F), (0x2066, 0x2069), (0x2100, 0x2101), (0x2105, 0x2106),
    (0x2488, 0x249B), (0x2A74, 0x2A74), (0x2FF0, 0x3000), (0x309B, 0x309C), (0x31EF, 0x31EF), (0x33C2, 0x33C2),
    (0x33C7, 0x33C7), (0x33D8, 0x33D8), (0xFC5E, 0xFC63), (0xFDFA, 0xFDFB), (0xFE12, 0xFE13), (0xFE16, 0xFE16),
    (0xFE19, 0xFE19), (0xFE30, 0xFE30), (0xFE47, 0xFE4C), (0xFE52, 0xFE52), (0xFE55, 0xFE56), (0xFE5F, 0xFE5F),
    (0xFE64, 0xFE65), (0xFE68, 0xFE68), (0xFE6A, 0xFE6B), (0xFE70, 0xFE70), (0xFE72, 0xFE72), (0xFE74, 0xFE74),
    (0xFE76, 0xFE76), (0xFE78, 0xFE78), (0xFE7A, 0xFE7A), (0xFE7C, 0xFE7C), (0xFE7E, 0xFE7E), (0xFF03, 0xFF03),
    (0xFF05, 0xFF05), (0xFF0F, 0xFF0F), (0xFF1A, 0xFF1A), (0xFF1C, 0xFF1C), (0xFF1E, 0xFF20), (0xFF3B, 0xFF3E),
    (0xFF5C, 0xFF5C), (0xFFE3, 0xFFE3), (0xFFF9, 0xFFFD), (0x110BD, 0x110BD), (0x110CD, 0x110CD),
    (0x13430, 0x1343F), (0x1F100, 0x1F100), (0xE0001, 0xE0001), (0xE0020, 0xE007F),
];
// Code points after which ZERO WIDTH (NON-)JOINER is allowed (canonical combining class Virama).
#[rustfmt::skip]
const VIRAMAS: &[u32] = &[
    0x94D, 0x9CD, 0xA4D, 0xACD, 0xB4D, 0xBCD, 0xC4D, 0xCCD, 0xD3B, 0xD3C, 0xD4D, 0xDCA, 0xE3A, 0xEBA, 0xF84,
    0x1039, 0x103A, 0x1714, 0x1734, 0x17D2, 0x1A60, 0x1B44, 0x1BAA, 0x1BAB, 0x1BF2, 0x1BF3, 0x2D7F, 0xA806, 0xA82C,
    0xA8C4, 0xA953, 0xA9C0, 0xAAF6, 0xABED, 0x10A3F, 0x11046, 0x1107F, 0x110B9, 0x11133, 0x11134, 0x111C0, 0x11235,
    0x112EA, 0x1134D, 0x11442, 0x114C2, 0x115BF, 0x1163F, 0x116B6, 0x1172B, 0x11839, 0x1193D, 0x1193E, 0x119E0,
    0x11A34, 0x11A47, 0x11A99, 0x11C3F, 0x11D44, 0x11D45, 0x11D97,
];

// Canonical compositions `(mark, bases, composites)`: the n-th base followed by the mark composes to the n-th
// composite. Generated with `String.prototype.normalize` over U+00C0..U+052F and U+1E00..U+1FFF.
#[rustfmt::skip]
const COMPOSITIONS: &[(char, &str, &str)] = &[
    ('\u{300}', "AEIOUaeiouÜüNnЕИеиĒēŌōWwÂâĂăÊêÔôƠơƯưYyἀἁἈἉἐἑἘἙἠἡἨἩἰἱἸἹὀὁὈὉὐὑὙὠὡὨὩαεηιουωΑΕΗ᾿ϊΙ῾ϋΥ¨ΟΩ", "ÀÈÌÒÙàèìòùǛǜǸǹЀЍѐѝḔḕṐṑẀẁẦầẰằỀềỒồỜờỪừỲỳἂἃἊἋἒἓἚἛἢἣἪἫἲἳἺἻὂὃὊὋὒὓὛὢὣὪὫὰὲὴὶὸὺὼᾺῈῊ῍ῒῚ῝ῢῪ῭ῸῺ"),
    ('\u{301}', "AEIOUYaeiouyCcLlNnRrSsZzÜüGgÅåÆæØø¨ΑΕΗΙΟΥΩϊαεηιϋουωϒГКгкÇçĒēÏïKkMmÕõŌōPpŨũWwÂâĂăÊêÔôƠơƯưἀἁἈἉἐἑἘἙἠἡἨἩἰἱἸἹὀὁὈὉὐὑὙὠὡὨὩ᾿῾", "ÁÉÍÓÚÝáéíóúýĆćĹĺŃńŔŕŚśŹźǗǘǴǵǺǻǼǽǾǿ΅ΆΈΉΊΌΎΏΐάέήίΰόύώϓЃЌѓќḈḉḖḗḮḯḰḱḾḿṌṍṒṓṔṕṸṹẂẃẤấẮắẾếỐốỚớỨứἄἅἌἍἔἕἜἝἤἥἬἭἴἵἼἽὄὅὌὍὔὕὝὤὥὬὭ῎῞"),
    ('\u{302}', "AEIOUaeiouCcGgHhJjSsWwYyZzẠạẸẹỌọ", "ÂÊÎÔÛâêîôûĈĉĜĝĤĥĴĵŜŝŴŵŶŷẐẑẬậỆệỘộ"),
    ('\u{303}', "ANOanoIiUuVvÂâĂăEeÊêÔôƠơƯưYy", "ÃÑÕãñõĨĩŨũṼṽẪẫẴẵẼẽỄễỖỗỠỡỮữỸỹ"),
    ('\u{304}', "AaEeIiOoUuÜüÄäȦȧÆæǪǫÖöÕõȮȯYyИиУуGgḶḷṚṛαΑιΙυΥ", "ĀāĒēĪīŌōŪūǕǖǞǟǠǡǢǣǬǭȪȫȬȭȰȱȲȳӢӣӮӯḠḡḸḹṜṝᾱᾹῑῙῡῩ"),
    ('\u{306}', "AaEeGgIiOoUuУИиуЖжАаЕеȨȩẠạαΑιΙυΥ", "ĂăĔĕĞğĬĭŎŏŬŭЎЙйўӁӂӐӑӖӗḜḝẶặᾰᾸῐῘῠῨ"),
    ('\u{307}', "CcEeGgIZzAaOoBbDdFfHhMmNnPpRrSsŚśŠšṢṣTtWwXxYyſ", "ĊċĖėĠġİŻżȦȧȮȯḂḃḊḋḞḟḢḣṀṁṄṅṖṗṘṙṠṡṤṥṦṧṨṩṪṫẆẇẊẋẎẏẛ"),
    ('\u{308}', "AEIOUaeiouyYΙΥιυϒЕІеіАаӘәЖжЗзИиОоӨөЭэУуЧчЫыHhÕõŪūWwXxt", "ÄËÏÖÜäëïöüÿŸΪΫϊϋϔЁЇёїӒӓӚӛӜӝӞӟӤӥӦӧӪӫӬӭӰӱӴӵӸӹḦḧṎṏṺṻẄẅẌẍẗ"),
    ('\u{309}', "AaÂâĂăEeÊêIiOoÔôƠơUuƯưYy", "ẢảẨẩẲẳẺẻỂểỈỉỎỏỔổỞởỦủỬửỶỷ"),
    ('\u{30A}', "AaUuwy", "ÅåŮůẘẙ"),
    ('\u{30B}', "OoUuУу", "ŐőŰűӲӳ"),
    ('\u{30C}', "CcDdEeLlNnRrSsTtZzAaIiOoUuÜüGgKkƷʒjHh", "ČčĎďĚěĽľŇňŘřŠšŤťŽžǍǎǏǐǑǒǓǔǙǚǦǧǨǩǮǯǰȞȟ"),
    ('\u{30F}', "AaEeIiOoRrUuѴѵ", "ȀȁȄȅȈȉȌȍȐȑȔȕѶѷ"),
    ('\u{311}', "AaEeIiOoRrUu", "ȂȃȆȇȊȋȎȏȒȓȖȗ"),
    ('\u{313}', "αΑεΕηΗιΙοΟυωΩρ", "ἀἈἐἘἠἨἰἸὀὈὐὠὨῤ"),
    ('\u{314}', "αΑεΕηΗιΙοΟυΥωΩρΡ", "ἁἉἑἙἡἩἱἹὁὉὑὙὡὩῥῬ"),
    ('\u{31B}', "OoUu", "ƠơƯư"),
    ('\u{323}', "BbDdHhKkLlMmNnRrSsTtVvWwZzAaEeIiOoƠơUuƯưYy", "ḄḅḌḍḤḥḲḳḶḷṂṃṆṇṚṛṢṣṬṭṾṿẈẉẒẓẠạẸẹỊịỌọỢợỤụỰựỴỵ"),
    ('\u{324}', "Uu", "Ṳṳ"),
    ('\u{325}', "Aa", "Ḁḁ"),
    ('\u{326}', "SsTt", "ȘșȚț"),
    ('\u{327}', "CcGgKkLlNnRrSsTtEeDdHh", "ÇçĢģĶķĻļŅņŖŗŞşŢţȨȩḐḑḨḩ"),
    ('\u{328}', "AaEeIiUuOo", "ĄąĘęĮįŲųǪǫ"),
    ('\u{32D}', "DdEeLlNnTtUu", "ḒḓḘḙḼḽṊṋṰṱṶṷ"),
    ('\u{32E}', "Hh", "Ḫḫ"),
    ('\u{330}', "EeIiUu", "ḚḛḬḭṴṵ"),
    ('\u{331}', "BbDdKkLlNnRrTtZzh", "ḆḇḎḏḴḵḺḻṈṉṞṟṮṯẔẕẖ"),
    ('\u{342}', "ἀἁἈἉἠἡἨἩἰἱἸἹὐὑὙὠὡὨὩα¨η᾿ιϊ῾υϋω", "ἆἇἎἏἦἧἮἯἶἷἾἿὖὗὟὦὧὮὯᾶ῁ῆ῏ῖῗ῟ῦῧῶ"),
    ('\u{345}', "ἀἁἂἃἄἅἆἇἈἉἊἋἌἍἎἏἠἡἢἣἤἥἦἧἨἩἪἫἬἭἮἯὠὡὢὣὤὥὦὧὨὩὪὫὬὭὮὯὰαάᾶΑὴηήῆΗὼωώῶΩ", "ᾀᾁᾂᾃᾄᾅᾆᾇᾈᾉᾊᾋᾌᾍᾎᾏᾐᾑᾒᾓᾔᾕᾖᾗᾘᾙᾚᾛᾜᾝᾞᾟᾠᾡᾢᾣᾤᾥᾦᾧᾨᾩᾪᾫᾬᾭᾮᾯᾲᾳᾴᾷᾼῂῃῄῇῌῲῳῴῷῼ"),
];

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
            ("http://aא.com/", Some("xn--a-0hc.com")),
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
