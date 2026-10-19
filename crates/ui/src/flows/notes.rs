//! Port of notesFlow.tsx: the Notes tab's state. One draft per note (a shared note is one draft for every session
//! showing it, so its cursor and unsaved text follow it), debounced revision-checked saves, reloads when the file
//! changed elsewhere, and the keys of notes focus (Enter). The daemon owns the files; records carry their text and
//! revision.

use std::collections::HashMap;
use std::rc::Rc;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_client::live::LiveClient;
use deckhand_core::key::Input;
use deckhand_core::notes::TaskLinkLookup;
use deckhand_core::text_editor::EditorState;
use deckhand_protocol::data::{NoteSection, SessionRecord};
use deckhand_sys::async_util::Timer;

use crate::app::state::App;
use crate::flow::{Flow, FlowCx, FlowThen};
use crate::keymap::Keymap;

/// What [`NotesFlow::active_file`] hands the file to (TS: the promise's `.then`).
pub type FileThen = Box<dyn FnOnce(&mut App, Option<String>)>;

/// TS: notesFlow.tsx SAVE_DEBOUNCE_MS.
pub const SAVE_DEBOUNCE_MS: u64 = 300;

/// TS: notesFlow.tsx Draft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    /// The session and section it is saved through (the last one edited from).
    pub session_id: String,
    pub section: NoteSection,
    /// For a shared note: `kind:id`, so a save never lands in another note.
    pub note_id: Option<String>,
    pub text: String,
    /// Byte offset into `text`.
    pub cursor: usize,
    /// The file's text and revision as last known; `text` differs from `saved` while unsaved.
    pub saved: String,
    pub revision: String,
    /// The text of the save in flight.
    pub in_flight: Option<String>,
    pub scroll_top: usize,
}

/// TS: notesFlow.tsx NoteDoc.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NoteDoc {
    pub text: String,
    pub revision: String,
    pub read_only: bool,
    pub too_large: bool,
}

/// TS: notesFlow.tsx docOf: a session's note of `section` as last pushed (none without one).
pub fn doc_of(session: Option<&SessionRecord>, section: NoteSection) -> Option<NoteDoc> {
    let _ = (session, section);
    todo!("F3")
}

/// TS: notesFlow.tsx plural.
pub fn plural(count: i64, word: &str) -> String {
    let _ = (count, word);
    todo!("F3")
}

/// TS: notesFlow.tsx NotesFlowOptions (values; `onExit`, `setError`, `setStatusMessage` and `onTasks` are effects).
#[derive(Clone, Copy)]
pub struct NotesFlowProps<'a> {
    pub client: Option<&'a Rc<LiveClient>>,
    pub session: Option<&'a SessionRecord>,
    pub sessions: &'a [&'a SessionRecord],
    pub focused: bool,
    /// Whether each task a `↗` line links to is open, done or gone (none until the list is loaded).
    pub links: Option<TaskLinkLookup<'a>>,
}

/// TS: notesFlow.tsx useNotesFlow's state.
#[derive(Debug)]
pub struct NotesFlow {
    /// Drafts by note key (notes.ts noteKey).
    pub drafts: HashMap<String, Draft>,
    /// Debounced saves by note key.
    pub timers: HashMap<String, Timer>,
    /// The section notes focus edits (and E opens); kept while switching sessions.
    pub section: NoteSection,
    /// The editor's wrap width and body rows at the last render (TS `viewport`; 40 × 10 before one).
    pub viewport_width: usize,
    pub viewport_rows: usize,
    /// Inside a bracketed paste (`[200~` … `[201~`).
    pub pasting: bool,
    /// Focus at the last call (TS: the `[focused]` effect saves everything when focus ends).
    pub was_focused: bool,
    /// An error found while rendering (a reload discarded an unsaved edit), reported at the next call (TS
    /// `queueMicrotask(() => setError(...))`).
    pub deferred_error: Option<String>,
}

impl Default for NotesFlow {
    fn default() -> Self {
        NotesFlow {
            drafts: HashMap::new(),
            timers: HashMap::new(),
            section: NoteSection::Session,
            viewport_width: 40,
            viewport_rows: 10,
            pasting: false,
            was_focused: false,
            deferred_error: None,
        }
    }
}

/// The notes focus keys.
pub static KEYMAP: Keymap = &[];

impl NotesFlow {
    /// TS: NotesFlow.focus (Enter / →): start editing (the section last edited, the session's by default).
    pub fn focus(&mut self, props: &NotesFlowProps<'_>) {
        let _ = props;
        todo!("F3")
    }

    /// The section being shown/edited (TS `activeSection`).
    pub fn active_section(&self, props: &NotesFlowProps<'_>) -> NoteSection {
        let _ = props;
        todo!("F3")
    }

    /// TS: the `[focused]` effect: leaving notes focus saves every draft at once.
    pub fn sync(&mut self, props: &NotesFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let _ = (props, cx);
        todo!("F3")
    }

    /// TS: useNotesFlow reconcile: applies a newer version of the file (silently when nothing is unsaved).
    pub fn reconcile(draft: &mut Draft, doc: &NoteDoc, cx: &mut FlowCx<App, Self>) {
        let _ = (draft, doc, cx);
        todo!("F3")
    }

    /// TS: useNotesFlow draftFor: the draft of `session`'s `section` note (created or reconciled); its key.
    pub fn draft_for(
        &mut self,
        session: &SessionRecord,
        section: NoteSection,
        cx: &mut FlowCx<App, Self>,
    ) -> Option<String> {
        let _ = (session, section, cx);
        todo!("F3")
    }

    /// TS: useNotesFlow flush: saves the draft `key` now (or after the save in flight); `then` runs when it settled.
    pub fn flush(
        &mut self,
        key: &str,
        client: Option<&Rc<LiveClient>>,
        cx: &mut FlowCx<App, Self>,
        then: Option<FlowThen<App, NotesFlow>>,
    ) {
        let _ = (key, client, cx, then);
        todo!("F3")
    }

    /// TS: useNotesFlow schedule: save `key` SAVE_DEBOUNCE_MS after the last edit.
    pub fn schedule(&mut self, key: &str, cx: &mut FlowCx<App, Self>) {
        let _ = (key, cx);
        todo!("F3")
    }

    /// TS: NotesFlow.openActiveInEditor (E / Ctrl+O): the active section's file in Cursor / VS Code.
    pub fn open_active_in_editor(&mut self, props: &NotesFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let _ = (props, cx);
        todo!("F3")
    }

    /// TS: NotesFlow.activeFile (o): saves the active section, then `then` gets its file (none if unavailable).
    pub fn active_file(&mut self, props: &NotesFlowProps<'_>, cx: &mut FlowCx<App, Self>, then: FileThen) {
        let _ = (props, cx, then);
        todo!("F3")
    }

    /// TS: useNotesFlow promote (Ctrl+P): the cursor line's open checklist item becomes a task.
    pub fn promote(&mut self, section: NoteSection, props: &NotesFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let _ = (section, props, cx);
        todo!("F3")
    }

    /// TS: useNotesFlow edit: applies an editor result to the draft (and schedules its save).
    pub fn edit(
        &mut self,
        key: &str,
        next: EditorState,
        session: &SessionRecord,
        section: NoteSection,
        cx: &mut FlowCx<App, Self>,
    ) {
        let _ = (key, next, session, section, cx);
        todo!("F3")
    }
}

impl Flow<App> for NotesFlow {
    type Props<'a> = NotesFlowProps<'a>;

    fn handle_input(&mut self, props: &NotesFlowProps<'_>, input: &Input, cx: &mut FlowCx<App, Self>) {
        let _ = (props, input, cx);
        todo!("F3")
    }

    fn render(&mut self, props: &NotesFlowProps<'_>, buf: &mut Buffer, area: Rect) {
        let _ = (props, buf, area);
        todo!("F3")
    }

    /// TS: NotesFlow.hint: the notes-focus footer.
    fn hint(&self, props: &NotesFlowProps<'_>, width: usize) -> String {
        let _ = (props, width);
        todo!("F3")
    }
}
