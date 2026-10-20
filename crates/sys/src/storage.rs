//! Port of storage.ts.
//!
//! state.json (sessions and worktree records) and config.json (the user config) under the Deckhand home. Both are
//! written atomically (a temporary file in the same directory, then a rename) with mode 0600, and the home directory
//! is kept 0700. config.json is shared by the UI and daemon processes: [`update_app_config`] serializes its
//! read-modify-write in-process (a queue) and across processes (an exclusive lockfile).
//!
//! The record types are deckhand-protocol's: `InkState` is [`StateFile`], `AppConfig` is [`AppConfig`] (its
//! normalisation, `normalizeAppConfig`, is [`AppConfig::from_raw`]).

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::async_util::{KeyedQueue, Task};
use crate::paths;
use deckhand_core::error::{DhError, Result};
use deckhand_core::jsstr::{self, js_trim, locale_compare_sign};
use deckhand_core::session_order::SessionLike;
use deckhand_core::worktree_records::{migrate_done_markers, migrate_worktree_records};
use deckhand_protocol::Ordered;
use deckhand_protocol::data::{AttentionState, ExitReason, SessionAttention, SessionRecord, SessionStatus, SetupState};
use deckhand_protocol::order::KeyOrder;
use deckhand_protocol::records::{AppConfig, RawConfig, StateFile, WorktreeRecord, to_ts_file_text};

/// TS: storage.ts LOCK_STALE_MS (a config lock older than this was left by a crashed writer).
pub const LOCK_STALE: Duration = Duration::from_millis(5000);
/// TS: storage.ts LOCK_TIMEOUT_MS (how long a writer waits for the config lock).
pub const LOCK_TIMEOUT: Duration = Duration::from_millis(10_000);
/// Mode of every file Deckhand writes under its home.
pub const FILE_MODE: u32 = 0o600;
/// Mode of the Deckhand home and the directories Deckhand creates in it.
pub const DIR_MODE: u32 = 0o700;

/// The directory ensure_config_dir last checked (TS `privateDir`): it is tightened once per process.
static PRIVATE_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

thread_local! {
    /// TS `configQueue`: config.json updates of this process (one LocalSet per thread) run one at a time.
    static CONFIG_QUEUE: KeyedQueue<()> = KeyedQueue::new();
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `Date.now()`; tests pin it to the fixtures' fixed clock.
fn now_ms() -> i64 {
    #[cfg(test)]
    if let Some(ms) = tests::clock_override() {
        return ms;
    }
    jsstr::date::now_ms()
}

/// `randomUUID()`; tests replay the fixtures' seeded sequence.
fn random_uuid() -> String {
    #[cfg(test)]
    if let Some(id) = tests::uuid_override() {
        return id;
    }
    uuid::Uuid::new_v4().to_string()
}

/// The error `fs.readFile` rejects with: open errors name the path; reading a directory does not.
fn read_error(error: &std::io::Error, path: &Path) -> DhError {
    if error.raw_os_error() == Some(libc::EISDIR) {
        return DhError::with_code("EISDIR", "EISDIR: illegal operation on a directory, read");
    }
    DhError::io(error, "open", &display(path))
}

/// `fs.readFile(path, 'utf8')`: invalid UTF-8 is replaced, as Node decodes it.
pub(crate) async fn read_text(path: &Path) -> std::result::Result<String, std::io::Error> {
    let bytes = tokio::fs::read(path).await?;
    Ok(match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
    })
}

/// What `JSON.parse` gives for the numbers serde_json reads as floats: an integral value is a plain integer to
/// `JSON.stringify` (`1.0` and `1e2` are written back as `1` and `100`).
pub(crate) fn js_numbers(value: &mut Value) {
    match value {
        Value::Number(number) => {
            if let Some(float) = number.as_f64().filter(|_| number.is_f64())
                && float.is_finite()
                && float.fract() == 0.0
                && float.abs() < 9.0e18
            {
                #[allow(clippy::cast_possible_truncation)]
                let integer = float as i64;
                *number = serde_json::Number::from(integer);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(js_numbers),
        Value::Object(map) => map.values_mut().for_each(js_numbers),
        _ => {}
    }
}

/// The four hex digits of a `\uXXXX` escape whose `u` is at `at`, as a UTF-16 code unit.
fn escape_unit(bytes: &[u8], at: usize) -> Option<u16> {
    let hex = bytes.get(at + 1..at + 5)?;
    if bytes.get(at) != Some(&b'u') || !hex.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    u16::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()
}

/// `text` with every `\uXXXX` escape of a lone UTF-16 surrogate (inside a JSON string) replaced by `�`.
/// JSON.parse accepts those (JSON.stringify writes one for a string a slice cut inside a surrogate pair) and keeps the
/// half; serde_json rejects them, and a Rust string cannot hold the half, so it becomes U+FFFD (one UTF-16 unit, like
/// the half: deckhand_core::jsstr's rule). Anything malformed is left for serde_json to reject.
fn replace_lone_surrogate_escapes(text: &str) -> std::borrow::Cow<'_, str> {
    let bytes = text.as_bytes();
    let mut out: Option<String> = None;
    let mut copied = 0;
    let mut in_string = false;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => in_string = !in_string,
            b'\\' if in_string => {
                let Some(unit) = escape_unit(bytes, index + 1) else {
                    // Any other escape (`\\` and `\"` included) is two bytes.
                    index += 2;
                    continue;
                };
                let paired = (0xD800..=0xDBFF).contains(&unit)
                    && bytes.get(index + 6) == Some(&b'\\')
                    && escape_unit(bytes, index + 7).is_some_and(|low| (0xDC00..=0xDFFF).contains(&low));
                if paired {
                    index += 12;
                    continue;
                }
                if (0xD800..=0xDFFF).contains(&unit) {
                    let out = out.get_or_insert_with(|| String::with_capacity(text.len()));
                    out.push_str(&text[copied..index]);
                    out.push_str("\\ufffd");
                    copied = index + 6;
                }
                index += 6;
                continue;
            }
            _ => {}
        }
        index += 1;
    }
    match out {
        Some(mut out) => {
            out.push_str(&text[copied..]);
            std::borrow::Cow::Owned(out)
        }
        None => std::borrow::Cow::Borrowed(text),
    }
}

/// `JSON.parse(text)` (with JS numbers), or `None` where it throws a SyntaxError. A lone surrogate escape reads as
/// U+FFFD (see [`replace_lone_surrogate_escapes`]).
pub(crate) fn parse_json(text: &str) -> Option<Value> {
    let mut value: Value = serde_json::from_str(&replace_lone_surrogate_escapes(text)).ok()?;
    js_numbers(&mut value);
    Some(value)
}

/// TS: storage.ts ensureConfigDir (creates the home 0700; tightens a group/world-accessible one we own, once per
/// process and directory).
pub async fn ensure_config_dir() -> Result<()> {
    let dir = paths::config_dir();
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(true).mode(DIR_MODE);
    builder
        .create(&dir)
        .await
        .map_err(|error| DhError::io(&error, "mkdir", &display(&dir)))?;
    let checked = PRIVATE_DIR
        .lock()
        .map(|private| private.as_ref() == Some(&dir))
        .unwrap_or(false);
    if checked {
        return Ok(());
    }
    // Tighten directories created by older versions or a permissive umask, but never someone else's.
    let stat = tokio::fs::metadata(&dir)
        .await
        .map_err(|error| DhError::io(&error, "stat", &display(&dir)))?;
    if stat.mode() & 0o077 != 0 && stat.uid() == nix::unistd::getuid().as_raw() {
        tokio::fs::set_permissions(&dir, std::fs::Permissions::from_mode(DIR_MODE))
            .await
            .map_err(|error| DhError::io(&error, "chmod", &display(&dir)))?;
    }
    if let Ok(mut private) = PRIVATE_DIR.lock() {
        *private = Some(dir);
    }
    Ok(())
}

/// TS: storage.ts `${path}.tmp-${pid}-${Date.now()}-${randomUUID()}`: a unique temporary name beside `path`.
pub fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".tmp-{}-{}-{}", std::process::id(), now_ms(), random_uuid()));
    PathBuf::from(name)
}

/// Writes `contents` to `path` atomically: a temporary file beside it (created with `mode`), then a rename over it;
/// the temporary file is removed when anything fails (storage.ts saveState/writeRawConfig, notesStore.ts
/// writeAtomically and the other temp + rename writers).
pub async fn write_atomic(path: &Path, contents: &[u8], mode: u32) -> Result<()> {
    let temporary = temporary_path(path);
    let result = async {
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(mode)
            .open(&temporary)
            .await
            .map_err(|error| DhError::io(&error, "open", &display(&temporary)))?;
        // flush waits for the blocking write and surfaces its error, as fs.writeFile rejects before the rename.
        tokio::io::AsyncWriteExt::write_all(&mut file, contents)
            .await
            .map_err(|error| DhError::io(&error, "write", &display(&temporary)))?;
        tokio::io::AsyncWriteExt::flush(&mut file)
            .await
            .map_err(|error| DhError::io(&error, "write", &display(&temporary)))?;
        drop(file);
        tokio::fs::rename(&temporary, path)
            .await
            .map_err(|error| DhError::io_dest(&error, "rename", &display(&temporary), &display(path)))
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    result
}

/// TS: storage.ts loadState.
pub async fn load_state() -> Result<StateFile> {
    Ok(read_state().await?.state)
}

/// TS: storage.ts readState result: the state, and whether it was migrated (written by a version before worktree
/// records or done markers) and so must be written back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadState {
    pub state: StateFile,
    pub migrated: bool,
}

/// Entries of state.json's arrays this version cannot read as records, each with its index in the file. TS keeps any
/// value: a newer version's record (a new agent or status), a hand edit or an off-type field. Rust's typed records
/// cannot hold them, so they are not loaded (the daemon does not show them) but are kept as they are and written back
/// by every save ([`save_state`]), recovered on daemon start like any session; they are never erased.
#[derive(Clone, Debug, Default, PartialEq)]
struct Unreadable {
    sessions: Vec<(usize, Value)>,
    worktrees: Vec<(usize, Value)>,
}

/// The unreadable entries of the state.json last read, and its path.
static UNREADABLE: Mutex<Option<(PathBuf, Unreadable)>> = Mutex::new(None);

/// The unreadable entries kept for the state.json at `path`.
fn unreadable_for(path: &Path) -> Unreadable {
    let kept = UNREADABLE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    match kept.as_ref() {
        Some((kept_path, unreadable)) if kept_path == path => unreadable.clone(),
        _ => Unreadable::default(),
    }
}

/// Keeps `unreadable` as the unreadable entries of the state.json at `path`.
fn keep_unreadable(path: PathBuf, unreadable: Unreadable) {
    *UNREADABLE.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((path, unreadable));
}

/// `Array.isArray(parsed[key]) ? parsed[key] : []`, as records, and the entries this version cannot read as one
/// (with their index).
fn records<T: serde::de::DeserializeOwned>(parsed: &Value, key: &str) -> (Vec<Ordered<T>>, Vec<(usize, Value)>) {
    let mut records = Vec::new();
    let mut unreadable = Vec::new();
    if let Some(Value::Array(items)) = parsed.get(key) {
        for (index, item) in items.iter().enumerate() {
            match serde_json::from_value(item.clone()) {
                Ok(record) => records.push(record),
                Err(_) => unreadable.push((index, item.clone())),
            }
        }
    }
    (records, unreadable)
}

/// `session.worktree?.id` of a raw session entry (the worktree record it references).
fn raw_worktree_id(session: &Value) -> Option<&str> {
    session.get("worktree")?.get("id")?.as_str().filter(|id| !id.is_empty())
}

/// TS: storage.ts readState (empty or missing file: an empty state is written; unparseable JSON is moved aside to
/// `state.json.corrupt-<ms>` and replaced by an empty state; otherwise worktree records and done markers are
/// migrated). Entries this version cannot read are kept aside (see [`Unreadable`]).
pub async fn read_state() -> Result<ReadState> {
    ensure_config_dir().await?;
    let state_path = paths::state_path();
    let raw = match read_text(&state_path).await {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            keep_unreadable(state_path, Unreadable::default());
            save_state(&StateFile::default()).await?;
            return Ok(ReadState::default());
        }
        Err(error) => return Err(read_error(&error, &state_path)),
    };
    if js_trim(&raw).is_empty() {
        keep_unreadable(state_path, Unreadable::default());
        save_state(&StateFile::default()).await?;
        return Ok(ReadState::default());
    }
    let Some(parsed) = parse_json(&raw) else {
        let mut backup = state_path.as_os_str().to_owned();
        backup.push(format!(".corrupt-{}", now_ms()));
        let _ = tokio::fs::rename(&state_path, PathBuf::from(backup)).await;
        keep_unreadable(state_path, Unreadable::default());
        save_state(&StateFile::default()).await?;
        return Ok(ReadState::default());
    };
    if parsed.is_null() {
        return Err(DhError::msg("Cannot read properties of null (reading 'sessions')"));
    }
    let (sessions, unreadable_sessions) = records::<SessionRecord>(&parsed, "sessions");
    let (worktrees, unreadable_worktrees) = records::<WorktreeRecord>(&parsed, "worktrees");
    // Worktree records an unreadable session references: TS sees that session, so they are not unreferenced.
    let used: HashSet<String> = unreadable_sessions
        .iter()
        .filter_map(|(_, session)| raw_worktree_id(session).map(str::to_owned))
        .collect();
    let original: HashMap<String, usize> = worktrees
        .iter()
        .enumerate()
        .map(|(index, record)| (record.id.clone(), index))
        .collect();
    let used_records: Vec<(usize, Ordered<WorktreeRecord>)> = worktrees
        .iter()
        .enumerate()
        .filter(|(_, record)| used.contains(&record.id))
        .map(|(index, record)| (index, record.clone()))
        .collect();
    let mut lifted = migrate_worktree_records(sessions, worktrees, &mut random_uuid);
    for (index, record) in used_records {
        if lifted.worktrees.iter().any(|kept| kept.id == record.id) {
            continue;
        }
        // Back in its place among the file's records (the migration appends the records it creates after them).
        let at = lifted
            .worktrees
            .iter()
            .take_while(|kept| original.get(&kept.id).is_some_and(|&before| before < index))
            .count();
        lifted.worktrees.insert(at, record);
    }
    // An unreadable worktree record is kept while unreadable sessions reference it. TS drops unreferenced records,
    // and a readable session's reference to one has been rebuilt as a record by the migration.
    let given = unreadable_worktrees.len();
    let unreadable_worktrees: Vec<(usize, Value)> = unreadable_worktrees
        .into_iter()
        .filter(|(_, record)| {
            record
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| used.contains(id) && !lifted.worktrees.iter().any(|kept| kept.id == id))
        })
        .collect();
    let dropped_unreadable = unreadable_worktrees.len() != given;
    keep_unreadable(
        state_path,
        Unreadable {
            sessions: unreadable_sessions,
            worktrees: unreadable_worktrees,
        },
    );
    // Main-checkout sessions' old `M` markers become done markers (after the linked worktrees' moved to their records).
    let done = migrate_done_markers(lifted.sessions);
    Ok(ReadState {
        state: StateFile {
            sessions: done.sessions,
            worktrees: lifted.worktrees,
        },
        migrated: lifted.changed || done.changed || dropped_unreadable,
    })
}

/// Writes `state` as saveState does; any serialisable state (a record of another shape included).
async fn write_state<T: Serialize + ?Sized>(state: &T) -> Result<()> {
    ensure_config_dir().await?;
    let text = to_ts_file_text(state).map_err(|error| DhError::msg(error.to_string()))?;
    write_atomic(&paths::state_path(), text.as_bytes(), FILE_MODE).await
}

/// One element of a written `sessions`/`worktrees` array: a record, or an entry kept as read.
#[derive(Serialize)]
#[serde(untagged)]
enum Entry<'a, T: Serialize> {
    Record(&'a Ordered<T>),
    Kept(&'a Value),
}

/// state.json with the unreadable entries put back.
#[derive(Serialize)]
struct WithUnreadable<'a> {
    sessions: Vec<Entry<'a, SessionRecord>>,
    worktrees: Vec<Entry<'a, WorktreeRecord>>,
}

/// `records` with each kept entry back at its index in the file (records in file order give the file's order).
fn in_file_order<'a, T: Serialize>(records: &'a [Ordered<T>], kept: &'a [(usize, Value)]) -> Vec<Entry<'a, T>> {
    let mut entries: Vec<Entry<'a, T>> = records.iter().map(Entry::Record).collect();
    for (index, value) in kept {
        let at = (*index).min(entries.len());
        entries.insert(at, Entry::Kept(value));
    }
    entries
}

/// A session's status and `createdAt`, for the sortSessionsNewestFirst comparator.
fn order_key<'a>(entry: &Entry<'a, SessionRecord>) -> (&'a str, &'a str) {
    match entry {
        Entry::Record(session) => (session.status.as_str(), session.created_at.as_str()),
        Entry::Kept(session) => (
            session.get("status").and_then(Value::as_str).unwrap_or(""),
            session.get("createdAt").and_then(Value::as_str).unwrap_or(""),
        ),
    }
}

/// `sessions` (newest first, as persist writes them) with each kept session inserted before the first one it sorts
/// before.
fn newest_first<'a>(
    sessions: &'a [Ordered<SessionRecord>],
    kept: &'a [(usize, Value)],
) -> Vec<Entry<'a, SessionRecord>> {
    let mut entries: Vec<Entry<'a, SessionRecord>> = sessions.iter().map(Entry::Record).collect();
    for (_, value) in kept {
        let entry = Entry::Kept(value);
        let key = order_key(&entry);
        let at = entries
            .iter()
            .position(|other| newest_first_order(key, order_key(other)) < 0)
            .unwrap_or(entries.len());
        entries.insert(at, entry);
    }
    entries
}

/// TS: storage.ts saveState (`JSON.stringify(state, null, 2)` plus a newline, atomically, 0600). The entries the
/// last read could not load are written back (see [`Unreadable`]): sessions where the newest-first order puts them
/// (the daemon's persist), worktree records at their place in the file.
pub async fn save_state(state: &StateFile) -> Result<()> {
    let unreadable = unreadable_for(&paths::state_path());
    if unreadable == Unreadable::default() {
        return write_state(state).await;
    }
    write_state(&WithUnreadable {
        sessions: newest_first(&state.sessions, &unreadable.sessions),
        worktrees: in_file_order(&state.worktrees, &unreadable.worktrees),
    })
    .await
}

/// [`save_state`] with every kept entry at its place in the file (markAllNonExitedSessionsExited keeps the file's
/// order).
async fn save_state_in_file_order(state: &StateFile, unreadable: &Unreadable) -> Result<()> {
    write_state(&WithUnreadable {
        sessions: in_file_order(&state.sessions, &unreadable.sessions),
        worktrees: in_file_order(&state.worktrees, &unreadable.worktrees),
    })
    .await
}

/// Forgets the recorded key order of `key`'s value (TS replaced it with a fresh object literal).
fn reset_child_order(session: &mut Ordered<SessionRecord>, key: &str) {
    if let KeyOrder::Object(mut keys) = session.key_order().clone() {
        for (name, child) in &mut keys {
            if name == key {
                *child = KeyOrder::Unrecorded;
            }
        }
        session.set_key_order(KeyOrder::Object(keys));
    }
}

/// The recovery of markAllNonExitedSessionsExited for one live session (`now`: ISO time).
fn interrupted(mut session: Ordered<SessionRecord>, now: &str) -> Ordered<SessionRecord> {
    // `{...session, status, updatedAt, pid, exitCode, exitSignal, devRunning, exitReason, attention, setup?}`: keys
    // the session has keep their place, new ones are appended in this order.
    for key in [
        "status",
        "updatedAt",
        "pid",
        "exitCode",
        "exitSignal",
        "devRunning",
        "exitReason",
        "attention",
    ] {
        session.touch(key);
    }
    session.status = SessionStatus::Exited;
    session.updated_at = now.to_owned();
    session.pid = None;
    if !matches!(session.exit_code, Some(Some(_))) {
        session.exit_code = Some(None);
    }
    if !matches!(session.exit_signal, Some(Some(_))) {
        session.exit_signal = Some(None);
    }
    session.dev_running = Some(false);
    session.exit_reason = Some(ExitReason::Interrupted);
    session.attention = Some(SessionAttention {
        state: AttentionState::Unknown,
        event: "DaemonRestart".to_owned(),
        at: now.to_owned(),
        reason: None,
        // A fresh literal: unknown keys of the old attention are lost (MIGRATIONS.md row 14).
        extra: Default::default(),
    });
    reset_child_order(&mut session, "attention");
    if let Some(setup) = session.setup.as_mut()
        && setup.state == SetupState::Running
    {
        setup.state = SetupState::Failed;
        setup.output = format!("{}\nInterrupted by daemon restart", setup.output);
        session.touch("setup");
    }
    session
}

/// TS: storage.ts markAllNonExitedSessionsExited (daemon start: sessions that were live are exited `interrupted`,
/// running setups failed, dev servers stopped; written back when anything changed or the state was migrated).
///
/// Daemon-crash/restart recovery: live PTYs are owned by the daemon process, so when a new daemon starts, persisted
/// non-exited sessions no longer have live handles and must be shown as exited. A normal frontend quit does not reach
/// this path (the daemon stays alive).
pub async fn mark_all_non_exited_sessions_exited() -> Result<StateFile> {
    let ReadState { state, migrated } = read_state().await?;
    // A migrated state is written back at once.
    let mut changed = migrated;
    let now = jsstr::date::to_iso_string(now_ms()).unwrap_or_default();
    let sessions = state
        .sessions
        .into_iter()
        .map(|mut session| {
            if session.status == SessionStatus::Exited {
                if session.dev_running == Some(true) {
                    changed = true;
                    session.dev_running = Some(false);
                }
                return session;
            }
            changed = true;
            interrupted(session, &now)
        })
        .collect();
    let recovered = StateFile {
        sessions,
        worktrees: state.worktrees,
    };
    // The sessions this version cannot read are recovered the same way, on the entries as read.
    let state_path = paths::state_path();
    let mut unreadable = unreadable_for(&state_path);
    for (_, session) in &mut unreadable.sessions {
        changed |= interrupted_entry(session, &now);
    }
    keep_unreadable(state_path, unreadable.clone());
    if changed {
        save_state_in_file_order(&recovered, &unreadable).await?;
    }
    Ok(recovered)
}

/// JS truthiness of a JSON value.
fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0 && !number.is_nan()),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `${value}` of a JSON value (`None`: undefined).
fn js_template(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_owned(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                item => js_template(Some(item)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_owned(),
        Some(other) => other.to_string(),
    }
}

/// markAllNonExitedSessionsExited on a session entry this version cannot read: `{...session, status, ...}` on the
/// object as read (keys it has keep their place, new ones are appended). Returns whether it changed. Anything but an
/// object is left as it is.
fn interrupted_entry(entry: &mut Value, now: &str) -> bool {
    let Value::Object(session) = entry else {
        return false;
    };
    if session.get("status").and_then(Value::as_str) == Some("exited") {
        if session.get("devRunning").is_some_and(js_truthy) {
            session.insert("devRunning".into(), Value::Bool(false));
            return true;
        }
        return false;
    }
    session.insert("status".into(), "exited".into());
    session.insert("updatedAt".into(), now.into());
    // `pid: undefined`: JSON.stringify leaves it out.
    session.shift_remove("pid");
    for key in ["exitCode", "exitSignal"] {
        // `session[key] ?? null`.
        let value = session.get(key).filter(|value| !value.is_null()).cloned();
        session.insert(key.into(), value.unwrap_or(Value::Null));
    }
    session.insert("devRunning".into(), Value::Bool(false));
    session.insert("exitReason".into(), "interrupted".into());
    session.insert(
        "attention".into(),
        serde_json::json!({"state": "unknown", "event": "DaemonRestart", "at": now}),
    );
    if let Some(Value::Object(setup)) = session.get_mut("setup")
        && setup.get("state").and_then(Value::as_str) == Some("running")
    {
        let output = format!("{}\nInterrupted by daemon restart", js_template(setup.get("output")));
        setup.insert("state".into(), "failed".into());
        setup.insert("output".into(), output.into());
    }
    true
}

/// The sortSessionsNewestFirst comparator on a session's status and `createdAt` (running first, else `localeCompare`;
/// two different statuses neither of which is running compare as 1 both ways, as in TS).
fn newest_first_order(a: (&str, &str), b: (&str, &str)) -> i32 {
    if a.0 != b.0 {
        return if a.0 == "running" { -1 } else { 1 };
    }
    locale_compare_sign(a.1, b.1)
}

/// TS: storage.ts sortSessionsNewestFirst (running first, then by `createdAt` ascending, `localeCompare`).
///
/// The comparator is not consistent for two statuses that are both not `running`, so the result depends on the
/// sorting algorithm: this is V8's (see [`js_sort`]).
pub fn sort_sessions_newest_first<S: SessionLike + Clone>(sessions: &[S]) -> Vec<S> {
    let mut sorted = sessions.to_vec();
    js_sort::sort_by(&mut sorted, |a, b| {
        let (a, b) = (a.session(), b.session());
        newest_first_order(
            (a.status.as_str(), a.created_at.as_str()),
            (b.status.as_str(), b.created_at.as_str()),
        )
    });
    sorted
}

/// storage.ts normalizeAppConfig: unlike [`AppConfig::from_raw`] (loadAppConfig's view), an empty config still has
/// `trustedProjects: {}` and `notifications: false`.
fn normalize_app_config(raw: &RawConfig) -> AppConfig {
    let mut config = AppConfig::from_raw(raw);
    if raw.is_empty() {
        config.trusted_projects = Some(indexmap::IndexMap::new());
        config.notifications = Some(false);
    }
    config
}

/// TS: storage.ts readRawConfig (config.json as stored; missing: empty; not an object: `<path> must contain a JSON
/// object`).
pub async fn read_raw_config() -> Result<RawConfig> {
    let config_path = paths::config_path();
    let text = match read_text(&config_path).await {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(RawConfig::new()),
        Err(error) => return Err(read_error(&error, &config_path)),
    };
    match parse_json(&text) {
        Some(Value::Object(map)) => Ok(map.into_iter().collect()),
        Some(_) => Err(DhError::msg(format!(
            "{} must contain a JSON object",
            display(&config_path)
        ))),
        None => Err(DhError::json_parse(&text)),
    }
}

/// TS: storage.ts writeRawConfig (atomic, 0600).
pub async fn write_raw_config(config: &RawConfig) -> Result<()> {
    let text = to_ts_file_text(config).map_err(|error| DhError::msg(error.to_string()))?;
    write_atomic(&paths::config_path(), text.as_bytes(), FILE_MODE).await
}

/// TS: storage.ts loadAppConfig (the normalised view; an empty file gives an empty config).
pub async fn load_app_config() -> Result<AppConfig> {
    ensure_config_dir().await?;
    let raw = read_raw_config().await?;
    Ok(AppConfig::from_raw(&raw))
}

/// A file's mtime in epoch milliseconds (`stat.mtimeMs`).
fn mtime_ms(stat: &std::fs::Metadata) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let ms = stat.mtime() as f64 * 1000.0 + stat.mtime_nsec() as f64 / 1_000_000.0;
    ms
}

/// `Date.now() - stat.mtimeMs > LOCK_STALE_MS`.
fn is_stale(stat: &std::fs::Metadata) -> bool {
    #[allow(clippy::cast_precision_loss)]
    let age = now_ms() as f64 - mtime_ms(stat);
    age > LOCK_STALE.as_millis() as f64
}

/// TS: `Math.min(5 * 2 ** attempt, 100) + Math.random() * 10` milliseconds.
fn backoff(attempt: u32) -> Duration {
    let base = if attempt >= 5 {
        100.0
    } else {
        f64::from(5u32 << attempt).min(100.0)
    };
    let jitter = f64::from(uuid::Uuid::new_v4().as_bytes()[0]) / 256.0 * 10.0;
    Duration::from_secs_f64((base + jitter) / 1000.0)
}

/// Takes `lock` (exclusive create with our token).
async fn acquire_lock(lock: &Path, token: &str) -> Result<()> {
    let deadline = now_ms() + i64::try_from(LOCK_TIMEOUT.as_millis()).unwrap_or(i64::MAX);
    let mut attempt: u32 = 0;
    loop {
        let opened = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(FILE_MODE)
            .open(lock)
            .await;
        match opened {
            Ok(mut handle) => {
                let written = async {
                    tokio::io::AsyncWriteExt::write_all(&mut handle, token.as_bytes()).await?;
                    tokio::io::AsyncWriteExt::flush(&mut handle).await
                }
                .await;
                drop(handle);
                return written.map_err(|error| DhError::io(&error, "write", &display(lock)));
            }
            Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => {
                return Err(DhError::io(&error, "open", &display(lock)));
            }
            Err(_) => {}
        }
        let stat = tokio::fs::metadata(lock).await.ok();
        if stat.as_ref().is_some_and(is_stale) {
            // A crashed writer left it behind. Checking and then deleting by path could delete a lock another writer
            // created in between, so move it aside atomically and only discard it if it is still stale.
            let mut aside = lock.as_os_str().to_owned();
            aside.push(format!(".{}.stale", random_uuid()));
            let aside = PathBuf::from(aside);
            if tokio::fs::rename(lock, &aside).await.is_ok() {
                let moved = tokio::fs::metadata(&aside).await.ok();
                // A live lock we raced with is put back (link fails rather than overwrite a newer lock).
                if moved.as_ref().is_some_and(|moved| !is_stale(moved)) {
                    let _ = tokio::fs::hard_link(&aside, lock).await;
                }
                remove_forced(&aside).await?;
            }
            attempt = attempt.saturating_add(1);
            continue;
        }
        if now_ms() > deadline {
            return Err(DhError::msg(format!("Timed out waiting for {}", display(lock))));
        }
        tokio::time::sleep(backoff(attempt)).await;
        attempt = attempt.saturating_add(1);
    }
}

/// `fs.rm(path, {force: true})` for a file.
async fn remove_forced(path: &Path) -> Result<()> {
    match tokio::fs::remove_file(path).await {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(DhError::io(&error, "rm", &display(path))),
        _ => Ok(()),
    }
}

/// TS: storage.ts withConfigLock: runs `operation` holding `config.json.lock` (exclusive create; a lock older than
/// LOCK_STALE is moved aside and discarded; `Timed out waiting for <lock>` after LOCK_TIMEOUT; only our own lock is
/// released).
///
/// The UI and daemon processes both write config.json; the lockfile serializes their read-modify-write.
pub async fn with_config_lock<F, Fut, T>(operation: F) -> Result<T>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut lock = paths::config_path().into_os_string();
    lock.push(".lock");
    let lock = PathBuf::from(lock);
    let token = format!("{}:{}", std::process::id(), random_uuid());
    acquire_lock(&lock, &token).await?;
    let result = operation().await;
    // Only release our own lock: if it was ever judged stale and replaced, the new holder's lock stays.
    let ours = read_text(&lock).await.ok().is_some_and(|held| held == token);
    if ours {
        remove_forced(&lock).await?;
    }
    result
}

/// `JSON.stringify(value)` of one AppConfig property (`None`: undefined).
fn property_json(value: &Option<Value>) -> Option<String> {
    value.as_ref().map(Value::to_string)
}

/// An optional AppConfig property as JSON (`None`: undefined).
fn json_of<T: Serialize>(value: &Option<T>) -> Option<Value> {
    value.as_ref().and_then(|value| serde_json::to_value(value).ok())
}

/// The AppConfig properties in normalizeAppConfig key order (all of them, as `Object.keys` lists them).
fn properties(config: &AppConfig) -> [(&'static str, Option<Value>); 6] {
    [
        ("dev_command", json_of(&config.dev_command)),
        ("attach_scroll_sensitivity", json_of(&config.attach_scroll_sensitivity)),
        ("trustedProjects", json_of(&config.trusted_projects)),
        ("agent_hooks", json_of(&config.agent_hooks)),
        ("notifications", json_of(&config.notifications)),
        ("defaults", config.defaults.clone()),
    ]
}

/// The read-modify-write of updateAppConfig, holding the lock.
async fn apply_update<F>(updater: F) -> Result<AppConfig>
where
    F: FnOnce(AppConfig) -> Result<AppConfig>,
{
    let raw = read_raw_config().await?;
    let current = normalize_app_config(&raw);
    let next = updater(current.clone())?;
    let mut written = raw.clone();
    // Only keys whose JSON changed are written back, so unknown keys and absent defaults are preserved.
    for ((key, known), (_, updated)) in properties(&current).into_iter().zip(properties(&next)) {
        if property_json(&known) == property_json(&updated) {
            continue;
        }
        match updated {
            None => {
                written.shift_remove(key);
            }
            Some(value) => {
                written.insert(key.to_owned(), value);
            }
        }
    }
    write_raw_config(&written).await?;
    Ok(normalize_app_config(&written))
}

/// TS: storage.ts updateAppConfig with an updater: a read-modify-write of config.json serialized in-process and
/// across processes. The updater sees the normalised view (and may fail, which writes nothing); only the keys whose
/// JSON changed are written back (a key set to `None` is removed), so unknown keys keep their place and value.
/// Returns the normalised config as written. Queued at call time (TS `configQueue`) and spawned at once on the
/// current `LocalSet`, so un-awaited updates still run, in order.
pub fn update_app_config<F>(updater: F) -> Task<Result<AppConfig>>
where
    F: FnOnce(AppConfig) -> Result<AppConfig> + 'static,
{
    CONFIG_QUEUE.with(|queue| {
        queue.run((), move || async move {
            ensure_config_dir().await?;
            with_config_lock(|| apply_update(updater)).await
        })
    })
}

/// TS: storage.ts updateAppConfig with a patch object (`{...current, ...patch}`: the patch's `Some` fields win).
pub fn patch_app_config(patch: AppConfig) -> Task<Result<AppConfig>> {
    update_app_config(move |current| {
        Ok(AppConfig {
            dev_command: patch.dev_command.or(current.dev_command),
            attach_scroll_sensitivity: patch.attach_scroll_sensitivity.or(current.attach_scroll_sensitivity),
            trusted_projects: patch.trusted_projects.or(current.trusted_projects),
            agent_hooks: patch.agent_hooks.or(current.agent_hooks),
            notifications: patch.notifications.or(current.notifications),
            defaults: patch.defaults.or(current.defaults),
        })
    })
}

/// TS: storage.ts stateFileDisplayPath (state.json relative to the cwd, else absolute).
pub fn state_file_display_path() -> String {
    let state = display(&paths::state_path());
    let relative = jsstr::path::relative(&display(&paths::current_dir()), &state);
    if relative.is_empty() { state } else { relative }
}

/// V8's `Array.prototype.sort` (TimSort, third_party/v8/builtins/array-sort.tq), so that an inconsistent comparator
/// (sortSessionsNewestFirst's) orders exactly as in Node. The comparator returns a JS comparison result's sign.
pub(crate) mod js_sort {
    const MIN_GALLOP: usize = 7;

    struct State<'a, T, F> {
        work: &'a mut Vec<T>,
        compare: F,
        min_gallop: usize,
        runs: Vec<(usize, usize)>,
    }

    /// Sorts `items` in place as V8 does with `compare`.
    pub fn sort_by<T: Clone, F: FnMut(&T, &T) -> i32>(items: &mut Vec<T>, compare: F) {
        let length = items.len();
        if length < 2 {
            return;
        }
        let mut state = State {
            work: items,
            compare,
            min_gallop: MIN_GALLOP,
            runs: Vec::new(),
        };
        let mut remaining = length;
        let mut low = 0;
        let min_run = min_run_length(remaining);
        while remaining != 0 {
            let mut run = state.count_and_make_run(low, low + remaining);
            if run < min_run {
                let forced = min_run.min(remaining);
                state.binary_insertion_sort(low, low + run, low + forced);
                run = forced;
            }
            state.runs.push((low, run));
            state.merge_collapse();
            low += run;
            remaining -= run;
        }
        state.merge_force_collapse();
    }

    fn min_run_length(mut n: usize) -> usize {
        let mut r = 0;
        while n >= 64 {
            r |= n & 1;
            n >>= 1;
        }
        n + r
    }

    impl<T: Clone, F: FnMut(&T, &T) -> i32> State<'_, T, F> {
        fn cmp(&mut self, a: &T, b: &T) -> i32 {
            (self.compare)(a, b)
        }

        fn count_and_make_run(&mut self, low_arg: usize, high: usize) -> usize {
            let low = low_arg + 1;
            if low == high {
                return 1;
            }
            let mut run = 2;
            let element_low = self.work[low].clone();
            let element_low_pre = self.work[low - 1].clone();
            let descending = self.cmp(&element_low, &element_low_pre) < 0;
            let mut previous = element_low;
            for index in low + 1..high {
                let current = self.work[index].clone();
                let order = self.cmp(&current, &previous);
                if descending {
                    if order >= 0 {
                        break;
                    }
                } else if order < 0 {
                    break;
                }
                previous = current;
                run += 1;
            }
            if descending {
                self.work[low_arg..low_arg + run].reverse();
            }
            run
        }

        fn binary_insertion_sort(&mut self, low: usize, start_arg: usize, high: usize) {
            let mut start = if low == start_arg { start_arg + 1 } else { start_arg };
            while start < high {
                let mut left = low;
                let mut right = start;
                let pivot = self.work[start].clone();
                while left < right {
                    let mid = left + ((right - left) >> 1);
                    let mid_value = self.work[mid].clone();
                    if self.cmp(&pivot, &mid_value) < 0 {
                        right = mid;
                    } else {
                        left = mid + 1;
                    }
                }
                let mut p = start;
                while p > left {
                    self.work[p] = self.work[p - 1].clone();
                    p -= 1;
                }
                self.work[left] = pivot;
                start += 1;
            }
        }

        fn run_length(&self, index: usize) -> usize {
            self.runs[index].1
        }

        fn run_invariant_established(&self, n: usize) -> bool {
            if n < 2 {
                return true;
            }
            self.run_length(n - 2) > self.run_length(n - 1) + self.run_length(n)
        }

        fn merge_collapse(&mut self) {
            while self.runs.len() > 1 {
                let mut n = self.runs.len() - 2;
                if !self.run_invariant_established(n + 1) || !self.run_invariant_established(n) {
                    if self.run_length(n - 1) < self.run_length(n + 1) {
                        n -= 1;
                    }
                    self.merge_at(n);
                } else if self.run_length(n) <= self.run_length(n + 1) {
                    self.merge_at(n);
                } else {
                    break;
                }
            }
        }

        fn merge_force_collapse(&mut self) {
            while self.runs.len() > 1 {
                let mut n = self.runs.len() - 2;
                if n > 0 && self.run_length(n - 1) < self.run_length(n + 1) {
                    n -= 1;
                }
                self.merge_at(n);
            }
        }

        fn merge_at(&mut self, i: usize) {
            let (mut base_a, mut length_a) = self.runs[i];
            let (base_b, mut length_b) = self.runs[i + 1];
            self.runs[i].1 = length_a + length_b;
            self.runs.remove(i + 1);

            // Where does b start in a? Elements in a before that can be ignored.
            let key_right = self.work[base_b].clone();
            let k = self.gallop_right(Source::Work, &key_right, base_a, length_a, 0);
            base_a += k;
            length_a -= k;
            if length_a == 0 {
                return;
            }
            // Where does a end in b? Elements in b after that can be ignored.
            let key_left = self.work[base_a + length_a - 1].clone();
            length_b = self.gallop_left(Source::Work, &key_left, base_b, length_b, length_b - 1);
            if length_b == 0 {
                return;
            }
            if length_a <= length_b {
                self.merge_low(base_a, length_a, base_b, length_b);
            } else {
                self.merge_high(base_a, length_a, base_b, length_b);
            }
        }

        fn at(&self, source: Source, temp: &[T], index: usize) -> T {
            match source {
                Source::Work => self.work[index].clone(),
                Source::Temp => temp[index].clone(),
            }
        }

        fn gallop_left_in(
            &mut self,
            source: Source,
            temp: &[T],
            key: &T,
            base: usize,
            length: usize,
            hint: usize,
        ) -> usize {
            let (base, length, hint) = (base as isize, length as isize, hint as isize);
            let mut last_ofs: isize = 0;
            let mut offset: isize = 1;
            let hint_value = self.at(source, temp, (base + hint) as usize);
            if self.cmp(&hint_value, key) < 0 {
                // a[hint] < key: gallop right, until a[hint + last_ofs] < key <= a[hint + offset].
                let max_ofs = length - hint;
                while offset < max_ofs {
                    let value = self.at(source, temp, (base + hint + offset) as usize);
                    if self.cmp(&value, key) >= 0 {
                        break;
                    }
                    last_ofs = offset;
                    offset = (offset << 1) + 1;
                }
                offset = offset.min(max_ofs);
                last_ofs += hint;
                offset += hint;
            } else {
                // key <= a[hint]: gallop left, until a[hint - offset] < key <= a[hint - last_ofs].
                let max_ofs = hint + 1;
                while offset < max_ofs {
                    let value = self.at(source, temp, (base + hint - offset) as usize);
                    if self.cmp(&value, key) < 0 {
                        break;
                    }
                    last_ofs = offset;
                    offset = (offset << 1) + 1;
                }
                offset = offset.min(max_ofs);
                let tmp = last_ofs;
                last_ofs = hint - offset;
                offset = hint - tmp;
            }
            last_ofs += 1;
            while last_ofs < offset {
                let m = last_ofs + ((offset - last_ofs) >> 1);
                let value = self.at(source, temp, (base + m) as usize);
                if self.cmp(&value, key) < 0 {
                    last_ofs = m + 1;
                } else {
                    offset = m;
                }
            }
            offset as usize
        }

        fn gallop_right_in(
            &mut self,
            source: Source,
            temp: &[T],
            key: &T,
            base: usize,
            length: usize,
            hint: usize,
        ) -> usize {
            let (base, length, hint) = (base as isize, length as isize, hint as isize);
            let mut last_ofs: isize = 0;
            let mut offset: isize = 1;
            let hint_value = self.at(source, temp, (base + hint) as usize);
            if self.cmp(key, &hint_value) < 0 {
                // key < a[hint]: gallop left, until a[hint - offset] <= key < a[hint - last_ofs].
                let max_ofs = hint + 1;
                while offset < max_ofs {
                    let value = self.at(source, temp, (base + hint - offset) as usize);
                    if self.cmp(key, &value) >= 0 {
                        break;
                    }
                    last_ofs = offset;
                    offset = (offset << 1) + 1;
                }
                offset = offset.min(max_ofs);
                let tmp = last_ofs;
                last_ofs = hint - offset;
                offset = hint - tmp;
            } else {
                // a[hint] <= key: gallop right, until a[hint + last_ofs] <= key < a[hint + offset].
                let max_ofs = length - hint;
                while offset < max_ofs {
                    let value = self.at(source, temp, (base + hint + offset) as usize);
                    if self.cmp(key, &value) < 0 {
                        break;
                    }
                    last_ofs = offset;
                    offset = (offset << 1) + 1;
                }
                offset = offset.min(max_ofs);
                last_ofs += hint;
                offset += hint;
            }
            last_ofs += 1;
            while last_ofs < offset {
                let m = last_ofs + ((offset - last_ofs) >> 1);
                let value = self.at(source, temp, (base + m) as usize);
                if self.cmp(key, &value) < 0 {
                    offset = m;
                } else {
                    last_ofs = m + 1;
                }
            }
            offset as usize
        }

        fn gallop_left(&mut self, source: Source, key: &T, base: usize, length: usize, hint: usize) -> usize {
            self.gallop_left_in(source, &[], key, base, length, hint)
        }

        fn gallop_right(&mut self, source: Source, key: &T, base: usize, length: usize, hint: usize) -> usize {
            self.gallop_right_in(source, &[], key, base, length, hint)
        }

        fn merge_low(&mut self, base_a: usize, length_a: usize, base_b: usize, length_b: usize) {
            let mut length_a = length_a;
            let mut length_b = length_b;
            let temp: Vec<T> = self.work[base_a..base_a + length_a].to_vec();
            let mut dest = base_a;
            let mut cursor_temp = 0;
            let mut cursor_b = base_b;

            self.work[dest] = self.work[cursor_b].clone();
            dest += 1;
            cursor_b += 1;

            enum Exit {
                Succeed,
                CopyB,
            }
            let exit = 'merge: {
                length_b -= 1;
                if length_b == 0 {
                    break 'merge Exit::Succeed;
                }
                if length_a == 1 {
                    break 'merge Exit::CopyB;
                }
                let mut min_gallop = self.min_gallop;
                loop {
                    let mut wins_a = 0;
                    let mut wins_b = 0;
                    // Straightforward merging until (if ever) one run appears to win consistently.
                    loop {
                        let b = self.work[cursor_b].clone();
                        if self.cmp(&b, &temp[cursor_temp]) < 0 {
                            self.work[dest] = b;
                            dest += 1;
                            cursor_b += 1;
                            wins_b += 1;
                            length_b -= 1;
                            wins_a = 0;
                            if length_b == 0 {
                                break 'merge Exit::Succeed;
                            }
                            if wins_b >= min_gallop {
                                break;
                            }
                        } else {
                            self.work[dest] = temp[cursor_temp].clone();
                            dest += 1;
                            cursor_temp += 1;
                            wins_a += 1;
                            length_a -= 1;
                            wins_b = 0;
                            if length_a == 1 {
                                break 'merge Exit::CopyB;
                            }
                            if wins_a >= min_gallop {
                                break;
                            }
                        }
                    }
                    // One run is winning so consistently that galloping may be a huge win.
                    min_gallop += 1;
                    let mut first = true;
                    while wins_a >= MIN_GALLOP || wins_b >= MIN_GALLOP || first {
                        first = false;
                        min_gallop = min_gallop.saturating_sub(1).max(1);
                        self.min_gallop = min_gallop;

                        let key = self.work[cursor_b].clone();
                        wins_a = self.gallop_right_in(Source::Temp, &temp, &key, cursor_temp, length_a, 0);
                        if wins_a > 0 {
                            self.work[dest..dest + wins_a].clone_from_slice(&temp[cursor_temp..cursor_temp + wins_a]);
                            dest += wins_a;
                            cursor_temp += wins_a;
                            length_a -= wins_a;
                            if length_a == 1 {
                                break 'merge Exit::CopyB;
                            }
                            // Impossible with a consistent comparator, which cannot be assumed.
                            if length_a == 0 {
                                break 'merge Exit::Succeed;
                            }
                        }
                        self.work[dest] = self.work[cursor_b].clone();
                        dest += 1;
                        cursor_b += 1;
                        length_b -= 1;
                        if length_b == 0 {
                            break 'merge Exit::Succeed;
                        }

                        let key = temp[cursor_temp].clone();
                        wins_b = self.gallop_left(Source::Work, &key, cursor_b, length_b, 0);
                        if wins_b > 0 {
                            for offset in 0..wins_b {
                                self.work[dest + offset] = self.work[cursor_b + offset].clone();
                            }
                            dest += wins_b;
                            cursor_b += wins_b;
                            length_b -= wins_b;
                            if length_b == 0 {
                                break 'merge Exit::Succeed;
                            }
                        }
                        self.work[dest] = temp[cursor_temp].clone();
                        dest += 1;
                        cursor_temp += 1;
                        length_a -= 1;
                        if length_a == 1 {
                            break 'merge Exit::CopyB;
                        }
                    }
                    // Penalize it for leaving galloping mode.
                    min_gallop += 1;
                    self.min_gallop = min_gallop;
                }
            };
            match exit {
                Exit::Succeed => {
                    self.work[dest..dest + length_a].clone_from_slice(&temp[cursor_temp..cursor_temp + length_a]);
                }
                Exit::CopyB => {
                    // The last element of run A belongs at the end of the merge.
                    for offset in 0..length_b {
                        self.work[dest + offset] = self.work[cursor_b + offset].clone();
                    }
                    self.work[dest + length_b] = temp[cursor_temp].clone();
                }
            }
        }

        fn merge_high(&mut self, base_a: usize, length_a: usize, base_b: usize, length_b: usize) {
            let mut length_a = length_a as isize;
            let mut length_b = length_b as isize;
            let temp: Vec<T> = self.work[base_b..base_b + length_b as usize].to_vec();
            let mut dest = (base_b as isize) + length_b - 1;
            let mut cursor_temp = length_b - 1;
            let mut cursor_a = (base_a as isize) + length_a - 1;

            self.work[dest as usize] = self.work[cursor_a as usize].clone();
            dest -= 1;
            cursor_a -= 1;

            enum Exit {
                Succeed,
                CopyA,
            }
            let exit = 'merge: {
                length_a -= 1;
                if length_a == 0 {
                    break 'merge Exit::Succeed;
                }
                if length_b == 1 {
                    break 'merge Exit::CopyA;
                }
                let mut min_gallop = self.min_gallop;
                loop {
                    let mut wins_a: isize = 0;
                    let mut wins_b: isize = 0;
                    loop {
                        let a = self.work[cursor_a as usize].clone();
                        if self.cmp(&temp[cursor_temp as usize], &a) < 0 {
                            self.work[dest as usize] = a;
                            dest -= 1;
                            cursor_a -= 1;
                            wins_a += 1;
                            length_a -= 1;
                            wins_b = 0;
                            if length_a == 0 {
                                break 'merge Exit::Succeed;
                            }
                            if wins_a >= min_gallop as isize {
                                break;
                            }
                        } else {
                            self.work[dest as usize] = temp[cursor_temp as usize].clone();
                            dest -= 1;
                            cursor_temp -= 1;
                            wins_b += 1;
                            length_b -= 1;
                            wins_a = 0;
                            if length_b == 1 {
                                break 'merge Exit::CopyA;
                            }
                            if wins_b >= min_gallop as isize {
                                break;
                            }
                        }
                    }
                    min_gallop += 1;
                    let mut first = true;
                    while wins_a >= MIN_GALLOP as isize || wins_b >= MIN_GALLOP as isize || first {
                        first = false;
                        min_gallop = min_gallop.saturating_sub(1).max(1);
                        self.min_gallop = min_gallop;

                        let key = temp[cursor_temp as usize].clone();
                        let k =
                            self.gallop_right(Source::Work, &key, base_a, length_a as usize, (length_a - 1) as usize);
                        wins_a = length_a - k as isize;
                        if wins_a > 0 {
                            dest -= wins_a;
                            cursor_a -= wins_a;
                            for offset in (0..wins_a).rev() {
                                self.work[(dest + 1 + offset) as usize] =
                                    self.work[(cursor_a + 1 + offset) as usize].clone();
                            }
                            length_a -= wins_a;
                            if length_a == 0 {
                                break 'merge Exit::Succeed;
                            }
                        }
                        self.work[dest as usize] = temp[cursor_temp as usize].clone();
                        dest -= 1;
                        cursor_temp -= 1;
                        length_b -= 1;
                        if length_b == 1 {
                            break 'merge Exit::CopyA;
                        }

                        let key = self.work[cursor_a as usize].clone();
                        let k = self.gallop_left_in(
                            Source::Temp,
                            &temp,
                            &key,
                            0,
                            length_b as usize,
                            (length_b - 1) as usize,
                        );
                        wins_b = length_b - k as isize;
                        if wins_b > 0 {
                            dest -= wins_b;
                            cursor_temp -= wins_b;
                            for offset in 0..wins_b {
                                self.work[(dest + 1 + offset) as usize] =
                                    temp[(cursor_temp + 1 + offset) as usize].clone();
                            }
                            length_b -= wins_b;
                            if length_b == 1 {
                                break 'merge Exit::CopyA;
                            }
                            // Impossible with a consistent comparator, which cannot be assumed.
                            if length_b == 0 {
                                break 'merge Exit::Succeed;
                            }
                        }
                        self.work[dest as usize] = self.work[cursor_a as usize].clone();
                        dest -= 1;
                        cursor_a -= 1;
                        length_a -= 1;
                        if length_a == 0 {
                            break 'merge Exit::Succeed;
                        }
                    }
                    min_gallop += 1;
                    self.min_gallop = min_gallop;
                }
            };
            match exit {
                Exit::Succeed => {
                    if length_b > 0 {
                        let start = dest - (length_b - 1);
                        for offset in 0..length_b {
                            self.work[(start + offset) as usize] = temp[offset as usize].clone();
                        }
                    }
                }
                Exit::CopyA => {
                    // The first element of run B belongs at the front of the merge.
                    dest -= length_a;
                    cursor_a -= length_a;
                    for offset in (0..length_a).rev() {
                        self.work[(dest + 1 + offset) as usize] = self.work[(cursor_a + 1 + offset) as usize].clone();
                    }
                    self.work[dest as usize] = temp[cursor_temp as usize].clone();
                }
            }
        }
    }

    /// Which array a gallop searches: the work array or a merge's temporary copy.
    #[derive(Clone, Copy)]
    enum Source {
        Work,
        Temp,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::{Cell, RefCell};
    use std::path::Path;
    use std::sync::{Mutex, MutexGuard};

    use deckhand_parity::{Case, DirOp, Outcome, Replay, replay, tags};
    use serde_json::{Value, json};

    use super::*;

    pub(crate) const UNIT: &str = "I1";
    /// The fixtures' fixed clock (`2026-01-01T00:00:00.000Z`).
    pub(crate) const FIXED_CLOCK_MS: i64 = 1_767_225_600_000;
    /// parity/lib.ts DEFAULT_SEED.
    const SEED: u32 = 0x5eed;

    /// parity/lib.ts installDeterminism's seeded randomUUID (mulberry32 bytes, version 4 / variant bits set).
    pub(crate) struct SeededUuids(u32);

    impl SeededUuids {
        fn next_float(&mut self) -> f64 {
            self.0 = self.0.wrapping_add(0x6d2b_79f5);
            let mut t = self.0;
            t = (t ^ (t >> 15)).wrapping_mul(t | 1);
            t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
            f64::from(t ^ (t >> 14)) / 4_294_967_296.0
        }

        fn next_uuid(&mut self) -> String {
            let mut bytes = [0u8; 16];
            for byte in &mut bytes {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let value = (self.next_float() * 256.0).floor() as u8;
                *byte = value;
            }
            bytes[6] = (bytes[6] & 0x0f) | 0x40;
            bytes[8] = (bytes[8] & 0x3f) | 0x80;
            let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
            format!(
                "{}-{}-{}-{}-{}",
                &hex[0..8],
                &hex[8..12],
                &hex[12..16],
                &hex[16..20],
                &hex[20..]
            )
        }
    }

    thread_local! {
        static CLOCK: Cell<Option<i64>> = const { Cell::new(None) };
        static UUIDS: RefCell<Option<SeededUuids>> = const { RefCell::new(None) };
    }

    pub(super) fn clock_override() -> Option<i64> {
        CLOCK.with(Cell::get)
    }

    pub(super) fn uuid_override() -> Option<String> {
        UUIDS.with(|uuids| uuids.borrow_mut().as_mut().map(SeededUuids::next_uuid))
    }

    /// Pins this thread's clock and randomUUID to a recording's (installDeterminism) until dropped.
    pub(crate) struct Determinism;

    impl Determinism {
        pub(crate) fn install() -> Determinism {
            CLOCK.with(|clock| clock.set(Some(FIXED_CLOCK_MS)));
            UUIDS.with(|uuids| *uuids.borrow_mut() = Some(SeededUuids(SEED)));
            Determinism
        }
    }

    impl Drop for Determinism {
        fn drop(&mut self) {
            CLOCK.with(|clock| clock.set(None));
            UUIDS.with(|uuids| *uuids.borrow_mut() = None);
        }
    }

    /// Tests that set DECKHAND_HOME run one at a time.
    static ENV: Mutex<()> = Mutex::new(());

    /// DECKHAND_HOME set to a directory while this lives.
    pub(crate) struct Home {
        _guard: MutexGuard<'static, ()>,
        previous: Option<std::ffi::OsString>,
    }

    impl Home {
        pub(crate) fn set(home: &Path) -> Home {
            let guard = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let previous = std::env::var_os(paths::HOME_ENV);
            // SAFETY: tests touching the environment hold ENV.
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
    pub(crate) fn block_on<F: std::future::Future>(future: F) -> F::Output {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        tokio::task::LocalSet::new().block_on(&runtime, future)
    }

    /// The outcome of a call returning `undefined`.
    pub(crate) fn undefined_or_error(result: Result<()>) -> Outcome {
        match result {
            Ok(()) => Outcome::ret(tags::undefined()),
            Err(error) => Outcome::throws(error),
        }
    }

    /// Replays every directory case of `module` the manifest lists through `port(fixture, root, op)`; fails with the
    /// summary of every failing case.
    pub(crate) fn replay_dir_cases(module: &str, mut port: impl FnMut(&str, &Path, &DirOp) -> Outcome) {
        let manifest = deckhand_parity::Manifest::load(UNIT).expect("manifest");
        let cases: Vec<String> = deckhand_parity::dir_cases(module)
            .into_iter()
            .filter(|case| manifest.fixtures.contains(case))
            .collect();
        assert!(!cases.is_empty(), "no directory cases for {module}");
        let mut failed = Vec::new();
        for fixture in cases {
            let report = Replay::new(UNIT, &fixture).run_dir(|root, op| port(&fixture, root, op));
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| report.assert_ok())).is_err() {
                failed.push(report.summary());
            }
        }
        assert!(failed.is_empty(), "{}", failed.join("\n\n"));
    }

    // ---- parity replays ----------------------------------------------------------------------------------------

    #[test]
    fn sort_sessions_newest_first_parity() {
        replay(
            UNIT,
            "storage/sortSessionsNewestFirst",
            |case: &Case| -> std::result::Result<Vec<Value>, String> {
                // The recorded sessions are partial (and some statuses are not SessionStatus values), so the replay sorts
                // them as JSON with the comparator and algorithm sort_sessions_newest_first uses.
                let mut sessions: Vec<Value> = case.arg_as(0)?;
                let field =
                    |session: &Value, key: &str| session.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
                js_sort::sort_by(&mut sessions, |a, b| {
                    newest_first_order(
                        (&field(a, "status"), &field(a, "createdAt")),
                        (&field(b, "status"), &field(b, "createdAt")),
                    )
                });
                Ok(sessions)
            },
        );
    }

    type Updater = Box<dyn FnOnce(AppConfig) -> Result<AppConfig>>;

    /// An updateAppConfig updater the fixtures name (`registerFn` in parity/gen/storage.gen.ts).
    fn named_updater(name: &str) -> Option<Updater> {
        Some(match name {
            "addTrust" => Box::new(|mut current: AppConfig| {
                current
                    .trusted_projects
                    .get_or_insert_with(Default::default)
                    .insert("/new".into(), vec!["hash".into()]);
                Ok(current)
            }),
            "clearDev" => Box::new(|current| {
                Ok(AppConfig {
                    dev_command: None,
                    ..current
                })
            }),
            "identity" => Box::new(Ok),
            "dropTrusted" => Box::new(|current| {
                Ok(AppConfig {
                    trusted_projects: Some(Default::default()),
                    ..current
                })
            }),
            _ => return None,
        })
    }

    fn storage_op(fixture: &str, op: &DirOp) -> Outcome {
        let value = |result: Result<StateFile>| -> Outcome { result.map_err(|error| error.to_string()).into() };
        block_on(async {
            match op.fn_name.as_str() {
                "ensureConfigDir" => undefined_or_error(ensure_config_dir().await),
                "loadState" => value(load_state().await),
                "markAllNonExitedSessionsExited" => value(mark_all_non_exited_sessions_exited().await),
                "saveState" => match op.arg_as::<StateFile>(1) {
                    Ok(state) => undefined_or_error(save_state(&state).await),
                    Err(error) => Outcome::throws(format!("parity: state argument: {error}")),
                },
                "loadAppConfig" => load_app_config().await.map_err(|error| error.to_string()).into(),
                "updateAppConfig" => {
                    let task = match (fixture, op.fn_name(1)) {
                        // The recording drops properties holding undefined: these patches were `{dev_command:
                        // undefined}` and `{defaults: undefined}`, which remove the key ({...current, ...patch}).
                        ("storage/update-config-patch-removes-key", _) => update_app_config(|current| {
                            Ok(AppConfig {
                                dev_command: None,
                                ..current
                            })
                        }),
                        ("storage/update-config-patch-undefined-defaults-removes", _) => update_app_config(|current| {
                            Ok(AppConfig {
                                defaults: None,
                                ..current
                            })
                        }),
                        (_, Some(name)) => match named_updater(name) {
                            Some(updater) => update_app_config(updater),
                            None => return Outcome::throws(format!("parity: unknown updater {name}")),
                        },
                        (_, None) => match op.arg_as::<AppConfig>(1) {
                            Ok(patch) => patch_app_config(patch),
                            Err(error) => return Outcome::throws(format!("parity: patch argument: {error}")),
                        },
                    };
                    task.await.map_err(|error| error.to_string()).into()
                }
                other => Outcome::throws(format!("parity: unknown op {other}")),
            }
        })
    }

    #[test]
    fn storage_dir_cases_parity() {
        replay_dir_cases("storage", |fixture, root, op| {
            let Ok(home) = op.arg_str(0) else {
                return Outcome::throws("parity: no home argument");
            };
            // The recording's setup dated a left-behind lock 2020-01-01 (tree.json keeps no mtimes).
            if let Ok(lock) = std::fs::File::options().write(true).open(root.join("config.json.lock")) {
                let old = std::time::UNIX_EPOCH + Duration::from_secs(1_577_836_800);
                if let Err(error) = lock.set_modified(old) {
                    return Outcome::throws(format!("parity: lock mtime: {error}"));
                }
            }
            let _home = Home::set(Path::new(home));
            let _determinism = Determinism::install();
            storage_op(fixture, op)
        });
    }

    /// fixtures/parity/storage/mark-exited-home-v45 (recorded by parity/gen/storage.gen.ts, not listed in the
    /// manifest): the v45 legacy state.json recovered on daemon start, written byte for byte as TS writes it. Unknown
    /// keys inside `agentSessionRef`, `worktree.links`, `setup` and an exited session's `attention` round-trip
    /// (fixtures/state/MIGRATIONS.md row 19); a live session's `attention` is replaced (row 14).
    #[test]
    fn legacy_home_state_recovers_like_ts() {
        let case = deckhand_parity::fixtures_root().join("storage/mark-exited-home-v45");
        let home = temp_home();
        std::fs::copy(case.join("before/state.json"), home.path().join("state.json")).unwrap();
        let _home = Home::set(home.path());
        let _determinism = Determinism::install();
        block_on(mark_all_non_exited_sessions_exited()).expect("recovered");
        let written = std::fs::read_to_string(home.path().join("state.json")).unwrap();
        let expected = std::fs::read_to_string(case.join("after/state.json")).unwrap();
        assert_eq!(written, expected);
        // Guard: the fixture still exercises every row 19 nested key.
        for key in [
            "futureRefField",
            "futureLinksField",
            "futureSetupField",
            "futureAttentionField",
        ] {
            assert!(written.contains(&format!("\"{key}\"")), "{key}");
        }
        assert_eq!(
            std::fs::metadata(home.path().join("state.json")).unwrap().mode() & 0o777,
            FILE_MODE
        );
    }

    // ---- unit tests --------------------------------------------------------------------------------------------

    #[test]
    fn js_sort_matches_a_stable_sort_for_consistent_comparators() {
        // Long inputs exercise runs, merges and galloping; with a consistent comparator any stable sort agrees.
        let mut seed = SeededUuids(7);
        for length in [0usize, 1, 2, 5, 63, 64, 65, 200, 1000, 3000] {
            let items: Vec<(u32, usize)> = (0..length)
                .map(|index| {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let key = (seed.next_float() * 20.0) as u32;
                    (key, index)
                })
                .collect();
            let mut expected = items.clone();
            expected.sort_by_key(|item| item.0);
            let mut sorted = items.clone();
            js_sort::sort_by(&mut sorted, |a, b| i32::from(a.0 > b.0) - i32::from(a.0 < b.0));
            assert_eq!(sorted, expected, "length {length}");
            // Mostly sorted input (long runs, galloping).
            #[allow(clippy::cast_possible_truncation)]
            let mut runs: Vec<(u32, usize)> = (0..length).map(|index| ((index % 97) as u32, index)).collect();
            let mut expected = runs.clone();
            expected.sort_by_key(|item| item.0);
            js_sort::sort_by(&mut runs, |a, b| i32::from(a.0 > b.0) - i32::from(a.0 < b.0));
            assert_eq!(runs, expected, "runs, length {length}");
        }
    }

    #[test]
    fn js_numbers_writes_integral_floats_as_integers() {
        let mut value: Value = serde_json::from_str(r#"{"a":1.0,"b":[1e2,-0.0,2.5],"c":1e300}"#).unwrap();
        js_numbers(&mut value);
        assert_eq!(value.to_string(), r#"{"a":1,"b":[100,0,2.5],"c":1e+300}"#);
    }

    // JSON.parse accepts a lone UTF-16 surrogate escape (JSON.stringify writes one for a string a slice cut inside a
    // surrogate pair); a Rust string cannot hold the half, so it reads as U+FFFD (one unit, like the half).
    #[test]
    fn parse_json_reads_lone_surrogate_escapes_as_replacement_characters() {
        let parse = |text: &str| parse_json(text).map(|value| value.to_string());
        let fffd = |text: &str| Some(text.replace('!', "\u{FFFD}"));
        // A lone low half, a lone high half, both at the end, in a key, uppercase hex.
        assert_eq!(parse(r#"{"o":"\ude00x"}"#), fffd(r#"{"o":"!x"}"#));
        assert_eq!(parse(r#"["a\ud83d"]"#), fffd(r#"["a!"]"#));
        assert_eq!(parse(r#"{"\uD83D":1}"#), fffd(r#"{"!":1}"#));
        // A high half followed by something other than a low half: only the high half is replaced.
        assert_eq!(parse(r#"["\ud83dA\ud83d😀"]"#), fffd(r#"["!A!😀"]"#));
        assert_eq!(parse(r#"["\ud83dx\ude00"]"#), fffd(r#"["!x!"]"#));
        // A pair stays one character; an escaped backslash before `u` is not an escape.
        assert_eq!(parse(r#"["😀"]"#), Some(r#"["😀"]"#.to_owned()));
        assert_eq!(parse(r#"["\\ud83d"]"#), Some(r#"["\\ud83d"]"#.to_owned()));
        assert_eq!(parse(r#"["\\\ud83d"]"#), fffd(r#"["\\!"]"#));
        // Outside a string, or malformed, it still throws.
        assert_eq!(parse(r"\ud83d"), None);
        assert_eq!(parse(r#"["\ud83"]"#), None);
        assert_eq!(parse(r#"["\ud83d"#), None);
    }

    /// A state.json TS wrote for a setup whose stored output (`output.slice(-4096)`) starts inside a surrogate pair.
    fn state_with_lone_surrogate(status: &str) -> String {
        format!(
            r#"{{"sessions":[{{"id":"id","title":"t","program":"claude","command":"claude","cwd":"/r","repoRoot":"/r","status":"{status}","createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","setup":{{"command":"yarn","state":"failed","output":"\udd0d Resolving packages"}}}}],"worktrees":[]}}"#
        )
    }

    #[test]
    fn load_state_keeps_sessions_holding_lone_surrogates() {
        let home = temp_home();
        let _home = Home::set(home.path());
        std::fs::write(home.path().join("state.json"), state_with_lone_surrogate("exited")).unwrap();
        let state = block_on(load_state()).expect("loaded");
        assert_eq!(state.sessions.len(), 1);
        assert_eq!(
            state.sessions[0].setup.as_ref().map(|setup| setup.output.as_str()),
            Some("\u{FFFD} Resolving packages")
        );
        let mut files: Vec<String> = std::fs::read_dir(home.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        assert_eq!(files, ["state.json"]);
    }

    #[test]
    fn fractional_sidebar_order_loads_and_is_written_back_as_read() {
        let home = temp_home();
        let _home = Home::set(home.path());
        let _determinism = Determinism::install();
        let session = |id: &str, order: &str| {
            format!(
                r#"{{"id":"{id}","title":"t","program":"claude","command":"claude","cwd":"/r","repoRoot":"/r","status":"running","createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","sidebarOrder":{order}}}"#
            )
        };
        std::fs::write(
            home.path().join("state.json"),
            format!(
                r#"{{"sessions":[{},{},{}],"worktrees":[]}}"#,
                session("a", "1.5"),
                session("b", "-0.25"),
                session("c", "2.0")
            ),
        )
        .unwrap();
        let state = block_on(mark_all_non_exited_sessions_exited()).expect("recovered");
        assert_eq!(state.sessions.len(), 3);
        let written = std::fs::read_to_string(home.path().join("state.json")).unwrap();
        assert!(written.contains(r#""sidebarOrder": 1.5,"#), "{written}");
        assert!(written.contains(r#""sidebarOrder": -0.25,"#), "{written}");
        // `JSON.parse('2.0')` is 2.
        assert!(written.contains(r#""sidebarOrder": 2,"#), "{written}");
    }

    #[test]
    fn crash_recovery_keeps_sessions_holding_lone_surrogates() {
        let home = temp_home();
        let _home = Home::set(home.path());
        std::fs::write(home.path().join("state.json"), state_with_lone_surrogate("running")).unwrap();
        let state = block_on(mark_all_non_exited_sessions_exited()).expect("recovered");
        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.sessions[0].exit_reason, Some(ExitReason::Interrupted));
        assert!(
            !std::fs::read_dir(home.path())
                .unwrap()
                .any(|entry| { entry.unwrap().file_name().to_string_lossy().contains("corrupt") })
        );
    }

    #[test]
    fn config_with_lone_surrogates_loads() {
        let home = temp_home();
        let _home = Home::set(home.path());
        std::fs::write(
            home.path().join("config.json"),
            r#"{"dev_command":"npm run dev \ud83d","trustedProjects":{"/r":["fp"]}}"#,
        )
        .unwrap();
        let config = block_on(load_app_config()).expect("loaded");
        assert_eq!(config.dev_command.as_deref(), Some("npm run dev \u{FFFD}"));
        let updated = block_on(async { update_app_config(Ok).await }).expect("updated");
        assert_eq!(updated, config);
    }

    fn temp_home() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("deckhand-storage-")
            .tempdir()
            .expect("temp home")
    }

    fn session(value: Value) -> Ordered<SessionRecord> {
        let mut base = json!({
            "id": "id", "title": "t", "program": "claude", "command": "claude", "cwd": "/r", "repoRoot": "/r",
            "status": "running", "createdAt": "2026-01-01T00:00:00.000Z", "updatedAt": "2026-01-01T00:00:00.000Z"
        });
        if let (Some(base), Value::Object(extra)) = (base.as_object_mut(), value) {
            base.extend(extra);
        }
        serde_json::from_value(base).expect("session")
    }

    #[test]
    fn sort_sessions_newest_first_puts_running_first_then_oldest() {
        let sessions = vec![
            session(json!({"id": "a", "status": "exited", "createdAt": "2026-01-02"})),
            session(json!({"id": "b", "status": "running", "createdAt": "2026-01-03"})),
            session(json!({"id": "c", "status": "exited", "createdAt": "2026-01-01"})),
            session(json!({"id": "d", "status": "running", "createdAt": "2026-01-01"})),
        ];
        let ids: Vec<String> = sort_sessions_newest_first(&sessions)
            .iter()
            .map(|session| session.id.clone())
            .collect();
        assert_eq!(ids, ["d", "b", "c", "a"]);
    }

    #[test]
    fn state_file_display_path_is_relative_to_the_cwd() {
        let home = temp_home();
        let _home = Home::set(home.path());
        let expected = jsstr::path::relative(
            &display(&paths::current_dir()),
            &display(&home.path().join("state.json")),
        );
        assert_eq!(state_file_display_path(), expected);
        assert!(state_file_display_path().ends_with("state.json"));
    }

    #[test]
    fn update_failures_write_nothing_and_release_the_lock() {
        let home = temp_home();
        let _home = Home::set(home.path());
        std::fs::write(home.path().join("config.json"), r#"{"keep":1}"#).unwrap();
        let result = block_on(async { update_app_config(|_| Err(DhError::msg("nope"))).await });
        assert_eq!(result, Err(DhError::msg("nope")));
        assert_eq!(
            std::fs::read_to_string(home.path().join("config.json")).unwrap(),
            r#"{"keep":1}"#
        );
        assert!(!home.path().join("config.json.lock").exists());
        // An unparseable config.json rejects with JSON.parse's message.
        std::fs::write(home.path().join("config.json"), "{nope").unwrap();
        let result = block_on(load_app_config());
        assert_eq!(
            result.unwrap_err().to_string(),
            DhError::json_parse("{nope").to_string()
        );
    }

    // ---- tests/storage.test.ts ---------------------------------------------------------------------------------

    // covers: B-I1-01 crash recovery marks live sessions interrupted and keeps their identity, archive state and notes
    #[test]
    fn crash_recovery_marks_live_sessions_interrupted() {
        let home = temp_home();
        let _home = Home::set(home.path());
        let session = session(json!({
            "archivedAt": "yesterday", "notes": "keep",
            "agentSessionRef": {"provider": "codex", "kind": "id", "value": "native"},
            "setup": {"command": "setup", "state": "running", "output": "partial"}
        }));
        let recovered = block_on(async {
            save_state(&StateFile {
                sessions: vec![session],
                worktrees: Vec::new(),
            })
            .await?;
            mark_all_non_exited_sessions_exited().await
        })
        .expect("recovered");
        let recovered = &recovered.sessions[0];
        assert_eq!(recovered.exit_reason, Some(ExitReason::Interrupted));
        assert_eq!(
            recovered.setup.as_ref().map(|setup| setup.state),
            Some(SetupState::Failed)
        );
        assert_eq!(
            recovered
                .agent_session_ref
                .as_ref()
                .map(|reference| reference.value.as_str()),
            Some("native")
        );
        assert_eq!(recovered.archived_at.as_deref(), Some("yesterday"));
        assert_eq!(recovered.notes.as_deref(), Some("keep"));
    }

    /// A state.json whose records this version cannot all read: an unknown program and status, a fractional pid, a
    /// string exit code, a null title, an unknown merge mode; `w1` is referenced only by an unreadable session, `w3`
    /// by no session.
    const UNREADABLE_STATE: &str = r#"{"sessions": [
        {"id": "s1", "title": "one", "program": "claude", "command": "claude", "cwd": "/r", "repoRoot": "/r", "status": "exited", "createdAt": "2026-01-01T00:00:00.000Z", "updatedAt": "2026-01-01T00:00:00.000Z"},
        {"id": "gem", "title": "g", "program": "gemini", "command": "gemini", "cwd": "/r", "repoRoot": "/r", "status": "running", "pid": 5, "devRunning": true, "attention": {"state": "needs-input", "event": "Notification", "at": "x", "future": 1}, "setup": {"command": "make", "state": "running", "output": "half"}, "createdAt": "2026-01-03T00:00:00.000Z", "updatedAt": "2026-01-03T00:00:00.000Z", "futureKey": [1]},
        {"id": "frac", "title": "f", "program": "claude", "command": "claude", "cwd": "/r", "repoRoot": "/r", "status": "running", "pid": 12.5, "createdAt": "2026-01-02T00:00:00.000Z", "updatedAt": "2026-01-02T00:00:00.000Z"},
        {"id": "code", "title": "c", "program": "claude", "command": "claude", "cwd": "/r", "repoRoot": "/r", "status": "exited", "exitCode": "1", "devRunning": true, "createdAt": "2026-01-04T00:00:00.000Z", "updatedAt": "2026-01-04T00:00:00.000Z"},
        {"id": "paused", "title": "p", "program": "claude", "command": "claude", "cwd": "/r/wt", "repoRoot": "/r", "status": "paused", "exitCode": 0, "worktree": {"mode": "managed", "path": "/r/wt", "branch": "b", "id": "w1"}, "createdAt": "2026-01-05T00:00:00.000Z", "updatedAt": "2026-01-05T00:00:00.000Z"},
        {"id": "nulltitle", "title": null, "program": "claude", "command": "claude", "cwd": "/r/wt2", "repoRoot": "/r", "status": "exited", "worktree": {"mode": "managed", "path": "/r/wt2", "branch": "c", "id": "w2"}, "createdAt": "2026-01-06T00:00:00.000Z", "updatedAt": "2026-01-06T00:00:00.000Z"}
    ], "worktrees": [
        {"id": "w1", "path": "/r/wt", "createdAt": "2026-01-05T00:00:00.000Z"},
        {"id": "w2", "path": "/r/wt2", "createdAt": "2026-01-06T00:00:00.000Z", "mergeMode": "octopus"},
        {"id": "w3", "path": "/r/wt3", "createdAt": "2026-01-06T00:00:00.000Z", "mergeMode": "octopus"}
    ]}"#;

    /// state.json, compact (key order kept).
    fn stored_state(home: &Path) -> String {
        let text = std::fs::read_to_string(home.join("state.json")).expect("state.json");
        serde_json::to_string(&serde_json::from_str::<Value>(&text).expect("json")).expect("compact")
    }

    /// TS readState keeps every element of `sessions`/`worktrees` as it is: a record this version cannot read is
    /// recovered and written back like any other (expected texts recorded from src/storage.ts
    /// markAllNonExitedSessionsExited, then saveState of sortSessionsNewestFirst as InkDaemon.persist does), never
    /// erased from state.json.
    #[test]
    fn unreadable_records_are_kept_through_recovery_and_saves() {
        let home = temp_home();
        let _home = Home::set(home.path());
        let _determinism = Determinism::install();
        std::fs::write(home.path().join("state.json"), UNREADABLE_STATE).unwrap();
        let recovered = block_on(mark_all_non_exited_sessions_exited()).expect("recovered");
        let ids: Vec<&str> = recovered.sessions.iter().map(|session| session.id.as_str()).collect();
        assert_eq!(ids, ["s1"]);
        assert_eq!(
            stored_state(home.path()),
            r#"{"sessions":[{"id":"s1","title":"one","program":"claude","command":"claude","cwd":"/r","repoRoot":"/r","status":"exited","createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"},{"id":"gem","title":"g","program":"gemini","command":"gemini","cwd":"/r","repoRoot":"/r","status":"exited","devRunning":false,"attention":{"state":"unknown","event":"DaemonRestart","at":"2026-01-01T00:00:00.000Z"},"setup":{"command":"make","state":"failed","output":"half\nInterrupted by daemon restart"},"createdAt":"2026-01-03T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","futureKey":[1],"exitCode":null,"exitSignal":null,"exitReason":"interrupted"},{"id":"frac","title":"f","program":"claude","command":"claude","cwd":"/r","repoRoot":"/r","status":"exited","createdAt":"2026-01-02T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","exitCode":null,"exitSignal":null,"devRunning":false,"exitReason":"interrupted","attention":{"state":"unknown","event":"DaemonRestart","at":"2026-01-01T00:00:00.000Z"}},{"id":"code","title":"c","program":"claude","command":"claude","cwd":"/r","repoRoot":"/r","status":"exited","exitCode":"1","devRunning":false,"createdAt":"2026-01-04T00:00:00.000Z","updatedAt":"2026-01-04T00:00:00.000Z"},{"id":"paused","title":"p","program":"claude","command":"claude","cwd":"/r/wt","repoRoot":"/r","status":"exited","exitCode":0,"worktree":{"mode":"managed","path":"/r/wt","branch":"b","id":"w1"},"createdAt":"2026-01-05T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","exitSignal":null,"devRunning":false,"exitReason":"interrupted","attention":{"state":"unknown","event":"DaemonRestart","at":"2026-01-01T00:00:00.000Z"}},{"id":"nulltitle","title":null,"program":"claude","command":"claude","cwd":"/r/wt2","repoRoot":"/r","status":"exited","worktree":{"mode":"managed","path":"/r/wt2","branch":"c","id":"w2"},"createdAt":"2026-01-06T00:00:00.000Z","updatedAt":"2026-01-06T00:00:00.000Z"}],"worktrees":[{"id":"w1","path":"/r/wt","createdAt":"2026-01-05T00:00:00.000Z"},{"id":"w2","path":"/r/wt2","createdAt":"2026-01-06T00:00:00.000Z","mergeMode":"octopus"}]}"#
        );
        // The daemon's persist (sessions newest first) writes them back too.
        block_on(save_state(&StateFile {
            sessions: sort_sessions_newest_first(&recovered.sessions),
            worktrees: recovered.worktrees.clone(),
        }))
        .expect("saved");
        let persisted = r#"{"sessions":[{"id":"s1","title":"one","program":"claude","command":"claude","cwd":"/r","repoRoot":"/r","status":"exited","createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"},{"id":"frac","title":"f","program":"claude","command":"claude","cwd":"/r","repoRoot":"/r","status":"exited","createdAt":"2026-01-02T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","exitCode":null,"exitSignal":null,"devRunning":false,"exitReason":"interrupted","attention":{"state":"unknown","event":"DaemonRestart","at":"2026-01-01T00:00:00.000Z"}},{"id":"gem","title":"g","program":"gemini","command":"gemini","cwd":"/r","repoRoot":"/r","status":"exited","devRunning":false,"attention":{"state":"unknown","event":"DaemonRestart","at":"2026-01-01T00:00:00.000Z"},"setup":{"command":"make","state":"failed","output":"half\nInterrupted by daemon restart"},"createdAt":"2026-01-03T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","futureKey":[1],"exitCode":null,"exitSignal":null,"exitReason":"interrupted"},{"id":"code","title":"c","program":"claude","command":"claude","cwd":"/r","repoRoot":"/r","status":"exited","exitCode":"1","devRunning":false,"createdAt":"2026-01-04T00:00:00.000Z","updatedAt":"2026-01-04T00:00:00.000Z"},{"id":"paused","title":"p","program":"claude","command":"claude","cwd":"/r/wt","repoRoot":"/r","status":"exited","exitCode":0,"worktree":{"mode":"managed","path":"/r/wt","branch":"b","id":"w1"},"createdAt":"2026-01-05T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","exitSignal":null,"devRunning":false,"exitReason":"interrupted","attention":{"state":"unknown","event":"DaemonRestart","at":"2026-01-01T00:00:00.000Z"}},{"id":"nulltitle","title":null,"program":"claude","command":"claude","cwd":"/r/wt2","repoRoot":"/r","status":"exited","worktree":{"mode":"managed","path":"/r/wt2","branch":"c","id":"w2"},"createdAt":"2026-01-06T00:00:00.000Z","updatedAt":"2026-01-06T00:00:00.000Z"}],"worktrees":[{"id":"w1","path":"/r/wt","createdAt":"2026-01-05T00:00:00.000Z"},{"id":"w2","path":"/r/wt2","createdAt":"2026-01-06T00:00:00.000Z","mergeMode":"octopus"}]}"#;
        assert_eq!(stored_state(home.path()), persisted);
        // A later daemon start reads them again (the recovery made two readable): nothing changes, nothing is lost.
        let again = block_on(mark_all_non_exited_sessions_exited()).expect("recovered again");
        let ids: Vec<&str> = again.sessions.iter().map(|session| session.id.as_str()).collect();
        assert_eq!(ids, ["s1", "frac", "paused"]);
        assert_eq!(stored_state(home.path()), persisted);
        // A change to a readable session (archiving it) keeps the others.
        let mut sessions = again.sessions.clone();
        sessions[0].archived_at = Some("2026-01-07T00:00:00.000Z".into());
        block_on(save_state(&StateFile {
            sessions,
            worktrees: again.worktrees,
        }))
        .expect("saved");
        let stored: Value = serde_json::from_str(&stored_state(home.path())).unwrap();
        let ids: Vec<&str> = stored["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|session| session["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["s1", "frac", "gem", "code", "paused", "nulltitle"]);
        assert_eq!(stored["sessions"][0]["archivedAt"], "2026-01-07T00:00:00.000Z");
    }

    /// A rewrite of an empty or missing state.json forgets what an earlier read kept.
    #[test]
    fn a_reset_state_forgets_unreadable_records() {
        let home = temp_home();
        let _home = Home::set(home.path());
        std::fs::write(home.path().join("state.json"), UNREADABLE_STATE).unwrap();
        block_on(load_state()).expect("loaded");
        std::fs::write(home.path().join("state.json"), "  \n").unwrap();
        let state = block_on(load_state()).expect("loaded");
        block_on(save_state(&state)).expect("saved");
        assert_eq!(stored_state(home.path()), r#"{"sessions":[],"worktrees":[]}"#);
    }

    // covers: B-I1-02 config is private, keeps unknown keys without writing absent defaults, and reads legacy single-fingerprint trust
    #[test]
    fn config_is_private_and_keeps_unknown_keys() {
        let home = temp_home();
        let _home = Home::set(home.path());
        let file = home.path().join("config.json");
        std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        block_on(ensure_config_dir()).unwrap();
        assert_eq!(std::fs::metadata(home.path()).unwrap().mode() & 0o777, 0o700);
        std::fs::write(
            &file,
            r#"{"future_setting":{"keep":true},"trustedProjects":{"/legacy":"old"}}"#,
        )
        .unwrap();
        let config = block_on(load_app_config()).unwrap();
        assert_eq!(
            serde_json::to_value(config.trusted_projects).unwrap(),
            json!({"/legacy": ["old"]})
        );
        block_on(async {
            patch_app_config(AppConfig {
                dev_command: Some("dev".into()),
                ..AppConfig::default()
            })
            .await
        })
        .unwrap();
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(
            written,
            json!({"future_setting": {"keep": true}, "trustedProjects": {"/legacy": "old"}, "dev_command": "dev"})
        );
        assert_eq!(std::fs::metadata(&file).unwrap().mode() & 0o777, 0o600);
    }

    fn trust(root: &str) -> Task<Result<AppConfig>> {
        let root = root.to_owned();
        update_app_config(move |mut current| {
            current
                .trusted_projects
                .get_or_insert_with(Default::default)
                .insert(root, vec!["hash".into()]);
            Ok(current)
        })
    }

    // covers: B-I1-03 config updates serialize within and across processes and recover from a stale lock
    #[test]
    fn config_updates_serialize_and_recover_from_a_stale_lock() {
        let home = temp_home();
        let _home = Home::set(home.path());
        let file = home.path().join("config.json");
        let config = block_on(async {
            // Started together (TS Promise.all); the queue runs them in call order.
            let updates = [
                patch_app_config(AppConfig {
                    dev_command: Some("dev".into()),
                    ..AppConfig::default()
                }),
                trust("/one"),
                trust("/two"),
                patch_app_config(AppConfig {
                    notifications: Some(true),
                    ..AppConfig::default()
                }),
            ];
            for update in updates {
                update.await?;
            }
            load_app_config().await
        })
        .unwrap();
        assert_eq!(config.dev_command.as_deref(), Some("dev"));
        assert_eq!(config.notifications, Some(true));
        assert_eq!(
            serde_json::to_value(config.trusted_projects).unwrap(),
            json!({"/one": ["hash"], "/two": ["hash"]})
        );

        let lock = home.path().join("config.json.lock");
        std::fs::write(&lock, "").unwrap();
        let stale = std::time::SystemTime::now() - Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&lock)
            .unwrap()
            .set_modified(stale)
            .unwrap();
        // Two writers with their own queues (as two processes have): only the lockfile serializes them.
        let workers: Vec<_> = [1, 2]
            .into_iter()
            .map(|id| {
                std::thread::spawn(move || {
                    block_on(async move {
                        for i in 0..5 {
                            trust(&format!("/p{id}-{i}")).await?;
                        }
                        Ok::<_, DhError>(())
                    })
                })
            })
            .collect();
        for worker in workers {
            worker.join().expect("worker").expect("updates");
        }
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(written["trustedProjects"].as_object().map(|map| map.len()), Some(12));
        assert!(!lock.exists());
    }

    // ---- tests/worktreeRecords.test.ts (deferred to I1 by P2) --------------------------------------------------

    fn at(minute: u32) -> String {
        format!("2026-01-01T00:{minute:02}:00.000Z")
    }

    /// tests/worktreeRecords.test.ts `session(id, created, worktree, extra)`, as raw JSON.
    fn legacy(id: &str, created: u32, worktree: Value, extra: Value) -> Value {
        let cwd = worktree.get("path").cloned().unwrap_or(json!("/repo"));
        let mut value = json!({
            "id": id, "title": id, "program": "claude", "command": "claude", "cwd": cwd, "repoRoot": "/repo",
            "launchWorktreeRoot": "/repo", "worktree": worktree, "status": "exited", "createdAt": at(created),
            "updatedAt": at(created),
        });
        for (key, entry) in extra.as_object().cloned().unwrap_or_default() {
            value[key] = entry;
        }
        value
    }

    fn managed(mode: &str, extra: Value) -> Value {
        let mut value = json!({"mode": mode, "path": "/wt/feature", "branch": "feature", "isMain": false});
        for (key, entry) in extra.as_object().cloned().unwrap_or_default() {
            value[key] = entry;
        }
        value
    }

    fn legacy_sessions() -> Vec<Value> {
        vec![
            legacy(
                "a",
                1,
                managed(
                    "managed",
                    json!({"mergedAt": at(10), "mergeMode": "squash", "mergeTargetBranch": "main", "mergeSourceRef": "feature", "deletedAt": at(30)}),
                ),
                json!({}),
            ),
            legacy(
                "b",
                2,
                managed(
                    "attached",
                    json!({"mergedAt": at(20), "mergeTargetBranch": "main", "mergeSourceRef": "feature", "mergeMarkedManually": true}),
                ),
                json!({}),
            ),
            legacy(
                "child",
                3,
                json!({"mode": "none"}),
                json!({"cwd": "/wt/feature", "launchWorktreeRoot": "/wt/feature", "requestedWorktreeMode": "none", "mergedAt": at(5), "mergeTargetBranch": "main", "mergeSourceRef": "feature", "mergeMarkedManually": true}),
            ),
            legacy(
                "restarted",
                4,
                managed("attached", json!({})),
                json!({"agentStartedAt": at(45)}),
            ),
            legacy("new", 40, managed("managed", json!({})), json!({})),
            legacy(
                "other",
                5,
                managed(
                    "managed",
                    json!({"path": "/wt/other", "mergedAt": at(8), "mergeMode": "merge", "mergeTargetBranch": "main", "mergeSourceRef": "other"}),
                ),
                json!({}),
            ),
            legacy(
                "main-none",
                6,
                json!({"mode": "none"}),
                json!({"mergedAt": at(9), "mergeTargetBranch": "main", "mergeSourceRef": "main", "mergeMarkedManually": true}),
            ),
            legacy(
                "main-attached",
                7,
                json!({"mode": "attached", "path": "/repo", "isMain": true, "mergedAt": at(9), "mergeTargetBranch": "dev", "mergeSourceRef": "main", "mergeMarkedManually": true}),
                json!({}),
            ),
            legacy(
                "waiting",
                8,
                json!({"mode": "none"}),
                json!({"launchWorktreeRoot": "/wt/feature", "requestedWorktreeMode": "new"}),
            ),
        ]
    }

    // covers: B-I1-05 a legacy state.json is migrated on daemon start and written back with records only
    #[test]
    fn legacy_state_is_migrated_on_daemon_start() {
        let home = temp_home();
        let _home = Home::set(home.path());
        let file = home.path().join("state.json");
        std::fs::write(&file, json!({"sessions": legacy_sessions()}).to_string()).unwrap();
        let recovered = block_on(mark_all_non_exited_sessions_exited()).expect("recovered");
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(written, serde_json::to_value(&recovered).unwrap());
        assert_eq!(written["worktrees"].as_array().map(|records| records.len()), Some(3));
        assert!(written["sessions"].as_array().unwrap().iter().all(|item| {
            let worktree = &item["worktree"];
            worktree.get("id").is_none_or(|id| id.is_null() || id == "")
                || (worktree.get("mergedAt").is_none() && worktree.get("deletedAt").is_none())
        }));
        assert_eq!(block_on(load_state()).expect("loaded"), recovered);
    }
}
