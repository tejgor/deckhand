//! Port of configDraft.ts.
//!
//! Pure helpers for the UI's config editor.

use crate::error::Result;

/// TS: configDraft.ts MAX_CONFIG_BYTES.
pub const MAX_CONFIG_BYTES: usize = 64 * 1024;

/// TS: configDraft.ts MAX_CONFIG_LABEL.
pub const MAX_CONFIG_LABEL: &str = "64 KiB";

/// TS: configDraft.ts formatConfigJson (`JSON.stringify(JSON.parse(text), null, 2)` plus a newline; the parse error
/// is JSON.parse's message, see `DhError::json_parse`).
pub fn format_config_json(text: &str) -> Result<String> {
    let _ = text;
    todo!("P11")
}
