//! Port of sessionFeatures.ts (file I/O parts).
//!
//! Exporting a handoff: the Markdown (pure, `deckhand_core::session_features::handoff_markdown`) written to
//! `<home>/handoffs/<session id>-<8 hex>.md` (directory 0700, file 0600, never overwriting).

use std::path::PathBuf;

use deckhand_core::error::Result;
use deckhand_core::session_features::{HandoffGitContext, HandoffTask};
use deckhand_protocol::data::SessionRecord;

/// TS: sessionFeatures.ts exportHandoff (returns the file written).
pub async fn export_handoff(
    session: &SessionRecord,
    include_output: bool,
    git: Option<&HandoffGitContext>,
    task: Option<&HandoffTask>,
) -> Result<PathBuf> {
    let _ = (session, include_output, git, task);
    todo!("I7")
}
