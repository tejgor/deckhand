//! The Notes tab (`docs/future/work-items.md`, *Notes*): the session's line and its steps, the line's brief and note,
//! the session's own note, and for a sub-session its parent's line note. Notes focus (Enter) moves between them with
//! `tab`: on the steps their keys tick, add and rename steps (ops on the list); on a note, the editor. One draft per
//! note being edited: debounced, revision-checked saves; a newer file reloads a draft with nothing unsaved, and one with
//! unsaved text keeps it and says so. The daemon owns the files: sessions carry their own note, the app's
//! [`NoteMap`] the line notes and briefs, its [`TaskList`] the lines.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use serde_json::json;

use deckhand_client::live::{LiveClient, Reply};
use deckhand_core::display::{HintPart, error_message, fit_hint};
use deckhand_core::error::DhError;
use deckhand_core::jsstr::{js_len, js_trim};
use deckhand_core::key::{Input, Key};
use deckhand_core::notes::{
    MAX_NOTES_CHARS, MAX_NOTES_LABEL, continue_checklist, insert_checklist_item, note_revision, parse_checklist_line,
    toggle_checklist,
};
use deckhand_core::task_list::{Line, TaskList};
use deckhand_core::text_editor::{
    EditOptions, EditorState, clean_inserted_text, edit_text, line_end, line_start, replace_range,
};
use deckhand_core::work_notes::NoteMap;
use deckhand_protocol::data::{
    NoteRef, NoteSaveResult, PromoteNoteItemResult, SessionRecord, TaskOp, TasksDoc, WorkNote, WorkNoteKind,
};
use deckhand_sys::async_util::Timer;
use deckhand_sys::desktop::open_in_editor;

use crate::app::event_loop::Dispatch;
use crate::app::state::App;
use crate::flow::{Flow, FlowCx, FlowEffect, FlowHost, FlowThen};
use crate::flows::tasks::{brief_date, title_options};
use crate::keymap::{Binding, HelpRef, KeySpec, Keymap, NamedKey};
pub use crate::panes::notes::NotePart;
use crate::panes::notes::{
    FoldInput, NotesEditing, NotesLayoutInput, NotesSectionInput, StepEditing, StepRow, StepsInput, notes_layout,
    render_notes_message, render_notes_rows,
};

/// What [`NotesFlow::active_file`] hands the file to.
pub type FileThen = Box<dyn FnOnce(&mut App, Option<String>)>;

/// What [`NotesFlow::file_of`] hands its result to (the list too, when the session's line was made on the way): none
/// when there is no client or session to ask.
type FileDone = Box<dyn FnOnce(&mut App, Option<Result<(Option<TasksDoc>, String), DhError>>)>;

/// What a step op does with the list it answered.
type ListThen = Box<dyn FnOnce(&mut NotesFlow, &TasksDoc, &mut FlowCx<App, NotesFlow>)>;

/// TS: notesFlow.tsx SAVE_DEBOUNCE_MS.
pub const SAVE_DEBOUNCE_MS: u64 = 300;

/// A note changed elsewhere while it had unsaved text here.
pub const CONFLICT_MESSAGE: &str = "This note changed elsewhere meanwhile, so your edit was not saved; it is still \
                                    here, and your next edit saves it over theirs";

/// The session moved to another line while its line's note had unsaved text.
const LINE_CHANGED_MESSAGE: &str =
    "This session's line changed, so your unsaved edit to its old line's note was not saved";

/// A note being edited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    pub session_id: String,
    /// `Line`, `Brief` or `Session`.
    pub part: NotePart,
    /// The line whose note or brief it is (none: a session's note, or the note of a line the first save makes).
    pub line_id: Option<String>,
    pub text: String,
    /// Byte offset into `text`.
    pub cursor: usize,
    /// The file's text as last saved or loaded, and the revision a save names; `text` differs from `saved` while
    /// unsaved.
    pub saved: String,
    pub revision: String,
    /// The text of the save in flight.
    pub in_flight: Option<String>,
    pub scroll_top: usize,
    /// The note changed elsewhere while this had unsaved text: nothing is saved until the next edit, which saves it
    /// over theirs.
    pub conflict: bool,
}

impl Draft {
    fn of(session_id: &str, part: NotePart, doc: &NoteDoc) -> Draft {
        Draft {
            session_id: session_id.to_owned(),
            part,
            line_id: doc.line_id.clone(),
            text: doc.text.clone(),
            cursor: doc.text.len(),
            saved: doc.text.clone(),
            revision: doc.revision.clone(),
            in_flight: None,
            scroll_top: 0,
            conflict: false,
        }
    }
}

/// A note as last heard of.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NoteDoc {
    pub text: String,
    pub revision: String,
    pub too_large: bool,
    /// The line it belongs to (a line's note or brief).
    pub line_id: Option<String>,
}

impl NoteDoc {
    fn of(note: &WorkNote, line_id: Option<String>) -> NoteDoc {
        NoteDoc {
            text: note.text.clone(),
            revision: note.revision.clone(),
            too_large: note.too_large == Some(true),
            line_id,
        }
    }
}

/// The key of the draft of `part` of a session: each session has one line, so its line's note and brief are keyed by
/// it too.
pub fn draft_key(session_id: &str, part: NotePart) -> String {
    let kind = match part {
        NotePart::Line => "line",
        NotePart::Brief => "brief",
        NotePart::Steps | NotePart::Session => "session",
    };
    format!("{kind}:{session_id}")
}

/// The note of `part` as last heard of: the session's own (once its file is known), its line's (empty, and of no line,
/// while it is on none), or its line's brief (none without one).
pub fn doc_of(session: &SessionRecord, part: NotePart, list: &TaskList, notes: &NoteMap) -> Option<NoteDoc> {
    match part {
        NotePart::Session => session.notes_file.as_ref().map(|file| NoteDoc {
            text: session.notes.clone().unwrap_or_default(),
            revision: file.revision.clone(),
            too_large: file.too_large == Some(true),
            line_id: None,
        }),
        NotePart::Line => Some(match list.line_of_session(&session.id) {
            Some(line) => NoteDoc::of(&notes.editable(WorkNoteKind::Line, &line.id), Some(line.id.clone())),
            None => NoteDoc {
                revision: note_revision(""),
                ..NoteDoc::default()
            },
        }),
        NotePart::Brief => {
            let line = list.line_of_session(&session.id)?;
            notes
                .brief(&line.id)
                .map(|brief| NoteDoc::of(brief, Some(line.id.clone())))
        }
        NotePart::Steps => None,
    }
}

/// The brief's folded row: `Brief · from <who> · <date>`.
fn brief_label(brief: &WorkNote) -> String {
    let mut label = "Brief".to_owned();
    if let Some(from) = brief.from.as_deref().filter(|from| !from.is_empty()) {
        label.push_str(&format!(" · from {from}"));
    }
    if let Some(at) = brief.at.as_deref().filter(|at| !at.is_empty()) {
        label.push_str(&format!(" · {}", brief_date(at)));
    }
    label
}

/// The Notes flow's values.
#[derive(Clone, Copy)]
pub struct NotesFlowProps<'a> {
    pub client: Option<&'a Rc<LiveClient>>,
    /// Where list ops go (the repository's list).
    pub repo_root: &'a str,
    pub session: Option<&'a SessionRecord>,
    /// The list, and its line notes and briefs.
    pub list: &'a TaskList,
    pub notes: &'a NoteMap,
    pub focused: bool,
}

impl<'a> NotesFlowProps<'a> {
    /// The line the session is on.
    fn line(&self) -> Option<&'a Line> {
        self.list.line_of_session(&self.session?.id)
    }

    /// Its steps: the lines under its line.
    fn steps(&self) -> Vec<&'a Line> {
        self.line().map_or_else(Vec::new, |line| self.list.children(&line.id))
    }

    /// Its line's brief.
    fn brief(&self) -> Option<&'a WorkNote> {
        self.notes.brief(&self.line()?.id)
    }

    /// A sub-session's parent's line, and its note (when it has text).
    fn parent_note(&self) -> Option<(&'a Line, &'a WorkNote)> {
        let parent = self.session?.parent_session_id.as_deref()?;
        let line = self.list.line_of_session(parent)?;
        Some((line, self.notes.line_note(&line.id)?))
    }
}

/// A step's title being written: a new step (`id` none), or a step renamed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepEditor {
    pub id: Option<String>,
    pub title: EditorState,
}

/// The Notes flow's state.
pub struct NotesFlow {
    /// Drafts by [`draft_key`].
    pub drafts: HashMap<String, Draft>,
    /// Debounced saves by draft key.
    pub timers: HashMap<String, Timer>,
    /// The part notes focus works on (and E opens); kept while switching sessions.
    pub part: NotePart,
    /// The selected step.
    pub step: usize,
    pub step_editor: Option<StepEditor>,
    /// Sessions whose brief, or parent's line note, is open.
    pub briefs_open: HashSet<String>,
    pub parents_open: HashSet<String>,
    /// The editor's wrap width and body rows at the last render (40 × 10 before one).
    pub viewport_width: usize,
    pub viewport_rows: usize,
    /// Inside a bracketed paste (`[200~` … `[201~`).
    pub pasting: bool,
    /// Focus at the last call (leaving notes focus saves everything).
    pub was_focused: bool,
    /// An error a call found (a reload kept or dropped an unsaved edit), reported when the call ends; a render with no
    /// dispatcher yet keeps it for the next call.
    pub deferred_error: Option<String>,
    /// A step op is in flight.
    busy: bool,
    /// The client and repository as of the last call, for work that outlives it.
    latest_client: Option<Rc<LiveClient>>,
    latest_repo_root: String,
    /// The host's dispatcher as of the last call: a render has no cx, so what it finds is sent through it.
    latest_dispatch: Option<Dispatch<App>>,
    /// What waits for each draft's save in flight.
    waiters: HashMap<String, Vec<FlowThen<App, NotesFlow>>>,
}

impl std::fmt::Debug for NotesFlow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotesFlow")
            .field("drafts", &self.drafts)
            .field("timers", &self.timers.keys().collect::<Vec<_>>())
            .field("part", &self.part)
            .field("step", &self.step)
            .field("step_editor", &self.step_editor)
            .field("viewport_width", &self.viewport_width)
            .field("viewport_rows", &self.viewport_rows)
            .field("pasting", &self.pasting)
            .field("was_focused", &self.was_focused)
            .field("deferred_error", &self.deferred_error)
            .finish_non_exhaustive()
    }
}

impl Default for NotesFlow {
    fn default() -> Self {
        NotesFlow {
            drafts: HashMap::new(),
            timers: HashMap::new(),
            part: NotePart::Session,
            step: 0,
            step_editor: None,
            briefs_open: HashSet::new(),
            parents_open: HashSet::new(),
            viewport_width: 40,
            viewport_rows: 10,
            pasting: false,
            was_focused: false,
            deferred_error: None,
            busy: false,
            latest_client: None,
            latest_repo_root: String::new(),
            latest_dispatch: None,
            waiters: HashMap::new(),
        }
    }
}

const fn notes_key(keys: &'static [KeySpec], action: &'static str) -> Binding {
    Binding {
        keys,
        action,
        help: HelpRef::Topic("Notes"),
    }
}

/// Notes focus's keys (each in the guide's Notes topic). Typing goes to the note being edited; these are the keys
/// around it and the steps' keys.
pub static KEYMAP: Keymap = &[
    notes_key(
        &[KeySpec::Named(NamedKey::Tab), KeySpec::Named(NamedKey::ShiftTab)],
        "next / previous part",
    ),
    notes_key(&[KeySpec::Ctrl("b")], "open or fold the brief"),
    notes_key(&[KeySpec::Char("j"), KeySpec::Named(NamedKey::Down)], "steps: next"),
    notes_key(&[KeySpec::Char("k"), KeySpec::Named(NamedKey::Up)], "steps: previous"),
    notes_key(&[KeySpec::Char(" ")], "steps: tick or reopen"),
    notes_key(&[KeySpec::Char("a")], "steps: add one"),
    notes_key(&[KeySpec::Named(NamedKey::Return)], "steps: rename it"),
    notes_key(&[KeySpec::Char("b")], "steps: open or fold the brief"),
    notes_key(&[KeySpec::Char("p")], "steps: open or fold the parent's note"),
    notes_key(&[KeySpec::Char("E"), KeySpec::Ctrl("o")], "the note in your editor"),
    notes_key(&[KeySpec::Ctrl("p")], "the checkbox becomes a step"),
    notes_key(&[KeySpec::Ctrl("x"), KeySpec::Ctrl("t")], "tick a checkbox / a new one"),
    notes_key(&[KeySpec::Named(NamedKey::Escape)], "done (saves)"),
];

/// `Math.min(cursor, text.length)`, kept on a character boundary.
fn clamp_cursor(cursor: usize, text: &str) -> usize {
    let mut cursor = cursor.min(text.len());
    while !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    cursor
}

/// Takes a newer version of the note into the draft. Nothing unsaved: silently. Unsaved text that the file does not
/// say: kept, the draft now naming the file's revision, so the next edit saves it over theirs (the message says so).
fn apply_reconcile(draft: &mut Draft, doc: &NoteDoc) -> Option<&'static str> {
    let unsaved = draft.text != draft.saved || draft.in_flight.is_some();
    if draft.line_id != doc.line_id {
        if draft.line_id.is_some() {
            // Another line's note now: what was typed belonged to the old one.
            let dropped = unsaved && draft.text != doc.text;
            *draft = Draft::of(&draft.session_id, draft.part, doc);
            return dropped.then_some(LINE_CHANGED_MESSAGE);
        }
        // Its line was made (by this draft's first save, or meanwhile): the draft is that line's note.
        draft.line_id.clone_from(&doc.line_id);
    }
    if doc.revision == draft.revision {
        return None;
    }
    if draft.in_flight.as_deref() == Some(doc.text.as_str()) {
        draft.saved = doc.text.clone();
        draft.revision = doc.revision.clone();
        return None;
    }
    if unsaved && draft.text != doc.text {
        draft.saved = doc.text.clone();
        draft.revision = doc.revision.clone();
        draft.conflict = true;
        return Some(CONFLICT_MESSAGE);
    }
    draft.text = doc.text.clone();
    draft.saved = doc.text.clone();
    draft.revision = doc.revision.clone();
    draft.cursor = clamp_cursor(draft.cursor, &draft.text);
    draft.conflict = false;
    None
}

/// A hint part with a short form and a drop rank.
fn hint_part(text: &str, short: Option<&str>, drop: i32) -> HintPart {
    HintPart {
        text: text.to_owned(),
        short: short.map(str::to_owned),
        drop: Some(drop),
    }
}

/// What a save answered, with the list when the session's line was made first.
struct SaveOutcome {
    tasks: Option<TasksDoc>,
    line_id: Option<String>,
    result: NoteSaveResult,
}

/// `session-line`: the session's line (made when it has none), and the list after it.
async fn make_session_line(
    client: &LiveClient,
    repo_root: &str,
    session_id: &str,
) -> Result<(TasksDoc, String), DhError> {
    let doc = client
        .task_op_value(repo_root, json!({"type": "session-line", "s": session_id}))
        .await?;
    let id = TaskList::parse(&doc.text)
        .line_of_session(session_id)
        .map(|line| line.id.clone())
        .ok_or_else(|| DhError::msg("The session's line was not made"))?;
    Ok((doc, id))
}

/// The note a part names. A line's note of a session on no line makes its line first (lines on demand); the list
/// after that comes back too.
async fn resolve_note(
    client: &LiveClient,
    repo_root: &str,
    session_id: &str,
    part: NotePart,
    line_id: Option<String>,
) -> Result<(NoteRef, Option<TasksDoc>, Option<String>), DhError> {
    let note = |kind, id: &str| NoteRef {
        kind,
        id: id.to_owned(),
    };
    match (part, line_id) {
        (NotePart::Session | NotePart::Steps, _) => Ok((note(WorkNoteKind::Session, session_id), None, None)),
        (NotePart::Brief, Some(id)) => Ok((note(WorkNoteKind::Brief, &id), None, Some(id))),
        (NotePart::Brief, None) => Err(DhError::msg("This session's line has no brief")),
        (NotePart::Line, Some(id)) => Ok((note(WorkNoteKind::Line, &id), None, Some(id))),
        (NotePart::Line, None) => {
            let (doc, id) = make_session_line(client, repo_root, session_id).await?;
            Ok((note(WorkNoteKind::Line, &id), Some(doc), Some(id)))
        }
    }
}

impl NotesFlow {
    /// Enter / →: notes focus, on the part last worked on (the session's note at first).
    pub fn focus(&mut self, props: &NotesFlowProps<'_>) {
        self.remember_props(props);
        self.part = self.active_part(props);
    }

    fn remember_props(&mut self, props: &NotesFlowProps<'_>) {
        self.latest_client = props.client.cloned();
        props.repo_root.clone_into(&mut self.latest_repo_root);
    }

    /// The parts `tab` moves through: the steps, the brief while it is open, the line's note, the session's.
    fn parts(&self, props: &NotesFlowProps<'_>) -> Vec<NotePart> {
        let mut parts = vec![NotePart::Steps];
        let brief_open = props
            .session
            .is_some_and(|session| self.briefs_open.contains(&session.id));
        if brief_open && props.brief().is_some() {
            parts.push(NotePart::Brief);
        }
        parts.extend([NotePart::Line, NotePart::Session]);
        parts
    }

    /// The part being worked on (a brief folded or gone: the line's note).
    pub fn active_part(&self, props: &NotesFlowProps<'_>) -> NotePart {
        if self.parts(props).contains(&self.part) {
            self.part
        } else {
            NotePart::Line
        }
    }

    /// Leaving notes focus saves every draft at once.
    pub fn sync(&mut self, props: &NotesFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        self.remember_props(props);
        self.remember(cx);
        let changed = self.was_focused != props.focused;
        self.was_focused = props.focused;
        if changed && !props.focused {
            self.pasting = false;
            self.step_editor = None;
            let keys: Vec<String> = self.drafts.keys().cloned().collect();
            for key in keys {
                self.flush(&key, props.client, cx, None);
            }
        }
        self.report_deferred(cx);
    }

    /// Keeps the host's dispatcher for a later render.
    fn remember(&mut self, cx: &FlowCx<App, Self>) {
        self.latest_dispatch = Some(cx.dispatch().clone());
    }

    /// Reports the error a call found.
    fn report_deferred(&mut self, cx: &mut FlowCx<App, Self>) {
        if let Some(message) = self.deferred_error.take() {
            cx.set_error(message);
        }
    }

    /// The draft of `part` of `session`, created or reconciled with the note as last heard of; its key, and what
    /// reconciling has to say.
    fn draft_core(
        &mut self,
        session: &SessionRecord,
        part: NotePart,
        props: &NotesFlowProps<'_>,
    ) -> Option<(String, Option<&'static str>)> {
        let doc = doc_of(session, part, props.list, props.notes)?;
        let key = draft_key(&session.id, part);
        if let Some(draft) = self.drafts.get_mut(&key) {
            let message = apply_reconcile(draft, &doc);
            return Some((key, message));
        }
        self.drafts.insert(key.clone(), Draft::of(&session.id, part, &doc));
        Some((key, None))
    }

    /// The draft of `part` of `session`; what reconciling had to say is reported when the call ends.
    fn draft_for(&mut self, session: &SessionRecord, part: NotePart, props: &NotesFlowProps<'_>) -> Option<String> {
        let (key, message) = self.draft_core(session, part, props)?;
        if let Some(message) = message {
            self.deferred_error = Some(message.to_owned());
        }
        Some(key)
    }

    /// What `f` gets of the draft `draft_for` would give, without touching it.
    fn peek<R>(&self, props: &NotesFlowProps<'_>, part: NotePart, f: impl FnOnce(&str, usize) -> R) -> Option<R> {
        let session = props.session?;
        let doc = doc_of(session, part, props.list, props.notes)?;
        let Some(draft) = self.drafts.get(&draft_key(&session.id, part)) else {
            return Some(f(&doc.text, doc.text.len()));
        };
        let mut reloaded = draft.clone();
        apply_reconcile(&mut reloaded, &doc);
        Some(f(&reloaded.text, reloaded.cursor))
    }

    /// Saves the draft `key` now (or after the save in flight); `then` runs when it settled. A draft in conflict is
    /// not saved until it is edited again.
    pub fn flush(
        &mut self,
        key: &str,
        client: Option<&Rc<LiveClient>>,
        cx: &mut FlowCx<App, Self>,
        then: Option<FlowThen<App, NotesFlow>>,
    ) {
        self.latest_client = client.cloned();
        self.remember(cx);
        self.timers.remove(key);
        let run_now = |flow: &mut Self, cx: &mut FlowCx<App, Self>, then: Option<FlowThen<App, NotesFlow>>| {
            if let Some(then) = then {
                then(flow, cx);
            }
        };
        let (Some(draft), Some(client)) = (self.drafts.get_mut(key), client) else {
            return run_now(self, cx, then);
        };
        if draft.in_flight.is_some() {
            if let Some(then) = then {
                self.waiters.entry(key.to_owned()).or_default().push(then);
            }
            return;
        }
        if draft.text == draft.saved || draft.conflict {
            return run_now(self, cx, then);
        }
        let text = draft.text.clone();
        draft.in_flight = Some(text.clone());
        let request: Reply<SaveOutcome> = {
            let client = Rc::clone(client);
            let repo_root = self.latest_repo_root.clone();
            let (session_id, part, line_id, revision) = (
                draft.session_id.clone(),
                draft.part,
                draft.line_id.clone(),
                draft.revision.clone(),
            );
            Box::pin(async move {
                let (note, tasks, line_id) = resolve_note(&client, &repo_root, &session_id, part, line_id).await?;
                let result = client.save_work_note(note, &text, &revision).await?;
                Ok(SaveOutcome { tasks, line_id, result })
            })
        };
        if let Some(then) = then {
            self.waiters.entry(key.to_owned()).or_default().push(then);
        }
        let key = key.to_owned();
        drop(cx.spawn(request, move |flow, result, cx| flow.saved(&key, result, cx)));
    }

    /// A save settled: saved (the draft now names the new revision), or the note had changed (it keeps what was typed
    /// and says so).
    fn saved(&mut self, key: &str, result: Result<SaveOutcome, DhError>, cx: &mut FlowCx<App, Self>) {
        let Some(draft) = self.drafts.get_mut(key) else {
            return self.settle(key, cx);
        };
        let sent = draft.in_flight.take();
        match result {
            Ok(outcome) => {
                if let Some(doc) = outcome.tasks {
                    cx.emit(FlowEffect::TasksDoc(Box::new(doc)));
                }
                if draft.line_id.is_none() {
                    draft.line_id = outcome.line_id;
                }
                let note = outcome.result.note;
                match sent {
                    // The file holds what was sent (a brief's as the daemon writes it).
                    Some(sent) if outcome.result.saved => {
                        draft.saved = sent;
                        draft.revision = note.revision.clone();
                        draft.conflict = false;
                    }
                    _ => {
                        let doc = NoteDoc::of(&note, draft.line_id.clone());
                        if let Some(message) = apply_reconcile(draft, &doc) {
                            cx.set_error(message.to_owned());
                        }
                    }
                }
                match outcome.result.session {
                    Some(session) => cx.emit(FlowEffect::Session(Box::new(session.into_inner()))),
                    None => cx.emit(FlowEffect::WorkNote(Box::new(note))),
                }
                // Typed while it was saving: save that too.
                if draft.text != draft.saved && !draft.conflict {
                    let client = self.latest_client.clone();
                    self.flush(key, client.as_ref(), cx, None);
                }
            }
            Err(error) => cx.set_error(format!("Notes not saved: {}", error_message(&error))),
        }
        self.settle(key, cx);
    }

    /// Runs what waited for the draft's saves, once none is in flight.
    fn settle(&mut self, key: &str, cx: &mut FlowCx<App, Self>) {
        if self.drafts.get(key).is_some_and(|draft| draft.in_flight.is_some()) {
            return;
        }
        for then in self.waiters.remove(key).unwrap_or_default() {
            then(self, cx);
        }
    }

    /// Saves every unsaved draft now (the UI is quitting; their debounce timers would die with it).
    pub fn flush_all(&mut self, client: Option<&Rc<LiveClient>>, cx: &mut FlowCx<App, Self>) {
        let keys: Vec<String> = self
            .drafts
            .iter()
            .filter(|(_, draft)| draft.text != draft.saved)
            .map(|(key, _)| key.clone())
            .collect();
        for key in keys {
            self.flush(&key, client, cx, None);
        }
    }

    /// A save is still in flight.
    pub fn saving(&self) -> bool {
        self.drafts.values().any(|draft| draft.in_flight.is_some())
    }

    /// Saves `key` SAVE_DEBOUNCE_MS after the last edit.
    pub fn schedule(&mut self, key: &str, cx: &mut FlowCx<App, Self>) {
        let owned = key.to_owned();
        let timer = cx.after(Duration::from_millis(SAVE_DEBOUNCE_MS), move |flow, cx| {
            let client = flow.latest_client.clone();
            flow.flush(&owned, client.as_ref(), cx, None);
        });
        // Replacing the timer cancels the old one.
        self.timers.insert(key.to_owned(), timer);
    }

    /// The note E and `o` open for `part`: the steps open their line's note, a folded brief the line's note too.
    fn file_part(&self, props: &NotesFlowProps<'_>, part: NotePart) -> NotePart {
        match part {
            NotePart::Steps => NotePart::Line,
            NotePart::Brief if props.brief().is_none() => NotePart::Line,
            other => other,
        }
    }

    /// The note of `part`'s file, saved first so the editor opens what is shown; the daemon creates the file if
    /// needed (and a line's note of a session on no line makes its line). `done` gets the file (or why not); it gets
    /// none when there is no client or session.
    fn file_of(&mut self, props: &NotesFlowProps<'_>, part: NotePart, cx: &mut FlowCx<App, Self>, done: FileDone) {
        let (Some(client), Some(session)) = (props.client, props.session) else {
            drop(cx.dispatch().spawn(async {}, move |app: &mut App, ()| done(app, None)));
            return;
        };
        let part = self.file_part(props, part);
        let key = self.draft_for(session, part, props);
        let session_id = session.id.clone();
        let line_now = props.line().map(|line| line.id.clone());
        let repo_root = props.repo_root.to_owned();
        let client = Rc::clone(client);
        let open_key = key.clone();
        let open: FlowThen<App, NotesFlow> = Box::new(move |flow, cx| {
            let line_id = open_key
                .and_then(|key| flow.drafts.get(&key))
                .and_then(|draft| draft.line_id.clone())
                .or(line_now);
            let request: Reply<(Option<TasksDoc>, String)> = Box::pin(async move {
                let (note, tasks, _) = resolve_note(&client, &repo_root, &session_id, part, line_id).await?;
                Ok((tasks, client.open_work_note(note).await?))
            });
            drop(
                cx.dispatch()
                    .spawn(request, move |app: &mut App, result| done(app, Some(result))),
            );
        });
        match key {
            Some(key) => self.flush(&key, props.client, cx, Some(open)),
            None => open(self, cx),
        }
    }

    /// The note of `part` in Cursor / VS Code.
    fn open_part(&mut self, props: &NotesFlowProps<'_>, part: NotePart, cx: &mut FlowCx<App, Self>) {
        self.file_of(
            props,
            part,
            cx,
            Box::new(|app, result| {
                let effects = match result {
                    None => return,
                    Some(Err(error)) => vec![FlowEffect::SetError(Some(error_message(&error)))],
                    Some(Ok((tasks, file))) => {
                        let errors = RefCell::new(Vec::new());
                        let label = open_in_editor(
                            &file,
                            &|message| errors.borrow_mut().push(format!("{message}; the note is {file}")),
                            None,
                        );
                        let mut effects: Vec<FlowEffect<App>> = tasks
                            .into_iter()
                            .map(|doc| FlowEffect::TasksDoc(Box::new(doc)))
                            .collect();
                        effects.extend(
                            errors
                                .into_inner()
                                .into_iter()
                                .map(|message| FlowEffect::SetError(Some(message))),
                        );
                        if let Some(label) = label {
                            effects.push(FlowEffect::SetStatus(Some(format!(
                                "Opened {file} in {label}; edits there show up here"
                            ))));
                        }
                        effects
                    }
                };
                app.apply_flow_effects(effects);
            }),
        );
    }

    /// E (browse, on Notes) / Ctrl+O: the note being worked on in Cursor / VS Code.
    pub fn open_active_in_editor(&mut self, props: &NotesFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        self.remember_props(props);
        self.remember(cx);
        let part = self.active_part(props);
        self.open_part(props, part, cx);
        self.report_deferred(cx);
    }

    /// `o` (browse, on Notes): saves the note being worked on, then `then` gets its file (none if unavailable).
    pub fn active_file(&mut self, props: &NotesFlowProps<'_>, cx: &mut FlowCx<App, Self>, then: FileThen) {
        self.remember_props(props);
        self.remember(cx);
        let part = self.active_part(props);
        self.file_of(
            props,
            part,
            cx,
            Box::new(|app, result| match result {
                Some(Ok((tasks, file))) => {
                    app.apply_flow_effects(
                        tasks
                            .into_iter()
                            .map(|doc| FlowEffect::TasksDoc(Box::new(doc)))
                            .collect(),
                    );
                    then(app, Some(file));
                }
                Some(Err(error)) => {
                    app.apply_flow_effects(vec![FlowEffect::SetError(Some(error_message(&error)))]);
                    then(app, None);
                }
                None => then(app, None),
            }),
        );
        self.report_deferred(cx);
    }

    /// Ctrl+P: the open checkbox under the cursor becomes a step of the session's line (made if needed), and leaves
    /// the note; what was typed is saved first.
    pub fn promote(&mut self, part: NotePart, props: &NotesFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let (Some(client), Some(session)) = (props.client, props.session) else {
            return;
        };
        let Some(key) = self.draft_for(session, part, props) else {
            return;
        };
        let Some(draft) = self.drafts.get(&key) else {
            return;
        };
        let before_cursor = &draft.text[..clamp_cursor(draft.cursor, &draft.text)];
        let at = before_cursor.matches('\n').count();
        let item = draft
            .text
            .split('\n')
            .nth(at)
            .and_then(parse_checklist_line)
            .filter(|item| !item.checked && !js_trim(&item.text).is_empty());
        let Some(item) = item else {
            cx.set_status("Put the cursor on an open checklist item (- [ ] …) to make it a step".to_owned());
            return;
        };
        let title = js_trim(&item.text).to_owned();
        let line = i64::try_from(at).unwrap_or(i64::MAX);
        let session_id = session.id.clone();
        let client = Rc::clone(client);
        let send: FlowThen<App, NotesFlow> = {
            let key = key.clone();
            Box::new(move |flow, cx| {
                let Some(draft) = flow.drafts.get(&key) else {
                    return;
                };
                if draft.text != draft.saved {
                    cx.set_error("These notes are not saved yet; try again".to_owned());
                    return;
                }
                let note = match (part, draft.line_id.clone()) {
                    (NotePart::Line, Some(id)) => NoteRef {
                        kind: WorkNoteKind::Line,
                        id,
                    },
                    (NotePart::Line, None) => return,
                    _ => NoteRef {
                        kind: WorkNoteKind::Session,
                        id: session_id,
                    },
                };
                let sent = draft.text.clone();
                let request = client.promote_work_note_item(note, line, &draft.revision);
                drop(cx.spawn(request, move |flow, result, cx| {
                    flow.promoted(&key, &sent, &title, result, cx)
                }));
            })
        };
        self.flush(&key, props.client, cx, Some(send));
    }

    /// Ctrl+P answered: the note without the checkbox, the list with the step.
    fn promoted(
        &mut self,
        key: &str,
        sent: &str,
        title: &str,
        result: Result<PromoteNoteItemResult, DhError>,
        cx: &mut FlowCx<App, Self>,
    ) {
        let result = match result {
            Ok(result) => result,
            Err(error) => return cx.set_error(error_message(&error)),
        };
        if let Some(draft) = self.drafts.get_mut(key) {
            let doc = NoteDoc::of(&result.note, draft.line_id.clone());
            if draft.text == sent {
                draft.text = doc.text;
                draft.saved = draft.text.clone();
                draft.revision = doc.revision;
                draft.cursor = clamp_cursor(draft.cursor, &draft.text);
                draft.conflict = false;
            } else if let Some(message) = apply_reconcile(draft, &doc) {
                cx.set_error(message.to_owned());
            }
        }
        cx.emit(FlowEffect::TasksDoc(Box::new(result.tasks)));
        match result.session {
            Some(session) => cx.emit(FlowEffect::Session(Box::new(session.into_inner()))),
            None => cx.emit(FlowEffect::WorkNote(Box::new(result.note))),
        }
        cx.set_status(format!("Now a step of this session's line: {title}"));
    }

    /// Applies an editor result to the draft (and schedules its save); an edit ends a conflict, knowingly.
    pub fn edit(&mut self, key: &str, next: EditorState, cx: &mut FlowCx<App, Self>) {
        if let Some(message) = next.message {
            cx.set_error(message);
            return;
        }
        let Some(draft) = self.drafts.get_mut(key) else {
            return;
        };
        if next.text == draft.text && next.cursor == draft.cursor {
            return;
        }
        let changed = next.text != draft.text;
        draft.text = next.text;
        draft.cursor = next.cursor;
        if changed {
            draft.conflict = false;
            self.schedule(key, cx);
        }
    }

    /// The editor's options for the notes.
    fn edit_options(&self) -> EditOptions {
        EditOptions {
            select_all: Some(false),
            max_chars: Some(MAX_NOTES_CHARS),
            limit_message: Some(format!(
                "Notes are limited to {MAX_NOTES_LABEL}; that edit was not applied."
            )),
            tab: Some(false),
            wrap_width: Some(self.viewport_width),
            page_rows: Some(self.viewport_rows.saturating_sub(1).max(1)),
        }
    }

    /// `tab` / `shift+tab`: the next or previous part.
    fn cycle(&mut self, props: &NotesFlowProps<'_>, back: bool) {
        let parts = self.parts(props);
        let current = self.active_part(props);
        let at = parts.iter().position(|part| *part == current).unwrap_or(0);
        let next = if back {
            (at + parts.len() - 1) % parts.len()
        } else {
            (at + 1) % parts.len()
        };
        self.part = parts.get(next).copied().unwrap_or_default();
    }

    /// Ctrl+B / `b` on the steps: opens or folds the brief (folding it while editing it moves to the line's note).
    fn toggle_brief(&mut self, props: &NotesFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let Some(session) = props.session else {
            return;
        };
        if props.brief().is_none() {
            cx.set_status("This session's line has no brief".to_owned());
            return;
        }
        if !self.briefs_open.remove(&session.id) {
            self.briefs_open.insert(session.id.clone());
        } else if self.part == NotePart::Brief {
            self.part = NotePart::Line;
        }
    }

    /// `p` on the steps: opens or folds the parent session's line note.
    fn toggle_parent(&mut self, props: &NotesFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        let Some(session) = props.session else {
            return;
        };
        if props.parent_note().is_none() {
            let status = if session.parent_session_id.is_some() {
                "Its parent session's line has no note"
            } else {
                "Not a sub-session: there is no parent's note"
            };
            cx.set_status(status.to_owned());
            return;
        }
        if !self.parents_open.remove(&session.id) {
            self.parents_open.insert(session.id.clone());
        }
    }

    /// A list op (or several, in `work`); the list it answers goes to the app, then `then`. One at a time.
    fn change_list(
        &mut self,
        props: &NotesFlowProps<'_>,
        cx: &mut FlowCx<App, Self>,
        work: impl FnOnce(Rc<LiveClient>, String) -> Reply<TasksDoc>,
        then: ListThen,
    ) {
        let Some(client) = props.client else {
            cx.set_error("still connecting to daemon".to_owned());
            return;
        };
        if self.busy {
            return;
        }
        self.busy = true;
        let request = work(Rc::clone(client), props.repo_root.to_owned());
        drop(cx.spawn(request, move |flow: &mut NotesFlow, result, cx| {
            flow.busy = false;
            match result {
                Ok(doc) => {
                    cx.emit(FlowEffect::TasksDoc(Box::new(doc.clone())));
                    then(flow, &doc, cx);
                }
                Err(error) => cx.set_error(error_message(&error)),
            }
        }));
    }

    /// The steps' keys.
    fn steps_key(&mut self, props: &NotesFlowProps<'_>, input: &Input, cx: &mut FlowCx<App, Self>) {
        let key = &input.key;
        let text = if key.ctrl || key.meta { "" } else { input.input.as_str() };
        let steps = props.steps();
        let selected = steps.get(self.step.min(steps.len().saturating_sub(1))).copied();
        if key.up_arrow || text == "k" {
            self.step = self.step.min(steps.len().saturating_sub(1)).saturating_sub(1);
        } else if key.down_arrow || text == "j" {
            self.step = (self.step + 1).min(steps.len().saturating_sub(1));
        } else if text == " " {
            let Some(step) = selected else {
                cx.set_status("No steps yet · a adds one".to_owned());
                return;
            };
            let message = if step.done {
                format!("Reopened {}", step.title)
            } else {
                format!("Done: {}", step.title)
            };
            let id = step.id.clone();
            self.change_list(
                props,
                cx,
                move |client, root| client.task_op(&root, TaskOp::Toggle { id }),
                Box::new(move |_, _, cx| cx.set_status(message)),
            );
        } else if text == "a" {
            self.step_editor = Some(StepEditor {
                id: None,
                title: EditorState::default(),
            });
        } else if key.return_ {
            if let Some(step) = selected {
                self.step_editor = Some(StepEditor {
                    id: Some(step.id.clone()),
                    title: EditorState {
                        cursor: step.title.len(),
                        text: step.title.clone(),
                        ..EditorState::default()
                    },
                });
            }
        } else if text == "b" {
            self.toggle_brief(props, cx);
        } else if text == "p" {
            self.toggle_parent(props, cx);
        } else if text == "E" || (key.ctrl && input.input == "o") {
            self.open_part(props, NotePart::Line, cx);
        }
    }

    /// Keys while a step's title is written: Enter saves (a new step under the session's line, made first if it has
    /// none; or the new title), Esc cancels.
    fn step_editor_key(
        &mut self,
        mut editor: StepEditor,
        props: &NotesFlowProps<'_>,
        input: &Input,
        cx: &mut FlowCx<App, Self>,
    ) {
        let key = &input.key;
        if self.pasting || !(key.escape || key.return_) {
            // A title is one line: a pasted line break or tab is a space.
            let next = if self.pasting {
                let typed = if key.return_ || key.tab {
                    " ".to_owned()
                } else {
                    clean_inserted_text(&input.input).replace(['\n', '\t'], " ")
                };
                replace_range(
                    &editor.title,
                    editor.title.cursor,
                    editor.title.cursor,
                    &typed,
                    &title_options(),
                )
            } else {
                edit_text(&editor.title, &input.input, key, &title_options())
            };
            if let Some(message) = next.message.clone() {
                cx.set_error(message);
            } else {
                editor.title = next;
            }
            self.step_editor = Some(editor);
            return;
        }
        if key.escape {
            return;
        }
        let title = js_trim(&editor.title.text).to_owned();
        let Some(session) = props.session else {
            return;
        };
        if title.is_empty() {
            return;
        }
        let session_id = session.id.clone();
        let line_id = props.line().map(|line| line.id.clone());
        match editor.id {
            Some(id) => {
                if props.list.get(&id).is_some_and(|step| step.title == title) {
                    return;
                }
                self.change_list(
                    props,
                    cx,
                    move |client, root| client.task_op(&root, TaskOp::Retitle { id, title }),
                    Box::new(|_, _, cx| cx.set_status("Renamed the step".to_owned())),
                );
            }
            None => {
                let before: HashSet<String> = props.list.lines().iter().map(|line| line.id.clone()).collect();
                let added = format!("Added a step: {title}");
                let owner = session_id.clone();
                self.change_list(
                    props,
                    cx,
                    move |client, root| {
                        Box::pin(async move {
                            let parent = match line_id {
                                Some(id) => id,
                                None => make_session_line(&client, &root, &session_id).await?.1,
                            };
                            client
                                .task_op_value(&root, json!({"type": "add", "title": title, "parent": parent}))
                                .await
                        })
                    },
                    Box::new(move |flow, doc, cx| {
                        let list = TaskList::parse(&doc.text);
                        if let Some(line) = list.line_of_session(&owner) {
                            let steps = list.children(&line.id);
                            if let Some(at) = steps.iter().position(|step| !before.contains(&step.id)) {
                                flow.step = at;
                            }
                        }
                        cx.set_status(added);
                    }),
                );
            }
        }
        self.step_editor = None;
    }

    /// The keys of notes focus.
    fn handle_key(&mut self, props: &NotesFlowProps<'_>, input: &Input, cx: &mut FlowCx<App, Self>) {
        let Some(session) = props.session else {
            cx.exit();
            return;
        };
        let key = &input.key;
        // Bracketed paste (enabled while editing): everything up to the end marker is text, Tab and Enter included.
        if input.input == "[200~" {
            self.pasting = true;
            return;
        }
        if input.input == "[201~" {
            self.pasting = false;
            return;
        }
        if let Some(editor) = self.step_editor.take() {
            self.step_editor_key(editor, props, input, cx);
            return;
        }
        let part = self.active_part(props);
        if part == NotePart::Steps {
            if self.pasting {
                return;
            }
            if key.escape {
                cx.exit();
            } else if key.tab {
                self.cycle(props, key.shift);
            } else if key.ctrl && input.input == "b" {
                self.toggle_brief(props, cx);
            } else {
                self.steps_key(props, input, cx);
            }
            return;
        }
        let draft_key = self.draft_for(session, part, props);
        let doc = doc_of(session, part, props.list, props.notes);
        let options = self.edit_options();
        let too_large = doc.as_ref().is_some_and(|doc| doc.too_large);
        let editable = draft_key.is_some() && !too_large;
        let state_of = |flow: &Self, draft_key: &str| {
            flow.drafts.get(draft_key).map(|draft| EditorState {
                text: draft.text.clone(),
                cursor: draft.cursor,
                ..EditorState::default()
            })
        };
        if self.pasting {
            let (true, Some(draft_key)) = (editable, draft_key.as_deref()) else {
                return;
            };
            let text = if key.return_ {
                "\n".to_owned()
            } else if key.tab {
                "\t".to_owned()
            } else {
                clean_inserted_text(&input.input)
            };
            if let (false, Some(state)) = (text.is_empty(), state_of(self, draft_key)) {
                let next = replace_range(&state, state.cursor, state.cursor, &text, &options);
                self.edit(draft_key, next, cx);
            }
            return;
        }
        if key.escape {
            cx.exit();
            return;
        }
        if key.tab {
            self.cycle(props, key.shift);
            return;
        }
        if key.ctrl && input.input == "b" {
            self.toggle_brief(props, cx);
            return;
        }
        if key.ctrl && input.input == "o" {
            self.open_part(props, part, cx);
            return;
        }
        let Some(draft_key) = draft_key else {
            return;
        };
        if key.ctrl && input.input == "p" {
            if part == NotePart::Brief {
                cx.set_status(
                    "A brief's checkboxes stay in it: ctrl+p works in the line's note and this session's".to_owned(),
                );
            } else if too_large {
                cx.set_error("This note is too large to change here".to_owned());
            } else {
                self.promote(part, props, cx);
            }
            return;
        }
        let Some(state) = state_of(self, &draft_key) else {
            return;
        };
        let navigation = key.left_arrow
            || key.right_arrow
            || key.up_arrow
            || key.down_arrow
            || key.home
            || key.end
            || key.page_up
            || key.page_down;
        if !editable && !navigation {
            cx.set_error(format!(
                "This note is longer than {MAX_NOTES_LABEL}: edit it in your editor (Ctrl+O)"
            ));
            return;
        }
        let mut next = if key.ctrl && input.input == "x" {
            toggle_checklist(&state)
        } else if key.ctrl && input.input == "t" {
            insert_checklist_item(&state)
        } else if key.return_ && !key.meta {
            continue_checklist(&state).unwrap_or_else(|| {
                let enter = Key {
                    return_: true,
                    ..Key::NONE
                };
                edit_text(&state, "", &enter, &options)
            })
        } else {
            edit_text(&state, &input.input, key, &options)
        };
        if let (Some(max), Some(draft)) = (options.max_chars, self.drafts.get(&draft_key)) {
            let length = js_len(&next.text);
            if length > max && length > js_len(&draft.text) {
                next = EditorState {
                    message: options.limit_message.clone(),
                    ..state
                };
            }
        }
        self.edit(&draft_key, next, cx);
    }

    /// The draft of `part` for rendering (reconciled; what that has to say goes out right after the frame).
    fn shown_draft(&mut self, session: &SessionRecord, part: NotePart, props: &NotesFlowProps<'_>) -> Option<Draft> {
        let (key, message) = self.draft_core(session, part, props)?;
        if let Some(message) = message {
            match &self.latest_dispatch {
                Some(dispatch) => dispatch.update(move |app: &mut App| {
                    app.apply_flow_effects(vec![FlowEffect::SetError(Some(message.to_owned()))]);
                }),
                None => self.deferred_error = Some(message.to_owned()),
            }
        }
        self.drafts.get(&key).cloned()
    }

    /// The empty row of a note: what is missing, and how to write it from where you are.
    fn empty_text(base: &str, part: NotePart, active: NotePart, focused: bool) -> String {
        if focused {
            format!("{base} · tab to write")
        } else if part == active {
            format!("{base} · enter to write")
        } else {
            base.to_owned()
        }
    }
}

impl Flow<App> for NotesFlow {
    type Props<'a> = NotesFlowProps<'a>;

    fn handle_input(&mut self, props: &NotesFlowProps<'_>, input: &Input, cx: &mut FlowCx<App, Self>) {
        self.remember_props(props);
        self.remember(cx);
        self.handle_key(props, input, cx);
        self.report_deferred(cx);
    }

    fn render(&mut self, props: &NotesFlowProps<'_>, buf: &mut Buffer, area: Rect) {
        let Some(session) = props.session else {
            render_notes_message("Select a session from the sidebar.", buf, area);
            return;
        };
        let (width, height) = (usize::from(area.width), usize::from(area.height));
        let focused = props.focused;
        let active = self.active_part(props);
        let editing = |part: NotePart| focused && active == part;
        let line = props.line();
        let steps = props.steps();
        let brief = props.brief();
        let line_draft = self.shown_draft(session, NotePart::Line, props);
        let session_draft = self.shown_draft(session, NotePart::Session, props);
        let brief_draft = brief.and_then(|_| self.shown_draft(session, NotePart::Brief, props));

        let step_input = StepsInput {
            line: line.map(|line| (line.title.clone(), line.done)),
            steps: steps
                .iter()
                .map(|step| StepRow {
                    title: step.title.clone(),
                    done: step.done,
                })
                .collect(),
            selected: (editing(NotePart::Steps) && !steps.is_empty()).then(|| self.step.min(steps.len() - 1)),
            editing: self.step_editor.as_ref().filter(|_| focused).map(|editor| StepEditing {
                at: editor
                    .id
                    .as_ref()
                    .and_then(|id| steps.iter().position(|step| step.id == *id)),
                title: editor.title.clone(),
            }),
            focused: editing(NotePart::Steps),
        };
        let note_editing = |draft: &Option<Draft>, part: NotePart| {
            draft.as_ref().filter(|_| editing(part)).map(|draft| NotesEditing {
                cursor: draft.cursor,
                scroll_top: draft.scroll_top,
            })
        };
        let too_large = |draft: &Option<Draft>, part: NotePart| {
            doc_of(session, part, props.list, props.notes)
                .is_some_and(|doc| doc.too_large)
                .then(|| "too large, E to edit".to_owned())
                .filter(|_| draft.is_some())
        };
        let brief_input = brief.map(|brief| {
            let label = brief_label(brief);
            FoldInput {
                open: self.briefs_open.contains(&session.id),
                section: NotesSectionInput {
                    titles: vec![format!("▾ {label}"), "▾ Brief".to_owned()],
                    text: brief_draft.as_ref().map(|draft| draft.text.clone()).unwrap_or_default(),
                    empty: "An empty brief".to_owned(),
                    flag: too_large(&brief_draft, NotePart::Brief),
                    editing: note_editing(&brief_draft, NotePart::Brief),
                    active: !focused && active == NotePart::Brief,
                },
                label,
            }
        });
        let line_input = NotesSectionInput {
            titles: match line {
                Some(line) => vec![
                    format!("{} · shared with whoever works on it", line.title),
                    format!("{} · shared", line.title),
                    line.title.clone(),
                ],
                None => vec![
                    "Its line's note · shared with whoever works on it".to_owned(),
                    "Its line's note".to_owned(),
                ],
            },
            text: line_draft.as_ref().map(|draft| draft.text.clone()).unwrap_or_default(),
            empty: Self::empty_text(
                if line.is_some() {
                    "No note for this line yet"
                } else {
                    "No line yet: writing here makes one"
                },
                NotePart::Line,
                active,
                focused,
            ),
            flag: too_large(&line_draft, NotePart::Line),
            editing: note_editing(&line_draft, NotePart::Line),
            active: !focused && active == NotePart::Line,
        };
        let session_input = NotesSectionInput {
            titles: vec![
                "This session · only this conversation".to_owned(),
                "This session".to_owned(),
            ],
            text: session_draft
                .as_ref()
                .map(|draft| draft.text.clone())
                .unwrap_or_default(),
            empty: Self::empty_text("No notes for this session", NotePart::Session, active, focused),
            flag: too_large(&session_draft, NotePart::Session),
            editing: note_editing(&session_draft, NotePart::Session),
            active: !focused && active == NotePart::Session,
        };
        let parent_input = props.parent_note().map(|(parent, note)| {
            let label = format!("{}'s note", parent.title);
            FoldInput {
                open: self.parents_open.contains(&session.id),
                section: NotesSectionInput {
                    titles: vec![format!("▾ {label} · read-only"), format!("▾ {label}")],
                    text: note.text.clone(),
                    ..NotesSectionInput::default()
                },
                label,
            }
        });
        let layout = notes_layout(&NotesLayoutInput {
            steps: &step_input,
            brief: brief_input.as_ref(),
            line: &line_input,
            session: &session_input,
            parent: parent_input.as_ref(),
            width,
            height,
            focus: focused.then_some(active),
        });
        if let (true, Some(scroll_top)) = (focused, layout.scroll_top)
            && let Some(edited) = self.drafts.get_mut(&draft_key(&session.id, active))
        {
            edited.scroll_top = scroll_top;
        }
        // The editor's wrap width and body rows, for Up/Down and PageUp/PageDown.
        self.viewport_width = width.max(1);
        self.viewport_rows = layout.edited_rows.unwrap_or(1).max(1);
        render_notes_rows(&layout.rows, buf, area);
    }

    /// The notes-focus footer.
    fn hint(&self, props: &NotesFlowProps<'_>, width: usize) -> String {
        if self.step_editor.is_some() {
            return fit_hint(
                &[HintPart::from("enter save"), HintPart::from("esc cancel")],
                width,
                " • ",
            );
        }
        let active = self.active_part(props);
        let mut parts: Vec<HintPart> = vec![HintPart::from("esc done")];
        let has_brief = props.brief().is_some();
        if active == NotePart::Steps {
            let any = !props.steps().is_empty();
            if any {
                parts.push(hint_part("space tick", None, 1));
            }
            parts.push(hint_part("a add a step", Some("a add"), 1));
            if any {
                parts.push(hint_part("enter rename", None, 2));
            }
            parts.push(hint_part("tab notes", None, 1));
            if has_brief {
                parts.push(hint_part("b brief", None, 2));
            }
            if props.parent_note().is_some() {
                parts.push(hint_part("p parent's note", None, 2));
            }
            parts.push(hint_part("E open in editor", Some("E editor"), 3));
            return fit_hint(&parts, width, " • ");
        }
        let parts_list = self.parts(props);
        let next = parts_list
            .iter()
            .position(|part| *part == active)
            .and_then(|at| parts_list.get((at + 1) % parts_list.len()))
            .copied()
            .unwrap_or_default();
        let next_text = match next {
            NotePart::Steps => "tab steps",
            NotePart::Brief => "tab brief",
            NotePart::Line => "tab line's note",
            NotePart::Session => "tab session's note",
        };
        parts.push(hint_part(next_text, Some("tab switch"), 1));
        parts.push(hint_part("ctrl+x toggle ☐", Some("ctrl+x ☐"), 1));
        parts.push(hint_part("ctrl+t new item", None, 2));
        parts.push(hint_part("ctrl+o open in editor", Some("ctrl+o editor"), 2));
        if has_brief {
            parts.push(hint_part("ctrl+b brief", None, 3));
        }
        // On an open checklist item, Ctrl+P (make it a step) is offered.
        let on_open_item = active != NotePart::Brief
            && self
                .peek(props, active, |text, cursor| {
                    let line = text
                        .get(line_start(text, cursor)..line_end(text, cursor))
                        .unwrap_or_default();
                    parse_checklist_line(line).is_some_and(|item| !item.checked && !js_trim(&item.text).is_empty())
                })
                .unwrap_or(false);
        if on_open_item {
            parts.push(hint_part("ctrl+p → step", None, 2));
        }
        fit_hint(&parts, width, " • ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::event_loop::Update;
    use crate::app::state::{AppProps, Mode};
    use crate::app::test_support::{app, session};
    use deckhand_client::live::LiveClientHandlers;
    use deckhand_protocol::data::{NoteFile, SessionStatus};
    use serde_json::Value;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;
    use tokio::net::unix::OwnedWriteHalf;
    use tokio::sync::mpsc::UnboundedReceiver;

    fn run(test: impl std::future::Future<Output = ()>) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        tokio::task::LocalSet::new().block_on(&runtime, test);
    }

    /// Token storage's session (`s1`) is on its line with three steps (one ticked, one a sub-session's: `s2`'s), under
    /// an idea; the line has a brief and a note.
    const LIST: &str = "- [ ] Rework auth <!-- dh:t=idea s=orch -->\n\
        \x20 - [ ] Token storage <!-- dh:t=tok s=s1 -->\n\
        \x20   - [x] schema <!-- dh:t=sch -->\n\
        \x20   - [ ] encryption <!-- dh:t=enc -->\n\
        \x20   - [ ] refresh tokens <!-- dh:t=ref s=s2 -->\n";

    fn work_note(kind: WorkNoteKind, id: &str, text: &str, revision: &str) -> WorkNote {
        WorkNote {
            kind,
            id: id.to_owned(),
            path: format!("/notes/{id}.md"),
            text: text.to_owned(),
            revision: revision.to_owned(),
            too_large: None,
            from: None,
            at: None,
        }
    }

    fn brief() -> WorkNote {
        WorkNote {
            from: Some("◎ Rework auth".to_owned()),
            at: Some("2026-11-14T10:00:00.000Z".to_owned()),
            ..work_note(
                WorkNoteKind::Brief,
                "tok",
                "Store tokens encrypted.\n- [ ] keep refresh out\n",
                "b1",
            )
        }
    }

    fn line_notes() -> Vec<WorkNote> {
        vec![
            work_note(WorkNoteKind::Line, "tok", "chose AES-GCM\n- [ ] rotate keys", "n1"),
            brief(),
        ]
    }

    /// A session whose own note is `text` at `revision`.
    fn noted(id: &str, title: &str, text: &str, revision: &str) -> SessionRecord {
        let mut record = session(id, title, SessionStatus::Running);
        record.notes = Some(text.to_owned());
        record.notes_file = Some(NoteFile {
            path: format!("/notes/{id}.md"),
            revision: revision.to_owned(),
            too_large: None,
        });
        record
    }

    fn token(text: &str, revision: &str) -> SessionRecord {
        noted("s1", "Token storage", text, revision)
    }

    fn refresh() -> SessionRecord {
        let mut record = noted("s2", "Refresh tokens", "", "e");
        record.parent_session_id = Some("s1".to_owned());
        record
    }

    /// The daemon end of the client's socket: requests arrive on a channel.
    struct Daemon {
        lines: UnboundedReceiver<Value>,
        writer: OwnedWriteHalf,
    }

    impl Daemon {
        async fn respond(&mut self, request: &Value, body: Value) {
            let mut message = json!({"type": "response", "requestId": request["requestId"]});
            if let (Some(message), Some(body)) = (message.as_object_mut(), body.as_object()) {
                message.extend(body.clone());
            }
            self.writer
                .write_all(format!("{message}\n").as_bytes())
                .await
                .expect("write");
        }
    }

    struct Harness {
        app: App,
        rx: UnboundedReceiver<Update<App>>,
        daemon: Daemon,
    }

    type Files = Rc<RefCell<Vec<Option<String>>>>;

    const KEY: &str = "session:s1";
    const LINE_KEY: &str = "line:s1";

    fn key(update: impl FnOnce(&mut Key)) -> Key {
        let mut key = Key::NONE;
        update(&mut key);
        key
    }

    fn list_doc(text: &str) -> TasksDoc {
        TasksDoc {
            key: "k".to_owned(),
            path: "/notes/tasks/k.md".to_owned(),
            text: text.to_owned(),
            revision: "t0".to_owned(),
            too_large: None,
            notes: None,
        }
    }

    impl Harness {
        /// An app in notes focus on `selected` of `sessions`, the list `list` with `notes`, connected to a fake daemon.
        fn new(sessions: Vec<SessionRecord>, selected: &str, list: &str, notes: Vec<WorkNote>) -> Harness {
            let (mut app, rx) = app(AppProps::default());
            let (client, server) = UnixStream::pair().expect("pair");
            let (read, writer) = server.into_split();
            let (lines_tx, lines) = tokio::sync::mpsc::unbounded_channel();
            tokio::task::spawn_local(async move {
                let mut reader = BufReader::new(read);
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line).await {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {
                            if let Ok(value) = serde_json::from_str(&line) {
                                let _ = lines_tx.send(value);
                            }
                        }
                    }
                }
            });
            app.connection.client = Some(Rc::new(LiveClient::new(client, LiveClientHandlers::default())));
            app.data.repo_sessions = sessions;
            app.set_tasks_doc(Some(TasksDoc {
                notes: Some(notes),
                ..list_doc(list)
            }));
            app.browse.selected_id = Some(selected.to_owned());
            app.mode = Mode::NotesFocus;
            Harness {
                app,
                rx,
                daemon: Daemon { lines, writer },
            }
        }

        /// Token storage's tab: its line, steps, brief and note, and its own note.
        fn token(text: &str) -> Harness {
            Harness::new(vec![token(text, "r1"), refresh()], "s1", LIST, line_notes())
        }

        /// A session on no line.
        fn lineless(text: &str) -> Harness {
            Harness::new(vec![token(text, "r1")], "s1", "", Vec::new())
        }

        fn flow(&self) -> &NotesFlow {
            &self.app.flows.notes
        }

        fn draft(&self, key: &str) -> &Draft {
            self.flow().drafts.get(key).expect("draft")
        }

        fn error(&self) -> Option<&str> {
            self.app.messages.error.as_deref()
        }

        fn status(&self) -> Option<&str> {
            self.app.messages.status_message.as_deref()
        }

        /// Applies what the loop would: replies, timers.
        async fn pump(&mut self) {
            for _ in 0..30 {
                tokio::task::yield_now().await;
                while let Ok(update) = self.rx.try_recv() {
                    update(&mut self.app);
                }
            }
        }

        /// Runs the loop until the daemon gets a request.
        async fn request(&mut self) -> Value {
            loop {
                tokio::select! {
                    Some(update) = self.rx.recv() => update(&mut self.app),
                    Some(line) = self.daemon.lines.recv() => return line,
                }
            }
        }

        /// Runs the loop until `wait` passes.
        async fn run_for(&mut self, wait: Duration) {
            let end = tokio::time::Instant::now() + wait;
            loop {
                tokio::select! {
                    Some(update) = self.rx.recv() => update(&mut self.app),
                    () = tokio::time::sleep_until(end) => return,
                }
            }
        }

        fn input(&mut self, input: Input) {
            self.app
                .notes_flow_call(|flow, props, cx| flow.handle_input(props, &input, cx));
        }

        fn type_text(&mut self, text: &str) {
            for character in text.chars() {
                self.input(Input::text(character.to_string()));
            }
        }

        fn press(&mut self, key: Key) {
            self.input(Input::key("", key));
        }

        fn tab(&mut self) {
            self.press(key(|key| key.tab = true));
        }

        fn ctrl(&mut self, letter: &str) {
            self.input(Input::key(
                letter,
                Key {
                    ctrl: true,
                    ..Key::NONE
                },
            ));
        }

        /// The daemon's answer: `data`.
        async fn ok(&mut self, request: &Value, data: Value) {
            self.daemon.respond(request, json!({"ok": true, "data": data})).await;
        }

        /// The daemon's answer to a session note's save.
        async fn saved(&mut self, request: &Value, record: &SessionRecord, saved: bool) {
            let note = json!({"kind": "session", "id": record.id, "path": "/notes/s.md",
                "text": record.notes.clone().unwrap_or_default(),
                "revision": record.notes_file.as_ref().map(|file| file.revision.clone()).unwrap_or_default()});
            self.ok(request, json!({"saved": saved, "note": note, "session": record}))
                .await;
        }

        fn render(&mut self, width: u16, height: u16) -> Vec<String> {
            let area = Rect::new(0, 0, width, height);
            let mut buf = Buffer::empty(area);
            self.app
                .notes_flow_call(|flow, props, _cx| flow.render(props, &mut buf, area));
            crate::flows::tasks::tests::buffer_rows(&buf)
        }

        fn hint(&mut self, width: usize) -> String {
            self.app.notes_flow_call(|flow, props, _cx| flow.hint(props, width))
        }

        fn part(&mut self) -> NotePart {
            self.app.notes_flow_call(|flow, props, _cx| flow.active_part(props))
        }

        /// `activeFile` with a sink for what it resolves to.
        fn active_file(&mut self) -> Files {
            let got: Files = Rc::default();
            let sink = Rc::clone(&got);
            self.app.notes_flow_call(|flow, props, cx| {
                flow.active_file(props, cx, Box::new(move |_app, file| sink.borrow_mut().push(file)));
            });
            got
        }
    }

    // ── What the tab shows ────────────────────────────────────────────────────────────────────────────────────

    #[test]
    fn a_session_on_a_line_shows_its_steps_the_folded_brief_and_both_notes() {
        run(async {
            let mut h = Harness::token("Review crypto.rs before merging");
            h.app.mode = Mode::Browse;
            let rule = "─".repeat(70);
            assert_eq!(
                h.render(70, 14),
                [
                    "◆ Token storage · 1/3 steps",
                    "  ☑ schema",
                    "  ☐ encryption",
                    "  ☐ refresh tokens",
                    "▸ Brief · from ◎ Rework auth · 14 Nov",
                    rule.as_str(),
                    "Token storage · shared with whoever works on it · ☐ 1 open",
                    "chose AES-GCM",
                    "☐ rotate keys",
                    rule.as_str(),
                    "This session · only this conversation",
                    "Review crypto.rs before merging",
                    "",
                    "",
                ]
            );
        });
    }

    #[test]
    fn a_session_on_no_line_says_so_and_how_to_write_each_note() {
        run(async {
            let mut h = Harness::lineless("");
            h.app.mode = Mode::Browse;
            let rows = h.render(70, 8);
            assert_eq!(rows[0], "No line yet");
            assert_eq!(rows[2], "Its line's note · shared with whoever works on it");
            assert_eq!(rows[3], "No line yet: writing here makes one");
            assert_eq!(rows[5], "This session · only this conversation");
            // Enter edits the session's note first.
            assert_eq!(rows[6], "No notes for this session · enter to write");
            h.app.mode = Mode::NotesFocus;
            let rows = h.render(70, 8);
            assert_eq!(rows[3], "No line yet: writing here makes one · tab to write");
            assert_eq!(rows[6], "");
        });
    }

    #[test]
    fn a_sub_session_shows_its_parents_line_note_folded_and_p_opens_it() {
        run(async {
            let mut h = Harness::new(vec![token("", "r1"), refresh()], "s2", LIST, line_notes());
            let rows = h.render(70, 12);
            assert_eq!(rows[0], "◆ refresh tokens");
            assert_eq!(rows[7..9], ["", "▸ Token storage's note"]);
            // On the steps, p opens it (read-only here) and folds it again.
            h.tab();
            assert_eq!(h.part(), NotePart::Steps);
            h.type_text("p");
            let rows = h.render(70, 12);
            assert_eq!(
                rows[8..],
                [
                    "▾ Token storage's note · read-only · ☐ 1 open",
                    "chose AES-GCM",
                    "☐ rotate keys",
                    ""
                ]
            );
            h.type_text("p");
            assert_eq!(h.render(70, 12)[8], "▸ Token storage's note");
            // A session that is not a sub-session has none.
            let mut h = Harness::token("");
            h.tab();
            h.type_text("p");
            assert_eq!(h.status(), Some("Not a sub-session: there is no parent's note"));
        });
    }

    // ── Switching notes ───────────────────────────────────────────────────────────────────────────────────────

    #[test]
    fn tab_moves_through_the_steps_the_open_brief_and_both_notes() {
        run(async {
            let mut h = Harness::token("");
            assert_eq!(h.part(), NotePart::Session);
            h.tab();
            assert_eq!(h.part(), NotePart::Steps);
            h.tab();
            assert_eq!(h.part(), NotePart::Line);
            h.tab();
            assert_eq!(h.part(), NotePart::Session);
            h.press(key(|key| {
                key.tab = true;
                key.shift = true;
            }));
            assert_eq!(h.part(), NotePart::Line);
            // Typing goes to the note being edited.
            h.type_text("!");
            assert_eq!(h.draft(LINE_KEY).text, "chose AES-GCM\n- [ ] rotate keys!");
            assert_eq!(h.draft(KEY).text, "");
            // An open brief is a stop too; ctrl+b folds it again (and leaves it for the line's note).
            h.ctrl("b");
            h.press(key(|key| {
                key.tab = true;
                key.shift = true;
            }));
            assert_eq!(h.part(), NotePart::Brief);
            h.ctrl("b");
            assert_eq!(h.part(), NotePart::Line);
            // A line without a brief says so.
            let mut h = Harness::lineless("");
            h.ctrl("b");
            assert_eq!(h.status(), Some("This session's line has no brief"));
        });
    }

    #[test]
    fn an_open_brief_is_edited_and_saved_as_the_lines_brief() {
        run(async {
            let mut h = Harness::token("");
            h.render(70, 20);
            h.ctrl("b");
            let rows = h.render(70, 20);
            assert_eq!(rows[4], "▾ Brief · from ◎ Rework auth · 14 Nov · ☐ 1 open");
            assert_eq!(rows[5], "Store tokens encrypted.");
            // Shift+Tab from the session's note: the line's note, then the brief.
            h.press(key(|key| {
                key.tab = true;
                key.shift = true;
            }));
            h.press(key(|key| {
                key.tab = true;
                key.shift = true;
            }));
            assert_eq!(h.part(), NotePart::Brief);
            h.type_text("x");
            let save = h.request().await;
            assert_eq!(save["type"], "save-note");
            assert_eq!(save["note"], json!({"kind": "brief", "id": "tok"}));
            assert_eq!(save["revision"], "b1");
            assert_eq!(save["text"], "Store tokens encrypted.\n- [ ] keep refresh out\nx");
            // ctrl+p leaves a brief's checkboxes in it.
            h.ctrl("p");
            assert!(
                h.status()
                    .is_some_and(|status| status.starts_with("A brief's checkboxes stay in it"))
            );
        });
    }

    // ── Steps ─────────────────────────────────────────────────────────────────────────────────────────────────

    #[test]
    fn space_ticks_the_selected_step_and_enter_renames_it() {
        run(async {
            let mut h = Harness::token("");
            h.tab();
            h.type_text("j");
            let rows = h.render(70, 14);
            assert_eq!(rows[2], "› ☐ encryption");
            h.type_text(" ");
            let op = h.request().await;
            assert_eq!(op["type"], "task-op");
            assert_eq!(op["op"], json!({"type": "toggle", "id": "enc"}));
            let ticked = LIST.replace("[ ] encryption", "[x] encryption");
            h.ok(&op, json!(list_doc(&ticked))).await;
            h.pump().await;
            assert_eq!(h.status(), Some("Done: encryption"));
            assert_eq!(h.render(70, 14)[0], "◆ Token storage · 2/3 steps");
            // Enter: its title in place; Enter saves it.
            h.press(key(|key| key.return_ = true));
            assert!(h.hint(100).starts_with("enter save"));
            h.press(key(|key| key.backspace = true));
            h.type_text("ing");
            assert_eq!(h.render(70, 14)[2], "› ☐ encryptioing");
            h.press(key(|key| key.return_ = true));
            let op = h.request().await;
            assert_eq!(
                op["op"],
                json!({"type": "retitle", "id": "enc", "title": "encryptioing"})
            );
            h.ok(&op, json!(list_doc(&ticked))).await;
            h.pump().await;
            assert_eq!(h.status(), Some("Renamed the step"));
            // Esc cancels a title, and leaves the steps only after.
            h.press(key(|key| key.return_ = true));
            h.press(key(|key| key.escape = true));
            assert_eq!(h.flow().step_editor, None);
            assert_eq!(h.app.mode, Mode::NotesFocus);
        });
    }

    #[test]
    fn a_adds_a_step_under_the_sessions_line() {
        run(async {
            let mut h = Harness::token("");
            h.tab();
            h.type_text("a");
            h.type_text("rotate keys");
            assert_eq!(h.render(70, 20)[4], "› ☐ rotate keys");
            h.press(key(|key| key.return_ = true));
            let op = h.request().await;
            assert_eq!(
                op["op"],
                json!({"type": "add", "title": "rotate keys", "parent": "tok"})
            );
            let added = format!("{LIST}    - [ ] rotate keys <!-- dh:t=rot -->\n");
            h.ok(&op, json!(list_doc(&added))).await;
            h.pump().await;
            assert_eq!(h.status(), Some("Added a step: rotate keys"));
            assert_eq!(h.flow().step, 3);
            assert_eq!(h.render(70, 20)[0], "◆ Token storage · 1/4 steps");
        });
    }

    #[test]
    fn a_step_for_a_session_on_no_line_makes_its_line_first() {
        run(async {
            let mut h = Harness::lineless("");
            h.tab();
            assert_eq!(h.render(70, 8)[0], "No line yet · a adds a step (it gets its line)");
            h.type_text("aReproduce it");
            h.press(key(|key| key.return_ = true));
            let made = h.request().await;
            assert_eq!(made["op"], json!({"type": "session-line", "s": "s1"}));
            let line = "- [ ] Token storage <!-- dh:t=new s=s1 -->\n";
            h.ok(&made, json!(list_doc(line))).await;
            let add = h.request().await;
            assert_eq!(
                add["op"],
                json!({"type": "add", "title": "Reproduce it", "parent": "new"})
            );
            h.ok(
                &add,
                json!(list_doc(&format!("{line}  - [ ] Reproduce it <!-- dh:t=rep -->\n"))),
            )
            .await;
            h.pump().await;
            assert_eq!(
                h.render(70, 8)[..2],
                ["◆ Token storage · 0/1 steps", "› ☐ Reproduce it"]
            );
        });
    }

    #[test]
    fn writing_the_line_note_of_a_session_on_no_line_makes_its_line() {
        run(async {
            let mut h = Harness::lineless("");
            h.render(70, 10);
            h.press(key(|key| {
                key.tab = true;
                key.shift = true;
            }));
            assert_eq!(h.part(), NotePart::Line);
            h.type_text("why");
            let made = h.request().await;
            assert_eq!(made["op"], json!({"type": "session-line", "s": "s1"}));
            h.ok(&made, json!(list_doc("- [ ] Token storage <!-- dh:t=new s=s1 -->\n")))
                .await;
            let save = h.request().await;
            assert_eq!(save["note"], json!({"kind": "line", "id": "new"}));
            assert_eq!(save["text"], "why");
            assert_eq!(save["revision"], note_revision(""));
            h.ok(
                &save,
                json!({"saved": true, "note": work_note(WorkNoteKind::Line, "new", "why", "n2")}),
            )
            .await;
            h.pump().await;
            let draft = h.draft(LINE_KEY);
            assert_eq!(
                (draft.line_id.as_deref(), draft.saved.as_str(), draft.revision.as_str()),
                (Some("new"), "why", "n2")
            );
            let rows = h.render(70, 10);
            assert_eq!(rows[0], "◆ Token storage");
            assert_eq!(rows[3], "Token storage · shared with whoever works on it");
            assert_eq!(h.error(), None);
        });
    }

    // ── Saving ────────────────────────────────────────────────────────────────────────────────────────────────

    #[test]
    fn typing_edits_the_draft_and_saves_300ms_after_the_last_key() {
        run(async {
            let mut h = Harness::token("hi");
            h.render(70, 14);
            h.type_text("a");
            assert_eq!(h.draft(KEY).text, "hia");
            h.run_for(Duration::from_millis(200)).await;
            let second_key = tokio::time::Instant::now();
            h.type_text("b");
            let request = h.request().await;
            assert!(
                second_key.elapsed() >= Duration::from_millis(280),
                "{:?}",
                second_key.elapsed()
            );
            assert_eq!(request["type"], "save-note");
            assert_eq!(request["note"], json!({"kind": "session", "id": "s1"}));
            assert_eq!(request["text"], "hiab");
            assert_eq!(request["revision"], "r1");
            assert_eq!(h.draft(KEY).in_flight.as_deref(), Some("hiab"));
            h.saved(&request, &token("hiab", "r2"), true).await;
            h.pump().await;
            let saved = h.draft(KEY);
            assert_eq!(
                (saved.saved.as_str(), saved.revision.as_str(), saved.in_flight.clone()),
                ("hiab", "r2", None)
            );
            // The answer's session is the app's now.
            assert_eq!(h.app.data.repo_sessions[0].notes.as_deref(), Some("hiab"));
            assert_eq!(h.error(), None);
        });
    }

    #[test]
    fn the_line_note_is_saved_against_its_revision_and_held_by_the_app() {
        run(async {
            let mut h = Harness::token("");
            h.render(70, 14);
            h.press(key(|key| {
                key.tab = true;
                key.shift = true;
            }));
            h.type_text("!");
            let save = h.request().await;
            assert_eq!(save["note"], json!({"kind": "line", "id": "tok"}));
            assert_eq!(save["revision"], "n1");
            let note = work_note(WorkNoteKind::Line, "tok", "chose AES-GCM\n- [ ] rotate keys!", "n2");
            h.ok(&save, json!({"saved": true, "note": note})).await;
            h.pump().await;
            assert_eq!(
                h.app
                    .data
                    .work_notes
                    .line_note("tok")
                    .map(|note| note.revision.as_str()),
                Some("n2")
            );
            assert_eq!(h.draft(LINE_KEY).revision, "n2");
        });
    }

    #[test]
    fn leaving_focus_saves_every_draft_at_once() {
        run(async {
            let mut h = Harness::token("");
            h.render(70, 14);
            h.app.notes_flow_call(|flow, props, cx| flow.sync(props, cx));
            h.type_text("x");
            h.app.mode = Mode::Browse;
            h.app.notes_flow_call(|flow, props, cx| flow.sync(props, cx));
            let start = tokio::time::Instant::now();
            let request = h.request().await;
            assert!(start.elapsed() < Duration::from_millis(250));
            assert_eq!(request["text"], "x");
            // The debounce timer was cancelled: no second save.
            h.saved(&request, &token("x", "r2"), true).await;
            h.run_for(Duration::from_millis(400)).await;
            assert!(h.daemon.lines.try_recv().is_err());
        });
    }

    #[test]
    fn text_typed_while_saving_is_saved_after_it() {
        run(async {
            let mut h = Harness::token("");
            h.render(70, 14);
            h.type_text("a");
            let first = h.request().await;
            assert_eq!(first["text"], "a");
            h.type_text("b");
            // The timer fires while the first save is in flight: it waits for it.
            h.run_for(Duration::from_millis(400)).await;
            assert!(h.daemon.lines.try_recv().is_err());
            h.saved(&first, &token("a", "r2"), true).await;
            let second = h.request().await;
            assert_eq!(
                (second["text"].as_str(), second["revision"].as_str()),
                (Some("ab"), Some("r2"))
            );
        });
    }

    #[test]
    fn a_save_refused_for_a_newer_note_keeps_the_typed_text_and_says_so() {
        run(async {
            let mut h = Harness::token("");
            h.render(70, 14);
            h.type_text("mine");
            let request = h.request().await;
            h.saved(&request, &token("theirs", "r9"), false).await;
            h.pump().await;
            let draft = h.draft(KEY);
            assert_eq!(
                (
                    draft.text.as_str(),
                    draft.saved.as_str(),
                    draft.revision.as_str(),
                    draft.conflict
                ),
                ("mine", "theirs", "r9", true)
            );
            assert_eq!(h.error(), Some(CONFLICT_MESSAGE));
            // Nothing is saved on its own after that; the next edit saves it over theirs, naming their revision.
            h.run_for(Duration::from_millis(400)).await;
            assert!(h.daemon.lines.try_recv().is_err());
            h.type_text("!");
            let again = h.request().await;
            assert_eq!(
                (again["text"].as_str(), again["revision"].as_str()),
                (Some("mine!"), Some("r9"))
            );
        });
    }

    #[test]
    fn a_failed_save_is_reported_and_the_text_kept() {
        run(async {
            let mut h = Harness::token("");
            h.render(70, 14);
            h.type_text("mine");
            let request = h.request().await;
            h.daemon
                .respond(&request, json!({"ok": false, "error": "disk full"}))
                .await;
            h.pump().await;
            assert_eq!(h.error(), Some("Notes not saved: disk full"));
            let draft = h.draft(KEY);
            assert_eq!(
                (draft.text.as_str(), draft.saved.as_str(), draft.in_flight.clone()),
                ("mine", "", None)
            );
        });
    }

    #[test]
    fn a_note_changed_elsewhere_reloads_unless_text_is_unsaved() {
        run(async {
            let mut h = Harness::token("old");
            h.render(70, 14);
            h.app.data.repo_sessions[0] = token("new from the editor", "r2");
            h.render(70, 14);
            assert_eq!(h.draft(KEY).text, "new from the editor");
            assert_eq!(h.error(), None);
            // Unsaved text is kept, reported right after the render.
            h.type_text("!");
            h.app.data.repo_sessions[0] = token("again", "r3");
            h.render(70, 14);
            assert_eq!(h.draft(KEY).text, "new! from the editor");
            assert_eq!(h.draft(KEY).revision, "r3");
            h.pump().await;
            assert_eq!(h.error(), Some(CONFLICT_MESSAGE));
            // A line note pushed while nothing is unsaved: taken as it is.
            h.app
                .data
                .work_notes
                .put(work_note(WorkNoteKind::Line, "tok", "pushed", "n5"));
            h.render(70, 14);
            assert_eq!(h.draft(LINE_KEY).text, "pushed");
        });
    }

    // ── ctrl+p ────────────────────────────────────────────────────────────────────────────────────────────────

    #[test]
    fn ctrl_p_in_the_session_note_saves_then_makes_the_checkbox_a_step() {
        run(async {
            let mut h = Harness::token("- [ ] one\n- [ ] two");
            h.render(70, 14);
            h.press(key(|key| key.up_arrow = true));
            h.press(key(|key| key.end = true));
            h.type_text("!");
            let text = h.draft(KEY).text.clone();
            assert_eq!(text, "- [ ] one!\n- [ ] two");
            assert!(h.hint(200).ends_with(" • ctrl+p → step"), "{}", h.hint(200));
            h.ctrl("p");
            // Saved first, so the daemon finds the line as shown.
            let save = h.request().await;
            assert_eq!(save["type"], "save-note");
            h.saved(&save, &token(&text, "r2"), true).await;
            let promote = h.request().await;
            assert_eq!(promote["type"], "promote-note-item");
            assert_eq!(promote["note"], json!({"kind": "session", "id": "s1"}));
            assert_eq!(
                (promote["line"].as_i64(), promote["revision"].as_str()),
                (Some(0), Some("r2"))
            );
            let promoted = format!("{LIST}    - [ ] one! <!-- dh:t=one -->\n");
            h.ok(
                &promote,
                json!({
                    "tasks": list_doc(&promoted),
                    "lineId": "one",
                    "note": {"kind": "session", "id": "s1", "path": "/notes/s1.md", "text": "- [ ] two", "revision": "r3"},
                    "session": token("- [ ] two", "r3"),
                }),
            )
            .await;
            h.pump().await;
            assert_eq!(h.draft(KEY).text, "- [ ] two");
            assert_eq!(h.status(), Some("Now a step of this session's line: one!"));
            assert_eq!(h.render(70, 14)[0], "◆ Token storage · 1/4 steps");
            assert_eq!(h.error(), None);
        });
    }

    #[test]
    fn ctrl_p_in_the_line_note_promotes_from_the_line_note() {
        run(async {
            let mut h = Harness::token("");
            h.render(70, 14);
            h.press(key(|key| {
                key.tab = true;
                key.shift = true;
            }));
            h.ctrl("p");
            let promote = h.request().await;
            assert_eq!(promote["note"], json!({"kind": "line", "id": "tok"}));
            assert_eq!(
                (promote["line"].as_i64(), promote["revision"].as_str()),
                (Some(1), Some("n1"))
            );
            h.ok(
                &promote,
                json!({
                    "tasks": list_doc(LIST),
                    "lineId": "rk",
                    "note": work_note(WorkNoteKind::Line, "tok", "chose AES-GCM", "n2"),
                }),
            )
            .await;
            h.pump().await;
            assert_eq!(h.draft(LINE_KEY).text, "chose AES-GCM");
            assert_eq!(
                h.app.data.work_notes.line_note("tok").map(|note| note.text.as_str()),
                Some("chose AES-GCM")
            );
            assert_eq!(h.status(), Some("Now a step of this session's line: rotate keys"));
        });
    }

    #[test]
    fn ctrl_p_off_an_open_item_and_a_refusal_say_so() {
        run(async {
            let mut h = Harness::token("plain");
            h.ctrl("p");
            assert_eq!(
                h.status(),
                Some("Put the cursor on an open checklist item (- [ ] …) to make it a step")
            );
            let mut h = Harness::token("- [ ] one");
            h.ctrl("p");
            let promote = h.request().await;
            h.daemon
                .respond(
                    &promote,
                    json!({"ok": false, "error": "These notes changed meanwhile; try again"}),
                )
                .await;
            h.pump().await;
            assert_eq!(h.error(), Some("These notes changed meanwhile; try again"));
        });
    }

    // ── The editor ────────────────────────────────────────────────────────────────────────────────────────────

    #[test]
    fn escape_leaves_notes_focus_and_a_missing_session_too() {
        run(async {
            let mut h = Harness::token("");
            h.press(key(|key| key.escape = true));
            assert_eq!(h.app.mode, Mode::Browse);
            let mut h = Harness::new(vec![], "none", "", Vec::new());
            assert_eq!(h.render(40, 2), ["Select a session from the sidebar.", ""]);
            h.type_text("x");
            h.pump().await;
            assert_eq!(h.app.mode, Mode::Browse);
        });
    }

    #[test]
    fn the_edited_note_scrolls_and_reports_its_viewport() {
        run(async {
            let text = (1..=30)
                .map(|index| format!("line {index}"))
                .collect::<Vec<_>>()
                .join("\n");
            let mut h = Harness::token(&text);
            let rows = h.render(30, 20);
            assert_eq!(rows[19], "line 30");
            assert!(h.flow().viewport_rows > 1);
            assert!(h.draft(KEY).scroll_top > 0);
            assert_eq!(h.flow().viewport_width, 30);
        });
    }

    #[test]
    fn bracketed_paste_inserts_enter_and_tab_as_text() {
        run(async {
            let mut h = Harness::token("");
            h.input(Input::text("[200~"));
            assert!(h.flow().pasting);
            h.input(Input::text("one"));
            h.press(key(|key| key.return_ = true));
            h.tab();
            h.input(Input::text("two\u{1b}[31m"));
            h.press(key(|key| key.escape = true));
            assert_eq!(h.app.mode, Mode::NotesFocus);
            h.input(Input::text("[201~"));
            assert!(!h.flow().pasting);
            assert_eq!(h.draft(KEY).text, "one\n\ttwo");
        });
    }

    #[test]
    fn checklist_keys_toggle_add_and_continue_items() {
        run(async {
            let mut h = Harness::token("- [ ] one");
            h.ctrl("x");
            assert_eq!(h.draft(KEY).text, "- [x] one");
            h.ctrl("x");
            h.press(key(|key| key.return_ = true));
            assert_eq!(h.draft(KEY).text, "- [ ] one\n- [ ] ");
            h.type_text("two");
            h.ctrl("t");
            assert_eq!(h.draft(KEY).text, "- [ ] one\n- [ ] two\n- [ ] ");
            h.press(key(|key| key.return_ = true));
            assert_eq!(h.draft(KEY).text, "- [ ] one\n- [ ] two\n");
        });
    }

    #[test]
    fn an_edit_over_the_limit_or_of_a_too_large_note_is_refused() {
        run(async {
            let mut h = Harness::token(&"a".repeat(MAX_NOTES_CHARS));
            h.type_text("b");
            assert_eq!(
                h.error(),
                Some(format!("Notes are limited to {MAX_NOTES_LABEL}; that edit was not applied.").as_str())
            );
            assert_eq!(js_len(&h.draft(KEY).text), MAX_NOTES_CHARS);
            let mut big = token("abc", "r1");
            if let Some(file) = big.notes_file.as_mut() {
                file.too_large = Some(true);
            }
            let mut h = Harness::new(vec![big], "s1", LIST, line_notes());
            h.type_text("x");
            assert_eq!(
                h.error(),
                Some(format!("This note is longer than {MAX_NOTES_LABEL}: edit it in your editor (Ctrl+O)").as_str())
            );
            h.ctrl("p");
            assert_eq!(h.error(), Some("This note is too large to change here"));
            assert_eq!(h.draft(KEY).text, "abc");
        });
    }

    // ── Hints and files ───────────────────────────────────────────────────────────────────────────────────────

    #[test]
    fn the_hint_follows_the_part() {
        run(async {
            let mut h = Harness::token("plain");
            assert_eq!(
                h.hint(200),
                "esc done • tab steps • ctrl+x toggle ☐ • ctrl+t new item • ctrl+o open in editor • ctrl+b brief"
            );
            h.tab();
            assert_eq!(
                h.hint(200),
                "esc done • space tick • a add a step • enter rename • tab notes • b brief • E open in editor"
            );
            h.tab();
            assert!(h.hint(200).contains("tab session's note"), "{}", h.hint(200));
            let mut h = Harness::new(vec![token("", "r1"), refresh()], "s2", LIST, line_notes());
            h.tab();
            assert_eq!(
                h.hint(200),
                "esc done • a add a step • tab notes • p parent's note • E open in editor"
            );
        });
    }

    #[test]
    fn the_active_file_is_saved_first_and_handed_over() {
        run(async {
            let mut h = Harness::token("");
            h.render(70, 14);
            h.type_text("x");
            let got = h.active_file();
            let save = h.request().await;
            assert_eq!(save["type"], "save-note");
            h.saved(&save, &token("x", "r2"), true).await;
            let open = h.request().await;
            assert_eq!(open["type"], "open-note");
            assert_eq!(open["note"], json!({"kind": "session", "id": "s1"}));
            h.ok(&open, json!("/notes/s1.md")).await;
            h.pump().await;
            assert_eq!(*got.borrow(), [Some("/notes/s1.md".to_owned())]);
        });
    }

    #[test]
    fn the_steps_and_a_folded_brief_open_the_lines_note_and_a_lineless_one_makes_the_line() {
        run(async {
            let mut h = Harness::token("");
            h.tab();
            let got = h.active_file();
            let open = h.request().await;
            assert_eq!(open["note"], json!({"kind": "line", "id": "tok"}));
            h.daemon
                .respond(&open, json!({"ok": false, "error": "no such note"}))
                .await;
            h.pump().await;
            assert_eq!(*got.borrow(), [None]);
            assert_eq!(h.error(), Some("no such note"));
            let mut h = Harness::lineless("");
            h.tab();
            let got = h.active_file();
            let made = h.request().await;
            assert_eq!(made["op"], json!({"type": "session-line", "s": "s1"}));
            h.ok(&made, json!(list_doc("- [ ] Token storage <!-- dh:t=new s=s1 -->\n")))
                .await;
            let open = h.request().await;
            assert_eq!(open["note"], json!({"kind": "line", "id": "new"}));
            h.ok(&open, json!("/notes/lines/new.md")).await;
            h.pump().await;
            assert_eq!(*got.borrow(), [Some("/notes/lines/new.md".to_owned())]);
            assert!(h.app.data.task_list.line_of_session("s1").is_some());
        });
    }

    #[test]
    fn the_active_file_is_none_without_a_client() {
        run(async {
            let mut h = Harness::token("");
            h.app.connection.client = None;
            let got = h.active_file();
            h.pump().await;
            assert_eq!(*got.borrow(), [None]);
        });
    }

    #[test]
    fn the_keymap_is_in_the_notes_topic() {
        assert!(
            crate::keymap::undocumented_in_topic(KEYMAP, "Notes").is_empty(),
            "{:?}",
            crate::keymap::undocumented_in_topic(KEYMAP, "Notes")
        );
        assert!(crate::keymap::unknown_topics(KEYMAP).is_empty());
    }

    #[test]
    fn reconciling_keeps_unsaved_text_and_follows_the_line() {
        let doc = |text: &str, revision: &str, line: Option<&str>| NoteDoc {
            text: text.to_owned(),
            revision: revision.to_owned(),
            too_large: false,
            line_id: line.map(str::to_owned),
        };
        let draft = |text: &str, saved: &str, revision: &str, line: Option<&str>| Draft {
            text: text.to_owned(),
            cursor: text.len(),
            saved: saved.to_owned(),
            revision: revision.to_owned(),
            line_id: line.map(str::to_owned),
            ..Draft::of("s1", NotePart::Line, &doc("", "", None))
        };
        // The same revision: nothing to do.
        let mut same = draft("mine", "mine", "r1", Some("l"));
        assert_eq!(apply_reconcile(&mut same, &doc("other", "r1", Some("l"))), None);
        assert_eq!(same.text, "mine");
        // Nothing unsaved: the new text, the cursor kept inside it.
        let mut clean = draft("long text", "long text", "r1", Some("l"));
        assert_eq!(apply_reconcile(&mut clean, &doc("ab", "r2", Some("l"))), None);
        assert_eq!((clean.text.as_str(), clean.cursor), ("ab", 2));
        // The echo of the save in flight only moves the revision.
        let mut flying = draft("mine!!", "mine", "r1", Some("l"));
        flying.in_flight = Some("mine!".to_owned());
        assert_eq!(apply_reconcile(&mut flying, &doc("mine!", "r2", Some("l"))), None);
        assert_eq!((flying.text.as_str(), flying.saved.as_str()), ("mine!!", "mine!"));
        // The line made by the first save: the draft follows it, nothing lost.
        let mut first = draft("why", "", &note_revision(""), None);
        assert_eq!(
            apply_reconcile(&mut first, &doc("", &note_revision(""), Some("new"))),
            None
        );
        assert_eq!((first.text.as_str(), first.line_id.as_deref()), ("why", Some("new")));
        // Another line's note: the unsaved text belonged to the old one.
        let mut moved = draft("typed", "", "r1", Some("old"));
        assert_eq!(
            apply_reconcile(&mut moved, &doc("theirs", "r7", Some("other"))),
            Some(LINE_CHANGED_MESSAGE)
        );
        assert_eq!(moved.text, "theirs");
        // The cursor never lands inside a character.
        let mut multibyte = draft("aé", "aé", "r1", Some("l"));
        multibyte.cursor = 3;
        apply_reconcile(&mut multibyte, &doc("é", "r2", Some("l")));
        assert_eq!(multibyte.cursor, 2);
    }
}
