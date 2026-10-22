//! `scripts/release-restart-note.sh`: the "Daemon restart" line the release workflow appends to a release's notes.
//!
//! Updating needs a daemon restart exactly when PROTOCOL_VERSION changed since the previous release (a client refuses
//! a daemon of another protocol); a first native release, or one after only npm releases, points at the npm guide.

use std::fs;
use std::path::Path;
use std::process::Command;

use deckhand_e2e::harness::{git, repo, workspace_root};

const REQUIRED_49_50: &str = "**Daemon restart:** required — protocol 49 → 50. Run `deckhand restart` after updating \
                              (running sessions are interrupted; resume them with s).";
const NOT_REQUIRED: &str = "**Daemon restart:** not required — the running daemon stays compatible; `deckhand \
                            restart` when convenient picks up daemon fixes.";
const FROM_NPM: &str =
    "**Daemon restart:** required when upgrading from the npm version — see docs/upgrading-from-npm.md.";

/// Runs the script in `repo` with `args`; returns (exit code, stdout, stderr).
fn note(repo: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let output = Command::new(workspace_root().join("scripts/release-restart-note.sh"))
        .args(args)
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("run release-restart-note.sh");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).trim_end().to_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn ok(repo: &Path, args: &[&str]) -> String {
    let (code, stdout, stderr) = note(repo, args);
    assert_eq!(code, Some(0), "{args:?}: {stderr}");
    stdout
}

/// Commits `PROTOCOL_VERSION = protocol` (or, `None`, a tree without crates/protocol, as the npm releases had) and
/// tags it with each of `tags`.
fn release(repo: &Path, protocol: Option<u32>, tags: &[&str]) {
    let file = repo.join("crates/protocol/src/lib.rs");
    match protocol {
        Some(version) => {
            fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
            fs::write(
                &file,
                format!("/// The wire version.\npub const PROTOCOL_VERSION: u32 = {version};\n"),
            )
            .expect("write lib.rs");
        }
        None => {
            let _ = fs::remove_dir_all(repo.join("crates"));
            fs::write(repo.join("types.ts"), "export const PROTOCOL_VERSION = 45;\n").expect("write types.ts");
        }
    }
    fs::write(repo.join("release.txt"), tags.join(" ")).expect("write release.txt");
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-m", &tags.join(" ")]);
    for tag in tags {
        git(repo, &["tag", tag]);
    }
}

#[test]
fn the_restart_line_follows_the_protocol_version_between_releases() {
    let dir = repo();
    let repo = dir.path();
    release(repo, None, &["v2.0.0"]);
    release(repo, Some(49), &["v3.0.0"]);
    release(repo, Some(49), &["v3.0.1"]);
    release(repo, Some(50), &["v3.1.0"]);
    release(repo, Some(51), &["v3.1.1-rc.1", "v3.1.1"]);

    // The previous release is found among the v* tags, by version.
    assert_eq!(
        ok(repo, &["v3.0.0"]),
        FROM_NPM,
        "the first native release follows only npm ones"
    );
    assert_eq!(ok(repo, &["v3.0.1"]), NOT_REQUIRED);
    assert_eq!(ok(repo, &["v3.1.0"]), REQUIRED_49_50);
    // A prerelease is never the previous release.
    assert_eq!(ok(repo, &["v3.1.1"]), REQUIRED_49_50.replace("49 → 50", "50 → 51"));
    // Explicit tags; an empty previous tag means there is none.
    assert_eq!(ok(repo, &["v3.0.0", "v3.1.0"]), REQUIRED_49_50);
    assert_eq!(ok(repo, &["", "v3.0.1"]), FROM_NPM);
    // A release without crates/protocol is an error, as is an unknown tag.
    let (code, _, stderr) = note(repo, &["v2.0.0"]);
    assert_eq!(code, Some(1));
    assert!(stderr.contains("no PROTOCOL_VERSION"), "{stderr}");
    let (code, _, stderr) = note(repo, &["v9.9.9"]);
    assert_eq!(code, Some(1));
    assert!(stderr.contains("no tag v9.9.9"), "{stderr}");
}

#[test]
fn a_first_release_points_at_the_npm_upgrade_guide() {
    let dir = repo();
    let repo = dir.path();
    release(repo, Some(49), &["v3.0.0"]);
    assert_eq!(ok(repo, &["v3.0.0"]), FROM_NPM);
}

#[test]
fn the_script_reads_the_protocol_version_as_this_repository_declares_it() {
    // The same commit twice: the real crates/protocol/src/lib.rs must parse (or the script fails).
    assert_eq!(ok(&workspace_root(), &["HEAD", "HEAD"]), NOT_REQUIRED);
}
