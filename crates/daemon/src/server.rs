//! The daemon's socket server: clients, subscriptions, broadcast.
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
use std::time::Duration;

use deckhand_core::error::{DhError, Result, errno_code, errno_description};
use deckhand_core::jsstr::path;
use deckhand_core::session_order::sort_sessions_for_sidebar;
use deckhand_core::session_scope::session_matches_scope;
use deckhand_protocol::Ordered;
use deckhand_protocol::PROTOCOL_VERSION;
use deckhand_protocol::data::{PingResult, SessionRecord};
use deckhand_protocol::events::{ServerMessage, SessionRef, SessionUpdated};
use deckhand_protocol::ndjson::{self, Frame, LineDecoder, MAX_REQUEST_LINE, oversized_request_id, parse_message};
use deckhand_protocol::wire::{Loose, Request, Subscribe, js_string, js_truthy, node_received, request_type};
use deckhand_sys::async_util::{Task, spawn};
use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::{Map, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::mpsc;

use crate::daemon::Daemon;
use crate::dispatch::failure;
use crate::worker_channel::{WireDir, wire_log};

/// TS: daemon.ts DEFAULT_PREVIEW_COLS (a client's pane sizes until it says otherwise).
pub const DEFAULT_PREVIEW_COLS: i64 = 80;

/// TS: daemon.ts DEFAULT_PREVIEW_ROWS.
pub const DEFAULT_PREVIEW_ROWS: i64 = 24;

/// One socket read at most.
const READ_CHUNK: usize = 64 * 1024;

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
        ClientHandle {
            id,
            tx,
            destroyed: Rc::new(Cell::new(false)),
        }
    }

    /// TS: `socket.destroyed`.
    pub fn is_destroyed(&self) -> bool {
        self.destroyed.get() || self.tx.is_closed()
    }

    /// TS: `socket.destroy()`: nothing more is written; the connection closes.
    pub fn destroy(&self) {
        if self.destroyed.replace(true) {
            return;
        }
        // An empty line is never a message (they end in `\n`): it tells the writer to close the connection.
        let _ = self.tx.send(String::new());
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
        Rc::new(Connection {
            client,
            attached_session_id: RefCell::new(None),
        })
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
    /// A request this daemon knows, and where its failure is reported: the message's `requestId` when it has the
    /// key (TS `'requestId' in message`, for every type), as text.
    Request {
        request: Box<Request>,
        reply_to: Option<String>,
    },
    /// An object with a string `type` no case handles: ignored without a response (the TS switch falls through).
    Ignored,
    /// A known `type` that could not be decoded (not reached: every request field decodes leniently, see
    /// [`deckhand_protocol::wire`]); answered with a failure when it carries a `requestId`.
    Invalid {
        request_id: Option<String>,
        message: String,
    },
}

/// Types a line that passed the `{type: string}` check ([`deckhand_protocol::ndjson::parse_message`]).
///
/// TS never validates a known request's fields: each case reads what it needs and passes on whatever was sent. The
/// wire types decode every field leniently into what the daemon needs to do the same (the cases then check values
/// where and as TS does, with its messages), so a request of a known type always decodes.
pub fn parse_request(value: Value) -> ParsedRequest {
    let Some(kind) = request_type(&value) else {
        return ParsedRequest::Ignored;
    };
    if !Request::TYPES.contains(&kind) {
        return ParsedRequest::Ignored;
    }
    let kind = kind.to_owned();
    let reply_to = value.get("requestId").map(|id| match id {
        Value::String(id) => id.clone(),
        other => js_string(other),
    });
    match Request::deserialize(&value) {
        Ok(request) => ParsedRequest::Request {
            request: Box::new(request),
            reply_to,
        },
        Err(error) => ParsedRequest::Invalid {
            request_id: reply_to,
            message: format!("Invalid {kind} request: {error}"),
        },
    }
}

/// TS: daemon.ts sendMessage: writes `message` as one line unless the socket is destroyed (wire-logged `d2c`).
pub fn send_message(client: &ClientHandle, message: &ServerMessage) {
    if client.is_destroyed() {
        return;
    }
    wire_log(WireDir::D2c, message, None);
    if let Ok(line) = ndjson::encode_line(message) {
        let _ = client.tx.send(line);
    }
}

/// TS: daemon.ts clampSize: at least 1; `fallback` for what is not a finite number (`None`; a fraction is already
/// floored by the request's decoding).
pub fn clamp_size(value: Option<i64>, fallback: i64) -> i64 {
    value.map_or(fallback, |value| value.max(1))
}

/// TS: daemon.ts clampNonNegative (0 for what is not a finite number).
pub fn clamp_non_negative(value: Option<i64>) -> i64 {
    value.map_or(0, |value| value.max(0))
}

/// Node's `listen` error: `listen EADDRINUSE: address already in use /path`.
fn listen_error(error: &std::io::Error, socket: &str) -> DhError {
    match errno_code(error) {
        Some(code) => DhError::with_code(code, format!("listen {code}: {} {socket}", errno_description(code))),
        None => DhError::msg(error.to_string()),
    }
}

/// Writes a connection's lines until it is destroyed (the empty line) or a write fails, then closes its side.
async fn write_lines(mut half: OwnedWriteHalf, mut lines: mpsc::UnboundedReceiver<String>) {
    while let Some(line) = lines.recv().await {
        if line.is_empty() || half.write_all(line.as_bytes()).await.is_err() {
            break;
        }
    }
    let _ = half.shutdown().await;
}

/// `{type: 'detach', target}` for a worker.
fn detach_payload(target: &str) -> Map<String, Value> {
    let mut payload = Map::new();
    payload.insert("type".to_owned(), Value::String("detach".to_owned()));
    payload.insert("target".to_owned(), Value::String(target.to_owned()));
    payload
}

impl Daemon {
    /// `void this.log(message)`: logged in the background.
    pub(crate) fn log_soon(&self, message: String) {
        let daemon = self.clone();
        drop(spawn(async move { daemon.log(&message).await }));
    }

    /// TS: InkDaemon.prepareSocket: removes a stale socket file (logged), any error but ENOENT fails startup.
    pub async fn prepare_socket(&self) -> Result<()> {
        match tokio::fs::remove_file(&self.paths.socket).await {
            Ok(()) => {
                self.log("removed stale socket before listen").await;
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(DhError::io(&error, "unlink", &self.paths.socket.to_string_lossy())),
        }
    }

    /// TS: InkDaemon.listen: binds the socket under a 0o077 umask (restored after) and starts accepting.
    pub async fn listen(&self) -> Result<()> {
        use nix::sys::stat::{Mode, umask};
        let socket = self.paths.socket.clone();
        // The socket file is created at bind time; a 0o077 umask keeps it private before the explicit chmod, so no
        // other local user can connect in between.
        let previous = umask(Mode::from_bits_truncate(0o077));
        let bound = std::os::unix::net::UnixListener::bind(&socket);
        umask(previous);
        let listener = bound.map_err(|error| listen_error(&error, &socket.to_string_lossy()))?;
        listener.set_nonblocking(true)?;
        let listener = tokio::net::UnixListener::from_std(listener)?;
        let daemon = self.downgrade();
        let accept = spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let Some(daemon) = daemon.upgrade() else {
                            return;
                        };
                        daemon.handle_connection(stream);
                    }
                    // Out of descriptors and the like: try again shortly instead of spinning.
                    Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
                }
            }
        });
        let replaced = self.server.borrow_mut().listener.replace(accept);
        if let Some(replaced) = replaced {
            replaced.abort();
        }
        Ok(())
    }

    /// TS: InkDaemon.handleConnection: registers the client and reads its requests until it closes (oversized lines
    /// are rejected; a malformed line closes the connection after logging `closing client after malformed
    /// request`).
    pub fn handle_connection(&self, stream: UnixStream) {
        self.log_soon("client connected".to_owned());
        let (tx, rx) = mpsc::unbounded_channel();
        let client = {
            let mut server = self.server.borrow_mut();
            server.next_client_id += 1;
            let client = ClientHandle::new(ClientId(server.next_client_id), tx);
            server.clients.insert(
                client.id,
                Client {
                    handle: client.clone(),
                    subscription: ClientSubscription::default(),
                },
            );
            client
        };
        let conn = Connection::new(client);
        let daemon = self.clone();
        drop(spawn(async move {
            let (mut read_half, write_half) = stream.into_split();
            let mut writer = spawn(write_lines(write_half, rx));
            let mut decoder = LineDecoder::with_max_line(MAX_REQUEST_LINE);
            let mut chunk = vec![0u8; READ_CHUNK];
            'connection: loop {
                let read = tokio::select! {
                    read = read_half.read(&mut chunk) => match read {
                        Ok(0) | Err(_) => break,
                        Ok(read) => read,
                    },
                    () = &mut writer => break,
                };
                let Some(bytes) = chunk.get(..read) else {
                    break;
                };
                for frame in decoder.push(bytes) {
                    let line = match frame {
                        // The connection is multiplexed (UI + attach); one oversized line is rejected, not fatal.
                        Frame::Oversized { excerpt } => {
                            daemon.reject_oversized_request(&conn.client, &excerpt);
                            continue;
                        }
                        Frame::Line(line) => line,
                    };
                    let Ok(value) = parse_message(&line) else {
                        daemon.log_soon("closing client after malformed request".to_owned());
                        conn.client.destroy();
                        break 'connection;
                    };
                    wire_log(WireDir::C2d, &value, None);
                    match parse_request(value) {
                        ParsedRequest::Request { request, reply_to } => {
                            // TS starts handleRequest without awaiting it, but its synchronous part (getClient,
                            // synchronous replies) runs before the next line is parsed. Yielding lets the spawned
                            // task get that far first, so replies keep line order and a malformed line later in
                            // the chunk can neither drop this reply nor run cleanup before getClient.
                            let (daemon, conn) = (daemon.clone(), Rc::clone(&conn));
                            drop(spawn(async move {
                                daemon.handle_request_from(&conn, *request, reply_to).await
                            }));
                            tokio::task::yield_now().await;
                        }
                        ParsedRequest::Ignored | ParsedRequest::Invalid { request_id: None, .. } => {}
                        ParsedRequest::Invalid {
                            request_id: Some(request_id),
                            message,
                        } => send_message(&conn.client, &failure(&request_id, message)),
                    }
                }
            }
            conn.client.destroy();
            daemon.client_disconnected(&conn);
        }));
    }

    /// TS: handleConnection's `cleanup` (socket `close`/`error`): releases the agent attach and every workspace pane
    /// attach this socket holds, forgets the client and re-syncs the Changes watches.
    pub fn client_disconnected(&self, conn: &Connection) {
        let id = conn.client.id;
        let attached = conn.attached_session_id.borrow_mut().take();
        if let Some(session_id) = attached {
            let worker = self.launch.borrow().workers.get(&session_id).cloned();
            if let Some(worker) = worker {
                let mine = worker
                    .attached_agent
                    .borrow()
                    .as_ref()
                    .is_some_and(|client| client.id == id);
                if mine {
                    worker.attached_agent.borrow_mut().take();
                    // TS sendWorkerEvent(attachedSessionId, ...): the worker just looked up.
                    worker.channel.send_event(detach_payload("agent"));
                }
            }
        }
        let workspaces: Vec<_> = self.workspaces.borrow().workspaces.values().cloned().collect();
        for workspace in workspaces {
            let panes: Vec<_> = workspace
                .attached
                .borrow()
                .iter()
                .filter(|(_, viewer)| viewer.client.id == id)
                .map(|(pane, _)| *pane)
                .collect();
            for pane in panes {
                workspace.attached.borrow_mut().shift_remove(&pane);
                if let Ok(Value::String(target)) = serde_json::to_value(pane) {
                    workspace.channel.send_event(detach_payload(&target));
                }
            }
        }
        self.server.borrow_mut().clients.shift_remove(&id);
        self.sync_change_watches();
        self.log_soon("client disconnected".to_owned());
    }

    /// TS: InkDaemon.rejectOversizedRequest: logs it and, when the excerpt names a `requestId`, answers
    /// `Request exceeds 256000 characters`.
    pub fn reject_oversized_request(&self, client: &ClientHandle, excerpt: &str) {
        let request_id = oversized_request_id(excerpt);
        let named = request_id.map(|id| format!(" requestId={id}")).unwrap_or_default();
        self.log_soon(format!(
            "rejected oversized client message (>{MAX_REQUEST_LINE} chars){named}"
        ));
        if let Some(request_id) = request_id {
            send_message(
                client,
                &failure(request_id, format!("Request exceeds {MAX_REQUEST_LINE} characters")),
            );
        }
    }

    /// TS: InkDaemon.getClient: runs `update` on the client's subscription (created with the defaults when missing).
    ///
    /// The server state is borrowed while `update` runs: it must not reach back into the daemon.
    pub fn get_client<R>(&self, client: &ClientHandle, update: impl FnOnce(&mut ClientSubscription) -> R) -> R {
        let mut server = self.server.borrow_mut();
        let entry = server.clients.entry(client.id).or_insert_with(|| Client {
            handle: client.clone(),
            subscription: ClientSubscription::default(),
        });
        update(&mut entry.subscription)
    }

    /// TS: InkDaemon.inClientScope: the client's checkout's sessions, and those of every checkout of its repository
    /// (an unknown checkout's key is looked up in the background, announcing its sessions when found).
    pub fn in_client_scope(&self, session: &SessionRecord, client: &ClientSubscription) -> bool {
        // TS `if (!client.repoRoot) return false`: an empty root is not subscribed either.
        let Some(repo_root) = client.repo_root.as_deref().filter(|root| !root.is_empty()) else {
            return false;
        };
        if session_matches_scope(session, repo_root) {
            return true;
        }
        let Some(repo_key) = &client.repo_key else {
            return false;
        };
        let root = path::resolve(&session.repo_root);
        let (key, missed) = {
            let server = self.server.borrow();
            (
                server.repo_keys.get(&root).cloned(),
                server.repo_key_misses.contains(&root),
            )
        };
        if key.is_none() && !missed {
            let daemon = self.clone();
            drop(spawn(async move { daemon.learn_repo_key(&root, true).await }));
        }
        key.as_ref() == Some(repo_key)
    }

    /// TS: InkDaemon.learnRepoKey: resolves a checkout's repository key once (misses are remembered); `announce`:
    /// broadcast that checkout's sessions to the clients now in scope.
    pub async fn learn_repo_key(&self, root: &str, announce: bool) {
        {
            let server = self.server.borrow();
            if server.repo_keys.contains_key(root) || server.repo_key_misses.contains(root) {
                return;
            }
        }
        let Ok(key) = self.tasks_key_for(root).await else {
            self.server.borrow_mut().repo_key_misses.insert(root.to_owned());
            return;
        };
        self.server.borrow_mut().repo_keys.insert(root.to_owned(), key);
        if announce {
            let sessions = self.session_list();
            for session in sessions {
                if path::resolve(&session.repo_root) == root {
                    self.broadcast_session_updated(&session);
                }
            }
        }
    }

    /// `[...this.sessions.values()]`, read from the state directly (so sending never holds it).
    fn session_list(&self) -> Vec<Ordered<SessionRecord>> {
        self.sessions.borrow().sessions.values().cloned().collect()
    }

    /// The clients, as they are now (so sending never holds the server state).
    fn client_list(&self) -> Vec<Client> {
        self.server.borrow().clients.values().cloned().collect()
    }

    /// TS: InkDaemon.broadcastSessionUpdated: the session as held now (else as given) to every client in scope.
    pub fn broadcast_session_updated(&self, session: &Ordered<SessionRecord>) {
        let held = self.sessions.borrow().sessions.get(&session.id).cloned();
        let session = held.unwrap_or_else(|| session.clone());
        for client in self.client_list() {
            if self.in_client_scope(&session, &client.subscription) {
                let message = ServerMessage::SessionUpdated(Box::new(SessionUpdated {
                    session: session.clone(),
                }));
                send_message(&client.handle, &message);
            }
        }
    }

    /// TS: InkDaemon.broadcastSessionRemoved.
    pub fn broadcast_session_removed(&self, session: &SessionRecord) {
        for client in self.client_list() {
            if self.in_client_scope(session, &client.subscription) {
                let message = ServerMessage::SessionRemoved(SessionRef {
                    session_id: session.id.clone(),
                });
                send_message(&client.handle, &message);
            }
        }
    }

    /// TS: handleRequest `ping`: `{ok: true, version: PROTOCOL_VERSION, home, channel}`.
    pub fn handle_ping(&self) -> Result<PingResult> {
        Ok(PingResult {
            ok: true,
            version: Some(PROTOCOL_VERSION),
            home: Some(self.paths.config_dir.to_string_lossy().into_owned()),
            channel: Some(std::env::var("DECKHAND_CHANNEL").unwrap_or_else(|_| "stable".to_owned())),
        })
    }

    /// TS: handleRequest `subscribe`: records the client's checkout and repository key, learns every session's
    /// repository key, and answers the sessions in the client's scope in sidebar order.
    pub async fn handle_subscribe(
        &self,
        conn: &Connection,
        request: &Subscribe,
    ) -> Result<Vec<Ordered<SessionRecord>>> {
        // `client.repoRoot = message.repoRoot`: without a root (absent, empty) nothing is in scope and the answer is
        // empty. A root that is not text is not kept (TS keeps it and fails in its path API on every session, which
        // is the answer here too when there is one).
        let repo_root = request.repo_root.known().cloned();
        let mut subscription = self.get_client(&conn.client, |client| {
            client.repo_root.clone_from(&repo_root);
            client.clone()
        });
        let repo_key = match repo_root.as_deref() {
            Some(root) => self.tasks_key_for(root).await.ok(),
            None => None,
        };
        // TS sets the key on the subscription object it holds: a client gone meanwhile is not registered again.
        subscription.repo_key.clone_from(&repo_key);
        if let Some(client) = self.server.borrow_mut().clients.get_mut(&conn.client.id) {
            client.subscription.repo_key = repo_key;
        }
        // Every checkout's key known first, so the answer has the whole repository's sessions.
        let mut roots: Vec<String> = Vec::new();
        for session in self.session_list() {
            let root = path::resolve(&session.repo_root);
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        futures::future::join_all(roots.iter().map(|root| self.learn_repo_key(root, false))).await;
        let current = self
            .server
            .borrow()
            .clients
            .get(&conn.client.id)
            .map(|client| client.subscription.clone());
        if let Some(current) = current {
            subscription = current;
        }
        if let Loose::Other(root) = &request.repo_root
            && js_truthy(root)
            && !self.session_list().is_empty()
        {
            return Err(DhError::msg(format!(
                "The \"paths[0]\" argument must be of type string. {}",
                node_received(Some(root))
            )));
        }
        let in_scope: Vec<Ordered<SessionRecord>> = self
            .session_list()
            .into_iter()
            .filter(|session| self.in_client_scope(session, &subscription))
            .collect();
        Ok(sort_sessions_for_sidebar(&in_scope).into_iter().cloned().collect())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::daemon::{DaemonOptions, DaemonPaths};
    use serde_json::json;
    use std::future::Future;
    use tokio::io::{AsyncBufReadExt, BufReader};

    /// Every test in this crate that sets or reads a process-wide environment variable (DECKHAND_HOME, PATH,
    /// DECKHAND_CHANNEL, ...) holds this, so tests in different modules never see each other's values under the
    /// parallel test runner.
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Takes [`ENV`] (a test that panicked while holding it has already restored the environment in its guard's drop).
    pub(crate) fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn every_module_shares_the_one_environment_lock() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(&src).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if name == "server.rs" || path.extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            if text.contains("Mutex<()> = Mutex::new(())") {
                offenders.push(format!("{name}: declares its own environment lock"));
            }
            let mutates = text.contains("std::env::set_var") || text.contains("std::env::remove_var");
            if mutates && !text.contains("env_lock") {
                offenders.push(format!(
                    "{name}: changes the environment without server::tests::env_lock"
                ));
            }
        }
        offenders.sort();
        assert!(offenders.is_empty(), "{offenders:#?}");
    }

    pub(crate) fn local<F: Future>(future: F) -> F::Output {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        tokio::task::LocalSet::new().block_on(&runtime, future)
    }

    /// A daemon whose files live in `dir`.
    pub(crate) fn test_daemon(dir: &std::path::Path) -> Daemon {
        Daemon::with_paths(
            DaemonOptions::default(),
            DaemonPaths {
                config_dir: dir.to_path_buf(),
                socket: dir.join("daemon.sock"),
                pid_file: dir.join("daemon.pid"),
                log_file: dir.join("daemon.log"),
                worker_dir: dir.join("workers"),
            },
        )
    }

    /// A client whose lines can be read back.
    pub(crate) fn test_client(id: u64) -> (ClientHandle, mpsc::UnboundedReceiver<String>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (ClientHandle::new(ClientId(id), tx), rx)
    }

    fn lines(rx: &mut mpsc::UnboundedReceiver<String>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(line) = rx.try_recv() {
            out.push(line);
        }
        out
    }

    pub(crate) fn session(id: &str, repo_root: &str, cwd: &str) -> Ordered<SessionRecord> {
        serde_json::from_value(json!({
            "id": id,
            "title": id,
            "program": "claude",
            "command": "claude",
            "cwd": cwd,
            "repoRoot": repo_root,
            "status": "running",
            "createdAt": "2026-01-01T00:00:00.000Z",
            "updatedAt": "2026-01-01T00:00:00.000Z",
        }))
        .unwrap()
    }

    fn subscribed(repo_root: Option<&str>, repo_key: Option<&str>) -> ClientSubscription {
        ClientSubscription {
            repo_root: repo_root.map(str::to_owned),
            repo_key: repo_key.map(str::to_owned),
            ..ClientSubscription::default()
        }
    }

    #[test]
    fn sizes_are_clamped_like_the_ts() {
        assert_eq!(clamp_size(Some(0), 80), 1);
        assert_eq!(clamp_size(Some(-5), 80), 1);
        assert_eq!(clamp_size(Some(120), 80), 120);
        // Not a finite number (`'x'`, `null`, absent): the fallback.
        assert_eq!(clamp_size(None, 80), 80);
        assert_eq!(clamp_non_negative(Some(-1)), 0);
        assert_eq!(clamp_non_negative(Some(7)), 7);
        assert_eq!(clamp_non_negative(None), 0);
        let defaults = ClientSubscription::default();
        assert_eq!((defaults.preview_cols, defaults.preview_rows), (80, 24));
        assert_eq!((defaults.action_cols, defaults.action_rows), (80, 24));
        assert_eq!(defaults.preview_scroll_offset, 0);
        assert!(defaults.repo_root.is_none() && defaults.watched_changes_session_id.is_none());
    }

    #[test]
    fn requests_are_typed_leniently() {
        let parsed = parse_request(json!({"type": "ping", "requestId": "r", "extra": 1}));
        assert!(
            matches!(&parsed, ParsedRequest::Request { request, reply_to } if request.type_name() == "ping" && reply_to.as_deref() == Some("r"))
        );
        assert_eq!(
            parse_request(json!({"type": "nope", "requestId": "r"})),
            ParsedRequest::Ignored
        );
        // Fractional sizes are floored (TS clampSize) instead of refused.
        let ParsedRequest::Request { request, .. } =
            parse_request(json!({"type": "resize", "sessionId": "s", "cols": 80.7, "rows": 24}))
        else {
            panic!("resize not parsed")
        };
        assert_eq!(
            serde_json::to_value(&*request).unwrap(),
            json!({"type": "resize", "sessionId": "s", "cols": 80, "rows": 24})
        );
        // TS reads fields without validating them: a known type is handled whatever it carries, and its failure is
        // reported wherever a `requestId` is (an input's too).
        for body in [
            json!({"type": "subscribe", "requestId": "r"}),
            json!({"type": "update-agent", "requestId": "r", "program": "nope"}),
            json!({"type": "input", "requestId": "r", "sessionId": 5}),
        ] {
            let ParsedRequest::Request { reply_to, .. } = parse_request(body.clone()) else {
                panic!("not handled: {body}")
            };
            assert_eq!(reply_to.as_deref(), Some("r"));
        }
        assert!(matches!(
            parse_request(json!({"type": "input", "sessionId": "s"})),
            ParsedRequest::Request { reply_to: None, .. }
        ));
        // A requestId of another type is answered as its string.
        assert!(matches!(
            parse_request(json!({"type": "ping", "requestId": 5})),
            ParsedRequest::Request { reply_to: Some(id), .. } if id == "5"
        ));
    }

    /// Sends `line` on a fresh connection and reads the first reply line.
    async fn first_reply(daemon: &Daemon, line: &str) -> Value {
        let stream = UnixStream::connect(&daemon.paths.socket).await.unwrap();
        let (read, mut write) = stream.into_split();
        let mut read = BufReader::new(read);
        write.write_all(format!("{line}\n").as_bytes()).await.unwrap();
        let mut reply = String::new();
        read.read_line(&mut reply).await.unwrap();
        serde_json::from_str(&reply).unwrap()
    }

    #[test]
    fn fields_are_checked_where_and_as_ts_checks_them() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let daemon = test_daemon(dir.path());
            daemon.listen().await.unwrap();
            let error = |reply: Value| reply["error"].as_str().unwrap_or_default().to_owned();
            // updateAgent: `Unknown agent ${String(program)}`.
            let reply = first_reply(&daemon, r#"{"type":"update-agent","requestId":"a","program":"nope"}"#).await;
            assert_eq!(
                (reply["requestId"].clone(), error(reply)),
                (json!("a"), "Unknown agent nope".to_owned())
            );
            let reply = first_reply(&daemon, r#"{"type":"update-agent","requestId":"a"}"#).await;
            assert_eq!(error(reply), "Unknown agent undefined");
            // No repository to subscribe to: nothing is in scope.
            let reply = first_reply(&daemon, r#"{"type":"subscribe","requestId":"s"}"#).await;
            assert_eq!(
                reply,
                json!({"type": "response", "requestId": "s", "ok": true, "data": []})
            );
            // tasksKeyFor first (`Invalid repository path`), then clientTaskOp.
            let reply = first_reply(
                &daemon,
                r#"{"type":"task-op","requestId":"t","cwd":5,"op":{"type":"nope"}}"#,
            )
            .await;
            assert_eq!(error(reply), "Invalid repository path");
            // A session's own lookups: a missing ID names none.
            let reply = first_reply(&daemon, r#"{"type":"promote-note-item","requestId":"p","line":1.5}"#).await;
            assert_eq!(error(reply), "Session does not exist");
            let reply = first_reply(&daemon, r#"{"type":"reorder-session","requestId":"o"}"#).await;
            assert_eq!(error(reply), "session does not exist");
            // Without a requestId a success is answered without one (TS `response(undefined, ...)`).
            let reply = first_reply(&daemon, r#"{"type":"list"}"#).await;
            assert_eq!(reply, json!({"type": "response", "ok": true, "data": []}));
            // An input's failure goes to the requestId it carries.
            let reply = first_reply(&daemon, r#"{"type":"input","requestId":"i","sessionId":"x"}"#).await;
            assert_eq!(error(reply), "Cannot read properties of undefined (reading 'replace')");
            // A detach without a session echoes none.
            let reply = first_reply(&daemon, r#"{"type":"detach"}"#).await;
            assert_eq!(reply, json!({"type": "detached"}));
            // Watching nothing (the sizes are not numbers: the client's are kept).
            let reply = first_reply(&daemon, r#"{"type":"watch-preview","requestId":"w","cols":"x"}"#).await;
            assert_eq!(reply["data"], json!({"content": "", "live": false}));
        });
    }

    #[test]
    fn messages_are_lines_until_the_socket_is_destroyed() {
        let (client, mut rx) = test_client(1);
        let message = failure("r1", "boom");
        send_message(&client, &message);
        assert_eq!(
            lines(&mut rx),
            vec!["{\"type\":\"response\",\"requestId\":\"r1\",\"ok\":false,\"error\":\"boom\"}\n"]
        );
        assert!(!client.is_destroyed());
        client.clone().destroy();
        assert!(client.is_destroyed());
        send_message(&client, &message);
        // The close marker only.
        assert_eq!(lines(&mut rx), vec![String::new()]);
        client.destroy();
        assert!(lines(&mut rx).is_empty());
        let (client, rx) = test_client(2);
        drop(rx);
        assert!(client.is_destroyed());
    }

    #[test]
    fn get_client_creates_the_default_subscription() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = test_daemon(dir.path());
        let (client, _rx) = test_client(9);
        let cols = daemon.get_client(&client, |subscription| {
            subscription.preview_cols = 100;
            subscription.terminal_cols
        });
        assert_eq!(cols, DEFAULT_PREVIEW_COLS);
        assert_eq!(
            daemon.get_client(&client, |subscription| subscription.preview_cols),
            100
        );
        assert_eq!(daemon.server.borrow().clients.len(), 1);
    }

    #[test]
    fn scope_is_the_checkout_then_the_repository_key() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = test_daemon(dir.path());
        let main = session("a", "/repo", "/repo");
        let linked = session("b", "/wt/feature", "/wt/feature");
        let other = session("c", "/other", "/other");
        assert!(!daemon.in_client_scope(&main, &subscribed(None, None)));
        // An empty root is falsy in TS: nothing is in scope, not even through a repository key.
        daemon
            .server
            .borrow_mut()
            .repo_keys
            .insert("/repo".into(), "key-0".into());
        assert!(!daemon.in_client_scope(&main, &subscribed(Some(""), Some("key-0"))));
        assert!(daemon.in_client_scope(&main, &subscribed(Some("/repo/"), None)));
        assert!(!daemon.in_client_scope(&linked, &subscribed(Some("/repo"), None)));
        {
            let mut server = daemon.server.borrow_mut();
            server.repo_keys.insert("/wt/feature".into(), "key-1".into());
            server.repo_key_misses.insert("/other".into());
        }
        assert!(daemon.in_client_scope(&linked, &subscribed(Some("/repo"), Some("key-1"))));
        assert!(!daemon.in_client_scope(&linked, &subscribed(Some("/repo"), Some("key-2"))));
        assert!(!daemon.in_client_scope(&other, &subscribed(Some("/repo"), Some("key-1"))));
    }

    #[test]
    fn broadcasts_reach_the_clients_in_scope_with_the_session_as_held() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = test_daemon(dir.path());
        let (near, mut near_rx) = test_client(1);
        let (far, mut far_rx) = test_client(2);
        let (unsubscribed, mut unsubscribed_rx) = test_client(3);
        daemon.get_client(&near, |client| client.repo_root = Some("/repo".into()));
        daemon.get_client(&far, |client| client.repo_root = Some("/elsewhere".into()));
        daemon.get_client(&unsubscribed, |_| ());
        let mut held = session("a", "/repo", "/repo/sub");
        held.title = "held".into();
        daemon.sessions.borrow_mut().sessions.insert("a".into(), held);
        daemon.broadcast_session_updated(&session("a", "/repo", "/repo/sub"));
        let sent = lines(&mut near_rx);
        assert_eq!(sent.len(), 1);
        let message: Value = serde_json::from_str(&sent[0]).unwrap();
        assert_eq!(message["type"], "session-updated");
        assert_eq!(message["session"]["title"], "held");
        assert!(lines(&mut far_rx).is_empty());
        assert!(lines(&mut unsubscribed_rx).is_empty());

        daemon.broadcast_session_removed(&session("gone", "/repo", "/repo"));
        assert_eq!(
            lines(&mut near_rx),
            vec!["{\"type\":\"session-removed\",\"sessionId\":\"gone\"}\n"]
        );
        assert!(lines(&mut far_rx).is_empty());
    }

    #[test]
    fn ping_names_the_protocol_home_and_channel() {
        // The ping reads DECKHAND_CHANNEL, which lifecycle's tests change.
        let _env = env_lock();
        let dir = tempfile::tempdir().unwrap();
        let daemon = test_daemon(dir.path());
        let ping = daemon.handle_ping().unwrap();
        assert!(ping.ok);
        assert_eq!(ping.version, Some(PROTOCOL_VERSION));
        assert_eq!(ping.home.as_deref(), Some(dir.path().to_str().unwrap()));
        let expected = std::env::var("DECKHAND_CHANNEL").unwrap_or_else(|_| "stable".into());
        assert_eq!(ping.channel, Some(expected));
    }

    #[test]
    fn socket_errors_read_like_node() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let daemon = test_daemon(dir.path());
            // Nothing to remove.
            daemon.prepare_socket().await.unwrap();
            std::fs::create_dir(&daemon.paths.socket).unwrap();
            let error = daemon.prepare_socket().await.unwrap_err().to_string();
            assert!(
                error.ends_with(&format!(", unlink '{}'", daemon.paths.socket.display())),
                "{error}"
            );
            std::fs::remove_dir(&daemon.paths.socket).unwrap();
            daemon.listen().await.unwrap();
            let again = test_daemon(dir.path());
            assert_eq!(
                again.listen().await.unwrap_err().to_string(),
                format!(
                    "listen EADDRINUSE: address already in use {}",
                    daemon.paths.socket.display()
                )
            );
        });
    }

    #[test]
    fn connections_answer_requests_and_reject_bad_lines() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let daemon = test_daemon(dir.path());
            daemon.listen().await.unwrap();
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&daemon.paths.socket).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "socket created under a private umask");

            let stream = UnixStream::connect(&daemon.paths.socket).await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut read = BufReader::new(read);
            let mut line = String::new();

            // Split across writes; unknown types are ignored; blank lines skipped.
            write
                .write_all(b"\n{\"type\":\"what\",\"requestId\":\"x\"}\n{\"type\":\"pi")
                .await
                .unwrap();
            write.write_all(b"ng\",\"requestId\":\"r1\"}\n").await.unwrap();
            read.read_line(&mut line).await.unwrap();
            let answer: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(answer["requestId"], "r1");
            assert_eq!(answer["ok"], true);
            assert_eq!(answer["data"]["version"], PROTOCOL_VERSION);
            assert_eq!(daemon.server.borrow().clients.len(), 1);

            // A known type with fields TS cannot use is handled as TS handles it: that request fails, the
            // connection carries on.
            line.clear();
            write
                .write_all(b"{\"type\":\"update-agent\",\"requestId\":\"r2\",\"program\":[]}\n")
                .await
                .unwrap();
            read.read_line(&mut line).await.unwrap();
            let answer: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(answer["requestId"], "r2");
            assert_eq!(answer["ok"], false);
            assert_eq!(answer["error"], "Unknown agent ");

            // An oversized line is answered by its request ID and the connection carries on.
            line.clear();
            let big = format!(
                "{{\"type\":\"save-note\",\"requestId\":\"big-1\",\"text\":\"{}\"}}\n",
                "x".repeat(MAX_REQUEST_LINE)
            );
            write.write_all(big.as_bytes()).await.unwrap();
            read.read_line(&mut line).await.unwrap();
            assert_eq!(
                line,
                "{\"type\":\"response\",\"requestId\":\"big-1\",\"ok\":false,\"error\":\"Request exceeds 256000 characters\"}\n"
            );
            line.clear();
            write
                .write_all(b"{\"type\":\"ping\",\"requestId\":\"r3\"}\n")
                .await
                .unwrap();
            read.read_line(&mut line).await.unwrap();
            assert!(line.contains("\"requestId\":\"r3\""));

            // A malformed line closes the connection.
            line.clear();
            write
                .write_all(b"{not json}\n{\"type\":\"ping\",\"requestId\":\"r4\"}\n")
                .await
                .unwrap();
            assert_eq!(read.read_line(&mut line).await.unwrap(), 0);
            assert!(line.is_empty());
        });
    }

    /// Reads lines until the connection closes.
    async fn read_until_closed(read: &mut BufReader<tokio::net::unix::OwnedReadHalf>) -> Vec<String> {
        let mut out = Vec::new();
        loop {
            let mut line = String::new();
            if read.read_line(&mut line).await.unwrap() == 0 {
                return out;
            }
            out.push(line);
        }
    }

    #[test]
    fn earlier_requests_in_a_chunk_are_answered_before_a_malformed_line_closes_it() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let daemon = test_daemon(dir.path());
            daemon.listen().await.unwrap();
            let stream = UnixStream::connect(&daemon.paths.socket).await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut read = BufReader::new(read);
            // One write: TS runs r1's handler (which answers synchronously) before it parses the malformed line.
            write
                .write_all(
                    b"{\"type\":\"ping\",\"requestId\":\"r1\"}\n{bad}\n{\"type\":\"ping\",\"requestId\":\"r2\"}\n",
                )
                .await
                .unwrap();
            let got = read_until_closed(&mut read).await;
            assert_eq!(got.len(), 1, "{got:?}");
            assert!(got[0].contains("\"requestId\":\"r1\""), "{got:?}");
        });
    }

    #[test]
    fn replies_keep_line_order_within_a_chunk() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let daemon = test_daemon(dir.path());
            daemon.listen().await.unwrap();
            let stream = UnixStream::connect(&daemon.paths.socket).await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut read = BufReader::new(read);
            let lines = format!(
                "{{\"type\":\"ping\",\"requestId\":\"r1\"}}\n{{\"type\":\"save-note\",\"requestId\":\"big-1\",\"text\":\"{}\"}}\n{{\"type\":\"ping\",\"requestId\":\"r2\"}}\n{{\"type\":\"subscribe\",\"requestId\":\"bad-1\"}}\n",
                "x".repeat(MAX_REQUEST_LINE)
            );
            write.write_all(lines.as_bytes()).await.unwrap();
            let mut ids = Vec::new();
            for _ in 0..4 {
                let mut line = String::new();
                read.read_line(&mut line).await.unwrap();
                let answer: Value = serde_json::from_str(&line).unwrap();
                ids.push(answer["requestId"].as_str().unwrap().to_owned());
            }
            assert_eq!(ids, ["r1", "big-1", "r2", "bad-1"]);
        });
    }

    #[test]
    fn a_client_closed_by_a_malformed_line_is_forgotten() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let daemon = test_daemon(dir.path());
            daemon.listen().await.unwrap();
            let stream = UnixStream::connect(&daemon.paths.socket).await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut read = BufReader::new(read);
            write
                .write_all(b"{\"type\":\"subscribe\",\"requestId\":\"r\",\"repoRoot\":\"/x\"}\n{bad}\n")
                .await
                .unwrap();
            let _ = read_until_closed(&mut read).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
            let left: Vec<_> = daemon
                .server
                .borrow()
                .clients
                .iter()
                .map(|(id, client)| (*id, client.subscription.repo_root.clone()))
                .collect();
            assert!(left.is_empty(), "clients left after the connection closed: {left:?}");
        });
    }
}
