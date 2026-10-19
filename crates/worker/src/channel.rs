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
use std::os::fd::RawFd;
use std::rc::Rc;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::net::UnixStream;
use tokio::net::unix::OwnedReadHalf;
use tokio::sync::mpsc;

use deckhand_core::error::DhError;
use deckhand_protocol::WorkerMessage;
use deckhand_protocol::ndjson::LineDecoder;

/// The descriptor the daemon passes the worker's end of the socketpair on (Node's `stdio[3] = 'ipc'`).
pub const CHANNEL_FD: RawFd = 3;

/// TS: sessionWorker.ts wireLog env var. `DECKHAND_WIRE_LOG=<dir>` (test/porting aid, off by default).
pub const WIRE_LOG_ENV: &str = "DECKHAND_WIRE_LOG";

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

/// TS: sessionWorker.ts wireLog. When `DECKHAND_WIRE_LOG` is set, appends `{"dir":..,"msg":..}` plus `\n` to
/// `<dir>/<role>-<pid>.jsonl` (synchronously, as `appendFileSync`); every failure is ignored.
pub fn wire_log<T: Serialize + ?Sized>(role: WireRole, dir: WireDir, msg: &T) {
    let _ = (role, dir, msg);
    todo!("W1")
}

/// TS: sessionWorker.ts post/ok/fail over `process.send`: the worker's sending side of the channel.
///
/// A cheap handle (clones share one channel). Must be created inside the process's `LocalSet` (it spawns the writer
/// task).
#[derive(Clone)]
pub struct WorkerChannel {
    inner: Rc<ChannelInner>,
}

#[allow(dead_code)] // W1: read once the methods below are written.
struct ChannelInner {
    /// The current wire-log role (a workspace worker switches it when it starts).
    role: Cell<WireRole>,
    /// Queued lines for the writer task; `None` once the daemon disconnected (`process.connected` false).
    writer: RefCell<Option<mpsc::UnboundedSender<Vec<u8>>>>,
    /// Lines queued but not yet written (for [`WorkerChannel::flush`]).
    pending: Cell<usize>,
}

impl WorkerChannel {
    /// Opens the daemon's socketpair end on [`CHANNEL_FD`] and posts [`WorkerMessage::Hello`] first. Fails when fd 3
    /// is not a socket (the worker was not started by a daemon).
    pub fn open(role: WireRole) -> Result<(WorkerChannel, ChannelReader), DhError> {
        let _ = role;
        todo!("W1")
    }

    /// [`WorkerChannel::open`] over an already connected stream (tests use one end of a `UnixStream::pair`). Posts
    /// [`WorkerMessage::Hello`] first.
    pub fn from_stream(stream: UnixStream, role: WireRole) -> (WorkerChannel, ChannelReader) {
        let _ = (stream, role);
        todo!("W1")
    }

    /// The wire-log role.
    pub fn role(&self) -> WireRole {
        let _ = &self.inner;
        todo!("W1")
    }

    /// TS: `wireRole = 'workspace-worker'` when a workspace worker starts.
    pub fn set_role(&self, role: WireRole) {
        let _ = role;
        todo!("W1")
    }

    /// TS: `process.connected`: false once the daemon end closed or a write failed.
    pub fn is_connected(&self) -> bool {
        todo!("W1")
    }

    /// TS: sessionWorker.ts post. A no-op once disconnected; otherwise wire-logs the message (`w2d`) and queues its
    /// line. Never fails: a broken channel is the disconnect, which the reader reports.
    pub fn post(&self, message: &WorkerMessage) {
        let _ = message;
        todo!("W1")
    }

    /// TS: sessionWorker.ts ok: `{type: 'response', requestId, ok: true, data}`.
    pub fn ok<T: Serialize + ?Sized>(&self, request_id: &str, data: &T) {
        let _ = (request_id, data);
        todo!("W1")
    }

    /// TS: sessionWorker.ts fail: `{type: 'response', requestId, ok: false, error: error.message}`.
    pub fn fail(&self, request_id: &str, error: &DhError) {
        let _ = (request_id, error);
        todo!("W1")
    }

    /// Resolves once every line queued so far was written (or the channel broke), so a worker can exit without
    /// losing its last messages (Node flushed `process.send` before `process.exit`).
    pub async fn flush(&self) {
        todo!("W1")
    }
}

/// TS: `process.on('message')` / `process.on('disconnect')`: the worker's receiving side of the channel.
#[allow(dead_code)] // W1: read once the methods below are written.
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
        todo!("W1")
    }

    /// The next message decoded as `C` (a [`deckhand_protocol::WorkerCommand`] or
    /// [`deckhand_protocol::WorkspaceWorkerCommand`]); one that does not decode is skipped, as the TS `switch` ignored
    /// unknown command types. `None` at disconnect.
    pub async fn next<C: DeserializeOwned>(&mut self) -> Option<C> {
        todo!("W1")
    }
}

/// TS: `setTimeout(() => process.exit(0), delay).unref?.()`: after `delay`, flushes `channel` and exits the process
/// with status 0. The timer is detached (it fires even if nothing else runs).
pub fn exit_after(channel: &WorkerChannel, delay: Duration) {
    let _ = (channel, delay);
    todo!("W1")
}
