//! Orchestrator plans and the orchestrator itself (not in the TS; docs/future/orchestrator.md, part 3).
//!
//! - `plan-check` validates a plan against the repository ([`deckhand_core::plan`]) and answers its summary.
//! - `plan-apply` validates it the same way (problems come back at once, nothing is created), then holds it as a
//!   [`PendingPlan`] and tells the repository's UIs (`plan-pending`; a UI that subscribes later is sent the pending
//!   plans of its repository right after its `subscribe` answer). The first `plan-decide` wins (`plan-settled` to
//!   everyone); unanswered, the plan expires after [`PLAN_EXPIRY`]. Approved, its sessions are created in order,
//!   parents first, each child once its parent's worktree is ready (so stacking sees the parent's live record), then
//!   its tasks; the request is answered with what was created. Creation stops at the first failure and keeps what was
//!   created. The confirmation only gates [`Daemon::create_plan`], so dropping it would be a small change.
//! - `open-orchestrator` starts the repository's orchestrator, or types a request into the running one and submits it
//!   ([`Daemon::deliver_prompt`]).

use std::collections::HashMap;
use std::time::Duration;

use deckhand_core::error::{DhError, Result};
use deckhand_core::jsstr::{date, path};
use deckhand_core::plan::{PlanContext, PlanParent, ValidPlan, plan_problems_message, validate_plan};
use deckhand_core::project_config::resolve_settings;
use deckhand_core::session_scope::is_path_inside;
use deckhand_core::tasks::{new_task_id, parse_tasks};
use deckhand_protocol::Ordered;
use deckhand_protocol::data::{
    CreateSessionInput, OkResult, OrchestratorResult, PendingPlan, PlanCreatedSession, PlanCreatedTask, PlanDecision,
    PlanFailure, PlanOutcome, PlanSession, PlanStatus, PlanSummary, PlanWorktree, ProgramKey, SessionRecord,
    SessionRole, SessionStatus, SubSessionKind, TaskOp, WorktreeMode,
};
use deckhand_protocol::events::{PlanPending, PlanSettled, ServerMessage};
use deckhand_protocol::wire::{Loose, OpenOrchestrator, PlanDecide, PlanRequest};
use deckhand_sys::config::load_project_config;
use deckhand_sys::git::{current_branch, find_repo_root, optional_git};
use deckhand_sys::notes_store::{NoteId, NoteKind};
use deckhand_sys::storage::load_app_config;
use indexmap::IndexMap;
use serde_json::Value;
use tokio::sync::oneshot;

use crate::daemon::Daemon;
use crate::server::{ClientHandle, ClientSubscription, DEFAULT_PREVIEW_COLS, DEFAULT_PREVIEW_ROWS, send_message};
use crate::sessions::ORCHESTRATOR_TITLE;
use crate::stacking::is_live_target;

/// How long a plan waits for the user's answer in Deckhand.
pub const PLAN_EXPIRY: Duration = Duration::from_secs(15 * 60);

/// How long creating a plan waits for a new parent's worktree before giving up on its children.
pub const PARENT_PREPARE_WAIT: Duration = Duration::from_secs(120);

/// How often that wait looks again.
const PARENT_PREPARE_POLL: Duration = Duration::from_millis(50);

/// `plan apply` from a session that is not the orchestrator.
pub const NOT_ORCHESTRATOR: &str = "deckhand plan apply runs only in the repository's orchestrator session, or by hand";

/// A plan waiting for its answer.
pub struct PendingEntry {
    pub plan: PendingPlan,
    /// What creating it needs (its parents resolved).
    pub valid: ValidPlan,
    /// Wakes the `plan-apply` that waits for the answer.
    pub answer: Option<oneshot::Sender<PlanDecision>>,
}

/// The plans' part of the daemon state.
pub struct PlansState {
    /// Pending plans by ID, oldest first.
    pub pending: IndexMap<String, PendingEntry>,
    /// [`PLAN_EXPIRY`], shorter in tests.
    pub expiry: Duration,
}

impl Default for PlansState {
    fn default() -> Self {
        PlansState {
            pending: IndexMap::new(),
            expiry: PLAN_EXPIRY,
        }
    }
}

/// `new Date(ms).toISOString()`.
fn iso(ms: i64) -> String {
    date::to_iso_string(ms).unwrap_or_default()
}

/// Whether a plan for `repo_root` concerns a client subscribed to `subscription` (its checkout, or the same
/// repository by key).
fn plan_in_scope(repo_root: &str, subscription: &ClientSubscription, repo_key: Option<&str>) -> bool {
    let Some(root) = subscription.repo_root.as_deref().filter(|root| !root.is_empty()) else {
        return false;
    };
    if path::resolve(root) == path::resolve(repo_root) || is_path_inside(repo_root, root) {
        return true;
    }
    repo_key.is_some() && subscription.repo_key.as_deref() == repo_key
}

/// The input creating plan session `session` sends, under `parent` (an existing or just created session).
pub fn plan_session_input(
    session: &PlanSession,
    parent: Option<&SessionRecord>,
    repo_root: &str,
) -> CreateSessionInput {
    let repo_root = parent.map_or(repo_root, |parent| parent.repo_root.as_str());
    let origin = parent.map_or(repo_root, |parent| parent.cwd.as_str());
    let (mode, cwd) = match session.worktree {
        PlanWorktree::New => (WorktreeMode::New, origin),
        PlanWorktree::Same => (WorktreeMode::None, origin),
        PlanWorktree::None => (WorktreeMode::None, repo_root),
    };
    let new = mode == WorktreeMode::New;
    CreateSessionInput {
        title: session.title.clone(),
        program: session.agent,
        cwd: cwd.to_owned(),
        repo_root: repo_root.to_owned(),
        cols: DEFAULT_PREVIEW_COLS,
        rows: DEFAULT_PREVIEW_ROWS,
        worktree_mode: Some(mode),
        existing_worktree_path: None,
        parent_session_id: parent.map(|parent| parent.id.clone()),
        sub_session_kind: parent.map(|_| SubSessionKind::Clean),
        handoff_from_session_id: None,
        base_branch: session.base.clone().filter(|_| new),
        task_id: session.task.clone(),
        project_fingerprint: None,
        fork_from_session_id: None,
        prompt: Some(session.prompt.clone()),
        submit: None,
        stacked: (new && session.flat == Some(true)).then_some(false),
        role: None,
    }
}

impl Daemon {
    /// The repository a plan request is for, and its authenticated caller (`plan apply` only from the orchestrator,
    /// unless run by hand).
    async fn plan_origin(
        &self,
        request: &PlanRequest,
        apply: bool,
    ) -> Result<(String, Option<Ordered<SessionRecord>>)> {
        if let Some(caller) = &request.caller {
            let caller = self.authenticate_caller(caller)?;
            if apply && caller.role != Some(SessionRole::Orchestrator) {
                return Err(DhError::msg(NOT_ORCHESTRATOR));
            }
            return Ok((caller.repo_root.clone(), Some(caller)));
        }
        let cwd = match &request.cwd {
            Loose::Known(cwd) if !cwd.is_empty() => cwd.clone(),
            _ => return Err(DhError::msg("Invalid repository path")),
        };
        Ok((find_repo_root(Some(&cwd)).await?, None))
    }

    /// Validates `raw` against the repository at `repo_root`: its sessions (any of them may be a parent), its tasks,
    /// its default agent, the branches its sessions stack on and the bases Git knows.
    pub async fn validate_plan_in(&self, repo_root: &str, raw: Option<&Value>) -> Result<ValidPlan> {
        let raw = raw.cloned().unwrap_or(Value::Null);
        let mut sessions: Vec<SessionRecord> = Vec::new();
        let mut same: HashMap<String, bool> = HashMap::new();
        for session in self.all_sessions() {
            let known = same.get(&session.repo_root).copied();
            let in_repo = match known {
                Some(in_repo) => in_repo,
                None => {
                    let in_repo = self
                        .same_repository(&session.repo_root, repo_root)
                        .await
                        .unwrap_or(false);
                    same.insert(session.repo_root.clone(), in_repo);
                    in_repo
                }
            };
            if in_repo {
                sessions.push((*session).clone());
            }
        }
        let key = self.tasks_key_for(repo_root).await?;
        let text = self
            .notes()
            .get(&NoteId {
                kind: NoteKind::Tasks,
                id: key,
            })
            .text;
        let tasks = parse_tasks(&text);
        let config_dir = self.paths.config_dir.to_string_lossy().into_owned();
        let default_agent = match (load_project_config(repo_root, None).await, load_app_config().await) {
            (Ok(project), Ok(config)) => resolve_settings(Some(&project), &config, &config_dir)
                .ok()
                .and_then(|settings| settings.default_agent),
            _ => None,
        };
        // Only what the plan names is looked up in Git.
        let entries: Vec<&Value> = raw
            .get("sessions")
            .and_then(Value::as_array)
            .map(|list| list.iter().collect())
            .unwrap_or_default();
        let named = |field: &str| -> Vec<String> {
            let mut values: Vec<String> = entries
                .iter()
                .filter_map(|entry| entry.get(field).and_then(Value::as_str))
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect();
            values.dedup();
            values
        };
        let parents = named("parent");
        let mut stack_branches = HashMap::new();
        for session in sessions
            .iter()
            .filter(|session| parents.iter().any(|parent| session.id.starts_with(parent.as_str())))
        {
            if let Some(branch) = self.stack_branch_of(session).await {
                stack_branches.insert(session.id.clone(), branch);
            }
        }
        let mut missing_bases = Vec::new();
        for base in named("base") {
            let commit = format!("{base}^{{commit}}");
            if optional_git(repo_root, &["rev-parse", "--verify", "--quiet", &commit], None)
                .await
                .is_none()
            {
                missing_bases.push(base);
            }
        }
        let cx = PlanContext {
            sessions: &sessions,
            tasks: &tasks,
            default_agent,
            stack_branches: &stack_branches,
            missing_bases: &missing_bases,
        };
        validate_plan(&raw, &cx).map_err(|problems| DhError::msg(plan_problems_message(&problems)))
    }

    /// The branch a new worktree under `session` would stack on (as [`Daemon::stack_target`] decides): its live
    /// linked worktree's branch.
    async fn stack_branch_of(&self, session: &SessionRecord) -> Option<String> {
        if session
            .worktree
            .as_ref()
            .is_some_and(|worktree| worktree.is_main == Some(true))
        {
            return None;
        }
        let record = self
            .worktree_record_of(session)
            .filter(|record| is_live_target(record))?;
        current_branch(&record.path)
            .await
            .ok()
            .filter(|branch| !branch.is_empty())
    }

    /// `plan-check`: the plan's summary, or every problem with it.
    pub async fn handle_plan_check(&self, request: &PlanRequest) -> Result<PlanSummary> {
        let (repo_root, _) = self.plan_origin(request, false).await?;
        Ok(self.validate_plan_in(&repo_root, request.plan.as_ref()).await?.summary)
    }

    /// `plan-apply`: validates, waits for the user's answer in Deckhand (or the expiry), then creates the plan.
    pub async fn handle_plan_apply(&self, request: &PlanRequest) -> Result<PlanOutcome> {
        let (repo_root, caller) = self.plan_origin(request, true).await?;
        let valid = self.validate_plan_in(&repo_root, request.plan.as_ref()).await?;
        let id = uuid::Uuid::new_v4().to_string();
        let expiry = self.plans.borrow().expiry;
        let now = date::now_ms();
        let pending = PendingPlan {
            id: id.clone(),
            repo_root: repo_root.clone(),
            caller_session_id: caller.as_ref().map(|caller| caller.id.clone()),
            caller_title: caller.as_ref().map(|caller| self.session_path(caller)),
            plan: valid.plan.clone(),
            summary: valid.summary.clone(),
            created_at: iso(now),
            expires_at: iso(now + i64::try_from(expiry.as_millis()).unwrap_or(i64::MAX)),
        };
        let (answer, mut answered) = oneshot::channel();
        self.plans.borrow_mut().pending.insert(
            id.clone(),
            PendingEntry {
                plan: pending.clone(),
                valid: valid.clone(),
                answer: Some(answer),
            },
        );
        self.log(&format!("plan {id} waits for an answer ({})", pending.summary.counts))
            .await;
        self.broadcast_plan(
            &repo_root,
            &ServerMessage::PlanPending(Box::new(PlanPending { plan: pending })),
        )
        .await;
        let decision = match tokio::time::timeout(expiry, &mut answered).await {
            Ok(Ok(decision)) => decision,
            // The sender went with the entry (the daemon is going away): nothing is created.
            Ok(Err(_)) => PlanDecision::Rejected,
            Err(_) => {
                let removed = self.plans.borrow_mut().pending.shift_remove(&id);
                if removed.is_some() {
                    self.settle_plan(&repo_root, &id, PlanDecision::Expired).await;
                    PlanDecision::Expired
                } else {
                    // Answered as it expired: the answer is in.
                    answered.try_recv().unwrap_or(PlanDecision::Expired)
                }
            }
        };
        let outcome = |status| PlanOutcome {
            plan_id: id.clone(),
            status,
            sessions: Vec::new(),
            tasks: Vec::new(),
            failed: None,
        };
        match decision {
            PlanDecision::Approved => {
                let caller_id = caller.as_ref().map(|caller| caller.id.clone());
                Ok(self.create_plan(&id, &repo_root, &valid, caller_id).await)
            }
            PlanDecision::Rejected => Ok(outcome(PlanStatus::Cancelled)),
            PlanDecision::Expired => Ok(outcome(PlanStatus::Expired)),
        }
    }

    /// `plan-decide`: the first answer to a pending plan wins; a later one (or one for an expired plan) is refused.
    pub async fn handle_plan_decide(&self, request: &PlanDecide) -> Result<OkResult> {
        let removed = self.plans.borrow_mut().pending.shift_remove(&request.plan_id);
        let Some(mut entry) = removed else {
            return Err(DhError::msg(format!(
                "No plan {} is waiting for an answer (it was answered already, or it expired)",
                request.plan_id
            )));
        };
        let decision = if request.approve == Some(true) {
            PlanDecision::Approved
        } else {
            PlanDecision::Rejected
        };
        if let Some(answer) = entry.answer.take() {
            let _ = answer.send(decision);
        }
        self.settle_plan(&entry.plan.repo_root, &request.plan_id, decision)
            .await;
        Ok(OkResult::default())
    }

    /// Tells the repository's clients that plan `id` was answered or expired.
    async fn settle_plan(&self, repo_root: &str, id: &str, decision: PlanDecision) {
        self.log(&format!("plan {id} {decision}")).await;
        let message = ServerMessage::PlanSettled(PlanSettled {
            plan_id: id.to_owned(),
            decision,
        });
        self.broadcast_plan(repo_root, &message).await;
    }

    /// Sends `message` to every client subscribed to the repository at `repo_root`.
    async fn broadcast_plan(&self, repo_root: &str, message: &ServerMessage) {
        let root = path::resolve(repo_root);
        self.learn_repo_key(&root, false).await;
        let key = self.server.borrow().repo_keys.get(&root).cloned();
        let clients: Vec<(ClientHandle, ClientSubscription)> = self
            .server
            .borrow()
            .clients
            .values()
            .map(|client| (client.handle.clone(), client.subscription.clone()))
            .collect();
        for (handle, subscription) in clients {
            if plan_in_scope(&root, &subscription, key.as_deref()) {
                send_message(&handle, message);
            }
        }
    }

    /// After a `subscribe` answer: the pending plans of the client's repository, as `plan-pending` events, so a UI
    /// that opens while a plan waits shows it.
    pub async fn send_pending_plans(&self, client: &ClientHandle) {
        let plans: Vec<PendingPlan> = self
            .plans
            .borrow()
            .pending
            .values()
            .map(|entry| entry.plan.clone())
            .collect();
        for plan in plans {
            let root = path::resolve(&plan.repo_root);
            self.learn_repo_key(&root, false).await;
            let key = self.server.borrow().repo_keys.get(&root).cloned();
            let subscription = self
                .server
                .borrow()
                .clients
                .get(&client.id)
                .map(|entry| entry.subscription.clone());
            if subscription.is_some_and(|subscription| plan_in_scope(&root, &subscription, key.as_deref())) {
                send_message(client, &ServerMessage::PlanPending(Box::new(PlanPending { plan })));
            }
        }
    }

    /// Creates an approved plan: its sessions in order (each child once its parent is ready), then its tasks. Stops at
    /// the first failure; what was created stays.
    pub async fn create_plan(
        &self,
        id: &str,
        repo_root: &str,
        valid: &ValidPlan,
        spawned_by: Option<String>,
    ) -> PlanOutcome {
        let mut outcome = PlanOutcome {
            plan_id: id.to_owned(),
            status: PlanStatus::Created,
            sessions: Vec::new(),
            tasks: Vec::new(),
            failed: None,
        };
        for (index, session) in valid.plan.sessions.iter().enumerate() {
            let parent = match valid.parents.get(index).cloned().flatten() {
                None => None,
                Some(PlanParent::Entry(parent)) => outcome
                    .sessions
                    .get(parent)
                    .map(|created| (created.session.id.clone(), true)),
                Some(PlanParent::Session(parent)) => Some((parent, false)),
            };
            let created = async {
                let parent = match parent {
                    Some((parent, created_here)) => Some(self.wait_prepared(&parent, created_here).await?),
                    None => None,
                };
                let input = plan_session_input(session, parent.as_deref(), repo_root);
                self.create_session_spawned(input, spawned_by.clone()).await
            }
            .await;
            match created {
                Ok(record) => outcome.sessions.push(PlanCreatedSession {
                    key: session.key.clone(),
                    session: record,
                }),
                Err(error) => {
                    outcome.status = PlanStatus::Failed;
                    outcome.failed = Some(PlanFailure {
                        key: Some(session.key.clone()),
                        title: session.title.clone(),
                        error: error.to_string(),
                    });
                    self.log(&format!("plan {id} stopped at {}: {error}", session.key))
                        .await;
                    return outcome;
                }
            }
        }
        if !valid.plan.tasks.is_empty() {
            let key = match self.tasks_key_for(repo_root).await {
                Ok(key) => key,
                Err(error) => {
                    outcome.status = PlanStatus::Failed;
                    outcome.failed = valid.plan.tasks.first().map(|task| PlanFailure {
                        key: None,
                        title: task.title.clone(),
                        error: error.to_string(),
                    });
                    return outcome;
                }
            };
            for task in &valid.plan.tasks {
                let task_id = new_task_id();
                let op = TaskOp::Add {
                    title: task.title.clone(),
                    body: task.details.clone(),
                    link: None,
                    id: Some(task_id.clone()),
                    from: None,
                    parent: None,
                    at: None,
                };
                match self.change_tasks(&key, op).await {
                    Ok(_) => outcome.tasks.push(PlanCreatedTask {
                        id: task_id,
                        title: task.title.clone(),
                    }),
                    Err(error) => {
                        outcome.status = PlanStatus::Failed;
                        outcome.failed = Some(PlanFailure {
                            key: None,
                            title: task.title.clone(),
                            error: error.to_string(),
                        });
                        return outcome;
                    }
                }
            }
        }
        // As held now (a new worktree's path and branch may be known by now).
        for created in &mut outcome.sessions {
            if let Some(current) = self.get_session(&created.session.id) {
                created.session = current;
            }
        }
        self.log(&format!(
            "plan {id} created {} sessions and {} tasks",
            outcome.sessions.len(),
            outcome.tasks.len()
        ))
        .await;
        outcome
    }

    /// `parent` once it is no longer being prepared: its worktree (if any) exists and is recorded, so a child can
    /// start in it or stack on it. A parent the plan just created must have started.
    async fn wait_prepared(&self, parent: &str, created_here: bool) -> Result<Ordered<SessionRecord>> {
        let deadline = tokio::time::Instant::now() + PARENT_PREPARE_WAIT;
        loop {
            let preparing = self.sessions.borrow().preparing.contains(parent);
            let Some(session) = self.get_session(parent) else {
                return Err(DhError::msg("its parent session no longer exists"));
            };
            if !preparing {
                if created_here && session.status == SessionStatus::Exited {
                    let why = session
                        .last_preview
                        .as_deref()
                        .map(str::trim)
                        .filter(|preview| !preview.is_empty())
                        .unwrap_or("it exited");
                    return Err(DhError::msg(format!(
                        "its parent {} did not start: {why}",
                        self.session_path(&session)
                    )));
                }
                return Ok(session);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(DhError::msg(format!(
                    "its parent {} was still preparing its worktree",
                    self.session_path(&session)
                )));
            }
            tokio::time::sleep(PARENT_PREPARE_POLL).await;
        }
    }

    /// `open-orchestrator`: the repository's running orchestrator (given `prompt`, typed and submitted), or a new one
    /// (`prompt` its first message).
    pub async fn handle_open_orchestrator(&self, request: &OpenOrchestrator) -> Result<OrchestratorResult> {
        let cwd = match &request.cwd {
            Loose::Known(cwd) if !cwd.is_empty() => cwd.clone(),
            _ => return Err(DhError::msg("Invalid repository path")),
        };
        let repo_root = find_repo_root(Some(&cwd)).await?;
        let prompt = request.prompt.clone().filter(|prompt| !prompt.trim().is_empty());
        if let Some(running) = self.running_orchestrator(&repo_root).await {
            let delivered = match prompt.as_deref() {
                Some(prompt) => {
                    self.deliver_prompt(&running.id, prompt).await?;
                    true
                }
                None => false,
            };
            return Ok(OrchestratorResult {
                session: self.get_session(&running.id).unwrap_or(running),
                started: false,
                delivered,
            });
        }
        let input = CreateSessionInput {
            title: ORCHESTRATOR_TITLE.to_owned(),
            program: ProgramKey::Claude,
            cwd: repo_root.clone(),
            repo_root,
            cols: request.cols.unwrap_or(DEFAULT_PREVIEW_COLS),
            rows: request.rows.unwrap_or(DEFAULT_PREVIEW_ROWS),
            worktree_mode: Some(WorktreeMode::None),
            existing_worktree_path: None,
            parent_session_id: None,
            sub_session_kind: None,
            handoff_from_session_id: None,
            base_branch: None,
            task_id: None,
            project_fingerprint: None,
            fork_from_session_id: None,
            prompt,
            submit: None,
            stacked: None,
            role: Some(SessionRole::Orchestrator),
        };
        let session = self.create_session(input).await?;
        Ok(OrchestratorResult {
            session,
            started: true,
            delivered: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::tests::Home;
    use crate::server::tests::{local, session, test_client, test_daemon};
    use crate::server::{Client, ClientId};
    use deckhand_protocol::wire::AgentCaller;
    use serde_json::json;
    use std::path::Path;
    use tokio::sync::mpsc;

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

    /// A repository with one commit on main, at its canonical path.
    fn repo(dir: &Path) -> String {
        std::fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("README.md"), "hi\n").unwrap();
        git(dir, &["add", "README.md"]);
        git(dir, &["commit", "-q", "-m", "init"]);
        std::fs::canonicalize(dir).unwrap().to_string_lossy().into_owned()
    }

    /// A client subscribed to `root`.
    fn subscribe(daemon: &Daemon, id: u64, root: &str) -> (ClientHandle, mpsc::UnboundedReceiver<String>) {
        let (handle, rx) = test_client(id);
        daemon.server.borrow_mut().clients.insert(
            ClientId(id),
            Client {
                handle: handle.clone(),
                subscription: ClientSubscription {
                    repo_root: Some(root.to_owned()),
                    ..ClientSubscription::default()
                },
            },
        );
        (handle, rx)
    }

    /// The messages a client got since the last call.
    fn received(rx: &mut mpsc::UnboundedReceiver<String>) -> Vec<Value> {
        let mut found = Vec::new();
        while let Ok(line) = rx.try_recv() {
            found.push(serde_json::from_str(&line).unwrap());
        }
        found
    }

    /// Those of type `kind`.
    fn of(messages: &[Value], kind: &str) -> Vec<Value> {
        messages
            .iter()
            .filter(|message| message["type"] == kind)
            .cloned()
            .collect()
    }

    fn request(root: &str, plan: Value) -> PlanRequest {
        serde_json::from_value(json!({"requestId": "r", "cwd": root, "plan": plan})).unwrap()
    }

    fn decide(plan_id: &str, approve: Value) -> PlanDecide {
        serde_json::from_value(json!({"requestId": "d", "planId": plan_id, "approve": approve})).unwrap()
    }

    /// A plan of tasks only (creating it needs no agent).
    fn tasks_plan() -> Value {
        json!({"tasks": [{"title": "Remove legacy endpoint", "details": "After Auth rework merges"}]})
    }

    /// Runs `apply` until its plan is pending, and answers its ID.
    async fn pending_id(daemon: &Daemon) -> String {
        for _ in 0..200 {
            if let Some(id) = daemon.plans.borrow().pending.keys().next().cloned() {
                return id;
            }
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("no plan became pending");
    }

    #[test]
    fn a_plan_with_problems_is_refused_at_once_and_nothing_waits() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        let daemon = test_daemon(dir.path());
        local(async {
            let plan = json!({"sessions": [{"key": "a", "title": "A"}, {"key": "a", "title": "B", "prompt": "x"}]});
            let error = daemon.handle_plan_apply(&request(&root, plan)).await.unwrap_err();
            assert_eq!(
                error.to_string(),
                "The plan has 2 problems (nothing was created):\n\
                 - sessions[0] (a): prompt is required (the session's complete brief, sent as its first message)\n\
                 - sessions[1] (a): key \"a\" is already used by sessions[0]"
            );
            assert!(daemon.plans.borrow().pending.is_empty());
            // Checking answers the summary without waiting for anyone.
            let summary = daemon.handle_plan_check(&request(&root, tasks_plan())).await.unwrap();
            assert_eq!(summary.counts, "1 task");
            assert!(daemon.plans.borrow().pending.is_empty());
        });
    }

    #[test]
    fn an_approved_plan_adds_its_tasks_and_the_first_answer_wins() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        let daemon = test_daemon(dir.path());
        local(async {
            let (_ui, mut ui_rx) = subscribe(&daemon, 1, &root);
            let (_elsewhere, mut other_rx) = subscribe(&daemon, 2, "/elsewhere");
            let applying = {
                let daemon = daemon.clone();
                let request = request(&root, tasks_plan());
                tokio::task::spawn_local(async move { daemon.handle_plan_apply(&request).await })
            };
            let id = pending_id(&daemon).await;
            let pending = of(&received(&mut ui_rx), "plan-pending");
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0]["plan"]["id"], id.as_str());
            assert_eq!(pending[0]["plan"]["repoRoot"], root.as_str());
            assert_eq!(pending[0]["plan"]["summary"]["counts"], "1 task");
            assert!(pending[0]["plan"].get("callerSessionId").is_none());
            // Another repository's UI hears nothing of it.
            assert!(of(&received(&mut other_rx), "plan-pending").is_empty());

            daemon.handle_plan_decide(&decide(&id, json!(true))).await.unwrap();
            let error = daemon.handle_plan_decide(&decide(&id, json!(false))).await.unwrap_err();
            assert!(
                error.to_string().starts_with(&format!("No plan {id} is waiting")),
                "{error}"
            );
            let outcome = applying.await.unwrap().unwrap();
            assert_eq!(outcome.status, PlanStatus::Created, "{outcome:?}");
            assert_eq!(outcome.tasks.len(), 1);
            assert_eq!(outcome.tasks[0].title, "Remove legacy endpoint");
            let settled = of(&received(&mut ui_rx), "plan-settled");
            assert_eq!(settled.len(), 1);
            assert_eq!(settled[0]["planId"], id.as_str());
            assert_eq!(settled[0]["decision"], "approved");
            // The task is in the backlog, with its details.
            let key = daemon.tasks_key_for(&root).await.unwrap();
            let tasks = parse_tasks(&daemon.tasks_doc(&key).text);
            let added = tasks.iter().find(|task| task.id == outcome.tasks[0].id).unwrap();
            assert_eq!(added.title, "Remove legacy endpoint");
            assert!(added.body.contains("After Auth rework merges"), "{added:?}");
        });
    }

    #[test]
    fn a_rejected_plan_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        let daemon = test_daemon(dir.path());
        local(async {
            let applying = {
                let daemon = daemon.clone();
                let request = request(&root, tasks_plan());
                tokio::task::spawn_local(async move { daemon.handle_plan_apply(&request).await })
            };
            let id = pending_id(&daemon).await;
            // Anything but `true` rejects.
            daemon.handle_plan_decide(&decide(&id, json!("yes"))).await.unwrap();
            let outcome = applying.await.unwrap().unwrap();
            assert_eq!(outcome.status, PlanStatus::Cancelled);
            assert!(outcome.tasks.is_empty() && outcome.sessions.is_empty());
            let key = daemon.tasks_key_for(&root).await.unwrap();
            assert!(parse_tasks(&daemon.tasks_doc(&key).text).is_empty());
        });
    }

    #[test]
    fn an_unanswered_plan_expires() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        let daemon = test_daemon(dir.path());
        daemon.plans.borrow_mut().expiry = Duration::from_millis(200);
        local(async {
            let (_ui, mut ui_rx) = subscribe(&daemon, 1, &root);
            let outcome = daemon.handle_plan_apply(&request(&root, tasks_plan())).await.unwrap();
            assert_eq!(outcome.status, PlanStatus::Expired);
            assert!(daemon.plans.borrow().pending.is_empty());
            let messages = received(&mut ui_rx);
            let pending = of(&messages, "plan-pending");
            assert_eq!(pending.len(), 1);
            let expires = pending[0]["plan"]["expiresAt"].as_str().unwrap().to_owned();
            assert!(expires.as_str() > pending[0]["plan"]["createdAt"].as_str().unwrap());
            // The UI was told, so its confirmation closes; a late answer is refused.
            let settled = of(&messages, "plan-settled");
            assert_eq!(settled.len(), 1);
            assert_eq!(settled[0]["decision"], "expired");
            let error = daemon
                .handle_plan_decide(&decide(&outcome.plan_id, json!(true)))
                .await
                .unwrap_err();
            assert!(error.to_string().contains("expired"), "{error}");
        });
    }

    #[test]
    fn a_ui_that_subscribes_late_is_sent_the_pending_plan() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        let daemon = test_daemon(dir.path());
        local(async {
            let applying = {
                let daemon = daemon.clone();
                let request = request(&root, tasks_plan());
                tokio::task::spawn_local(async move { daemon.handle_plan_apply(&request).await })
            };
            let id = pending_id(&daemon).await;
            let (late, mut late_rx) = subscribe(&daemon, 3, &root);
            daemon.send_pending_plans(&late).await;
            let pending = of(&received(&mut late_rx), "plan-pending");
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0]["plan"]["id"], id.as_str());
            // A UI of another repository is not.
            let (other, mut other_rx) = subscribe(&daemon, 4, "/elsewhere");
            daemon.send_pending_plans(&other).await;
            assert!(of(&received(&mut other_rx), "plan-pending").is_empty());
            daemon.handle_plan_decide(&decide(&id, json!(false))).await.unwrap();
            assert_eq!(applying.await.unwrap().unwrap().status, PlanStatus::Cancelled);
        });
    }

    #[test]
    fn only_the_orchestrator_applies_a_plan_from_inside_a_session() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        local(async {
            let (daemon, _worker, _fake) = crate::hooks::tests::setup(dir.path()).await;
            daemon.patch_session("s", |session| {
                session.repo_root.clone_from(&root);
                session.cwd.clone_from(&root);
            });
            let as_caller = |token: &str| {
                let mut request = request("/not-used", tasks_plan());
                request.caller = Some(AgentCaller {
                    session_id: "s".into(),
                    launch_id: "launch-1".into(),
                    token: token.into(),
                });
                request
            };
            // Unauthenticated callers are refused first, as for every deckhand command.
            let error = daemon.handle_plan_apply(&as_caller("other")).await.unwrap_err();
            assert_eq!(error.to_string(), crate::hooks::STALE_CALLER);
            let error = daemon.handle_plan_apply(&as_caller("token")).await.unwrap_err();
            assert_eq!(error.to_string(), NOT_ORCHESTRATOR);
            // Any session may check a plan, for its own repository.
            let summary = daemon.handle_plan_check(&as_caller("token")).await.unwrap();
            assert_eq!(summary.counts, "1 task");
            daemon.patch_session("s", |session| session.role = Some(SessionRole::Orchestrator));
            let applying = {
                let daemon = daemon.clone();
                let request = as_caller("token");
                tokio::task::spawn_local(async move { daemon.handle_plan_apply(&request).await })
            };
            let id = pending_id(&daemon).await;
            let pending = daemon.plans.borrow().pending[&id].plan.clone();
            assert_eq!(pending.caller_session_id.as_deref(), Some("s"));
            assert_eq!(pending.repo_root, root);
            daemon.handle_plan_decide(&decide(&id, json!(false))).await.unwrap();
            assert_eq!(applying.await.unwrap().unwrap().status, PlanStatus::Cancelled);
        });
    }

    #[test]
    fn a_plan_session_is_created_from_its_parent_s_directory() {
        let plan_session = |worktree: PlanWorktree| PlanSession {
            key: "k".into(),
            title: "Tests".into(),
            prompt: "Write them".into(),
            agent: ProgramKey::Codex,
            parent: Some("p".into()),
            worktree,
            base: Some("main".into()),
            flat: Some(true),
            task: Some("t1".into()),
        };
        let mut parent = (*session("p", "/repo", "/wt/p")).clone();
        parent.id = "p".into();
        let new = plan_session_input(&plan_session(PlanWorktree::New), Some(&parent), "/elsewhere");
        assert_eq!(
            (new.cwd.as_str(), new.repo_root.as_str(), new.worktree_mode),
            ("/wt/p", "/repo", Some(WorktreeMode::New))
        );
        assert_eq!(new.parent_session_id.as_deref(), Some("p"));
        assert_eq!(new.sub_session_kind, Some(SubSessionKind::Clean));
        assert_eq!((new.base_branch.as_deref(), new.stacked), (Some("main"), Some(false)));
        assert_eq!((new.prompt.as_deref(), new.submit), (Some("Write them"), None));
        assert_eq!(new.task_id.as_deref(), Some("t1"));
        let same = plan_session_input(&plan_session(PlanWorktree::Same), Some(&parent), "/repo");
        assert_eq!(
            (same.cwd.as_str(), same.worktree_mode, same.base_branch, same.stacked),
            ("/wt/p", Some(WorktreeMode::None), None, None)
        );
        let none = plan_session_input(&plan_session(PlanWorktree::None), Some(&parent), "/repo");
        assert_eq!(
            (none.cwd.as_str(), none.worktree_mode),
            ("/repo", Some(WorktreeMode::None))
        );
        let top = plan_session_input(&plan_session(PlanWorktree::New), None, "/repo");
        assert_eq!(
            (top.cwd.as_str(), top.parent_session_id, top.sub_session_kind),
            ("/repo", None, None)
        );
    }
}
