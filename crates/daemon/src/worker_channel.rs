//! Spawning workers, and the daemon side of the fd-3 NDJSON channel.
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
use std::fs::OpenOptions;
use std::future::Future;
use std::io::Write as _;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::rc::{Rc, Weak};
use std::time::Duration;

use deckhand_core::error::{DhError, Result};
use deckhand_protocol::PROTOCOL_VERSION;
use deckhand_protocol::ndjson::{self, Frame, LineDecoder};
use deckhand_protocol::worker_ipc::{WorkerMessage, WorkerResponse};
use deckhand_sys::async_util::{Timer, spawn};
use nix::sys::signal::Signal;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use serde_with::skip_serializing_none;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{mpsc, oneshot, watch};

/// TS: daemon.ts WORKER_REQUEST_TIMEOUT_MS (a request unanswered this long fails with `worker request timed out`).
pub const WORKER_REQUEST_TIMEOUT: Duration = Duration::from_millis(10_000);

/// TS: daemon.ts WORKER_KILL_GRACE_MS (stopWorker: SIGKILL a worker still alive this long after `kill`).
pub const WORKER_KILL_GRACE: Duration = Duration::from_millis(2000);

/// The fd the worker finds its end of the channel on (Node's IPC channel was fd 3 too: `stdio[3] = 'ipc'`).
pub const WORKER_CHANNEL_FD: i32 = 3;

/// TS: daemon.ts wireLog's `DECKHAND_WIRE_LOG` (test/porting aid: a directory every message is appended to).
pub const WIRE_LOG_ENV: &str = "DECKHAND_WIRE_LOG";

/// How long the exit event waits, once the process exited, for the messages it wrote before (the socket's EOF). Only
/// a grandchild that kept fd 3 open makes it wait the whole time.
const EXIT_DRAIN_GRACE: Duration = Duration::from_millis(1000);

/// One socket read at most.
const READ_CHUNK: usize = 64 * 1024;

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
        match self {
            WorkerKind::Session => "--session-worker",
            WorkerKind::Workspace => "--workspace-worker",
        }
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
    let Some(log_dir) = std::env::var_os(WIRE_LOG_ENV).filter(|value| !value.is_empty()) else {
        return;
    };
    wire_log_to(Path::new(&log_dir), dir, msg, worker);
}

/// [`wire_log`] into `log_dir` (synchronously, as `appendFileSync`).
fn wire_log_to<T: Serialize + ?Sized>(log_dir: &Path, dir: WireDir, msg: &T, worker: Option<WorkerKind>) {
    let Ok(mut line) = serde_json::to_string(&WireLogLine { dir, worker, msg }) else {
        return;
    };
    line.push('\n');
    let path = log_dir.join(format!("daemon-{}.jsonl", std::process::id()));
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
}

/// TS: daemon.ts channelKind.
pub fn channel_kind(worker: &WorkerChannel) -> WorkerKind {
    worker.kind()
}

/// TS: paths.ts getCliEntryPath as the daemon uses it: the binary to run as a worker (`configured`, else the running
/// executable, see [`running_exe`]).
pub fn worker_exe(configured: Option<&Path>) -> Result<PathBuf> {
    match configured {
        Some(path) => Ok(path.to_path_buf()),
        None => {
            let self_exe = cfg!(target_os = "linux").then_some(Path::new("/proc/self/exe"));
            Ok(running_exe(deckhand_sys::paths::cli_entry_path()?, self_exe))
        }
    }
}

/// What Linux appends to readlink(/proc/self/exe) once the running binary's file is unlinked or renamed over.
const DELETED_SUFFIX: &str = " (deleted)";

/// A spawnable path for the running executable `path` (as `current_exe` reported it). TS forks `dist/cli.js`, a plain
/// path that stays valid across an upgrade; on Linux `current_exe` instead reads `/proc/self/exe`, which becomes
/// `<path> (deleted)` once the long-lived daemon's binary is replaced (package upgrade, `install`, a rebuild), and
/// spawning that fails with ENOENT. So a missing `<path> (deleted)` is `<path>` again (the upgraded binary, as TS
/// would run), or, when nothing is there any more, `self_exe` (`/proc/self/exe`, which stays executable after
/// unlink). Without `self_exe` (no procfs), or when `path` exists, it is returned unchanged.
fn running_exe(path: PathBuf, self_exe: Option<&Path>) -> PathBuf {
    if path.exists() {
        return path;
    }
    use std::os::unix::ffi::OsStrExt as _;
    let Some(stripped) = path.as_os_str().as_bytes().strip_suffix(DELETED_SUFFIX.as_bytes()) else {
        return path;
    };
    let stripped = PathBuf::from(std::ffi::OsStr::from_bytes(stripped));
    if stripped.is_file() {
        return stripped;
    }
    match self_exe {
        Some(self_exe) => self_exe.to_path_buf(),
        None => path,
    }
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

/// Moves `fd` above the standard streams (the child's stdio is set up before fd 3 is, so a channel end on 0-2 would
/// be overwritten first). Close-on-exec, like the original.
fn above_stdio(fd: OwnedFd) -> Result<OwnedFd> {
    if fd.as_raw_fd() > 2 {
        return Ok(fd);
    }
    // SAFETY: F_DUPFD_CLOEXEC on a descriptor this process owns; it only allocates a new descriptor.
    let duplicate = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, WORKER_CHANNEL_FD) };
    if duplicate < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: `duplicate` is a fresh descriptor nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(duplicate) })
}

/// Appends a worker's output stream to its log (TS `fs.appendFile(logPath, chunk).catch(() => {})` per chunk).
async fn append_output(mut stream: impl AsyncRead + Unpin, log_path: PathBuf) {
    let mut chunk = vec![0u8; READ_CHUNK];
    loop {
        let read = match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        let Some(bytes) = chunk.get(..read) else {
            return;
        };
        if let Ok(mut file) = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .await
        {
            let _ = file.write_all(bytes).await;
        }
    }
}

/// Writes queued lines in order until the queue closes (disconnect) or a write fails.
async fn write_lines(mut half: OwnedWriteHalf, mut queue: mpsc::UnboundedReceiver<String>, inner: Weak<ChannelInner>) {
    while let Some(line) = queue.recv().await {
        if half.write_all(line.as_bytes()).await.is_err() {
            if let Some(inner) = inner.upgrade() {
                inner.writer.borrow_mut().take();
            }
            return;
        }
    }
    let _ = half.shutdown().await;
}

/// Reads the worker's messages until EOF: the handshake first, then responses (settled here) and events (handed on).
async fn read_messages(
    mut half: OwnedReadHalf,
    inner: Weak<ChannelInner>,
    events: mpsc::UnboundedSender<ChannelEvent>,
) {
    let mut decoder = LineDecoder::new();
    let mut chunk = vec![0u8; READ_CHUNK];
    let mut greeted = false;
    loop {
        let read = match half.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        let Some(bytes) = chunk.get(..read) else {
            break;
        };
        for frame in decoder.push(bytes) {
            let Frame::Line(line) = frame else {
                continue;
            };
            let Some(inner) = inner.upgrade() else {
                return;
            };
            let channel = WorkerChannel { inner };
            // A line that is not a worker message is skipped (Node's IPC channel never delivers one).
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let Ok(message) = serde_json::from_value::<WorkerMessage>(value.clone()) else {
                continue;
            };
            if !greeted {
                greeted = true;
                if let Err(error) = check_hello(&message) {
                    channel.disconnect();
                    channel.reject_pending(error.message());
                    return;
                }
                continue;
            }
            wire_log(WireDir::W2d, &value, Some(channel.kind()));
            match message {
                WorkerMessage::Hello(_) => {}
                WorkerMessage::Response(response) => channel.settle_response(response),
                other => {
                    let _ = events.send(ChannelEvent::Message(Box::new(other)));
                }
            }
        }
    }
    // TS 'disconnect': nothing more can be sent.
    if let Some(inner) = inner.upgrade() {
        inner.writer.borrow_mut().take();
    }
}

impl WorkerChannel {
    /// TS: `fork(...)` plus the listeners daemon.ts wires before its first await: spawns `options.exe` with the kind's
    /// flag, the socketpair's other end as fd 3, stdin closed and stdout/stderr appended to the log; starts the reader
    /// (handshake, responses, events) and writer tasks on the current `LocalSet`.
    pub fn spawn(options: SpawnOptions) -> Result<(WorkerChannel, ChannelEvents)> {
        let (daemon_end, worker_end) = channel_socketpair()?;
        let worker_end = above_stdio(worker_end)?;
        let worker_fd: RawFd = worker_end.as_raw_fd();
        let mut command = tokio::process::Command::new(&options.exe);
        command
            .arg(options.kind.flag())
            .envs(options.env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // SAFETY: the closure runs in the forked child before exec and only calls async-signal-safe functions (dup2,
        // fcntl) on descriptors that exist in the child.
        unsafe {
            command.pre_exec(move || {
                if worker_fd == WORKER_CHANNEL_FD {
                    // Already fd 3: dup2 would keep close-on-exec, so clear it.
                    let flags = libc::fcntl(worker_fd, libc::F_GETFD);
                    if flags < 0 || libc::fcntl(worker_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                } else if libc::dup2(worker_fd, WORKER_CHANNEL_FD) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().map_err(DhError::from)?;
        // The child has its copy; the daemon's must close so the worker's exit reads as EOF here.
        drop(worker_end);
        let pid = child.id();
        if let Some(stdout) = child.stdout.take() {
            drop(spawn(append_output(stdout, options.log_path.clone())));
        }
        if let Some(stderr) = child.stderr.take() {
            drop(spawn(append_output(stderr, options.log_path.clone())));
        }
        let stream = std::os::unix::net::UnixStream::from(daemon_end);
        stream.set_nonblocking(true)?;
        let stream = UnixStream::from_std(stream)?;
        let exit = async move {
            match child.wait().await {
                Ok(status) => ProcessExit {
                    code: status.code(),
                    signal: status.signal(),
                },
                Err(_) => ProcessExit {
                    code: None,
                    signal: None,
                },
            }
        };
        Ok(WorkerChannel::attach(options.kind, pid, stream, exit))
    }

    /// A channel over a connected `stream` to a worker whose exit `exit` resolves with (the part of [`spawn`] after
    /// the fork; tests drive it without a process).
    ///
    /// [`spawn`]: WorkerChannel::spawn
    pub(crate) fn attach(
        kind: WorkerKind,
        pid: Option<u32>,
        stream: UnixStream,
        exit: impl Future<Output = ProcessExit> + 'static,
    ) -> (WorkerChannel, ChannelEvents) {
        let (read_half, write_half) = stream.into_split();
        let (line_tx, line_rx) = mpsc::unbounded_channel();
        let (exit_tx, exit_rx) = watch::channel(None);
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let inner = Rc::new(ChannelInner {
            kind,
            pid,
            writer: RefCell::new(Some(line_tx)),
            pending: RefCell::new(HashMap::new()),
            exited: Cell::new(false),
            process_exit: exit_rx,
        });
        drop(spawn(write_lines(write_half, line_rx, Rc::downgrade(&inner))));
        let mut reader = spawn(read_messages(read_half, Rc::downgrade(&inner), events_tx.clone()));
        drop(spawn(async move {
            let exit = exit.await;
            let _ = exit_tx.send(Some(exit));
            // The messages the worker wrote before exiting come first; then the exit, last.
            if tokio::time::timeout(EXIT_DRAIN_GRACE, &mut reader).await.is_err() {
                reader.abort();
            }
            let _ = events_tx.send(ChannelEvent::Exit(exit));
        }));
        (WorkerChannel { inner }, events_rx)
    }

    /// Whether `self` and `other` are the same channel (TS `===` on the worker object).
    pub fn same(&self, other: &WorkerChannel) -> bool {
        Rc::ptr_eq(&self.inner, &other.inner)
    }

    /// TS: channelKind.
    pub fn kind(&self) -> WorkerKind {
        self.inner.kind
    }

    /// TS: `child.pid`.
    pub fn pid(&self) -> Option<u32> {
        self.inner.pid
    }

    /// TS: `child.connected` (the socket is open and the handshake did not fail).
    pub fn is_connected(&self) -> bool {
        self.inner.writer.borrow().is_some()
    }

    /// TS: WorkerChannel `exited`.
    pub fn is_exited(&self) -> bool {
        self.inner.exited.get()
    }

    /// TS: `worker.exited = true`.
    pub fn mark_exited(&self) {
        self.inner.exited.set(true);
    }

    /// TS: `child.exitCode !== null || child.signalCode !== null`.
    pub fn has_process_exited(&self) -> bool {
        self.inner.process_exit.borrow().is_some()
    }

    /// TS: `child.once('exit', ...)`: resolves once the process exited (at once when it already has).
    pub async fn wait_process_exit(&self) {
        let mut exit = self.inner.process_exit.clone();
        // An error means the exit watcher is gone without an exit: never resolves, like a JS listener never called.
        if exit.wait_for(Option::is_some).await.is_err() {
            std::future::pending::<()>().await;
        }
    }

    /// TS: `child.kill(signal)` (errors ignored, as the TS wraps it in try/catch).
    pub fn kill(&self, signal: Signal) {
        // An exited (and reaped) process's pid may already belong to another process.
        if self.has_process_exited() {
            return;
        }
        if let Some(pid) = self.inner.pid {
            let _ = deckhand_sys::proc::kill_pid(pid, signal);
        }
    }

    /// TS: daemon.ts requestChannel: sends `{...payload, requestId}` and resolves with the response's `data` (typed as
    /// `T`), or fails with its `error`, `worker request timed out` after [`WORKER_REQUEST_TIMEOUT`], or the reason
    /// the channel closed. A channel already exited or disconnected fails at once with `worker is not running`.
    pub async fn request<T: DeserializeOwned>(&self, payload: Map<String, Value>) -> Result<T> {
        self.request_within(payload, WORKER_REQUEST_TIMEOUT).await
    }

    /// [`WorkerChannel::request`] with its timeout.
    pub(crate) async fn request_within<T: DeserializeOwned>(
        &self,
        payload: Map<String, Value>,
        timeout: Duration,
    ) -> Result<T> {
        let Some(writer) = self.inner.writer.borrow().clone().filter(|_| !self.is_exited()) else {
            return Err(DhError::msg("worker is not running"));
        };
        let request_id = uuid::Uuid::new_v4().to_string();
        let (reply, settled) = oneshot::channel();
        let weak = Rc::downgrade(&self.inner);
        let timer_id = request_id.clone();
        let timer = Timer::after(timeout, move || {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let pending = inner.pending.borrow_mut().remove(&timer_id);
            if let Some(PendingRequest { reply, timer }) = pending {
                // This very timer is running: let it finish instead of aborting it.
                timer.detach();
                let _ = reply.send(Err(DhError::msg("worker request timed out")));
            }
        });
        self.inner
            .pending
            .borrow_mut()
            .insert(request_id.clone(), PendingRequest { reply, timer });
        let mut message = payload;
        message.insert("requestId".to_owned(), Value::String(request_id.clone()));
        wire_log(WireDir::D2w, &message, Some(self.kind()));
        let sent = ndjson::encode_line(&message)
            .map_err(DhError::from)
            .and_then(|line| writer.send(line).map_err(|_| DhError::msg("Channel closed")));
        if let Err(error) = sent {
            let pending = self.inner.pending.borrow_mut().remove(&request_id);
            drop(pending);
            return Err(error);
        }
        let value = match settled.await {
            Ok(result) => result?,
            // Dropped unsettled (the channel is gone).
            Err(_) => return Err(DhError::msg("worker is not running")),
        };
        serde_json::from_value(value).map_err(DhError::from)
    }

    /// TS: daemon.ts sendChannelEvent: sends `payload` (no response expected) unless the worker exited or is
    /// disconnected.
    pub fn send_event(&self, payload: Map<String, Value>) {
        if self.is_exited() || !self.is_connected() {
            return;
        }
        let _ = self.send_message(&payload);
    }

    /// TS: `worker.process.send?.(message)` when `connected` (the direct `kill`/`shutdown` sends of cleanup and
    /// stopWorker, which ignore the `exited` flag). Wire-logged as `d2w`.
    pub fn send_message(&self, message: &Map<String, Value>) -> Result<()> {
        let Some(writer) = self.inner.writer.borrow().clone() else {
            return Ok(());
        };
        wire_log(WireDir::D2w, message, Some(self.kind()));
        let line = ndjson::encode_line(message)?;
        writer.send(line).map_err(|_| DhError::msg("Channel closed"))
    }

    /// TS: daemon.ts settleResponse: settles the pending request the response names (unknown IDs are ignored).
    pub fn settle_response(&self, message: WorkerResponse) {
        let pending = self.inner.pending.borrow_mut().remove(&message.request_id);
        let Some(PendingRequest { reply, mut timer }) = pending else {
            return;
        };
        timer.clear();
        let result = if message.ok {
            Ok(message.data.unwrap_or(Value::Null))
        } else {
            // `new Error(undefined).message` is ''.
            Err(DhError::msg(message.error.unwrap_or_default()))
        };
        let _ = reply.send(result);
    }

    /// TS: daemon.ts rejectPending: fails every pending request with `reason`.
    pub fn reject_pending(&self, reason: &str) {
        let pending: Vec<PendingRequest> = self
            .inner
            .pending
            .borrow_mut()
            .drain()
            .map(|(_, entry)| entry)
            .collect();
        for PendingRequest { reply, mut timer } in pending {
            timer.clear();
            let _ = reply.send(Err(DhError::msg(reason)));
        }
    }

    /// Closes the socket (the worker sees EOF, as a Node child sees its IPC channel close).
    pub fn disconnect(&self) {
        self.inner.writer.borrow_mut().take();
    }
}

/// The handshake: the first message must be `hello` naming [`deckhand_protocol::PROTOCOL_VERSION`]. Error:
/// `worker speaks protocol <v>, expected <PROTOCOL_VERSION>` (or `worker did not say hello` for anything else).
pub fn check_hello(message: &WorkerMessage) -> Result<()> {
    match message {
        WorkerMessage::Hello(hello) if hello.version == PROTOCOL_VERSION => Ok(()),
        WorkerMessage::Hello(hello) => Err(DhError::msg(format!(
            "worker speaks protocol {}, expected {PROTOCOL_VERSION}",
            hello.version
        ))),
        _ => Err(DhError::msg("worker did not say hello")),
    }
}

/// Marks `fd` close-on-exec.
fn set_cloexec(fd: RawFd) -> std::io::Result<()> {
    // SAFETY: fcntl on a descriptor this process owns.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    // SAFETY: as above.
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// A connected `AF_UNIX` stream socketpair, both ends close-on-exec (the worker's end is dup2'd onto fd 3 in the
/// child, which clears the flag there).
pub fn channel_socketpair() -> Result<(OwnedFd, OwnedFd)> {
    use nix::sys::socket::{AddressFamily, SockFlag, SockType, socketpair};
    let (left, right) = socketpair(AddressFamily::Unix, SockType::Stream, None, SockFlag::empty())
        .map_err(|errno| DhError::from(std::io::Error::from(errno)))?;
    set_cloexec(left.as_raw_fd())?;
    set_cloexec(right.as_raw_fd())?;
    Ok((left, right))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::hello_line;
    use deckhand_protocol::worker_ipc::Hello;
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::{AsyncBufReadExt, BufReader};

    fn local<F: Future>(future: F) -> F::Output {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        tokio::task::LocalSet::new().block_on(&runtime, future)
    }

    fn payload(value: Value) -> Map<String, Value> {
        match value {
            Value::Object(map) => map,
            _ => panic!("not an object"),
        }
    }

    /// A worker in sh: says hello, answers `echo` with `{ok: true}`, `fail` with an error, `event` with a `running`
    /// event, exits 3 on `exit` and ignores anything else.
    const SH_WORKER: &str = r#"#!/bin/sh
if [ -n "$HELLO" ]; then printf '%s\n' "$HELLO" >&3; else printf '@HELLO@\n' >&3; fi
echo "started $1"
while IFS= read -r line <&3; do
  id=$(printf '%s' "$line" | sed -n 's/.*"requestId":"\([^"]*\)".*/\1/p')
  case "$line" in
    *'"type":"echo"'*) printf '{"type":"response","requestId":"%s","ok":true,"data":{"ok":true}}\n' "$id" >&3 ;;
    *'"type":"fail"'*) printf '{"type":"response","requestId":"%s","ok":false,"error":"boom"}\n' "$id" >&3 ;;
    *'"type":"event"'*) printf '{"type":"running","pid":7}\n' >&3 ;;
    *'"type":"exit"'*) printf '{"type":"running","pid":8}\n' >&3; exit 3 ;;
  esac
done
"#;

    fn sh_worker(dir: &Path, env: Vec<(String, String)>) -> SpawnOptions {
        let exe = dir.join("worker.sh");
        std::fs::write(&exe, SH_WORKER.replace("@HELLO@", &hello_line())).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        SpawnOptions {
            kind: WorkerKind::Session,
            exe,
            env,
            log_path: dir.join("worker.log"),
        }
    }

    #[test]
    fn flags_and_kinds() {
        assert_eq!(WorkerKind::Session.flag(), "--session-worker");
        assert_eq!(WorkerKind::Workspace.flag(), "--workspace-worker");
        assert_eq!(serde_json::to_string(&WorkerKind::Workspace).unwrap(), r#""workspace""#);
        assert_eq!(WORKER_REQUEST_TIMEOUT, Duration::from_millis(10_000));
        assert_eq!(WORKER_KILL_GRACE, Duration::from_millis(2000));
        assert_eq!(
            worker_exe(Some(Path::new("/x/deckhand"))).unwrap(),
            PathBuf::from("/x/deckhand")
        );
    }

    #[test]
    fn a_replaced_running_binary_is_spawned_from_its_path_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deckhand");
        std::fs::write(&path, "").unwrap();
        // readlink(/proc/self/exe) once the running binary was renamed over (an upgrade or a rebuild).
        let deleted = dir.path().join("deckhand (deleted)");
        let self_exe = Path::new("/proc/self/exe");
        assert_eq!(running_exe(deleted.clone(), Some(self_exe)), path);
        // Still in place: as is.
        assert_eq!(running_exe(path.clone(), Some(self_exe)), path);
        // Unlinked with nothing at its path: the running image itself, which stays executable.
        std::fs::remove_file(&path).unwrap();
        assert_eq!(running_exe(deleted.clone(), Some(self_exe)), self_exe);
        // Without /proc (macOS keeps the original path anyway): unchanged.
        assert_eq!(running_exe(deleted.clone(), None), deleted);
        // The running test binary resolves to itself.
        let exe = worker_exe(None).unwrap();
        assert!(exe.is_file(), "{}", exe.display());
    }

    #[test]
    fn hello_must_name_this_protocol() {
        assert!(check_hello(&WorkerMessage::Hello(Hello::default())).is_ok());
        assert_eq!(
            check_hello(&WorkerMessage::Hello(Hello { version: 45 }))
                .unwrap_err()
                .to_string(),
            format!("worker speaks protocol 45, expected {PROTOCOL_VERSION}")
        );
        let running: WorkerMessage = serde_json::from_value(json!({"type": "running", "pid": 1})).unwrap();
        assert_eq!(
            check_hello(&running).unwrap_err().to_string(),
            "worker did not say hello"
        );
    }

    #[test]
    fn wire_log_lines_match_the_ts_shape() {
        let dir = tempfile::tempdir().unwrap();
        wire_log_to(
            dir.path(),
            WireDir::C2d,
            &json!({"type": "ping", "requestId": "r"}),
            None,
        );
        wire_log_to(
            dir.path(),
            WireDir::D2w,
            &json!({"type": "idle"}),
            Some(WorkerKind::Workspace),
        );
        let text = std::fs::read_to_string(dir.path().join(format!("daemon-{}.jsonl", std::process::id()))).unwrap();
        assert_eq!(
            text,
            "{\"dir\":\"c2d\",\"msg\":{\"type\":\"ping\",\"requestId\":\"r\"}}\n\
             {\"dir\":\"d2w\",\"worker\":\"workspace\",\"msg\":{\"type\":\"idle\"}}\n"
        );
    }

    #[test]
    fn socketpair_ends_are_close_on_exec() {
        let (left, right) = channel_socketpair().unwrap();
        for fd in [left.as_raw_fd(), right.as_raw_fd()] {
            // SAFETY: F_GETFD on an open descriptor.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
            assert_ne!(flags & libc::FD_CLOEXEC, 0);
        }
    }

    #[test]
    fn requests_settle_by_id_and_events_end_with_the_exit() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let options = sh_worker(dir.path(), vec![("DECKHAND_SESSION_ID".into(), "s1".into())]);
            let (channel, mut events) = WorkerChannel::spawn(options).unwrap();
            assert!(channel.pid().is_some());
            assert_eq!(channel_kind(&channel), WorkerKind::Session);
            assert!(channel.same(&channel.clone()));
            assert!(channel.is_connected());

            let answer: Value = channel.request(payload(json!({"type": "echo"}))).await.unwrap();
            assert_eq!(answer, json!({"ok": true}));
            let error = channel
                .request::<Value>(payload(json!({"type": "fail"})))
                .await
                .unwrap_err();
            assert_eq!(error.to_string(), "boom");
            assert!(channel.inner.pending.borrow().is_empty());

            channel.send_event(payload(json!({"type": "event"})));
            let event = events.recv().await.unwrap();
            assert_eq!(
                event,
                ChannelEvent::Message(Box::new(
                    serde_json::from_value(json!({"type": "running", "pid": 7})).unwrap()
                ))
            );

            channel.send_event(payload(json!({"type": "exit"})));
            let last_message = events.recv().await.unwrap();
            assert!(matches!(last_message, ChannelEvent::Message(_)));
            let exit = events.recv().await.unwrap();
            assert_eq!(
                exit,
                ChannelEvent::Exit(ProcessExit {
                    code: Some(3),
                    signal: None
                })
            );
            assert!(events.recv().await.is_none());
            assert!(channel.has_process_exited());
            channel.wait_process_exit().await;
            assert!(!channel.is_connected());
            let error = channel
                .request::<Value>(payload(json!({"type": "echo"})))
                .await
                .unwrap_err();
            assert_eq!(error.to_string(), "worker is not running");
            let log = std::fs::read_to_string(dir.path().join("worker.log")).unwrap();
            assert_eq!(log, "started --session-worker\n");
        });
    }

    #[test]
    fn a_worker_of_another_protocol_is_refused() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let options = sh_worker(
                dir.path(),
                vec![("HELLO".into(), r#"{"type":"hello","version":1}"#.into())],
            );
            let (channel, mut events) = WorkerChannel::spawn(options).unwrap();
            let error = channel
                .request::<Value>(payload(json!({"type": "ignored"})))
                .await
                .unwrap_err();
            let message = error.to_string();
            // The refusal itself, or (when the handshake was read before the request was sent) the closed channel.
            assert!(
                message == format!("worker speaks protocol 1, expected {PROTOCOL_VERSION}")
                    || message == "worker is not running",
                "{message}"
            );
            assert!(!channel.is_connected());
            // The worker sees EOF and its loop ends.
            assert_eq!(
                events.recv().await.unwrap(),
                ChannelEvent::Exit(ProcessExit {
                    code: Some(0),
                    signal: None
                })
            );
        });
    }

    #[test]
    fn exited_channels_and_kills() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let (channel, mut events) = WorkerChannel::spawn(sh_worker(dir.path(), Vec::new())).unwrap();
            channel.mark_exited();
            assert!(channel.is_exited());
            let error = channel
                .request::<Value>(payload(json!({"type": "echo"})))
                .await
                .unwrap_err();
            assert_eq!(error.to_string(), "worker is not running");
            // An exited channel takes no events, but a direct send still goes out.
            channel.send_event(payload(json!({"type": "exit"})));
            channel.kill(Signal::SIGKILL);
            assert_eq!(
                events.recv().await.unwrap(),
                ChannelEvent::Exit(ProcessExit {
                    code: None,
                    signal: Some(9)
                })
            );
            channel.kill(Signal::SIGKILL);
        });
    }

    #[test]
    fn unanswered_requests_time_out_and_pending_ones_can_be_rejected() {
        local(async {
            let (ours, theirs) = UnixStream::pair().unwrap();
            let (exit_tx, exit_rx) = oneshot::channel::<()>();
            let (channel, _events) = WorkerChannel::attach(WorkerKind::Workspace, None, ours, async move {
                let _ = exit_rx.await;
                ProcessExit {
                    code: Some(0),
                    signal: None,
                }
            });
            let mut theirs = BufReader::new(theirs);
            theirs
                .get_mut()
                .write_all(format!("{}\n", hello_line()).as_bytes())
                .await
                .unwrap();
            let error = channel
                .request_within::<Value>(payload(json!({"type": "idle"})), Duration::from_millis(30))
                .await
                .unwrap_err();
            assert_eq!(error.to_string(), "worker request timed out");
            assert!(channel.inner.pending.borrow().is_empty());
            let mut line = String::new();
            theirs.read_line(&mut line).await.unwrap();
            let sent: Value = serde_json::from_str(&line).unwrap();
            let keys: Vec<&String> = sent.as_object().unwrap().keys().collect();
            assert_eq!(keys, ["type", "requestId"]);

            let request = channel.request::<Value>(payload(json!({"type": "snapshot", "target": "git"})));
            let rejecter = channel.clone();
            let (result, ()) = tokio::join!(request, async move {
                tokio::task::yield_now().await;
                rejecter.reject_pending("workspace worker stopped");
            });
            assert_eq!(result.unwrap_err().to_string(), "workspace worker stopped");

            // A response to an unknown request is ignored; a known one settles with its data (absent: null).
            channel.settle_response(WorkerResponse {
                request_id: "nope".into(),
                ok: true,
                data: None,
                error: None,
            });
            let request = channel.request::<Option<Value>>(payload(json!({"type": "shutdown"})));
            let (result, ()) = tokio::join!(request, async {
                let mut line = String::new();
                // The snapshot request first, then the shutdown.
                theirs.read_line(&mut line).await.unwrap();
                line.clear();
                theirs.read_line(&mut line).await.unwrap();
                let sent: Value = serde_json::from_str(&line).unwrap();
                let id = sent["requestId"].as_str().unwrap();
                let answer = format!("{{\"type\":\"response\",\"requestId\":\"{id}\",\"ok\":true}}\n");
                theirs.get_mut().write_all(answer.as_bytes()).await.unwrap();
            });
            assert_eq!(result.unwrap(), None);
            let _ = exit_tx.send(());
            channel.wait_process_exit().await;
            assert!(channel.has_process_exited());
        });
    }
}
