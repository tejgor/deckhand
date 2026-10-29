//! `deckhand new|ls|task|note|kill|remove|guide`: Deckhand actions from a shell, for the agents Deckhand runs (not in
//! the TS).
//!
//! Inside a session the command acts for that session, the caller: its `DECKHAND_SESSION_ID`, `DECKHAND_LAUNCH_ID`
//! and `DECKHAND_HOOK_TOKEN` go with `create`, `kill` and `remove`, and the daemon checks them as it does hook
//! callbacks (a session's sub-sessions are its own to end). Run by hand there is no caller. The daemon is reached
//! (and started when it is not running) as the UI does it (`create_live_client`); `guide` and `--help` need none.
//! Parsing is [`args`], the requests [`plan`], the output [`view`].

pub mod args;
pub mod plan;
pub mod view;

use std::process::ExitCode;
use std::time::Duration;

use serde::Serialize;
use serde_json::json;
use tokio::sync::mpsc;

use deckhand_client::live::{LiveClient, LiveClientHandlers, create_live_client};
use deckhand_core::agent_guide::GUIDE;
use deckhand_core::display::truncate;
use deckhand_core::error::DhError;
use deckhand_core::jsstr::date::now_ms;
use deckhand_core::session_order::sort_sessions_for_sidebar;
use deckhand_core::tasks::{append_note_item, parse_tasks};
use deckhand_core::tasks_board::{work_key_of, work_label};
use deckhand_protocol::data::{SessionRecord, SessionStatus, Task, TaskOp};
use deckhand_protocol::wire::AgentCaller;
use deckhand_sys::git::ensure_git_repo;

use crate::cli::print_line;
use crate::hook::HookEnv;

use args::{Action, Invocation, MoveTo, NewArgs, WorktreeChoice};
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
        let repo_root = match caller.as_ref() {
            Some(caller) => caller.repo_root.clone(),
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
    async fn settled(&mut self, mut session: SessionRecord) -> SessionRecord {
        let ready = |session: &SessionRecord| {
            session.status != SessionStatus::Starting
                || session
                    .worktree
                    .as_ref()
                    .is_some_and(|worktree| worktree.path.as_deref().is_some_and(|path| !path.is_empty()))
        };
        let deadline = tokio::time::Instant::now() + START_WAIT;
        while !ready(&session) {
            match tokio::time::timeout_at(deadline, self.updates.recv()).await {
                Ok(Some(update)) if update.id == session.id => session = update,
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => break,
            }
        }
        session
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
