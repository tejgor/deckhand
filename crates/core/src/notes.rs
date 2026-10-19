//! Port of notes.ts.
//!
//! Notes as pure data: identities, revisions, Markdown checklists and the Notes tab's layout. The daemon keeps the
//! files (`deckhand_sys::notes_store`); the UI edits them (notes flow) and renders them (notes pane).
//!
//! Offsets: [`ChecklistItem::prefix`] and [`ChecklistItem::box_at`] are byte offsets into the line, which equal the
//! TS UTF-16 offsets (the marker is ASCII). Editor states use byte cursors (see `text_editor`).

use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::text_editor::EditorState;
use deckhand_protocol::data::{NoteSection, SessionRecord, SharedNoteKind};

/// TS: notes.ts MAX_NOTES_CHARS (UTF-16 units).
pub const MAX_NOTES_CHARS: usize = 50_000;

/// TS: notes.ts MAX_NOTES_LABEL.
pub const MAX_NOTES_LABEL: &str = "50 000 characters";

/// TS: notes.ts noteRevision (first 16 hex of the text's sha256).
pub fn note_revision(text: &str) -> String {
    let _ = text;
    todo!("P3")
}

/// TS: notes.ts repoNoteId (first 16 hex of the resolved root's sha256).
pub fn repo_note_id(root: &str) -> String {
    let _ = root;
    todo!("P3")
}

/// TS: notes.ts sharedNoteIdentity result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedNoteIdentity {
    pub kind: SharedNoteKind,
    pub id: String,
}

/// TS: notes.ts sharedNoteIdentity (the worktree incarnation's note, else the main checkout's; none while preparing).
pub fn shared_note_identity(session: &SessionRecord) -> Option<SharedNoteIdentity> {
    let _ = session;
    todo!("P3")
}

/// TS: notes.ts showsOwnNote.
pub fn shows_own_note(session: &SessionRecord) -> bool {
    let _ = session;
    todo!("P3")
}

/// One session note to merge into its worktree's (TS: `{title, text}`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TitledNote {
    pub title: String,
    pub text: String,
}

/// TS: notes.ts mergeIntoWorktreeNote.
pub fn merge_into_worktree_note(worktree_text: &str, notes: &[TitledNote]) -> String {
    let _ = (worktree_text, notes);
    todo!("P3")
}

/// TS: notes.ts noteKey (the UI's draft key: `session:<id>`, or `<kind>:<id>` of the shared note).
pub fn note_key(session: &SessionRecord, section: NoteSection) -> Option<String> {
    let _ = (session, section);
    todo!("P3")
}

/// TS: notes.ts ChecklistItem (`- [ ] item`; `prefix`: the marker's length with its trailing space; `box`: the
/// offset of the mark inside the brackets).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecklistItem {
    pub indent: String,
    pub bullet: String,
    pub checked: bool,
    pub prefix: usize,
    pub text: String,
    #[serde(rename = "box")]
    pub box_at: usize,
}

/// TS: notes.ts parseChecklistLine.
pub fn parse_checklist_line(line: &str) -> Option<ChecklistItem> {
    let _ = line;
    todo!("P3")
}

/// TS: notes.ts checklistCounts result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecklistCounts {
    pub open: usize,
    pub done: usize,
}

/// TS: notes.ts checklistCounts.
pub fn checklist_counts(text: Option<&str>) -> ChecklistCounts {
    let _ = text;
    todo!("P3")
}

/// TS: notes.ts toggleChecklistLine (`line` 0-based; `None` when that line is not an item).
pub fn toggle_checklist_line(text: &str, line: usize) -> Option<String> {
    let _ = (text, line);
    todo!("P3")
}

/// TS: notes.ts TaskLinkState (where the task a note's `↗` line links to is).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskLinkState {
    #[serde(rename = "open")]
    Open,
    #[serde(rename = "done")]
    Done,
    #[serde(rename = "gone")]
    Gone,
}

/// TS: notes.ts TaskLinkLookup.
pub type TaskLinkLookup<'a> = &'a dyn Fn(&str) -> TaskLinkState;

/// TS: notes.ts taskLinkSuffix (`· in Tasks` without a state).
pub fn task_link_suffix(state: Option<TaskLinkState>) -> &'static str {
    let _ = state;
    todo!("P3")
}

/// TS: notes.ts linkedTaskId (the `<!-- dh:t=<id> -->` task ID of a line).
pub fn linked_task_id(line: &str) -> Option<&str> {
    let _ = line;
    todo!("P3")
}

/// TS: notes.ts toggleChecklist (Ctrl+X).
pub fn toggle_checklist(state: &EditorState) -> EditorState {
    let _ = state;
    todo!("P3")
}

/// TS: notes.ts insertChecklistItem (Ctrl+T).
pub fn insert_checklist_item(state: &EditorState) -> EditorState {
    let _ = state;
    todo!("P3")
}

/// TS: notes.ts continueChecklist (Enter on an item; `None`: a plain newline).
pub fn continue_checklist(state: &EditorState) -> Option<EditorState> {
    let _ = state;
    todo!("P3")
}

/// TS: notes.ts NoteRow kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NoteRowKind {
    #[serde(rename = "open")]
    Open,
    #[serde(rename = "done")]
    Done,
    #[serde(rename = "heading")]
    Heading,
    #[serde(rename = "more")]
    More,
    #[serde(rename = "link")]
    Link,
}

/// TS: notes.ts NoteRow (one row of a note outside editing).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteRow {
    pub text: String,
    pub kind: Option<NoteRowKind>,
}

/// TS: notes.ts visibleText (tabs as spaces, other controls and U+2028/2029 as `?`).
pub fn visible_text(text: &str) -> String {
    let _ = text;
    todo!("P3")
}

/// TS: notes.ts noteReadRows (word-wrapped at `width`; checklist items as `☐`/`☑` with a hanging indent).
pub fn note_read_rows(text: &str, width: usize, links: Option<TaskLinkLookup<'_>>) -> Vec<NoteRow> {
    let _ = (text, width, links);
    todo!("P3")
}

/// TS: notes.ts fitReadRows (the last row says how many more there are when they do not fit).
pub fn fit_read_rows(rows: Vec<NoteRow>, height: usize) -> Vec<NoteRow> {
    let _ = (rows, height);
    todo!("P3")
}

/// TS: notes.ts MIN_SECTION_ROWS.
pub const MIN_SECTION_ROWS: usize = 3;

/// TS: notes.ts budgetSections result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionBudget {
    pub shared: usize,
    pub session: usize,
}

/// TS: notes.ts budgetSections (`shared`: the shared section's need, `None` without one).
pub fn budget_sections(
    total: usize,
    shared: Option<usize>,
    session: usize,
    focus: Option<NoteSection>,
) -> SectionBudget {
    let _ = (total, shared, session, focus);
    todo!("P3")
}

/// TS: notes.ts checklistLabel (`☐ 2 open` / `☑ 3 done` / empty).
pub fn checklist_label(text: Option<&str>) -> String {
    let _ = text;
    todo!("P3")
}

/// TS: notes.ts fitFirst (the first candidate that fits `width`, else the last one cut).
pub fn fit_first(candidates: &[String], width: usize) -> String {
    let _ = (candidates, width);
    todo!("P3")
}

/// TS: notes.ts openChecklistText (`☐ 3 open (2 worktree)`, shortened to fit; `None` without open items).
pub fn open_checklist_text(session: &SessionRecord, width: usize) -> Option<String> {
    let _ = (session, width);
    todo!("P3")
}
