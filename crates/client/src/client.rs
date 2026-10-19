//! Port of client.ts (request, ping, spawn daemon).
//!
//! One-shot requests open a fresh connection to the daemon socket, write one NDJSON request and wait for the response
//! with the same `requestId`. [`ensure_daemon_running`] pings the daemon and starts one when none answers, but never
//! replaces a live daemon that speaks another protocol version: it fails with a `kill <pid>` hint instead, because
//! killing it would take every running agent down with it. The long-lived subscription lives in [`crate::live`].

use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::UnixStream;

use deckhand_core::error::DhError;
use deckhand_protocol::ndjson::LineDecoder;
use deckhand_protocol::{Request, ServerMessage};

/// TS: client.ts request default `timeoutMs` (15 s).
pub const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 15_000;

/// TS: client.ts waitForDaemon poll interval (150 ms between pings).
pub const DAEMON_POLL_INTERVAL: Duration = Duration::from_millis(150);

/// TS: client.ts ensureDaemonRunning: how long a freshly spawned daemon gets to answer a ping (5 s).
pub const DAEMON_START_DEADLINE_MS: u64 = 5_000;

/// TS: client.ts ensureDaemonRunning: how long a live daemon that failed one ping gets before we give up (2 s).
pub const DAEMON_RETRY_DEADLINE_MS: u64 = 2_000;

/// paths.ts items this module needs. deckhand-sys was being stubbed in parallel when this skeleton was frozen, so K1
/// replaces these with the `deckhand_sys::paths` equivalents (same semantics) when it implements the client.
#[allow(dead_code)]
mod sys_paths {
    use std::path::PathBuf;

    /// TS: paths.ts getConfigDir (`$DECKHAND_HOME` resolved, else `~/.deckhand`).
    pub(crate) fn get_config_dir() -> PathBuf {
        todo!("K1")
    }

    /// TS: paths.ts getSocketPath (`<config dir>/daemon.sock`).
    pub(crate) fn get_socket_path() -> PathBuf {
        todo!("K1")
    }

    /// TS: paths.ts getDaemonPidPath (`<config dir>/daemon.pid`).
    pub(crate) fn get_daemon_pid_path() -> PathBuf {
        todo!("K1")
    }

    /// TS: paths.ts getDaemonLogPath (`<config dir>/daemon.log`).
    pub(crate) fn get_daemon_log_path() -> PathBuf {
        todo!("K1")
    }

    /// TS: paths.ts getProjectRoot (the daemon's working directory).
    pub(crate) fn get_project_root() -> PathBuf {
        todo!("K1")
    }

    /// TS: paths.ts getCliEntryPath: in the Rust port, the `deckhand` executable that runs `--daemon` (the current
    /// executable; the TS dev runtime's tsx loader has no equivalent).
    pub(crate) fn get_cli_entry_path() -> PathBuf {
        todo!("K1")
    }
}

/// Which way a wire-logged message went (`{dir, msg}` in the wire log).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum WireDir {
    /// Client to daemon.
    #[serde(rename = "c2d")]
    C2d,
    /// Daemon to client.
    #[serde(rename = "d2c")]
    D2c,
}

/// TS: client.ts createConnection (connects to the daemon socket; a failure carries Node's message, e.g.
/// `connect ENOENT <socket path>`).
pub async fn create_connection() -> Result<UnixStream, DhError> {
    todo!("K1")
}

/// TS: client.ts wireLog. `DECKHAND_WIRE_LOG=<dir>` (test/porting aid, off by default): every message sent or received
/// is appended to `<dir>/client-<pid>.jsonl` as `{"dir":..,"msg":..}`; failures are ignored.
pub fn wire_log<T: Serialize + ?Sized>(dir: WireDir, msg: &T) {
    let _ = (dir, msg);
    todo!("K1")
}

/// The line [`write_message`] writes (`JSON.stringify(message)` + `\n`), wire-logged as `c2d`. Lets a caller that
/// queues writes (the [`crate::live::LiveClient`] writer) send synchronously, as `socket.write` does in TS.
pub fn frame_message(message: &Request) -> Result<Vec<u8>, DhError> {
    let _ = message;
    todo!("K1")
}

/// TS: client.ts writeMessage (exported): wire-logs and writes one request line.
pub async fn write_message<W: AsyncWrite + Unpin>(writer: &mut W, message: &Request) -> Result<(), DhError> {
    let _ = (writer, message);
    todo!("K1")
}

/// TS: client.ts attachJsonParser (exported): the per-socket framing state. Feeding it a chunk replaces the TS `data`
/// listener; dropping it replaces the returned stop function (`decoder.end()` output is discarded there too).
#[derive(Debug, Default)]
pub struct JsonParser {
    decoder: LineDecoder,
}

impl JsonParser {
    /// A parser with no line limit (`StringDecoder('utf8')`, split on `\n`, lines trimmed, empty lines skipped).
    pub fn new() -> Self {
        todo!("K1")
    }

    /// Feeds one chunk; returns the messages it completed, in order, each wire-logged as `d2c`. A line that is not
    /// JSON fails with `JSON.parse`'s message ([`DhError::json_parse`]); a JSON line whose `type` no
    /// [`ServerMessage`] variant has is skipped, as every TS consumer ignores unknown types.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<ServerMessage>, DhError> {
        let _ = (&self.decoder, chunk);
        todo!("K1")
    }
}

/// Reads [`ServerMessage`]s off a socket with a [`JsonParser`] (the async face of `attachJsonParser` + `on('data')`).
#[derive(Debug)]
pub struct MessageReader<R> {
    reader: R,
    parser: JsonParser,
    queued: std::collections::VecDeque<ServerMessage>,
}

impl<R: AsyncRead + Unpin> MessageReader<R> {
    /// Wraps the read half of a daemon connection.
    pub fn new(reader: R) -> Self {
        let _ = reader;
        todo!("K1")
    }

    /// The next message; `Ok(None)` once the daemon closed the connection.
    pub async fn next(&mut self) -> Result<Option<ServerMessage>, DhError> {
        let _ = (&mut self.reader, &mut self.parser, &mut self.queued);
        todo!("K1")
    }

    /// The read half back.
    pub fn into_inner(self) -> R {
        todo!("K1")
    }
}

/// TS: client.ts request (exported). Sends one request on a fresh connection and resolves with the matching response's
/// `data`. Errors (verbatim TS messages): the response's `error` (or `daemon request failed`), `daemon request timed
/// out` after `timeout_ms` (default [`DEFAULT_REQUEST_TIMEOUT_MS`]), `daemon connection closed before response`, or the
/// socket's own error.
pub async fn request<T: DeserializeOwned>(message: &Request, timeout_ms: Option<u64>) -> Result<T, DhError> {
    let _ = (message, timeout_ms);
    todo!("K1")
}

/// TS: client.ts ProtocolMismatchError vs any other ping failure.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PingError {
    /// TS: ProtocolMismatchError: `daemon protocol mismatch: expected v46, got <version>` (`undefined` when the daemon
    /// sent no version).
    #[error("{0}")]
    ProtocolMismatch(String),
    /// The ping request itself failed (no daemon, timeout, closed connection, ...).
    #[error(transparent)]
    Failed(#[from] DhError),
}

/// TS: client.ts ping: a `ping` request whose `version` must equal [`deckhand_protocol::PROTOCOL_VERSION`].
pub async fn ping() -> Result<(), PingError> {
    todo!("K1")
}

/// TS: client.ts readDaemonPid: the positive integer in `daemon.pid` (`parseInt` semantics), if any.
pub async fn read_daemon_pid() -> Option<i32> {
    todo!("K1")
}

/// TS: client.ts isProcessAlive (`kill(pid, 0)` succeeds; `None`/0 is never alive).
pub fn is_process_alive(pid: Option<i32>) -> bool {
    let _ = pid;
    todo!("K1")
}

/// TS: client.ts appendClientLog: appends `[<ISO time>] client <message>\n` to `daemon.log` (creating the config dir).
pub async fn append_client_log(message: &str) -> Result<(), DhError> {
    let _ = message;
    todo!("K1")
}

/// TS: client.ts spawnDaemon: starts `<deckhand> --daemon` detached (own session), cwd the project root, env plus
/// `DECKHAND_DAEMON=1`, stdin ignored and stdout/stderr appended to `daemon.log`; never waited for.
pub fn spawn_daemon() -> Result<(), DhError> {
    todo!("K1")
}

/// TS: client.ts waitForDaemon: pings every [`DAEMON_POLL_INTERVAL`] until one succeeds or `deadline_ms` passes, then
/// fails with `failed to start daemon: <last ping error>`.
pub async fn wait_for_daemon(deadline_ms: u64) -> Result<(), DhError> {
    let _ = deadline_ms;
    todo!("K1")
}

/// TS: client.ts ensureDaemonRunning (exported).
///
/// - Ping succeeds: done.
/// - Protocol mismatch and the pid in `daemon.pid` is alive: logs and fails with
///   `<mismatch message>; stop the old daemon first: kill <pid> (or kill $(cat <pid path>))`.
/// - Other failure with a live pid: retries for [`DAEMON_RETRY_DEADLINE_MS`], then fails with
///   `daemon pid <pid> is alive but did not respond; see <log path>: <error>`.
/// - Otherwise: logs, removes the stale socket, spawns a daemon and waits [`DAEMON_START_DEADLINE_MS`] for it.
pub async fn ensure_daemon_running() -> Result<(), DhError> {
    todo!("K1")
}

/// TS: client.ts openPersistentConnection (exported): [`ensure_daemon_running`], then a new connection.
pub async fn open_persistent_connection() -> Result<UnixStream, DhError> {
    todo!("K1")
}
