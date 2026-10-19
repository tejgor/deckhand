//! Port of projectConfigDocument.ts (file I/O parts).
//!
//! C's two editable documents: the user's global defaults (config.json `defaults`, pretty-printed; its revision hashes
//! that value) and the repository's deckhand.json in the main checkout (its revision hashes the file's bytes). Saves
//! are revision-checked, serialized per file, written atomically, and keep trust for edits made through Deckhand. The
//! pure parts (checkDraft, globalRevision, globalDocument, the starter text) are in
//! `deckhand_core::project_config_document`; the document types are deckhand-protocol's.

use std::path::Path;

use deckhand_core::error::Result;
use deckhand_protocol::data::{ConfigTargets, ProjectConfigDocument, SavedConfigDocument};

use crate::git::RepoContext;

/// TS: projectConfigDocument.ts readConfigTargets (each target stays available when the other cannot be read; its
/// error message is reported instead).
pub async fn read_config_targets(cwd: &str) -> ConfigTargets {
    let _ = cwd;
    todo!("I6")
}

/// TS: projectConfigDocument.ts repositoryContext (`Bare repositories have no main checkout, so no repository
/// deckhand.json; use global defaults`). The returned context always has `main_root`.
pub async fn repository_context(cwd: &str) -> Result<RepoContext> {
    let _ = cwd;
    todo!("I6")
}

/// TS: projectConfigDocument.ts readDocumentAt result: the document and the file's mode (0644 for a new file).
#[derive(Clone, Debug, PartialEq)]
pub struct DocumentAt {
    pub document: ProjectConfigDocument,
    pub mode: u32,
}

/// TS: projectConfigDocument.ts readDocumentAt (malformed JSON is allowed so it can be repaired; missing: the
/// starter text with a `null` revision; a symlink: `Refusing to edit a symlinked deckhand.json; edit its target
/// explicitly`).
pub async fn read_document_at(root: &str) -> Result<DocumentAt> {
    let _ = root;
    todo!("I6")
}

/// TS: projectConfigDocument.ts readProjectConfigDocument (always the main checkout's).
pub async fn read_project_config_document(cwd: &str) -> Result<ProjectConfigDocument> {
    let _ = cwd;
    todo!("I6")
}

/// TS: projectConfigDocument.ts createExclusive (a hard link that never overwrites; an exclusive copy where the
/// filesystem has no hard links).
pub async fn create_exclusive(temporary: &Path, file: &Path) -> Result<()> {
    let _ = (temporary, file);
    todo!("I6")
}

/// TS: projectConfigDocument.ts saveProjectConfigDocument options.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SaveDocumentOptions {
    /// A save made through Deckhand: decide trust from exactly the replaced bytes (see `config::saved_project_trust`)
    /// and trust the new bytes when allowed.
    pub keep_trust: bool,
}

/// TS: projectConfigDocument.ts saveProjectConfigDocument (writes the repository's deckhand.json if it still has
/// `expected_revision` (`None`: must not exist), else `deckhand.json changed on disk. Your draft is kept; close and
/// reopen the editor before saving.`; never replaces a valid file with an invalid draft; serialized per file).
pub async fn save_project_config_document(
    cwd: &str,
    raw: &str,
    expected_revision: Option<&str>,
    options: SaveDocumentOptions,
) -> Result<SavedConfigDocument> {
    let _ = (cwd, raw, expected_revision, options);
    todo!("I6")
}

/// TS: projectConfigDocument.ts readGlobalDefaultsDocument.
pub async fn read_global_defaults_document() -> Result<ProjectConfigDocument> {
    todo!("I6")
}

/// TS: projectConfigDocument.ts saveGlobalDefaultsDocument (validated as `defaults`; `Global defaults changed on disk.
/// Your draft is kept; close and reopen the editor before saving.` when the stored value's revision differs).
pub async fn save_global_defaults_document(
    raw: &str,
    expected_revision: Option<&str>,
) -> Result<ProjectConfigDocument> {
    let _ = (raw, expected_revision);
    todo!("I6")
}
