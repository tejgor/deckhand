//! Orchestrator plans and the orchestrator itself (not in the TS; docs/future/orchestrator.md, part 3).
//!
//! - `plan-check` validates a plan against the repository ([`deckhand_core::plan`]) and answers its summary.
//! - `plan-apply` validates it the same way (problems come back at once, nothing is created), then holds it as a
//!   [`PendingPlan`], tells the repository's UIs (`plan-pending`; a UI that subscribes later is sent the pending plans
//!   of its repository right after its `subscribe` answer) and answers at once ([`PlanWaiting`]): nothing blocks on the
//!   user. `plan-park` puts a plan aside (it keeps waiting, without opening by itself). The first `plan-decide` wins
//!   (`plan-settled` to everyone). Approved, its sessions are created in order, parents first, each child once its
//!   parent's worktree is ready (so stacking sees the parent's live record), then its tasks; creation stops at the
//!   first failure and keeps what was created. The confirmation only gates [`Daemon::create_plan`].
//! - **The answer is a message.** The orchestrator that applied the plan is told through its message queue (a notice,
//!   delivered once it is idle): what was created ([`approved_text`]), or that the user cancelled it, with their note
//!   ([`cancelled_text`]). A plan applied by hand has nobody to tell.
//! - **Lifetime.** A plan waits until it is answered, or its orchestrator exits (it is told when it runs again,
//!   [`dropped_text`]) or is removed. Pending plans are kept in `plans.json` in the state directory, as `messages.json`
//!   keeps held messages, so they wait through a daemon restart (which leaves every session exited, not exiting: an
//!   orchestrator's plan stays, and the answer waits in its queue until it is resumed).
//! - An orchestrator's plan belongs to its idea (docs/future/orchestrator.md, 3b): its entries without a parent become
//!   the orchestrator's sub-sessions (their lines go under the idea's line, the orchestrator's), its tasks lines under
//!   the idea's line, and its `brief` the idea's line's brief. Run by hand, they are top-level and go to the backlog.
//! - `open-orchestrator` starts a new orchestrator for an idea, always: titled by its name (else after the idea), the
//!   idea its first message, on a new top-level line of the same title (the idea's line). Several run at once.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use deckhand_core::error::{DhError, Result};
use deckhand_core::jsstr::{date, path};
use deckhand_core::plan::{
    PlanContext, PlanParent, ValidPlan, approved_text, cancelled_text, dropped_text, numbered_title,
    orchestrator_title, plan_problems_message, validate_plan,
};
use deckhand_core::project_config::resolve_settings;
use deckhand_core::session_scope::is_path_inside;
use deckhand_core::tasks::new_task_id;
use deckhand_protocol::Ordered;
use deckhand_protocol::data::{
    CreateSessionInput, OkResult, OrchestratorResult, PendingPlan, PlanCreatedSession, PlanCreatedTask, PlanDecision,
    PlanFailure, PlanOutcome, PlanSession, PlanStatus, PlanSummary, PlanWaiting, PlanWorktree, ProgramKey,
    SessionRecord, SessionRole, SessionStatus, SubSessionKind, WorktreeMode,
};
use deckhand_protocol::events::{PlanPending, PlanSettled, ServerMessage};
use deckhand_protocol::wire::{Loose, OpenOrchestrator, PlanDecide, PlanPark, PlanRequest};
use deckhand_sys::async_util::{Coalescer, SharedTask, spawn};
use deckhand_sys::config::load_project_config;
use deckhand_sys::git::{current_branch, find_repo_root, optional_git};
use deckhand_sys::storage::{self, load_app_config};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::daemon::Daemon;
use crate::server::{ClientHandle, ClientSubscription, DEFAULT_PREVIEW_COLS, DEFAULT_PREVIEW_ROWS, send_message};
use crate::stacking::is_live_target;

/// The file in the state directory that keeps the pending plans.
pub const PLANS_FILE: &str = "plans.json";

/// How long creating a plan waits for a new parent's worktree before giving up on its children.
pub const PARENT_PREPARE_WAIT: Duration = Duration::from_secs(120);

/// How long telling the orchestrator what was created waits for the new sessions' worktrees (so it can name their
/// branches).
pub const CREATED_SETTLE_WAIT: Duration = Duration::from_secs(20);

/// How often those waits look again.
const PARENT_PREPARE_POLL: Duration = Duration::from_millis(50);

/// `plan apply` from a session that is not an orchestrator.
pub const NOT_ORCHESTRATOR: &str = "deckhand plan apply runs only in an orchestrator session, or by hand";

/// `open-orchestrator` without an idea.
pub const NO_IDEA: &str = "Say what you want to do: the idea is the new orchestrator's first message";

/// A plan waiting for its answer.
pub struct PendingEntry {
    pub plan: PendingPlan,
    /// What creating it needs (its parents resolved).
    pub valid: ValidPlan,
}

impl PendingEntry {
    fn of(plan: PendingPlan) -> PendingEntry {
        let valid = ValidPlan::from_validated(plan.plan.clone(), plan.summary.clone());
        PendingEntry { plan, valid }
    }
}

/// The plans' part of the daemon state.
#[derive(Default)]
pub struct PlansState {
    /// Pending plans by ID, oldest first.
    pub pending: IndexMap<String, PendingEntry>,
    /// Records of `plans.json` another version wrote that this one cannot read: written back as they were.
    pub foreign: Vec<Value>,
    /// The file's other keys.
    pub file_extra: IndexMap<String, Value>,
    /// The coalesced write of `plans.json` (made by the first save).
    pub save: Option<Coalescer<Result<()>>>,
}

/// `plans.json`.
#[derive(Default, Serialize, Deserialize)]
struct PlansFile {
    #[serde(default)]
    plans: Vec<Value>,
    #[serde(flatten)]
    extra: IndexMap<String, Value>,
}

/// The message for a plan that is no longer pending.
fn not_waiting(plan_id: &str) -> DhError {
    DhError::msg(format!(
        "No plan {plan_id} is waiting for an answer (it was answered already, or its orchestrator ended)"
    ))
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

/// What a plan session under `parent` is told after its brief: to send its parent an update once it has finished (a
/// sub-session's parent is the session it was planned under, a top-level entry's the orchestrator).
pub fn report_to_parent(parent: &SessionRecord) -> String {
    let name = if parent.role == Some(SessionRole::Orchestrator) {
        format!("◎ {}", parent.title)
    } else {
        parent.title.clone()
    };
    format!(
        "When you have finished this task, send your parent session, \"{name}\", an update (what you did, and anything \
         left open): `deckhand send {} \"<update>\"`.",
        parent.id
    )
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
        prompt: Some(match parent {
            Some(parent) => format!("{}\n\n{}", session.prompt.trim_end(), report_to_parent(parent)),
            None => session.prompt.clone(),
        }),
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
    /// its default agent, the branches its sessions stack on and the bases Git knows. `caller`: the session checking
    /// or applying it; an orchestrator's plan belongs to its idea.
    pub async fn validate_plan_in(
        &self,
        repo_root: &str,
        raw: Option<&Value>,
        caller: Option<&SessionRecord>,
    ) -> Result<ValidPlan> {
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
        // Any open line nobody is on can be started on, at any depth.
        let tasks: Vec<_> = self.task_list(&key).lines().iter().map(|line| line.task()).collect();
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
            orchestrator: caller.filter(|caller| caller.role == Some(SessionRole::Orchestrator)),
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
        let (repo_root, caller) = self.plan_origin(request, false).await?;
        Ok(self
            .validate_plan_in(&repo_root, request.plan.as_ref(), caller.as_deref())
            .await?
            .summary)
    }

    /// A new plan ID: 8 hex digits, as the commands show IDs, that no pending plan's starts with (nor the reverse).
    fn new_plan_id(&self) -> String {
        let plans = self.plans.borrow();
        loop {
            let id: String = uuid::Uuid::new_v4().simple().to_string().chars().take(8).collect();
            if !plans
                .pending
                .keys()
                .any(|other| other.starts_with(&id) || id.starts_with(other.as_str()))
            {
                return id;
            }
        }
    }

    /// `plan-apply`: validates, holds the plan for the user's answer in Deckhand and answers at once; the answer
    /// reaches the orchestrator as a message ([`Daemon::handle_plan_decide`]).
    pub async fn handle_plan_apply(&self, request: &PlanRequest) -> Result<PlanWaiting> {
        let (repo_root, caller) = self.plan_origin(request, true).await?;
        self.apply_plan(&repo_root, request.plan.as_ref(), caller.as_deref())
            .await
    }

    /// [`Daemon::handle_plan_apply`] for `caller` (authenticated; `None` by hand) in `repo_root`.
    pub async fn apply_plan(
        &self,
        repo_root: &str,
        raw: Option<&Value>,
        caller: Option<&SessionRecord>,
    ) -> Result<PlanWaiting> {
        let repo_root = repo_root.to_owned();
        let valid = self.validate_plan_in(&repo_root, raw, caller).await?;
        let id = self.new_plan_id();
        let pending = PendingPlan {
            id: id.clone(),
            repo_root: repo_root.clone(),
            caller_session_id: caller.map(|caller| caller.id.clone()),
            caller_title: caller.map(|caller| self.session_path(caller)),
            plan: valid.plan.clone(),
            summary: valid.summary.clone(),
            created_at: iso(date::now_ms()),
            parked: None,
            extra: IndexMap::new(),
        };
        let summary = valid.summary.clone();
        self.plans.borrow_mut().pending.insert(
            id.clone(),
            PendingEntry {
                plan: pending.clone(),
                valid,
            },
        );
        self.save_plans_soon();
        self.log(&format!("plan {id} waits for an answer ({})", summary.counts))
            .await;
        self.broadcast_plan(
            &repo_root,
            &ServerMessage::PlanPending(Box::new(PlanPending { plan: pending })),
        )
        .await;
        Ok(PlanWaiting { plan_id: id, summary })
    }

    /// `plan-decide`: the first answer to a pending plan wins; a later one is refused. Approved, the plan is created in
    /// the background and the orchestrator told what was; cancelled, it is told so, with the user's note.
    pub async fn handle_plan_decide(&self, request: &PlanDecide) -> Result<OkResult> {
        let removed = self.plans.borrow_mut().pending.shift_remove(&request.plan_id);
        let Some(entry) = removed else {
            return Err(not_waiting(&request.plan_id));
        };
        self.save_plans_soon();
        let decision = if request.approve == Some(true) {
            PlanDecision::Approved
        } else {
            PlanDecision::Rejected
        };
        self.settle_plan(&entry.plan.repo_root, &request.plan_id, decision)
            .await;
        match decision {
            PlanDecision::Approved => {
                let daemon = self.clone();
                drop(spawn(async move { daemon.create_approved(entry).await }));
            }
            _ => {
                if let Some(caller) = entry.plan.caller_session_id.as_deref() {
                    self.queue_notice(caller, cancelled_text(&entry.plan.id, request.note.as_deref()));
                }
            }
        }
        Ok(OkResult::default())
    }

    /// Creates an approved plan, then tells the orchestrator that applied it what was created (by hand: nobody).
    pub async fn create_approved(&self, entry: PendingEntry) {
        let plan = entry.plan;
        let mut outcome = self
            .create_plan(&plan.id, &plan.repo_root, &entry.valid, plan.caller_session_id.clone())
            .await;
        let Some(caller) = plan.caller_session_id.as_deref() else {
            return;
        };
        // The new worktrees' branches, once they are made, say where each session works.
        let deadline = tokio::time::Instant::now() + CREATED_SETTLE_WAIT;
        while tokio::time::Instant::now() < deadline
            && outcome
                .sessions
                .iter()
                .any(|created| self.sessions.borrow().preparing.contains(&created.session.id))
        {
            tokio::time::sleep(PARENT_PREPARE_POLL).await;
        }
        for created in &mut outcome.sessions {
            if let Some(current) = self.get_session(&created.session.id) {
                created.session = current;
            }
        }
        let text = approved_text(&outcome, |session| self.session_path(session));
        self.queue_notice(caller, text);
    }

    /// `plan-park`: the user put a pending plan aside. It keeps waiting; the UIs hear it (`plan-pending`, parked) so
    /// it no longer opens by itself or detaches an attach.
    pub async fn handle_plan_park(&self, request: &PlanPark) -> Result<OkResult> {
        let plan = {
            let mut plans = self.plans.borrow_mut();
            let Some(entry) = plans.pending.get_mut(&request.plan_id) else {
                return Err(not_waiting(&request.plan_id));
            };
            entry.plan.parked = Some(true);
            entry.plan.clone()
        };
        self.save_plans_soon();
        self.broadcast_plan(
            &plan.repo_root.clone(),
            &ServerMessage::PlanPending(Box::new(PlanPending { plan })),
        )
        .await;
        Ok(OkResult::default())
    }

    /// Session `session_id` exited for good (`removed`: or was removed): the plans it applied are dropped and the UIs
    /// close them. One that exited is told when it runs again (held in its queue); one removed has no queue.
    pub async fn plans_on_exit(&self, session_id: &str, removed: bool) {
        let dropped: Vec<PendingPlan> = {
            let mut plans = self.plans.borrow_mut();
            let ids: Vec<String> = plans
                .pending
                .values()
                .filter(|entry| entry.plan.caller_session_id.as_deref() == Some(session_id))
                .map(|entry| entry.plan.id.clone())
                .collect();
            ids.iter()
                .filter_map(|id| plans.pending.shift_remove(id))
                .map(|entry| entry.plan)
                .collect()
        };
        if dropped.is_empty() {
            return;
        }
        self.save_plans_soon();
        for plan in dropped {
            self.settle_plan(&plan.repo_root, &plan.id, PlanDecision::Dropped).await;
            if !removed {
                self.queue_notice(session_id, dropped_text(&plan.id));
            }
        }
    }

    fn plans_path(&self) -> PathBuf {
        self.paths.config_dir.join(PLANS_FILE)
    }

    /// Reads `plans.json` at startup: a plan whose orchestrator is gone is dropped; the rest wait for their answer.
    pub async fn load_plans(&self) -> Result<()> {
        let path = self.plans_path();
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(DhError::io(&error, "open", &path.to_string_lossy())),
        };
        let file: PlansFile = match serde_json::from_str(&text) {
            Ok(file) => file,
            Err(error) => {
                // Kept aside rather than overwritten by the next save.
                let aside = self.paths.config_dir.join(format!("{PLANS_FILE}.unreadable"));
                let _ = tokio::fs::rename(&path, &aside).await;
                self.log(&format!(
                    "{PLANS_FILE} is unreadable ({error}); moved to {} and starting empty",
                    aside.to_string_lossy()
                ))
                .await;
                return Ok(());
            }
        };
        let mut pending = IndexMap::new();
        let mut foreign = Vec::new();
        let mut gone = 0;
        for value in file.plans {
            match serde_json::from_value::<PendingPlan>(value.clone()) {
                Ok(plan)
                    if plan
                        .caller_session_id
                        .as_deref()
                        .is_some_and(|caller| self.get_session(caller).is_none()) =>
                {
                    gone += 1;
                }
                Ok(plan) => {
                    pending.insert(plan.id.clone(), PendingEntry::of(plan));
                }
                Err(_) => foreign.push(value),
            }
        }
        let waiting = pending.len();
        {
            let mut plans = self.plans.borrow_mut();
            plans.pending = pending;
            plans.foreign = foreign;
            plans.file_extra = file.extra;
        }
        if gone > 0 {
            self.save_plans_soon();
        }
        if waiting > 0 || gone > 0 {
            self.log(&format!(
                "{waiting} plan(s) wait for an answer; {gone} dropped (their orchestrator was removed)"
            ))
            .await;
        }
        Ok(())
    }

    /// Writes `plans.json` (coalesced: callers before the queued write starts share it).
    pub fn save_plans(&self) -> SharedTask<Result<()>> {
        let coalescer = {
            let mut plans = self.plans.borrow_mut();
            plans
                .save
                .get_or_insert_with(|| {
                    let weak = self.downgrade();
                    Coalescer::new(move || {
                        let contents = weak.upgrade().map(|daemon| {
                            let plans = daemon.plans.borrow();
                            let mut records: Vec<Value> = plans
                                .pending
                                .values()
                                .filter_map(|entry| serde_json::to_value(&entry.plan).ok())
                                .collect();
                            records.extend(plans.foreign.iter().cloned());
                            let file = PlansFile {
                                plans: records,
                                extra: plans.file_extra.clone(),
                            };
                            (daemon.plans_path(), serde_json::to_string_pretty(&file))
                        });
                        async move {
                            match contents {
                                Some((path, Ok(text))) => {
                                    storage::write_atomic(&path, format!("{text}\n").as_bytes(), storage::FILE_MODE)
                                        .await
                                }
                                Some((_, Err(error))) => Err(DhError::msg(error.to_string())),
                                None => Ok(()),
                            }
                        }
                    })
                })
                .clone()
        };
        coalescer.request()
    }

    /// [`Daemon::save_plans`] in the background, a failure logged.
    fn save_plans_soon(&self) {
        let saved = self.save_plans();
        let daemon = self.clone();
        drop(spawn(async move {
            if let Err(error) = saved.await {
                daemon.log(&format!("saving {PLANS_FILE} failed: {error}")).await;
            }
        }));
    }

    /// Tells the repository's clients that plan `id` was answered or dropped.
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

    /// Creates an approved plan: an orchestrator's idea brief first, then its sessions in order (each child once its
    /// parent is ready), then its tasks. Stops at the first failure; what was created stays. `spawned_by`: the session
    /// that applied it; when that is an orchestrator, the plan goes under it and its idea's line.
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
        let caller = spawned_by.as_deref().and_then(|caller| self.get_session(caller));
        let orchestrator = caller
            .clone()
            .filter(|caller| caller.role == Some(SessionRole::Orchestrator));
        // Who wrote the briefs: the orchestrator (or the session) that applied the plan, else the plan run by hand.
        let brief_from = caller.map_or_else(
            || "a plan".to_owned(),
            |caller| {
                if caller.role.is_some() {
                    format!("◎ {}", caller.title)
                } else {
                    caller.title.clone()
                }
            },
        );
        // The idea's line: the orchestrator's, made when it has none (an older one, or its line was removed), before
        // its sessions' lines go under it. What was agreed is its brief.
        let idea_line = match orchestrator.as_deref() {
            Some(orchestrator) => match self.ensure_session_line(orchestrator).await {
                Ok(line) => Some(line),
                Err(error) => {
                    self.log(&format!("plan {id}: the idea's line failed: {error}")).await;
                    None
                }
            },
            None => None,
        };
        if let (Some((key, line)), Some(brief)) = (idea_line.as_ref(), valid.plan.brief.as_deref())
            && let Err(error) = self.write_brief(key, line, brief, &brief_from).await
        {
            self.log(&format!("plan {id}: the idea's brief failed: {error}")).await;
        }
        for (index, session) in valid.plan.sessions.iter().enumerate() {
            let parent = match valid.parents.get(index).cloned().flatten() {
                // In an orchestrator's plan, a sub-session of the orchestrator.
                None => orchestrator
                    .as_ref()
                    .map(|orchestrator| (orchestrator.id.clone(), false)),
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
                Ok(record) => {
                    // What it was told is its line's brief (a brief is a reason to have a line; a session started on a
                    // line is on it already): its sub-sessions' lines go under it.
                    let brief = async {
                        let (key, line) = self.ensure_session_line(&record).await?;
                        self.write_brief(&key, &line, &session.prompt, &brief_from).await
                    }
                    .await;
                    if let Err(error) = brief {
                        self.log(&format!("plan {id}: the brief of {} failed: {error}", session.key))
                            .await;
                    }
                    outcome.sessions.push(PlanCreatedSession {
                        key: session.key.clone(),
                        session: record,
                    });
                }
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
            // Parked under the idea's line, else in the backlog.
            let key = match idea_line.as_ref() {
                Some((key, _)) => Ok(key.clone()),
                None => self.tasks_key_for(repo_root).await,
            };
            let parent = idea_line.as_ref().map(|(_, line)| line.clone());
            let key = match key {
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
                let op = crate::notes_tasks::add_line(&task.title, &task_id, parent.clone(), None);
                // Its details are its line's note.
                let added = async {
                    self.change_tasks(&key, op).await?;
                    match task.details.as_deref().filter(|details| !details.trim().is_empty()) {
                        Some(details) => self.append_line_note(&key, &task_id, details).await,
                        None => Ok(()),
                    }
                }
                .await;
                match added {
                    Ok(()) => outcome.tasks.push(PlanCreatedTask {
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

    /// `open-orchestrator`: a new orchestrator for the idea `prompt` (its first message), titled `title` or else after
    /// the idea (numbered while an active session has the title), put on a new top-level line of the same title: the
    /// idea's line.
    pub async fn handle_open_orchestrator(&self, request: &OpenOrchestrator) -> Result<OrchestratorResult> {
        let cwd = match &request.cwd {
            Loose::Known(cwd) if !cwd.is_empty() => cwd.clone(),
            _ => return Err(DhError::msg("Invalid repository path")),
        };
        let Some(idea) = request.prompt.clone().filter(|prompt| !prompt.trim().is_empty()) else {
            return Err(DhError::msg(NO_IDEA));
        };
        let repo_root = find_repo_root(Some(&cwd)).await?;
        let title = self.new_orchestrator_title(&repo_root, request.title.as_deref(), &idea);
        let input = CreateSessionInput {
            title,
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
            prompt: Some(idea),
            submit: None,
            stacked: None,
            role: Some(SessionRole::Orchestrator),
        };
        let session = self.create_session(input).await?;
        let line_id = match self.ensure_session_line(&session).await {
            Ok((_, line)) => Some(line),
            Err(error) => {
                self.log(&format!(
                    "the idea's line of orchestrator {} failed: {error}",
                    session.id
                ))
                .await;
                None
            }
        };
        Ok(OrchestratorResult {
            session: self.get_session(&session.id).unwrap_or(session),
            line_id,
        })
    }

    /// A new orchestrator's title in `repo_root`: its `name` when it has text, else its idea's first line
    /// ([`orchestrator_title`]), numbered ` 2`, ` 3`… while an active session of the repository has it.
    fn new_orchestrator_title(&self, repo_root: &str, name: Option<&str>, idea: &str) -> String {
        let base = orchestrator_title(name, idea);
        let taken = |title: &str| {
            self.all_sessions().iter().any(|session| {
                session.repo_root == repo_root
                    && session.status != SessionStatus::Exited
                    && self.session_path(session) == title
            })
        };
        if !taken(&base) {
            return base;
        }
        (2..)
            .map(|n| numbered_title(&base, n))
            .find(|title| !taken(title))
            .unwrap_or(base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::tests::Home;
    use crate::server::tests::{local, session, test_client, test_daemon};
    use crate::server::{Client, ClientId};
    use deckhand_protocol::data::{MessageKind, MessageRecord, MessageStatus};
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

    fn park(plan_id: &str) -> PlanPark {
        serde_json::from_value(json!({"requestId": "k", "planId": plan_id})).unwrap()
    }

    /// A plan of tasks only (creating it needs no agent).
    fn tasks_plan() -> Value {
        json!({"tasks": [{"title": "Remove legacy endpoint", "details": "After Auth rework merges"}]})
    }

    /// The lines of `root`'s list, once creating a plan in the background added `count` of them.
    async fn lines_once(daemon: &Daemon, root: &str, count: usize) -> Vec<deckhand_core::task_list::Line> {
        let key = daemon.tasks_key_for(root).await.unwrap();
        for _ in 0..400 {
            let lines = daemon.task_list(&key).lines().to_vec();
            if lines.len() >= count {
                return lines;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the plan's lines were not added");
    }

    /// The notices queued to session `to`, once there are `count`.
    async fn notices_once(daemon: &Daemon, to: &str, count: usize) -> Vec<MessageRecord> {
        for _ in 0..400 {
            let notices = notices_to(daemon, to);
            if notices.len() >= count {
                return notices;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("no notice reached {to}");
    }

    fn notices_to(daemon: &Daemon, to: &str) -> Vec<MessageRecord> {
        daemon
            .messages
            .borrow()
            .messages
            .iter()
            .filter(|message| message.to_session_id.as_deref() == Some(to))
            .cloned()
            .collect()
    }

    /// Session `by` (as if authenticated) applies `plan` in `root`: the plan's ID.
    async fn apply_as(daemon: &Daemon, root: &str, by: &str, plan: Value) -> String {
        let caller = daemon.get_session(by).unwrap();
        daemon
            .apply_plan(root, Some(&plan), Some(&caller))
            .await
            .unwrap()
            .plan_id
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
    fn applying_answers_at_once_and_the_first_answer_creates_the_plan() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        let daemon = test_daemon(dir.path());
        local(async {
            let (_ui, mut ui_rx) = subscribe(&daemon, 1, &root);
            let (_elsewhere, mut other_rx) = subscribe(&daemon, 2, "/elsewhere");
            // Nobody has answered: apply comes back with the plan's ID and what it holds, and the plan waits.
            let waiting = daemon.handle_plan_apply(&request(&root, tasks_plan())).await.unwrap();
            let id = waiting.plan_id.clone();
            assert_eq!(id.len(), 8, "{id}");
            assert_eq!(waiting.summary.counts, "1 task");
            assert!(daemon.plans.borrow().pending.contains_key(&id));
            let pending = of(&received(&mut ui_rx), "plan-pending");
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0]["plan"]["id"], id.as_str());
            assert_eq!(pending[0]["plan"]["repoRoot"], root.as_str());
            assert_eq!(pending[0]["plan"]["summary"]["counts"], "1 task");
            assert!(pending[0]["plan"].get("callerSessionId").is_none());
            assert!(pending[0]["plan"].get("parked").is_none());
            // Another repository's UI hears nothing of it.
            assert!(of(&received(&mut other_rx), "plan-pending").is_empty());

            daemon.handle_plan_decide(&decide(&id, json!(true))).await.unwrap();
            let error = daemon.handle_plan_decide(&decide(&id, json!(false))).await.unwrap_err();
            assert_eq!(
                error.to_string(),
                format!("No plan {id} is waiting for an answer (it was answered already, or its orchestrator ended)")
            );
            let settled = of(&received(&mut ui_rx), "plan-settled");
            assert_eq!(settled.len(), 1);
            assert_eq!(settled[0]["planId"], id.as_str());
            assert_eq!(settled[0]["decision"], "approved");
            // The task is a line in the backlog; its details are the line's note.
            let lines = lines_once(&daemon, &root, 1).await;
            let added = &lines[0];
            assert_eq!(added.title, "Remove legacy endpoint");
            assert!(added.body.is_empty() && added.parent.is_none() && !added.is_taken());
            let note = daemon.work_note(deckhand_protocol::data::WorkNoteKind::Line, &added.id);
            assert!(note.text.contains("After Auth rework merges"), "{note:?}");
            // Applied by hand, there is no orchestrator to tell.
            assert!(daemon.messages.borrow().messages.is_empty());
            assert!(daemon.plans.borrow().pending.is_empty());
        });
    }

    #[test]
    fn a_cancelled_plan_by_hand_creates_nothing_and_tells_nobody() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        let daemon = test_daemon(dir.path());
        local(async {
            let id = daemon
                .handle_plan_apply(&request(&root, tasks_plan()))
                .await
                .unwrap()
                .plan_id;
            // Anything but `true` cancels.
            daemon.handle_plan_decide(&decide(&id, json!("yes"))).await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            let key = daemon.tasks_key_for(&root).await.unwrap();
            assert!(daemon.task_list(&key).lines().is_empty());
            assert!(daemon.messages.borrow().messages.is_empty());
        });
    }

    #[test]
    fn a_ui_that_subscribes_late_is_sent_the_pending_plan() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        let daemon = test_daemon(dir.path());
        local(async {
            let id = daemon
                .handle_plan_apply(&request(&root, tasks_plan()))
                .await
                .unwrap()
                .plan_id;
            let (late, mut late_rx) = subscribe(&daemon, 3, &root);
            daemon.send_pending_plans(&late).await;
            let pending = of(&received(&mut late_rx), "plan-pending");
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0]["plan"]["id"], id.as_str());
            // A UI of another repository is not.
            let (other, mut other_rx) = subscribe(&daemon, 4, "/elsewhere");
            daemon.send_pending_plans(&other).await;
            assert!(of(&received(&mut other_rx), "plan-pending").is_empty());
        });
    }

    #[test]
    fn a_plan_put_aside_keeps_waiting_and_tells_nobody() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        let daemon = test_daemon(dir.path());
        local(async {
            orchestrator(&daemon, "orch", &root, "Rework auth");
            let (_ui, mut ui_rx) = subscribe(&daemon, 1, &root);
            let id = apply_as(&daemon, &root, "orch", tasks_plan()).await;
            received(&mut ui_rx);
            daemon.handle_plan_park(&park(&id)).await.unwrap();
            // Still waiting, marked parked: the UIs hear it (so it no longer opens by itself), and a late one too.
            assert_eq!(daemon.plans.borrow().pending[&id].plan.parked, Some(true));
            let events = received(&mut ui_rx);
            let pending = of(&events, "plan-pending");
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0]["plan"]["parked"], true);
            assert!(of(&events, "plan-settled").is_empty());
            let (late, mut late_rx) = subscribe(&daemon, 2, &root);
            daemon.send_pending_plans(&late).await;
            assert_eq!(of(&received(&mut late_rx), "plan-pending")[0]["plan"]["parked"], true);
            // The orchestrator hears nothing until it is answered; it can still be answered.
            assert!(notices_to(&daemon, "orch").is_empty());
            let error = daemon.handle_plan_park(&park("nope")).await.unwrap_err();
            assert!(error.to_string().starts_with("No plan nope is waiting"), "{error}");
            daemon.handle_plan_decide(&decide(&id, json!(true))).await.unwrap();
            notices_once(&daemon, "orch", 1).await;
        });
    }

    #[test]
    fn approving_an_orchestrators_plan_tells_it_what_was_created() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        let daemon = test_daemon(dir.path());
        local(async {
            orchestrator(&daemon, "orch", &root, "Rework auth");
            let plan = json!({"tasks": [{"title": "Remove legacy endpoint"}, {"title": "Update the docs"}]});
            let id = apply_as(&daemon, &root, "orch", plan).await;
            assert_eq!(
                daemon.plans.borrow().pending[&id].plan.caller_session_id.as_deref(),
                Some("orch")
            );
            daemon.handle_plan_decide(&decide(&id, json!(true))).await.unwrap();
            let notices = notices_once(&daemon, "orch", 1).await;
            assert_eq!(notices.len(), 1);
            let notice = &notices[0];
            // A notice, from nobody (no header, nothing to reply to), held until the orchestrator is idle.
            assert_eq!(notice.kind, MessageKind::Notice);
            assert_eq!(
                (notice.from_session_id.as_deref(), notice.status),
                (None, MessageStatus::Held)
            );
            let (key, idea) = daemon.session_line("orch").unwrap();
            let list = daemon.task_list(&key);
            let parked: Vec<String> = list
                .lines()
                .iter()
                .filter(|line| line.parent.as_deref() == Some(idea.id.as_str()))
                .map(|line| format!("{}  ☐ {}  (parked under your idea)", line.id, line.title))
                .collect();
            assert_eq!(parked.len(), 2);
            assert_eq!(
                notice.text,
                format!(
                    "[The user approved your plan {id}]\n\
                     Created 0 sessions and 2 tasks; each session got its brief as its first message:\n{}",
                    parked.join("\n")
                )
            );
        });
    }

    #[test]
    fn cancelling_an_orchestrators_plan_tells_it_with_the_users_note() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        let daemon = test_daemon(dir.path());
        local(async {
            orchestrator(&daemon, "orch", &root, "Rework auth");
            let first = apply_as(&daemon, &root, "orch", tasks_plan()).await;
            let second = apply_as(&daemon, &root, "orch", tasks_plan()).await;
            let mut with_note = decide(&first, json!(false));
            with_note.note = Some("Split the tests into their own session".into());
            daemon.handle_plan_decide(&with_note).await.unwrap();
            daemon.handle_plan_decide(&decide(&second, json!(false))).await.unwrap();
            let texts: Vec<String> = notices_to(&daemon, "orch")
                .into_iter()
                .map(|notice| notice.text)
                .collect();
            assert_eq!(
                texts,
                [
                    format!(
                        "[The user cancelled your plan {first}: nothing was created]\n\
                         Their note: Split the tests into their own session"
                    ),
                    format!("[The user cancelled your plan {second}: nothing was created]"),
                ]
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
            let key = daemon.tasks_key_for(&root).await.unwrap();
            assert!(daemon.task_list(&key).lines().is_empty());
        });
    }

    #[test]
    fn several_plans_wait_side_by_side_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        let daemon = test_daemon(dir.path());
        local(async {
            let first = daemon
                .handle_plan_apply(&request(&root, tasks_plan()))
                .await
                .unwrap()
                .plan_id;
            let second = daemon
                .handle_plan_apply(&request(&root, json!({"tasks": [{"title": "Other"}]})))
                .await
                .unwrap()
                .plan_id;
            assert_ne!(first, second);
            let (late, mut late_rx) = subscribe(&daemon, 1, &root);
            daemon.send_pending_plans(&late).await;
            let ids: Vec<Value> = of(&received(&mut late_rx), "plan-pending")
                .iter()
                .map(|event| event["plan"]["id"].clone())
                .collect();
            assert_eq!(ids, [json!(first), json!(second)]);
            // Answering one leaves the other waiting.
            daemon.handle_plan_decide(&decide(&second, json!(false))).await.unwrap();
            let left: Vec<String> = daemon.plans.borrow().pending.keys().cloned().collect();
            assert_eq!(left, [first]);
        });
    }

    #[test]
    fn an_orchestrator_that_exits_or_is_removed_drops_its_plans() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        let daemon = test_daemon(dir.path());
        local(async {
            orchestrator(&daemon, "orch", &root, "Rework auth");
            orchestrator(&daemon, "other", &root, "Login");
            let (_ui, mut ui_rx) = subscribe(&daemon, 1, &root);
            let mine = apply_as(&daemon, &root, "orch", tasks_plan()).await;
            let by_hand = daemon
                .handle_plan_apply(&request(&root, tasks_plan()))
                .await
                .unwrap()
                .plan_id;
            received(&mut ui_rx);
            // It exits: its plan is dropped (the UIs close it), the other waits on, and it is told when it runs again.
            daemon.patch_session("orch", |session| session.status = SessionStatus::Exited);
            daemon.plans_on_exit("orch", false).await;
            let left: Vec<String> = daemon.plans.borrow().pending.keys().cloned().collect();
            assert_eq!(left, std::slice::from_ref(&by_hand));
            let settled = of(&received(&mut ui_rx), "plan-settled");
            assert_eq!(settled.len(), 1);
            assert_eq!(settled[0]["planId"], mine.as_str());
            assert_eq!(settled[0]["decision"], "dropped");
            let notices = notices_to(&daemon, "orch");
            assert_eq!(notices.len(), 1);
            assert_eq!(notices[0].status, MessageStatus::Held);
            assert!(
                notices[0].text.starts_with(&format!("[Your plan {mine} was dropped")),
                "{}",
                notices[0].text
            );
            // Answering it now is refused.
            let error = daemon
                .handle_plan_decide(&decide(&mine, json!(true)))
                .await
                .unwrap_err();
            assert!(error.to_string().contains("its orchestrator ended"), "{error}");

            // Removed (an orchestrator a daemon restart left exited): its plan goes, with nobody to tell.
            let theirs = apply_as(&daemon, &root, "other", tasks_plan()).await;
            daemon.patch_session("other", |session| session.status = SessionStatus::Exited);
            daemon.remove_session("other").await.unwrap();
            assert!(!daemon.plans.borrow().pending.contains_key(&theirs));
            assert!(daemon.plans.borrow().pending.contains_key(&by_hand));
            assert!(notices_to(&daemon, "other").is_empty());
        });
    }

    #[test]
    fn pending_plans_wait_through_a_daemon_restart() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        let state = dir.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        local(async {
            let daemon = test_daemon(&state);
            orchestrator(&daemon, "orch", &root, "Rework auth");
            orchestrator(&daemon, "gone", &root, "Gone");
            daemon.set_session(session("lead", &root, &root));
            let plan = json!({"sessions": [
                {"key": "a", "title": "Token storage", "prompt": "Store them", "parent": "lead", "worktree": "same"},
                {"key": "b", "title": "Tests", "prompt": "Test them", "parent": "a", "worktree": "same"},
            ]});
            let kept = apply_as(&daemon, &root, "orch", plan).await;
            daemon.handle_plan_park(&park(&kept)).await.unwrap();
            apply_as(&daemon, &root, "gone", tasks_plan()).await;
            daemon.save_plans().await.unwrap();
            let written = std::fs::read_to_string(state.join(PLANS_FILE)).unwrap();
            assert!(written.contains(&format!("\"id\": \"{kept}\"")), "{written}");

            // The next daemon knows the orchestrator (left exited), not the session removed meanwhile.
            let next = test_daemon(&state);
            orchestrator(&next, "orch", &root, "Rework auth");
            next.patch_session("orch", |session| session.status = SessionStatus::Exited);
            next.set_session(session("lead", &root, &root));
            next.load_plans().await.unwrap();
            let plans = next.plans.borrow();
            let ids: Vec<&String> = plans.pending.keys().collect();
            assert_eq!(ids, [&kept]);
            let entry = &plans.pending[&kept];
            let before = &daemon.plans.borrow().pending[&kept];
            assert_eq!(entry.plan, before.plan);
            assert_eq!(entry.plan.parked, Some(true));
            // What creating it needs is found again: the first under an existing session, the second under the first.
            assert_eq!(entry.valid, before.valid);
            assert_eq!(
                entry.valid.parents,
                [Some(PlanParent::Session("lead".into())), Some(PlanParent::Entry(0))]
            );
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
            let id = daemon.handle_plan_apply(&as_caller("token")).await.unwrap().plan_id;
            let pending = daemon.plans.borrow().pending[&id].plan.clone();
            assert_eq!(pending.caller_session_id.as_deref(), Some("s"));
            assert_eq!(pending.repo_root, root);
        });
    }

    /// Session `id` of `root`: an orchestrator titled `title`, running.
    fn orchestrator(daemon: &Daemon, id: &str, root: &str, title: &str) -> Ordered<SessionRecord> {
        daemon.set_session(session(id, root, root));
        daemon.patch_session(id, |session| {
            session.title = title.to_owned();
            session.role = Some(SessionRole::Orchestrator);
        });
        daemon.get_session(id).unwrap()
    }

    #[test]
    fn an_orchestrators_plan_parks_its_tasks_under_its_idea_and_writes_the_ideas_brief() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        let daemon = test_daemon(dir.path());
        local(async {
            let orch = orchestrator(&daemon, "orch", &root, "Rework auth");
            let plan = json!({
                "tasks": [
                    {"title": "Remove legacy endpoint", "details": "Waits for Token storage to merge"},
                    {"title": "Update the docs"},
                ],
                "brief": "Problem: tokens leak.\nRuled out: SSO.",
            });
            let valid = daemon.validate_plan_in(&root, Some(&plan), Some(&orch)).await.unwrap();
            assert_eq!(valid.summary.idea.as_deref(), Some("Rework auth"));
            // Run by hand, a brief has no idea to go to.
            let error = daemon.validate_plan_in(&root, Some(&plan), None).await.unwrap_err();
            assert!(error.to_string().contains("brief is the idea's brief"), "{error}");

            let outcome = daemon.create_plan("p", &root, &valid, Some("orch".into())).await;
            assert_eq!(outcome.status, PlanStatus::Created, "{outcome:?}");
            // The orchestrator had no line yet: the idea's line is made, top-level, named after it, and it is on it.
            let (key, idea) = daemon.session_line("orch").unwrap();
            assert_eq!((idea.title.as_str(), idea.parent.as_deref()), ("Rework auth", None));
            let list = daemon.task_list(&key);
            for task in &outcome.tasks {
                let line = list.get(&task.id).unwrap();
                assert_eq!(line.parent.as_deref(), Some(idea.id.as_str()), "{}", line.title);
                assert!(!line.is_taken() && !line.done, "{}", line.title);
            }
            let parked = daemon.work_note(deckhand_protocol::data::WorkNoteKind::Line, &outcome.tasks[0].id);
            assert!(parked.text.contains("Waits for Token storage to merge"), "{parked:?}");
            let brief = daemon.work_note(deckhand_protocol::data::WorkNoteKind::Brief, &idea.id);
            assert_eq!(brief.text, "Problem: tokens leak.\nRuled out: SSO.\n");
            assert_eq!(brief.from.as_deref(), Some("◎ Rework auth"));

            // The idea ticks by roll-up once every line under it is ticked (and reopens with one of them).
            for task in &outcome.tasks {
                daemon
                    .handle_client_task_op(&root, &json!({"type": "tick", "id": task.id}))
                    .await
                    .unwrap();
            }
            assert!(daemon.task_list(&key).get(&idea.id).unwrap().done);
            daemon
                .handle_client_task_op(&root, &json!({"type": "reopen", "id": outcome.tasks[1].id}))
                .await
                .unwrap();
            assert!(!daemon.task_list(&key).get(&idea.id).unwrap().done);

            // A later plan from it joins the same idea: its line is not made again, and its brief replaces the first.
            let later = json!({"tasks": [{"title": "Rotate the keys"}], "brief": "Also: rotate the keys."});
            let valid = daemon.validate_plan_in(&root, Some(&later), Some(&orch)).await.unwrap();
            let outcome = daemon.create_plan("p2", &root, &valid, Some("orch".into())).await;
            let list = daemon.task_list(&key);
            assert_eq!(
                list.get(&outcome.tasks[0].id).unwrap().parent.as_deref(),
                Some(idea.id.as_str())
            );
            assert_eq!(list.lines().iter().filter(|line| line.parent.is_none()).count(), 1);
            let brief = daemon.work_note(deckhand_protocol::data::WorkNoteKind::Brief, &idea.id);
            assert_eq!(brief.text, "Also: rotate the keys.\n");
        });
    }

    #[test]
    fn a_plan_from_a_session_that_is_not_an_orchestrator_is_checked_as_by_hand() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        let daemon = test_daemon(dir.path());
        local(async {
            daemon.set_session(session("plain", &root, &root));
            let plain = daemon.get_session("plain").unwrap();
            let valid = daemon
                .validate_plan_in(&root, Some(&tasks_plan()), Some(&plain))
                .await
                .unwrap();
            assert_eq!(valid.summary.idea, None);
        });
    }

    #[test]
    fn opening_an_orchestrator_needs_an_idea() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        let daemon = test_daemon(dir.path());
        local(async {
            for prompt in [json!(null), json!("  \n ")] {
                let request: OpenOrchestrator =
                    serde_json::from_value(json!({"requestId": "o", "cwd": root, "prompt": prompt})).unwrap();
                let error = daemon.handle_open_orchestrator(&request).await.unwrap_err();
                assert_eq!(error.to_string(), NO_IDEA);
            }
            assert!(daemon.all_sessions().is_empty());
        });
    }

    #[test]
    fn an_orchestrator_is_named_by_its_name_else_its_idea_numbered_while_taken() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let daemon = test_daemon(dir.path());
        let idea = "Rework auth so tokens are stored encrypted\nand rotated weekly";
        let title = |name: Option<&str>| daemon.new_orchestrator_title("/repo", name, idea);
        assert_eq!(title(Some("  Auth   tokens ")), "Auth tokens");
        // No name, or a blank one: the idea's first line, as before.
        assert_eq!(title(None), "Rework auth so tokens are stored encrypted");
        assert_eq!(title(Some(" ")), "Rework auth so tokens are stored encrypted");
        // Past 64 characters, a name is cut at a word.
        let long = "Rework the authentication layer so that tokens are stored encrypted";
        assert_eq!(
            title(Some(long)),
            "Rework the authentication layer so that tokens are stored"
        );
        // Taken by an active session of the repository: numbered, as an idea's title is.
        orchestrator(&daemon, "first", "/repo", "Auth tokens");
        orchestrator(&daemon, "second", "/repo", "Auth tokens 2");
        assert_eq!(title(Some("Auth tokens")), "Auth tokens 3");
        // Not by an exited one, nor by another repository's.
        daemon.patch_session("first", |session| session.status = SessionStatus::Exited);
        orchestrator(&daemon, "elsewhere", "/other", "Login UI");
        assert_eq!(title(Some("Auth tokens")), "Auth tokens");
        assert_eq!(title(Some("Login UI")), "Login UI");
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
        parent.title = "Auth rework".into();
        let new = plan_session_input(&plan_session(PlanWorktree::New), Some(&parent), "/elsewhere");
        assert_eq!(
            (new.cwd.as_str(), new.repo_root.as_str(), new.worktree_mode),
            ("/wt/p", "/repo", Some(WorktreeMode::New))
        );
        assert_eq!(new.parent_session_id.as_deref(), Some("p"));
        assert_eq!(new.sub_session_kind, Some(SubSessionKind::Clean));
        assert_eq!((new.base_branch.as_deref(), new.stacked), (Some("main"), Some(false)));
        // Told to send its parent an update once it has finished.
        assert_eq!(
            (new.prompt.as_deref(), new.submit),
            (
                Some(
                    "Write them\n\nWhen you have finished this task, send your parent session, \"Auth rework\", an \
                     update (what you did, and anything left open): `deckhand send p \"<update>\"`."
                ),
                None
            )
        );
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
        // Without a parent (a plan applied by hand) there is nobody to tell.
        assert_eq!(top.prompt.as_deref(), Some("Write them"));
        // An orchestrator's own sub-sessions send it theirs.
        parent.role = Some(SessionRole::Orchestrator);
        parent.title = "Auth tokens".into();
        let under_orchestrator = plan_session_input(&plan_session(PlanWorktree::New), Some(&parent), "/repo");
        assert!(
            under_orchestrator
                .prompt
                .as_deref()
                .is_some_and(|prompt| prompt.ends_with("send your parent session, \"◎ Auth tokens\", an update (what \
                                                       you did, and anything left open): `deckhand send p \"<update>\"`."))
        );
    }
}
