//! Port of agentVersionCheck.ts.
//!
//! The daemon's agent version I/O: the installed version (`<binary> --version`, cached by the binary's realpath and
//! mtime), the latest release (`npm view <package> version`, cached, refreshed at most every LATEST_MAX_AGE in the
//! background or on demand), and running an agent's own update command. Everything runs with the daemon's
//! environment, the one agents are launched with. Failures make a version unknown; nothing here fails except
//! [`AgentVersionChecker::update`]. Version parsing and comparison are pure (`deckhand_core::agent_versions`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use futures::future::LocalBoxFuture;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::async_util::{KeyedSharedSlot, SharedSlot, SharedTask};
use deckhand_core::error::Result;
use deckhand_core::session_order::SessionLike;
use deckhand_protocol::data::{AgentVersions, ProgramKey};

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

/// TS: agentVersionCheck.ts findOnPath (the first executable regular file named `program` on `path_value`, default
/// `$PATH`: as agent launches resolve it).
pub async fn find_on_path(program: &str, path_value: Option<&str>) -> Option<PathBuf> {
    let _ = (program, path_value);
    todo!("I8")
}

/// TS: agentVersionCheck.ts RunResult.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunResult {
    /// `None` when killed by a signal or never started.
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

/// TS: agentVersionCheck.ts runBounded (stdin closed, the daemon's environment, its own process group: a timeout
/// sends SIGTERM to the group, SIGKILL after KILL_GRACE; never fails).
pub async fn run_bounded(file: &Path, args: &[&str], options: &RunOptions) -> RunResult {
    let _ = (file, args, options);
    todo!("I8")
}

/// TS: agentVersionCheck.ts `process.on('exit')` handler: SIGKILLs the process groups of runs still going (the daemon
/// calls it on its way out; dropped runs are already killed).
pub fn kill_running_groups() {
    todo!("I8")
}

/// TS: agentVersionCheck.ts LatestLookup (`program`, its npm package → the latest release; `None` or an error when
/// unknown).
pub type LatestLookup = Rc<dyn Fn(ProgramKey, String) -> LocalBoxFuture<'static, Result<Option<String>>>>;

/// TS: agentVersionCheck.ts npmLatest (`npm view <package> version`; `DECKHAND_AGENT_LATEST` (JSON, program →
/// version) replaces the lookup entirely so tests never reach the network: a program missing from it fails with `not
/// listed in DECKHAND_AGENT_LATEST`).
pub async fn npm_latest(program: ProgramKey, npm_package: &str) -> Result<Option<String>> {
    let _ = (program, npm_package);
    todo!("I8")
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
#[allow(dead_code)] // Wave 0 skeleton: read once I8 implements the checker.
struct BinaryVersion {
    stamp: String,
    version: SharedTask<Option<String>>,
}

/// One latest-release lookup (TS `{version?, checkedAt, error?}`; `checked_at`: epoch ms).
#[derive(Clone, Debug, PartialEq)]
#[allow(dead_code)] // Wave 0 skeleton: read once I8 implements the checker.
struct LatestEntry {
    version: Option<String>,
    checked_at: f64,
    error: Option<String>,
}

#[allow(dead_code)] // Wave 0 skeleton: read once I8 implements the checker.
struct CheckerInner {
    /// By realpath; only successes are kept.
    binaries: RefCell<HashMap<String, BinaryVersion>>,
    latest: RefCell<HashMap<ProgramKey, LatestEntry>>,
    /// One latest-release refresh at a time.
    latest_run: SharedSlot<()>,
    /// One update per agent at a time; a second request gets the same result.
    updates: KeyedSharedSlot<ProgramKey, Result<UpdateOutcome>>,
    lookup: LatestLookup,
}

/// TS: agentVersionCheck.ts AgentVersionChecker. Clones share the caches.
#[derive(Clone)]
#[allow(dead_code)] // Wave 0 skeleton: read once I8 implements the checker.
pub struct AgentVersionChecker {
    inner: Rc<CheckerInner>,
}

impl AgentVersionChecker {
    /// TS: `new AgentVersionChecker(lookup = npmLatest)`.
    pub fn new(lookup: Option<LatestLookup>) -> Self {
        let _ = lookup;
        todo!("I8")
    }

    /// TS: AgentVersionChecker.versionOf (the binary `command` names: a path, or a name looked up on PATH).
    pub async fn version_of(&self, command: &str) -> InstalledVersion {
        let _ = command;
        todo!("I8")
    }

    /// TS: AgentVersionChecker.installed (`program` on PATH).
    pub async fn installed(&self, program: ProgramKey) -> InstalledVersion {
        let _ = program;
        todo!("I8")
    }

    /// TS: AgentVersionChecker.refreshLatest (every agent's latest release again: always when `force`, else only once
    /// LATEST_MAX_AGE passed; one run at a time, shared). Spawned at once on the current `LocalSet`.
    pub fn refresh_latest(&self, force: bool) -> SharedTask<()> {
        let _ = force;
        todo!("I8")
    }

    /// TS: AgentVersionChecker.updating.
    pub fn updating(&self, program: ProgramKey) -> bool {
        let _ = program;
        todo!("I8")
    }

    /// TS: AgentVersionChecker.snapshot (every agent's versions, with its running sessions and how many launched
    /// with an older version).
    pub async fn snapshot<S: SessionLike>(&self, sessions: &[S]) -> AgentVersions {
        let _ = sessions;
        todo!("I8")
    }

    /// TS: AgentVersionChecker.update (the agent's own update command, then the installed version read again; a
    /// second request while one runs gets the same result; `<Label> is not installed; run deckhand setup to install
    /// it`). `on_start` runs once the update has started.
    pub async fn update(&self, program: ProgramKey, on_start: Option<Box<dyn FnOnce()>>) -> Result<UpdateOutcome> {
        let _ = (program, on_start);
        todo!("I8")
    }
}
