//! What the sidebar, the session's Notes tab and the confirmations say about the list: the lines a session is on,
//! who is on a line and in what state, and the open items of the old worktree notes. The board itself (the list as a
//! tree, its Notes view and its `w` menu) is `lines_board`. A work is keyed `wt:<record id>` / `s:<session id>` (see
//! `tasks::link_key`).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::display::{Color, THEME, display_session_title, status_color, status_glyph};
use crate::notes::TaskLinkState;
use crate::session_order::SessionLike;
use crate::sidebar_model::status_words;
use crate::tasks::{is_linked, link_key, link_of_key, open_note_items};
use deckhand_protocol::data::{
    AgentActivityStatus, AttentionState, NoteSection, SessionRecord, SessionStatus, SharedNoteKind, Task, TaskAuto,
    TaskLink,
};

/// JS truthiness of an optional string.
fn truthy(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.is_empty())
}

/// `session.archivedAt`, read as JS does (truthy).
fn archived(session: &SessionRecord) -> bool {
    truthy(session.archived_at.as_deref())
}

/// `session.worktree?.id` when truthy.
fn worktree_id(session: &SessionRecord) -> Option<&str> {
    session
        .worktree
        .as_ref()
        .and_then(|worktree| worktree.id.as_deref())
        .filter(|id| !id.is_empty())
}

/// `session.worktree?.branch` when truthy.
fn worktree_branch(session: &SessionRecord) -> Option<&str> {
    session
        .worktree
        .as_ref()
        .and_then(|worktree| worktree.branch.as_deref())
        .filter(|branch| !branch.is_empty())
}

/// TS: tasksBoard.ts the `steps` of a task row: its work note's checklist, done/total (also stepCount's result).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepCount {
    pub done: usize,
    pub total: usize,
}

/// TS: tasksBoard.ts WorkNote (the note a work's steps are in, and how the board saves it).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkNote {
    pub session_id: String,
    pub section: NoteSection,
    pub note_id: Option<String>,
    pub text: String,
    pub revision: String,
    pub read_only: Option<bool>,
}

/// TS: tasksBoard.ts workNote.
pub fn work_note<S: SessionLike>(section: &str, sessions: &[S]) -> Option<WorkNote> {
    let working = work_sessions(section, sessions);
    if let Some(id) = section.strip_prefix("wt:") {
        // Any session of the worktree holds its note; one not archived first.
        let holder = working
            .iter()
            .map(SessionLike::session)
            .filter(|session| !archived(session))
            .chain(
                working
                    .iter()
                    .map(SessionLike::session)
                    .filter(|session| archived(session)),
            )
            .find(|session| {
                session
                    .shared_notes
                    .as_ref()
                    .is_some_and(|shared| shared.kind == SharedNoteKind::Worktree && shared.id == id)
            })?;
        let shared = holder.shared_notes.as_ref()?;
        return Some(WorkNote {
            session_id: holder.id.clone(),
            section: NoteSection::Shared,
            note_id: Some(format!("{}:{}", shared.kind, shared.id)),
            text: shared.text.clone(),
            revision: shared.revision.clone(),
            read_only: (shared.read_only == Some(true)).then_some(true),
        });
    }
    let session = working.first()?.session();
    let notes_file = session.notes_file.as_ref()?;
    Some(WorkNote {
        session_id: session.id.clone(),
        section: NoteSection::Session,
        note_id: None,
        text: session.notes.clone().unwrap_or_default(),
        revision: notes_file.revision.clone(),
        read_only: None,
    })
}

/// TS: tasksBoard.ts workSessions.
pub fn work_sessions<'a, S: SessionLike>(section: &str, sessions: &'a [S]) -> Vec<&'a S> {
    match link_of_key(section) {
        Some(TaskLink::Worktree { wt }) => sessions
            .iter()
            .filter(|item| {
                item.session()
                    .worktree
                    .as_ref()
                    .and_then(|worktree| worktree.id.as_deref())
                    == Some(wt.as_str())
            })
            .collect(),
        Some(TaskLink::Session { s }) => sessions.iter().filter(|item| item.session().id == s).collect(),
        None => Vec::new(),
    }
}

/// TS: tasksBoard.ts workKeyOf.
pub fn work_key_of(session: &SessionRecord) -> String {
    match worktree_id(session) {
        Some(id) => format!("wt:{id}"),
        None => format!("s:{}", session.id),
    }
}

/// TS: tasksBoard.ts workLabel (`⎇ <branch>`, or `main checkout · <session>`).
pub fn work_label<S: SessionLike>(section: &str, sessions: &[S]) -> String {
    let working = work_sessions(section, sessions);
    if section.starts_with("wt:") {
        return match working.iter().find_map(|item| worktree_branch(item.session())) {
            Some(branch) => format!("⎇ {branch}"),
            None if !working.is_empty() => "⎇ worktree".to_owned(),
            None => "⎇ a worktree not listed".to_owned(),
        };
    }
    match working.first() {
        Some(first) => format!("main checkout · {}", display_session_title(first.session(), sessions)),
        None => "a session not listed".to_owned(),
    }
}

/// TS: tasksBoard.ts urgency (needs you 3, working 2, idle 1, exited 0).
pub fn urgency(session: &SessionRecord) -> u8 {
    let attention = if session.status == SessionStatus::Running {
        session.attention.as_ref().map(|attention| attention.state)
    } else {
        None
    };
    if matches!(
        attention,
        Some(AttentionState::NeedsInput | AttentionState::Failed | AttentionState::Limited)
    ) {
        return 3;
    }
    if matches!(session.status, SessionStatus::Running | SessionStatus::Starting) {
        return if session.agent_status == Some(AgentActivityStatus::Active) {
            2
        } else {
            1
        };
    }
    0
}

/// TS: tasksBoard.ts leadSession (the most urgent; the first on ties).
pub fn lead_session<S: SessionLike>(sessions: &[S]) -> Option<&S> {
    sessions.iter().fold(None, |best: Option<&S>, item| match best {
        Some(best) if urgency(item.session()) <= urgency(best.session()) => Some(best),
        _ => Some(item),
    })
}

/// TS: tasksBoard.ts linkedTasks.
pub fn linked_tasks<'a>(tasks: &'a [Task], session: Option<&SessionRecord>) -> Vec<&'a Task> {
    let Some(session) = session else {
        return Vec::new();
    };
    let session_worktree = session.worktree.as_ref().and_then(|worktree| worktree.id.as_deref());
    tasks
        .iter()
        .filter(|task| {
            is_linked(task)
                && match task.meta.wt.as_deref().filter(|wt| !wt.is_empty()) {
                    Some(wt) => Some(wt) == session_worktree,
                    None => task.meta.s.as_deref() == Some(session.id.as_str()),
                }
        })
        .collect()
}

/// TS: tasksBoard.ts linkedTask.
pub fn linked_task<'a>(tasks: &'a [Task], session: Option<&SessionRecord>) -> Option<&'a Task> {
    let linked = linked_tasks(tasks, session);
    linked
        .iter()
        .find(|task| !task.done)
        .or_else(|| linked.first())
        .copied()
}

/// TS: tasksBoard.ts otherOpenTasks.
pub fn other_open_tasks(tasks: &[Task], session: Option<&SessionRecord>) -> usize {
    let main = linked_task(tasks, session);
    linked_tasks(tasks, session)
        .into_iter()
        .filter(|task| !task.done && !main.is_some_and(|main| std::ptr::eq(main, *task)))
        .count()
}

/// TS: tasksBoard.ts TaskState.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TaskState {
    pub glyph: String,
    pub color: Color,
    pub text: String,
    #[serde(rename = "where")]
    pub where_: Option<String>,
}

/// TS: tasksBoard.ts taskState (`all_sessions` defaults to `sessions`).
pub fn task_state<S: SessionLike>(
    task: &Task,
    sessions: &[S],
    spinner_frame: &str,
    all_sessions: Option<&[S]>,
) -> TaskState {
    let meta = &task.meta;
    if task.done {
        let text = match meta.auto {
            Some(TaskAuto::Merge) => "merged",
            Some(TaskAuto::Done) => "done (D)",
            Some(TaskAuto::Lines) | None => "done",
        };
        return TaskState {
            glyph: "✓".to_owned(),
            color: THEME.success,
            text: text.to_owned(),
            where_: meta.done.clone().filter(|done| !done.is_empty()),
        };
    }
    if !is_linked(task) {
        let given = |value: &Option<String>| value.clone().filter(|value| !value.is_empty());
        let text = if let Some(tried) = given(&meta.tried) {
            format!("tried in ⎇ {tried}")
        } else if let Some(from) = given(&meta.from) {
            format!("left open in ⎇ {from}")
        } else if let Some(added) = given(&meta.added) {
            format!("added {added}")
        } else {
            String::new()
        };
        return TaskState {
            glyph: "○".to_owned(),
            color: THEME.muted,
            text,
            where_: None,
        };
    }
    work_state(
        &link_key(meta).unwrap_or_default(),
        sessions,
        spinner_frame,
        all_sessions,
    )
}

/// TS: tasksBoard.ts workState (`all_sessions` defaults to `sessions`).
pub fn work_state<S: SessionLike>(
    section: &str,
    sessions: &[S],
    spinner_frame: &str,
    all_sessions: Option<&[S]>,
) -> TaskState {
    let all_sessions = all_sessions.unwrap_or(sessions);
    let working = work_sessions(section, sessions);
    let Some(lead) = lead_session(&working).map(SessionLike::session) else {
        return TaskState {
            glyph: "◌".to_owned(),
            color: THEME.muted,
            text: "session not shown".to_owned(),
            where_: None,
        };
    };
    let where_ = match worktree_branch(lead) {
        Some(branch) => format!("⎇ {branch}"),
        None => display_session_title(lead, all_sessions),
    };
    TaskState {
        glyph: status_glyph(lead, spinner_frame),
        color: status_color(lead),
        text: if urgency(lead) == 3 {
            "needs you"
        } else {
            status_words(lead)
        }
        .to_owned(),
        where_: Some(where_),
    }
}

/// TS: tasksBoard.ts taskLinkLookup result: a TaskLinkLookup owning its snapshot of the list.
pub type OwnedTaskLinkLookup = Box<dyn Fn(&str) -> TaskLinkState>;

/// TS: tasksBoard.ts taskLinkLookup (`None` while the list is not loaded).
pub fn task_link_lookup(tasks: Option<&[Task]>) -> Option<OwnedTaskLinkLookup> {
    // `new Map(...)`: a later task with the same id wins.
    let done: HashMap<String, bool> = tasks?.iter().map(|task| (task.id.clone(), task.done)).collect();
    Some(Box::new(move |id: &str| match done.get(id) {
        None => TaskLinkState::Gone,
        Some(true) => TaskLinkState::Done,
        Some(false) => TaskLinkState::Open,
    }))
}

/// TS: tasksBoard.ts taskCountLabel (`☐ 3 tasks · b`, empty without open tasks).
pub fn task_count_label(tasks: &[Task]) -> String {
    match tasks.iter().filter(|task| !task.done).count() {
        0 => String::new(),
        1 => "☐ 1 task · b".to_owned(),
        open => format!("☐ {open} tasks · b"),
    }
}

/// TS: tasksBoard.ts worktreeOpenItems.
pub fn worktree_open_items<S: SessionLike>(record_id: Option<&str>, sessions: &[S]) -> Vec<String> {
    let Some(record_id) = record_id.filter(|id| !id.is_empty()) else {
        return Vec::new();
    };
    match work_note(&format!("wt:{record_id}"), sessions) {
        Some(note) if note.read_only != Some(true) => open_note_items(Some(&note.text)),
        _ => Vec::new(),
    }
}

/// TS: tasksBoard.ts openItemsRemovedWith.
pub fn open_items_removed_with<S: SessionLike>(session: &SessionRecord, sessions: &[S]) -> Vec<String> {
    // A worktree's note goes with its record, so with its last session.
    let last = worktree_id(session).is_some_and(|worktree| {
        !sessions.iter().map(SessionLike::session).any(|other| {
            other.id != session.id && other.worktree.as_ref().and_then(|record| record.id.as_deref()) == Some(worktree)
        })
    });
    let mut items = open_note_items(session.notes.as_deref());
    if last
        && let Some(shared) = session.shared_notes.as_ref()
        && shared.kind == SharedNoteKind::Worktree
    {
        items.extend(open_note_items(Some(&shared.text)));
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsstr::date;
    use crate::session_features::tests::{partial_session, session as sess, sessions_arg};
    use crate::tasks::parse_tasks;
    use deckhand_testkit::{Case, replay, tags};
    use serde_json::{Value, json};

    // ── Fixture plumbing ──────────────────────────────────────────────────────────────────────────────────────

    fn de<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
        serde_json::from_value(value).map_err(|error| error.to_string())
    }

    fn tasks_arg(value: &Value) -> Result<Vec<Task>, String> {
        de(tags::untag(value))
    }

    fn task_arg(value: &Value) -> Result<Task, String> {
        de(tags::untag(value))
    }

    fn opt_str(case: &Case, index: usize) -> Result<Option<&str>, String> {
        if case.arg_is_undefined(index) {
            Ok(None)
        } else {
            case.arg_str(index).map(Some)
        }
    }

    /// Index of `item` (a reference into `list`).
    fn index_in<T>(list: &[T], item: &T) -> usize {
        list.iter()
            .position(|candidate| std::ptr::eq(candidate, item))
            .unwrap_or(usize::MAX)
    }

    /// The given argument item at `index` (TS returns the same objects it was given).
    fn given(arg: &Value, index: usize) -> Value {
        arg.get(index).cloned().unwrap_or(Value::Null)
    }

    fn session_or_none(case: &Case, index: usize) -> Result<Option<SessionRecord>, String> {
        if case.arg_is_undefined(index) {
            Ok(None)
        } else {
            partial_session(case.arg(index)).map(Some)
        }
    }

    fn sessions_or_none(case: &Case, index: usize) -> Result<Option<Vec<SessionRecord>>, String> {
        if case.arg_is_undefined(index) {
            Ok(None)
        } else {
            sessions_arg(case.arg(index)).map(Some)
        }
    }

    // ── Ported TS tests (tests/tasksBoard.test.ts) ────────────────────────────────────────────────────────────

    fn now() -> i64 {
        date::parse("2026-10-09T12:00:00Z").unwrap_or_default()
    }

    /// The test's `session(id, fields)`.
    fn session_with(id: &str, fields: Value) -> SessionRecord {
        let mut base = json!({"id": id, "title": id, "cwd": "/repo", "repoRoot": "/repo", "program": "claude",
            "status": "running", "createdAt": "2026-10-01T00:00:00Z", "updatedAt": "2026-10-01T00:00:00Z"});
        if let (Some(base), Some(fields)) = (base.as_object_mut(), fields.as_object()) {
            base.extend(fields.clone());
        }
        sess(base)
    }

    /// The test's `inWorktree(id, wt, branch, fields)`.
    fn in_worktree(id: &str, wt: &str, branch: &str, fields: Value) -> SessionRecord {
        let mut base = json!({"cwd": format!("/wt/{branch}"),
            "worktree": {"mode": "managed", "id": wt, "path": format!("/wt/{branch}"), "branch": branch}});
        if let (Some(base), Some(fields)) = (base.as_object_mut(), fields.as_object()) {
            base.extend(fields.clone());
        }
        session_with(id, base)
    }

    fn base_sessions() -> Vec<SessionRecord> {
        vec![
            session_with("main", json!({})),
            in_worktree("auth", "w1", "feat/auth", json!({})),
            in_worktree("auth-tests", "w1", "feat/auth", json!({})),
            in_worktree("e2e", "w2", "fix/e2e", json!({})),
            in_worktree("old", "w3", "old/work", json!({"archivedAt": "2026-10-02T00:00:00Z"})),
        ]
    }

    fn base_tasks() -> Vec<Task> {
        parse_tasks(
            &[
                "- [ ] Fix flaky e2e <!-- dh:t=e wt=w2 -->",
                "- [ ] Add OAuth <!-- dh:t=a wt=w1 -->",
                "- [ ] Rate-limit export <!-- dh:t=r -->",
                "- [ ] Add tests <!-- dh:t=t wt=w1 -->",
                "- [x] Shipped <!-- dh:t=s wt=w1 done=2026-10-08 auto=merge -->",
            ]
            .join("\n"),
        )
    }

    #[test]
    fn session_linked_task() {
        let (tasks, sessions) = (base_tasks(), base_sessions());
        assert_eq!(
            linked_task(&tasks, Some(&sessions[2])).map(|task| task.title.as_str()),
            Some("Add OAuth")
        );
        assert_eq!(other_open_tasks(&tasks, Some(&sessions[2])), 1);
        let follow_up_only = parse_tasks(
            "- [x] Done <!-- dh:t=d wt=w1 done=2026-10-08 -->\n- [ ] Later <!-- dh:t=l wt=w1 assigned=2026-10-09 -->",
        );
        assert_eq!(
            linked_task(&follow_up_only, Some(&sessions[1])).map(|task| task.title.as_str()),
            Some("Later")
        );
        assert_eq!(other_open_tasks(&follow_up_only, Some(&sessions[1])), 0);
        assert_eq!(linked_task(&tasks, Some(&sessions[0])), None);
    }

    // ── Parity replays ────────────────────────────────────────────────────────────────────────────────────────

    #[test]
    fn work_note_parity() {
        replay("tasksBoard/workNote", |case: &Case| -> Result<Value, String> {
            let sessions = sessions_arg(case.arg(1))?;
            Ok(work_note(case.arg_str(0)?, &sessions).map_or_else(tags::undefined, |note| json!(note)))
        });
    }

    #[test]
    fn work_sessions_parity() {
        replay("tasksBoard/workSessions", |case: &Case| -> Result<Value, String> {
            let sessions = sessions_arg(case.arg(1))?;
            let found = work_sessions(case.arg_str(0)?, &sessions);
            Ok(Value::Array(
                found
                    .into_iter()
                    .map(|item| given(case.arg(1), index_in(&sessions, item)))
                    .collect(),
            ))
        });
    }

    #[test]
    fn work_key_of_parity() {
        replay("tasksBoard/workKeyOf", |case: &Case| {
            partial_session(case.arg(0)).map(|session| work_key_of(&session))
        });
    }

    #[test]
    fn work_label_parity() {
        replay("tasksBoard/workLabel", |case: &Case| -> Result<String, String> {
            Ok(work_label(case.arg_str(0)?, &sessions_arg(case.arg(1))?))
        });
    }

    #[test]
    fn lead_session_parity() {
        replay("tasksBoard/leadSession", |case: &Case| -> Result<Value, String> {
            let sessions = sessions_arg(case.arg(0))?;
            Ok(lead_session(&sessions)
                .map_or_else(tags::undefined, |lead| given(case.arg(0), index_in(&sessions, lead))))
        });
    }

    #[test]
    fn linked_tasks_parity() {
        replay("tasksBoard/linkedTasks", |case: &Case| -> Result<Value, String> {
            let tasks = tasks_arg(case.arg(0))?;
            let session = session_or_none(case, 1)?;
            let found = linked_tasks(&tasks, session.as_ref());
            Ok(Value::Array(
                found
                    .into_iter()
                    .map(|task| given(case.arg(0), index_in(&tasks, task)))
                    .collect(),
            ))
        });
    }

    #[test]
    fn linked_task_parity() {
        replay("tasksBoard/linkedTask", |case: &Case| -> Result<Value, String> {
            let tasks = tasks_arg(case.arg(0))?;
            let session = session_or_none(case, 1)?;
            Ok(linked_task(&tasks, session.as_ref())
                .map_or_else(tags::undefined, |task| given(case.arg(0), index_in(&tasks, task))))
        });
    }

    #[test]
    fn other_open_tasks_parity() {
        replay("tasksBoard/otherOpenTasks", |case: &Case| -> Result<usize, String> {
            let tasks = tasks_arg(case.arg(0))?;
            let session = session_or_none(case, 1)?;
            Ok(other_open_tasks(&tasks, session.as_ref()))
        });
    }

    #[test]
    fn task_state_parity() {
        replay("tasksBoard/taskState", |case: &Case| -> Result<TaskState, String> {
            let task = task_arg(case.arg(0))?;
            let sessions = sessions_arg(case.arg(1))?;
            let all = sessions_or_none(case, 3)?;
            Ok(task_state(&task, &sessions, case.arg_str(2)?, all.as_deref()))
        });
    }

    #[test]
    fn work_state_parity() {
        replay("tasksBoard/workState", |case: &Case| -> Result<TaskState, String> {
            let sessions = sessions_arg(case.arg(1))?;
            let all = sessions_or_none(case, 3)?;
            Ok(work_state(
                case.arg_str(0)?,
                &sessions,
                case.arg_str(2)?,
                all.as_deref(),
            ))
        });
    }

    #[test]
    fn task_link_lookup_parity() {
        // The fixture applies the lookup to a list of ids (a closure cannot be recorded).
        replay("tasksBoard/taskLinkLookup", |case: &Case| -> Result<Value, String> {
            let tasks = if case.arg_is_undefined(0) {
                None
            } else {
                Some(tasks_arg(case.arg(0))?)
            };
            let ids: Vec<String> = case.arg_as(1)?;
            Ok(
                task_link_lookup(tasks.as_deref()).map_or_else(tags::undefined, |lookup| {
                    json!(ids.iter().map(|id| lookup(id)).collect::<Vec<_>>())
                }),
            )
        });
    }

    #[test]
    fn task_count_label_parity() {
        replay("tasksBoard/taskCountLabel", |case: &Case| {
            tasks_arg(case.arg(0)).map(|tasks| task_count_label(&tasks))
        });
    }

    #[test]
    fn worktree_open_items_parity() {
        replay(
            "tasksBoard/worktreeOpenItems",
            |case: &Case| -> Result<Vec<String>, String> {
                Ok(worktree_open_items(opt_str(case, 0)?, &sessions_arg(case.arg(1))?))
            },
        );
    }

    #[test]
    fn open_items_removed_with_parity() {
        replay(
            "tasksBoard/openItemsRemovedWith",
            |case: &Case| -> Result<Vec<String>, String> {
                let session = partial_session(case.arg(0))?;
                Ok(open_items_removed_with(&session, &sessions_arg(case.arg(1))?))
            },
        );
    }

    #[test]
    fn sending_open_items() -> Result<(), Box<dyn std::error::Error>> {
        use crate::tasks::{PromotedItem, apply_task_op, promote_open_note_lines};
        use deckhand_protocol::data::{TaskLink, TaskOp};
        let note = "# Plan\n- [x] Read the spec\n- [ ] Write the parser\n  - [ ] Handle tabs\n- ↗ Docs <!-- dh:t=d0c5 -->\n- [ ] \nprose";
        let mut next = 0;
        let mut new_id = || {
            next += 1;
            format!("id{}", next - 1)
        };
        let sent = promote_open_note_lines(note, &mut new_id);
        let item = |id: &str, title: &str| PromotedItem {
            id: id.to_owned(),
            title: title.to_owned(),
        };
        assert_eq!(
            sent.items,
            [item("id0", "Write the parser"), item("id1", "Handle tabs")]
        );
        assert_eq!(
            sent.text.split('\n').collect::<Vec<_>>(),
            [
                "# Plan",
                "- [x] Read the spec",
                "- ↗ Write the parser <!-- dh:t=id0 -->",
                "  - ↗ Handle tabs <!-- dh:t=id1 -->",
                "- ↗ Docs <!-- dh:t=d0c5 -->",
                "- [ ] ",
                "prose"
            ]
        );
        let plain = promote_open_note_lines("just text", &mut || "x".to_owned());
        assert_eq!((plain.text.as_str(), plain.items.len()), ("just text", 0));
        let added = apply_task_op(
            "",
            &TaskOp::Add {
                title: "Write the parser".to_owned(),
                body: None,
                link: None,
                id: Some("id0".to_owned()),
                from: Some("feat/auth".to_owned()),
                parent: None,
                at: None,
            },
            now(),
        )?;
        assert!(
            added
                .text
                .starts_with("- [ ] Write the parser <!-- dh:t=id0 from=feat/auth added=2026-10-09 -->"),
            "{}",
            added.text
        );
        // Assigned to a work, `from` does not apply.
        let linked = apply_task_op(
            "",
            &TaskOp::Add {
                title: "x".to_owned(),
                body: None,
                link: Some(TaskLink::Worktree { wt: "w1".to_owned() }),
                id: None,
                from: Some("feat/auth".to_owned()),
                parent: None,
                at: None,
            },
            now(),
        )?;
        assert!(!linked.text.contains("from="), "{}", linked.text);
        // What merging or deleting the worktree offers to send: its note's open items (not a deleted worktree's read-only note).
        let sessions = vec![
            in_worktree(
                "auth",
                "w1",
                "feat/auth",
                json!({"sharedNotes": {"kind": "worktree", "id": "w1", "path": "/n/w1.md", "text": note, "revision": "r1"}}),
            ),
            in_worktree(
                "gone",
                "w9",
                "gone",
                json!({"sharedNotes": {"kind": "worktree", "id": "w9", "path": "/n/w9.md", "text": "- [ ] Left behind",
                    "revision": "r9", "readOnly": true}}),
            ),
        ];
        assert_eq!(
            worktree_open_items(Some("w1"), &sessions),
            ["Write the parser", "Handle tabs"]
        );
        assert!(worktree_open_items(Some("w9"), &sessions).is_empty());
        assert!(worktree_open_items(None, &sessions).is_empty());
        Ok(())
    }

    #[test]
    fn link_lines_say_where_their_task_is() {
        use crate::notes::{NoteRow, NoteRowKind, note_read_rows};
        let tasks = parse_tasks("- [ ] Open <!-- dh:t=aaaa -->\n- [x] Done <!-- dh:t=bbbb -->");
        let links = task_link_lookup(Some(&tasks));
        let state = |id: &str| links.as_deref().map(|links| links(id));
        assert_eq!(state("aaaa"), Some(TaskLinkState::Open));
        assert_eq!(state("bbbb"), Some(TaskLinkState::Done));
        assert_eq!(state("cccc"), Some(TaskLinkState::Gone));
        // Before the list is loaded nothing is claimed.
        assert!(task_link_lookup(None).is_none());
        assert_eq!(
            note_read_rows("- ↗ Done <!-- dh:t=bbbb -->", 40, links.as_deref()),
            [NoteRow {
                text: "↗ Done · done".to_owned(),
                kind: Some(NoteRowKind::Link)
            }]
        );
    }
}
