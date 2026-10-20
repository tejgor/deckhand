//! Port of daemon.ts (notes and tasks).
//!
//! Notes live in files (notesStore.ts); the daemon projects each session's own note and the note its worktree (or main
//! checkout) shares into the session, and re-broadcasts sessions when a note changes, also outside Deckhand. Tasks:
//! one list per repository (tasks.ts), a file beside the notes, keyed by the repository's main checkout; open note
//! items can be sent to it and tasks returned to their note.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;

use deckhand_core::error::{DhError, Result};
use deckhand_core::jsstr::{self, date, path};
use deckhand_core::notes::{
    MAX_NOTES_CHARS, TitledNote, merge_into_worktree_note, repo_note_id, shared_note_identity, shows_own_note,
};
use deckhand_core::session_order::sort_sessions_for_sidebar;
use deckhand_core::tasks::{
    append_note_item, apply_task_op, client_task_op, find_note_task_link, is_linked, link_matches, link_of_key,
    migrate_follow_ups, new_task_id, open_note_items, parse_tasks, promote_note_line, promote_open_note_lines,
    return_task_to_note,
};
use deckhand_core::worktree_records::WorktreeRecords;
use deckhand_protocol::data::{
    NoteSaveResult, NoteSection, PromoteNoteItemResult, SendOpenItemsResult, SessionRecord, SessionStatus, SharedNote,
    SharedNoteKind, Task, TaskAuto, TaskLink, TaskOp, TasksDoc,
};
use deckhand_protocol::events::{ServerMessage, TasksUpdated};
use deckhand_protocol::records::WorktreeRecord;
use deckhand_protocol::wire::{CwdRequest, Loose, OpenNote, PromoteNoteItem, SaveNote, SendOpenItems, TaskOpRequest};
use deckhand_protocol::{KeyOrder, Ordered};
use deckhand_sys::async_util::{SharedTask, spawn};
use deckhand_sys::git::resolve_repo_context;
use deckhand_sys::notes_store::{NoteId, NoteKind, NotesStore, note_file_path, note_file_stem};
use futures::FutureExt;

use crate::daemon::Daemon;
use crate::server::{Connection, send_message};

/// The notes' and tasks' part of the daemon state.
#[derive(Default)]
pub struct NotesTasksState {
    /// TS `notes` (created by [`Daemon::new`], wired to notesChanged).
    pub notes: Option<NotesStore>,
    /// TS `tasksKeys`: the repository key of a checkout path, resolved once (a failed lookup is forgotten).
    pub tasks_keys: HashMap<String, SharedTask<Result<String>>>,
}

/// TS: InkDaemon.taskOrigin result: the note linking to a task, and whether it is read-only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskOrigin {
    pub note: NoteId,
    pub read_only: bool,
}

/// TS: InkDaemon.sendOpenNoteItems `options`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SendItemsOptions {
    /// The work the tasks are for; `None`: the backlog.
    pub link: Option<TaskLink>,
    /// The branch the items were left open in.
    pub from: Option<String>,
    /// Refuse when the note changed since this revision.
    pub revision: Option<String>,
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

fn shared_kind(kind: SharedNoteKind) -> NoteKind {
    match kind {
        SharedNoteKind::Worktree => NoteKind::Worktree,
        SharedNoteKind::Repo => NoteKind::Repo,
    }
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `{...stored.tooLarge ? {tooLarge: true} : {}}`.
fn only_true(value: Option<bool>) -> Option<bool> {
    value.filter(|&flag| flag)
}

/// The note a request's `section` names (TS noteOf: `section === 'session'` is the session's own, anything else its
/// shared one).
fn note_section(section: &Loose<NoteSection>) -> NoteSection {
    match section {
        Loose::Known(NoteSection::Session) => NoteSection::Session,
        _ => NoteSection::Shared,
    }
}

/// `message.section === 'shared'` (the shared note's checks apply only then).
fn is_shared_section(section: &Loose<NoteSection>) -> bool {
    section.known() == Some(&NoteSection::Shared)
}

fn session_note(id: &str) -> NoteId {
    NoteId {
        kind: NoteKind::Session,
        id: id.to_owned(),
    }
}

fn tasks_note(key: &str) -> NoteId {
    NoteId {
        kind: NoteKind::Tasks,
        id: key.to_owned(),
    }
}

/// The text a task list contains when it links that work (`wt=<id>` / `s=<id>`), a cheap pre-filter.
fn link_needle(link: &TaskLink) -> String {
    match link {
        TaskLink::Worktree { wt } => format!("wt={wt}"),
        TaskLink::Session { s } => format!("s={s}"),
    }
}

/// TS `op.type`.
fn op_type(op: &TaskOp) -> &'static str {
    match op {
        TaskOp::Add { .. } => "add",
        TaskOp::Edit { .. } => "edit",
        TaskOp::Toggle { .. } => "toggle",
        TaskOp::Remove { .. } => "remove",
        TaskOp::Move { .. } => "move",
        TaskOp::Link { .. } => "link",
        TaskOp::Assign { .. } => "assign",
        TaskOp::TickLinked { .. } => "tick-linked",
        TaskOp::ReopenLinked { .. } => "reopen-linked",
        TaskOp::AbandonLinked { .. } => "abandon-linked",
    }
}

/// `{type: 'add', title, id, ...link ? {link} : {}, ...from ? {from} : {}}`.
fn add_op(title: &str, id: Option<&str>, link: Option<TaskLink>, from: Option<&str>) -> TaskOp {
    TaskOp::Add {
        title: title.to_owned(),
        body: None,
        link,
        id: id.map(str::to_owned),
        from: from.filter(|from| !from.is_empty()).map(str::to_owned),
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

/// TS: daemon.ts projectNotes: the session with its own note (`notes`, `notesFile`) and its shared note
/// (`sharedNotes`, read-only for a deleted worktree) from the store.
pub fn project_notes(
    session: Ordered<SessionRecord>,
    worktrees: &WorktreeRecords,
    notes: &NotesStore,
) -> Ordered<SessionRecord> {
    let mut session = session;
    let own_id = session_note(&session.id);
    let own = notes.get(&own_id);
    let shared_notes = shared_note_identity(&session).map(|identity| {
        let note = NoteId {
            kind: shared_kind(identity.kind),
            id: identity.id.clone(),
        };
        let shared = notes.get(&note);
        let read_only = identity.kind == SharedNoteKind::Worktree
            && worktrees
                .get(&identity.id)
                .is_none_or(|record| truthy(record.deleted_at.as_deref()));
        SharedNote {
            kind: identity.kind,
            id: identity.id,
            path: path_text(&note_file_path(&note)),
            text: shared.text,
            revision: shared.revision,
            too_large: only_true(shared.too_large),
            read_only: read_only.then_some(true),
        }
    });
    replace_keys(&mut session, &["notes", "notesFile", "sharedNotes"]);
    session.notes = Some(own.text);
    session.notes_file = Some(deckhand_protocol::data::NoteFile {
        path: path_text(&note_file_path(&own_id)),
        revision: own.revision,
        too_large: only_true(own.too_large),
    });
    session.shared_notes = shared_notes;
    session
}

impl Daemon {
    /// TS: `this.notes`.
    pub fn notes(&self) -> NotesStore {
        // A daemon always has its store; without one (tests of other subsystems) an empty store stands in.
        self.notes_tasks
            .borrow()
            .notes
            .clone()
            .unwrap_or_else(|| NotesStore::new(|_, _| {}))
    }

    /// TS: InkDaemon.noteOf (`This session has no worktree note yet: its worktree is not ready`).
    pub fn note_of(&self, session: &SessionRecord, section: NoteSection) -> Result<NoteId> {
        if section == NoteSection::Session {
            return Ok(session_note(&session.id));
        }
        let shared = session
            .shared_notes
            .as_ref()
            .ok_or_else(|| DhError::msg("This session has no worktree note yet: its worktree is not ready"))?;
        Ok(NoteId {
            kind: shared_kind(shared.kind),
            id: shared.id.clone(),
        })
    }

    /// TS: InkDaemon.mergeWorktreeSessionNotes: at start, worktree sessions' own notes merge into their worktree's
    /// note (in sidebar order; originals to the trash), unless the result would be too large.
    pub async fn merge_worktree_session_notes(&self, sessions: &[Ordered<SessionRecord>]) -> Result<()> {
        let notes = self.notes();
        let mut by_worktree: indexmap::IndexMap<String, Vec<&Ordered<SessionRecord>>> = indexmap::IndexMap::new();
        for session in sort_sessions_for_sidebar(sessions) {
            let Some(id) = worktree_id(session) else { continue };
            if jsstr::js_trim(&notes.get(&session_note(&session.id)).text).is_empty() {
                continue;
            }
            by_worktree.entry(id.to_owned()).or_default().push(session);
        }
        for (id, owners) in by_worktree {
            let note = NoteId {
                kind: NoteKind::Worktree,
                id: id.clone(),
            };
            let stored = notes.get(&note);
            let titled: Vec<TitledNote> = owners
                .iter()
                .map(|session| TitledNote {
                    title: session.title.clone(),
                    text: notes.get(&session_note(&session.id)).text,
                })
                .collect();
            let text = merge_into_worktree_note(&stored.text, &titled);
            if stored.too_large == Some(true) || jsstr::js_len(&text) > MAX_NOTES_CHARS {
                self.log(&format!(
                    "not merging {} session note(s) into worktree note {id}: it would be too large",
                    owners.len()
                ))
                .await;
                continue;
            }
            let saved = match notes.save(&note, &text, &stored.revision).await {
                Ok(saved) => saved.saved,
                Err(error) => {
                    self.log(&format!("merging into worktree note {id} failed: {error}"))
                        .await;
                    false
                }
            };
            if !saved {
                continue;
            }
            for session in &owners {
                let label = format!("{} (merged into its worktree note)", session.title);
                notes.remove(&session_note(&session.id), Some(&label)).await?;
            }
            let titles: Vec<&str> = owners.iter().map(|session| session.title.as_str()).collect();
            self.log(&format!(
                "merged the notes of {} into worktree note {id}",
                titles.join(", ")
            ))
            .await;
        }
        Ok(())
    }

    /// TS: InkDaemon.saveNote (`Invalid note`, `A session in a worktree writes in the worktree’s note (shared by its
    /// sessions)`, `This session now shares another note; nothing was saved`, `Its worktree was deleted: the worktree
    /// note is read-only`): revision-checked.
    pub async fn save_note(&self, message: &SaveNote) -> Result<NoteSaveResult> {
        let session = self.require_session(&message.session_id)?;
        let (Some(text), Some(revision)) = (message.text.as_deref(), message.revision.as_deref()) else {
            return Err(DhError::msg("Invalid note"));
        };
        let section = note_section(&message.section);
        if section == NoteSection::Session && !shows_own_note(&session) {
            return Err(DhError::msg(
                "A session in a worktree writes in the worktree’s note (shared by its sessions)",
            ));
        }
        let note = self.note_of(&session, section)?;
        if is_shared_section(&message.section) {
            self.check_shared_note(&session, &note, &message.note_id, "saved")?;
        }
        let result = self.notes().save(&note, text, revision).await?;
        if result.changed {
            self.notes_changed(note.kind, &note_file_stem(&note.id));
        }
        Ok(NoteSaveResult {
            saved: result.saved,
            session: self.require_session(&session.id)?,
        })
    }

    /// The checks of a change to a session's shared note: the one the UI showed, and not a deleted worktree's.
    fn check_shared_note(
        &self,
        session: &SessionRecord,
        note: &NoteId,
        note_id: &Loose<String>,
        done: &str,
    ) -> Result<()> {
        // `noteId !== undefined && noteId !== '<kind>:<id>'`: a value of another type never matches.
        if !note_id.is_absent() && note_id.known() != Some(&format!("{}:{}", note.kind.as_str(), note.id)) {
            return Err(DhError::msg(format!(
                "This session now shares another note; nothing was {done}"
            )));
        }
        if session
            .shared_notes
            .as_ref()
            .is_some_and(|shared| shared.read_only == Some(true))
        {
            return Err(DhError::msg("Its worktree was deleted: the worktree note is read-only"));
        }
        Ok(())
    }

    /// tasksKeyFor on a request's `cwd`: `typeof cwd !== 'string' || !cwd` is `Invalid repository path`.
    pub async fn tasks_key_for_request(&self, cwd: &Loose<String>) -> Result<String> {
        match cwd.known() {
            Some(cwd) => self.tasks_key_for(cwd).await,
            None => Err(DhError::msg("Invalid repository path")),
        }
    }

    /// TS: InkDaemon.tasksKeyFor (`Invalid repository path`): the task list key of the repository `cwd` is in.
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

    /// TS: InkDaemon.linkedTasksOf: every task linked to the session's work, in file order.
    pub fn linked_tasks_of(&self, session: &SessionRecord) -> Vec<Task> {
        let mut needles = Vec::new();
        if let Some(wt) = worktree_id(session) {
            needles.push(TaskLink::Worktree { wt: wt.to_owned() });
        }
        needles.push(TaskLink::Session { s: session.id.clone() });
        let mut found = Vec::new();
        for (_, note) in self.notes().entries(NoteKind::Tasks) {
            if needles.iter().any(|link| note.text.contains(&link_needle(link))) {
                found.extend(
                    parse_tasks(&note.text)
                        .into_iter()
                        .filter(|task| needles.iter().any(|needle| link_matches(&task.meta, needle))),
                );
            }
        }
        found
    }

    /// TS: InkDaemon.checkTaskLink (`That worktree is gone; pick another`, `That session is gone or now works in a
    /// worktree; pick another`, `That work belongs to another repository`).
    pub async fn check_task_link(&self, key: &str, link: &TaskLink) -> Result<()> {
        let sessions: Vec<Ordered<SessionRecord>> = match link {
            TaskLink::Worktree { wt } => {
                let gone = self
                    .worktrees
                    .borrow()
                    .records
                    .get(wt)
                    .is_none_or(|record| truthy(record.deleted_at.as_deref()));
                if gone {
                    return Err(DhError::msg("That worktree is gone; pick another"));
                }
                self.all_sessions()
                    .into_iter()
                    .filter(|session| session.worktree.as_ref().and_then(|worktree| worktree.id.as_ref()) == Some(wt))
                    .collect()
            }
            TaskLink::Session { s } => match self.get_session(s) {
                Some(session) if worktree_id(&session).is_none() => vec![session],
                _ => {
                    return Err(DhError::msg(
                        "That session is gone or now works in a worktree; pick another",
                    ));
                }
            },
        };
        for session in sessions {
            if self.tasks_key_for(&session.repo_root).await.ok().as_deref() == Some(key) {
                return Ok(());
            }
        }
        Err(DhError::msg("That work belongs to another repository"))
    }

    /// TS: InkDaemon.tasksDoc.
    pub fn tasks_doc(&self, key: &str) -> TasksDoc {
        let id = tasks_note(key);
        let note = self.notes().get(&id);
        TasksDoc {
            key: key.to_owned(),
            path: path_text(&note_file_path(&id)),
            text: note.text,
            revision: note.revision,
            too_large: only_true(note.too_large),
        }
    }

    /// TS: InkDaemon.broadcastTasks: `tasks-updated` to the clients showing that list.
    pub fn broadcast_tasks(&self, key: &str) {
        let doc = self.tasks_doc(key);
        let clients: Vec<_> = self
            .server
            .borrow()
            .clients
            .values()
            .filter(|client| client.subscription.tasks_key.as_deref() == Some(key))
            .map(|client| client.handle.clone())
            .collect();
        for client in clients {
            send_message(
                &client,
                &ServerMessage::TasksUpdated(TasksUpdated { tasks: doc.clone() }),
            );
        }
    }

    /// TS: InkDaemon.changeTasks: applies `op` to the list as it is on disk now; answers how many tasks changed.
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

    /// TS: InkDaemon.changeLinkedTasks: a linked-work op on every list that links that work (failures are logged).
    pub async fn change_linked_tasks(&self, op: TaskOp) {
        let link = match &op {
            TaskOp::Link { link, .. }
            | TaskOp::TickLinked { link, .. }
            | TaskOp::ReopenLinked { link, .. }
            | TaskOp::AbandonLinked { link, .. } => link.clone(),
            _ => return,
        };
        let needle = link_needle(&link);
        let notes = self.notes();
        let stems: Vec<String> = notes
            .entries(NoteKind::Tasks)
            .into_iter()
            .map(|(stem, _)| stem)
            .collect();
        for key in stems {
            // As a live Map iteration sees it: a list changed meanwhile is read as it is now.
            if !notes.get(&tasks_note(&key)).text.contains(&needle) {
                continue;
            }
            let changed = match self.change_tasks(&key, op.clone()).await {
                Ok(changed) => changed,
                Err(error) => {
                    self.log(&format!("updating tasks {key} ({}) failed: {error}", op_type(&op)))
                        .await;
                    0
                }
            };
            if changed > 0 {
                self.log(&format!("{}: {changed} task(s) of {needle}", op_type(&op)))
                    .await;
            }
        }
    }

    /// TS: InkDaemon.syncWorktreeTasks: merged ticks the worktree's task, unmerged reopens it, deleted unmerged sends
    /// it back to the backlog.
    pub async fn sync_worktree_tasks(
        &self,
        before: Option<&WorktreeRecord>,
        record: &WorktreeRecord,
        branch: Option<&str>,
    ) {
        let link = TaskLink::Worktree { wt: record.id.clone() };
        let was_merged = before.is_some_and(|before| truthy(before.merged_at.as_deref()));
        let merged = truthy(record.merged_at.as_deref());
        if !was_merged && merged {
            self.change_linked_tasks(TaskOp::TickLinked {
                link: link.clone(),
                auto: TaskAuto::Merge,
            })
            .await;
        } else if was_merged && !merged {
            self.change_linked_tasks(TaskOp::ReopenLinked {
                link: link.clone(),
                auto: TaskAuto::Merge,
            })
            .await;
        }
        // Idempotent (only open linked tasks change), so a record superseded at join is covered too.
        if truthy(record.deleted_at.as_deref()) && !merged {
            self.change_linked_tasks(TaskOp::AbandonLinked {
                link,
                tried: branch.map(str::to_owned),
            })
            .await;
        }
    }

    /// TS: InkDaemon.syncDoneTasks: done on the last session of a task's work ticks it; undone reopens it.
    pub async fn sync_done_tasks(&self, session: &SessionRecord, done: bool) {
        // TS worktreeRecordOf: the record of the worktree incarnation the session runs in.
        let record =
            worktree_id(session).and_then(|id| self.worktrees.borrow().records.get(id).map(|record| record.id.clone()));
        let link = match &record {
            Some(wt) => TaskLink::Worktree { wt: wt.clone() },
            None => TaskLink::Session { s: session.id.clone() },
        };
        if !done {
            self.change_linked_tasks(TaskOp::ReopenLinked {
                link,
                auto: TaskAuto::Done,
            })
            .await;
            return;
        }
        let all_done = match &record {
            Some(wt) => self
                .all_sessions()
                .iter()
                .filter(|other| other.worktree.as_ref().and_then(|worktree| worktree.id.as_ref()) == Some(wt))
                .all(|other| truthy(other.done_at.as_deref())),
            None => truthy(session.done_at.as_deref()),
        };
        if all_done {
            self.change_linked_tasks(TaskOp::TickLinked {
                link,
                auto: TaskAuto::Done,
            })
            .await;
        }
    }

    /// TS: InkDaemon.sendOpenNoteItems (`These notes changed meanwhile; try again`): every open item of a note to the
    /// tasks at once (all or nothing); answers how many were sent.
    pub async fn send_open_note_items(&self, note: &NoteId, key: &str, options: SendItemsOptions) -> Result<usize> {
        let notes = self.notes();
        let stored = notes.get(note);
        if options
            .revision
            .as_ref()
            .is_some_and(|revision| *revision != stored.revision)
        {
            return Err(DhError::msg("These notes changed meanwhile; try again"));
        }
        let promoted = promote_open_note_lines(&stored.text, &mut new_task_id);
        if promoted.items.is_empty() {
            return Ok(0);
        }
        let mut added: Vec<String> = Vec::new();
        let mut sent = Ok(());
        for item in &promoted.items {
            let op = add_op(
                &item.title,
                Some(&item.id),
                options.link.clone(),
                options.from.as_deref(),
            );
            if let Err(error) = self.change_tasks(key, op).await {
                sent = Err(error);
                break;
            }
            added.push(item.id.clone());
        }
        if sent.is_ok() {
            sent = match notes.save(note, &promoted.text, &stored.revision).await {
                Ok(saved) if !saved.saved => Err(DhError::msg("These notes changed meanwhile; try again")),
                Ok(saved) => {
                    if saved.changed {
                        self.notes_changed(note.kind, &note_file_stem(&note.id));
                    }
                    Ok(())
                }
                Err(error) => Err(error),
            };
        }
        if let Err(error) = sent {
            for id in added {
                let _ = self.change_tasks(key, TaskOp::Remove { id }).await;
            }
            return Err(error);
        }
        self.log(&format!(
            "sent {} open note item(s) of {}:{} to tasks",
            promoted.items.len(),
            note.kind.as_str(),
            note.id
        ))
        .await;
        Ok(promoted.items.len())
    }

    /// TS: InkDaemon.sendWorktreeItemsToBacklog: a worktree note's open items to the backlog, from its branch.
    pub async fn send_worktree_items_to_backlog(
        &self,
        record: Option<&WorktreeRecord>,
        repo_root: &str,
        branch: Option<&str>,
    ) -> Result<usize> {
        let Some(record) = record else { return Ok(0) };
        let note = NoteId {
            kind: NoteKind::Worktree,
            id: record.id.clone(),
        };
        let key = self.tasks_key_for(repo_root).await?;
        let options = SendItemsOptions {
            from: branch.filter(|branch| !branch.is_empty()).map(str::to_owned),
            ..SendItemsOptions::default()
        };
        self.send_open_note_items(&note, &key, options).await
    }

    /// TS: InkDaemon.sendNoteItems (`A` in the Notes view; `This session now shares another note; nothing was sent`).
    pub async fn send_note_items(&self, message: &SendOpenItems) -> Result<SendOpenItemsResult> {
        let session = self.require_session(&message.session_id)?;
        let note = self.note_of(&session, note_section(&message.section))?;
        if is_shared_section(&message.section) {
            self.check_shared_note(&session, &note, &message.note_id, "sent")?;
        }
        let key = self.tasks_key_for(&session.repo_root).await?;
        // `revision !== undefined && stored.revision !== revision`: a revision of another type never matches (a NUL
        // never appears in a revision).
        let revision = match &message.revision {
            Loose::Absent => None,
            Loose::Known(revision) => Some(revision.clone()),
            Loose::Other(other) => Some(format!("\0{other}")),
        };
        let options = SendItemsOptions {
            revision,
            ..SendItemsOptions::default()
        };
        let sent = self.send_open_note_items(&note, &key, options).await?;
        Ok(SendOpenItemsResult {
            session: self.require_session(&session.id)?,
            tasks: self.tasks_doc(&key),
            sent: i64::try_from(sent).unwrap_or(i64::MAX),
        })
    }

    /// TS: InkDaemon.promoteNoteItem (`Invalid line`, `Put the cursor on a checklist item (- [ ] …) to send it to
    /// Tasks`, `That item is already ticked; only open items go to Tasks`): one open item to the backlog.
    pub async fn promote_note_item(&self, message: &PromoteNoteItem) -> Result<PromoteNoteItemResult> {
        let session = self.require_session(&message.session_id)?;
        let note = self.note_of(&session, note_section(&message.section))?;
        if is_shared_section(&message.section) {
            self.check_shared_note(&session, &note, &message.note_id, "sent")?;
        }
        // `!Number.isInteger(line) || line < 0` (a fraction is refused, not floored).
        let Some(line) = message.line.and_then(|line| usize::try_from(line).ok()) else {
            return Err(DhError::msg("Invalid line"));
        };
        let notes = self.notes();
        let stored = notes.get(&note);
        // `stored.revision !== message.revision`: a revision of another type never matches.
        let Some(revision) = message
            .revision
            .as_deref()
            .filter(|revision| *revision == stored.revision)
        else {
            return Err(DhError::msg("These notes changed meanwhile; try again"));
        };
        let id = new_task_id();
        let promoted = promote_note_line(&stored.text, line, &id)
            .ok_or_else(|| DhError::msg("Put the cursor on a checklist item (- [ ] …) to send it to Tasks"))?;
        if promoted.done {
            return Err(DhError::msg("That item is already ticked; only open items go to Tasks"));
        }
        let key = self.tasks_key_for(&session.repo_root).await?;
        // An item sent from a note is work of its own: the backlog (the note's own work's to-dos are its steps).
        self.change_tasks(&key, add_op(&promoted.title, Some(&id), None, None))
            .await?;
        let saved = notes.save(&note, &promoted.text, revision).await?;
        if !saved.saved {
            let _ = self.change_tasks(&key, TaskOp::Remove { id }).await;
            return Err(DhError::msg("These notes changed meanwhile; try again"));
        }
        if saved.changed {
            self.notes_changed(note.kind, &note_file_stem(&note.id));
        }
        Ok(PromoteNoteItemResult {
            session: self.require_session(&session.id)?,
            tasks: self.tasks_doc(&key),
        })
    }

    /// The open task `task_id` of list `key` (`That task is no longer in the list`, `That task is done; space reopens
    /// it`).
    fn open_task(&self, key: &str, task_id: &str) -> Result<Task> {
        let task = parse_tasks(&self.notes().get(&tasks_note(key)).text)
            .into_iter()
            .find(|item| item.id == task_id)
            .ok_or_else(|| DhError::msg("That task is no longer in the list"))?;
        if task.done {
            return Err(DhError::msg("That task is done; space reopens it"));
        }
        Ok(task)
    }

    /// Takes task `task_id` out of list `key` after its note was written; if that fails, the note is put back.
    async fn remove_task_after_note(&self, key: &str, task_id: &str, note: &NoteId, before: &str) -> Result<()> {
        let Err(error) = self.change_tasks(key, TaskOp::Remove { id: task_id.to_owned() }).await else {
            return Ok(());
        };
        let notes = self.notes();
        let now = notes.get(note);
        if notes.save(note, before, &now.revision).await?.changed {
            self.notes_changed(note.kind, &note_file_stem(&note.id));
        }
        Err(error)
    }

    /// TS: InkDaemon.taskToWorkNote (`That task is no longer in the list`, `That task is done; space reopens it`,
    /// `Invalid work`, `Its note changed meanwhile; try again`): the task becomes an item of its work's note.
    pub async fn task_to_work_note(&self, cwd: &str, task_id: &str, section: &str) -> Result<TasksDoc> {
        let key = self.tasks_key_for(cwd).await?;
        let task = self.open_task(&key, task_id)?;
        let link = link_of_key(section).ok_or_else(|| DhError::msg("Invalid work"))?;
        self.check_task_link(&key, &link).await?;
        let note = match &link {
            TaskLink::Worktree { wt } => NoteId {
                kind: NoteKind::Worktree,
                id: wt.clone(),
            },
            TaskLink::Session { s } => session_note(s),
        };
        let notes = self.notes();
        let stored = notes.get(&note);
        // Sent from this very note: its ↗ line becomes the item again, rather than a second copy at the end.
        let text = match find_note_task_link(&stored.text, task_id) {
            Some(line) => return_task_to_note(&stored.text, line, &task)?,
            None => append_note_item(&stored.text, &task.title, &task.body),
        };
        let saved = notes.save(&note, &text, &stored.revision).await?;
        if !saved.saved {
            return Err(DhError::msg("Its note changed meanwhile; try again"));
        }
        if saved.changed {
            self.notes_changed(note.kind, &note_file_stem(&note.id));
        }
        self.remove_task_after_note(&key, task_id, &note, &stored.text).await?;
        Ok(self.tasks_doc(&key))
    }

    /// TS: InkDaemon.migrateFollowUpTasks: at start, open tasks assigned to live work move into its note as items.
    pub async fn migrate_follow_up_tasks(&self, sessions: &[Ordered<SessionRecord>]) {
        // `sessions.find(item => item.id === link.s)`: the first with that ID decides.
        let mut main_checkout: HashMap<String, bool> = HashMap::new();
        for session in sessions {
            main_checkout
                .entry(session.id.clone())
                .or_insert_with(|| worktree_id(session).is_none());
        }
        let main_checkout = Rc::new(main_checkout);
        let notes = self.notes();
        let stems: Vec<String> = notes
            .entries(NoteKind::Tasks)
            .into_iter()
            .map(|(stem, _)| stem)
            .collect();
        for stem in stems {
            let tasks = tasks_note(&stem);
            let has_note = self.follow_up_notes(&main_checkout);
            let Some(migrated) = migrate_follow_ups(&notes.get(&tasks).text, &|link| has_note(link)) else {
                continue;
            };
            let moved = async {
                for item in &migrated.moved {
                    let note = match &item.link {
                        TaskLink::Worktree { wt } => NoteId {
                            kind: NoteKind::Worktree,
                            id: wt.clone(),
                        },
                        TaskLink::Session { s } => session_note(s),
                    };
                    let (title, body) = (item.title.clone(), item.body.clone());
                    notes
                        .modify(&note, move |text| Ok((append_note_item(text, &title, &body), ())))
                        .await?;
                }
                // Again on the file as it is (the same text at start), so nothing in it is lost.
                let has_note = self.follow_up_notes(&main_checkout);
                notes
                    .modify(&tasks, move |text| {
                        let next = migrate_follow_ups(text, &|link| has_note(link))
                            .map_or_else(|| text.to_owned(), |migrated| migrated.text);
                        Ok((next, ()))
                    })
                    .await?;
                Ok::<(), DhError>(())
            }
            .await;
            match moved {
                Ok(()) => {
                    self.log(&format!(
                        "moved {} follow-up task(s) of {stem} into their notes as steps",
                        migrated.moved.len()
                    ))
                    .await;
                }
                Err(error) => {
                    self.log(&format!("migrating the follow-ups of {stem} failed: {error}"))
                        .await;
                }
            }
        }
    }

    /// migrateFollowUpTasks `noteOf(link) !== undefined`: a worktree while its record is live, a main-checkout
    /// session's own (as the records are now).
    fn follow_up_notes(&self, main_checkout: &Rc<HashMap<String, bool>>) -> impl Fn(&TaskLink) -> bool + 'static {
        let live: HashSet<String> = self
            .worktrees
            .borrow()
            .records
            .values()
            .filter(|record| !truthy(record.deleted_at.as_deref()))
            .map(|record| record.id.clone())
            .collect();
        let main_checkout = Rc::clone(main_checkout);
        move |link: &TaskLink| match link {
            TaskLink::Worktree { wt } => live.contains(wt),
            TaskLink::Session { s } => main_checkout.get(s).copied().unwrap_or(false),
        }
    }

    /// TS: InkDaemon.returnTaskToNote (`Some work is for that task; it stays a task (w: the backlog first)`, `No note
    /// links to that task any more (...)`, `Its worktree was deleted: that note is read-only`).
    pub async fn return_task_to_note(&self, cwd: &str, task_id: &str) -> Result<TasksDoc> {
        let key = self.tasks_key_for(cwd).await?;
        let task = self.open_task(&key, task_id)?;
        if is_linked(&task) {
            return Err(DhError::msg(
                "Some work is for that task; it stays a task (w: the backlog first)",
            ));
        }
        let origin = self.task_origin(&key, task_id).await.ok_or_else(|| {
            DhError::msg("No note links to that task any more (its session was removed, or the ↗ line was edited)")
        })?;
        if origin.read_only {
            return Err(DhError::msg("Its worktree was deleted: that note is read-only"));
        }
        let notes = self.notes();
        let stored = notes.get(&origin.note);
        // A link that is gone is refused by returnTaskToNote (line -1 in TS).
        let line = find_note_task_link(&stored.text, task_id).unwrap_or(usize::MAX);
        let text = return_task_to_note(&stored.text, line, &task)?;
        let saved = notes.save(&origin.note, &text, &stored.revision).await?;
        if !saved.saved {
            return Err(DhError::msg("Its note changed meanwhile; try again"));
        }
        if saved.changed {
            self.notes_changed(origin.note.kind, &note_file_stem(&origin.note.id));
        }
        self.remove_task_after_note(&key, task_id, &origin.note, &stored.text)
            .await?;
        Ok(self.tasks_doc(&key))
    }

    /// TS: InkDaemon.taskOrigin: the note of a session of this repository that links to task `id`.
    pub async fn task_origin(&self, key: &str, id: &str) -> Option<TaskOrigin> {
        let notes = self.notes();
        for session in self.all_sessions() {
            let mut candidates = vec![TaskOrigin {
                note: session_note(&session.id),
                read_only: false,
            }];
            if let Some(shared) = shared_note_identity(&session) {
                candidates.push(TaskOrigin {
                    note: NoteId {
                        kind: shared_kind(shared.kind),
                        id: shared.id,
                    },
                    read_only: session
                        .shared_notes
                        .as_ref()
                        .is_some_and(|notes| notes.read_only == Some(true)),
                });
            }
            let found = candidates
                .into_iter()
                .find(|origin| find_note_task_link(&notes.get(&origin.note).text, id).is_some());
            if let Some(found) = found
                && self.tasks_key_for(&session.repo_root).await.ok().as_deref() == Some(key)
            {
                return Some(found);
            }
        }
        None
    }

    /// TS: InkDaemon.moveOpenNoteItems (`kill the session before removing it`): before removing a session, the open
    /// items of its notes go to the backlog.
    pub async fn move_open_note_items(&self, session_id: &str) -> Result<()> {
        let session = self.require_session(session_id)?;
        // Checked here too, so items are never copied for a removal that is then refused.
        if self.launch.borrow().workers.contains_key(session_id) || session.status == SessionStatus::Running {
            return Err(DhError::msg("kill the session before removing it"));
        }
        let worktree = worktree_id(&session).map(str::to_owned);
        let last_in_worktree = worktree.as_deref().filter(|worktree| {
            !self.all_sessions().iter().any(|other| {
                other.id != session_id
                    && other.worktree.as_ref().and_then(|record| record.id.as_deref()) == Some(*worktree)
            })
        });
        let branch = worktree_branch(&session);
        let notes = self.notes();
        let mut items = open_note_items(Some(&notes.get(&session_note(session_id)).text));
        if let Some(worktree) = last_in_worktree {
            let note = NoteId {
                kind: NoteKind::Worktree,
                id: worktree.to_owned(),
            };
            items.extend(open_note_items(Some(&notes.get(&note).text)));
        }
        if items.is_empty() {
            return Ok(());
        }
        let key = self.tasks_key_for(&session.repo_root).await?;
        // As when a worktree's work ends: backlog tasks left open in its branch (the notes go to the trash, so no ↗
        // links).
        for title in &items {
            self.change_tasks(&key, add_op(title, None, None, branch.as_deref()))
                .await?;
        }
        self.log(&format!(
            "moved {} open note item(s) of {} to tasks",
            items.len(),
            session.title
        ))
        .await;
        Ok(())
    }

    /// TS: InkDaemon.notesChanged: every session showing the note is re-projected and broadcast (a task list:
    /// `tasks-updated`).
    pub fn notes_changed(&self, kind: NoteKind, stem: &str) {
        if kind == NoteKind::Tasks {
            self.broadcast_tasks(stem);
            return;
        }
        // Notes are not in state.json, so nothing is persisted.
        for session in self.all_sessions() {
            let shows = if kind == NoteKind::Session {
                note_file_stem(&session.id) == stem
            } else {
                shared_note_identity(&session)
                    .is_some_and(|shared| shared_kind(shared.kind) == kind && note_file_stem(&shared.id) == stem)
            };
            if !shows {
                continue;
            }
            self.set_session(session.clone());
            self.broadcast_session_updated(&session);
        }
    }

    /// TS: handleRequest `open-note`: the note's file (created when missing), for an editor.
    pub async fn handle_open_note(&self, request: &OpenNote) -> Result<String> {
        let session = self.require_session(&request.session_id)?;
        let note = self.note_of(&session, note_section(&request.section))?;
        Ok(path_text(&self.notes().ensure_file(&note).await?))
    }

    /// TS: handleRequest `watch-tasks`: the client now shows that repository's list.
    pub async fn handle_watch_tasks(&self, conn: &Connection, request: &CwdRequest) -> Result<TasksDoc> {
        let key = self.tasks_key_for_request(&request.cwd).await?;
        self.get_client(&conn.client, |client| client.tasks_key = Some(key.clone()));
        Ok(self.tasks_doc(&key))
    }

    /// TS: handleRequest `task-op`: the repository's list first, then tasks.ts clientTaskOp on the op exactly as the
    /// client sent it (its texts and order: `Invalid repository path` before `Unknown task change`).
    pub async fn handle_task_op(&self, request: &TaskOpRequest) -> Result<TasksDoc> {
        let key = self.tasks_key_for_request(&request.cwd).await?;
        self.apply_client_task_op(&key, &request.op).await
    }

    /// TS: handleRequest `task-op` body: the list for `cwd`, then clientTaskOp on the op exactly as the client sent
    /// it (`raw`), the link check, and the change.
    pub async fn handle_client_task_op(&self, cwd: &str, raw: &serde_json::Value) -> Result<TasksDoc> {
        let key = self.tasks_key_for(cwd).await?;
        self.apply_client_task_op(&key, raw).await
    }

    /// clientTaskOp, the link check and the change, on list `key`.
    async fn apply_client_task_op(&self, key: &str, raw: &serde_json::Value) -> Result<TasksDoc> {
        let key = key.to_owned();
        let op = client_task_op(raw)?;
        if let TaskOp::Add { link: Some(link), .. } | TaskOp::Assign { link: Some(link), .. } = &op {
            self.check_task_link(&key, link).await?;
        }
        self.change_tasks(&key, op).await?;
        Ok(self.tasks_doc(&key))
    }

    /// TS: handleRequest `open-tasks`: the list's file (created when missing).
    pub async fn handle_open_tasks(&self, request: &CwdRequest) -> Result<String> {
        let key = self.tasks_key_for_request(&request.cwd).await?;
        Ok(path_text(&self.notes().ensure_file(&tasks_note(&key)).await?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::{local, session, test_client, test_daemon};
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

    fn worktree_session(id: &str, root: &str, wt: &str) -> Ordered<SessionRecord> {
        record(
            id,
            root,
            json!({"status": "exited", "worktree": {"mode": "managed", "path": format!("{root}-{wt}"), "branch": format!("b-{wt}"), "id": wt}}),
        )
    }

    fn keys(session: &Ordered<SessionRecord>) -> Vec<String> {
        match serde_json::to_value(session).unwrap() {
            Value::Object(map) => map.keys().cloned().collect(),
            _ => Vec::new(),
        }
    }

    fn titles(doc: &TasksDoc) -> Vec<String> {
        parse_tasks(&doc.text).into_iter().map(|task| task.title).collect()
    }

    fn task_op(cwd: &str, op: TaskOp) -> TaskOpRequest {
        TaskOpRequest {
            request_id: "r".to_owned(),
            cwd: cwd.into(),
            op: serde_json::to_value(&op).unwrap(),
        }
    }

    fn add(title: &str) -> TaskOp {
        add_op(title, None, None, None)
    }

    fn save(session: &str, section: NoteSection, text: &str, revision: &str, note_id: Option<&str>) -> SaveNote {
        SaveNote {
            request_id: "r".to_owned(),
            session_id: session.to_owned(),
            section: section.into(),
            note_id: note_id.map_or(Loose::Absent, Loose::from),
            text: Some(text.to_owned()),
            revision: Some(revision.to_owned()),
        }
    }

    fn find(daemon: &Daemon, key: &str, title: &str) -> Task {
        parse_tasks(&daemon.tasks_doc(key).text)
            .into_iter()
            .find(|task| task.title == title)
            .unwrap()
    }

    #[test]
    fn projected_notes_follow_the_session_and_its_shared_note() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let notes = NotesStore::new(|_, _| {});
        let mut records = WorktreeRecords::new();
        // A main-checkout session shares the repository note; the keys follow the session's own.
        let projected = project_notes(record("a", "/repo", json!({})), &records, &notes);
        assert_eq!(projected.notes.as_deref(), Some(""));
        let file = projected.notes_file.clone().unwrap();
        assert_eq!(file.path, path_text(&note_file_path(&session_note("a"))));
        assert_eq!(file.too_large, None);
        let shared = projected.shared_notes.clone().unwrap();
        assert_eq!(shared.kind, SharedNoteKind::Repo);
        assert_eq!(shared.id, repo_note_id("/repo"));
        assert_eq!(shared.read_only, None);
        let order = keys(&projected);
        assert_eq!(&order[order.len() - 3..], ["notes", "notesFile", "sharedNotes"]);
        // Keys the session already had keep their place.
        let held: Ordered<SessionRecord> = serde_json::from_value(json!({
            "id": "b", "notes": "old", "title": "b", "program": "claude", "command": "claude", "cwd": "/repo",
            "repoRoot": "/repo", "status": "exited", "createdAt": "x", "updatedAt": "x",
        }))
        .unwrap();
        let projected = project_notes(held, &records, &notes);
        assert_eq!(keys(&projected)[..2], ["id", "notes"]);
        assert_eq!(projected.notes.as_deref(), Some(""));
        // A worktree's note is read-only without a live record.
        let in_worktree = worktree_session("c", "/repo", "w1");
        let projected = project_notes(in_worktree.clone(), &records, &notes);
        let shared = projected.shared_notes.clone().unwrap();
        assert_eq!(
            (shared.kind, shared.id.as_str(), shared.read_only),
            (SharedNoteKind::Worktree, "w1", Some(true))
        );
        records.insert("w1".to_owned(), worktree_record("w1", "/repo-w1", json!({})));
        let projected = project_notes(in_worktree.clone(), &records, &notes);
        assert_eq!(projected.shared_notes.clone().unwrap().read_only, None);
        records.insert(
            "w1".to_owned(),
            worktree_record("w1", "/repo-w1", json!({"deletedAt": "2026-01-02"})),
        );
        let projected = project_notes(in_worktree, &records, &notes);
        assert_eq!(projected.shared_notes.clone().unwrap().read_only, Some(true));
    }

    #[test]
    fn saves_are_revision_checked_and_checked_against_the_shared_note() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        local(async {
            let daemon = daemon(dir.path()).await;
            daemon.set_session(record("a", "/repo", json!({"status": "exited"})));
            let revision = daemon.get_session("a").unwrap().notes_file.clone().unwrap().revision;
            let saved = daemon
                .save_note(&save("a", NoteSection::Session, "hello", &revision, None))
                .await
                .unwrap();
            assert!(saved.saved);
            assert_eq!(saved.session.notes.as_deref(), Some("hello"));
            let file = saved.session.notes_file.clone().unwrap();
            assert_eq!(std::fs::read_to_string(&file.path).unwrap(), "hello");
            // A stale save writes nothing.
            let stale = daemon
                .save_note(&save("a", NoteSection::Session, "lost", &revision, None))
                .await
                .unwrap();
            assert!(!stale.saved);
            assert_eq!(stale.session.notes.as_deref(), Some("hello"));
            // The shared note: the one the UI showed.
            let shared = saved.session.shared_notes.clone().unwrap();
            let error = daemon
                .save_note(&save(
                    "a",
                    NoteSection::Shared,
                    "x",
                    &shared.revision,
                    Some("repo:other"),
                ))
                .await
                .unwrap_err();
            assert_eq!(
                error.to_string(),
                "This session now shares another note; nothing was saved"
            );
            let note_id = format!("repo:{}", shared.id);
            let saved = daemon
                .save_note(&save(
                    "a",
                    NoteSection::Shared,
                    "shared",
                    &shared.revision,
                    Some(&note_id),
                ))
                .await
                .unwrap();
            assert!(saved.saved);
            assert_eq!(saved.session.shared_notes.clone().unwrap().text, "shared");
            // A session in a worktree writes in its worktree's note, read-only once the worktree is gone.
            daemon.set_session(worktree_session("w", "/repo", "w1"));
            let error = daemon
                .save_note(&save("w", NoteSection::Session, "x", &revision, None))
                .await
                .unwrap_err();
            assert_eq!(
                error.to_string(),
                "A session in a worktree writes in the worktree’s note (shared by its sessions)"
            );
            let held = daemon.get_session("w").unwrap();
            let shared_revision = held.shared_notes.clone().unwrap().revision;
            let error = daemon
                .save_note(&save("w", NoteSection::Shared, "x", &shared_revision, None))
                .await
                .unwrap_err();
            assert_eq!(
                error.to_string(),
                "Its worktree was deleted: the worktree note is read-only"
            );
            let mut preparing = record("p", "/repo", json!({"status": "starting"}));
            preparing.shared_notes = None;
            daemon.sessions.borrow_mut().sessions.insert("p".to_owned(), preparing);
            let error = daemon
                .save_note(&save("p", NoteSection::Shared, "x", &revision, None))
                .await
                .unwrap_err();
            assert_eq!(
                error.to_string(),
                "This session has no worktree note yet: its worktree is not ready"
            );
            let error = daemon
                .save_note(&save("gone", NoteSection::Session, "x", &revision, None))
                .await
                .unwrap_err();
            assert_eq!(error.to_string(), "Session does not exist");
        });
    }

    #[test]
    fn an_edit_outside_deckhand_reprojects_and_broadcasts_the_sessions_showing_it() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        local(async {
            let daemon = daemon(dir.path()).await;
            daemon.set_session(record("a", "/repo", json!({"status": "exited"})));
            daemon.set_session(record("b", "/repo", json!({"status": "exited"})));
            let (client, mut rx) = test_client(1);
            daemon.get_client(&client, |subscription| {
                subscription.repo_root = Some("/repo".to_owned());
            });
            let note = session_note("a");
            std::fs::write(note_file_path(&note), "from the editor").unwrap();
            // The store re-reads it (as its watcher would) and tells the daemon.
            daemon
                .notes()
                .modify(&note, |text| Ok((text.to_owned(), ())))
                .await
                .unwrap();
            daemon.notes_changed(NoteKind::Session, "a");
            assert_eq!(
                daemon.get_session("a").unwrap().notes.as_deref(),
                Some("from the editor")
            );
            let message: Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
            assert_eq!(message["type"], "session-updated");
            assert_eq!(message["session"]["id"], "a");
            assert_eq!(message["session"]["notes"], "from the editor");
            assert!(rx.try_recv().is_err());
            // The repository note is shown by both.
            let shared = daemon.get_session("a").unwrap().shared_notes.clone().unwrap();
            daemon.notes_changed(NoteKind::Repo, &shared.id);
            let ids: Vec<Value> = std::iter::from_fn(|| rx.try_recv().ok())
                .map(|line| serde_json::from_str::<Value>(&line).unwrap()["session"]["id"].clone())
                .collect();
            assert_eq!(ids, [json!("a"), json!("b")]);
        });
    }

    #[test]
    fn task_ops_change_the_file_and_reach_watching_clients() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        local(async {
            let daemon = daemon(dir.path()).await;
            assert_eq!(
                daemon.tasks_key_for("").await.unwrap_err().to_string(),
                "Invalid repository path"
            );
            let (client, mut rx) = test_client(1);
            let conn = Connection::new(client);
            let request = CwdRequest {
                request_id: "r".to_owned(),
                cwd: root.clone().into(),
            };
            let watched = daemon.handle_watch_tasks(&conn, &request).await.unwrap();
            assert_eq!(watched.key, repo_note_id(&root));
            assert_eq!(watched.text, "");
            daemon.handle_task_op(&task_op(&root, add("First task"))).await.unwrap();
            let doc = daemon
                .handle_task_op(&task_op(&root, add("Second task")))
                .await
                .unwrap();
            assert_eq!(doc.path, path_text(&note_file_path(&tasks_note(&doc.key))));
            assert_eq!(titles(&doc), ["First task", "Second task"]);
            let pushed: Vec<Value> = std::iter::from_fn(|| rx.try_recv().ok())
                .map(|line| serde_json::from_str(&line).unwrap())
                .collect();
            assert_eq!(pushed.len(), 2);
            assert_eq!(pushed[1]["type"], "tasks-updated");
            assert_eq!(pushed[1]["tasks"]["text"], json!(doc.text));
            let first = find(&daemon, &doc.key, "First task");
            let link = TaskOp::Link {
                id: first.id.clone(),
                link: TaskLink::Session { s: "x".to_owned() },
            };
            let refused = daemon.handle_task_op(&task_op(&root, link)).await.unwrap_err();
            assert_eq!(refused.to_string(), "Unknown task change");
            let gone = daemon
                .handle_task_op(&task_op(&root, TaskOp::Toggle { id: "gone".to_owned() }))
                .await
                .unwrap_err();
            assert!(gone.to_string().contains("no longer in the list"));
            // Work of no session, or of another repository, is refused.
            let assign = |link: TaskLink| TaskOp::Assign {
                id: first.id.clone(),
                link: Some(link),
            };
            let error = daemon
                .handle_task_op(&task_op(&root, assign(TaskLink::Session { s: "nobody".to_owned() })))
                .await
                .unwrap_err();
            assert_eq!(
                error.to_string(),
                "That session is gone or now works in a worktree; pick another"
            );
            let elsewhere = dir.path().join("elsewhere");
            std::fs::create_dir_all(&elsewhere).unwrap();
            daemon.set_session(record("elsewhere", elsewhere.to_str().unwrap(), json!({})));
            let error = daemon
                .handle_task_op(&task_op(
                    &root,
                    assign(TaskLink::Session {
                        s: "elsewhere".to_owned(),
                    }),
                ))
                .await
                .unwrap_err();
            assert_eq!(error.to_string(), "That work belongs to another repository");
            let error = daemon
                .handle_task_op(&task_op(&root, assign(TaskLink::Worktree { wt: "w9".to_owned() })))
                .await
                .unwrap_err();
            assert_eq!(error.to_string(), "That worktree is gone; pick another");
            daemon.set_session(record("here", &root, json!({})));
            let doc = daemon
                .handle_task_op(&task_op(&root, assign(TaskLink::Session { s: "here".to_owned() })))
                .await
                .unwrap();
            assert_eq!(find(&daemon, &doc.key, "First task").meta.s.as_deref(), Some("here"));
            let linked = daemon.linked_tasks_of(&daemon.get_session("here").unwrap());
            assert_eq!(linked.len(), 1);
            assert_eq!(linked[0].title, "First task");
            // A linked worktree of the same repository shares the list.
            let other = dir.path().join("other");
            git(
                Path::new(&root),
                &["worktree", "add", "-q", "-b", "other", other.to_str().unwrap()],
            );
            assert_eq!(daemon.tasks_key_for(other.to_str().unwrap()).await.unwrap(), doc.key);
            assert_eq!(daemon.handle_open_tasks(&request).await.unwrap(), doc.path);
            // Not a repository: the lookup fails and is forgotten.
            assert!(daemon.tasks_key_for(elsewhere.to_str().unwrap()).await.is_err());
            tokio::task::yield_now().await;
            assert!(
                !daemon
                    .notes_tasks
                    .borrow()
                    .tasks_keys
                    .contains_key(&path::resolve(elsewhere.to_str().unwrap()))
            );
        });
    }

    /// TS daemon.ts task-op runs clientTaskOp on `message.op` as parsed from the line: a null or malformed link is
    /// refused (not dropped), a non-true `after` and a non-string `body` are ignored, a missing title is named.
    #[test]
    fn a_task_op_is_checked_as_the_client_sent_it() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        local(async {
            let daemon = daemon(dir.path()).await;
            daemon.set_session(record("here", &root, json!({})));
            let send = |op: Value| {
                let daemon = &daemon;
                let root = root.clone();
                async move { daemon.handle_client_task_op(&root, &op).await }
            };
            let doc = send(json!({"type": "add", "title": "First", "link": {"s": "here"}}))
                .await
                .unwrap();
            let first = find(&daemon, &doc.key, "First");
            assert_eq!(first.meta.s.as_deref(), Some("here"));
            let refused = [
                json!({"type": "assign", "id": first.id, "link": null}),
                json!({"type": "assign", "id": first.id, "link": {"s": "here", "wt": null}}),
                json!({"type": "assign", "id": first.id, "link": {"s": "here", "wt": "w1"}}),
                json!({"type": "add", "title": "x", "link": null}),
                json!({"type": "add", "title": "x", "link": {"wt": "w1", "s": "s1"}}),
            ];
            for op in refused {
                let error = send(op.clone()).await.unwrap_err();
                assert_eq!(error.to_string(), "Invalid task change: link", "{op}");
            }
            // Nothing changed: the task is still linked and nothing was added.
            let first = find(&daemon, &doc.key, "First");
            assert_eq!(first.meta.s.as_deref(), Some("here"));
            assert_eq!(titles(&daemon.tasks_doc(&doc.key)), ["First"]);
            let error = send(json!({"type": "add"})).await.unwrap_err();
            assert_eq!(error.to_string(), "Invalid task change: title");
            let error = send(json!({"type": "edit", "id": first.id, "title": "t", "body": 5}))
                .await
                .unwrap_err();
            assert_eq!(error.to_string(), "Invalid task change: body");
            let error = send(json!({"type": "nope"})).await.unwrap_err();
            assert_eq!(error.to_string(), "Unknown task change");
            let error = send(json!("add")).await.unwrap_err();
            assert_eq!(error.to_string(), "Unknown task change");
            // A non-string body and a non-true `after` are ignored, as in TS.
            let doc = send(json!({"type": "add", "title": "Second", "body": 5}))
                .await
                .unwrap();
            assert_eq!(titles(&doc), ["First", "Second"]);
            let second = find(&daemon, &doc.key, "Second");
            assert_eq!(second.body, "");
            let doc = send(json!({"type": "move", "id": second.id, "target": first.id, "after": 1}))
                .await
                .unwrap();
            assert_eq!(titles(&doc), ["Second", "First"]);
            // An explicit unassign (no `link` key) still clears the link.
            send(json!({"type": "assign", "id": first.id})).await.unwrap();
            assert_eq!(find(&daemon, &doc.key, "First").meta.s, None);
        });
    }

    #[test]
    fn a_checklist_item_goes_to_tasks_and_back_and_open_items_move_at_removal() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        local(async {
            let daemon = daemon(dir.path()).await;
            daemon.set_session(record("s", &root, json!({"status": "exited"})));
            let revision = daemon.get_session("s").unwrap().notes_file.clone().unwrap().revision;
            let text = "scratch\n- [ ] Promote me\n- [x] Ticked\n- [ ] Keep me";
            let saved = daemon
                .save_note(&save("s", NoteSection::Session, text, &revision, None))
                .await
                .unwrap();
            let revision = saved.session.notes_file.clone().unwrap().revision;
            let promote = |line: i64, revision: &str| PromoteNoteItem {
                request_id: "r".to_owned(),
                session_id: "s".to_owned(),
                section: NoteSection::Session.into(),
                line: Some(line),
                revision: Some(revision.to_owned()),
                note_id: Loose::Absent,
            };
            let error = daemon.promote_note_item(&promote(0, &revision)).await.unwrap_err();
            assert_eq!(
                error.to_string(),
                "Put the cursor on a checklist item (- [ ] …) to send it to Tasks"
            );
            let error = daemon.promote_note_item(&promote(2, &revision)).await.unwrap_err();
            assert_eq!(
                error.to_string(),
                "That item is already ticked; only open items go to Tasks"
            );
            let error = daemon.promote_note_item(&promote(1, "stale")).await.unwrap_err();
            assert_eq!(error.to_string(), "These notes changed meanwhile; try again");
            let error = daemon.promote_note_item(&promote(-1, &revision)).await.unwrap_err();
            assert_eq!(error.to_string(), "Invalid line");
            let promoted = daemon.promote_note_item(&promote(1, &revision)).await.unwrap();
            let key = promoted.tasks.key.clone();
            let task = find(&daemon, &key, "Promote me");
            assert_eq!((task.meta.s.as_deref(), task.meta.wt.as_deref()), (None, None));
            assert_eq!(
                promoted.session.notes.as_deref().unwrap(),
                format!(
                    "scratch\n- ↗ Promote me <!-- dh:t={} -->\n- [x] Ticked\n- [ ] Keep me",
                    task.id
                )
            );
            // And back: the ↗ line becomes the item again (with the task's details), and the task leaves the list.
            let edit = TaskOp::Edit {
                id: task.id.clone(),
                title: "Promote me".to_owned(),
                body: "details".to_owned(),
            };
            daemon.handle_task_op(&task_op(&root, edit)).await.unwrap();
            let returned = daemon.return_task_to_note(&root, &task.id).await.unwrap();
            assert!(parse_tasks(&returned.text).is_empty());
            assert_eq!(
                daemon.get_session("s").unwrap().notes.as_deref(),
                Some("scratch\n- [ ] Promote me\n  details\n- [x] Ticked\n- [ ] Keep me")
            );
            let error = daemon.return_task_to_note(&root, &task.id).await.unwrap_err();
            assert_eq!(error.to_string(), "That task is no longer in the list");
            daemon
                .handle_task_op(&task_op(&root, add("Second task")))
                .await
                .unwrap();
            let second = find(&daemon, &key, "Second task");
            let error = daemon.return_task_to_note(&root, &second.id).await.unwrap_err();
            assert_eq!(
                error.to_string(),
                "No note links to that task any more (its session was removed, or the ↗ line was edited)"
            );
            // Before a removal, its open items can go to the backlog (not while it runs).
            daemon.set_session(record("r", &root, json!({"status": "running"})));
            let error = daemon.move_open_note_items("r").await.unwrap_err();
            assert_eq!(error.to_string(), "kill the session before removing it");
            daemon.move_open_note_items("s").await.unwrap();
            let tasks = parse_tasks(&daemon.tasks_doc(&key).text);
            let moved: Vec<(&str, Option<&str>)> = tasks
                .iter()
                .map(|task| (task.title.as_str(), task.meta.from.as_deref()))
                .collect();
            assert_eq!(moved, [("Second task", None), ("Promote me", None), ("Keep me", None)]);
            assert!(tasks.iter().all(|task| task.body.is_empty()));
            // A worktree's last session also takes its worktree's open items, from its branch.
            daemon
                .worktrees
                .borrow_mut()
                .records
                .insert("w1".to_owned(), worktree_record("w1", "/x", json!({})));
            daemon.set_session(worktree_session("w", &root, "w1"));
            let note = NoteId {
                kind: NoteKind::Worktree,
                id: "w1".to_owned(),
            };
            let notes = daemon.notes();
            notes
                .save(&note, "- [ ] Left open", &notes.get(&note).revision)
                .await
                .unwrap();
            daemon.move_open_note_items("w").await.unwrap();
            assert_eq!(find(&daemon, &key, "Left open").meta.from.as_deref(), Some("b-w1"));
        });
    }

    #[test]
    fn every_open_item_of_a_note_goes_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        local(async {
            let daemon = daemon(dir.path()).await;
            daemon.set_session(record("s", &root, json!({"status": "exited"})));
            let shared = daemon.get_session("s").unwrap().shared_notes.clone().unwrap();
            let text = "- [ ] One\n- [x] Done\n- [ ] Two\n";
            let saved = daemon
                .save_note(&save("s", NoteSection::Shared, text, &shared.revision, None))
                .await
                .unwrap();
            let revision = saved.session.shared_notes.clone().unwrap().revision;
            let send = |revision: &str, note_id: Option<&str>| SendOpenItems {
                request_id: "r".to_owned(),
                session_id: "s".to_owned(),
                section: NoteSection::Shared.into(),
                revision: revision.into(),
                note_id: note_id.map_or(Loose::Absent, Loose::from),
            };
            let error = daemon.send_note_items(&send("stale", None)).await.unwrap_err();
            assert_eq!(error.to_string(), "These notes changed meanwhile; try again");
            let error = daemon
                .send_note_items(&send(&revision, Some("worktree:x")))
                .await
                .unwrap_err();
            assert_eq!(
                error.to_string(),
                "This session now shares another note; nothing was sent"
            );
            let sent = daemon.send_note_items(&send(&revision, None)).await.unwrap();
            assert_eq!(sent.sent, 2);
            assert_eq!(titles(&sent.tasks), ["One", "Two"]);
            let tasks = parse_tasks(&sent.tasks.text);
            assert_eq!(
                sent.session.shared_notes.clone().unwrap().text,
                format!(
                    "- ↗ One <!-- dh:t={} -->\n- [x] Done\n- ↗ Two <!-- dh:t={} -->\n",
                    tasks[0].id, tasks[1].id
                )
            );
            // Nothing open any more: nothing sent.
            let revision = sent.session.shared_notes.clone().unwrap().revision;
            assert_eq!(daemon.send_note_items(&send(&revision, None)).await.unwrap().sent, 0);
            // A task returns to the shared note it came from.
            daemon.return_task_to_note(&root, &tasks[1].id).await.unwrap();
            assert_eq!(
                daemon.get_session("s").unwrap().shared_notes.clone().unwrap().text,
                format!("- ↗ One <!-- dh:t={} -->\n- [x] Done\n- [ ] Two\n", tasks[0].id)
            );
            // A worktree's note goes to the backlog from its branch; no record: nothing.
            let record = worktree_record("w1", "/x", json!({}));
            let note = NoteId {
                kind: NoteKind::Worktree,
                id: "w1".to_owned(),
            };
            let notes = daemon.notes();
            notes
                .save(&note, "- [ ] Step", &notes.get(&note).revision)
                .await
                .unwrap();
            assert_eq!(
                daemon
                    .send_worktree_items_to_backlog(None, &root, Some("b"))
                    .await
                    .unwrap(),
                0
            );
            let sent = daemon
                .send_worktree_items_to_backlog(Some(&record), &root, Some("b"))
                .await
                .unwrap();
            assert_eq!(sent, 1);
            assert_eq!(
                find(&daemon, &sent_key(&daemon, &root).await, "Step")
                    .meta
                    .from
                    .as_deref(),
                Some("b")
            );
        });
    }

    async fn sent_key(daemon: &Daemon, root: &str) -> String {
        daemon.tasks_key_for(root).await.unwrap()
    }

    #[test]
    fn merges_and_done_tick_linked_tasks_and_deleted_work_goes_back_to_the_backlog() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        local(async {
            let daemon = daemon(dir.path()).await;
            let key = daemon.tasks_key_for(&root).await.unwrap();
            let wt = TaskLink::Worktree { wt: "w1".to_owned() };
            daemon
                .change_tasks(&key, add_op("Worktree task", None, Some(wt), None))
                .await
                .unwrap();
            let main_link = TaskLink::Session { s: "m".to_owned() };
            daemon
                .change_tasks(&key, add_op("Session task", None, Some(main_link), None))
                .await
                .unwrap();
            let open = worktree_record("w1", "/x", json!({}));
            let merged = worktree_record("w1", "/x", json!({"mergedAt": "2026-01-02T00:00:00.000Z"}));
            daemon.sync_worktree_tasks(Some(&open), &merged, Some("b1")).await;
            let ticked = find(&daemon, &key, "Worktree task");
            assert!(ticked.done);
            assert_eq!(ticked.meta.auto, Some(TaskAuto::Merge));
            daemon.sync_worktree_tasks(Some(&merged), &open, Some("b1")).await;
            assert!(!find(&daemon, &key, "Worktree task").done);
            let deleted = worktree_record("w1", "/x", json!({"deletedAt": "2026-01-03T00:00:00.000Z"}));
            daemon.sync_worktree_tasks(Some(&open), &deleted, Some("b1")).await;
            let back = find(&daemon, &key, "Worktree task");
            assert_eq!(
                (back.meta.wt.as_deref(), back.meta.tried.as_deref()),
                (None, Some("b1"))
            );
            // Done on a main-checkout session ticks its task; undone reopens it.
            let main = record("m", &root, json!({"doneAt": "2026-01-02T00:00:00.000Z"}));
            daemon.sync_done_tasks(&main, true).await;
            let done = find(&daemon, &key, "Session task");
            assert!(done.done);
            assert_eq!(done.meta.auto, Some(TaskAuto::Done));
            daemon.sync_done_tasks(&main, false).await;
            assert!(!find(&daemon, &key, "Session task").done);
            // A worktree's task is ticked once every session there is done.
            let shared_work = TaskLink::Worktree { wt: "w2".to_owned() };
            daemon
                .change_tasks(&key, add_op("Shared work", None, Some(shared_work), None))
                .await
                .unwrap();
            daemon
                .worktrees
                .borrow_mut()
                .records
                .insert("w2".to_owned(), worktree_record("w2", "/y", json!({})));
            let mut first = worktree_session("a", &root, "w2");
            first.done_at = Some("2026-01-02T00:00:00.000Z".to_owned());
            daemon.set_session(first.clone());
            daemon.set_session(worktree_session("b", &root, "w2"));
            daemon.sync_done_tasks(&first, true).await;
            assert!(!find(&daemon, &key, "Shared work").done);
            let mut second = worktree_session("b", &root, "w2");
            second.done_at = Some("2026-01-02T00:00:00.000Z".to_owned());
            daemon.set_session(second.clone());
            daemon.sync_done_tasks(&second, true).await;
            assert!(find(&daemon, &key, "Shared work").done);
        });
    }

    #[test]
    fn w_onto_work_puts_the_task_into_its_note() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let root = repo(&dir.path().join("repo"));
        local(async {
            let daemon = daemon(dir.path()).await;
            daemon.set_session(record("s", &root, json!({"status": "exited"})));
            let key = daemon.tasks_key_for(&root).await.unwrap();
            daemon.change_tasks(&key, add("Step")).await.unwrap();
            let step = find(&daemon, &key, "Step");
            let edit = TaskOp::Edit {
                id: step.id.clone(),
                title: "Step".to_owned(),
                body: "why".to_owned(),
            };
            daemon.change_tasks(&key, edit).await.unwrap();
            let error = daemon.task_to_work_note(&root, &step.id, "nonsense").await.unwrap_err();
            assert_eq!(error.to_string(), "Invalid work");
            let error = daemon.task_to_work_note(&root, "gone", "s:s").await.unwrap_err();
            assert_eq!(error.to_string(), "That task is no longer in the list");
            let doc = daemon.task_to_work_note(&root, &step.id, "s:s").await.unwrap();
            assert!(parse_tasks(&doc.text).is_empty());
            assert_eq!(
                daemon.get_session("s").unwrap().notes.as_deref(),
                Some("- [ ] Step\n  why\n")
            );
            // A done task stays.
            daemon.change_tasks(&key, add("Finished")).await.unwrap();
            let finished = find(&daemon, &key, "Finished");
            daemon
                .change_tasks(
                    &key,
                    TaskOp::Toggle {
                        id: finished.id.clone(),
                    },
                )
                .await
                .unwrap();
            let error = daemon.task_to_work_note(&root, &finished.id, "s:s").await.unwrap_err();
            assert_eq!(error.to_string(), "That task is done; space reopens it");
            let error = daemon.return_task_to_note(&root, &finished.id).await.unwrap_err();
            assert_eq!(error.to_string(), "That task is done; space reopens it");
            // Work started for a task keeps it.
            let linked = TaskLink::Session { s: "s".to_owned() };
            daemon
                .change_tasks(&key, add_op("Linked", None, Some(linked), None))
                .await
                .unwrap();
            let linked = find(&daemon, &key, "Linked");
            let error = daemon.return_task_to_note(&root, &linked.id).await.unwrap_err();
            assert_eq!(
                error.to_string(),
                "Some work is for that task; it stays a task (w: the backlog first)"
            );
        });
    }

    #[test]
    fn at_start_worktree_session_notes_merge_and_follow_ups_become_steps() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        local(async {
            let daemon = daemon(dir.path()).await;
            let notes = daemon.notes();
            for (id, text) in [("a", "note of a"), ("b", "note of b")] {
                let note = session_note(id);
                notes.save(&note, text, &notes.get(&note).revision).await.unwrap();
            }
            let mut a = worktree_session("a", "/repo", "w1");
            a.title = "Alpha".to_owned();
            let mut b = worktree_session("b", "/repo", "w1");
            b.title = "Beta".to_owned();
            b.created_at = "2026-01-02T00:00:00.000Z".to_owned();
            daemon
                .merge_worktree_session_notes(&[b.clone(), a.clone()])
                .await
                .unwrap();
            let worktree_note = NoteId {
                kind: NoteKind::Worktree,
                id: "w1".to_owned(),
            };
            let merged = notes.get(&worktree_note).text;
            assert!(merged.contains("## Alpha\nnote of a") && merged.contains("## Beta\nnote of b"));
            assert_eq!(notes.get(&session_note("a")).text, "");
            assert!(!note_file_path(&session_note("a")).exists());
            let trash = std::fs::read_dir(deckhand_sys::paths::notes_dir().join("trash"))
                .unwrap()
                .count();
            assert_eq!(trash, 2);
            // Follow-ups of the old model move into their work's note; one whose work is gone stays, in the backlog.
            daemon
                .worktrees
                .borrow_mut()
                .records
                .insert("w1".to_owned(), worktree_record("w1", "/repo-w1", json!({})));
            let tasks = tasks_note("k");
            let legacy = "- [ ] Add tests <!-- dh:t=f wt=w1 assigned=2026-10-01 added=2026-10-01 -->\n  details\n- [ ] Gone work <!-- dh:t=g wt=w9 assigned=2026-10-01 -->\n- [ ] Main <!-- dh:t=m s=main assigned=2026-10-01 -->\n";
            notes.save(&tasks, legacy, &notes.get(&tasks).revision).await.unwrap();
            let main = record("main", "/repo", json!({}));
            daemon.migrate_follow_up_tasks(&[main, a]).await;
            let list = parse_tasks(&notes.get(&tasks).text);
            assert_eq!(list.len(), 1);
            assert_eq!(
                (list[0].title.as_str(), list[0].meta.wt.as_deref()),
                ("Gone work", None)
            );
            assert_eq!(
                notes.get(&worktree_note).text,
                format!("{merged}\n- [ ] Add tests\n  details\n")
            );
            assert_eq!(notes.get(&session_note("main")).text, "- [ ] Main\n");
        });
    }
}
