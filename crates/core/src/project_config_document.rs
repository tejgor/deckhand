//! Config documents (a layer's raw JSON and its revision): the pure parts (file I/O is in
//! deckhand_sys::config_document).
//!
//! C's two config documents: the global defaults (config.json `defaults`) and the repository's deckhand.json. Reading
//! and saving them is I/O and lives in `deckhand_sys::config_document`; the document types are deckhand-protocol's.
//!
//! Global defaults live inside config.json, so their document is the pretty-printed `defaults` value and its revision
//! hashes that value: a concurrent change to `defaults` rejects a stale draft, other keys do not.

use serde_json::Value;

use crate::config_draft::{MAX_CONFIG_BYTES, MAX_CONFIG_LABEL, format_config_json};
use crate::error::{DhError, Result};
use crate::jsstr::path;
use crate::project_config::sha256;
use deckhand_protocol::data::{ConfigTargetKind, ProjectConfigDocument};

/// TS: projectConfigDocument.ts STARTER_PROJECT_CONFIG.
pub const STARTER_PROJECT_CONFIG: &str =
    "{\n  \"defaultAgent\": \"claude\",\n  \"defaultWorkspace\": \"new\",\n  \"actions\": {}\n}\n";

/// TS: projectConfigDocument.ts checkDraft. Errors: `Configuration must be text of at most 64 KiB`, `Invalid
/// configuration revision`.
pub fn check_draft(raw: &str, expected_revision: Option<&str>) -> Result<()> {
    if raw.len() > MAX_CONFIG_BYTES {
        return Err(DhError::msg(format!(
            "Configuration must be text of at most {MAX_CONFIG_LABEL}"
        )));
    }
    if let Some(revision) = expected_revision
        && !(revision.len() == 64 && revision.bytes().all(|byte| matches!(byte, b'a'..=b'f' | b'0'..=b'9')))
    {
        return Err(DhError::msg("Invalid configuration revision"));
    }
    Ok(())
}

/// `JSON.stringify(value, null, 2)` with JS semantics (number formatting, integer-like keys first).
fn stringify_pretty(value: &Value) -> String {
    let text = serde_json::to_string(value).unwrap_or_else(|_| String::from("null"));
    let mut pretty = format_config_json(&text).unwrap_or(text);
    if pretty.ends_with('\n') {
        pretty.pop();
    }
    pretty
}

/// `JSON.stringify(value)`: the pretty form without the whitespace between tokens (strings never hold a raw newline,
/// so everything outside them that is a space or newline is layout).
fn stringify(value: &Value) -> String {
    let pretty = stringify_pretty(value);
    let mut out = String::with_capacity(pretty.len());
    let (mut in_string, mut escaped) = (false, false);
    for ch in pretty.chars() {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
        } else if ch == '"' {
            in_string = true;
            out.push(ch);
        } else if ch != ' ' && ch != '\n' {
            out.push(ch);
        }
    }
    out
}

/// TS: projectConfigDocument.ts globalRevision (sha256 of `JSON.stringify(defaults)`; `None` without defaults).
pub fn global_revision(value: Option<&Value>) -> Option<String> {
    value.map(|value| sha256(stringify(value).as_bytes()))
}

/// TS: projectConfigDocument.ts globalDocument (`config_dir`: DECKHAND_HOME; its `config.json` is the path).
pub fn global_document(value: Option<&Value>, config_dir: &str) -> ProjectConfigDocument {
    ProjectConfigDocument {
        kind: ConfigTargetKind::Global,
        root: config_dir.to_owned(),
        path: path::join(&[config_dir, "config.json"]),
        raw: match value {
            Some(value) => format!("{}\n", stringify_pretty(value)),
            None => STARTER_PROJECT_CONFIG.to_owned(),
        },
        revision: global_revision(value),
        exists: value.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn starter_is_the_pretty_printed_default() {
        let starter = json!({"defaultAgent": "claude", "defaultWorkspace": "new", "actions": {}});
        assert_eq!(format!("{}\n", stringify_pretty(&starter)), STARTER_PROJECT_CONFIG);
    }

    #[test]
    fn drafts_are_bounded_and_revisions_are_hex() {
        assert_eq!(check_draft("{}", None), Ok(()));
        assert_eq!(check_draft("{}", Some(&"a1".repeat(32))), Ok(()));
        assert_eq!(
            check_draft(&"x".repeat(MAX_CONFIG_BYTES + 1), None)
                .unwrap_err()
                .to_string(),
            "Configuration must be text of at most 64 KiB"
        );
        // Bytes, not characters: 3-byte characters reach the limit sooner.
        assert!(check_draft(&"中".repeat(MAX_CONFIG_BYTES / 3 + 1), None).is_err());
        for revision in [String::new(), "A1".repeat(32), "a".repeat(63), "g".repeat(64)] {
            assert_eq!(
                check_draft("{}", Some(&revision)).unwrap_err().to_string(),
                "Invalid configuration revision"
            );
        }
    }

    #[test]
    fn global_document_hashes_the_defaults_value() {
        let missing = global_document(None, "/home/me/.deckhand");
        assert_eq!(
            (missing.kind, missing.root.as_str(), missing.path.as_str()),
            (
                ConfigTargetKind::Global,
                "/home/me/.deckhand",
                "/home/me/.deckhand/config.json"
            )
        );
        assert_eq!(
            (missing.raw.as_str(), missing.revision, missing.exists),
            (STARTER_PROJECT_CONFIG, None, false)
        );
        let defaults = json!({"devCommand": "npm run dev", "actions": {"b": "x", "1": "y"}, "n": 1.5e21});
        let document = global_document(Some(&defaults), "/h");
        assert_eq!(
            document.raw,
            "{\n  \"devCommand\": \"npm run dev\",\n  \"actions\": {\n    \"1\": \"y\",\n    \"b\": \"x\"\n  },\n  \"n\": 1.5e+21\n}\n"
        );
        // JSON.stringify(defaults): no layout, JS number formatting and key order; spaces inside strings stay.
        assert_eq!(
            document.revision.as_deref(),
            Some(sha256(br#"{"devCommand":"npm run dev","actions":{"1":"y","b":"x"},"n":1.5e+21}"#).as_str())
        );
        assert!(document.exists);
        let null = global_document(Some(&Value::Null), "/h");
        assert_eq!((null.raw.as_str(), null.revision), ("null\n", Some(sha256(b"null"))));
        assert_eq!(
            global_revision(Some(&json!({"a": "x \\\" \n y"}))),
            Some(sha256(br#"{"a":"x \\\" \n y"}"#))
        );
    }
}
