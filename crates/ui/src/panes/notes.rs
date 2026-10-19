//! Port of notesPane.tsx: the Notes tab's rows. The worktree's shared note above the session's own, each with a
//! header, separated by a rule; read mode shows rendered checklists from the top (`+N more lines` when cut), the
//! section being edited shows the raw Markdown with a cursor, scrolled to keep it visible. Layout is pure
//! ([`notes_layout`]) so it is unit-tested.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use deckhand_core::notes::{SectionBudget, TaskLinkLookup};
use deckhand_protocol::data::NoteSection;

use crate::primitives::Row;

/// TS: notesPane.tsx NotesSectionInput `editing`: the cursor (a byte offset) and the first row shown before this
/// render.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NotesEditing {
    pub cursor: usize,
    pub scroll_top: usize,
}

/// TS: notesPane.tsx NotesSectionInput.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NotesSectionInput {
    /// Header, longest first; the first that fits (with the checklist count) is shown.
    pub titles: Vec<String>,
    pub text: String,
    /// The one muted line of an empty section that is not being edited.
    pub empty: String,
    /// Shown after the title (warning color): read-only, too large.
    pub flag: Option<String>,
    pub editing: Option<NotesEditing>,
    /// The section E / Ctrl+O open (header in bold).
    pub active: bool,
}

/// TS: notesPane.tsx NotesPart.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NotesPart {
    pub text: String,
    pub color: Option<Color>,
    pub bold: bool,
    pub dim: bool,
    pub inverse: bool,
}

/// TS: notesPane.tsx NotesRow.
pub type NotesRow = Vec<NotesPart>;

/// TS: notesPane.tsx header: the section's header row (title, flag, checklist count).
pub fn header(section: &NotesSectionInput, width: usize, focused: bool) -> NotesRow {
    let _ = (section, width, focused);
    todo!("U4")
}

/// TS: notesPane.tsx need: body rows a section needs at `width`.
pub fn need(section: &NotesSectionInput, width: usize, links: Option<TaskLinkLookup<'_>>) -> usize {
    let _ = (section, width, links);
    todo!("U4")
}

/// TS: notesPane.tsx body's result: the rows, and the edited section's first row for the next render.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NotesBody {
    pub rows: Vec<NotesRow>,
    pub scroll_top: Option<usize>,
}

/// TS: notesPane.tsx body: a section's body in `rows` rows.
pub fn body(section: &NotesSectionInput, width: usize, rows: usize, links: Option<TaskLinkLookup<'_>>) -> NotesBody {
    let _ = (section, width, rows, links);
    todo!("U4")
}

/// TS: notesPane.tsx notesLayout's argument.
#[derive(Clone, Copy)]
pub struct NotesLayoutInput<'a> {
    pub shared: Option<&'a NotesSectionInput>,
    pub session: Option<&'a NotesSectionInput>,
    pub width: usize,
    pub height: usize,
    pub focus: Option<NoteSection>,
    /// Whether each linked task is open, done or gone (`↗` lines say so).
    pub links: Option<TaskLinkLookup<'a>>,
}

/// TS: notesPane.tsx notesLayout's result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NotesLayout {
    /// Exactly `height` rows (at least one).
    pub rows: Vec<NotesRow>,
    /// The edited section's first row, for the next render.
    pub scroll_top: Option<usize>,
    /// The rows each section's body got.
    pub bodies: SectionBudget,
}

/// TS: notesPane.tsx notesLayout: the tab's rows.
pub fn notes_layout(input: &NotesLayoutInput<'_>) -> NotesLayout {
    let _ = input;
    todo!("U4")
}

/// The styled row of a NotesRow (a blank row renders as one space, as Ink drops empty ones).
pub fn notes_row(row: &NotesRow) -> Row {
    let _ = row;
    todo!("U4")
}

/// TS: notesPane.tsx NotesRows.
pub fn render_notes_rows(rows: &[NotesRow], buf: &mut Buffer, area: Rect) {
    let _ = (rows, buf, area);
    todo!("U4")
}

/// TS: notesPane.tsx NotesMessage: one muted line.
pub fn render_notes_message(text: &str, buf: &mut Buffer, area: Rect) {
    let _ = (text, buf, area);
    todo!("U4")
}
