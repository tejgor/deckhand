//! Port of storage.ts.
//!
//! state.json (sessions and worktree records) and config.json (the user config) under the Deckhand home. Both are
//! written atomically (a temporary file in the same directory, then a rename) with mode 0600, and the home directory
//! is kept 0700. config.json is shared by the UI and daemon processes: [`update_app_config`] serializes its
//! read-modify-write in-process (a queue) and across processes (an exclusive lockfile).
//!
//! The record types are deckhand-protocol's: `InkState` is [`StateFile`], `AppConfig` is [`AppConfig`] (its
//! normalisation, `normalizeAppConfig`, is [`AppConfig::from_raw`]).

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::async_util::Task;
use deckhand_core::error::Result;
use deckhand_core::session_order::SessionLike;
use deckhand_protocol::records::{AppConfig, RawConfig, StateFile};

/// TS: storage.ts LOCK_STALE_MS (a config lock older than this was left by a crashed writer).
pub const LOCK_STALE: Duration = Duration::from_millis(5000);
/// TS: storage.ts LOCK_TIMEOUT_MS (how long a writer waits for the config lock).
pub const LOCK_TIMEOUT: Duration = Duration::from_millis(10_000);
/// Mode of every file Deckhand writes under its home.
pub const FILE_MODE: u32 = 0o600;
/// Mode of the Deckhand home and the directories Deckhand creates in it.
pub const DIR_MODE: u32 = 0o700;

/// TS: storage.ts ensureConfigDir (creates the home 0700; tightens a group/world-accessible one we own, once per
/// process and directory).
pub async fn ensure_config_dir() -> Result<()> {
    todo!("I1")
}

/// TS: storage.ts `${path}.tmp-${pid}-${Date.now()}-${randomUUID()}`: a unique temporary name beside `path`.
pub fn temporary_path(path: &Path) -> PathBuf {
    let _ = path;
    todo!("I1")
}

/// Writes `contents` to `path` atomically: a temporary file beside it (created with `mode`), then a rename over it;
/// the temporary file is removed when anything fails (storage.ts saveState/writeRawConfig, notesStore.ts
/// writeAtomically and the other temp + rename writers).
pub async fn write_atomic(path: &Path, contents: &[u8], mode: u32) -> Result<()> {
    let _ = (path, contents, mode);
    todo!("I1")
}

/// TS: storage.ts loadState.
pub async fn load_state() -> Result<StateFile> {
    todo!("I1")
}

/// TS: storage.ts readState result: the state, and whether it was migrated (written by a version before worktree
/// records or done markers) and so must be written back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadState {
    pub state: StateFile,
    pub migrated: bool,
}

/// TS: storage.ts readState (empty or missing file: an empty state is written; unparseable JSON is moved aside to
/// `state.json.corrupt-<ms>` and replaced by an empty state; otherwise worktree records and done markers are
/// migrated).
pub async fn read_state() -> Result<ReadState> {
    todo!("I1")
}

/// TS: storage.ts saveState (`JSON.stringify(state, null, 2)` plus a newline, atomically, 0600).
pub async fn save_state(state: &StateFile) -> Result<()> {
    let _ = state;
    todo!("I1")
}

/// TS: storage.ts markAllNonExitedSessionsExited (daemon start: sessions that were live are exited `interrupted`,
/// running setups failed, dev servers stopped; written back when anything changed or the state was migrated).
pub async fn mark_all_non_exited_sessions_exited() -> Result<StateFile> {
    todo!("I1")
}

/// TS: storage.ts sortSessionsNewestFirst (running first, then by `createdAt` ascending, `localeCompare`).
pub fn sort_sessions_newest_first<S: SessionLike + Clone>(sessions: &[S]) -> Vec<S> {
    let _ = sessions;
    todo!("I1")
}

/// TS: storage.ts readRawConfig (config.json as stored; missing: empty; not an object: `<path> must contain a JSON
/// object`).
pub async fn read_raw_config() -> Result<RawConfig> {
    todo!("I1")
}

/// TS: storage.ts writeRawConfig (atomic, 0600).
pub async fn write_raw_config(config: &RawConfig) -> Result<()> {
    let _ = config;
    todo!("I1")
}

/// TS: storage.ts loadAppConfig (the normalised view; an empty file gives an empty config).
pub async fn load_app_config() -> Result<AppConfig> {
    todo!("I1")
}

/// TS: storage.ts withConfigLock: runs `operation` holding `config.json.lock` (exclusive create; a lock older than
/// LOCK_STALE is moved aside and discarded; `Timed out waiting for <lock>` after LOCK_TIMEOUT; only our own lock is
/// released).
pub async fn with_config_lock<F, Fut, T>(operation: F) -> Result<T>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let _ = operation;
    todo!("I1")
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
    let _ = updater;
    todo!("I1")
}

/// TS: storage.ts updateAppConfig with a patch object (`{...current, ...patch}`: the patch's `Some` fields win).
pub fn patch_app_config(patch: AppConfig) -> Task<Result<AppConfig>> {
    let _ = patch;
    todo!("I1")
}

/// TS: storage.ts stateFileDisplayPath (state.json relative to the cwd, else absolute).
pub fn state_file_display_path() -> String {
    todo!("I1")
}
