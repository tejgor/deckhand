//! Port of notesStore.ts.
//!
//! The daemon's notes: one Markdown file per note under the state directory (the single source of truth), held in
//! memory and projected into the session records it sends. Files are written atomically (temp file + rename), saves
//! are revision-checked against the file as it is on disk, and the directories are watched (notify) so edits made in
//! an editor show up in Deckhand (and are never overwritten by it).
//!
//! Runtime: the store lives on the daemon's `LocalSet`. Per note, saves and re-reads run one at a time
//! ([`KeyedQueue`]); watcher events arrive on notify's thread and are forwarded over a channel to a local task that
//! debounces them ([`KeyedTimers`]) before re-reading.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::async_util::{KeyedQueue, KeyedTimers, Task};
use deckhand_core::error::Result;

/// TS: notesStore.ts NoteKind: a session's own note, a linked worktree incarnation's, the main checkout's, or a
/// repository's task list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NoteKind {
    #[serde(rename = "session")]
    Session,
    #[serde(rename = "worktree")]
    Worktree,
    #[serde(rename = "repo")]
    Repo,
    #[serde(rename = "tasks")]
    Tasks,
}

impl NoteKind {
    /// TS: notesStore.ts KINDS (DIRECTORIES key order).
    pub const ALL: [NoteKind; 4] = [NoteKind::Session, NoteKind::Worktree, NoteKind::Repo, NoteKind::Tasks];

    /// The kind's name (`session`, ...), as in cache keys and trash file names.
    pub fn as_str(self) -> &'static str {
        match self {
            NoteKind::Session => "session",
            NoteKind::Worktree => "worktree",
            NoteKind::Repo => "repo",
            NoteKind::Tasks => "tasks",
        }
    }

    /// TS: notesStore.ts DIRECTORIES (the kind's directory under `notes/`).
    pub fn directory(self) -> &'static str {
        match self {
            NoteKind::Session => "sessions",
            NoteKind::Worktree => "worktrees",
            NoteKind::Repo => "repos",
            NoteKind::Tasks => "tasks",
        }
    }
}

/// TS: notesStore.ts NoteId: `id` is the session ID, the worktree record ID, or the repository hash (repo, tasks).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NoteId {
    pub kind: NoteKind,
    pub id: String,
}

/// TS: notesStore.ts StoredNote (`too_large`: the file is longer than the store reads or holds; `text` is cut and
/// `revision` carries the size).
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredNote {
    pub text: String,
    pub revision: String,
    pub too_large: Option<bool>,
}

/// TS: notesStore.ts READ_LIMIT (bytes read from one file at most).
pub const READ_LIMIT: usize = 1024 * 1024;
/// TS: notesStore.ts WATCH_DEBOUNCE_MS (editors save in bursts; changes settle this long before a re-read).
pub const WATCH_DEBOUNCE: Duration = Duration::from_millis(120);
/// TS: notesStore.ts WATCH_RETRY_MS (a failed directory watch is retried after this).
pub const WATCH_RETRY: Duration = Duration::from_millis(1000);
/// TS: notesStore.ts TRASH_DIRECTORY (removed notes, never read back: `notes/trash/`).
pub const TRASH_DIRECTORY: &str = "trash";

/// TS: notesStore.ts EMPTY (an empty note: no file).
pub fn empty_note() -> StoredNote {
    todo!("I2")
}

/// TS: notesStore.ts trashFileName (`<ISO date with - for :>_<kind>_<label slug>_<stem>.md`; `now_ms`: epoch ms,
/// TS `new Date()`).
pub fn trash_file_name(kind: NoteKind, label: Option<&str>, stem: &str, now_ms: i64) -> String {
    let _ = (kind, label, stem, now_ms);
    todo!("I2")
}

/// TS: notesStore.ts noteFileStem (the ID when file-name-safe, else `x-` and 32 hex of its sha256).
pub fn note_file_stem(id: &str) -> String {
    let _ = id;
    todo!("I2")
}

/// TS: notesStore.ts noteFilePath (`notes/<directory>/<stem>.md`).
pub fn note_file_path(note: &NoteId) -> PathBuf {
    let _ = note;
    todo!("I2")
}

/// TS: notesStore.ts noteStem (`<stem>.md`, not hidden or temporary; `None` otherwise).
pub fn note_stem(name: &str) -> Option<&str> {
    let _ = name;
    todo!("I2")
}

/// TS: notesStore.ts readNoteFile (at most READ_LIMIT bytes; missing: empty; too long: cut, `too_large`).
pub async fn read_note_file(file: &std::path::Path) -> Result<StoredNote> {
    let _ = file;
    todo!("I2")
}

/// TS: notesStore.ts NotesStore.save result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SaveOutcome {
    /// The text was written (or already was the file's); false: the file's revision differed, nothing written.
    pub saved: bool,
    /// The cached note changed (its sessions must hear about it).
    pub changed: bool,
}

/// TS: notesStore.ts NotesStore.modify result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModifyOutcome<T> {
    pub result: T,
    pub note: StoredNote,
    pub changed: bool,
}

/// The callback hearing about notes whose file changed outside a save (an editor, a deletion): kind and file stem.
pub type NoteChangeHandler = Box<dyn Fn(NoteKind, &str)>;

/// TS: notesStore.ts NotesStore. Clones share the store.
#[derive(Clone)]
#[allow(dead_code)] // Wave 0 skeleton: read once I2 implements the store.
pub struct NotesStore {
    inner: Rc<NotesStoreInner>,
}

#[allow(dead_code)] // Wave 0 skeleton: read once I2 implements the store.
struct NotesStoreInner {
    /// `<kind>:<stem>` → note; empty notes are not cached.
    cache: RefCell<IndexMap<String, StoredNote>>,
    /// Per note: saves and re-reads run one at a time.
    queues: KeyedQueue<String>,
    /// Watcher debounce per note.
    timers: KeyedTimers<String>,
    /// One directory watcher per kind.
    watchers: RefCell<HashMap<NoteKind, notify::RecommendedWatcher>>,
    closed: Cell<bool>,
    on_change: NoteChangeHandler,
}

#[allow(dead_code)] // Wave 0 skeleton: the private helpers are called once I2 implements the store.
impl NotesStore {
    /// TS: `new NotesStore(onChange)`.
    pub fn new(on_change: impl Fn(NoteKind, &str) + 'static) -> Self {
        let _ = on_change;
        todo!("I2")
    }

    /// TS: NotesStore.ensureDirectories (every kind's directory, 0700).
    pub async fn ensure_directories(&self) -> Result<()> {
        todo!("I2")
    }

    /// TS: NotesStore.load (reads every note file into memory; unreadable files are skipped).
    pub async fn load(&self) -> Result<()> {
        todo!("I2")
    }

    /// TS: NotesStore.migrate (`notes` stored in state.json by older versions move into their files; an existing
    /// file wins). `sessions`: each session's ID and its legacy `notes` value (`None`: absent). Returns whether any
    /// session still carried notes (state.json must be written again without them).
    pub async fn migrate(&self, sessions: &[(String, Option<serde_json::Value>)]) -> Result<bool> {
        let _ = sessions;
        todo!("I2")
    }

    /// TS: NotesStore.get (the cached note, else empty).
    pub fn get(&self, note: &NoteId) -> StoredNote {
        let _ = note;
        todo!("I2")
    }

    /// TS: NotesStore.save: writes `text` (cut to MAX_NOTES_CHARS) when the file still has `revision`; otherwise
    /// nothing is written and the file as it is now is cached. A too-large file is refused (`This note is longer than
    /// 50 000 characters; edit it in your editor (E)`). Queued per note at call time.
    pub fn save(&self, note: &NoteId, text: &str, revision: &str) -> Task<Result<SaveOutcome>> {
        let _ = (note, text, revision);
        todo!("I2")
    }

    /// TS: NotesStore.modify: rewrites the note with `transform` of its text as it is on disk now; the transform's
    /// error is returned; nothing is written when it returns the same text; a result longer than MAX_NOTES_CHARS is
    /// refused. Queued per note at call time.
    pub fn modify<T, F>(&self, note: &NoteId, transform: F) -> Task<Result<ModifyOutcome<T>>>
    where
        T: 'static,
        F: FnOnce(&str) -> Result<(String, T)> + 'static,
    {
        let _ = (note, transform);
        todo!("I2")
    }

    /// TS: NotesStore.entries (the cached notes of a kind: file stems and contents).
    pub fn entries(&self, kind: NoteKind) -> Vec<(String, StoredNote)> {
        let _ = kind;
        todo!("I2")
    }

    /// TS: NotesStore.ensureFile (the note's file, created empty when missing, e.g. to open it in an editor).
    pub async fn ensure_file(&self, note: &NoteId) -> Result<PathBuf> {
        let _ = note;
        todo!("I2")
    }

    /// TS: NotesStore.remove: a note with text moves to `notes/trash/` (see [`trash_file_name`]); an empty one is
    /// deleted. `label`: what it belonged to. Queued per note at call time.
    pub fn remove(&self, note: &NoteId, label: Option<&str>) -> Task<Result<()>> {
        let _ = (note, label);
        todo!("I2")
    }

    /// TS: NotesStore.watch (watches every kind's directory; a changed file is re-read after WATCH_DEBOUNCE; a
    /// failed watch is retried after WATCH_RETRY).
    pub fn watch(&self) {
        todo!("I2")
    }

    /// TS: NotesStore.close (stops the watchers and pending re-reads).
    pub fn close(&self) {
        todo!("I2")
    }

    /// TS: NotesStore.watchDirectory.
    fn watch_directory(&self, kind: NoteKind) {
        let _ = kind;
        todo!("I2")
    }

    /// TS: NotesStore.schedule (debounced re-read of one note).
    fn schedule(&self, kind: NoteKind, stem: &str) {
        let _ = (kind, stem);
        todo!("I2")
    }

    /// TS: NotesStore.rescan (no file name from the watcher: every file of the directory and every cached note).
    async fn rescan(&self, kind: NoteKind) {
        let _ = kind;
        todo!("I2")
    }

    /// TS: NotesStore.refresh (re-reads one note; tells `on_change` when what the sessions show changed).
    fn refresh(&self, kind: NoteKind, stem: &str) -> Task<()> {
        let _ = (kind, stem);
        todo!("I2")
    }

    /// TS: NotesStore.remember (caches a note as read or written; true when that changed what the sessions show).
    fn remember(&self, kind: NoteKind, stem: &str, note: StoredNote) -> bool {
        let _ = (kind, stem, note);
        todo!("I2")
    }
}
