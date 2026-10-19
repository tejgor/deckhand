//! Port of notesFlow.tsx: the Notes tab's state. One draft per note (a shared note is one draft for every session
//! showing it, so its cursor and unsaved text follow it), debounced revision-checked saves, reloads when the file
//! changed elsewhere, and the keys of notes focus (Enter). The daemon owns the files; records carry their text and
//! revision.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_client::live::LiveClient;
use deckhand_core::display::{HintPart, display_session_title, error_message, fit_hint};
use deckhand_core::error::DhError;
use deckhand_core::jsstr::{js_len, js_trim, path};
use deckhand_core::key::{Input, Key};
use deckhand_core::notes::{
    MAX_NOTES_CHARS, MAX_NOTES_LABEL, TaskLinkLookup, continue_checklist, insert_checklist_item, note_key,
    parse_checklist_line, shows_own_note, toggle_checklist,
};
use deckhand_core::text_editor::{
    EditOptions, EditorState, clean_inserted_text, edit_text, line_end, line_start, replace_range,
};
use deckhand_protocol::data::{NoteSaveResult, NoteSection, PromoteNoteItemResult, SessionRecord, SharedNoteKind};
use deckhand_sys::async_util::Timer;
use deckhand_sys::desktop::open_in_editor;

use crate::app::state::App;
use crate::flow::{Flow, FlowCx, FlowEffect, FlowHost, FlowThen};
use crate::keymap::Keymap;
use crate::panes::notes::{
    NotesEditing, NotesLayoutInput, NotesSectionInput, notes_layout, render_notes_message, render_notes_rows,
};

/// What [`NotesFlow::active_file`] hands the file to (TS: the promise's `.then`).
pub type FileThen = Box<dyn FnOnce(&mut App, Option<String>)>;

/// What [`NotesFlow::file_of`] hands its result to: none when there is no client or session to ask (TS: `fileOf`
/// returns `undefined`).
type FileDone = Box<dyn FnOnce(&mut App, Option<Result<String, DhError>>)>;

/// TS: notesFlow.tsx SAVE_DEBOUNCE_MS.
pub const SAVE_DEBOUNCE_MS: u64 = 300;

/// Shown when a reload dropped an unsaved edit (TS: reconcile's `setError`).
const RELOADED_MESSAGE: &str =
    "These notes changed outside Deckhand (an editor?): reloaded them; your last unsaved edit was not saved";

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
    let session = session?;
    if section == NoteSection::Shared {
        return session.shared_notes.as_ref().map(|shared| NoteDoc {
            text: shared.text.clone(),
            revision: shared.revision.clone(),
            read_only: shared.read_only == Some(true),
            too_large: shared.too_large == Some(true),
        });
    }
    session.notes_file.as_ref().map(|file| NoteDoc {
        text: session.notes.clone().unwrap_or_default(),
        revision: file.revision.clone(),
        read_only: false,
        too_large: file.too_large == Some(true),
    })
}

/// TS: notesFlow.tsx plural.
pub fn plural(count: i64, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
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

/// TS: useNotesFlow's state.
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
    /// The client as of the last call (TS `latest.current.client`), for work that outlives it.
    latest_client: Option<Rc<LiveClient>>,
    /// What waits for each draft's save in flight (TS: the `pending` promise's `.then`s).
    waiters: HashMap<String, Vec<FlowThen<App, NotesFlow>>>,
}

impl std::fmt::Debug for NotesFlow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotesFlow")
            .field("drafts", &self.drafts)
            .field("timers", &self.timers.keys().collect::<Vec<_>>())
            .field("section", &self.section)
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
            section: NoteSection::Session,
            viewport_width: 40,
            viewport_rows: 10,
            pasting: false,
            was_focused: false,
            deferred_error: None,
            latest_client: None,
            waiters: HashMap::new(),
        }
    }
}

/// The notes focus keys.
pub static KEYMAP: Keymap = &[];

/// `Math.min(cursor, text.length)`, kept on a character boundary.
fn clamp_cursor(cursor: usize, text: &str) -> usize {
    let mut cursor = cursor.min(text.len());
    while !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    cursor
}

/// TS: reconcile's body: takes the file's newer version into the draft; whether that dropped an unsaved edit.
fn apply_reconcile(draft: &mut Draft, doc: &NoteDoc) -> bool {
    if doc.revision == draft.revision {
        return false;
    }
    if draft.in_flight.as_deref() == Some(doc.text.as_str()) {
        draft.saved = doc.text.clone();
        draft.revision = doc.revision.clone();
        return false;
    }
    let unsaved = draft.text != draft.saved || draft.in_flight.is_some();
    let discarded = unsaved && draft.text != doc.text;
    draft.text = doc.text.clone();
    draft.saved = doc.text.clone();
    draft.revision = doc.revision.clone();
    draft.cursor = clamp_cursor(draft.cursor, &draft.text);
    discarded
}

/// A hint part with a short form and a drop rank.
fn hint_part(text: &str, short: Option<&str>, drop: i32) -> HintPart {
    HintPart {
        text: text.to_owned(),
        short: short.map(str::to_owned),
        drop: Some(drop),
    }
}

impl NotesFlow {
    /// TS: NotesFlow.focus (Enter / →): start editing (the section last edited, the session's by default).
    pub fn focus(&mut self, props: &NotesFlowProps<'_>) {
        self.latest_client = props.client.cloned();
        if self.section == NoteSection::Shared && !shared_editable(props.session) {
            self.section = NoteSection::Session;
        }
    }

    /// The section being shown/edited (TS `activeSection`).
    pub fn active_section(&self, props: &NotesFlowProps<'_>) -> NoteSection {
        let has_shared = props.session.is_some_and(|session| session.shared_notes.is_some());
        let own_note = own_note(props.session);
        if has_shared && (self.section == NoteSection::Shared || !own_note) {
            NoteSection::Shared
        } else {
            NoteSection::Session
        }
    }

    /// TS: the `[focused]` effect: leaving notes focus saves every draft at once.
    pub fn sync(&mut self, props: &NotesFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        self.latest_client = props.client.cloned();
        let changed = self.was_focused != props.focused;
        self.was_focused = props.focused;
        if changed && !props.focused {
            self.pasting = false;
            let keys: Vec<String> = self.drafts.keys().cloned().collect();
            for key in keys {
                self.flush(&key, props.client, cx, None);
            }
        }
        self.report_deferred(cx);
    }

    /// Reports the error a render found (TS: the microtask `queueMicrotask` queued).
    fn report_deferred(&mut self, cx: &mut FlowCx<App, Self>) {
        if let Some(message) = self.deferred_error.take() {
            cx.set_error(message);
        }
    }

    /// TS: useNotesFlow reconcile: applies a newer version of the file (silently when nothing is unsaved).
    pub fn reconcile(draft: &mut Draft, doc: &NoteDoc, cx: &mut FlowCx<App, Self>) {
        if apply_reconcile(draft, doc) {
            cx.set_error(RELOADED_MESSAGE.to_owned());
        }
    }

    /// draftFor's body without a cx (render has none): the key of the draft, and whether reconciling dropped an
    /// unsaved edit.
    fn draft_core(&mut self, session: &SessionRecord, section: NoteSection) -> Option<(String, bool)> {
        let key = note_key(session, section)?;
        let doc = doc_of(Some(session), section)?;
        if let Some(draft) = self.drafts.get_mut(&key) {
            let discarded = apply_reconcile(draft, &doc);
            return Some((key, discarded));
        }
        let draft = Draft {
            session_id: session.id.clone(),
            section,
            note_id: (section == NoteSection::Shared).then(|| key.clone()),
            cursor: doc.text.len(),
            saved: doc.text.clone(),
            text: doc.text,
            revision: doc.revision,
            in_flight: None,
            scroll_top: 0,
        };
        self.drafts.insert(key.clone(), draft);
        Some((key, false))
    }

    /// TS: useNotesFlow draftFor: the draft of `session`'s `section` note (created or reconciled); its key.
    pub fn draft_for(
        &mut self,
        session: &SessionRecord,
        section: NoteSection,
        _cx: &mut FlowCx<App, Self>,
    ) -> Option<String> {
        // The discarded-edit error is reported when the call ends, after the call's own errors, as TS's microtask.
        let (key, discarded) = self.draft_core(session, section)?;
        if discarded {
            self.deferred_error = Some(RELOADED_MESSAGE.to_owned());
        }
        Some(key)
    }

    /// What `f` gets of the draft `draftFor` would give, without touching it: the text and cursor after reconciling.
    fn peek<R>(&self, session: &SessionRecord, section: NoteSection, f: impl FnOnce(&str, usize) -> R) -> Option<R> {
        let key = note_key(session, section)?;
        let doc = doc_of(Some(session), section)?;
        let Some(draft) = self.drafts.get(&key) else {
            return Some(f(&doc.text, doc.text.len()));
        };
        if draft.revision == doc.revision {
            return Some(f(&draft.text, draft.cursor));
        }
        let mut reloaded = draft.clone();
        apply_reconcile(&mut reloaded, &doc);
        Some(f(&reloaded.text, reloaded.cursor))
    }

    /// TS: useNotesFlow flush: saves the draft `key` now (or after the save in flight); `then` runs when it settled.
    pub fn flush(
        &mut self,
        key: &str,
        client: Option<&Rc<LiveClient>>,
        cx: &mut FlowCx<App, Self>,
        then: Option<FlowThen<App, NotesFlow>>,
    ) {
        self.latest_client = client.cloned();
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
        if draft.text == draft.saved {
            return run_now(self, cx, then);
        }
        let text = draft.text.clone();
        draft.in_flight = Some(text.clone());
        let request = client.save_note(
            &draft.session_id,
            draft.section,
            &text,
            &draft.revision,
            draft.note_id.as_deref(),
        );
        if let Some(then) = then {
            self.waiters.entry(key.to_owned()).or_default().push(then);
        }
        let key = key.to_owned();
        drop(cx.spawn(request, move |flow, result, cx| flow.saved(&key, result, cx)));
    }

    /// TS: flush's `.then(result => ..., error => ...)`: the save settled.
    fn saved(&mut self, key: &str, result: Result<NoteSaveResult, DhError>, cx: &mut FlowCx<App, Self>) {
        let Some(draft) = self.drafts.get_mut(key) else {
            return self.settle(key, cx);
        };
        draft.in_flight = None;
        match result {
            Ok(result) => {
                let doc = doc_of(Some(&result.session), draft.section);
                match doc {
                    Some(doc) if result.saved => {
                        draft.saved = doc.text;
                        draft.revision = doc.revision;
                    }
                    Some(doc) => Self::reconcile(draft, &doc, cx),
                    None => {}
                }
                // Typed while it was saving: save that too.
                if draft.text != draft.saved {
                    let client = self.latest_client.clone();
                    self.flush(key, client.as_ref(), cx, None);
                }
            }
            Err(error) => cx.set_error(format!("Notes not saved: {}", error_message(&error))),
        }
        self.settle(key, cx);
    }

    /// Runs what waited for the draft's saves, once none is in flight (TS: the `pending` promise resolving).
    fn settle(&mut self, key: &str, cx: &mut FlowCx<App, Self>) {
        if self.drafts.get(key).is_some_and(|draft| draft.in_flight.is_some()) {
            return;
        }
        for then in self.waiters.remove(key).unwrap_or_default() {
            then(self, cx);
        }
    }

    /// TS: useNotesFlow schedule: save `key` SAVE_DEBOUNCE_MS after the last edit.
    pub fn schedule(&mut self, key: &str, cx: &mut FlowCx<App, Self>) {
        let owned = key.to_owned();
        let timer = cx.after(Duration::from_millis(SAVE_DEBOUNCE_MS), move |flow, cx| {
            let client = flow.latest_client.clone();
            flow.flush(&owned, client.as_ref(), cx, None);
        });
        // Replacing the timer cancels the old one (clearTimeout).
        self.timers.insert(key.to_owned(), timer);
    }

    /// TS: useNotesFlow fileOf: `which` saved first, so the editor opens what is shown; the daemon creates the file
    /// if needed. `done` gets the file (or why not); it gets none when there is no client or session.
    fn file_of(&mut self, props: &NotesFlowProps<'_>, which: NoteSection, cx: &mut FlowCx<App, Self>, done: FileDone) {
        let (Some(client), Some(session)) = (props.client, props.session) else {
            drop(cx.dispatch().spawn(async {}, move |app: &mut App, ()| done(app, None)));
            return;
        };
        let key = note_key(session, which);
        let session_id = session.id.clone();
        let client = Rc::clone(client);
        let open: FlowThen<App, NotesFlow> = Box::new(move |_flow, cx| {
            let request = client.open_note(&session_id, which);
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

    /// TS: useNotesFlow openSection: the note in Cursor / VS Code.
    fn open_section(&mut self, props: &NotesFlowProps<'_>, which: NoteSection, cx: &mut FlowCx<App, Self>) {
        self.file_of(
            props,
            which,
            cx,
            Box::new(|app, result| {
                let effects = match result {
                    None => return,
                    Some(Err(error)) => vec![FlowEffect::SetError(Some(error_message(&error)))],
                    Some(Ok(file)) => {
                        let errors = RefCell::new(Vec::new());
                        let label = open_in_editor(
                            &file,
                            &|message| errors.borrow_mut().push(format!("{message}; the note is {file}")),
                            None,
                        );
                        let mut effects: Vec<FlowEffect<App>> = errors
                            .into_inner()
                            .into_iter()
                            .map(|message| FlowEffect::SetError(Some(message)))
                            .collect();
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

    /// TS: NotesFlow.openActiveInEditor (E / Ctrl+O): the active section's file in Cursor / VS Code.
    pub fn open_active_in_editor(&mut self, props: &NotesFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        self.latest_client = props.client.cloned();
        let which = self.active_section(props);
        self.open_section(props, which, cx);
    }

    /// TS: NotesFlow.activeFile (o): saves the active section, then `then` gets its file (none if unavailable).
    pub fn active_file(&mut self, props: &NotesFlowProps<'_>, cx: &mut FlowCx<App, Self>, then: FileThen) {
        self.latest_client = props.client.cloned();
        let active = self.active_section(props);
        // A deleted worktree's note is read-only: edit the session's instead.
        let which = if active == NoteSection::Shared && !shared_editable(props.session) && own_note(props.session) {
            NoteSection::Session
        } else {
            active
        };
        self.file_of(
            props,
            which,
            cx,
            Box::new(|app, result| match result {
                Some(Ok(file)) => then(app, Some(file)),
                Some(Err(error)) => {
                    app.apply_flow_effects(vec![FlowEffect::SetError(Some(error_message(&error)))]);
                    then(app, None);
                }
                None => then(app, None),
            }),
        );
    }

    /// TS: useNotesFlow promote (Ctrl+P): the cursor line's open checklist item becomes a task.
    pub fn promote(&mut self, section: NoteSection, props: &NotesFlowProps<'_>, cx: &mut FlowCx<App, Self>) {
        self.latest_client = props.client.cloned();
        let (Some(client), Some(session)) = (props.client, props.session) else {
            return;
        };
        let Some(key) = self.draft_for(session, section, cx) else {
            return;
        };
        let Some(draft) = self.drafts.get(&key) else {
            return;
        };
        let before_cursor = &draft.text[..clamp_cursor(draft.cursor, &draft.text)];
        let line = i64::try_from(before_cursor.matches('\n').count()).unwrap_or(i64::MAX);
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
                let sent = draft.text.clone();
                let request =
                    client.promote_note_item(&session_id, section, line, &draft.revision, draft.note_id.as_deref());
                drop(cx.spawn(request, move |flow, result, cx| {
                    flow.promoted(&key, section, &sent, result, cx)
                }));
            })
        };
        self.flush(&key, props.client, cx, Some(send));
    }

    /// TS: promote's `.then(({result, sent}) => ..., error => setError(...))`.
    fn promoted(
        &mut self,
        key: &str,
        section: NoteSection,
        sent: &str,
        result: Result<PromoteNoteItemResult, DhError>,
        cx: &mut FlowCx<App, Self>,
    ) {
        let result = match result {
            Ok(result) => result,
            Err(error) => return cx.set_error(error_message(&error)),
        };
        if let (Some(draft), Some(doc)) = (self.drafts.get_mut(key), doc_of(Some(&result.session), section)) {
            if draft.text == sent {
                draft.text = doc.text;
                draft.saved = draft.text.clone();
                draft.revision = doc.revision;
                draft.cursor = clamp_cursor(draft.cursor, &draft.text);
            } else {
                Self::reconcile(draft, &doc, cx);
            }
        }
        cx.emit(FlowEffect::TasksDoc(Box::new(result.tasks)));
        cx.set_status("Sent to Tasks (b shows it); the note keeps a ↗ link".to_owned());
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
            draft.session_id = session.id.clone();
            draft.section = section;
            self.schedule(key, cx);
        }
    }

    /// The editor's options for the notes (TS: handleInput's `options`).
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

    /// TS: handleInput's body.
    fn handle_key(&mut self, props: &NotesFlowProps<'_>, input: &Input, cx: &mut FlowCx<App, Self>) {
        let Some(session) = props.session else {
            cx.exit();
            return;
        };
        let key = &input.key;
        let which = self.active_section(props);
        let draft_key = self.draft_for(session, which, cx);
        let doc = doc_of(Some(session), which);
        // Bracketed paste (enabled while editing): everything up to the end marker is text, Tab and Enter included.
        if input.input == "[200~" {
            self.pasting = true;
            return;
        }
        if input.input == "[201~" {
            self.pasting = false;
            return;
        }
        let options = self.edit_options();
        let editable = draft_key.is_some() && doc.as_ref().is_none_or(|doc| !doc.read_only && !doc.too_large);
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
                self.edit(draft_key, next, session, which, cx);
            }
            return;
        }
        if key.escape {
            cx.exit();
            return;
        }
        let has_shared = session.shared_notes.is_some();
        if key.tab {
            if !own_note(Some(session)) {
                cx.set_status("In a worktree there is one note, shared by its sessions".to_owned());
                return;
            }
            if !has_shared {
                cx.set_status("This session has no worktree note yet: its worktree is not ready".to_owned());
                return;
            }
            if which == NoteSection::Session && !shared_editable(Some(session)) {
                cx.set_status("Its worktree was deleted: the worktree note is read-only".to_owned());
                return;
            }
            self.section = if which == NoteSection::Shared {
                NoteSection::Session
            } else {
                NoteSection::Shared
            };
            return;
        }
        if key.ctrl && input.input == "o" {
            self.open_section(props, which, cx);
            return;
        }
        let Some(draft_key) = draft_key else {
            return;
        };
        let too_large = doc.as_ref().is_some_and(|doc| doc.too_large);
        if key.ctrl && input.input == "p" {
            if !editable {
                cx.set_error(
                    if too_large {
                        "This note is too large to change here"
                    } else {
                        "Its worktree was deleted: the worktree note is read-only"
                    }
                    .to_owned(),
                );
                return;
            }
            self.promote(which, props, cx);
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
            cx.set_error(if too_large {
                format!("This note is longer than {MAX_NOTES_LABEL}: edit it in your editor (Ctrl+O)")
            } else {
                "Its worktree was deleted: the worktree note is read-only".to_owned()
            });
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
        self.edit(&draft_key, next, session, which, cx);
    }
}

/// TS: `!session || showsOwnNote(session)`.
fn own_note(session: Option<&SessionRecord>) -> bool {
    session.is_none_or(shows_own_note)
}

/// TS: `Boolean(session?.sharedNotes && !session.sharedNotes.readOnly)`.
fn shared_editable(session: Option<&SessionRecord>) -> bool {
    session
        .and_then(|session| session.shared_notes.as_ref())
        .is_some_and(|shared| shared.read_only != Some(true))
}

impl Flow<App> for NotesFlow {
    type Props<'a> = NotesFlowProps<'a>;

    fn handle_input(&mut self, props: &NotesFlowProps<'_>, input: &Input, cx: &mut FlowCx<App, Self>) {
        self.latest_client = props.client.cloned();
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
        let active = self.active_section(props);
        let own_note = own_note(Some(session));
        let has_shared = session.shared_notes.is_some();
        let editing_section = focused.then_some(active);
        let reload = |flow: &mut Self, which| {
            let (key, discarded) = flow.draft_core(session, which)?;
            if discarded {
                flow.deferred_error = Some(RELOADED_MESSAGE.to_owned());
            }
            Some(key)
        };
        let own_key = reload(self, NoteSection::Session);
        let shared_key_of_draft = if has_shared {
            reload(self, NoteSection::Shared)
        } else {
            None
        };
        let draft_of = |flow: &Self, key: &Option<String>| key.as_ref().and_then(|key| flow.drafts.get(key)).cloned();
        let own = draft_of(self, &own_key);
        let shared = draft_of(self, &shared_key_of_draft);

        let titled = |target: &SessionRecord| {
            let title = display_session_title(target, props.sessions);
            if title.is_empty() {
                "(untitled)".to_owned()
            } else {
                title
            }
        };
        let shared_note = session.shared_notes.as_ref();
        let shared_key = note_key(session, NoteSection::Shared);
        let sharing: Vec<&&SessionRecord> = match &shared_key {
            Some(shared_key) => props
                .sessions
                .iter()
                .filter(|other| note_key(other, NoteSection::Shared).as_ref() == Some(shared_key))
                .collect(),
            None => Vec::new(),
        };
        // Archived sessions (gone from the active view) are not counted; the one shown always is.
        let count = sharing
            .iter()
            .filter(|other| other.archived_at.as_deref().is_none_or(str::is_empty) || other.id == session.id)
            .count()
            .max(1);
        let mut shared_input = None;
        if let (Some(shared_note), Some(shared)) = (shared_note, &shared) {
            let own_branch = session.worktree.as_ref().and_then(|worktree| worktree.branch.clone());
            let branch = own_branch.filter(|branch| !branch.is_empty()).or_else(|| {
                sharing
                    .iter()
                    .filter_map(|other| other.worktree.as_ref())
                    .find(|worktree| {
                        worktree.branch.as_deref().is_some_and(|branch| !branch.is_empty())
                            && worktree.is_main != Some(true)
                    })
                    .and_then(|worktree| worktree.branch.clone())
            });
            let is_repo = shared_note.kind == SharedNoteKind::Repo;
            let where_ = if is_repo {
                "main checkout".to_owned()
            } else {
                branch.filter(|branch| !branch.is_empty()).unwrap_or_else(|| {
                    match session.worktree.as_ref().and_then(|worktree| worktree.path.as_deref()) {
                        Some(worktree_path) if !worktree_path.is_empty() => path::basename(worktree_path),
                        _ => "worktree".to_owned(),
                    }
                })
            };
            let what = if is_repo {
                "No main checkout notes"
            } else {
                "No worktree notes"
            };
            let read_only = shared_note.read_only == Some(true);
            let too_large = shared_note.too_large == Some(true);
            let sessions_label = plural(i64::try_from(count).unwrap_or(i64::MAX), "session");
            shared_input = Some(NotesSectionInput {
                titles: if is_repo {
                    vec![
                        format!("Main checkout (shared by {sessions_label})"),
                        "Main checkout".to_owned(),
                    ]
                } else {
                    vec![
                        format!("Worktree · {where_} (shared by {sessions_label})"),
                        format!("Worktree · {where_}"),
                        "Worktree".to_owned(),
                    ]
                },
                text: shared.text.clone(),
                empty: if read_only {
                    format!("{what} (its worktree was deleted)")
                } else if !own_note {
                    if focused {
                        format!("{what} · type to add")
                    } else {
                        format!("{what} · enter to add")
                    }
                } else if focused {
                    format!("{what} · tab to add")
                } else {
                    format!("{what} · enter, then tab to add")
                },
                flag: if read_only {
                    Some("deleted, read-only".to_owned())
                } else if too_large {
                    Some("too large, E to edit".to_owned())
                } else {
                    None
                },
                editing: (editing_section == Some(NoteSection::Shared)).then_some(NotesEditing {
                    cursor: shared.cursor,
                    scroll_top: shared.scroll_top,
                }),
                active: !focused && active == NoteSection::Shared,
            });
        }
        let session_input = own_note.then(|| NotesSectionInput {
            titles: vec![format!("This session · {}", titled(session)), "This session".to_owned()],
            text: own.as_ref().map(|own| own.text.clone()).unwrap_or_default(),
            empty: if focused {
                "No notes for this session · tab to add".to_owned()
            } else {
                "No notes for this session · enter to add".to_owned()
            },
            flag: session
                .notes_file
                .as_ref()
                .is_some_and(|file| file.too_large == Some(true))
                .then(|| "too large, E to edit".to_owned()),
            editing: own
                .as_ref()
                .filter(|_| editing_section == Some(NoteSection::Session))
                .map(|own| NotesEditing {
                    cursor: own.cursor,
                    scroll_top: own.scroll_top,
                }),
            active: !focused && active == NoteSection::Session && has_shared,
        });
        let layout = notes_layout(&NotesLayoutInput {
            shared: shared_input.as_ref(),
            session: session_input.as_ref(),
            width,
            height,
            focus: editing_section,
            links: props.links,
        });
        let edited_key = match editing_section {
            Some(NoteSection::Shared) => shared_key_of_draft.as_ref(),
            Some(NoteSection::Session) => own_key.as_ref(),
            None => None,
        };
        if let (Some(edited), Some(scroll_top)) =
            (edited_key.and_then(|key| self.drafts.get_mut(key)), layout.scroll_top)
        {
            edited.scroll_top = scroll_top;
        }
        // The editor's wrap width and body rows, for Up/Down and PageUp/PageDown.
        self.viewport_width = width.max(1);
        self.viewport_rows = match editing_section {
            Some(NoteSection::Shared) => layout.bodies.shared,
            Some(NoteSection::Session) => layout.bodies.session,
            None => 1,
        }
        .max(1);
        render_notes_rows(&layout.rows, buf, area);
    }

    /// TS: NotesFlow.hint: the notes-focus footer.
    fn hint(&self, props: &NotesFlowProps<'_>, width: usize) -> String {
        let session = props.session;
        let has_shared = session.is_some_and(|session| session.shared_notes.is_some());
        let active = self.active_section(props);
        let mut parts: Vec<HintPart> = vec![HintPart::from("esc done")];
        if has_shared && own_note(session) {
            let text = if active == NoteSection::Shared {
                "tab this session"
            } else if session
                .and_then(|session| session.shared_notes.as_ref())
                .is_some_and(|shared| shared.kind == SharedNoteKind::Repo)
            {
                "tab main checkout notes"
            } else {
                "tab worktree notes"
            };
            parts.push(hint_part(text, Some("tab switch"), 1));
        }
        parts.push(hint_part("ctrl+x toggle ☐", Some("ctrl+x ☐"), 1));
        parts.push(hint_part("ctrl+t new item", None, 2));
        parts.push(hint_part("ctrl+o open in editor", Some("ctrl+o editor"), 2));
        // On an open checklist item, Ctrl+P (send it to Tasks) takes the word keys' place.
        let on_open_item = session
            .and_then(|session| {
                self.peek(session, active, |text, cursor| {
                    let line = text
                        .get(line_start(text, cursor)..line_end(text, cursor))
                        .unwrap_or_default();
                    parse_checklist_line(line).is_some_and(|item| !item.checked && !js_trim(&item.text).is_empty())
                })
            })
            .unwrap_or(false);
        if on_open_item {
            parts.push(hint_part("ctrl+p → tasks", None, 2));
        }
        fit_hint(&parts, width, " • ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::event_loop::{Dispatch, Update};
    use crate::app::state::{AppProps, Mode};
    use crate::app::test_support::{app, session};
    use deckhand_client::live::LiveClientHandlers;
    use deckhand_protocol::data::{NoteFile, SessionStatus, SessionWorktreeMode, SessionWorktreeRecord, SharedNote};
    use serde_json::{Value, json};
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

    /// A session whose own note is `text` at `revision`.
    fn noted(id: &str, text: &str, revision: &str) -> SessionRecord {
        let mut record = session(id, "Fix login", SessionStatus::Running);
        record.notes = Some(text.to_owned());
        record.notes_file = Some(NoteFile {
            path: format!("/notes/{id}.md"),
            revision: revision.to_owned(),
            too_large: None,
        });
        record
    }

    /// `record` with the shared note `wt1` (text, revision) beside its own, as a main-checkout session has.
    fn side_by_side(mut record: SessionRecord, text: &str, revision: &str) -> SessionRecord {
        record.shared_notes = Some(SharedNote {
            kind: SharedNoteKind::Worktree,
            id: "wt1".to_owned(),
            path: "/notes/wt1.md".to_owned(),
            text: text.to_owned(),
            revision: revision.to_owned(),
            too_large: None,
            read_only: None,
        });
        record
    }

    /// `record` in the worktree note `wt1` (text, revision), the worktree on `branch`: it shows only that note.
    fn in_worktree(mut record: SessionRecord, text: &str, revision: &str, branch: &str) -> SessionRecord {
        let mut worktree = SessionWorktreeRecord::new(SessionWorktreeMode::Managed);
        worktree.branch = Some(branch.to_owned());
        worktree.path = Some("/repo/.wt/feat-x".to_owned());
        worktree.id = Some("wt1".to_owned());
        record.worktree = Some(worktree);
        side_by_side(record, text, revision)
    }

    fn read_only(mut record: SessionRecord) -> SessionRecord {
        if let Some(note) = record.shared_notes.as_mut() {
            note.read_only = Some(true);
        }
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

    impl Harness {
        /// An app in notes focus on `selected` of `sessions`, connected to a fake daemon.
        fn new(sessions: Vec<SessionRecord>, selected: &str) -> Harness {
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
            app.browse.selected_id = Some(selected.to_owned());
            app.mode = Mode::NotesFocus;
            Harness {
                app,
                rx,
                daemon: Daemon { lines, writer },
            }
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

        fn ctrl(&mut self, letter: &str) {
            self.input(Input::key(
                letter,
                Key {
                    ctrl: true,
                    ..Key::NONE
                },
            ));
        }

        /// The daemon's answer to a save.
        async fn saved(&mut self, request: &Value, record: &SessionRecord, saved: bool) {
            let body = json!({"ok": true, "data": {"saved": saved, "session": record}});
            self.daemon.respond(request, body).await;
        }

        fn render(&mut self, width: u16, height: u16) -> Vec<String> {
            let area = Rect::new(0, 0, width, height);
            let mut buf = Buffer::empty(area);
            self.app
                .notes_flow_call(|flow, props, _cx| flow.render(props, &mut buf, area));
            (0..height)
                .map(|y| {
                    (0..width)
                        .map(|x| buf[(x, y)].symbol().to_owned())
                        .collect::<String>()
                        .trim_end()
                        .to_owned()
                })
                .collect()
        }

        fn hint(&mut self, width: usize) -> String {
            self.app.notes_flow_call(|flow, props, _cx| flow.hint(props, width))
        }

        fn replace_session(&mut self, record: SessionRecord) {
            self.app.data.repo_sessions = vec![record];
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

    const KEY: &str = "session:s1";

    fn key(update: impl FnOnce(&mut Key)) -> Key {
        let mut key = Key::NONE;
        update(&mut key);
        key
    }

    // doc_of / plural / reconcile

    #[test]
    fn doc_of_reads_the_note_of_each_section() {
        let own = noted("s1", "- [ ] a", "r1");
        assert_eq!(
            doc_of(Some(&own), NoteSection::Session),
            Some(NoteDoc {
                text: "- [ ] a".to_owned(),
                revision: "r1".to_owned(),
                read_only: false,
                too_large: false
            })
        );
        assert_eq!(doc_of(Some(&own), NoteSection::Shared), None);
        assert_eq!(doc_of(None, NoteSection::Session), None);
        let mut bare = session("s2", "T", SessionStatus::Running);
        assert_eq!(doc_of(Some(&bare), NoteSection::Session), None);
        bare.notes_file = Some(NoteFile {
            path: "/n".to_owned(),
            revision: "r".to_owned(),
            too_large: Some(true),
        });
        let doc = doc_of(Some(&bare), NoteSection::Session).expect("doc");
        assert_eq!((doc.text.as_str(), doc.too_large), ("", true));
        let shared = read_only(in_worktree(own, "shared", "w1", "feat/x"));
        let doc = doc_of(Some(&shared), NoteSection::Shared).expect("doc");
        assert_eq!(
            (doc.text.as_str(), doc.revision.as_str(), doc.read_only),
            ("shared", "w1", true)
        );
    }

    #[test]
    fn plural_adds_an_s_except_for_one() {
        assert_eq!(plural(1, "session"), "1 session");
        assert_eq!(plural(0, "session"), "0 sessions");
        assert_eq!(plural(3, "session"), "3 sessions");
    }

    fn draft(text: &str, saved: &str, revision: &str) -> Draft {
        Draft {
            session_id: "s1".to_owned(),
            section: NoteSection::Session,
            note_id: None,
            text: text.to_owned(),
            cursor: text.len(),
            saved: saved.to_owned(),
            revision: revision.to_owned(),
            in_flight: None,
            scroll_top: 0,
        }
    }

    fn doc(text: &str, revision: &str) -> NoteDoc {
        NoteDoc {
            text: text.to_owned(),
            revision: revision.to_owned(),
            ..NoteDoc::default()
        }
    }

    #[test]
    fn reconcile_reloads_silently_unless_it_drops_an_unsaved_edit() {
        // The same revision: nothing to do.
        let mut same = draft("mine", "mine", "r1");
        assert!(!apply_reconcile(&mut same, &doc("other", "r1")));
        assert_eq!(same.text, "mine");
        // Nothing unsaved: the new file is taken, the cursor kept inside it.
        let mut clean = draft("long text", "long text", "r1");
        assert!(!apply_reconcile(&mut clean, &doc("ab", "r2")));
        assert_eq!(
            (
                clean.text.as_str(),
                clean.saved.as_str(),
                clean.revision.as_str(),
                clean.cursor
            ),
            ("ab", "ab", "r2", 2)
        );
        // An unsaved edit that the new file lacks: dropped, and reported.
        let mut dirty = draft("mine!", "mine", "r1");
        assert!(apply_reconcile(&mut dirty, &doc("theirs", "r2")));
        assert_eq!(dirty.text, "theirs");
        // The unsaved edit is what the file now says: nothing lost.
        let mut same_text = draft("mine!", "mine", "r1");
        assert!(!apply_reconcile(&mut same_text, &doc("mine!", "r2")));
        // The echo of the save in flight only moves the revision.
        let mut flying = draft("mine!!", "mine", "r1");
        flying.in_flight = Some("mine!".to_owned());
        assert!(!apply_reconcile(&mut flying, &doc("mine!", "r2")));
        assert_eq!(
            (flying.text.as_str(), flying.saved.as_str(), flying.revision.as_str()),
            ("mine!!", "mine!", "r2")
        );
        // A different file while one is in flight: the edit is dropped.
        let mut lost = draft("mine!!", "mine", "r1");
        lost.in_flight = Some("mine!".to_owned());
        assert!(apply_reconcile(&mut lost, &doc("theirs", "r2")));
        // The cursor never lands inside a character.
        let mut multibyte = draft("aé", "aé", "r1");
        multibyte.cursor = 3;
        apply_reconcile(&mut multibyte, &doc("é", "r2"));
        assert_eq!(multibyte.cursor, 2);
    }

    // render

    #[test]
    fn without_a_session_it_says_to_select_one() {
        run(async {
            let mut h = Harness::new(vec![], "none");
            assert_eq!(h.render(40, 2), ["Select a session from the sidebar.", ""]);
            // Any key leaves notes focus.
            h.type_text("x");
            h.pump().await;
            assert_eq!(h.app.mode, Mode::Browse);
        });
    }

    #[test]
    fn a_session_note_shows_its_title_and_the_empty_line() {
        run(async {
            let mut h = Harness::new(vec![noted("s1", "", "r1")], "s1");
            h.app.mode = Mode::Browse;
            assert_eq!(
                h.render(50, 2),
                ["This session · Fix login", "No notes for this session · enter to add"]
            );
            h.app.mode = Mode::NotesFocus;
            let rows = h.render(50, 2);
            assert_eq!(rows[1], "");
            assert_eq!(h.flow().viewport_width, 50);
            let mut h = Harness::new(vec![noted("s1", "- [ ] ship it", "r1")], "s1");
            h.app.mode = Mode::Browse;
            assert_eq!(
                h.render(50, 3),
                ["This session · Fix login · ☐ 1 open", "☐ ship it", ""]
            );
        });
    }

    #[test]
    fn a_worktree_note_names_its_branch_and_the_sessions_sharing_it() {
        run(async {
            let first = in_worktree(noted("s1", "", "r1"), "- [ ] shared item", "w1", "feat/x");
            let mut second = in_worktree(noted("s2", "", "r2"), "- [ ] shared item", "w1", "feat/x");
            let mut third = in_worktree(noted("s3", "", "r3"), "- [ ] shared item", "w1", "feat/x");
            second.title = "Other".to_owned();
            let mut h = Harness::new(vec![first.clone(), second.clone(), third.clone()], "s1");
            h.app.mode = Mode::Browse;
            let rows = h.render(60, 6);
            assert_eq!(rows[0], "Worktree · feat/x (shared by 3 sessions) · ☐ 1 open");
            assert_eq!(rows[1], "☐ shared item");
            // An archived session is not counted.
            third.archived_at = Some("2026-01-02T00:00:00.000Z".to_owned());
            h.app.data.repo_sessions = vec![first, second, third];
            let rows = h.render(60, 6);
            assert_eq!(rows[0], "Worktree · feat/x (shared by 2 sessions) · ☐ 1 open");
        });
    }

    #[test]
    fn a_worktree_note_without_a_branch_is_named_by_its_folder() {
        run(async {
            let mut record = in_worktree(noted("s1", "", "r1"), "", "w1", "");
            if let Some(worktree) = record.worktree.as_mut() {
                worktree.branch = None;
            }
            let mut h = Harness::new(vec![record], "s1");
            h.app.mode = Mode::Browse;
            assert_eq!(h.render(60, 6)[0], "Worktree · feat-x (shared by 1 session)");
        });
    }

    #[test]
    fn a_main_checkout_note_and_its_empty_lines() {
        run(async {
            let mut record = in_worktree(noted("s1", "- [ ] own", "r1"), "", "w1", "main");
            if let Some(note) = record.shared_notes.as_mut() {
                note.kind = SharedNoteKind::Repo;
                note.id = "repo1".to_owned();
            }
            let mut h = Harness::new(vec![record], "s1");
            h.app.mode = Mode::Browse;
            let rows = h.render(60, 8);
            assert_eq!(rows[0], "Main checkout (shared by 1 session)");
            assert_eq!(rows[1], "No main checkout notes · enter, then tab to add");
            h.app.mode = Mode::NotesFocus;
            assert_eq!(h.render(60, 8)[1], "No main checkout notes · tab to add");
        });
    }

    #[test]
    fn a_worktree_session_shows_only_the_worktree_note() {
        run(async {
            let record = in_worktree(session("s1", "T", SessionStatus::Running), "", "w1", "feat/x");
            let mut h = Harness::new(vec![record], "s1");
            h.app.mode = Mode::Browse;
            let rows = h.render(60, 4);
            assert_eq!(rows[0], "Worktree · feat/x (shared by 1 session)");
            assert_eq!(rows[1], "No worktree notes · enter to add");
            assert!(rows.iter().all(|row| !row.contains("This session")), "{rows:?}");
            h.app.mode = Mode::NotesFocus;
            assert_eq!(h.render(60, 4)[1], "");
            assert!(!h.hint(200).contains("tab"));
        });
    }

    #[test]
    fn a_deleted_worktree_note_is_flagged_read_only() {
        run(async {
            let record = read_only(in_worktree(noted("s1", "", "r1"), "", "w1", "feat/x"));
            let mut h = Harness::new(vec![record], "s1");
            h.app.mode = Mode::Browse;
            let rows = h.render(70, 8);
            assert_eq!(rows[0], "Worktree · feat/x (shared by 1 session) · deleted, read-only");
            assert_eq!(rows[1], "No worktree notes (its worktree was deleted)");
        });
    }

    #[test]
    fn the_editing_section_scrolls_and_reports_its_viewport() {
        run(async {
            let text = (1..=30)
                .map(|index| format!("line {index}"))
                .collect::<Vec<_>>()
                .join("\n");
            let mut h = Harness::new(vec![noted("s1", &text, "r1")], "s1");
            let rows = h.render(30, 8);
            // Editing: the cursor is at the end, so the last lines are shown.
            assert_eq!(rows[0], "This session · Fix login");
            assert_eq!(rows[7], "line 30");
            assert_eq!(h.flow().viewport_rows, 7);
            assert!(h.draft(KEY).scroll_top > 0);
        });
    }

    // hint

    #[test]
    fn the_hint_follows_the_section_and_the_cursor_line() {
        run(async {
            let mut h = Harness::new(vec![noted("s1", "plain", "r1")], "s1");
            assert_eq!(
                h.hint(200),
                "esc done • ctrl+x toggle ☐ • ctrl+t new item • ctrl+o open in editor"
            );
            assert_eq!(h.hint(48), "esc done • ctrl+x ☐ • ctrl+t new item");
            let mut h = Harness::new(vec![noted("s1", "- [ ] do it", "r1")], "s1");
            assert!(h.hint(200).ends_with(" • ctrl+p → tasks"), "{}", h.hint(200));
            let mut h = Harness::new(vec![noted("s1", "- [x] done", "r1")], "s1");
            assert!(!h.hint(200).contains("ctrl+p"));
            let mut h = Harness::new(vec![noted("s1", "- [ ]  ", "r1")], "s1");
            assert!(!h.hint(200).contains("ctrl+p"));
            let shared = side_by_side(noted("s1", "", "r1"), "", "w1");
            let mut h = Harness::new(vec![shared.clone()], "s1");
            assert!(h.hint(200).contains("tab worktree notes"), "{}", h.hint(200));
            h.app.flows.notes.section = NoteSection::Shared;
            assert!(h.hint(200).contains("tab this session"), "{}", h.hint(200));
            let mut repo = shared;
            if let Some(note) = repo.shared_notes.as_mut() {
                note.kind = SharedNoteKind::Repo;
            }
            let mut h = Harness::new(vec![repo], "s1");
            assert!(h.hint(200).contains("tab main checkout notes"), "{}", h.hint(200));
        });
    }

    // keys

    #[test]
    fn escape_leaves_notes_focus() {
        run(async {
            let mut h = Harness::new(vec![noted("s1", "", "r1")], "s1");
            h.press(key(|key| key.escape = true));
            assert_eq!(h.app.mode, Mode::Browse);
        });
    }

    #[test]
    fn typing_edits_the_draft_and_saves_300ms_after_the_last_key() {
        run(async {
            let mut h = Harness::new(vec![noted("s1", "hi", "r1")], "s1");
            h.render(40, 5);
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
            assert_eq!(request["sessionId"], "s1");
            assert_eq!(request["section"], "session");
            assert_eq!(request["text"], "hiab");
            assert_eq!(request["revision"], "r1");
            assert!(request.get("noteId").is_none());
            assert_eq!(h.draft(KEY).in_flight.as_deref(), Some("hiab"));
            h.saved(&request, &noted("s1", "hiab", "r2"), true).await;
            h.pump().await;
            let saved = h.draft(KEY);
            assert_eq!(
                (saved.saved.as_str(), saved.revision.as_str(), saved.in_flight.clone()),
                ("hiab", "r2", None)
            );
            assert_eq!(h.error(), None);
        });
    }

    #[test]
    fn leaving_focus_saves_every_draft_at_once() {
        run(async {
            let mut h = Harness::new(vec![noted("s1", "", "r1")], "s1");
            h.render(40, 5);
            h.app.notes_flow_call(|flow, props, cx| flow.sync(props, cx));
            h.type_text("x");
            h.app.mode = Mode::Browse;
            h.app.notes_flow_call(|flow, props, cx| flow.sync(props, cx));
            let start = tokio::time::Instant::now();
            let request = h.request().await;
            assert!(start.elapsed() < Duration::from_millis(250));
            assert_eq!(request["text"], "x");
            // The debounce timer was cancelled: no second save.
            h.saved(&request, &noted("s1", "x", "r2"), true).await;
            h.run_for(Duration::from_millis(400)).await;
            assert!(h.daemon.lines.try_recv().is_err());
        });
    }

    #[test]
    fn text_typed_while_saving_is_saved_after_it() {
        run(async {
            let mut h = Harness::new(vec![noted("s1", "", "r1")], "s1");
            h.render(40, 5);
            h.type_text("a");
            let first = h.request().await;
            assert_eq!(first["text"], "a");
            h.type_text("b");
            // The timer fires while the first save is in flight: it waits for it.
            h.run_for(Duration::from_millis(400)).await;
            assert!(h.daemon.lines.try_recv().is_err());
            h.saved(&first, &noted("s1", "a", "r2"), true).await;
            let second = h.request().await;
            assert_eq!(
                (second["text"].as_str(), second["revision"].as_str()),
                (Some("ab"), Some("r2"))
            );
            h.saved(&second, &noted("s1", "ab", "r3"), true).await;
            h.pump().await;
            assert_eq!(
                (h.draft(KEY).saved.as_str(), h.draft(KEY).revision.as_str()),
                ("ab", "r3")
            );
        });
    }

    #[test]
    fn a_save_the_daemon_refused_reloads_the_file_and_says_the_edit_was_dropped() {
        run(async {
            let mut h = Harness::new(vec![noted("s1", "", "r1")], "s1");
            h.render(40, 5);
            h.type_text("mine");
            let request = h.request().await;
            h.saved(&request, &noted("s1", "theirs", "r9"), false).await;
            h.pump().await;
            let draft = h.draft(KEY);
            assert_eq!(
                (draft.text.as_str(), draft.saved.as_str(), draft.revision.as_str()),
                ("theirs", "theirs", "r9")
            );
            assert_eq!(h.error(), Some(RELOADED_MESSAGE));
        });
    }

    #[test]
    fn a_failed_save_is_reported_and_the_text_kept() {
        run(async {
            let mut h = Harness::new(vec![noted("s1", "", "r1")], "s1");
            h.render(40, 5);
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
    fn a_file_changed_elsewhere_is_reloaded_when_shown() {
        run(async {
            let mut h = Harness::new(vec![noted("s1", "old", "r1")], "s1");
            h.render(40, 5);
            h.replace_session(noted("s1", "new from the editor", "r2"));
            h.render(40, 5);
            assert_eq!(h.draft(KEY).text, "new from the editor");
            assert_eq!(h.error(), None);
            // An unsaved edit is dropped with a message, reported at the next call.
            h.type_text("!");
            h.replace_session(noted("s1", "again", "r3"));
            h.render(40, 5);
            assert_eq!(h.draft(KEY).text, "again");
            assert_eq!(h.error(), None);
            h.type_text("?");
            assert_eq!(h.error(), Some(RELOADED_MESSAGE));
        });
    }

    #[test]
    fn tab_switches_sections_and_says_why_it_cannot() {
        run(async {
            let tab = key(|key| key.tab = true);
            // No worktree note yet.
            let mut h = Harness::new(vec![noted("s1", "", "r1")], "s1");
            h.press(tab);
            assert_eq!(
                h.status(),
                Some("This session has no worktree note yet: its worktree is not ready")
            );
            // A worktree session has one note.
            let worktree = in_worktree(session("s1", "T", SessionStatus::Running), "", "w1", "b");
            let mut h = Harness::new(vec![worktree], "s1");
            h.press(tab);
            assert_eq!(
                h.status(),
                Some("In a worktree there is one note, shared by its sessions")
            );
            // Both: it switches, there and back.
            let mut h = Harness::new(vec![side_by_side(noted("s1", "", "r1"), "", "w1")], "s1");
            h.press(tab);
            assert_eq!(h.flow().section, NoteSection::Shared);
            h.type_text("w");
            assert_eq!(h.draft("worktree:wt1").text, "w");
            assert_eq!(h.draft("worktree:wt1").note_id.as_deref(), Some("worktree:wt1"));
            assert_eq!(h.draft(KEY).text, "");
            h.press(tab);
            assert_eq!(h.flow().section, NoteSection::Session);
            // A deleted worktree's note cannot be switched to.
            let gone = read_only(side_by_side(noted("s1", "", "r1"), "", "w1"));
            let mut h = Harness::new(vec![gone], "s1");
            h.press(tab);
            assert_eq!(
                h.status(),
                Some("Its worktree was deleted: the worktree note is read-only")
            );
            assert_eq!(h.flow().section, NoteSection::Session);
        });
    }

    #[test]
    fn a_shared_note_is_saved_with_its_note_id() {
        run(async {
            let mut h = Harness::new(vec![side_by_side(noted("s1", "", "r1"), "w", "w1")], "s1");
            h.press(key(|key| key.tab = true));
            h.type_text("!");
            let request = h.request().await;
            assert_eq!(
                (
                    request["section"].as_str(),
                    request["noteId"].as_str(),
                    request["text"].as_str(),
                    request["revision"].as_str()
                ),
                (Some("shared"), Some("worktree:wt1"), Some("w!"), Some("w1"))
            );
        });
    }

    #[test]
    fn focus_falls_back_to_the_session_note_when_the_shared_one_is_read_only() {
        run(async {
            let gone = read_only(side_by_side(noted("s1", "", "r1"), "", "w1"));
            let mut h = Harness::new(vec![gone], "s1");
            h.app.flows.notes.section = NoteSection::Shared;
            h.app.notes_flow_call(|flow, props, _cx| flow.focus(props));
            assert_eq!(h.flow().section, NoteSection::Session);
        });
    }

    #[test]
    fn a_read_only_note_allows_only_navigation() {
        run(async {
            let gone = read_only(in_worktree(
                session("s1", "T", SessionStatus::Running),
                "abc",
                "w1",
                "b",
            ));
            let mut h = Harness::new(vec![gone], "s1");
            h.type_text("x");
            assert_eq!(
                h.error(),
                Some("Its worktree was deleted: the worktree note is read-only")
            );
            assert_eq!(h.draft("worktree:wt1").text, "abc");
            h.app.messages.error = None;
            h.press(key(|key| key.left_arrow = true));
            assert_eq!(h.error(), None);
            assert_eq!(h.draft("worktree:wt1").cursor, 2);
            h.ctrl("p");
            assert_eq!(
                h.error(),
                Some("Its worktree was deleted: the worktree note is read-only")
            );
        });
    }

    #[test]
    fn a_too_large_note_is_edited_in_the_editor() {
        run(async {
            let mut big = noted("s1", "abc", "r1");
            if let Some(file) = big.notes_file.as_mut() {
                file.too_large = Some(true);
            }
            let mut h = Harness::new(vec![big], "s1");
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

    #[test]
    fn an_edit_over_the_limit_is_refused() {
        run(async {
            let mut h = Harness::new(vec![noted("s1", &"a".repeat(MAX_NOTES_CHARS), "r1")], "s1");
            h.type_text("b");
            assert_eq!(
                h.error(),
                Some(format!("Notes are limited to {MAX_NOTES_LABEL}; that edit was not applied.").as_str())
            );
            assert_eq!(js_len(&h.draft(KEY).text), MAX_NOTES_CHARS);
            // Deleting still works.
            h.app.messages.error = None;
            h.press(key(|key| key.backspace = true));
            assert_eq!(js_len(&h.draft(KEY).text), MAX_NOTES_CHARS - 1);
            assert_eq!(h.error(), None);
        });
    }

    #[test]
    fn bracketed_paste_inserts_enter_and_tab_as_text() {
        run(async {
            let mut h = Harness::new(vec![noted("s1", "", "r1")], "s1");
            h.input(Input::text("[200~"));
            assert!(h.flow().pasting);
            h.input(Input::text("one"));
            h.press(key(|key| key.return_ = true));
            h.press(key(|key| key.tab = true));
            h.input(Input::text("two\u{1b}[31m"));
            // Escape does not leave while pasting.
            h.press(key(|key| key.escape = true));
            assert_eq!(h.app.mode, Mode::NotesFocus);
            h.input(Input::text("[201~"));
            assert!(!h.flow().pasting);
            assert_eq!(h.draft(KEY).text, "one\n\ttwo");
            h.input(Input::text("x"));
            assert_eq!(h.draft(KEY).text, "one\n\ttwox");
        });
    }

    #[test]
    fn checklist_keys_toggle_add_and_continue_items() {
        run(async {
            let mut h = Harness::new(vec![noted("s1", "- [ ] one", "r1")], "s1");
            h.ctrl("x");
            assert_eq!(h.draft(KEY).text, "- [x] one");
            h.ctrl("x");
            assert_eq!(h.draft(KEY).text, "- [ ] one");
            h.press(key(|key| key.return_ = true));
            assert_eq!(h.draft(KEY).text, "- [ ] one\n- [ ] ");
            h.type_text("two");
            h.ctrl("t");
            assert_eq!(h.draft(KEY).text, "- [ ] one\n- [ ] two\n- [ ] ");
            // Enter on an empty item ends the list.
            h.press(key(|key| key.return_ = true));
            assert_eq!(h.draft(KEY).text, "- [ ] one\n- [ ] two\n");
            // A plain line breaks as usual.
            let mut h = Harness::new(vec![noted("s1", "note", "r1")], "s1");
            h.press(key(|key| key.return_ = true));
            assert_eq!(h.draft(KEY).text, "note\n");
        });
    }

    // promote

    #[test]
    fn ctrl_p_saves_then_sends_the_cursor_line_to_tasks() {
        run(async {
            let mut h = Harness::new(vec![noted("s1", "- [ ] one\n- [ ] two", "r1")], "s1");
            h.render(40, 5);
            h.press(key(|key| key.up_arrow = true));
            h.press(key(|key| key.end = true));
            h.type_text("!");
            let text = h.draft(KEY).text.clone();
            assert_eq!(text, "- [ ] one!\n- [ ] two");
            h.ctrl("p");
            // Saved first, so the daemon finds the line as shown.
            let save = h.request().await;
            assert_eq!(save["type"], "save-note");
            assert_eq!(save["text"].as_str(), Some(text.as_str()));
            h.saved(&save, &noted("s1", &text, "r2"), true).await;
            let promote = h.request().await;
            assert_eq!(promote["type"], "promote-note-item");
            assert_eq!(
                (promote["line"].as_i64(), promote["revision"].as_str()),
                (Some(0), Some("r2"))
            );
            let linked = noted("s1", "- [ ] one! ↗ T1\n- [ ] two", "r3");
            let body = json!({"ok": true, "data": {
                "session": linked,
                "tasks": {"key": "k", "path": "/repo/tasks.md", "text": "- [ ] one!", "revision": "t1"},
            }});
            h.daemon.respond(&promote, body).await;
            h.pump().await;
            assert_eq!(h.draft(KEY).text, "- [ ] one! ↗ T1\n- [ ] two");
            assert_eq!(h.draft(KEY).revision, "r3");
            assert_eq!(h.status(), Some("Sent to Tasks (b shows it); the note keeps a ↗ link"));
            assert_eq!(
                h.app.data.tasks_doc.as_ref().map(|doc| doc.revision.as_str()),
                Some("t1")
            );
            assert_eq!(h.error(), None);
        });
    }

    #[test]
    fn ctrl_p_reports_a_daemon_error() {
        run(async {
            let mut h = Harness::new(vec![noted("s1", "- [ ] one", "r1")], "s1");
            h.ctrl("p");
            let promote = h.request().await;
            assert_eq!(promote["type"], "promote-note-item");
            h.daemon
                .respond(&promote, json!({"ok": false, "error": "That line is not an open item"}))
                .await;
            h.pump().await;
            assert_eq!(h.error(), Some("That line is not an open item"));
            assert_eq!(h.status(), None);
        });
    }

    #[test]
    fn ctrl_p_refuses_while_the_note_is_still_unsaved() {
        run(async {
            let mut h = Harness::new(vec![noted("s1", "- [ ] one", "r1")], "s1");
            h.type_text("!");
            h.ctrl("p");
            let save = h.request().await;
            // The save fails, so the note is still not saved when the flush settles.
            h.daemon
                .respond(&save, json!({"ok": false, "error": "disk full"}))
                .await;
            h.pump().await;
            assert_eq!(h.error(), Some("These notes are not saved yet; try again"));
            assert!(h.daemon.lines.try_recv().is_err());
        });
    }

    // active file / open

    #[test]
    fn the_active_file_is_saved_first_and_handed_over() {
        run(async {
            let mut h = Harness::new(vec![noted("s1", "", "r1")], "s1");
            h.render(40, 5);
            h.type_text("x");
            let got = h.active_file();
            let save = h.request().await;
            assert_eq!(save["type"], "save-note");
            h.saved(&save, &noted("s1", "x", "r2"), true).await;
            let open = h.request().await;
            assert_eq!(
                (open["type"].as_str(), open["section"].as_str()),
                (Some("open-note"), Some("session"))
            );
            h.daemon
                .respond(&open, json!({"ok": true, "data": "/notes/s1.md"}))
                .await;
            h.pump().await;
            assert_eq!(*got.borrow(), [Some("/notes/s1.md".to_owned())]);
        });
    }

    #[test]
    fn the_active_file_of_a_deleted_worktree_is_the_sessions() {
        run(async {
            let gone = read_only(side_by_side(noted("s1", "", "r1"), "", "w1"));
            let mut h = Harness::new(vec![gone], "s1");
            h.app.flows.notes.section = NoteSection::Shared;
            let got = h.active_file();
            let open = h.request().await;
            assert_eq!(open["section"], "session");
            h.daemon
                .respond(&open, json!({"ok": false, "error": "no such note"}))
                .await;
            h.pump().await;
            assert_eq!(*got.borrow(), [None]);
            assert_eq!(h.error(), Some("no such note"));
        });
    }

    #[test]
    fn the_active_file_is_none_without_a_client() {
        run(async {
            let mut h = Harness::new(vec![noted("s1", "", "r1")], "s1");
            h.app.connection.client = None;
            let got = h.active_file();
            h.pump().await;
            assert_eq!(*got.borrow(), [None]);
        });
    }

    #[test]
    fn a_flush_without_a_client_runs_what_waits_at_once() {
        run(async {
            let (dispatch, _rx) = Dispatch::<App>::channel();
            let mut flow = NotesFlow::default();
            let mut cx = FlowCx::new(dispatch, |_app: &mut App| -> &mut NotesFlow { unreachable!() });
            let ran = Rc::new(RefCell::new(0));
            let counter = Rc::clone(&ran);
            flow.flush(
                "session:s1",
                None,
                &mut cx,
                Some(Box::new(move |_flow, _cx| *counter.borrow_mut() += 1)),
            );
            assert_eq!(*ran.borrow(), 1);
        });
    }
}
