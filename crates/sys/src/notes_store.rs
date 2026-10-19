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
use std::io;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Duration;

use indexmap::IndexMap;
use notify::Watcher;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;

use crate::async_util::{KeyedQueue, KeyedTimers, Task, Timer, spawn};
use crate::paths;
use deckhand_core::error::{DhError, Result};
use deckhand_core::jsstr::{js_len, js_slice, js_to_lower, js_trim};
use deckhand_core::notes::{MAX_NOTES_CHARS, MAX_NOTES_LABEL, note_revision};

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

const FILE_MODE: u32 = 0o600;
const DIR_MODE: u32 = 0o700;

/// TS: notesStore.ts EMPTY (an empty note: no file).
pub fn empty_note() -> StoredNote {
    StoredNote {
        text: String::new(),
        revision: note_revision(""),
        too_large: None,
    }
}

/// TS: notesStore.ts trashFileName (`<ISO date with - for :>_<kind>_<label slug>_<stem>.md`; `now_ms`: epoch ms,
/// TS `new Date()`).
pub fn trash_file_name(kind: NoteKind, label: Option<&str>, stem: &str, now_ms: i64) -> String {
    format_trash_file_name(kind.as_str(), label, stem, now_ms)
}

/// [`trash_file_name`] with the kind as text (the fixtures also record kinds outside the union).
fn format_trash_file_name(kind: &str, label: Option<&str>, stem: &str, now_ms: i64) -> String {
    let date = iso_string(now_ms)
        .chars()
        .take(19)
        .collect::<String>()
        .replace(':', "-");
    let slug = label_slug(label.unwrap_or(""));
    let slug = if slug.is_empty() { "note" } else { slug.as_str() };
    format!("{date}_{kind}_{slug}_{stem}.md")
}

/// `label.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 60)`.
fn label_slug(label: &str) -> String {
    let mut slug = String::new();
    let mut in_gap = false;
    for ch in js_to_lower(label).chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            if in_gap {
                slug.push('-');
                in_gap = false;
            }
            slug.push(ch);
        } else {
            in_gap = true;
        }
    }
    // A leading run only collapses into a leading `-` that the trim removes; a trailing one is never pushed above.
    let slug = slug.trim_matches('-');
    // ASCII only: bytes are UTF-16 units.
    slug[..slug.len().min(60)].to_string()
}

/// `new Date(ms).toISOString()` (`±YYYYYY` outside years 0..=9999).
fn iso_string(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let in_day = ms.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    let year_text = if (0..=9999).contains(&year) {
        format!("{year:04}")
    } else if year < 0 {
        format!("-{:06}", -year)
    } else {
        format!("+{year:06}")
    };
    format!(
        "{year_text}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        in_day / 3_600_000,
        in_day / 60_000 % 60,
        in_day / 1000 % 60,
        in_day % 1000
    )
}

/// Days since 1970-01-01 → proleptic Gregorian (year, month, day) (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The current time in epoch ms (`new Date()`); tests pin it to the fixtures' fixed clock.
fn now_ms() -> i64 {
    #[cfg(test)]
    if let Some(ms) = tests::clock_override() {
        return ms;
    }
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// TS: notesStore.ts noteFileStem (the ID when file-name-safe, else `x-` and 32 hex of its sha256).
pub fn note_file_stem(id: &str) -> String {
    let safe = (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
    if safe {
        return id.to_string();
    }
    let digest = Sha256::digest(id.as_bytes());
    let hex: String = digest.iter().take(16).map(|byte| format!("{byte:02x}")).collect();
    format!("x-{hex}")
}

/// TS: notesStore.ts noteFilePath (`notes/<directory>/<stem>.md`).
pub fn note_file_path(note: &NoteId) -> PathBuf {
    kind_directory(note.kind).join(format!("{}.md", note_file_stem(&note.id)))
}

fn kind_directory(kind: NoteKind) -> PathBuf {
    paths::notes_dir().join(kind.directory())
}

/// TS: notesStore.ts noteStem (`<stem>.md`, not hidden or temporary; `None` otherwise).
pub fn note_stem(name: &str) -> Option<&str> {
    if !name.starts_with('.') && name.len() > 3 {
        name.strip_suffix(".md")
    } else {
        None
    }
}

fn cache_key(kind: NoteKind, stem: &str) -> String {
    format!("{}:{stem}", kind.as_str())
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// TS: notesStore.ts readNoteFile (at most READ_LIMIT bytes; missing: empty; too long: cut, `too_large`).
pub async fn read_note_file(file: &std::path::Path) -> Result<StoredNote> {
    let mut handle = match tokio::fs::File::open(file).await {
        Ok(handle) => handle,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(empty_note()),
        Err(error) => return Err(DhError::io(&error, "open", &display(file))),
    };
    let size = handle.metadata().await?.len();
    let want = usize::try_from(size).unwrap_or(usize::MAX).min(READ_LIMIT);
    let mut buffer = vec![0u8; want];
    let mut read = 0;
    while read < want {
        match handle.read(&mut buffer[read..]).await? {
            0 => break,
            count => read += count,
        }
    }
    buffer.truncate(read);
    let text = String::from_utf8_lossy(&buffer).into_owned();
    let too_large = size > READ_LIMIT as u64 || js_len(&text) > MAX_NOTES_CHARS;
    Ok(if too_large {
        StoredNote {
            text: js_slice(&text, 0, Some(MAX_NOTES_CHARS as i64)),
            revision: format!("{}-{size}", note_revision(&text)),
            too_large: Some(true),
        }
    } else {
        StoredNote {
            revision: note_revision(&text),
            text,
            too_large: None,
        }
    })
}

async fn make_directory(directory: &Path) -> Result<()> {
    tokio::fs::DirBuilder::new()
        .recursive(true)
        .mode(DIR_MODE)
        .create(directory)
        .await
        .map_err(|error| DhError::io(&error, "mkdir", &display(directory)))
}

async fn make_parent(file: &Path) -> Result<()> {
    match file.parent() {
        Some(parent) => make_directory(parent).await,
        None => Ok(()),
    }
}

/// TS: notesStore.ts writeAtomically (`.<name>.tmp-<pid>-<uuid8>` beside the file, then a rename over it).
async fn write_atomically(file: &Path, text: &str) -> Result<()> {
    let name = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let unique = uuid::Uuid::new_v4().simple().to_string();
    let temporary = file.with_file_name(format!(".{name}.tmp-{}-{}", std::process::id(), &unique[..8]));
    let result = async {
        let mut handle = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(FILE_MODE)
            .open(&temporary)
            .await
            .map_err(|error| DhError::io(&error, "open", &display(&temporary)))?;
        tokio::io::AsyncWriteExt::write_all(&mut handle, text.as_bytes()).await?;
        drop(handle);
        tokio::fs::rename(&temporary, file)
            .await
            .map_err(|error| DhError::io_dest(&error, "rename", &display(&temporary), &display(file)))
    }
    .await;
    let _ = tokio::fs::remove_file(&temporary).await;
    result
}

/// `fs.rm(file, {force: true})`.
async fn remove_forced(file: &Path) -> Result<()> {
    match tokio::fs::remove_file(file).await {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(DhError::io(&error, "rm", &display(file))),
        _ => Ok(()),
    }
}

fn same_note(left: Option<&StoredNote>, right: &StoredNote) -> bool {
    let empty_revision;
    let revision = match left {
        Some(note) => note.revision.as_str(),
        None => {
            empty_revision = note_revision("");
            empty_revision.as_str()
        }
    };
    let left_large = left.and_then(|note| note.too_large).unwrap_or(false);
    revision == right.revision && left_large == right.too_large.unwrap_or(false)
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

/// A notify callback's report for one kind's directory, forwarded to the store's local task.
struct WatchMessage {
    kind: NoteKind,
    /// Which watcher of the kind sent it (a retried watch replaces the one that failed).
    generation: u64,
    event: notify::Result<notify::Event>,
}

/// TS: notesStore.ts NotesStore. Clones share the store.
#[derive(Clone)]
pub struct NotesStore {
    inner: Rc<NotesStoreInner>,
}

struct NotesStoreInner {
    /// `<kind>:<stem>` → note; empty notes are not cached.
    cache: RefCell<IndexMap<String, StoredNote>>,
    /// Per note: saves and re-reads run one at a time.
    queues: KeyedQueue<String>,
    /// Watcher debounce per note.
    timers: KeyedTimers<String>,
    /// One directory watcher per kind, with its generation.
    watchers: RefCell<HashMap<NoteKind, (u64, notify::RecommendedWatcher)>>,
    next_generation: Cell<u64>,
    /// Where the watchers send their events (set while watching).
    events: RefCell<Option<mpsc::UnboundedSender<WatchMessage>>>,
    closed: Cell<bool>,
    on_change: NoteChangeHandler,
}

impl NotesStore {
    /// TS: `new NotesStore(onChange)`.
    pub fn new(on_change: impl Fn(NoteKind, &str) + 'static) -> Self {
        NotesStore {
            inner: Rc::new(NotesStoreInner {
                cache: RefCell::new(IndexMap::new()),
                queues: KeyedQueue::new(),
                timers: KeyedTimers::new(),
                watchers: RefCell::new(HashMap::new()),
                next_generation: Cell::new(0),
                events: RefCell::new(None),
                closed: Cell::new(false),
                on_change: Box::new(on_change),
            }),
        }
    }

    fn from_weak(weak: &Weak<NotesStoreInner>) -> Option<Self> {
        weak.upgrade().map(|inner| NotesStore { inner })
    }

    /// TS: NotesStore.ensureDirectories (every kind's directory, 0700).
    pub async fn ensure_directories(&self) -> Result<()> {
        for kind in NoteKind::ALL {
            make_directory(&kind_directory(kind)).await?;
        }
        Ok(())
    }

    /// TS: NotesStore.load (reads every note file into memory; unreadable files are skipped).
    pub async fn load(&self) -> Result<()> {
        self.ensure_directories().await?;
        for kind in NoteKind::ALL {
            let directory = kind_directory(kind);
            for name in read_names(&directory).await {
                let Some(stem) = note_stem(&name) else { continue };
                if let Ok(note) = read_note_file(&directory.join(&name)).await {
                    self.remember(kind, stem, note);
                }
            }
        }
        Ok(())
    }

    /// TS: NotesStore.migrate (`notes` stored in state.json by older versions move into their files; an existing
    /// file wins). `sessions`: each session's ID and its legacy `notes` value (`None`: absent). Returns whether any
    /// session still carried notes (state.json must be written again without them).
    pub async fn migrate(&self, sessions: &[(String, Option<serde_json::Value>)]) -> Result<bool> {
        self.ensure_directories().await?;
        let mut legacy = false;
        for (id, notes) in sessions {
            let Some(notes) = notes else { continue };
            legacy = true;
            let Some(notes) = notes.as_str().filter(|notes| !notes.is_empty()) else {
                continue;
            };
            let file = note_file_path(&NoteId {
                kind: NoteKind::Session,
                id: id.clone(),
            });
            if tokio::fs::metadata(&file).await.is_err() {
                write_atomically(&file, &js_slice(notes, 0, Some(MAX_NOTES_CHARS as i64))).await?;
            }
        }
        Ok(legacy)
    }

    /// TS: NotesStore.get (the cached note, else empty).
    pub fn get(&self, note: &NoteId) -> StoredNote {
        let key = cache_key(note.kind, &note_file_stem(&note.id));
        self.inner.cache.borrow().get(&key).cloned().unwrap_or_else(empty_note)
    }

    /// TS: NotesStore.save: writes `text` (cut to MAX_NOTES_CHARS) when the file still has `revision`; otherwise
    /// nothing is written and the file as it is now is cached. A too-large file is refused (`This note is longer than
    /// 50 000 characters; edit it in your editor (E)`). Queued per note at call time.
    pub fn save(&self, note: &NoteId, text: &str, revision: &str) -> Task<Result<SaveOutcome>> {
        let store = self.clone();
        let note = note.clone();
        let text = text.to_string();
        let revision = revision.to_string();
        let stem = note_file_stem(&note.id);
        self.inner.queues.run(cache_key(note.kind, &stem), move || async move {
            let file = note_file_path(&note);
            let current = read_note_file(&file).await?;
            let changed = store.remember(note.kind, &stem, current.clone());
            if current.too_large == Some(true) {
                return Err(DhError::msg(format!(
                    "This note is longer than {MAX_NOTES_LABEL}; edit it in your editor (E)"
                )));
            }
            if current.revision != revision {
                return Ok(SaveOutcome { saved: false, changed });
            }
            let next = js_slice(&text, 0, Some(MAX_NOTES_CHARS as i64));
            if next == current.text {
                return Ok(SaveOutcome { saved: true, changed });
            }
            make_parent(&file).await?;
            write_atomically(&file, &next).await?;
            let revision = note_revision(&next);
            store.remember(
                note.kind,
                &stem,
                StoredNote {
                    text: next,
                    revision,
                    too_large: None,
                },
            );
            Ok(SaveOutcome {
                saved: true,
                changed: true,
            })
        })
    }

    /// TS: NotesStore.modify: rewrites the note with `transform` of its text as it is on disk now; the transform's
    /// error is returned; nothing is written when it returns the same text; a result longer than MAX_NOTES_CHARS is
    /// refused. Queued per note at call time.
    pub fn modify<T, F>(&self, note: &NoteId, transform: F) -> Task<Result<ModifyOutcome<T>>>
    where
        T: 'static,
        F: FnOnce(&str) -> Result<(String, T)> + 'static,
    {
        let store = self.clone();
        let note = note.clone();
        let stem = note_file_stem(&note.id);
        self.inner.queues.run(cache_key(note.kind, &stem), move || async move {
            let file = note_file_path(&note);
            let current = read_note_file(&file).await?;
            let changed = store.remember(note.kind, &stem, current.clone());
            if current.too_large == Some(true) {
                return Err(DhError::msg(format!(
                    "This file is longer than {MAX_NOTES_LABEL}; edit it in your editor"
                )));
            }
            let (text, result) = transform(&current.text)?;
            if text == current.text {
                return Ok(ModifyOutcome {
                    result,
                    note: current,
                    changed,
                });
            }
            if js_len(&text) > MAX_NOTES_CHARS {
                return Err(DhError::msg(format!(
                    "That would make the file longer than {MAX_NOTES_LABEL}"
                )));
            }
            make_parent(&file).await?;
            write_atomically(&file, &text).await?;
            let written = StoredNote {
                revision: note_revision(&text),
                text,
                too_large: None,
            };
            store.remember(note.kind, &stem, written.clone());
            Ok(ModifyOutcome {
                result,
                note: written,
                changed: true,
            })
        })
    }

    /// TS: NotesStore.entries (the cached notes of a kind: file stems and contents).
    pub fn entries(&self, kind: NoteKind) -> Vec<(String, StoredNote)> {
        let prefix = format!("{}:", kind.as_str());
        self.inner
            .cache
            .borrow()
            .iter()
            .filter_map(|(key, note)| key.strip_prefix(&prefix).map(|stem| (stem.to_string(), note.clone())))
            .collect()
    }

    /// TS: NotesStore.ensureFile (the note's file, created empty when missing, e.g. to open it in an editor).
    pub async fn ensure_file(&self, note: &NoteId) -> Result<PathBuf> {
        let file = note_file_path(note);
        make_parent(&file).await?;
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(FILE_MODE)
            .open(&file)
            .await
        {
            Err(error) if error.kind() != io::ErrorKind::AlreadyExists => {
                Err(DhError::io(&error, "open", &display(&file)))
            }
            _ => Ok(file),
        }
    }

    /// TS: NotesStore.remove: a note with text moves to `notes/trash/` (see [`trash_file_name`]); an empty one is
    /// deleted. `label`: what it belonged to. Queued per note at call time.
    pub fn remove(&self, note: &NoteId, label: Option<&str>) -> Task<Result<()>> {
        let store = self.clone();
        let note = note.clone();
        let label = label.map(str::to_string);
        let stem = note_file_stem(&note.id);
        self.inner.queues.run(cache_key(note.kind, &stem), move || async move {
            store
                .inner
                .cache
                .borrow_mut()
                .shift_remove(&cache_key(note.kind, &stem));
            let file = note_file_path(&note);
            let text = match tokio::fs::read(&file).await {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(_) => String::new(),
            };
            if js_trim(&text).is_empty() {
                return remove_forced(&file).await;
            }
            let trash = paths::notes_dir().join(TRASH_DIRECTORY);
            make_directory(&trash).await?;
            let target = trash.join(trash_file_name(note.kind, label.as_deref(), &stem, now_ms()));
            if tokio::fs::rename(&file, &target).await.is_err() {
                tokio::fs::copy(&file, &target)
                    .await
                    .map_err(|error| DhError::io_dest(&error, "copyfile", &display(&file), &display(&target)))?;
                remove_forced(&file).await?;
            }
            Ok(())
        })
    }

    /// TS: NotesStore.watch (watches every kind's directory; a changed file is re-read after WATCH_DEBOUNCE; a
    /// failed watch is retried after WATCH_RETRY).
    pub fn watch(&self) {
        for kind in NoteKind::ALL {
            self.watch_directory(kind);
        }
    }

    /// TS: NotesStore.close (stops the watchers and pending re-reads).
    pub fn close(&self) {
        self.inner.closed.set(true);
        let watchers: Vec<_> = self.inner.watchers.borrow_mut().drain().collect();
        drop(watchers);
        let sender = self.inner.events.borrow_mut().take();
        drop(sender);
        self.inner.timers.clear_all();
    }

    /// The sender the watchers report to, starting the local task that handles their events on first use.
    fn event_sender(&self) -> mpsc::UnboundedSender<WatchMessage> {
        if let Some(sender) = self.inner.events.borrow().as_ref() {
            return sender.clone();
        }
        let (sender, mut receiver) = mpsc::unbounded_channel::<WatchMessage>();
        *self.inner.events.borrow_mut() = Some(sender.clone());
        let weak = Rc::downgrade(&self.inner);
        drop(spawn(async move {
            while let Some(message) = receiver.recv().await {
                let Some(store) = NotesStore::from_weak(&weak) else {
                    break;
                };
                store.handle_watch(message);
            }
        }));
        sender
    }

    /// TS: the `watch(directory, ...)` listener and its `error` handler.
    fn handle_watch(&self, message: WatchMessage) {
        if self.inner.closed.get() {
            return;
        }
        let event = match message.event {
            Ok(event) => event,
            Err(_) => return self.retry_watch(message.kind, Some(message.generation)),
        };
        // fs.watch reports changes, not reads (inotify's open/close events would re-read forever).
        if matches!(event.kind, notify::EventKind::Access(_)) {
            return;
        }
        if event.need_rescan() || event.paths.is_empty() {
            self.spawn_rescan(message.kind);
            return;
        }
        for path in &event.paths {
            let name = path.file_name().map(|name| name.to_string_lossy().into_owned());
            if let Some(stem) = name.as_deref().and_then(note_stem) {
                self.schedule(message.kind, stem);
            }
        }
    }

    fn spawn_rescan(&self, kind: NoteKind) {
        let store = self.clone();
        drop(spawn(async move { store.rescan(kind).await }));
    }

    /// TS: watchDirectory's `retry`: drops the kind's watcher and watches again after WATCH_RETRY (creating the
    /// directory first). `generation`: the failed watcher (`None`: creating the watch failed).
    fn retry_watch(&self, kind: NoteKind, generation: Option<u64>) {
        let removed = {
            let mut watchers = self.inner.watchers.borrow_mut();
            match (generation, watchers.get(&kind)) {
                (Some(failed), Some((current, _))) if *current != failed => None,
                _ => watchers.remove(&kind),
            }
        };
        if generation.is_some() && removed.is_none() {
            // A stale report from a watcher that was already replaced.
            return;
        }
        drop(removed);
        if self.inner.closed.get() {
            return;
        }
        let weak = Rc::downgrade(&self.inner);
        let directory = kind_directory(kind);
        Timer::after(WATCH_RETRY, move || {
            drop(spawn(async move {
                if make_directory(&directory).await.is_ok()
                    && let Some(store) = NotesStore::from_weak(&weak)
                {
                    store.watch_directory(kind);
                }
            }));
        })
        .detach();
    }

    /// TS: NotesStore.watchDirectory.
    fn watch_directory(&self, kind: NoteKind) {
        if self.inner.closed.get() || self.inner.watchers.borrow().contains_key(&kind) {
            return;
        }
        let directory = kind_directory(kind);
        let generation = self.inner.next_generation.get();
        self.inner.next_generation.set(generation.wrapping_add(1));
        let sender = self.event_sender();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let _ = sender.send(WatchMessage {
                kind,
                generation,
                event,
            });
        })
        .and_then(|mut watcher| {
            watcher.watch(&directory, notify::RecursiveMode::NonRecursive)?;
            Ok(watcher)
        });
        match watcher {
            Ok(watcher) => {
                self.inner.watchers.borrow_mut().insert(kind, (generation, watcher));
            }
            Err(_) => self.retry_watch(kind, None),
        }
    }

    /// TS: NotesStore.schedule (debounced re-read of one note).
    fn schedule(&self, kind: NoteKind, stem: &str) {
        let weak = Rc::downgrade(&self.inner);
        let owned = stem.to_string();
        self.inner
            .timers
            .schedule(cache_key(kind, stem), WATCH_DEBOUNCE, move || {
                if let Some(store) = NotesStore::from_weak(&weak) {
                    drop(store.refresh(kind, &owned));
                }
            });
    }

    /// TS: NotesStore.rescan (no file name from the watcher: every file of the directory and every cached note).
    async fn rescan(&self, kind: NoteKind) {
        let names = read_names(&kind_directory(kind)).await;
        let mut stems: Vec<String> = Vec::new();
        let mut add = |stem: &str| {
            if !stems.iter().any(|known| known == stem) {
                stems.push(stem.to_string());
            }
        };
        for name in &names {
            if let Some(stem) = note_stem(name) {
                add(stem);
            }
        }
        let prefix = format!("{}:", kind.as_str());
        let cached: Vec<String> = self
            .inner
            .cache
            .borrow()
            .keys()
            .filter_map(|key| key.strip_prefix(&prefix).map(str::to_string))
            .collect();
        for stem in &cached {
            add(stem);
        }
        for stem in &stems {
            self.schedule(kind, stem);
        }
    }

    /// TS: NotesStore.refresh (re-reads one note; tells `on_change` when what the sessions show changed).
    fn refresh(&self, kind: NoteKind, stem: &str) -> Task<()> {
        let store = self.clone();
        let stem = stem.to_string();
        self.inner.queues.run(cache_key(kind, &stem), move || async move {
            if store.inner.closed.get() {
                return;
            }
            let file = kind_directory(kind).join(format!("{stem}.md"));
            let Ok(current) = read_note_file(&file).await else {
                return;
            };
            if store.remember(kind, &stem, current) {
                (store.inner.on_change)(kind, &stem);
            }
        })
    }

    /// TS: NotesStore.remember (caches a note as read or written; true when that changed what the sessions show).
    fn remember(&self, kind: NoteKind, stem: &str, note: StoredNote) -> bool {
        let key = cache_key(kind, stem);
        let mut cache = self.inner.cache.borrow_mut();
        let changed = !same_note(cache.get(&key), &note);
        if note.revision == note_revision("") && note.too_large != Some(true) {
            cache.shift_remove(&key);
        } else {
            cache.insert(key, note);
        }
        changed
    }
}

/// `fs.readdir(directory).catch(() => [])` (sorted: the order the files are cached in does not depend on the file
/// system).
async fn read_names(directory: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(directory).await else {
        return names;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    names
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::Cell;
    use std::path::Path;
    use std::sync::{Mutex, MutexGuard};

    use deckhand_core::jsstr::js_to_upper;
    use deckhand_parity::{Case, DirOp, replay, replay_dir};
    use serde_json::{Value, json};

    use super::*;

    const UNIT: &str = "I2";
    /// The fixtures' fixed clock (`2026-01-01T00:00:00.000Z`).
    const FIXED_CLOCK_MS: i64 = 1_767_225_600_000;

    thread_local! {
        static CLOCK: Cell<Option<i64>> = const { Cell::new(None) };
    }

    pub(super) fn clock_override() -> Option<i64> {
        CLOCK.with(Cell::get)
    }

    /// Tests that set DECKHAND_HOME run one at a time.
    static ENV: Mutex<()> = Mutex::new(());

    pub(crate) struct Home {
        _guard: MutexGuard<'static, ()>,
        previous: Option<std::ffi::OsString>,
    }

    impl Home {
        pub(crate) fn set(home: &Path) -> Home {
            let guard = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let previous = std::env::var_os(paths::HOME_ENV);
            // SAFETY: tests touching the environment hold ENV; nothing else in this crate's tests reads it
            // concurrently.
            unsafe { std::env::set_var(paths::HOME_ENV, home) };
            Home {
                _guard: guard,
                previous,
            }
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            // SAFETY: as in Home::set (ENV is still held).
            unsafe {
                match self.previous.take() {
                    Some(value) => std::env::set_var(paths::HOME_ENV, value),
                    None => std::env::remove_var(paths::HOME_ENV),
                }
            }
        }
    }

    /// Runs `future` on a fresh current_thread runtime inside a LocalSet.
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        tokio::task::LocalSet::new().block_on(&runtime, future)
    }

    /// A recorded string argument (a `{"$utf16":[..]}` one decoded lossily, as Node encodes it to UTF-8).
    fn js_string(value: &Value) -> Result<String, String> {
        if let Some(text) = value.as_str() {
            return Ok(text.to_string());
        }
        let units: Vec<u16> = value
            .get("$utf16")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("expected a string, got {value}"))?
            .iter()
            .map(|unit| {
                unit.as_u64()
                    .and_then(|unit| u16::try_from(unit).ok())
                    .unwrap_or(0xFFFD)
            })
            .collect();
        Ok(String::from_utf16_lossy(&units))
    }

    /// `{"$date":"<ISO>"}` → epoch ms.
    fn date_ms(value: &Value) -> Result<i64, String> {
        let iso = value
            .get("$date")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("expected a date, got {value}"))?;
        parse_iso(iso).ok_or_else(|| format!("bad date {iso}"))
    }

    fn parse_iso(iso: &str) -> Option<i64> {
        let (date, time) = iso.strip_suffix('Z')?.split_once('T')?;
        let (sign, date) = match date.as_bytes().first()? {
            b'+' => (1, &date[1..]),
            b'-' => (-1, &date[1..]),
            _ => (1, date),
        };
        let mut parts = date.split('-');
        let year: i64 = sign * parts.next()?.parse::<i64>().ok()?;
        let month: i64 = parts.next()?.parse().ok()?;
        let day: i64 = parts.next()?.parse().ok()?;
        let (clock, millis) = time.split_once('.').unwrap_or((time, "0"));
        let mut clock = clock.split(':');
        let hours: i64 = clock.next()?.parse().ok()?;
        let minutes: i64 = clock.next()?.parse().ok()?;
        let seconds: i64 = clock.next()?.parse().ok()?;
        let millis: i64 = millis.parse().ok()?;
        // Days from civil (Howard Hinnant).
        let y = if month <= 2 { year - 1 } else { year };
        let era = y.div_euclid(400);
        let yoe = y.rem_euclid(400);
        let mp = (month + 9) % 12;
        let doy = (153 * mp + 2) / 5 + day - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        Some(((days * 24 + hours) * 60 + minutes) * 60_000 + seconds * 1000 + millis)
    }

    fn note_arg(case: &Case, index: usize) -> Result<NoteId, String> {
        case.arg_as::<NoteId>(index)
    }

    fn sorted_entries(store: &NotesStore, kind: NoteKind) -> Value {
        let mut entries = store.entries(kind);
        entries.sort_by(|(a, _), (b, _)| a.encode_utf16().cmp(b.encode_utf16()));
        json!(entries)
    }

    #[test]
    fn iso_strings_match_date_to_iso_string() {
        assert_eq!(iso_string(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_string(FIXED_CLOCK_MS), "2026-01-01T00:00:00.000Z");
        for iso in [
            "2026-10-09T14:02:11.500Z",
            "1999-12-31T23:59:59.999Z",
            "0099-01-01T00:00:00.000Z",
            "+012345-06-07T08:09:10.000Z",
            "-000001-12-31T23:59:59.999Z",
        ] {
            let ms = parse_iso(iso).expect(iso);
            assert_eq!(iso_string(ms), iso);
        }
    }

    // TS: tests/notes.test.ts "a removed note goes to the trash under a name that says when and what it belonged
    // to" (deferred to I2 by P3).
    #[test]
    fn a_removed_note_goes_to_the_trash_under_a_name_that_says_when_and_what_it_belonged_to() {
        let at = |iso| parse_iso(iso).expect("date");
        assert_eq!(
            trash_file_name(
                NoteKind::Session,
                Some("school-doc-upload / prompt-audit"),
                "af7fc4e8",
                at("2026-10-09T14:02:11.500Z")
            ),
            "2026-10-09T14-02-11_session_school-doc-upload-prompt-audit_af7fc4e8.md"
        );
        assert_eq!(
            trash_file_name(NoteKind::Worktree, None, "w1", at("2026-10-09T14:02:11.000Z")),
            "2026-10-09T14-02-11_worktree_note_w1.md"
        );
    }

    #[test]
    fn trash_file_name_parity() {
        replay(
            UNIT,
            "notesStore/trashFileName",
            |case: &Case| -> Result<String, String> {
                let kind = js_string(case.arg(0))?;
                let label = if case.arg_is_undefined(1) {
                    None
                } else {
                    Some(js_string(case.arg(1))?)
                };
                let stem = js_string(case.arg(2))?;
                let now = if case.arg_is_undefined(3) {
                    FIXED_CLOCK_MS
                } else {
                    date_ms(case.arg(3))?
                };
                Ok(format_trash_file_name(&kind, label.as_deref(), &stem, now))
            },
        );
    }

    #[test]
    fn note_file_stem_parity() {
        replay(
            UNIT,
            "notesStore/noteFileStem",
            |case: &Case| -> Result<String, String> { Ok(note_file_stem(&js_string(case.arg(0))?)) },
        );
    }

    #[test]
    fn note_file_path_parity() {
        let _home = Home::set(Path::new("/parity-home"));
        replay(
            UNIT,
            "notesStore/noteFilePath",
            |case: &Case| -> Result<String, String> {
                let arg = case.arg(0);
                let kind: NoteKind = serde_json::from_value(arg.get("kind").cloned().unwrap_or(Value::Null))
                    .map_err(|error| error.to_string())?;
                let id = js_string(arg.get("id").unwrap_or(&Value::Null))?;
                Ok(display(&note_file_path(&NoteId { kind, id })))
            },
        );
    }

    #[test]
    fn note_stems_of_directory_entries() {
        assert_eq!(note_stem("a.md"), Some("a"));
        assert_eq!(note_stem(".md"), None);
        assert_eq!(note_stem(".a.md.tmp-1-abcd"), None);
        assert_eq!(note_stem(".hidden.md"), None);
        assert_eq!(note_stem("notes.txt"), None);
        assert_eq!(note_stem("x.md.md"), Some("x.md"));
    }

    /// The transforms the fixtures registered (`registerFn`).
    fn transform(name: &str, text: &str) -> Result<(String, Value)> {
        Ok(match name {
            "append-line" => (format!("{text}more\n"), json!(js_len(text))),
            "identity" => (text.to_string(), json!("same")),
            "make-huge" => (format!("{text}{}", "x".repeat(50_000)), Value::Null),
            "throw-boom" => return Err(DhError::msg("boom")),
            "uppercase" => (js_to_upper(text), json!([js_len(text), text.split('\n').count()])),
            "clear" => (String::new(), json!("cleared")),
            other => return Err(DhError::msg(format!("unknown transform {other}"))),
        })
    }

    /// `cold`: the recording skipped `load()` before the save (saveCold).
    async fn run_op(op: &DirOp, cold: bool) -> Result<Value, String> {
        let store = NotesStore::new(|_, _| {});
        let fail = |error: DhError| error.to_string();
        match op.fn_name.as_str() {
            "NotesStore.save" => {
                let note = note_arg(op, 1)?;
                if !cold {
                    store.load().await.map_err(fail)?;
                }
                let outcome = store.save(&note, op.arg_str(2)?, op.arg_str(3)?).await.map_err(fail)?;
                Ok(json!({"saved": outcome.saved, "changed": outcome.changed, "after": store.get(&note)}))
            }
            "NotesStore.modify" => {
                let note = note_arg(op, 1)?;
                let name = op.fn_name(2).ok_or("no transform")?.to_string();
                let outcome = store
                    .modify(&note, move |text| transform(&name, text))
                    .await
                    .map_err(fail)?;
                Ok(
                    json!({"result": outcome.result, "note": outcome.note, "changed": outcome.changed,
                    "get": store.get(&note)}),
                )
            }
            "NotesStore.remove" => {
                let note = note_arg(op, 1)?;
                let label = if op.arg_is_undefined(2) {
                    None
                } else {
                    Some(op.arg_str(2)?.to_string())
                };
                store.load().await.map_err(fail)?;
                store.remove(&note, label.as_deref()).await.map_err(fail)?;
                Ok(json!({"get": store.get(&note), "entries": store.entries(note.kind)}))
            }
            "NotesStore.migrate" => {
                let sessions: Vec<(String, Option<Value>)> = op
                    .arg(1)
                    .as_array()
                    .ok_or("sessions: expected an array")?
                    .iter()
                    .map(|session| {
                        let id = session
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        (id, session.get("notes").cloned())
                    })
                    .collect();
                Ok(json!(store.migrate(&sessions).await.map_err(fail)?))
            }
            "NotesStore.ensureFile" => {
                let note = note_arg(op, 1)?;
                Ok(json!(display(&store.ensure_file(&note).await.map_err(fail)?)))
            }
            "NotesStore.load" => {
                store.load().await.map_err(fail)?;
                let mut out = serde_json::Map::new();
                for kind in NoteKind::ALL {
                    out.insert(kind.as_str().to_string(), sorted_entries(&store, kind));
                }
                Ok(Value::Object(out))
            }
            other => Err(format!("unknown op {other}")),
        }
    }

    fn replay_store_case(name: &str) {
        replay_dir(UNIT, &format!("notesStore/{name}"), |root: &Path, op: &DirOp| {
            let _home = Home::set(root);
            CLOCK.with(|clock| clock.set(Some(FIXED_CLOCK_MS)));
            let outcome = block_on(run_op(op, name.ends_with("-cold-cache")));
            CLOCK.with(|clock| clock.set(None));
            outcome
        });
    }

    macro_rules! store_cases {
        ($($test:ident => $case:literal,)*) => {
            $(
                #[test]
                fn $test() {
                    replay_store_case($case);
                }
            )*
        };
    }

    store_cases! {
        ensure_file_creates_empty => "ensure-file-creates-empty",
        ensure_file_hashed_id => "ensure-file-hashed-id",
        ensure_file_keeps_existing => "ensure-file-keeps-existing",
        load_empty_home => "load-empty-home",
        load_legacy_home_v45 => "load-legacy-home-v45",
        load_mixed_files => "load-mixed-files",
        migrate_empty_and_nonstring => "migrate-empty-and-nonstring",
        migrate_empty_list => "migrate-empty-list",
        migrate_existing_file_wins => "migrate-existing-file-wins",
        migrate_no_notes_property => "migrate-no-notes-property",
        migrate_truncates => "migrate-truncates",
        migrate_writes_legacy_notes => "migrate-writes-legacy-notes",
        modify_append_existing => "modify-append-existing",
        modify_append_new_note => "modify-append-new-note",
        modify_existing_too_large_throws => "modify-existing-too-large-throws",
        modify_identity_missing_file => "modify-identity-missing-file",
        modify_identity_writes_nothing => "modify-identity-writes-nothing",
        modify_to_empty => "modify-to-empty",
        modify_too_long_result_throws => "modify-too-long-result-throws",
        modify_transform_throws => "modify-transform-throws",
        modify_unicode => "modify-unicode",
        remove_empty_deletes => "remove-empty-deletes",
        remove_hashed_id => "remove-hashed-id",
        remove_leaves_other_notes => "remove-leaves-other-notes",
        remove_missing_file => "remove-missing-file",
        remove_unicode_label => "remove-unicode-label",
        remove_whitespace_only_deletes => "remove-whitespace-only-deletes",
        remove_with_text_to_trash => "remove-with-text-to-trash",
        remove_without_label => "remove-without-label",
        save_exactly_max_chars => "save-exactly-max-chars",
        save_existing_too_large_throws => "save-existing-too-large-throws",
        save_hashed_id_update => "save-hashed-id-update",
        save_invalid_utf8_on_disk => "save-invalid-utf8-on-disk",
        save_matching_revision => "save-matching-revision",
        save_new_file => "save-new-file",
        save_new_file_wrong_revision => "save-new-file-wrong-revision",
        save_same_text => "save-same-text",
        save_same_text_cold_cache => "save-same-text-cold-cache",
        save_stale_revision => "save-stale-revision",
        save_to_empty => "save-to-empty",
        save_truncates_long_text => "save-truncates-long-text",
        save_truncates_surrogate_boundary => "save-truncates-surrogate-boundary",
        save_unicode_and_hashed_id => "save-unicode-and-hashed-id",
    }

    #[test]
    fn saves_of_one_note_run_in_call_order() {
        let root = tempfile::tempdir().expect("tempdir");
        let _home = Home::set(root.path());
        block_on(async {
            let store = NotesStore::new(|_, _| {});
            let note = NoteId {
                kind: NoteKind::Session,
                id: "s1".into(),
            };
            let first = store.save(&note, "one", &note_revision(""));
            let second = store.save(&note, "two", &note_revision("one"));
            let stale = store.save(&note, "three", &note_revision(""));
            assert_eq!(
                first.await,
                Ok(SaveOutcome {
                    saved: true,
                    changed: true
                })
            );
            assert_eq!(
                second.await,
                Ok(SaveOutcome {
                    saved: true,
                    changed: true
                })
            );
            assert_eq!(
                stale.await,
                Ok(SaveOutcome {
                    saved: false,
                    changed: false
                })
            );
            assert_eq!(store.get(&note).text, "two");
        });
    }

    #[test]
    fn an_edit_made_in_an_editor_is_picked_up_by_the_watcher() {
        let root = tempfile::tempdir().expect("tempdir");
        let _home = Home::set(root.path());
        block_on(async {
            let heard: Rc<RefCell<Vec<(NoteKind, String)>>> = Rc::default();
            let sink = Rc::clone(&heard);
            let store = NotesStore::new(move |kind, stem| sink.borrow_mut().push((kind, stem.to_string())));
            store.load().await.expect("load");
            store.watch();
            let note = NoteId {
                kind: NoteKind::Tasks,
                id: "r1".into(),
            };
            let file = note_file_path(&note);
            // Let the watchers settle before the edit (FSEvents may drop events from before the stream started).
            tokio::time::sleep(Duration::from_millis(300)).await;
            std::fs::write(&file, "- [ ] edited\n").expect("write");
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while store.get(&note).text != "- [ ] edited\n" && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert_eq!(store.get(&note).text, "- [ ] edited\n");
            assert!(heard.borrow().contains(&(NoteKind::Tasks, "r1".to_string())));
            // A save of its own is not reported as an outside change.
            heard.borrow_mut().clear();
            let revision = store.get(&note).revision;
            store.save(&note, "mine\n", &revision).await.expect("save");
            tokio::time::sleep(WATCH_DEBOUNCE * 4).await;
            assert!(heard.borrow().is_empty());
            // A deletion empties the note.
            std::fs::remove_file(&file).expect("remove");
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while !store.entries(NoteKind::Tasks).is_empty() && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert_eq!(store.get(&note), empty_note());
            assert!(heard.borrow().contains(&(NoteKind::Tasks, "r1".to_string())));
            store.close();
            assert!(store.inner.watchers.borrow().is_empty());
        });
    }
}
