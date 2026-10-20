//! Port of tests/helpers.ts: temp homes, `repo()`, the CLI under test, fake agents, dev-daemon control.
//!
//! Everything is synchronous (threads, no async runtime): a black-box test mostly waits on child processes and sockets.

use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// The workspace root (the checkout this crate lives in).
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The deckhand executable under test: `DECKHAND_TEST_BIN` (absolute, or relative to the workspace root), else
/// `target/debug/deckhand`. Panics if it does not exist.
pub fn bin() -> PathBuf {
    let root = workspace_root();
    let bin = match std::env::var_os("DECKHAND_TEST_BIN") {
        Some(value) if !value.is_empty() => root.join(value),
        _ => root.join("target/debug/deckhand"),
    };
    let bin = lexical(&bin);
    assert!(
        bin.exists(),
        "deckhand binary under test not found (cargo build, or set DECKHAND_TEST_BIN): {}",
        bin.display()
    );
    bin
}

/// `path.resolve`: collapses `.` and `..` without touching the filesystem.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The launcher the dev-daemon tests drive (`scripts/deckhand-dev.sh`).
pub fn launcher() -> PathBuf {
    lexical(&workspace_root().join("scripts/deckhand-dev.sh"))
}

/// Refuses a state directory that is, contains or sits inside the user's real Deckhand/agent state, or lies outside the
/// temp directory (the same check as scripts/verify-unit isolation).
pub fn assert_isolated(path: &Path) {
    let real_home = nix::unistd::User::from_uid(nix::unistd::getuid())
        .ok()
        .flatten()
        .map(|user| user.dir);
    let resolve = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| lexical(p));
    let candidate = resolve(path);
    if let Some(home) = real_home {
        let home = resolve(&home);
        for name in [".deckhand", ".deckhand-dev", ".codex", ".claude"] {
            let protected = home.join(name);
            assert!(
                !(candidate.starts_with(&protected) || protected.starts_with(&candidate)),
                "refusing to run against real state: {} vs {}",
                candidate.display(),
                protected.display()
            );
        }
    }
    let temp = resolve(&std::env::temp_dir());
    assert!(
        candidate.starts_with(&temp),
        "test state must live under the temp dir {}: {}",
        temp.display(),
        candidate.display()
    );
}

/// Polls `read` every 40ms until `matches`; panics with the last value on timeout (default 8s in TS).
pub fn wait_for<T: std::fmt::Debug>(mut read: impl FnMut() -> T, matches: impl Fn(&T) -> bool, timeout: Duration) -> T {
    let deadline = Instant::now() + timeout;
    loop {
        let value = read();
        if matches(&value) {
            return value;
        }
        if Instant::now() >= deadline {
            panic!("Condition timed out: {value:?}");
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

/// `wait_for` with helpers.ts' default 8s timeout.
pub fn wait_for_default<T: std::fmt::Debug>(read: impl FnMut() -> T, matches: impl Fn(&T) -> bool) -> T {
    wait_for(read, matches, Duration::from_millis(8000))
}

pub fn is_alive(pid: i32) -> bool {
    pid > 0 && nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
}

fn signal_group(pid: i32, signal: nix::sys::signal::Signal) {
    let _ = nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pid), signal);
}

const GIT_ENV: [(&str, &str); 2] = [("GIT_CONFIG_GLOBAL", "/dev/null"), ("GIT_CONFIG_NOSYSTEM", "1")];

/// Runs git in `cwd` ignoring the user's global/system config; returns trimmed stdout.
pub fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .envs(GIT_ENV)
        .output()
        .expect("spawn git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A fixture repository exactly like helpers.ts `repo()`: one commit on `main`, no signing, no hooks.
pub fn repo() -> TempDir {
    let directory = tempfile::Builder::new()
        .prefix("deckhand-test-")
        .tempdir()
        .expect("temp repo");
    let path = directory.path();
    git(path, &["init", "-b", "main"]);
    git(path, &["config", "user.name", "Deckhand Test"]);
    git(path, &["config", "user.email", "deckhand@example.invalid"]);
    git(path, &["config", "commit.gpgsign", "false"]);
    let hooks = path.join(".git").join("no-hooks");
    git(path, &["config", "core.hooksPath", &hooks.to_string_lossy()]);
    fs::write(path.join("file.txt"), "first\n").expect("write file.txt");
    git(path, &["add", "."]);
    git(path, &["commit", "-m", "initial"]);
    directory
}

/// Waits up to `timeout` for the child; `None` if it is still running.
pub fn wait_exit(child: &mut Child, timeout: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// helpers.ts `stop`: SIGTERM and wait for exit.
pub fn stop(child: &mut Child) {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }
    // Only the process itself: a daemon's sessions must get to see it shut down on its own terms.
    let pid = nix::unistd::Pid::from_raw(child.id() as i32);
    let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM);
    if wait_exit(child, Duration::from_secs(10)).is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Which fake agent variant `install_agent` writes.
#[derive(Clone, Copy, Default)]
pub struct AgentOptions {
    /// helpers.ts `withoutHooks`: never reports SessionStart.
    pub without_hooks: bool,
    /// The agent's update command logs the call, prints an npm error and exits 1 (the TS test's Pi).
    pub update_fails: bool,
}

/// Installs the fake agent (`fake_agent`: pass `env!("CARGO_BIN_EXE_fake_agent")`) as `dir/<provider>`. It is a copy, so
/// its "self rewrite" (a `<name>.version` file next to it) never touches the shared binary; options become sidecar files.
pub fn install_agent(dir: &Path, provider: &str, fake_agent: &Path, options: AgentOptions) -> PathBuf {
    fs::create_dir_all(dir).expect("agent bin dir");
    let target = dir.join(provider);
    fs::copy(fake_agent, &target).expect("copy fake agent");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).expect("chmod fake agent");
    if options.without_hooks {
        fs::write(sidecar(&target, "nohooks"), "").expect("sidecar");
    }
    if options.update_fails {
        fs::write(sidecar(&target, "update-fails"), "").expect("sidecar");
    }
    target
}

/// `<agent>.<suffix>`: where the fake agent keeps its version and flags.
pub fn sidecar(agent: &Path, suffix: &str) -> PathBuf {
    let mut name = agent.as_os_str().to_owned();
    name.push(".");
    name.push(suffix);
    PathBuf::from(name)
}

/// Prefix of every sandbox directory (short on purpose, see [`Sandbox::new`]).
pub const SANDBOX_PREFIX: &str = "dh-";

/// `sun_path` holds 104 bytes on macOS (108 on Linux) including the trailing NUL.
const MAX_SOCKET_PATH: usize = 103;

/// The longest socket path a sandbox hands out is its dev daemon's, `<root>/dev-home/daemon.sock`; the daemon may
/// resolve symlinks (`/var` -> `/private/var`), so the canonical root counts too. Panics with the cure if one is too long.
fn assert_socket_paths_fit(label: &str, root: &Path) {
    let canonical = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    for base in [root, canonical.as_path()] {
        for socket in ["home/daemon.sock", "dev-home/daemon.sock"] {
            let path = base.join(socket);
            let length = path.as_os_str().len();
            assert!(
                length <= MAX_SOCKET_PATH,
                "sandbox {label:?}: socket path {} is {length} bytes, over the {MAX_SOCKET_PATH}-byte unix socket limit; \
                 set TMPDIR to a shorter directory (e.g. TMPDIR=/tmp)",
                path.display()
            );
        }
    }
}

/// An isolated test world under `$TMPDIR`: its own `DECKHAND_HOME`, `HOME` and XDG dirs. Dropping it stops the daemon of
/// `DECKHAND_HOME` and of the dev home, kills the process groups of every child spawned through it, and removes the lot.
pub struct Sandbox {
    root: TempDir,
    children: Mutex<Vec<i32>>,
}

impl Sandbox {
    /// `label` names the suite in panic messages only: the directory gets a short fixed prefix (`dh-`) so that the
    /// daemon's socket paths stay under the macOS `sun_path` limit whatever `$TMPDIR` is.
    pub fn new(label: &str) -> Sandbox {
        let root = tempfile::Builder::new()
            .prefix(SANDBOX_PREFIX)
            .tempdir()
            .expect("sandbox temp dir");
        assert_isolated(root.path());
        assert_socket_paths_fit(label, root.path());
        for dir in [
            "home",
            "user",
            "dev-home",
            "xdg/config",
            "xdg/data",
            "xdg/cache",
            "xdg/state",
        ] {
            fs::create_dir_all(root.path().join(dir)).expect("sandbox dir");
        }
        Sandbox {
            root,
            children: Mutex::new(Vec::new()),
        }
    }

    pub fn root(&self) -> &Path {
        self.root.path()
    }

    /// `DECKHAND_HOME`.
    pub fn home(&self) -> PathBuf {
        self.root().join("home")
    }

    /// `HOME`, kept apart from the state directories.
    pub fn user_home(&self) -> PathBuf {
        self.root().join("user")
    }

    /// `DECKHAND_DEV_HOME` for the isolated dev launcher.
    pub fn dev_home(&self) -> PathBuf {
        self.root().join("dev-home")
    }

    pub fn socket(&self) -> PathBuf {
        self.home().join("daemon.sock")
    }

    /// A command with the sandbox environment: private HOME/XDG/DECKHAND_HOME, hermetic Git, no inherited Deckhand
    /// variables, `DECKHAND_DEV=0`, `DECKHAND_AGENT_LATEST={}` (never npm) and `TEST_CLI` = the binary under test.
    pub fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(program);
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("DECKHAND_") {
                command.env_remove(key);
            }
        }
        let bin = bin();
        command
            .env("HOME", self.user_home())
            .env("DECKHAND_HOME", self.home())
            .env("XDG_CONFIG_HOME", self.root().join("xdg/config"))
            .env("XDG_DATA_HOME", self.root().join("xdg/data"))
            .env("XDG_CACHE_HOME", self.root().join("xdg/cache"))
            .env("XDG_STATE_HOME", self.root().join("xdg/state"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("DECKHAND_DEV", "0")
            .env("DECKHAND_AGENT_LATEST", "{}")
            .env("DECKHAND_TEST_BIN", &bin)
            .env("TEST_CLI", &bin)
            .env("SHELL", "/bin/sh");
        command
    }

    /// helpers.ts `spawnCli`: the binary under test with the sandbox environment (adjust the returned command's env first
    /// via [`Sandbox::spawn`]).
    pub fn cli(&self, args: &[&str]) -> Command {
        let mut command = self.command(bin());
        command.args(args);
        command
    }

    /// Spawns `command` in its own process group (pgid == pid), tracked so drop can kill it.
    pub fn spawn(&self, mut command: Command) -> Child {
        command.process_group(0);
        let child = command.spawn().expect("spawn child");
        self.children
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(child.id() as i32);
        child
    }

    /// helpers.ts `spawnCli`.
    pub fn spawn_cli(&self, args: &[&str]) -> Child {
        self.spawn(self.cli(args))
    }

    /// helpers.ts `execCli`: runs to completion (stdin closed) and returns the output; kills it after `timeout`.
    pub fn exec_cli(&self, args: &[&str], timeout: Duration) -> Output {
        self.exec(self.cli(args), timeout)
    }

    /// Runs `command` with piped output and no stdin; panics if it outlives `timeout`.
    pub fn exec(&self, mut command: Command, timeout: Duration) -> Output {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = self.spawn(command);
        let drain = |stream: Option<Box<dyn Read + Send>>| {
            std::thread::spawn(move || {
                let mut buffer = Vec::new();
                if let Some(mut stream) = stream {
                    let _ = stream.read_to_end(&mut buffer);
                }
                buffer
            })
        };
        let out = drain(child.stdout.take().map(|s| Box::new(s) as _));
        let err = drain(child.stderr.take().map(|s| Box::new(s) as _));
        let status = match wait_exit(&mut child, timeout) {
            Some(status) => status,
            None => {
                signal_group(child.id() as i32, nix::sys::signal::Signal::SIGKILL);
                let _ = child.wait();
                panic!("command timed out after {timeout:?}");
            }
        };
        Output {
            status,
            stdout: out.join().unwrap_or_default(),
            stderr: err.join().unwrap_or_default(),
        }
    }

    /// Runs the isolated dev launcher (`scripts/deckhand-dev.sh`) with the dev environment (see [`Sandbox::dev_env`]).
    pub fn exec_launcher(&self, args: &[&str], timeout: Duration) -> Output {
        let mut command = self.command(launcher());
        command.args(args);
        self.dev_env(&mut command);
        self.exec(command, timeout)
    }

    /// The environment the launcher (and a PTY UI running it) needs: `DECKHAND_DEV_HOME` beside, never inside, the
    /// production home, and no stable `DECKHAND_HOME`.
    pub fn dev_env(&self, command: &mut Command) {
        command
            .env("DECKHAND_DEV_HOME", self.dev_home())
            .env_remove("DECKHAND_HOME")
            .env_remove("DECKHAND_CHANNEL");
    }

    /// helpers.ts `stopDevDaemon`: stops the isolated dev daemon of the dev home and waits for its process to exit.
    pub fn stop_dev_daemon(&self) {
        let pid = read_pid(&self.dev_home().join("daemon.pid"));
        if launcher().exists() && self.dev_home().exists() {
            let mut command = self.command(launcher());
            command.arg("stop");
            self.dev_env(&mut command);
            command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            if let Ok(mut child) = command.spawn()
                && wait_exit(&mut child, Duration::from_secs(10)).is_none()
            {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        terminate_pid(pid);
    }
}

fn read_pid(file: &Path) -> Option<i32> {
    fs::read_to_string(file)
        .ok()
        .and_then(|text| text.trim().parse::<i32>().ok())
        .filter(|pid| *pid > 0)
}

/// Waits for `pid` to exit (5s), then SIGKILLs it.
fn terminate_pid(pid: Option<i32>) {
    let Some(pid) = pid else { return };
    let deadline = Instant::now() + Duration::from_secs(5);
    while is_alive(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(40));
    }
    if is_alive(pid) {
        let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), nix::sys::signal::Signal::SIGKILL);
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // Daemons first (they own the sessions), so nothing rewrites files while the directory is removed.
        self.stop_dev_daemon();
        let daemon = read_pid(&self.home().join("daemon.pid"));
        if let Some(pid) = daemon {
            let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), nix::sys::signal::Signal::SIGTERM);
        }
        terminate_pid(daemon);
        let children: BTreeSet<i32> = self
            .children
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .copied()
            .collect();
        for pid in &children {
            signal_group(*pid, nix::sys::signal::Signal::SIGTERM);
        }
        std::thread::sleep(Duration::from_millis(20));
        for pid in &children {
            signal_group(*pid, nix::sys::signal::Signal::SIGKILL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_dirs_use_the_short_prefix_and_socket_paths_fit() {
        let sandbox = Sandbox::new("unit");
        let name = sandbox.root().file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with(SANDBOX_PREFIX), "{name}");
        assert!(sandbox.socket().as_os_str().len() <= MAX_SOCKET_PATH);
    }

    #[test]
    #[should_panic(expected = "unix socket limit")]
    fn too_long_socket_paths_panic_with_a_clear_message() {
        let long = Path::new("/tmp").join("x".repeat(120));
        assert_socket_paths_fit("unit", &long);
    }
}
