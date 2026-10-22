//! Sessions: create, restart, kill, remove, order.
//!
//! Sessions as the daemon holds and sends them (TS SessionMap): setting one projects its worktree record's markers
//! (worktreeRecords.ts) and its notes (notesStore.ts) into it, so every reader sees them; persist() strips both. A
//! record's key order is JS insertion order ([`Ordered`]): a spread that adds a key appends it.

use std::collections::HashSet;
use std::rc::Rc;

use deckhand_core::agent_signals::hooks_enabled;
use deckhand_core::agents::{LaunchPlan, agent_spec, launch_args, new_agent_ref, relaunch_plan};
use deckhand_core::error::{DhError, Result};
use deckhand_core::jsstr::{self, date, path};
use deckhand_core::project_config::{resolve_settings, resolve_setup_command};
use deckhand_core::session_features::HandoffTask;
use deckhand_core::session_order::{compare_session_order, sort_sessions_for_sidebar};
use deckhand_core::session_scope::session_matches_scope;
use deckhand_core::tasks::{parse_tasks, task_prompt};
use deckhand_core::workspace::workspace_key;
use deckhand_core::worktree_records::{own_worktree_path, project_worktree};
use deckhand_protocol::data::{
    AgentActivityStatus, AgentSessionRef, CreateSessionInput, ExitReason, JsNumber, LoadedProject, MoveDirection,
    OkResult, ProgramKey, RestartMode, SessionRecord, SessionSetup, SessionStatus, SessionWorktreeMode,
    SessionWorktreeRecord, SetupState, SubSessionKind, TaskLink, TaskOp, WorktreeCreator, WorktreeMode, WorktreeOrigin,
};
use deckhand_protocol::events::{PreviewUpdated, ServerMessage};
use deckhand_protocol::wire::{
    ArchiveSession, ExportHandoff, Remove, SessionRequest, SetSessionDone, js_string, js_truthy, node_received,
};
use deckhand_protocol::{KeyOrder, Ordered};
use deckhand_sys::agent_path::integration_args;
use deckhand_sys::async_util::{Timer, spawn};
use deckhand_sys::git::{
    branch_creation_commit, create_worktree_for_session, current_branch, delete_local_branch, find_git_common_dir,
    find_repo_root, head_sha, list_worktrees, remove_worktree,
};
use deckhand_sys::notes_store::{NoteId, NoteKind};
use deckhand_sys::session_features::export_handoff;
use deckhand_sys::storage::load_app_config;
use deckhand_sys::workspace_git::get_handoff_git_context;
use deckhand_sys::{config::load_project_config, proc};
use indexmap::IndexMap;
use nix::sys::signal::Signal;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::daemon::Daemon;
use crate::hooks::DRAFT_WAIT;
use crate::launch::{
    Draft, SETUP_OUTPUT_STORED_LIMIT, WorkerRuntime, build_deckhand_agent_name, build_preview_record,
    prepare_agent_session_ref,
};
use crate::server::send_message;
use crate::server::{DEFAULT_PREVIEW_COLS, DEFAULT_PREVIEW_ROWS};
use crate::worktrees::is_linked_worktree_root;

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

/// A JS-truthy optional string (set and not empty).
fn truthy(value: Option<&str>) -> bool {
    value.is_some_and(|text| !text.is_empty())
}

/// `new Date().toISOString()`.
fn now_iso() -> String {
    date::to_iso_string(date::now_ms()).unwrap_or_default()
}

/// `Number.prototype.toString(36)` of a non-negative integer (`Date.now().toString(36)`).
fn base36(value: i64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let negative = value < 0;
    let mut rest = value.unsigned_abs();
    let mut out = Vec::new();
    loop {
        let digit = usize::try_from(rest % 36).unwrap_or(0);
        out.push(DIGITS.get(digit).copied().unwrap_or(b'0'));
        rest /= 36;
        if rest == 0 {
            break;
        }
    }
    if negative {
        out.push(b'-');
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// A JSON object (anything else: empty).
fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// `{type, ...}` payloads for a worker.
fn payload(value: Value) -> Map<String, Value> {
    object(value)
}

/// A literal's key order (`{a, b, c}`): every key it names, also those holding `undefined`, keeps its place.
fn literal_order(keys: &[&str]) -> KeyOrder {
    KeyOrder::Object(
        keys.iter()
            .map(|key| ((*key).to_owned(), KeyOrder::Unrecorded))
            .collect(),
    )
}

/// `{...record, [key]: value}`: the key is recorded as set now (a new key goes last, a known one keeps its place) and
/// its value is a new object, so its own key order is the value's.
fn assign(session: &mut Ordered<SessionRecord>, key: &str) {
    session.touch(key);
    let mut order = session.key_order().clone();
    if let KeyOrder::Object(keys) = &mut order
        && let Some((_, child)) = keys.iter_mut().find(|(name, _)| name == key)
    {
        *child = KeyOrder::Unrecorded;
    }
    session.set_key_order(order);
}

/// A record's own keys in JS order with the values they hold: its recorded keys (also those holding `undefined`,
/// which serialisation drops), then any present keys it has no record of.
fn own_keys(record: &Ordered<SessionRecord>) -> Result<(Map<String, Value>, Vec<String>)> {
    let values = object(record.to_value()?);
    let mut keys: Vec<String> = match record.key_order() {
        KeyOrder::Object(recorded) => recorded.iter().map(|(key, _)| key.clone()).collect(),
        _ => Vec::new(),
    };
    for key in values.keys() {
        if !keys.contains(key) {
            keys.push(key.clone());
        }
    }
    Ok((values, keys))
}

/// `{...base, ...over}`: base's keys in their order with over's values, then over's other keys. A key over holds as
/// `undefined` (recorded in its key order, absent from its value) overrides base's value, as an own key does in JS.
fn spread(base: &Ordered<SessionRecord>, over: &Ordered<SessionRecord>) -> Result<Ordered<SessionRecord>> {
    let (mut merged, mut keys) = own_keys(base)?;
    let (mut over_values, over_keys) = own_keys(over)?;
    for key in over_keys {
        match over_values.remove(&key) {
            Some(value) => {
                merged.insert(key.clone(), value);
            }
            None => {
                merged.remove(&key);
            }
        }
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    let order = KeyOrder::Object(
        keys.into_iter()
            .map(|key| {
                let child = merged.get(&key).map_or(KeyOrder::Unrecorded, KeyOrder::of);
                (key, child)
            })
            .collect(),
    );
    Ok(Ordered::with_order(
        serde_json::from_value(Value::Object(merged))?,
        order,
    ))
}

/// A launch plan's `ref`.
fn plan_ref(plan: &LaunchPlan) -> Option<AgentSessionRef> {
    match plan {
        LaunchPlan::New { reference, .. } | LaunchPlan::Fork { reference, .. } => reference.clone(),
        LaunchPlan::Resume { reference } => Some(reference.clone()),
    }
}

/// TS: daemon.ts silentExitNote: what an exited session shows when its agent printed nothing (it failed to start).
pub fn silent_exit_note(session: &SessionRecord, exit_code: Option<i64>, exit_signal: Option<i64>) -> String {
    let how = match exit_signal.filter(|signal| *signal != 0) {
        Some(signal) => format!("was stopped by signal {signal}"),
        None => format!(
            "exited with code {}",
            exit_code.map_or_else(|| "unknown".to_owned(), |code| code.to_string())
        ),
    };
    let command: Vec<&str> = std::iter::once(session.command.as_str())
        .chain(session.args.iter().flatten().map(String::as_str))
        .collect();
    format!(
        "{} {how} before showing anything: it failed to start.\nCommand: {}\nCheck that it runs in a terminal; s or S tries again.",
        session.program,
        command.join(" ")
    )
}

/// TS: daemon.ts truncateSessionTitle (UTF-16 lengths; `…` when cut).
pub fn truncate_session_title(value: &str, max_length: usize) -> String {
    if jsstr::js_len(value) <= max_length {
        return value.to_owned();
    }
    let max = i64::try_from(max_length).unwrap_or(i64::MAX);
    if max_length <= 1 {
        return jsstr::js_slice(value, 0, Some(max));
    }
    let cut = jsstr::js_slice(value, 0, Some(max - 1));
    format!("{}…", jsstr::js_trim_end(&cut))
}

/// TS: daemon.ts inheritedChildTitle: `<parent> / <child>` within 64 characters.
pub fn inherited_child_title(parent_title: &str, child_title: &str) -> String {
    const SEPARATOR: &str = " / ";
    let parent = jsstr::js_collapse_whitespace(jsstr::js_trim(parent_title));
    let child = jsstr::js_collapse_whitespace(jsstr::js_trim(child_title));
    if parent.is_empty() || child == parent || child.starts_with(&format!("{parent}{SEPARATOR}")) {
        return truncate_session_title(&child, 64);
    }
    let separator_length = jsstr::js_len(SEPARATOR);
    let parent_length = jsstr::js_len(&parent);
    if parent_length + separator_length + jsstr::js_len(&child) <= 64 {
        return format!("{parent}{SEPARATOR}{child}");
    }
    let min_prefix_length = parent_length.min(12);
    let max_child_length = 64usize.saturating_sub(separator_length + min_prefix_length).max(1);
    let child = truncate_session_title(&child, max_child_length);
    let prefix = truncate_session_title(
        &parent,
        64usize.saturating_sub(separator_length + jsstr::js_len(&child)).max(1),
    );
    format!("{prefix}{SEPARATOR}{child}")
}

/// TS: daemon.ts readAgentExit.
///
/// What an exited agent's last screen says about its conversation: the ref to keep, whether the launch failed, and a
/// note appended to the preview saying what s/S will do. A forked child never takes its parent's identity; a fork
/// whose parent (or whose own new conversation) the agent cannot find stores the parent's ref, so s forks it again.
pub fn read_agent_exit(session: &SessionRecord, output: &str) -> AgentExitReading {
    let agent = agent_spec(session.program);
    let fork_parent = match session.sub_session_kind {
        Some(SubSessionKind::Forked) => session.forked_from_agent_session_ref.as_ref(),
        _ => None,
    };
    let missing = agent.missing_conversation(output).filter(|missing| !missing.is_empty());
    if let Some(parent) = fork_parent {
        let missing_own = missing.as_deref().is_some_and(|missing| {
            missing == parent.value
                || session
                    .agent_session_ref
                    .as_ref()
                    .is_some_and(|own| own.value == missing)
        });
        if agent.fork_failed(output) || missing_own {
            return AgentExitReading {
                agent_session_ref: Some(parent.clone()),
                failed: true,
                note: Some(format!(
                    "{} found no saved conversation to fork (a conversation is saved once it has a message). Press s to fork the parent again, or S to start a fresh conversation.",
                    agent.label
                )),
            };
        }
    }
    // Never start fresh behind the user's back: say how to (S) instead.
    if let Some(missing) = missing {
        return AgentExitReading {
            agent_session_ref: None,
            failed: true,
            note: Some(format!(
                "{} has no saved conversation {missing}. Press S to start a fresh conversation.",
                agent.label
            )),
        };
    }
    let reference = agent.exit_ref(output);
    if let (Some(found), Some(parent)) = (reference.as_ref(), fork_parent)
        && found.value == parent.value
    {
        return AgentExitReading {
            agent_session_ref: None,
            failed: false,
            note: None,
        };
    }
    if reference.is_none() && fork_parent.is_some() && session.agent_session_ref.is_none() && !agent.id_at_launch {
        return AgentExitReading {
            agent_session_ref: None,
            failed: false,
            note: Some(format!(
                "{} did not report this fork's conversation ID, so it cannot be reopened. Press s to fork the parent again, or S to start a fresh conversation.",
                agent.label
            )),
        };
    }
    AgentExitReading {
        agent_session_ref: reference,
        failed: false,
        note: None,
    }
}

/// What a raw `create` input showed that a [`CreateSessionInput`] cannot hold: a `taskId` or `baseBranch` sent as
/// something other than text (TS refuses them at their own checks).
#[derive(Clone, Copy, Debug, Default)]
struct CreateChecks {
    task_id_not_text: bool,
    base_branch_not_text: bool,
}

impl Daemon {
    /// TS: SessionMap.set: stores the session with its worktree markers and notes projected into it.
    pub fn set_session(&self, session: Ordered<SessionRecord>) {
        let notes = self.notes_tasks.borrow().notes.clone();
        let projected = {
            let worktrees = self.worktrees.borrow();
            let projected = project_worktree(&session, &worktrees.records);
            match notes {
                Some(notes) => crate::notes_tasks::project_notes(projected, &worktrees.records, &notes),
                // No store (never in a running daemon): nothing to project.
                None => projected,
            }
        };
        self.sessions
            .borrow_mut()
            .sessions
            .insert(projected.id.clone(), projected);
    }

    /// TS: `this.sessions.get(id)`.
    pub fn get_session(&self, session_id: &str) -> Option<Ordered<SessionRecord>> {
        self.sessions.borrow().sessions.get(session_id).cloned()
    }

    /// TS: `[...this.sessions.values()]`.
    pub fn all_sessions(&self) -> Vec<Ordered<SessionRecord>> {
        self.sessions.borrow().sessions.values().cloned().collect()
    }

    /// TS: InkDaemon.requireSession (`Session does not exist`).
    pub fn require_session(&self, session_id: &str) -> Result<Ordered<SessionRecord>> {
        self.get_session(session_id)
            .ok_or_else(|| DhError::msg("Session does not exist"))
    }

    /// TS: InkDaemon.saveSession: sets it (devRunning in line), persists, broadcasts, re-syncs the Changes watches and
    /// answers the session as held now.
    pub async fn save_session(&self, session: Ordered<SessionRecord>) -> Result<Ordered<SessionRecord>> {
        let session_id = session.id.clone();
        self.set_session_in_workspace(session.clone());
        self.persist().await?;
        self.broadcast_session_updated(&session);
        // A session that joined or left a workspace (prepared, worktree deleted) moves its Changes watchers.
        self.sync_change_watches();
        self.require_session(&session_id)
    }

    /// TS: InkDaemon.setSessionInWorkspace: sets it with devRunning mirroring its workspace's Dev.
    ///
    /// devRunning mirrors the workspace's shared Dev (see syncDevRunning), also for sessions that just joined or left
    /// it.
    pub fn set_session_in_workspace(&self, session: Ordered<SessionRecord>) {
        let session_id = session.id.clone();
        self.set_session(session);
        let Some(mut projected) = self.get_session(&session_id) else {
            return;
        };
        let key = self.workspace_key_of(&projected);
        let dev_running = self.workspace_dev_live(key.as_deref());
        if projected.dev_running.unwrap_or(false) != dev_running {
            assign(&mut projected, "devRunning");
            projected.dev_running = Some(dev_running);
            self.set_session(projected);
        }
    }

    /// TS: InkDaemon.patchSession: applies `patch` to the latest record (TS `{...current, ...patch}`: keys the patch
    /// adds must be [`Ordered::touch`]ed) and answers it as now held; `None` when the session is gone.
    ///
    /// Merges onto the latest record so work that awaited in between never writes back a stale copy.
    pub fn patch_session(
        &self,
        session_id: &str,
        patch: impl FnOnce(&mut Ordered<SessionRecord>),
    ) -> Option<Ordered<SessionRecord>> {
        let mut current = self.get_session(session_id)?;
        patch(&mut current);
        self.set_session(current);
        self.get_session(session_id)
    }

    /// TS: InkDaemon.sessionsForRepo (in sidebar order).
    pub fn sessions_for_repo(&self, repo_root: &str) -> Vec<Ordered<SessionRecord>> {
        let matching: Vec<Ordered<SessionRecord>> = self
            .all_sessions()
            .into_iter()
            .filter(|session| session_matches_scope(session, repo_root))
            .collect();
        sort_sessions_for_sidebar(&matching).into_iter().cloned().collect()
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
        let Some(existing) = self
            .get_session(session_id)
            .filter(|existing| existing.status != SessionStatus::Exited)
        else {
            return Ok(());
        };
        // The exit record below drops startPrompt itself (a save here would race it with the running record).
        self.drop_draft(session_id, Some("the agent exited"), false);
        let worker = self.launch.borrow_mut().workers.shift_remove(session_id);
        let now = now_iso();
        let agent_exit = read_agent_exit(&existing, last_preview);
        // node-pty reports a signal death (e.g. SIGKILL) as {exitCode: 0, signal: 9}: that is not a completion.
        let exit_reason = if existing.exit_reason == Some(ExitReason::Stopped) {
            ExitReason::Stopped
        } else if agent_exit.failed {
            ExitReason::Failed
        } else if exit_code.is_none() || exit_signal.is_some_and(|signal| signal != 0) {
            ExitReason::Interrupted
        } else if exit_code != Some(0) {
            ExitReason::Failed
        } else {
            ExitReason::Completed
        };
        let mut exited = existing.clone();
        if let Some(reference) = agent_exit.agent_session_ref.clone() {
            assign(&mut exited, "agentSessionRef");
            exited.agent_session_ref = Some(reference);
        }
        assign(&mut exited, "startPrompt");
        exited.start_prompt = None;
        assign(&mut exited, "status");
        exited.status = SessionStatus::Exited;
        assign(&mut exited, "agentStatus");
        exited.agent_status = Some(AgentActivityStatus::Idle);
        assign(&mut exited, "agentStatusUpdatedAt");
        exited.agent_status_updated_at = Some(now.clone());
        assign(&mut exited, "updatedAt");
        exited.updated_at = now;
        assign(&mut exited, "pid");
        exited.pid = None;
        assign(&mut exited, "exitCode");
        exited.exit_code = Some(exit_code);
        assign(&mut exited, "exitSignal");
        exited.exit_signal = Some(exit_signal);
        assign(&mut exited, "lastPreview");
        exited.last_preview = Some(match agent_exit.note.as_deref() {
            Some(note) if !note.is_empty() => format!("{last_preview}\n\n{note}"),
            _ if !jsstr::js_trim(last_preview).is_empty() => last_preview.to_owned(),
            _ => silent_exit_note(&existing, exit_code, exit_signal),
        });
        assign(&mut exited, "exitReason");
        exited.exit_reason = Some(exit_reason);
        self.set_session(exited);
        let _ = tokio::fs::remove_file(self.paths.worker_dir.join(format!("{session_id}.pid"))).await;

        let worktree = existing.worktree.clone();
        if let (Some(worker), Some(worktree)) = (worker, worktree)
            && worker.delete_worktree_on_exit.get()
            && truthy(worktree.path.as_deref())
        {
            self.delete_worktree_on_exit(session_id, &existing, &worker, &worktree)
                .await?;
        }
        // It shows as exited from the set above, so it may have been removed during the awaits since: nothing to save.
        let Some(updated) = self.get_session(session_id) else {
            return Ok(());
        };
        self.save_session(updated.clone()).await?;
        {
            let daemon = self.clone();
            let session = updated.clone();
            let message = format!(
                "Agent process {}",
                updated.exit_reason.map_or("exited", ExitReason::as_str)
            );
            drop(spawn(async move { daemon.notify(&session, &message).await }));
        }
        let clients: Vec<_> = self.server.borrow().clients.values().cloned().collect();
        for client in clients {
            if client.subscription.watched_preview_session_id.as_deref() == Some(session_id) {
                let preview = build_preview_record(&updated, updated.last_preview.as_deref().unwrap_or(""), 0, 0);
                send_message(
                    &client.handle,
                    &ServerMessage::PreviewUpdated(PreviewUpdated { preview }),
                );
            }
        }
        Ok(())
    }

    /// handleWorkerSessionExit's deletion of the worktree a kill asked for (cleanup failures are recorded on the
    /// session as `cleanupError`, never thrown).
    async fn delete_worktree_on_exit(
        &self,
        session_id: &str,
        existing: &SessionRecord,
        worker: &Rc<WorkerRuntime>,
        worktree: &SessionWorktreeRecord,
    ) -> Result<()> {
        let worktree_path = worktree.path.clone().unwrap_or_default();
        let cleanup_key = path::resolve(&worktree_path);
        self.worktrees.borrow_mut().cleanup.insert(cleanup_key.clone());
        let outcome: Result<()> = async {
            let attempt: Result<()> = async {
                let repo_cwd = existing
                    .launch_worktree_root
                    .clone()
                    .unwrap_or_else(|| existing.repo_root.clone());
                self.assert_cleanup_allowed(
                    session_id,
                    worker.delete_branch_on_exit.get(),
                    worker.allow_data_loss.get(),
                )
                .await?;
                if worker.send_open_items_on_exit.get() {
                    let record = self.worktree_record_of(&*self.require_session(session_id)?);
                    self.send_worktree_items_to_backlog(
                        record.as_deref(),
                        &existing.repo_root,
                        worktree.branch.as_deref(),
                    )
                    .await?;
                }
                // The workspace's shared Terminal/Git/Dev run inside the worktree: stop them (and wait) before removal.
                if let Some(workspace) = workspace_key(existing) {
                    self.stop_changes_watch(&workspace);
                    self.retire_workspace(&workspace).await?;
                }
                remove_worktree(&worktree_path, &repo_cwd, worktree.name.as_deref()).await?;
                self.mark_worktree_deleted(session_id, worktree).await?;
                if worker.delete_branch_on_exit.get()
                    && let Some(branch) = worktree.branch.as_deref().filter(|branch| !branch.is_empty())
                {
                    delete_local_branch(&repo_cwd, branch).await?;
                }
                Ok(())
            }
            .await;
            if let Err(error) = attempt {
                let gone = match tokio::fs::metadata(&worktree_path).await {
                    Ok(_) => false,
                    Err(stat_error) => stat_error.kind() == std::io::ErrorKind::NotFound,
                };
                let marked = self
                    .get_session(session_id)
                    .and_then(|session| session.worktree.clone())
                    .is_some_and(|current| truthy(current.deleted_at.as_deref()));
                if gone && !marked {
                    self.mark_worktree_deleted(session_id, worktree).await?;
                }
                let cleanup_error = format!(
                    "{}: {error}",
                    if gone {
                        "Worktree removed; cleanup incomplete"
                    } else {
                        "Worktree retained"
                    }
                );
                let recorded = cleanup_error.clone();
                self.patch_session(session_id, move |session| {
                    assign(session, "cleanupError");
                    session.cleanup_error = Some(recorded);
                });
                self.log(&cleanup_error).await;
            }
            Ok(())
        }
        .await;
        self.worktrees.borrow_mut().cleanup.remove(&cleanup_key);
        outcome
    }

    /// handleWorkerSessionExit's markDeleted: the deletion marks the worktree's incarnation, so every session of it
    /// loses its workspace and restart/merge. Every write patches the latest record: notes/archive edits may land
    /// during the awaits.
    async fn mark_worktree_deleted(&self, session_id: &str, worktree: &SessionWorktreeRecord) -> Result<()> {
        let record = self.worktree_record_of(&*self.require_session(session_id)?);
        if let Some(mut record) = record {
            record.touch("deletedAt");
            record.deleted_at = Some(now_iso());
            return self.save_worktree_record(record).await;
        }
        let fallback = worktree.clone();
        self.patch_session(session_id, move |session| {
            let mut current = session.worktree.clone().unwrap_or(fallback);
            current.deleted_at = Some(now_iso());
            session.touch("worktree");
            session.worktree = Some(current);
            assign(session, "updatedAt");
            session.updated_at = now_iso();
        });
        Ok(())
    }

    /// TS: InkDaemon.reorderSession (`session does not exist`): swaps it with its neighbour among its siblings.
    pub async fn reorder_session(
        &self,
        session_id: &str,
        direction: MoveDirection,
    ) -> Result<Vec<Ordered<SessionRecord>>> {
        let Some(session) = self.get_session(session_id) else {
            return Err(DhError::msg("session does not exist"));
        };
        let mut siblings: Vec<Ordered<SessionRecord>> = self
            .all_sessions()
            .into_iter()
            .filter(|candidate| {
                candidate.repo_root == session.repo_root && candidate.parent_session_id == session.parent_session_id
            })
            .collect();
        siblings.sort_by(|a, b| compare_session_order(a, b));
        let index = siblings.iter().position(|candidate| candidate.id == session_id);
        let swap_index = index.and_then(|index| match direction {
            MoveDirection::Up => index.checked_sub(1),
            MoveDirection::Down => Some(index + 1).filter(|swap| *swap < siblings.len()),
        });
        let (Some(index), Some(swap_index)) = (index, swap_index) else {
            return Ok(self.sessions_for_repo(&session.repo_root));
        };
        let now = now_iso();
        let normalized: Vec<Ordered<SessionRecord>> = siblings
            .into_iter()
            .enumerate()
            .map(|(order, mut candidate)| {
                assign(&mut candidate, "sidebarOrder");
                #[allow(clippy::cast_precision_loss)]
                let order = order as f64;
                candidate.sidebar_order = Some(JsNumber(order));
                candidate
            })
            .collect();
        let (Some(current), Some(other)) = (normalized.get(index), normalized.get(swap_index)) else {
            return Ok(self.sessions_for_repo(&session.repo_root));
        };
        let mut updated_current = current.clone();
        updated_current.sidebar_order = other.sidebar_order;
        assign(&mut updated_current, "updatedAt");
        updated_current.updated_at.clone_from(&now);
        let mut updated_other = other.clone();
        updated_other.sidebar_order = current.sidebar_order;
        assign(&mut updated_other, "updatedAt");
        updated_other.updated_at = now;
        for candidate in &normalized {
            if candidate.id == updated_current.id {
                self.set_session(updated_current.clone());
            } else if candidate.id == updated_other.id {
                self.set_session(updated_other.clone());
            } else {
                self.set_session(candidate.clone());
            }
        }
        self.persist().await?;
        self.broadcast_session_updated(&updated_current);
        self.broadcast_session_updated(&updated_other);
        Ok(self.sessions_for_repo(&session.repo_root))
    }

    /// TS: InkDaemon.sameRepository (the same path, or the same Git common dir).
    pub async fn same_repository(&self, left: &str, right: &str) -> Result<bool> {
        if left == right {
            return Ok(true);
        }
        let left = find_git_common_dir(left).await?;
        let right = find_git_common_dir(right).await?;
        Ok(left == right)
    }

    /// TS: handleRequest `create`: createSession on the `input` exactly as sent. Its checks of the raw value come in
    /// createSession's order: the input itself (a TypeError reading `program`), `Unsupported agent`, a handoff into a
    /// fork, the title (a TypeError for one that is not text), then [`Daemon::create_session`]'s, where a `taskId`
    /// or `baseBranch` that is not text is refused as TS refuses it.
    ///
    /// The rest is read as [`CreateSessionInput`] holds it: an ID of another type names no session (as in TS), an
    /// unknown `worktreeMode`/`subSessionKind` is none/clean (TS also records the unknown string), a size that is no
    /// number is the default (the worker's), and a `cwd`/`repoRoot` that is not text is refused with Node's
    /// path error (TS fails later, in a path API).
    pub async fn create_session_request(&self, input: Option<&Value>) -> Result<Ordered<SessionRecord>> {
        let input = match input {
            None => return Err(DhError::msg("Cannot read properties of undefined (reading 'program')")),
            Some(Value::Null) => return Err(DhError::msg("Cannot read properties of null (reading 'program')")),
            Some(input) => input,
        };
        let field = |key: &str| input.as_object().and_then(|fields| fields.get(key));
        let Some(program) = field("program").and_then(|program| ProgramKey::deserialize(program).ok()) else {
            return Err(DhError::msg("Unsupported agent"));
        };
        let id = |key: &str| match field(key) {
            Some(Value::String(text)) => Some(text.clone()).filter(|text| !text.is_empty()),
            Some(other) if js_truthy(other) => Some(js_string(other)),
            _ => None,
        };
        let text = |key: &str| field(key).and_then(Value::as_str).map(str::to_owned);
        let handoff_from_session_id = id("handoffFromSessionId");
        let sub_session_kind = field("subSessionKind").and_then(|kind| SubSessionKind::deserialize(kind).ok());
        if handoff_from_session_id.is_some() && field("subSessionKind") == Some(&json!("forked")) {
            return Err(DhError::msg("Handoffs require a clean session"));
        }
        let parent_session_id = id("parentSessionId");
        let has_parent = parent_session_id
            .as_deref()
            .is_some_and(|id| self.get_session(id).is_some());
        let title = match field("title") {
            Some(Value::String(title)) => title.clone(),
            None => return Err(DhError::msg("Cannot read properties of undefined (reading 'trim')")),
            Some(Value::Null) => return Err(DhError::msg("Cannot read properties of null (reading 'trim')")),
            Some(_) if has_parent => return Err(DhError::msg("childTitle.trim is not a function")),
            Some(_) => return Err(DhError::msg("input.title.trim is not a function")),
        };
        let path_text = |key: &str| match field(key) {
            Some(Value::String(path)) => Ok(path.clone()),
            other => Err(DhError::msg(format!(
                "The \"paths[0]\" argument must be of type string. {}",
                node_received(other)
            ))),
        };
        let (cwd, repo_root) = (path_text("cwd")?, path_text("repoRoot")?);
        let size = |key: &str, fallback: i64| {
            field(key)
                .and_then(Value::as_f64)
                .filter(|size| size.is_finite())
                .map_or(fallback, |size| {
                    // Saturating, as the worker clamps it anyway.
                    #[allow(clippy::cast_possible_truncation)]
                    let floored = size.floor() as i64;
                    floored
                })
        };
        // `taskId !== undefined` / `baseBranch !== undefined` with a value that is not text: refused at their checks.
        let checks = CreateChecks {
            task_id_not_text: field("taskId").is_some_and(|task| !task.is_string()),
            base_branch_not_text: field("baseBranch").is_some_and(|branch| !branch.is_string()),
        };
        let input = CreateSessionInput {
            title,
            program,
            cwd,
            repo_root,
            cols: size("cols", DEFAULT_PREVIEW_COLS),
            rows: size("rows", DEFAULT_PREVIEW_ROWS),
            worktree_mode: field("worktreeMode").and_then(|mode| WorktreeMode::deserialize(mode).ok()),
            existing_worktree_path: text("existingWorktreePath"),
            parent_session_id,
            sub_session_kind,
            handoff_from_session_id,
            base_branch: if checks.base_branch_not_text {
                Some(String::new())
            } else {
                text("baseBranch")
            },
            task_id: if checks.task_id_not_text {
                Some(String::new())
            } else {
                text("taskId")
            },
            project_fingerprint: text("projectFingerprint"),
        };
        self.create_session_checked(input, checks).await
    }

    /// TS: InkDaemon.createSession: validates (title, parent, fork, task, base branch, handoff), records the session
    /// as starting and finishes it in the background (finishCreateSession, failStartingSession on error).
    pub async fn create_session(&self, input: CreateSessionInput) -> Result<Ordered<SessionRecord>> {
        self.create_session_checked(input, CreateChecks::default()).await
    }

    /// [`Daemon::create_session`] with what the raw input showed ([`CreateChecks`]).
    async fn create_session_checked(
        &self,
        input: CreateSessionInput,
        checks: CreateChecks,
    ) -> Result<Ordered<SessionRecord>> {
        let forked = input.sub_session_kind == Some(SubSessionKind::Forked);
        let handoff_from = input.handoff_from_session_id.clone().filter(|id| !id.is_empty());
        if handoff_from.is_some() && forked {
            return Err(DhError::msg("Handoffs require a clean session"));
        }
        let parent_id = input.parent_session_id.clone().filter(|id| !id.is_empty());
        let parent_session = parent_id.as_deref().and_then(|id| self.get_session(id));
        let title = match parent_session.as_ref() {
            Some(parent) => inherited_child_title(&parent.title, &input.title),
            None => jsstr::js_trim(&input.title).to_owned(),
        };
        if title.is_empty() {
            return Err(DhError::msg("title cannot be empty"));
        }
        if jsstr::js_len(&title) > 64 {
            return Err(DhError::msg("title cannot be longer than 64 characters"));
        }
        if parent_id.is_some() {
            let same = match parent_session.as_ref() {
                Some(parent) => self.same_repository(&parent.repo_root, &input.repo_root).await?,
                None => false,
            };
            if !same {
                return Err(DhError::msg("parent session does not exist in this repo"));
            }
        }
        if forked {
            let Some(parent) = parent_session.as_ref() else {
                return Err(DhError::msg("forked sub-session requires a parent session"));
            };
            let agent = agent_spec(parent.program);
            if !agent.forks {
                return Err(DhError::msg(format!(
                    "forked sub-sessions are not supported for {}",
                    agent.label
                )));
            }
            if input.program != parent.program {
                return Err(DhError::msg("forked sub-session must use the parent session program"));
            }
            if parent.agent_session_ref.is_none() {
                return Err(DhError::msg(if agent.id_at_launch {
                    "parent session does not have a resumable agent reference".to_owned()
                } else {
                    format!(
                        "{} has not reported the parent's conversation ID yet (it comes from the SessionStart hook or the exit hint), so it cannot be forked",
                        agent.label
                    )
                }));
            }
            // Checked here so a fork never fails at agent launch: this agent may reopen the fork in the parent's
            // directory.
            if !agent.forks_across_directories
                && (input.worktree_mode.unwrap_or(WorktreeMode::None) != WorktreeMode::None
                    || path::resolve(&input.cwd) != path::resolve(&parent.cwd))
            {
                return Err(DhError::msg(format!(
                    "{} forks stay in the parent's worktree; create a clean sub-session to work in another one",
                    agent.label
                )));
            }
        }
        let conflict = self.all_sessions().into_iter().any(|session| {
            session.repo_root == input.repo_root && session.title == title && session.status != SessionStatus::Exited
        });
        if conflict {
            return Err(DhError::msg(format!(
                "an active session named \"{title}\" already exists in this repo"
            )));
        }

        let command = self.resolve_program_command(input.program).await;
        let session_id = uuid::Uuid::new_v4().to_string();
        let now = now_iso();
        let sibling_orders: Vec<f64> = self
            .all_sessions()
            .iter()
            .filter(|session| {
                session.repo_root == input.repo_root && session.parent_session_id == input.parent_session_id
            })
            .map(|session| {
                session
                    .sidebar_order
                    .map(|order| order.0)
                    .filter(|order| order.is_finite())
                    .unwrap_or(0.0)
            })
            .collect();
        let next_sidebar_order = sibling_orders
            .iter()
            .copied()
            .reduce(f64::max)
            .map_or(0.0, |max| max + 1.0);
        let mut start_prompt = None;
        if let Some(task_id) = input.task_id.as_deref() {
            if checks.task_id_not_text || handoff_from.is_some() || parent_session.is_some() {
                return Err(DhError::msg("Only a new top-level session can start from a task"));
            }
            let key = self.tasks_key_for(&input.cwd).await?;
            let text = self
                .notes()
                .get(&NoteId {
                    kind: NoteKind::Tasks,
                    id: key,
                })
                .text;
            let Some(task) = parse_tasks(&text).into_iter().find(|item| item.id == task_id) else {
                return Err(DhError::msg("That task is no longer in the list"));
            };
            if task.done {
                return Err(DhError::msg("That task is done; reopen it (space) to work on it"));
            }
            if truthy(task.meta.wt.as_deref()) || truthy(task.meta.s.as_deref()) {
                return Err(DhError::msg("That task is already being worked on"));
            }
            start_prompt = Some(task_prompt(&task)).filter(|prompt| !prompt.is_empty());
        }
        if input.base_branch.is_some()
            && (checks.base_branch_not_text || input.worktree_mode.unwrap_or(WorktreeMode::None) != WorktreeMode::New)
        {
            return Err(DhError::msg("A base branch applies only to a new worktree"));
        }
        let mut handoff_path = None;
        if let Some(source_id) = handoff_from.as_deref() {
            let source = self.require_session(source_id)?;
            if !self.same_repository(&source.repo_root, &input.repo_root).await? {
                return Err(DhError::msg("Handoff source must be in the same repository"));
            }
            handoff_path = match source.handoff_path.clone() {
                Some(existing) => Some(existing),
                None => Some(self.export_session_handoff(&source, false).await?),
            };
        }
        let launch_id = uuid::Uuid::new_v4().to_string();
        let parent_ref = if forked {
            parent_session
                .as_ref()
                .and_then(|parent| parent.agent_session_ref.clone())
        } else {
            None
        };
        let record: SessionRecord = serde_json::from_value(json!({
            "id": session_id,
            "launchId": launch_id,
            "title": title,
            "program": input.program,
            "command": command,
            "args": [],
            "cwd": input.cwd,
            "repoRoot": input.repo_root,
            "launchCwd": input.cwd,
            "launchWorktreeRoot": input.cwd,
            "worktree": {"mode": "none"},
            "requestedWorktreeMode": input.worktree_mode.unwrap_or(WorktreeMode::None),
            "handoffPath": handoff_path,
            "startPrompt": start_prompt,
            "status": SessionStatus::Starting,
            "agentStatus": AgentActivityStatus::Unknown,
            "agentStatusUpdatedAt": now,
            "createdAt": now,
            "updatedAt": now,
            "lastPreview": "",
            "parentSessionId": input.parent_session_id,
            "subSessionKind": input.sub_session_kind,
            "forkedFromSessionId": if forked { parent_session.as_ref().map(|parent| parent.id.clone()) } else { None },
            "forkedFromAgentSessionRef": parent_ref,
            "sidebarOrder": JsNumber(next_sidebar_order),
        }))?;
        let mut keys = vec![
            "id",
            "launchId",
            "title",
            "program",
            "command",
            "args",
            "cwd",
            "repoRoot",
            "launchCwd",
            "launchWorktreeRoot",
            "worktree",
            "requestedWorktreeMode",
            "handoffPath",
        ];
        if record.start_prompt.is_some() {
            keys.push("startPrompt");
        }
        keys.extend([
            "status",
            "agentStatus",
            "agentStatusUpdatedAt",
            "createdAt",
            "updatedAt",
            "lastPreview",
            "parentSessionId",
            "subSessionKind",
            "forkedFromSessionId",
            "forkedFromAgentSessionRef",
            "sidebarOrder",
        ]);
        let base_session = Ordered::with_order(record, literal_order(&keys));

        self.set_session(base_session.clone());
        self.broadcast_session_updated(&base_session);
        {
            let persisted = self.persist();
            drop(spawn(async move {
                if let Err(error) = persisted.await {
                    eprintln!("failed to persist starting session {error}");
                }
            }));
        }
        self.sessions.borrow_mut().preparing.insert(base_session.id.clone());
        {
            let daemon = self.clone();
            let session_id = base_session.id.clone();
            let launch_id = base_session.launch_id.clone();
            drop(spawn(async move {
                if let Err(error) = daemon.finish_create_session(&session_id, input).await
                    && let Err(failed) = daemon
                        .fail_starting_session(&session_id, &error, launch_id.as_deref())
                        .await
                {
                    daemon.log(&format!("unhandled rejection: {failed}")).await;
                }
                daemon.sessions.borrow_mut().preparing.remove(&session_id);
            }));
        }
        Ok(base_session)
    }

    /// TS: InkDaemon.finishCreateSession: worktree (new or existing), launch plan, worktree incarnation, task link,
    /// setup, then the worker.
    pub async fn finish_create_session(&self, session_id: &str, input: CreateSessionInput) -> Result<()> {
        let Some(starting_session) = self
            .get_session(session_id)
            .filter(|session| session.status == SessionStatus::Starting)
        else {
            return Ok(());
        };

        let title = starting_session.title.clone();
        let launch_cwd = input.cwd.clone();
        let launch_worktree_root = find_repo_root(Some(&launch_cwd)).await?;
        let app_config = load_app_config().await?;
        let config_dir = self.paths.config_dir.to_string_lossy().into_owned();
        let mut session_cwd = launch_cwd.clone();
        let mut worktree = SessionWorktreeRecord::new(SessionWorktreeMode::None);
        let requested_worktree_mode = input.worktree_mode.unwrap_or(WorktreeMode::None);
        // Project configuration only matters for new worktrees (worktree settings + setup), so a malformed
        // deckhand.json never blocks none/existing sessions. The repository config is the main checkout's.
        let mut launch_project: Option<LoadedProject> = None;
        let mut project_error: Option<String> = None;
        if requested_worktree_mode == WorktreeMode::New {
            match load_project_config(&launch_cwd, None).await {
                Ok(project) => launch_project = Some(project),
                Err(error) => project_error = Some(error.to_string()),
            }
            // Worktree location/links: global defaults, overlaid by the repository override only when trusted.
            // Invalid global defaults are reported by setup below, so they do not block creation here.
            let worktree_settings = resolve_settings(launch_project.as_ref(), &app_config, &config_dir)
                .ok()
                .and_then(|settings| settings.worktree.clone());
            let created = create_worktree_for_session(
                &title,
                &launch_cwd,
                worktree_settings.as_ref(),
                input.base_branch.as_deref().filter(|base| !base.is_empty()),
            )
            .await?;
            if let Some(links) = created.links.as_ref().filter(|links| !links.notes.is_empty()) {
                self.log(&format!(
                    "worktree links for {}: {}",
                    created.path,
                    links.notes.join("; ")
                ))
                .await;
            }
            session_cwd.clone_from(&created.path);
            worktree = SessionWorktreeRecord::new(if created.origin == WorktreeOrigin::Created {
                SessionWorktreeMode::Managed
            } else {
                SessionWorktreeMode::Attached
            });
            worktree.path = Some(created.path.clone());
            worktree.branch = Some(created.branch.clone());
            worktree.head = Some(created.head.clone());
            worktree.is_main = Some(created.is_main);
            worktree.origin = Some(created.origin);
            worktree.creator = Some(created.creator);
            worktree.name = Some(created.name.clone());
            worktree.links.clone_from(&created.links);
            // The base the new branch actually started from (branchFrom), so cleanup and create-pr --base compare
            // against it.
            worktree.base_ref.clone_from(&created.base_ref);
        } else if requested_worktree_mode == WorktreeMode::Existing {
            let Some(selected_path) = input
                .existing_worktree_path
                .clone()
                .filter(|selected| !selected.is_empty())
            else {
                return Err(DhError::msg("existing worktree path is required"));
            };
            let worktrees = list_worktrees(&launch_cwd).await?;
            let Some(selected) = worktrees.into_iter().find(|item| item.path == selected_path) else {
                return Err(DhError::msg(format!(
                    "selected path is not a git worktree: {selected_path}"
                )));
            };
            session_cwd.clone_from(&selected.path);
            worktree = SessionWorktreeRecord::new(SessionWorktreeMode::Attached);
            worktree.path = Some(selected.path.clone());
            worktree.branch = Some(selected.branch.clone());
            worktree.head = Some(selected.head.clone());
            worktree.is_main = Some(selected.is_main);
            worktree.origin = Some(WorktreeOrigin::Selected);
            worktree.creator = Some(WorktreeCreator::Picker);
            worktree.name = Some(if selected.branch.is_empty() {
                selected.path.rsplit('/').next().unwrap_or_default().to_owned()
            } else {
                selected.branch.clone()
            });
        }

        let program = starting_session.program;
        let agent_name = build_deckhand_agent_name(&title, session_id, None);
        // A fork copies the parent's conversation as saved when the child launches (a turn in progress is not
        // included).
        let fork_from = match starting_session.sub_session_kind {
            Some(SubSessionKind::Forked) => starting_session.forked_from_agent_session_ref.clone(),
            _ => None,
        };
        let agent_session_ref = starting_session
            .agent_session_ref
            .clone()
            .or_else(|| new_agent_ref(program));
        let plan = match fork_from {
            Some(parent) => LaunchPlan::Fork {
                parent,
                reference: agent_session_ref.clone(),
                name: agent_name,
            },
            None => LaunchPlan::New {
                reference: agent_session_ref.clone(),
                name: agent_name,
            },
        };
        let base_ref = match worktree.base_ref.clone() {
            Some(base_ref) => base_ref,
            None => {
                let branch = current_branch(&launch_worktree_root).await?;
                if branch.is_empty() {
                    head_sha(&launch_worktree_root).await?
                } else {
                    branch
                }
            }
        };
        // The linked worktree the session runs in shares its markers with every session there: its own, or for a
        // session without one (e.g. a sub-session) the linked worktree it was launched in. The main checkout has none.
        let linked_root = match own_worktree_path(Some(&worktree)) {
            Some(root) => Some(root),
            None if requested_worktree_mode == WorktreeMode::None
                && is_linked_worktree_root(&launch_worktree_root).await =>
            {
                Some(launch_worktree_root.clone())
            }
            None => None,
        };
        let mut prepared_session = starting_session.clone();
        assign(&mut prepared_session, "cwd");
        prepared_session.cwd.clone_from(&session_cwd);
        assign(&mut prepared_session, "args");
        prepared_session.args = Some(launch_args(program, &plan));
        assign(&mut prepared_session, "agentSessionRef");
        prepared_session.agent_session_ref.clone_from(&agent_session_ref);
        assign(&mut prepared_session, "launchWorktreeRoot");
        prepared_session.launch_worktree_root = Some(launch_worktree_root.clone());
        assign(&mut prepared_session, "worktree");
        prepared_session.worktree = Some({
            let mut prepared = worktree.clone();
            prepared.base_ref = Some(base_ref);
            prepared
        });
        assign(&mut prepared_session, "updatedAt");
        prepared_session.updated_at = now_iso();
        // Where the worktree's branch started (merge detection needs a commit beyond it): its creation in the reflog,
        // else HEAD now.
        let base_sha = match linked_root.as_deref() {
            None => None,
            Some(root) => {
                let linked_branch = if worktree.mode == SessionWorktreeMode::None {
                    current_branch(root).await.ok()
                } else {
                    worktree.branch.clone()
                };
                let mut found = None;
                if let Some(branch) = linked_branch.filter(|branch| !branch.is_empty()) {
                    found = branch_creation_commit(root, &branch)
                        .await
                        .filter(|sha| !sha.is_empty());
                }
                if found.is_none() && worktree.mode != SessionWorktreeMode::None {
                    found = worktree.head.clone().filter(|head| !head.is_empty());
                }
                if found.is_none() {
                    found = head_sha(root).await.ok().filter(|sha| !sha.is_empty());
                }
                found
            }
        };
        let current = self.get_session(session_id);
        let cancelled = current
            .as_ref()
            .is_none_or(|current| current.status != SessionStatus::Starting);
        if cancelled && !(current.is_some() && truthy(worktree.path.as_deref())) {
            return Ok(());
        }
        // No await from joining the incarnation until the session that references it is set.
        let joined = linked_root
            .as_deref()
            .map(|root| self.join_worktree(root, worktree.origin == Some(WorktreeOrigin::Created), base_sha.clone()));
        if let Some(joined) = joined.as_ref()
            && let Some(prepared) = prepared_session.worktree.as_mut()
        {
            prepared.id = Some(joined.id.clone());
        }
        match current {
            Some(mut current) if cancelled => {
                assign(&mut current, "cwd");
                current.cwd.clone_from(&session_cwd);
                assign(&mut current, "launchWorktreeRoot");
                current.launch_worktree_root = Some(launch_worktree_root.clone());
                assign(&mut current, "worktree");
                current.worktree.clone_from(&prepared_session.worktree);
                assign(&mut current, "updatedAt");
                current.updated_at = now_iso();
                self.save_session(current).await?;
            }
            _ => {
                self.set_session(prepared_session.clone());
                self.broadcast_session_updated(&prepared_session);
                self.sessions.borrow_mut().preparing.remove(session_id);
            }
        }
        if let Some(superseded) = joined.as_ref().and_then(|joined| joined.superseded.clone()) {
            self.save_worktree_record(superseded).await?;
        }
        // Started from a task: it is now this worktree's (or, in the main checkout, this session's) work.
        if let Some(task_id) = input.task_id.as_deref().filter(|id| !id.is_empty()) {
            let link = match joined.as_ref() {
                Some(joined) => TaskLink::Worktree { wt: joined.id.clone() },
                None => TaskLink::Session {
                    s: session_id.to_owned(),
                },
            };
            let key = self.tasks_key_for(&input.cwd).await?;
            let op = TaskOp::Link {
                id: task_id.to_owned(),
                link,
            };
            if let Err(error) = self.change_tasks(&key, op).await {
                self.log(&format!("linking task {task_id} failed: {error}")).await;
            }
        }
        if cancelled {
            return Ok(());
        }

        // Setup uses the configuration that was reviewed for this launch (the same bytes whose trust placed the
        // worktree): global defaults, overlaid by the repository override only when trusted.
        if worktree.origin == Some(WorktreeOrigin::Created) {
            let resolved = match project_error.as_deref() {
                Some(problem) => Err(DhError::msg(format!(
                    "{problem}. Fix it with C, then press s to retry setup"
                ))),
                None => resolve_setup_command(
                    launch_project.as_ref(),
                    &app_config,
                    input.project_fingerprint.as_deref(),
                    &config_dir,
                ),
            };
            let setup_command = match resolved {
                Ok(command) => command,
                Err(error) => {
                    // Unreadable or unreviewed config: setup is owed once it is fixed or reviewed.
                    let command = launch_project
                        .as_ref()
                        .and_then(|project| project.config.setup_command.clone())
                        .unwrap_or_else(|| "configuration".to_owned());
                    let output = error.to_string();
                    self.patch_session(session_id, move |session| {
                        assign(session, "setup");
                        session.setup = Some(SessionSetup {
                            command,
                            state: SetupState::Failed,
                            output,
                            exit_code: None,
                            extra: IndexMap::new(),
                        });
                    });
                    return Err(DhError::msg(format!("Worktree retained: {error}")));
                }
            };
            if let Some(setup_command) = setup_command.filter(|command| !command.is_empty()) {
                // Recorded before anything can fail or be cancelled, so a restart knows setup is still owed.
                let mut pending = self.require_session(session_id)?;
                assign(&mut pending, "setup");
                pending.setup = Some(SessionSetup {
                    command: setup_command.clone(),
                    state: SetupState::Pending,
                    output: String::new(),
                    exit_code: None,
                    extra: IndexMap::new(),
                });
                self.save_session(pending).await?;
                self.run_setup(session_id, &setup_command).await?;
            }
        }
        self.assert_current_launch(session_id, starting_session.launch_id.as_deref())?;
        let mut args = prepared_session.args.clone().unwrap_or_default();
        args.extend(
            integration_args(
                prepared_session.program,
                &prepared_session.command,
                hooks_enabled(app_config.agent_hooks, prepared_session.program),
                app_config.claude_scroll_override != Some(false),
            )
            .await?,
        );
        args.extend(self.first_message_args(&prepared_session, &plan, true));
        prepared_session.args = Some(args.clone());
        let mut launch_session = self.require_session(session_id)?;
        assign(&mut launch_session, "args");
        launch_session.args = Some(args);
        assign(&mut launch_session, "handoffPath");
        launch_session.handoff_path.clone_from(&prepared_session.handoff_path);
        self.set_session(launch_session.clone());
        let running_session = self
            .start_worker(
                launch_session.clone(),
                input.cols,
                input.rows,
                app_config.claude_scroll_override != Some(false),
            )
            .await?;
        // Started from a task: its text waits to be typed into the agent's input (typeDraft), never sent. Registered
        // before the session is announced as running, so input typed right away always drops it.
        let worker = self.launch.borrow().workers.get(session_id).cloned();
        if let (Some(worker), Some(text)) = (
            worker,
            launch_session.start_prompt.clone().filter(|prompt| !prompt.is_empty()),
        ) {
            let daemon = self.downgrade();
            let id = session_id.to_owned();
            let timer = Timer::after(DRAFT_WAIT, move || {
                if let Some(daemon) = daemon.upgrade() {
                    daemon.drop_draft(&id, Some("the agent did not settle at its input in time"), true);
                }
            });
            *worker.draft.borrow_mut() = Some(Draft {
                text,
                timer,
                settle: None,
            });
        }
        let mut running = spread(&running_session, &self.require_session(session_id)?)?;
        assign(&mut running, "status");
        running.status = SessionStatus::Running;
        assign(&mut running, "pid");
        running.pid = running_session.pid;
        self.save_session(running).await?;
        Ok(())
    }

    /// TS: InkDaemon.failStartingSession: a session still starting (with this launch) exits as failed
    /// (`Failed to start session: <message>`).
    pub async fn fail_starting_session(
        &self,
        session_id: &str,
        error: &DhError,
        launch_id: Option<&str>,
    ) -> Result<()> {
        let Some(mut session) = self.get_session(session_id) else {
            return Ok(());
        };
        if session.status != SessionStatus::Starting
            || launch_id
                .is_some_and(|launch_id| !launch_id.is_empty() && session.launch_id.as_deref() != Some(launch_id))
        {
            return Ok(());
        }
        let setup_output = session
            .setup
            .as_ref()
            .map(|setup| setup.output.clone())
            .filter(|output| !output.is_empty())
            .map(|output| format!("\n{output}"))
            .unwrap_or_default();
        assign(&mut session, "status");
        session.status = SessionStatus::Exited;
        assign(&mut session, "updatedAt");
        session.updated_at = now_iso();
        assign(&mut session, "pid");
        session.pid = None;
        assign(&mut session, "exitCode");
        session.exit_code = Some(None);
        assign(&mut session, "exitSignal");
        session.exit_signal = Some(None);
        assign(&mut session, "lastPreview");
        session.last_preview = Some(format!("Failed to start session: {error}{setup_output}"));
        assign(&mut session, "exitReason");
        session.exit_reason = Some(ExitReason::Failed);
        self.save_session(session).await?;
        Ok(())
    }

    /// TS: InkDaemon.restartSession: `s` resumes (or re-forks), `S` starts fresh; retries owed setup first.
    ///
    /// `mode` is `None` for a mode the client sent that is neither: TS then tests `mode === 'resume'` and
    /// `mode === 'fresh'` false alike (relaunched as a resume, without reading the exit hint or the continuation).
    pub async fn restart_session(
        &self,
        session_id: &str,
        cols: i64,
        rows: i64,
        mode: Option<RestartMode>,
        project_fingerprint: Option<String>,
    ) -> Result<Ordered<SessionRecord>> {
        let Some(existing) = self.get_session(session_id) else {
            return Err(DhError::msg("session does not exist"));
        };
        if self.sessions.borrow().preparing.contains(session_id) {
            return Err(DhError::msg(
                "Worktree preparation is still finishing; wait before retrying",
            ));
        }
        let has_worker = self.launch.borrow().workers.contains_key(session_id);
        if has_worker || existing.status != SessionStatus::Exited {
            return Err(DhError::msg("session is already running"));
        }
        if existing
            .worktree
            .as_ref()
            .is_some_and(|worktree| truthy(worktree.deleted_at.as_deref()))
        {
            return Err(DhError::msg("cannot restart session because its worktree was deleted"));
        }

        let now = now_iso();
        let fresh = mode == Some(RestartMode::Fresh);
        let parsed_agent_session_ref = match existing.last_preview.as_deref() {
            Some(preview) if mode == Some(RestartMode::Resume) && !preview.is_empty() => {
                read_agent_exit(&existing, preview).agent_session_ref
            }
            _ => None,
        };
        let never_started = truthy(existing.launch_id.as_deref()) && !truthy(existing.agent_started_at.as_deref());
        let source = match parsed_agent_session_ref {
            Some(reference) => {
                let mut source = existing.clone();
                assign(&mut source, "agentSessionRef");
                source.agent_session_ref = Some(reference);
                source
            }
            None => existing.clone(),
        };
        // continuedConversation only follows a resume (`mode !== 'resume'` returns the source).
        let restart_source = self
            .continued_conversation(source, mode.unwrap_or(RestartMode::Fresh), never_started)
            .await;
        if never_started
            && existing
                .requested_worktree_mode
                .is_some_and(|requested| requested != WorktreeMode::None)
            && !existing
                .worktree
                .as_ref()
                .is_some_and(|worktree| truthy(worktree.path.as_deref()))
        {
            return Err(DhError::msg(
                "Worktree preparation did not complete. Create a new session to retry instead of launching in the original checkout.",
            ));
        }
        let agent = agent_spec(existing.program);
        let suffix = fresh.then(|| format!("fresh-{}", base36(date::now_ms())));
        let agent_name = build_deckhand_agent_name(&existing.title, &existing.id, suffix.as_deref());
        // s resumes the session's own conversation; a fork without one forks its parent again (new child ID); S
        // starts fresh.
        let relaunch_mode = if fresh { RestartMode::Fresh } else { RestartMode::Resume };
        let Some(plan) = relaunch_plan(&restart_source, relaunch_mode, never_started, &agent_name) else {
            let hooks = if agent.id_at_launch {
                String::new()
            } else {
                format!(", or enable trusted {} hooks before starting new sessions", agent.label)
            };
            return Err(DhError::msg(format!(
                "{} conversation ID is unknown. Use S for a fresh session{hooks}.",
                agent.label
            )));
        };
        let missing_conversation = agent
            .missing_conversation(existing.last_preview.as_deref().unwrap_or(""))
            .filter(|missing| !missing.is_empty());
        if let (LaunchPlan::Resume { reference }, Some(missing)) = (&plan, missing_conversation.as_deref())
            && missing == reference.value
        {
            return Err(DhError::msg(format!(
                "{} has no saved conversation {missing}. Use S for a fresh session.",
                agent.label
            )));
        }
        // The agent is looked up on PATH again: the path stored at its first launch may be gone (reinstalled or
        // moved), and launching it would fail at once without a word.
        let command = self.resolve_program_command(existing.program).await;
        if !command.contains('/') {
            return Err(DhError::msg(format!(
                "{} ({}) is not installed or not on PATH; install it (deckhand setup) and press s again",
                agent.label, existing.program
            )));
        }
        let mut starting = restart_source;
        assign(&mut starting, "command");
        starting.command = command;
        assign(&mut starting, "launchId");
        starting.launch_id = Some(uuid::Uuid::new_v4().to_string());
        assign(&mut starting, "agentStartedAt");
        starting.agent_started_at = if fresh { None } else { existing.agent_started_at.clone() };
        assign(&mut starting, "archivedAt");
        starting.archived_at = None;
        assign(&mut starting, "attention");
        starting.attention = None;
        assign(&mut starting, "exitReason");
        starting.exit_reason = None;
        assign(&mut starting, "cleanupError");
        starting.cleanup_error = None;
        assign(&mut starting, "args");
        starting.args = Some(launch_args(existing.program, &plan));
        assign(&mut starting, "agentSessionRef");
        starting.agent_session_ref = plan_ref(&plan);
        // A fresh conversation is no longer a copy of the parent's, so s never forks the parent for it.
        if fresh {
            assign(&mut starting, "forkedFromAgentSessionRef");
            starting.forked_from_agent_session_ref = None;
        }
        assign(&mut starting, "status");
        starting.status = SessionStatus::Starting;
        assign(&mut starting, "agentStatus");
        starting.agent_status = Some(AgentActivityStatus::Unknown);
        assign(&mut starting, "agentStatusUpdatedAt");
        starting.agent_status_updated_at = Some(now.clone());
        assign(&mut starting, "updatedAt");
        starting.updated_at = now;
        assign(&mut starting, "pid");
        starting.pid = None;
        assign(&mut starting, "exitCode");
        starting.exit_code = None;
        assign(&mut starting, "exitSignal");
        starting.exit_signal = None;
        assign(&mut starting, "lastPreview");
        starting.last_preview = Some(String::new());
        self.save_session(starting.clone()).await?;

        let launch_id = starting.launch_id.clone();
        let attempt: Result<Ordered<SessionRecord>> = async {
            let config = load_app_config().await?;
            // Setup that never completed (pending, failed, untrusted, cancelled) is retried against the repository's
            // current effective configuration. Plain restarts never read deckhand.json.
            if existing
                .setup
                .as_ref()
                .is_some_and(|setup| setup.state != SetupState::Complete)
            {
                let project = load_project_config(&existing.cwd, None).await?;
                let config_dir = self.paths.config_dir.to_string_lossy().into_owned();
                let setup_command =
                    resolve_setup_command(Some(&project), &config, project_fingerprint.as_deref(), &config_dir)?;
                match setup_command.filter(|command| !command.is_empty()) {
                    None => {
                        self.patch_session(session_id, |session| {
                            assign(session, "setup");
                            session.setup = None;
                        });
                    }
                    Some(command) => self.run_setup(session_id, &command).await?,
                }
            }
            self.assert_current_launch(session_id, launch_id.as_deref())?;
            let mut args = starting.args.clone().unwrap_or_default();
            args.extend(
                integration_args(
                    starting.program,
                    &starting.command,
                    hooks_enabled(config.agent_hooks, starting.program),
                    config.claude_scroll_override != Some(false),
                )
                .await?,
            );
            args.extend(self.first_message_args(&starting, &plan, never_started));
            starting.args = Some(args.clone());
            prepare_agent_session_ref(starting.agent_session_ref.as_ref()).await?;
            let mut launch = self.require_session(session_id)?;
            assign(&mut launch, "args");
            launch.args = Some(args);
            let running_session = self
                .start_worker(launch, cols, rows, config.claude_scroll_override != Some(false))
                .await?;
            let mut running = spread(&running_session, &self.require_session(session_id)?)?;
            assign(&mut running, "status");
            running.status = SessionStatus::Running;
            assign(&mut running, "pid");
            running.pid = running_session.pid;
            self.save_session(running).await
        }
        .await;
        match attempt {
            Ok(running) => Ok(running),
            Err(error) => {
                let still_starting = self
                    .get_session(session_id)
                    .is_some_and(|session| session.status == SessionStatus::Starting);
                if still_starting {
                    self.fail_starting_session(session_id, &error, launch_id.as_deref())
                        .await?;
                }
                Err(error)
            }
        }
    }

    /// TS: InkDaemon.killSession: checks the cleanup a delete asks for, marks the worker's exit plan and asks it to
    /// kill the agent (`session is not running`).
    pub async fn kill_session(&self, session_id: &str, options: KillOptions) -> Result<()> {
        let delete_on_exit = options.delete_worktree || options.delete_branch;
        let assert_running = || -> Result<(Ordered<SessionRecord>, Rc<WorkerRuntime>)> {
            let current = self.require_session(session_id)?;
            let worker = self.launch.borrow().workers.get(session_id).cloned();
            match worker {
                Some(worker) if !worker.channel.is_exited() && current.status != SessionStatus::Exited => {
                    Ok((current, worker))
                }
                _ => Err(DhError::msg("session is not running")),
            }
        };
        assert_running()?;
        if delete_on_exit {
            self.assert_cleanup_allowed(session_id, options.delete_branch, options.allow_data_loss)
                .await?;
        }
        // Re-read after the slow cleanup checks: the session may have exited or been edited meanwhile.
        let (mut current, worker) = assert_running()?;
        worker.allow_data_loss.set(options.allow_data_loss);
        worker.delete_worktree_on_exit.set(delete_on_exit);
        worker.delete_branch_on_exit.set(options.delete_branch);
        worker
            .send_open_items_on_exit
            .set(delete_on_exit && options.send_open_items);
        assign(&mut current, "exitReason");
        current.exit_reason = Some(ExitReason::Stopped);
        assign(&mut current, "cleanupError");
        current.cleanup_error = None;
        self.set_session(current);
        let _: Value = self
            .send_worker_request(session_id, payload(json!({"type": "kill", "force": options.force})))
            .await?;
        Ok(())
    }

    /// TS: InkDaemon.removeSession (`session does not exist`, `kill the session before removing it`): forgets it, its
    /// note, its worktree record when it was the last, abandons its tasks and retires an orphaned workspace.
    pub async fn remove_session(&self, session_id: &str) -> Result<()> {
        let Some(existing) = self.get_session(session_id) else {
            return Err(DhError::msg("session does not exist"));
        };
        let has_worker = self.launch.borrow().workers.contains_key(session_id);
        if has_worker || existing.status == SessionStatus::Running {
            return Err(DhError::msg("kill the session before removing it"));
        }
        let slot_prefix = format!("{session_id}\0");
        self.changes
            .borrow_mut()
            .summaries
            .retain(|slot, _| !slot.starts_with(&slot_prefix));
        let workspace = self.workspace_key_of(&existing);
        self.sessions.borrow_mut().sessions.shift_remove(session_id);
        // A worktree record lives as long as a session references it.
        let worktree_id = existing
            .worktree
            .as_ref()
            .and_then(|worktree| worktree.id.clone())
            .filter(|id| !id.is_empty());
        let dropped = worktree_id.filter(|id| {
            !self
                .sessions
                .borrow()
                .sessions
                .values()
                .any(|session| session.worktree.as_ref().and_then(|worktree| worktree.id.as_ref()) == Some(id))
        });
        if let Some(dropped) = dropped.as_deref() {
            self.worktrees.borrow_mut().records.shift_remove(dropped);
        }
        self.persist().await?;
        // A session's note goes with it, a worktree's with its record; the main checkout's note is never deleted.
        let notes = self.notes();
        let session_note = NoteId {
            kind: NoteKind::Session,
            id: session_id.to_owned(),
        };
        if let Err(error) = notes.remove(&session_note, Some(&existing.title)).await {
            self.log(&format!("removing notes of {session_id} failed: {error}"))
                .await;
        }
        let branch = existing
            .worktree
            .as_ref()
            .and_then(|worktree| worktree.branch.clone())
            .filter(|branch| !branch.is_empty());
        if let Some(dropped) = dropped.as_deref() {
            let worktree_note = NoteId {
                kind: NoteKind::Worktree,
                id: dropped.to_owned(),
            };
            let label = branch.clone().unwrap_or_else(|| existing.title.clone());
            if let Err(error) = notes.remove(&worktree_note, Some(&label)).await {
                self.log(&format!("removing worktree notes {dropped} failed: {error}"))
                    .await;
            }
        }
        // Tasks it was doing go back to the backlog: the session's own link, and its worktree's once nothing refers to
        // it.
        self.change_linked_tasks(TaskOp::AbandonLinked {
            link: TaskLink::Session {
                s: session_id.to_owned(),
            },
            tried: branch.clone(),
        })
        .await;
        if let Some(dropped) = dropped.as_deref() {
            self.change_linked_tasks(TaskOp::AbandonLinked {
                link: TaskLink::Worktree { wt: dropped.to_owned() },
                tried: branch,
            })
            .await;
        }
        self.broadcast_session_removed(&existing);
        self.sync_change_watches();
        // Workspace panes outlive agents but not their workspace's sessions: with the last one gone, nothing could
        // show or stop them (a shell never exits on its own).
        if let Some(workspace) = workspace {
            let still_used = self
                .all_sessions()
                .iter()
                .any(|session| self.workspace_key_of(session).as_deref() == Some(workspace.as_str()));
            if !still_used {
                let daemon = self.clone();
                drop(spawn(async move {
                    if let Err(error) = daemon.retire_workspace(&workspace).await {
                        daemon
                            .log(&format!("stopping workspace {workspace} failed: {error}"))
                            .await;
                    }
                }));
            }
        }
        Ok(())
    }

    /// TS: InkDaemon.exportSessionHandoff: Git context (bounded, fail-soft; none for a deleted worktree) and the
    /// linked task, written by sessionFeatures.ts exportHandoff; answers the file path.
    pub async fn export_session_handoff(&self, session: &SessionRecord, include_output: bool) -> Result<String> {
        let worktree = session.worktree.as_ref();
        let git_context = if worktree.is_some_and(|worktree| truthy(worktree.deleted_at.as_deref())) {
            None
        } else {
            get_handoff_git_context(&session.cwd, worktree.and_then(|worktree| worktree.base_ref.as_deref())).await
        };
        let linked = self.linked_tasks_of(session);
        let task = linked
            .iter()
            .find(|item| !item.done)
            .or_else(|| linked.first())
            .map(|task| HandoffTask {
                title: task.title.clone(),
                body: task.body.clone(),
                done: task.done,
            });
        let file = export_handoff(session, include_output, git_context.as_ref(), task.as_ref()).await?;
        Ok(file.to_string_lossy().into_owned())
    }

    /// TS: handleRequest `list`: every session in sidebar order.
    pub fn handle_list(&self) -> Result<Vec<Ordered<SessionRecord>>> {
        let sessions = self.all_sessions();
        Ok(sort_sessions_for_sidebar(&sessions).into_iter().cloned().collect())
    }

    /// TS: handleRequest `set-session-done`: sets or clears `doneAt`, then ticks or reopens the linked tasks.
    ///
    /// Independent of merged (worktree) and archived: any session, sub-sessions in a worktree included.
    pub async fn handle_set_session_done(&self, request: &SetSessionDone) -> Result<Ordered<SessionRecord>> {
        let mut session = self.require_session(&request.session_id)?;
        let done_at = if request.done {
            Some(session.done_at.clone().unwrap_or_else(now_iso))
        } else {
            None
        };
        assign(&mut session, "doneAt");
        session.done_at = done_at;
        assign(&mut session, "updatedAt");
        session.updated_at = now_iso();
        let saved = self.save_session(session).await?;
        self.sync_done_tasks(&saved, request.done).await;
        Ok(saved)
    }

    /// TS: handleRequest `archive-session`.
    pub async fn handle_archive_session(&self, request: &ArchiveSession) -> Result<Ordered<SessionRecord>> {
        let mut session = self.require_session(&request.session_id)?;
        assign(&mut session, "archivedAt");
        session.archived_at = request.archived.then(now_iso);
        self.save_session(session).await
    }

    /// TS: handleRequest `export-handoff`: exports and records `handoffPath`; answers the path.
    pub async fn handle_export_handoff(&self, request: &ExportHandoff) -> Result<String> {
        let source = self.require_session(&request.session_id)?;
        let handoff_path = self
            .export_session_handoff(&source, request.include_output.unwrap_or(false))
            .await?;
        let mut session = self.require_session(&request.session_id)?;
        assign(&mut session, "handoffPath");
        session.handoff_path = Some(handoff_path.clone());
        self.save_session(session).await?;
        Ok(handoff_path)
    }

    /// TS: handleRequest `cancel-start` (`Session is not starting`): stops setup and the worker, records the session
    /// as stopped (`Setup cancelled; worktree retained.` or the startup-cancelled note).
    pub async fn handle_cancel_start(&self, request: &SessionRequest) -> Result<OkResult> {
        let mut session = self.require_session(&request.session_id)?;
        if session.status != SessionStatus::Starting {
            return Err(DhError::msg("Session is not starting"));
        }
        let child = self.launch.borrow().setup_processes.get(&session.id).copied();
        if let Some(pid) = child.and_then(|child| child.pid).filter(|pid| *pid != 0) {
            let _ = proc::kill_group(pid, Signal::SIGTERM);
        }
        let has_worker = self.launch.borrow().workers.contains_key(&session.id);
        if has_worker {
            self.send_worker_event(
                &session.id,
                payload(json!({"type": "kill", "requestId": uuid::Uuid::new_v4().to_string(), "force": true})),
            );
        }
        // Only an in-flight setup is cancelled; pending/failed setup stays retryable as-is.
        let setup_running = child.is_some()
            || session
                .setup
                .as_ref()
                .is_some_and(|setup| setup.state == SetupState::Running);
        assign(&mut session, "status");
        session.status = SessionStatus::Exited;
        assign(&mut session, "exitReason");
        session.exit_reason = Some(ExitReason::Stopped);
        assign(&mut session, "lastPreview");
        session.last_preview = Some(if setup_running {
            "Setup cancelled; worktree retained.".to_owned()
        } else {
            "Startup cancelled. In-flight worktree preparation may finish, but the agent will not launch.".to_owned()
        });
        session.touch("setup");
        if setup_running && let Some(setup) = session.setup.as_mut() {
            // `{...session.setup, state, output}`: the setup's own keys keep their places.
            setup.state = SetupState::Cancelled;
            let limit = i64::try_from(SETUP_OUTPUT_STORED_LIMIT).unwrap_or(i64::MAX);
            setup.output = jsstr::js_slice(&setup.output, -limit, None);
        }
        self.save_session(session).await?;
        Ok(OkResult::default())
    }

    /// TS: handleRequest `remove`: moves the open note items first when asked, then removeSession.
    pub async fn handle_remove(&self, request: &Remove) -> Result<OkResult> {
        if request.move_open_items == Some(true) {
            self.move_open_note_items(&request.session_id).await?;
        }
        self.remove_session(&request.session_id).await?;
        Ok(OkResult::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::{local, session, test_client, test_daemon};
    use deckhand_protocol::records::WorktreeRecord;
    use std::path::Path;
    use std::sync::MutexGuard;

    /// DECKHAND_HOME = `dir` while this lives.
    struct Home {
        _guard: MutexGuard<'static, ()>,
        previous: Option<std::ffi::OsString>,
    }

    impl Home {
        fn set(dir: &Path) -> Home {
            let guard = crate::server::tests::env_lock();
            let previous = std::env::var_os(deckhand_sys::paths::HOME_ENV);
            // SAFETY: tests touching the environment hold server::tests::env_lock.
            unsafe { std::env::set_var(deckhand_sys::paths::HOME_ENV, dir) };
            Home {
                _guard: guard,
                previous,
            }
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            // SAFETY: as in Home::set (the environment lock is still held).
            unsafe {
                match self.previous.take() {
                    Some(value) => std::env::set_var(deckhand_sys::paths::HOME_ENV, value),
                    None => std::env::remove_var(deckhand_sys::paths::HOME_ENV),
                }
            }
        }
    }

    /// A daemon without a notes store (the notes projection belongs to notes_tasks.rs).
    fn daemon(dir: &Path) -> Daemon {
        let daemon = test_daemon(dir);
        daemon.notes_tasks.borrow_mut().notes = None;
        daemon
    }

    /// A session record from JSON (missing basics filled in).
    fn record(value: Value) -> Ordered<SessionRecord> {
        let mut map = object(json!({
            "id": "s",
            "title": "s",
            "program": "claude",
            "command": "/bin/claude",
            "cwd": "/r",
            "repoRoot": "/r",
            "status": "exited",
            "createdAt": "2026-01-01T00:00:00.000Z",
            "updatedAt": "2026-01-01T00:00:00.000Z",
        }));
        for (key, value) in object(value) {
            map.insert(key, value);
        }
        serde_json::from_value(Value::Object(map)).unwrap()
    }

    fn put(daemon: &Daemon, session: Ordered<SessionRecord>) {
        daemon
            .sessions
            .borrow_mut()
            .sessions
            .insert(session.id.clone(), session);
    }

    fn input(value: Value) -> CreateSessionInput {
        let mut map = object(json!({
            "title": "new",
            "program": "claude",
            "cwd": "/r",
            "repoRoot": "/r",
            "cols": 80,
            "rows": 24,
        }));
        for (key, value) in object(value) {
            map.insert(key, value);
        }
        serde_json::from_value(Value::Object(map)).unwrap()
    }

    fn error<T: std::fmt::Debug>(result: Result<T>) -> String {
        result.unwrap_err().to_string()
    }

    fn git(cwd: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(["-c", "user.name=Deckhand Test", "-c", "user.email=test@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    /// A repository with one commit on main.
    fn repo(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("README.md"), "hi\n").unwrap();
        git(dir, &["add", "README.md"]);
        git(dir, &["commit", "-q", "-m", "init"]);
    }

    #[test]
    fn silent_exit_notes_say_how_the_agent_ended() {
        let mut exited = record(json!({"program": "codex", "command": "/bin/codex", "args": ["--yolo", "x"]}));
        assert_eq!(
            silent_exit_note(&exited, Some(1), None),
            "codex exited with code 1 before showing anything: it failed to start.\nCommand: /bin/codex --yolo x\nCheck that it runs in a terminal; s or S tries again."
        );
        assert!(silent_exit_note(&exited, None, Some(9)).starts_with("codex was stopped by signal 9 before"));
        // Signal 0 is no signal; a missing code is unknown.
        assert!(silent_exit_note(&exited, None, Some(0)).starts_with("codex exited with code unknown before"));
        exited.args = None;
        assert!(silent_exit_note(&exited, Some(0), None).contains("\nCommand: /bin/codex\n"));
    }

    #[test]
    fn titles_are_cut_in_utf16_units_like_the_ts() {
        assert_eq!(truncate_session_title("hello world", 6), "hello…");
        assert_eq!(truncate_session_title("abc", 1), "a");
        assert_eq!(truncate_session_title("abc", 0), "");
        assert_eq!(truncate_session_title("a😀bc", 4), "a😀…");
        assert_eq!(truncate_session_title("short", 5), "short");

        assert_eq!(
            inherited_child_title("Parent  task", "  child\tone "),
            "Parent task / child one"
        );
        assert_eq!(inherited_child_title("", "child"), "child");
        assert_eq!(inherited_child_title("Parent", "Parent / child"), "Parent / child");
        assert_eq!(
            inherited_child_title(&"P".repeat(40), &"c".repeat(40)),
            format!("{}… / {}", "P".repeat(20), "c".repeat(40))
        );
        assert_eq!(
            inherited_child_title("Parent", &"x".repeat(70)),
            format!("Parent / {}…", "x".repeat(54))
        );
        assert_eq!(
            inherited_child_title(
                "A long parent title that goes on",
                "and a long child title that also goes on"
            ),
            "A long parent title… / and a long child title that also goes on"
        );
    }

    #[test]
    fn agent_exits_keep_or_refuse_conversations() {
        let id = "0b5c6c4e-1d2f-4a3b-9c8d-7e6f5a4b3c2d";
        let parent_id = "11111111-2222-4333-8444-555555555555";
        let plain = record(json!({}));
        let resumed = read_agent_exit(&plain, &format!("bye\n  claude --resume {id}\n"));
        assert!(!resumed.failed);
        assert_eq!(resumed.note, None);
        assert_eq!(resumed.agent_session_ref.unwrap().value, id);

        let missing = read_agent_exit(&plain, &format!("No conversation found with session ID: {id}"));
        assert!(missing.failed);
        assert_eq!(missing.agent_session_ref, None);
        assert_eq!(
            missing.note.as_deref(),
            Some(format!("Claude has no saved conversation {id}. Press S to start a fresh conversation.").as_str())
        );

        let fork = record(json!({
            "subSessionKind": "forked",
            "forkedFromAgentSessionRef": {"provider": "claude", "kind": "id", "value": parent_id},
            "agentSessionRef": {"provider": "claude", "kind": "id", "value": id},
        }));
        let failed = read_agent_exit(&fork, &format!("No conversation found with session ID: {id}"));
        assert!(failed.failed);
        assert_eq!(failed.agent_session_ref.unwrap().value, parent_id);
        assert!(
            failed
                .note
                .unwrap()
                .starts_with("Claude found no saved conversation to fork")
        );
        // A fork never takes its parent's identity.
        let parents = read_agent_exit(&fork, &format!("claude --resume {parent_id}"));
        assert_eq!(
            parents,
            AgentExitReading {
                agent_session_ref: None,
                failed: false,
                note: None
            }
        );

        let codex_fork = record(json!({
            "program": "codex",
            "subSessionKind": "forked",
            "forkedFromAgentSessionRef": {"provider": "codex", "kind": "id", "value": parent_id},
        }));
        let unreported = read_agent_exit(&codex_fork, "bye");
        assert!(!unreported.failed);
        assert_eq!(
            unreported.note.as_deref(),
            Some(
                "Codex did not report this fork's conversation ID, so it cannot be reopened. Press s to fork the parent again, or S to start a fresh conversation."
            )
        );
        let pi_fork = record(json!({
            "program": "pi",
            "subSessionKind": "forked",
            "forkedFromAgentSessionRef": {"provider": "pi", "kind": "path", "value": "/p/s.jsonl"},
        }));
        let pi = read_agent_exit(&pi_fork, "No session found matching x");
        assert!(pi.failed);
        assert_eq!(pi.agent_session_ref.unwrap().value, "/p/s.jsonl");
    }

    #[test]
    fn fresh_suffixes_are_base36_like_js() {
        assert_eq!(base36(0), "0");
        assert_eq!(base36(35), "z");
        assert_eq!(base36(36), "10");
        assert_eq!(base36(1_767_225_600_000), "mjuohs00");
        assert_eq!(base36(-71), "-1z");
    }

    #[test]
    fn the_session_map_projects_worktree_markers_and_patches_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon(dir.path());
        let record_json = json!({"id": "w1", "path": "/wt", "createdAt": "2026-01-01T00:00:00.000Z", "mergedAt": "2026-01-02T00:00:00.000Z"});
        let worktree: Ordered<WorktreeRecord> = serde_json::from_value(record_json).unwrap();
        daemon.worktrees.borrow_mut().records.insert("w1".into(), worktree);
        daemon.set_session(record(
            json!({"id": "a", "worktree": {"mode": "managed", "path": "/wt", "id": "w1"}}),
        ));
        daemon.set_session(record(json!({"id": "b"})));
        let held = daemon.require_session("a").unwrap();
        assert_eq!(
            held.worktree.as_ref().unwrap().merged_at.as_deref(),
            Some("2026-01-02T00:00:00.000Z")
        );
        assert_eq!(error(daemon.require_session("zzz")), "Session does not exist");
        assert!(daemon.get_session("zzz").is_none());
        assert!(daemon.patch_session("zzz", |_| {}).is_none());

        // `{...current, doneAt}`: a new key goes last; a re-set keeps the session's place in the map.
        let patched = daemon
            .patch_session("a", |session| {
                assign(session, "doneAt");
                session.done_at = Some("2026-01-03T00:00:00.000Z".into());
                assign(session, "title");
                session.title = "renamed".into();
            })
            .unwrap();
        let text = serde_json::to_string(&patched).unwrap();
        assert!(text.ends_with(r#""doneAt":"2026-01-03T00:00:00.000Z"}"#), "{text}");
        assert!(text.starts_with(r#"{"id":"a","title":"renamed""#), "{text}");
        let ids: Vec<String> = daemon.all_sessions().iter().map(|session| session.id.clone()).collect();
        assert_eq!(ids, ["a", "b"]);
    }

    #[test]
    fn lists_are_in_sidebar_order_and_scoped_to_the_repository() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon(dir.path());
        daemon.set_session(record(json!({"id": "late", "sidebarOrder": 2})));
        daemon.set_session(record(json!({"id": "early", "sidebarOrder": 0})));
        daemon.set_session(record(
            json!({"id": "child", "parentSessionId": "early", "sidebarOrder": 0}),
        ));
        daemon.set_session(record(json!({"id": "other", "repoRoot": "/o", "cwd": "/o"})));
        let ids = |sessions: Vec<Ordered<SessionRecord>>| -> Vec<String> {
            sessions.iter().map(|session| session.id.clone()).collect()
        };
        assert_eq!(ids(daemon.sessions_for_repo("/r")), ["early", "child", "late"]);
        assert_eq!(ids(daemon.sessions_for_repo("/o/")), ["other"]);
        assert_eq!(ids(daemon.handle_list().unwrap()).len(), 4);
    }

    #[test]
    fn reordering_swaps_a_session_with_its_sibling_and_persists() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let _home = Home::set(dir.path());
            let daemon = daemon(dir.path());
            let (client, mut rx) = test_client(1);
            daemon.get_client(&client, |subscription| subscription.repo_root = Some("/r".into()));
            daemon.set_session(record(
                json!({"id": "a", "sidebarOrder": 5, "createdAt": "2026-01-01T00:00:01.000Z"}),
            ));
            daemon.set_session(record(
                json!({"id": "b", "sidebarOrder": 7, "createdAt": "2026-01-01T00:00:02.000Z"}),
            ));
            daemon.set_session(record(
                json!({"id": "c", "sidebarOrder": 9, "createdAt": "2026-01-01T00:00:03.000Z"}),
            ));
            daemon.set_session(record(json!({"id": "kid", "parentSessionId": "a"})));

            let listed = daemon.reorder_session("c", MoveDirection::Up).await.unwrap();
            let order: Vec<(String, Option<f64>)> = listed
                .iter()
                .map(|session| (session.id.clone(), session.sidebar_order.map(|order| order.0)))
                .collect();
            assert_eq!(
                order,
                [
                    ("a".to_owned(), Some(0.0)),
                    ("kid".to_owned(), None),
                    ("c".to_owned(), Some(1.0)),
                    ("b".to_owned(), Some(2.0))
                ]
            );
            let a = daemon.require_session("a").unwrap();
            assert_eq!(a.updated_at, "2026-01-01T00:00:00.000Z");
            assert_ne!(daemon.require_session("c").unwrap().updated_at, a.updated_at);
            let mut sent = Vec::new();
            while let Ok(line) = rx.try_recv() {
                let message: Value = serde_json::from_str(&line).unwrap();
                sent.push(message["session"]["id"].as_str().unwrap().to_owned());
            }
            assert_eq!(sent, ["c", "b"]);
            let state: Value =
                serde_json::from_str(&std::fs::read_to_string(deckhand_sys::paths::state_path()).unwrap()).unwrap();
            assert_eq!(state["sessions"].as_array().unwrap().len(), 4);

            // At an end, or unknown: nothing moves.
            let again = daemon.reorder_session("a", MoveDirection::Up).await.unwrap();
            assert_eq!(again.len(), 4);
            assert_eq!(daemon.require_session("a").unwrap().sidebar_order, Some(JsNumber(0.0)));
            assert_eq!(
                error(daemon.reorder_session("zzz", MoveDirection::Down).await),
                "session does not exist"
            );
        });
    }

    #[test]
    fn repositories_match_by_path_or_git_common_dir() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let daemon = daemon(dir.path());
            let main = dir.path().join("main");
            repo(&main);
            let linked = dir.path().join("linked");
            git(
                &main,
                &["worktree", "add", "-q", "-b", "feature", linked.to_str().unwrap()],
            );
            let other = dir.path().join("other");
            repo(&other);
            let main = main.to_str().unwrap();
            assert!(daemon.same_repository("/nowhere", "/nowhere").await.unwrap());
            assert!(daemon.same_repository(main, linked.to_str().unwrap()).await.unwrap());
            assert!(!daemon.same_repository(main, other.to_str().unwrap()).await.unwrap());
            let outside = dir.path().join("plain");
            std::fs::create_dir(&outside).unwrap();
            assert!(daemon.same_repository(main, outside.to_str().unwrap()).await.is_err());
        });
    }

    #[test]
    fn creating_a_session_validates_before_recording_it() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let daemon = daemon(dir.path());
            // The agent is resolved after validation; a cached name keeps the test off PATH.
            daemon
                .launch
                .borrow_mut()
                .program_command_cache
                .insert(deckhand_protocol::data::ProgramKey::Claude, "claude".into());
            let parent_id = "11111111-2222-4333-8444-555555555555";
            put(
                &daemon,
                record(json!({"id": "p", "title": "Parent", "status": "running",
                    "agentSessionRef": {"provider": "claude", "kind": "id", "value": parent_id}})),
            );
            put(
                &daemon,
                record(json!({"id": "bare", "title": "Bare", "program": "codex", "status": "running"})),
            );
            put(
                &daemon,
                record(json!({"id": "busy", "title": "Busy", "status": "running"})),
            );
            put(
                &daemon,
                record(
                    json!({"id": "cx", "title": "Cx", "program": "codex", "status": "running",
                    "agentSessionRef": {"provider": "codex", "kind": "id", "value": parent_id}}),
                ),
            );
            let cases = [
                (
                    json!({"handoffFromSessionId": "p", "subSessionKind": "forked"}),
                    "Handoffs require a clean session",
                ),
                (json!({"title": "  \t "}), "title cannot be empty"),
                (
                    json!({"title": "x".repeat(65)}),
                    "title cannot be longer than 64 characters",
                ),
                (
                    json!({"parentSessionId": "gone"}),
                    "parent session does not exist in this repo",
                ),
                (
                    json!({"subSessionKind": "forked"}),
                    "forked sub-session requires a parent session",
                ),
                (
                    json!({"parentSessionId": "p", "subSessionKind": "forked", "program": "codex"}),
                    "forked sub-session must use the parent session program",
                ),
                (
                    json!({"parentSessionId": "bare", "subSessionKind": "forked", "program": "codex"}),
                    "Codex has not reported the parent's conversation ID yet (it comes from the SessionStart hook or the exit hint), so it cannot be forked",
                ),
                (
                    json!({"parentSessionId": "cx", "subSessionKind": "forked", "program": "codex", "cwd": "/r/sub"}),
                    "Codex forks stay in the parent's worktree; create a clean sub-session to work in another one",
                ),
                (
                    json!({"parentSessionId": "cx", "subSessionKind": "forked", "program": "codex", "worktreeMode": "new"}),
                    "Codex forks stay in the parent's worktree; create a clean sub-session to work in another one",
                ),
                (
                    json!({"title": " Busy "}),
                    "an active session named \"Busy\" already exists in this repo",
                ),
                (
                    json!({"parentSessionId": "p", "title": "Parent"}),
                    "an active session named \"Parent\" already exists in this repo",
                ),
                (
                    json!({"baseBranch": "main"}),
                    "A base branch applies only to a new worktree",
                ),
                (
                    json!({"baseBranch": "main", "worktreeMode": "existing"}),
                    "A base branch applies only to a new worktree",
                ),
                (json!({"handoffFromSessionId": "gone"}), "Session does not exist"),
                (
                    json!({"taskId": "t1", "parentSessionId": "p"}),
                    "Only a new top-level session can start from a task",
                ),
            ];
            for (fields, expected) in cases {
                assert_eq!(
                    error(daemon.create_session(input(fields.clone())).await),
                    expected,
                    "{fields}"
                );
            }
            assert_eq!(daemon.all_sessions().len(), 4);
            assert!(daemon.sessions.borrow().preparing.is_empty());
        });
    }

    #[test]
    fn restarting_refuses_what_it_cannot_relaunch() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let daemon = daemon(dir.path());
            let restart = |id: &'static str, mode: RestartMode| {
                let daemon = daemon.clone();
                async move { error(daemon.restart_session(id, 80, 24, Some(mode), None).await) }
            };
            let resume = RestartMode::Resume;
            assert_eq!(restart("gone", resume).await, "session does not exist");
            put(&daemon, record(json!({"id": "running", "status": "running"})));
            assert_eq!(restart("running", resume).await, "session is already running");
            put(&daemon, record(json!({"id": "prep"})));
            daemon.sessions.borrow_mut().preparing.insert("prep".into());
            assert_eq!(
                restart("prep", resume).await,
                "Worktree preparation is still finishing; wait before retrying"
            );
            put(
                &daemon,
                record(
                    json!({"id": "deleted", "worktree": {"mode": "managed", "path": "/wt", "deletedAt": "2026-01-01T00:00:00.000Z"}}),
                ),
            );
            assert_eq!(
                restart("deleted", resume).await,
                "cannot restart session because its worktree was deleted"
            );
            put(
                &daemon,
                record(
                    json!({"id": "unprepared", "program": "codex", "launchId": "l1", "requestedWorktreeMode": "new", "worktree": {"mode": "none"}}),
                ),
            );
            assert_eq!(
                restart("unprepared", resume).await,
                "Worktree preparation did not complete. Create a new session to retry instead of launching in the original checkout."
            );
            put(
                &daemon,
                record(
                    json!({"id": "noref", "program": "codex", "launchId": "l1", "agentStartedAt": "2026-01-01T00:00:00.000Z"}),
                ),
            );
            assert_eq!(
                restart("noref", resume).await,
                "Codex conversation ID is unknown. Use S for a fresh session, or enable trusted Codex hooks before starting new sessions."
            );
            put(
                &daemon,
                record(
                    json!({"id": "missing", "program": "codex", "launchId": "l1", "agentStartedAt": "2026-01-01T00:00:00.000Z",
                    "agentSessionRef": {"provider": "codex", "kind": "id", "value": "c1"}}),
                ),
            );
            daemon
                .launch
                .borrow_mut()
                .program_command_cache
                .insert(deckhand_protocol::data::ProgramKey::Codex, "codex".into());
            assert_eq!(
                restart("missing", RestartMode::Fresh).await,
                "Codex (codex) is not installed or not on PATH; install it (deckhand setup) and press s again"
            );
            // Nothing was recorded.
            let untouched = daemon.require_session("missing").unwrap();
            assert_eq!(untouched.status, SessionStatus::Exited);
            assert_eq!(untouched.launch_id.as_deref(), Some("l1"));
        });
    }

    #[test]
    fn requests_on_missing_or_wrong_state_sessions_fail_with_the_ts_text() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let daemon = daemon(dir.path());
            put(&daemon, record(json!({"id": "running", "status": "running"})));
            put(&daemon, record(json!({"id": "done"})));
            let request = |id: &str| SessionRequest {
                request_id: "r".into(),
                session_id: id.into(),
            };
            assert_eq!(
                error(daemon.kill_session("gone", KillOptions::default()).await),
                "Session does not exist"
            );
            assert_eq!(
                error(daemon.kill_session("running", KillOptions::default()).await),
                "session is not running"
            );
            assert_eq!(error(daemon.remove_session("gone").await), "session does not exist");
            assert_eq!(
                error(daemon.remove_session("running").await),
                "kill the session before removing it"
            );
            assert_eq!(
                error(daemon.handle_cancel_start(&request("gone")).await),
                "Session does not exist"
            );
            assert_eq!(
                error(daemon.handle_cancel_start(&request("done")).await),
                "Session is not starting"
            );
            let done = SetSessionDone {
                request_id: "r".into(),
                session_id: "gone".into(),
                done: true,
            };
            assert_eq!(
                error(daemon.handle_set_session_done(&done).await),
                "Session does not exist"
            );
            let archive = ArchiveSession {
                request_id: "r".into(),
                session_id: "gone".into(),
                archived: true,
            };
            assert_eq!(
                error(daemon.handle_archive_session(&archive).await),
                "Session does not exist"
            );
            let export = ExportHandoff {
                request_id: "r".into(),
                session_id: "gone".into(),
                include_output: None,
            };
            assert_eq!(
                error(daemon.handle_export_handoff(&export).await),
                "Session does not exist"
            );
            let remove = Remove {
                request_id: "r".into(),
                session_id: "gone".into(),
                move_open_items: None,
            };
            assert_eq!(error(daemon.handle_remove(&remove).await), "session does not exist");
        });
    }

    #[test]
    fn exits_and_failures_of_other_launches_change_nothing() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let daemon = daemon(dir.path());
            put(&daemon, record(json!({"id": "done", "lastPreview": "kept"})));
            put(
                &daemon,
                record(json!({"id": "starting", "status": "starting", "launchId": "now"})),
            );
            put(&daemon, session("running", "/r", "/r"));
            daemon
                .handle_worker_session_exit("done", Some(1), None, "")
                .await
                .unwrap();
            daemon
                .handle_worker_session_exit("gone", Some(1), None, "")
                .await
                .unwrap();
            assert_eq!(
                daemon.require_session("done").unwrap().last_preview.as_deref(),
                Some("kept")
            );
            let failure = DhError::msg("boom");
            daemon
                .fail_starting_session("starting", &failure, Some("earlier"))
                .await
                .unwrap();
            daemon.fail_starting_session("running", &failure, None).await.unwrap();
            daemon.fail_starting_session("gone", &failure, None).await.unwrap();
            assert_eq!(
                daemon.require_session("starting").unwrap().status,
                SessionStatus::Starting
            );
            assert_eq!(
                daemon.require_session("running").unwrap().status,
                SessionStatus::Running
            );
        });
    }

    #[test]
    fn literal_orders_keep_undefined_keys_in_place() {
        let mut base = Ordered::with_order(
            record(json!({"id": "x"})).into_inner(),
            literal_order(&[
                "id",
                "handoffPath",
                "title",
                "program",
                "command",
                "cwd",
                "repoRoot",
                "status",
                "createdAt",
                "updatedAt",
            ]),
        );
        assign(&mut base, "handoffPath");
        base.handoff_path = Some("/h.md".into());
        assign(&mut base, "lastPreview");
        base.last_preview = Some(String::new());
        let text = serde_json::to_string(&base).unwrap();
        assert!(
            text.starts_with(r#"{"id":"x","handoffPath":"/h.md","title":"s","#),
            "{text}"
        );
        assert!(text.ends_with(r#""lastPreview":""}"#), "{text}");

        let running = record(json!({"id": "x", "pid": 4, "agentVersion": "1.0"}));
        let held = record(json!({"id": "x", "status": "starting", "pid": 3, "launchId": "l"}));
        let merged = spread(&running, &held).unwrap();
        assert_eq!(merged.pid, Some(3));
        assert_eq!(merged.agent_version.as_deref(), Some("1.0"));
        assert_eq!(merged.launch_id.as_deref(), Some("l"));
        assert_eq!(merged.status, SessionStatus::Starting);
    }

    /// `{...runningSession, ...current}` where the daemon cleared a key (`{...session, doneAt: undefined}`) while the
    /// worker started the agent: the own key holding `undefined` wins, so the worker's stale value does not come back.
    #[test]
    fn spread_lets_keys_cleared_to_undefined_override_the_base() {
        let running = record(json!({
            "id": "x",
            "status": "running",
            "pid": 4,
            "doneAt": "2026-01-02T00:00:00.000Z",
            "archivedAt": "2026-01-03T00:00:00.000Z",
        }));
        let mut held = record(json!({
            "id": "x",
            "status": "starting",
            "doneAt": "2026-01-02T00:00:00.000Z",
            "archivedAt": "2026-01-03T00:00:00.000Z",
        }));
        assign(&mut held, "doneAt");
        held.done_at = None;
        assign(&mut held, "archivedAt");
        held.archived_at = None;
        let mut merged = spread(&running, &held).unwrap();
        assert_eq!(merged.done_at, None, "doneAt resurrected");
        assert_eq!(merged.archived_at, None, "archivedAt resurrected");
        assert_eq!(merged.pid, Some(4));
        assert_eq!(merged.status, SessionStatus::Starting);

        // The cleared keys stay own keys in base's places: setting one later keeps that position, as in JS.
        merged.done_at = Some("later".into());
        let text = serde_json::to_string(&merged).unwrap();
        let done = text.find(r#""doneAt""#).unwrap();
        let pid = text.find(r#""pid""#).unwrap();
        assert!(done > pid, "{text}");
        assert!(!text.contains("archivedAt"), "{text}");

        // A key only over holds as undefined overrides too, and base's other keys keep their values.
        let based = record(json!({"id": "x", "startPrompt": "go", "agentVersion": "1.0"}));
        let mut cleared = record(json!({"id": "x"}));
        assign(&mut cleared, "startPrompt");
        cleared.start_prompt = None;
        let merged = spread(&based, &cleared).unwrap();
        assert_eq!(merged.start_prompt, None);
        assert_eq!(merged.agent_version.as_deref(), Some("1.0"));

        // An over with no recorded order (built on the Rust side) overrides only with the keys it holds.
        let plain = Ordered::new(record(json!({"id": "x", "status": "starting"})).into_inner());
        let merged = spread(&based, &plain).unwrap();
        assert_eq!(merged.start_prompt.as_deref(), Some("go"));
    }
}
