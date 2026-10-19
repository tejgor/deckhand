//! Port of uiState.ts.
//!
//! Per-repository UI preferences in `ui-state.json` (repository root → [`UiState`]). They are a convenience: an
//! unreadable file falls back to defaults, and saves are serialized in-process (one queue) and written atomically.
//! The record and its normalisation (`normalizeUiState`) are deckhand-protocol's [`UiState`] /
//! [`UiState::normalize`].

use crate::async_util::Task;
use deckhand_core::error::Result;
use deckhand_protocol::records::{UiState, UiStateFile};

/// TS: uiState.ts readAll (`ui-state.json`; missing or unparseable: empty; not an object: empty).
pub async fn read_all() -> Result<UiStateFile> {
    todo!("I1")
}

/// TS: uiState.ts loadUiState (the entry of the resolved `repo_root`, normalised; defaults on any error).
pub async fn load_ui_state(repo_root: &str) -> UiState {
    let _ = repo_root;
    todo!("I1")
}

/// TS: uiState.ts saveUiState: normalises a snapshot now and queues the write (one in-process queue, so saves land
/// in call order even when not awaited): reads the file, replaces the entry of the resolved `repo_root`, writes it
/// back atomically with mode 0600. Spawned at once on the current `LocalSet`.
pub fn save_ui_state(repo_root: &str, state: &UiState) -> Task<Result<()>> {
    let _ = (repo_root, state);
    todo!("I1")
}
