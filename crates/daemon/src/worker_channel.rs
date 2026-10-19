//! Port of daemon.ts (spawn workers, daemon side of the fd-3 NDJSON channel).
//!
//! The TS daemon `fork`s `cli.js --session-worker` / `--workspace-worker` and talks to the child over Node's IPC
//! channel (`process.send` / `child.on('message')`). The port re-executes the running binary with the same flag and
//! hands it one end of a `socketpair` as fd 3; both sides frame messages as NDJSON ([`deckhand_protocol::ndjson`]).
//! The worker's first message is `hello` with its [`deckhand_protocol::PROTOCOL_VERSION`]; a channel whose worker
//! speaks another version is closed, which fails every request on it (the daemon then reports the launch failed).
//!
//! Requests are `{...payload, requestId}` (the ID last, as requestChannel spreads it) and are answered by a
//! `response` carrying the same ID; responses settle their pending request inside the channel, every other message
//! is handed to the owner through [`ChannelEvents`] in arrival order, followed by one [`ChannelEvent::Exit`].
//!
//! The worker's stdout and stderr are appended to its log file (TS `child.stdout.on('data', appendFile)`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use deckhand_core::error::Result;
use deckhand_protocol::worker_ipc::{WorkerMessage, WorkerResponse};
use deckhand_sys::async_util::Timer;
use nix::sys::signal::Signal;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use serde_with::skip_serializing_none;
use tokio::sync::{mpsc, oneshot, watch};

/// TS: daemon.ts WORKER_REQUEST_TIMEOUT_MS (a request unanswered this long fails with `worker request timed out`).
pub const WORKER_REQUEST_TIMEOUT: Duration = Duration::from_millis(10_000);

/// TS: daemon.ts WORKER_KILL_GRACE_MS (stopWorker: SIGKILL a worker still alive this long after `kill`).
pub const WORKER_KILL_GRACE: Duration = Duration::from_millis(2000);

/// The fd the worker finds its end of the channel on (Node's IPC channel was fd 3 too: `stdio[3] = 'ipc'`).
pub const WORKER_CHANNEL_FD: i32 = 3;

/// TS: daemon.ts wireLog's `DECKHAND_WIRE_LOG` (test/porting aid: a directory every message is appended to).
pub const WIRE_LOG_ENV: &str = "DECKHAND_WIRE_LOG";

/// TS: daemon.ts channelKind result: which worker a channel talks to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WorkerKind {
    /// One agent PTY (sessionWorker.ts runSessionWorker).
    #[serde(rename = "session")]
    Session,
    /// The Terminal, Git, Dev and action panes of one workspace (sessionWorker.ts runWorkspaceWorker).
    #[serde(rename = "workspace")]
    Workspace,
}

impl WorkerKind {
    /// The argv flag that makes the binary run as this worker (`--session-worker` / `--workspace-worker`).
    pub fn flag(self) -> &'static str {
        todo!("D1")
    }
}

/// TS: daemon.ts wireLog `dir`: client->daemon, daemon->client, daemon->worker, worker->daemon.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireDir {
    #[serde(rename = "c2d")]
    C2d,
    #[serde(rename = "d2c")]
    D2c,
    #[serde(rename = "d2w")]
    D2w,
    #[serde(rename = "w2d")]
    W2d,
}

/// One wire-log line: `{dir, msg}`, or `{dir, worker, msg}` for IPC lines (TS key order).
#[skip_serializing_none]
#[derive(Debug, Serialize)]
pub struct WireLogLine<'a, T: Serialize + ?Sized> {
    pub dir: WireDir,
    pub worker: Option<WorkerKind>,
    pub msg: &'a T,
}

/// TS: daemon.ts wireLog: appends `{dir, [worker,] msg}` to `<DECKHAND_WIRE_LOG>/daemon-<pid>.jsonl` when the variable
/// is set; failures are ignored.
pub fn wire_log<T: Serialize + ?Sized>(dir: WireDir, msg: &T, worker: Option<WorkerKind>) {
    let _ = (dir, msg, worker);
    todo!("D1")
}

/// TS: daemon.ts channelKind.
pub fn channel_kind(worker: &WorkerChannel) -> WorkerKind {
    let _ = worker;
    todo!("D1")
}

/// TS: paths.ts getCliEntryPath as the daemon uses it: the binary to run as a worker (`configured`, else the running
/// executable).
pub fn worker_exe(configured: Option<&Path>) -> Result<PathBuf> {
    let _ = configured;
    todo!("D1")
}

/// What [`WorkerChannel::spawn`] starts (TS `fork(getCliEntryPath(), [flag], {env, stdio})`).
#[derive(Clone, Debug)]
pub struct SpawnOptions {
    pub kind: WorkerKind,
    /// The binary to run ([`worker_exe`]).
    pub exe: PathBuf,
    /// Added to the daemon's own environment (session workers: DECKHAND_SESSION_ID, DECKHAND_LAUNCH_ID,
    /// DECKHAND_HOOK_TOKEN).
    pub env: Vec<(String, String)>,
    /// stdout and stderr are appended here (paths.ts getWorkerLogPath).
    pub log_path: PathBuf,
}

/// How a worker process ended (TS `exitCode` / `signalCode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessExit {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

/// What a channel hands its owner (TS `child.on('message')` and `child.on('exit')`).
#[derive(Clone, Debug, PartialEq)]
pub enum ChannelEvent {
    /// Any message but `hello` and `response` (those are handled by the channel).
    Message(Box<WorkerMessage>),
    /// The process exited (always the last event).
    Exit(ProcessExit),
}

/// The receiving end of a channel's events.
pub type ChannelEvents = mpsc::UnboundedReceiver<ChannelEvent>;

/// TS: daemon.ts WorkerChannel `pending` entry: the request's settle callbacks and its timeout.
pub struct PendingRequest {
    pub reply: oneshot::Sender<Result<Value>>,
    pub timer: Timer,
}

/// TS: daemon.ts WorkerChannel: an IPC child the daemon sends requests to. Clones share the channel; identity is
/// [`WorkerChannel::same`].
#[derive(Clone)]
#[allow(dead_code)] // Wave 0 skeleton: read once D1 implements the channel.
pub struct WorkerChannel {
    inner: Rc<ChannelInner>,
}

/// The shared state behind a [`WorkerChannel`].
pub struct ChannelInner {
    pub kind: WorkerKind,
    pub pid: Option<u32>,
    /// Lines to write to the socket (a writer task owns the socket's write half); `None` once disconnected.
    pub writer: RefCell<Option<mpsc::UnboundedSender<String>>>,
    /// TS `pending`: requests awaiting their response, by request ID.
    pub pending: RefCell<HashMap<String, PendingRequest>>,
    /// TS `exited`: set by the daemon once it saw the worker exit (or retired it).
    pub exited: Cell<bool>,
    /// The process's exit, once it happened (TS `exitCode` / `signalCode` / `once('exit')`).
    pub process_exit: watch::Receiver<Option<ProcessExit>>,
}

impl WorkerChannel {
    /// TS: `fork(...)` plus the listeners daemon.ts wires before its first await: spawns `options.exe` with the kind's
    /// flag, the socketpair's other end as fd 3, stdin closed and stdout/stderr appended to the log; starts the reader
    /// (handshake, responses, events) and writer tasks on the current `LocalSet`.
    pub fn spawn(options: SpawnOptions) -> Result<(WorkerChannel, ChannelEvents)> {
        let _ = options;
        todo!("D1")
    }

    /// Whether `self` and `other` are the same channel (TS `===` on the worker object).
    pub fn same(&self, other: &WorkerChannel) -> bool {
        let _ = other;
        todo!("D1")
    }

    /// TS: channelKind.
    pub fn kind(&self) -> WorkerKind {
        todo!("D1")
    }

    /// TS: `child.pid`.
    pub fn pid(&self) -> Option<u32> {
        todo!("D1")
    }

    /// TS: `child.connected` (the socket is open and the handshake did not fail).
    pub fn is_connected(&self) -> bool {
        todo!("D1")
    }

    /// TS: WorkerChannel `exited`.
    pub fn is_exited(&self) -> bool {
        todo!("D1")
    }

    /// TS: `worker.exited = true`.
    pub fn mark_exited(&self) {
        todo!("D1")
    }

    /// TS: `child.exitCode !== null || child.signalCode !== null`.
    pub fn has_process_exited(&self) -> bool {
        todo!("D1")
    }

    /// TS: `child.once('exit', ...)`: resolves once the process exited (at once when it already has).
    pub async fn wait_process_exit(&self) {
        todo!("D1")
    }

    /// TS: `child.kill(signal)` (errors ignored, as the TS wraps it in try/catch).
    pub fn kill(&self, signal: Signal) {
        let _ = signal;
        todo!("D1")
    }

    /// TS: daemon.ts requestChannel: sends `{...payload, requestId}` and resolves with the response's `data` (typed as
    /// `T`), or fails with its `error`, `worker request timed out` after [`WORKER_REQUEST_TIMEOUT`], or the reason
    /// the channel closed. A channel already exited or disconnected fails at once with `worker is not running`.
    pub async fn request<T: DeserializeOwned>(&self, payload: Map<String, Value>) -> Result<T> {
        let _ = payload;
        todo!("D1")
    }

    /// TS: daemon.ts sendChannelEvent: sends `payload` (no response expected) unless the worker exited or is
    /// disconnected.
    pub fn send_event(&self, payload: Map<String, Value>) {
        let _ = payload;
        todo!("D1")
    }

    /// TS: `worker.process.send?.(message)` when `connected` (the direct `kill`/`shutdown` sends of cleanup and
    /// stopWorker, which ignore the `exited` flag). Wire-logged as `d2w`.
    pub fn send_message(&self, message: &Map<String, Value>) -> Result<()> {
        let _ = message;
        todo!("D1")
    }

    /// TS: daemon.ts settleResponse: settles the pending request the response names (unknown IDs are ignored).
    pub fn settle_response(&self, message: WorkerResponse) {
        let _ = message;
        todo!("D1")
    }

    /// TS: daemon.ts rejectPending: fails every pending request with `reason`.
    pub fn reject_pending(&self, reason: &str) {
        let _ = reason;
        todo!("D1")
    }

    /// Closes the socket (the worker sees EOF, as a Node child sees its IPC channel close).
    pub fn disconnect(&self) {
        todo!("D1")
    }
}

/// The handshake: the first message must be `hello` naming [`deckhand_protocol::PROTOCOL_VERSION`]. Error:
/// `worker speaks protocol <v>, expected <PROTOCOL_VERSION>` (or `worker did not say hello` for anything else).
pub fn check_hello(message: &WorkerMessage) -> Result<()> {
    let _ = message;
    todo!("D1")
}

/// A connected `AF_UNIX` stream socketpair, both ends close-on-exec (the worker's end is dup2'd onto fd 3 in the
/// child, which clears the flag there).
pub fn channel_socketpair() -> Result<(OwnedFd, OwnedFd)> {
    todo!("D1")
}
