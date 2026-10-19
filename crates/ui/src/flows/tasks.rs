//! Port of tasksFlow.tsx: the Tasks board (b). The repository's task list in the right pane, grouped by the
//! worktree (or main-checkout session) each open task is on, then the backlog; v narrows it to the work of the
//! session it was opened from. Also the open note items not yet tasks (tab), a small editor for adding and editing a
//! task (title, then body), and the w picker that moves a task to a worktree or a main-checkout session (two views,
//! Tab switches; / searches, as in the sidebar), back to the backlog, or back to the note it was sent from. The
//! daemon owns the file; every change is an op (deckhand_core::tasks) applied to the file as it is, so an editor's
//! edits are never overwritten.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::LazyLock;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use regex::Regex;

use deckhand_client::live::{LiveClient, Reply};
use deckhand_core::display::{
    HINT_SEPARATOR, HintPart, error_message, fit_hint, scrolled_list_top, strip_terminal_controls, truncate,
};
use deckhand_core::jsstr::{date, js_len, js_pad_end, js_slice, js_to_upper, js_trim, path};
use deckhand_core::key::{Input, Key};
use deckhand_core::session_order::SessionLike;
use deckhand_core::sidebar_model::attention_reason_lines;
use deckhand_core::tasks::{MAX_TASK_BODY, MAX_TASK_TITLE, append_note_item, link_key, link_of_key};
use deckhand_core::tasks_board::{
    BoardRow, NoteItem, NoteLineStyle, NotesViewOptions, PickerView, StepCount, TaskGroup, WorkOption, WorkOptionKind,
    WorkStep, block_items, board_rows, has_main_task, lead_session, note_group_label, note_steps, notes_view_rows,
    picker_counts, picker_rows, picker_view_of, row_key, selectable_row, task_link_lookup, task_origin, task_sessions,
    task_state, work_label, work_note, work_options, work_state,
};
use deckhand_core::text_editor::{EditOptions, EditorLine, EditorState, edit_text, wrap_rows, wrapped_editor_lines};
use deckhand_protocol::data::{
    NoteSaveResult, NoteSection, PromoteNoteItemResult, SendOpenItemsResult, SessionRecord, Task, TaskOp, TasksDoc,
};

use crate::app::state::App;
use crate::flow::{Flow, FlowCx, FlowEffect};
use crate::keymap::{Binding, HelpRef, KeySpec, Keymap, NamedKey};
use crate::primitives::{
    Border, Column, Row, Seg, TextStyle, Wrap, bordered, draw_rows, fit_row, space_between, truncate_row, with_base,
};
use crate::theme::{THEME, color};

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
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    // JS upper-cases the first UTF-16 unit: half of a surrogate pair stays as it is.
    if first.len_utf16() > 1 {
        return text.to_owned();
    }
    format!("{}{}", js_to_upper(first.encode_utf8(&mut [0; 4])), chars.as_str())
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
    /// A new task's work key (it becomes that work's task, ◆), or the work whose note a step goes into; none: the
    /// backlog.
    pub section: Option<String>,
    pub field: TaskEditorField,
    pub title: EditorState,
    pub body: EditorState,
}

/// TS: tasksFlow.tsx TITLE_OPTIONS.
pub fn title_options() -> EditOptions {
    EditOptions {
        select_all: Some(false),
        max_chars: Some(MAX_TASK_TITLE),
        limit_message: Some(format!("A title is at most {MAX_TASK_TITLE} characters")),
        tab: Some(false),
        wrap_width: None,
        page_rows: None,
    }
}

/// TS: tasksFlow.tsx BODY_OPTIONS.
pub fn body_options() -> EditOptions {
    EditOptions {
        select_all: Some(false),
        max_chars: Some(MAX_TASK_BODY),
        limit_message: Some(format!("A task's details are at most {MAX_TASK_BODY} characters")),
        tab: Some(false),
        wrap_width: None,
        page_rows: None,
    }
}

/// parseAdded's `/<!--[ \t]*dh:t=([0-9a-f]+)/g`.
static TASK_ID_MARK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<!--[ \t]*dh:t=([0-9a-f]+)").expect("TASK_ID_MARK is a valid regex"));

/// TS: tasksFlow.tsx parseAdded: the ID of the task `next` has that `before` did not (the one just added).
pub fn parse_added(next: &TasksDoc, before: &HashSet<String>) -> Option<String> {
    TASK_ID_MARK
        .captures_iter(&next.text)
        .filter_map(|captures| captures.get(1))
        .map(|id| id.as_str())
        .find(|id| !before.contains(*id))
        .map(str::to_owned)
}

/// TS: tasksFlow.tsx neighborInGroup: the task row next to `index` in `direction` within the same group (a
/// worktree's, the backlog, done), if any.
pub fn neighbor_in_group<'a>(rows: &'a [BoardRow], index: usize, direction: isize, section: &str) -> Option<&'a Task> {
    let row = rows.get(index.checked_add_signed(direction)?)?;
    match row {
        BoardRow::Task { task, section: own, .. } if own == section => Some(task),
        _ => None,
    }
}

/// An Ink color name as a ratatui color.
fn ink(name: &str) -> Option<Color> {
    color(name)
}

/// A `<Text wrap="truncate-end">` of `segs` under `base`, cut to `width` columns.
fn line(segs: Row, base: TextStyle, width: usize) -> Row {
    truncate_row(&with_base(segs, base), width, Wrap::TruncateEnd)
}

/// A JS width difference, clamped at zero (`Math.max(0, a - b)`).
fn minus(a: usize, b: usize) -> usize {
    a.saturating_sub(b)
}

/// `Math.max(low, Math.min(value, high))` on JS numbers, as an index.
fn to_index(value: i64) -> usize {
    usize::try_from(value.max(0)).unwrap_or(0)
}

fn signed(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The `selected ? '›' : ' '` marker of a board row.
fn marker(selected: bool) -> &'static str {
    if selected { "›" } else { " " }
}

/// TS: tasksFlow.tsx BoardLine: one board row.
pub fn board_line(
    row: &BoardRow,
    selected: bool,
    width: usize,
    sessions: &[&SessionRecord],
    spinner_frame: &str,
) -> Row {
    let mark = marker(selected);
    // A nested Text's color when not selected (selected rows take the bar's).
    let unless_selected = |name: Option<Color>| TextStyle::default().color(if selected { None } else { name });
    match row {
        BoardRow::Heading { text, count } => {
            let mut segs = vec![Seg::new(text.clone(), TextStyle::fg(THEME.muted).bold(true))];
            if let Some(count) = count.as_deref().filter(|count| !count.is_empty()) {
                segs.push(Seg::new(format!(" · {count}"), TextStyle::fg(THEME.muted)));
            }
            line(segs, TextStyle::default(), width)
        }
        BoardRow::Empty { text, .. } => {
            let base = TextStyle::default()
                .inverse(selected)
                .color(if selected { THEME.active } else { THEME.muted });
            line(
                vec![Seg::plain(js_pad_end(&format!("{mark} {text}"), width, " "))],
                base,
                width,
            )
        }
        BoardRow::Work { section, count, label } => {
            // The work's name and open count, its most urgent session's state on the right.
            let work = (section != "main").then(|| work_state(section, sessions, spinner_frame, None));
            let state = work
                .as_ref()
                .map_or(String::new(), |work| format!("{} {}", work.glyph, work.text));
            let name = label.clone().unwrap_or_else(|| work_label(section, sessions));
            let label = truncate(&name, minus(width, js_len(&state) + 8).max(4));
            let mut left = vec![Seg::new(label, TextStyle::fg(THEME.accent_soft).bold(true))];
            if *count > 0 {
                left.push(Seg::new(format!(" · {count}"), TextStyle::fg(THEME.muted)));
            }
            let right = match &work {
                Some(work) => vec![Seg::new(state.clone(), TextStyle::default().color(ink(work.color)))],
                None => Vec::new(),
            };
            space_between(&left, &right, width)
        }
        BoardRow::Older { count, shown } => {
            let text = format!(
                "{mark} {} {count} older done · enter {} them",
                if *shown { '▾' } else { '▸' },
                if *shown { "hides" } else { "shows" }
            );
            let base = TextStyle::default()
                .inverse(selected)
                .color(if selected { THEME.active } else { THEME.muted });
            line(vec![Seg::plain(js_pad_end(&text, width, " "))], base, width)
        }
        // The Notes view: a note's name (and its open items), then its lines, indented under it.
        BoardRow::NoteHead { block, open } => {
            let right = if *open > 0 {
                format!("{open} open")
            } else {
                String::new()
            };
            let right_len = js_len(&right);
            let head = format!(
                "{mark} {}",
                truncate(&capitalize(&block.label), minus(width, right_len + 4).max(1))
            );
            let base = TextStyle::default().inverse(selected).bold(true).color(if selected {
                THEME.active
            } else {
                THEME.accent_soft
            });
            let segs = vec![
                Seg::plain(js_pad_end(&head, minus(width, right_len), " ")),
                Seg::new(right, unless_selected(Some(THEME.muted)).bold(false)),
            ];
            line(segs, base, width)
        }
        BoardRow::NoteLine { text, style, .. } => {
            let color = if selected {
                Some(THEME.active)
            } else {
                match style {
                    NoteLineStyle::Done => Some(THEME.muted),
                    NoteLineStyle::Link => Some(THEME.accent_soft),
                    _ => None,
                }
            };
            let base = TextStyle::default()
                .inverse(selected)
                .bold(selected || *style == NoteLineStyle::Heading)
                .color(color);
            let text = format!("{mark}   {}", truncate(text, minus(width, 4).max(1)));
            line(vec![Seg::plain(js_pad_end(&text, width, " "))], base, width)
        }
        // A step: an item of the work's note, under the task it was started for.
        BoardRow::Step { step, .. } => {
            let color = if selected {
                Some(THEME.active)
            } else if step.done {
                Some(THEME.muted)
            } else {
                None
            };
            let base = TextStyle::default().inverse(selected).bold(selected).color(color);
            let text = format!(
                "{mark}   {} {}",
                if step.done { '☑' } else { '☐' },
                truncate(&step.text, minus(width, 6).max(1))
            );
            line(vec![Seg::plain(js_pad_end(&text, width, " "))], base, width)
        }
        BoardRow::Task {
            task,
            group,
            section,
            steps,
            heads_work,
        } => {
            if *heads_work == Some(true) {
                // A work's only task heads its group: ◆ title, then the work's name, its most urgent session's state
                // and the steps.
                let work = work_state(section, sessions, spinner_frame, None);
                let state = format!("{} {}", work.glyph, work.text);
                let steps = steps.map_or(String::new(), |steps| format!("  {}/{}", steps.done, steps.total));
                let label = truncate(
                    &work_label(section, sessions),
                    minus(width / 2, js_len(&state) + js_len(&steps) + 2).max(4),
                );
                let right_len = js_len(&format!("{label}  {state}{steps}"));
                let title = truncate(&task.title, minus(width, right_len + 6).max(1));
                let base = TextStyle::default()
                    .inverse(selected)
                    .bold(selected)
                    .color(if selected { Some(THEME.active) } else { None });
                let segs = vec![
                    Seg::plain(format!("{mark} ")),
                    Seg::new("◆", unless_selected(Some(THEME.accent_soft))),
                    Seg::plain(js_pad_end(&format!(" {title}"), minus(width, right_len + 3), " ")),
                    Seg::new(label, unless_selected(Some(THEME.accent_soft))),
                    Seg::plain("  "),
                    Seg::new(state, unless_selected(ink(work.color))),
                    Seg::new(steps, unless_selected(Some(THEME.muted))),
                ];
                return line(segs, base, width);
            }
            // Under a work's heading (several tasks) the task is ◆, counting its steps (the work note's checklist).
            let in_work = *group == TaskGroup::Progress;
            let state = task_state(task, sessions, spinner_frame, None);
            let (glyph, glyph_color) = if in_work {
                ("◆".to_owned(), Some(THEME.accent_soft))
            } else {
                (state.glyph.clone(), ink(state.color))
            };
            let right = if in_work {
                steps.map_or(String::new(), |steps| format!("{}/{}", steps.done, steps.total))
            } else {
                let parts: Vec<&str> = [state.where_.as_deref().unwrap_or(""), state.text.as_str()]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect();
                truncate(&parts.join("  "), width / 2)
            };
            let right_len = js_len(&right);
            let title = truncate(&task.title, minus(width, right_len + 6).max(1));
            let color = if selected {
                Some(THEME.active)
            } else if task.done {
                Some(THEME.muted)
            } else {
                None
            };
            let base = TextStyle::default().inverse(selected).bold(selected).color(color);
            let segs = vec![
                Seg::plain(format!("{mark} ")),
                Seg::new(glyph, unless_selected(glyph_color)),
                Seg::plain(js_pad_end(&format!(" {title}"), minus(width, right_len + 3), " ")),
                Seg::new(right, unless_selected(Some(THEME.muted))),
            ];
            line(segs, base, width)
        }
    }
}

/// `text` word-wrapped as the details show it (wrapRows, `width` code points), one string per row.
fn wrapped(text: &str, width: usize) -> Vec<String> {
    wrap_rows(text, width, false)
        .iter()
        .map(|range| text.get(range.start..range.end).unwrap_or("").to_owned())
        .collect()
}

/// TS: tasksFlow.tsx detailLines: the selected row's details under the list (its title, where its work is, and its
/// details text).
pub fn detail_lines(
    row: Option<&BoardRow>,
    sessions: &[&SessionRecord],
    spinner_frame: &str,
    width: usize,
) -> Vec<Row> {
    let bold = TextStyle::default().bold(true);
    let muted = TextStyle::fg(THEME.muted);
    let text_line = |text: String, style: TextStyle| line(vec![Seg::plain(text)], style, width);
    match row {
        Some(BoardRow::NoteHead { block, .. } | BoardRow::NoteLine { block, .. }) => {
            let place = if block.section == NoteSection::Shared {
                format!("The {}", block.label)
            } else {
                format!("The notes of {}", block.label)
            };
            let place = format!("{place} · {}", note_group_label(&block.group, sessions));
            match row {
                Some(BoardRow::NoteLine { text, item, .. }) => {
                    let trimmed = js_trim(text);
                    let mut lines: Vec<Row> = wrapped(trimmed, minus(width, 2).max(1))
                        .into_iter()
                        .take(DETAIL_ROWS - 1)
                        .map(|text| text_line(text, bold))
                        .collect();
                    let promote = if item.is_some() {
                        "a adds it as a task (on that work), the note keeps a ↗ link · "
                    } else {
                        ""
                    };
                    lines.push(text_line(format!("{place} · {promote}enter opens the note"), muted));
                    lines
                }
                Some(BoardRow::NoteHead { open, .. }) => {
                    let count = if *open > 0 {
                        format!("{open} open item{} · ", if *open == 1 { "" } else { "s" })
                    } else {
                        String::new()
                    };
                    vec![
                        text_line(place, bold),
                        text_line(format!("{count}enter opens it in its session’s Notes tab"), muted),
                    ]
                }
                _ => Vec::new(),
            }
        }
        Some(BoardRow::Step { section, step, .. }) => {
            let work = work_label(section, sessions);
            vec![
                text_line(step.text.clone(), bold),
                text_line(
                    format!(
                        "A step: a checklist item in the note of {work} · space {} it · enter opens the note to edit it",
                        if step.done { "reopens" } else { "ticks" }
                    ),
                    muted,
                ),
            ]
        }
        Some(BoardRow::Task { task, group, .. }) => {
            let state = task_state(task, sessions, spinner_frame, None);
            let place = match group {
                TaskGroup::Progress => format!(
                    "The task of {}: merging or marking it done ticks it · o opens it",
                    work_label(&link_key(&task.meta).unwrap_or_default(), sessions)
                ),
                TaskGroup::Backlog => format!(
                    "{}n starts a session for it",
                    if state.text.is_empty() {
                        String::new()
                    } else {
                        format!("{} · ", state.text)
                    }
                ),
                TaskGroup::Done => format!(
                    "{}{} · space reopens it",
                    state.text,
                    state
                        .where_
                        .as_deref()
                        .filter(|place| !place.is_empty())
                        .map_or(String::new(), |place| format!(" {place}"))
                ),
            };
            // Sent from a note that still links it: w can put it back there.
            let back = task_origin(task, sessions)
                .map_or(String::new(), |origin| format!(" · w: back to the note of {origin}"));
            let body = if task.body.is_empty() {
                Vec::new()
            } else {
                wrapped(&task.body, minus(width, 2).max(1))
            };
            // What its work's most urgent session asks or said (agent signals), before the body.
            let lead = if *group == TaskGroup::Progress {
                lead_session(&task_sessions(task, sessions)).map(|lead| lead.session().clone())
            } else {
                None
            };
            let reason = lead.and_then(|lead| {
                attention_reason_lines(&lead, width, 1)
                    .into_iter()
                    .next()
                    .and_then(|parts| parts.into_iter().next())
            });
            let room = minus(DETAIL_ROWS - 2, usize::from(reason.is_some()));
            let mut lines = vec![
                text_line(task.title.clone(), bold),
                line(vec![Seg::plain(place), Seg::plain(back)], muted, width),
            ];
            if let Some(reason) = reason {
                let style = TextStyle::default().color(reason.color.and_then(ink));
                lines.push(text_line(reason.text, style));
            }
            for (index, text) in body.iter().take(room).enumerate() {
                let text = if index + 1 == room && body.len() > room {
                    format!("{} …", js_slice(text, 0, Some(signed(minus(width, 6)))))
                } else {
                    text.clone()
                };
                lines.push(text_line(format!("  {text}"), TextStyle::default()));
            }
            lines
        }
        _ => Vec::new(),
    }
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

/// TS: tasksFlow.tsx TaskBanner's rows (the task line and a rule): the task, how many more are open on its work,
/// its state, and the board's key.
pub fn task_banner_rows(props: &TaskBannerProps<'_>, width: usize) -> Vec<Row> {
    let state = task_state(props.task, props.sessions, props.spinner_frame, None);
    let steps = match props.steps {
        Some(steps) if steps.total > 0 => format!("{}/{} steps · ", steps.done, steps.total),
        _ => String::new(),
    };
    let more = if props.more > 0 {
        format!("+{} open here · ", props.more)
    } else {
        String::new()
    };
    let state_text = if state.text.is_empty() {
        String::new()
    } else {
        format!("{} · ", state.text)
    };
    let right = format!("{steps}{more}{state_text}b board");
    let title = truncate(&props.task.title, minus(width, js_len(&right) + 10).max(4));
    let left = vec![
        Seg::new("◆ Task", TextStyle::fg(THEME.accent)),
        Seg::plain(" "),
        Seg::new(title, TextStyle::fg(THEME.accent_soft).bold(true)),
    ];
    let right = vec![Seg::new(right, TextStyle::fg(THEME.muted))];
    vec![
        space_between(&left, &right, width),
        vec![Seg::new("─".repeat(width.max(1)), TextStyle::fg(THEME.border))],
    ]
}

/// TS: tasksFlow.tsx TaskBanner.
pub fn render_task_banner(props: &TaskBannerProps<'_>, buf: &mut Buffer, area: Rect) {
    let rows = task_banner_rows(props, usize::from(area.width));
    draw_rows(buf, area, &rows);
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

impl RowSelection {
    /// `{key: 'task:<id>', index: 0}`.
    fn task(id: &str) -> Self {
        RowSelection {
            key: Some(format!("task:{id}")),
            index: 0,
        }
    }
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

const fn board_key(keys: &'static [KeySpec], action: &'static str) -> Binding {
    Binding {
        keys,
        action,
        help: HelpRef::Topic("Tasks"),
    }
}

/// The Tasks board's keys (keysDocumented: each must be in the guide's Tasks topic).
pub static KEYMAP: Keymap = &[
    board_key(&[KeySpec::Named(NamedKey::Up), KeySpec::Char("k")], "move up"),
    board_key(&[KeySpec::Named(NamedKey::Down), KeySpec::Char("j")], "move down"),
    board_key(&[KeySpec::Char("g"), KeySpec::Named(NamedKey::Home)], "first row"),
    board_key(&[KeySpec::Char("G"), KeySpec::Named(NamedKey::End)], "last row"),
    board_key(&[KeySpec::Char("v")], "only the selected session's work"),
    board_key(&[KeySpec::Char("f")], "notes: only open items"),
    board_key(
        &[KeySpec::Char("a")],
        "add here (a step on a work) / notes: send the item",
    ),
    board_key(&[KeySpec::Char("A")], "a backlog task / notes: send every open item"),
    board_key(&[KeySpec::Char("E")], "the list in your editor"),
    board_key(&[KeySpec::Char("n")], "start a session for it"),
    board_key(&[KeySpec::Char("w")], "move it"),
    board_key(&[KeySpec::Char("o")], "open the session doing it"),
    board_key(&[KeySpec::Char("x")], "delete (twice)"),
    board_key(&[KeySpec::Char("J"), KeySpec::Char("K")], "reorder"),
    board_key(&[KeySpec::Char(" ")], "tick or reopen"),
    board_key(&[KeySpec::Named(NamedKey::Return)], "edit it / open the note"),
    board_key(&[KeySpec::Named(NamedKey::Tab)], "the notes view / back to tasks"),
    board_key(&[KeySpec::Named(NamedKey::Escape)], "back"),
];

/// The picker row that is a work (a worktree or a main-checkout session).
fn listed(option: &WorkOption) -> bool {
    matches!(option.kind, WorkOptionKind::Worktree | WorkOptionKind::Session)
}

/// A hint part with a short form and a drop rank.
fn hint(text: &str, short: Option<&str>, drop: Option<i32>) -> HintPart {
    HintPart {
        text: text.to_owned(),
        short: short.map(str::to_owned),
        drop,
    }
}

/// A new, empty editor state.
fn empty_state() -> EditorState {
    EditorState::default()
}

/// What the flow's async continuations run with (TS: a `.then` callback of `run`).
type RunThen<T> = Box<dyn FnOnce(&mut TasksFlow, T, &mut FlowCx<App, TasksFlow>)>;

/// The lower part of the board (picker, editor or details): its nodes (what the TS counts) and their rows.
struct Lower {
    nodes: usize,
    rows: Vec<Row>,
}

impl Lower {
    fn of(rows: Vec<Row>) -> Self {
        Lower {
            nodes: rows.len(),
            rows,
        }
    }
}

impl TasksFlow {
    /// TS: TasksFlow.open (b): the board's task view, the selection on the first open task of `scope` (the selected
    /// session's work, which v shows alone).
    pub fn open(&mut self, scope: Option<&str>, props: &TasksFlowProps<'_>) {
        self.view = TasksView::Tasks;
        self.editor = None;
        self.picker = None;
        self.pending_delete = None;
        self.scope = scope.map(str::to_owned);
        self.scoped = false;
        if let Some(first) = first_task_of(scope, props.tasks) {
            self.selected = RowSelection::task(&first.id);
        }
    }

    /// v's work while the board is narrowed to it.
    fn scoped_to(&self) -> Option<&str> {
        if self.scoped { self.scope.as_deref() } else { None }
    }

    /// The board's rows as shown now (TS `rows`: the notes view or the task view).
    pub fn rows(&self, props: &TasksFlowProps<'_>) -> Vec<BoardRow> {
        match self.view {
            TasksView::Notes => {
                let lookup = props.doc.and_then(|_| task_link_lookup(Some(props.tasks)));
                notes_view_rows(
                    props.sessions,
                    &NotesViewOptions {
                        items_only: self.items_only,
                        scope: self.scoped_to(),
                        links: lookup.as_deref(),
                    },
                )
            }
            TasksView::Tasks => {
                // A work's steps: its note's checklist (a worktree's note, a main-checkout session's own), under its
                // task.
                let steps_of = |section: &str| -> Vec<WorkStep> {
                    let note = work_note(section, props.sessions);
                    note_steps(note.as_ref().map(|note| note.text.as_str()))
                };
                board_rows(
                    props.tasks,
                    self.show_older,
                    date::now_ms(),
                    self.scoped_to(),
                    Some(&steps_of),
                )
            }
        }
    }

    /// The selected row's index in `rows` (TS `current`, -1: none). The selection follows its row (by key) across
    /// updates; a row that went keeps the position.
    pub fn current(&self, rows: &[BoardRow]) -> Option<usize> {
        if let Some(key) = self.selected.key.as_deref()
            && let Some(index) = rows.iter().position(|row| row_key(row).as_deref() == Some(key))
        {
            return Some(index);
        }
        let selectable = selectable_indexes(rows);
        let last = selectable.len().checked_sub(1)?;
        selectable.get(self.selected.index.min(last)).copied()
    }

    /// TS `select(index)`.
    fn select(&mut self, rows: &[BoardRow], selectable: &[usize], index: usize) {
        self.selected = RowSelection {
            key: rows.get(index).and_then(row_key),
            index: selectable.iter().position(|at| *at == index).unwrap_or(0),
        };
    }

    /// TS `move(direction)`.
    fn move_by(&mut self, rows: &[BoardRow], current: Option<usize>, direction: i64) {
        let selectable = selectable_indexes(rows);
        if selectable.is_empty() {
            return;
        }
        let at = current
            .and_then(|current| selectable.iter().position(|index| *index == current))
            .map_or(0, signed);
        let to = to_index((at + direction).max(0).min(signed(selectable.len() - 1)));
        if let Some(index) = selectable.get(to).copied() {
            self.select(rows, &selectable, index);
        }
    }

    /// TS `run(work, then)`: one request at a time; a failure goes to the error line.
    fn run<T: 'static>(
        &mut self,
        props: &TasksFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
        work: impl FnOnce(&LiveClient) -> Reply<T>,
        then: RunThen<T>,
    ) {
        let Some(client) = props.client else {
            cx.set_error(Some("still connecting to daemon".to_owned()));
            return;
        };
        if self.busy {
            return;
        }
        self.busy = true;
        let reply = work(client);
        let _detached = cx.spawn(reply, move |flow: &mut TasksFlow, result, cx| {
            match result {
                Ok(value) => then(flow, value, cx),
                Err(error) => cx.set_error(Some(error_message(&error))),
            }
            flow.busy = false;
        });
    }

    /// TS: useTasksFlow run/apply: one op on the task list at a time; the new doc goes to the app, then `then`.
    pub fn apply(
        &mut self,
        op: TaskOp,
        props: &TasksFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
        then: Option<ApplyThen>,
    ) {
        let repo_root = props.repo_root.to_owned();
        self.run(
            props,
            cx,
            move |client| client.task_op(&repo_root, op),
            Box::new(move |flow, next: TasksDoc, cx| {
                cx.emit(FlowEffect::TasksDoc(Box::new(next.clone())));
                if let Some(then) = then {
                    then(flow, &next, cx);
                }
            }),
        );
    }

    /// TS: useTasksFlow save: the editor's task (added, edited, or a step).
    pub fn save(&mut self, state: TaskEditor, props: &TasksFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let title = js_trim(&state.title.text).to_owned();
        if title.is_empty() {
            cx.set_error(Some(
                if state.kind == TaskEditorKind::Step {
                    "A step needs some text"
                } else {
                    "A task needs a title"
                }
                .to_owned(),
            ));
            return;
        }
        if state.kind == TaskEditorKind::Step {
            let section = state.section.clone().unwrap_or_default();
            self.add_step(&section, &title, props, cx, true);
            return;
        }
        let before: HashSet<String> = props.tasks.iter().map(|task| task.id.clone()).collect();
        let section = state.section.as_deref().filter(|section| !section.is_empty());
        let link = section.and_then(link_of_key);
        match state.kind {
            TaskEditorKind::Add => {
                let message = match section {
                    Some(section) => format!("Added as the task of {} (◆)", work_label(section, props.sessions)),
                    None => "Added to the backlog".to_owned(),
                };
                let op = TaskOp::Add {
                    title,
                    body: Some(state.body.text.clone()),
                    link,
                    id: None,
                    from: None,
                };
                let then: ApplyThen = Box::new(move |flow, next, cx| {
                    flow.editor = None;
                    if let Some(added) = parse_added(next, &before) {
                        flow.selected = RowSelection::task(&added);
                    }
                    cx.set_status(Some(message));
                });
                self.apply(op, props, cx, Some(then));
            }
            _ => {
                if let Some(id) = state.id.clone() {
                    let op = TaskOp::Edit {
                        id,
                        title,
                        body: state.body.text.clone(),
                    };
                    let then: ApplyThen = Box::new(|flow, _, cx| {
                        flow.editor = None;
                        cx.set_status(Some("Saved the task".to_owned()));
                    });
                    self.apply(op, props, cx, Some(then));
                }
            }
        }
    }

    /// TS: useTasksFlow editorInput: keys while the editor is open.
    pub fn editor_input(
        &mut self,
        input: &Input,
        state: TaskEditor,
        props: &TasksFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
    ) {
        let key = &input.key;
        if key.escape {
            self.editor = None;
            return;
        }
        if key.ctrl && input.input == "s" {
            self.save(state, props, cx);
            return;
        }
        if key.tab {
            if state.kind != TaskEditorKind::Step {
                let field = if state.field == TaskEditorField::Title {
                    TaskEditorField::Body
                } else {
                    TaskEditorField::Title
                };
                self.editor = Some(TaskEditor { field, ..state });
            }
            return;
        }
        if state.field == TaskEditorField::Title {
            if key.return_ {
                self.save(state, props, cx);
                return;
            }
            if key.up_arrow || key.down_arrow {
                return;
            }
            let next = edit_text(&state.title, &input.input, key, &title_options());
            if let Some(message) = next.message.filter(|message| !message.is_empty()) {
                cx.set_error(Some(message));
                return;
            }
            let title = EditorState {
                text: next.text.replace('\n', " "),
                cursor: next.cursor,
                ..EditorState::default()
            };
            self.editor = Some(TaskEditor { title, ..state });
            return;
        }
        let options = EditOptions {
            wrap_width: Some(self.body_width),
            page_rows: Some(EDITOR_BODY_ROWS),
            ..body_options()
        };
        let next = if key.return_ {
            let enter = Key {
                return_: true,
                ..Key::NONE
            };
            edit_text(&state.body, "", &enter, &options)
        } else {
            edit_text(&state.body, &input.input, key, &options)
        };
        if let Some(message) = next.message.filter(|message| !message.is_empty()) {
            cx.set_error(Some(message));
            return;
        }
        let body = EditorState {
            text: next.text,
            cursor: next.cursor,
            ..EditorState::default()
        };
        self.editor = Some(TaskEditor { body, ..state });
    }

    /// A new step at the end of a work's note (revision-checked, as if typed in its Notes tab); `close_editor`: the
    /// editor closes once it is saved.
    fn add_step(
        &mut self,
        section: &str,
        title: &str,
        props: &TasksFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
        close_editor: bool,
    ) {
        let Some(note) = work_note(section, props.sessions) else {
            cx.set_status(Some("Its note is not in this list (archived or filtered?)".to_owned()));
            return;
        };
        if note.read_only == Some(true) {
            cx.set_status(Some("Its worktree was deleted: its note is read-only".to_owned()));
            return;
        }
        let message = format!(
            "Added a step to {}: {}",
            work_label(section, props.sessions),
            truncate(title, 40)
        );
        let text = append_note_item(&note.text, title, "");
        self.run(
            props,
            cx,
            move |client| {
                client.save_note(
                    &note.session_id,
                    note.section,
                    &text,
                    &note.revision,
                    note.note_id.as_deref(),
                )
            },
            Box::new(move |flow, result: NoteSaveResult, cx| {
                cx.emit(FlowEffect::Session(Box::new(result.session.into_inner())));
                if !result.saved {
                    cx.set_status(Some("The note changed meanwhile; try again".to_owned()));
                    return;
                }
                if close_editor {
                    flow.editor = None;
                }
                cx.set_status(Some(message));
            }),
        );
    }

    /// The picker's rows for `state` (pickerRowsOf).
    fn picker_rows_of(state: &TaskPicker, props: &TasksFlowProps<'_>) -> Vec<WorkOption> {
        picker_rows(
            &work_options(props.sessions, state.current.as_deref()),
            state.view,
            &state.query,
            state.origin.as_deref(),
        )
    }

    /// Where the selection starts in a view: where the task is now (without a search), else the first listed match.
    fn picker_start(state: &TaskPicker, props: &TasksFlowProps<'_>) -> usize {
        let rows = Self::picker_rows_of(state, props);
        let here = if state.query.is_empty() {
            rows.iter()
                .position(|option| listed(option) && option.section == state.current)
        } else {
            None
        };
        here.or_else(|| rows.iter().position(listed)).unwrap_or(0)
    }

    /// TS openPicker: the picker on `state`, its selection where it starts.
    fn open_picker(&mut self, state: TaskPicker, props: &TasksFlowProps<'_>) {
        let index = Self::picker_start(
            &TaskPicker {
                index: 0,
                ..state.clone()
            },
            props,
        );
        self.picker = Some(TaskPicker { index, ..state });
    }

    /// TS becomesStep: onto work without a task, w makes the task that work's (◆); onto work with one, a step in its
    /// note.
    fn becomes_step(state: &TaskPicker, section: Option<&str>, tasks: &[Task]) -> bool {
        section.is_some_and(|section| !section.is_empty() && has_main_task(section, tasks, Some(&state.task_id)))
    }

    /// TS: useTasksFlow pickerInput: keys while the w picker is open.
    pub fn picker_input(
        &mut self,
        input: &Input,
        state: TaskPicker,
        props: &TasksFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
    ) {
        let key = &input.key;
        let text = input.input.as_str();
        let rows = Self::picker_rows_of(&state, props);
        let last = signed(rows.len()) - 1;
        let to = |flow: &mut TasksFlow, index: i64| {
            flow.picker = Some(TaskPicker {
                index: to_index(index.min(last).max(0)),
                ..state.clone()
            });
        };
        let index = signed(state.index);
        let switched = || TaskPicker {
            view: if state.view == PickerView::Worktrees {
                PickerView::Sessions
            } else {
                PickerView::Worktrees
            },
            ..state.clone()
        };
        if key.tab {
            self.open_picker(switched(), props);
            return;
        }
        if key.up_arrow {
            return to(self, index - 1);
        }
        if key.down_arrow {
            return to(self, index + 1);
        }
        if key.page_up {
            return to(self, index - signed(PICKER_ROWS));
        }
        if key.page_down {
            return to(self, index + signed(PICKER_ROWS));
        }
        // Searching (/): keys are text until Enter keeps the search or Esc clears it, as in the sidebar's search.
        if state.typing {
            if key.escape {
                self.open_picker(
                    TaskPicker {
                        query: String::new(),
                        typing: false,
                        ..state
                    },
                    props,
                );
                return;
            }
            if key.return_ {
                self.picker = Some(TaskPicker { typing: false, ..state });
                return;
            }
            if key.backspace || key.delete {
                let query = js_slice(&state.query, 0, Some(-1));
                self.open_picker(TaskPicker { query, ..state }, props);
                return;
            }
            let typed = if key.ctrl || key.meta {
                String::new()
            } else {
                strip_terminal_controls(text)
            };
            if !typed.is_empty() {
                let query = js_slice(&format!("{}{typed}", state.query), 0, Some(64));
                self.open_picker(TaskPicker { query, ..state }, props);
            }
            return;
        }
        if key.escape {
            if state.query.is_empty() {
                self.picker = None;
            } else {
                self.open_picker(
                    TaskPicker {
                        query: String::new(),
                        ..state
                    },
                    props,
                );
            }
            return;
        }
        if text == "/" {
            self.picker = Some(TaskPicker { typing: true, ..state });
            return;
        }
        if key.left_arrow || key.right_arrow || text == "h" || text == "l" {
            self.open_picker(switched(), props);
            return;
        }
        if text == "k" {
            return to(self, index - 1);
        }
        if text == "j" {
            return to(self, index + 1);
        }
        if key.home || text == "g" {
            return to(self, 0);
        }
        if key.end || text == "G" {
            return to(self, last);
        }
        if !key.return_ {
            return;
        }
        let Some(option) = rows.get(state.index).cloned() else {
            return;
        };
        let repo_root = props.repo_root.to_owned();
        let task_id = state.task_id.clone();
        if option.kind == WorkOptionKind::Note {
            let message = format!(
                "Back in the note of {}: {}",
                state.origin.as_deref().unwrap_or("undefined"),
                truncate(&state.title, 40)
            );
            self.run(
                props,
                cx,
                move |client| client.return_task_to_note(&repo_root, &task_id),
                Box::new(move |flow, next: TasksDoc, cx| {
                    cx.emit(FlowEffect::TasksDoc(Box::new(next)));
                    flow.picker = None;
                    cx.set_status(Some(message));
                }),
            );
            return;
        }
        if option.section == state.current {
            self.picker = None;
            cx.set_status(Some("It is already there".to_owned()));
            return;
        }
        if Self::becomes_step(&state, option.section.as_deref(), props.tasks) {
            let section = option.section.clone().unwrap_or_default();
            let message = format!(
                "Now a step in the note of {} (it has its task already): {}",
                option.label,
                truncate(&state.title, 40)
            );
            self.run(
                props,
                cx,
                move |client| client.task_to_note(&repo_root, &task_id, &section),
                Box::new(move |flow, next: TasksDoc, cx| {
                    cx.emit(FlowEffect::TasksDoc(Box::new(next)));
                    flow.picker = None;
                    cx.set_status(Some(message));
                }),
            );
            return;
        }
        let section = option.section.as_deref().filter(|section| !section.is_empty());
        let link = section.and_then(link_of_key);
        let message = match section {
            None => "Moved to the backlog".to_owned(),
            Some(_) => format!(
                "Now the task of {} (◆): its note's checklist shows as its steps, merging or finishing it ticks it",
                option.label
            ),
        };
        let then: ApplyThen = Box::new(move |flow, _, cx| {
            flow.picker = None;
            flow.selected = RowSelection::task(&task_id);
            cx.set_status(Some(message));
        });
        self.apply(
            TaskOp::Assign {
                id: state.task_id.clone(),
                link,
            },
            props,
            cx,
            Some(then),
        );
    }

    /// Space on a step: ticks it in its note (revision-checked, like typing in the Notes tab).
    fn toggle_step(&mut self, section: &str, step: &WorkStep, props: &TasksFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let Some(note) = work_note(section, props.sessions) else {
            cx.set_status(Some("Its note is not in this list (archived or filtered?)".to_owned()));
            return;
        };
        if note.read_only == Some(true) {
            cx.set_status(Some("Its worktree was deleted: its note is read-only".to_owned()));
            return;
        }
        let Some(text) = deckhand_core::notes::toggle_checklist_line(&note.text, step.line) else {
            cx.set_status(Some("That line of the note changed; try again".to_owned()));
            return;
        };
        let done_message = if step.done {
            format!("Reopened: {}", truncate(&step.text, 50))
        } else {
            format!("Ticked: {}", truncate(&step.text, 50))
        };
        self.run(
            props,
            cx,
            move |client| {
                client.save_note(
                    &note.session_id,
                    note.section,
                    &text,
                    &note.revision,
                    note.note_id.as_deref(),
                )
            },
            Box::new(move |_, result: NoteSaveResult, cx| {
                cx.emit(FlowEffect::Session(Box::new(result.session.into_inner())));
                cx.set_status(Some(if result.saved {
                    done_message
                } else {
                    "The note changed meanwhile; try again".to_owned()
                }));
            }),
        );
    }

    /// TS: useTasksFlow promote: a note item becomes a task.
    pub fn promote(&mut self, item: &NoteItem, props: &TasksFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let item = item.clone();
        let message = format!("Sent to Tasks: {} (the note keeps a ↗ link)", truncate(&item.title, 40));
        self.run(
            props,
            cx,
            move |client| {
                client.promote_note_item(
                    &item.session_id,
                    item.section,
                    signed(item.line),
                    &item.revision,
                    item.note_id.as_deref(),
                )
            },
            Box::new(move |_, result: PromoteNoteItemResult, cx| {
                cx.emit(FlowEffect::Session(Box::new(result.session.into_inner())));
                cx.emit(FlowEffect::TasksDoc(Box::new(result.tasks)));
                cx.set_status(Some(message));
            }),
        );
    }

    /// TS addSection: a new task joins the group the selection is in (a worktree's, or the backlog); in the v view,
    /// that work's. a in a work's group: a step in its note (it has its task), or its task when it has none;
    /// elsewhere the backlog.
    fn add_section(&self, current_row: Option<&BoardRow>) -> Option<String> {
        if self.scoped {
            return self.scope.clone();
        }
        match current_row {
            Some(BoardRow::Task {
                group: TaskGroup::Progress,
                section,
                ..
            }) => Some(section.clone()),
            Some(BoardRow::Step { section, .. }) => Some(section.clone()),
            _ => None,
        }
    }

    /// The Notes view's keys (after the shared ones).
    fn notes_input(
        &mut self,
        input: &Input,
        current_row: Option<&BoardRow>,
        props: &TasksFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
    ) {
        let key = &input.key;
        let text = input.input.as_str();
        if key.escape || key.tab {
            self.view = TasksView::Tasks;
            self.selected = RowSelection::default();
            return;
        }
        if text == "f" {
            self.items_only = !self.items_only;
            self.selected = RowSelection::default();
            return;
        }
        let line_item = match current_row {
            Some(BoardRow::NoteLine { item, .. }) => item.as_ref(),
            _ => None,
        };
        let block = match current_row {
            Some(BoardRow::NoteHead { block, .. } | BoardRow::NoteLine { block, .. }) => Some(Rc::clone(block)),
            _ => None,
        };
        if text == "a" {
            match line_item {
                Some(item) => self.promote(item, props, cx),
                None => cx.set_status(Some("Select an open checklist item (☐) to add it as a task".to_owned())),
            }
            return;
        }
        // A: every open item of the selected note at once.
        if text == "A" {
            let Some(block) = block else {
                cx.set_status(Some("Select a note first (j/k)".to_owned()));
                return;
            };
            if block_items(&block, props.sessions).is_empty() {
                cx.set_status(Some("No open items in this note".to_owned()));
                return;
            }
            self.run(
                props,
                cx,
                move |client| {
                    client.send_open_items(
                        &block.session_id,
                        block.section,
                        &block.revision,
                        block.note_id.as_deref(),
                    )
                },
                Box::new(|_, result: SendOpenItemsResult, cx| {
                    cx.emit(FlowEffect::Session(Box::new(result.session.into_inner())));
                    cx.emit(FlowEffect::TasksDoc(Box::new(result.tasks)));
                    cx.set_status(Some(format!(
                        "Sent {} item{} to Tasks (the note keeps a ↗ link for each)",
                        result.sent,
                        if result.sent == 1 { "" } else { "s" }
                    )));
                }),
            );
            return;
        }
        if key.return_
            && let Some(block) = block
        {
            cx.emit(FlowEffect::OpenNote(block.session_id.clone()));
        }
    }

    /// E: the task list in the user's editor.
    fn open_list_in_editor(&mut self, props: &TasksFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let repo_root = props.repo_root.to_owned();
        self.run(
            props,
            cx,
            move |client| client.open_tasks(&repo_root),
            Box::new(|_, file: String, cx| {
                let errors = RefCell::new(Vec::new());
                let label = deckhand_sys::desktop::open_in_editor(
                    &file,
                    &|message| errors.borrow_mut().push(format!("{message}; the task list is {file}")),
                    None,
                );
                for error in errors.into_inner() {
                    cx.set_error(Some(error));
                }
                if let Some(label) = label.filter(|label| !label.is_empty()) {
                    cx.set_status(Some(format!(
                        "Opened the task list in {label}; edits there show up here"
                    )));
                }
            }),
        );
    }

    /// TS pickerLines: the w picker under the list.
    fn picker_lines(state: &TaskPicker, props: &TasksFlowProps<'_>, width: usize) -> Vec<Row> {
        let options = work_options(props.sessions, state.current.as_deref());
        let rows = picker_rows(&options, state.view, &state.query, state.origin.as_deref());
        let counts = picker_counts(&options, &state.query);
        let start =
            to_index((signed(state.index) - signed(PICKER_ROWS) + 1).min(signed(rows.len()) - signed(PICKER_ROWS)));
        let tab = |view: PickerView, text: String| {
            let on = state.view == view;
            Seg::new(
                format!(" {text} "),
                TextStyle::default()
                    .inverse(on)
                    .bold(on)
                    .color(if on { THEME.active } else { THEME.muted }),
            )
        };
        let any_listed = rows.iter().any(listed);
        let view_name = match state.view {
            PickerView::Worktrees => "worktrees",
            PickerView::Sessions => "sessions",
        };
        let none = if !state.query.is_empty() {
            format!("Nothing in {view_name} matches “{}”", state.query)
        } else if state.view == PickerView::Worktrees {
            "No worktrees".to_owned()
        } else {
            "No sessions in the main checkout".to_owned()
        };
        let accent = TextStyle::fg(THEME.accent).bold(true);
        let mut lines = vec![line(
            vec![
                Seg::new("Move ", accent),
                Seg::plain(truncate(&format!("“{}”", state.title), minus(width, 12).max(4))),
                Seg::new(" to", accent),
            ],
            TextStyle::default(),
            width,
        )];
        let search = if state.typing {
            format!("search: {}▏", state.query)
        } else if !state.query.is_empty() {
            format!("search: {}", state.query)
        } else {
            "/ search".to_owned()
        };
        let searching = !state.query.is_empty() || state.typing;
        lines.push(line(
            vec![
                tab(PickerView::Worktrees, format!("Worktrees {}", counts.worktrees)),
                Seg::plain(" "),
                tab(PickerView::Sessions, format!("Sessions {}", counts.sessions)),
                Seg::new(
                    format!("   {search}"),
                    TextStyle::fg(if searching { THEME.active } else { THEME.muted }),
                ),
            ],
            TextStyle::default(),
            width,
        ));
        for (offset, option) in rows.iter().skip(start).take(PICKER_ROWS).enumerate() {
            let selected = start + offset == state.index;
            // Worktrees and sessions say what the task would be there: its task (◆), or a step in its note.
            let role = if listed(option) && option.section != state.current {
                if Self::becomes_step(state, option.section.as_deref(), props.tasks) {
                    "→ a step in its note"
                } else {
                    "◆ its task"
                }
            } else {
                ""
            };
            let parts: Vec<&str> = [option.note.as_str(), role]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect();
            let note = if parts.is_empty() {
                String::new()
            } else {
                format!(" {}", parts.join(" · "))
            };
            let note_len = js_len(&note);
            let label = format!(
                "{} {}",
                marker(selected),
                truncate(&option.label, minus(width, note_len + 2).max(1))
            );
            let base = TextStyle::default()
                .inverse(selected)
                .bold(selected)
                .color(if selected { Some(THEME.active) } else { None });
            lines.push(line(
                vec![
                    Seg::plain(js_pad_end(&label, minus(width, note_len), " ")),
                    Seg::new(
                        note,
                        TextStyle::default().color(if selected { None } else { Some(THEME.muted) }),
                    ),
                ],
                base,
                width,
            ));
        }
        if !any_listed {
            lines.push(line(
                vec![Seg::plain(format!("  {none}"))],
                TextStyle::fg(THEME.muted),
                width,
            ));
        }
        lines
    }

    /// TS editorLines: the add/edit form under the list (records the body's wrap width for Up/Down).
    fn editor_lines(&mut self, state: &TaskEditor, props: &TasksFlowProps<'_>, width: usize) -> Lower {
        let label = |text: &str, active: bool| {
            Seg::new(
                text,
                TextStyle::default()
                    .color(if active { THEME.active } else { THEME.muted })
                    .bold(active),
            )
        };
        let cursor_of = |line: &EditorLine, active: bool| -> Vec<Seg> {
            let mut segs = vec![Seg::plain(line.before.clone())];
            match (&line.cursor, active) {
                (Some(cursor), true) => segs.push(Seg::new(cursor.clone(), TextStyle::default().inverse(true))),
                (cursor, _) => segs.push(Seg::plain(cursor.clone().unwrap_or_default())),
            }
            segs.push(Seg::plain(line.after.clone()));
            segs
        };
        let title_width = minus(width, 8).max(4);
        let title_rows = wrapped_editor_lines(&state.title, title_width);
        let title_line = title_rows
            .lines
            .get(title_rows.cursor_row)
            .cloned()
            .unwrap_or_else(|| EditorLine {
                before: state.title.text.clone(),
                cursor: None,
                after: String::new(),
            });
        self.body_width = minus(width, 2).max(4);
        let body = wrapped_editor_lines(&state.body, self.body_width);
        let start = to_index(
            (signed(body.cursor_row) - signed(EDITOR_BODY_ROWS) + 1)
                .min(signed(body.lines.len()) - signed(EDITOR_BODY_ROWS)),
        );
        let heading = match state.kind {
            TaskEditorKind::Step => "New step",
            TaskEditorKind::Add => "New task",
            TaskEditorKind::Edit => "Edit task",
        };
        let mut head = vec![Seg::new(heading, TextStyle::fg(THEME.accent).bold(true))];
        if state.kind != TaskEditorKind::Edit {
            let place = match state.section.as_deref().filter(|section| !section.is_empty()) {
                Some(section) => format!(
                    "{}{}",
                    if state.kind == TaskEditorKind::Step {
                        "the note of "
                    } else {
                        "the task of "
                    },
                    work_label(section, props.sessions)
                ),
                None => "backlog".to_owned(),
            };
            head.push(Seg::new(format!(" · {place}"), TextStyle::fg(THEME.muted)));
        }
        let on_title = state.field == TaskEditorField::Title;
        let on_body = state.field == TaskEditorField::Body;
        let mut title = vec![label("Title  ", on_title)];
        if on_title {
            title.extend(cursor_of(&title_line, true));
        } else {
            title.push(Seg::plain(truncate(&state.title.text, title_width)));
        }
        let optional = if !state.body.text.is_empty() || on_body {
            ""
        } else {
            " (optional; typed into the agent's input with the title)"
        };
        let body_label = vec![
            label("Details", on_body),
            Seg::new(optional, TextStyle::fg(THEME.muted)),
        ];
        let mut rows = vec![
            line(head, TextStyle::default(), width),
            line(title, TextStyle::default(), width),
        ];
        // The details label is a wrapping Text: one node, possibly several rows.
        rows.extend(fit_row(&body_label, width, Wrap::Wrap));
        let mut nodes = 3;
        for (index, body_line) in body.lines.iter().skip(start).take(EDITOR_BODY_ROWS).enumerate() {
            let mut segs = vec![Seg::plain("  ")];
            segs.extend(cursor_of(body_line, on_body && start + index == body.cursor_row));
            rows.push(line(segs, TextStyle::default(), width));
            nodes += 1;
        }
        Lower { nodes, rows }
    }
}

/// The indexes of the selectable rows.
fn selectable_indexes(rows: &[BoardRow]) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, row)| selectable_row(row))
        .map(|(index, _)| index)
        .collect()
}

/// TS firstTaskOf: the first open task of a work.
fn first_task_of<'a>(section: Option<&str>, tasks: &'a [Task]) -> Option<&'a Task> {
    let section = section.filter(|section| !section.is_empty())?;
    tasks
        .iter()
        .find(|task| !task.done && link_key(&task.meta).as_deref() == Some(section))
}

impl Flow<App> for TasksFlow {
    type Props<'a> = TasksFlowProps<'a>;

    fn handle_input(&mut self, props: &TasksFlowProps<'_>, input: &Input, cx: &mut FlowCx<App, Self>) {
        if let Some(editor) = self.editor.clone() {
            self.editor_input(input, editor, props, cx);
            return;
        }
        if let Some(picker) = self.picker.clone() {
            self.picker_input(input, picker, props, cx);
            return;
        }
        let key = &input.key;
        let text = input.input.as_str();
        let rows = self.rows(props);
        let selectable = selectable_indexes(&rows);
        let current = self.current(&rows);
        let current_row = current.and_then(|index| rows.get(index));
        let deleting = self.pending_delete.take();
        if key.up_arrow || text == "k" {
            self.move_by(&rows, current, -1);
            return;
        }
        if key.down_arrow || text == "j" {
            self.move_by(&rows, current, 1);
            return;
        }
        if (text == "g" || key.home)
            && let Some(first) = selectable.first().copied()
        {
            self.select(&rows, &selectable, first);
            return;
        }
        if (text == "G" || key.end)
            && let Some(last) = selectable.last().copied()
        {
            self.select(&rows, &selectable, last);
            return;
        }
        // v: only the work of the session the board was opened from (its worktree), in either view.
        if text == "v" {
            let Some(scope) = self.scope.clone().filter(|scope| !scope.is_empty()) else {
                cx.set_status(Some(format!(
                    "Select a session first: v shows the {} of its worktree",
                    if self.view == TasksView::Notes {
                        "notes"
                    } else {
                        "tasks"
                    }
                )));
                return;
            };
            let first = if self.view == TasksView::Tasks {
                first_task_of(Some(&scope), props.tasks)
            } else {
                None
            };
            self.scoped = !self.scoped;
            self.selected = first.map_or_else(RowSelection::default, |first| RowSelection::task(&first.id));
            return;
        }
        if self.view == TasksView::Notes {
            self.notes_input(input, current_row, props, cx);
            return;
        }
        if key.escape {
            cx.exit();
            return;
        }
        if key.tab {
            self.view = TasksView::Notes;
            self.selected = RowSelection::default();
            return;
        }
        if text == "a" {
            let section = self.add_section(current_row);
            let step = section
                .as_deref()
                .is_some_and(|section| !section.is_empty() && has_main_task(section, props.tasks, None));
            self.editor = Some(TaskEditor {
                kind: if step {
                    TaskEditorKind::Step
                } else {
                    TaskEditorKind::Add
                },
                id: None,
                section,
                field: TaskEditorField::Title,
                title: empty_state(),
                body: empty_state(),
            });
            return;
        }
        // A: a new backlog task from anywhere (a adds a step when on a work, which has its task).
        if text == "A" {
            self.editor = Some(TaskEditor {
                kind: TaskEditorKind::Add,
                id: None,
                section: None,
                field: TaskEditorField::Title,
                title: empty_state(),
                body: empty_state(),
            });
            return;
        }
        if text == "E" {
            self.open_list_in_editor(props, cx);
            return;
        }
        if matches!(current_row, Some(BoardRow::Older { .. })) && (key.return_ || text == " ") {
            self.show_older = !self.show_older;
            return;
        }
        if let Some(BoardRow::Step { section, step, .. }) = current_row {
            if text == " " {
                self.toggle_step(section, step, props, cx);
                return;
            }
            if key.return_ {
                match work_note(section, props.sessions) {
                    Some(note) => cx.emit(FlowEffect::OpenNote(note.session_id)),
                    None => cx.set_status(Some("Its note is not in this list (archived or filtered?)".to_owned())),
                }
                return;
            }
            if matches!(text, "n" | "o" | "x" | "w" | "J" | "K") {
                cx.set_status(Some(
                    "A step from the note: space ticks it, enter opens the note to edit it".to_owned(),
                ));
            }
            return;
        }
        let Some(BoardRow::Task {
            task,
            group,
            section: task_section,
            ..
        }) = current_row
        else {
            if matches!(text, "n" | "o" | "x" | "w" | " ") || key.return_ {
                cx.set_status(Some("Select a task first (j/k)".to_owned()));
            }
            return;
        };
        let group = *group;
        if key.return_ {
            self.editor = Some(TaskEditor {
                kind: TaskEditorKind::Edit,
                id: Some(task.id.clone()),
                section: None,
                field: TaskEditorField::Title,
                title: EditorState {
                    text: task.title.clone(),
                    cursor: task.title.len(),
                    ..EditorState::default()
                },
                body: EditorState {
                    text: task.body.clone(),
                    cursor: task.body.len(),
                    ..EditorState::default()
                },
            });
            return;
        }
        if text == " " {
            let message = if task.done {
                format!("Reopened {}", task.title)
            } else {
                format!("Done: {}", task.title)
            };
            let then: ApplyThen = Box::new(move |_, _, cx| cx.set_status(Some(message)));
            self.apply(TaskOp::Toggle { id: task.id.clone() }, props, cx, Some(then));
            return;
        }
        if text == "n" {
            match group {
                TaskGroup::Backlog => cx.emit(FlowEffect::StartTask(Box::new(task.clone()))),
                TaskGroup::Progress => cx.set_status(Some(
                    "That task is already being worked on (o opens its session)".to_owned(),
                )),
                TaskGroup::Done => cx.set_status(Some("That task is done; space reopens it".to_owned())),
            }
            return;
        }
        if text == "w" {
            if task.done {
                cx.set_status(Some("That task is done; space reopens it".to_owned()));
                return;
            }
            let current = link_key(&task.meta);
            let view = picker_view_of(current.as_deref());
            self.open_picker(
                TaskPicker {
                    task_id: task.id.clone(),
                    title: task.title.clone(),
                    origin: task_origin(task, props.sessions),
                    current,
                    view,
                    query: String::new(),
                    typing: false,
                    index: 0,
                },
                props,
            );
            return;
        }
        if text == "o" {
            let working = task_sessions(task, props.sessions);
            match lead_session(&working) {
                Some(lead) => cx.emit(FlowEffect::GoTo(lead.session().id.clone())),
                None => cx.set_status(Some(
                    if group == TaskGroup::Progress {
                        "Its session is not in this list (archived or filtered?)"
                    } else {
                        "That task has no session; n starts one"
                    }
                    .to_owned(),
                )),
            }
            return;
        }
        if text == "x" {
            if deleting.as_deref() == Some(task.id.as_str()) {
                let message = format!("Deleted {}", task.title);
                let then: ApplyThen = Box::new(move |_, _, cx| cx.set_status(Some(message)));
                self.apply(TaskOp::Remove { id: task.id.clone() }, props, cx, Some(then));
                return;
            }
            self.pending_delete = Some(task.id.clone());
            cx.set_status(Some(format!("x again deletes “{}”", truncate(&task.title, 40))));
            return;
        }
        if (text == "J" || text == "K") && group != TaskGroup::Done {
            let direction: isize = if text == "J" { 1 } else { -1 };
            let Some(index) = current else { return };
            if let Some(neighbor) = neighbor_in_group(&rows, index, direction, task_section) {
                let op = TaskOp::Move {
                    id: task.id.clone(),
                    target: neighbor.id.clone(),
                    after: (direction > 0).then_some(true),
                };
                self.apply(op, props, cx, None);
            }
        }
    }

    fn render(&mut self, props: &TasksFlowProps<'_>, buf: &mut Buffer, area: Rect) {
        let width = usize::from(area.width);
        let height = usize::from(area.height);
        let inner = minus(width, 4).max(10);
        let rows = self.rows(props);
        let current = self.current(&rows);
        let current_row = current.and_then(|index| rows.get(index));
        let open = props.tasks.iter().filter(|task| !task.done).count();
        let in_progress = rows
            .iter()
            .filter(|row| {
                matches!(
                    row,
                    BoardRow::Task {
                        group: TaskGroup::Progress,
                        ..
                    }
                )
            })
            .count();
        let narrowed = if self.scoped {
            self.scope.as_deref().filter(|scope| !scope.is_empty())
        } else {
            None
        };
        let name = match narrowed {
            Some(scope) => work_label(scope, props.sessions),
            None => {
                let base = path::basename(props.repo_root);
                if base.is_empty() {
                    props.repo_root.to_owned()
                } else {
                    base
                }
            }
        };
        let open_here = narrowed.map_or(0, |scope| {
            props
                .tasks
                .iter()
                .filter(|task| !task.done && link_key(&task.meta).as_deref() == Some(scope))
                .count()
        });
        let open_items = rows
            .iter()
            .filter(|row| matches!(row, BoardRow::NoteLine { item: Some(_), .. }))
            .count();
        let right = if self.view == TasksView::Notes {
            format!(
                "{open_items} open item{}{}{}",
                if open_items == 1 { "" } else { "s" },
                if self.items_only { " · items only" } else { "" },
                if narrowed.is_some() { " · v all" } else { "" }
            )
        } else if narrowed.is_some() {
            format!("{open_here} open · v all tasks")
        } else {
            format!("{open} open · {in_progress} in progress")
        };
        let lower = if let Some(picker) = self.picker.clone() {
            Lower::of(Self::picker_lines(&picker, props, inner))
        } else if let Some(editor) = self.editor.clone() {
            self.editor_lines(&editor, props, inner)
        } else {
            Lower::of(detail_lines(current_row, props.sessions, props.spinner_frame, inner))
        };
        // Border (2), title and status lines (2), the `+N more` line (1), then the rule and the lower part.
        let lower_rows = if lower.nodes > 0 { lower.nodes + 1 } else { 0 };
        let list_rows = to_index((signed(height) - 2 - 3 - signed(lower_rows)).max(1));
        self.top = scrolled_list_top(self.top, current, list_rows, rows.len(), &|index| {
            rows.get(index).is_some_and(selectable_row)
        });
        let top = self.top;
        let content = bordered(buf, area, &Border::round(THEME.border_active), 1);
        let mut column = Column::new();
        let title = if self.view == TasksView::Notes {
            "✎ Notes"
        } else {
            "☐ Tasks"
        };
        let left = with_base(
            vec![
                Seg::new(title, TextStyle::fg(THEME.active).bold(true)),
                Seg::new(
                    format!(" · {}", truncate(&name, minus(inner, js_len(&right) + 12).max(4))),
                    TextStyle::fg(THEME.muted),
                ),
            ],
            TextStyle::default(),
        );
        column.push(space_between(
            &left,
            &[Seg::new(right.clone(), TextStyle::fg(THEME.muted))],
            inner,
        ));
        if props.doc.is_some_and(|doc| doc.too_large == Some(true)) {
            column.push(line(
                vec![Seg::plain(
                    "The task list is too large to change here; E opens it in your editor",
                )],
                TextStyle::fg(THEME.warn),
                inner,
            ));
        } else {
            column.push(vec![Seg::plain(" ")]);
        }
        let editing = self.editor.is_some() || self.picker.is_some();
        for offset in 0..list_rows {
            let index = top + offset;
            match rows.get(index) {
                Some(row) => column.push(board_line(
                    row,
                    current == Some(index) && !editing,
                    inner,
                    props.sessions,
                    props.spinner_frame,
                )),
                None => column.blank(),
            }
        }
        if rows.len() > top + list_rows {
            column.push(vec![Seg::new(
                format!("  +{} more", rows.len() - top - list_rows),
                TextStyle::fg(THEME.muted),
            )]);
        } else {
            column.push(vec![Seg::plain(" ")]);
        }
        if lower.nodes > 0 {
            column.push(vec![Seg::new("─".repeat(inner), TextStyle::fg(THEME.border))]);
        }
        column.extend(lower.rows);
        column.draw(buf, content);
    }

    /// TS: TasksFlow.hint: the board's footer.
    fn hint(&self, props: &TasksFlowProps<'_>, width: usize) -> String {
        if let Some(editor) = &self.editor {
            let parts: Vec<HintPart> = if editor.kind == TaskEditorKind::Step {
                vec!["enter add to the note".into(), "esc cancel".into()]
            } else if editor.field == TaskEditorField::Title {
                vec!["enter save".into(), "tab details".into(), "esc cancel".into()]
            } else {
                vec![
                    "enter new line".into(),
                    "ctrl+s save".into(),
                    "tab title".into(),
                    "esc cancel".into(),
                ]
            };
            return fit_hint(&parts, width, HINT_SEPARATOR);
        }
        if let Some(picker) = &self.picker {
            let parts: Vec<HintPart> = if picker.typing {
                vec![
                    "type to search".into(),
                    "enter done".into(),
                    "↑↓ move".into(),
                    "esc clear".into(),
                ]
            } else {
                let searched = !picker.query.is_empty();
                vec![
                    "enter move here".into(),
                    if searched { "/ edit search" } else { "/ search" }.into(),
                    hint("tab worktrees / sessions", Some("tab view"), None),
                    if searched { "esc clear search" } else { "esc cancel" }.into(),
                ]
            };
            return fit_hint(&parts, width, HINT_SEPARATOR);
        }
        let rows = self.rows(props);
        let current_row = self.current(&rows).and_then(|index| rows.get(index));
        let has_scope = self.scope.as_deref().is_some_and(|scope| !scope.is_empty());
        if self.view == TasksView::Notes {
            let on_item = matches!(current_row, Some(BoardRow::NoteLine { item: Some(_), .. }));
            let on_note = matches!(current_row, Some(BoardRow::NoteHead { .. } | BoardRow::NoteLine { .. }));
            let mut parts = Vec::new();
            if on_item {
                parts.push("a add as a task".into());
            }
            if on_note {
                parts.push(hint("A send all open items", Some("A send all"), Some(1)));
            }
            parts.push(hint("enter open the note", Some("enter open"), None));
            parts.push(hint(
                if self.items_only {
                    "f every note"
                } else {
                    "f open items only"
                },
                Some("f filter"),
                Some(1),
            ));
            if has_scope {
                parts.push(hint(
                    if self.scoped { "v all notes" } else { "v this worktree" },
                    Some("v view"),
                    Some(1),
                ));
            }
            parts.push("tab tasks".into());
            return fit_hint(&parts, width, HINT_SEPARATOR);
        }
        let add_to = self.add_section(current_row);
        let adds_step = add_to
            .as_deref()
            .is_some_and(|section| !section.is_empty() && has_main_task(section, props.tasks, None));
        // A (a backlog task) shows wherever a adds a step; E (the list in an editor) everywhere.
        let new_task = hint("A new task", Some("A task"), None);
        let edit_list = hint("E open in editor", Some("E editor"), Some(1));
        let back = hint("esc back", None, Some(2));
        if let Some(BoardRow::Step { step, .. }) = current_row {
            let parts = vec![
                hint(if step.done { "space reopen" } else { "space tick" }, None, None),
                hint("enter open the note", Some("enter note"), None),
                hint("a add step", Some("a step"), None),
                new_task,
                edit_list,
                back,
            ];
            return fit_hint(&parts, width, HINT_SEPARATOR);
        }
        if let Some(BoardRow::Empty { .. }) = current_row {
            let parts = vec![hint("a add task", Some("a add"), None), edit_list, back];
            return fit_hint(&parts, width, HINT_SEPARATOR);
        }
        let mut parts = vec![if adds_step {
            hint("a add step", Some("a step"), None)
        } else {
            hint("a add task", Some("a add"), None)
        }];
        if adds_step {
            parts.push(new_task);
        }
        parts.push(hint("n start session", Some("n start"), None));
        parts.push(hint("w move to a worktree", Some("w move"), None));
        parts.push(hint("enter edit", None, Some(1)));
        parts.push(hint("space done", Some("space ✓"), None));
        if has_scope {
            parts.push(hint(
                if self.scoped { "v all tasks" } else { "v this worktree" },
                Some("v view"),
                Some(1),
            ));
        }
        parts.push(hint("o open its session", Some("o session"), Some(1)));
        parts.push(hint("x delete", None, Some(2)));
        parts.push(hint("tab notes", None, Some(1)));
        parts.push(edit_list);
        parts.push(back);
        fit_hint(&parts, width, HINT_SEPARATOR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::event_loop::Dispatch;
    use crate::primitives::{row_text, string_width};
    use deckhand_protocol::data::TaskMeta;
    use serde_json::Value;

    const TASK_ID: &str = "a1b2c3";
    const DATE: &str = "2026-10-19";

    fn task(id: &str, title: &str, body: &str) -> Task {
        Task {
            id: id.to_owned(),
            title: title.to_owned(),
            done: false,
            body: body.to_owned(),
            meta: TaskMeta {
                t: Some(id.to_owned()),
                added: Some(DATE.to_owned()),
                ..TaskMeta::default()
            },
        }
    }

    fn props<'a>(tasks: &'a [Task], sessions: &'a [&'a SessionRecord]) -> TasksFlowProps<'a> {
        TasksFlowProps {
            client: None,
            repo_root: "<TMP>/demo",
            doc: None,
            tasks,
            sessions,
            spinner_frame: "⠋",
        }
    }

    fn cx() -> FlowCx<App, TasksFlow> {
        let (dispatch, rx) = Dispatch::<App>::channel();
        // Nothing async runs in these tests: the receiver can go.
        drop(rx);
        FlowCx::new(dispatch, |app: &mut App| &mut app.flows.tasks)
    }

    fn effects(cx: &mut FlowCx<App, TasksFlow>) -> Vec<String> {
        cx.take_effects().iter().map(|effect| format!("{effect:?}")).collect()
    }

    fn key(apply: impl FnOnce(&mut Key)) -> Input {
        let mut key = Key::NONE;
        apply(&mut key);
        Input::key("", key)
    }

    fn ctrl_s() -> Input {
        Input::key(
            "s",
            Key {
                ctrl: true,
                ..Key::NONE
            },
        )
    }

    fn press(flow: &mut TasksFlow, props: &TasksFlowProps<'_>, cx: &mut FlowCx<App, TasksFlow>, input: Input) {
        flow.handle_input(props, &input, cx);
    }

    fn typed(flow: &mut TasksFlow, props: &TasksFlowProps<'_>, cx: &mut FlowCx<App, TasksFlow>, text: &str) {
        press(flow, props, cx, Input::text(text));
    }

    // -----------------------------------------------------------------------------------------------------------
    // Goldens: the right pane of fixtures/ui (130x36: the pane is columns 32.., rows 2..33).

    const PANE_X: usize = 32;
    const PANE_Y: usize = 2;
    const PANE_W: u16 = 98;
    const PANE_H: u16 = 31;

    fn golden(name: &str) -> (Vec<String>, Value) {
        let base = format!("{}/../../fixtures/ui/{name}", env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(format!("{base}.txt")).unwrap_or_else(|error| panic!("{name}: {error}"));
        let json = std::fs::read_to_string(format!("{base}.json")).unwrap_or_else(|error| panic!("{name}: {error}"));
        let json: Value = serde_json::from_str(&json).unwrap_or_else(|error| panic!("{name}: {error}"));
        (text.lines().map(str::to_owned).collect(), json)
    }

    fn render(flow: &mut TasksFlow, props: &TasksFlowProps<'_>) -> Buffer {
        let area = Rect::new(0, 0, PANE_W, PANE_H);
        let mut buf = Buffer::empty(area);
        flow.render(props, &mut buf, area);
        buf
    }

    fn buffer_rows(buf: &Buffer) -> Vec<String> {
        (0..buf.area.height)
            .map(|y| {
                let mut line = String::new();
                let mut x = 0;
                while x < buf.area.width {
                    let symbol = buf[(x, y)].symbol();
                    line.push_str(symbol);
                    x += u16::try_from(string_width(symbol).max(1)).unwrap_or(1);
                }
                // The goldens' date normalisation keeps the width: `<DATE>` and four more spaces.
                line.replace(DATE, "<DATE>    ").trim_end().to_owned()
            })
            .collect()
    }

    /// A golden cell's style: fg palette index, bold, dim, italic, underline, inverse.
    type CellStyle = (Option<u64>, bool, bool, bool, bool, bool);

    fn golden_style(json: &Value, row: usize, column: usize) -> CellStyle {
        let mut style = (None, false, false, false, false, false);
        let Some(entry) = json["styles"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|entry| entry["row"].as_u64() == u64::try_from(row).ok())
        else {
            return style;
        };
        for run in entry["runs"].as_array().into_iter().flatten() {
            let bound = |name: &str| usize::try_from(run[name].as_u64().unwrap_or(0)).unwrap_or(0);
            if (bound("start")..bound("end")).contains(&column) {
                style = (
                    run["fg"].as_u64(),
                    run["bold"].as_bool().unwrap_or(false),
                    run["dim"].as_bool().unwrap_or(false),
                    run["italic"].as_bool().unwrap_or(false),
                    run["underline"].as_bool().unwrap_or(false),
                    run["inverse"].as_bool().unwrap_or(false),
                );
            }
        }
        style
    }

    fn palette(color: Color) -> Option<u64> {
        Some(match color {
            Color::Black => 0,
            Color::Red => 1,
            Color::Green => 2,
            Color::Yellow => 3,
            Color::Blue => 4,
            Color::Magenta => 5,
            Color::Cyan => 6,
            Color::Gray => 7,
            Color::DarkGray => 8,
            Color::LightRed => 9,
            Color::LightGreen => 10,
            Color::LightYellow => 11,
            Color::LightBlue => 12,
            Color::LightMagenta => 13,
            Color::LightCyan => 14,
            Color::White => 15,
            Color::Indexed(index) => u64::from(index),
            _ => return None,
        })
    }

    fn cell_style(buf: &Buffer, x: u16, y: u16) -> CellStyle {
        use ratatui::style::Modifier;
        let cell = &buf[(x, y)];
        let modifier = cell.modifier;
        (
            palette(cell.fg),
            modifier.contains(Modifier::BOLD),
            modifier.contains(Modifier::DIM),
            modifier.contains(Modifier::ITALIC),
            modifier.contains(Modifier::UNDERLINED),
            modifier.contains(Modifier::REVERSED),
        )
    }

    /// The pane rendered by `flow` matches the golden's pane, text and styles.
    fn assert_pane(name: &str, flow: &mut TasksFlow, props: &TasksFlowProps<'_>) {
        let (rows, json) = golden(name);
        let buf = render(flow, props);
        let got = buffer_rows(&buf);
        for (index, got) in got.iter().enumerate() {
            let want: String = rows[PANE_Y + index].chars().skip(PANE_X).collect();
            assert_eq!(got, want.trim_end(), "{name} pane row {index}");
        }
        for y in 0..PANE_H {
            // The four columns a normalised date gives back to the row lost its style in the golden.
            let pane_row: Vec<char> = rows[PANE_Y + usize::from(y)].chars().skip(PANE_X).collect();
            let date_pad = pane_row
                .windows(6)
                .position(|window| window.iter().collect::<String>() == "<DATE>")
                .map(|at| at + 6..at + 10);
            for x in 0..PANE_W {
                if date_pad.as_ref().is_some_and(|pad| pad.contains(&usize::from(x))) {
                    continue;
                }
                let want = golden_style(&json, PANE_Y + usize::from(y), PANE_X + usize::from(x));
                assert_eq!(cell_style(&buf, x, y), want, "{name} style at pane ({x}, {y})");
            }
        }
    }

    /// The footer hint matches the golden's hint row (row 33).
    fn assert_hint(name: &str, flow: &TasksFlow, props: &TasksFlowProps<'_>) {
        let (rows, _) = golden(name);
        assert_eq!(flow.hint(props, 130), rows[33].trim_end(), "{name} hint");
    }

    fn editor(kind: TaskEditorKind, field: TaskEditorField, title: &str, body: &str) -> TaskEditor {
        TaskEditor {
            kind,
            id: (kind == TaskEditorKind::Edit).then(|| TASK_ID.to_owned()),
            section: None,
            field,
            title: EditorState {
                text: title.to_owned(),
                cursor: title.len(),
                ..EditorState::default()
            },
            body: EditorState {
                text: body.to_owned(),
                cursor: body.len(),
                ..EditorState::default()
            },
        }
    }

    #[test]
    fn the_empty_board_matches_the_golden() {
        let mut flow = TasksFlow::default();
        let props = props(&[], &[]);
        flow.open(None, &props);
        assert_pane("15-tasks-empty", &mut flow, &props);
        assert_hint("15-tasks-empty", &flow, &props);
    }

    #[test]
    fn the_new_task_editor_matches_the_goldens() {
        let mut flow = TasksFlow::default();
        let props = props(&[], &[]);
        flow.editor = Some(editor(
            TaskEditorKind::Add,
            TaskEditorField::Title,
            "Write the docs",
            "",
        ));
        assert_pane("16-task-new", &mut flow, &props);
        assert_hint("16-task-new", &flow, &props);
        flow.editor = Some(editor(
            TaskEditorKind::Add,
            TaskEditorField::Body,
            "Write the docs",
            "why: people ask",
        ));
        assert_pane("17-task-new-details", &mut flow, &props);
        assert_hint("17-task-new-details", &flow, &props);
    }

    #[test]
    fn the_board_with_a_task_matches_the_goldens() {
        let tasks = [task(TASK_ID, "Write the docs", "why: people ask")];
        let props = props(&tasks, &[]);
        let mut flow = TasksFlow {
            selected: RowSelection::task(TASK_ID),
            ..TasksFlow::default()
        };
        assert_pane("18-tasks-board", &mut flow, &props);
        assert_hint("18-tasks-board", &flow, &props);
        // Enter: the edit form, filled in.
        let mut cx = cx();
        press(&mut flow, &props, &mut cx, key(|key| key.return_ = true));
        assert_eq!(
            flow.editor,
            Some(editor(
                TaskEditorKind::Edit,
                TaskEditorField::Title,
                "Write the docs",
                "why: people ask"
            ))
        );
        assert_pane("19-task-edit", &mut flow, &props);
        assert_hint("19-task-edit", &flow, &props);
        // Esc, then w: the move picker, on the backlog where it is.
        press(&mut flow, &props, &mut cx, key(|key| key.escape = true));
        assert_eq!(flow.editor, None);
        typed(&mut flow, &props, &mut cx, "w");
        assert_eq!(
            flow.picker,
            Some(TaskPicker {
                task_id: TASK_ID.to_owned(),
                title: "Write the docs".to_owned(),
                current: None,
                origin: None,
                view: PickerView::Worktrees,
                query: String::new(),
                typing: false,
                index: 0,
            })
        );
        assert_pane("20-task-move", &mut flow, &props);
        assert_hint("20-task-move", &flow, &props);
        // Esc, then tab: the notes view (no notes yet).
        press(&mut flow, &props, &mut cx, key(|key| key.escape = true));
        assert_eq!(flow.picker, None);
        press(&mut flow, &props, &mut cx, key(|key| key.tab = true));
        assert_eq!(flow.view, TasksView::Notes);
        assert_pane("21-tasks-notes", &mut flow, &props);
        assert_hint("21-tasks-notes", &flow, &props);
        assert!(effects(&mut cx).is_empty());
    }

    // -----------------------------------------------------------------------------------------------------------
    // The TS board (recorded once from src/tasksFlow.tsx rendered by Ink with the same repository and keys).

    /// (label, input, key as Ink JSON, hint, callbacks, frame rows).
    type TsStep = (
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static [&'static str],
        &'static [&'static str],
    );

    /// The TS board (tasksFlow.tsx rendered by Ink at 98x31) after each step: the label, the input and key,
    /// the hint at 98 columns, the callbacks it made, and the frame rows checked (all, or the last 14).
    const TS_STEPS: &[TsStep] = &[
        (
            "start",
            "",
            "{}",
            "a step • A task • n start • w move • enter edit • space ✓ • o session • tab notes • E editor",
            &[],
            &[
                "╭────────────────────────────────────────────────────────────────────────────────────────────────╮",
                "│ ☐ Tasks · repo                                                          5 open · 3 in progress │",
                "│                                                                                                │",
                "│ › ◆ Ship the feature                                                       ⎇ feat  ● idle  1/3 │",
                "│     ☐ step one                                                                                 │",
                "│     ☑ step two                                                                                 │",
                "│     ☐ ship it                                                                                  │",
                "│ main checkout · beta · 2                                                           ? needs you │",
                "│   ◆ Check the logs                                                                         0/2 │",
                "│     ☐ check logs                                                                               │",
                "│     ☐ Second item ↗ <!-- dh:t=zz99 -->                                                         │",
                "│   ◆ Second for beta                                                                            │",
                "│ BACKLOG · 2                                                                                    │",
                "│   ○ Tried before                                                         tried in ⎇ old-branch │",
                "│   ○ Plain backlog                                                             added 2026-01-02 │",
                "│ DONE · this week                                                                               │",
                "│   ▸ 1 older done · enter shows them                                                            │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│ ────────────────────────────────────────────────────────────────────────────────────────────── │",
                "│ Ship the feature                                                                               │",
                "│ The task of ⎇ feat: merging or marking it done ticks it · o opens it                           │",
                "│   the body line one                                                                            │",
                "│   second line of the body which is quite long so it wraps around the details area width more   │",
                "│   than once for sure yes …                                                                     │",
                "╰────────────────────────────────────────────────────────────────────────────────────────────────╯",
            ],
        ),
        (
            "picker w1",
            "w",
            "{}",
            "enter move here • / search • tab worktrees / sessions • esc cancel",
            &[],
            &[],
        ),
        (
            "picker tab",
            "",
            "{\"tab\": true}",
            "enter move here • / search • tab worktrees / sessions • esc cancel",
            &[],
            &[],
        ),
        (
            "picker down",
            "",
            "{\"downArrow\": true}",
            "enter move here • / search • tab worktrees / sessions • esc cancel",
            &[],
            &[],
        ),
        (
            "picker search",
            "/",
            "{}",
            "type to search • enter done • ↑↓ move • esc clear",
            &[],
            &[],
        ),
        (
            "picker typed",
            "be",
            "{}",
            "type to search • enter done • ↑↓ move • esc clear",
            &[],
            &[
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│ ────────────────────────────────────────────────────────────────────────────────────────────── │",
                "│ Move “Ship the feature” to                                                                     │",
                "│  Worktrees 0   Sessions 1    search: be▏                                                       │",
                "│   Backlog · no worktree                                                                        │",
                "│ › main checkout · beta                                                    → a step in its note │",
                "╰────────────────────────────────────────────────────────────────────────────────────────────────╯",
            ],
        ),
        (
            "picker done",
            "",
            "{\"return\": true}",
            "enter move here • / edit search • tab worktrees / sessions • esc clear search",
            &[],
            &[],
        ),
        (
            "picker esc",
            "",
            "{\"escape\": true}",
            "enter move here • / search • tab worktrees / sessions • esc cancel",
            &[],
            &[],
        ),
        (
            "picker esc2",
            "",
            "{\"escape\": true}",
            "a step • A task • n start • w move • enter edit • space ✓ • o session • tab notes • E editor",
            &[],
            &[],
        ),
        (
            "j1",
            "j",
            "{}",
            "space tick • enter open the note • a add step • A new task • E open in editor • esc back",
            &[],
            &[],
        ),
        (
            "j2",
            "j",
            "{}",
            "space reopen • enter open the note • a add step • A new task • E open in editor • esc back",
            &[],
            &[],
        ),
        (
            "j3",
            "j",
            "{}",
            "space tick • enter open the note • a add step • A new task • E open in editor • esc back",
            &[],
            &[],
        ),
        (
            "j4",
            "j",
            "{}",
            "a step • A task • n start • w move • enter edit • space ✓ • o session • tab notes • E editor",
            &[],
            &[
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│ ────────────────────────────────────────────────────────────────────────────────────────────── │",
                "│ Check the logs                                                                                 │",
                "│ The task of main checkout · beta: merging or marking it done ticks it · o opens it             │",
                "│ Which database should I use for the cache layer?                                               │",
                "╰────────────────────────────────────────────────────────────────────────────────────────────────╯",
            ],
        ),
        (
            "o",
            "o",
            "{}",
            "a step • A task • n start • w move • enter edit • space ✓ • o session • tab notes • E editor",
            &["goto s2"],
            &[],
        ),
        (
            "n",
            "n",
            "{}",
            "a step • A task • n start • w move • enter edit • space ✓ • o session • tab notes • E editor",
            &["status That task is already being worked on (o opens its session)"],
            &[],
        ),
        (
            "x",
            "x",
            "{}",
            "a step • A task • n start • w move • enter edit • space ✓ • o session • tab notes • E editor",
            &["status x again deletes “Check the logs”"],
            &[],
        ),
        (
            "J",
            "J",
            "{}",
            "a step • A task • n start • w move • enter edit • space ✓ • o session • tab notes • E editor",
            &[],
            &[],
        ),
        (
            "w s2",
            "w",
            "{}",
            "enter move here • / search • tab worktrees / sessions • esc cancel",
            &[],
            &[
                "╭────────────────────────────────────────────────────────────────────────────────────────────────╮",
                "│ ☐ Tasks · repo                                                          5 open · 3 in progress │",
                "│                                                                                                │",
                "│   ◆ Ship the feature                                                       ⎇ feat  ● idle  1/3 │",
                "│     ☐ step one                                                                                 │",
                "│     ☑ step two                                                                                 │",
                "│     ☐ ship it                                                                                  │",
                "│ main checkout · beta · 2                                                           ? needs you │",
                "│   ◆ Check the logs                                                                         0/2 │",
                "│     ☐ check logs                                                                               │",
                "│     ☐ Second item ↗ <!-- dh:t=zz99 -->                                                         │",
                "│   ◆ Second for beta                                                                            │",
                "│ BACKLOG · 2                                                                                    │",
                "│   ○ Tried before                                                         tried in ⎇ old-branch │",
                "│   ○ Plain backlog                                                             added 2026-01-02 │",
                "│ DONE · this week                                                                               │",
                "│   ▸ 1 older done · enter shows them                                                            │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│ ────────────────────────────────────────────────────────────────────────────────────────────── │",
                "│ Move “Check the logs” to                                                                       │",
                "│  Worktrees 1   Sessions 1    / search                                                          │",
                "│   Backlog · no worktree                                                                        │",
                "│ › main checkout · beta                                                                    here │",
                "╰────────────────────────────────────────────────────────────────────────────────────────────────╯",
            ],
        ),
        (
            "w s2 esc",
            "",
            "{\"escape\": true}",
            "a step • A task • n start • w move • enter edit • space ✓ • o session • tab notes • E editor",
            &[],
            &[],
        ),
        (
            "a step editor",
            "a",
            "{}",
            "enter add to the note • esc cancel",
            &[],
            &[
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│ ────────────────────────────────────────────────────────────────────────────────────────────── │",
                "│ New step · the note of main checkout · beta                                                    │",
                "│ Title                                                                                          │",
                "│ Details (optional; typed into the agent's input with the title)                                │",
                "│                                                                                                │",
                "╰────────────────────────────────────────────────────────────────────────────────────────────────╯",
            ],
        ),
        (
            "typed",
            "hello world",
            "{}",
            "enter add to the note • esc cancel",
            &[],
            &[],
        ),
        (
            "tab in step",
            "",
            "{\"tab\": true}",
            "enter add to the note • esc cancel",
            &[],
            &[],
        ),
        (
            "esc",
            "",
            "{\"escape\": true}",
            "a step • A task • n start • w move • enter edit • space ✓ • o session • tab notes • E editor",
            &[],
            &[],
        ),
        ("A editor", "A", "{}", "enter save • tab details • esc cancel", &[], &[]),
        (
            "tab body",
            "",
            "{\"tab\": true}",
            "enter new line • ctrl+s save • tab title • esc cancel",
            &[],
            &[],
        ),
        (
            "body typed",
            "line one",
            "{}",
            "enter new line • ctrl+s save • tab title • esc cancel",
            &[],
            &[
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│ ────────────────────────────────────────────────────────────────────────────────────────────── │",
                "│ New task · backlog                                                                             │",
                "│ Title                                                                                          │",
                "│ Details                                                                                        │",
                "│   line one                                                                                     │",
                "╰────────────────────────────────────────────────────────────────────────────────────────────────╯",
            ],
        ),
        (
            "esc2",
            "",
            "{\"escape\": true}",
            "a step • A task • n start • w move • enter edit • space ✓ • o session • tab notes • E editor",
            &[],
            &[],
        ),
        (
            "G",
            "G",
            "{}",
            "a add • n start • w move • enter edit • space ✓ • o session • x delete • tab notes • E editor",
            &[],
            &[],
        ),
        (
            "older",
            "",
            "{\"return\": true}",
            "a add • n start • w move • enter edit • space ✓ • o session • x delete • tab notes • E editor",
            &[],
            &[],
        ),
        (
            "G2",
            "G",
            "{}",
            "a add • n start • w move • enter edit • space ✓ • o session • x delete • tab notes • E editor",
            &[],
            &[
                "╭────────────────────────────────────────────────────────────────────────────────────────────────╮",
                "│ ☐ Tasks · repo                                                          5 open · 3 in progress │",
                "│                                                                                                │",
                "│   ◆ Ship the feature                                                       ⎇ feat  ● idle  1/3 │",
                "│     ☐ step one                                                                                 │",
                "│     ☑ step two                                                                                 │",
                "│     ☐ ship it                                                                                  │",
                "│ main checkout · beta · 2                                                           ? needs you │",
                "│   ◆ Check the logs                                                                         0/2 │",
                "│     ☐ check logs                                                                               │",
                "│     ☐ Second item ↗ <!-- dh:t=zz99 -->                                                         │",
                "│   ◆ Second for beta                                                                            │",
                "│ BACKLOG · 2                                                                                    │",
                "│   ○ Tried before                                                         tried in ⎇ old-branch │",
                "│   ○ Plain backlog                                                             added 2026-01-02 │",
                "│ DONE · this week                                                                               │",
                "│   ▾ 1 older done · enter hides them                                                            │",
                "│ › ✓ Finished                                                                2026-01-01  merged │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│ ────────────────────────────────────────────────────────────────────────────────────────────── │",
                "│ Finished                                                                                       │",
                "│ merged 2026-01-01 · space reopens it                                                           │",
                "╰────────────────────────────────────────────────────────────────────────────────────────────────╯",
            ],
        ),
        (
            "k on older",
            "k",
            "{}",
            "a add • n start • w move • enter edit • space ✓ • o session • x delete • tab notes • E editor",
            &[],
            &[],
        ),
        (
            "v",
            "v",
            "{}",
            "a add • n start • w move • enter edit • space ✓ • o session • x delete • tab notes • E editor",
            &["status Select a session first: v shows the tasks of its worktree"],
            &[],
        ),
        (
            "notes",
            "",
            "{\"tab\": true}",
            "A send all open items • enter open the note • f open items only • tab tasks",
            &[],
            &[
                "╭────────────────────────────────────────────────────────────────────────────────────────────────╮",
                "│ ✎ Notes · repo                                                                    4 open items │",
                "│                                                                                                │",
                "│ ⎇ feat · 2                                                                              ● idle │",
                "│ › Worktree note                                                                         2 open │",
                "│     Plan                                                                                       │",
                "│     ☐ step one                                                                                 │",
                "│     ☑ step two                                                                                 │",
                "│     ☐ ship it                                                                                  │",
                "│ main checkout · 2                                                                              │",
                "│   Beta                                                                                  2 open │",
                "│     ☐ check logs                                                                               │",
                "│     plain line                                                                                 │",
                "│     ☐ Second item ↗ <!-- dh:t=zz99 -->                                                         │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│ ────────────────────────────────────────────────────────────────────────────────────────────── │",
                "│ The worktree note · ⎇ feat                                                                     │",
                "│ 2 open items · enter opens it in its session’s Notes tab                                       │",
                "╰────────────────────────────────────────────────────────────────────────────────────────────────╯",
            ],
        ),
        (
            "nj",
            "j",
            "{}",
            "A send all open items • enter open the note • f open items only • tab tasks",
            &[],
            &[],
        ),
        (
            "nj2",
            "j",
            "{}",
            "a add as a task • A send all open items • enter open the note • f open items only • tab tasks",
            &[],
            &[],
        ),
        (
            "nj3",
            "j",
            "{}",
            "A send all open items • enter open the note • f open items only • tab tasks",
            &[],
            &[
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│ ────────────────────────────────────────────────────────────────────────────────────────────── │",
                "│ ☑ step two                                                                                     │",
                "│ The worktree note · ⎇ feat · enter opens the note                                              │",
                "╰────────────────────────────────────────────────────────────────────────────────────────────────╯",
            ],
        ),
        (
            "nj4",
            "j",
            "{}",
            "a add as a task • A send all open items • enter open the note • f open items only • tab tasks",
            &[],
            &[],
        ),
        (
            "nj5",
            "j",
            "{}",
            "A send all open items • enter open the note • f open items only • tab tasks",
            &[],
            &[],
        ),
        (
            "nj6",
            "j",
            "{}",
            "a add as a task • A send all open items • enter open the note • f open items only • tab tasks",
            &[],
            &[],
        ),
        (
            "nj7",
            "j",
            "{}",
            "A send all open items • enter open the note • f open items only • tab tasks",
            &[],
            &[],
        ),
        (
            "items only",
            "f",
            "{}",
            "A send all open items • enter open the note • f every note • tab tasks",
            &[],
            &[
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│                                                                                                │",
                "│ ────────────────────────────────────────────────────────────────────────────────────────────── │",
                "│ The worktree note · ⎇ feat                                                                     │",
                "│ 2 open items · enter opens it in its session’s Notes tab                                       │",
                "╰────────────────────────────────────────────────────────────────────────────────────────────────╯",
            ],
        ),
        (
            "ij",
            "j",
            "{}",
            "a add as a task • A send all open items • enter open the note • f every note • tab tasks",
            &[],
            &[],
        ),
        (
            "enter note",
            "",
            "{\"return\": true}",
            "a add as a task • A send all open items • enter open the note • f every note • tab tasks",
            &["note s1"],
            &[],
        ),
    ];

    const TS_BANNER: &[&str] = &[
        "◆ Task Ship the…   1/3 steps · +1 open here · idle · b board",
        "────────────────────────────────────────────────────────────",
    ];

    /// The repository of TS_STEPS: a worktree session with a note checklist and a main-checkout session that
    /// needs input, with notes of its own.
    const TS_SESSIONS: &str = r##"[{"id": "s1", "title": "alpha", "program": "claude", "command": "claude", "cwd": "/repo/wt/feat", "repoRoot": "/repo", "status": "running", "agentStatus": "idle", "createdAt": "2026-01-01T00:00:00.000Z", "updatedAt": "2026-01-01T00:00:00.000Z", "worktree": {"mode": "managed", "path": "/repo/wt/feat", "branch": "feat", "id": "w1"}, "sharedNotes": {"kind": "worktree", "id": "w1", "path": "/n/w1.md", "text": "# Plan\n- [ ] step one\n- [x] step two\n- [ ] ship it\n", "revision": "r1"}}, {"id": "s2", "title": "beta", "program": "claude", "command": "claude", "cwd": "/repo", "repoRoot": "/repo", "status": "running", "agentStatus": "active", "createdAt": "2026-01-02T00:00:00.000Z", "updatedAt": "2026-01-02T00:00:00.000Z", "notes": "- [ ] check logs\nplain line\n- [ ] Second item ↗ <!-- dh:t=zz99 -->\n", "notesFile": {"path": "/n/s2.md", "revision": "n1"}, "attention": {"state": "needs-input", "event": "x", "at": "2026-01-02T00:00:00.000Z", "reason": {"kind": "question", "text": "Which database should I use for the cache layer?"}}}]"##;

    const TS_TASKS: &str = "- [ ] Ship the feature <!-- dh:t=aa01 wt=w1 -->\n  the body line one\n  second line of the body which is quite long so it wraps around the details area width more than once for sure yes\n  third\n  fourth\n- [ ] Check the logs <!-- dh:t=bb02 s=s2 -->\n- [ ] Second for beta <!-- dh:t=cc03 s=s2 -->\n- [ ] Tried before <!-- dh:t=dd04 tried=old-branch -->\n- [ ] Plain backlog <!-- dh:t=ee05 added=2026-01-02 -->\n- [x] Finished <!-- dh:t=ff06 done=2026-01-01 auto=merge -->\n";

    /// An effect as the TS scenario logged the callback.
    fn ts_log(effect: &FlowEffect<App>) -> String {
        match effect {
            FlowEffect::Exit => "exit".to_owned(),
            FlowEffect::StartTask(task) => format!("start {}", task.id),
            FlowEffect::GoTo(id) => format!("goto {id}"),
            FlowEffect::OpenNote(id) => format!("note {id}"),
            FlowEffect::TasksDoc(_) => "doc".to_owned(),
            FlowEffect::Session(_) => "session".to_owned(),
            FlowEffect::SetError(error) => format!("error {}", error.as_deref().unwrap_or("undefined")),
            FlowEffect::SetStatus(status) => format!("status {}", status.as_deref().unwrap_or("undefined")),
            other => format!("{other:?}"),
        }
    }

    #[test]
    fn the_board_renders_and_answers_keys_like_the_ts() {
        let sessions: Vec<SessionRecord> = serde_json::from_str(TS_SESSIONS).unwrap_or_else(|error| panic!("{error}"));
        let refs: Vec<&SessionRecord> = sessions.iter().collect();
        let tasks = deckhand_core::tasks::parse_tasks(TS_TASKS);
        let doc = TasksDoc {
            key: "k".to_owned(),
            path: "/repo/tasks.md".to_owned(),
            text: TS_TASKS.to_owned(),
            revision: "r".to_owned(),
            too_large: None,
        };
        let props = TasksFlowProps {
            client: None,
            repo_root: "/repo",
            doc: Some(&doc),
            tasks: &tasks,
            sessions: &refs,
            spinner_frame: "⠋",
        };
        let mut flow = TasksFlow::default();
        let mut cx = cx();
        for (label, input, key, hint, log, rows) in TS_STEPS {
            let key: Key = serde_json::from_str(key).unwrap_or_else(|error| panic!("{error}"));
            if !input.is_empty() || key != Key::NONE {
                flow.handle_input(&props, &Input::key(*input, key), &mut cx);
            }
            let logged: Vec<String> = cx.take_effects().iter().map(ts_log).collect();
            assert_eq!(logged, *log, "{label}: callbacks");
            let area = Rect::new(0, 0, 98, 31);
            let mut buf = Buffer::empty(area);
            flow.render(&props, &mut buf, area);
            let got = buffer_rows(&buf);
            let shown = if rows.len() == got.len() {
                &got[..]
            } else {
                &got[got.len() - rows.len()..]
            };
            assert_eq!(shown, *rows, "{label}: frame");
            assert_eq!(flow.hint(&props, 98), *hint, "{label}: hint");
        }
        let banner = task_banner_rows(
            &TaskBannerProps {
                task: &tasks[0],
                more: 1,
                steps: Some(StepCount { done: 1, total: 3 }),
                sessions: &refs,
                spinner_frame: "⠋",
            },
            60,
        );
        let banner: Vec<String> = banner.iter().map(|row| row_text(row).trim_end().to_owned()).collect();
        assert_eq!(banner, TS_BANNER);
    }

    // -----------------------------------------------------------------------------------------------------------
    // Pure helpers

    #[test]
    fn capitalize_upper_cases_the_first_code_unit() {
        assert_eq!(capitalize(""), "");
        assert_eq!(capitalize("worktree note"), "Worktree note");
        assert_eq!(capitalize("ßeta"), "SSeta");
        // An astral first character is half a surrogate pair to JS: left as it is.
        assert_eq!(capitalize("𐐨x"), "𐐨x");
    }

    #[test]
    fn parse_added_finds_the_new_id() {
        let doc = TasksDoc {
            key: "k".to_owned(),
            path: "/tasks.md".to_owned(),
            text: "- [ ] A <!-- dh:t=aa -->\n- [ ] B <!--\tdh:t=bb added=1 -->\n- [ ] C <!--dh:t=XY -->\n".to_owned(),
            revision: "r".to_owned(),
            too_large: None,
        };
        let before: HashSet<String> = ["aa".to_owned()].into_iter().collect();
        assert_eq!(parse_added(&doc, &before), Some("bb".to_owned()));
        let all: HashSet<String> = ["aa".to_owned(), "bb".to_owned()].into_iter().collect();
        assert_eq!(parse_added(&doc, &all), None);
    }

    #[test]
    fn neighbors_stay_within_their_group() {
        let a = task("aa", "A", "");
        let b = task("bb", "B", "");
        let row = |task: &Task, section: &str| BoardRow::Task {
            task: task.clone(),
            group: TaskGroup::Backlog,
            section: section.to_owned(),
            steps: None,
            heads_work: None,
        };
        let rows = vec![
            BoardRow::Heading {
                text: "BACKLOG".to_owned(),
                count: None,
            },
            row(&a, "backlog"),
            row(&b, "backlog"),
            row(&a, "done"),
        ];
        let id = |task: Option<&Task>| task.map(|task| task.id.clone());
        assert_eq!(id(neighbor_in_group(&rows, 1, 1, "backlog")), Some("bb".to_owned()));
        assert_eq!(id(neighbor_in_group(&rows, 2, -1, "backlog")), Some("aa".to_owned()));
        assert_eq!(neighbor_in_group(&rows, 1, -1, "backlog"), None);
        assert_eq!(neighbor_in_group(&rows, 2, 1, "backlog"), None);
        assert_eq!(neighbor_in_group(&rows, 0, -1, "backlog"), None);
        assert_eq!(neighbor_in_group(&rows, 3, 1, "done"), None);
    }

    #[test]
    fn editor_options_match_the_ts_limits() {
        assert_eq!(title_options().max_chars, Some(MAX_TASK_TITLE));
        assert_eq!(
            title_options().limit_message.as_deref(),
            Some("A title is at most 300 characters")
        );
        assert_eq!(
            body_options().limit_message.as_deref(),
            Some("A task's details are at most 4000 characters")
        );
        assert_eq!(body_options().select_all, Some(false));
        assert_eq!(body_options().tab, Some(false));
    }

    #[test]
    fn the_task_banner_names_the_task_and_the_board_key() {
        let task = task(TASK_ID, "Write the docs", "");
        let props = TaskBannerProps {
            task: &task,
            more: 2,
            steps: Some(StepCount { done: 1, total: 3 }),
            sessions: &[],
            spinner_frame: "⠋",
        };
        let rows = task_banner_rows(&props, 80);
        assert_eq!(rows.len(), 2);
        let first = row_text(&rows[0]);
        assert!(first.starts_with("◆ Task Write the docs"), "{first}");
        assert!(
            first.ends_with(&format!("1/3 steps · +2 open here · added {DATE} · b board")),
            "{first}"
        );
        assert_eq!(string_width(&first), 80);
        assert_eq!(row_text(&rows[1]), "─".repeat(80));
        let no_steps = TaskBannerProps {
            more: 0,
            steps: Some(StepCount { done: 0, total: 0 }),
            ..props
        };
        assert!(row_text(&task_banner_rows(&no_steps, 80)[0]).ends_with(&format!("added {DATE} · b board")));
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 2));
        render_task_banner(&props, &mut buf, Rect::new(0, 0, 40, 2));
        assert_eq!(buf[(0, 1)].symbol(), "─");
    }

    #[test]
    fn the_keymap_is_in_the_tasks_topic() {
        assert!(crate::keymap::undocumented_in_topic(KEYMAP, "Tasks").is_empty());
        assert!(crate::keymap::unknown_topics(KEYMAP).is_empty());
    }

    // -----------------------------------------------------------------------------------------------------------
    // Keys

    #[test]
    fn the_editor_adds_to_the_backlog_and_checks_the_title() {
        let mut flow = TasksFlow::default();
        let props = props(&[], &[]);
        let mut cx = cx();
        typed(&mut flow, &props, &mut cx, "a");
        let state = flow.editor.clone().unwrap_or_else(|| panic!("the editor"));
        assert_eq!(
            (state.kind, state.section, state.field),
            (TaskEditorKind::Add, None, TaskEditorField::Title)
        );
        // An empty title is refused.
        press(&mut flow, &props, &mut cx, key(|key| key.return_ = true));
        assert_eq!(effects(&mut cx), ["SetError(Some(\"A task needs a title\"))"]);
        // Typed text (a pasted newline becomes a space); arrows do nothing in the title.
        typed(&mut flow, &props, &mut cx, "Fix\nit");
        press(&mut flow, &props, &mut cx, key(|key| key.up_arrow = true));
        assert_eq!(
            flow.editor.as_ref().map(|state| state.title.text.as_str()),
            Some("Fix it")
        );
        // Tab: the details; Enter there is a new line.
        press(&mut flow, &props, &mut cx, key(|key| key.tab = true));
        typed(&mut flow, &props, &mut cx, "one");
        press(&mut flow, &props, &mut cx, key(|key| key.return_ = true));
        typed(&mut flow, &props, &mut cx, "two");
        let state = flow.editor.clone().unwrap_or_else(|| panic!("the editor"));
        assert_eq!(state.field, TaskEditorField::Body);
        assert_eq!(state.body.text, "one\ntwo");
        assert_eq!(
            flow.hint(&props, 130),
            "enter new line • ctrl+s save • tab title • esc cancel"
        );
        // Ctrl+S saves: without a daemon, the error says so.
        press(&mut flow, &props, &mut cx, ctrl_s());
        assert_eq!(effects(&mut cx), ["SetError(Some(\"still connecting to daemon\"))"]);
        assert!(flow.editor.is_some());
        // Esc closes it.
        press(&mut flow, &props, &mut cx, key(|key| key.escape = true));
        assert!(flow.editor.is_none());
        // A: always a backlog task.
        typed(&mut flow, &props, &mut cx, "A");
        assert_eq!(flow.editor.as_ref().map(|state| state.kind), Some(TaskEditorKind::Add));
    }

    #[test]
    fn the_title_limit_is_reported() {
        let mut flow = TasksFlow::default();
        let props = props(&[], &[]);
        let mut cx = cx();
        typed(&mut flow, &props, &mut cx, "a");
        typed(&mut flow, &props, &mut cx, &"x".repeat(MAX_TASK_TITLE + 1));
        assert_eq!(
            effects(&mut cx),
            ["SetError(Some(\"A title is at most 300 characters\"))"]
        );
        assert_eq!(flow.editor.as_ref().map(|state| state.title.text.as_str()), Some(""));
    }

    #[test]
    fn task_keys_on_a_backlog_task() {
        let tasks = [task(TASK_ID, "Write the docs", ""), task("bb", "Second", "")];
        let props = props(&tasks, &[]);
        let mut flow = TasksFlow::default();
        let mut cx = cx();
        // The first selectable row is the first backlog task.
        let rows = flow.rows(&props);
        let current = flow.current(&rows).unwrap_or_else(|| panic!("a selection"));
        assert_eq!(row_key(&rows[current]).as_deref(), Some("task:a1b2c3"));
        typed(&mut flow, &props, &mut cx, "j");
        assert_eq!(flow.selected.key.as_deref(), Some("task:bb"));
        typed(&mut flow, &props, &mut cx, "g");
        assert_eq!(flow.selected.key.as_deref(), Some("task:a1b2c3"));
        typed(&mut flow, &props, &mut cx, "G");
        assert_eq!(flow.selected.key.as_deref(), Some("task:bb"));
        typed(&mut flow, &props, &mut cx, "k");
        assert_eq!(flow.selected, RowSelection::task(TASK_ID));
        typed(&mut flow, &props, &mut cx, "n");
        typed(&mut flow, &props, &mut cx, "o");
        typed(&mut flow, &props, &mut cx, "v");
        assert_eq!(
            effects(&mut cx),
            [
                "StartTask(\"a1b2c3\")",
                "SetStatus(Some(\"That task has no session; n starts one\"))",
                "SetStatus(Some(\"Select a session first: v shows the tasks of its worktree\"))",
            ]
        );
        // x asks first; another key forgets it; x twice deletes (no daemon here).
        typed(&mut flow, &props, &mut cx, "x");
        assert_eq!(flow.pending_delete.as_deref(), Some(TASK_ID));
        typed(&mut flow, &props, &mut cx, "z");
        assert_eq!(flow.pending_delete, None);
        typed(&mut flow, &props, &mut cx, "x");
        typed(&mut flow, &props, &mut cx, "x");
        assert_eq!(
            effects(&mut cx),
            [
                "SetStatus(Some(\"x again deletes “Write the docs”\"))",
                "SetStatus(Some(\"x again deletes “Write the docs”\"))",
                "SetError(Some(\"still connecting to daemon\"))",
            ]
        );
        // The picker: Enter on the backlog where it is already.
        typed(&mut flow, &props, &mut cx, "w");
        assert!(flow.picker.is_some());
        press(&mut flow, &props, &mut cx, key(|key| key.return_ = true));
        assert_eq!(flow.picker, None);
        assert_eq!(effects(&mut cx), ["SetStatus(Some(\"It is already there\"))"]);
        // Esc leaves the board.
        press(&mut flow, &props, &mut cx, key(|key| key.escape = true));
        assert_eq!(effects(&mut cx), ["Exit"]);
    }

    #[test]
    fn the_picker_searches_and_switches_views() {
        let tasks = [task(TASK_ID, "Write the docs", "")];
        let props = props(&tasks, &[]);
        let mut flow = TasksFlow::default();
        let mut cx = cx();
        typed(&mut flow, &props, &mut cx, "w");
        typed(&mut flow, &props, &mut cx, "/");
        assert_eq!(flow.picker.as_ref().map(|picker| picker.typing), Some(true));
        assert_eq!(
            flow.hint(&props, 130),
            "type to search • enter done • ↑↓ move • esc clear"
        );
        typed(&mut flow, &props, &mut cx, "do");
        typed(&mut flow, &props, &mut cx, "c\u{1b}[A");
        assert_eq!(flow.picker.as_ref().map(|picker| picker.query.as_str()), Some("doc"));
        press(&mut flow, &props, &mut cx, key(|key| key.backspace = true));
        assert_eq!(flow.picker.as_ref().map(|picker| picker.query.as_str()), Some("do"));
        press(&mut flow, &props, &mut cx, key(|key| key.return_ = true));
        let picker = flow.picker.clone().unwrap_or_else(|| panic!("the picker"));
        assert_eq!((picker.typing, picker.query.as_str()), (false, "do"));
        assert_eq!(
            flow.hint(&props, 130),
            "enter move here • / edit search • tab worktrees / sessions • esc clear search"
        );
        let buf = render(&mut flow, &props);
        let rows = buffer_rows(&buf);
        assert!(rows.iter().any(|row| row.contains("search: do")), "{rows:#?}");
        assert!(
            rows.iter().any(|row| row.contains("Nothing in worktrees matches “do”")),
            "{rows:#?}"
        );
        typed(&mut flow, &props, &mut cx, "l");
        assert_eq!(
            flow.picker.as_ref().map(|picker| picker.view),
            Some(PickerView::Sessions)
        );
        press(&mut flow, &props, &mut cx, key(|key| key.tab = true));
        assert_eq!(
            flow.picker.as_ref().map(|picker| picker.view),
            Some(PickerView::Worktrees)
        );
        // Esc clears the search first, then closes.
        press(&mut flow, &props, &mut cx, key(|key| key.escape = true));
        assert_eq!(flow.picker.as_ref().map(|picker| picker.query.as_str()), Some(""));
        press(&mut flow, &props, &mut cx, key(|key| key.escape = true));
        assert_eq!(flow.picker, None);
        assert!(effects(&mut cx).is_empty());
    }

    #[test]
    fn nothing_selected_asks_for_a_task() {
        let props = props(&[], &[]);
        let mut flow = TasksFlow::default();
        let mut cx = cx();
        // The empty backlog row is selected: it is not a task.
        typed(&mut flow, &props, &mut cx, "n");
        typed(&mut flow, &props, &mut cx, "q");
        assert_eq!(effects(&mut cx), ["SetStatus(Some(\"Select a task first (j/k)\"))"]);
        // The notes view's keys without a note.
        press(&mut flow, &props, &mut cx, key(|key| key.tab = true));
        typed(&mut flow, &props, &mut cx, "a");
        typed(&mut flow, &props, &mut cx, "A");
        typed(&mut flow, &props, &mut cx, "f");
        assert!(flow.items_only);
        assert_eq!(
            effects(&mut cx),
            [
                "SetStatus(Some(\"Select an open checklist item (☐) to add it as a task\"))",
                "SetStatus(Some(\"Select a note first (j/k)\"))",
            ]
        );
        assert_eq!(flow.hint(&props, 130), "enter open the note • f every note • tab tasks");
        press(&mut flow, &props, &mut cx, key(|key| key.escape = true));
        assert_eq!(flow.view, TasksView::Tasks);
    }

    #[test]
    fn open_resets_the_board_onto_the_works_first_task() {
        let mut linked = task("cc", "Linked", "");
        linked.meta.wt = Some("w1".to_owned());
        let tasks = [task(TASK_ID, "Backlog", ""), linked];
        let props = props(&tasks, &[]);
        let mut flow = TasksFlow {
            view: TasksView::Notes,
            pending_delete: Some("x".to_owned()),
            scoped: true,
            ..TasksFlow::default()
        };
        flow.open(Some("wt:w1"), &props);
        assert_eq!(flow.view, TasksView::Tasks);
        assert_eq!(flow.scope.as_deref(), Some("wt:w1"));
        assert!(!flow.scoped && flow.pending_delete.is_none());
        assert_eq!(flow.selected, RowSelection::task("cc"));
        // v narrows to that work, and its hint offers the way back.
        let mut cx = cx();
        typed(&mut flow, &props, &mut cx, "v");
        assert!(flow.scoped);
        let rows = flow.rows(&props);
        assert!(
            rows.iter()
                .all(|row| !matches!(row, BoardRow::Task { task, .. } if task.id == TASK_ID))
        );
        let buf = render(&mut flow, &props);
        let rendered = buffer_rows(&buf);
        assert!(rendered[1].contains("1 open · v all tasks"), "{rendered:#?}");
        assert!(flow.hint(&props, 200).contains("v all tasks"));
    }

    // -----------------------------------------------------------------------------------------------------------
    // Requests (a daemon over a socket pair)

    #[test]
    fn saving_a_new_task_sends_the_op_and_selects_it() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| panic!("{error}"));
        tokio::task::LocalSet::new().block_on(&runtime, async {
            let (mut app, mut rx) = crate::app::test_support::app(crate::app::state::AppProps::default());
            let (socket, peer) = tokio::net::UnixStream::pair().unwrap_or_else(|error| panic!("{error}"));
            app.connection.client = Some(Rc::new(LiveClient::new(
                socket,
                deckhand_client::live::LiveClientHandlers::default(),
            )));
            let (read, mut write) = peer.into_split();
            let mut lines = BufReader::new(read);
            app.flows.tasks.editor = Some(editor(TaskEditorKind::Add, TaskEditorField::Body, "  New one ", "why"));
            app.tasks_flow_call(|flow, props, cx| flow.handle_input(props, &ctrl_s(), cx));
            assert!(app.flows.tasks.busy);
            // A second op while one is in flight is ignored.
            app.tasks_flow_call(|flow, props, cx| flow.handle_input(props, &ctrl_s(), cx));
            let mut line = String::new();
            lines
                .read_line(&mut line)
                .await
                .unwrap_or_else(|error| panic!("{error}"));
            let request: Value = serde_json::from_str(&line).unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(request["type"], "task-op");
            assert_eq!(request["cwd"], "/repo");
            assert_eq!(
                request["op"],
                serde_json::json!({"type": "add", "title": "New one", "body": "why"})
            );
            let response = serde_json::json!({
                "type": "response",
                "requestId": request["requestId"],
                "ok": true,
                "data": {
                    "key": "k",
                    "path": "/repo/tasks.md",
                    "text": "# Tasks\n\n- [ ] New one <!-- dh:t=0f0f added=2026-10-19 -->\n",
                    "revision": "r1",
                },
            });
            write
                .write_all(format!("{response}\n").as_bytes())
                .await
                .unwrap_or_else(|error| panic!("{error}"));
            let update = rx.recv().await.unwrap_or_else(|| panic!("the reply's update"));
            update(&mut app);
            let flow = &app.flows.tasks;
            assert!(!flow.busy);
            assert!(flow.editor.is_none());
            assert_eq!(flow.selected, RowSelection::task("0f0f"));
            assert_eq!(app.messages.status_message.as_deref(), Some("Added to the backlog"));
            let ids: Vec<&str> = app.data.tasks.iter().map(|task| task.id.as_str()).collect();
            assert_eq!(ids, ["0f0f"]);
            // Nothing else was sent.
            let mut more = String::new();
            let pending = tokio::time::timeout(std::time::Duration::from_millis(20), lines.read_line(&mut more)).await;
            assert!(pending.is_err(), "unexpected request: {more}");
        });
    }

    #[test]
    fn a_failed_op_shows_its_error() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| panic!("{error}"));
        tokio::task::LocalSet::new().block_on(&runtime, async {
            let (mut app, mut rx) = crate::app::test_support::app(crate::app::state::AppProps::default());
            let (socket, peer) = tokio::net::UnixStream::pair().unwrap_or_else(|error| panic!("{error}"));
            app.connection.client = Some(Rc::new(LiveClient::new(
                socket,
                deckhand_client::live::LiveClientHandlers::default(),
            )));
            let (read, mut write) = peer.into_split();
            let mut lines = BufReader::new(read);
            app.set_tasks_doc(Some(TasksDoc {
                key: "k".to_owned(),
                path: "/repo/tasks.md".to_owned(),
                text: "- [ ] One <!-- dh:t=aa -->\n- [ ] Two <!-- dh:t=bb -->\n".to_owned(),
                revision: "r1".to_owned(),
                too_large: None,
            }));
            app.flows.tasks.selected = RowSelection::task("bb");
            app.tasks_flow_call(|flow, props, cx| flow.handle_input(props, &Input::text("K"), cx));
            let mut line = String::new();
            lines
                .read_line(&mut line)
                .await
                .unwrap_or_else(|error| panic!("{error}"));
            let request: Value = serde_json::from_str(&line).unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(
                request["op"],
                serde_json::json!({"type": "move", "id": "bb", "target": "aa"})
            );
            let response = serde_json::json!({
                "type": "response",
                "requestId": request["requestId"],
                "ok": false,
                "error": "the task list changed",
            });
            write
                .write_all(format!("{response}\n").as_bytes())
                .await
                .unwrap_or_else(|error| panic!("{error}"));
            let update = rx.recv().await.unwrap_or_else(|| panic!("the reply's update"));
            update(&mut app);
            assert!(!app.flows.tasks.busy);
            assert_eq!(app.messages.error.as_deref(), Some("the task list changed"));
        });
    }
}
