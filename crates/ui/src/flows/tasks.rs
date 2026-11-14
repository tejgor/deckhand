//! The Tasks board (b): the repository's one list in the right pane, as the tree it is (`docs/future/work-items.md`).
//! Any line can have lines under it and a session on it; the backlog is the top-level lines nobody is on. v narrows
//! the board to the selected session's line (or its nearest ancestor's). Also the Notes view (tab: the scratchpad,
//! line notes and session notes, for reading), a small editor (a line's title, then its note; or the scratchpad), and
//! the w menu that moves a line: to the backlog, under another line, or onto a session (`deckhand task move`'s rules).
//! The daemon owns the files; every change is an op (`deckhand_core::tasks`) or a revision-checked note save, applied
//! to the file as it is, so an editor's edits are never overwritten.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use serde_json::{Value, json};

use deckhand_client::live::{LiveClient, Reply};
use deckhand_core::display::{
    HINT_SEPARATOR, HintPart, error_message, fit_hint, scrolled_list_top, strip_terminal_controls, truncate,
};
use deckhand_core::error::DhError;
use deckhand_core::jsstr::{date, js_len, js_pad_end, js_slice, js_trim, path};
use deckhand_core::key::{Input, Key};
use deckhand_core::lines_board::{
    BoardOptions, BoardRow, BoardScope, LineState, MoveChoice, MoveOption, MoveView, NoteLineStyle, NoteOf,
    NotesViewOptions, Section, StartOn, board_rows, line_state, move_counts, move_options, move_plan, move_rows,
    notes_view_rows, row_key, scope_root, selectable_row, start_on,
};
use deckhand_core::notes::{MAX_NOTES_CHARS, parse_checklist_line};
use deckhand_core::sidebar_model::attention_reason_lines;
use deckhand_core::task_list::{Line, MovePlan, TaskList};
use deckhand_core::tasks::MAX_TASK_TITLE;
use deckhand_core::tasks_board::{StepCount, task_state};
use deckhand_core::text_editor::{EditOptions, EditorLine, EditorState, edit_text, wrap_rows, wrapped_editor_lines};
use deckhand_core::work_notes::NoteMap;
use deckhand_protocol::data::{
    NoteRef, NoteSaveResult, PromoteNoteItemResult, SessionRecord, Task, TaskOp, TasksDoc, WorkNoteKind,
};

use crate::app::state::App;
use crate::flow::{Flow, FlowCx, FlowEffect};
use crate::keymap::{Binding, HelpRef, KeySpec, Keymap, NamedKey};
use crate::press_again::PressAgain;
use crate::primitives::{
    Border, Column, Row, Seg, TextStyle, Wrap, bordered, draw_rows, fit_row, space_between, truncate_row, with_base,
};
use crate::theme::{THEME, color};

/// TS: useTasksFlow apply's `then`: runs with the new list.
pub type ApplyThen = Box<dyn FnOnce(&mut TasksFlow, &TasksDoc, &mut FlowCx<App, TasksFlow>)>;

/// The selected row's details under the list (at most).
pub const DETAIL_ROWS: usize = 5;

/// The editor's note rows (a line's note).
pub const EDITOR_BODY_ROWS: usize = 4;

/// The editor's rows for the scratchpad.
pub const SCRATCHPAD_ROWS: usize = 6;

/// The w menu's rows.
pub const PICKER_ROWS: usize = 8;

/// The w menu: the line moving, the list shown (tab), the search and the selected row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskPicker {
    pub line_id: String,
    pub title: String,
    pub view: MoveView,
    pub query: String,
    /// Typing the search (after /) until Enter keeps it or Esc clears it.
    pub typing: bool,
    pub index: usize,
}

/// What the editor edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TaskEditorKind {
    /// A new line (its title, and a note to start it with).
    Add,
    /// A line's title and its note.
    Edit,
    /// The repository's scratchpad.
    Scratchpad,
}

/// The editor's field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TaskEditorField {
    Title,
    Note,
}

/// The editor under the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskEditor {
    pub kind: TaskEditorKind,
    /// Edit: the line.
    pub id: Option<String>,
    /// Add: the line it goes under (none: the top level).
    pub parent: Option<String>,
    /// Add: under this session's line, made first (`v` on a session without one).
    pub session: Option<String>,
    pub field: TaskEditorField,
    pub title: EditorState,
    pub note: EditorState,
    /// The note as it was when the editor opened (or last saved), and the revision a save names.
    pub note_before: String,
    pub revision: String,
}

impl TaskEditor {
    /// The note it edits (Edit: the line's, Scratchpad: the list's).
    fn note_ref(&self, props: &TasksFlowProps<'_>) -> Option<NoteRef> {
        match self.kind {
            TaskEditorKind::Edit => self.id.clone().map(|id| NoteRef {
                kind: WorkNoteKind::Line,
                id,
            }),
            TaskEditorKind::Scratchpad => scratchpad_id(props).map(|id| NoteRef {
                kind: WorkNoteKind::Scratchpad,
                id,
            }),
            TaskEditorKind::Add => None,
        }
    }
}

/// The scratchpad's ID: the list's key.
fn scratchpad_id(props: &TasksFlowProps<'_>) -> Option<String> {
    props
        .notes
        .key()
        .map(str::to_owned)
        .or_else(|| props.doc.map(|doc| doc.key.clone()))
}

/// A line title's limits.
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

/// A note's limits (as in the Notes tab).
pub fn note_options() -> EditOptions {
    EditOptions {
        select_all: Some(false),
        max_chars: Some(MAX_NOTES_CHARS),
        limit_message: Some(format!(
            "A note is at most {} characters",
            deckhand_core::notes::MAX_NOTES_LABEL
        )),
        tab: Some(false),
        wrap_width: None,
        page_rows: None,
    }
}

/// The ID of the line `next` has that `before` did not (the one just added).
pub fn parse_added(next: &TasksDoc, before: &HashSet<String>) -> Option<String> {
    TaskList::parse(&next.text)
        .lines()
        .iter()
        .map(|line| line.id.as_str())
        .find(|id| !before.contains(*id) && !id.starts_with('~'))
        .map(str::to_owned)
}

/// A `<Text wrap="truncate-end">` of `segs` under `base`, cut to `width` columns.
fn line(segs: Row, base: TextStyle, width: usize) -> Row {
    truncate_row(&with_base(segs, base), width, Wrap::TruncateEnd)
}

/// A width difference, clamped at zero.
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

/// The `selected ? '›' : ' '` marker of a row.
fn marker(selected: bool) -> &'static str {
    if selected { "›" } else { " " }
}

/// `n lines` / `1 line`.
fn lines_count(count: usize) -> String {
    format!("{count} line{}", if count == 1 { "" } else { "s" })
}

/// One board row (`sessions`: the repository's, for who is on a line).
pub fn board_line(
    row: &BoardRow,
    selected: bool,
    width: usize,
    sessions: &[&SessionRecord],
    spinner_frame: &str,
) -> Row {
    let mark = marker(selected);
    // A nested Text's color when not selected (selected rows take the bar's).
    let unless_selected =
        |name: Option<ratatui::style::Color>| TextStyle::default().color(if selected { None } else { name });
    let bar = |selected: bool| {
        TextStyle::default()
            .inverse(selected)
            .color(if selected { THEME.active } else { THEME.muted })
    };
    match row {
        BoardRow::Heading { text, count } => {
            let mut segs = vec![Seg::new(text.clone(), TextStyle::fg(THEME.muted).bold(true))];
            if let Some(count) = count {
                segs.push(Seg::new(format!(" · {count}"), TextStyle::fg(THEME.muted)));
            }
            line(segs, TextStyle::default(), width)
        }
        BoardRow::Empty { text, .. } => line(
            vec![Seg::plain(js_pad_end(&format!("{mark} {text}"), width, " "))],
            bar(selected),
            width,
        ),
        BoardRow::Older { count, shown } => {
            let text = format!(
                "{mark} {} {count} older done · enter {} them",
                if *shown { '▾' } else { '▸' },
                if *shown { "hides" } else { "shows" }
            );
            line(vec![Seg::plain(js_pad_end(&text, width, " "))], bar(selected), width)
        }
        BoardRow::Line {
            line: item,
            section,
            depth,
            count,
            folded,
            brief,
        } => {
            let state = line_state(item, sessions, spinner_frame);
            let count = count.map_or(String::new(), |count| {
                format!("{}{}/{}", if *folded { "▸ " } else { "" }, count.ticked, count.total)
            });
            let brief = if *brief { "✎" } else { "" };
            let tail: Vec<&str> = [count.as_str(), brief]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect();
            let tail = if tail.is_empty() {
                String::new()
            } else {
                format!("  {}", tail.join(" "))
            };
            // Who is on it: the session's name, its glyph and state; else how it ended or what it was. A step ticked
            // by hand among the open lines says nothing more.
            let ticked_step = item.done && *section == Section::Open && item.meta.auto.is_none();
            let state_text = if ticked_step {
                String::new()
            } else if state.who.is_some() || item.done || state.text == "session not shown" {
                format!("{} {}", state.glyph, state.text)
            } else {
                state.text.clone()
            };
            let room = minus(width / 2, js_len(&state_text) + js_len(&tail) + 1);
            let who = state
                .who
                .as_deref()
                .map(|who| truncate(who, room.max(4)))
                .unwrap_or_default();
            let right_len = js_len(&who) + usize::from(!who.is_empty()) + js_len(&state_text) + js_len(&tail);
            let indent = "  ".repeat(*depth);
            let tick = if item.done { "☑" } else { "☐" };
            let lead = format!("{mark} {indent}");
            let title_room = minus(width, js_len(&lead) + right_len + 4).max(1);
            let title = truncate(&item.title, title_room);
            let base = TextStyle::default()
                .inverse(selected)
                .bold(selected)
                .color(if selected {
                    Some(THEME.active)
                } else if item.done {
                    Some(THEME.muted)
                } else {
                    None
                });
            let tick_color = if item.done { Some(THEME.success) } else { None };
            let state_color = if state.who.is_some() || item.done {
                color(state.color)
            } else {
                Some(THEME.muted)
            };
            let mut segs = vec![
                Seg::plain(lead.clone()),
                Seg::new(tick, unless_selected(tick_color)),
                Seg::plain(js_pad_end(
                    &format!(" {title}"),
                    minus(width, js_len(&lead) + 1 + right_len),
                    " ",
                )),
            ];
            if !who.is_empty() {
                segs.push(Seg::new(format!("{who} "), unless_selected(Some(THEME.accent_soft))));
            }
            segs.push(Seg::new(state_text, unless_selected(state_color)));
            segs.push(Seg::new(tail, unless_selected(Some(THEME.muted))));
            line(segs, base, width)
        }
        // The Notes view: a note's name, then its lines, indented under it.
        BoardRow::NoteHead {
            note,
            label,
            depth,
            done,
            brief,
            empty,
        } => {
            let right = match note {
                NoteOf::Scratchpad if *empty => "empty · enter writes in it",
                NoteOf::Line(_) if *empty && *brief => "✎ brief · no note yet",
                NoteOf::Line(_) if *empty => "no note yet",
                NoteOf::Line(_) if *brief => "✎ brief",
                NoteOf::Session(_) if *empty => "session · no note yet",
                NoteOf::Session(_) => "session",
                _ => "",
            };
            let right_len = js_len(right);
            let tick = match note {
                NoteOf::Line(_) if *done => "☑ ",
                NoteOf::Line(_) => "☐ ",
                _ => "",
            };
            let head = format!(
                "{mark} {}{tick}{}",
                "  ".repeat(*depth),
                truncate(label, minus(width, right_len + 6 + depth * 2).max(1))
            );
            let base = TextStyle::default()
                .inverse(selected)
                .bold(!*empty || selected)
                .color(if selected {
                    THEME.active
                } else if *empty {
                    THEME.muted
                } else {
                    THEME.accent_soft
                });
            let segs = vec![
                Seg::plain(js_pad_end(&head, minus(width, right_len), " ")),
                Seg::new(right, unless_selected(Some(THEME.muted)).bold(false)),
            ];
            line(segs, base, width)
        }
        BoardRow::NoteLine { text, style, depth, .. } => {
            let color = if selected {
                Some(THEME.active)
            } else if *style == NoteLineStyle::Done {
                Some(THEME.muted)
            } else {
                None
            };
            let base = TextStyle::default()
                .inverse(selected)
                .bold(selected || *style == NoteLineStyle::Heading)
                .color(color);
            let indent = "  ".repeat(*depth);
            let text = format!(
                "{mark}   {indent}{}",
                truncate(text, minus(width, 4 + depth * 2).max(1))
            );
            line(vec![Seg::plain(js_pad_end(&text, width, " "))], base, width)
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

/// A brief's date as `14 Nov` (the ISO date when it is not one).
fn brief_date(at: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let day = js_slice(at, 8, Some(10));
    let month = js_slice(at, 5, Some(7)).parse::<usize>().ok();
    match (
        day.parse::<u32>().ok(),
        month.and_then(|month| MONTHS.get(month.wrapping_sub(1))),
    ) {
        (Some(day), Some(month)) => format!("{day} {month}"),
        _ => at.to_owned(),
    }
}

/// What the details say about who is on a line and what `n` / `o` do there.
fn line_place(line: &Line, state: &LineState, props: &TasksFlowProps<'_>) -> String {
    let parent = props.list.parent(&line.id);
    let under = parent.map_or(String::new(), |parent| {
        format!("Under “{}” · ", truncate(&parent.title, 30))
    });
    if line.done {
        return format!("{under}{} · space reopens it", state.text);
    }
    if let Some(who) = state.who.as_deref() {
        return format!("{under}{who} is on it ({}) · o selects it", state.text);
    }
    if line.is_taken() {
        return format!("{under}Its session is not in this list (archived or filtered?)");
    }
    let start = match start_on(props.list, &line.id, props.sessions) {
        StartOn::TopLevel => "n starts a session on it".to_owned(),
        StartOn::SubSessionOf(id) => {
            let name = props
                .sessions
                .iter()
                .find(|session| session.id == id)
                .map(|session| deckhand_core::display::display_session_title(session, props.sessions))
                .unwrap_or_default();
            format!("n starts a sub-session of {name}")
        }
    };
    let marker = if state.text.is_empty() {
        String::new()
    } else {
        format!("{} · ", state.text)
    };
    format!("{under}Nobody is on it · {marker}{start}")
}

/// The selected row's details under the list.
pub fn detail_lines(row: Option<&BoardRow>, props: &TasksFlowProps<'_>, width: usize) -> Vec<Row> {
    let bold = TextStyle::default().bold(true);
    let muted = TextStyle::fg(THEME.muted);
    let text_line = |text: String, style: TextStyle| line(vec![Seg::plain(text)], style, width);
    match row {
        Some(BoardRow::NoteHead { note, label, .. } | BoardRow::NoteLine { note, text: label, .. }) => {
            let (title, about) = match (row, note) {
                (Some(BoardRow::NoteHead { .. }), NoteOf::Scratchpad) => (
                    "Scratchpad".to_owned(),
                    "Loose thoughts for the repository · enter edits it · ctrl+p there makes a checkbox a line",
                ),
                (Some(BoardRow::NoteHead { .. }), NoteOf::Line(_)) => (
                    label.clone(),
                    "A line's note · enter edits it · E opens it in your editor",
                ),
                (Some(BoardRow::NoteHead { .. }), NoteOf::Session(_)) => (
                    label.clone(),
                    "A session's note · enter opens its Notes tab · E opens it in your editor",
                ),
                (_, NoteOf::Session(_)) => (js_trim(label).to_owned(), "enter opens the session's Notes tab"),
                _ => (js_trim(label).to_owned(), "enter edits the note"),
            };
            let mut lines: Vec<Row> = wrapped(&title, minus(width, 2).max(1))
                .into_iter()
                .take(DETAIL_ROWS - 1)
                .map(|text| text_line(text, bold))
                .collect();
            lines.push(text_line(about.to_owned(), muted));
            lines
        }
        Some(BoardRow::Line { line: item, .. }) => {
            let state = line_state(item, props.sessions, props.spinner_frame);
            let mut lines = vec![
                text_line(item.title.clone(), bold),
                text_line(line_place(item, &state, props), muted),
            ];
            // What its session asks or said (agent signals).
            let lead = state
                .session_id
                .as_deref()
                .and_then(|id| props.sessions.iter().find(|session| session.id == id));
            if let Some(reason) = lead.and_then(|lead| {
                attention_reason_lines(lead, width, 1)
                    .into_iter()
                    .next()
                    .and_then(|parts| parts.into_iter().next())
            }) {
                let style = TextStyle::default().color(reason.color.and_then(color));
                lines.push(text_line(reason.text, style));
            }
            if let Some(brief) = props.notes.brief(&item.id) {
                let from = [
                    brief.from.clone().unwrap_or_default(),
                    brief.at.as_deref().map(brief_date).unwrap_or_default(),
                ]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
                let first = brief
                    .text
                    .lines()
                    .map(js_trim)
                    .find(|text| !text.is_empty())
                    .unwrap_or("");
                let head = if from.is_empty() {
                    "✎ Brief".to_owned()
                } else {
                    format!("✎ Brief · {from}")
                };
                lines.push(line(
                    vec![
                        Seg::new(head, TextStyle::fg(THEME.accent)),
                        Seg::plain(format!(": {first}")),
                    ],
                    TextStyle::default(),
                    width,
                ));
            }
            let room = minus(DETAIL_ROWS, lines.len());
            match props.notes.line_note(&item.id) {
                Some(note) => {
                    let body: Vec<String> = note
                        .text
                        .lines()
                        .filter(|text| !js_trim(text).is_empty())
                        .flat_map(|text| wrapped(text, minus(width, 2).max(1)))
                        .collect();
                    for (index, text) in body.iter().take(room).enumerate() {
                        let text = if index + 1 == room && body.len() > room {
                            format!("{} …", js_slice(text, 0, Some(signed(minus(width, 6)))))
                        } else {
                            text.clone()
                        };
                        lines.push(text_line(format!("  {text}"), TextStyle::default()));
                    }
                }
                None if room > 0 && !item.done => {
                    lines.push(text_line(
                        "  No note yet · enter, then tab, writes one".to_owned(),
                        muted,
                    ));
                }
                None => {}
            }
            lines
        }
        _ => Vec::new(),
    }
}

/// The Notes tab's first line for a session on a line: its line, how many more are open (legacy), its lines ticked,
/// its state, and the board's key.
#[derive(Clone, Copy, Debug)]
pub struct TaskBannerProps<'a> {
    pub task: &'a Task,
    /// More open tasks on its work.
    pub more: usize,
    /// Its lines, ticked of total.
    pub steps: Option<StepCount>,
    pub sessions: &'a [&'a SessionRecord],
    pub spinner_frame: &'a str,
}

/// The banner's rows (the line and a rule).
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

/// The banner, drawn.
pub fn render_task_banner(props: &TaskBannerProps<'_>, buf: &mut Buffer, area: Rect) {
    let rows = task_banner_rows(props, usize::from(area.width));
    draw_rows(buf, area, &rows);
}

/// The board's values (the callbacks are effects: exit, start, go to, open a note, the doc, a note, a session, the
/// error and status lines).
#[derive(Clone, Copy, Debug)]
pub struct TasksFlowProps<'a> {
    pub client: Option<&'a Rc<LiveClient>>,
    pub repo_root: &'a str,
    pub doc: Option<&'a TasksDoc>,
    /// The list as a tree.
    pub list: &'a TaskList,
    /// Its line notes, briefs and scratchpad.
    pub notes: &'a NoteMap,
    /// Every session of the repository.
    pub sessions: &'a [&'a SessionRecord],
    pub spinner_frame: &'a str,
}

/// The board's two views (tab).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TasksView {
    #[default]
    Tasks,
    Notes,
}

/// The selected row's key, else its position among the selectable rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowSelection {
    pub key: Option<String>,
    pub index: usize,
}

impl RowSelection {
    /// `{key: 'line:<id>', index: 0}`.
    pub fn line(id: &str) -> Self {
        RowSelection {
            key: Some(format!("line:{id}")),
            index: 0,
        }
    }
}

/// The board's state.
#[derive(Debug)]
pub struct TasksFlow {
    pub view: TasksView,
    pub selected: RowSelection,
    pub show_older: bool,
    pub editor: Option<TaskEditor>,
    /// The line whose removal waits for a second x ([`PressAgain`]).
    pub pending_delete: PressAgain<String>,
    /// The session selected when the board opened (v shows its line).
    pub scope: Option<String>,
    pub scoped: bool,
    pub picker: Option<TaskPicker>,
    /// The Notes view: only notes with text (f).
    pub with_text: bool,
    /// The list's first shown row.
    pub top: usize,
    /// An op in flight: further ops are ignored.
    pub busy: bool,
    /// The editor note's wrap width at the last render (40 before one).
    pub body_width: usize,
    /// The list's rows at the last render, for PgDn / PgUp (and Ctrl+D / Ctrl+U): half of them.
    pub list_rows: usize,
    /// The lines whose lines are hidden (c), kept across attaches by the app's caller.
    pub collapsed_steps: Vec<String>,
}

impl Default for TasksFlow {
    fn default() -> Self {
        TasksFlow {
            view: TasksView::Tasks,
            selected: RowSelection::default(),
            show_older: false,
            editor: None,
            pending_delete: PressAgain::default(),
            scope: None,
            scoped: false,
            picker: None,
            with_text: false,
            top: 0,
            busy: false,
            body_width: 40,
            list_rows: 10,
            collapsed_steps: Vec::new(),
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
    board_key(&[KeySpec::Char("v")], "only the selected session's line"),
    board_key(&[KeySpec::Char("f")], "notes: only notes with text"),
    board_key(&[KeySpec::Char("a")], "add a line under the selected one"),
    board_key(&[KeySpec::Char("A")], "add a top-level line (the backlog)"),
    board_key(&[KeySpec::Char("E")], "its note in your editor"),
    board_key(&[KeySpec::Char("n")], "start a session on it"),
    board_key(&[KeySpec::Char("w")], "move it"),
    board_key(&[KeySpec::Char("o")], "select the session on it"),
    board_key(&[KeySpec::Char("x")], "remove it and its lines (twice)"),
    board_key(&[KeySpec::Char("J"), KeySpec::Char("K")], "reorder"),
    board_key(&[KeySpec::Char(" ")], "tick or reopen"),
    board_key(&[KeySpec::Char("c")], "hide or show its lines"),
    board_key(
        &[KeySpec::Named(NamedKey::Return)],
        "edit it (tab: its note) / open the note",
    ),
    board_key(&[KeySpec::Named(NamedKey::Tab)], "the notes view / back to the list"),
    board_key(
        &[
            KeySpec::Named(NamedKey::Escape),
            KeySpec::Char("h"),
            KeySpec::Named(NamedKey::Left),
        ],
        "back",
    ),
];

/// A hint part with a short form and a drop rank.
fn hint(text: &str, short: Option<&str>, drop: Option<i32>) -> HintPart {
    HintPart {
        text: text.to_owned(),
        short: short.map(str::to_owned),
        drop,
    }
}

/// What the flow's async continuations run with.
type RunThen<T> = Box<dyn FnOnce(&mut TasksFlow, T, &mut FlowCx<App, TasksFlow>)>;

/// The lower part of the board (picker, editor or details): its nodes and their rows.
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

/// What saving the editor did: the list after a retitle or an add, and the note save's answer.
struct Saved {
    doc: Option<TasksDoc>,
    note: Option<NoteSaveResult>,
}

/// The ops of a move, in order (taking a session off its line first, for the backlog).
fn move_ops(line_id: &str, plan: &MovePlan) -> Vec<Value> {
    match plan {
        MovePlan::Already => Vec::new(),
        MovePlan::SessionOn { s } => vec![json!({"type": "session-on", "id": line_id, "s": s})],
        MovePlan::MoveTo { parent, unhook } => {
            let mut ops = Vec::new();
            if let Some(s) = unhook {
                ops.push(json!({"type": "session-off", "s": s}));
            }
            ops.push(json!({"type": "move-to", "id": line_id, "parent": parent}));
            ops
        }
    }
}

/// The line the cursor is on (0-based).
fn cursor_line(state: &EditorState) -> usize {
    state
        .text
        .get(..state.cursor.min(state.text.len()))
        .map_or(0, |before| before.matches('\n').count())
}

impl TasksFlow {
    /// b: the board's list view, the selection on the line of `scope` (the selected session, which v shows alone).
    pub fn open(&mut self, scope: Option<&str>, props: &TasksFlowProps<'_>) {
        self.view = TasksView::Tasks;
        self.editor = None;
        self.picker = None;
        self.pending_delete.clear();
        self.scope = scope.map(str::to_owned);
        self.scoped = false;
        if let Some(root) = scope.and_then(|scope| scope_root(props.list, scope, props.sessions)) {
            self.selected = RowSelection::line(&root.id);
        }
    }

    /// What the board shows now: everything, or (v) the tree of the selected session's line.
    fn board_scope<'a>(&'a self, props: &TasksFlowProps<'a>) -> BoardScope<'a> {
        if !self.scoped {
            return BoardScope::All;
        }
        match self
            .scope
            .as_deref()
            .and_then(|scope| scope_root(props.list, scope, props.sessions))
        {
            Some(root) => BoardScope::Under(root.id.as_str()),
            None => BoardScope::Nothing,
        }
    }

    /// The board's rows as shown now (the Notes view or the list).
    pub fn rows(&self, props: &TasksFlowProps<'_>) -> Vec<BoardRow> {
        let scope = self.board_scope(props);
        match self.view {
            TasksView::Notes => notes_view_rows(
                props.list,
                props.notes,
                props.sessions,
                &NotesViewOptions {
                    with_text: self.with_text,
                    scope,
                    session: if self.scoped { self.scope.as_deref() } else { None },
                },
            ),
            TasksView::Tasks => board_rows(
                props.list,
                &BoardOptions {
                    show_older: self.show_older,
                    now_ms: date::now_ms(),
                    scope,
                    folded: &self.collapsed_steps,
                    notes: props.notes,
                },
            ),
        }
    }

    /// The selected row's index in `rows`. The selection follows its row (by key) across updates; a row that went
    /// keeps the position.
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

    fn select(&mut self, rows: &[BoardRow], selectable: &[usize], index: usize) {
        self.selected = RowSelection {
            key: rows.get(index).and_then(row_key),
            index: selectable.iter().position(|at| *at == index).unwrap_or(0),
        };
    }

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

    /// One request at a time; a failure goes to the error line.
    fn run<T: 'static>(
        &mut self,
        props: &TasksFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
        work: impl FnOnce(&Rc<LiveClient>) -> Reply<T>,
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

    /// One op on the list; the new doc goes to the app, then `then`.
    pub fn apply(
        &mut self,
        op: TaskOp,
        props: &TasksFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
        then: Option<ApplyThen>,
    ) {
        let op = serde_json::to_value(&op).unwrap_or(Value::Null);
        self.apply_ops(vec![op], props, cx, then);
    }

    /// Ops (as JSON) on the list, one after the other; the doc after the last goes to the app, then `then`.
    fn apply_ops(
        &mut self,
        ops: Vec<Value>,
        props: &TasksFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
        then: Option<ApplyThen>,
    ) {
        let repo_root = props.repo_root.to_owned();
        self.run(
            props,
            cx,
            move |client| {
                let client = Rc::clone(client);
                Box::pin(async move {
                    let mut last = None;
                    for op in ops {
                        last = Some(client.task_op_value(&repo_root, op).await?);
                    }
                    last.ok_or_else(|| DhError::msg("Nothing to change"))
                })
            },
            Box::new(move |flow, next: TasksDoc, cx| {
                cx.emit(FlowEffect::TasksDoc(Box::new(next.clone())));
                if let Some(then) = then {
                    then(flow, &next, cx);
                }
            }),
        );
    }

    /// The editor's line, scratchpad or new line, saved: a retitle and the note (revision-checked), or an add (with
    /// its note).
    pub fn save(&mut self, state: TaskEditor, props: &TasksFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let title = js_trim(&state.title.text).to_owned();
        if state.kind != TaskEditorKind::Scratchpad && title.is_empty() {
            cx.set_error(Some("A line needs a title".to_owned()));
            return;
        }
        let repo_root = props.repo_root.to_owned();
        let note_text = state.note.text.clone();
        match state.kind {
            TaskEditorKind::Add => {
                let before: HashSet<String> = props.list.lines().iter().map(|line| line.id.clone()).collect();
                let parent = state.parent.clone();
                let session = state.session.clone();
                let message = match parent.as_deref().and_then(|id| props.list.get(id)) {
                    Some(parent) => format!("Added under “{}”", truncate(&parent.title, 40)),
                    None if session.is_some() => "Added a step to the session's line".to_owned(),
                    None => "Added to the backlog".to_owned(),
                };
                self.run(
                    props,
                    cx,
                    move |client| {
                        let client = Rc::clone(client);
                        Box::pin(async move {
                            // `v` on a session without a line: its line first (lines on demand).
                            let parent = match (parent, session) {
                                (Some(parent), _) => Some(parent),
                                (None, Some(s)) => {
                                    let doc = client
                                        .task_op_value(&repo_root, json!({"type": "session-line", "s": s}))
                                        .await?;
                                    TaskList::parse(&doc.text)
                                        .line_of_session(&s)
                                        .map(|line| line.id.clone())
                                }
                                (None, None) => None,
                            };
                            let mut op = json!({"type": "add", "title": title});
                            if let Some(parent) = parent.clone() {
                                op["parent"] = Value::String(parent);
                            }
                            if !js_trim(&note_text).is_empty() {
                                op["note"] = Value::String(note_text);
                            }
                            let next = client.task_op_value(&repo_root, op).await?;
                            Ok((next, parent))
                        })
                    },
                    Box::new(move |flow, (next, parent): (TasksDoc, Option<String>), cx| {
                        flow.editor = None;
                        // The session's line made on the way is not the line added.
                        let mut before = before;
                        before.extend(parent);
                        if let Some(added) = parse_added(&next, &before) {
                            flow.selected = RowSelection::line(&added);
                        }
                        cx.emit(FlowEffect::TasksDoc(Box::new(next)));
                        cx.set_status(Some(message));
                    }),
                );
            }
            TaskEditorKind::Edit | TaskEditorKind::Scratchpad => {
                let Some(note) = state.note_ref(props) else {
                    self.editor = None;
                    return;
                };
                let retitle = match (state.kind, state.id.as_deref().and_then(|id| props.list.get(id))) {
                    (TaskEditorKind::Edit, Some(line)) if line.title != title => {
                        Some(json!({"type": "retitle", "id": line.id, "title": title}))
                    }
                    _ => None,
                };
                let note_changed = note_text != state.note_before;
                if retitle.is_none() && !note_changed {
                    self.editor = None;
                    return;
                }
                let revision = state.revision.clone();
                let kind = state.kind;
                self.run(
                    props,
                    cx,
                    move |client| {
                        let client = Rc::clone(client);
                        Box::pin(async move {
                            let doc = match retitle {
                                Some(op) => Some(client.task_op_value(&repo_root, op).await?),
                                None => None,
                            };
                            let note = if note_changed {
                                Some(client.save_work_note(note, &note_text, &revision).await?)
                            } else {
                                None
                            };
                            Ok(Saved { doc, note })
                        })
                    },
                    Box::new(move |flow, saved: Saved, cx| {
                        if let Some(doc) = saved.doc {
                            cx.emit(FlowEffect::TasksDoc(Box::new(doc)));
                        }
                        let Some(result) = saved.note else {
                            flow.editor = None;
                            cx.set_status(Some("Saved the line".to_owned()));
                            return;
                        };
                        if let Some(session) = result.session {
                            cx.emit(FlowEffect::Session(Box::new(session.into_inner())));
                        }
                        let revision = result.note.revision.clone();
                        let current = result.note.text.clone();
                        cx.emit(FlowEffect::WorkNote(Box::new(result.note)));
                        if result.saved {
                            flow.editor = None;
                            cx.set_status(Some(
                                if kind == TaskEditorKind::Scratchpad {
                                    "Saved the scratchpad"
                                } else {
                                    "Saved the line and its note"
                                }
                                .to_owned(),
                            ));
                            return;
                        }
                        // Changed elsewhere meanwhile: nothing was written. The editor keeps what was typed; saving
                        // again replaces the note as it is now, knowingly.
                        if let Some(editor) = flow.editor.as_mut() {
                            editor.revision = revision;
                            editor.note_before = current;
                        }
                        cx.set_error(Some(
                            "The note changed meanwhile (nothing was saved): ctrl+s saves yours over it, esc after \
                             clearing it keeps theirs"
                                .to_owned(),
                        ));
                    }),
                );
            }
        }
    }

    /// ctrl+p in a note: the checkbox under the cursor becomes a line (under the note's line; from the scratchpad, in
    /// the backlog) and leaves the note. What was typed is saved first.
    fn promote(&mut self, state: TaskEditor, props: &TasksFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let Some(note) = state.note_ref(props) else {
            cx.set_status(Some("Save the line first (enter), then ctrl+p in its note".to_owned()));
            return;
        };
        let at = cursor_line(&state.note);
        let item = state
            .note
            .text
            .split('\n')
            .nth(at)
            .and_then(parse_checklist_line)
            .filter(|item| !item.checked && !js_trim(&item.text).is_empty());
        let Some(item) = item else {
            cx.set_status(Some(
                "Put the cursor on an open checklist item (- [ ] …) to make it a line".to_owned(),
            ));
            return;
        };
        let title = truncate(js_trim(&item.text), 40);
        let place = match (note.kind, state.id.as_deref().and_then(|id| props.list.get(id))) {
            (WorkNoteKind::Line, Some(line)) => format!("under “{}”", truncate(&line.title, 30)),
            _ => "in the backlog".to_owned(),
        };
        let text = state.note.text.clone();
        let save_first = text != state.note_before;
        let revision = state.revision.clone();
        self.run(
            props,
            cx,
            move |client| {
                let client = Rc::clone(client);
                Box::pin(async move {
                    let mut revision = revision;
                    if save_first {
                        let saved = client.save_work_note(note.clone(), &text, &revision).await?;
                        if !saved.saved {
                            return Err(DhError::msg(
                                "The note changed meanwhile (nothing was saved): ctrl+s saves yours over it first",
                            ));
                        }
                        revision = saved.note.revision;
                    }
                    client.promote_work_note_item(note, signed(at), &revision).await
                })
            },
            Box::new(move |flow, result: PromoteNoteItemResult, cx| {
                cx.emit(FlowEffect::TasksDoc(Box::new(result.tasks)));
                if let Some(session) = result.session {
                    cx.emit(FlowEffect::Session(Box::new(session.into_inner())));
                }
                if let Some(editor) = flow.editor.as_mut() {
                    let text = result.note.text.clone();
                    editor.note = EditorState {
                        cursor: editor.note.cursor.min(text.len()),
                        text: text.clone(),
                        ..EditorState::default()
                    };
                    // Back on a char boundary after the text shrank.
                    while !editor.note.text.is_char_boundary(editor.note.cursor) {
                        editor.note.cursor -= 1;
                    }
                    editor.note_before = text;
                    editor.revision = result.note.revision.clone();
                }
                cx.emit(FlowEffect::WorkNote(Box::new(result.note)));
                cx.set_status(Some(format!("Now a line {place}: {title}")));
            }),
        );
    }

    /// Keys while the editor is open.
    pub fn editor_input(
        &mut self,
        input: &Input,
        state: TaskEditor,
        props: &TasksFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
    ) {
        let key = &input.key;
        // Esc saves what was typed, as in the notes editor; nothing typed, or nothing changed, just closes.
        if key.escape {
            let untouched = match state.kind {
                TaskEditorKind::Add => js_trim(&state.title.text).is_empty(),
                TaskEditorKind::Scratchpad => state.note.text == state.note_before,
                TaskEditorKind::Edit => {
                    js_trim(&state.title.text).is_empty()
                        || state.note.text == state.note_before
                            && state
                                .id
                                .as_deref()
                                .and_then(|id| props.list.get(id))
                                .is_some_and(|line| line.title == js_trim(&state.title.text))
                }
            };
            if untouched {
                self.editor = None;
            } else {
                self.save(state, props, cx);
            }
            return;
        }
        if key.ctrl && input.input == "s" {
            self.save(state, props, cx);
            return;
        }
        if key.tab {
            if state.kind != TaskEditorKind::Scratchpad {
                let field = if state.field == TaskEditorField::Title {
                    TaskEditorField::Note
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
        if key.ctrl && input.input == "p" {
            self.promote(state, props, cx);
            return;
        }
        let rows = if state.kind == TaskEditorKind::Scratchpad {
            SCRATCHPAD_ROWS
        } else {
            EDITOR_BODY_ROWS
        };
        let options = EditOptions {
            wrap_width: Some(self.body_width),
            page_rows: Some(rows),
            ..note_options()
        };
        let next = if key.return_ {
            let enter = Key {
                return_: true,
                ..Key::NONE
            };
            edit_text(&state.note, "", &enter, &options)
        } else {
            edit_text(&state.note, &input.input, key, &options)
        };
        if let Some(message) = next.message.filter(|message| !message.is_empty()) {
            cx.set_error(Some(message));
            return;
        }
        let note = EditorState {
            text: next.text,
            cursor: next.cursor,
            ..EditorState::default()
        };
        self.editor = Some(TaskEditor { note, ..state });
    }

    /// The editor on a line (its title, or its note with `field` Note).
    fn edit_line(
        &mut self,
        line: &Line,
        field: TaskEditorField,
        props: &TasksFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
    ) {
        let note = props.notes.editable(WorkNoteKind::Line, &line.id);
        if note.too_large == Some(true) {
            cx.set_status(Some(
                "Its note is too large to edit here; E opens it in your editor".to_owned(),
            ));
            return;
        }
        self.editor = Some(TaskEditor {
            kind: TaskEditorKind::Edit,
            id: Some(line.id.clone()),
            parent: None,
            session: None,
            field,
            title: EditorState {
                text: line.title.clone(),
                cursor: line.title.len(),
                ..EditorState::default()
            },
            note: EditorState {
                text: note.text.clone(),
                cursor: note.text.len(),
                ..EditorState::default()
            },
            note_before: note.text,
            revision: note.revision,
        });
    }

    /// The editor on the scratchpad.
    fn edit_scratchpad(&mut self, props: &TasksFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let Some(id) = scratchpad_id(props) else {
            cx.set_status(Some("The list is still loading".to_owned()));
            return;
        };
        let note = props.notes.editable(WorkNoteKind::Scratchpad, &id);
        if note.too_large == Some(true) {
            cx.set_status(Some(
                "The scratchpad is too large to edit here; E opens it in your editor".to_owned(),
            ));
            return;
        }
        self.editor = Some(TaskEditor {
            kind: TaskEditorKind::Scratchpad,
            id: None,
            parent: None,
            session: None,
            field: TaskEditorField::Note,
            title: EditorState::default(),
            note: EditorState {
                text: note.text.clone(),
                cursor: note.text.len(),
                ..EditorState::default()
            },
            note_before: note.text,
            revision: note.revision,
        });
    }

    /// A new line's editor: under `parent` (none: the top level), or under `session`'s line, made first.
    fn add_line(&mut self, parent: Option<String>, session: Option<String>) {
        self.editor = Some(TaskEditor {
            kind: TaskEditorKind::Add,
            id: None,
            parent,
            session,
            field: TaskEditorField::Title,
            title: EditorState::default(),
            note: EditorState::default(),
            note_before: String::new(),
            revision: String::new(),
        });
    }

    /// The w menu's rows for `state`.
    fn picker_rows_of(state: &TaskPicker, props: &TasksFlowProps<'_>) -> Vec<MoveOption> {
        move_rows(
            &move_options(props.list, &state.line_id, props.sessions),
            state.view,
            &state.query,
        )
    }

    /// The w menu on `state`, the selection on the line's parent (in the lines) or the first row.
    fn open_picker(&mut self, state: TaskPicker, props: &TasksFlowProps<'_>) {
        let rows = Self::picker_rows_of(&state, props);
        let parent = props.list.parent(&state.line_id).map(|parent| parent.id.clone());
        let index = parent
            .filter(|_| state.query.is_empty())
            .and_then(|parent| {
                rows.iter()
                    .position(|option| option.choice == MoveChoice::Line(parent.clone()))
            })
            .unwrap_or_else(|| usize::from(!state.query.is_empty() && rows.len() > 1));
        self.picker = Some(TaskPicker { index, ..state });
    }

    /// What a w menu row would make of the line.
    fn picker_role(state: &TaskPicker, option: &MoveOption, props: &TasksFlowProps<'_>) -> &'static str {
        let Some(line) = props.list.get(&state.line_id) else {
            return "";
        };
        match (
            move_plan(props.list, line, &option.choice, props.sessions),
            &option.choice,
        ) {
            (Some(MovePlan::Already), _) => "here",
            (Some(MovePlan::SessionOn { .. }), _) => "◆ its line",
            (Some(MovePlan::MoveTo { .. }), MoveChoice::Session(_)) => "→ a step of its line",
            (Some(MovePlan::MoveTo { .. }), MoveChoice::Line(_)) => "→ under it",
            _ => "",
        }
    }

    /// Keys while the w menu is open.
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
            view: if state.view == MoveView::Lines {
                MoveView::Sessions
            } else {
                MoveView::Lines
            },
            ..state.clone()
        };
        if key.tab {
            self.open_picker(switched(), props);
            return;
        }
        // Ctrl+P / Ctrl+N and Ctrl+K / Ctrl+J move too, as in the sidebar's search.
        let (up, down) = crate::app::modes::typing_moves(input);
        if up {
            return to(self, index - 1);
        }
        if down {
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
        let Some(line) = props.list.get(&state.line_id) else {
            self.picker = None;
            cx.set_status(Some("That line is not in the list any more".to_owned()));
            return;
        };
        let Some(plan) = move_plan(props.list, line, &option.choice, props.sessions) else {
            cx.set_status(Some("That is not in the list any more".to_owned()));
            return;
        };
        let title = truncate(&line.title, 40);
        let message = match (&plan, &option.choice) {
            (MovePlan::Already, _) => {
                self.picker = None;
                cx.set_status(Some("It is already there".to_owned()));
                return;
            }
            (MovePlan::SessionOn { .. }, _) => format!(
                "Now the line of {} (merging or finishing that work ticks it): {title}",
                option.label
            ),
            (_, MoveChoice::Backlog) => format!("Moved to the backlog: {title}"),
            (_, MoveChoice::Session(_)) => format!("Now a step of the line of {}: {title}", option.label),
            (_, MoveChoice::Line(_)) => format!("Now under “{}”: {title}", truncate(&option.label, 40)),
        };
        let line_id = state.line_id.clone();
        let then: ApplyThen = Box::new(move |flow, _, cx| {
            flow.picker = None;
            flow.selected = RowSelection::line(&line_id);
            cx.set_status(Some(message));
        });
        self.apply_ops(move_ops(&state.line_id, &plan), props, cx, Some(then));
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
        if key.escape || key.tab || key.left_arrow || text == "h" {
            self.view = TasksView::Tasks;
            self.selected = RowSelection::default();
            return;
        }
        if text == "f" {
            self.with_text = !self.with_text;
            self.selected = RowSelection::default();
            return;
        }
        let note = match current_row {
            Some(BoardRow::NoteHead { note, .. } | BoardRow::NoteLine { note, .. }) => Some(note.clone()),
            _ => None,
        };
        if text == "E" {
            match note {
                Some(NoteOf::Scratchpad) => match scratchpad_id(props) {
                    Some(id) => self.open_note_in_editor(WorkNoteKind::Scratchpad, id, props, cx),
                    None => cx.set_status(Some("The list is still loading".to_owned())),
                },
                Some(NoteOf::Line(id)) => self.open_note_in_editor(WorkNoteKind::Line, id, props, cx),
                Some(NoteOf::Session(id)) => self.open_note_in_editor(WorkNoteKind::Session, id, props, cx),
                None => {}
            }
            return;
        }
        if !key.return_ {
            if matches!(text, "a" | "A" | "n" | "w" | "x" | " ") {
                cx.set_status(Some(
                    "The notes are for reading here: enter edits one, tab goes back to the list".to_owned(),
                ));
            }
            return;
        }
        match note {
            Some(NoteOf::Scratchpad) => self.edit_scratchpad(props, cx),
            Some(NoteOf::Line(id)) => match props.list.get(&id) {
                Some(line) => self.edit_line(&line.clone(), TaskEditorField::Note, props, cx),
                None => cx.set_status(Some("That line is not in the list any more".to_owned())),
            },
            Some(NoteOf::Session(id)) => cx.emit(FlowEffect::OpenNote(id)),
            None => {}
        }
    }

    /// E: a note in the user's editor (`open-note` makes its file when missing).
    fn open_note_in_editor(
        &mut self,
        kind: WorkNoteKind,
        id: String,
        props: &TasksFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
    ) {
        let what = match kind {
            WorkNoteKind::Scratchpad => "the scratchpad",
            WorkNoteKind::Session => "the session's note",
            _ => "the line's note",
        };
        self.run(
            props,
            cx,
            move |client| client.open_work_note(NoteRef { kind, id }),
            Box::new(move |_, file: String, cx| open_file(&file, what, cx)),
        );
    }

    /// E with no line selected: the list's file in the user's editor.
    fn open_list_in_editor(&mut self, props: &TasksFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let repo_root = props.repo_root.to_owned();
        self.run(
            props,
            cx,
            move |client| client.open_tasks(&repo_root),
            Box::new(|_, file: String, cx| open_file(&file, "the list", cx)),
        );
    }

    /// The w menu under the list.
    fn picker_lines(state: &TaskPicker, props: &TasksFlowProps<'_>, width: usize) -> Vec<Row> {
        let options = move_options(props.list, &state.line_id, props.sessions);
        let rows = move_rows(&options, state.view, &state.query);
        let (lines_count_now, sessions_count) = move_counts(&options, &state.query);
        let start =
            to_index((signed(state.index) - signed(PICKER_ROWS) + 1).min(signed(rows.len()) - signed(PICKER_ROWS)));
        let tab = |view: MoveView, text: String| {
            let on = state.view == view;
            Seg::new(
                format!(" {text} "),
                TextStyle::default()
                    .inverse(on)
                    .bold(on)
                    .color(if on { THEME.active } else { THEME.muted }),
            )
        };
        let any_listed = rows.iter().any(|option| option.choice != MoveChoice::Backlog);
        let view_name = match state.view {
            MoveView::Lines => "lines",
            MoveView::Sessions => "sessions",
        };
        let none = if !state.query.is_empty() {
            format!("Nothing in {view_name} matches “{}”", state.query)
        } else if state.view == MoveView::Lines {
            "No other open lines".to_owned()
        } else {
            "No sessions".to_owned()
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
                tab(MoveView::Lines, format!("Lines {lines_count_now}")),
                Seg::plain(" "),
                tab(MoveView::Sessions, format!("Sessions {sessions_count}")),
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
            let role = Self::picker_role(state, option, props);
            let note = if role.is_empty() {
                String::new()
            } else {
                format!(" {role}")
            };
            let note_len = js_len(&note);
            let indent = "  ".repeat(option.depth);
            let label = format!(
                "{} {indent}{}",
                marker(selected),
                truncate(&option.label, minus(width, note_len + 2 + indent.len()).max(1))
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

    /// The editor under the list (records the note's wrap width for Up/Down).
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
        let note_rows = if state.kind == TaskEditorKind::Scratchpad {
            SCRATCHPAD_ROWS
        } else {
            EDITOR_BODY_ROWS
        };
        self.body_width = minus(width, 2).max(4);
        let body = wrapped_editor_lines(&state.note, self.body_width);
        let start = to_index(
            (signed(body.cursor_row) - signed(note_rows) + 1).min(signed(body.lines.len()) - signed(note_rows)),
        );
        let on_note = state.field == TaskEditorField::Note;
        let mut head = vec![Seg::new(
            match state.kind {
                TaskEditorKind::Add => "New line",
                TaskEditorKind::Edit => "Edit line",
                TaskEditorKind::Scratchpad => "Scratchpad",
            },
            TextStyle::fg(THEME.accent).bold(true),
        )];
        let place = match state.kind {
            TaskEditorKind::Add => match state.parent.as_deref().and_then(|id| props.list.get(id)) {
                Some(parent) => format!(" · under “{}”", truncate(&parent.title, 40)),
                None if state.session.is_some() => " · a step of the session's line (made now)".to_owned(),
                None => " · top level (the backlog)".to_owned(),
            },
            TaskEditorKind::Scratchpad => " · loose thoughts for the repository".to_owned(),
            TaskEditorKind::Edit => String::new(),
        };
        head.push(Seg::new(place, TextStyle::fg(THEME.muted)));
        let mut rows = vec![line(head, TextStyle::default(), width)];
        let mut nodes = 1;
        if state.kind != TaskEditorKind::Scratchpad {
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
            let on_title = state.field == TaskEditorField::Title;
            let mut title = vec![label("Title  ", on_title)];
            if on_title {
                title.extend(cursor_of(&title_line, true));
            } else {
                title.push(Seg::plain(truncate(&state.title.text, title_width)));
            }
            rows.push(line(title, TextStyle::default(), width));
            let optional = if !state.note.text.is_empty() || on_note {
                ""
            } else {
                " (optional; whoever takes the line on is given it)"
            };
            let note_label = vec![label("Note", on_note), Seg::new(optional, TextStyle::fg(THEME.muted))];
            // The note's label is a wrapping Text: one node, possibly several rows.
            rows.extend(fit_row(&note_label, width, Wrap::Wrap));
            nodes += 2;
        }
        for (index, body_line) in body.lines.iter().skip(start).take(note_rows).enumerate() {
            let mut segs = vec![Seg::plain("  ")];
            segs.extend(cursor_of(body_line, on_note && start + index == body.cursor_row));
            rows.push(line(segs, TextStyle::default(), width));
            nodes += 1;
        }
        Lower { nodes, rows }
    }
}

/// Opens `file` in Cursor / VS Code, saying where it is if that fails.
fn open_file(file: &str, what: &str, cx: &mut FlowCx<App, TasksFlow>) {
    let errors = RefCell::new(Vec::new());
    let label = deckhand_sys::desktop::open_in_editor(
        file,
        &|message| errors.borrow_mut().push(format!("{message}; {what} is {file}")),
        None,
    );
    for error in errors.into_inner() {
        cx.set_error(Some(error));
    }
    if let Some(label) = label.filter(|label| !label.is_empty()) {
        cx.set_status(Some(format!("Opened {what} in {label}; edits there show up here")));
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
        if key.page_down || key.page_up {
            let half = signed((self.list_rows / 2).max(1));
            self.move_by(&rows, current, if key.page_down { half } else { -half });
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
        // v: only the line of the session the board was opened from (or its nearest ancestor's), in either view.
        if text == "v" {
            let Some(scope) = self.scope.clone().filter(|scope| !scope.is_empty()) else {
                cx.set_status(Some(format!(
                    "Select a session first: v shows the {} of its line",
                    if self.view == TasksView::Notes {
                        "notes"
                    } else {
                        "lines"
                    }
                )));
                return;
            };
            self.scoped = !self.scoped;
            self.selected = scope_root(props.list, &scope, props.sessions)
                .filter(|_| self.view == TasksView::Tasks)
                .map_or_else(RowSelection::default, |root| RowSelection::line(&root.id));
            return;
        }
        if self.view == TasksView::Notes {
            self.notes_input(input, current_row, props, cx);
            return;
        }
        // h and ← go back too, as from every pane.
        if key.escape || key.left_arrow || text == "h" {
            cx.exit();
            return;
        }
        if key.tab {
            self.view = TasksView::Notes;
            self.selected = RowSelection::default();
            return;
        }
        let selected_line = match current_row {
            Some(BoardRow::Line { line, section, .. }) => Some((line, *section)),
            _ => None,
        };
        if text == "a" {
            match (selected_line, current_row) {
                (Some((line, Section::Open)), _) => self.add_line(Some(line.id.clone()), None),
                // v on a session without a line: under its line, made first.
                (_, Some(BoardRow::Empty { .. })) if self.scoped => self.add_line(None, self.scope.clone()),
                _ => self.add_line(None, None),
            }
            return;
        }
        if text == "A" {
            self.add_line(None, None);
            return;
        }
        if text == "E" {
            match selected_line {
                Some((line, _)) => self.open_note_in_editor(WorkNoteKind::Line, line.id.clone(), props, cx),
                None => self.open_list_in_editor(props, cx),
            }
            return;
        }
        if matches!(current_row, Some(BoardRow::Older { .. })) && (key.return_ || text == " ") {
            self.show_older = !self.show_older;
            return;
        }
        let Some((line, section)) = selected_line else {
            if matches!(text, "n" | "o" | "x" | "w" | " " | "c" | "J" | "K") || key.return_ {
                cx.set_status(Some("Select a line first (j/k)".to_owned()));
            }
            return;
        };
        // c: its lines hidden or shown (on a line without any, its parent's, which takes the selection).
        if text == "c" {
            let target = if props.list.count(&line.id).total > 0 {
                Some(line.id.clone())
            } else {
                props.list.parent(&line.id).map(|parent| parent.id.clone())
            };
            let Some(target) = target else {
                cx.set_status(Some("That line has no lines under it to hide".to_owned()));
                return;
            };
            if self.collapsed_steps.contains(&target) {
                self.collapsed_steps.retain(|id| *id != target);
            } else {
                self.collapsed_steps.push(target.clone());
            }
            self.selected = RowSelection::line(&target);
            return;
        }
        if key.return_ {
            self.edit_line(line, TaskEditorField::Title, props, cx);
            return;
        }
        if text == " " {
            let message = if line.done {
                format!("Reopened {}", line.title)
            } else {
                format!("Done: {}", line.title)
            };
            let then: ApplyThen = Box::new(move |_, _, cx| cx.set_status(Some(message)));
            self.apply(TaskOp::Toggle { id: line.id.clone() }, props, cx, Some(then));
            return;
        }
        if text == "n" {
            if line.done {
                cx.set_status(Some("That line is done; space reopens it".to_owned()));
            } else if line.is_taken() {
                cx.set_status(Some("Someone is on it already (o selects the session)".to_owned()));
            } else {
                let parent = match start_on(props.list, &line.id, props.sessions) {
                    StartOn::TopLevel => None,
                    StartOn::SubSessionOf(id) => Some(id),
                };
                cx.emit(FlowEffect::StartTask {
                    task: Box::new(line.task()),
                    parent,
                });
            }
            return;
        }
        if text == "w" {
            if line.done {
                cx.set_status(Some("That line is done; space reopens it".to_owned()));
                return;
            }
            self.open_picker(
                TaskPicker {
                    line_id: line.id.clone(),
                    title: line.title.clone(),
                    view: MoveView::Lines,
                    query: String::new(),
                    typing: false,
                    index: 0,
                },
                props,
            );
            return;
        }
        if text == "o" {
            let state = line_state(line, props.sessions, props.spinner_frame);
            match state.session_id {
                Some(id) => cx.emit(FlowEffect::GoTo(id)),
                None => cx.set_status(Some(
                    if line.is_taken() {
                        "Its session is not in this list (archived or filtered?)"
                    } else {
                        "Nobody is on it; n starts a session"
                    }
                    .to_owned(),
                )),
            }
            return;
        }
        if text == "x" {
            let under = props.list.subtree(&line.id).len().saturating_sub(1);
            let with = if under > 0 {
                format!(" and the {} under it", lines_count(under))
            } else {
                String::new()
            };
            if deleting.as_deref() == Some(line.id.as_str()) {
                let message = format!("Removed {}{with}", line.title);
                let then: ApplyThen = Box::new(move |_, _, cx| cx.set_status(Some(message)));
                self.apply(TaskOp::Remove { id: line.id.clone() }, props, cx, Some(then));
                return;
            }
            self.pending_delete.arm(line.id.clone());
            cx.set_status(Some(format!("x again removes “{}”{with}", truncate(&line.title, 40))));
            return;
        }
        if text == "J" || text == "K" {
            if section == Section::Done {
                cx.set_status(Some("Finished lines are listed by when they were done".to_owned()));
                return;
            }
            let op = TaskOp::Reorder {
                id: line.id.clone(),
                down: text == "J",
            };
            self.apply(op, props, cx, None);
        }
    }

    fn render(&mut self, props: &TasksFlowProps<'_>, buf: &mut Buffer, area: Rect) {
        let width = usize::from(area.width);
        let height = usize::from(area.height);
        let inner = minus(width, 4).max(10);
        let rows = self.rows(props);
        let current = self.current(&rows);
        let current_row = current.and_then(|index| rows.get(index));
        let scope = self.board_scope(props);
        let name = match scope {
            BoardScope::Under(root) => props.list.get(root).map(|root| root.title.clone()).unwrap_or_default(),
            BoardScope::Nothing => self
                .scope
                .as_deref()
                .and_then(|id| props.sessions.iter().find(|session| session.id == id))
                .map(|session| session.title.clone())
                .unwrap_or_default(),
            BoardScope::All => {
                let base = path::basename(props.repo_root);
                if base.is_empty() {
                    props.repo_root.to_owned()
                } else {
                    base
                }
            }
        };
        let right = if self.view == TasksView::Notes {
            let written = rows
                .iter()
                .filter(|row| matches!(row, BoardRow::NoteHead { empty: false, .. }))
                .count();
            format!(
                "{written} note{}{}{}",
                if written == 1 { "" } else { "s" },
                if self.with_text { " · with text" } else { "" },
                if self.scoped { " · v all" } else { "" }
            )
        } else if self.scoped {
            let open = rows
                .iter()
                .filter(|row| matches!(row, BoardRow::Line { line, .. } if !line.done))
                .count();
            format!("{open} open · v all lines")
        } else {
            let open = props.list.open_count();
            let on = props
                .list
                .lines()
                .iter()
                .filter(|line| !line.done && line.is_taken())
                .count();
            format!("{open} open · {on} with a session")
        };
        let lower = if let Some(picker) = self.picker.clone() {
            Lower::of(Self::picker_lines(&picker, props, inner))
        } else if let Some(editor) = self.editor.clone() {
            self.editor_lines(&editor, props, inner)
        } else {
            Lower::of(detail_lines(current_row, props, inner))
        };
        // Border (2), title and status lines (2), the `+N more` line (1), then the rule and the lower part.
        let lower_rows = if lower.nodes > 0 { lower.nodes + 1 } else { 0 };
        let list_rows = to_index((signed(height) - 2 - 3 - signed(lower_rows)).max(1));
        self.list_rows = list_rows;
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
                    "The list is too large to change here; E on an empty row opens it in your editor",
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

    /// The board's footer.
    fn hint(&self, props: &TasksFlowProps<'_>, width: usize) -> String {
        if let Some(editor) = &self.editor {
            let parts: Vec<HintPart> = match (editor.kind, editor.field) {
                (TaskEditorKind::Scratchpad, _) => vec![
                    "enter new line".into(),
                    hint("ctrl+p checkbox → line", Some("ctrl+p → line"), Some(1)),
                    "ctrl+s save".into(),
                    "esc save".into(),
                ],
                (_, TaskEditorField::Title) => vec!["enter save".into(), "tab note".into(), "esc save".into()],
                (TaskEditorKind::Edit, TaskEditorField::Note) => vec![
                    "enter new line".into(),
                    hint("ctrl+p checkbox → line", Some("ctrl+p → line"), Some(1)),
                    "ctrl+s save".into(),
                    "tab title".into(),
                    "esc save".into(),
                ],
                (_, TaskEditorField::Note) => vec![
                    "enter new line".into(),
                    "ctrl+s save".into(),
                    "tab title".into(),
                    "esc save".into(),
                ],
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
                    hint("tab lines / sessions", Some("tab view"), None),
                    if searched { "esc clear search" } else { "esc cancel" }.into(),
                ]
            };
            return fit_hint(&parts, width, HINT_SEPARATOR);
        }
        let rows = self.rows(props);
        let current_row = self.current(&rows).and_then(|index| rows.get(index));
        let has_scope = self.scope.as_deref().is_some_and(|scope| !scope.is_empty());
        let view_hint = |all: &str| {
            hint(
                if self.scoped { all } else { "v this session's line" },
                Some("v view"),
                Some(1),
            )
        };
        if self.view == TasksView::Notes {
            let mut parts = Vec::new();
            if let Some(BoardRow::NoteHead { note, .. } | BoardRow::NoteLine { note, .. }) = current_row {
                parts.push(match note {
                    NoteOf::Session(_) => hint("enter open its Notes tab", Some("enter open"), None),
                    _ => hint("enter edit", None, None),
                });
                parts.push(hint("E open in editor", Some("E editor"), Some(2)));
            }
            parts.push(hint(
                if self.with_text {
                    "f every note"
                } else {
                    "f only with text"
                },
                Some("f filter"),
                Some(1),
            ));
            if has_scope {
                parts.push(view_hint("v all notes"));
            }
            parts.push("tab list".into());
            return fit_hint(&parts, width, HINT_SEPARATOR);
        }
        let edit_list = hint("E open in editor", Some("E editor"), Some(1));
        let back = hint("esc back", None, Some(2));
        let Some(BoardRow::Line {
            line,
            section,
            count,
            folded,
            ..
        }) = current_row
        else {
            let mut parts = vec![hint("a add line", Some("a add"), None)];
            if has_scope {
                parts.push(view_hint("v all lines"));
            }
            parts.extend([edit_list, back]);
            return fit_hint(&parts, width, HINT_SEPARATOR);
        };
        let mut parts = vec![if *section == Section::Open {
            hint("a add under", Some("a add"), None)
        } else {
            hint("a add line", Some("a add"), None)
        }];
        parts.push(hint("A top level", Some("A top"), Some(2)));
        if !line.done && !line.is_taken() {
            parts.push(hint("n start session", Some("n start"), None));
        }
        if line.is_taken() {
            parts.push(hint("o its session", Some("o session"), Some(1)));
        }
        if !line.done {
            parts.push(hint("w move", None, None));
        }
        parts.push(hint("enter edit", None, Some(1)));
        parts.push(hint(
            if line.done { "space reopen" } else { "space done" },
            Some("space ✓"),
            None,
        ));
        if count.is_some() {
            let (text, short) = if *folded {
                ("c show lines", "c show")
            } else {
                ("c hide lines", "c hide")
            };
            parts.push(hint(text, Some(short), Some(2)));
        }
        if has_scope {
            parts.push(view_hint("v all lines"));
        }
        parts.push(hint("x remove", None, Some(2)));
        parts.push(hint("tab notes", None, Some(1)));
        parts.push(hint("E note in editor", Some("E editor"), Some(1)));
        parts.push(back);
        fit_hint(&parts, width, HINT_SEPARATOR)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::app::event_loop::Dispatch;
    use crate::primitives::{row_text, string_width};
    use deckhand_protocol::data::{TaskMeta, WorkNote};
    use deckhand_protocol::events::WorkNoteUpdated;
    use ratatui::style::Color;

    const TASK_ID: &str = "a1b2c3";
    const DATE: &str = "2026-10-19";

    /// The walkthrough of the design: an idea an orchestrator is on, two sessions on lines under it, steps, a
    /// sub-session, a line nobody is on that was tried; a backlog line, a finished tree this week and an older one.
    const LIST: &str = "- [ ] Rework auth <!-- dh:t=idea s=orch -->\n\
        \x20 - [ ] Token storage <!-- dh:t=tok s=token -->\n\
        \x20   - [x] schema <!-- dh:t=sch done=2026-11-13 -->\n\
        \x20   - [ ] encryption <!-- dh:t=enc -->\n\
        \x20   - [ ] refresh tokens <!-- dh:t=ref s=refresh -->\n\
        \x20 - [ ] Login UI <!-- dh:t=ui s=login -->\n\
        \x20 - [ ] Remove legacy endpoint <!-- dh:t=leg tried=old-auth -->\n\
        - [ ] Rate-limit export <!-- dh:t=rate -->\n\
        - [x] Shipped parser <!-- dh:t=ship done=2099-01-01 auto=merge -->\n\
        - [x] Old thing <!-- dh:t=old done=2020-10-01 -->\n";

    fn record(id: &str, fields: Value) -> SessionRecord {
        let mut base = json!({"id": id, "title": id, "cwd": "/repo", "repoRoot": "/repo", "program": "claude",
            "command": "claude", "status": "running", "agentStatus": "idle", "createdAt": "2026-11-01T00:00:00Z",
            "updatedAt": "2026-11-01T00:00:00Z"});
        if let (Some(base), Some(fields)) = (base.as_object_mut(), fields.as_object()) {
            base.extend(fields.clone());
        }
        serde_json::from_value(base).unwrap_or_else(|error| panic!("a session: {error}"))
    }

    fn walkthrough_sessions() -> Vec<SessionRecord> {
        vec![
            record("orch", json!({"title": "Rework auth"})),
            record(
                "token",
                json!({"title": "Token storage", "parentSessionId": "orch", "cwd": "/repo/wt/tok",
                    "worktree": {"mode": "managed", "id": "w1", "path": "/repo/wt/tok", "branch": "tok"},
                    "notes": "Review crypto.rs before merging\n", "notesFile": {"path": "/n/token.md", "revision": "sn"}}),
            ),
            record(
                "refresh",
                json!({"title": "Refresh tokens", "parentSessionId": "token", "agentStatus": "active"}),
            ),
            record(
                "login",
                json!({"title": "Login UI", "parentSessionId": "orch", "attention": {"state": "needs-input",
                    "event": "x", "at": "2026-11-14T00:00:00.000Z"}}),
            ),
            record("loose", json!({"title": "No line yet", "parentSessionId": "token"})),
        ]
    }

    fn work_note(kind: WorkNoteKind, id: &str, text: &str, revision: &str) -> WorkNote {
        WorkNote {
            kind,
            id: id.into(),
            path: format!("/notes/{id}.md"),
            text: text.into(),
            revision: revision.into(),
            too_large: None,
            from: None,
            at: None,
        }
    }

    fn doc(text: &str, notes: Option<Vec<WorkNote>>) -> TasksDoc {
        TasksDoc {
            key: "k".to_owned(),
            path: "/notes/tasks/k.md".to_owned(),
            text: text.to_owned(),
            revision: "r".to_owned(),
            too_large: None,
            notes,
        }
    }

    /// The walkthrough's notes as `watch-tasks` answers them: Token storage's brief and note, and the scratchpad.
    fn walkthrough_note_list() -> Vec<WorkNote> {
        let mut brief = work_note(
            WorkNoteKind::Brief,
            "tok",
            "Store tokens encrypted at rest.\nDone when tests pass.",
            "b1",
        );
        brief.from = Some("◎ Rework auth".into());
        brief.at = Some("2026-11-14T09:00:00.000Z".into());
        vec![
            brief,
            work_note(
                WorkNoteKind::Line,
                "tok",
                "- [x] chose AES-GCM\nkeys live in the keychain\n",
                "l1",
            ),
            work_note(
                WorkNoteKind::Scratchpad,
                "k",
                "# Ideas\n- [ ] cache the JWKS\nloose thought\n",
                "s1",
            ),
        ]
    }

    fn walkthrough_notes() -> NoteMap {
        let mut map = NoteMap::default();
        map.load(&doc(LIST, Some(walkthrough_note_list())));
        map
    }

    /// A board's inputs, kept for the props to borrow.
    struct Board {
        list: TaskList,
        notes: NoteMap,
        doc: TasksDoc,
        sessions: Vec<SessionRecord>,
    }

    impl Board {
        fn new(text: &str, notes: NoteMap, sessions: Vec<SessionRecord>) -> Board {
            Board {
                list: TaskList::parse(text),
                notes,
                doc: doc(text, None),
                sessions,
            }
        }

        fn walkthrough() -> Board {
            Board::new(LIST, walkthrough_notes(), walkthrough_sessions())
        }
    }

    fn props<'a>(board: &'a Board, refs: &'a [&'a SessionRecord]) -> TasksFlowProps<'a> {
        TasksFlowProps {
            client: None,
            repo_root: "<TMP>/demo",
            doc: Some(&board.doc),
            list: &board.list,
            notes: &board.notes,
            sessions: refs,
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

    fn ctrl(letter: &str) -> Input {
        Input::key(
            letter,
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
    // Snapshots: the right pane of the 130x36 app, its footer hint and its styles (fixtures/ui/panes).

    const PANE_W: u16 = 98;
    const PANE_H: u16 = 31;

    fn render(flow: &mut TasksFlow, props: &TasksFlowProps<'_>) -> Buffer {
        let area = Rect::new(0, 0, PANE_W, PANE_H);
        let mut buf = Buffer::empty(area);
        flow.render(props, &mut buf, area);
        buf
    }

    pub(crate) fn buffer_rows(buf: &Buffer) -> Vec<String> {
        (0..buf.area.height)
            .map(|y| {
                let mut line = String::new();
                let mut x = 0;
                while x < buf.area.width {
                    let symbol = buf[(x, y)].symbol();
                    line.push_str(symbol);
                    x += u16::try_from(string_width(symbol).max(1)).unwrap_or(1);
                }
                line.trim_end().to_owned()
            })
            .collect()
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

    /// A cell's style as snapshot text (`fg=8 bold`), empty for the default style.
    fn cell_style(buf: &Buffer, x: u16, y: u16) -> String {
        use ratatui::style::Modifier;
        let cell = &buf[(x, y)];
        let color = |name: &str, color: Color| match (color, palette(color)) {
            (Color::Reset, _) => None,
            (_, Some(index)) => Some(format!("{name}={index}")),
            (other, None) => Some(format!("{name}={other:?}")),
        };
        let modifiers = [
            (Modifier::BOLD, "bold"),
            (Modifier::DIM, "dim"),
            (Modifier::ITALIC, "italic"),
            (Modifier::UNDERLINED, "underline"),
            (Modifier::REVERSED, "inverse"),
        ];
        [color("fg", cell.fg), color("bg", cell.bg)]
            .into_iter()
            .flatten()
            .chain(
                modifiers
                    .into_iter()
                    .filter(|(modifier, _)| cell.modifier.contains(*modifier))
                    .map(|(_, name)| name.to_owned()),
            )
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Per row, the runs of cells whose style is not the default: `row 3: 0..12 fg=8 dim, 14..20 bold`.
    pub(crate) fn style_rows(buf: &Buffer) -> Vec<String> {
        (0..buf.area.height)
            .filter_map(|y| {
                let mut runs: Vec<(u16, u16, String)> = Vec::new();
                for x in 0..buf.area.width {
                    let style = cell_style(buf, x, y);
                    match runs.last_mut() {
                        Some((_, end, last)) if *end == x && *last == style => *end = x + 1,
                        _ if style.is_empty() => {}
                        _ => runs.push((x, x + 1, style)),
                    }
                }
                let runs: Vec<String> = runs
                    .into_iter()
                    .map(|(start, end, style)| format!("{start}..{end} {style}"))
                    .collect();
                (!runs.is_empty()).then(|| format!("row {y}: {}", runs.join(", ")))
            })
            .collect()
    }

    /// The pane rendered by `flow`, its hint and its styles match the snapshot `fixtures/ui/panes/<name>.txt`.
    fn assert_pane(name: &str, flow: &mut TasksFlow, props: &TasksFlowProps<'_>) {
        let buf = render(flow, props);
        let mut text = buffer_rows(&buf);
        text.push("── hint".to_owned());
        text.push(flow.hint(props, 130));
        text.push("── styles".to_owned());
        text.extend(style_rows(&buf));
        deckhand_testkit::assert_snapshot(format!("fixtures/ui/panes/{name}.txt"), &text.join("\n"));
    }

    fn editor(kind: TaskEditorKind, field: TaskEditorField, title: &str, note: &str) -> TaskEditor {
        TaskEditor {
            kind,
            id: (kind == TaskEditorKind::Edit).then(|| TASK_ID.to_owned()),
            parent: None,
            session: None,
            field,
            title: EditorState {
                text: title.to_owned(),
                cursor: title.len(),
                ..EditorState::default()
            },
            note: EditorState {
                text: note.to_owned(),
                cursor: note.len(),
                ..EditorState::default()
            },
            note_before: String::new(),
            revision: deckhand_core::notes::note_revision(""),
        }
    }

    fn one_line() -> String {
        format!("- [ ] Write the docs <!-- dh:t={TASK_ID} added={DATE} -->\n")
    }

    #[test]
    fn the_empty_board_matches_the_snapshot() {
        let board = Board::new("", NoteMap::default(), Vec::new());
        let props = props(&board, &[]);
        let mut flow = TasksFlow::default();
        flow.open(None, &props);
        assert_pane("15-tasks-empty", &mut flow, &props);
    }

    #[test]
    fn the_new_line_editor_matches_the_snapshots() {
        let board = Board::new("", NoteMap::default(), Vec::new());
        let props = props(&board, &[]);
        let mut flow = TasksFlow {
            editor: Some(editor(
                TaskEditorKind::Add,
                TaskEditorField::Title,
                "Write the docs",
                "",
            )),
            ..TasksFlow::default()
        };
        assert_pane("16-task-new", &mut flow, &props);
        flow.editor = Some(editor(
            TaskEditorKind::Add,
            TaskEditorField::Note,
            "Write the docs",
            "why: people ask",
        ));
        assert_pane("17-task-new-details", &mut flow, &props);
    }

    #[test]
    fn the_board_with_a_line_matches_the_snapshots() {
        let mut notes = NoteMap::default();
        notes.load(&doc(
            &one_line(),
            Some(vec![work_note(WorkNoteKind::Line, TASK_ID, "why: people ask\n", "n1")]),
        ));
        let board = Board::new(&one_line(), notes, Vec::new());
        let props = props(&board, &[]);
        let mut flow = TasksFlow {
            selected: RowSelection::line(TASK_ID),
            ..TasksFlow::default()
        };
        assert_pane("18-tasks-board", &mut flow, &props);
        // Enter: the title and the line's note, filled in, the note's revision kept for the save.
        let mut cx = cx();
        press(&mut flow, &props, &mut cx, key(|key| key.return_ = true));
        let editing = flow.editor.clone().unwrap_or_else(|| panic!("the editor"));
        assert_eq!(
            (
                editing.kind,
                editing.field,
                editing.title.text.as_str(),
                editing.note.text.as_str()
            ),
            (
                TaskEditorKind::Edit,
                TaskEditorField::Title,
                "Write the docs",
                "why: people ask\n"
            )
        );
        assert_eq!(editing.revision, "n1");
        assert_pane("19-task-edit", &mut flow, &props);
        // Esc (nothing changed) closes; w: the move menu, on the backlog where it is.
        press(&mut flow, &props, &mut cx, key(|key| key.escape = true));
        assert_eq!(flow.editor, None);
        typed(&mut flow, &props, &mut cx, "w");
        assert_eq!(
            flow.picker,
            Some(TaskPicker {
                line_id: TASK_ID.to_owned(),
                title: "Write the docs".to_owned(),
                view: MoveView::Lines,
                query: String::new(),
                typing: false,
                index: 0,
            })
        );
        assert_pane("20-task-move", &mut flow, &props);
        // Esc, then tab: the notes view.
        press(&mut flow, &props, &mut cx, key(|key| key.escape = true));
        assert_eq!(flow.picker, None);
        press(&mut flow, &props, &mut cx, key(|key| key.tab = true));
        assert_eq!(flow.view, TasksView::Notes);
        assert_pane("21-tasks-notes", &mut flow, &props);
        assert!(effects(&mut cx).is_empty());
    }

    #[test]
    fn a_nested_board_matches_the_snapshot() {
        let board = Board::walkthrough();
        let refs: Vec<&SessionRecord> = board.sessions.iter().collect();
        let props = props(&board, &refs);
        let mut flow = TasksFlow::default();
        flow.open(Some("token"), &props);
        // b from a session selects its line.
        assert_eq!(flow.selected, RowSelection::line("tok"));
        assert_pane("22-tasks-nested", &mut flow, &props);
        // v: only that line's tree.
        let mut cx = cx();
        typed(&mut flow, &props, &mut cx, "v");
        let shown: Vec<String> = flow
            .rows(&props)
            .iter()
            .filter_map(|row| match row {
                BoardRow::Line { line, depth, .. } => Some(format!("{}{}", "  ".repeat(*depth), line.title)),
                _ => None,
            })
            .collect();
        assert_eq!(shown, ["Token storage", "  schema", "  encryption", "  refresh tokens"]);
    }

    #[test]
    fn the_notes_view_matches_the_snapshot() {
        let board = Board::walkthrough();
        let refs: Vec<&SessionRecord> = board.sessions.iter().collect();
        let props = props(&board, &refs);
        let mut flow = TasksFlow {
            view: TasksView::Notes,
            with_text: true,
            ..TasksFlow::default()
        };
        assert_pane("65-tasks-notes-view", &mut flow, &props);
    }

    #[test]
    fn rows_show_who_is_on_a_line_its_count_and_its_brief() {
        let board = Board::walkthrough();
        let refs: Vec<&SessionRecord> = board.sessions.iter().collect();
        let props = props(&board, &refs);
        let flow = TasksFlow::default();
        let text = |id: &str| -> String {
            let rows = flow.rows(&props);
            let row = rows
                .iter()
                .find(|row| matches!(row, BoardRow::Line { line, .. } if line.id == id))
                .unwrap_or_else(|| panic!("no row {id}"));
            row_text(&board_line(row, false, 90, props.sessions, "⠋"))
                .trim_end()
                .to_owned()
        };
        let token = text("tok");
        assert!(token.starts_with("    ☐ Token storage"), "{token}");
        assert!(token.ends_with("Token storage ● idle  1/3 ✎"), "{token}");
        assert!(text("idea").ends_with("Rework auth ● idle  0/3"));
        assert!(text("ref").ends_with("Refresh tokens ⠋ working"));
        assert!(text("ui").ends_with("Login UI ? needs you"));
        assert!(text("leg").ends_with("tried in ⎇ old-auth"));
        assert!(text("sch").starts_with("      ☑ schema"));
        assert!(text("ship").ends_with("✓ merged 2099-01-01"));
        // Nothing on the right of a backlog line.
        assert_eq!(text("rate"), "  ☐ Rate-limit export");
        assert_eq!(string_width(&text("tok")), 90);
    }

    #[test]
    fn the_task_banner_counts_its_lines() {
        let task = Task {
            id: TASK_ID.to_owned(),
            title: "Write the docs".to_owned(),
            done: false,
            body: String::new(),
            meta: TaskMeta {
                t: Some(TASK_ID.to_owned()),
                added: Some(DATE.to_owned()),
                ..TaskMeta::default()
            },
        };
        let props = TaskBannerProps {
            task: &task,
            more: 0,
            steps: Some(StepCount { done: 1, total: 3 }),
            sessions: &[],
            spinner_frame: "⠋",
        };
        let rows = task_banner_rows(&props, 80);
        let first = row_text(&rows[0]);
        assert!(first.starts_with("◆ Task Write the docs"), "{first}");
        assert!(
            first.ends_with(&format!("1/3 steps · added {DATE} · b board")),
            "{first}"
        );
        assert_eq!(row_text(&rows[1]), "─".repeat(80));
    }

    #[test]
    fn the_keymap_is_in_the_tasks_topic() {
        assert!(crate::keymap::undocumented_in_topic(KEYMAP, "Tasks").is_empty());
        assert!(crate::keymap::unknown_topics(KEYMAP).is_empty());
    }

    #[test]
    fn parse_added_finds_the_new_id() {
        let before: HashSet<String> = ["aa".to_owned()].into_iter().collect();
        let next = doc("- [ ] A <!-- dh:t=aa -->\n  - [ ] B <!-- dh:t=bb -->\n", None);
        assert_eq!(parse_added(&next, &before).as_deref(), Some("bb"));
        assert_eq!(parse_added(&doc("- [ ] A <!-- dh:t=aa -->\n", None), &before), None);
    }

    // -----------------------------------------------------------------------------------------------------------
    // Keys without the daemon

    #[test]
    fn n_on_a_nested_line_starts_a_sub_session_of_the_session_above() {
        let board = Board::walkthrough();
        let refs: Vec<&SessionRecord> = board.sessions.iter().collect();
        let props = props(&board, &refs);
        let mut flow = TasksFlow::default();
        let mut cx = cx();
        let mut n_on = |flow: &mut TasksFlow, id: &str| {
            flow.selected = RowSelection::line(id);
            typed(flow, &props, &mut cx, "n");
            effects(&mut cx)
        };
        // Under Token storage's line: a sub-session of Token storage, on that line.
        assert_eq!(
            n_on(&mut flow, "enc"),
            [r#"StartTask { task: "enc", parent: Some("token") }"#]
        );
        // Under the idea's line (nobody on the line itself): a sub-session of the orchestrator.
        assert_eq!(
            n_on(&mut flow, "leg"),
            [r#"StartTask { task: "leg", parent: Some("orch") }"#]
        );
        // A top-level line: the usual new session.
        assert_eq!(n_on(&mut flow, "rate"), [r#"StartTask { task: "rate", parent: None }"#]);
        // Someone on it, or done: said, nothing started.
        assert_eq!(
            n_on(&mut flow, "tok"),
            [r#"SetStatus(Some("Someone is on it already (o selects the session)"))"#]
        );
        assert_eq!(
            n_on(&mut flow, "sch"),
            [r#"SetStatus(Some("That line is done; space reopens it"))"#]
        );
    }

    #[test]
    fn o_selects_the_session_on_the_line_and_c_hides_its_lines() {
        let board = Board::walkthrough();
        let refs: Vec<&SessionRecord> = board.sessions.iter().collect();
        let props = props(&board, &refs);
        let mut flow = TasksFlow {
            selected: RowSelection::line("tok"),
            ..TasksFlow::default()
        };
        let mut cx = cx();
        typed(&mut flow, &props, &mut cx, "o");
        assert_eq!(effects(&mut cx), [r#"GoTo("token")"#]);
        flow.selected = RowSelection::line("rate");
        typed(&mut flow, &props, &mut cx, "o");
        assert_eq!(
            effects(&mut cx),
            [r#"SetStatus(Some("Nobody is on it; n starts a session"))"#]
        );
        // c on a line without lines folds its parent, which takes the selection; c again shows them.
        flow.selected = RowSelection::line("enc");
        typed(&mut flow, &props, &mut cx, "c");
        assert_eq!(flow.collapsed_steps, ["tok"]);
        assert_eq!(flow.selected, RowSelection::line("tok"));
        assert!(
            !flow
                .rows(&props)
                .iter()
                .any(|row| matches!(row, BoardRow::Line { line, .. } if line.id == "enc"))
        );
        assert!(flow.hint(&props, 200).contains("c show lines"));
        typed(&mut flow, &props, &mut cx, "c");
        assert!(flow.collapsed_steps.is_empty());
        // x asks first, naming what goes with it.
        typed(&mut flow, &props, &mut cx, "x");
        assert_eq!(
            effects(&mut cx),
            [r#"SetStatus(Some("x again removes “Token storage” and the 3 lines under it"))"#]
        );
        // a: a new line under the selected one; A: at the top level.
        typed(&mut flow, &props, &mut cx, "a");
        let adding = flow.editor.take().unwrap_or_else(|| panic!("the editor"));
        assert_eq!(
            (adding.kind, adding.parent.as_deref()),
            (TaskEditorKind::Add, Some("tok"))
        );
        typed(&mut flow, &props, &mut cx, "A");
        let adding = flow.editor.take().unwrap_or_else(|| panic!("the editor"));
        assert_eq!(adding.parent, None);
    }

    #[test]
    fn the_notes_view_opens_each_note_where_it_is_edited() {
        let board = Board::walkthrough();
        let refs: Vec<&SessionRecord> = board.sessions.iter().collect();
        let props = props(&board, &refs);
        let mut flow = TasksFlow {
            view: TasksView::Notes,
            ..TasksFlow::default()
        };
        let mut cx = cx();
        let enter = || key(|key| key.return_ = true);
        // The scratchpad: the board's editor on it, at its revision.
        flow.selected = RowSelection {
            key: Some("note:scratchpad".to_owned()),
            index: 0,
        };
        press(&mut flow, &props, &mut cx, enter());
        let editing = flow.editor.take().unwrap_or_else(|| panic!("the editor"));
        assert_eq!(
            (editing.kind, editing.revision.as_str()),
            (TaskEditorKind::Scratchpad, "s1")
        );
        assert!(editing.note.text.starts_with("# Ideas"));
        // A line's note: the line's editor, on its note.
        flow.selected = RowSelection {
            key: Some("note:line:tok".to_owned()),
            index: 0,
        };
        press(&mut flow, &props, &mut cx, enter());
        let editing = flow.editor.take().unwrap_or_else(|| panic!("the editor"));
        assert_eq!(
            (
                editing.kind,
                editing.field,
                editing.id.as_deref(),
                editing.revision.as_str()
            ),
            (TaskEditorKind::Edit, TaskEditorField::Note, Some("tok"), "l1")
        );
        // A session's note: its Notes tab.
        flow.selected = RowSelection {
            key: Some("noteline:session:token:0".to_owned()),
            index: 0,
        };
        press(&mut flow, &props, &mut cx, enter());
        assert_eq!(effects(&mut cx), [r#"OpenNote("token")"#]);
        // f: only notes with text; the empty lines' notes go.
        let heads = |flow: &TasksFlow| {
            flow.rows(&props)
                .iter()
                .filter(|row| matches!(row, BoardRow::NoteHead { .. }))
                .count()
        };
        let every = heads(&flow);
        typed(&mut flow, &props, &mut cx, "f");
        assert!(flow.with_text);
        assert_eq!(
            heads(&flow),
            3,
            "the scratchpad, Token storage's note, the session's note (of {every})"
        );
        // a no longer sends anything anywhere.
        typed(&mut flow, &props, &mut cx, "a");
        assert_eq!(
            effects(&mut cx),
            [r#"SetStatus(Some("The notes are for reading here: enter edits one, tab goes back to the list"))"#]
        );
        // v: the selected session's line and notes.
        flow.scope = Some("loose".to_owned());
        typed(&mut flow, &props, &mut cx, "f");
        typed(&mut flow, &props, &mut cx, "v");
        let labels: Vec<String> = flow
            .rows(&props)
            .iter()
            .filter_map(|row| match row {
                BoardRow::NoteHead { label, .. } => Some(label.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            labels,
            [
                "Token storage",
                "encryption",
                "refresh tokens",
                "Token storage",
                "No line yet"
            ]
        );
        // tab: back to the list.
        press(&mut flow, &props, &mut cx, key(|key| key.tab = true));
        assert_eq!(flow.view, TasksView::Tasks);
    }

    #[test]
    fn the_editor_needs_a_title_and_tab_switches_to_the_note() {
        let board = Board::new("", NoteMap::default(), Vec::new());
        let props = props(&board, &[]);
        let mut flow = TasksFlow::default();
        let mut cx = cx();
        typed(&mut flow, &props, &mut cx, "a");
        assert!(flow.hint(&props, 130).contains("tab note"));
        press(&mut flow, &props, &mut cx, key(|key| key.tab = true));
        assert_eq!(
            flow.editor.as_ref().map(|editor| editor.field),
            Some(TaskEditorField::Note)
        );
        typed(&mut flow, &props, &mut cx, "some thinking");
        press(&mut flow, &props, &mut cx, ctrl("s"));
        assert_eq!(effects(&mut cx), [r#"SetError(Some("A line needs a title"))"#]);
        // Esc with no title just closes.
        press(&mut flow, &props, &mut cx, key(|key| key.escape = true));
        assert_eq!(flow.editor, None);
    }

    #[test]
    fn the_hints_fit_and_name_the_lines_keys() {
        let board = Board::walkthrough();
        let refs: Vec<&SessionRecord> = board.sessions.iter().collect();
        let props = props(&board, &refs);
        let mut flow = TasksFlow {
            selected: RowSelection::line("enc"),
            scope: Some("token".to_owned()),
            ..TasksFlow::default()
        };
        let hint = flow.hint(&props, 400);
        for part in [
            "a add under",
            "n start session",
            "w move",
            "space done",
            "v this session's line",
            "x remove",
            "tab notes",
        ] {
            assert!(hint.contains(part), "{part} in {hint}");
        }
        assert!(!hint.contains("o its session"));
        flow.selected = RowSelection::line("tok");
        let on = flow.hint(&props, 400);
        assert!(on.contains("o its session") && !on.contains("n start"), "{on}");
        assert!(string_width(&flow.hint(&props, 98)) <= 98);
    }

    // -----------------------------------------------------------------------------------------------------------
    // With a daemon: what the keys send

    type Lines = tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>;
    type Updates = tokio::sync::mpsc::UnboundedReceiver<crate::app::event_loop::Update<App>>;

    /// The app on the walkthrough list, with a client whose daemon end the test plays.
    struct Wired {
        app: App,
        rx: Updates,
        lines: Lines,
        write: tokio::net::unix::OwnedWriteHalf,
    }

    impl Wired {
        fn new(sessions: Vec<SessionRecord>) -> Wired {
            let (mut app, rx) = crate::app::test_support::app(crate::app::state::AppProps::default());
            let (socket, peer) = tokio::net::UnixStream::pair().unwrap_or_else(|error| panic!("{error}"));
            app.connection.client = Some(Rc::new(LiveClient::new(
                socket,
                deckhand_client::live::LiveClientHandlers::default(),
            )));
            app.data.repo_sessions = sessions;
            // The watch answer: the list and its notes.
            app.set_tasks_doc(Some(doc(LIST, Some(walkthrough_note_list()))));
            let (read, write) = peer.into_split();
            Wired {
                app,
                rx,
                lines: tokio::io::BufReader::new(read),
                write,
            }
        }

        fn press(&mut self, input: Input) {
            self.app
                .tasks_flow_call(|flow, props, cx| flow.handle_input(props, &input, cx));
        }

        fn typed(&mut self, text: &str) {
            for ch in text.chars() {
                self.press(Input::text(ch.to_string()));
            }
        }

        fn select(&mut self, id: &str) {
            self.app.flows.tasks.selected = RowSelection::line(id);
        }

        /// The next request the client sent.
        async fn request(&mut self) -> Value {
            use tokio::io::AsyncBufReadExt;
            let mut line = String::new();
            self.lines
                .read_line(&mut line)
                .await
                .unwrap_or_else(|error| panic!("{error}"));
            serde_json::from_str(&line).unwrap_or_else(|error| panic!("{error}: {line}"))
        }

        async fn respond(&mut self, request: &Value, data: Value) {
            use tokio::io::AsyncWriteExt;
            let response = json!({"type": "response", "requestId": request["requestId"], "ok": true, "data": data});
            self.write
                .write_all(format!("{response}\n").as_bytes())
                .await
                .unwrap_or_else(|error| panic!("{error}"));
        }

        /// The task op of the next request.
        async fn op(&mut self) -> (Value, Value) {
            let request = self.request().await;
            assert_eq!(request["type"], "task-op", "{request}");
            assert_eq!(request["cwd"], "/repo");
            (request["op"].clone(), request)
        }

        /// The reply's update, applied.
        async fn settle(&mut self) {
            let update = self.rx.recv().await.unwrap_or_else(|| panic!("the reply's update"));
            update(&mut self.app);
            assert!(!self.app.flows.tasks.busy);
        }

        fn status(&self) -> Option<&str> {
            self.app.messages.status_message.as_deref()
        }
    }

    fn run(test: impl std::future::Future<Output = ()>) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| panic!("{error}"));
        tokio::task::LocalSet::new().block_on(&runtime, test);
    }

    fn list_doc(text: &str) -> Value {
        json!({"key": "k", "path": "/notes/tasks/k.md", "text": text, "revision": "r2"})
    }

    #[test]
    fn space_x_and_j_k_send_their_ops() {
        run(async {
            let mut wired = Wired::new(walkthrough_sessions());
            wired.select("tok");
            wired.press(Input::text(" "));
            let (op, request) = wired.op().await;
            assert_eq!(op, json!({"type": "toggle", "id": "tok"}));
            wired.respond(&request, list_doc(LIST)).await;
            wired.settle().await;
            assert_eq!(wired.status(), Some("Done: Token storage"));
            wired.select("rate");
            wired.press(Input::text("K"));
            let (op, request) = wired.op().await;
            assert_eq!(op, json!({"type": "reorder", "id": "rate", "down": false}));
            wired.respond(&request, list_doc(LIST)).await;
            wired.settle().await;
            wired.press(Input::text("J"));
            let (op, request) = wired.op().await;
            assert_eq!(op, json!({"type": "reorder", "id": "rate", "down": true}));
            wired.respond(&request, list_doc(LIST)).await;
            wired.settle().await;
            // x twice removes it (with its lines).
            wired.select("tok");
            wired.press(Input::text("x"));
            wired.press(Input::text("x"));
            let (op, request) = wired.op().await;
            assert_eq!(op, json!({"type": "remove", "id": "tok"}));
            wired
                .respond(&request, list_doc("- [ ] Rework auth <!-- dh:t=idea s=orch -->\n"))
                .await;
            wired.settle().await;
            assert_eq!(wired.status(), Some("Removed Token storage and the 3 lines under it"));
            assert_eq!(wired.app.data.task_list.lines().len(), 1);
        });
    }

    #[test]
    fn a_adds_a_line_under_the_selected_one_with_its_note() {
        run(async {
            let mut wired = Wired::new(walkthrough_sessions());
            wired.select("tok");
            wired.press(Input::text("a"));
            wired.typed("rotate keys");
            wired.press(key(|key| key.tab = true));
            wired.typed("monthly");
            wired.press(ctrl("s"));
            let (op, request) = wired.op().await;
            assert_eq!(
                op,
                json!({"type": "add", "title": "rotate keys", "parent": "tok", "note": "monthly"})
            );
            let next = format!("{LIST}- [ ] x <!-- dh:t=0f0f -->\n");
            wired.respond(&request, list_doc(&next)).await;
            wired.settle().await;
            assert_eq!(wired.app.flows.tasks.editor, None);
            assert_eq!(wired.app.flows.tasks.selected, RowSelection::line("0f0f"));
            assert_eq!(wired.status(), Some("Added under “Token storage”"));
        });
    }

    #[test]
    fn a_under_v_on_a_session_without_a_line_makes_its_line_first() {
        run(async {
            let mut sessions = walkthrough_sessions();
            sessions.push(record("solo", json!({"title": "Solo work"})));
            let mut wired = Wired::new(sessions);
            wired.app.flows.tasks.scope = Some("solo".to_owned());
            wired.press(Input::text("v"));
            assert!(matches!(
                wired.app.flows.tasks.rows(&props_of(&wired.app)).first(),
                Some(BoardRow::Empty { adds: true, .. })
            ));
            wired.press(Input::text("a"));
            wired.typed("first step");
            wired.press(key(|key| key.return_ = true));
            let (op, request) = wired.op().await;
            assert_eq!(op, json!({"type": "session-line", "s": "solo"}));
            let with_line = format!("{LIST}- [ ] Solo work <!-- dh:t=sl s=solo -->\n");
            wired.respond(&request, list_doc(&with_line)).await;
            let (op, request) = wired.op().await;
            assert_eq!(op, json!({"type": "add", "title": "first step", "parent": "sl"}));
            let next = format!("{with_line}  - [ ] first step <!-- dh:t=0a0a -->\n");
            wired.respond(&request, list_doc(&next)).await;
            wired.settle().await;
            assert_eq!(wired.app.flows.tasks.selected, RowSelection::line("0a0a"));
        });
    }

    /// The tasks flow's props from the app (for reading rows in a test).
    fn props_of(app: &App) -> TasksFlowProps<'_> {
        TasksFlowProps {
            client: None,
            repo_root: "/repo",
            doc: app.data.tasks_doc.as_ref(),
            list: &app.data.task_list,
            notes: &app.data.work_notes,
            sessions: &[],
            spinner_frame: "⠋",
        }
    }

    #[test]
    fn w_moves_a_line_by_the_rules_of_task_move() {
        run(async {
            let mut wired = Wired::new(walkthrough_sessions());
            // Onto a session that has a line: a step of it (under its line).
            wired.select("rate");
            wired.press(Input::text("w"));
            wired.press(key(|key| key.tab = true));
            assert_eq!(
                wired.app.flows.tasks.picker.as_ref().map(|picker| picker.view),
                Some(MoveView::Sessions)
            );
            wired.press(Input::text("/"));
            wired.typed("login");
            wired.press(key(|key| key.return_ = true));
            wired.press(key(|key| key.return_ = true));
            let (op, request) = wired.op().await;
            assert_eq!(op, json!({"type": "move-to", "id": "rate", "parent": "ui"}));
            wired.respond(&request, list_doc(LIST)).await;
            wired.settle().await;
            assert_eq!(
                wired.status(),
                Some("Now a step of the line of Login UI: Rate-limit export")
            );
            // Onto a session without a line: its line.
            wired.press(Input::text("w"));
            wired.press(key(|key| key.tab = true));
            wired.press(Input::text("/"));
            wired.typed("no line");
            wired.press(key(|key| key.return_ = true));
            wired.press(key(|key| key.return_ = true));
            let (op, request) = wired.op().await;
            assert_eq!(op, json!({"type": "session-on", "id": "rate", "s": "loose"}));
            wired.respond(&request, list_doc(LIST)).await;
            wired.settle().await;
            // Under another line.
            wired.select("leg");
            wired.press(Input::text("w"));
            wired.press(Input::text("/"));
            wired.typed("rate");
            wired.press(key(|key| key.return_ = true));
            wired.press(key(|key| key.return_ = true));
            let (op, request) = wired.op().await;
            assert_eq!(op, json!({"type": "move-to", "id": "leg", "parent": "rate"}));
            wired.respond(&request, list_doc(LIST)).await;
            wired.settle().await;
            assert_eq!(
                wired.status(),
                Some("Now under “Rate-limit export”: Remove legacy endpoint")
            );
            // A line a session is on, to the backlog: the session leaves it, then it goes to the top level.
            wired.select("tok");
            wired.press(Input::text("w"));
            // The menu opens on its parent's row; g goes to the backlog.
            assert_eq!(
                wired.app.flows.tasks.picker.as_ref().map(|picker| picker.index),
                Some(1)
            );
            wired.press(Input::text("g"));
            wired.press(key(|key| key.return_ = true));
            let (op, request) = wired.op().await;
            assert_eq!(op, json!({"type": "session-off", "s": "token"}));
            wired.respond(&request, list_doc(LIST)).await;
            let (op, request) = wired.op().await;
            assert_eq!(op, json!({"type": "move-to", "id": "tok", "parent": null}));
            wired.respond(&request, list_doc(LIST)).await;
            wired.settle().await;
            assert_eq!(wired.status(), Some("Moved to the backlog: Token storage"));
        });
    }

    #[test]
    fn a_lines_note_is_saved_against_its_revision() {
        run(async {
            let mut wired = Wired::new(walkthrough_sessions());
            wired.select("tok");
            wired.press(key(|key| key.return_ = true));
            wired.press(key(|key| key.tab = true));
            wired.typed("more");
            wired.press(ctrl("s"));
            let request = wired.request().await;
            assert_eq!(request["type"], "save-note");
            assert_eq!(request["note"], json!({"kind": "line", "id": "tok"}));
            assert_eq!(request["revision"], "l1");
            assert_eq!(request["text"], "- [x] chose AES-GCM\nkeys live in the keychain\nmore");
            // Changed elsewhere meanwhile: nothing saved, the editor keeps what was typed, the note held is theirs.
            let theirs = json!({"kind": "line", "id": "tok", "path": "/notes/lines/tok.md", "text": "theirs\n",
                "revision": "l2"});
            wired.respond(&request, json!({"saved": false, "note": theirs})).await;
            wired.settle().await;
            let editing = wired
                .app
                .flows
                .tasks
                .editor
                .clone()
                .unwrap_or_else(|| panic!("still editing"));
            assert_eq!(editing.revision, "l2");
            assert!(editing.note.text.ends_with("more"));
            assert!(
                wired
                    .app
                    .messages
                    .error
                    .as_deref()
                    .is_some_and(|error| error.contains("changed meanwhile"))
            );
            assert_eq!(
                wired
                    .app
                    .data
                    .work_notes
                    .line_note("tok")
                    .map(|note| note.text.as_str()),
                Some("theirs\n")
            );
            // Saving again names the new revision.
            wired.press(ctrl("s"));
            let request = wired.request().await;
            assert_eq!(request["revision"], "l2");
            let ours = json!({"kind": "line", "id": "tok", "path": "/notes/lines/tok.md",
                "text": request["text"], "revision": "l3"});
            wired.respond(&request, json!({"saved": true, "note": ours})).await;
            wired.settle().await;
            assert_eq!(wired.app.flows.tasks.editor, None);
            assert_eq!(wired.status(), Some("Saved the line and its note"));
            assert_eq!(
                wired
                    .app
                    .data
                    .work_notes
                    .line_note("tok")
                    .map(|note| note.revision.as_str()),
                Some("l3")
            );
            // A changed title alone is a retitle (esc saves it).
            wired.press(key(|key| key.return_ = true));
            wired.typed("!");
            wired.press(key(|key| key.escape = true));
            let (op, request) = wired.op().await;
            assert_eq!(op, json!({"type": "retitle", "id": "tok", "title": "Token storage!"}));
            wired.respond(&request, list_doc(LIST)).await;
            wired.settle().await;
            assert_eq!(wired.status(), Some("Saved the line"));
        });
    }

    #[test]
    fn ctrl_p_in_the_scratchpad_makes_a_checkbox_a_backlog_line() {
        run(async {
            let mut wired = Wired::new(walkthrough_sessions());
            wired.press(key(|key| key.tab = true));
            wired.app.flows.tasks.selected = RowSelection {
                key: Some("note:scratchpad".to_owned()),
                index: 0,
            };
            wired.press(key(|key| key.return_ = true));
            assert!(
                wired
                    .app
                    .flows
                    .tasks
                    .hint(&props_of(&wired.app), 130)
                    .contains("ctrl+p checkbox → line")
            );
            // The cursor on the checkbox's line.
            if let Some(editor) = wired.app.flows.tasks.editor.as_mut() {
                editor.note.cursor = editor.note.text.find("cache").unwrap_or(0);
            }
            wired.press(ctrl("p"));
            let request = wired.request().await;
            assert_eq!(request["type"], "promote-note-item");
            assert_eq!(request["note"], json!({"kind": "scratchpad", "id": "k"}));
            assert_eq!(
                (request["line"].clone(), request["revision"].clone()),
                (json!(1), json!("s1"))
            );
            let next = format!("{LIST}- [ ] cache the JWKS <!-- dh:t=c4c4 -->\n");
            let note = json!({"kind": "scratchpad", "id": "k", "path": "/notes/repos/k.md",
                "text": "# Ideas\nloose thought\n", "revision": "s2"});
            wired
                .respond(
                    &request,
                    json!({"tasks": list_doc(&next), "lineId": "c4c4", "note": note}),
                )
                .await;
            wired.settle().await;
            assert_eq!(wired.status(), Some("Now a line in the backlog: cache the JWKS"));
            let editing = wired
                .app
                .flows
                .tasks
                .editor
                .clone()
                .unwrap_or_else(|| panic!("still editing"));
            assert_eq!(
                (editing.note.text.as_str(), editing.revision.as_str()),
                ("# Ideas\nloose thought\n", "s2")
            );
            assert_eq!(
                wired
                    .app
                    .data
                    .work_notes
                    .scratchpad()
                    .map(|note| note.revision.as_str()),
                Some("s2")
            );
            assert!(wired.app.data.task_list.get("c4c4").is_some());
            // A line that is not a checkbox stays.
            if let Some(editor) = wired.app.flows.tasks.editor.as_mut() {
                editor.note.cursor = 0;
            }
            wired.press(ctrl("p"));
            assert_eq!(
                wired.status(),
                Some("Put the cursor on an open checklist item (- [ ] …) to make it a line")
            );
        });
    }

    #[test]
    fn the_app_holds_the_notes_of_the_watch_answer_events_and_save_answers() {
        let (mut app, _rx) = crate::app::test_support::app(crate::app::state::AppProps::default());
        app.set_tasks_doc(Some(doc(
            LIST,
            Some(vec![work_note(WorkNoteKind::Line, "tok", "first", "a")]),
        )));
        assert_eq!(
            app.data.work_notes.line_note("tok").map(|note| note.text.as_str()),
            Some("first")
        );
        // The doc kept drops the notes (they live in the map); the tree is parsed.
        assert_eq!(app.data.tasks_doc.as_ref().and_then(|doc| doc.notes.as_ref()), None);
        assert_eq!(app.data.task_list.children("idea").len(), 3);
        // tasks-updated (no notes) keeps them; a work-note-updated event replaces one.
        app.set_tasks_doc(Some(doc(LIST, None)));
        assert!(app.data.work_notes.line_note("tok").is_some());
        app.work_note_updated(WorkNoteUpdated {
            key: "k".into(),
            note: work_note(WorkNoteKind::Brief, "ui", "set up", "b"),
        });
        assert!(app.data.work_notes.brief("ui").is_some());
        // Another list's event is not this one's.
        app.work_note_updated(WorkNoteUpdated {
            key: "other".into(),
            note: work_note(WorkNoteKind::Brief, "rate", "theirs", "c"),
        });
        assert!(app.data.work_notes.brief("rate").is_none());
        // A save's answer, from a flow.
        crate::flow::FlowHost::apply_flow_effects(
            &mut app,
            vec![FlowEffect::WorkNote(Box::new(work_note(
                WorkNoteKind::Line,
                "tok",
                "saved",
                "z",
            )))],
        );
        assert_eq!(
            app.data.work_notes.line_note("tok").map(|note| note.revision.as_str()),
            Some("z")
        );
    }
}
