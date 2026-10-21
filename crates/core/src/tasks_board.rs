//! Port of tasksBoard.ts.
//!
//! The Tasks board (b) as pure data: its rows (one group per worktree, or main-checkout session, with open tasks;
//! the backlog; done; the folded older done ones; and the open note items not yet tasks), each task's linked sessions
//! and the state shown for it. A group is keyed by its work (`wt:<record id>` / `s:<session id>`, see
//! `tasks::link_key`). The tasks flow keeps the selection and keys.
//!
//! Note rows share their [`NoteBlock`] (`Rc`): a note's every line row points at it rather than copying its text.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::display::{Color, THEME, display_session_title, status_color, status_glyph};
use crate::jsstr::{js_slice, js_split_whitespace, js_to_lower, js_trim, js_trim_end};
use crate::notes::{TaskLinkLookup, TaskLinkState, note_key, parse_checklist_line, task_link_suffix};
use crate::session_order::{SessionLike, sort_sessions_for_sidebar};
use crate::sidebar_model::status_words;
use crate::tasks::{
    find_note_task_link, group_tasks, is_linked, link_key, link_of_key, open_note_items, parse_note_task_link,
};
use crate::workspace::workspace_key;
use deckhand_protocol::data::{
    AgentActivityStatus, AttentionState, NoteSection, SessionRecord, SessionStatus, SharedNoteKind, Task, TaskAuto,
    TaskLink,
};

/// TS noteLine's heading: `/^#{1,6}[ \t]+(.*)$/` (JS `.` stops at every line terminator, not only `\n`).
static HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("^#{1,6}[ \t]+([^\n\r\u{2028}\u{2029}]*)$").expect("HEADING is a valid regex"));

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

/// TS: tasksBoard.ts TaskGroup.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskGroup {
    #[serde(rename = "progress")]
    Progress,
    #[serde(rename = "backlog")]
    Backlog,
    #[serde(rename = "done")]
    Done,
}

impl TaskGroup {
    fn as_str(self) -> &'static str {
        match self {
            TaskGroup::Progress => "progress",
            TaskGroup::Backlog => "backlog",
            TaskGroup::Done => "done",
        }
    }
}

/// TS: tasksBoard.ts NoteItem (an open checklist item of a note, not a task).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteItem {
    pub key: String,
    pub title: String,
    pub session_id: String,
    pub section: NoteSection,
    /// The note's line of the item (0-based) and the revision it was read at.
    pub line: usize,
    pub revision: String,
    pub note_id: Option<String>,
    /// The note it is in: `worktree note`, `main checkout note`, or the session's title.
    pub source: String,
    /// Its group on the board: the worktree (`wt:<record id>`) or `main`.
    pub group: String,
}

/// TS: tasksBoard.ts BoardRow `empty.addsTo`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AddsTo {
    #[serde(rename = "backlog")]
    Backlog,
    #[serde(rename = "work")]
    Work,
}

impl AddsTo {
    fn as_str(self) -> &'static str {
        match self {
            AddsTo::Backlog => "backlog",
            AddsTo::Work => "work",
        }
    }
}

/// TS: tasksBoard.ts NoteLineStyle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NoteLineStyle {
    #[serde(rename = "open")]
    Open,
    #[serde(rename = "done")]
    Done,
    #[serde(rename = "link")]
    Link,
    #[serde(rename = "heading")]
    Heading,
    #[serde(rename = "text")]
    Text,
}

/// TS: tasksBoard.ts the `steps` of a task row: its work note's checklist, done/total (also stepCount's result).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepCount {
    pub done: usize,
    pub total: usize,
}

/// TS: tasksBoard.ts BoardRow.
#[derive(Clone, Debug, PartialEq)]
pub enum BoardRow {
    Heading {
        text: String,
        count: Option<String>,
    },
    /// The heading of a work's group: `section` is its key (`main` in the note list); `label` replaces its name.
    Work {
        section: String,
        count: usize,
        label: Option<String>,
    },
    /// `section`: the work's key for tasks in progress, else `backlog` or `done`.
    Task {
        task: Task,
        group: TaskGroup,
        section: String,
        steps: Option<StepCount>,
        heads_work: Option<bool>,
    },
    /// A checklist item of the work's note, under the task the work was started for.
    Step {
        section: String,
        task_id: String,
        step: WorkStep,
    },
    Older {
        count: usize,
        shown: bool,
    },
    Empty {
        text: String,
        adds_to: Option<AddsTo>,
    },
    /// The Notes view (tab): a note's heading (its open items counted)...
    NoteHead {
        block: Rc<NoteBlock>,
        open: usize,
    },
    /// ...then each line of it.
    NoteLine {
        block: Rc<NoteBlock>,
        line: usize,
        text: String,
        style: NoteLineStyle,
        item: Option<NoteItem>,
    },
}

/// TS: tasksBoard.ts selectableRow.
pub fn selectable_row(row: &BoardRow) -> bool {
    match row {
        BoardRow::Task { .. }
        | BoardRow::Step { .. }
        | BoardRow::Older { .. }
        | BoardRow::NoteHead { .. }
        | BoardRow::NoteLine { .. } => true,
        BoardRow::Empty { adds_to, .. } => adds_to.is_some(),
        BoardRow::Heading { .. } | BoardRow::Work { .. } => false,
    }
}

/// TS: tasksBoard.ts rowKey.
pub fn row_key(row: &BoardRow) -> Option<String> {
    match row {
        BoardRow::Task { task, .. } => Some(format!("task:{}", task.id)),
        BoardRow::Step { section, step, .. } => Some(format!("step:{section}:{}", step.line)),
        BoardRow::Older { .. } => Some("older".to_owned()),
        BoardRow::Empty {
            adds_to: Some(adds_to), ..
        } => Some(format!("empty:{}", adds_to.as_str())),
        BoardRow::NoteHead { block, .. } => Some(format!("notehead:{}", block.key)),
        BoardRow::NoteLine { block, line, .. } => Some(format!("noteline:{}:{line}", block.key)),
        BoardRow::Empty { adds_to: None, .. } | BoardRow::Heading { .. } | BoardRow::Work { .. } => None,
    }
}

/// TS: tasksBoard.ts WorkStep (one checklist item of a work's note).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkStep {
    pub line: usize,
    pub text: String,
    pub done: bool,
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

/// TS: tasksBoard.ts stepCount.
pub fn step_count(steps: &[WorkStep]) -> StepCount {
    StepCount {
        done: steps.iter().filter(|step| step.done).count(),
        total: steps.len(),
    }
}

/// TS: tasksBoard.ts noteSteps (checklist items with text, not `↗` links).
pub fn note_steps(text: Option<&str>) -> Vec<WorkStep> {
    text.unwrap_or("")
        .split('\n')
        .enumerate()
        .filter_map(|(index, line)| {
            let item = parse_checklist_line(line)?;
            let text = js_trim(&item.text);
            (!text.is_empty()).then(|| WorkStep {
                line: index,
                text: text.to_owned(),
                done: item.checked,
            })
        })
        .collect()
}

/// TS: tasksBoard.ts boardRows `stepsOf`: a work's steps (its note's checklist).
pub type StepsOf<'a> = &'a dyn Fn(&str) -> Vec<WorkStep>;

/// boardRows' `section`: a heading, its tasks, and `empty` (if any) when there are none.
fn push_section(rows: &mut Vec<BoardRow>, text: &str, list: &[Task], group: TaskGroup, empty: Option<&str>) {
    rows.push(BoardRow::Heading {
        text: text.to_owned(),
        count: (!list.is_empty()).then(|| list.len().to_string()),
    });
    for task in list {
        rows.push(BoardRow::Task {
            task: task.clone(),
            group,
            section: group.as_str().to_owned(),
            steps: None,
            heads_work: None,
        });
    }
    if let (true, Some(empty)) = (list.is_empty(), empty) {
        rows.push(BoardRow::Empty {
            text: empty.to_owned(),
            adds_to: (group == TaskGroup::Backlog).then_some(AddsTo::Backlog),
        });
    }
}

/// TS: tasksBoard.ts boardRows (`scope`: v's work; `steps_of`: a work's steps).
pub fn board_rows(
    tasks: &[Task],
    show_older: bool,
    now_ms: i64,
    scope: Option<&str>,
    steps_of: Option<StepsOf<'_>>,
) -> Vec<BoardRow> {
    let scope = scope.filter(|scope| !scope.is_empty());
    let groups = match scope {
        Some(scope) => {
            let scoped: Vec<Task> = tasks
                .iter()
                .filter(|task| link_key(&task.meta).as_deref() == Some(scope))
                .cloned()
                .collect();
            group_tasks(&scoped, now_ms)
        }
        None => group_tasks(tasks, now_ms),
    };
    let mut rows: Vec<BoardRow> = Vec::new();
    let mut works: Vec<String> = Vec::new();
    for task in &groups.progress {
        let key = link_key(&task.meta).unwrap_or_default();
        if !works.contains(&key) {
            works.push(key);
        }
    }
    if let Some(scope) = scope
        && !works.iter().any(|work| work == scope)
    {
        works.push(scope.to_owned());
    }
    for work in &works {
        let list: Vec<&Task> = groups
            .progress
            .iter()
            .filter(|task| link_key(&task.meta).unwrap_or_default() == *work)
            .collect();
        // A work has one task, whose row names the work; a heading only for none (v) or several (a hand-edited list).
        let heads_work = list.len() == 1;
        if !heads_work {
            rows.push(BoardRow::Work {
                section: work.clone(),
                count: list.len(),
                label: None,
            });
        }
        let steps = steps_of.map(|steps_of| steps_of(work)).unwrap_or_default();
        for (index, task) in list.iter().enumerate() {
            // The first task leads its work's steps.
            let lead = index == 0 && !steps.is_empty();
            rows.push(BoardRow::Task {
                task: (*task).clone(),
                group: TaskGroup::Progress,
                section: work.clone(),
                steps: lead.then(|| step_count(&steps)),
                heads_work: heads_work.then_some(true),
            });
            if lead {
                for step in &steps {
                    rows.push(BoardRow::Step {
                        section: work.clone(),
                        task_id: task.id.clone(),
                        step: step.clone(),
                    });
                }
            }
        }
        if list.is_empty() {
            rows.push(BoardRow::Empty {
                text: "No open tasks here · a adds one".to_owned(),
                adds_to: Some(AddsTo::Work),
            });
        }
    }
    if works.is_empty() {
        push_section(
            &mut rows,
            "IN PROGRESS",
            &[],
            TaskGroup::Progress,
            Some("n on a backlog task starts a session for it · w gives it to a worktree"),
        );
    }
    if scope.is_none() {
        let empty = if tasks.is_empty() {
            "No tasks yet · a adds one"
        } else {
            "Nothing waiting · a adds a task here"
        };
        push_section(&mut rows, "BACKLOG", &groups.backlog, TaskGroup::Backlog, Some(empty));
    }
    if !groups.done.is_empty() || !groups.older_done.is_empty() {
        push_section(&mut rows, "DONE · this week", &groups.done, TaskGroup::Done, None);
        if !groups.older_done.is_empty() {
            rows.push(BoardRow::Older {
                count: groups.older_done.len(),
                shown: show_older,
            });
            if show_older {
                for task in &groups.older_done {
                    rows.push(BoardRow::Task {
                        task: task.clone(),
                        group: TaskGroup::Done,
                        section: "done".to_owned(),
                        steps: None,
                        heads_work: None,
                    });
                }
            }
        }
    }
    rows
}

/// TS: tasksBoard.ts hasMainTask.
pub fn has_main_task(section: &str, tasks: &[Task], except: Option<&str>) -> bool {
    tasks
        .iter()
        .any(|task| !task.done && Some(task.id.as_str()) != except && link_key(&task.meta).as_deref() == Some(section))
}

/// TS: tasksBoard.ts taskSessions.
pub fn task_sessions<'a, S: SessionLike>(task: &Task, sessions: &'a [S]) -> Vec<&'a S> {
    match link_key(&task.meta) {
        Some(section) => work_sessions(&section, sessions),
        None => Vec::new(),
    }
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

/// TS: tasksBoard.ts WorkOption kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WorkOptionKind {
    #[serde(rename = "backlog")]
    Backlog,
    #[serde(rename = "note")]
    Note,
    #[serde(rename = "worktree")]
    Worktree,
    #[serde(rename = "session")]
    Session,
}

/// TS: tasksBoard.ts WorkOption.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkOption {
    pub section: Option<String>,
    pub kind: WorkOptionKind,
    pub label: String,
    pub note: String,
    pub search: String,
}

/// TS: tasksBoard.ts PickerView.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PickerView {
    #[serde(rename = "worktrees")]
    Worktrees,
    #[serde(rename = "sessions")]
    Sessions,
}

/// TS: tasksBoard.ts pickerViewOf.
pub fn picker_view_of(section: Option<&str>) -> PickerView {
    if section.is_some_and(|section| section.starts_with("s:")) {
        PickerView::Sessions
    } else {
        PickerView::Worktrees
    }
}

/// TS: tasksBoard.ts workOptions.
pub fn work_options<S: SessionLike>(sessions: &[S], current: Option<&str>) -> Vec<WorkOption> {
    let current = current.filter(|current| !current.is_empty());
    let mut options = vec![WorkOption {
        section: None,
        kind: WorkOptionKind::Backlog,
        label: "Backlog · no worktree".to_owned(),
        note: if current.is_some() { "" } else { "here" }.to_owned(),
        search: String::new(),
    }];
    let mut seen: HashSet<String> = HashSet::new();
    for item in sort_sessions_for_sidebar(sessions) {
        let session = item.session();
        if workspace_key(session).is_none() {
            continue;
        }
        let section = work_key_of(session);
        if seen.contains(&section) {
            continue;
        }
        let working = work_sessions(&section, sessions);
        if working.iter().all(|other| archived(other.session())) {
            continue;
        }
        seen.insert(section.clone());
        let merged = worktree_id(session).is_some()
            && working.iter().any(|other| {
                other
                    .session()
                    .worktree
                    .as_ref()
                    .is_some_and(|worktree| truthy(worktree.merged_at.as_deref()))
            });
        let label = work_label(&section, sessions);
        let here = current == Some(section.as_str());
        let note = [if merged { "✓ merged" } else { "" }, if here { "here" } else { "" }]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let search = std::iter::once(label.clone())
            .chain(
                working
                    .iter()
                    .map(|other| display_session_title(other.session(), sessions)),
            )
            .collect::<Vec<_>>()
            .join(" ");
        options.push(WorkOption {
            kind: if section.starts_with("wt:") {
                WorkOptionKind::Worktree
            } else {
                WorkOptionKind::Session
            },
            section: Some(section),
            label,
            note,
            search: js_to_lower(&search),
        });
    }
    options
}

/// TS: tasksBoard.ts pickerRows.
pub fn picker_rows(options: &[WorkOption], view: PickerView, query: &str, origin: Option<&str>) -> Vec<WorkOption> {
    let lowered = js_to_lower(query);
    let terms = js_split_whitespace(&lowered);
    let kind = match view {
        PickerView::Worktrees => WorkOptionKind::Worktree,
        PickerView::Sessions => WorkOptionKind::Session,
    };
    let mut rows: Vec<WorkOption> = options
        .iter()
        .filter(|option| option.kind == WorkOptionKind::Backlog)
        .cloned()
        .collect();
    if let Some(origin) = origin.filter(|origin| !origin.is_empty()) {
        rows.push(WorkOption {
            section: None,
            kind: WorkOptionKind::Note,
            label: format!("↩ Back to its note · {origin}"),
            note: String::new(),
            search: String::new(),
        });
    }
    rows.extend(
        options
            .iter()
            .filter(|option| option.kind == kind && terms.iter().all(|term| option.search.contains(term)))
            .cloned(),
    );
    rows
}

/// TS: tasksBoard.ts pickerCounts result (`Record<PickerView, number>`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickerCounts {
    pub worktrees: usize,
    pub sessions: usize,
}

/// TS: tasksBoard.ts pickerCounts.
pub fn picker_counts(options: &[WorkOption], query: &str) -> PickerCounts {
    let count = |view: PickerView| {
        picker_rows(options, view, query, None)
            .iter()
            .filter(|option| option.kind != WorkOptionKind::Backlog)
            .count()
    };
    PickerCounts {
        worktrees: count(PickerView::Worktrees),
        sessions: count(PickerView::Sessions),
    }
}

/// TS: tasksBoard.ts taskOrigin.
pub fn task_origin<S: SessionLike>(task: &Task, sessions: &[S]) -> Option<String> {
    if task.done || is_linked(task) || !truthy(task.meta.t.as_deref()) {
        return None;
    }
    for item in sessions {
        let session = item.session();
        if find_note_task_link(session.notes.as_deref().unwrap_or(""), &task.id).is_some() {
            let title = display_session_title(session, sessions);
            return Some(if title.is_empty() { "session".to_owned() } else { title });
        }
        if let Some(shared) = session.shared_notes.as_ref()
            && shared.read_only != Some(true)
            && find_note_task_link(&shared.text, &task.id).is_some()
        {
            return Some(match shared.kind {
                SharedNoteKind::Repo => "main checkout".to_owned(),
                SharedNoteKind::Worktree => format!("⎇ {}", worktree_branch(session).unwrap_or("worktree")),
            });
        }
    }
    None
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
            None => "done",
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

/// TS: tasksBoard.ts NoteBlock (one note the Notes view lists).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteBlock {
    /// The note's draft key (`worktree:<id>`, `repo:<id>`, `session:<id>`).
    pub key: String,
    /// Its group: the worktree (`wt:<record id>`) or `main`.
    pub group: String,
    pub label: String,
    /// A session showing it (the first not archived, in sidebar order).
    pub session_id: String,
    pub section: NoteSection,
    pub note_id: Option<String>,
    pub text: String,
    pub revision: String,
}

/// noteBlocks' `groupOf`: the worktree (`wt:<record id>`) or `main`.
fn note_group_of(session: &SessionRecord) -> String {
    match worktree_id(session) {
        Some(id) => format!("wt:{id}"),
        None => "main".to_owned(),
    }
}

/// TS: tasksBoard.ts noteBlocks.
pub fn note_blocks<S: SessionLike>(sessions: &[S]) -> Vec<NoteBlock> {
    let ordered = sort_sessions_for_sidebar(sessions);
    let preferred = ordered
        .iter()
        .map(SessionLike::session)
        .filter(|session| !archived(session))
        .chain(
            ordered
                .iter()
                .map(SessionLike::session)
                .filter(|session| archived(session)),
        );
    let mut blocks: Vec<NoteBlock> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for session in preferred {
        let (Some(shared), Some(key)) = (session.shared_notes.as_ref(), note_key(session, NoteSection::Shared)) else {
            continue;
        };
        if shared.read_only == Some(true) || seen.contains(&key) || js_trim(&shared.text).is_empty() {
            continue;
        }
        seen.insert(key.clone());
        blocks.push(NoteBlock {
            key,
            group: note_group_of(session),
            label: match shared.kind {
                SharedNoteKind::Repo => "main checkout note",
                SharedNoteKind::Worktree => "worktree note",
            }
            .to_owned(),
            session_id: session.id.clone(),
            section: NoteSection::Shared,
            note_id: Some(format!("{}:{}", shared.kind, shared.id)),
            text: shared.text.clone(),
            revision: shared.revision.clone(),
        });
    }
    for session in ordered.iter().map(SessionLike::session) {
        let (Some(key), Some(notes), Some(notes_file)) = (
            note_key(session, NoteSection::Session),
            session.notes.as_deref(),
            session.notes_file.as_ref(),
        ) else {
            continue;
        };
        if js_trim(notes).is_empty() {
            continue;
        }
        let title = display_session_title(session, sessions);
        blocks.push(NoteBlock {
            key,
            group: note_group_of(session),
            label: if title.is_empty() { "session".to_owned() } else { title },
            session_id: session.id.clone(),
            section: NoteSection::Session,
            note_id: None,
            text: notes.to_owned(),
            revision: notes_file.revision.clone(),
        });
    }
    // Worktrees in sidebar order, the main checkout last; within a group, shared notes first.
    let mut groups: Vec<String> = Vec::new();
    let worktree_groups = ordered
        .iter()
        .filter_map(|item| worktree_id(item.session()))
        .map(|id| format!("wt:{id}"));
    for group in worktree_groups.chain(std::iter::once("main".to_owned())) {
        if !groups.contains(&group) {
            groups.push(group);
        }
    }
    groups
        .iter()
        .flat_map(|group| blocks.iter().filter(move |block| block.group == *group).cloned())
        .collect()
}

/// TS: tasksBoard.ts blockItems.
pub fn block_items<S: SessionLike>(block: &NoteBlock, sessions: &[S]) -> Vec<NoteItem> {
    let _ = sessions;
    block
        .text
        .split('\n')
        .enumerate()
        .filter_map(|(index, line)| {
            let item = parse_checklist_line(line)?;
            let title = js_trim(&item.text);
            if item.checked || title.is_empty() || parse_note_task_link(line).is_some() {
                return None;
            }
            Some(NoteItem {
                key: format!("{}:{index}", block.key),
                title: title.to_owned(),
                session_id: block.session_id.clone(),
                section: block.section,
                line: index,
                revision: block.revision.clone(),
                note_id: block.note_id.clone().filter(|id| !id.is_empty()),
                source: block.label.clone(),
                group: block.group.clone(),
            })
        })
        .collect()
}

/// TS: tasksBoard.ts noteItems.
pub fn note_items<S: SessionLike>(sessions: &[S]) -> Vec<NoteItem> {
    note_blocks(sessions)
        .iter()
        .flat_map(|block| block_items(block, sessions))
        .collect()
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

/// TS: tasksBoard.ts noteLine result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteLineView {
    pub text: String,
    pub style: NoteLineStyle,
}

/// TS: tasksBoard.ts noteLine (`None` for a blank line).
pub fn note_line(line: &str, links: Option<TaskLinkLookup<'_>>) -> Option<NoteLineView> {
    if js_trim(line).is_empty() {
        return None;
    }
    if let Some(link) = parse_note_task_link(line) {
        let suffix = task_link_suffix(links.map(|links| links(&link.id)));
        return Some(NoteLineView {
            text: format!("{}↗ {} {suffix}", link.indent, link.title),
            style: NoteLineStyle::Link,
        });
    }
    if let Some(item) = parse_checklist_line(line) {
        let mark = if item.checked { '☑' } else { '☐' };
        return Some(NoteLineView {
            text: format!("{}{mark} {}", item.indent, js_trim(&item.text)),
            style: if item.checked {
                NoteLineStyle::Done
            } else {
                NoteLineStyle::Open
            },
        });
    }
    if let Some(heading) = HEADING.captures(line).and_then(|captures| captures.get(1)) {
        return Some(NoteLineView {
            text: js_trim(heading.as_str()).to_owned(),
            style: NoteLineStyle::Heading,
        });
    }
    Some(NoteLineView {
        text: js_trim_end(line).to_owned(),
        style: NoteLineStyle::Text,
    })
}

/// TS: tasksBoard.ts notesViewRows options.
#[derive(Clone, Copy, Default)]
pub struct NotesViewOptions<'a> {
    /// f: only open checklist items.
    pub items_only: bool,
    /// v: one worktree (`wt:<id>`) or a main-checkout session (`s:<id>`).
    pub scope: Option<&'a str>,
    pub links: Option<TaskLinkLookup<'a>>,
}

/// One note of the Notes view: its block, its open items counted, its line rows.
struct NoteEntry {
    block: Rc<NoteBlock>,
    open: usize,
    lines: Vec<BoardRow>,
}

/// TS: tasksBoard.ts notesViewRows.
pub fn notes_view_rows<S: SessionLike>(sessions: &[S], options: &NotesViewOptions<'_>) -> Vec<BoardRow> {
    let scope = options.scope.filter(|scope| !scope.is_empty());
    let in_scope = |block: &NoteBlock| match scope {
        None => true,
        Some(scope) if scope.starts_with("wt:") => block.group == scope,
        Some(scope) => {
            block.group == "main"
                && (block.section == NoteSection::Shared || block.session_id == js_slice(scope, 2, None))
        }
    };
    let entries: Vec<NoteEntry> = note_blocks(sessions)
        .into_iter()
        .filter(|block| in_scope(block))
        .map(|block| {
            let block = Rc::new(block);
            let items = block_items(&block, sessions);
            let lines: Vec<BoardRow> = block
                .text
                .split('\n')
                .enumerate()
                .filter_map(|(line, raw)| {
                    let shown = note_line(raw, options.links)?;
                    if options.items_only && shown.style != NoteLineStyle::Open {
                        return None;
                    }
                    let item = if shown.style == NoteLineStyle::Open {
                        items.iter().find(|candidate| candidate.line == line).cloned()
                    } else {
                        None
                    };
                    Some(BoardRow::NoteLine {
                        block: Rc::clone(&block),
                        line,
                        text: shown.text,
                        style: shown.style,
                        item,
                    })
                })
                .collect();
            NoteEntry {
                block,
                open: items.len(),
                lines,
            }
        })
        .filter(|entry| !entry.lines.is_empty())
        .collect();
    if entries.is_empty() {
        let text = if options.items_only {
            "No open checklist items in these notes · f shows every note"
        } else if scope.is_some() {
            "No notes here yet · esc, then a session’s Notes tab (a in the sidebar)"
        } else {
            "No notes yet · esc, then a session’s Notes tab (a in the sidebar)"
        };
        return vec![BoardRow::Empty {
            text: text.to_owned(),
            adds_to: None,
        }];
    }
    let mut groups: Vec<&str> = Vec::new();
    for entry in &entries {
        if !groups.contains(&entry.block.group.as_str()) {
            groups.push(entry.block.group.as_str());
        }
    }
    let mut rows = Vec::new();
    for group in groups {
        let in_group: Vec<&NoteEntry> = entries.iter().filter(|entry| entry.block.group == group).collect();
        rows.push(BoardRow::Work {
            section: group.to_owned(),
            count: in_group.iter().map(|entry| entry.open).sum(),
            label: Some(note_group_label(group, sessions)),
        });
        for entry in in_group {
            rows.push(BoardRow::NoteHead {
                block: Rc::clone(&entry.block),
                open: entry.open,
            });
            rows.extend(entry.lines.iter().cloned());
        }
    }
    rows
}

/// TS: tasksBoard.ts noteGroupLabel.
pub fn note_group_label<S: SessionLike>(group: &str, sessions: &[S]) -> String {
    if group == "main" {
        "main checkout".to_owned()
    } else {
        work_label(group, sessions)
    }
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
    use serde_json::{Map, Value, json};

    // ── Fixture plumbing ──────────────────────────────────────────────────────────────────────────────────────

    /// The fixed clock `installDeterminism` gives `new Date()`.
    const FIXED_NOW: &str = "2026-01-01T00:00:00.000Z";

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

    /// The TS `stepsOf` callbacks the fixtures pass.
    fn steps_fixed(section: &str) -> Vec<WorkStep> {
        let step = |line: usize, text: &str, done: bool| WorkStep {
            line,
            text: text.to_owned(),
            done,
        };
        if section == "wt:w1" {
            vec![step(0, "plan", true), step(2, "ship 😀", false)]
        } else if section.starts_with("s:") {
            vec![step(5, "中文", false)]
        } else {
            Vec::new()
        }
    }

    fn steps_none(_: &str) -> Vec<WorkStep> {
        Vec::new()
    }

    fn steps_fn(name: Option<&str>) -> Result<Option<StepsOf<'static>>, String> {
        match name {
            None => Ok(None),
            // `stepsUndefined` returns undefined, which `?? []` reads as no steps.
            Some("stepsNone" | "stepsUndefined") => Ok(Some(&steps_none)),
            Some("stepsFixed") => Ok(Some(&steps_fixed)),
            Some(other) => Err(format!("unknown stepsOf {other}")),
        }
    }

    fn links_open(_: &str) -> TaskLinkState {
        TaskLinkState::Open
    }
    fn links_done(_: &str) -> TaskLinkState {
        TaskLinkState::Done
    }
    fn links_gone(_: &str) -> TaskLinkState {
        TaskLinkState::Gone
    }
    /// `(id.charCodeAt(0) || 0) % 3`.
    fn links_by_code(id: &str) -> TaskLinkState {
        match id.encode_utf16().next().unwrap_or(0) % 3 {
            0 => TaskLinkState::Open,
            1 => TaskLinkState::Done,
            _ => TaskLinkState::Gone,
        }
    }

    fn links_fn(value: &Value) -> Result<Option<TaskLinkLookup<'static>>, String> {
        match tags::tag_of(value) {
            Some(("$fn", Value::String(name))) => match name.as_str() {
                "linksOpen" => Ok(Some(&links_open)),
                "linksDone" => Ok(Some(&links_done)),
                "linksGone" => Ok(Some(&links_gone)),
                "linksByCode" => Ok(Some(&links_by_code)),
                other => Err(format!("unknown links {other}")),
            },
            _ if tags::is_undefined(value) => Ok(None),
            _ => Err(format!("not a links fn: {value}")),
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

    fn row_value(row: &BoardRow) -> Value {
        let mut object = Map::new();
        let mut put = |key: &str, value: Value| {
            object.insert(key.to_owned(), value);
        };
        match row {
            BoardRow::Heading { text, count } => {
                put("kind", json!("heading"));
                put("text", json!(text));
                if let Some(count) = count {
                    put("count", json!(count));
                }
            }
            BoardRow::Work { section, count, label } => {
                put("kind", json!("work"));
                put("section", json!(section));
                put("count", json!(count));
                if let Some(label) = label {
                    put("label", json!(label));
                }
            }
            BoardRow::Task {
                task,
                group,
                section,
                steps,
                heads_work,
            } => {
                put("kind", json!("task"));
                put("task", json!(task));
                put("group", json!(group));
                put("section", json!(section));
                if let Some(steps) = steps {
                    put("steps", json!(steps));
                }
                if let Some(heads_work) = heads_work {
                    put("headsWork", json!(heads_work));
                }
            }
            BoardRow::Step { section, task_id, step } => {
                put("kind", json!("step"));
                put("section", json!(section));
                put("taskId", json!(task_id));
                put("step", json!(step));
            }
            BoardRow::Older { count, shown } => {
                put("kind", json!("older"));
                put("count", json!(count));
                put("shown", json!(shown));
            }
            BoardRow::Empty { text, adds_to } => {
                put("kind", json!("empty"));
                put("text", json!(text));
                if let Some(adds_to) = adds_to {
                    put("addsTo", json!(adds_to));
                }
            }
            BoardRow::NoteHead { block, open } => {
                put("kind", json!("notehead"));
                put("block", json!(block.as_ref()));
                put("open", json!(open));
            }
            BoardRow::NoteLine {
                block,
                line,
                text,
                style,
                item,
            } => {
                put("kind", json!("noteline"));
                put("block", json!(block.as_ref()));
                put("line", json!(line));
                put("text", json!(text));
                put("style", json!(style));
                if let Some(item) = item {
                    put("item", json!(item));
                }
            }
        }
        Value::Object(object)
    }

    fn rows_value(rows: &[BoardRow]) -> Value {
        Value::Array(rows.iter().map(row_value).collect())
    }

    fn row_from(value: &Value) -> Result<BoardRow, String> {
        let value = tags::untag(value);
        let field = |key: &str| value.get(key).cloned().unwrap_or(Value::Null);
        let opt = |key: &str| value.get(key).filter(|value| !value.is_null()).cloned();
        Ok(match value.get("kind").and_then(Value::as_str) {
            Some("heading") => BoardRow::Heading {
                text: de(field("text"))?,
                count: opt("count").map(de).transpose()?,
            },
            Some("work") => BoardRow::Work {
                section: de(field("section"))?,
                count: de(field("count"))?,
                label: opt("label").map(de).transpose()?,
            },
            Some("task") => BoardRow::Task {
                task: de(field("task"))?,
                group: de(field("group"))?,
                section: de(field("section"))?,
                steps: opt("steps").map(de).transpose()?,
                heads_work: opt("headsWork").map(de).transpose()?,
            },
            Some("step") => BoardRow::Step {
                section: de(field("section"))?,
                task_id: de(field("taskId"))?,
                step: de(field("step"))?,
            },
            Some("older") => BoardRow::Older {
                count: de(field("count"))?,
                shown: de(field("shown"))?,
            },
            Some("empty") => BoardRow::Empty {
                text: de(field("text"))?,
                adds_to: opt("addsTo").map(de).transpose()?,
            },
            Some("notehead") => BoardRow::NoteHead {
                block: Rc::new(de(field("block"))?),
                open: de(field("open"))?,
            },
            Some("noteline") => BoardRow::NoteLine {
                block: Rc::new(de(field("block"))?),
                line: de(field("line"))?,
                text: de(field("text"))?,
                style: de(field("style"))?,
                item: opt("item").map(de).transpose()?,
            },
            other => return Err(format!("unknown row kind {other:?}")),
        })
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

    fn describe(rows: &[BoardRow]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                BoardRow::Work { section, count, .. } => format!("[{section} {count}]"),
                BoardRow::Heading { text, .. } => format!("# {text}"),
                BoardRow::Task {
                    task,
                    section,
                    heads_work,
                    ..
                } => format!(
                    "{}{section}: {}",
                    if heads_work == &Some(true) { "◆ " } else { "" },
                    task.title
                ),
                BoardRow::Empty { text, .. } => format!("({text})"),
                other => kind_of(other).to_owned(),
            })
            .collect()
    }

    fn kind_of(row: &BoardRow) -> &'static str {
        match row {
            BoardRow::Heading { .. } => "heading",
            BoardRow::Work { .. } => "work",
            BoardRow::Task { .. } => "task",
            BoardRow::Step { .. } => "step",
            BoardRow::Older { .. } => "older",
            BoardRow::Empty { .. } => "empty",
            BoardRow::NoteHead { .. } => "notehead",
            BoardRow::NoteLine { .. } => "noteline",
        }
    }

    fn selectable_kinds(rows: &[BoardRow]) -> Vec<&'static str> {
        rows.iter().filter(|row| selectable_row(row)).map(kind_of).collect()
    }

    // covers: B-P6-01 board: a group per worktree with open tasks (first task's order; a lone task is its own heading), then the backlog and done
    #[test]
    fn board_groups_per_worktree() {
        assert_eq!(
            describe(&board_rows(&base_tasks(), false, now(), None, None)),
            [
                "◆ wt:w2: Fix flaky e2e",
                "[wt:w1 2]",
                "wt:w1: Add OAuth",
                "wt:w1: Add tests",
                "# BACKLOG",
                "backlog: Rate-limit export",
                "# DONE · this week",
                "done: Shipped",
            ]
        );
        // Nothing on a worktree yet: a hint where the groups go.
        let solo = describe(&board_rows(
            &parse_tasks("- [ ] Solo <!-- dh:t=x -->"),
            false,
            now(),
            None,
            None,
        ));
        assert_eq!(
            solo[..2],
            [
                "# IN PROGRESS",
                "(n on a backlog task starts a session for it · w gives it to a worktree)"
            ]
        );
    }

    // covers: B-P6-02 board: an empty backlog can be selected, so a adds there when every task is on a worktree
    #[test]
    fn board_empty_backlog_selectable() {
        let rows = board_rows(
            &parse_tasks("- [ ] Add OAuth <!-- dh:t=a wt=w1 -->"),
            false,
            now(),
            None,
            None,
        );
        assert_eq!(
            describe(&rows),
            [
                "◆ wt:w1: Add OAuth",
                "# BACKLOG",
                "(Nothing waiting · a adds a task here)"
            ]
        );
        assert_eq!(selectable_kinds(&rows), ["task", "empty"]);
        // Other placeholders stay unselectable.
        let solo = board_rows(&parse_tasks("- [ ] Solo <!-- dh:t=x -->"), false, now(), None, None);
        assert_eq!(selectable_kinds(&solo).len(), 1);
    }

    // covers: B-P6-03 board v: only one work's group (even empty) and its done tasks
    #[test]
    fn board_scoped_to_one_work() {
        let tasks = base_tasks();
        assert_eq!(
            describe(&board_rows(&tasks, false, now(), Some("wt:w1"), None)),
            [
                "[wt:w1 2]",
                "wt:w1: Add OAuth",
                "wt:w1: Add tests",
                "# DONE · this week",
                "done: Shipped"
            ]
        );
        assert_eq!(
            describe(&board_rows(&tasks, false, now(), Some("s:main"), None)),
            ["[s:main 0]", "(No open tasks here · a adds one)"]
        );
        // Selectable, so the footer offers a (its task) rather than keys that need a task.
        assert_eq!(
            selectable_kinds(&board_rows(&tasks, false, now(), Some("s:main"), None)),
            ["empty"]
        );
    }

    // covers: B-P6-04 work keys, labels and where w can move a task
    #[test]
    fn work_keys_labels_and_options() {
        let sessions = base_sessions();
        assert_eq!(work_key_of(&sessions[1]), "wt:w1");
        assert_eq!(work_key_of(&sessions[0]), "s:main");
        assert_eq!(work_label("wt:w1", &sessions), "⎇ feat/auth");
        assert_eq!(work_label("s:main", &sessions), "main checkout · main");
        assert_eq!(work_label("wt:gone", &sessions), "⎇ a worktree not listed");
        // The backlog, then each worktree and main-checkout session once, in sidebar order (not a worktree whose
        // sessions are all archived).
        let mut with_merged = sessions.clone();
        with_merged.push(in_worktree(
            "merged",
            "w4",
            "done/it",
            json!({"worktree": {"mode": "managed", "id": "w4", "path": "/wt/done", "branch": "done/it",
                "mergedAt": "2026-10-08T00:00:00Z"}}),
        ));
        let options = work_options(&with_merged, Some("wt:w1"));
        let shown: Vec<(Option<&str>, &str, &str)> = options
            .iter()
            .map(|option| (option.section.as_deref(), option.label.as_str(), option.note.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                (None, "Backlog · no worktree", ""),
                (Some("wt:w1"), "⎇ feat/auth", "here"),
                (Some("wt:w2"), "⎇ fix/e2e", ""),
                (Some("s:main"), "main checkout · main", ""),
                (Some("wt:w4"), "⎇ done/it", "✓ merged"),
            ]
        );
        assert_eq!(work_options(&sessions, None)[0].note, "here");
    }

    // covers: B-P6-05 a session's task: the open one it was started for, then a follow-up; the rest counted
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

    // covers: B-P6-06 the w menu: backlog and the way back to the note pinned, then one view (worktrees or main-checkout sessions), searchable
    #[test]
    fn work_picker_rows() {
        let mut many = base_sessions();
        many.push(session_with("notes", json!({"title": "triage notes"})));
        many.push(session_with("spike", json!({"title": "perf spike"})));
        let options = work_options(&many, Some("wt:w1"));
        let labels = |rows: Vec<WorkOption>| rows.into_iter().map(|row| row.label).collect::<Vec<_>>();
        assert_eq!(
            labels(picker_rows(&options, PickerView::Worktrees, "", None)),
            ["Backlog · no worktree", "⎇ feat/auth", "⎇ fix/e2e"]
        );
        assert_eq!(
            labels(picker_rows(&options, PickerView::Sessions, "", Some("triage notes"))),
            [
                "Backlog · no worktree",
                "↩ Back to its note · triage notes",
                "main checkout · main",
                "main checkout · triage notes",
                "main checkout · perf spike"
            ]
        );
        // Search matches every word, in labels and in the titles of a worktree's sessions; the pinned rows stay.
        assert_eq!(
            labels(picker_rows(&options, PickerView::Worktrees, "tests", None)),
            ["Backlog · no worktree", "⎇ feat/auth"]
        );
        assert_eq!(
            labels(picker_rows(&options, PickerView::Sessions, "PERF spike", None)),
            ["Backlog · no worktree", "main checkout · perf spike"]
        );
        assert_eq!(
            picker_counts(&options, ""),
            PickerCounts {
                worktrees: 2,
                sessions: 3
            }
        );
        assert_eq!(
            picker_counts(&options, "e2e"),
            PickerCounts {
                worktrees: 1,
                sessions: 0
            }
        );
        assert_eq!(picker_view_of(Some("s:main")), PickerView::Sessions);
        assert_eq!(picker_view_of(Some("wt:w1")), PickerView::Worktrees);
        assert_eq!(picker_view_of(None), PickerView::Worktrees);
    }

    // covers: B-P6-07 a task's origin: the note (session's or shared) still holding its ↗ line, unless done, some work's task, or read-only
    #[test]
    fn task_origin_from_notes() {
        let with_notes = vec![
            session_with(
                "a",
                json!({"title": "notes holder", "notes": "- ↗ From a session <!-- dh:t=11111111 -->"}),
            ),
            in_worktree(
                "b",
                "w1",
                "feat/auth",
                json!({"sharedNotes": {"kind": "worktree", "id": "w1",
                    "text": "- ↗ From the worktree <!-- dh:t=22222222 -->", "revision": "r"}}),
            ),
            in_worktree(
                "c",
                "w9",
                "gone",
                json!({"sharedNotes": {"kind": "worktree", "id": "w9", "text": "- ↗ Deleted <!-- dh:t=33333333 -->",
                    "revision": "r", "readOnly": true}}),
            ),
        ];
        let tasks = parse_tasks(
            &[
                "- [ ] From a session <!-- dh:t=11111111 -->",
                "- [ ] From the worktree <!-- dh:t=22222222 -->",
                "- [ ] Deleted <!-- dh:t=33333333 -->",
                "- [ ] Started <!-- dh:t=11111111 wt=w1 -->",
                "- [x] Done <!-- dh:t=22222222 -->",
            ]
            .join("\n"),
        );
        assert_eq!(tasks.len(), 5);
        assert_eq!(task_origin(&tasks[0], &with_notes).as_deref(), Some("notes holder"));
        assert_eq!(task_origin(&tasks[1], &with_notes).as_deref(), Some("⎇ feat/auth"));
        assert_eq!(task_origin(&tasks[2], &with_notes), None);
        assert_eq!(task_origin(&tasks[3], &with_notes), None);
        assert_eq!(task_origin(&tasks[4], &with_notes), None);
    }

    fn notes_of(text: &str) -> Value {
        json!({"notes": text, "notesFile": {"path": "/n", "revision": "r"}})
    }

    fn merged(mut a: Value, b: Value) -> Value {
        if let (Some(a), Some(b)) = (a.as_object_mut(), b.as_object()) {
            a.extend(b.clone());
        }
        a
    }

    // covers: B-P6-08 note items: every session's notes, grouped by worktree (its shared note first) in sidebar order, the main checkout last
    #[test]
    fn note_items_grouped() {
        let shared = |id: &str, text: &str| json!({"sharedNotes": {"kind": "worktree", "id": id, "text": text, "revision": "r"}});
        let all = vec![
            session_with(
                "triage",
                merged(
                    json!({"title": "triage"}),
                    notes_of("- [ ] main one\n- [x] ticked\n- ↗ sent <!-- dh:t=1 -->"),
                ),
            ),
            in_worktree(
                "a1",
                "w1",
                "feat/auth",
                merged(notes_of("- [ ] from a1"), shared("w1", "- [ ] shared auth")),
            ),
            in_worktree(
                "a2",
                "w1",
                "feat/auth",
                merged(notes_of(""), shared("w1", "- [ ] shared auth")),
            ),
            in_worktree("e", "w2", "fix/e2e", notes_of("- [ ] from e2e")),
        ];
        let items = note_items(&all);
        let shown: Vec<(&str, &str, &str)> = items
            .iter()
            .map(|item| (item.group.as_str(), item.title.as_str(), item.source.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                ("wt:w1", "shared auth", "worktree note"),
                ("wt:w1", "from a1", "a1"),
                ("wt:w2", "from e2e", "e"),
                ("main", "main one", "triage"),
            ]
        );
        assert_eq!(note_group_label("wt:w2", &all), "⎇ fix/e2e");
        assert_eq!(note_group_label("main", &all), "main checkout");
    }

    // covers: B-P6-09 the Notes view: every note in full by worktree, then the main checkout; f only open items; v one worktree or session
    #[test]
    fn notes_view() {
        let all = vec![
            session_with(
                "triage",
                merged(
                    json!({"title": "triage", "sharedNotes": {"kind": "repo", "id": "r1", "text": "# Repo\n- [x] shipped",
                        "revision": "r", "path": "/r"}}),
                    notes_of("context only\n\n- [ ] main item"),
                ),
            ),
            in_worktree(
                "a1",
                "w1",
                "feat/auth",
                merged(
                    notes_of(""),
                    json!({"sharedNotes": {"kind": "worktree", "id": "w1",
                        "text": "plan\n- [ ] add tests\n- ↗ sent <!-- dh:t=1234abcd -->", "revision": "r", "path": "/w"}}),
                ),
            ),
            in_worktree(
                "e",
                "w2",
                "fix/e2e",
                merged(
                    notes_of(""),
                    json!({"sharedNotes": {"kind": "worktree", "id": "w2", "text": "", "revision": "r", "path": "/w2"}}),
                ),
            ),
        ];
        let shape = |rows: &[BoardRow]| -> Vec<String> {
            rows.iter()
                .map(|row| match row {
                    BoardRow::Work { label, count, .. } => format!("[{} {count}]", label.as_deref().unwrap_or("")),
                    BoardRow::NoteHead { block, open } => format!("# {} ({open})", block.label),
                    BoardRow::NoteLine { style, item, text, .. } => format!(
                        "{}{}: {text}",
                        serde_json::to_value(style)
                            .ok()
                            .and_then(|style| style.as_str().map(str::to_owned))
                            .unwrap_or_default(),
                        if item.is_some() { "*" } else { "" }
                    ),
                    BoardRow::Empty { text, .. } => format!("({text})"),
                    other => kind_of(other).to_owned(),
                })
                .collect()
        };
        let view = |items_only: bool, scope: Option<&str>| {
            notes_view_rows(
                &all,
                &NotesViewOptions {
                    items_only,
                    scope,
                    links: None,
                },
            )
        };
        assert_eq!(
            shape(&view(false, None)),
            [
                "[⎇ feat/auth 1]",
                "# worktree note (1)",
                "text: plan",
                "open*: ☐ add tests",
                "link: ↗ sent · in Tasks",
                "[main checkout 1]",
                "# main checkout note (0)",
                "heading: Repo",
                "done: ☑ shipped",
                "# triage (1)",
                "text: context only",
                "open*: ☐ main item",
            ]
        );
        assert_eq!(
            shape(&view(true, None)),
            [
                "[⎇ feat/auth 1]",
                "# worktree note (1)",
                "open*: ☐ add tests",
                "[main checkout 1]",
                "# triage (1)",
                "open*: ☐ main item"
            ]
        );
        assert_eq!(
            shape(&view(false, Some("wt:w1")))[..2],
            ["[⎇ feat/auth 1]", "# worktree note (1)"]
        );
        let heads: Vec<String> = shape(&view(false, Some("s:triage")))
            .into_iter()
            .filter(|line| line.starts_with('#'))
            .collect();
        assert_eq!(heads, ["# main checkout note (0)", "# triage (1)"]);
        assert_eq!(
            shape(&view(false, Some("wt:w2"))),
            ["(No notes here yet · esc, then a session’s Notes tab (a in the sidebar))"]
        );
        // The open item carries what sending it to Tasks needs.
        let item = view(false, None)
            .into_iter()
            .find_map(|row| match row {
                BoardRow::NoteLine { item: Some(item), .. } => Some(item),
                _ => None,
            })
            .expect("an open item");
        assert_eq!(
            (item.section, item.note_id.as_deref(), item.line, item.title.as_str()),
            (NoteSection::Shared, Some("worktree:w1"), 1, "add tests")
        );
    }

    // ── Parity replays ────────────────────────────────────────────────────────────────────────────────────────

    #[test]
    fn selectable_row_parity() {
        replay("tasksBoard/selectableRow", |case: &Case| {
            row_from(case.arg(0)).map(|row| selectable_row(&row))
        });
    }

    #[test]
    fn row_key_parity() {
        replay("tasksBoard/rowKey", |case: &Case| -> Result<Value, String> {
            Ok(row_key(&row_from(case.arg(0))?).map_or_else(tags::undefined, Value::String))
        });
    }

    #[test]
    fn work_note_parity() {
        replay("tasksBoard/workNote", |case: &Case| -> Result<Value, String> {
            let sessions = sessions_arg(case.arg(1))?;
            Ok(work_note(case.arg_str(0)?, &sessions).map_or_else(tags::undefined, |note| json!(note)))
        });
    }

    #[test]
    fn step_count_parity() {
        replay("tasksBoard/stepCount", |case: &Case| {
            case.arg_as::<Vec<WorkStep>>(0).map(|steps| step_count(&steps))
        });
    }

    #[test]
    fn note_steps_parity() {
        replay("tasksBoard/noteSteps", |case: &Case| opt_str(case, 0).map(note_steps));
    }

    #[test]
    fn board_rows_parity() {
        replay("tasksBoard/boardRows", |case: &Case| -> Result<Value, String> {
            let tasks = tasks_arg(case.arg(0))?;
            let now = if case.arg_is_undefined(2) {
                FIXED_NOW.to_owned()
            } else {
                case.arg_as::<String>(2)?
            };
            let now_ms = date::parse(&now).ok_or("invalid date")?;
            let steps = steps_fn(case.fn_name(4))?;
            Ok(rows_value(&board_rows(
                &tasks,
                case.arg_bool(1)?,
                now_ms,
                opt_str(case, 3)?,
                steps,
            )))
        });
    }

    #[test]
    fn has_main_task_parity() {
        replay("tasksBoard/hasMainTask", |case: &Case| -> Result<bool, String> {
            let tasks = tasks_arg(case.arg(1))?;
            Ok(has_main_task(case.arg_str(0)?, &tasks, opt_str(case, 2)?))
        });
    }

    #[test]
    fn task_sessions_parity() {
        replay("tasksBoard/taskSessions", |case: &Case| -> Result<Value, String> {
            let task = task_arg(case.arg(0))?;
            let sessions = sessions_arg(case.arg(1))?;
            let found = task_sessions(&task, &sessions);
            Ok(Value::Array(
                found
                    .into_iter()
                    .map(|item| given(case.arg(1), index_in(&sessions, item)))
                    .collect(),
            ))
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
    fn picker_view_of_parity() {
        replay("tasksBoard/pickerViewOf", |case: &Case| {
            opt_str(case, 0).map(picker_view_of)
        });
    }

    #[test]
    fn work_options_parity() {
        replay(
            "tasksBoard/workOptions",
            |case: &Case| -> Result<Vec<WorkOption>, String> {
                Ok(work_options(&sessions_arg(case.arg(0))?, opt_str(case, 1)?))
            },
        );
    }

    #[test]
    fn picker_rows_parity() {
        replay(
            "tasksBoard/pickerRows",
            |case: &Case| -> Result<Vec<WorkOption>, String> {
                let options: Vec<WorkOption> = case.arg_as(0)?;
                Ok(picker_rows(
                    &options,
                    case.arg_as(1)?,
                    case.arg_str(2)?,
                    opt_str(case, 3)?,
                ))
            },
        );
    }

    #[test]
    fn picker_counts_parity() {
        replay(
            "tasksBoard/pickerCounts",
            |case: &Case| -> Result<PickerCounts, String> {
                let options: Vec<WorkOption> = case.arg_as(0)?;
                Ok(picker_counts(&options, case.arg_str(1)?))
            },
        );
    }

    #[test]
    fn task_origin_parity() {
        replay("tasksBoard/taskOrigin", |case: &Case| -> Result<Value, String> {
            let task = task_arg(case.arg(0))?;
            let sessions = sessions_arg(case.arg(1))?;
            Ok(task_origin(&task, &sessions).map_or_else(tags::undefined, Value::String))
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
    fn note_blocks_parity() {
        replay("tasksBoard/noteBlocks", |case: &Case| {
            sessions_arg(case.arg(0)).map(|sessions| note_blocks(&sessions))
        });
    }

    #[test]
    fn block_items_parity() {
        replay(
            "tasksBoard/blockItems",
            |case: &Case| -> Result<Vec<NoteItem>, String> {
                let block: NoteBlock = case.arg_as(0)?;
                Ok(block_items(&block, &sessions_arg(case.arg(1))?))
            },
        );
    }

    #[test]
    fn note_items_parity() {
        replay("tasksBoard/noteItems", |case: &Case| {
            sessions_arg(case.arg(0)).map(|sessions| note_items(&sessions))
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
    fn note_line_parity() {
        replay("tasksBoard/noteLine", |case: &Case| -> Result<Value, String> {
            let links = links_fn(case.arg(1))?;
            Ok(note_line(case.arg_str(0)?, links).map_or_else(tags::undefined, |view| json!(view)))
        });
    }

    #[test]
    fn notes_view_rows_parity() {
        replay("tasksBoard/notesViewRows", |case: &Case| -> Result<Value, String> {
            let sessions = sessions_arg(case.arg(0))?;
            let object = case.arg(1).as_object().cloned().unwrap_or_default();
            let links = match object.get("links") {
                Some(value) => links_fn(value)?,
                None => None,
            };
            let options = NotesViewOptions {
                items_only: object.get("itemsOnly").and_then(Value::as_bool).unwrap_or(false),
                scope: object.get("scope").and_then(Value::as_str),
                links,
            };
            Ok(rows_value(&notes_view_rows(&sessions, &options)))
        });
    }

    #[test]
    fn note_group_label_parity() {
        replay("tasksBoard/noteGroupLabel", |case: &Case| -> Result<String, String> {
            Ok(note_group_label(case.arg_str(0)?, &sessions_arg(case.arg(1))?))
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

    // covers: B-P6-10 a work's steps: its note's checklist (open and ticked, not ↗ links or empty items), under the task it was started for
    #[test]
    fn work_steps_from_note() {
        let note = "# Plan\n- [x] Read the spec\n- [ ] Write the parser\n  - [ ] Handle tabs\n- ↗ Docs <!-- dh:t=d0c5 -->\n- [ ] \nprose";
        let step = |line: usize, text: &str, done: bool| WorkStep {
            line,
            text: text.to_owned(),
            done,
        };
        assert_eq!(
            note_steps(Some(note)),
            [
                step(1, "Read the spec", true),
                step(2, "Write the parser", false),
                step(3, "Handle tabs", false)
            ]
        );
        let sessions = vec![
            session_with(
                "main",
                json!({"notes": "- [ ] Ask about rate limits", "notesFile": {"path": "/n/main.md", "revision": "rm"}}),
            ),
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
        let shared = work_note("wt:w1", &sessions);
        let shared = shared.as_ref();
        assert_eq!(shared.map(|n| n.session_id.as_str()), Some("auth"));
        assert_eq!(shared.map(|n| n.section), Some(NoteSection::Shared));
        assert_eq!(shared.and_then(|n| n.note_id.as_deref()), Some("worktree:w1"));
        assert_eq!(shared.map(|n| n.text.as_str()), Some(note));
        assert_eq!(shared.map(|n| n.revision.as_str()), Some("r1"));
        assert_eq!(shared.and_then(|n| n.read_only), None);
        let own = work_note("s:main", &sessions);
        let own = own.as_ref();
        assert_eq!(own.map(|n| n.session_id.as_str()), Some("main"));
        assert_eq!(own.map(|n| n.section), Some(NoteSection::Session));
        assert_eq!(own.and_then(|n| n.note_id.as_deref()), None);
        assert_eq!(own.map(|n| n.text.as_str()), Some("- [ ] Ask about rate limits"));
        assert_eq!(own.map(|n| n.revision.as_str()), Some("rm"));
        assert_eq!(own.and_then(|n| n.read_only), None);
        assert_eq!(work_note("wt:w9", &sessions).and_then(|n| n.read_only), Some(true));

        let tasks = parse_tasks(
            &[
                "- [ ] Add OAuth <!-- dh:t=a wt=w1 -->",
                "- [ ] Elsewhere <!-- dh:t=e -->",
            ]
            .join("\n"),
        );
        let steps_of = |section: &str| note_steps(work_note(section, &sessions).as_ref().map(|n| n.text.as_str()));
        let rows = board_rows(&tasks, false, now(), None, Some(&steps_of));
        let describe = |row: &BoardRow| match row {
            BoardRow::Task { task, steps, .. } => format!(
                "{}{}",
                task.title,
                steps
                    .as_ref()
                    .map_or(String::new(), |s| format!(" {}/{}", s.done, s.total))
            ),
            BoardRow::Step { step, .. } => format!("  {} {}", if step.done { '☑' } else { '☐' }, step.text),
            BoardRow::Heading { .. } => "heading".to_owned(),
            _ => "other".to_owned(),
        };
        // Under the work's task (◆, its only one, so no heading row), which counts them.
        assert_eq!(
            rows.iter().take(5).map(describe).collect::<Vec<_>>(),
            [
                "Add OAuth 1/3",
                "  ☑ Read the spec",
                "  ☐ Write the parser",
                "  ☐ Handle tabs",
                "heading"
            ]
        );
        // No task on the work: no group, so no steps on the board (the Notes view still lists them).
        let only = parse_tasks("- [ ] Elsewhere <!-- dh:t=e -->");
        let all_steps = |_: &str| note_steps(Some(note));
        assert!(
            !board_rows(&only, false, now(), None, Some(&all_steps))
                .iter()
                .any(|row| matches!(row, BoardRow::Step { .. }))
        );
        // Space on a step ticks the note's line (and back).
        let line = |text: Option<String>, at: usize| text.and_then(|t| t.split('\n').nth(at).map(str::to_owned));
        assert_eq!(
            line(crate::notes::toggle_checklist_line(note, 2), 2).as_deref(),
            Some("- [x] Write the parser")
        );
        assert_eq!(
            line(crate::notes::toggle_checklist_line(note, 1), 1).as_deref(),
            Some("- [ ] Read the spec")
        );
        assert_eq!(
            line(crate::notes::toggle_checklist_line(note, 3), 3).as_deref(),
            Some("  - [x] Handle tabs")
        );
        assert_eq!(crate::notes::toggle_checklist_line(note, 0), None);
    }

    // covers: B-P6-11 sending a note's open items at once: each becomes its ↗ link; the backlog remembers the branch they were left in
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

    // covers: B-P6-12 ↗ lines say where their task is: in Tasks, done, or no longer in the list
    #[test]
    fn link_lines_say_where_their_task_is() {
        use crate::notes::{NoteRow, NoteRowKind, note_read_rows};
        let tasks = parse_tasks("- [ ] Open <!-- dh:t=aaaa -->\n- [x] Done <!-- dh:t=bbbb -->");
        let links = task_link_lookup(Some(&tasks));
        let text = |line: &str, links: Option<TaskLinkLookup<'_>>| note_line(line, links).map(|view| view.text);
        assert_eq!(
            text("- ↗ Open <!-- dh:t=aaaa -->", links.as_deref()).as_deref(),
            Some("↗ Open · in Tasks")
        );
        assert_eq!(
            text("- ↗ Done <!-- dh:t=bbbb -->", links.as_deref()).as_deref(),
            Some("↗ Done · done")
        );
        assert_eq!(
            text("- ↗ Gone <!-- dh:t=cccc -->", links.as_deref()).as_deref(),
            Some("↗ Gone · not in Tasks")
        );
        // Before the list is loaded nothing is claimed.
        assert!(task_link_lookup(None).is_none());
        assert_eq!(
            text("- ↗ Gone <!-- dh:t=cccc -->", None).as_deref(),
            Some("↗ Gone · in Tasks")
        );
        assert_eq!(
            note_read_rows("- ↗ Done <!-- dh:t=bbbb -->", 40, links.as_deref()),
            [NoteRow {
                text: "↗ Done · done".to_owned(),
                kind: Some(NoteRowKind::Link)
            }]
        );
    }

    // covers: B-P6-13 w onto work without a task makes the task that work's (◆); onto work with one, the daemon puts it into its note as a step
    #[test]
    fn assign_onto_work_with_or_without_task() -> Result<(), Box<dyn std::error::Error>> {
        use crate::tasks::{apply_task_op, client_task_op};
        use deckhand_protocol::data::{TaskLink, TaskOp};
        let text = "- [ ] Fix flaky test <!-- dh:t=f1 -->\n- [ ] Write docs <!-- dh:t=d1 -->";
        let main = apply_task_op(
            text,
            &TaskOp::Assign {
                id: "f1".to_owned(),
                link: Some(TaskLink::Session { s: "flaky".to_owned() }),
            },
            now(),
        )?;
        assert!(
            main.text.starts_with("- [ ] Fix flaky test <!-- dh:t=f1 s=flaky -->"),
            "{}",
            main.text
        );
        // The menu asks: does that work have an open task other than the one moving? Then it becomes a step instead.
        let parsed = parse_tasks(&main.text);
        assert!(has_main_task("s:flaky", &parsed, None));
        assert!(!has_main_task("s:flaky", &parsed, Some("f1")));
        assert!(!has_main_task("s:other", &parsed, None));
        // From a client, assign carries only a link (no roles any more).
        assert_eq!(
            client_task_op(&json!({"type": "assign", "id": "x", "link": {"wt": "w1"}, "main": true}))?,
            TaskOp::Assign {
                id: "x".to_owned(),
                link: Some(TaskLink::Worktree { wt: "w1".to_owned() })
            }
        );
        Ok(())
    }
}
