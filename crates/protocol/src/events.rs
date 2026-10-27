//! Daemon → client responses and events.
//!
//! Everything the daemon writes to a client socket is a [`ServerMessage`]: a [`ServerResponse`] to a request, or a
//! push event. A response's `data` stays a JSON value in the envelope (its shape depends on the request it answers);
//! [`ResponseData::decode`] types it given the request type.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use serde_with::skip_serializing_none;

use crate::data::{
    AgentUpdateResult, AgentVersions, AttachResult, BranchList, CandidateSizes, ChangeDiff, ChangeStageResult,
    ChangesRecord, CreatePrResult, MergePreview, NoteSaveResult, OkResult, PaneRecord, PingResult, PreviewRecord,
    ProjectInfo, PromoteNoteItemResult, SavedConfigDocument, SendOpenItemsResult, SessionCleanupInspection,
    SessionRecord, SettingsInfo, TasksDoc, WorkspaceSummary, WorktreeCandidates, WorktreeCleanupInspection,
    WorktreeDeleteResult, WorktreeInfoRecord, WorktreeMergeResult, WorktreeOverview,
};
use crate::order::Ordered;

/// Deserialises a field that may be present with a `null` value as `Some(Value::Null)` (absent stays `None`), so
/// `data: null` round-trips.
pub(crate) fn present_value<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

/// types.ts ServerResponse: `{type: 'response', requestId, ok, data?, error?}` (the tag is written by the enclosing
/// message enum).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerResponse {
    /// Empty when the request had none (TS `response(undefined, data)` writes no key).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub request_id: String,
    pub ok: bool,
    #[serde(default, deserialize_with = "present_value")]
    pub data: Option<Value>,
    pub error: Option<String>,
}

impl ServerResponse {
    /// daemon.ts `response()`: a success carrying `data`.
    pub fn success<T: Serialize>(request_id: impl Into<String>, data: &T) -> serde_json::Result<Self> {
        Ok(ServerResponse {
            request_id: request_id.into(),
            ok: true,
            data: Some(serde_json::to_value(data)?),
            error: None,
        })
    }

    /// daemon.ts `failure()`: the error's message.
    pub fn failure(request_id: impl Into<String>, error: impl Into<String>) -> Self {
        ServerResponse {
            request_id: request_id.into(),
            ok: false,
            data: None,
            error: Some(error.into()),
        }
    }
}

/// A push naming only a session (attached/detached and friends).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRef {
    /// Empty when the request named none (TS echoes `undefined`, which writes no key).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
}

/// PTY output for an attached client.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionOutput {
    pub session_id: String,
    pub data: String,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionUpdated {
    pub session: Ordered<SessionRecord>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PreviewUpdated {
    pub preview: PreviewRecord,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TerminalUpdated {
    pub terminal: Ordered<PaneRecord>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GitUpdated {
    pub git: Ordered<PaneRecord>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DevUpdated {
    pub dev: Ordered<PaneRecord>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActionUpdated {
    pub action: Ordered<PaneRecord>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChangesUpdated {
    pub changes: ChangesRecord,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentVersionsUpdated {
    pub versions: AgentVersions,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TasksUpdated {
    pub tasks: TasksDoc,
}

/// types.ts ServerMessage (= ServerResponse | ServerEvent), internally tagged on `type`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    #[serde(rename = "response")]
    Response(ServerResponse),
    #[serde(rename = "output")]
    Output(SessionOutput),
    #[serde(rename = "session-updated")]
    SessionUpdated(Box<SessionUpdated>),
    #[serde(rename = "session-removed")]
    SessionRemoved(SessionRef),
    #[serde(rename = "preview-updated")]
    PreviewUpdated(PreviewUpdated),
    #[serde(rename = "terminal-updated")]
    TerminalUpdated(TerminalUpdated),
    #[serde(rename = "git-updated")]
    GitUpdated(GitUpdated),
    #[serde(rename = "dev-updated")]
    DevUpdated(DevUpdated),
    #[serde(rename = "action-updated")]
    ActionUpdated(ActionUpdated),
    #[serde(rename = "changes-updated")]
    ChangesUpdated(ChangesUpdated),
    #[serde(rename = "agent-versions-updated")]
    AgentVersionsUpdated(Box<AgentVersionsUpdated>),
    #[serde(rename = "tasks-updated")]
    TasksUpdated(TasksUpdated),
    #[serde(rename = "terminal-output")]
    TerminalOutput(SessionOutput),
    #[serde(rename = "git-output")]
    GitOutput(SessionOutput),
    #[serde(rename = "dev-output")]
    DevOutput(SessionOutput),
    #[serde(rename = "action-output")]
    ActionOutput(SessionOutput),
    #[serde(rename = "attached")]
    Attached(SessionRef),
    #[serde(rename = "detached")]
    Detached(SessionRef),
    #[serde(rename = "terminal-attached")]
    TerminalAttached(SessionRef),
    #[serde(rename = "terminal-detached")]
    TerminalDetached(SessionRef),
    #[serde(rename = "git-attached")]
    GitAttached(SessionRef),
    #[serde(rename = "git-detached")]
    GitDetached(SessionRef),
    #[serde(rename = "dev-attached")]
    DevAttached(SessionRef),
    #[serde(rename = "dev-detached")]
    DevDetached(SessionRef),
    #[serde(rename = "action-attached")]
    ActionAttached(SessionRef),
    #[serde(rename = "action-detached")]
    ActionDetached(SessionRef),
}

impl ServerMessage {
    /// Every `type` tag, in declaration order.
    pub const TYPES: &'static [&'static str] = &[
        "response",
        "output",
        "session-updated",
        "session-removed",
        "preview-updated",
        "terminal-updated",
        "git-updated",
        "dev-updated",
        "action-updated",
        "changes-updated",
        "agent-versions-updated",
        "tasks-updated",
        "terminal-output",
        "git-output",
        "dev-output",
        "action-output",
        "attached",
        "detached",
        "terminal-attached",
        "terminal-detached",
        "git-attached",
        "git-detached",
        "dev-attached",
        "dev-detached",
        "action-attached",
        "action-detached",
    ];

    /// The message's `type` tag.
    pub fn type_name(&self) -> &'static str {
        match self {
            ServerMessage::Response(_) => "response",
            ServerMessage::Output(_) => "output",
            ServerMessage::SessionUpdated(_) => "session-updated",
            ServerMessage::SessionRemoved(_) => "session-removed",
            ServerMessage::PreviewUpdated(_) => "preview-updated",
            ServerMessage::TerminalUpdated(_) => "terminal-updated",
            ServerMessage::GitUpdated(_) => "git-updated",
            ServerMessage::DevUpdated(_) => "dev-updated",
            ServerMessage::ActionUpdated(_) => "action-updated",
            ServerMessage::ChangesUpdated(_) => "changes-updated",
            ServerMessage::AgentVersionsUpdated(_) => "agent-versions-updated",
            ServerMessage::TasksUpdated(_) => "tasks-updated",
            ServerMessage::TerminalOutput(_) => "terminal-output",
            ServerMessage::GitOutput(_) => "git-output",
            ServerMessage::DevOutput(_) => "dev-output",
            ServerMessage::ActionOutput(_) => "action-output",
            ServerMessage::Attached(_) => "attached",
            ServerMessage::Detached(_) => "detached",
            ServerMessage::TerminalAttached(_) => "terminal-attached",
            ServerMessage::TerminalDetached(_) => "terminal-detached",
            ServerMessage::GitAttached(_) => "git-attached",
            ServerMessage::GitDetached(_) => "git-detached",
            ServerMessage::DevAttached(_) => "dev-attached",
            ServerMessage::DevDetached(_) => "dev-detached",
            ServerMessage::ActionAttached(_) => "action-attached",
            ServerMessage::ActionDetached(_) => "action-detached",
        }
    }
}

/// A successful response's `data`, typed by the request it answers (see daemon.ts handleRequest).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ResponseData {
    Ping(PingResult),
    Ok(OkResult),
    /// A file path (`export-handoff`, `open-note`, `open-tasks`).
    Path(String),
    Session(Ordered<SessionRecord>),
    Sessions(Vec<Ordered<SessionRecord>>),
    SavedConfig(SavedConfigDocument),
    SettingsInfo(Box<SettingsInfo>),
    WorktreeCandidates(WorktreeCandidates),
    CandidateSizes(CandidateSizes),
    ProjectInfo(Box<ProjectInfo>),
    WorkspaceSummary(WorkspaceSummary),
    CreatePr(CreatePrResult),
    SessionCleanup(SessionCleanupInspection),
    WorktreeCleanup(WorktreeCleanupInspection),
    Pane(Ordered<PaneRecord>),
    Preview(PreviewRecord),
    AgentVersions(Box<AgentVersions>),
    AgentUpdate(Box<AgentUpdateResult>),
    Worktrees(Vec<WorktreeInfoRecord>),
    WorktreeOverview(WorktreeOverview),
    WorktreeDelete(WorktreeDeleteResult),
    Changes(ChangesRecord),
    ChangeDiff(ChangeDiff),
    ChangeStage(ChangeStageResult),
    NoteSave(NoteSaveResult),
    Tasks(TasksDoc),
    SendOpenItems(SendOpenItemsResult),
    PromoteNoteItem(PromoteNoteItemResult),
    Branches(BranchList),
    MergePreview(Box<MergePreview>),
    Merge(Box<Ordered<WorktreeMergeResult>>),
    Attach(Box<Ordered<AttachResult>>),
}

/// `ResponseData::decode` was given a request type the daemon does not answer with data.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("no response data for request type {0}")]
    UnknownRequest(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl ResponseData {
    /// Types a successful response's `data` for a request of type `request_type`.
    pub fn decode(request_type: &str, data: Value) -> Result<Self, DecodeError> {
        use serde_json::from_value as from;
        Ok(match request_type {
            "ping" => ResponseData::Ping(from(data)?),
            "shutdown" | "agent-hook" | "cancel-start" | "stop-dev" | "stop-action" | "kill" | "remove" => {
                ResponseData::Ok(from(data)?)
            }
            "export-handoff" | "open-note" | "open-tasks" => ResponseData::Path(from(data)?),
            "set-session-done"
            | "set-reply-read"
            | "archive-session"
            | "create"
            | "restart"
            | "resolve-merge"
            | "mark-session-merged" => ResponseData::Session(from(data)?),
            "list" | "subscribe" | "reorder-session" => ResponseData::Sessions(from(data)?),
            "save-config" => ResponseData::SavedConfig(from(data)?),
            "settings-info" => ResponseData::SettingsInfo(Box::new(from(data)?)),
            "worktree-candidates" | "global-link-candidates" => ResponseData::WorktreeCandidates(from(data)?),
            "worktree-candidate-sizes" => ResponseData::CandidateSizes(from(data)?),
            "project-info" | "trust-project" => ResponseData::ProjectInfo(Box::new(from(data)?)),
            "workspace-summary" => ResponseData::WorkspaceSummary(from(data)?),
            "create-pr" => ResponseData::CreatePr(from(data)?),
            "inspect-cleanup" => ResponseData::SessionCleanup(from(data)?),
            "inspect-worktree" => ResponseData::WorktreeCleanup(from(data)?),
            "run-action" | "start-dev" | "watch-terminal" | "watch-git" | "watch-dev" | "watch-action" => {
                ResponseData::Pane(from(data)?)
            }
            "watch-preview" => ResponseData::Preview(from(data)?),
            "agent-versions" => ResponseData::AgentVersions(Box::new(from(data)?)),
            "update-agent" => ResponseData::AgentUpdate(Box::new(from(data)?)),
            "list-worktrees" => ResponseData::Worktrees(from(data)?),
            "worktree-overview" => ResponseData::WorktreeOverview(from(data)?),
            "delete-worktree" => ResponseData::WorktreeDelete(from(data)?),
            "watch-changes" => ResponseData::Changes(from(data)?),
            "changes-diff" => ResponseData::ChangeDiff(from(data)?),
            "change-stage" => ResponseData::ChangeStage(from(data)?),
            "save-note" => ResponseData::NoteSave(from(data)?),
            "watch-tasks" | "task-op" | "return-task-to-note" | "task-to-note" => ResponseData::Tasks(from(data)?),
            "send-open-items" => ResponseData::SendOpenItems(from(data)?),
            "promote-note-item" => ResponseData::PromoteNoteItem(from(data)?),
            "list-branches" => ResponseData::Branches(from(data)?),
            "merge-preview" => ResponseData::MergePreview(Box::new(from(data)?)),
            "merge-worktree" => ResponseData::Merge(Box::new(from(data)?)),
            "attach" | "attach-terminal" | "attach-git" | "attach-dev" | "attach-action" => {
                ResponseData::Attach(Box::new(from(data)?))
            }
            other => return Err(DecodeError::UnknownRequest(other.to_owned())),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_keep_present_null_data() {
        let text = r#"{"type":"response","requestId":"r","ok":true,"data":null}"#;
        let message: ServerMessage = serde_json::from_str(text).unwrap();
        assert_eq!(serde_json::to_string(&message).unwrap(), text);
        let text = r#"{"type":"response","requestId":"r","ok":false,"error":"Session does not exist"}"#;
        let message: ServerMessage = serde_json::from_str(text).unwrap();
        assert_eq!(
            message,
            ServerMessage::Response(ServerResponse::failure("r", "Session does not exist"))
        );
        assert_eq!(serde_json::to_string(&message).unwrap(), text);
    }

    #[test]
    fn type_names_match_the_tags() {
        let message = ServerMessage::GitDetached(SessionRef { session_id: "s".into() });
        assert_eq!(serde_json::to_value(&message).unwrap()["type"], message.type_name());
        assert!(ServerMessage::TYPES.contains(&message.type_name()));
    }
}
