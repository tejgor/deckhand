//! Port of worker side of the fd-3 NDJSON socketpair (replaces process.send/on('message')).
//!
//! The TS daemon forked each worker with Node's IPC channel and the worker used `process.send`,
//! `process.on('message')` and `process.on('disconnect')`. The Rust daemon instead hands the worker one end of a
//! `socketpair` as fd [`CHANNEL_FD`] and both sides exchange NDJSON lines ([`deckhand_protocol::ndjson`]): commands
//! from [`deckhand_protocol::worker_ipc`] one way, [`WorkerMessage`]s the other. The first line a worker writes is
//! [`WorkerMessage::Hello`] with [`deckhand_protocol::PROTOCOL_VERSION`] so the daemon can refuse a mismatched binary.
//!
//! `post` stays synchronous, as `process.send` is: it queues the line for a writer task, so it can be called from
//! timer callbacks and PTY handlers without awaiting (and never while a `RefCell` borrow is held across an await).
//! EOF or a read/write error on the socket is the daemon's `disconnect`: posting becomes a no-op
//! (`process.connected` is false) and the reader yields `None`.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::os::fd::{FromRawFd, RawFd};
use std::path::Path;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{mpsc, oneshot};

use deckhand_core::error::DhError;
use deckhand_protocol::wire::{js_string, js_truthy};
use deckhand_protocol::{WorkerCommand, WorkerMessage};

use crate::session::{DEFAULT_COLS, DEFAULT_ROWS};
use deckhand_protocol::ndjson::{self, Frame, LineDecoder};
use deckhand_protocol::worker_ipc::{Hello, WorkerResponse};

/// The descriptor the daemon passes the worker's end of the socketpair on (Node's `stdio[3] = 'ipc'`).
pub const CHANNEL_FD: RawFd = 3;

/// TS: sessionWorker.ts wireLog env var. `DECKHAND_WIRE_LOG=<dir>` (test/porting aid, off by default).
pub const WIRE_LOG_ENV: &str = "DECKHAND_WIRE_LOG";

/// How much a single socket read takes at most.
const READ_CHUNK: usize = 64 * 1024;

/// Set once fd 3 has been taken over, so a second [`WorkerChannel::open`] cannot own the descriptor twice.
static CHANNEL_FD_TAKEN: AtomicBool = AtomicBool::new(false);

/// TS: sessionWorker.ts `wireRole` (`'worker' | 'workspace-worker'`): names the wire-log file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum WireRole {
    #[serde(rename = "worker")]
    Worker,
    #[serde(rename = "workspace-worker")]
    WorkspaceWorker,
}

impl WireRole {
    /// The role as written in the log file name.
    pub fn as_str(self) -> &'static str {
        match self {
            WireRole::Worker => "worker",
            WireRole::WorkspaceWorker => "workspace-worker",
        }
    }
}

/// Which way a wire-logged message went (`{dir, msg}` in the wire log).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum WireDir {
    /// Daemon to worker.
    #[serde(rename = "d2w")]
    D2w,
    /// Worker to daemon.
    #[serde(rename = "w2d")]
    W2d,
}

/// One wire-log line, `{dir, msg}` in TS key order.
#[derive(Serialize)]
struct WireLine<'a, T: Serialize + ?Sized> {
    dir: WireDir,
    msg: &'a T,
}

/// TS: sessionWorker.ts wireLog. When `DECKHAND_WIRE_LOG` is set, appends `{"dir":..,"msg":..}` plus `\n` to
/// `<dir>/<role>-<pid>.jsonl` (synchronously, as `appendFileSync`); every failure is ignored.
pub fn wire_log<T: Serialize + ?Sized>(role: WireRole, dir: WireDir, msg: &T) {
    let Some(log_dir) = std::env::var_os(WIRE_LOG_ENV).filter(|value| !value.is_empty()) else {
        return;
    };
    wire_log_to(Path::new(&log_dir), role, dir, msg);
}

/// [`wire_log`] into `log_dir`.
fn wire_log_to<T: Serialize + ?Sized>(log_dir: &Path, role: WireRole, dir: WireDir, msg: &T) {
    let Ok(mut line) = serde_json::to_string(&WireLine { dir, msg }) else {
        return;
    };
    line.push('\n');
    let path = log_dir.join(format!("{}-{}.jsonl", role.as_str(), std::process::id()));
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
}

/// What the writer task is asked to do, in order.
enum Outgoing {
    /// Write one encoded line.
    Line(Vec<u8>),
    /// Signal once every line queued before it was written.
    Flush(oneshot::Sender<()>),
}

/// TS: sessionWorker.ts post/ok/fail over `process.send`: the worker's sending side of the channel.
///
/// A cheap handle (clones share one channel). Must be created inside the process's `LocalSet` (it spawns the writer
/// task).
#[derive(Clone)]
pub struct WorkerChannel {
    inner: Rc<ChannelInner>,
}

struct ChannelInner {
    /// The current wire-log role (a workspace worker switches it when it starts).
    role: Cell<WireRole>,
    /// Queue of the writer task; `None` once the daemon disconnected (`process.connected` false).
    writer: RefCell<Option<mpsc::UnboundedSender<Outgoing>>>,
}

impl ChannelInner {
    /// The daemon is gone: later posts are no-ops. Lines already queued are still attempted by the writer.
    fn disconnect(&self) {
        self.writer.borrow_mut().take();
    }
}

/// Writes queued lines in order until the queue closes or a write fails (the disconnect).
async fn write_loop(mut half: OwnedWriteHalf, mut queue: mpsc::UnboundedReceiver<Outgoing>, inner: Weak<ChannelInner>) {
    while let Some(item) = queue.recv().await {
        match item {
            Outgoing::Line(bytes) => {
                if half.write_all(&bytes).await.is_err() {
                    if let Some(inner) = inner.upgrade() {
                        inner.disconnect();
                    }
                    break;
                }
            }
            Outgoing::Flush(done) => {
                let _ = done.send(());
            }
        }
    }
}

/// Whether `fd` is an open socket.
fn is_socket(fd: RawFd) -> bool {
    // SAFETY: fstat only writes into the zeroed struct we pass.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: as above; an invalid descriptor makes fstat fail, nothing else.
    if unsafe { libc::fstat(fd, &mut stat) } != 0 {
        return false;
    }
    stat.st_mode & libc::S_IFMT == libc::S_IFSOCK
}

/// Marks `fd` close-on-exec, so the PTY children a worker spawns never inherit the daemon channel.
fn set_cloexec(fd: RawFd) -> std::io::Result<()> {
    // SAFETY: fcntl on a descriptor this process owns.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    // SAFETY: as above.
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

impl WorkerChannel {
    /// Opens the daemon's socketpair end on [`CHANNEL_FD`] and posts [`WorkerMessage::Hello`] first. Fails when fd 3
    /// is not a socket (the worker was not started by a daemon).
    pub fn open(role: WireRole) -> Result<(WorkerChannel, ChannelReader), DhError> {
        if !is_socket(CHANNEL_FD) {
            return Err(DhError::msg(format!(
                "fd {CHANNEL_FD} is not a socket: deckhand workers are started by the daemon"
            )));
        }
        if CHANNEL_FD_TAKEN.swap(true, Ordering::SeqCst) {
            return Err(DhError::msg(format!(
                "fd {CHANNEL_FD} is already in use as the worker channel"
            )));
        }
        set_cloexec(CHANNEL_FD)?;
        // SAFETY: fd 3 is an open socket (checked above) that nothing else in this process owns (guarded by
        // CHANNEL_FD_TAKEN); ownership moves into the stream.
        let stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(CHANNEL_FD) };
        stream.set_nonblocking(true)?;
        let stream = UnixStream::from_std(stream)?;
        Ok(WorkerChannel::from_stream(stream, role))
    }

    /// [`WorkerChannel::open`] over an already connected stream (tests use one end of a `UnixStream::pair`). Posts
    /// [`WorkerMessage::Hello`] first.
    pub fn from_stream(stream: UnixStream, role: WireRole) -> (WorkerChannel, ChannelReader) {
        let (read_half, write_half) = stream.into_split();
        let (sender, receiver) = mpsc::unbounded_channel();
        let inner = Rc::new(ChannelInner {
            role: Cell::new(role),
            writer: RefCell::new(Some(sender)),
        });
        drop(tokio::task::spawn_local(write_loop(
            write_half,
            receiver,
            Rc::downgrade(&inner),
        )));
        let channel = WorkerChannel { inner };
        channel.post(&WorkerMessage::Hello(Hello::default()));
        let reader = ChannelReader {
            reader: read_half,
            decoder: LineDecoder::new(),
            channel: channel.clone(),
            queued: VecDeque::new(),
        };
        (channel, reader)
    }

    /// The wire-log role.
    pub fn role(&self) -> WireRole {
        self.inner.role.get()
    }

    /// TS: `wireRole = 'workspace-worker'` when a workspace worker starts.
    pub fn set_role(&self, role: WireRole) {
        self.inner.role.set(role);
    }

    /// TS: `process.connected`: false once the daemon end closed or a write failed.
    pub fn is_connected(&self) -> bool {
        self.inner.writer.borrow().is_some()
    }

    /// TS: sessionWorker.ts post. A no-op once disconnected; otherwise wire-logs the message (`w2d`) and queues its
    /// line. Never fails: a broken channel is the disconnect, which the reader reports.
    pub fn post(&self, message: &WorkerMessage) {
        let Some(sender) = self.inner.writer.borrow().clone() else {
            return;
        };
        wire_log(self.role(), WireDir::W2d, message);
        let Ok(line) = ndjson::encode(message) else {
            return;
        };
        if sender.send(Outgoing::Line(line)).is_err() {
            // The writer stopped after a failed write: the daemon is gone.
            self.inner.disconnect();
        }
    }

    /// TS: sessionWorker.ts ok: `{type: 'response', requestId, ok: true, data}`.
    pub fn ok<T: Serialize + ?Sized>(&self, request_id: &str, data: &T) {
        // `data: undefined` is dropped by JSON.stringify; a value that cannot be serialised is sent the same way.
        let data = serde_json::to_value(data).ok();
        self.post(&WorkerMessage::Response(WorkerResponse {
            request_id: request_id.to_owned(),
            ok: true,
            data,
            error: None,
        }));
    }

    /// TS: sessionWorker.ts fail: `{type: 'response', requestId, ok: false, error: error.message}`.
    pub fn fail(&self, request_id: &str, error: &DhError) {
        self.post(&WorkerMessage::Response(WorkerResponse {
            request_id: request_id.to_owned(),
            ok: false,
            data: None,
            error: Some(error.message().to_owned()),
        }));
    }

    /// Resolves once every line queued so far was written (or the channel broke), so a worker can exit without
    /// losing its last messages (Node flushed `process.send` before `process.exit`).
    pub async fn flush(&self) {
        let sender = self.inner.writer.borrow().clone();
        let Some(sender) = sender else {
            return;
        };
        let (done, written) = oneshot::channel();
        if sender.send(Outgoing::Flush(done)).is_err() {
            return;
        }
        let _ = written.await;
    }
}

/// TS: `process.on('message')` / `process.on('disconnect')`: the worker's receiving side of the channel.
pub struct ChannelReader {
    reader: OwnedReadHalf,
    decoder: LineDecoder,
    /// The sending side, for its role (wire log) and to mark the channel disconnected at EOF.
    channel: WorkerChannel,
    queued: VecDeque<Value>,
}

impl ChannelReader {
    /// The next message as JSON, wire-logged as `d2w`; `None` at EOF or on a read error (the `disconnect`). Lines
    /// that are not JSON objects are skipped (Node's IPC channel never delivered such a message).
    pub async fn next_value(&mut self) -> Option<Value> {
        let mut buf = vec![0u8; READ_CHUNK];
        loop {
            if let Some(value) = self.queued.pop_front() {
                return Some(value);
            }
            let read = match self.reader.read(&mut buf).await {
                Ok(0) | Err(_) => {
                    self.channel.inner.disconnect();
                    return None;
                }
                Ok(read) => read,
            };
            for frame in self.decoder.push(buf.get(..read).unwrap_or_default()) {
                let Frame::Line(line) = frame else {
                    continue;
                };
                if let Ok(value @ Value::Object(_)) = serde_json::from_str::<Value>(&line) {
                    wire_log(self.channel.role(), WireDir::D2w, &value);
                    self.queued.push_back(value);
                }
            }
        }
    }

    /// The next message decoded as `C` (a [`deckhand_protocol::WorkerCommand`] or
    /// [`deckhand_protocol::WorkspaceWorkerCommand`]); one that does not decode is skipped, as the TS `switch` ignored
    /// unknown command types. `None` at disconnect.
    pub async fn next<C: DeserializeOwned>(&mut self) -> Option<C> {
        loop {
            let value = self.next_value().await?;
            if let Ok(command) = serde_json::from_value(value) {
                return Some(command);
            }
        }
    }
}

/// What a session worker makes of a message (TS SessionWorker.handle on `message as WorkerCommand`, which reads its
/// fields without checking them).
#[derive(Clone, Debug, PartialEq)]
pub enum SessionIncoming {
    /// A command to run.
    Command(WorkerCommand),
    /// What TS's `catch` answers: `fail(requestId, error)` (only a message with a `requestId` gets one).
    Failure { request_id: String, error: String },
    /// Nothing for TS to do (an unknown agent command, or no `requestId` to answer).
    Ignored,
}

/// TS sessionWorker.ts size (`Number.isFinite(value) ? Math.max(1, Math.floor(value)) : fallback`).
fn ts_size(value: Option<&Value>, fallback: i64) -> i64 {
    value
        .and_then(Value::as_f64)
        .filter(|size| size.is_finite())
        .map_or(fallback, |size| {
            // Saturating: a PTY size beyond i64 is clamped by the PTY anyway.
            #[allow(clippy::cast_possible_truncation)]
            let floored = size.floor() as i64;
            floored.max(1)
        })
}

/// Decodes a session worker command the way TS uses it. A command whose fields do not fit the types is first read
/// as TS's code reads it (sizes through `size()`, `force ?? false` as a flag, `scrollOffset` as a number or the
/// default, the `requestId` as text); one that still cannot run is answered as TS answers it: `start` without a
/// session fails reading it, and any other command fails the `target !== 'agent'` check with `<target> pane is hosted
/// by the workspace worker`, unless it targets the agent (then TS's switch does nothing with it).
pub fn decode_session_command(value: Value) -> SessionIncoming {
    if let Ok(command) = WorkerCommand::deserialize(&value) {
        return SessionIncoming::Command(command);
    }
    let Value::Object(fields) = &value else {
        return SessionIncoming::Ignored;
    };
    let kind = fields
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let mut read = fields.clone();
    if let Some(id) = read.get("requestId").filter(|id| !id.is_string()) {
        let id = js_string(id);
        read.insert("requestId".to_owned(), Value::String(id));
    }
    if matches!(kind.as_str(), "snapshot" | "attach" | "resize" | "start") {
        for (key, fallback) in [("cols", DEFAULT_COLS), ("rows", DEFAULT_ROWS)] {
            let fallback = i64::from(fallback);
            let size = ts_size(read.get(key), fallback);
            read.insert(key.to_owned(), Value::from(size));
        }
    }
    match read.get("scrollOffset") {
        Some(Value::Number(offset)) if !offset.is_i64() => {
            // `scrollOffset = 0` default aside, the snapshot floors it.
            #[allow(clippy::cast_possible_truncation)]
            let floored = offset
                .as_f64()
                .filter(|offset| offset.is_finite())
                .map_or(0, |offset| offset.floor() as i64);
            read.insert("scrollOffset".to_owned(), Value::from(floored));
        }
        Some(Value::Number(_)) | None => {}
        Some(_) => {
            read.remove("scrollOffset");
        }
    }
    match read.get("force").cloned() {
        Some(Value::Null) => {
            read.remove("force");
        }
        Some(force) => {
            read.insert("force".to_owned(), Value::Bool(js_truthy(&force)));
        }
        None => {}
    }
    let read = Value::Object(read);
    let error = match WorkerCommand::deserialize(&read) {
        Ok(command) => return SessionIncoming::Command(command),
        Err(error) => error,
    };
    let Some(request_id) = read.get("requestId").and_then(Value::as_str).map(str::to_owned) else {
        return SessionIncoming::Ignored;
    };
    let failure = |error: String| SessionIncoming::Failure {
        request_id: request_id.clone(),
        error,
    };
    match kind.as_str() {
        // startAgent reads `session.command` first.
        "start" => match fields.get("session") {
            None => failure("Cannot read properties of undefined (reading 'command')".to_owned()),
            Some(Value::Null) => failure("Cannot read properties of null (reading 'command')".to_owned()),
            Some(_) => failure(error.to_string()),
        },
        "kill" => failure(error.to_string()),
        _ => match fields.get("target") {
            Some(Value::String(target)) if target == "agent" => SessionIncoming::Ignored,
            target => failure(format!(
                "{} pane is hosted by the workspace worker",
                target.map_or_else(|| "undefined".to_owned(), js_string)
            )),
        },
    }
}

/// TS: `setTimeout(() => process.exit(0), delay).unref?.()`: after `delay`, flushes `channel` and exits the process
/// with status 0. The timer is detached (it fires even if nothing else runs).
pub fn exit_after(channel: &WorkerChannel, delay: Duration) {
    let channel = channel.clone();
    // Dropping the handle detaches the task.
    drop(deckhand_sys::async_util::spawn(async move {
        tokio::time::sleep(delay).await;
        channel.flush().await;
        std::process::exit(0);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use deckhand_protocol::WorkerCommand;
    use deckhand_protocol::worker_ipc::Running;
    use tokio::io::{AsyncBufReadExt, BufReader};

    fn run_local<F: std::future::Future>(future: F) -> F::Output {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        tokio::task::LocalSet::new().block_on(&runtime, future)
    }

    async fn read_line(lines: &mut tokio::io::Lines<BufReader<OwnedReadHalf>>) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .expect("line in time")
            .expect("read")
            .expect("not EOF");
        serde_json::from_str(&line).expect("JSON line")
    }

    #[test]
    fn posts_hello_first_then_messages_in_order() {
        run_local(async {
            let (worker_end, daemon_end) = UnixStream::pair().expect("pair");
            let (channel, _reader) = WorkerChannel::from_stream(worker_end, WireRole::Worker);
            assert!(channel.is_connected());
            channel.post(&WorkerMessage::Running(Running { pid: 42 }));
            channel.ok("r1", &serde_json::json!({"ok": true}));
            channel.ok("r2", &());
            channel.fail("r3", &DhError::msg("session is not running"));
            channel.flush().await;
            let (read_half, _write_half) = daemon_end.into_split();
            let mut lines = BufReader::new(read_half).lines();
            assert_eq!(
                read_line(&mut lines).await,
                serde_json::json!({"type": "hello", "version": 48})
            );
            assert_eq!(
                read_line(&mut lines).await,
                serde_json::json!({"type": "running", "pid": 42})
            );
            let ok = read_line(&mut lines).await;
            assert_eq!(
                serde_json::to_string(&ok).expect("json"),
                r#"{"type":"response","requestId":"r1","ok":true,"data":{"ok":true}}"#
            );
            assert_eq!(
                read_line(&mut lines).await,
                serde_json::json!({"type": "response", "requestId": "r2", "ok": true, "data": null})
            );
            assert_eq!(
                read_line(&mut lines).await,
                serde_json::json!({"type": "response", "requestId": "r3", "ok": false, "error": "session is not running"})
            );
        });
    }

    #[test]
    fn reader_decodes_commands_skips_junk_and_reports_disconnect() {
        run_local(async {
            let (worker_end, mut daemon_end) = UnixStream::pair().expect("pair");
            let (channel, mut reader) = WorkerChannel::from_stream(worker_end, WireRole::Worker);
            daemon_end
                .write_all(
                    b"not json\n[1,2]\n{\"type\":\"bogus\"}\n  \n{\"type\":\"kill\",\"requestId\":\"k\",\"force\":true}\n{\"type\":\"detach\",\"tar",
                )
                .await
                .expect("write");
            let command: WorkerCommand = reader.next().await.expect("command");
            assert!(
                matches!(command, WorkerCommand::Kill(ref kill) if kill.request_id == "k" && kill.force == Some(true))
            );
            daemon_end.write_all(b"get\":\"agent\"}\n").await.expect("write");
            let command: WorkerCommand = reader.next().await.expect("command");
            assert!(matches!(command, WorkerCommand::Detach(_)));
            drop(daemon_end);
            assert!(reader.next::<WorkerCommand>().await.is_none());
            assert!(!channel.is_connected());
            // Posting after the disconnect is a no-op, and flush returns at once.
            channel.post(&WorkerMessage::Running(Running { pid: 1 }));
            channel.flush().await;
        });
    }

    #[test]
    fn session_commands_that_do_not_decode_are_answered_as_ts_answers_them() {
        use serde_json::json;
        let failure = |request_id: &str, error: &str| SessionIncoming::Failure {
            request_id: request_id.to_owned(),
            error: error.to_owned(),
        };
        // A pane TS's session worker does not host, or none: the `target !== 'agent'` check fails it.
        assert_eq!(
            decode_session_command(
                json!({"type": "snapshot", "requestId": "a", "target": "bogus", "cols": 80, "rows": 24})
            ),
            failure("a", "bogus pane is hosted by the workspace worker")
        );
        assert_eq!(
            decode_session_command(json!({"type": "bogus", "requestId": "b"})),
            failure("b", "undefined pane is hosted by the workspace worker")
        );
        // An unknown command for the agent: TS's switch does nothing.
        assert_eq!(
            decode_session_command(json!({"type": "bogus", "requestId": "c", "target": "agent"})),
            SessionIncoming::Ignored
        );
        // Nothing to answer without a requestId.
        assert_eq!(
            decode_session_command(json!({"type": "attach", "target": "bogus"})),
            SessionIncoming::Ignored
        );
        // startAgent reads the session first.
        assert_eq!(
            decode_session_command(json!({"type": "start", "requestId": "d", "cols": 80, "rows": 24})),
            failure("d", "Cannot read properties of undefined (reading 'command')")
        );
        // Fields TS reads leniently run: sizes through size(), `force ?? false` as a flag, a numeric requestId.
        assert_eq!(
            decode_session_command(
                json!({"type": "snapshot", "requestId": 7, "target": "agent", "cols": "x", "rows": 30.5, "scrollOffset": "y"})
            ),
            SessionIncoming::Command(WorkerCommand::Snapshot(deckhand_protocol::worker_ipc::Snapshot {
                target: deckhand_protocol::data::AttachTarget::Agent,
                cols: 80,
                rows: 30,
                scroll_offset: None,
                request_id: "7".into(),
            }))
        );
        assert!(matches!(
            decode_session_command(json!({"type": "kill", "requestId": "k", "force": 1})),
            SessionIncoming::Command(WorkerCommand::Kill(ref kill)) if kill.force == Some(true)
        ));
    }

    #[test]
    fn next_value_returns_raw_objects() {
        run_local(async {
            let (worker_end, mut daemon_end) = UnixStream::pair().expect("pair");
            let (_channel, mut reader) = WorkerChannel::from_stream(worker_end, WireRole::WorkspaceWorker);
            daemon_end
                .write_all(b"\"text\"\n{\"type\":\"x\",\"n\":1}\n")
                .await
                .expect("write");
            assert_eq!(
                reader.next_value().await,
                Some(serde_json::json!({"type": "x", "n": 1}))
            );
        });
    }

    #[test]
    fn write_failure_is_a_disconnect() {
        run_local(async {
            let (worker_end, daemon_end) = UnixStream::pair().expect("pair");
            let (channel, _reader) = WorkerChannel::from_stream(worker_end, WireRole::Worker);
            drop(daemon_end);
            for _ in 0..50 {
                channel.post(&WorkerMessage::Running(Running { pid: 1 }));
                channel.flush().await;
                if !channel.is_connected() {
                    break;
                }
            }
            assert!(!channel.is_connected());
        });
    }

    #[test]
    fn role_can_switch() {
        run_local(async {
            let (worker_end, _daemon_end) = UnixStream::pair().expect("pair");
            let (channel, _reader) = WorkerChannel::from_stream(worker_end, WireRole::Worker);
            assert_eq!(channel.role(), WireRole::Worker);
            channel.set_role(WireRole::WorkspaceWorker);
            assert_eq!(channel.clone().role(), WireRole::WorkspaceWorker);
        });
    }

    #[test]
    fn wire_log_appends_dir_and_msg_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        wire_log_to(
            dir.path(),
            WireRole::WorkspaceWorker,
            WireDir::W2d,
            &WorkerMessage::Running(Running { pid: 7 }),
        );
        wire_log_to(
            dir.path(),
            WireRole::WorkspaceWorker,
            WireDir::D2w,
            &serde_json::json!({"type": "idle"}),
        );
        let path = dir
            .path()
            .join(format!("workspace-worker-{}.jsonl", std::process::id()));
        let text = std::fs::read_to_string(path).expect("log");
        assert_eq!(
            text,
            "{\"dir\":\"w2d\",\"msg\":{\"type\":\"running\",\"pid\":7}}\n{\"dir\":\"d2w\",\"msg\":{\"type\":\"idle\"}}\n"
        );
        // A directory that does not exist is ignored.
        wire_log_to(&dir.path().join("missing"), WireRole::Worker, WireDir::W2d, &1);
    }

    #[test]
    fn open_refuses_a_descriptor_that_is_not_a_socket() {
        // In the test harness fd 3 is either closed or not ours as a socket; it must never be taken over then.
        if !is_socket(CHANNEL_FD) {
            let error = run_local(async { WorkerChannel::open(WireRole::Worker).err() }).expect("error");
            assert_eq!(
                error.message(),
                "fd 3 is not a socket: deckhand workers are started by the daemon"
            );
        }
    }
}
