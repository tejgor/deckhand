//! Port of uiState.ts.
//!
//! Per-repository UI preferences in `ui-state.json` (repository root → [`UiState`]). They are a convenience: an
//! unreadable file falls back to defaults, and saves are serialized in-process (one queue) and written atomically.
//! The record and its normalisation (`normalizeUiState`) are deckhand-protocol's [`UiState`] /
//! [`UiState::normalize`].

use serde_json::Value;

use crate::async_util::{KeyedQueue, Task};
use crate::paths;
use crate::storage::{FILE_MODE, parse_json, read_text, write_atomic};
use deckhand_core::error::{DhError, Result};
use deckhand_protocol::records::{UiState, UiStateFile, to_ts_file_text};

thread_local! {
    /// TS `queue`: this process's saves (one LocalSet per thread) run one at a time, in call order.
    static SAVE_QUEUE: KeyedQueue<()> = KeyedQueue::new();
}

/// `path.resolve(repoRoot)`, the key of a repository's entry.
fn repo_key(repo_root: &str) -> String {
    paths::resolve(repo_root).to_string_lossy().into_owned()
}

/// TS: uiState.ts readAll (`ui-state.json`; missing or unparseable: empty; not an object: empty).
pub async fn read_all() -> Result<UiStateFile> {
    let file = paths::ui_state_path();
    let text = match read_text(&file).await {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(UiStateFile::new()),
        Err(error) if error.raw_os_error() == Some(libc::EISDIR) => {
            return Err(DhError::with_code(
                "EISDIR",
                "EISDIR: illegal operation on a directory, read",
            ));
        }
        Err(error) => return Err(DhError::io(&error, "open", &file.to_string_lossy())),
    };
    Ok(match parse_json(&text) {
        Some(Value::Object(map)) => map.into_iter().collect(),
        _ => UiStateFile::new(),
    })
}

/// TS: uiState.ts loadUiState (the entry of the resolved `repo_root`, normalised; defaults on any error).
pub async fn load_ui_state(repo_root: &str) -> UiState {
    match read_all().await {
        Ok(all) => UiState::normalize(all.get(&repo_key(repo_root)).unwrap_or(&Value::Null)),
        Err(_) => UiState::default(),
    }
}

/// TS: uiState.ts saveUiState: normalises a snapshot now and queues the write (one in-process queue, so saves land
/// in call order even when not awaited): reads the file, replaces the entry of the resolved `repo_root`, writes it
/// back atomically with mode 0600. Spawned at once on the current `LocalSet`.
pub fn save_ui_state(repo_root: &str, state: &UiState) -> Task<Result<()>> {
    // `normalizeUiState(JSON.parse(JSON.stringify(state)))`: a non-finite width becomes null, then undefined.
    let snapshot = serde_json::to_value(state).map(|value| UiState::normalize(&value));
    let key = repo_key(repo_root);
    SAVE_QUEUE.with(|queue| {
        queue.run((), move || async move {
            let snapshot = snapshot.map_err(|error| DhError::msg(error.to_string()))?;
            let dir = paths::config_dir();
            tokio::fs::DirBuilder::new()
                .recursive(true)
                .create(&dir)
                .await
                .map_err(|error| DhError::io(&error, "mkdir", &dir.to_string_lossy()))?;
            let mut all = read_all().await?;
            all.insert(
                key,
                serde_json::to_value(&snapshot).map_err(|error| DhError::msg(error.to_string()))?,
            );
            let text = to_ts_file_text(&all).map_err(|error| DhError::msg(error.to_string()))?;
            write_atomic(&paths::ui_state_path(), text.as_bytes(), FILE_MODE).await
        })
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use deckhand_parity::{Case, Outcome, replay, tags};
    use deckhand_protocol::data::RightPaneTab;
    use deckhand_protocol::records::SessionFilter;
    use serde_json::json;

    use super::*;
    use crate::storage::tests::{Determinism, Home, UNIT, block_on, replay_dir_cases, undefined_or_error};

    #[test]
    fn normalize_ui_state_parity() {
        replay(
            UNIT,
            "uiState/normalizeUiState",
            |case: &Case| -> std::result::Result<UiState, String> { Ok(UiState::normalize(&tags::untag(case.arg(0)))) },
        );
    }

    #[test]
    fn ui_state_dir_cases_parity() {
        replay_dir_cases("uiState", |_fixture, _root, op| {
            let (Ok(home), Ok(repo)) = (op.arg_str(0), op.arg_str(1)) else {
                return Outcome::throws("parity: no home or repository argument");
            };
            let _home = Home::set(Path::new(home));
            let _determinism = Determinism::install();
            block_on(async {
                match op.fn_name.as_str() {
                    "loadUiState" => Outcome::ret(load_ui_state(repo).await),
                    // The recorded state may hold what a UiState cannot (saveUiState normalises it either way).
                    "saveUiState" => {
                        undefined_or_error(save_ui_state(repo, &UiState::normalize(&tags::untag(op.arg(2)))).await)
                    }
                    other => Outcome::throws(format!("parity: unknown op {other}")),
                }
            })
        });
    }

    // covers: B-I1-04 UI state is validated and stored per repository
    #[test]
    fn ui_state_is_validated_and_stored_per_repository() {
        let home = tempfile::Builder::new()
            .prefix("deckhand-storage-")
            .tempdir()
            .expect("temp home");
        let _home = Home::set(home.path());
        let state = UiState::normalize(&json!({
            "activeTab": "notes", "selectedId": "id", "sessionFilter": "archived", "sessionQuery": "search",
            "sessionTabs": {"id": "notes", "bad": "invalid"}, "collapsedSessionIds": ["id", 123]
        }));
        assert_eq!(state.collapsed_session_ids, ["id"]);
        assert_eq!(
            serde_json::to_value(&state.session_tabs).unwrap(),
            json!({"id": "notes"})
        );
        assert_eq!(
            UiState::normalize(&json!({"activeTab": ["notes"], "sessionTabs": {"bad": ["notes"]}})).active_tab,
            None
        );
        let (one, two) = block_on(async {
            let saves = [
                save_ui_state("/one", &state),
                save_ui_state("/two", &UiState::normalize(&json!({"activeTab": "git"}))),
            ];
            for save in saves {
                save.await?;
            }
            Ok::<_, DhError>((load_ui_state("/one").await, load_ui_state("/two").await))
        })
        .unwrap();
        assert_eq!(one.selected_id.as_deref(), Some("id"));
        assert_eq!(one.session_filter, SessionFilter::Archived);
        assert_eq!(one.session_query, "search");
        assert_eq!(two.active_tab, Some(RightPaneTab::Git));
    }

    // TS caps sessionQuery with `slice(0, 4096)`, which can end on a high surrogate that JSON.stringify writes as a
    // lone escape; TS reads that file back, so it must not count as unparseable (which would drop every repository).
    #[test]
    fn ui_state_with_lone_surrogates_keeps_every_repository() {
        let home = tempfile::Builder::new()
            .prefix("deckhand-storage-")
            .tempdir()
            .expect("temp home");
        let _home = Home::set(home.path());
        std::fs::write(
            home.path().join("ui-state.json"),
            r#"{"/other":{"selectedId":"keep"},"/r":{"selectedId":"x","sessionQuery":"a\ud83d"}}"#,
        )
        .unwrap();
        let (other, r) = block_on(async { (load_ui_state("/other").await, load_ui_state("/r").await) });
        assert_eq!(other.selected_id.as_deref(), Some("keep"));
        assert_eq!(r.session_query, "a\u{FFFD}");
        block_on(async { save_ui_state("/new", &UiState::normalize(&json!({"selectedId": "n"}))).await }).unwrap();
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(home.path().join("ui-state.json")).unwrap()).unwrap();
        let keys: Vec<&String> = written.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["/other", "/r", "/new"]);
        assert_eq!(written["/r"]["sessionQuery"], "a\u{FFFD}");
    }
}
