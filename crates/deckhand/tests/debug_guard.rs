//! Port of the debug-build home guard's interaction with tests/devLauncher.test.ts (which runs a "production" daemon
//! under a throwaway HOME).

use std::process::Command;

// Regression: with HOME pointing at a temp dir and no DECKHAND_HOME, the state directory is <temp>/.deckhand, a
// throwaway; the debug guard protects only the account's real ~/.deckhand, so the command runs instead of refusing.
#[test]
fn debug_guard_allows_a_throwaway_home_dot_deckhand() {
    let home = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_deckhand"))
        .arg("status")
        .env("HOME", home.path())
        .env("DECKHAND_CHANNEL", "dev")
        .env_remove("DECKHAND_HOME")
        .env_remove("DECKHAND_ALLOW_PROD_HOME")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "stderr: {stderr}");
    assert!(
        !stderr.contains("Refusing to use the production Deckhand home"),
        "{stderr}"
    );
    assert!(stdout.starts_with("No isolated dev daemon running ("), "{stdout}");
    assert!(stdout.contains(".deckhand"), "{stdout}");
}

// The account's own ~/.deckhand stays refused even when HOME points elsewhere.
#[test]
fn debug_guard_still_refuses_the_account_home() {
    let home = tempfile::tempdir().unwrap();
    let account = nix_home();
    let output = Command::new(env!("CARGO_BIN_EXE_deckhand"))
        .arg("status")
        .env("HOME", home.path())
        .env("DECKHAND_CHANNEL", "dev")
        .env("DECKHAND_HOME", account.join(".deckhand"))
        .env_remove("DECKHAND_ALLOW_PROD_HOME")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains("Refusing to use the production Deckhand home"),
        "{stderr}"
    );
}

fn nix_home() -> std::path::PathBuf {
    nix::unistd::User::from_uid(nix::unistd::getuid()).unwrap().unwrap().dir
}
