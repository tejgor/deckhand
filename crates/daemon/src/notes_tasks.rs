//! The daemon's list and notes (`docs/future/work-items.md`).
//!
//! - **The list**: one per repository, `notes/tasks/<key>.md` (`key`: the hash of the repository's main checkout),
//!   changed only through TaskOps applied to the file as it is on disk (`deckhand_core::tasks::apply_task_op`). It
//!   nests; a session is on at most one line (`s=`). Clients watching it (`watch-tasks`) get `tasks-updated`.
//! - **Notes**, each a file under `notes/`: a session's own note (`sessions/<session id>.md`, projected into its record
//!   as `notes` and `notesFile`), a line's note (`lines/<line id>.md`), a line's brief (`briefs/<line id>.md`, who
//!   wrote it and when on its first line) and a repository's scratchpad (`repos/<key>.md`). A line's note and brief
//!   and the scratchpad reach the clients watching the list: all of them in `watch-tasks`' answer, then each change as
//!   `work-note-updated`, so a change sends one note, never all of them.
//! - **Lines on demand**: a session gets a line only when something needs one: a step, a line note or a brief for it
//!   ([`Daemon::ensure_session_line`]).
//! - **The rules**: a session started on a line is put on it (`sessions.rs`); merging a worktree ticks its sessions'
//!   lines and lifts their open steps out ([`Daemon::sync_worktree_tasks`]); done ticks the session's line and undone
//!   reopens it ([`Daemon::sync_done_tasks`]); a worktree deleted unmerged unhooks its sessions' lines, marked
//!   `tried`, and so does removing a session ([`Daemon::release_session_line`]).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use deckhand_core::error::{DhError, Result};
use deckhand_core::jsstr::{date, path};
use deckhand_core::notes::repo_note_id;
use deckhand_core::session_features::{HandoffBrief, HandoffStep, HandoffTask};
use deckhand_core::task_list::TaskList;
use deckhand_core::tasks::{apply_task_op, client_task_op, new_task_id};
use deckhand_core::work_notes::{
    Brief, append_paragraph, format_brief, open_note_text, parse_brief, session_line_title, take_checkbox,
};
use deckhand_protocol::data::{
    NoteRef, NoteSaveResult, PromoteNoteItemResult, SessionRecord, SessionStatus, TaskAuto, TaskLink, TaskOp, TasksDoc,
    WorkNote, WorkNoteKind,
};
use deckhand_protocol::events::{ServerMessage, TasksUpdated, WorkNoteUpdated};
use deckhand_protocol::records::WorktreeRecord;
use deckhand_protocol::wire::{CwdRequest, Loose, OpenNote, PromoteNoteItem, SaveNote, TaskOpRequest};
use deckhand_protocol::{KeyOrder, Ordered};
use deckhand_sys::async_util::{SharedTask, spawn};
use deckhand_sys::git::resolve_repo_context;
use deckhand_sys::notes_store::{NoteId, NoteKind, NotesStore, note_file_path, note_file_stem};
use futures::FutureExt;

use crate::daemon::Daemon;
use crate::server::{Connection, send_message};

/// The list's and the notes' part of the daemon state.
#[derive(Default)]
pub struct NotesTasksState {
    /// The notes store (created by [`Daemon::new`], wired to [`Daemon::notes_changed`]).
    pub notes: Option<NotesStore>,
    /// The list key of a checkout path, resolved once (a failed lookup is forgotten).
    pub tasks_keys: HashMap<String, SharedTask<Result<String>>>,
    /// Worktree records whose merge, about to be recorded, keeps the open steps under the merged lines
    /// (`merge-worktree`'s `keepOpenSteps`).
    pub keep_open_steps: HashSet<String>,
    /// For the UIs (`ping`'s `notice`): what went wrong at start that the user should know about.
    pub notice: Option<String>,
}

/// JS truthiness of an optional string.
fn truthy(value: Option<&str>) -> bool {
    value.is_some_and(|text| !text.is_empty())
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
fn worktree_branch(session: &SessionRecord) -> Option<String> {
    session
        .worktree
        .as_ref()
        .and_then(|worktree| worktree.branch.clone())
        .filter(|branch| !branch.is_empty())
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `{...stored.tooLarge ? {tooLarge: true} : {}}`.
fn only_true(value: Option<bool>) -> Option<bool> {
    value.filter(|&flag| flag)
}

fn now_iso() -> String {
    date::to_iso_string(date::now_ms()).unwrap_or_default()
}

fn note(kind: NoteKind, id: &str) -> NoteId {
    NoteId {
        kind,
        id: id.to_owned(),
    }
}

fn tasks_note(key: &str) -> NoteId {
    note(NoteKind::Tasks, key)
}

fn session_note(id: &str) -> NoteId {
    note(NoteKind::Session, id)
}

/// Where a note of that kind lives (a scratchpad is the store's `repo` kind).
fn store_kind(kind: WorkNoteKind) -> NoteKind {
    match kind {
        WorkNoteKind::Session => NoteKind::Session,
        WorkNoteKind::Line => NoteKind::Line,
        WorkNoteKind::Brief => NoteKind::Brief,
        WorkNoteKind::Scratchpad => NoteKind::Repo,
    }
}

/// A list key as `tasks_key_for` makes them: sixteen hex digits.
fn is_list_key(key: &str) -> bool {
    key.len() == 16 && key.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// A session's linked-work op, by its session's ID.
fn session_link(s: &str) -> TaskLink {
    TaskLink::Session { s: s.to_owned() }
}

/// The `type` of an op, for the log.
fn op_type(op: &TaskOp) -> &'static str {
    match op {
        TaskOp::Add { .. } => "add",
        TaskOp::Edit { .. } => "edit",
        TaskOp::Retitle { .. } => "retitle",
        TaskOp::Toggle { .. } => "toggle",
        TaskOp::Tick { .. } => "tick",
        TaskOp::Reopen { .. } => "reopen",
        TaskOp::Remove { .. } => "remove",
        TaskOp::Move { .. } => "move",
        TaskOp::MoveTo { .. } => "move-to",
        TaskOp::Reorder { .. } => "reorder",
        TaskOp::Link { .. } => "link",
        TaskOp::Assign { .. } => "assign",
        TaskOp::SessionOn { .. } => "session-on",
        TaskOp::SessionOff { .. } => "session-off",
        TaskOp::TickLinked { .. } => "tick-linked",
        TaskOp::ReopenLinked { .. } => "reopen-linked",
        TaskOp::AbandonLinked { .. } => "abandon-linked",
    }
}

/// A new open line, `added=` today.
pub fn add_line(title: &str, id: &str, parent: Option<String>, link: Option<TaskLink>) -> TaskOp {
    TaskOp::Add {
        title: title.to_owned(),
        body: None,
        link,
        id: Some(id.to_owned()),
        from: None,
        parent,
        at: None,
    }
}

/// `{...session, <keys>: ...}`: the keys keep their place when the session has them, else follow in this order; their
/// values are new objects, so no order recorded for an earlier value applies to them.
fn replace_keys(session: &mut Ordered<SessionRecord>, keys: &[&str]) {
    for key in keys {
        session.touch(key);
    }
    let mut order = session.key_order().clone();
    if let KeyOrder::Object(entries) = &mut order {
        for (key, child) in entries.iter_mut() {
            if keys.contains(&key.as_str()) {
                *child = KeyOrder::Unrecorded;
            }
        }
    }
    session.set_key_order(order);
}

/// The session with its own note (`notes`, `notesFile`) from the store. Since line notes there is no shared note:
/// `sharedNotes` is never set.
pub fn project_notes(session: Ordered<SessionRecord>, notes: &NotesStore) -> Ordered<SessionRecord> {
    let mut session = session;
    let own_id = session_note(&session.id);
    let own = notes.get(&own_id);
    replace_keys(&mut session, &["notes", "notesFile"]);
    session.notes = Some(own.text);
    session.notes_file = Some(deckhand_protocol::data::NoteFile {
        path: path_text(&note_file_path(&own_id)),
        revision: own.revision,
        too_large: only_true(own.too_large),
    });
    session.shared_notes = None;
    session
}

impl Daemon {
    /// The notes store.
    pub fn notes(&self) -> NotesStore {
        // A daemon always has its store; without one (tests of other subsystems) an empty store stands in.
        self.notes_tasks
            .borrow()
            .notes
            .clone()
            .unwrap_or_else(|| NotesStore::new(|_, _| {}))
    }

    /// The notice `ping` carries, if any.
    pub fn notice(&self) -> Option<String> {
        self.notes_tasks.borrow().notice.clone()
    }

    /// tasksKeyFor on a request's `cwd`: `typeof cwd !== 'string' || !cwd` is `Invalid repository path`.
    pub async fn tasks_key_for_request(&self, cwd: &Loose<String>) -> Result<String> {
        match cwd.known() {
            Some(cwd) => self.tasks_key_for(cwd).await,
            None => Err(DhError::msg("Invalid repository path")),
        }
    }

    /// The list key of the repository `cwd` is in (`Invalid repository path`).
    pub async fn tasks_key_for(&self, cwd: &str) -> Result<String> {
        if cwd.is_empty() {
            return Err(DhError::msg("Invalid repository path"));
        }
        let resolved = path::resolve(cwd);
        let existing = self.notes_tasks.borrow().tasks_keys.get(&resolved).cloned();
        let key = match existing {
            Some(key) => key,
            None => {
                let daemon = self.downgrade();
                let target = resolved.clone();
                let task = spawn(async move {
                    let key = resolve_repo_context(&target)
                        .await
                        .map(|context| repo_note_id(&context.trust_root));
                    // A failed lookup is tried again next time (the checkout may be ready by then).
                    if key.is_err()
                        && let Some(daemon) = daemon.upgrade()
                    {
                        daemon.notes_tasks.borrow_mut().tasks_keys.remove(&target);
                    }
                    key
                });
                let shared = task.boxed_local().shared();
                self.notes_tasks
                    .borrow_mut()
                    .tasks_keys
                    .insert(resolved, shared.clone());
                shared
            }
        };
        key.await
    }

    /// List `key` as it is now.
    pub fn task_list(&self, key: &str) -> TaskList {
        TaskList::parse(&self.notes().get(&tasks_note(key)).text)
    }

    /// The list line `id` is in (the first whose file names it).
    pub fn key_of_line(&self, id: &str) -> Option<String> {
        let needle = format!("t={id}");
        self.notes()
            .entries(NoteKind::Tasks)
            .into_iter()
            .find(|(_, list)| list.text.contains(&needle) && TaskList::parse(&list.text).get(id).is_some())
            .map(|(key, _)| key)
    }

    /// The line session `s` is on, and the key of its list.
    pub fn session_line(&self, s: &str) -> Option<(String, deckhand_core::task_list::Line)> {
        let needle = format!("s={s}");
        self.notes()
            .entries(NoteKind::Tasks)
            .into_iter()
            .filter(|(_, list)| list.text.contains(&needle))
            .find_map(|(key, list)| {
                TaskList::parse(&list.text)
                    .line_of_session(s)
                    .cloned()
                    .map(|line| (key, line))
            })
    }

    /// A line's note or brief, a scratchpad or a session's note, as the store has it now.
    pub fn work_note(&self, kind: WorkNoteKind, id: &str) -> WorkNote {
        let note = note(store_kind(kind), id);
        let stored = self.notes().get(&note);
        let (text, from, at) = match kind {
            WorkNoteKind::Brief => {
                let brief = parse_brief(&stored.text);
                (brief.text, brief.from, brief.at)
            }
            _ => (stored.text, None, None),
        };
        WorkNote {
            kind,
            id: id.to_owned(),
            path: path_text(&note_file_path(&note)),
            text,
            revision: stored.revision,
            too_large: only_true(stored.too_large),
            from,
            at,
        }
    }

    /// The list `key` (without its notes: see [`Daemon::tasks_doc_with_notes`]).
    pub fn tasks_doc(&self, key: &str) -> TasksDoc {
        let id = tasks_note(key);
        let list = self.notes().get(&id);
        TasksDoc {
            key: key.to_owned(),
            path: path_text(&note_file_path(&id)),
            text: list.text,
            revision: list.revision,
            too_large: only_true(list.too_large),
            notes: None,
        }
    }

    /// The list `key` with every line note and brief that has text, and its scratchpad (`watch-tasks`).
    pub fn tasks_doc_with_notes(&self, key: &str) -> TasksDoc {
        let mut doc = self.tasks_doc(key);
        let list = TaskList::parse(&doc.text);
        let mut notes = Vec::new();
        let mut seen = HashSet::new();
        for line in list.lines() {
            if !seen.insert(line.id.clone()) {
                continue;
            }
            for kind in [WorkNoteKind::Line, WorkNoteKind::Brief] {
                let note = self.work_note(kind, &line.id);
                if !note.text.is_empty() || note.too_large == Some(true) || note.from.is_some() {
                    notes.push(note);
                }
            }
        }
        notes.push(self.work_note(WorkNoteKind::Scratchpad, key));
        doc.notes = Some(notes);
        doc
    }

    /// The clients watching list `key`.
    fn watchers(&self, key: &str) -> Vec<crate::server::ClientHandle> {
        self.server
            .borrow()
            .clients
            .values()
            .filter(|client| client.subscription.tasks_key.as_deref() == Some(key))
            .map(|client| client.handle.clone())
            .collect()
    }

    /// `tasks-updated` to the clients watching list `key`.
    pub fn broadcast_tasks(&self, key: &str) {
        let doc = self.tasks_doc(key);
        for client in self.watchers(key) {
            send_message(
                &client,
                &ServerMessage::TasksUpdated(TasksUpdated { tasks: doc.clone() }),
            );
        }
    }

    /// `work-note-updated` to the clients watching list `key`.
    pub fn broadcast_work_note(&self, key: &str, kind: WorkNoteKind, id: &str) {
        let note = self.work_note(kind, id);
        for client in self.watchers(key) {
            send_message(
                &client,
                &ServerMessage::WorkNoteUpdated(WorkNoteUpdated {
                    key: key.to_owned(),
                    note: note.clone(),
                }),
            );
        }
    }

    /// Applies `op` to list `key` as it is on disk now; answers how many lines changed.
    pub async fn change_tasks(&self, key: &str, op: TaskOp) -> Result<usize> {
        let outcome = self
            .notes()
            .modify(&tasks_note(key), move |text| {
                let applied = apply_task_op(text, &op, date::now_ms())?;
                Ok((applied.text, applied.changed))
            })
            .await?;
        if outcome.changed {
            self.broadcast_tasks(key);
        }
        Ok(outcome.result)
    }

    /// A linked-work op for session `s` (tick-linked, reopen-linked, abandon-linked, session-off) on every list that
    /// has it on a line; failures are logged.
    pub async fn change_session_lines(&self, s: &str, op: TaskOp) {
        let needle = format!("s={s}");
        let notes = self.notes();
        let keys: Vec<String> = notes
            .entries(NoteKind::Tasks)
            .into_iter()
            .filter(|(_, list)| list.text.contains(&needle))
            .map(|(key, _)| key)
            .collect();
        for key in keys {
            match self.change_tasks(&key, op.clone()).await {
                Ok(0) => {}
                Ok(changed) => {
                    self.log(&format!("{}: {changed} line(s) of session {s}", op_type(&op)))
                        .await;
                }
                Err(error) => {
                    self.log(&format!("updating list {key} ({}) failed: {error}", op_type(&op)))
                        .await;
                }
            }
        }
    }

    /// The line `session` is on, made when it has none (lines on demand): named after it, last under its parent
    /// session's line when that has one in the same list, else at the top level where `add` puts a line. Answers the
    /// list's key and the line's ID. Checked and made in one change of the file, so two callers never make two.
    pub async fn ensure_session_line(&self, session: &SessionRecord) -> Result<(String, String)> {
        if let Some((key, line)) = self.session_line(&session.id)
            && !line.id.starts_with('~')
        {
            return Ok((key, line.id));
        }
        let key = self.tasks_key_for(&session.repo_root).await?;
        let s = session.id.clone();
        let parent_s = session.parent_session_id.clone().filter(|parent| !parent.is_empty());
        let title = session_line_title(&session.title);
        let new_id = new_task_id();
        let now = date::now_ms();
        let outcome = self
            .notes()
            .modify(&tasks_note(&key), move |text| {
                let list = TaskList::parse(text);
                if let Some(line) = list.line_of_session(&s) {
                    if !line.id.starts_with('~') {
                        return Ok((text.to_owned(), (line.id.clone(), false)));
                    }
                    // Not written by Deckhand yet: any op gives every line its ID.
                    let op = TaskOp::SessionOn {
                        id: line.id.clone(),
                        s: s.clone(),
                    };
                    let applied = apply_task_op(text, &op, now)?;
                    let id = TaskList::parse(&applied.text)
                        .line_of_session(&s)
                        .map(|line| line.id.clone())
                        .unwrap_or_default();
                    return Ok((applied.text, (id, false)));
                }
                let parent = parent_s
                    .as_deref()
                    .and_then(|parent| list.line_of_session(parent))
                    .map(|line| line.id.clone());
                let op = add_line(&title, &new_id, parent, Some(session_link(&s)));
                let applied = apply_task_op(text, &op, now)?;
                Ok((applied.text, (new_id.clone(), true)))
            })
            .await?;
        if outcome.changed {
            self.broadcast_tasks(&key);
        }
        let (id, created) = outcome.result;
        if created {
            self.log(&format!("a line for session {} ({id})", session.id)).await;
        }
        Ok((key, id))
    }

    /// Appends `text` to line `id`'s note (in list `key`) as a paragraph of its own.
    pub async fn append_line_note(&self, key: &str, id: &str, text: &str) -> Result<()> {
        let addition = text.to_owned();
        let outcome = self
            .notes()
            .modify(&note(NoteKind::Line, id), move |current| {
                Ok((append_paragraph(current, &addition), ()))
            })
            .await?;
        if outcome.changed {
            self.broadcast_work_note(key, WorkNoteKind::Line, id);
        }
        Ok(())
    }

    /// Writes line `id`'s brief (in list `key`): `text`, written by `from` now.
    pub async fn write_brief(&self, key: &str, id: &str, text: &str, from: &str) -> Result<()> {
        let file = format_brief(&Brief {
            from: Some(from.to_owned()),
            at: Some(now_iso()),
            text: text.to_owned(),
        });
        let outcome = self
            .notes()
            .modify(&note(NoteKind::Brief, id), move |_| Ok((file, ())))
            .await?;
        if outcome.changed {
            self.broadcast_work_note(key, WorkNoteKind::Brief, id);
        }
        Ok(())
    }

    /// Line `id`'s note and brief go to the trash (its line was removed); `label`: its title.
    async fn retire_line_notes(&self, key: &str, id: &str, label: &str) {
        let notes = self.notes();
        for kind in [WorkNoteKind::Line, WorkNoteKind::Brief] {
            let had = !notes.get(&note(store_kind(kind), id)).text.is_empty();
            if let Err(error) = notes.remove(&note(store_kind(kind), id), Some(label)).await {
                self.log(&format!("removing the {kind} of line {id} failed: {error}"))
                    .await;
            } else if had {
                self.broadcast_work_note(key, kind, id);
            }
        }
    }

    /// Where a request's note is, checked: the store's note and the list it belongs to (none for a session's note).
    /// `That line is no longer in the list`, `Invalid note`.
    fn locate_note(&self, note_ref: &NoteRef) -> Result<(NoteId, Option<String>)> {
        let id = note_ref.id.as_str();
        let key = match note_ref.kind {
            WorkNoteKind::Session => {
                self.require_session(id)?;
                None
            }
            WorkNoteKind::Line | WorkNoteKind::Brief => Some(
                self.key_of_line(id)
                    .ok_or_else(|| DhError::msg("That line is no longer in the list"))?,
            ),
            WorkNoteKind::Scratchpad if is_list_key(id) => Some(id.to_owned()),
            WorkNoteKind::Scratchpad => return Err(DhError::msg("Invalid note")),
        };
        Ok((note(store_kind(note_ref.kind), id), key))
    }

    /// After a note changed: the sessions showing it hear about it (a session's note), or the clients watching its
    /// list do.
    fn note_written(&self, note_ref: &NoteRef, key: Option<&str>) {
        match key {
            Some(key) => self.broadcast_work_note(key, note_ref.kind, &note_ref.id),
            None => self.notes_changed(NoteKind::Session, &note_file_stem(&note_ref.id)),
        }
    }

    /// `save-note` (`Invalid note`, `That line is no longer in the list`): revision-checked. A brief keeps who wrote it
    /// and when (one written here first is `you`'s, now).
    pub async fn save_note(&self, message: &SaveNote) -> Result<NoteSaveResult> {
        let (Some(note_ref), Some(text), Some(revision)) = (
            message.note.known(),
            message.text.as_deref(),
            message.revision.as_deref(),
        ) else {
            return Err(DhError::msg("Invalid note"));
        };
        let (target, key) = self.locate_note(note_ref)?;
        let notes = self.notes();
        let text = match note_ref.kind {
            WorkNoteKind::Brief => {
                let current = parse_brief(&notes.get(&target).text);
                format_brief(&Brief {
                    from: current.from.or_else(|| Some("you".to_owned())),
                    at: current.at.or_else(|| Some(now_iso())),
                    text: text.to_owned(),
                })
            }
            _ => text.to_owned(),
        };
        let result = notes.save(&target, &text, revision).await?;
        if result.changed {
            self.note_written(note_ref, key.as_deref());
        }
        Ok(NoteSaveResult {
            saved: result.saved,
            note: self.work_note(note_ref.kind, &note_ref.id),
            session: match note_ref.kind {
                WorkNoteKind::Session => Some(self.require_session(&note_ref.id)?),
                _ => None,
            },
        })
    }

    /// `open-note`: the note's file (created when missing), for an editor.
    pub async fn handle_open_note(&self, request: &OpenNote) -> Result<String> {
        let note_ref = request.note.known().ok_or_else(|| DhError::msg("Invalid note"))?;
        let (target, _) = self.locate_note(note_ref)?;
        Ok(path_text(&self.notes().ensure_file(&target).await?))
    }

    /// `promote-note-item` (`Invalid note`, `Invalid line`, `These notes changed meanwhile; try again`, `Put the cursor
    /// on a checklist item (- [ ] …) to make it a line`, `That item is already ticked; only open items become lines`,
    /// `A brief's checkboxes stay in it`): the checkbox becomes a line, under the note's line (a line's note; a
    /// session's note: the session's line, made when it has none) or at the end of the backlog (the scratchpad), with
    /// what was indented under it as its note; it leaves the note.
    pub async fn promote_note_item(&self, message: &PromoteNoteItem) -> Result<PromoteNoteItemResult> {
        let note_ref = message.note.known().ok_or_else(|| DhError::msg("Invalid note"))?;
        if note_ref.kind == WorkNoteKind::Brief {
            return Err(DhError::msg("A brief's checkboxes stay in it"));
        }
        let (target, note_key) = self.locate_note(note_ref)?;
        // `!Number.isInteger(line) || line < 0` (a fraction is refused, not floored).
        let Some(line) = message.line.and_then(|line| usize::try_from(line).ok()) else {
            return Err(DhError::msg("Invalid line"));
        };
        let notes = self.notes();
        let stored = notes.get(&target);
        let Some(revision) = message
            .revision
            .as_deref()
            .filter(|revision| *revision == stored.revision)
        else {
            return Err(DhError::msg("These notes changed meanwhile; try again"));
        };
        let taken = take_checkbox(&stored.text, line)
            .ok_or_else(|| DhError::msg("Put the cursor on a checklist item (- [ ] …) to make it a line"))?;
        if taken.done {
            return Err(DhError::msg(
                "That item is already ticked; only open items become lines",
            ));
        }
        let (key, parent) = match note_ref.kind {
            WorkNoteKind::Session => {
                let session = self.require_session(&note_ref.id)?;
                let (key, line) = self.ensure_session_line(&session).await?;
                (key, Some(line))
            }
            WorkNoteKind::Line => (note_key.clone().unwrap_or_default(), Some(note_ref.id.clone())),
            _ => (note_key.clone().unwrap_or_default(), None),
        };
        let id = new_task_id();
        self.change_tasks(&key, add_line(&taken.title, &id, parent, None))
            .await?;
        let saved = notes.save(&target, &taken.text, revision).await;
        let saved = match saved {
            Ok(saved) if saved.saved => saved,
            other => {
                let _ = self.change_tasks(&key, TaskOp::Remove { id: id.clone() }).await;
                return match other {
                    Err(error) => Err(error),
                    Ok(_) => Err(DhError::msg("These notes changed meanwhile; try again")),
                };
            }
        };
        if saved.changed {
            self.note_written(note_ref, note_key.as_deref());
        }
        if !taken.note.is_empty() {
            self.append_line_note(&key, &id, &taken.note).await?;
        }
        Ok(PromoteNoteItemResult {
            tasks: self.tasks_doc(&key),
            line_id: id,
            note: self.work_note(note_ref.kind, &note_ref.id),
            session: match note_ref.kind {
                WorkNoteKind::Session => Some(self.require_session(&note_ref.id)?),
                _ => None,
            },
        })
    }

    /// Sessions working in worktree record `wt`.
    fn sessions_of_record(&self, wt: &str) -> Vec<Ordered<SessionRecord>> {
        self.all_sessions()
            .into_iter()
            .filter(|session| worktree_id(session) == Some(wt))
            .collect()
    }

    /// Open lines directly under the lines of the sessions in worktree `wt`: what a merge lifts out of them.
    pub fn open_steps_of_worktree(&self, wt: &str) -> usize {
        self.sessions_of_record(wt)
            .iter()
            .filter_map(|session| self.session_line(&session.id))
            .map(|(key, line)| {
                self.task_list(&key)
                    .children(&line.id)
                    .iter()
                    .filter(|step| !step.done)
                    .count()
            })
            .sum()
    }

    /// The lines of the sessions in the worktree follow its record: merged ticks them (`auto=merge`), lifting their
    /// open steps out beside them marked with the branch (unless the merge asked to keep them,
    /// [`NotesTasksState::keep_open_steps`]); a stacked worktree's lines sit under its parent's, so the roll-up ticks
    /// that when they were its last open lines. Unmerged reopens them; deleted unmerged unhooks them, marked `tried`.
    pub async fn sync_worktree_tasks(
        &self,
        before: Option<&WorktreeRecord>,
        record: &WorktreeRecord,
        branch: Option<&str>,
    ) {
        let sessions = self.sessions_of_record(&record.id);
        let was_merged = before.is_some_and(|before| truthy(before.merged_at.as_deref()));
        let merged = truthy(record.merged_at.as_deref());
        let from = branch.filter(|branch| !branch.is_empty()).map(str::to_owned);
        if !was_merged && merged {
            let keep = self.notes_tasks.borrow_mut().keep_open_steps.remove(&record.id);
            for session in &sessions {
                let op = TaskOp::TickLinked {
                    link: session_link(&session.id),
                    auto: TaskAuto::Merge,
                    lift: (!keep).then_some(true),
                    from: from.clone(),
                };
                self.change_session_lines(&session.id, op).await;
            }
        } else if was_merged && !merged {
            for session in &sessions {
                let op = TaskOp::ReopenLinked {
                    link: session_link(&session.id),
                    auto: TaskAuto::Merge,
                };
                self.change_session_lines(&session.id, op).await;
            }
        }
        // Idempotent (only open lines someone is on change), so a record saved again changes nothing.
        if truthy(record.deleted_at.as_deref()) && !merged {
            for session in &sessions {
                let op = TaskOp::AbandonLinked {
                    link: session_link(&session.id),
                    tried: from.clone(),
                };
                self.change_session_lines(&session.id, op).await;
            }
        }
    }

    /// Done (`space`) ticks the session's line (`auto=done`); undone reopens it.
    pub async fn sync_done_tasks(&self, session: &SessionRecord, done: bool) {
        let link = session_link(&session.id);
        let op = if done {
            TaskOp::TickLinked {
                link,
                auto: TaskAuto::Done,
                lift: None,
                from: None,
            }
        } else {
            TaskOp::ReopenLinked {
                link,
                auto: TaskAuto::Done,
            }
        };
        self.change_session_lines(&session.id, op).await;
    }

    /// A session leaves the list for good (it is removed): its line, when open, is unhooked and marked `tried` (the
    /// branch, unless its worktree was merged); a ticked one just loses it. The line stays where it is.
    pub async fn release_session_line(&self, session: &SessionRecord) {
        let merged = session
            .worktree
            .as_ref()
            .is_some_and(|worktree| truthy(worktree.merged_at.as_deref()));
        let tried = if merged { None } else { worktree_branch(session) };
        self.change_session_lines(
            &session.id,
            TaskOp::AbandonLinked {
                link: session_link(&session.id),
                tried,
            },
        )
        .await;
        self.change_session_lines(&session.id, TaskOp::SessionOff { s: session.id.clone() })
            .await;
    }

    /// Before removing a session (`remove`'s `moveNote`; `kill the session before removing it`): what is still open
    /// in its note goes to the end of its line's note, the line made for it when it has none.
    pub async fn move_session_note_to_line(&self, session_id: &str) -> Result<()> {
        let session = self.require_session(session_id)?;
        if self.launch.borrow().workers.contains_key(session_id) || session.status == SessionStatus::Running {
            return Err(DhError::msg("kill the session before removing it"));
        }
        let open = open_note_text(&self.notes().get(&session_note(session_id)).text);
        if open.is_empty() {
            return Ok(());
        }
        let (key, line) = self.ensure_session_line(&session).await?;
        let title = self.session_path(&session);
        self.append_line_note(&key, &line, &format!("From the note of session {title}:\n\n{open}"))
            .await?;
        self.log(&format!("moved the open note of {title} into its line's note ({line})"))
            .await;
        Ok(())
    }

    /// What a handoff says about the line the session is on: its title, steps, brief and note.
    pub fn handoff_task(&self, session: &SessionRecord) -> Option<HandoffTask> {
        let (key, line) = self.session_line(&session.id)?;
        let steps = self
            .task_list(&key)
            .children(&line.id)
            .iter()
            .map(|step| HandoffStep {
                title: step.title.clone(),
                done: step.done,
            })
            .collect();
        let brief = self.work_note(WorkNoteKind::Brief, &line.id);
        Some(HandoffTask {
            title: line.title.clone(),
            body: self.work_note(WorkNoteKind::Line, &line.id).text,
            done: line.done,
            steps,
            brief: (!brief.text.trim().is_empty()).then_some(HandoffBrief {
                text: brief.text,
                from: brief.from,
                at: brief.at,
            }),
        })
    }

    /// A note's file changed (a save, or an editor): the sessions showing a session's note are re-projected and
    /// broadcast; a list goes out as `tasks-updated`; a line's note or brief, or a scratchpad, as `work-note-updated`.
    pub fn notes_changed(&self, kind: NoteKind, stem: &str) {
        match kind {
            NoteKind::Tasks => self.broadcast_tasks(stem),
            NoteKind::Session => {
                // Notes are not in state.json, so nothing is persisted.
                for session in self.all_sessions() {
                    if note_file_stem(&session.id) == stem {
                        self.set_session(session.clone());
                        self.broadcast_session_updated(&session);
                    }
                }
            }
            NoteKind::Line | NoteKind::Brief => {
                let kind = if kind == NoteKind::Line {
                    WorkNoteKind::Line
                } else {
                    WorkNoteKind::Brief
                };
                if let Some(key) = self.key_of_line(stem) {
                    self.broadcast_work_note(&key, kind, stem);
                }
            }
            NoteKind::Repo => self.broadcast_work_note(stem, WorkNoteKind::Scratchpad, stem),
            // The old model's: read only by the work-items migration.
            NoteKind::Worktree => {}
        }
    }

    /// `watch-tasks`: the client now shows that repository's list; answered with its notes.
    pub async fn handle_watch_tasks(&self, conn: &Connection, request: &CwdRequest) -> Result<TasksDoc> {
        let key = self.tasks_key_for_request(&request.cwd).await?;
        self.get_client(&conn.client, |client| client.tasks_key = Some(key.clone()));
        Ok(self.tasks_doc_with_notes(&key))
    }

    /// `task-op`: the repository's list first, then the op as the client sent it (`Invalid repository path` before
    /// `Unknown task change`).
    pub async fn handle_task_op(&self, request: &TaskOpRequest) -> Result<TasksDoc> {
        let key = self.tasks_key_for_request(&request.cwd).await?;
        self.apply_client_task_op(&key, &request.op).await
    }

    /// `task-op`'s body on the list of `cwd`'s repository.
    pub async fn handle_client_task_op(&self, cwd: &str, raw: &serde_json::Value) -> Result<TasksDoc> {
        let key = self.tasks_key_for(cwd).await?;
        self.apply_client_task_op(&key, raw).await
    }

    /// Session `s` of this repository (`That session is gone`, `That session belongs to another repository`).
    async fn session_of_list(&self, key: &str, s: &str) -> Result<Ordered<SessionRecord>> {
        let session = self
            .get_session(s)
            .ok_or_else(|| DhError::msg("That session is gone"))?;
        if self.tasks_key_for(&session.repo_root).await.ok().as_deref() != Some(key) {
            return Err(DhError::msg("That session belongs to another repository"));
        }
        Ok(session)
    }

    /// A client's op on list `key`: `{type: "session-line", s}` makes session `s`'s line when it has none (lines on
    /// demand: before a client writes a step or a note for it); anything else is checked by `client_task_op`. An
    /// add's `note` (text) becomes the new line's note; a `session-on`'s session must be of this repository; a
    /// removed line's note and brief go to the trash with it.
    async fn apply_client_task_op(&self, key: &str, raw: &serde_json::Value) -> Result<TasksDoc> {
        let field = |name: &str| raw.get(name).and_then(serde_json::Value::as_str);
        if field("type") == Some("session-line") {
            let s = field("s")
                .filter(|s| !s.is_empty())
                .ok_or_else(|| DhError::msg("Invalid task change: s"))?;
            let session = self.session_of_list(key, s).await?;
            self.ensure_session_line(&session).await?;
            return Ok(self.tasks_doc(key));
        }
        let mut op = client_task_op(raw)?;
        let mut added = None;
        let mut removed: Vec<(String, String)> = Vec::new();
        match &mut op {
            TaskOp::Add { id, .. } => {
                let new_id = new_task_id();
                *id = Some(new_id.clone());
                added = Some(new_id);
            }
            TaskOp::SessionOn { s, .. } => {
                self.session_of_list(key, s).await?;
            }
            TaskOp::Remove { id } => {
                removed = self
                    .task_list(key)
                    .subtree(id)
                    .iter()
                    .map(|line| (line.id.clone(), line.title.clone()))
                    .collect();
            }
            _ => {}
        }
        self.change_tasks(key, op).await?;
        for (id, title) in &removed {
            self.retire_line_notes(key, id, title).await;
        }
        if let (Some(id), Some(note)) = (added, field("note").filter(|note| !note.trim().is_empty())) {
            self.append_line_note(key, &id, note).await?;
        }
        Ok(self.tasks_doc(key))
    }

    /// `open-tasks`: the list's file (created when missing).
    pub async fn handle_open_tasks(&self, request: &CwdRequest) -> Result<String> {
        let key = self.tasks_key_for_request(&request.cwd).await?;
        Ok(path_text(&self.notes().ensure_file(&tasks_note(&key)).await?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::{local, session, test_client, test_daemon};
    use deckhand_core::task_list::Line;
    use serde_json::{Value, json};
    use std::sync::MutexGuard;

    /// DECKHAND_HOME = `dir` while this lives.
    struct Home {
        _guard: MutexGuard<'static, ()>,
        previous: Option<std::ffi::OsString>,
    }

    impl Home {
        fn set(dir: &Path) -> Home {
            let guard = crate::server::tests::env_lock();
            let previous = std::env::var_os(deckhand_sys::paths::HOME_ENV);
            // SAFETY: tests touching the environment hold server::tests::env_lock.
            unsafe { std::env::set_var(deckhand_sys::paths::HOME_ENV, dir) };
            Home {
                _guard: guard,
                previous,
            }
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            // SAFETY: as in Home::set (the environment lock is still held).
            unsafe {
                match self.previous.take() {
                    Some(value) => std::env::set_var(deckhand_sys::paths::HOME_ENV, value),
                    None => std::env::remove_var(deckhand_sys::paths::HOME_ENV),
                }
            }
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    /// A repository with one commit on main; its real path.
    fn repo(dir: &Path) -> String {
        std::fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-q", "-b", "main"]);
        git(dir, &["config", "commit.gpgsign", "false"]);
        std::fs::write(dir.join("README.md"), "hi\n").unwrap();
        git(dir, &["add", "README.md"]);
        git(dir, &["commit", "-q", "-m", "init"]);
        std::fs::canonicalize(dir).unwrap().to_string_lossy().into_owned()
    }

    /// A daemon with its files in `dir` and its notes store loaded.
    async fn daemon(dir: &Path) -> Daemon {
        let daemon = test_daemon(dir);
        daemon.notes().load().await.unwrap();
        daemon
    }

    /// A session record: the basics and `extra`.
    fn record(id: &str, root: &str, extra: Value) -> Ordered<SessionRecord> {
        let mut value = serde_json::to_value(session(id, root, root)).unwrap();
        if let (Value::Object(map), Value::Object(extra)) = (&mut value, extra) {
            map.extend(extra);
        }
        serde_json::from_value(value).unwrap()
    }

    fn worktree_record(id: &str, path: &str, extra: Value) -> Ordered<WorktreeRecord> {
        let mut value = json!({"id": id, "path": path, "createdAt": "2026-01-01T00:00:00.000Z"});
        if let (Value::Object(map), Value::Object(extra)) = (&mut value, extra) {
            map.extend(extra);
        }
        serde_json::from_value(value).unwrap()
    }

    /// An exited session in worktree record `wt` (branch `b-<wt>`), with `extra`.
    fn in_worktree(id: &str, root: &str, wt: &str, extra: Value) -> Ordered<SessionRecord> {
        let mut fields = json!({"status": "exited", "worktree": {"mode": "managed", "origin": "created",
            "path": format!("{root}-{wt}"), "branch": format!("b-{wt}"), "id": wt}});
        if let (Value::Object(map), Value::Object(extra)) = (&mut fields, extra) {
            map.extend(extra);
        }
        record(id, root, fields)
    }

    fn op(cwd: &str, op: Value) -> TaskOpRequest {
        TaskOpRequest {
            request_id: "r".to_owned(),
            cwd: cwd.into(),
            op,
        }
    }

    fn note_ref(kind: WorkNoteKind, id: &str) -> Loose<NoteRef> {
        Loose::Known(NoteRef {
            kind,
            id: id.to_owned(),
        })
    }

    fn save(kind: WorkNoteKind, id: &str, text: &str, revision: &str) -> SaveNote {
        SaveNote {
            request_id: "r".to_owned(),
            note: note_ref(kind, id),
            text: Some(text.to_owned()),
            revision: Some(revision.to_owned()),
        }
    }

    fn promote(kind: WorkNoteKind, id: &str, line: i64, revision: &str) -> PromoteNoteItem {
        PromoteNoteItem {
            request_id: "r".to_owned(),
            note: note_ref(kind, id),
            line: Some(line),
            revision: Some(revision.to_owned()),
        }
    }

    fn line(daemon: &Daemon, key: &str, title: &str) -> Line {
        daemon
            .task_list(key)
            .lines()
            .iter()
            .find(|line| line.title == title)
            .cloned()
            .unwrap_or_else(|| panic!("no line {title} in\n{}", daemon.tasks_doc(key).text))
    }

    fn titles_under(daemon: &Daemon, key: &str, id: &str) -> Vec<String> {
        daemon
            .task_list(key)
            .children(id)
            .iter()
            .map(|line| line.title.clone())
            .collect()
    }

    fn received(rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>) -> Vec<Value> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .map(|line| serde_json::from_str(&line).unwrap())
            .collect()
    }

    fn empty() -> String {
        deckhand_core::notes::note_revision("")
    }

    /// A session's record carries its own note and nothing shared: the worktree and main-checkout notes became line
    /// notes and the scratchpad.
    #[test]
    fn a_sessions_projection_carries_only_its_own_note() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        local(async {
            let daemon = daemon(dir.path()).await;
            let notes = daemon.notes();
            notes
                .save(&session_note("s1"), "remember the tests", &empty())
                .await
                .unwrap();
            let projected = project_notes(in_worktree("s1", "/repo", "w1", json!({})), &notes);
            assert_eq!(projected.notes.as_deref(), Some("remember the tests"));
            let file = projected.notes_file.clone().unwrap();
            assert!(file.path.ends_with("notes/sessions/s1.md"), "{}", file.path);
            assert_eq!(file.revision, deckhand_core::notes::note_revision("remember the tests"));
            assert_eq!(projected.shared_notes, None);
            let value = serde_json::to_value(&projected).unwrap();
            assert!(value.get("sharedNotes").is_none());
        });
    }

    /// `watch-tasks` answers the list with its line notes, briefs and scratchpad; afterwards a note's change reaches
    /// the watchers as that one note (`work-note-updated`), and a list's change as `tasks-updated` without notes.
    #[test]
    fn watching_a_list_gets_its_notes_once_then_each_change() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        local(async {
            let daemon = daemon(dir.path()).await;
            let doc = daemon
                .handle_task_op(&op(
                    &root,
                    json!({"type": "add", "title": "Parked", "note": "Why it waits"}),
                ))
                .await
                .unwrap();
            assert!(doc.notes.is_none(), "a change's answer leaves the notes out");
            let key = doc.key.clone();
            let parked = line(&daemon, &key, "Parked");
            daemon
                .write_brief(&key, &parked.id, "What it is for", "you")
                .await
                .unwrap();
            let (client, mut rx) = test_client(1);
            let conn = Connection::new(client);
            let watched = daemon
                .handle_watch_tasks(
                    &conn,
                    &CwdRequest {
                        request_id: "r".into(),
                        cwd: root.clone().into(),
                    },
                )
                .await
                .unwrap();
            let notes = watched.notes.unwrap();
            let summary: Vec<(WorkNoteKind, String, String)> = notes
                .iter()
                .map(|note| (note.kind, note.id.clone(), note.text.clone()))
                .collect();
            assert_eq!(
                summary,
                [
                    (WorkNoteKind::Line, parked.id.clone(), "Why it waits\n".to_owned()),
                    (WorkNoteKind::Brief, parked.id.clone(), "What it is for\n".to_owned()),
                    (WorkNoteKind::Scratchpad, key.clone(), String::new()),
                ]
            );
            assert_eq!(notes[1].from.as_deref(), Some("you"));
            assert!(notes[2].path.ends_with(&format!("notes/repos/{key}.md")));
            // One note changes: only it goes out.
            let saved = daemon
                .save_note(&save(
                    WorkNoteKind::Line,
                    &parked.id,
                    "Why it waits, really",
                    &notes[0].revision,
                ))
                .await
                .unwrap();
            assert!(saved.saved && saved.session.is_none());
            assert_eq!(saved.note.text, "Why it waits, really");
            let pushed = received(&mut rx);
            assert_eq!(pushed.len(), 1, "{pushed:?}");
            assert_eq!(pushed[0]["type"], "work-note-updated");
            assert_eq!(pushed[0]["key"], key.as_str());
            assert_eq!(pushed[0]["note"]["kind"], "line");
            assert_eq!(pushed[0]["note"]["text"], "Why it waits, really");
            // The scratchpad is the repository's, by its list key.
            let scratch = daemon
                .save_note(&save(
                    WorkNoteKind::Scratchpad,
                    &key,
                    "- [ ] someday",
                    &notes[2].revision,
                ))
                .await
                .unwrap();
            assert!(scratch.saved);
            let pushed = received(&mut rx);
            assert_eq!(pushed[0]["note"]["kind"], "scratchpad");
            daemon
                .handle_task_op(&op(&root, json!({"type": "tick", "id": parked.id})))
                .await
                .unwrap();
            let pushed = received(&mut rx);
            assert_eq!(pushed.len(), 1);
            assert_eq!(pushed[0]["type"], "tasks-updated");
            assert!(pushed[0]["tasks"].get("notes").is_none());
        });
    }

    /// The ops a client sends: lines added under lines with a note, retitled, ticked with the roll-up, moved and
    /// reordered; a removed line's note and brief go to the trash with it; a session goes on a line only of its own
    /// repository; the old model's ops are refused.
    #[test]
    fn client_ops_change_the_tree_and_its_notes_follow() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        local(async {
            let daemon = daemon(dir.path()).await;
            let send = |value: Value| {
                let (daemon, root) = (&daemon, root.clone());
                async move { daemon.handle_client_task_op(&root, &value).await }
            };
            let doc = send(json!({"type": "add", "title": "Rework auth"})).await.unwrap();
            let key = doc.key.clone();
            let idea = line(&daemon, &key, "Rework auth");
            send(json!({"type": "add", "title": "Token storage", "parent": idea.id}))
                .await
                .unwrap();
            send(json!({"type": "add", "title": "Login UI", "parent": idea.id, "at": 0, "note": "Use the kit"}))
                .await
                .unwrap();
            assert_eq!(titles_under(&daemon, &key, &idea.id), ["Login UI", "Token storage"]);
            let login = line(&daemon, &key, "Login UI");
            assert_eq!(daemon.work_note(WorkNoteKind::Line, &login.id).text, "Use the kit\n");
            send(json!({"type": "retitle", "id": login.id, "title": "Login screen"}))
                .await
                .unwrap();
            send(json!({"type": "reorder", "id": login.id, "down": true}))
                .await
                .unwrap();
            assert_eq!(titles_under(&daemon, &key, &idea.id), ["Token storage", "Login screen"]);
            // The last open line under a line ticks it (`auto=lines`); reopening one reopens it.
            let tokens = line(&daemon, &key, "Token storage");
            send(json!({"type": "tick", "id": tokens.id})).await.unwrap();
            assert!(!line(&daemon, &key, "Rework auth").done);
            send(json!({"type": "tick", "id": login.id})).await.unwrap();
            let rolled = line(&daemon, &key, "Rework auth");
            assert!(rolled.done && rolled.meta.auto == Some(TaskAuto::Lines));
            send(json!({"type": "reopen", "id": login.id})).await.unwrap();
            assert!(!line(&daemon, &key, "Rework auth").done);
            // Moved to the top level, then removed: its note goes to the trash.
            send(json!({"type": "move-to", "id": login.id, "parent": null}))
                .await
                .unwrap();
            assert_eq!(line(&daemon, &key, "Login screen").parent, None);
            send(json!({"type": "remove", "id": login.id})).await.unwrap();
            assert!(daemon.task_list(&key).get(&login.id).is_none());
            assert_eq!(daemon.work_note(WorkNoteKind::Line, &login.id).text, "");
            let trash: Vec<String> = std::fs::read_dir(dir.path().join("notes").join("trash"))
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            assert!(
                trash.iter().any(|name| name.contains("_line_login-screen_")),
                "{trash:?}"
            );
            // A session of this repository goes on a line; another repository's, or none, is refused.
            daemon.set_session(record("here", &root, json!({})));
            let elsewhere = dir.path().join("elsewhere");
            std::fs::create_dir_all(&elsewhere).unwrap();
            daemon.set_session(record("there", elsewhere.to_str().unwrap(), json!({})));
            let error = send(json!({"type": "session-on", "id": tokens.id, "s": "there"}))
                .await
                .unwrap_err();
            assert_eq!(error.to_string(), "That session belongs to another repository");
            let error = send(json!({"type": "session-on", "id": tokens.id, "s": "nobody"}))
                .await
                .unwrap_err();
            assert_eq!(error.to_string(), "That session is gone");
            send(json!({"type": "reopen", "id": tokens.id})).await.unwrap();
            send(json!({"type": "session-on", "id": tokens.id, "s": "here"}))
                .await
                .unwrap();
            assert_eq!(line(&daemon, &key, "Token storage").meta.s.as_deref(), Some("here"));
            assert_eq!(
                daemon.session_line("here").map(|(_, line)| line.id),
                Some(tokens.id.clone())
            );
            send(json!({"type": "session-off", "s": "here"})).await.unwrap();
            assert!(daemon.session_line("here").is_none());
            for refused in [
                json!({"type": "assign", "id": tokens.id, "link": {"s": "here"}}),
                json!({"type": "edit", "id": tokens.id, "title": "x", "body": "y"}),
                json!({"type": "link", "id": tokens.id, "link": {"wt": "w1"}}),
            ] {
                assert_eq!(send(refused).await.unwrap_err().to_string(), "Unknown task change");
            }
            // Nothing written carries a worktree link.
            assert!(!daemon.tasks_doc(&key).text.contains("wt="));
        });
    }

    /// A session has no line until something needs one; then it gets one, named after it, under its parent session's
    /// line (else at the top level), once.
    #[test]
    fn a_session_gets_a_line_only_when_something_needs_one() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        local(async {
            let daemon = daemon(dir.path()).await;
            daemon.set_session(record("parent", &root, json!({"title": "Token storage"})));
            daemon.set_session(record(
                "child",
                &root,
                json!({"title": "Refresh tokens", "parentSessionId": "parent"}),
            ));
            daemon.set_session(record("quick", &root, json!({"title": "Quick look"})));
            let key = daemon.tasks_key_for(&root).await.unwrap();
            assert!(daemon.session_line("quick").is_none());
            assert!(
                daemon.task_list(&key).lines().is_empty(),
                "no line for sessions with nothing on them"
            );
            let send = |value: Value| {
                let (daemon, root) = (&daemon, root.clone());
                async move { daemon.handle_client_task_op(&root, &value).await }
            };
            send(json!({"type": "session-line", "s": "parent"})).await.unwrap();
            send(json!({"type": "session-line", "s": "child"})).await.unwrap();
            send(json!({"type": "session-line", "s": "child"})).await.unwrap();
            let parent = line(&daemon, &key, "Token storage");
            let child = line(&daemon, &key, "Refresh tokens");
            assert_eq!(parent.meta.s.as_deref(), Some("parent"));
            assert_eq!(parent.parent, None);
            assert_eq!(child.meta.s.as_deref(), Some("child"));
            assert_eq!(child.parent.as_deref(), Some(parent.id.as_str()));
            assert_eq!(daemon.task_list(&key).lines().len(), 2, "made once");
            // A sub-session whose parent has no line goes to the top level.
            daemon.set_session(record(
                "orphan",
                &root,
                json!({"title": "Side", "parentSessionId": "quick"}),
            ));
            send(json!({"type": "session-line", "s": "orphan"})).await.unwrap();
            assert_eq!(line(&daemon, &key, "Side").parent, None);
            let error = send(json!({"type": "session-line"})).await.unwrap_err();
            assert_eq!(error.to_string(), "Invalid task change: s");
        });
    }

    /// Saves are revision-checked for every kind of note; a line's note needs its line; a brief keeps who wrote it and
    /// when.
    #[test]
    fn saves_are_revision_checked_and_a_brief_keeps_its_author() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        local(async {
            let daemon = daemon(dir.path()).await;
            daemon.set_session(record("s1", &root, json!({})));
            let saved = daemon
                .save_note(&save(WorkNoteKind::Session, "s1", "mine", &empty()))
                .await
                .unwrap();
            assert!(saved.saved);
            assert_eq!(saved.session.unwrap().notes.as_deref(), Some("mine"));
            let stale = daemon
                .save_note(&save(WorkNoteKind::Session, "s1", "theirs", &empty()))
                .await
                .unwrap();
            assert!(!stale.saved);
            assert_eq!(stale.note.text, "mine");
            let gone = daemon
                .save_note(&save(WorkNoteKind::Line, "deadbeef", "x", &empty()))
                .await
                .unwrap_err();
            assert_eq!(gone.to_string(), "That line is no longer in the list");
            let invalid = daemon
                .save_note(&save(WorkNoteKind::Scratchpad, "../x", "x", &empty()))
                .await
                .unwrap_err();
            assert_eq!(invalid.to_string(), "Invalid note");
            let bad = SaveNote {
                note: Loose::Other(json!({"kind": "worktree", "id": "w1"})),
                ..save(WorkNoteKind::Session, "s1", "x", &empty())
            };
            assert_eq!(daemon.save_note(&bad).await.unwrap_err().to_string(), "Invalid note");
            // A brief written by an orchestrator keeps its author through an edit by hand.
            let doc = daemon
                .handle_task_op(&op(&root, json!({"type": "add", "title": "Token storage"})))
                .await
                .unwrap();
            let id = line(&daemon, &doc.key, "Token storage").id;
            daemon
                .write_brief(&doc.key, &id, "Store them encrypted", "◎ Orchestrator")
                .await
                .unwrap();
            let brief = daemon.work_note(WorkNoteKind::Brief, &id);
            let edited = daemon
                .save_note(&save(
                    WorkNoteKind::Brief,
                    &id,
                    "Store them encrypted, at rest",
                    &brief.revision,
                ))
                .await
                .unwrap();
            assert!(edited.saved);
            assert_eq!(edited.note.text, "Store them encrypted, at rest\n");
            assert_eq!(edited.note.from.as_deref(), Some("◎ Orchestrator"));
            assert_eq!(edited.note.at, brief.at);
            let file = std::fs::read_to_string(dir.path().join("notes/briefs").join(format!("{id}.md"))).unwrap();
            assert!(file.starts_with("<!-- dh:brief at="), "{file}");
            // One written by hand here is yours.
            let doc = daemon
                .handle_task_op(&op(&root, json!({"type": "add", "title": "Login UI"})))
                .await
                .unwrap();
            let id = line(&daemon, &doc.key, "Login UI").id;
            let mine = daemon
                .save_note(&save(WorkNoteKind::Brief, &id, "Use the kit", &empty()))
                .await
                .unwrap();
            assert_eq!(mine.note.from.as_deref(), Some("you"));
        });
    }

    /// A note's checkbox becomes a line (`ctrl+p`): under the note's line, under a session's line (made for it), or in
    /// the backlog from the scratchpad; it leaves the note, and what was indented under it is the new line's note.
    #[test]
    fn a_promoted_checkbox_becomes_a_line_where_its_note_belongs() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        local(async {
            let daemon = daemon(dir.path()).await;
            daemon.set_session(record("s1", &root, json!({"title": "Fix login"})));
            let key = daemon.tasks_key_for(&root).await.unwrap();
            let note = "Thinking\n- [ ] Retry setup\n  when trust changes\n- [x] Done already\n";
            let saved = daemon
                .save_note(&save(WorkNoteKind::Session, "s1", note, &empty()))
                .await
                .unwrap();
            let promoted = daemon
                .promote_note_item(&promote(WorkNoteKind::Session, "s1", 1, &saved.note.revision))
                .await
                .unwrap();
            let session_line = line(&daemon, &key, "Fix login");
            assert_eq!(session_line.meta.s.as_deref(), Some("s1"), "the session got its line");
            let retry = line(&daemon, &key, "Retry setup");
            assert_eq!(promoted.line_id, retry.id);
            assert_eq!(retry.parent.as_deref(), Some(session_line.id.as_str()));
            assert_eq!(
                daemon.work_note(WorkNoteKind::Line, &retry.id).text,
                "when trust changes\n"
            );
            assert_eq!(promoted.note.text, "Thinking\n- [x] Done already\n");
            assert_eq!(
                promoted.session.unwrap().notes.as_deref(),
                Some("Thinking\n- [x] Done already\n")
            );
            let ticked = daemon
                .promote_note_item(&promote(WorkNoteKind::Session, "s1", 1, &promoted.note.revision))
                .await
                .unwrap_err();
            assert_eq!(
                ticked.to_string(),
                "That item is already ticked; only open items become lines"
            );
            let stale = daemon
                .promote_note_item(&promote(WorkNoteKind::Session, "s1", 1, &empty()))
                .await
                .unwrap_err();
            assert_eq!(stale.to_string(), "These notes changed meanwhile; try again");
            // From a line's note: under that line.
            let current = daemon.work_note(WorkNoteKind::Line, &retry.id).revision;
            let line_note = daemon
                .save_note(&save(
                    WorkNoteKind::Line,
                    &retry.id,
                    "- [ ] Log it\nwhen trust changes\n",
                    &current,
                ))
                .await
                .unwrap();
            assert!(line_note.saved);
            daemon
                .promote_note_item(&promote(WorkNoteKind::Line, &retry.id, 0, &line_note.note.revision))
                .await
                .unwrap();
            assert_eq!(titles_under(&daemon, &key, &retry.id), ["Log it"]);
            // From the scratchpad: the backlog.
            let scratch = daemon
                .save_note(&save(
                    WorkNoteKind::Scratchpad,
                    &key,
                    "- [ ] Someday: dark mode\n",
                    &empty(),
                ))
                .await
                .unwrap();
            daemon
                .promote_note_item(&promote(WorkNoteKind::Scratchpad, &key, 0, &scratch.note.revision))
                .await
                .unwrap();
            let someday = line(&daemon, &key, "Someday: dark mode");
            assert!(someday.parent.is_none() && !someday.is_taken());
            let brief = daemon
                .promote_note_item(&promote(WorkNoteKind::Brief, &retry.id, 0, &empty()))
                .await
                .unwrap_err();
            assert_eq!(brief.to_string(), "A brief's checkboxes stay in it");
        });
    }

    /// Merging a worktree ticks the line of every session in it and lifts their open steps out beside them, marked
    /// with the branch, unless the merge keeps them; a stacked worktree's line, under its parent's, rolls it up once
    /// it was the last open one; unmerging reopens them.
    #[test]
    fn merging_ticks_the_worktrees_session_lines_and_rolls_up() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        local(async {
            let daemon = daemon(dir.path()).await;
            for (id, path) in [("wp", "parent"), ("wc", "child"), ("wk", "kept")] {
                let record = worktree_record(id, &format!("{root}-{path}"), json!({}));
                daemon.worktrees.borrow_mut().records.insert(id.to_owned(), record);
            }
            daemon.set_session(in_worktree("parent", &root, "wp", json!({"title": "Token storage"})));
            daemon.set_session(in_worktree(
                "child",
                &root,
                "wc",
                json!({"title": "Refresh tokens", "parentSessionId": "parent"}),
            ));
            daemon.set_session(in_worktree("kept", &root, "wk", json!({"title": "Kept"})));
            let send = |value: Value| {
                let (daemon, root) = (&daemon, root.clone());
                async move { daemon.handle_client_task_op(&root, &value).await }
            };
            let key = daemon.tasks_key_for(&root).await.unwrap();
            for s in ["parent", "child", "kept"] {
                send(json!({"type": "session-line", "s": s})).await.unwrap();
            }
            let parent = line(&daemon, &key, "Token storage");
            let child = line(&daemon, &key, "Refresh tokens");
            send(json!({"type": "add", "title": "schema", "parent": parent.id}))
                .await
                .unwrap();
            send(json!({"type": "tick", "id": line(&daemon, &key, "schema").id}))
                .await
                .unwrap();
            send(json!({"type": "add", "title": "rotate keys", "parent": child.id}))
                .await
                .unwrap();
            assert_eq!(daemon.open_steps_of_worktree("wc"), 1);
            let merged = |id: &str| {
                let before = daemon.worktrees.borrow().records.get(id).cloned().unwrap();
                let mut after = before.clone();
                after.merged_at = Some("2026-11-14T12:00:00.000Z".to_owned());
                (before, after)
            };
            // The child merges into the parent's branch: its line ticks and its open step moves up beside it (under
            // the parent's line), so the parent's line still has an open line.
            let (before, after) = merged("wc");
            daemon.sync_worktree_tasks(Some(&before), &after, Some("b-wc")).await;
            let child_now = line(&daemon, &key, "Refresh tokens");
            assert!(child_now.done && child_now.meta.auto == Some(TaskAuto::Merge));
            let rotate = line(&daemon, &key, "rotate keys");
            assert_eq!(rotate.parent.as_deref(), Some(parent.id.as_str()));
            assert_eq!(rotate.meta.from.as_deref(), Some("b-wc"));
            assert!(!line(&daemon, &key, "Token storage").done);
            // Its last open line ticked: the parent's line rolls up.
            send(json!({"type": "tick", "id": rotate.id})).await.unwrap();
            let rolled = line(&daemon, &key, "Token storage");
            assert!(rolled.done && rolled.meta.auto == Some(TaskAuto::Lines));
            // Unmerged: the child's line reopens, and so does the line it rolled up.
            daemon.sync_worktree_tasks(Some(&after), &before, Some("b-wc")).await;
            assert!(!line(&daemon, &key, "Refresh tokens").done);
            assert!(!line(&daemon, &key, "Token storage").done);
            // A merge that keeps the open steps leaves them under the ticked line.
            let kept = line(&daemon, &key, "Kept");
            send(json!({"type": "add", "title": "later", "parent": kept.id}))
                .await
                .unwrap();
            daemon.notes_tasks.borrow_mut().keep_open_steps.insert("wk".into());
            let (before, after) = merged("wk");
            daemon.sync_worktree_tasks(Some(&before), &after, Some("b-wk")).await;
            assert!(line(&daemon, &key, "Kept").done);
            assert_eq!(titles_under(&daemon, &key, &kept.id), ["later"]);
            assert!(daemon.notes_tasks.borrow().keep_open_steps.is_empty());
        });
    }

    /// Done ticks the session's line (`auto=done`) and undone reopens it; a worktree deleted unmerged unhooks its
    /// sessions' open lines, marked `tried` in its branch, where they are.
    #[test]
    fn done_ticks_and_a_worktree_deleted_unmerged_unhooks_its_lines() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        local(async {
            let daemon = daemon(dir.path()).await;
            daemon
                .worktrees
                .borrow_mut()
                .records
                .insert("w1".into(), worktree_record("w1", &format!("{root}-w1"), json!({})));
            daemon.set_session(in_worktree("a", &root, "w1", json!({"title": "A"})));
            daemon.set_session(in_worktree("b", &root, "w1", json!({"title": "B"})));
            daemon.set_session(record("m", &root, json!({"title": "Main"})));
            let key = daemon.tasks_key_for(&root).await.unwrap();
            for s in ["a", "b", "m"] {
                daemon
                    .handle_task_op(&op(&root, json!({"type": "session-line", "s": s})))
                    .await
                    .unwrap();
            }
            let main = daemon.get_session("m").unwrap();
            daemon.sync_done_tasks(&main, true).await;
            let ticked = line(&daemon, &key, "Main");
            assert!(ticked.done && ticked.meta.auto == Some(TaskAuto::Done));
            daemon.sync_done_tasks(&main, false).await;
            assert!(!line(&daemon, &key, "Main").done);
            // Each session has its own line: done on one ticks only its own.
            daemon.sync_done_tasks(&daemon.get_session("a").unwrap(), true).await;
            assert!(line(&daemon, &key, "A").done && !line(&daemon, &key, "B").done);
            let before = daemon.worktrees.borrow().records.get("w1").cloned().unwrap();
            let mut deleted = before.clone();
            deleted.deleted_at = Some("2026-11-14T12:00:00.000Z".into());
            daemon.sync_worktree_tasks(Some(&before), &deleted, Some("b-w1")).await;
            let b = line(&daemon, &key, "B");
            assert!(!b.is_taken() && !b.done);
            assert_eq!(b.meta.tried.as_deref(), Some("b-w1"));
            // A ticked line is no dropped attempt: it keeps its session.
            assert_eq!(line(&daemon, &key, "A").meta.s.as_deref(), Some("a"));
        });
    }

    /// Removing a session unhooks its line (marked `tried` in its branch when open) and, when asked, first moves what
    /// is still open in its note into its line's note, making the line when it had none.
    #[test]
    fn a_removed_session_leaves_its_line_and_can_leave_its_note_there() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        local(async {
            let daemon = daemon(dir.path()).await;
            daemon
                .worktrees
                .borrow_mut()
                .records
                .insert("w1".into(), worktree_record("w1", &format!("{root}-w1"), json!({})));
            daemon.set_session(in_worktree("s1", &root, "w1", json!({"title": "Fix login"})));
            let key = daemon.tasks_key_for(&root).await.unwrap();
            daemon
                .save_note(&save(
                    WorkNoteKind::Session,
                    "s1",
                    "Review crypto.rs\n- [x] told it the scope\n- [ ] check the diff\n",
                    &empty(),
                ))
                .await
                .unwrap();
            daemon.move_session_note_to_line("s1").await.unwrap();
            let fix = line(&daemon, &key, "Fix login");
            assert_eq!(
                daemon.work_note(WorkNoteKind::Line, &fix.id).text,
                "From the note of session Fix login:\n\nReview crypto.rs\n- [ ] check the diff\n"
            );
            daemon.remove_session("s1").await.unwrap();
            let left = line(&daemon, &key, "Fix login");
            assert!(!left.is_taken());
            assert_eq!(left.meta.tried.as_deref(), Some("b-w1"));
            // A session with nothing open in its note gets no line.
            daemon.set_session(record("s2", &root, json!({"title": "Quick", "status": "exited"})));
            daemon.move_session_note_to_line("s2").await.unwrap();
            assert!(daemon.session_line("s2").is_none());
        });
    }

    /// A handoff carries the line the session is on: its title, steps, brief and note.
    #[test]
    fn a_handoff_names_the_line_its_steps_brief_and_note() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        local(async {
            let daemon = daemon(dir.path()).await;
            daemon.set_session(record("s1", &root, json!({"title": "Token storage"})));
            assert_eq!(daemon.handoff_task(&daemon.get_session("s1").unwrap()), None);
            let (key, id) = daemon
                .ensure_session_line(&daemon.get_session("s1").unwrap())
                .await
                .unwrap();
            daemon
                .handle_task_op(&op(&root, json!({"type": "add", "title": "schema", "parent": id})))
                .await
                .unwrap();
            daemon
                .write_brief(&key, &id, "Store them encrypted", "◎ Orchestrator")
                .await
                .unwrap();
            daemon.append_line_note(&key, &id, "Found the cache").await.unwrap();
            let task = daemon.handoff_task(&daemon.get_session("s1").unwrap()).unwrap();
            assert_eq!(task.title, "Token storage");
            assert_eq!(task.body, "Found the cache\n");
            assert_eq!(
                task.steps,
                [HandoffStep {
                    title: "schema".into(),
                    done: false
                }]
            );
            let brief = task.brief.unwrap();
            assert_eq!(brief.text, "Store them encrypted\n");
            assert_eq!(brief.from.as_deref(), Some("◎ Orchestrator"));
        });
    }

    /// An editor's change to a line's note reaches the clients watching its list.
    #[test]
    fn an_edit_outside_deckhand_reaches_the_lists_watchers() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        local(async {
            let daemon = daemon(dir.path()).await;
            let doc = daemon
                .handle_task_op(&op(&root, json!({"type": "add", "title": "Parked"})))
                .await
                .unwrap();
            let id = line(&daemon, &doc.key, "Parked").id;
            let (client, mut rx) = test_client(1);
            let conn = Connection::new(client);
            daemon
                .handle_watch_tasks(
                    &conn,
                    &CwdRequest {
                        request_id: "r".into(),
                        cwd: root.clone().into(),
                    },
                )
                .await
                .unwrap();
            let file = dir.path().join("notes/lines").join(format!("{id}.md"));
            std::fs::write(&file, "written in vim\n").unwrap();
            // What the watcher does once it has re-read the file.
            daemon
                .notes()
                .modify(&note(NoteKind::Line, &id), |text| Ok((text.to_owned(), ())))
                .await
                .unwrap();
            daemon.notes_changed(NoteKind::Line, &id);
            let pushed = received(&mut rx);
            assert_eq!(pushed.last().unwrap()["note"]["text"], "written in vim\n");
        });
    }
}
