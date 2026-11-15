//! Deckhand's own update check: asking GitHub for the latest release, the state directory's `update-check` file that
//! keeps the answer, and what `deckhand update` reads to tell how this executable was installed.
//!
//! The lookup is one plain request with curl's own User-Agent and nothing about the user or this install:
//! `curl -fsS --max-time 10 -o /dev/null -w %{redirect_url} https://github.com/tejgor/deckhand/releases/latest`, read
//! for where GitHub redirects (`…/releases/tag/v3.4.0`). It needs no API token, so GitHub's API rate limit does not
//! apply. Failing (offline, curl missing, an unexpected answer) only means no answer: the daemon logs it and keeps
//! the last one. Tests never reach the network: `DECKHAND_LATEST_RELEASE` replaces the lookup, and without it a debug
//! build or the dev channel never looks.

use std::path::{Path, PathBuf};
use std::time::Duration;

use deckhand_core::jsstr::date;
use deckhand_core::self_update::{
    CHECK_MAX_AGE, InstallFacts, LATEST_RELEASE_URL, UpdateCheck, cargo_source, receipt_binaries, version_from_redirect,
};
use deckhand_protocol::AppConfig;

use crate::agent_version_check::{RunOptions, run_bounded};
use crate::paths::{self, home_dir};
use crate::storage;

/// Replaces the lookup (a version, or anything else for a failed one), and allows the check in debug builds.
pub const LATEST_OVERRIDE_VAR: &str = "DECKHAND_LATEST_RELEASE";
/// `1` turns the background check off, whatever config.json says.
pub const NO_CHECK_VAR: &str = "DECKHAND_NO_UPDATE_CHECK";
/// How long the lookup may take.
pub const LOOKUP_TIMEOUT: Duration = Duration::from_secs(15);

/// The `update-check` file in the current state directory.
pub fn update_check_path() -> PathBuf {
    paths::config_dir().join("update-check")
}

/// The last answer kept in `path`; `None` when there is none or it cannot be read.
pub async fn read_update_check(path: &Path) -> Option<UpdateCheck> {
    let text = tokio::fs::read_to_string(path).await.ok()?;
    serde_json::from_str(&text).ok()
}

/// Keeps `check` in `path`.
pub async fn write_update_check(path: &Path, check: &UpdateCheck) -> deckhand_core::error::Result<()> {
    let text = serde_json::to_string(check).unwrap_or_default();
    storage::write_atomic(path, format!("{text}\n").as_bytes(), 0o644).await
}

/// Whether the daemon looks for a new release in the background: not when `config` turns it off (`update_check:
/// false`) or [`NO_CHECK_VAR`] is `1`, and in a debug build or the dev channel only with [`LATEST_OVERRIDE_VAR`] set.
pub fn background_check_enabled(config: &AppConfig) -> bool {
    if config.update_check == Some(false) || std::env::var(NO_CHECK_VAR).as_deref() == Ok("1") {
        return false;
    }
    std::env::var_os(LATEST_OVERRIDE_VAR).is_some()
        || !(cfg!(debug_assertions) || paths::is_dev_channel() || paths::is_dev_runtime())
}

/// The latest release's version, or why it is unknown.
pub async fn fetch_latest() -> Result<String, String> {
    if let Some(answer) = std::env::var_os(LATEST_OVERRIDE_VAR) {
        let answer = answer.to_string_lossy().into_owned();
        return version_from_redirect(&format!(
            "{}/tag/v{answer}",
            LATEST_RELEASE_URL.trim_end_matches("/latest")
        ))
        .ok_or_else(|| format!("no release ({LATEST_OVERRIDE_VAR}={answer})"));
    }
    let options = RunOptions {
        timeout: LOOKUP_TIMEOUT,
        limit: 2000,
        cwd: None,
    };
    let args = [
        "-fsS",
        "--max-time",
        "10",
        "-o",
        "/dev/null",
        "-w",
        "%{redirect_url}",
        LATEST_RELEASE_URL,
    ];
    let result = run_bounded(Path::new("curl"), &args, &options).await;
    if let Some(error) = result.error {
        return Err(error);
    }
    if result.timed_out {
        return Err("curl timed out".to_owned());
    }
    if result.exit_code != Some(0) {
        let output = result.output.trim();
        return Err(format!("curl failed ({}): {output}", result.exit_code.unwrap_or(-1)));
    }
    version_from_redirect(&result.output).ok_or_else(|| format!("unexpected answer: {}", result.output.trim()))
}

/// Looks the latest release up and keeps it in `path` unless the kept answer is younger than [`CHECK_MAX_AGE`] at
/// `now_ms`: the new answer, `Ok(None)` when none was due, or why the lookup failed (the kept answer stays).
pub async fn refresh_if_due(path: &Path, now_ms: i64) -> Result<Option<UpdateCheck>, String> {
    if let Some(kept) = read_update_check(path).await
        && let Some(at) = date::parse(&kept.checked_at)
        && (0..CHECK_MAX_AGE.as_millis() as i64).contains(&now_ms.saturating_sub(at))
    {
        return Ok(None);
    }
    let latest = fetch_latest().await?;
    let check = UpdateCheck {
        latest,
        checked_at: date::to_iso_string(now_ms).unwrap_or_default(),
    };
    write_update_check(path, &check)
        .await
        .map_err(|error| error.to_string())?;
    Ok(Some(check))
}

/// What `deckhand update` reads to tell how the executable at `exe` was installed (`dev`: a debug build or the dev
/// channel): its realpath, cargo's record of its `deckhand` install and the installer's receipt, each only when it
/// names this executable.
pub fn install_facts(exe: &Path, dev: bool) -> InstallFacts {
    let canonical = std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
    let same = |candidate: &Path| std::fs::canonicalize(candidate).is_ok_and(|found| found == canonical);
    let cargo_home = std::env::var_os("CARGO_HOME")
        .filter(|home| !home.is_empty())
        .map_or_else(|| home_dir().join(".cargo"), PathBuf::from);
    let cargo_bin = cargo_home.join("bin");
    let cargo = std::fs::read_to_string(cargo_home.join(".crates2.json"))
        .ok()
        .and_then(|text| cargo_source(&text))
        .filter(|_| same(&cargo_bin.join("deckhand")))
        .map(|source| (source, cargo_bin));
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|home| !home.is_empty())
        .map_or_else(|| home_dir().join(".config"), PathBuf::from);
    let installer_dir = std::fs::read_to_string(config_home.join("deckhand").join("deckhand-receipt.json"))
        .ok()
        .and_then(|text| receipt_binaries(&text).into_iter().find(|binary| same(binary)))
        .and_then(|binary| binary.parent().map(Path::to_path_buf));
    InstallFacts {
        canonical,
        dev,
        cargo,
        installer_dir,
    }
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

    fn at(iso: &str) -> i64 {
        date::parse(iso).unwrap_or_else(|| panic!("{iso}"))
    }

    #[test]
    fn a_kept_answer_younger_than_a_day_is_not_looked_up_again() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = dir.path().join("update-check");
        let kept = UpdateCheck {
            latest: "3.4.0".to_owned(),
            checked_at: "2026-11-15T10:00:00.000Z".to_owned(),
        };
        run(async {
            assert_eq!(read_update_check(&path).await, None);
            write_update_check(&path, &kept)
                .await
                .unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(read_update_check(&path).await.as_ref(), Some(&kept));
            // Within the day: nothing is looked up (no lookup could succeed here without the override).
            assert_eq!(refresh_if_due(&path, at("2026-11-16T09:59:00.000Z")).await, Ok(None));
        });
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(
            text,
            "{\"latest\":\"3.4.0\",\"checkedAt\":\"2026-11-15T10:00:00.000Z\"}\n"
        );
    }

    #[test]
    fn an_unreadable_file_is_no_answer() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = dir.path().join("update-check");
        std::fs::write(&path, "not json").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(run(read_update_check(&path)), None);
    }

    #[test]
    fn install_facts_name_only_installs_of_this_executable() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let exe = dir.path().join("deckhand");
        std::fs::write(&exe, "").unwrap_or_else(|error| panic!("{error}"));
        // No cargo or installer record for it here (HOME and CARGO_HOME are the test runner's, so only the realpath
        // and the dev flag are certain).
        let facts = install_facts(&exe, true);
        assert_eq!(facts.canonical, std::fs::canonicalize(&exe).unwrap_or_default());
        assert!(facts.dev);
        assert_eq!(facts.installer_dir, None);
    }
}
