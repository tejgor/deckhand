//! Shows the UI's "older daemon still running" notice once per daemon.
//!
//! The notice is informational (the daemon is compatible), so it must not come back on every launch or reconnect. The
//! file `daemon-notice` in the state directory holds the key (`deckhand_core::daemon_build::notice_key`: daemon pid,
//! daemon build, client build) the notice was last shown for. It is its own file so that none of the formats other
//! Deckhand versions read (state.json, config.json, ui-state.json) gains a key.

use std::path::Path;

use crate::{paths, storage};

/// Claims the notice for `key` in [`paths::daemon_notice_path`]: see [`claim_notice_at`].
pub async fn claim_notice(key: &str) -> bool {
    claim_notice_at(&paths::daemon_notice_path(), key).await
}

/// `true` when the notice was not shown for `key` yet, recording it as shown; `false` when it was. When the marker
/// cannot be written the notice still shows, on every launch rather than never.
pub async fn claim_notice_at(path: &Path, key: &str) -> bool {
    if let Ok(shown) = tokio::fs::read_to_string(path).await
        && shown.trim_end() == key
    {
        return false;
    }
    let _ = storage::write_atomic(path, format!("{key}\n").as_bytes(), 0o600).await;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| panic!("{error}"))
            .block_on(future)
    }

    #[test]
    fn the_notice_is_claimed_once_per_key() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = dir.path().join("daemon-notice");
        let first = "pid=42 daemon=3.0.0 client=3.0.1";
        run(async {
            assert!(claim_notice_at(&path, first).await);
            assert!(!claim_notice_at(&path, first).await);
            // Another daemon process, or another build on either side, shows it again (once).
            let restarted = "pid=43 daemon=3.0.0 client=3.0.1";
            assert!(claim_notice_at(&path, restarted).await);
            assert!(!claim_notice_at(&path, restarted).await);
            assert!(claim_notice_at(&path, first).await);
        });
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(text, format!("{first}\n"));
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).map(|meta| meta.permissions().mode() & 0o777);
        assert_eq!(mode.ok(), Some(0o600));
    }

    #[test]
    fn an_unwritable_marker_still_shows_the_notice() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = dir.path().join("missing-dir").join("daemon-notice");
        run(async {
            assert!(claim_notice_at(&path, "k").await);
            assert!(claim_notice_at(&path, "k").await);
        });
    }
}
