//! The isolated dev launcher, `scripts/deckhand-dev.sh`.
//!
//! The launcher's `isolatedHome` / `createSandbox` exports are its `isolated_home` / `create_sandbox` shell
//! functions, reached by sourcing the script with `DECKHAND_DEV_LIB=1`. Every run gets a temp `HOME` (the TS test
//! used the real one, which the port's isolation rules forbid), so "production" is a throwaway `~/.deckhand`.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::symlink;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use deckhand_e2e::harness::{bin, launcher};

const HOOK_KEYS: [&str; 5] = [
    "DECKHAND_HOME",
    "DECKHAND_CHANNEL",
    "DECKHAND_SESSION_ID",
    "DECKHAND_LAUNCH_ID",
    "DECKHAND_HOOK_TOKEN",
];

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A command with a private HOME and no inherited Deckhand state or Git config.
fn hermetic(mut command: Command, home: &Path) -> Command {
    command
        .env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("DECKHAND_DEV_HOME")
        .env_remove("DECKHAND_TEST_BIN");
    for key in HOOK_KEYS {
        command.env_remove(key);
    }
    command
}

/// Calls one of the launcher's shell functions (`isolated_home`, `create_sandbox`) with a single argument.
fn call(function: &str, argument: &Path, home: &Path, env: &[(&str, &str)]) -> Output {
    let mut command = hermetic(Command::new("sh"), home);
    command
        .arg("-c")
        .arg(format!(". \"$0\"; {function} \"$1\""))
        .arg(launcher())
        .arg(argument)
        .env("DECKHAND_DEV_LIB", "1");
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().expect("run sh")
}

fn launcher_run(args: &[&str], cwd: &Path, home: &Path, env: &[(&str, &Path)]) -> Output {
    let mut command = hermetic(Command::new(launcher()), home);
    command
        .args(args)
        .current_dir(cwd)
        .env("DECKHAND_DEV", "0")
        .env("DECKHAND_AGENT_LATEST", "{}");
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().expect("run launcher")
}

fn wait_for<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        if let Some(value) = probe() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A daemon the test started; terminated when dropped.
struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(i32::try_from(self.0.id()).unwrap_or(0)),
                nix::sys::signal::Signal::SIGTERM,
            );
            let deadline = Instant::now() + Duration::from_secs(5);
            while matches!(self.0.try_wait(), Ok(None)) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(25));
            }
            if matches!(self.0.try_wait(), Ok(None)) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }
}

/// A `ping` over the daemon's Unix socket (1.5 s ceiling), or None when nothing answers.
fn ping(home: &Path) -> Option<serde_json::Value> {
    let mut stream = UnixStream::connect(home.join("daemon.sock")).ok()?;
    stream.set_read_timeout(Some(Duration::from_millis(1500))).ok()?;
    stream
        .write_all(b"{\"type\":\"ping\",\"requestId\":\"launcher-test\"}\n")
        .ok()?;
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        if value["type"] == "response" && value["requestId"] == "launcher-test" {
            return Some(value["data"].clone());
        }
    }
}

#[test]
fn dev_isolation_rejects_production_paths_and_symlink_aliases() {
    let temp = tempfile::tempdir().expect("temp dir");
    let home = temp.path().join("home");
    fs::create_dir(&home).expect("home");
    let production = home.join(".deckhand");
    let rejected = |candidate: &Path, env: &[(&str, &str)]| {
        let output = call("isolated_home", candidate, &home, env);
        assert!(!output.status.success(), "{} should be refused", candidate.display());
        assert!(
            text(&output.stderr).contains("separate"),
            "stderr: {}",
            text(&output.stderr)
        );
        assert_eq!(
            text(&output.stderr).trim_end(),
            "The dev state directory must be separate from ~/.deckhand and DECKHAND_HOME (including symlinks)"
        );
    };
    rejected(&production, &[]);
    rejected(&production.join("dev"), &[]);
    // The alias points at a production directory that does not exist yet: dangling links still resolve.
    symlink(&production, temp.path().join("alias")).expect("symlink");
    rejected(&temp.path().join("alias"), &[]);
    let canonical_temp = fs::canonicalize(temp.path()).expect("canonical temp");
    let preview = call("isolated_home", &temp.path().join("preview"), &home, &[]);
    assert!(preview.status.success(), "stderr: {}", text(&preview.stderr));
    assert_eq!(text(&preview.stdout), canonical_temp.join("preview").to_string_lossy());
    let custom = temp.path().join("custom");
    rejected(
        &canonical_temp.join("custom"),
        &[("DECKHAND_HOME", &custom.to_string_lossy())],
    );
    // Inside an isolated dev session DECKHAND_HOME is the dev home itself, so it no longer counts as production.
    let allowed = call(
        "isolated_home",
        &canonical_temp.join("custom"),
        &home,
        &[
            ("DECKHAND_HOME", &custom.to_string_lossy()),
            ("DECKHAND_CHANNEL", "dev"),
        ],
    );
    assert!(allowed.status.success(), "stderr: {}", text(&allowed.stderr));
}

#[test]
fn dev_sandbox_does_not_overwrite_existing_work_and_is_built_aside() {
    let temp = tempfile::tempdir().expect("temp dir");
    let home = temp.path().join("home");
    fs::create_dir(&home).expect("home");
    let state = temp.path().join("state");
    fs::create_dir(&state).expect("state");
    let created = call("create_sandbox", &state, &home, &[]);
    assert!(created.status.success(), "stderr: {}", text(&created.stderr));
    let cwd = PathBuf::from(text(&created.stdout));
    assert_eq!(cwd, state.join("sandbox"));
    for file in [
        "package.json",
        "app.js",
        "app.test.js",
        "deckhand.json",
        "README.md",
        ".git",
    ] {
        assert!(cwd.join(file).exists(), "{file} missing");
    }
    assert_eq!(
        fs::read_to_string(cwd.join("deckhand.json")).expect("deckhand.json"),
        "{\n  \"defaultWorkspace\": \"new\",\n  \"devCommand\": \"npm run dev\",\n  \"actions\": {\n    \"test\": \"npm test\"\n  }\n}\n"
    );
    fs::write(cwd.join("README.md"), "keep my work").expect("write");
    let again = call("create_sandbox", &state, &home, &[]);
    assert!(again.status.success(), "stderr: {}", text(&again.stderr));
    assert_eq!(
        fs::read_to_string(cwd.join("README.md")).expect("README"),
        "keep my work"
    );
    let leftovers: Vec<_> = fs::read_dir(&state)
        .expect("read state")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("sandbox.tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    // A directory that is not a repository is never overwritten.
    let other = temp.path().join("other");
    fs::create_dir_all(other.join("sandbox")).expect("other sandbox");
    let refused = call("create_sandbox", &other, &home, &[]);
    assert!(!refused.status.success());
    assert_eq!(
        text(&refused.stderr).trim_end(),
        format!(
            "Refusing to overwrite existing sandbox directory: {}",
            other.join("sandbox").display()
        )
    );
}

#[test]
fn dev_status_stop_control_only_the_isolated_dev_daemon() {
    // HOME is a temp dir, so "production" (~/.deckhand) is a throwaway stable daemon, never the user's.
    let fake_home = tempfile::Builder::new().prefix("dh-home-").tempdir().expect("temp dir");
    let fake = fake_home.path();
    let dev_home = fake.join("dev");
    let stable_home = fake.join(".deckhand");
    let bin = bin();
    let start = |extra: &[(&str, &Path)]| -> Daemon {
        let mut command = hermetic(Command::new(&bin), fake);
        command
            .arg("--daemon")
            .env("DECKHAND_DEV", "0")
            .env("DECKHAND_AGENT_LATEST", "{}")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (key, value) in extra {
            command.env(key, value);
        }
        Daemon(command.spawn().expect("spawn daemon"))
    };
    // The stable daemon lives in the fake HOME; debug builds refuse any `~/.deckhand` unless told otherwise.
    let mut stable = start(&[("DECKHAND_ALLOW_PROD_HOME", Path::new("1"))]);
    let mut dev = start(&[("DECKHAND_HOME", &dev_home), ("DECKHAND_CHANNEL", Path::new("dev"))]);
    let env: Vec<(&str, &Path)> = vec![("DECKHAND_DEV_HOME", &dev_home), ("DECKHAND_TEST_BIN", &bin)];
    wait_for("the dev daemon to report running", || {
        let output = launcher_run(&["status"], fake, fake, &env);
        text(&output.stdout)
            .starts_with("Isolated dev daemon running")
            .then_some(())
    });
    wait_for("the stable daemon to answer", || {
        ping(&stable_home)
            .filter(|data| data["channel"] == "stable")
            .map(|_| ())
    });
    let source = launcher_run(&["--source", "hooks", "codex"], fake, fake, &env);
    assert!(source.status.success(), "stderr: {}", text(&source.stderr));
    let stdout = text(&source.stdout);
    assert!(stdout.contains("SessionStart"), "{stdout}");
    assert!(!stdout.contains("StopFailure"), "{stdout}");
    let production_env: Vec<(&str, &Path)> = vec![("DECKHAND_DEV_HOME", &stable_home), ("DECKHAND_TEST_BIN", &bin)];
    let refused = launcher_run(&["stop"], fake, fake, &production_env);
    assert!(!refused.status.success());
    assert!(text(&refused.stderr).contains("separate"), "{}", text(&refused.stderr));
    let stopped = launcher_run(&["stop"], fake, fake, &env);
    assert!(stopped.status.success(), "stderr: {}", text(&stopped.stderr));
    wait_for("the dev daemon to exit", || dev.0.try_wait().ok().flatten());
    let after = launcher_run(&["status"], fake, fake, &env);
    assert!(
        text(&after.stdout).contains("No isolated dev daemon"),
        "{}",
        text(&after.stdout)
    );
    let data = ping(&stable_home).expect("stable daemon still answers");
    assert_eq!(data["channel"], "stable");
    assert!(
        matches!(stable.0.try_wait(), Ok(None)),
        "stable daemon must still be running"
    );
}

#[test]
fn launcher_refuses_global_setup_and_missing_binaries_and_prints_help() {
    let temp = tempfile::tempdir().expect("temp dir");
    let home = temp.path().join("home");
    fs::create_dir(&home).expect("home");
    let dev_home = temp.path().join("dev");
    let missing = temp.path().join("missing-deckhand");
    let env: Vec<(&str, &Path)> = vec![("DECKHAND_DEV_HOME", &dev_home), ("DECKHAND_TEST_BIN", &missing)];
    let setup = launcher_run(&["setup"], temp.path(), &home, &env);
    assert!(!setup.status.success());
    assert_eq!(
        text(&setup.stderr).trim_end(),
        "The isolated launcher does not install global agent tools. Use setup --check for read-only inspection."
    );
    let absent = launcher_run(&["doctor", "--check"], temp.path(), &home, &env);
    assert!(!absent.status.success());
    assert_eq!(
        text(&absent.stderr).trim_end(),
        format!("DECKHAND_TEST_BIN not found: {}", missing.display())
    );
    assert!(
        dev_home.is_dir(),
        "the state directory is created before the binary is looked up"
    );
    let help = launcher_run(&["--help"], temp.path(), &home, &[]);
    assert!(help.status.success());
    assert!(text(&help.stdout).starts_with("Usage: scripts/deckhand-dev.sh"));
}

/// Asserts a `create_sandbox` call failed outright: non-zero status, nothing on stdout, no temporary left behind.
fn assert_sandbox_failed(output: &Output, state: &Path) {
    assert!(
        !output.status.success(),
        "create_sandbox must fail; stdout: {:?} stderr: {}",
        text(&output.stdout),
        text(&output.stderr)
    );
    assert_eq!(text(&output.stdout), "", "a failed sandbox prints no workspace");
    assert!(
        !text(&output.stderr).trim().is_empty(),
        "the failure is reported on stderr"
    );
    let leftovers: Vec<_> = fs::read_dir(state)
        .expect("read state")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("sandbox.tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

// Regression: a failed sandbox build is an error (the TS createSandbox rethrows), never an empty success.
#[test]
fn dev_sandbox_build_failures_are_reported_not_swallowed() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().expect("temp dir");
    let home = temp.path().join("home");
    fs::create_dir(&home).expect("home");

    // A dangling `sandbox` symlink: the TS rename onto it fails (ENOTDIR) and createSandbox rethrows.
    let dangling = temp.path().join("dangling");
    fs::create_dir(&dangling).expect("dangling state");
    symlink(temp.path().join("nowhere"), dangling.join("sandbox")).expect("symlink");
    let output = call("create_sandbox", &dangling, &home, &[]);
    assert_sandbox_failed(&output, &dangling);
    assert!(
        text(&output.stderr).starts_with("ENOTDIR: not a directory, rename "),
        "stderr: {}",
        text(&output.stderr)
    );
    assert!(
        fs::symlink_metadata(dangling.join("sandbox"))
            .expect("link kept")
            .file_type()
            .is_symlink(),
        "the existing link is left alone"
    );

    // Git failing inside the sandbox (an unparsable global config) fails the whole build.
    let broken_git = temp.path().join("broken-git");
    fs::create_dir(&broken_git).expect("broken-git state");
    let bad_config = temp.path().join("bad.gitconfig");
    fs::write(&bad_config, "[bad\n").expect("bad config");
    let output = call(
        "create_sandbox",
        &broken_git,
        &home,
        &[("GIT_CONFIG_GLOBAL", &bad_config.to_string_lossy())],
    );
    assert_sandbox_failed(&output, &broken_git);
    assert!(
        text(&output.stderr).starts_with("Command failed: git -C "),
        "stderr: {}",
        text(&output.stderr)
    );
    assert!(!broken_git.join("sandbox").exists());

    // A state directory the sandbox cannot be created in (skipped as root, which ignores modes).
    let read_only = temp.path().join("read-only");
    fs::create_dir(&read_only).expect("read-only state");
    fs::set_permissions(&read_only, fs::Permissions::from_mode(0o555)).expect("chmod");
    let writable = fs::write(read_only.join("probe"), "").is_ok();
    let output = (!writable).then(|| call("create_sandbox", &read_only, &home, &[]));
    fs::set_permissions(&read_only, fs::Permissions::from_mode(0o755)).expect("chmod back");
    if let Some(output) = output {
        assert_sandbox_failed(&output, &read_only);
    }
}

// Regression: main stops (exit 1) instead of running the CLI in the caller's directory.
#[test]
fn launcher_exits_without_running_the_cli_when_the_sandbox_cannot_be_built() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().expect("temp dir");
    let home = temp.path().join("home");
    fs::create_dir(&home).expect("home");
    let work = temp.path().join("work");
    fs::create_dir(&work).expect("work");
    let dev_home = temp.path().join("dev");
    fs::create_dir(&dev_home).expect("dev home");
    symlink(temp.path().join("nowhere"), dev_home.join("sandbox")).expect("symlink");
    let marker = temp.path().join("cli-ran");
    let fake = temp.path().join("fake-deckhand");
    fs::write(&fake, format!("#!/bin/sh\npwd > '{}'\n", marker.display())).expect("fake cli");
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).expect("chmod fake");
    let env: Vec<(&str, &Path)> = vec![("DECKHAND_DEV_HOME", &dev_home), ("DECKHAND_TEST_BIN", &fake)];
    let output = launcher_run(&["--sandbox", "status"], &work, &home, &env);
    assert!(!output.status.success(), "stderr: {}", text(&output.stderr));
    assert!(!marker.exists(), "the CLI must not run in the caller's directory");
    let stderr = text(&output.stderr);
    assert!(!stderr.contains("Workspace:"), "stderr: {stderr}");
    assert!(stderr.contains("ENOTDIR"), "stderr: {stderr}");
}
