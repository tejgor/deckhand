//! The daemon: `Daemon(Rc<Inner>)` and its shared state.
//!
//! TS InkDaemon is one class whose fields every method reads and writes. The port keeps one [`Daemon`] (a cheap
//! `Rc` handle, cloned into every task it starts) whose [`Inner`] holds one `RefCell` per subsystem; each
//! subsystem's state struct and its methods (`impl Daemon` blocks) live in that subsystem's module, so a unit adds
//! fields and methods without touching this file. Runtime model (docs/conventions.md): one `current_thread`
//! runtime with a `LocalSet`; borrow a subsystem's state, copy what is needed, drop the borrow, then await.
//!
//! Where each daemon.ts function went:
//!
//! | daemon.ts | module | Rust | unit |
//! |---|---|---|---|
//! | `InkDaemon` | daemon.rs | `Daemon` | D1 |
//! | `DEFAULT_PREVIEW_COLS` | server.rs | `DEFAULT_PREVIEW_COLS` | D1 |
//! | `DEFAULT_PREVIEW_ROWS` | server.rs | `DEFAULT_PREVIEW_ROWS` | D1 |
//! | `MAX_REQUEST_LINE` | server.rs | `deckhand_protocol::ndjson::MAX_REQUEST_LINE` | D1 |
//! | `ClientSubscription` | server.rs | `ClientSubscription` | D1 |
//! | `sendMessage` | server.rs | `send_message` | D1 |
//! | `clampSize` | server.rs | `clamp_size` | D1 |
//! | `clampNonNegative` | server.rs | `clamp_non_negative` | D1 |
//! | `prepareSocket` | server.rs | `Daemon::prepare_socket` | D1 |
//! | `listen` | server.rs | `Daemon::listen` | D1 |
//! | `handleConnection` | server.rs | `Daemon::handle_connection` | D1 |
//! | `handleConnection.cleanup` | server.rs | `Daemon::client_disconnected` | D1 |
//! | `rejectOversizedRequest` | server.rs | `Daemon::reject_oversized_request` | D1 |
//! | `getClient` | server.rs | `Daemon::get_client` | D1 |
//! | `inClientScope` | server.rs | `Daemon::in_client_scope` | D1 |
//! | `learnRepoKey` | server.rs | `Daemon::learn_repo_key` | D1 |
//! | `broadcastSessionUpdated` | server.rs | `Daemon::broadcast_session_updated` | D1 |
//! | `broadcastSessionRemoved` | server.rs | `Daemon::broadcast_session_removed` | D1 |
//! | `handleRequest.ping` | server.rs | `Daemon::handle_ping` | D1 |
//! | `handleRequest.subscribe` | server.rs | `Daemon::handle_subscribe` | D1 |
//! | `response` | dispatch.rs | `response` | D1 |
//! | `failure` | dispatch.rs | `failure` | D1 |
//! | `handleRequest` | dispatch.rs | `Daemon::handle_request` | D1 |
//! | `WORKER_REQUEST_TIMEOUT_MS` | worker_channel.rs | `WORKER_REQUEST_TIMEOUT` | D1 |
//! | `WORKER_KILL_GRACE_MS` | worker_channel.rs | `WORKER_KILL_GRACE` | D1 |
//! | `WorkerChannel` | worker_channel.rs | `WorkerChannel` | D1 |
//! | `WorkerResponse` | worker_channel.rs | `deckhand_protocol::worker_ipc::WorkerResponse` | D1 |
//! | `wireLog` | worker_channel.rs | `wire_log` | D1 |
//! | `channelKind` | worker_channel.rs | `channel_kind` | D1 |
//! | `requestChannel` | worker_channel.rs | `WorkerChannel::request` | D1 |
//! | `sendChannelEvent` | worker_channel.rs | `WorkerChannel::send_event` | D1 |
//! | `settleResponse` | worker_channel.rs | `WorkerChannel::settle_response` | D1 |
//! | `rejectPending` | worker_channel.rs | `WorkerChannel::reject_pending` | D1 |
//! | `persistedSession` | lifecycle.rs | `persisted_session` | D2 |
//! | `isProcessAlive` | lifecycle.rs | `is_process_alive` | D2 |
//! | `start` | lifecycle.rs | `Daemon::start` | D2 |
//! | `log` | lifecycle.rs | `Daemon::log` | D2 |
//! | `assertNoLiveDaemonFromPidFile` | lifecycle.rs | `Daemon::assert_no_live_daemon_from_pid_file` | D2 |
//! | `writePidFile` | lifecycle.rs | `Daemon::write_pid_file` | D2 |
//! | `removePidFile` | lifecycle.rs | `Daemon::remove_pid_file` | D2 |
//! | `setupProcessHandlers` | lifecycle.rs | `Daemon::setup_process_handlers` | D2 |
//! | `cleanup` | lifecycle.rs | `Daemon::cleanup` | D2 |
//! | `persist` | lifecycle.rs | `Daemon::persist` | D2 |
//! | `handleRequest.shutdown` | lifecycle.rs | `Daemon::handle_shutdown` | D2 |
//! | `SessionMap` | sessions.rs | `Daemon::set_session` | D3 |
//! | `silentExitNote` | sessions.rs | `silent_exit_note` | D3 |
//! | `truncateSessionTitle` | sessions.rs | `truncate_session_title` | D3 |
//! | `inheritedChildTitle` | sessions.rs | `inherited_child_title` | D3 |
//! | `readAgentExit` | sessions.rs | `read_agent_exit` | D3 |
//! | `requireSession` | sessions.rs | `Daemon::require_session` | D3 |
//! | `saveSession` | sessions.rs | `Daemon::save_session` | D3 |
//! | `setSessionInWorkspace` | sessions.rs | `Daemon::set_session_in_workspace` | D3 |
//! | `patchSession` | sessions.rs | `Daemon::patch_session` | D3 |
//! | `sessionsForRepo` | sessions.rs | `Daemon::sessions_for_repo` | D3 |
//! | `handleWorkerSessionExit` | sessions.rs | `Daemon::handle_worker_session_exit` | D3 |
//! | `reorderSession` | sessions.rs | `Daemon::reorder_session` | D3 |
//! | `sameRepository` | sessions.rs | `Daemon::same_repository` | D3 |
//! | `createSession` | sessions.rs | `Daemon::create_session` | D3 |
//! | `finishCreateSession` | sessions.rs | `Daemon::finish_create_session` | D3 |
//! | `failStartingSession` | sessions.rs | `Daemon::fail_starting_session` | D3 |
//! | `restartSession` | sessions.rs | `Daemon::restart_session` | D3 |
//! | `killSession` | sessions.rs | `Daemon::kill_session` | D3 |
//! | `removeSession` | sessions.rs | `Daemon::remove_session` | D3 |
//! | `exportSessionHandoff` | sessions.rs | `Daemon::export_session_handoff` | D3 |
//! | `handleRequest.list` | sessions.rs | `Daemon::handle_list` | D3 |
//! | `handleRequest.set-session-done` | sessions.rs | `Daemon::handle_set_session_done` | D3 |
//! | `handleRequest.archive-session` | sessions.rs | `Daemon::handle_archive_session` | D3 |
//! | `handleRequest.export-handoff` | sessions.rs | `Daemon::handle_export_handoff` | D3 |
//! | `handleRequest.cancel-start` | sessions.rs | `Daemon::handle_cancel_start` | D3 |
//! | `handleRequest.remove` | sessions.rs | `Daemon::handle_remove` | D3 |
//! | `SETUP_OUTPUT_LIVE_LIMIT` | launch.rs | `SETUP_OUTPUT_LIVE_LIMIT` | D4 |
//! | `SETUP_OUTPUT_STORED_LIMIT` | launch.rs | `SETUP_OUTPUT_STORED_LIMIT` | D4 |
//! | `programCommandCache` | launch.rs | `LaunchState` | D4 |
//! | `WorkerRuntime` | launch.rs | `WorkerRuntime` | D4 |
//! | `WorkerEvent` | launch.rs | `deckhand_protocol::worker_ipc::WorkerMessage` | D4 |
//! | `resolveProgramCommand` | launch.rs | `Daemon::resolve_program_command` | D4 |
//! | `buildDeckhandAgentName` | launch.rs | `build_deckhand_agent_name` | D4 |
//! | `prepareAgentSessionRef` | launch.rs | `prepare_agent_session_ref` | D4 |
//! | `resolveShellCommand` | launch.rs | `resolve_shell_command` | D4 |
//! | `handoffPrompt` | launch.rs | `handoff_prompt` | D4 |
//! | `movedForkPrompt` | launch.rs | `moved_fork_prompt` | D4 |
//! | `buildPreviewRecord` | launch.rs | `build_preview_record` | D4 |
//! | `isCurrentLaunch` | launch.rs | `Daemon::is_current_launch` | D4 |
//! | `assertCurrentLaunch` | launch.rs | `Daemon::assert_current_launch` | D4 |
//! | `runSetup` | launch.rs | `Daemon::run_setup` | D4 |
//! | `startWorker` | launch.rs | `Daemon::start_worker` | D4 |
//! | `stopWorker` | launch.rs | `Daemon::stop_worker` | D4 |
//! | `sendWorkerRequest` | launch.rs | `Daemon::send_worker_request` | D4 |
//! | `sendWorkerEvent` | launch.rs | `Daemon::send_worker_event` | D4 |
//! | `handleWorkerMessage` | launch.rs | `Daemon::handle_worker_message` | D4 |
//! | `handleWorkerProcessExit` | launch.rs | `Daemon::handle_worker_process_exit` | D4 |
//! | `firstMessageArgs` | launch.rs | `Daemon::first_message_args` | D4 |
//! | `continuedConversation` | launch.rs | `Daemon::continued_conversation` | D4 |
//! | `getPreviewRecord` | launch.rs | `Daemon::get_preview_record` | D4 |
//! | `handleRequest.watch-preview` | launch.rs | `Daemon::handle_watch_preview` | D4 |
//! | `handleRequest.attach` | launch.rs | `Daemon::handle_attach` | D4 |
//! | `handleRequest.input` | launch.rs | `Daemon::handle_input` | D4 |
//! | `handleRequest.resize` | launch.rs | `Daemon::handle_resize` | D4 |
//! | `handleRequest.detach` | launch.rs | `Daemon::handle_detach` | D4 |
//! | `AGENT_VERSIONS_TICK_MS` | agents.rs | `AGENT_VERSIONS_TICK` | D5 |
//! | `publishAgentVersions` | agents.rs | `Daemon::publish_agent_versions` | D5 |
//! | `refreshAgentVersions` | agents.rs | `Daemon::refresh_agent_versions` | D5 |
//! | `updateAgent` | agents.rs | `Daemon::update_agent` | D5 |
//! | `handleRequest.agent-versions` | agents.rs | `Daemon::handle_agent_versions` | D5 |
//! | `DRAFT_WAIT_MS` | hooks.rs | `DRAFT_WAIT` | D5 |
//! | `DRAFT_SETTLE_MS` | hooks.rs | `DRAFT_SETTLE` | D5 |
//! | `AGENT_PROMPT_PATTERN` | hooks.rs | `AGENT_PROMPT_PATTERN` | D5 |
//! | `execFileAsync` | hooks.rs | `drop: promisify(execFile) for notify; deckhand_sys::proc::exec does that` | D5 |
//! | `handleAgentHook` | hooks.rs | `Daemon::handle_agent_hook` | D5 |
//! | `markAnswered` | hooks.rs | `Daemon::mark_answered` | D5 |
//! | `acceptedNativeRef` | hooks.rs | `Daemon::accepted_native_ref` | D5 |
//! | `notify` | hooks.rs | `Daemon::notify` | D5 |
//! | `setWorkerAgentStatus` | hooks.rs | `Daemon::set_worker_agent_status` | D5 |
//! | `typeDraft` | hooks.rs | `Daemon::type_draft` | D5 |
//! | `dropDraft` | hooks.rs | `Daemon::drop_draft` | D5 |
//! | `WORKSPACE_SHUTDOWN_TIMEOUT_MS` | workspaces.rs | `WORKSPACE_SHUTDOWN_TIMEOUT` | D6 |
//! | `WorkspacePane` | workspaces.rs | `WorkspacePane` | D6 |
//! | `WorkspacePaneRecord` | workspaces.rs | `WorkspacePaneRecord` | D6 |
//! | `PANE_LABELS` | workspaces.rs | `WorkspacePane::label` | D6 |
//! | `PANE_ATTACH_NAMES` | workspaces.rs | `WorkspacePane::attach_name` | D6 |
//! | `WorkspaceRuntime` | workspaces.rs | `WorkspaceRuntime` | D6 |
//! | `WorkspaceEvent` | workspaces.rs | `deckhand_protocol::worker_ipc::WorkerMessage` | D6 |
//! | `paneUpdatedMessage` | workspaces.rs | `pane_updated_message` | D6 |
//! | `updatedPaneRecord` | workspaces.rs | `updated_pane_record` | D6 |
//! | `startsOnView` | workspaces.rs | `starts_on_view` | D6 |
//! | `workspaceKeyOf` | workspaces.rs | `Daemon::workspace_key_of` | D6 |
//! | `requireWorkspace` | workspaces.rs | `Daemon::require_workspace` | D6 |
//! | `workspaceOfSession` | workspaces.rs | `Daemon::workspace_of_session` | D6 |
//! | `workspaceDevLive` | workspaces.rs | `Daemon::workspace_dev_live` | D6 |
//! | `assertWorkspaceAvailable` | workspaces.rs | `Daemon::assert_workspace_available` | D6 |
//! | `openWorkspace` | workspaces.rs | `Daemon::open_workspace` | D6 |
//! | `ensureWorkspace` | workspaces.rs | `Daemon::ensure_workspace` | D6 |
//! | `startWorkspaceWorker` | workspaces.rs | `Daemon::start_workspace_worker` | D6 |
//! | `requestPane` | workspaces.rs | `Daemon::request_pane` | D6 |
//! | `retireIfIdle` | workspaces.rs | `Daemon::retire_if_idle` | D6 |
//! | `handleWorkspaceMessage` | workspaces.rs | `Daemon::handle_workspace_message` | D6 |
//! | `handleWorkspaceExit` | workspaces.rs | `Daemon::handle_workspace_exit` | D6 |
//! | `retireWorkspace` | workspaces.rs | `Daemon::retire_workspace` | D6 |
//! | `detachWorkspaceClients` | workspaces.rs | `Daemon::detach_workspace_clients` | D6 |
//! | `watchWorkspacePane` | workspaces.rs | `Daemon::watch_workspace_pane` | D6 |
//! | `attachWorkspacePane` | workspaces.rs | `Daemon::attach_workspace_pane` | D6 |
//! | `startWorkspaceDev` | workspaces.rs | `Daemon::start_workspace_dev` | D6 |
//! | `startWorkspaceAction` | workspaces.rs | `Daemon::start_workspace_action` | D6 |
//! | `stopWorkspacePane` | workspaces.rs | `Daemon::stop_workspace_pane` | D6 |
//! | `publishWorkspacePane` | workspaces.rs | `Daemon::publish_workspace_pane` | D6 |
//! | `syncDevRunning` | workspaces.rs | `Daemon::sync_dev_running` | D6 |
//! | `resolveSessionDevCommand` | workspaces.rs | `Daemon::resolve_session_dev_command` | D6 |
//! | `handleRequest.run-action` | workspaces.rs | `Daemon::handle_run_action` | D6 |
//! | `handleRequest.watch-pane` | workspaces.rs | `Daemon::handle_watch_pane` | D6 |
//! | `handleRequest.start-dev` | workspaces.rs | `Daemon::handle_start_dev` | D6 |
//! | `handleRequest.stop-pane` | workspaces.rs | `Daemon::handle_stop_pane` | D6 |
//! | `handleRequest.pane-input` | workspaces.rs | `Daemon::handle_pane_input` | D6 |
//! | `handleRequest.pane-resize` | workspaces.rs | `Daemon::handle_pane_resize` | D6 |
//! | `handleRequest.pane-detach` | workspaces.rs | `Daemon::handle_pane_detach` | D6 |
//! | `SUMMARY_TTL_MS` | changes.rs | `SUMMARY_TTL` | D7 |
//! | `CHANGES_POLL_MS` | changes.rs | `CHANGES_POLL` | D7 |
//! | `CHANGES_FRESH_MS` | changes.rs | `CHANGES_FRESH` | D7 |
//! | `ChangesWatch` | changes.rs | `ChangesWatch` | D7 |
//! | `changesWatch` | changes.rs | `Daemon::changes_watch` | D7 |
//! | `changesExclusive` | changes.rs | `Daemon::changes_exclusive` | D7 |
//! | `refreshChanges` | changes.rs | `Daemon::refresh_changes` | D7 |
//! | `publishChanges` | changes.rs | `Daemon::publish_changes` | D7 |
//! | `watchChanges` | changes.rs | `Daemon::watch_changes` | D7 |
//! | `syncChangeWatches` | changes.rs | `Daemon::sync_change_watches` | D7 |
//! | `stopChangesWatch` | changes.rs | `Daemon::stop_changes_watch` | D7 |
//! | `handleRequest.watch-changes` | changes.rs | `Daemon::handle_watch_changes` | D7 |
//! | `handleRequest.changes-diff` | changes.rs | `Daemon::handle_changes_diff` | D7 |
//! | `handleRequest.change-stage` | changes.rs | `Daemon::handle_change_stage` | D7 |
//! | `handleRequest.workspace-summary` | changes.rs | `Daemon::handle_workspace_summary` | D7 |
//! | `handleRequest.create-pr` | changes.rs | `Daemon::handle_create_pr` | D7 |
//! | `projectNotes` | notes_tasks.rs | `project_notes` | D8 |
//! | `noteOf` | notes_tasks.rs | `Daemon::note_of` | D8 |
//! | `mergeWorktreeSessionNotes` | notes_tasks.rs | `Daemon::merge_worktree_session_notes` | D8 |
//! | `saveNote` | notes_tasks.rs | `Daemon::save_note` | D8 |
//! | `tasksKeyFor` | notes_tasks.rs | `Daemon::tasks_key_for` | D8 |
//! | `linkedTasksOf` | notes_tasks.rs | `Daemon::linked_tasks_of` | D8 |
//! | `checkTaskLink` | notes_tasks.rs | `Daemon::check_task_link` | D8 |
//! | `tasksDoc` | notes_tasks.rs | `Daemon::tasks_doc` | D8 |
//! | `broadcastTasks` | notes_tasks.rs | `Daemon::broadcast_tasks` | D8 |
//! | `changeTasks` | notes_tasks.rs | `Daemon::change_tasks` | D8 |
//! | `changeLinkedTasks` | notes_tasks.rs | `Daemon::change_linked_tasks` | D8 |
//! | `syncWorktreeTasks` | notes_tasks.rs | `Daemon::sync_worktree_tasks` | D8 |
//! | `syncDoneTasks` | notes_tasks.rs | `Daemon::sync_done_tasks` | D8 |
//! | `sendOpenNoteItems` | notes_tasks.rs | `Daemon::send_open_note_items` | D8 |
//! | `sendWorktreeItemsToBacklog` | notes_tasks.rs | `Daemon::send_worktree_items_to_backlog` | D8 |
//! | `sendNoteItems` | notes_tasks.rs | `Daemon::send_note_items` | D8 |
//! | `promoteNoteItem` | notes_tasks.rs | `Daemon::promote_note_item` | D8 |
//! | `taskToWorkNote` | notes_tasks.rs | `Daemon::task_to_work_note` | D8 |
//! | `migrateFollowUpTasks` | notes_tasks.rs | `Daemon::migrate_follow_up_tasks` | D8 |
//! | `returnTaskToNote` | notes_tasks.rs | `Daemon::return_task_to_note` | D8 |
//! | `taskOrigin` | notes_tasks.rs | `Daemon::task_origin` | D8 |
//! | `moveOpenNoteItems` | notes_tasks.rs | `Daemon::move_open_note_items` | D8 |
//! | `notesChanged` | notes_tasks.rs | `Daemon::notes_changed` | D8 |
//! | `handleRequest.open-note` | notes_tasks.rs | `Daemon::handle_open_note` | D8 |
//! | `handleRequest.watch-tasks` | notes_tasks.rs | `Daemon::handle_watch_tasks` | D8 |
//! | `handleRequest.task-op` | notes_tasks.rs | `Daemon::handle_task_op` | D8 |
//! | `handleRequest.open-tasks` | notes_tasks.rs | `Daemon::handle_open_tasks` | D8 |
//! | `MERGE_DETECT_MS` | merge.rs | `MERGE_DETECT_INTERVAL` | D9 |
//! | `MERGE_DETECT_MAX` | merge.rs | `MERGE_DETECT_MAX` | D9 |
//! | `PendingMerge` | merge.rs | `PendingMerge` | D9 |
//! | `defaultBranchRefs` | merge.rs | `default_branch_refs` | D9 |
//! | `mergeSource` | merge.rs | `Daemon::merge_source` | D9 |
//! | `mergePreview` | merge.rs | `Daemon::merge_preview` | D9 |
//! | `mergeSessionWorktree` | merge.rs | `Daemon::merge_session_worktree` | D9 |
//! | `resolveMerge` | merge.rs | `Daemon::resolve_merge` | D9 |
//! | `saveMergeMarkers` | merge.rs | `Daemon::save_merge_markers` | D9 |
//! | `markSessionMerged` | merge.rs | `Daemon::mark_session_merged` | D9 |
//! | `detectMerges` | merge.rs | `Daemon::detect_merges` | D9 |
//! | `runMergeDetection` | merge.rs | `Daemon::run_merge_detection` | D9 |
//! | `detectMergeFromSummary` | merge.rs | `Daemon::detect_merge_from_summary` | D9 |
//! | `markDetectedMerge` | merge.rs | `Daemon::mark_detected_merge` | D9 |
//! | `WORKTREE_OVERVIEW_MAX` | worktrees.rs | `WORKTREE_OVERVIEW_MAX` | D10 |
//! | `WORKTREE_OVERVIEW_CONCURRENCY` | worktrees.rs | `WORKTREE_OVERVIEW_CONCURRENCY` | D10 |
//! | `STOP_FOR_DELETE_MS` | worktrees.rs | `STOP_FOR_DELETE` | D10 |
//! | `WorktreeTarget` | worktrees.rs | `WorktreeTarget` | D10 |
//! | `forEachLimited` | worktrees.rs | `for_each_limited` | D10 |
//! | `realpathOrResolve` | worktrees.rs | `realpath_or_resolve` | D10 |
//! | `isLinkedWorktreeRoot` | worktrees.rs | `is_linked_worktree_root` | D10 |
//! | `worktreeRecordOf` | worktrees.rs | `Daemon::worktree_record_of` | D10 |
//! | `saveWorktreeRecord` | worktrees.rs | `Daemon::save_worktree_record` | D10 |
//! | `joinWorktree` | worktrees.rs | `Daemon::join_worktree` | D10 |
//! | `cleanupBlockers` | worktrees.rs | `Daemon::cleanup_blockers` | D10 |
//! | `inspectSessionCleanup` | worktrees.rs | `Daemon::inspect_session_cleanup` | D10 |
//! | `assertCleanupAllowed` | worktrees.rs | `Daemon::assert_cleanup_allowed` | D10 |
//! | `updateWorktreeRecord` | worktrees.rs | `Daemon::update_worktree_record` | D10 |
//! | `repositorySessions` | worktrees.rs | `Daemon::repository_sessions` | D10 |
//! | `sessionsInWorktree` | worktrees.rs | `Daemon::sessions_in_worktree` | D10 |
//! | `worktreeOverview` | worktrees.rs | `Daemon::worktree_overview` | D10 |
//! | `worktreeTarget` | worktrees.rs | `Daemon::worktree_target` | D10 |
//! | `worktreeBlockers` | worktrees.rs | `Daemon::worktree_blockers` | D10 |
//! | `inspectWorktree` | worktrees.rs | `Daemon::inspect_worktree` | D10 |
//! | `deleteWorktree` | worktrees.rs | `Daemon::delete_worktree` | D10 |
//! | `stopForDeletion` | worktrees.rs | `Daemon::stop_for_deletion` | D10 |
//! | `handleRequest.list-worktrees` | worktrees.rs | `Daemon::handle_list_worktrees` | D10 |
//! | `projectInfo` | config.rs | `project_info` | D11 |
//! | `listBranches` | config.rs | `Daemon::list_branches` | D11 |
//! | `handleRequest.save-config` | config.rs | `Daemon::handle_save_config` | D11 |
//! | `handleRequest.settings-info` | config.rs | `Daemon::handle_settings_info` | D11 |
//! | `handleRequest.worktree-candidates` | config.rs | `Daemon::handle_worktree_candidates` | D11 |
//! | `handleRequest.global-link-candidates` | config.rs | `Daemon::handle_global_link_candidates` | D11 |
//! | `handleRequest.worktree-candidate-sizes` | config.rs | `Daemon::handle_worktree_candidate_sizes` | D11 |
//! | `handleRequest.project-info` | config.rs | `Daemon::handle_project_info` | D11 |
//! | `handleRequest.trust-project` | config.rs | `Daemon::handle_trust_project` | D11 |

use std::cell::RefCell;
use std::ops::Deref;
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use deckhand_core::error::Result;
use deckhand_sys::agent_version_check::AgentVersionChecker;
use deckhand_sys::notes_store::NotesStore;
use deckhand_sys::paths;

use crate::agents::AgentsState;
use crate::changes::ChangesState;
use crate::config::ConfigState;
use crate::hooks::HooksState;
use crate::launch::LaunchState;
use crate::lifecycle::LifecycleState;
use crate::merge::MergeState;
use crate::notes_tasks::NotesTasksState;
use crate::server::ServerState;
use crate::sessions::SessionsState;
use crate::workspaces::WorkspacesState;
use crate::worktrees::WorktreesState;

/// How the daemon is run (TS InkDaemon takes none: everything comes from paths.ts and the environment).
#[derive(Clone, Debug, Default)]
pub struct DaemonOptions {
    /// The binary started as session and workspace workers; `None`: the running executable (TS getCliEntryPath).
    pub worker_exe: Option<PathBuf>,
}

/// The files the daemon owns, resolved once at construction (TS calls paths.ts each time; DECKHAND_HOME does not
/// change while the daemon runs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DaemonPaths {
    /// paths.ts getConfigDir.
    pub config_dir: PathBuf,
    /// paths.ts getSocketPath.
    pub socket: PathBuf,
    /// paths.ts getDaemonPidPath.
    pub pid_file: PathBuf,
    /// paths.ts getDaemonLogPath.
    pub log_file: PathBuf,
    /// paths.ts getWorkerDir.
    pub worker_dir: PathBuf,
}

impl DaemonPaths {
    /// The paths for the current DECKHAND_HOME (deckhand_sys::paths).
    pub fn current() -> Self {
        DaemonPaths {
            config_dir: paths::config_dir(),
            socket: paths::socket_path(),
            pid_file: paths::daemon_pid_path(),
            log_file: paths::daemon_log_path(),
            worker_dir: paths::worker_dir(),
        }
    }
}

/// TS: the InkDaemon instance's fields, grouped by subsystem (see each module for its state).
pub struct Inner {
    pub options: DaemonOptions,
    pub paths: DaemonPaths,
    /// Clients, subscriptions, the listener, repository keys.
    pub server: RefCell<ServerState>,
    /// Shutting down, the coalesced persist.
    pub lifecycle: RefCell<LifecycleState>,
    /// The sessions (TS SessionMap) and those still being prepared.
    pub sessions: RefCell<SessionsState>,
    /// Session workers, setup processes, resolved agent commands.
    pub launch: RefCell<LaunchState>,
    /// Agent versions and updates.
    pub agents: RefCell<AgentsState>,
    /// Agent hook signals.
    pub hooks: RefCell<HooksState>,
    /// Workspace workers and their panes.
    pub workspaces: RefCell<WorkspacesState>,
    /// The Git tab's Changes watches and workspace summaries.
    pub changes: RefCell<ChangesState>,
    /// The notes store and task list keys.
    pub notes_tasks: RefCell<NotesTasksState>,
    /// Pending conflicted merges and merge detection.
    pub merge: RefCell<MergeState>,
    /// Worktree records and worktrees being deleted.
    pub worktrees: RefCell<WorktreesState>,
    /// Configuration (no state of its own yet).
    pub config: RefCell<ConfigState>,
}

/// TS: daemon.ts InkDaemon. Clones share the daemon.
#[derive(Clone)]
pub struct Daemon(Rc<Inner>);

/// A daemon reference that does not keep it alive (timers and store callbacks hold this).
#[derive(Clone)]
pub struct WeakDaemon(Weak<Inner>);

impl Daemon {
    /// TS: `new InkDaemon()`: the empty state (the notes store wired to notesChanged, the agent version checker),
    /// nothing read or started yet.
    pub fn new(options: DaemonOptions) -> Self {
        Daemon::with_paths(options, DaemonPaths::current())
    }

    /// [`Daemon::new`] with its files somewhere else (tests).
    pub(crate) fn with_paths(options: DaemonOptions, paths: DaemonPaths) -> Self {
        Daemon(Rc::new_cyclic(|weak: &Weak<Inner>| {
            let daemon = WeakDaemon(weak.clone());
            // Edits made outside Deckhand are re-read by the store and broadcast by notesChanged.
            let notes = NotesStore::new(move |kind, stem| {
                if let Some(daemon) = daemon.upgrade() {
                    daemon.notes_changed(kind, stem);
                }
            });
            Inner {
                options,
                paths,
                server: RefCell::new(ServerState::default()),
                lifecycle: RefCell::new(LifecycleState::default()),
                sessions: RefCell::new(SessionsState::default()),
                launch: RefCell::new(LaunchState::default()),
                agents: RefCell::new(AgentsState {
                    checker: Some(AgentVersionChecker::new(None)),
                    ..AgentsState::default()
                }),
                hooks: RefCell::new(HooksState::default()),
                workspaces: RefCell::new(WorkspacesState::default()),
                changes: RefCell::new(ChangesState::default()),
                notes_tasks: RefCell::new(NotesTasksState {
                    notes: Some(notes),
                    ..NotesTasksState::default()
                }),
                merge: RefCell::new(MergeState::default()),
                worktrees: RefCell::new(WorktreesState::default()),
                config: RefCell::new(ConfigState::default()),
            }
        }))
    }

    /// A weak reference to this daemon.
    pub fn downgrade(&self) -> WeakDaemon {
        WeakDaemon(Rc::downgrade(&self.0))
    }
}

impl WeakDaemon {
    /// The daemon, while it is alive.
    pub fn upgrade(&self) -> Option<Daemon> {
        self.0.upgrade().map(Daemon)
    }
}

impl Deref for Daemon {
    type Target = Inner;

    fn deref(&self) -> &Inner {
        &self.0
    }
}

/// TS: cli.ts `--daemon` running `new InkDaemon().start()` and staying alive: builds the daemon, starts it and runs
/// until it shuts down (signals, `shutdown`, or a fatal error; see lifecycle.rs).
pub async fn run_daemon(options: DaemonOptions) -> Result<()> {
    let daemon = Daemon::new(options);
    daemon.start().await?;
    // The daemon's tasks run on the LocalSet; the process ends by exiting from a shutdown path (lifecycle.rs), as the
    // TS process.exit does.
    std::future::pending::<()>().await;
    drop(daemon);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_follow_paths_rs() {
        let current = DaemonPaths::current();
        assert_eq!(current.config_dir, paths::config_dir());
        assert_eq!(current.socket, paths::socket_path());
        assert_eq!(current.pid_file, paths::daemon_pid_path());
        assert_eq!(current.log_file, paths::daemon_log_path());
        assert_eq!(current.worker_dir, paths::worker_dir());
    }

    #[test]
    fn a_new_daemon_is_empty_and_weak_references_do_not_keep_it() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DaemonPaths {
            config_dir: dir.path().to_path_buf(),
            socket: dir.path().join("daemon.sock"),
            pid_file: dir.path().join("daemon.pid"),
            log_file: dir.path().join("daemon.log"),
            worker_dir: dir.path().join("workers"),
        };
        let daemon = Daemon::with_paths(DaemonOptions::default(), paths.clone());
        assert_eq!(daemon.paths, paths);
        assert!(daemon.options.worker_exe.is_none());
        assert!(daemon.server.borrow().clients.is_empty());
        assert!(daemon.sessions.borrow().sessions.is_empty());
        assert!(daemon.launch.borrow().workers.is_empty());
        assert!(daemon.notes_tasks.borrow().notes.is_some());
        assert!(daemon.agents.borrow().checker.is_some());
        assert!(!daemon.lifecycle.borrow().shutting_down);
        let weak = daemon.downgrade();
        assert!(weak.upgrade().is_some());
        drop(daemon);
        assert!(weak.upgrade().is_none());
    }
}
