//! Port of the NDJSON client in src/client.ts (`request`, `LiveClient`), over `serde_json::Value`.
//!
//! Messages are untyped on purpose: a black-box test asserts on the wire shape, not on a Rust model of it.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The TS `request` timeout.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Default)]
struct Shared {
    responses: HashMap<String, Value>,
    /// Every non-response message, in arrival order (`session-updated`, `agent-versions-updated`, ...).
    events: Vec<Value>,
    closed: bool,
}

type SharedState = Arc<(Mutex<Shared>, Condvar)>;

/// One persistent connection: a reader thread files responses by `requestId` and keeps the events.
pub struct Client {
    stream: UnixStream,
    shared: SharedState,
}

impl Client {
    pub fn connect(socket: &Path) -> std::io::Result<Client> {
        let stream = UnixStream::connect(socket)?;
        let reader = BufReader::new(stream.try_clone()?);
        let shared: SharedState = Arc::default();
        let thread_shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            for line in reader.split(b'\n') {
                let Ok(line) = line else { break };
                let line = String::from_utf8_lossy(&line);
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let Ok(message) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                let (lock, wake) = &*thread_shared;
                let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
                match message.get("type").and_then(Value::as_str) {
                    Some("response") => {
                        let id = message["requestId"].as_str().unwrap_or_default().to_string();
                        state.responses.insert(id, message);
                    }
                    _ => state.events.push(message),
                }
                wake.notify_all();
            }
            let (lock, wake) = &*thread_shared;
            lock.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
            wake.notify_all();
        });
        Ok(Client { stream, shared })
    }

    /// Sends `message` with a fresh `requestId` and returns the response `data`, or `Err(error text)` when `ok` is false.
    pub fn request(&mut self, message: Value) -> Result<Value, String> {
        self.request_timeout(message, REQUEST_TIMEOUT)
    }

    pub fn request_timeout(&mut self, mut message: Value, timeout: Duration) -> Result<Value, String> {
        let id = uuid::Uuid::new_v4().to_string();
        message["requestId"] = json!(id);
        let mut line = message.to_string();
        line.push('\n');
        self.stream.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
        let deadline = Instant::now() + timeout;
        let (lock, wake) = &*self.shared;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(response) = state.responses.remove(&id) {
                if response["ok"].as_bool() == Some(true) {
                    return Ok(response.get("data").cloned().unwrap_or(Value::Null));
                }
                return Err(response["error"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("daemon request failed")
                    .to_string());
            }
            if state.closed {
                return Err("daemon connection closed before response".into());
            }
            let now = Instant::now();
            if now >= deadline {
                return Err("daemon request timed out".into());
            }
            state = wake
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// `subscribe`: returns the repo's sessions; afterwards the daemon pushes events for it.
    pub fn subscribe(&mut self, repo_root: &str) -> Result<Value, String> {
        self.request(json!({"type": "subscribe", "repoRoot": repo_root}))
    }

    /// A snapshot of every event received so far.
    pub fn events(&self) -> Vec<Value> {
        self.shared.0.lock().unwrap_or_else(|e| e.into_inner()).events.clone()
    }

    /// Events of one `type`.
    pub fn events_of(&self, kind: &str) -> Vec<Value> {
        self.events()
            .into_iter()
            .filter(|e| e["type"].as_str() == Some(kind))
            .collect()
    }

    pub fn is_closed(&self) -> bool {
        self.shared.0.lock().unwrap_or_else(|e| e.into_inner()).closed
    }

    pub fn close(&self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.close();
    }
}

/// The TS `request`: a one-shot connection per call.
pub fn request(socket: &Path, message: Value) -> Result<Value, String> {
    Client::connect(socket).map_err(|e| e.to_string())?.request(message)
}
