//! What the commands print.

use serde::Serialize;

use std::collections::HashSet;

use deckhand_core::jsstr::{js_len, js_pad_end};
use deckhand_core::sidebar_model::status_words;
use deckhand_core::task_list::{Line, TaskList};
use deckhand_core::tasks::{RECENT_DONE_DAYS, today};
use deckhand_protocol::data::{
    OrchestratorResult, PlanOutcome, PlanStatus, SessionRecord, SessionWorktreeMode, WorkNote, WorkNoteKind,
    WorktreeMode,
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
pub fn session_table(sessions: &[&SessionRecord], list: &TaskList, caller: Option<&str>) -> String {
    if sessions.is_empty() {
        return "No sessions in this repository.".to_owned();
    }
    let mut rows = vec![
        ["", "ID", "PARENT", "STATE", "AGENT", "SESSION", "WORKTREE", "TASK"]
            .map(str::to_owned)
            .to_vec(),
    ];
    for session in sessions {
        let task = list
            .line_of_session(&session.id)
            .map(|line| format!("{} {}", short_id(&line.id), line.title))
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

/// A line as `--json` prints it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineView {
    pub id: String,
    pub title: String,
    pub done: bool,
    /// The line it is under (`None`: top-level).
    pub parent: Option<String>,
    pub depth: usize,
    /// The session on it.
    pub session: Option<String>,
    /// Ticked and total lines directly under it.
    pub ticked: usize,
    pub total: usize,
    /// It has a note, a brief.
    pub note: bool,
    pub brief: bool,
    /// When it was ticked (`YYYY-MM-DD`); the branch of an attempt dropped unmerged; the branch it was left open in.
    pub done_at: Option<String>,
    pub tried: Option<String>,
    pub from: Option<String>,
}

/// Whether a note of `kind` with text belongs to line `id`.
fn has_note(notes: &[WorkNote], kind: WorkNoteKind, id: &str) -> bool {
    notes
        .iter()
        .any(|note| note.kind == kind && note.id == id && !note.text.trim().is_empty())
}

/// `line` as `--json` prints it (`notes`: the list's notes, as `watch-tasks` answers them).
pub fn line_view(line: &Line, list: &TaskList, notes: &[WorkNote]) -> LineView {
    let count = list.count(&line.id);
    LineView {
        id: line.id.clone(),
        title: line.title.clone(),
        done: line.done,
        parent: line.parent.clone(),
        depth: line.depth,
        session: line.meta.s.clone().filter(|s| !s.is_empty()),
        ticked: count.ticked,
        total: count.total,
        note: has_note(notes, WorkNoteKind::Line, &line.id),
        brief: has_note(notes, WorkNoteKind::Brief, &line.id),
        done_at: line.meta.done.clone(),
        tried: line.meta.tried.clone(),
        from: line.meta.from.clone(),
    }
}

/// One line of the tree: `☐ <id>  <title>  2/5 ✎  ← <session>  · tried in ⎇ <branch>`, indented by its depth (from
/// `base`).
fn line_row(line: &Line, base: usize, list: &TaskList, sessions: &[SessionRecord], notes: &[WorkNote]) -> String {
    let mut row = format!(
        "{}{} {}  {}",
        "  ".repeat(line.depth.saturating_sub(base)),
        if line.done { '☑' } else { '☐' },
        short_id(&line.id),
        line.title
    );
    let count = list.count(&line.id);
    if count.total > 0 {
        row.push_str(&format!("  {}/{}", count.ticked, count.total));
    }
    if has_note(notes, WorkNoteKind::Brief, &line.id) {
        row.push_str("  ✎");
    }
    if let Some(s) = line.meta.s.as_deref().filter(|s| !s.is_empty()) {
        let name = sessions.iter().find(|session| session.id == s).map_or_else(
            || "a session no longer listed".to_owned(),
            |session| session_path(session).to_owned(),
        );
        row.push_str(&format!("  ← {name} ({})", short_id(s)));
    }
    if let Some(tried) = line.meta.tried.as_deref().filter(|tried| !tried.is_empty()) {
        row.push_str(&format!("  · tried in ⎇ {tried}"));
    }
    if let Some(from) = line.meta.from.as_deref().filter(|from| !from.is_empty()) {
        row.push_str(&format!("  · left open in ⎇ {from}"));
    }
    row
}

/// `deckhand task list`: the list as a tree, open lines and those ticked in the last 7 days (with the lines above
/// them), each with who is on it and how far its lines are.
pub fn task_tree(list: &TaskList, sessions: &[SessionRecord], notes: &[WorkNote], now_ms: i64) -> String {
    let cutoff = today(now_ms.saturating_sub(RECENT_DONE_DAYS * 86_400_000)).unwrap_or_default();
    let recent = |line: &Line| !line.done || line.meta.done.as_deref().is_some_and(|done| *done >= *cutoff);
    let mut shown: HashSet<&str> = HashSet::new();
    let mut hidden = 0;
    for line in list.lines() {
        if recent(line) {
            shown.insert(line.id.as_str());
            for above in list.ancestors(&line.id) {
                shown.insert(above.id.as_str());
            }
        } else {
            hidden += 1;
        }
    }
    let mut out: Vec<String> = list
        .lines()
        .iter()
        .filter(|line| shown.contains(line.id.as_str()))
        .map(|line| line_row(line, 0, list, sessions, notes))
        .collect();
    let hidden = hidden
        - list
            .lines()
            .iter()
            .filter(|line| !recent(line) && shown.contains(line.id.as_str()))
            .count();
    if hidden > 0 {
        out.push(format!("({hidden} done earlier not listed)"));
    }
    if out.is_empty() {
        return "No tasks in this repository.".to_owned();
    }
    out.join("\n")
}

/// `deckhand step list`: the caller's line and every line under it.
pub fn step_tree(list: &TaskList, id: &str, sessions: &[SessionRecord], notes: &[WorkNote]) -> String {
    let lines = list.subtree(id);
    let Some(root) = lines.first() else {
        return "This session has no line yet: deckhand step add gives it one".to_owned();
    };
    let mut out = vec![line_row(root, root.depth, list, sessions, notes)];
    out.extend(
        lines
            .iter()
            .skip(1)
            .map(|line| format!("  {}", line_row(line, root.depth + 1, list, sessions, notes))),
    );
    if lines.len() == 1 {
        out.push("  (no steps yet)".to_owned());
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
        let list = TaskList::parse(&format!(
            "- [ ] Fix the login bug <!-- dh:t=5e6f7a8b s={} -->\n",
            lead.id
        ));
        assert_eq!(
            session_table(&[&lead, &child], &list, Some(&lead.id)),
            "   ID        PARENT    STATE     AGENT   SESSION           WORKTREE    TASK\n\
             *  1a2b3c4d  -         idle      claude  Fix auth          ⎇ fix-auth  5e6f7a8b Fix the login bug\n   \
             9f8e7d6c  1a2b3c4d  starting  codex   Fix auth / tests  checkout"
        );
        assert_eq!(session_table(&[], &list, None), "No sessions in this repository.");
    }

    #[test]
    fn task_list_is_the_tree_with_who_is_on_what() {
        let lead = session(json!({"pathTitle": "Fix auth"}));
        let now = deckhand_core::jsstr::date::now_ms();
        let today = deckhand_core::jsstr::date::to_iso_string(now).unwrap_or_default();
        let list = TaskList::parse(&format!(
            "- [ ] Rework auth <!-- dh:t=aaaa0000 -->\n  - [x] Token storage <!-- dh:t=bbbb0000 done={} -->\n  - [ ] \
             Login UI <!-- dh:t=cccc0000 s={} -->\n- [x] Old thing <!-- dh:t=dddd0000 done=2000-01-01 -->\n- [ ] \
             Retry <!-- dh:t=eeee0000 tried=fix-auth -->\n",
            &today[..10],
            lead.id
        ));
        let brief = WorkNote {
            kind: WorkNoteKind::Brief,
            id: "cccc0000".into(),
            path: String::new(),
            text: "Use the kit\n".into(),
            revision: String::new(),
            too_large: None,
            from: Some("you".into()),
            at: None,
        };
        let notes = [brief];
        assert_eq!(
            task_tree(&list, std::slice::from_ref(&lead), &notes, now),
            "☐ aaaa0000  Rework auth  1/2\n  ☑ bbbb0000  Token storage\n  ☐ cccc0000  Login UI  ✎  ← Fix auth \
             (1a2b3c4d)\n☐ eeee0000  Retry  · tried in ⎇ fix-auth\n(1 done earlier not listed)"
        );
        assert_eq!(
            task_tree(&TaskList::default(), &[], &[], now),
            "No tasks in this repository."
        );
        assert_eq!(
            step_tree(&list, "aaaa0000", &[], &notes),
            "☐ aaaa0000  Rework auth  1/2\n  ☑ bbbb0000  Token storage\n  ☐ cccc0000  Login UI  ✎  ← a session no \
             longer listed (1a2b3c4d)"
        );
        let view = line_view(list.get("cccc0000").expect("a line"), &list, &notes);
        assert_eq!(
            (
                view.parent.as_deref(),
                view.depth,
                view.session.as_deref(),
                view.brief,
                view.note
            ),
            (Some("aaaa0000"), 1, Some(lead.id.as_str()), true, false)
        );
        let idea = line_view(list.get("aaaa0000").expect("a line"), &list, &notes);
        assert_eq!((idea.ticked, idea.total), (1, 2));
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
