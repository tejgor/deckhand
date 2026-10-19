//! Port of agentSignals.ts (pure parts).
//!
//! Agent hook payloads → attention signals, and the hook settings Deckhand passes to agents. The parts that run
//! processes or know the deckhand binary (`hookCommand`, `hookSettings`, `integrationArgs` and its `--help` probe
//! cache) live in `deckhand_sys::agent_path`, which builds on [`hook_settings_for`].

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use serde_with::skip_serializing_none;

use crate::error::Result;
use deckhand_protocol::data::{AgentSessionRef, AttentionReason, AttentionState, ProgramKey};

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
    /// Claude's idle prompt, which only ends a working or unknown state.
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

/// TS: agentSignals.ts normalizeHook (`Ok(None)`: an event that says nothing about attention). Errors:
/// `Hook input must be an object`, `Hook event is missing`.
pub fn normalize_hook(program: ProgramKey, payload: &Value) -> Result<Option<AgentSignal>> {
    let _ = (program, payload);
    todo!("P1")
}

/// TS: agentSignals.ts answersPrompt (keys typed into the agent that answer the prompt on its screen).
pub fn answers_prompt(data: &str) -> bool {
    let _ = data;
    todo!("P1")
}

/// TS: agentSignals.ts oneLine (agent text as one display line, cut to MAX_REASON_LENGTH with `…`).
pub fn one_line(text: &str) -> String {
    let _ = text;
    todo!("P1")
}

/// TS: agentSignals.ts hookPayloadFields (what `deckhand hook` forwards: bounded lifecycle and reason fields).
pub fn hook_payload_fields(payload: &Value) -> Result<Map<String, Value>> {
    let _ = payload;
    todo!("P1")
}

/// TS: agentSignals.ts hooksEnabled (the setting, else on for Claude only).
pub fn hooks_enabled(setting: Option<bool>, program: ProgramKey) -> bool {
    let _ = (setting, program);
    todo!("P1")
}

/// TS: agentSignals.ts attentionMessage (the reason's text, else the state in words).
pub fn attention_message(state: AttentionState, reason: Option<&AttentionReason>) -> String {
    let _ = (state, reason);
    todo!("P1")
}

/// TS: agentSignals.ts shellQuote (`'...'` with `'\''` for quotes).
pub fn shell_quote(value: &str) -> String {
    let _ = value;
    todo!("P1")
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
pub fn hook_settings_for(command: &str, async_: bool) -> HookSettings {
    let _ = (command, async_);
    todo!("P1")
}

/// TS: agentSignals.ts codexResumeFromOutput (the `codex resume <id>` exit hint).
pub fn codex_resume_from_output(output: &str) -> Option<AgentSessionRef> {
    let _ = output;
    todo!("P1")
}

/// TS: agentSignals.ts needsAttention.
pub fn needs_attention(state: Option<AttentionState>) -> bool {
    let _ = state;
    todo!("P1")
}
