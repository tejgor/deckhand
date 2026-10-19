//! Port of daemon.ts (session create/restart/kill/remove/order).
//!
//! Sessions as the daemon holds and sends them (TS SessionMap): setting one projects its worktree record's markers
//! (worktreeRecords.ts) and its notes (notesStore.ts) into it, so every reader sees them; persist() strips both. A
//! record's key order is JS insertion order ([`Ordered`]): a spread that adds a key appends it.

use std::collections::HashSet;

use deckhand_core::error::{DhError, Result};
use deckhand_protocol::Ordered;
use deckhand_protocol::data::{
    AgentSessionRef, CreateSessionInput, MoveDirection, OkResult, RestartMode, SessionRecord,
};
use deckhand_protocol::wire::{ArchiveSession, ExportHandoff, Remove, SessionRequest, SetSessionDone};
use indexmap::IndexMap;

use crate::daemon::Daemon;

/// The sessions' part of the daemon state.
#[derive(Default)]
pub struct SessionsState {
    /// TS `sessions` (SessionMap), in insertion order (JS Map order: a re-set keeps its place).
    pub sessions: IndexMap<String, Ordered<SessionRecord>>,
    /// TS `preparingSessions`: sessions whose worktree is still being prepared (they have no workspace yet).
    pub preparing: HashSet<String>,
}

/// TS: daemon.ts readAgentExit result: what an exited agent's last screen says about its conversation.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentExitReading {
    /// The conversation to keep (TS `ref`).
    pub agent_session_ref: Option<AgentSessionRef>,
    /// The launch failed (no conversation to resume, or a fork that found nothing to fork).
    pub failed: bool,
    /// Appended to the preview: what s/S will do.
    pub note: Option<String>,
}

/// TS: InkDaemon.killSession's flags (`kill` request fields, defaults false).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KillOptions {
    pub delete_worktree: bool,
    pub delete_branch: bool,
    pub force: bool,
    pub allow_data_loss: bool,
    /// Send the worktree note's open items to the backlog before deleting it.
    pub send_open_items: bool,
}

/// TS: daemon.ts silentExitNote: what an exited session shows when its agent printed nothing (it failed to start).
pub fn silent_exit_note(session: &SessionRecord, exit_code: Option<i64>, exit_signal: Option<i64>) -> String {
    let _ = (session, exit_code, exit_signal);
    todo!("D3")
}

/// TS: daemon.ts truncateSessionTitle (UTF-16 lengths; `…` when cut).
pub fn truncate_session_title(value: &str, max_length: usize) -> String {
    let _ = (value, max_length);
    todo!("D3")
}

/// TS: daemon.ts inheritedChildTitle: `<parent> / <child>` within 64 characters.
pub fn inherited_child_title(parent_title: &str, child_title: &str) -> String {
    let _ = (parent_title, child_title);
    todo!("D3")
}

/// TS: daemon.ts readAgentExit.
pub fn read_agent_exit(session: &SessionRecord, output: &str) -> AgentExitReading {
    let _ = (session, output);
    todo!("D3")
}

impl Daemon {
    /// TS: SessionMap.set: stores the session with its worktree markers and notes projected into it.
    pub fn set_session(&self, session: Ordered<SessionRecord>) {
        let _ = session;
        todo!("D3")
    }

    /// TS: `this.sessions.get(id)`.
    pub fn get_session(&self, session_id: &str) -> Option<Ordered<SessionRecord>> {
        let _ = session_id;
        todo!("D3")
    }

    /// TS: `[...this.sessions.values()]`.
    pub fn all_sessions(&self) -> Vec<Ordered<SessionRecord>> {
        todo!("D3")
    }

    /// TS: InkDaemon.requireSession (`Session does not exist`).
    pub fn require_session(&self, session_id: &str) -> Result<Ordered<SessionRecord>> {
        let _ = session_id;
        todo!("D3")
    }

    /// TS: InkDaemon.saveSession: sets it (devRunning in line), persists, broadcasts, re-syncs the Changes watches and
    /// answers the session as held now.
    pub async fn save_session(&self, session: Ordered<SessionRecord>) -> Result<Ordered<SessionRecord>> {
        let _ = session;
        todo!("D3")
    }

    /// TS: InkDaemon.setSessionInWorkspace: sets it with devRunning mirroring its workspace's Dev.
    pub fn set_session_in_workspace(&self, session: Ordered<SessionRecord>) {
        let _ = session;
        todo!("D3")
    }

    /// TS: InkDaemon.patchSession: applies `patch` to the latest record (TS `{...current, ...patch}`: keys the patch
    /// adds must be [`Ordered::touch`]ed) and answers it as now held; `None` when the session is gone.
    pub fn patch_session(
        &self,
        session_id: &str,
        patch: impl FnOnce(&mut Ordered<SessionRecord>),
    ) -> Option<Ordered<SessionRecord>> {
        let _ = (session_id, patch);
        todo!("D3")
    }

    /// TS: InkDaemon.sessionsForRepo (in sidebar order).
    pub fn sessions_for_repo(&self, repo_root: &str) -> Vec<Ordered<SessionRecord>> {
        let _ = repo_root;
        todo!("D3")
    }

    /// TS: InkDaemon.handleWorkerSessionExit: the exit record (reason, last screen or silentExitNote), then the
    /// worktree deletion a kill asked for, then save, notify and push the final preview.
    pub async fn handle_worker_session_exit(
        &self,
        session_id: &str,
        exit_code: Option<i64>,
        exit_signal: Option<i64>,
        last_preview: &str,
    ) -> Result<()> {
        let _ = (session_id, exit_code, exit_signal, last_preview);
        todo!("D3")
    }

    /// TS: InkDaemon.reorderSession (`session does not exist`): swaps it with its neighbour among its siblings.
    pub async fn reorder_session(
        &self,
        session_id: &str,
        direction: MoveDirection,
    ) -> Result<Vec<Ordered<SessionRecord>>> {
        let _ = (session_id, direction);
        todo!("D3")
    }

    /// TS: InkDaemon.sameRepository (the same path, or the same Git common dir).
    pub async fn same_repository(&self, left: &str, right: &str) -> Result<bool> {
        let _ = (left, right);
        todo!("D3")
    }

    /// TS: InkDaemon.createSession: validates (title, parent, fork, task, base branch, handoff), records the session
    /// as starting and finishes it in the background (finishCreateSession, failStartingSession on error).
    pub async fn create_session(&self, input: CreateSessionInput) -> Result<Ordered<SessionRecord>> {
        let _ = input;
        todo!("D3")
    }

    /// TS: InkDaemon.finishCreateSession: worktree (new or existing), launch plan, worktree incarnation, task link,
    /// setup, then the worker.
    pub async fn finish_create_session(&self, session_id: &str, input: CreateSessionInput) -> Result<()> {
        let _ = (session_id, input);
        todo!("D3")
    }

    /// TS: InkDaemon.failStartingSession: a session still starting (with this launch) exits as failed
    /// (`Failed to start session: <message>`).
    pub async fn fail_starting_session(
        &self,
        session_id: &str,
        error: &DhError,
        launch_id: Option<&str>,
    ) -> Result<()> {
        let _ = (session_id, error, launch_id);
        todo!("D3")
    }

    /// TS: InkDaemon.restartSession: `s` resumes (or re-forks), `S` starts fresh; retries owed setup first.
    pub async fn restart_session(
        &self,
        session_id: &str,
        cols: i64,
        rows: i64,
        mode: RestartMode,
        project_fingerprint: Option<String>,
    ) -> Result<Ordered<SessionRecord>> {
        let _ = (session_id, cols, rows, mode, project_fingerprint);
        todo!("D3")
    }

    /// TS: InkDaemon.killSession: checks the cleanup a delete asks for, marks the worker's exit plan and asks it to
    /// kill the agent (`session is not running`).
    pub async fn kill_session(&self, session_id: &str, options: KillOptions) -> Result<()> {
        let _ = (session_id, options);
        todo!("D3")
    }

    /// TS: InkDaemon.removeSession (`session does not exist`, `kill the session before removing it`): forgets it, its
    /// note, its worktree record when it was the last, abandons its tasks and retires an orphaned workspace.
    pub async fn remove_session(&self, session_id: &str) -> Result<()> {
        let _ = session_id;
        todo!("D3")
    }

    /// TS: InkDaemon.exportSessionHandoff: Git context (bounded, fail-soft; none for a deleted worktree) and the
    /// linked task, written by sessionFeatures.ts exportHandoff; answers the file path.
    pub async fn export_session_handoff(&self, session: &SessionRecord, include_output: bool) -> Result<String> {
        let _ = (session, include_output);
        todo!("D3")
    }

    /// TS: handleRequest `list`: every session in sidebar order.
    pub fn handle_list(&self) -> Result<Vec<Ordered<SessionRecord>>> {
        todo!("D3")
    }

    /// TS: handleRequest `set-session-done`: sets or clears `doneAt`, then ticks or reopens the linked tasks.
    pub async fn handle_set_session_done(&self, request: &SetSessionDone) -> Result<Ordered<SessionRecord>> {
        let _ = request;
        todo!("D3")
    }

    /// TS: handleRequest `archive-session`.
    pub async fn handle_archive_session(&self, request: &ArchiveSession) -> Result<Ordered<SessionRecord>> {
        let _ = request;
        todo!("D3")
    }

    /// TS: handleRequest `export-handoff`: exports and records `handoffPath`; answers the path.
    pub async fn handle_export_handoff(&self, request: &ExportHandoff) -> Result<String> {
        let _ = request;
        todo!("D3")
    }

    /// TS: handleRequest `cancel-start` (`Session is not starting`): stops setup and the worker, records the session
    /// as stopped (`Setup cancelled; worktree retained.` or the startup-cancelled note).
    pub async fn handle_cancel_start(&self, request: &SessionRequest) -> Result<OkResult> {
        let _ = request;
        todo!("D3")
    }

    /// TS: handleRequest `remove`: moves the open note items first when asked, then removeSession.
    pub async fn handle_remove(&self, request: &Remove) -> Result<OkResult> {
        let _ = request;
        todo!("D3")
    }
}
