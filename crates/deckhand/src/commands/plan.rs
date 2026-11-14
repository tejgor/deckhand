//! What the commands send, worked out from their arguments and what the daemon said (no I/O here).

use deckhand_core::error::DhError;
use deckhand_core::jsstr::js_len;
use deckhand_core::task_list::Line;
pub use deckhand_core::task_list::{MovePlan, MoveTarget, plan_move};
use deckhand_daemon::server::{DEFAULT_PREVIEW_COLS, DEFAULT_PREVIEW_ROWS};
use deckhand_protocol::data::{
    CreateSessionInput, ProgramKey, SessionRecord, SessionStatus, SubSessionKind, Task, WorktreeMode,
};
use deckhand_ui::app::{fork_stays_in_parent, session_title_from_task};

use super::args::{NewArgs, WorktreeChoice};

/// The longest session name the daemon takes.
const MAX_TITLE: usize = 64;

/// What `deckhand new` builds its request from, besides its arguments.
#[derive(Clone, Copy, Debug)]
pub struct NewContext<'a> {
    /// The session the command runs in (from its environment), if any.
    pub caller: Option<&'a SessionRecord>,
    /// The current directory's repository (used without a caller).
    pub repo_root: &'a str,
    /// The repository's `defaultAgent`, if set.
    pub default_agent: Option<ProgramKey>,
    /// The `--task` found in the task list.
    pub task: Option<&'a Task>,
    /// `--worktree <path>`, made absolute.
    pub existing_path: Option<&'a str>,
    /// Every session the daemon has, so a name made up here does not clash with an active one.
    pub sessions: &'a [SessionRecord],
    /// `--parent`, found among them (the daemon checks the caller may start sessions under it).
    pub parent: Option<&'a SessionRecord>,
}

fn needs_caller(what: &str) -> DhError {
    DhError::msg(format!(
        "{what} needs a Deckhand session to act for: run it inside one (DECKHAND_SESSION_ID is not set)"
    ))
}

/// The `create` input for `deckhand new` (see `NEW_USAGE`), built as the UI's form does (`submit_create`).
///
/// - A sub-session of `--parent`, else of the caller unless `--top-level` or no caller. `--task` puts it on that line
///   (any open line nobody is on), wherever it runs.
/// - The new session starts from its parent's (or fork source's) directory, else the repository's checkout.
///   `--worktree same` is the parent's worktree (else the caller's).
/// - The agent: `--agent`, the caller's, the repository's default, else Claude; a fork keeps the caller's.
pub fn create_input(args: &NewArgs, cx: &NewContext<'_>) -> Result<CreateSessionInput, DhError> {
    let caller = cx.caller;
    if args.fork && caller.is_none() {
        return Err(needs_caller("--fork"));
    }
    let top_level = args.top_level || caller.is_none();
    let parent = match cx.parent {
        Some(parent) => Some(parent),
        None => caller.filter(|_| !top_level),
    };
    let program = match (args.fork, caller) {
        (true, Some(source)) => match args.agent {
            Some(agent) if agent != source.program => {
                return Err(DhError::msg(format!(
                    "A fork keeps this session's agent ({}); leave out --agent",
                    source.program
                )));
            }
            _ => source.program,
        },
        _ => args
            .agent
            .or(caller.map(|caller| caller.program))
            .or(cx.default_agent)
            .unwrap_or(ProgramKey::Claude),
    };
    let kind = args.fork.then_some(SubSessionKind::Forked);
    let repo_root = cx
        .parent
        .or(caller)
        .map_or(cx.repo_root, |session| session.repo_root.as_str());
    // Where it starts from: its parent, or the session a top-level fork copies (as the UI's `create_origin`).
    let origin = parent.or(caller.filter(|_| args.fork));
    let origin_cwd = origin.map_or(repo_root, |origin| origin.cwd.as_str());
    let default_choice = if fork_stays_in_parent(Some(program), kind) {
        WorktreeChoice::Same
    } else {
        WorktreeChoice::New
    };
    let (worktree_mode, cwd, existing_worktree_path) = match args.worktree.clone().unwrap_or(default_choice) {
        WorktreeChoice::New => (WorktreeMode::New, origin_cwd, None),
        WorktreeChoice::Same => match cx.parent.or(caller) {
            Some(shared) => (WorktreeMode::None, shared.cwd.as_str(), None),
            None => return Err(needs_caller("--worktree same")),
        },
        WorktreeChoice::None => (WorktreeMode::None, repo_root, None),
        WorktreeChoice::Path(path) => (
            WorktreeMode::Existing,
            origin_cwd,
            Some(cx.existing_path.unwrap_or(&path).to_owned()),
        ),
    };
    let title = session_title(args, cx, parent, program, repo_root);
    Ok(CreateSessionInput {
        title,
        program,
        cwd: cwd.to_owned(),
        repo_root: repo_root.to_owned(),
        cols: DEFAULT_PREVIEW_COLS,
        rows: DEFAULT_PREVIEW_ROWS,
        worktree_mode: Some(worktree_mode),
        existing_worktree_path,
        parent_session_id: parent.map(|parent| parent.id.clone()),
        sub_session_kind: if parent.is_some() || args.fork {
            Some(kind.unwrap_or(SubSessionKind::Clean))
        } else {
            None
        },
        handoff_from_session_id: None,
        base_branch: args.base.clone().filter(|_| worktree_mode == WorktreeMode::New),
        task_id: cx.task.map(|task| task.id.clone()),
        project_fingerprint: None,
        fork_from_session_id: caller
            .filter(|_| args.fork && parent.is_none())
            .map(|caller| caller.id.clone()),
        prompt: args.prompt.clone(),
        submit: args.draft.then_some(false),
        // Absent: the daemon stacks a sub-session's new worktree on its parent's where it can.
        stacked: (args.flat && worktree_mode == WorktreeMode::New).then_some(false),
        role: None,
    })
}

/// A name as the UI's form takes it (its characters, at most 64 of them, cut at a word).
fn clean_title(text: &str) -> Option<String> {
    Some(session_title_from_task(text)).filter(|title| !title.is_empty())
}

/// A session's path (`parent / child`), as the daemon projects it.
pub fn session_path(session: &SessionRecord) -> &str {
    session.path_title.as_deref().unwrap_or(&session.title)
}

/// `--title`; else the task's title, the prompt's first line or the agent (`fork` for a fork), made unique among
/// the active sessions of the repository with the same parent (`claude 2`), as the daemon refuses a clash.
fn session_title(
    args: &NewArgs,
    cx: &NewContext<'_>,
    parent: Option<&SessionRecord>,
    program: ProgramKey,
    repo_root: &str,
) -> String {
    if let Some(title) = args.title.as_deref().and_then(clean_title) {
        return title;
    }
    let first_line = args
        .prompt
        .as_deref()
        .and_then(|prompt| prompt.lines().map(str::trim).find(|line| !line.is_empty()));
    let base = cx
        .task
        .and_then(|task| clean_title(&task.title))
        .or_else(|| first_line.and_then(clean_title))
        .unwrap_or_else(|| if args.fork { "fork" } else { program.as_str() }.to_owned());
    let taken = |title: &str| {
        let path = match parent {
            Some(parent) => format!("{} / {title}", session_path(parent)),
            None => title.to_owned(),
        };
        cx.sessions.iter().any(|session| {
            session.repo_root == repo_root && session.status != SessionStatus::Exited && session_path(session) == path
        })
    };
    if !taken(&base) {
        return base;
    }
    (2..)
        .map(|n| {
            let suffix = format!(" {n}");
            let room = MAX_TITLE.saturating_sub(js_len(&suffix));
            let cut: String = base.chars().take(room).collect();
            format!("{}{suffix}", cut.trim_end())
        })
        .find(|title| !taken(title))
        .unwrap_or(base)
}

/// The item whose ID is `id`, or the only one it is a prefix of.
fn find_by_id<'a, T>(
    id: &str,
    items: &'a [T],
    id_of: impl Fn(&T) -> &str,
    what: &str,
    list: &str,
) -> Result<&'a T, DhError> {
    if let Some(exact) = items.iter().find(|item| id_of(item) == id) {
        return Ok(exact);
    }
    let mut matches = items.iter().filter(|item| id_of(item).starts_with(id));
    match (matches.next(), matches.next()) {
        (Some(only), None) if !id.is_empty() => Ok(only),
        (Some(_), Some(_)) => Err(DhError::msg(format!(
            "{id} is the start of more than one {what} ID; give more of it"
        ))),
        _ => Err(DhError::msg(format!("No {what} {id} (see {list})"))),
    }
}

/// The session `id` names (in full, or a unique prefix).
pub fn find_session<'a>(id: &str, sessions: &'a [SessionRecord]) -> Result<&'a SessionRecord, DhError> {
    find_by_id(id, sessions, |session| &session.id, "session", "deckhand ls")
}

/// The task `id` names (in full, or a unique prefix).
pub fn find_task<'a>(id: &str, tasks: &'a [Task]) -> Result<&'a Task, DhError> {
    find_by_id(id, tasks, |task| &task.id, "task", "deckhand task list")
}

/// The line `id` names (in full, or a unique prefix), among `lines`.
pub fn find_line<'a>(id: &str, lines: &'a [Line], list: &str) -> Result<&'a Line, DhError> {
    find_by_id(id, lines, |line| &line.id, "line", list)
}

#[cfg(test)]
mod tests {
    use deckhand_protocol::data::TaskMeta;
    use serde_json::json;

    use super::*;

    fn session(id: &str, extra: serde_json::Value) -> SessionRecord {
        let mut record = json!({
            "id": id, "title": id, "program": "claude", "command": "claude", "cwd": "/repo", "repoRoot": "/repo",
            "status": "running", "createdAt": "", "updatedAt": "",
        });
        if let (Some(record), Some(extra)) = (record.as_object_mut(), extra.as_object()) {
            record.extend(extra.clone());
        }
        serde_json::from_value(record).expect("a session record")
    }

    /// A session in its own worktree `wt-<id>` at `/wt/<id>`.
    fn in_worktree(id: &str, program: &str) -> SessionRecord {
        session(
            id,
            json!({
                "program": program, "cwd": format!("/wt/{id}"),
                "worktree": {"mode": "managed", "path": format!("/wt/{id}"), "branch": id, "id": format!("wt-{id}")},
            }),
        )
    }

    fn task(id: &str, title: &str) -> Task {
        Task {
            id: id.to_owned(),
            title: title.to_owned(),
            done: false,
            body: String::new(),
            meta: TaskMeta {
                t: Some(id.to_owned()),
                ..TaskMeta::default()
            },
        }
    }

    fn cx<'a>(caller: Option<&'a SessionRecord>, sessions: &'a [SessionRecord]) -> NewContext<'a> {
        NewContext {
            caller,
            repo_root: "/here",
            default_agent: None,
            task: None,
            existing_path: None,
            sessions,
            parent: None,
        }
    }

    fn prompt(text: &str) -> NewArgs {
        NewArgs {
            prompt: Some(text.to_owned()),
            ..NewArgs::default()
        }
    }

    #[test]
    fn by_default_a_caller_gets_a_sub_session_in_a_new_worktree_from_its_own() {
        let caller = in_worktree("lead", "codex");
        let sessions = [caller.clone()];
        let input = create_input(
            &prompt("Write the tests.\nThen run them."),
            &cx(Some(&caller), &sessions),
        )
        .expect("input");
        assert_eq!(
            input,
            CreateSessionInput {
                title: "Write the tests.".into(),
                program: ProgramKey::Codex,
                cwd: "/wt/lead".into(),
                repo_root: "/repo".into(),
                cols: DEFAULT_PREVIEW_COLS,
                rows: DEFAULT_PREVIEW_ROWS,
                worktree_mode: Some(WorktreeMode::New),
                existing_worktree_path: None,
                parent_session_id: Some("lead".into()),
                sub_session_kind: Some(SubSessionKind::Clean),
                handoff_from_session_id: None,
                base_branch: None,
                task_id: None,
                project_fingerprint: None,
                fork_from_session_id: None,
                prompt: Some("Write the tests.\nThen run them.".into()),
                submit: None,
                stacked: None,
                role: None,
            }
        );
    }

    #[test]
    fn without_a_caller_it_is_a_top_level_session_of_the_current_repository() {
        let mut cx = cx(None, &[]);
        cx.default_agent = Some(ProgramKey::Pi);
        let input = create_input(&NewArgs::default(), &cx).expect("input");
        assert_eq!(input.parent_session_id, None);
        assert_eq!(input.sub_session_kind, None);
        assert_eq!((input.cwd.as_str(), input.repo_root.as_str()), ("/here", "/here"));
        assert_eq!(input.program, ProgramKey::Pi);
        assert_eq!(input.title, "pi");
        assert_eq!(input.prompt, None);
        assert_eq!(
            create_input(&NewArgs::default(), &self::cx(None, &[]))
                .map(|input| input.program)
                .ok(),
            Some(ProgramKey::Claude)
        );
    }

    #[test]
    fn parent_puts_the_session_under_another_session_with_or_without_a_caller() {
        let caller = in_worktree("lead", "claude");
        let started = in_worktree("auth", "codex");
        let sessions = [caller.clone(), started.clone()];
        let args = |worktree: Option<WorktreeChoice>| NewArgs {
            parent: Some("auth".into()),
            worktree,
            prompt: Some("Token storage".into()),
            ..NewArgs::default()
        };
        let under = |caller: Option<&SessionRecord>, worktree: Option<WorktreeChoice>| {
            let mut cx = cx(caller, &sessions);
            cx.parent = Some(&started);
            create_input(&args(worktree), &cx).expect("input")
        };
        for caller in [Some(&caller), None] {
            let input = under(caller, None);
            assert_eq!(input.parent_session_id.as_deref(), Some("auth"));
            assert_eq!(input.sub_session_kind, Some(SubSessionKind::Clean));
            // From the parent's worktree, so the daemon stacks it there.
            assert_eq!(
                (input.cwd.as_str(), input.worktree_mode),
                ("/wt/auth", Some(WorktreeMode::New))
            );
            let same = under(caller, Some(WorktreeChoice::Same));
            assert_eq!(
                (same.cwd.as_str(), same.worktree_mode),
                ("/wt/auth", Some(WorktreeMode::None))
            );
        }
        // The caller's agent is still the default.
        assert_eq!(under(Some(&caller), None).program, ProgramKey::Claude);
        assert_eq!(under(None, None).program, ProgramKey::Claude);
    }

    #[test]
    fn worktree_choices_pick_the_mode_and_directory() {
        let caller = in_worktree("lead", "claude");
        let with = |worktree: WorktreeChoice, top_level: bool| {
            let args = NewArgs {
                worktree: Some(worktree),
                top_level,
                ..NewArgs::default()
            };
            let mut cx = cx(Some(&caller), &[]);
            cx.existing_path = Some("/abs/other");
            let input = create_input(&args, &cx).expect("input");
            (
                input.worktree_mode,
                input.cwd,
                input.existing_worktree_path,
                input.parent_session_id,
            )
        };
        let lead = Some("lead".to_owned());
        assert_eq!(
            with(WorktreeChoice::Same, false),
            (Some(WorktreeMode::None), "/wt/lead".into(), None, lead.clone())
        );
        assert_eq!(
            with(WorktreeChoice::None, false),
            (Some(WorktreeMode::None), "/repo".into(), None, lead.clone())
        );
        assert_eq!(
            with(WorktreeChoice::Path("../other".into()), false),
            (
                Some(WorktreeMode::Existing),
                "/wt/lead".into(),
                Some("/abs/other".into()),
                lead
            )
        );
        // A top-level session starts from the repository's checkout, not the caller's worktree.
        assert_eq!(
            with(WorktreeChoice::New, true),
            (Some(WorktreeMode::New), "/repo".into(), None, None)
        );
        assert_eq!(
            with(WorktreeChoice::Same, true),
            (Some(WorktreeMode::None), "/wt/lead".into(), None, None)
        );
    }

    #[test]
    fn a_draft_is_typed_and_a_base_applies_to_a_new_worktree() {
        let caller = in_worktree("lead", "claude");
        let args = NewArgs {
            prompt: Some("Review this".into()),
            draft: true,
            base: Some("main".into()),
            ..NewArgs::default()
        };
        let input = create_input(&args, &cx(Some(&caller), &[])).expect("input");
        assert_eq!(input.submit, Some(false));
        assert_eq!(input.base_branch.as_deref(), Some("main"));
    }

    #[test]
    fn flat_asks_for_an_unstacked_worktree_and_the_default_leaves_it_to_the_daemon() {
        let caller = in_worktree("lead", "claude");
        let flat = NewArgs {
            flat: true,
            ..NewArgs::default()
        };
        let input = create_input(&flat, &cx(Some(&caller), &[])).expect("input");
        assert_eq!(input.stacked, Some(false));
        let input = create_input(&NewArgs::default(), &cx(Some(&caller), &[])).expect("input");
        assert_eq!(input.stacked, None);
    }

    #[test]
    fn a_task_starts_a_sub_session_named_after_it_wherever_it_runs() {
        let caller = in_worktree("lead", "claude");
        let found = task("ab12cd34", "Fix the login redirect loop");
        let mut cx = cx(Some(&caller), &[]);
        cx.task = Some(&found);
        let args = NewArgs {
            task: Some("ab12".into()),
            prompt: Some("Brief".into()),
            ..NewArgs::default()
        };
        let input = create_input(&args, &cx).expect("input");
        assert_eq!(input.task_id.as_deref(), Some("ab12cd34"));
        assert_eq!(input.parent_session_id.as_deref(), Some("lead"));
        assert_eq!(input.title, "Fix the login redirect loop");
        assert_eq!(
            (input.cwd.as_str(), input.worktree_mode),
            ("/wt/lead", Some(WorktreeMode::New))
        );
        // In the caller's worktree it is still a sub-session on that line.
        let same = NewArgs {
            worktree: Some(WorktreeChoice::Same),
            ..args
        };
        let input = create_input(&same, &cx).expect("input");
        assert_eq!(input.parent_session_id.as_deref(), Some("lead"));
        assert_eq!(input.task_id.as_deref(), Some("ab12cd34"));
    }

    #[test]
    fn a_fork_copies_the_caller_and_codex_forks_stay_in_its_worktree() {
        let claude = in_worktree("lead", "claude");
        let fork = NewArgs {
            fork: true,
            ..NewArgs::default()
        };
        let input = create_input(&fork, &cx(Some(&claude), &[])).expect("input");
        assert_eq!(input.sub_session_kind, Some(SubSessionKind::Forked));
        assert_eq!(input.parent_session_id.as_deref(), Some("lead"));
        assert_eq!(input.worktree_mode, Some(WorktreeMode::New));
        assert_eq!(input.title, "fork");

        let codex = in_worktree("lead", "codex");
        let input = create_input(&fork, &cx(Some(&codex), &[])).expect("input");
        assert_eq!(
            (input.worktree_mode, input.cwd.as_str()),
            (Some(WorktreeMode::None), "/wt/lead")
        );

        // A top-level fork names its source instead of a parent.
        let top = NewArgs {
            fork: true,
            top_level: true,
            ..NewArgs::default()
        };
        let input = create_input(&top, &cx(Some(&claude), &[])).expect("input");
        assert_eq!(input.parent_session_id, None);
        assert_eq!(input.fork_from_session_id.as_deref(), Some("lead"));
        assert_eq!(input.sub_session_kind, Some(SubSessionKind::Forked));
        assert_eq!(input.cwd, "/wt/lead");

        let other_agent = NewArgs {
            fork: true,
            agent: Some(ProgramKey::Codex),
            ..NewArgs::default()
        };
        assert_eq!(
            create_input(&other_agent, &cx(Some(&claude), &[])).map_err(|error| error.to_string()),
            Err("A fork keeps this session's agent (claude); leave out --agent".to_owned())
        );
    }

    #[test]
    fn forking_or_sharing_a_worktree_needs_a_caller() {
        for args in [
            NewArgs {
                fork: true,
                ..NewArgs::default()
            },
            NewArgs {
                worktree: Some(WorktreeChoice::Same),
                ..NewArgs::default()
            },
        ] {
            let error = create_input(&args, &cx(None, &[])).expect_err("refused").to_string();
            assert!(error.contains("needs a Deckhand session to act for"), "{error}");
        }
    }

    #[test]
    fn titles_are_cleaned_cut_to_64_and_made_unique_among_active_siblings() {
        let caller = session("lead", json!({"pathTitle": "Lead"}));
        let long = "Refactor the session list so that sub-sessions sort under their parents everywhere";
        let input = create_input(&prompt(long), &cx(Some(&caller), &[])).expect("input");
        assert_eq!(
            input.title,
            "Refactor the session list so that sub-sessions sort under their"
        );
        assert!(js_len(&input.title) <= 64);

        let explicit = NewArgs {
            title: Some("  Fix   “auth”  ".into()),
            ..NewArgs::default()
        };
        assert_eq!(
            create_input(&explicit, &cx(Some(&caller), &[]))
                .map(|input| input.title)
                .ok(),
            Some("Fix auth".into())
        );

        let sibling = |path: &str, status: &str| session(path, json!({"pathTitle": path, "status": status}));
        let sessions = [
            caller.clone(),
            sibling("Lead / claude", "running"),
            sibling("Lead / claude 2", "starting"),
            sibling("Lead / claude 3", "exited"),
            sibling("claude 3", "running"),
        ];
        let input = create_input(&NewArgs::default(), &cx(Some(&caller), &sessions)).expect("input");
        assert_eq!(input.title, "claude 3");
    }

    #[test]
    fn ids_may_be_shortened_to_a_unique_prefix() {
        let sessions = [
            session("1a2b3c", json!({})),
            session("1a2bff", json!({})),
            session("9f", json!({})),
        ];
        assert_eq!(find_session("9", &sessions).map(|s| s.id.as_str()).ok(), Some("9f"));
        assert_eq!(
            find_session("1a2b3c", &sessions).map(|s| s.id.as_str()).ok(),
            Some("1a2b3c")
        );
        assert_eq!(
            find_session("1a2b", &sessions)
                .map_err(|error| error.to_string())
                .err()
                .as_deref(),
            Some("1a2b is the start of more than one session ID; give more of it")
        );
        assert_eq!(
            find_task("zz", &[]).map_err(|error| error.to_string()).err().as_deref(),
            Some("No task zz (see deckhand task list)")
        );
    }
}
