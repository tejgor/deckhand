//! Daemon ↔ worker IPC messages.
//!
//! The TS daemon forks two kinds of worker over Node's IPC channel (JSON, one message per line): a session worker per
//! running agent and a workspace worker per worktree (Terminal, Git, Dev and action panes). The Rust port keeps the
//! messages and frames them as NDJSON ([`crate::ndjson`]); a Rust worker's first message is [`Hello`] with
//! [`crate::PROTOCOL_VERSION`] so the daemon can refuse a mismatched binary.
//!
//! Field order: requests the daemon sends through `requestChannel` are `{...payload, requestId}` (the ID last); the
//! direct `kill` sends put it second. Worker messages follow sessionWorker.ts's literals.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_with::skip_serializing_none;

use crate::data::{
    AgentActivityStatus, AttachSnapshot, AttachTarget, OkResult, PaneRecord, PreviewRecord, SessionRecord,
};
use crate::events::{DecodeError, present_value};
use crate::order::Ordered;

/// `snapshot`: the pane's current record (resizing it), for watch requests.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub target: AttachTarget,
    pub cols: i64,
    pub rows: i64,
    pub scroll_offset: Option<i64>,
    pub request_id: String,
}

/// `attach`: start forwarding the pane's output (answered with an [`AttachSnapshot`]).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaneAttach {
    pub target: AttachTarget,
    pub cols: i64,
    pub rows: i64,
    /// Not in the TS: the Dev process to attach to (by name); absent is the first.
    pub process: Option<String>,
    pub request_id: String,
}

/// `detach`: stop forwarding output (no response).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaneDetach {
    pub target: AttachTarget,
}

/// `input`: keys for the pane's PTY (no response).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaneInput {
    pub target: AttachTarget,
    pub data: String,
}

/// `resize`: the attached terminal's size (no response).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaneResize {
    pub target: AttachTarget,
    pub cols: i64,
    pub rows: i64,
}

/// Session worker `start`: spawn the agent PTY for `session` (answered with the session, now running).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartAgent {
    pub session: Ordered<SessionRecord>,
    pub cols: i64,
    pub rows: i64,
    pub request_id: String,
    /// Not in the TS: `claude_scroll_override` (absent: on), for the agent's environment.
    pub scroll_override: Option<bool>,
}

/// Session worker `kill`: SIGTERM the agent's process group (`force`: SIGKILL after a second).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KillAgent {
    pub request_id: String,
    pub force: Option<bool>,
}

/// sessionWorker.ts WorkerCommand: daemon -> session worker.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WorkerCommand {
    #[serde(rename = "snapshot")]
    Snapshot(Snapshot),
    #[serde(rename = "attach")]
    Attach(PaneAttach),
    #[serde(rename = "detach")]
    Detach(PaneDetach),
    #[serde(rename = "input")]
    Input(PaneInput),
    #[serde(rename = "resize")]
    Resize(PaneResize),
    #[serde(rename = "start")]
    Start(Box<StartAgent>),
    #[serde(rename = "kill")]
    Kill(KillAgent),
}

impl WorkerCommand {
    pub const TYPES: &'static [&'static str] = &["snapshot", "attach", "detach", "input", "resize", "start", "kill"];
}

/// Workspace worker `start`: the workspace (worktree root) its panes run in.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartWorkspace {
    pub cwd: String,
    pub request_id: String,
}

/// Workspace worker `start-dev`. With `processes` (not in the TS) Dev runs each named command in a PTY of its own
/// and `command` is unused; `process` starts only that one (`restart`: stopping it first if it runs).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartDev {
    pub command: String,
    pub processes: Option<Vec<DevProcessSpec>>,
    pub process: Option<String>,
    pub restart: Option<bool>,
    pub cols: i64,
    pub rows: i64,
    pub request_id: String,
}

/// Not in the TS: one named Dev command of `start-dev`.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DevProcessSpec {
    pub name: String,
    pub command: String,
}

/// Workspace worker `stop-dev`: every Dev process, or only `process` (not in the TS).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopDev {
    pub process: Option<String>,
    pub request_id: String,
}

/// Workspace worker `start-action`.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartAction {
    pub name: String,
    pub command: String,
    pub cols: i64,
    pub rows: i64,
    pub request_id: String,
}

/// A command with nothing but its ID (`stop-dev`, `stop-action`, `idle`, `shutdown`).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerRequestOnly {
    pub request_id: String,
}

/// sessionWorker.ts WorkspaceWorkerCommand: daemon -> workspace worker.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WorkspaceWorkerCommand {
    #[serde(rename = "snapshot")]
    Snapshot(Snapshot),
    #[serde(rename = "attach")]
    Attach(PaneAttach),
    #[serde(rename = "detach")]
    Detach(PaneDetach),
    #[serde(rename = "input")]
    Input(PaneInput),
    #[serde(rename = "resize")]
    Resize(PaneResize),
    #[serde(rename = "start")]
    Start(StartWorkspace),
    #[serde(rename = "start-dev")]
    StartDev(StartDev),
    #[serde(rename = "stop-dev")]
    StopDev(StopDev),
    #[serde(rename = "start-action")]
    StartAction(StartAction),
    #[serde(rename = "stop-action")]
    StopAction(WorkerRequestOnly),
    #[serde(rename = "idle")]
    Idle(WorkerRequestOnly),
    #[serde(rename = "shutdown")]
    Shutdown(WorkerRequestOnly),
}

impl WorkspaceWorkerCommand {
    pub const TYPES: &'static [&'static str] = &[
        "snapshot",
        "attach",
        "detach",
        "input",
        "resize",
        "start",
        "start-dev",
        "stop-dev",
        "start-action",
        "stop-action",
        "idle",
        "shutdown",
    ];
}

/// A worker's answer to a request: `{type: 'response', requestId, ok, data?}` or `{..., ok: false, error}`.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerResponse {
    pub request_id: String,
    pub ok: bool,
    #[serde(default, deserialize_with = "present_value")]
    pub data: Option<Value>,
    pub error: Option<String>,
}

/// New in the Rust port: the first message a worker sends, naming its protocol version.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub version: u32,
}

impl Default for Hello {
    fn default() -> Self {
        Hello {
            version: crate::PROTOCOL_VERSION,
        }
    }
}

/// `running`: the agent PTY's pid.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Running {
    pub pid: i64,
}

/// `exit`: the agent exited (the worker exits right after).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentExit {
    #[serialize_always]
    pub exit_code: Option<i64>,
    #[serialize_always]
    pub exit_signal: Option<i64>,
    pub last_preview: String,
}

/// `agent-status`: the activity heuristic changed.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatusChanged {
    pub agent_status: AgentActivityStatus,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkerPreviewUpdated {
    pub preview: PreviewRecord,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkerTerminalUpdated {
    pub terminal: Ordered<PaneRecord>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkerGitUpdated {
    pub git: Ordered<PaneRecord>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkerDevUpdated {
    pub dev: Ordered<PaneRecord>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkerActionUpdated {
    pub action: Ordered<PaneRecord>,
}

/// `output`: PTY output of an attached pane.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkerOutput {
    pub target: AttachTarget,
    pub data: String,
}

/// sessionWorker.ts WorkerMessage: worker -> daemon (both kinds), plus the port's `hello`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WorkerMessage {
    #[serde(rename = "hello")]
    Hello(Hello),
    #[serde(rename = "response")]
    Response(WorkerResponse),
    #[serde(rename = "running")]
    Running(Running),
    #[serde(rename = "exit")]
    Exit(AgentExit),
    #[serde(rename = "agent-status")]
    AgentStatus(AgentStatusChanged),
    #[serde(rename = "preview-updated")]
    PreviewUpdated(WorkerPreviewUpdated),
    #[serde(rename = "terminal-updated")]
    TerminalUpdated(WorkerTerminalUpdated),
    #[serde(rename = "git-updated")]
    GitUpdated(WorkerGitUpdated),
    #[serde(rename = "dev-updated")]
    DevUpdated(WorkerDevUpdated),
    #[serde(rename = "action-updated")]
    ActionUpdated(WorkerActionUpdated),
    #[serde(rename = "output")]
    Output(WorkerOutput),
}

impl WorkerMessage {
    pub const TYPES: &'static [&'static str] = &[
        "hello",
        "response",
        "running",
        "exit",
        "agent-status",
        "preview-updated",
        "terminal-updated",
        "git-updated",
        "dev-updated",
        "action-updated",
        "output",
    ];
}

/// `{idle}`: whether nothing lives in the workspace worker any more (`idle`, `stop-dev`, `stop-action`).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IdleResult {
    pub idle: bool,
}

/// A successful worker response's `data`, typed by the command it answers.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum WorkerResponseData {
    /// Session worker `start`: the session as now running.
    Session(Box<Ordered<SessionRecord>>),
    Ok(OkResult),
    Idle(IdleResult),
    /// Agent `snapshot`.
    Preview(PreviewRecord),
    /// Pane `snapshot`, `start-dev`, `start-action`.
    Pane(Ordered<PaneRecord>),
    /// `attach` (either kind).
    Attach(Box<Ordered<AttachSnapshot>>),
}

impl WorkerResponseData {
    /// Types `data` for a `command_type` request to a session worker (`workspace` false) or a workspace worker.
    pub fn decode(workspace: bool, command_type: &str, data: Value) -> Result<Self, DecodeError> {
        use serde_json::from_value as from;
        Ok(match (workspace, command_type) {
            (false, "start") => WorkerResponseData::Session(Box::new(from(data)?)),
            (false, "kill") | (true, "start" | "shutdown") => WorkerResponseData::Ok(from(data)?),
            (false, "snapshot") => WorkerResponseData::Preview(from(data)?),
            (true, "snapshot" | "start-dev" | "start-action") => WorkerResponseData::Pane(from(data)?),
            (_, "attach") => WorkerResponseData::Attach(Box::new(from(data)?)),
            (true, "stop-dev" | "stop-action" | "idle") => WorkerResponseData::Idle(from(data)?),
            (_, other) => return Err(DecodeError::UnknownRequest(other.to_owned())),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_names_the_protocol_version() {
        let hello = WorkerMessage::Hello(Hello::default());
        assert_eq!(
            serde_json::to_string(&hello).unwrap(),
            r#"{"type":"hello","version":49}"#
        );
    }

    #[test]
    fn exit_writes_null_codes() {
        let exit = WorkerMessage::Exit(AgentExit {
            exit_code: None,
            exit_signal: Some(15),
            last_preview: String::new(),
        });
        let text = r#"{"type":"exit","exitCode":null,"exitSignal":15,"lastPreview":""}"#;
        assert_eq!(serde_json::to_string(&exit).unwrap(), text);
        assert_eq!(serde_json::from_str::<WorkerMessage>(text).unwrap(), exit);
    }
}
