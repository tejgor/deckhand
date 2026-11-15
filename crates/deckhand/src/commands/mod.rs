//! `deckhand new|ls|task|step|note|send|reply|kill|remove|guide|plan|do`: Deckhand actions from a shell, for the
//! agents Deckhand runs (not in the TS), the orchestrator's `plan` and your `do`.
//!
//! Inside a session the command acts for that session, the caller: its `DECKHAND_SESSION_ID`, `DECKHAND_LAUNCH_ID`
//! and `DECKHAND_HOOK_TOKEN` go with `create`, `kill` and `remove`, and the daemon checks them as it does hook
//! callbacks (the sessions a session started are its own to end). Run by hand there is no caller. The daemon is reached
//! (and started when it is not running) as the UI does it (`create_live_client`); `guide` and `--help` need none.
//! Parsing is [`args`], the requests [`plan`], the output [`view`].

pub mod args;
pub mod plan;
pub mod view;

use std::io::{IsTerminal, Read};
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
use deckhand_core::notes::note_revision;
use deckhand_core::plan::summary_lines;
use deckhand_core::session_order::sort_sessions_for_sidebar;
use deckhand_core::task_list::{Line, TaskList};
use deckhand_core::work_notes::append_paragraph;
use deckhand_daemon::server::{DEFAULT_PREVIEW_COLS, DEFAULT_PREVIEW_ROWS};
use deckhand_protocol::Ordered;
use deckhand_protocol::data::{
    MessageRecord, NoteRef, PlanStatus, SessionRecord, SessionStatus, Task, TasksDoc, WaitOutcomeKind, WorkNote,
    WorkNoteKind,
};
use deckhand_protocol::wire::AgentCaller;
use deckhand_sys::git::ensure_git_repo;

use crate::cli::print_line;
use crate::hook::HookEnv;

use args::{Action, Invocation, MoveTo, NewArgs, PlanSource, SendArgs, TextSource, WorktreeChoice};
use plan::{
    MovePlan, MoveTarget, NewContext, caller_tree, create_input, find_line, find_session, find_task, is_archived,
    plan_move, session_path,
};
use view::{line_view, short_id};

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

/// Runs `args` (starting with the command word). A `-` text is read from stdin first, so a refusal comes before
/// anything reaches the daemon.
pub async fn run(args: &[String]) -> Result<ExitCode, DhError> {
    let Invocation { mut action, json } = args::parse(args)?;
    if let Some(text) = action.stdin_text() {
        let stdin = std::io::stdin();
        let piped = !stdin.is_terminal();
        *text.slot = TextSource::Given(piped_text(text.what, text.form, piped, stdin.lock())?);
    }
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
                exit: None,
            };
            let result = cx.act(action).await;
            cx.client.close();
            result?;
            if let Some(code) = cx.exit {
                return Ok(ExitCode::from(code));
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// What a `-` reads (`what`, for `form`): the text piped in, without the newline a heredoc ends with. Refused when
/// nothing is piped in (stdin is a terminal: reading would wait for the user, or closed) or only blank lines came.
pub fn piped_text(what: &str, form: &str, piped: bool, mut stdin: impl Read) -> Result<String, DhError> {
    let mut text = String::new();
    if piped {
        stdin
            .read_to_string(&mut text)
            .map_err(|error| DhError::msg(format!("Cannot read {what} from stdin: {error}")))?;
    }
    if text.trim().is_empty() {
        return Err(DhError::msg(format!(
            "{form} reads {what} from stdin, but nothing was piped in. Pass it in a quoted heredoc, EOF alone on \
             the last line:\n{form} <<'EOF'\n<text>\nEOF\nor give the text itself in place of -."
        )));
    }
    Ok(text.trim_end_matches('\n').to_owned())
}

/// `send --wait`'s exit status for how the wait ended (0: a reply).
pub fn wait_exit_status(kind: WaitOutcomeKind) -> u8 {
    match kind {
        WaitOutcomeKind::Replied => 0,
        WaitOutcomeKind::Timeout => 2,
        WaitOutcomeKind::Waiting => 3,
        WaitOutcomeKind::Ended => 4,
        WaitOutcomeKind::Exited => 5,
        WaitOutcomeKind::NotStarted => 6,
        WaitOutcomeKind::Cancelled => 7,
    }
}

/// One command's connection and caller.
struct Cx {
    client: LiveClient,
    updates: mpsc::UnboundedReceiver<SessionRecord>,
    caller: Option<AgentCaller>,
    json: bool,
    /// An exit status other than success without an error (`send --wait` ending early).
    exit: Option<u8>,
}

/// A text [`run`] has read: a `-` left unread here would be a bug, refused rather than sent empty.
fn given(source: &TextSource) -> Result<&str, DhError> {
    source
        .given()
        .ok_or_else(|| DhError::msg("The text from stdin was not read"))
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
            Action::Ls { all } => self.ls(all).await,
            Action::TaskList => self.task_list().await,
            Action::TaskAdd { title, details } => {
                self.task_add(&title, details.as_ref().and_then(TextSource::given))
                    .await
            }
            Action::TaskDone(id) => self.task_tick(&id, true).await,
            Action::TaskReopen(id) => self.task_tick(&id, false).await,
            Action::TaskMove { id, to } => self.task_move(&id, &to).await,
            Action::StepAdd(text) => self.step_add(&text).await,
            Action::StepDone(id) => self.step_tick(&id, true).await,
            Action::StepReopen(id) => self.step_tick(&id, false).await,
            Action::StepList => self.step_list().await,
            Action::NoteAdd(text) => self.note_add(given(&text)?).await,
            Action::Send(args) => self.send(&args).await,
            Action::CancelMessage(id) => self.cancel_message(&id).await,
            Action::Reply { id, text } => self.reply(&id, &text).await,
            Action::Kill(id) => self.end(&id, true).await,
            Action::Remove(id) => self.end(&id, false).await,
            Action::PlanCheck(source) => self.plan_check(&source).await,
            Action::PlanApply(source) => self.plan_apply(&source).await,
            Action::Do(request) => self.hand_to_orchestrator(given(&request)?).await,
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

    /// The list of `cwd`'s repository, with its notes (`watch-tasks`).
    async fn list(&self, cwd: &str) -> Result<TasksDoc, DhError> {
        self.client.watch_tasks(cwd).await
    }

    /// Every line of `cwd`'s repository's list, as tasks (any of them can be started on).
    async fn tasks(&self, cwd: &str) -> Result<Vec<Task>, DhError> {
        let doc = self.list(cwd).await?;
        Ok(TaskList::parse(&doc.text).lines().iter().map(Line::task).collect())
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

    /// Inside a session (unless `all`): the caller's tree, from every session the daemon has; else, or when the
    /// daemon does not know the caller, the sessions of the current checkout.
    async fn ls(&self, all: bool) -> Result<(), DhError> {
        if let Some(caller) = self.caller.as_ref().filter(|_| !all) {
            let sessions = self.client.list_sessions().await?;
            if let Some((root, tree)) = caller_tree(&sessions, &caller.session_id) {
                if self.json {
                    return print_json(&tree);
                }
                let list = self
                    .list(&root.repo_root)
                    .await
                    .map(|doc| TaskList::parse(&doc.text))
                    .unwrap_or_default();
                print_line(&view::tree_table(root, &tree, &list, &caller.session_id));
                return Ok(());
            }
        }
        let repo_root = self.repo_root().await?;
        let sessions = self.client.subscribe(&repo_root).await?;
        let listed: Vec<SessionRecord> = sessions.into_iter().filter(|session| !is_archived(session)).collect();
        let ordered = sort_sessions_for_sidebar(&listed);
        if self.json {
            return print_json(&ordered);
        }
        let list = self
            .list(&repo_root)
            .await
            .map(|doc| TaskList::parse(&doc.text))
            .unwrap_or_default();
        let caller = self.caller.as_ref().map(|caller| caller.session_id.as_str());
        print_line(&view::session_table(&ordered, &list, caller));
        Ok(())
    }

    /// The repository's sessions (for who is on what) and its list with its notes.
    async fn board(&self) -> Result<(String, Vec<SessionRecord>, TasksDoc), DhError> {
        let repo_root = self.repo_root().await?;
        let sessions = self.client.subscribe(&repo_root).await?;
        let doc = self.list(&repo_root).await?;
        Ok((repo_root, sessions, doc))
    }

    async fn task_list(&self) -> Result<(), DhError> {
        let (_, sessions, doc) = self.board().await?;
        let list = TaskList::parse(&doc.text);
        let notes = doc.notes.unwrap_or_default();
        if self.json {
            let views: Vec<_> = list.lines().iter().map(|line| line_view(line, &list, &notes)).collect();
            return print_json(&views);
        }
        print_line(&view::task_tree(&list, &sessions, &notes, now_ms()));
        Ok(())
    }

    /// `op` (as JSON) on the list of `repo_root`; the list after it.
    async fn apply(&self, repo_root: &str, op: Value) -> Result<TaskList, DhError> {
        Ok(TaskList::parse(&self.client.task_op_value(repo_root, op).await?.text))
    }

    /// Prints line `id` of `list` as JSON, or `message`.
    fn report_line(&self, id: &str, list: &TaskList, message: &str) -> Result<(), DhError> {
        if self.json {
            let view = list.get(id).map(|line| line_view(line, list, &[]));
            return print_json(&view);
        }
        print_line(message);
        Ok(())
    }

    /// The line added by the change from `before` to `after` (the one whose ID is new).
    fn added(before: &TaskList, after: &TaskList) -> Result<Line, DhError> {
        after
            .lines()
            .iter()
            .find(|line| before.get(&line.id).is_none())
            .cloned()
            .ok_or_else(|| DhError::msg("The line was saved but is not in the list"))
    }

    async fn task_add(&self, title: &str, details: Option<&str>) -> Result<(), DhError> {
        let (repo_root, _, doc) = self.board().await?;
        let before = TaskList::parse(&doc.text);
        let after = self
            .apply(&repo_root, json!({"type": "add", "title": title, "note": details}))
            .await?;
        let added = Self::added(&before, &after)?;
        let message = format!("Added to the backlog: {} {}", short_id(&added.id), added.title);
        self.report_line(&added.id, &after, &message)
    }

    /// Ticks (`done`) or reopens one of `lines` of the list of `repo_root`.
    async fn tick(&self, repo_root: &str, line: &Line, done: bool, list: &TaskList) -> Result<(), DhError> {
        let (verb, already) = if done {
            ("Ticked", "already done")
        } else {
            ("Reopened", "not done")
        };
        if line.done == done {
            let message = format!("It is {already}: {} {}", short_id(&line.id), line.title);
            return self.report_line(&line.id, list, &message);
        }
        let kind = if done { "tick" } else { "reopen" };
        let after = self.apply(repo_root, json!({"type": kind, "id": line.id})).await?;
        let mut message = format!("{verb}: {} {}", short_id(&line.id), line.title);
        if let Some(parent) = line.parent.as_deref().and_then(|parent| after.get(parent))
            && parent.done
            && done
        {
            message.push_str(&format!(" (and {}: every line under it is ticked)", parent.title));
        }
        self.report_line(&line.id, &after, &message)
    }

    async fn task_tick(&self, id: &str, done: bool) -> Result<(), DhError> {
        let (repo_root, _, doc) = self.board().await?;
        let list = TaskList::parse(&doc.text);
        let line = find_line(id, list.lines(), "deckhand task list")?.clone();
        self.tick(&repo_root, &line, done, &list).await
    }

    async fn task_move(&self, id: &str, to: &MoveTo) -> Result<(), DhError> {
        let (repo_root, sessions, doc) = self.board().await?;
        let list = TaskList::parse(&doc.text);
        let line = find_line(id, list.lines(), "deckhand task list")?.clone();
        let target = match to {
            MoveTo::Backlog => MoveTarget::Backlog,
            MoveTo::Session(to) => match (find_session(to, &sessions), find_line(to, list.lines(), "")) {
                (Ok(session), Err(_)) => MoveTarget::Session(session),
                (Err(_), Ok(under)) => MoveTarget::Line(under),
                (Ok(_), Ok(_)) => {
                    return Err(DhError::msg(format!(
                        "{to} names a session and a line; give more of it"
                    )));
                }
                (Err(_), Err(_)) => {
                    return Err(DhError::msg(format!(
                        "No session or line {to} (see deckhand ls and deckhand task list)"
                    )));
                }
            },
        };
        let title = truncate(&line.title, 40);
        let (after, message) = match plan_move(&line, target, &list) {
            MovePlan::Already => (list.clone(), "It is already there".to_owned()),
            MovePlan::SessionOn { s } => {
                let after = self
                    .apply(&repo_root, json!({"type": "session-on", "id": line.id, "s": s}))
                    .await?;
                let name = match target {
                    MoveTarget::Session(session) => session_path(session).to_owned(),
                    _ => s,
                };
                let message = format!("Now the line of {name} (merging or finishing that work ticks it): {title}");
                (after, message)
            }
            MovePlan::MoveTo { parent, unhook } => {
                if let Some(s) = unhook {
                    self.apply(&repo_root, json!({"type": "session-off", "s": s})).await?;
                }
                let after = self
                    .apply(&repo_root, json!({"type": "move-to", "id": line.id, "parent": parent}))
                    .await?;
                let message = match target {
                    MoveTarget::Backlog => format!("Moved to the backlog: {title}"),
                    MoveTarget::Session(session) => {
                        format!("Now a step of the line of {}: {title}", session_path(session))
                    }
                    MoveTarget::Line(under) => format!("Now under {}: {title}", truncate(&under.title, 40)),
                };
                (after, message)
            }
        };
        self.report_line(&line.id, &after, &message)
    }

    /// The caller (`what` needs one) and the sessions the daemon has.
    async fn caller(&self, what: &str) -> Result<(SessionRecord, Vec<SessionRecord>), DhError> {
        let needs = || {
            DhError::msg(format!(
                "deckhand {what} needs a Deckhand session to act for: run it inside one (DECKHAND_SESSION_ID is not \
                 set)"
            ))
        };
        if self.caller.is_none() {
            return Err(needs());
        }
        let (sessions, caller) = self.sessions().await?;
        Ok((caller.ok_or_else(needs)?, sessions))
    }

    /// The caller's line, made when it has none (lines on demand); the list after that.
    async fn caller_line(&self, caller: &SessionRecord) -> Result<(Line, TaskList), DhError> {
        let list = self
            .apply(&caller.repo_root, json!({"type": "session-line", "s": caller.id}))
            .await?;
        let line = list
            .line_of_session(&caller.id)
            .cloned()
            .ok_or_else(|| DhError::msg("This session's line was made but is not in the list"))?;
        Ok((line, list))
    }

    async fn step_add(&self, text: &str) -> Result<(), DhError> {
        let (caller, _) = self.caller("step add").await?;
        let (line, before) = self.caller_line(&caller).await?;
        let after = self
            .apply(
                &caller.repo_root,
                json!({"type": "add", "title": text, "parent": line.id}),
            )
            .await?;
        let added = Self::added(&before, &after)?;
        let message = format!(
            "Added a step to {}: {} {}",
            truncate(&line.title, 40),
            short_id(&added.id),
            added.title
        );
        self.report_line(&added.id, &after, &message)
    }

    async fn step_tick(&self, id: &str, done: bool) -> Result<(), DhError> {
        let what = if done { "step done" } else { "step reopen" };
        let (caller, _) = self.caller(what).await?;
        let list = TaskList::parse(&self.list(&caller.repo_root).await?.text);
        let Some(line) = list.line_of_session(&caller.id) else {
            return Err(DhError::msg(
                "This session has no steps yet (deckhand step add adds one)",
            ));
        };
        let steps: Vec<Line> = list.subtree(&line.id).into_iter().skip(1).cloned().collect();
        let step = find_line(id, &steps, "deckhand step list")?.clone();
        self.tick(&caller.repo_root, &step, done, &list).await
    }

    async fn step_list(&self) -> Result<(), DhError> {
        let (caller, sessions) = self.caller("step list").await?;
        let doc = self.list(&caller.repo_root).await?;
        let list = TaskList::parse(&doc.text);
        let notes = doc.notes.unwrap_or_default();
        let line = list.line_of_session(&caller.id);
        if self.json {
            let views: Vec<_> = line
                .map(|line| list.subtree(&line.id))
                .unwrap_or_default()
                .into_iter()
                .map(|line| line_view(line, &list, &notes))
                .collect();
            return print_json(&views);
        }
        print_line(&match line {
            Some(line) => view::step_tree(&list, &line.id, &sessions, &notes),
            None => "This session has no line yet: deckhand step add gives it one, with its first step".to_owned(),
        });
        Ok(())
    }

    async fn note_add(&self, text: &str) -> Result<(), DhError> {
        let (caller, _) = self.caller("note add").await?;
        let (line, _) = self.caller_line(&caller).await?;
        for _ in 0..NOTE_ATTEMPTS {
            let doc = self.list(&caller.repo_root).await?;
            let current: Option<WorkNote> = doc
                .notes
                .unwrap_or_default()
                .into_iter()
                .find(|note| note.kind == WorkNoteKind::Line && note.id == line.id);
            let (before, revision) = match current {
                Some(note) => (note.text, note.revision),
                None => (String::new(), note_revision("")),
            };
            let next = append_paragraph(&before, text);
            let note = NoteRef {
                kind: WorkNoteKind::Line,
                id: line.id.clone(),
            };
            let saved = self.client.save_work_note(note, &next, &revision).await?;
            if !saved.saved {
                continue;
            }
            if self.json {
                return print_json(&json!({"line": line.id, "text": saved.note.text}));
            }
            print_line(&format!(
                "Added to the note of {}: {}",
                truncate(&line.title, 40),
                truncate(text, 60)
            ));
            return Ok(());
        }
        Err(DhError::msg("The note kept changing while saving; try again"))
    }

    async fn send(&mut self, args: &SendArgs) -> Result<(), DhError> {
        let text = given(&args.text)?;
        let (sessions, _) = self.sessions().await?;
        let target = find_session(&args.to, &sessions)?.clone();
        let result = self
            .client
            .send_message(&target.id, text, args.wait, self.caller.clone())
            .await?;
        if let Some(outcome) = &result.outcome {
            let code = wait_exit_status(outcome.kind);
            if code != 0 {
                self.exit = Some(code);
            }
        }
        if self.json {
            return print_json(&result);
        }
        print_line(&view::sent(&result, &target, args.wait, self.caller.is_some()));
        Ok(())
    }

    async fn cancel_message(&self, id: &str) -> Result<(), DhError> {
        let cancelled = self.client.cancel_message(id, self.caller.clone()).await?;
        if self.json {
            return print_json(&cancelled);
        }
        print_line(&format!(
            "Cancelled message {} to {}",
            cancelled.id,
            view::quoted_title(cancelled.to_title.as_deref())
        ));
        Ok(())
    }

    async fn reply(&self, id: &str, text: &TextSource) -> Result<(), DhError> {
        let reply: MessageRecord = self.client.reply_message(id, given(text)?, self.caller.clone()).await?;
        if self.json {
            return print_json(&reply);
        }
        print_line(&view::replied(&reply));
        Ok(())
    }

    /// The plan's JSON, from its file or stdin.
    fn read_plan(source: &PlanSource) -> Result<Value, DhError> {
        let text = match source {
            PlanSource::File(path) => std::fs::read_to_string(path)
                .map_err(|error| DhError::msg(format!("Cannot read the plan {path}: {error}")))?,
            PlanSource::Stdin => {
                let stdin = std::io::stdin();
                let piped = !stdin.is_terminal();
                piped_text("the plan", "deckhand plan check|apply -", piped, stdin.lock())?
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

    /// `deckhand do`: a new orchestrator for the idea, always (one per idea).
    async fn hand_to_orchestrator(&self, request: &str) -> Result<(), DhError> {
        if self.caller.is_some() {
            return Err(DhError::msg(
                "deckhand do is for you, from a terminal in the repository, not for a session: ask the user to run it",
            ));
        }
        let repo_root = self.repo_root().await?;
        let result = self
            .client
            .open_orchestrator(&repo_root, request, DEFAULT_PREVIEW_COLS, DEFAULT_PREVIEW_ROWS)
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

#[cfg(test)]
mod tests {
    use super::*;

    const FORM: &str = "deckhand new --prompt -";

    #[test]
    fn a_dash_takes_what_is_piped_in_without_the_heredoc_s_last_newline() {
        let text = piped_text(
            "the prompt",
            FORM,
            true,
            "Fix the login.\n\n  Then \"test\" it.\n".as_bytes(),
        );
        assert_eq!(text.unwrap(), "Fix the login.\n\n  Then \"test\" it.");
    }

    #[test]
    fn a_dash_with_nothing_piped_in_is_refused_with_the_exact_form() {
        let refusal = |piped: bool, input: &str| {
            piped_text("the prompt", FORM, piped, input.as_bytes())
                .unwrap_err()
                .to_string()
        };
        let expected = "deckhand new --prompt - reads the prompt from stdin, but nothing was piped in. Pass it in a \
                        quoted heredoc, EOF alone on the last line:\ndeckhand new --prompt - <<'EOF'\n<text>\nEOF\nor \
                        give the text itself in place of -.";
        // Empty, or only blank lines.
        assert_eq!(refusal(true, ""), expected);
        assert_eq!(refusal(true, "\n  \n"), expected);
        // A terminal: nothing is read (it would wait for the user), whatever it holds.
        assert_eq!(refusal(false, "typed"), expected);
    }
}
