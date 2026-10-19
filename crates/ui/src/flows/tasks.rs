//! Port of tasksFlow.tsx: the Tasks board (b). The repository's task list in the right pane, grouped by the
//! worktree (or main-checkout session) each open task is on, then the backlog; v narrows it to the work of the
//! session it was opened from. Also the open note items not yet tasks (tab), a small editor for adding and editing a
//! task (title, then body), and the w picker that moves a task to a worktree or a main-checkout session. The daemon
//! owns the file; every change is an op applied to the file as it is.

use std::collections::HashSet;
use std::rc::Rc;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_client::live::LiveClient;
use deckhand_core::key::Input;
use deckhand_core::tasks_board::{BoardRow, NoteItem, PickerView, StepCount};
use deckhand_core::text_editor::{EditOptions, EditorState};
use deckhand_protocol::data::{SessionRecord, Task, TaskOp, TasksDoc};

use crate::app::state::App;
use crate::flow::{Flow, FlowCx};
use crate::keymap::Keymap;
use crate::primitives::Row;

/// TS: useTasksFlow apply's `then`: runs with the new task list.
pub type ApplyThen = Box<dyn FnOnce(&mut TasksFlow, &TasksDoc, &mut FlowCx<App, TasksFlow>)>;

/// TS: tasksFlow.tsx DETAIL_ROWS.
pub const DETAIL_ROWS: usize = 5;

/// TS: tasksFlow.tsx EDITOR_BODY_ROWS.
pub const EDITOR_BODY_ROWS: usize = 4;

/// TS: tasksFlow.tsx PICKER_ROWS.
pub const PICKER_ROWS: usize = 8;

/// TS: tasksFlow.tsx capitalize.
pub fn capitalize(text: &str) -> String {
    let _ = text;
    todo!("F5")
}

/// TS: tasksFlow.tsx TaskPicker: the w menu (the task, where it is now, the note it came from, the view, the search
/// and the selected row).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskPicker {
    pub task_id: String,
    pub title: String,
    pub current: Option<String>,
    pub origin: Option<String>,
    pub view: PickerView,
    pub query: String,
    /// Typing the search (after /) until Enter keeps it or Esc clears it.
    pub typing: bool,
    pub index: usize,
}

/// TS: tasksFlow.tsx TaskEditor `kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TaskEditorKind {
    Add,
    Edit,
    /// A new checklist item of `section`'s note (that work has its task already), title only.
    Step,
}

/// TS: tasksFlow.tsx TaskEditor `field`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TaskEditorField {
    Title,
    Body,
}

/// TS: tasksFlow.tsx TaskEditor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskEditor {
    pub kind: TaskEditorKind,
    pub id: Option<String>,
    /// A new task's work key, or the work whose note a step goes into; none: the backlog.
    pub section: Option<String>,
    pub field: TaskEditorField,
    pub title: EditorState,
    pub body: EditorState,
}

/// TS: tasksFlow.tsx TITLE_OPTIONS.
pub fn title_options() -> EditOptions {
    todo!("F5")
}

/// TS: tasksFlow.tsx BODY_OPTIONS.
pub fn body_options() -> EditOptions {
    todo!("F5")
}

/// TS: tasksFlow.tsx parseAdded: the ID of the task `next` has that `before` did not (the one just added).
pub fn parse_added(next: &TasksDoc, before: &HashSet<String>) -> Option<String> {
    let _ = (next, before);
    todo!("F5")
}

/// TS: tasksFlow.tsx neighborInGroup: the task row next to `index` in `direction` within the same group.
pub fn neighbor_in_group<'a>(rows: &'a [BoardRow], index: usize, direction: isize, section: &str) -> Option<&'a Task> {
    let _ = (rows, index, direction, section);
    todo!("F5")
}

/// TS: tasksFlow.tsx BoardLine: one board row.
pub fn board_line(
    row: &BoardRow,
    selected: bool,
    width: usize,
    sessions: &[&SessionRecord],
    spinner_frame: &str,
) -> Row {
    let _ = (row, selected, width, sessions, spinner_frame);
    todo!("F5")
}

/// TS: tasksFlow.tsx detailLines: the selected row's details under the list.
pub fn detail_lines(
    row: Option<&BoardRow>,
    sessions: &[&SessionRecord],
    spinner_frame: &str,
    width: usize,
) -> Vec<Row> {
    let _ = (row, sessions, spinner_frame, width);
    todo!("F5")
}

/// TS: tasksFlow.tsx TaskBanner props: the Notes tab's first line for a session doing a task.
#[derive(Clone, Copy, Debug)]
pub struct TaskBannerProps<'a> {
    pub task: &'a Task,
    /// More open tasks on its work.
    pub more: usize,
    /// The work note's checklist, done of total (the started task's steps).
    pub steps: Option<StepCount>,
    pub sessions: &'a [&'a SessionRecord],
    pub spinner_frame: &'a str,
}

/// TS: tasksFlow.tsx TaskBanner's rows (the task line and a rule).
pub fn task_banner_rows(props: &TaskBannerProps<'_>, width: usize) -> Vec<Row> {
    let _ = (props, width);
    todo!("F5")
}

/// TS: tasksFlow.tsx TaskBanner.
pub fn render_task_banner(props: &TaskBannerProps<'_>, buf: &mut Buffer, area: Rect) {
    let _ = (props, buf, area);
    todo!("F5")
}

/// TS: tasksFlow.tsx TasksFlowOptions (values; the callbacks are effects: `onExit`, `onStart`, `onGoTo`,
/// `onOpenNote`, `onDoc`, `onSession`, `setError`, `setStatusMessage`).
#[derive(Clone, Copy, Debug)]
pub struct TasksFlowProps<'a> {
    pub client: Option<&'a Rc<LiveClient>>,
    pub repo_root: &'a str,
    pub doc: Option<&'a TasksDoc>,
    pub tasks: &'a [Task],
    /// Every session of the repository.
    pub sessions: &'a [&'a SessionRecord],
    pub spinner_frame: &'a str,
}

/// TS: tasksFlow.tsx the board's two views (tab).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TasksView {
    #[default]
    Tasks,
    Notes,
}

/// TS: tasksFlow.tsx `selected`: the selected row's key, else its position among the selectable rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowSelection {
    pub key: Option<String>,
    pub index: usize,
}

/// TS: tasksFlow.tsx useTasksFlow's state.
#[derive(Debug)]
pub struct TasksFlow {
    pub view: TasksView,
    pub selected: RowSelection,
    pub show_older: bool,
    pub editor: Option<TaskEditor>,
    /// The task whose deletion waits for a second x.
    pub pending_delete: Option<String>,
    /// The work the board was opened from (v shows it alone).
    pub scope: Option<String>,
    pub scoped: bool,
    pub picker: Option<TaskPicker>,
    /// The Notes view: only open checklist items (f).
    pub items_only: bool,
    /// The list's first shown row (TS `top` ref).
    pub top: usize,
    /// An op in flight (TS `busy` ref): further ops are ignored.
    pub busy: bool,
    /// The editor body's wrap width at the last render (TS `bodyWidth`, 40 before one).
    pub body_width: usize,
}

impl Default for TasksFlow {
    fn default() -> Self {
        TasksFlow {
            view: TasksView::Tasks,
            selected: RowSelection::default(),
            show_older: false,
            editor: None,
            pending_delete: None,
            scope: None,
            scoped: false,
            picker: None,
            items_only: false,
            top: 0,
            busy: false,
            body_width: 40,
        }
    }
}

/// The Tasks board's keys (keysDocumented: each must be in the guide's Tasks topic).
pub static KEYMAP: Keymap = &[];

impl TasksFlow {
    /// TS: TasksFlow.open (b): the board's task view, the selection on the first open task of `scope`.
    pub fn open(&mut self, scope: Option<&str>, props: &TasksFlowProps<'_>) {
        let _ = (scope, props);
        todo!("F5")
    }

    /// The board's rows as shown now (TS `rows`: the notes view or the task view).
    pub fn rows(&self, props: &TasksFlowProps<'_>) -> Vec<BoardRow> {
        let _ = props;
        todo!("F5")
    }

    /// The selected row's index in `rows` (TS `current`, -1: none).
    pub fn current(&self, rows: &[BoardRow]) -> Option<usize> {
        let _ = rows;
        todo!("F5")
    }

    /// TS: useTasksFlow run/apply: one op on the task list at a time; the new doc goes to the app, then `then`.
    pub fn apply(
        &mut self,
        op: TaskOp,
        props: &TasksFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
        then: Option<ApplyThen>,
    ) {
        let _ = (op, props, cx, then);
        todo!("F5")
    }

    /// TS: useTasksFlow save: the editor's task (added, edited, or a step).
    pub fn save(&mut self, state: TaskEditor, props: &TasksFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let _ = (state, props, cx);
        todo!("F5")
    }

    /// TS: useTasksFlow editorInput: keys while the editor is open.
    pub fn editor_input(
        &mut self,
        input: &Input,
        state: TaskEditor,
        props: &TasksFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
    ) {
        let _ = (input, state, props, cx);
        todo!("F5")
    }

    /// TS: useTasksFlow pickerInput: keys while the w picker is open.
    pub fn picker_input(
        &mut self,
        input: &Input,
        state: TaskPicker,
        props: &TasksFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
    ) {
        let _ = (input, state, props, cx);
        todo!("F5")
    }

    /// TS: useTasksFlow promote: a note item becomes a task.
    pub fn promote(&mut self, item: &NoteItem, props: &TasksFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let _ = (item, props, cx);
        todo!("F5")
    }
}

impl Flow<App> for TasksFlow {
    type Props<'a> = TasksFlowProps<'a>;

    fn handle_input(&mut self, props: &TasksFlowProps<'_>, input: &Input, cx: &mut FlowCx<App, Self>) {
        let _ = (props, input, cx);
        todo!("F5")
    }

    fn render(&mut self, props: &TasksFlowProps<'_>, buf: &mut Buffer, area: Rect) {
        let _ = (props, buf, area);
        todo!("F5")
    }

    /// TS: TasksFlow.hint: the board's footer.
    fn hint(&self, props: &TasksFlowProps<'_>, width: usize) -> String {
        let _ = (props, width);
        todo!("F5")
    }
}
