//! Port of daemon.ts (notes and tasks).
//!
//! Notes live in files (notesStore.ts); the daemon projects each session's own note and the note its worktree (or main
//! checkout) shares into the session, and re-broadcasts sessions when a note changes, also outside Deckhand. Tasks:
//! one list per repository (tasks.ts), a file beside the notes, keyed by the repository's main checkout; open note
//! items can be sent to it and tasks returned to their note.

use std::collections::HashMap;

use deckhand_core::error::Result;
use deckhand_core::worktree_records::WorktreeRecords;
use deckhand_protocol::Ordered;
use deckhand_protocol::data::{
    NoteSaveResult, NoteSection, PromoteNoteItemResult, SendOpenItemsResult, SessionRecord, Task, TaskLink, TaskOp,
    TasksDoc,
};
use deckhand_protocol::records::WorktreeRecord;
use deckhand_protocol::wire::{CwdRequest, OpenNote, PromoteNoteItem, SaveNote, SendOpenItems, TaskOpRequest};
use deckhand_sys::async_util::SharedTask;
use deckhand_sys::notes_store::{NoteId, NoteKind, NotesStore};

use crate::daemon::Daemon;
use crate::server::Connection;

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

/// TS: daemon.ts projectNotes: the session with its own note (`notes`, `notesFile`) and its shared note
/// (`sharedNotes`, read-only for a deleted worktree) from the store.
pub fn project_notes(
    session: Ordered<SessionRecord>,
    worktrees: &WorktreeRecords,
    notes: &NotesStore,
) -> Ordered<SessionRecord> {
    let _ = (session, worktrees, notes);
    todo!("D8")
}

impl Daemon {
    /// TS: `this.notes`.
    pub fn notes(&self) -> NotesStore {
        todo!("D8")
    }

    /// TS: InkDaemon.noteOf (`This session has no worktree note yet: its worktree is not ready`).
    pub fn note_of(&self, session: &SessionRecord, section: NoteSection) -> Result<NoteId> {
        let _ = (session, section);
        todo!("D8")
    }

    /// TS: InkDaemon.mergeWorktreeSessionNotes: at start, worktree sessions' own notes merge into their worktree's
    /// note (in sidebar order; originals to the trash), unless the result would be too large.
    pub async fn merge_worktree_session_notes(&self, sessions: &[Ordered<SessionRecord>]) -> Result<()> {
        let _ = sessions;
        todo!("D8")
    }

    /// TS: InkDaemon.saveNote (`Invalid note`, `A session in a worktree writes in the worktree’s note (shared by its
    /// sessions)`, `This session now shares another note; nothing was saved`, `Its worktree was deleted: the worktree
    /// note is read-only`): revision-checked.
    pub async fn save_note(&self, message: &SaveNote) -> Result<NoteSaveResult> {
        let _ = message;
        todo!("D8")
    }

    /// TS: InkDaemon.tasksKeyFor (`Invalid repository path`): the task list key of the repository `cwd` is in.
    pub async fn tasks_key_for(&self, cwd: &str) -> Result<String> {
        let _ = cwd;
        todo!("D8")
    }

    /// TS: InkDaemon.linkedTasksOf: every task linked to the session's work, in file order.
    pub fn linked_tasks_of(&self, session: &SessionRecord) -> Vec<Task> {
        let _ = session;
        todo!("D8")
    }

    /// TS: InkDaemon.checkTaskLink (`That worktree is gone; pick another`, `That session is gone or now works in a
    /// worktree; pick another`, `That work belongs to another repository`).
    pub async fn check_task_link(&self, key: &str, link: &TaskLink) -> Result<()> {
        let _ = (key, link);
        todo!("D8")
    }

    /// TS: InkDaemon.tasksDoc.
    pub fn tasks_doc(&self, key: &str) -> TasksDoc {
        let _ = key;
        todo!("D8")
    }

    /// TS: InkDaemon.broadcastTasks: `tasks-updated` to the clients showing that list.
    pub fn broadcast_tasks(&self, key: &str) {
        let _ = key;
        todo!("D8")
    }

    /// TS: InkDaemon.changeTasks: applies `op` to the list as it is on disk now; answers how many tasks changed.
    pub async fn change_tasks(&self, key: &str, op: TaskOp) -> Result<usize> {
        let _ = (key, op);
        todo!("D8")
    }

    /// TS: InkDaemon.changeLinkedTasks: a linked-work op on every list that links that work (failures are logged).
    pub async fn change_linked_tasks(&self, op: TaskOp) {
        let _ = op;
        todo!("D8")
    }

    /// TS: InkDaemon.syncWorktreeTasks: merged ticks the worktree's task, unmerged reopens it, deleted unmerged sends
    /// it back to the backlog.
    pub async fn sync_worktree_tasks(
        &self,
        before: Option<&WorktreeRecord>,
        record: &WorktreeRecord,
        branch: Option<&str>,
    ) {
        let _ = (before, record, branch);
        todo!("D8")
    }

    /// TS: InkDaemon.syncDoneTasks: done on the last session of a task's work ticks it; undone reopens it.
    pub async fn sync_done_tasks(&self, session: &SessionRecord, done: bool) {
        let _ = (session, done);
        todo!("D8")
    }

    /// TS: InkDaemon.sendOpenNoteItems (`These notes changed meanwhile; try again`): every open item of a note to the
    /// tasks at once (all or nothing); answers how many were sent.
    pub async fn send_open_note_items(&self, note: &NoteId, key: &str, options: SendItemsOptions) -> Result<usize> {
        let _ = (note, key, options);
        todo!("D8")
    }

    /// TS: InkDaemon.sendWorktreeItemsToBacklog: a worktree note's open items to the backlog, from its branch.
    pub async fn send_worktree_items_to_backlog(
        &self,
        record: Option<&WorktreeRecord>,
        repo_root: &str,
        branch: Option<&str>,
    ) -> Result<usize> {
        let _ = (record, repo_root, branch);
        todo!("D8")
    }

    /// TS: InkDaemon.sendNoteItems (`A` in the Notes view; `This session now shares another note; nothing was sent`).
    pub async fn send_note_items(&self, message: &SendOpenItems) -> Result<SendOpenItemsResult> {
        let _ = message;
        todo!("D8")
    }

    /// TS: InkDaemon.promoteNoteItem (`Invalid line`, `Put the cursor on a checklist item (- [ ] …) to send it to
    /// Tasks`, `That item is already ticked; only open items go to Tasks`): one open item to the backlog.
    pub async fn promote_note_item(&self, message: &PromoteNoteItem) -> Result<PromoteNoteItemResult> {
        let _ = message;
        todo!("D8")
    }

    /// TS: InkDaemon.taskToWorkNote (`That task is no longer in the list`, `That task is done; space reopens it`,
    /// `Invalid work`, `Its note changed meanwhile; try again`): the task becomes an item of its work's note.
    pub async fn task_to_work_note(&self, cwd: &str, task_id: &str, section: &str) -> Result<TasksDoc> {
        let _ = (cwd, task_id, section);
        todo!("D8")
    }

    /// TS: InkDaemon.migrateFollowUpTasks: at start, open tasks assigned to live work move into its note as items.
    pub async fn migrate_follow_up_tasks(&self, sessions: &[Ordered<SessionRecord>]) {
        let _ = sessions;
        todo!("D8")
    }

    /// TS: InkDaemon.returnTaskToNote (`Some work is for that task; it stays a task (w: the backlog first)`, `No note
    /// links to that task any more (...)`, `Its worktree was deleted: that note is read-only`).
    pub async fn return_task_to_note(&self, cwd: &str, task_id: &str) -> Result<TasksDoc> {
        let _ = (cwd, task_id);
        todo!("D8")
    }

    /// TS: InkDaemon.taskOrigin: the note of a session of this repository that links to task `id`.
    pub async fn task_origin(&self, key: &str, id: &str) -> Option<TaskOrigin> {
        let _ = (key, id);
        todo!("D8")
    }

    /// TS: InkDaemon.moveOpenNoteItems (`kill the session before removing it`): before removing a session, the open
    /// items of its notes go to the backlog.
    pub async fn move_open_note_items(&self, session_id: &str) -> Result<()> {
        let _ = session_id;
        todo!("D8")
    }

    /// TS: InkDaemon.notesChanged: every session showing the note is re-projected and broadcast (a task list:
    /// `tasks-updated`).
    pub fn notes_changed(&self, kind: NoteKind, stem: &str) {
        let _ = (kind, stem);
        todo!("D8")
    }

    /// TS: handleRequest `open-note`: the note's file (created when missing), for an editor.
    pub async fn handle_open_note(&self, request: &OpenNote) -> Result<String> {
        let _ = request;
        todo!("D8")
    }

    /// TS: handleRequest `watch-tasks`: the client now shows that repository's list.
    pub async fn handle_watch_tasks(&self, conn: &Connection, request: &CwdRequest) -> Result<TasksDoc> {
        let _ = (conn, request);
        todo!("D8")
    }

    /// TS: handleRequest `task-op` (validated by tasks.ts clientTaskOp; a link is checked first).
    pub async fn handle_task_op(&self, request: &TaskOpRequest) -> Result<TasksDoc> {
        let _ = request;
        todo!("D8")
    }

    /// TS: handleRequest `open-tasks`: the list's file (created when missing).
    pub async fn handle_open_tasks(&self, request: &CwdRequest) -> Result<String> {
        let _ = request;
        todo!("D8")
    }
}
