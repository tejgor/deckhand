//! Port of projectConfigDocument.ts (pure parts).
//!
//! C's two config documents: the global defaults (config.json `defaults`) and the repository's deckhand.json. Reading
//! and saving them is I/O and lives in `deckhand_sys::config_document`; the document types are deckhand-protocol's.

use serde_json::Value;

use crate::error::Result;
use deckhand_protocol::data::ProjectConfigDocument;

/// TS: projectConfigDocument.ts STARTER_PROJECT_CONFIG.
pub const STARTER_PROJECT_CONFIG: &str =
    "{\n  \"defaultAgent\": \"claude\",\n  \"defaultWorkspace\": \"new\",\n  \"actions\": {}\n}\n";

/// TS: projectConfigDocument.ts checkDraft. Errors: `Configuration must be text of at most 64 KiB`, `Invalid
/// configuration revision`.
pub fn check_draft(raw: &str, expected_revision: Option<&str>) -> Result<()> {
    let _ = (raw, expected_revision);
    todo!("P10")
}

/// TS: projectConfigDocument.ts globalRevision (sha256 of `JSON.stringify(defaults)`; `None` without defaults).
pub fn global_revision(value: Option<&Value>) -> Option<String> {
    let _ = value;
    todo!("P10")
}

/// TS: projectConfigDocument.ts globalDocument (`config_dir`: DECKHAND_HOME; its `config.json` is the path).
pub fn global_document(value: Option<&Value>, config_dir: &str) -> ProjectConfigDocument {
    let _ = (value, config_dir);
    todo!("P10")
}
