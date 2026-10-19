//! Port of projectConfig.ts (file I/O parts).
//!
//! Reading the repository override (the main checkout's live deckhand.json; copies inside linked worktrees are
//! ignored) together with the creation hook, which are trusted together, and the trust decision for a deckhand.json
//! saved through Deckhand. Validation, fingerprints and effective settings are pure and live in
//! `deckhand_core::project_config`.

use std::path::Path;

use deckhand_core::error::Result;
use deckhand_core::project_config::UserSettings;
use deckhand_protocol::data::{LoadedProject, SaveTrust};

use crate::git::RepoContext;

/// TS: projectConfig.ts readBoundedUtf8 result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundedFile {
    /// The bytes read (a BOM kept, so the text round-trips to the hashed bytes).
    pub bytes: Vec<u8>,
    pub text: String,
    /// Permission bits (`mode & 0o777`).
    pub mode: u32,
}

/// TS: projectConfig.ts readBoundedUtf8 options.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadBounded {
    /// Refuse a symlink (`O_NOFOLLOW`: `<name> must not be a symlink`, code `ELOOP`).
    pub no_follow: bool,
    /// Bytes allowed at most (`<name> exceeds <max/1024 rounded> KiB`).
    pub max: usize,
}

/// TS: projectConfig.ts readBoundedUtf8 (strict UTF-8; a non-blocking open so a FIFO never hangs; `<name> must be a
/// regular file`; fs errors keep their code, e.g. `ENOENT`).
pub async fn read_bounded_utf8(file: &Path, options: ReadBounded) -> Result<BoundedFile> {
    let _ = (file, options);
    todo!("I6")
}

/// TS: projectConfig.ts loadProjectConfig: the main checkout's deckhand.json (errors prefixed `<file>: `) and the
/// creation hook (enabled unless `worktree.hook: false` in the file or the global defaults; read, fingerprinted).
/// `user` is the user config (default empty): every caller that compares fingerprints must pass the same one.
/// `context`: the repository context when the caller has it (else resolved from `cwd`).
pub async fn load_project_config(
    cwd: &str,
    user: Option<&UserSettings>,
    context: Option<&RepoContext>,
) -> Result<LoadedProject> {
    let _ = (cwd, user, context);
    todo!("I6")
}

/// The bytes a save replaced (TS savedProjectTrust `previous`): exactly what the user edited from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviousDocument {
    pub raw: String,
    pub exists: bool,
}

/// TS: projectConfig.ts savedProjectTrust result: whether the new bytes stay (or become) trusted, and the fingerprint
/// to add to the trusted list when they do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedTrust {
    pub trust: SaveTrust,
    pub fingerprint: Option<String>,
}

/// TS: projectConfig.ts savedProjectTrust (self-edits keep trust only when the whole previous bundle was trusted and
/// the creation hook does not join the bundle through this save).
pub async fn saved_project_trust(
    context: &RepoContext,
    previous: &PreviousDocument,
    next: &str,
    user: &UserSettings,
) -> Result<SavedTrust> {
    let _ = (context, previous, next, user);
    todo!("I6")
}
