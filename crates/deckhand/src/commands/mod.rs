//! `deckhand new|ls|task|note|kill|remove|guide|plan|do`: Deckhand actions from a shell, for the agents Deckhand runs
//! (not in the TS), the orchestrator's `plan` and your `do`.
//!
//! Inside a session the command acts for that session, the caller: its `DECKHAND_SESSION_ID`, `DECKHAND_LAUNCH_ID`
//! and `DECKHAND_HOOK_TOKEN` go with `create`, `kill` and `remove`, and the daemon checks them as it does hook
//! callbacks (the sessions a session started are its own to end). Run by hand there is no caller. The daemon is reached
//! (and started when it is not running) as the UI does it (`create_live_client`); `guide` and `--help` need none.
//! Parsing is [`args`], the requests [`plan`], the output [`view`].

pub mod args;
pub mod plan;
pub mod view;

use std::io::Read;
use std::process::ExitCode;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use deckhand_client::live::{LiveClient, LiveClientHandlers, create_live_client};
use deckhand_core::agent_guide::GUIDE;
use deckhand_core::display::truncate;
use deckhand_core::error::DhError;
use deckhand_core::jsstr::date::now_ms;
use deckhand_core::plan::summary_lines;
use deckhand_core::session_order::sort_sessions_for_sidebar;
use deckhand_core::tasks::{append_note_item, parse_tasks};
use deckhand_core::tasks_board::{work_key_of, work_label};
use deckhand_daemon::server::{DEFAULT_PREVIEW_COLS, DEFAULT_PREVIEW_ROWS};
use deckhand_protocol::Ordered;
use deckhand_protocol::data::{PlanStatus, SessionRecord, SessionStatus, Task, TaskOp};
use deckhand_protocol::wire::AgentCaller;
use deckhand_sys::git::ensure_git_repo;

use crate::cli::print_line;
use crate::hook::HookEnv;

use args::{Action, Invocation, MoveTo, NewArgs, PlanSource, WorktreeChoice};
use plan::{MovePlan, NewContext, caller_note, create_input, find_session, find_task, plan_move, session_path};
use view::{short_id, task_view};

/// How long `deckhand new` waits for the new session's worktree before printing what it has.
const START_WAIT: Duration = Duration::from_secs(20);

/// How often `note add` re-reads a note that changed while it was saving.
const NOTE_ATTEMPTS: usize = 3;

/// The session the command runs in, from its environment (all three variables set, as for hooks).
pub fn caller_from_env() -> Option<AgentCaller> {
    HookEnv::from_env().map(|env| AgentCaller {
        session_id: env.session_id,
        launch_id: env.launch_id,
        token: env.token,
    })
}

/// Runs `args` (starting with the command word).
pub async fn run(args: &[String]) -> Result<ExitCode, DhError> {
    let Invocation { action, json } = args::parse(args)?;
    match action {
        Action::Help(usage) => print_line(usage),
        Action::Guide => print_line(GUIDE.trim_end()),
        action => {
            let (updates, receiver) = mpsc::unbounded_channel();
            let handlers = LiveClientHandlers {
                on_session_updated: Some(Box::new(move |session| {
                    let _ = updates.send(session);
                })),
                ..LiveClientHandlers::default()
            };
            let client = create_live_client(handlers).await?;
            let mut cx = Cx {
                client,
                updates: receiver,
                caller: caller_from_env(),
                json,
            };
            let result = cx.act(action).await;
            cx.client.close();
            result?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// One command's connection and caller.
struct Cx {
    client: LiveClient,
    updates: mpsc::UnboundedReceiver<SessionRecord>,
    caller: Option<AgentCaller>,
    json: bool,
}

/// Pretty JSON on stdout.
fn print_json(value: &impl Serialize) -> Result<(), DhError> {
    print_line(&serde_json::to_string_pretty(value)?);
    Ok(())
}

impl Cx {
    async fn act(&mut self, action: Action) -> Result<(), DhError> {
        match action {
            Action::New(args) => self.new_session(&args).await,
            Action::Ls => self.ls().await,
            Action::TaskList => self.task_list().await,
            Action::TaskAdd { title, details } => self.task_add(&title, details).await,
            Action::TaskDone(id) => self.task_tick(&id, true).await,
            Action::TaskReopen(id) => self.task_tick(&id, false).await,
            Action::TaskMove { id, to } => self.task_move(&id, &to).await,
            Action::NoteAdd(item) => self.note_add(&item).await,
            Action::Kill(id) => self.end(&id, true).await,
            Action::Remove(id) => self.end(&id, false).await,
            Action::PlanCheck(source) => self.plan_check(&source).await,
            Action::PlanApply(source) => self.plan_apply(&source).await,
            Action::Do(request) => self.hand_to_orchestrator(&request).await,
            Action::Help(_) | Action::Guide => Ok(()),
        }
    }

    /// Every session the daemon has, and the caller's record among them.
    async fn sessions(&self) -> Result<(Vec<SessionRecord>, Option<SessionRecord>), DhError> {
        let sessions = self.client.list_sessions().await?;
        let caller = match self.caller.as_ref() {
            None => None,
            Some(caller) => Some(
                sessions
                    .iter()
                    .find(|session| session.id == caller.session_id)
                    .cloned()
                    .ok_or_else(|| {
                        DhError::msg(format!(
                            "The Deckhand session this runs in ({}) is not known to the daemon",
                            caller.session_id
                        ))
                    })?,
            ),
        };
        Ok((sessions, caller))
    }

    /// The repository the command is about: the current directory's.
    async fn repo_root(&self) -> Result<String, DhError> {
        ensure_git_repo(None).await
    }

    /// The tasks of `cwd`'s repository.
    async fn tasks(&self, cwd: &str) -> Result<Vec<Task>, DhError> {
        Ok(parse_tasks(&self.client.watch_tasks(cwd).await?.text))
    }

    async fn new_session(&mut self, args: &NewArgs) -> Result<(), DhError> {
        let (sessions, caller) = self.sessions().await?;
        let parent = args
            .parent
            .as_deref()
            .map(|id| find_session(id, &sessions))
            .transpose()?
            .cloned();
        let repo_root = match parent.as_ref().or(caller.as_ref()) {
            Some(session) => session.repo_root.clone(),
            None => self.repo_root().await?,
        };
        let tasks = match args.task.as_ref() {
            Some(_) => self.tasks(&repo_root).await?,
            None => Vec::new(),
        };
        let task = args.task.as_deref().map(|id| find_task(id, &tasks)).transpose()?;
        let default_agent = if args.agent.is_none() && caller.is_none() {
            // A repository without a readable config has no default: Claude, as the UI's form.
            self.client
                .project_info(&repo_root)
                .await
                .ok()
                .and_then(|project| project.effective.default_agent)
        } else {
            None
        };
        let existing_path = match args.worktree.as_ref() {
            Some(WorktreeChoice::Path(path)) => Some(
                std::fs::canonicalize(path)
                    .map_err(|error| DhError::msg(format!("--worktree {path}: {error}")))?
                    .to_string_lossy()
                    .into_owned(),
            ),
            _ => None,
        };
        let input = create_input(
            args,
            &NewContext {
                caller: caller.as_ref(),
                repo_root: &repo_root,
                default_agent,
                task,
                existing_path: existing_path.as_deref(),
                sessions: &sessions,
                parent: parent.as_ref(),
            },
        )?;
        // Subscribed first, so the updates that bring its worktree are not missed.
        self.client.subscribe(&input.repo_root).await?;
        let prompt = input.prompt.as_ref().map(|_| input.submit != Some(false));
        let created = self.client.create_session_as(input, self.caller.clone()).await?;
        let session = self.settled(created).await;
        if self.json {
            return print_json(&session);
        }
        print_line(&view::started(&session, prompt));
        Ok(())
    }

    /// `session` once it has its worktree or has left `starting` (or after [`START_WAIT`]).
    async fn settled(&mut self, session: SessionRecord) -> SessionRecord {
        let fallback = session.clone();
        self.all_settled(vec![session]).await.pop().unwrap_or(fallback)
    }

    /// Each of `sessions` once it has its worktree or has left `starting`, waiting [`START_WAIT`] at most in all.
    async fn all_settled(&mut self, mut sessions: Vec<SessionRecord>) -> Vec<SessionRecord> {
        let ready = |session: &SessionRecord| {
            session.status != SessionStatus::Starting
                || session
                    .worktree
                    .as_ref()
                    .is_some_and(|worktree| worktree.path.as_deref().is_some_and(|path| !path.is_empty()))
        };
        let deadline = tokio::time::Instant::now() + START_WAIT;
        while !sessions.iter().all(ready) {
            match tokio::time::timeout_at(deadline, self.updates.recv()).await {
                Ok(Some(update)) => {
                    if let Some(held) = sessions.iter_mut().find(|session| session.id == update.id) {
                        *held = update;
                    }
                }
                Ok(None) | Err(_) => break,
            }
        }
        sessions
    }

    async fn ls(&self) -> Result<(), DhError> {
        let repo_root = self.repo_root().await?;
        let sessions = self.client.subscribe(&repo_root).await?;
        let listed: Vec<SessionRecord> = sessions
            .into_iter()
            .filter(|session| session.archived_at.as_deref().is_none_or(str::is_empty))
            .collect();
        let ordered = sort_sessions_for_sidebar(&listed);
        if self.json {
            return print_json(&ordered);
        }
        let tasks = self.tasks(&repo_root).await.unwrap_or_default();
        let caller = self.caller.as_ref().map(|caller| caller.session_id.as_str());
        print_line(&view::session_table(&ordered, &tasks, caller));
        Ok(())
    }

    /// The repository's sessions (for the work tasks name) and tasks.
    async fn board(&self) -> Result<(String, Vec<SessionRecord>, Vec<Task>), DhError> {
        let repo_root = self.repo_root().await?;
        let sessions = self.client.subscribe(&repo_root).await?;
        let tasks = self.tasks(&repo_root).await?;
        Ok((repo_root, sessions, tasks))
    }

    async fn task_list(&self) -> Result<(), DhError> {
        let (_, sessions, tasks) = self.board().await?;
        if self.json {
            let views: Vec<_> = tasks.iter().map(|task| task_view(task, &sessions)).collect();
            return print_json(&views);
        }
        print_line(&view::task_list(&tasks, &sessions, now_ms()));
        Ok(())
    }

    /// Applies `op` to the repository's tasks and returns the list after it.
    async fn apply(&self, repo_root: &str, op: TaskOp) -> Result<Vec<Task>, DhError> {
        Ok(parse_tasks(&self.client.task_op(repo_root, op).await?.text))
    }

    /// Prints `task` (looked up again in `tasks` by ID) as JSON, or `message`.
    fn report_task(&self, id: &str, tasks: &[Task], sessions: &[SessionRecord], message: &str) -> Result<(), DhError> {
        if self.json {
            let task = tasks.iter().find(|task| task.id == id);
            return print_json(&task.map(|task| task_view(task, sessions)));
        }
        print_line(message);
        Ok(())
    }

    async fn task_add(&self, title: &str, details: Option<String>) -> Result<(), DhError> {
        let (repo_root, sessions, before) = self.board().await?;
        let op = TaskOp::Add {
            title: title.to_owned(),
            body: details,
            link: None,
            id: None,
            from: None,
            parent: None,
            at: None,
        };
        let after = self.apply(&repo_root, op).await?;
        let added = after
            .iter()
            .rev()
            .find(|task| !before.iter().any(|old| old.id == task.id))
            .ok_or_else(|| DhError::msg("The task was saved but is not in the list"))?;
        let message = format!("Added to the backlog: {} {}", short_id(&added.id), added.title);
        self.report_task(&added.id, &after, &sessions, &message)
    }

    async fn task_tick(&self, id: &str, done: bool) -> Result<(), DhError> {
        let (repo_root, sessions, tasks) = self.board().await?;
        let task = find_task(id, &tasks)?.clone();
        let (verb, already) = if done {
            ("Ticked", "already done")
        } else {
            ("Reopened", "not done")
        };
        if task.done == done {
            let message = format!("It is {already}: {} {}", short_id(&task.id), task.title);
            return self.report_task(&task.id, &tasks, &sessions, &message);
        }
        let after = self.apply(&repo_root, TaskOp::Toggle { id: task.id.clone() }).await?;
        let message = format!("{verb}: {} {}", short_id(&task.id), task.title);
        self.report_task(&task.id, &after, &sessions, &message)
    }

    async fn task_move(&self, id: &str, to: &MoveTo) -> Result<(), DhError> {
        let (repo_root, sessions, tasks) = self.board().await?;
        let task = find_task(id, &tasks)?.clone();
        let target = match to {
            MoveTo::Backlog => None,
            MoveTo::Session(session) => Some(find_session(session, &sessions)?),
        };
        let title = truncate(&task.title, 40);
        let label = target.map(|target| work_label(&work_key_of(target), &sessions));
        let (after, message) = match plan_move(&task, target, &tasks) {
            MovePlan::Already => (tasks.clone(), "It is already there".to_owned()),
            MovePlan::Step { section } => {
                let doc = self.client.task_to_note(&repo_root, &task.id, &section).await?;
                let message = format!(
                    "Now a step in the note of {} (it has its task already): {title}",
                    label.unwrap_or_default()
                );
                (parse_tasks(&doc.text), message)
            }
            MovePlan::Assign { link } => {
                let backlog = link.is_none();
                let after = self
                    .apply(
                        &repo_root,
                        TaskOp::Assign {
                            id: task.id.clone(),
                            link,
                        },
                    )
                    .await?;
                let message = if backlog {
                    format!("Moved to the backlog: {title}")
                } else {
                    format!(
                        "Now the task of {} (merging or finishing that work ticks it): {title}",
                        label.unwrap_or_default()
                    )
                };
                (after, message)
            }
        };
        self.report_task(&task.id, &after, &sessions, &message)
    }

    async fn note_add(&self, item: &str) -> Result<(), DhError> {
        if self.caller.is_none() {
            return Err(DhError::msg(
                "deckhand note add needs a Deckhand session to act for: run it inside one (DECKHAND_SESSION_ID is not set)",
            ));
        }
        let mut created_file = false;
        for _ in 0..NOTE_ATTEMPTS {
            let (sessions, caller) = self.sessions().await?;
            let Some(caller) = caller else { break };
            let Some(note) = caller_note(&caller, &sessions)? else {
                if created_file {
                    return Err(DhError::msg("This session has no note"));
                }
                // A session's own note has no file until it is first opened.
                let section = deckhand_protocol::data::NoteSection::Session;
                self.client.open_note(&caller.id, section).await?;
                created_file = true;
                continue;
            };
            let text = append_note_item(&note.text, item, "");
            let saved = self
                .client
                .save_note(
                    &note.session_id,
                    note.section,
                    &text,
                    &note.revision,
                    note.note_id.as_deref(),
                )
                .await?;
            if !saved.saved {
                continue;
            }
            let section = work_key_of(&caller);
            if self.json {
                return print_json(&json!({"work": section, "item": item, "text": text}));
            }
            print_line(&format!(
                "Added a step to the note of {}: {}",
                work_label(&section, &sessions),
                truncate(item, 60)
            ));
            return Ok(());
        }
        Err(DhError::msg("The note kept changing while saving; try again"))
    }

    /// The plan's JSON, from its file or stdin.
    fn read_plan(source: &PlanSource) -> Result<Value, DhError> {
        let text = match source {
            PlanSource::File(path) => std::fs::read_to_string(path)
                .map_err(|error| DhError::msg(format!("Cannot read the plan {path}: {error}")))?,
            PlanSource::Stdin => {
                let mut text = String::new();
                std::io::stdin()
                    .read_to_string(&mut text)
                    .map_err(|error| DhError::msg(format!("Cannot read the plan from stdin: {error}")))?;
                text
            }
        };
        serde_json::from_str(&text).map_err(|error| DhError::msg(format!("The plan is not valid JSON: {error}")))
    }

    /// The repository a plan is for: the caller's (the daemon reads it from the caller), else the current directory's.
    async fn plan_cwd(&self) -> Result<String, DhError> {
        match self.caller {
            Some(_) => Ok(String::new()),
            None => self.repo_root().await,
        }
    }

    async fn plan_check(&self, source: &PlanSource) -> Result<(), DhError> {
        let plan = Self::read_plan(source)?;
        let cwd = self.plan_cwd().await?;
        let summary = self.client.plan_check(&cwd, plan, self.caller.clone()).await?;
        if self.json {
            return print_json(&summary);
        }
        let mut lines = summary_lines(&summary);
        lines.push(
            "The plan is valid. Nothing was created: deckhand plan apply asks for confirmation in Deckhand.".into(),
        );
        print_line(&lines.join("\n"));
        Ok(())
    }

    async fn plan_apply(&mut self, source: &PlanSource) -> Result<(), DhError> {
        let plan = Self::read_plan(source)?;
        let cwd = self.plan_cwd().await?;
        let repo_root = match self.caller.as_ref() {
            Some(_) => self
                .sessions()
                .await?
                .1
                .map(|caller| caller.repo_root)
                .unwrap_or_default(),
            None => cwd.clone(),
        };
        // Subscribed first, so the updates that bring the new worktrees are not missed.
        self.client.subscribe(&repo_root).await?;
        eprintln!(
            "Waiting for confirmation in Deckhand (open it in this repository: enter creates the plan, esc cancels it; \
             it expires in 15 minutes)..."
        );
        let mut outcome = self.client.plan_apply(&cwd, plan, self.caller.clone()).await?;
        let records: Vec<SessionRecord> = outcome
            .sessions
            .iter()
            .map(|created| (*created.session).clone())
            .collect();
        let settled = self.all_settled(records).await;
        for (created, session) in outcome.sessions.iter_mut().zip(settled) {
            created.session = Ordered::from(session);
        }
        if self.json {
            print_json(&outcome)?;
        } else {
            print_line(&view::plan_outcome(&outcome));
        }
        match outcome.status {
            PlanStatus::Created => Ok(()),
            PlanStatus::Cancelled => Err(DhError::msg("Cancelled in Deckhand")),
            PlanStatus::Expired => Err(DhError::msg("The plan expired unanswered")),
            PlanStatus::Failed => Err(DhError::msg("The plan was created only in part")),
        }
    }

    /// `deckhand do`: the request goes to the repository's orchestrator (started with it, or typed into it).
    async fn hand_to_orchestrator(&self, request: &str) -> Result<(), DhError> {
        if self.caller.is_some() {
            return Err(DhError::msg(
                "deckhand do is for you, from a terminal in the repository, not for a session: ask the user to run it",
            ));
        }
        let repo_root = self.repo_root().await?;
        let result = self
            .client
            .open_orchestrator(&repo_root, Some(request), DEFAULT_PREVIEW_COLS, DEFAULT_PREVIEW_ROWS)
            .await?;
        if self.json {
            return print_json(&result);
        }
        print_line(&view::orchestrator(&result));
        Ok(())
    }

    /// `kill` (`stop`: end it) or `remove` the session `id` names.
    async fn end(&self, id: &str, stop: bool) -> Result<(), DhError> {
        let (sessions, _) = self.sessions().await?;
        let session = find_session(id, &sessions)?.clone();
        if stop {
            self.client.kill_session_as(&session.id, self.caller.clone()).await?;
        } else {
            self.client.remove_session_as(&session.id, self.caller.clone()).await?;
        }
        if self.json {
            return print_json(&json!({"ok": true, "sessionId": session.id}));
        }
        let verb = if stop { "Ending" } else { "Removed" };
        print_line(&format!("{verb} {} {}", short_id(&session.id), session_path(&session)));
        Ok(())
    }
}
