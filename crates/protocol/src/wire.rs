//! Port of types.ts (client->daemon requests).
//!
//! [`Request`] is types.ts ClientRequest, internally tagged on `type`. Every variant is a named struct whose field
//! order is the order client.ts (or attach.ts / cli.ts) writes the keys, so a Rust client writes what the TS client
//! writes. Requests with a `requestId` get exactly one response ([`crate::events::ServerResponse`]); the input,
//! resize and detach messages of an attach get none.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_with::skip_serializing_none;

use crate::data::{
    ChangeGroup, ConfigTargetKind, CreateSessionInput, MoveDirection, NoteSection, ProgramKey, ResolveMergeAction,
    RestartMode, StageMode, TaskOp, WorktreeMergeMode,
};

/// A request naming only its ID (`ping`, `shutdown`, `list`).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestOnly {
    pub request_id: String,
}

/// A request about the repository or checkout `cwd` belongs to.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CwdRequest {
    pub request_id: String,
    pub cwd: String,
}

/// A request about one session.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRequest {
    pub request_id: String,
    pub session_id: String,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveConfig {
    pub request_id: String,
    pub target: ConfigTargetKind,
    pub cwd: String,
    pub raw: String,
    /// `string | null`: always written.
    #[serialize_always]
    pub revision: Option<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeCandidateSizes {
    pub request_id: String,
    pub cwd: String,
    pub paths: Vec<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustProject {
    pub request_id: String,
    pub cwd: String,
    pub fingerprint: String,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSummaryRequest {
    pub request_id: String,
    pub session_id: String,
    pub include_pr: Option<bool>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectCleanup {
    pub request_id: String,
    pub session_id: String,
    pub delete_branch: Option<bool>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePr {
    pub request_id: String,
    pub session_id: String,
    pub branch: Option<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveSession {
    pub request_id: String,
    pub session_id: String,
    pub archived: bool,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportHandoff {
    pub request_id: String,
    pub session_id: String,
    pub include_output: Option<bool>,
}

/// cli.ts `hook`: an agent signal relayed from the agent's hook (`payload` is the reduced hook input).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentHook {
    pub request_id: String,
    pub session_id: String,
    pub launch_id: String,
    pub token: String,
    pub payload: Value,
    pub sent_at: Option<i64>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunAction {
    pub request_id: String,
    pub session_id: String,
    pub action: String,
    pub cols: i64,
    pub rows: i64,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentVersionsRequest {
    pub request_id: String,
    pub refresh: Option<bool>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAgent {
    pub request_id: String,
    pub program: ProgramKey,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscribe {
    pub request_id: String,
    pub repo_root: String,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectWorktree {
    pub request_id: String,
    pub cwd: String,
    pub path: String,
    pub delete_branch: Option<bool>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteWorktree {
    pub request_id: String,
    pub cwd: String,
    pub path: String,
    pub branch: Option<String>,
    pub delete_branch: Option<bool>,
    pub stop_sessions: Option<bool>,
    pub allow_data_loss: Option<bool>,
    pub send_open_items: Option<bool>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchPreview {
    pub request_id: String,
    pub session_id: Option<String>,
    pub cols: i64,
    pub rows: i64,
    pub scroll_offset: Option<i64>,
}

/// `watch-terminal`, `watch-git`, `watch-dev`, `watch-action`: no `sessionId` stops watching.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchPane {
    pub request_id: String,
    pub session_id: Option<String>,
    pub cols: i64,
    pub rows: i64,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchChanges {
    pub request_id: String,
    pub session_id: Option<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangesDiff {
    pub request_id: String,
    pub session_id: String,
    pub group: ChangeGroup,
    pub path: String,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeStage {
    pub request_id: String,
    pub session_id: String,
    pub mode: StageMode,
    pub group: Option<ChangeGroup>,
    pub path: Option<String>,
}

/// `start-dev`.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSizeRequest {
    pub request_id: String,
    pub session_id: String,
    pub cols: i64,
    pub rows: i64,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveNote {
    pub request_id: String,
    pub session_id: String,
    pub section: NoteSection,
    pub note_id: Option<String>,
    pub text: String,
    pub revision: String,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenNote {
    pub request_id: String,
    pub session_id: String,
    pub section: NoteSection,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskOpRequest {
    pub request_id: String,
    pub cwd: String,
    pub op: TaskOp,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendOpenItems {
    pub request_id: String,
    pub session_id: String,
    pub section: NoteSection,
    pub revision: String,
    pub note_id: Option<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromoteNoteItem {
    pub request_id: String,
    pub session_id: String,
    pub section: NoteSection,
    pub line: i64,
    pub revision: String,
    pub note_id: Option<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReturnTaskToNote {
    pub request_id: String,
    pub cwd: String,
    pub task_id: String,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskToNote {
    pub request_id: String,
    pub cwd: String,
    pub task_id: String,
    /// The work whose note gets the task: `wt:<id>` or `s:<id>`.
    pub section: String,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Create {
    pub request_id: String,
    pub input: CreateSessionInput,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReorderSession {
    pub request_id: String,
    pub session_id: String,
    pub direction: MoveDirection,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Restart {
    pub request_id: String,
    pub session_id: String,
    pub cols: i64,
    pub rows: i64,
    pub mode: Option<RestartMode>,
    pub project_fingerprint: Option<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Kill {
    pub request_id: String,
    pub session_id: String,
    pub delete_worktree: Option<bool>,
    pub delete_branch: Option<bool>,
    pub force: Option<bool>,
    pub allow_data_loss: Option<bool>,
    pub send_open_items: Option<bool>,
}

/// `merge-preview`, `mark-session-merged`.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTargetRequest {
    pub request_id: String,
    pub session_id: String,
    pub target_cwd: String,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeWorktree {
    pub request_id: String,
    pub session_id: String,
    pub mode: WorktreeMergeMode,
    pub target_cwd: String,
    pub commit_first: Option<bool>,
    pub send_open_items: Option<bool>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveMerge {
    pub request_id: String,
    pub session_id: String,
    pub target_cwd: String,
    pub action: ResolveMergeAction,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSessionDone {
    pub request_id: String,
    pub session_id: String,
    pub done: bool,
}

#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Remove {
    pub request_id: String,
    pub session_id: String,
    pub move_open_items: Option<bool>,
}

/// `attach`, `attach-terminal`, `attach-git`, `attach-dev`, `attach-action` (attach.ts literal order).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attach {
    pub request_id: String,
    pub session_id: String,
    pub cols: Option<i64>,
    pub rows: Option<i64>,
}

/// Keys typed into an attached PTY (no response).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    pub session_id: String,
    pub data: String,
}

/// An attached terminal's new size (no response).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resize {
    pub session_id: String,
    pub cols: i64,
    pub rows: i64,
}

/// Ends an attach (answered by a `*detached` event, not a response).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Detach {
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
            cols: 80,
            rows: 24,
            mode: Some(RestartMode::Fresh),
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
            raw: "{}\n".into(),
            revision: None,
        });
        let text =
            r#"{"type":"save-config","requestId":"r","target":"global","cwd":"/x","raw":"{}\n","revision":null}"#;
        assert_eq!(serde_json::to_string(&request).unwrap(), text);
        assert_eq!(serde_json::from_str::<Request>(text).unwrap(), request);
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
