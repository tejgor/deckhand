//! What the commands print.

use serde::Serialize;

use deckhand_core::jsstr::{js_len, js_pad_end};
use deckhand_core::sidebar_model::status_words;
use deckhand_core::tasks::{group_tasks, link_key};
use deckhand_core::tasks_board::{linked_task, task_sessions, work_label};
use deckhand_protocol::data::{
    OrchestratorResult, PlanOutcome, PlanStatus, SessionRecord, SessionWorktreeMode, Task, WorktreeMode,
};

use super::plan::session_path;

/// How much of an ID the text output shows; any unique prefix is accepted back.
pub const SHORT_ID: usize = 8;

/// The first [`SHORT_ID`] characters of an ID.
pub fn short_id(id: &str) -> &str {
    id.char_indices().nth(SHORT_ID).map_or(id, |(at, _)| &id[..at])
}

/// Where a session works: `⎇ <branch>` (or the worktree's path) in a worktree, else `checkout`.
pub fn worktree_text(session: &SessionRecord) -> String {
    match session.worktree.as_ref() {
        Some(worktree) if worktree.mode != SessionWorktreeMode::None => {
            match worktree.branch.as_deref().filter(|branch| !branch.is_empty()) {
                Some(branch) => format!("⎇ {branch}"),
                None => worktree.path.clone().unwrap_or_else(|| "worktree".to_owned()),
            }
        }
        _ => "checkout".to_owned(),
    }
}

/// Columns padded to their widest cell (the last one not padded), two spaces apart.
fn table(rows: &[Vec<String>]) -> String {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|column| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|cell| js_len(cell))
                .max()
                .unwrap_or(0)
        })
        .collect();
    rows.iter()
        .map(|row| {
            let cells: Vec<String> = row
                .iter()
                .enumerate()
                .map(|(column, cell)| {
                    if column + 1 == row.len() {
                        cell.clone()
                    } else {
                        js_pad_end(cell, widths.get(column).copied().unwrap_or(0), " ")
                    }
                })
                .collect();
            cells.join("  ").trim_end().to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `deckhand ls`: one row per session in sidebar order, `*` marking `caller`.
pub fn session_table(sessions: &[&SessionRecord], tasks: &[Task], caller: Option<&str>) -> String {
    if sessions.is_empty() {
        return "No sessions in this repository.".to_owned();
    }
    let mut rows = vec![
        ["", "ID", "PARENT", "STATE", "AGENT", "SESSION", "WORKTREE", "TASK"]
            .map(str::to_owned)
            .to_vec(),
    ];
    for session in sessions {
        let task = linked_task(tasks, Some(session))
            .map(|task| format!("{} {}", short_id(&task.id), task.title))
            .unwrap_or_default();
        rows.push(vec![
            if Some(session.id.as_str()) == caller { "*" } else { "" }.to_owned(),
            short_id(&session.id).to_owned(),
            session
                .parent_session_id
                .as_deref()
                .map_or_else(|| "-".to_owned(), |parent| short_id(parent).to_owned()),
            status_words(session).to_owned(),
            session.program.to_string(),
            session_path(session).to_owned(),
            worktree_text(session),
            task,
        ]);
    }
    table(&rows)
}

/// A task as `--json` prints it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskView {
    pub id: String,
    pub title: String,
    pub details: String,
    pub done: bool,
    /// `progress` (the task of some work), `backlog` or `done`.
    pub group: &'static str,
    /// The work it is the task of: `wt:<worktree record>` or `s:<session>`.
    pub work: Option<String>,
    /// The sessions of that work.
    pub sessions: Vec<String>,
}

/// `task` as `--json` prints it.
pub fn task_view(task: &Task, sessions: &[SessionRecord]) -> TaskView {
    let work = link_key(&task.meta);
    TaskView {
        id: task.id.clone(),
        title: task.title.clone(),
        details: task.body.clone(),
        done: task.done,
        group: match (task.done, work.is_some()) {
            (true, _) => "done",
            (false, true) => "progress",
            (false, false) => "backlog",
        },
        work,
        sessions: task_sessions(task, sessions)
            .into_iter()
            .map(|session| session.id.clone())
            .collect(),
    }
}

/// One task's line: its ID, title and (in progress) its work.
fn task_line(task: &Task, sessions: &[SessionRecord]) -> String {
    let work = link_key(&task.meta)
        .map(|section| format!("  ({})", work_label(&section, sessions)))
        .unwrap_or_default();
    format!("  {}  {}{work}", short_id(&task.id), task.title)
}

/// `deckhand task list`: in progress, the backlog, and done in the last 7 days (the board's groups).
pub fn task_list(tasks: &[Task], sessions: &[SessionRecord], now_ms: i64) -> String {
    let groups = group_tasks(tasks, now_ms);
    let mut out = Vec::new();
    for (heading, list) in [
        ("In progress", &groups.progress),
        ("Backlog", &groups.backlog),
        ("Done", &groups.done),
    ] {
        if list.is_empty() {
            continue;
        }
        out.push(format!("{heading} ({})", list.len()));
        out.extend(list.iter().map(|task| task_line(task, sessions)));
    }
    if !groups.older_done.is_empty() {
        out.push(format!("({} done earlier not listed)", groups.older_done.len()));
    }
    if out.is_empty() {
        return "No tasks in this repository.".to_owned();
    }
    out.join("\n")
}

/// `deckhand new`: the session started.
pub fn started(session: &SessionRecord, prompt: Option<bool>) -> String {
    let place = match session.worktree.as_ref() {
        Some(worktree) if worktree.mode != SessionWorktreeMode::None => {
            let path = worktree.path.as_deref().unwrap_or("?");
            match worktree.branch.as_deref().filter(|branch| !branch.is_empty()) {
                Some(branch) => format!("worktree {path} (⎇ {branch})"),
                None => format!("worktree {path}"),
            }
        }
        _ if session
            .requested_worktree_mode
            .is_some_and(|mode| mode.as_str() != "none") =>
        {
            "a worktree being created".to_owned()
        }
        _ => format!("{} (no worktree)", session.cwd),
    };
    let prompt = match prompt {
        Some(true) => ", prompt sent",
        Some(false) => ", prompt typed as a draft",
        None => "",
    };
    let parent = if session.parent_session_id.is_some() {
        "sub-session"
    } else {
        "session"
    };
    format!(
        "Started {parent} {}: {}\n  {} in {place}{prompt}",
        short_id(&session.id),
        session_path(session),
        session.program
    )
}

/// Where a session a plan created works: its worktree's branch and what it merges into, or no worktree.
pub fn plan_placement(session: &SessionRecord) -> String {
    match session.worktree.as_ref() {
        Some(worktree) if worktree.mode != SessionWorktreeMode::None => {
            let place = match worktree.branch.as_deref().filter(|branch| !branch.is_empty()) {
                Some(branch) => format!("⎇ {branch}"),
                None => worktree.path.clone().unwrap_or_else(|| "a worktree".to_owned()),
            };
            match worktree.target_branch.as_deref().filter(|target| !target.is_empty()) {
                Some(target) => format!("{place}, stacked into {target}"),
                None => format!("{place}, flat"),
            }
        }
        _ if session.requested_worktree_mode == Some(WorktreeMode::New) => "a new worktree being created".to_owned(),
        _ if session.parent_session_id.is_some() && session.cwd != session.repo_root => {
            "its parent's worktree".to_owned()
        }
        _ => "no worktree (the repository's checkout)".to_owned(),
    }
}

/// `deckhand plan apply`: a line per session and task created, then what came of the plan.
pub fn plan_outcome(outcome: &PlanOutcome) -> String {
    let mut lines = Vec::new();
    for created in &outcome.sessions {
        let session = &created.session;
        lines.push(format!(
            "{}  {}  ({} · {})",
            short_id(&session.id),
            session_path(session),
            plan_placement(session),
            session.program
        ));
    }
    for task in &outcome.tasks {
        lines.push(format!("{}  ☐ {}  (backlog)", short_id(&task.id), task.title));
    }
    let count = |n: usize, what: &str| format!("{n} {what}{}", if n == 1 { "" } else { "s" });
    let made = format!(
        "{} and {}",
        count(outcome.sessions.len(), "session"),
        count(outcome.tasks.len(), "task")
    );
    lines.push(match outcome.status {
        PlanStatus::Created => format!("Created {made}; each session got its brief as its first message."),
        PlanStatus::Cancelled => "Cancelled in Deckhand: nothing was created.".to_owned(),
        PlanStatus::Expired => "The plan expired unanswered after 15 minutes: nothing was created.".to_owned(),
        PlanStatus::Failed => {
            let failed = outcome.failed.as_ref();
            let what = failed.map_or_else(String::new, |failed| match failed.key.as_deref() {
                Some(key) => format!(" at {} ({key})", failed.title),
                None => format!(" at the task {}", failed.title),
            });
            let why = failed.map_or("", |failed| failed.error.as_str());
            format!(
                "Stopped{what}: {why}\nCreated before that: {made}. They are kept (nothing is rolled back); see \
                 deckhand ls, and apply a plan for the rest."
            )
        }
    });
    lines.join("\n")
}

/// `deckhand do`: which orchestrator has the request, and how to reach it.
pub fn orchestrator(result: &OrchestratorResult) -> String {
    let id = short_id(&result.session.id);
    let first = if result.started {
        format!("Started the orchestrator {id} with your request.")
    } else if result.delivered {
        format!("Sent your request to the orchestrator {id}.")
    } else {
        format!("The orchestrator {id} is running.")
    };
    format!(
        "{first}\nTo follow it, open Deckhand here (deckhand), select {} and press enter to attach.",
        session_path(&result.session)
    )
}

#[cfg(test)]
mod tests {
    use deckhand_protocol::data::TaskMeta;
    use serde_json::json;

    use super::*;

    fn session(extra: serde_json::Value) -> SessionRecord {
        let mut record = json!({
            "id": "1a2b3c4d-0000-4000-8000-000000000000", "title": "Fix auth", "program": "claude",
            "command": "claude", "cwd": "/repo", "repoRoot": "/repo", "status": "running", "agentStatus": "idle",
            "createdAt": "", "updatedAt": "",
        });
        if let (Some(record), Some(extra)) = (record.as_object_mut(), extra.as_object()) {
            record.extend(extra.clone());
        }
        serde_json::from_value(record).expect("a session record")
    }

    fn task(id: &str, title: &str, wt: Option<&str>, done: Option<&str>) -> Task {
        Task {
            id: id.to_owned(),
            title: title.to_owned(),
            done: done.is_some(),
            body: String::new(),
            meta: TaskMeta {
                t: Some(id.to_owned()),
                wt: wt.map(str::to_owned),
                done: done.map(str::to_owned),
                ..TaskMeta::default()
            },
        }
    }

    #[test]
    fn short_ids_are_the_first_eight_characters() {
        assert_eq!(short_id("1a2b3c4d-0000"), "1a2b3c4d");
        assert_eq!(short_id("ab12"), "ab12");
    }

    #[test]
    fn ls_lists_parent_state_agent_path_worktree_and_task() {
        let lead = session(json!({
            "pathTitle": "Fix auth",
            "worktree": {"mode": "managed", "path": "/wt/fix-auth", "branch": "fix-auth", "id": "wt1"},
        }));
        let child = session(json!({
            "id": "9f8e7d6c-1111", "title": "tests", "pathTitle": "Fix auth / tests", "program": "codex",
            "parentSessionId": lead.id, "status": "starting",
        }));
        let tasks = [task("5e6f7a8b", "Fix the login bug", Some("wt1"), None)];
        assert_eq!(
            session_table(&[&lead, &child], &tasks, Some(&lead.id)),
            "   ID        PARENT    STATE     AGENT   SESSION           WORKTREE    TASK\n\
             *  1a2b3c4d  -         idle      claude  Fix auth          ⎇ fix-auth  5e6f7a8b Fix the login bug\n   \
             9f8e7d6c  1a2b3c4d  starting  codex   Fix auth / tests  checkout"
        );
        assert_eq!(session_table(&[], &[], None), "No sessions in this repository.");
    }

    #[test]
    fn task_list_groups_like_the_board_and_names_the_work() {
        let lead =
            session(json!({"worktree": {"mode": "managed", "path": "/wt/a", "branch": "fix-auth", "id": "wt1"}}));
        let now = deckhand_core::jsstr::date::now_ms();
        let today = deckhand_core::jsstr::date::to_iso_string(now).unwrap_or_default();
        let tasks = [
            task("5e6f7a8b", "Fix the login bug", Some("wt1"), None),
            task("1234abcd", "Write docs", None, None),
            task("99990000", "Old thing", None, Some("2000-01-01")),
            task("77770000", "Shipped", None, Some(&today[..10])),
        ];
        assert_eq!(
            task_list(&tasks, std::slice::from_ref(&lead), now),
            "In progress (1)\n  5e6f7a8b  Fix the login bug  (⎇ fix-auth)\nBacklog (1)\n  1234abcd  Write docs\n\
             Done (1)\n  77770000  Shipped\n(1 done earlier not listed)"
        );
        assert_eq!(task_list(&[], &[], now), "No tasks in this repository.");
        let view = task_view(&tasks[0], std::slice::from_ref(&lead));
        assert_eq!((view.group, view.work.as_deref()), ("progress", Some("wt:wt1")));
        assert_eq!(view.sessions, vec![lead.id.clone()]);
        assert_eq!(task_view(&tasks[1], &[]).group, "backlog");
        assert_eq!(task_view(&tasks[2], &[]).group, "done");
    }

    #[test]
    fn plan_apply_lists_what_was_created_and_what_became_of_the_plan() {
        use deckhand_protocol::data::{PlanCreatedSession, PlanCreatedTask, PlanFailure};
        let created = |extra: serde_json::Value, key: &str| PlanCreatedSession {
            key: key.to_owned(),
            session: session(extra).into(),
        };
        let mut outcome = PlanOutcome {
            plan_id: "p".into(),
            status: PlanStatus::Created,
            sessions: vec![
                created(
                    json!({"title": "Auth rework", "requestedWorktreeMode": "new",
                        "worktree": {"mode": "managed", "path": "/wt/auth", "branch": "auth-rework"}}),
                    "auth",
                ),
                created(
                    json!({"id": "2b3c4d5e-1111", "title": "Token storage", "pathTitle": "Auth rework / Token storage",
                        "parentSessionId": "1a2b", "program": "codex", "requestedWorktreeMode": "new",
                        "worktree": {"mode": "managed", "path": "/wt/tokens", "branch": "auth-rework-token_storage",
                            "targetBranch": "auth-rework"}}),
                    "tokens",
                ),
                created(
                    json!({"id": "3c4d5e6f-2222", "title": "Login UI", "pathTitle": "Auth rework / Login UI",
                        "parentSessionId": "1a2b", "cwd": "/wt/auth", "requestedWorktreeMode": "none",
                        "worktree": {"mode": "none"}}),
                    "ui",
                ),
                created(
                    json!({"id": "4d5e6f70-3333", "title": "Script", "requestedWorktreeMode": "none",
                        "worktree": {"mode": "none"}}),
                    "script",
                ),
            ],
            tasks: vec![PlanCreatedTask {
                id: "ab12cd34".into(),
                title: "Remove legacy endpoint".into(),
            }],
            failed: None,
        };
        assert_eq!(
            plan_outcome(&outcome),
            "1a2b3c4d  Auth rework  (⎇ auth-rework, flat · claude)\n\
             2b3c4d5e  Auth rework / Token storage  (⎇ auth-rework-token_storage, stacked into auth-rework · codex)\n\
             3c4d5e6f  Auth rework / Login UI  (its parent's worktree · claude)\n\
             4d5e6f70  Script  (no worktree (the repository's checkout) · claude)\n\
             ab12cd34  ☐ Remove legacy endpoint  (backlog)\n\
             Created 4 sessions and 1 task; each session got its brief as its first message."
        );
        outcome.status = PlanStatus::Failed;
        outcome.sessions.truncate(1);
        outcome.tasks.clear();
        outcome.failed = Some(PlanFailure {
            key: Some("tokens".into()),
            title: "Token storage".into(),
            error: "its parent Auth rework did not start: setup failed".into(),
        });
        assert_eq!(
            plan_outcome(&outcome).lines().skip(1).collect::<Vec<_>>(),
            [
                "Stopped at Token storage (tokens): its parent Auth rework did not start: setup failed",
                "Created before that: 1 session and 0 tasks. They are kept (nothing is rolled back); see deckhand ls, \
                 and apply a plan for the rest."
            ]
        );
        outcome.status = PlanStatus::Cancelled;
        outcome.sessions.clear();
        assert_eq!(plan_outcome(&outcome), "Cancelled in Deckhand: nothing was created.");
    }

    #[test]
    fn do_names_the_orchestrator_and_how_to_attach() {
        let result = |started, delivered| OrchestratorResult {
            session: session(json!({"title": "Orchestrator", "role": "orchestrator"})).into(),
            started,
            delivered,
        };
        assert_eq!(
            orchestrator(&result(true, false)),
            "Started the orchestrator 1a2b3c4d with your request.\n\
             To follow it, open Deckhand here (deckhand), select Orchestrator and press enter to attach."
        );
        assert!(orchestrator(&result(false, true)).starts_with("Sent your request to the orchestrator 1a2b3c4d."));
    }

    #[test]
    fn new_says_where_the_session_started_and_what_became_of_the_prompt() {
        let in_worktree = session(json!({
            "parentSessionId": "lead", "pathTitle": "Lead / Fix auth",
            "worktree": {"mode": "managed", "path": "/wt/fix-auth", "branch": "lead-fix-auth"},
        }));
        assert_eq!(
            started(&in_worktree, Some(true)),
            "Started sub-session 1a2b3c4d: Lead / Fix auth\n  claude in worktree /wt/fix-auth (⎇ lead-fix-auth), prompt sent"
        );
        let pending = session(json!({"requestedWorktreeMode": "new", "worktree": {"mode": "none"}}));
        assert_eq!(
            started(&pending, Some(false)),
            "Started session 1a2b3c4d: Fix auth\n  claude in a worktree being created, prompt typed as a draft"
        );
        let checkout = session(json!({"requestedWorktreeMode": "none", "worktree": {"mode": "none"}}));
        assert_eq!(
            started(&checkout, None),
            "Started session 1a2b3c4d: Fix auth\n  claude in /repo (no worktree)"
        );
    }
}
