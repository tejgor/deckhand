//! Port of daemon.ts (socket server, clients, subscriptions, broadcast).
//!
//! The daemon listens on the private Unix socket (paths.ts getSocketPath). Every connection is multiplexed: the UI's
//! requests, its subscriptions (what it watches, at which size) and attach streams share it. Each connection gets a
//! [`ClientHandle`] (TS: the `net.Socket` used as the `clients` map key and as the attach target) and a
//! [`Connection`] holding what handleConnection kept in closure variables. Requests are read as NDJSON lines
//! ([`deckhand_protocol::ndjson::LineDecoder`] with `MAX_REQUEST_LINE`) and each is handled in its own local task, as
//! TS starts `handleRequest` without awaiting it.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use deckhand_core::error::Result;
use deckhand_protocol::Ordered;
use deckhand_protocol::data::{PingResult, SessionRecord};
use deckhand_protocol::events::ServerMessage;
use deckhand_protocol::wire::{Request, Subscribe};
use deckhand_sys::async_util::Task;
use indexmap::IndexMap;
use serde_json::Value;
use tokio::net::UnixStream;
use tokio::sync::mpsc;

use crate::daemon::Daemon;

/// TS: daemon.ts DEFAULT_PREVIEW_COLS (a client's pane sizes until it says otherwise).
pub const DEFAULT_PREVIEW_COLS: i64 = 80;

/// TS: daemon.ts DEFAULT_PREVIEW_ROWS.
pub const DEFAULT_PREVIEW_ROWS: i64 = 24;

/// One client connection's identity (TS: the socket object as a map key).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClientId(pub u64);

/// TS: a client's `net.Socket` as the daemon writes to it: lines go to the connection's writer task. Clones address
/// the same connection.
#[derive(Clone, Debug)]
pub struct ClientHandle {
    pub id: ClientId,
    /// Complete NDJSON lines for the writer task.
    pub tx: mpsc::UnboundedSender<String>,
    /// TS `socket.destroyed`.
    pub destroyed: Rc<Cell<bool>>,
}

impl ClientHandle {
    /// A handle writing to `tx`.
    pub fn new(id: ClientId, tx: mpsc::UnboundedSender<String>) -> Self {
        let _ = (id, tx);
        todo!("D1")
    }

    /// TS: `socket.destroyed`.
    pub fn is_destroyed(&self) -> bool {
        todo!("D1")
    }

    /// TS: `socket.destroy()`: nothing more is written; the connection closes.
    pub fn destroy(&self) {
        todo!("D1")
    }
}

/// What daemon.ts handleConnection keeps per connection besides its `clients` entry.
#[derive(Debug)]
pub struct Connection {
    pub client: ClientHandle,
    /// TS handleConnection `attachedSessionId` (set through handleRequest's `setAttachedSessionId`).
    pub attached_session_id: RefCell<Option<String>>,
}

impl Connection {
    /// A connection with nothing attached.
    pub fn new(client: ClientHandle) -> Rc<Self> {
        let _ = client;
        todo!("D1")
    }
}

/// TS: daemon.ts ClientSubscription: what a client watches and at which sizes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientSubscription {
    /// The checkout the client was opened in (`subscribe`); it also hears about every session of the same
    /// repository (`repo_key`).
    pub repo_root: Option<String>,
    pub repo_key: Option<String>,
    /// The repository whose task list this client shows (`watch-tasks`).
    pub tasks_key: Option<String>,
    pub watched_preview_session_id: Option<String>,
    pub watched_terminal_session_id: Option<String>,
    pub watched_git_session_id: Option<String>,
    pub watched_dev_session_id: Option<String>,
    pub watched_action_session_id: Option<String>,
    pub watched_changes_session_id: Option<String>,
    pub preview_cols: i64,
    pub preview_rows: i64,
    pub preview_scroll_offset: i64,
    pub terminal_cols: i64,
    pub terminal_rows: i64,
    pub git_cols: i64,
    pub git_rows: i64,
    pub dev_cols: i64,
    pub dev_rows: i64,
    pub action_cols: i64,
    pub action_rows: i64,
}

impl Default for ClientSubscription {
    /// The literal handleConnection and getClient start a client with.
    fn default() -> Self {
        ClientSubscription {
            repo_root: None,
            repo_key: None,
            tasks_key: None,
            watched_preview_session_id: None,
            watched_terminal_session_id: None,
            watched_git_session_id: None,
            watched_dev_session_id: None,
            watched_action_session_id: None,
            watched_changes_session_id: None,
            preview_cols: DEFAULT_PREVIEW_COLS,
            preview_rows: DEFAULT_PREVIEW_ROWS,
            preview_scroll_offset: 0,
            terminal_cols: DEFAULT_PREVIEW_COLS,
            terminal_rows: DEFAULT_PREVIEW_ROWS,
            git_cols: DEFAULT_PREVIEW_COLS,
            git_rows: DEFAULT_PREVIEW_ROWS,
            dev_cols: DEFAULT_PREVIEW_COLS,
            dev_rows: DEFAULT_PREVIEW_ROWS,
            action_cols: DEFAULT_PREVIEW_COLS,
            action_rows: DEFAULT_PREVIEW_ROWS,
        }
    }
}

/// One entry of TS `clients` (`Map<net.Socket, ClientSubscription>`).
#[derive(Clone, Debug)]
pub struct Client {
    pub handle: ClientHandle,
    pub subscription: ClientSubscription,
}

/// The server's part of the daemon state.
#[derive(Default)]
pub struct ServerState {
    /// TS `server`: the accept loop, dropped (stopping it) at cleanup.
    pub listener: Option<Task<()>>,
    /// TS `clients`, in connection order (JS Map iteration order).
    pub clients: IndexMap<ClientId, Client>,
    /// The next [`ClientId`].
    pub next_client_id: u64,
    /// TS `repoKeys`: resolved repository keys by checkout path (sync, for broadcasts).
    pub repo_keys: HashMap<String, String>,
    /// TS `repoKeyMisses`: checkouts that could not be resolved (not tried again).
    pub repo_key_misses: HashSet<String>,
}

/// What a received line is, once parsed (TS handleConnection's JSON.parse check plus handleRequest's switch).
#[derive(Clone, Debug, PartialEq)]
pub enum ParsedRequest {
    /// A request this daemon knows.
    Request(Box<Request>),
    /// An object with a string `type` no case handles: ignored without a response (the TS switch falls through).
    Ignored,
    /// A known `type` whose fields do not fit: answered with a failure when it carries a `requestId`.
    Invalid {
        request_id: Option<String>,
        message: String,
    },
}

/// Types a line that passed the `{type: string}` check ([`deckhand_protocol::ndjson::parse_message`]).
pub fn parse_request(value: Value) -> ParsedRequest {
    let _ = value;
    todo!("D1")
}

/// TS: daemon.ts sendMessage: writes `message` as one line unless the socket is destroyed (wire-logged `d2c`).
pub fn send_message(client: &ClientHandle, message: &ServerMessage) {
    let _ = (client, message);
    todo!("D1")
}

/// TS: daemon.ts clampSize: at least 1 (TS also maps non-finite numbers to `fallback`; the wire types are integers).
pub fn clamp_size(value: i64, fallback: i64) -> i64 {
    let _ = (value, fallback);
    todo!("D1")
}

/// TS: daemon.ts clampNonNegative.
pub fn clamp_non_negative(value: i64) -> i64 {
    let _ = value;
    todo!("D1")
}

impl Daemon {
    /// TS: InkDaemon.prepareSocket: removes a stale socket file (logged), any error but ENOENT fails startup.
    pub async fn prepare_socket(&self) -> Result<()> {
        todo!("D1")
    }

    /// TS: InkDaemon.listen: binds the socket under a 0o077 umask (restored after) and starts accepting.
    pub async fn listen(&self) -> Result<()> {
        todo!("D1")
    }

    /// TS: InkDaemon.handleConnection: registers the client and reads its requests until it closes (oversized lines
    /// are rejected; a malformed line closes the connection after logging `closing client after malformed
    /// request`).
    pub fn handle_connection(&self, stream: UnixStream) {
        let _ = stream;
        todo!("D1")
    }

    /// TS: handleConnection's `cleanup` (socket `close`/`error`): releases the agent attach and every workspace pane
    /// attach this socket holds, forgets the client and re-syncs the Changes watches.
    pub fn client_disconnected(&self, conn: &Connection) {
        let _ = conn;
        todo!("D1")
    }

    /// TS: InkDaemon.rejectOversizedRequest: logs it and, when the excerpt names a `requestId`, answers
    /// `Request exceeds 256000 characters`.
    pub fn reject_oversized_request(&self, client: &ClientHandle, excerpt: &str) {
        let _ = (client, excerpt);
        todo!("D1")
    }

    /// TS: InkDaemon.getClient: runs `update` on the client's subscription (created with the defaults when missing).
    pub fn get_client<R>(&self, client: &ClientHandle, update: impl FnOnce(&mut ClientSubscription) -> R) -> R {
        let _ = (client, update);
        todo!("D1")
    }

    /// TS: InkDaemon.inClientScope: the client's checkout's sessions, and those of every checkout of its repository
    /// (an unknown checkout's key is looked up in the background, announcing its sessions when found).
    pub fn in_client_scope(&self, session: &SessionRecord, client: &ClientSubscription) -> bool {
        let _ = (session, client);
        todo!("D1")
    }

    /// TS: InkDaemon.learnRepoKey: resolves a checkout's repository key once (misses are remembered); `announce`:
    /// broadcast that checkout's sessions to the clients now in scope.
    pub async fn learn_repo_key(&self, root: &str, announce: bool) {
        let _ = (root, announce);
        todo!("D1")
    }

    /// TS: InkDaemon.broadcastSessionUpdated: the session as held now (else as given) to every client in scope.
    pub fn broadcast_session_updated(&self, session: &Ordered<SessionRecord>) {
        let _ = session;
        todo!("D1")
    }

    /// TS: InkDaemon.broadcastSessionRemoved.
    pub fn broadcast_session_removed(&self, session: &SessionRecord) {
        let _ = session;
        todo!("D1")
    }

    /// TS: handleRequest `ping`: `{ok: true, version: PROTOCOL_VERSION, home, channel}`.
    pub fn handle_ping(&self) -> Result<PingResult> {
        todo!("D1")
    }

    /// TS: handleRequest `subscribe`: records the client's checkout and repository key, learns every session's
    /// repository key, and answers the sessions in the client's scope in sidebar order.
    pub async fn handle_subscribe(
        &self,
        conn: &Connection,
        request: &Subscribe,
    ) -> Result<Vec<Ordered<SessionRecord>>> {
        let _ = (conn, request);
        todo!("D1")
    }
}
