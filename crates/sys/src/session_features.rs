//! Port of sessionFeatures.ts (file I/O parts).
//!
//! Exporting a handoff: the Markdown (pure, `deckhand_core::session_features::handoff_markdown`) written to
//! `<home>/handoffs/<session id>-<8 hex>.md` (directory 0700, file 0600, never overwriting).

use std::path::PathBuf;

use tokio::io::AsyncWriteExt;

use deckhand_core::error::{DhError, Result};
use deckhand_core::session_features::{HandoffGitContext, HandoffTask, handoff_markdown};
use deckhand_protocol::data::SessionRecord;

use crate::paths;

/// TS: sessionFeatures.ts exportHandoff (returns the file written).
pub async fn export_handoff(
    session: &SessionRecord,
    include_output: bool,
    git: Option<&HandoffGitContext>,
    task: Option<&HandoffTask>,
) -> Result<PathBuf> {
    let directory = paths::config_dir().join("handoffs");
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder
        .create(&directory)
        .await
        .map_err(|error| DhError::io(&error, "mkdir", &directory.to_string_lossy()))?;
    let id = uuid::Uuid::new_v4().to_string();
    let file = directory.join(format!("{}-{}.md", session.id, id.get(..8).unwrap_or(&id)));
    let display = file.to_string_lossy().into_owned();
    // flag 'wx': never overwrite an earlier export.
    let mut handle = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&file)
        .await
        .map_err(|error| DhError::io(&error, "open", &display))?;
    let markdown = handoff_markdown(session, include_output, git, task);
    handle
        .write_all(markdown.as_bytes())
        .await
        .map_err(|error| DhError::io(&error, "write", &display))?;
    handle
        .flush()
        .await
        .map_err(|error| DhError::io(&error, "write", &display))?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test(flavor = "current_thread")]
    async fn export_handoff_writes_a_private_file_per_export() {
        let session: SessionRecord = serde_json::from_value(serde_json::json!({
            "id": "s-1", "title": "Fix it", "program": "claude", "command": "claude", "cwd": "/w", "repoRoot": "/w",
            "status": "running", "createdAt": "2026-01-01T00:00:00.000Z", "updatedAt": "2026-01-01T00:00:00.000Z",
        }))
        .unwrap();
        let first = export_handoff(&session, false, None, None).await.unwrap();
        let second = export_handoff(&session, false, None, None).await.unwrap();
        assert_ne!(first, second);
        let directory = paths::config_dir().join("handoffs");
        assert_eq!(first.parent(), Some(directory.as_path()));
        let name = first.file_name().unwrap().to_string_lossy().into_owned();
        let suffix = name
            .strip_prefix("s-1-")
            .and_then(|rest| rest.strip_suffix(".md"))
            .unwrap();
        assert!(suffix.len() == 8 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(
            std::fs::read_to_string(&first).unwrap(),
            handoff_markdown(&session, false, None, None)
        );
        assert_eq!(std::fs::metadata(&first).unwrap().permissions().mode() & 0o777, 0o600);
    }
}
