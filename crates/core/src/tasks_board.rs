//! Port of tasksBoard.ts.
//!
//! The Tasks board (b) as pure data: its rows (one group per worktree, or main-checkout session, with open tasks;
//! the backlog; done; the folded older done ones; and the open note items not yet tasks), each task's linked sessions
//! and the state shown for it. A group is keyed by its work (`wt:<record id>` / `s:<session id>`, see
//! `tasks::link_key`). The tasks flow keeps the selection and keys.
//!
//! Note rows share their [`NoteBlock`] (`Rc`): a note's every line row points at it rather than copying its text.

use std::rc::Rc;

use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::display::Color;
use crate::notes::{TaskLinkLookup, TaskLinkState};
use crate::session_order::SessionLike;
use deckhand_protocol::data::{NoteSection, SessionRecord, Task};

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
    let _ = row;
    todo!("P6")
}

/// TS: tasksBoard.ts rowKey.
pub fn row_key(row: &BoardRow) -> Option<String> {
    let _ = row;
    todo!("P6")
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
    let _ = (section, sessions);
    todo!("P6")
}

/// TS: tasksBoard.ts stepCount.
pub fn step_count(steps: &[WorkStep]) -> StepCount {
    let _ = steps;
    todo!("P6")
}

/// TS: tasksBoard.ts noteSteps (checklist items with text, not `↗` links).
pub fn note_steps(text: Option<&str>) -> Vec<WorkStep> {
    let _ = text;
    todo!("P6")
}

/// TS: tasksBoard.ts boardRows `stepsOf`: a work's steps (its note's checklist).
pub type StepsOf<'a> = &'a dyn Fn(&str) -> Vec<WorkStep>;

/// TS: tasksBoard.ts boardRows (`scope`: v's work; `steps_of`: a work's steps).
pub fn board_rows(
    tasks: &[Task],
    show_older: bool,
    now_ms: i64,
    scope: Option<&str>,
    steps_of: Option<StepsOf<'_>>,
) -> Vec<BoardRow> {
    let _ = (tasks, show_older, now_ms, scope, steps_of);
    todo!("P6")
}

/// TS: tasksBoard.ts hasMainTask.
pub fn has_main_task(section: &str, tasks: &[Task], except: Option<&str>) -> bool {
    let _ = (section, tasks, except);
    todo!("P6")
}

/// TS: tasksBoard.ts taskSessions.
pub fn task_sessions<'a, S: SessionLike>(task: &Task, sessions: &'a [S]) -> Vec<&'a S> {
    let _ = (task, sessions);
    todo!("P6")
}

/// TS: tasksBoard.ts workSessions.
pub fn work_sessions<'a, S: SessionLike>(section: &str, sessions: &'a [S]) -> Vec<&'a S> {
    let _ = (section, sessions);
    todo!("P6")
}

/// TS: tasksBoard.ts workKeyOf.
pub fn work_key_of(session: &SessionRecord) -> String {
    let _ = session;
    todo!("P6")
}

/// TS: tasksBoard.ts workLabel (`⎇ <branch>`, or `main checkout · <session>`).
pub fn work_label<S: SessionLike>(section: &str, sessions: &[S]) -> String {
    let _ = (section, sessions);
    todo!("P6")
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
    let _ = section;
    todo!("P6")
}

/// TS: tasksBoard.ts workOptions.
pub fn work_options<S: SessionLike>(sessions: &[S], current: Option<&str>) -> Vec<WorkOption> {
    let _ = (sessions, current);
    todo!("P6")
}

/// TS: tasksBoard.ts pickerRows.
pub fn picker_rows(options: &[WorkOption], view: PickerView, query: &str, origin: Option<&str>) -> Vec<WorkOption> {
    let _ = (options, view, query, origin);
    todo!("P6")
}

/// TS: tasksBoard.ts pickerCounts result (`Record<PickerView, number>`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickerCounts {
    pub worktrees: usize,
    pub sessions: usize,
}

/// TS: tasksBoard.ts pickerCounts.
pub fn picker_counts(options: &[WorkOption], query: &str) -> PickerCounts {
    let _ = (options, query);
    todo!("P6")
}

/// TS: tasksBoard.ts taskOrigin.
pub fn task_origin<S: SessionLike>(task: &Task, sessions: &[S]) -> Option<String> {
    let _ = (task, sessions);
    todo!("P6")
}

/// TS: tasksBoard.ts urgency (needs you 3, working 2, idle 1, exited 0).
pub fn urgency(session: &SessionRecord) -> u8 {
    let _ = session;
    todo!("P6")
}

/// TS: tasksBoard.ts leadSession (the most urgent; the first on ties).
pub fn lead_session<S: SessionLike>(sessions: &[S]) -> Option<&S> {
    let _ = sessions;
    todo!("P6")
}

/// TS: tasksBoard.ts linkedTasks.
pub fn linked_tasks<'a>(tasks: &'a [Task], session: Option<&SessionRecord>) -> Vec<&'a Task> {
    let _ = (tasks, session);
    todo!("P6")
}

/// TS: tasksBoard.ts linkedTask.
pub fn linked_task<'a>(tasks: &'a [Task], session: Option<&SessionRecord>) -> Option<&'a Task> {
    let _ = (tasks, session);
    todo!("P6")
}

/// TS: tasksBoard.ts otherOpenTasks.
pub fn other_open_tasks(tasks: &[Task], session: Option<&SessionRecord>) -> usize {
    let _ = (tasks, session);
    todo!("P6")
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
    let _ = (task, sessions, spinner_frame, all_sessions);
    todo!("P6")
}

/// TS: tasksBoard.ts workState (`all_sessions` defaults to `sessions`).
pub fn work_state<S: SessionLike>(
    section: &str,
    sessions: &[S],
    spinner_frame: &str,
    all_sessions: Option<&[S]>,
) -> TaskState {
    let _ = (section, sessions, spinner_frame, all_sessions);
    todo!("P6")
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

/// TS: tasksBoard.ts noteBlocks.
pub fn note_blocks<S: SessionLike>(sessions: &[S]) -> Vec<NoteBlock> {
    let _ = sessions;
    todo!("P6")
}

/// TS: tasksBoard.ts blockItems.
pub fn block_items<S: SessionLike>(block: &NoteBlock, sessions: &[S]) -> Vec<NoteItem> {
    let _ = (block, sessions);
    todo!("P6")
}

/// TS: tasksBoard.ts noteItems.
pub fn note_items<S: SessionLike>(sessions: &[S]) -> Vec<NoteItem> {
    let _ = sessions;
    todo!("P6")
}

/// TS: tasksBoard.ts taskLinkLookup result: a TaskLinkLookup owning its snapshot of the list.
pub type OwnedTaskLinkLookup = Box<dyn Fn(&str) -> TaskLinkState>;

/// TS: tasksBoard.ts taskLinkLookup (`None` while the list is not loaded).
pub fn task_link_lookup(tasks: Option<&[Task]>) -> Option<OwnedTaskLinkLookup> {
    let _ = tasks;
    todo!("P6")
}

/// TS: tasksBoard.ts noteLine result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteLineView {
    pub text: String,
    pub style: NoteLineStyle,
}

/// TS: tasksBoard.ts noteLine (`None` for a blank line).
pub fn note_line(line: &str, links: Option<TaskLinkLookup<'_>>) -> Option<NoteLineView> {
    let _ = (line, links);
    todo!("P6")
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

/// TS: tasksBoard.ts notesViewRows.
pub fn notes_view_rows<S: SessionLike>(sessions: &[S], options: &NotesViewOptions<'_>) -> Vec<BoardRow> {
    let _ = (sessions, options);
    todo!("P6")
}

/// TS: tasksBoard.ts noteGroupLabel.
pub fn note_group_label<S: SessionLike>(group: &str, sessions: &[S]) -> String {
    let _ = (group, sessions);
    todo!("P6")
}

/// TS: tasksBoard.ts taskCountLabel (`☐ 3 tasks · b`, empty without open tasks).
pub fn task_count_label(tasks: &[Task]) -> String {
    let _ = tasks;
    todo!("P6")
}

/// TS: tasksBoard.ts worktreeOpenItems.
pub fn worktree_open_items<S: SessionLike>(record_id: Option<&str>, sessions: &[S]) -> Vec<String> {
    let _ = (record_id, sessions);
    todo!("P6")
}

/// TS: tasksBoard.ts openItemsRemovedWith.
pub fn open_items_removed_with<S: SessionLike>(session: &SessionRecord, sessions: &[S]) -> Vec<String> {
    let _ = (session, sessions);
    todo!("P6")
}
