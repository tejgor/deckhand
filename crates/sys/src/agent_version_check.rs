//! Port of agentVersionCheck.ts.
//!
//! The daemon's agent version I/O: the installed version (`<binary> --version`, cached by the binary's realpath and
//! mtime), the latest release (`npm view <package> version`, cached, refreshed at most every LATEST_MAX_AGE in the
//! background or on demand), and running an agent's own update command. Everything runs with the daemon's
//! environment, the one agents are launched with. Failures make a version unknown; nothing here fails except
//! [`AgentVersionChecker::update`]. Version parsing and comparison are pure (`deckhand_core::agent_versions`).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::rc::Rc;
use std::sync::Mutex;
use std::time::Duration;

use futures::FutureExt;
use futures::future::{LocalBoxFuture, join_all};
use nix::sys::signal::Signal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_with::skip_serializing_none;
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::async_util::{KeyedSharedSlot, SharedSlot, SharedTask};
use crate::paths::home_dir;
use crate::proc::{self, ExecOptions, GroupGuard};
use deckhand_core::agent_versions::{AGENT_PROGRAMS, parse_version, session_outdated};
use deckhand_core::agents::agent_spec;
use deckhand_core::error::{DhError, Result, errno_code};
use deckhand_core::jsstr::{date, js_len, js_slice, js_trim, js_trim_end, path as jspath};
use deckhand_core::session_order::SessionLike;
use deckhand_protocol::data::{AgentVersionInfo, AgentVersions, ProgramKey, SessionRecord, SessionStatus};

/// TS: agentVersionCheck.ts VERSION_TIMEOUT_MS.
pub const VERSION_TIMEOUT: Duration = Duration::from_millis(10_000);
/// TS: agentVersionCheck.ts LATEST_TIMEOUT_MS.
pub const LATEST_TIMEOUT: Duration = Duration::from_millis(10_000);
/// TS: agentVersionCheck.ts LATEST_MAX_AGE_MS (latest releases are looked up again after this).
pub const LATEST_MAX_AGE: Duration = Duration::from_millis(6 * 60 * 60_000);
/// TS: agentVersionCheck.ts UPDATE_TIMEOUT_MS.
pub const UPDATE_TIMEOUT: Duration = Duration::from_millis(5 * 60_000);
/// TS: agentVersionCheck.ts UPDATE_OUTPUT_LIMIT (characters of updater output kept).
pub const UPDATE_OUTPUT_LIMIT: usize = 8000;
/// TS: agentVersionCheck.ts KILL_GRACE_MS (SIGTERM, then SIGKILL after this).
pub const KILL_GRACE: Duration = Duration::from_millis(2000);

/// Characters of `--version` output kept.
const VERSION_OUTPUT_LIMIT: usize = 2000;
/// Characters of `npm view` output kept.
const LATEST_OUTPUT_LIMIT: usize = 4096;
/// The variable that replaces the npm lookup (tests never reach the network).
const LATEST_OVERRIDE_VAR: &str = "DECKHAND_AGENT_LATEST";

/// TS: agentVersionCheck.ts findOnPath (the first executable regular file named `program` on `path_value`, default
/// `$PATH`: as agent launches resolve it).
pub async fn find_on_path(program: &str, path_value: Option<&str>) -> Option<PathBuf> {
    let from_env;
    let path_value = match path_value {
        Some(value) => value,
        None => {
            from_env = std::env::var_os("PATH")
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default();
            from_env.as_str()
        }
    };
    for directory in path_value.split(':') {
        if directory.is_empty() {
            continue;
        }
        let candidate = jspath::join(&[directory, program]);
        // fs.access(X_OK) then fs.stat: both follow symlinks, so a link to an executable file counts.
        if nix::unistd::access(Path::new(&candidate), nix::unistd::AccessFlags::X_OK).is_err() {
            continue;
        }
        if tokio::fs::metadata(&candidate).await.is_ok_and(|meta| meta.is_file()) {
            return Some(PathBuf::from(candidate));
        }
    }
    None
}

/// TS: agentVersionCheck.ts RunResult.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunResult {
    /// `None` (TS `null`, always written) when killed by a signal or never started.
    #[serialize_always]
    pub exit_code: Option<i32>,
    /// Combined stdout and stderr, the last `limit` characters.
    pub output: String,
    pub timed_out: bool,
    /// Why it could not be started.
    pub error: Option<String>,
}

/// TS: agentVersionCheck.ts runBounded options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunOptions {
    pub timeout: Duration,
    /// Characters of combined output kept (the last ones).
    pub limit: usize,
    /// Default the home directory.
    pub cwd: Option<PathBuf>,
}

/// TS `runningGroups`: runs are detached (their own process group), so they would outlive a daemon that exits
/// mid-run; [`kill_running_groups`] stops them on exit.
static RUNNING_GROUPS: Mutex<Option<HashSet<u32>>> = Mutex::new(None);

/// Keeps a run's process group in [`RUNNING_GROUPS`] while it runs.
struct RunningGroup(u32);

impl RunningGroup {
    fn register(pid: u32) -> Self {
        if let Ok(mut groups) = RUNNING_GROUPS.lock() {
            groups.get_or_insert_with(HashSet::new).insert(pid);
        }
        RunningGroup(pid)
    }
}

impl Drop for RunningGroup {
    fn drop(&mut self) {
        if let Ok(mut groups) = RUNNING_GROUPS.lock()
            && let Some(groups) = groups.as_mut()
        {
            groups.remove(&self.0);
        }
    }
}

/// TS `kill(signal)`: the group, else the child alone.
fn kill_run(pid: u32, signal: Signal) {
    if proc::kill_group(pid, signal).is_err() {
        let _ = proc::kill_pid(pid, signal);
    }
}

/// Node's `error.message` for a failed spawn: `spawn <file> <CODE>`.
fn spawn_error_message(file: &str, error: &std::io::Error) -> String {
    match errno_code(error) {
        Some(code) => format!("spawn {file} {code}"),
        None => error.to_string(),
    }
}

/// Reads the next chunk of an open stream; never resolves once it is closed (the select branch is disabled then).
async fn read_chunk<R: AsyncRead + Unpin>(stream: &mut Option<R>, buffer: &mut [u8]) -> std::io::Result<usize> {
    match stream {
        Some(stream) => stream.read(buffer).await,
        None => std::future::pending().await,
    }
}

/// `output = `${output}${chunk.toString('utf8')}`.slice(-limit)` (`slice(-0)` keeps everything).
fn append_bounded(output: &mut String, chunk: &[u8], limit: usize) {
    output.push_str(&String::from_utf8_lossy(chunk));
    if limit > 0 && js_len(output) > limit {
        let keep = i64::try_from(limit).unwrap_or(i64::MAX);
        *output = js_slice(output, -keep, None);
    }
}

/// TS: agentVersionCheck.ts runBounded (stdin closed, the daemon's environment, its own process group: a timeout
/// sends SIGTERM to the group, SIGKILL after KILL_GRACE; never fails).
pub async fn run_bounded(file: &Path, args: &[&str], options: &RunOptions) -> RunResult {
    let program = file.to_string_lossy().into_owned();
    let exec_options = ExecOptions {
        cwd: Some(options.cwd.clone().unwrap_or_else(home_dir)),
        ..ExecOptions::default()
    };
    let mut command = proc::command(&program, args, &exec_options);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut output = String::new();
    let mut timed_out = false;
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return RunResult {
                exit_code: None,
                output,
                timed_out,
                error: Some(spawn_error_message(&program, &error)),
            };
        }
    };
    let pid = child.id();
    let _registered = pid.map(RunningGroup::register);
    // Dropping the run (its future) kills the group; a run that finishes leaves it alone.
    let mut guard = GroupGuard::new(pid, true);
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let mut stdout_buffer = vec![0u8; 64 * 1024];
    let mut stderr_buffer = vec![0u8; 64 * 1024];
    let deadline = tokio::time::sleep(options.timeout);
    tokio::pin!(deadline);
    let mut status = None;
    // Node's `close`: the child exited and both pipes are closed.
    while status.is_none() || stdout.is_some() || stderr.is_some() {
        tokio::select! {
            read = read_chunk(&mut stdout, &mut stdout_buffer), if stdout.is_some() => match read {
                Ok(0) | Err(_) => stdout = None,
                Ok(count) => append_bounded(&mut output, stdout_buffer.get(..count).unwrap_or_default(), options.limit),
            },
            read = read_chunk(&mut stderr, &mut stderr_buffer), if stderr.is_some() => match read {
                Ok(0) | Err(_) => stderr = None,
                Ok(count) => append_bounded(&mut output, stderr_buffer.get(..count).unwrap_or_default(), options.limit),
            },
            exited = child.wait(), if status.is_none() => status = Some(exited.ok().and_then(|status| status.code())),
            () = &mut deadline, if !timed_out => {
                timed_out = true;
                if let Some(pid) = pid {
                    kill_run(pid, Signal::SIGTERM);
                    // TS: an unref'd SIGKILL timer, which fires whether or not the group is gone by then.
                    tokio::spawn(async move {
                        tokio::time::sleep(KILL_GRACE).await;
                        kill_run(pid, Signal::SIGKILL);
                    });
                }
            },
        }
    }
    guard.disarm();
    RunResult {
        exit_code: status.flatten(),
        output,
        timed_out,
        error: None,
    }
}

/// TS: agentVersionCheck.ts `process.on('exit')` handler: SIGKILLs the process groups of runs still going (the daemon
/// calls it on its way out; dropped runs are already killed).
pub fn kill_running_groups() {
    let pids: Vec<u32> = match RUNNING_GROUPS.lock() {
        Ok(groups) => groups.iter().flatten().copied().collect(),
        Err(_) => return,
    };
    kill_groups(pids);
}

/// SIGKILLs each process group, ignoring ones already gone (TS `try { process.kill(-pid, 'SIGKILL'); } catch {}`).
fn kill_groups(pids: impl IntoIterator<Item = u32>) {
    for pid in pids {
        let _ = proc::kill_group(pid, Signal::SIGKILL);
    }
}

/// TS: agentVersionCheck.ts LatestLookup (`program`, its npm package → the latest release; `None` or an error when
/// unknown).
pub type LatestLookup = Rc<dyn Fn(ProgramKey, String) -> LocalBoxFuture<'static, Result<Option<String>>>>;

/// TS `firstLine`: the first non-blank line, trimmed.
fn first_line(text: &str) -> &str {
    text.split('\n')
        .map(js_trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
}

/// The `DECKHAND_AGENT_LATEST` branch of npmLatest: `JSON.parse(override || '{}')[program]`, which must be a string.
fn latest_from_override(text: &str, program: ProgramKey) -> Result<Option<String>> {
    let source = if text.is_empty() { "{}" } else { text };
    let parsed: Value = serde_json::from_str(source).map_err(|_| DhError::json_parse(source))?;
    let value = match &parsed {
        Value::Null => {
            return Err(DhError::msg(format!(
                "Cannot read properties of null (reading '{}')",
                program.as_str()
            )));
        }
        Value::Object(map) => map.get(program.as_str()),
        // A property of a number, string, boolean or array named claude/pi/codex is undefined.
        _ => None,
    };
    match value {
        Some(Value::String(version)) => Ok(parse_version(Some(version))),
        _ => Err(DhError::msg("not listed in DECKHAND_AGENT_LATEST")),
    }
}

/// TS: agentVersionCheck.ts npmLatest (`npm view <package> version`; `DECKHAND_AGENT_LATEST` (JSON, program →
/// version) replaces the lookup entirely so tests never reach the network: a program missing from it fails with `not
/// listed in DECKHAND_AGENT_LATEST`).
pub async fn npm_latest(program: ProgramKey, npm_package: &str) -> Result<Option<String>> {
    if let Some(value) = std::env::var_os(LATEST_OVERRIDE_VAR) {
        return latest_from_override(&value.to_string_lossy(), program);
    }
    let options = RunOptions {
        timeout: LATEST_TIMEOUT,
        limit: LATEST_OUTPUT_LIMIT,
        cwd: None,
    };
    let result = run_bounded(Path::new("npm"), &["view", npm_package, "version"], &options).await;
    if result.exit_code != Some(0) {
        let message = if result.timed_out {
            "npm view timed out".to_owned()
        } else if let Some(error) = result.error {
            error
        } else {
            match first_line(&result.output) {
                "" => format!(
                    "npm view exited with {}",
                    result
                        .exit_code
                        .map_or_else(|| "null".to_owned(), |code| code.to_string())
                ),
                line => line.to_owned(),
            }
        };
        return Err(DhError::msg(message));
    }
    match parse_version(Some(&result.output)) {
        Some(version) => Ok(Some(version)),
        None => Err(DhError::msg("npm view printed no version")),
    }
}

/// TS: agentVersionCheck.ts InstalledVersion.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledVersion {
    pub path: Option<String>,
    pub version: Option<String>,
    pub error: Option<String>,
}

/// TS: agentVersionCheck.ts UpdateOutcome.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateOutcome {
    pub ok: bool,
    #[serialize_always]
    pub exit_code: Option<i32>,
    pub command: String,
    pub output: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

/// One cached `--version` read (TS `{stamp, version}`): reused while the binary's mtime and size are unchanged.
#[derive(Clone)]
struct BinaryVersion {
    stamp: String,
    version: SharedTask<Option<String>>,
}

/// One latest-release lookup (TS `{version?, checkedAt, error?}`; `checked_at`: epoch ms).
#[derive(Clone, Debug, PartialEq)]
struct LatestEntry {
    version: Option<String>,
    checked_at: f64,
    error: Option<String>,
}

struct CheckerInner {
    /// By realpath; only successes are kept.
    binaries: RefCell<HashMap<String, BinaryVersion>>,
    latest: RefCell<HashMap<ProgramKey, LatestEntry>>,
    /// One latest-release refresh at a time.
    latest_run: SharedSlot<()>,
    /// One update per agent at a time; a second request gets the same result.
    updates: KeyedSharedSlot<ProgramKey, Result<UpdateOutcome>>,
    lookup: LatestLookup,
    /// Where agents are looked up instead of `$PATH` (tests only: the TS reads `process.env.PATH`).
    search_path: Option<String>,
}

/// TS: agentVersionCheck.ts AgentVersionChecker. Clones share the caches.
#[derive(Clone)]
pub struct AgentVersionChecker {
    inner: Rc<CheckerInner>,
}

/// The default lookup (TS `lookup = npmLatest`).
fn npm_lookup() -> LatestLookup {
    Rc::new(|program, npm_package| async move { npm_latest(program, &npm_package).await }.boxed_local())
}

/// `${stat.mtimeMs}:${stat.size}` of the binary's realpath, with that realpath.
async fn binary_stamp(file: &str) -> Option<(String, String)> {
    let real = tokio::fs::canonicalize(file).await.ok()?;
    let meta = tokio::fs::metadata(&real).await.ok()?;
    let stamp = format!("{}.{:09}:{}", meta.mtime(), meta.mtime_nsec(), meta.size());
    Some((real.to_string_lossy().into_owned(), stamp))
}

/// Epoch milliseconds as the TS `Date.now()` number.
#[allow(clippy::cast_precision_loss)] // epoch ms fit an f64 exactly (as in JS)
fn now_ms() -> f64 {
    date::now_ms() as f64
}

impl AgentVersionChecker {
    /// TS: `new AgentVersionChecker(lookup = npmLatest)`.
    pub fn new(lookup: Option<LatestLookup>) -> Self {
        Self::with_search_path(lookup, None)
    }

    fn with_search_path(lookup: Option<LatestLookup>, search_path: Option<String>) -> Self {
        AgentVersionChecker {
            inner: Rc::new(CheckerInner {
                binaries: RefCell::new(HashMap::new()),
                latest: RefCell::new(HashMap::new()),
                latest_run: SharedSlot::new(),
                updates: KeyedSharedSlot::new(),
                lookup: lookup.unwrap_or_else(npm_lookup),
                search_path,
            }),
        }
    }

    /// TS: AgentVersionChecker.versionOf (the binary `command` names: a path, or a name looked up on PATH).
    pub async fn version_of(&self, command: &str) -> InstalledVersion {
        let file = if command.contains('/') {
            command.to_owned()
        } else {
            match find_on_path(command, self.inner.search_path.as_deref()).await {
                Some(found) => found.to_string_lossy().into_owned(),
                None => {
                    return InstalledVersion {
                        error: Some("not found on PATH".to_owned()),
                        ..InstalledVersion::default()
                    };
                }
            }
        };
        let Some((real, stamp)) = binary_stamp(&file).await else {
            return InstalledVersion {
                error: Some(format!("{file} is missing")),
                ..InstalledVersion::default()
            };
        };
        let cached = {
            let mut binaries = self.inner.binaries.borrow_mut();
            match binaries.get(&real) {
                Some(entry) if entry.stamp == stamp => entry.version.clone(),
                _ => {
                    let binary = file.clone();
                    let version = async move {
                        let options = RunOptions {
                            timeout: VERSION_TIMEOUT,
                            limit: VERSION_OUTPUT_LIMIT,
                            cwd: None,
                        };
                        parse_version(Some(
                            &run_bounded(Path::new(&binary), &["--version"], &options).await.output,
                        ))
                    }
                    .boxed_local()
                    .shared();
                    binaries.insert(
                        real.clone(),
                        BinaryVersion {
                            stamp,
                            version: version.clone(),
                        },
                    );
                    version
                }
            }
        };
        let version = cached.clone().await;
        if version.is_none() {
            let mut binaries = self.inner.binaries.borrow_mut();
            if binaries.get(&real).is_some_and(|entry| entry.version.ptr_eq(&cached)) {
                binaries.remove(&real);
            }
        }
        match version {
            Some(version) => InstalledVersion {
                path: Some(file),
                version: Some(version),
                error: None,
            },
            None => InstalledVersion {
                error: Some(format!("{} --version printed no version", jspath::basename(&file))),
                path: Some(file),
                version: None,
            },
        }
    }

    /// TS: AgentVersionChecker.installed (`program` on PATH).
    pub async fn installed(&self, program: ProgramKey) -> InstalledVersion {
        self.version_of(program.as_str()).await
    }

    /// TS: AgentVersionChecker.refreshLatest (every agent's latest release again: always when `force`, else only once
    /// LATEST_MAX_AGE passed; one run at a time, shared). Spawned at once on the current `LocalSet`.
    pub fn refresh_latest(&self, force: bool) -> SharedTask<()> {
        if let Some(run) = self.inner.latest_run.get() {
            return run;
        }
        let now = now_ms();
        #[allow(clippy::cast_precision_loss)] // six hours of milliseconds
        let max_age = LATEST_MAX_AGE.as_millis() as f64;
        let fresh = {
            let latest = self.inner.latest.borrow();
            AGENT_PROGRAMS
                .iter()
                .all(|program| now - latest.get(program).map_or(f64::NEG_INFINITY, |entry| entry.checked_at) < max_age)
        };
        if !force && fresh {
            return futures::future::ready(()).boxed_local().shared();
        }
        let inner = Rc::clone(&self.inner);
        self.inner.latest_run.get_or_start(move || async move {
            join_all(AGENT_PROGRAMS.map(|program| {
                let inner = Rc::clone(&inner);
                async move {
                    let lookup = Rc::clone(&inner.lookup);
                    let result = lookup(program, agent_spec(program).version.npm_package.to_owned()).await;
                    let checked_at = now_ms();
                    let entry = match result {
                        Ok(version) => LatestEntry {
                            version,
                            checked_at,
                            error: None,
                        },
                        Err(error) => LatestEntry {
                            version: None,
                            checked_at,
                            error: Some(format!("latest unknown: {error}")),
                        },
                    };
                    inner.latest.borrow_mut().insert(program, entry);
                }
            }))
            .await;
        })
    }

    /// TS: AgentVersionChecker.updating.
    pub fn updating(&self, program: ProgramKey) -> bool {
        self.inner.updates.contains(&program)
    }

    /// One agent's snapshot entry.
    async fn program_info(&self, program: ProgramKey, sessions: &[&SessionRecord]) -> AgentVersionInfo {
        let installed = self.installed(program).await;
        let latest = self.inner.latest.borrow().get(&program).cloned();
        let running: Vec<&SessionRecord> = sessions
            .iter()
            .copied()
            .filter(|session| session.program == program && session.status == SessionStatus::Running)
            .collect();
        let outdated = running
            .iter()
            .filter(|session| session_outdated(session, installed.version.as_deref()))
            .count();
        let path_without_version = installed.path.as_deref().is_some_and(|path| !path.is_empty())
            && installed.version.as_deref().is_none_or(str::is_empty);
        AgentVersionInfo {
            program,
            installed: installed.version.clone(),
            latest: latest.as_ref().and_then(|entry| entry.version.clone()),
            path: installed.path.clone(),
            #[allow(clippy::cast_possible_truncation)] // integral epoch ms
            checked_at: latest
                .as_ref()
                .and_then(|entry| date::to_iso_string(entry.checked_at as i64)),
            error: if path_without_version {
                installed.error.clone()
            } else {
                latest.as_ref().and_then(|entry| entry.error.clone())
            },
            updating: self.updating(program).then_some(true),
            running: i64::try_from(running.len()).unwrap_or(i64::MAX),
            outdated: i64::try_from(outdated).unwrap_or(i64::MAX),
        }
    }

    /// TS: AgentVersionChecker.snapshot (every agent's versions, with its running sessions and how many launched
    /// with an older version).
    pub async fn snapshot<S: SessionLike>(&self, sessions: &[S]) -> AgentVersions {
        let all: Vec<&SessionRecord> = sessions.iter().map(SessionLike::session).collect();
        let [claude, pi, codex] = AGENT_PROGRAMS;
        let (claude, pi, codex) = futures::join!(
            self.program_info(claude, &all),
            self.program_info(pi, &all),
            self.program_info(codex, &all)
        );
        AgentVersions { claude, pi, codex }
    }

    /// TS: AgentVersionChecker.update (the agent's own update command, then the installed version read again; a
    /// second request while one runs gets the same result; `<Label> is not installed; run deckhand setup to install
    /// it`). `on_start` runs once the update has started.
    pub async fn update(&self, program: ProgramKey, on_start: Option<Box<dyn FnOnce()>>) -> Result<UpdateOutcome> {
        let spec = agent_spec(program);
        if let Some(running) = self.inner.updates.get(&program) {
            return running.await;
        }
        let before = self.installed(program).await;
        if let Some(started) = self.inner.updates.get(&program) {
            return started.await;
        }
        let Some(path) = before.path.clone() else {
            return Err(DhError::msg(format!(
                "{} is not installed; run deckhand setup to install it",
                spec.label
            )));
        };
        let command = format!("{} {}", program.as_str(), spec.version.update_args.join(" "));
        let checker = self.clone();
        let run = self.inner.updates.get_or_start(program, move || async move {
            let options = RunOptions {
                timeout: UPDATE_TIMEOUT,
                limit: UPDATE_OUTPUT_LIMIT,
                cwd: None,
            };
            let result = run_bounded(Path::new(&path), spec.version.update_args, &options).await;
            // The updater may have replaced the binary in place; read it again.
            checker.inner.binaries.borrow_mut().clear();
            let after = checker.installed(program).await;
            let note = if result.timed_out {
                format!("\n(stopped after {} minutes)", UPDATE_TIMEOUT.as_millis() / 60_000)
            } else if let Some(error) = &result.error {
                format!("\n{error}")
            } else {
                String::new()
            };
            Ok(UpdateOutcome {
                ok: result.exit_code == Some(0),
                exit_code: result.exit_code,
                command,
                output: js_trim(&format!("{}{note}", js_trim_end(&result.output))).to_owned(),
                before: before.version,
                after: after.version,
            })
        });
        if let Some(on_start) = on_start {
            on_start();
        }
        run.await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deckhand_parity::{Case, DirOp, replay_async, replay_dir, tags};
    use std::cell::Cell;
    use std::os::unix::fs::PermissionsExt;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a current_thread runtime")
    }

    fn local<F: std::future::Future>(future: F) -> F::Output {
        runtime().block_on(tokio::task::LocalSet::new().run_until(future))
    }

    fn undefined_or<T: Serialize>(value: Option<T>) -> Value {
        value.map_or_else(tags::undefined, |value| {
            serde_json::to_value(value).unwrap_or(Value::Null)
        })
    }

    #[derive(Deserialize)]
    struct RunArgs {
        timeout: u64,
        limit: usize,
    }

    #[test]
    fn run_bounded_parity() {
        local(replay_async("I8", "agentVersionCheck/runBounded", |case: &Case| {
            let file = case.arg_str(0).map(str::to_owned);
            let args = case.arg_as::<Vec<String>>(1);
            let options = case.arg_as::<RunArgs>(2);
            async move {
                let (file, args, options) = (file?, args?, options?);
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                let options = RunOptions {
                    timeout: Duration::from_millis(options.timeout),
                    limit: options.limit,
                    cwd: None,
                };
                Ok::<_, String>(run_bounded(Path::new(&file), &args, &options).await)
            }
        }));
    }

    #[test]
    fn npm_latest_override_parity() {
        local(replay_async("I8", "agentVersionCheck/npmLatest", |case: &Case| {
            let outcome = (|| -> std::result::Result<Result<Option<String>>, String> {
                Ok(latest_from_override(case.arg_str(0)?, case.arg_as::<ProgramKey>(1)?))
            })();
            async move {
                match outcome {
                    Ok(Ok(version)) => Ok(undefined_or(version)),
                    Ok(Err(error)) => Err(error.to_string()),
                    Err(problem) => Err(format!("bad case: {problem}")),
                }
            }
        }));
    }

    fn find_case(root: &Path, op: &DirOp) -> std::result::Result<Value, String> {
        let _ = root;
        let program = op.arg_str(0)?.to_owned();
        let path_value = op.arg_str(1)?.to_owned();
        let found = runtime().block_on(find_on_path(&program, Some(&path_value)));
        Ok(undefined_or(found.map(|path| path.to_string_lossy().into_owned())))
    }

    #[test]
    fn find_on_path_parity() {
        for case in [
            "find-broken-symlink",
            "find-empty-path",
            "find-empty-path-entries",
            "find-first-of-two",
            "find-nested-program",
            "find-not-found",
            "find-relative-entry",
            "find-skips-directory",
            "find-skips-non-executable",
            "find-symlink-to-executable",
            "find-trailing-slash-entry",
            "find-unicode-name",
        ] {
            replay_dir("I8", &format!("agentVersionCheck/{case}"), find_case);
        }
    }

    /// A fake agent: `--version` prints the contents of `<dir>/version`; `update` prints a line, writes `next` (when
    /// present) over `version`, and exits with the contents of `<dir>/update-exit` (default 0).
    fn fake_agent(dir: &Path, name: &str, version: &str) -> PathBuf {
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).expect("bin dir");
        let state = dir.join(name);
        std::fs::create_dir_all(&state).expect("state dir");
        std::fs::write(state.join("version"), version).expect("version file");
        let script = format!(
            "#!/bin/sh\nstate='{}'\necho run >> \"$state/calls\"\nif [ \"$1\" = --version ]; then cat \"$state/version\"; exit 0; fi\nif [ \"$1\" = update ]; then echo \"updating $*\"; echo warn 1>&2; if [ -f \"$state/next\" ]; then cp \"$state/next\" \"$state/version\"; fi; if [ -f \"$state/update-sleep\" ]; then sleep \"$(cat \"$state/update-sleep\")\"; fi; exit \"$(cat \"$state/update-exit\" 2>/dev/null || echo 0)\"; fi\nexit 9\n",
            state.display()
        );
        let file = bin.join(name);
        std::fs::write(&file, script).expect("script");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        file
    }

    fn calls(dir: &Path, name: &str) -> usize {
        std::fs::read_to_string(dir.join(name).join("calls"))
            .map(|text| text.lines().count())
            .unwrap_or(0)
    }

    fn fixed_lookup(versions: &'static [(ProgramKey, &'static str)], count: Rc<Cell<usize>>) -> LatestLookup {
        Rc::new(move |program, package| {
            count.set(count.get() + 1);
            async move {
                assert_eq!(package, agent_spec(program).version.npm_package);
                match versions.iter().find(|(key, _)| *key == program) {
                    Some((_, version)) => Ok(Some((*version).to_owned())),
                    None => Err(DhError::msg("offline")),
                }
            }
            .boxed_local()
        })
    }

    fn session(program: &str, status: &str, agent_version: Option<&str>) -> SessionRecord {
        let mut value = serde_json::json!({
            "id": "s", "title": "t", "program": program, "command": program, "cwd": "/", "repoRoot": "/",
            "status": status, "createdAt": "2026-01-01T00:00:00.000Z", "updatedAt": "2026-01-01T00:00:00.000Z",
        });
        if let (Some(version), Some(object)) = (agent_version, value.as_object_mut()) {
            object.insert("agentVersion".into(), Value::String(version.to_owned()));
        }
        serde_json::from_value(value).expect("a session record")
    }

    #[test]
    fn version_of_reads_and_caches_by_realpath_and_stamp() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = fake_agent(dir.path(), "claude", "2.1.287 (Claude Code)\n");
        let file_text = file.to_string_lossy().into_owned();
        local(async {
            let checker = AgentVersionChecker::new(None);
            let first = checker.version_of(&file_text).await;
            assert_eq!(first.version.as_deref(), Some("2.1.287"));
            assert_eq!(first.path.as_deref(), Some(file_text.as_str()));
            assert_eq!(checker.version_of(&file_text).await, first);
            assert_eq!(calls(dir.path(), "claude"), 1, "the second read is cached");
            // A different size changes the stamp.
            std::fs::write(dir.path().join("claude/version"), "codex-cli 10.20.30\n").expect("version");
            let script = std::fs::read_to_string(&file).expect("script");
            std::fs::write(&file, format!("{script}\n")).expect("script");
            assert_eq!(
                checker.version_of(&file_text).await.version.as_deref(),
                Some("10.20.30")
            );
            assert_eq!(calls(dir.path(), "claude"), 2);
            // Failures are not cached.
            std::fs::write(dir.path().join("claude/version"), "nothing here").expect("version");
            std::fs::write(&file, format!("{script}\n\n")).expect("script");
            let failed = checker.version_of(&file_text).await;
            assert_eq!(failed.error.as_deref(), Some("claude --version printed no version"));
            assert_eq!(failed.path.as_deref(), Some(file_text.as_str()));
            assert_eq!(failed.version, None);
            checker.version_of(&file_text).await;
            assert_eq!(calls(dir.path(), "claude"), 4);
            assert_eq!(
                checker.version_of("/no/such/deckhand-agent").await,
                InstalledVersion {
                    error: Some("/no/such/deckhand-agent is missing".into()),
                    ..InstalledVersion::default()
                }
            );
            assert_eq!(
                checker.version_of("deckhand-no-such-agent-binary").await,
                InstalledVersion {
                    error: Some("not found on PATH".into()),
                    ..InstalledVersion::default()
                }
            );
        });
    }

    #[test]
    fn refresh_latest_runs_once_at_a_time_and_only_when_due() {
        let count = Rc::new(Cell::new(0));
        let lookup = fixed_lookup(
            &[(ProgramKey::Claude, "2.0.0"), (ProgramKey::Pi, "1.0.0")],
            Rc::clone(&count),
        );
        let dir = tempfile::tempdir().expect("tempdir");
        let empty_path = dir.path().to_string_lossy().into_owned();
        local(async {
            let checker = AgentVersionChecker::with_search_path(Some(lookup), Some(empty_path));
            let first = checker.refresh_latest(false);
            let second = checker.refresh_latest(true);
            assert!(first.ptr_eq(&second), "one run at a time");
            first.await;
            assert_eq!(count.get(), 3);
            checker.refresh_latest(false).await;
            assert_eq!(count.get(), 3, "fresh results are kept");
            checker.refresh_latest(true).await;
            assert_eq!(count.get(), 6);
            let versions = checker.snapshot::<SessionRecord>(&[]).await;
            assert_eq!(versions.claude.latest.as_deref(), Some("2.0.0"));
            assert_eq!(versions.pi.latest.as_deref(), Some("1.0.0"));
            assert_eq!(versions.codex.latest, None);
            assert_eq!(versions.codex.error.as_deref(), Some("latest unknown: offline"));
            assert_eq!(versions.claude.error, None);
            assert!(
                versions
                    .claude
                    .checked_at
                    .as_deref()
                    .is_some_and(|at| at.ends_with('Z'))
            );
            assert_eq!(versions.claude.installed, None);
            assert_eq!(versions.claude.path, None);
        });
    }

    #[test]
    fn snapshot_counts_running_and_outdated_sessions() {
        let dir = tempfile::tempdir().expect("tempdir");
        fake_agent(dir.path(), "claude", "2.1.0\n");
        let pi = fake_agent(dir.path(), "pi", "no version\n");
        let search = dir.path().join("bin").to_string_lossy().into_owned();
        let lookup = fixed_lookup(&[], Rc::new(Cell::new(0)));
        local(async {
            let checker = AgentVersionChecker::with_search_path(Some(lookup), Some(search));
            let sessions = [
                session("claude", "running", Some("2.0.9")),
                session("claude", "running", Some("2.1.0")),
                session("claude", "running", None),
                session("claude", "exited", Some("1.0.0")),
                session("pi", "running", Some("0.1.0")),
            ];
            let versions = checker.snapshot(&sessions).await;
            assert_eq!(versions.claude.installed.as_deref(), Some("2.1.0"));
            assert_eq!(versions.claude.running, 3);
            assert_eq!(versions.claude.outdated, 1);
            assert_eq!(versions.claude.updating, None);
            assert_eq!(versions.claude.checked_at, None);
            assert_eq!(versions.pi.installed, None);
            assert_eq!(versions.pi.path.as_deref(), Some(pi.to_string_lossy().as_ref()));
            assert_eq!(versions.pi.error.as_deref(), Some("pi --version printed no version"));
            assert_eq!(versions.pi.running, 1);
            assert_eq!(versions.pi.outdated, 0);
            assert_eq!(versions.codex.path, None);
            assert_eq!(versions.codex.error, None);
            let json = serde_json::to_string(&versions.codex).expect("json");
            assert_eq!(json, r#"{"program":"codex","running":0,"outdated":0}"#);
        });
    }

    #[test]
    fn update_runs_the_agents_updater_once_and_reads_the_version_again() {
        let dir = tempfile::tempdir().expect("tempdir");
        fake_agent(dir.path(), "claude", "1.0.0\n");
        fake_agent(dir.path(), "pi", "0.1.0\n");
        std::fs::write(dir.path().join("claude/next"), "1.2.0\n").expect("next");
        std::fs::write(dir.path().join("claude/update-sleep"), "0.2").expect("sleep");
        std::fs::write(dir.path().join("pi/update-exit"), "3").expect("exit");
        let search = dir.path().join("bin").to_string_lossy().into_owned();
        local(async {
            let checker = AgentVersionChecker::with_search_path(None, Some(search));
            // on_start runs once the update is registered: `updating` is true from then until it settles.
            let started = Rc::new(Cell::new(0));
            let counter = Rc::clone(&started);
            let watcher = checker.clone();
            let first = checker.update(
                ProgramKey::Claude,
                Some(Box::new(move || {
                    assert!(watcher.updating(ProgramKey::Claude));
                    counter.set(counter.get() + 1);
                })),
            );
            let second = checker.update(ProgramKey::Claude, None);
            let (first, second) = futures::join!(first, second);
            assert!(!checker.updating(ProgramKey::Claude));
            assert_eq!(started.get(), 1);
            let outcome = first.expect("an outcome");
            assert_eq!(second.expect("the same outcome"), outcome);
            assert_eq!(
                outcome,
                UpdateOutcome {
                    ok: true,
                    exit_code: Some(0),
                    command: "claude update".into(),
                    output: outcome.output.clone(),
                    before: Some("1.0.0".into()),
                    after: Some("1.2.0".into()),
                }
            );
            assert!(outcome.output.contains("updating update"), "{}", outcome.output);
            assert!(outcome.output.contains("warn"), "{}", outcome.output);
            assert_eq!(
                calls(dir.path(), "claude"),
                3,
                "--version once before, the update, --version again"
            );

            let failed = checker.update(ProgramKey::Pi, None).await.expect("an outcome");
            assert!(!failed.ok);
            assert_eq!(failed.exit_code, Some(3));
            assert_eq!(failed.command, "pi update --self");
            assert_eq!(failed.before.as_deref(), Some("0.1.0"));
            assert_eq!(failed.after.as_deref(), Some("0.1.0"));

            let missing = checker.update(ProgramKey::Codex, None).await;
            assert_eq!(
                missing.map_err(|error| error.to_string()),
                Err("Codex is not installed; run deckhand setup to install it".to_owned())
            );
            let json = serde_json::to_string(&failed).expect("json");
            assert!(json.starts_with(r#"{"ok":false,"exitCode":3,"command":"pi update --self","output":"#));
        });
    }

    #[test]
    fn run_bounded_times_out_and_kills_the_group() {
        local(async {
            let options = RunOptions {
                timeout: Duration::from_millis(100),
                limit: 100,
                cwd: None,
            };
            let started = std::time::Instant::now();
            // The background sleep holds the pipes open: only a group kill closes them.
            let result = run_bounded(Path::new("/bin/sh"), &["-c", "sleep 30 & printf x; wait"], &options).await;
            assert!(started.elapsed() < Duration::from_secs(10));
            assert_eq!(
                result,
                RunResult {
                    exit_code: None,
                    output: "x".into(),
                    timed_out: true,
                    error: None,
                }
            );
        });
    }

    #[test]
    fn helpers_match_the_ts_expressions() {
        assert_eq!(first_line("\n  \n  npm ERR! 404  \nmore"), "npm ERR! 404");
        assert_eq!(first_line(""), "");
        let mut output = String::new();
        append_bounded(&mut output, "abc".as_bytes(), 0);
        append_bounded(&mut output, "def".as_bytes(), 4);
        assert_eq!(output, "cdef");
        assert_eq!(
            latest_from_override("", ProgramKey::Pi).map_err(|error| error.to_string()),
            Err("not listed in DECKHAND_AGENT_LATEST".to_owned())
        );
        assert_eq!(
            latest_from_override("{\"pi\":\"v1.2.3\"}", ProgramKey::Pi).ok(),
            Some(Some("1.2.3".to_owned()))
        );
    }

    /// The exit handler's kill, on a group this test owns: `kill_running_groups` itself SIGKILLs every run in the
    /// process-wide set, which would kill the `run_bounded` children of tests running alongside this one.
    #[test]
    fn kill_groups_sigkills_each_group() {
        use std::os::unix::process::{CommandExt, ExitStatusExt};
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 30 & wait"])
            .process_group(0)
            .spawn()
            .expect("spawn /bin/sh");
        let started = std::time::Instant::now();
        kill_groups([child.id()]);
        let status = child.wait().expect("wait");
        assert_eq!(status.signal(), Some(9), "{status:?}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
