//! The daemon's startup, recovery, shutdown and persistence.
//!
//! Startup recovers from the previous daemon: sessions it left running are marked exited (their PTYs died with it),
//! notes kept in state.json by older versions move to their files, worktree sessions' own notes merge into their
//! worktree's note and follow-up tasks become note items; then the socket is opened. Shutdown (signal, `shutdown`
//! request, fatal error) stops every worker, persists once more and removes the socket and pid file.

use std::os::unix::fs::PermissionsExt;
use std::sync::Mutex;
use std::time::Duration;

use deckhand_core::error::{DhError, Result, errno_code};
use deckhand_core::jsstr::{self, js_trim};
use deckhand_core::worktree_records::stored_session;
use deckhand_protocol::Ordered;
use deckhand_protocol::data::{OkResult, SessionRecord};
use deckhand_protocol::order::KeyOrder;
use deckhand_protocol::records::StateFile;
use deckhand_sys::async_util::{Coalescer, Interval, SharedTask, Timer, spawn};
use deckhand_sys::{paths, storage};
use nix::sys::signal::Signal;
use serde_json::{Map, Value};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;

use crate::agents::AGENT_VERSIONS_TICK;
use crate::daemon::Daemon;
use crate::merge::MERGE_DETECT_INTERVAL;

/// TS cleanup: how long a session worker gets to exit after `kill` before SIGKILL (process.exit follows cleanup, so
/// the fallback must not be deferred).
const CLEANUP_KILL_GRACE: Duration = Duration::from_millis(500);

/// TS handleRequest `shutdown`: the delay before cleaning up, so the answer goes out first.
const SHUTDOWN_DELAY: Duration = Duration::from_millis(25);

/// Where the panic hook reports a panic (TS `process.on('uncaughtException')`): the daemon's handler task.
static PANICS: Mutex<Option<mpsc::UnboundedSender<String>>> = Mutex::new(None);

/// The lifecycle's part of the daemon state.
#[derive(Default)]
pub struct LifecycleState {
    /// TS `shuttingDown`.
    pub shutting_down: bool,
    /// TS `persistInFlight` / `persistQueued`: at most one state.json write in flight plus one queued, shared by
    /// every caller arriving before the queued one snapshots the sessions. Created by the first [`Daemon::persist`].
    pub persist: Option<Coalescer<Result<()>>>,
}

/// Drops `names` from a recorded key order (the keys a rest destructuring leaves out).
fn without_keys(order: &KeyOrder, names: &[&str]) -> KeyOrder {
    match order {
        KeyOrder::Object(keys) => KeyOrder::Object(
            keys.iter()
                .filter(|(key, _)| !names.contains(&key.as_str()))
                .cloned()
                .collect(),
        ),
        other => other.clone(),
    }
}

/// TS: daemon.ts persistedSession: the session as state.json stores it: no projected markers or notes, and no
/// attention reason (what the agent asked may quote code; kept in memory only).
pub fn persisted_session(session: &Ordered<SessionRecord>) -> Ordered<SessionRecord> {
    let stored = stored_session(session);
    // `{notes, notesFile, sharedNotes, ...stored}`: the rest keeps its order.
    let order = without_keys(stored.key_order(), &["notes", "notesFile", "sharedNotes"]);
    let mut record = stored.into_inner();
    record.notes = None;
    record.notes_file = None;
    record.shared_notes = None;
    // What the agent asks or said (attention.reason) may quote commands or code: kept in memory, never written.
    // `{...stored, attention}` keeps attention's place; `{reason, ...attention}` the order of its other keys.
    if let Some(attention) = record.attention.as_mut() {
        attention.reason = None;
    }
    Ordered::with_order(record, order)
}

/// TS: InkDaemon.isProcessAlive (`kill(pid, 0)` succeeds).
pub fn is_process_alive(pid: i32) -> bool {
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
}

/// `Number.parseInt(text, 10)` (leading whitespace, an optional sign, then decimal digits); `None` for NaN.
fn parse_int(text: &str) -> Option<f64> {
    let text = js_trim(text);
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, text.get(1..)?),
        Some(b'+') => (false, text.get(1..)?),
        _ => (false, text),
    };
    let end = digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(digits.len());
    let value: f64 = digits.get(..end).filter(|digits| !digits.is_empty())?.parse().ok()?;
    Some(if negative { -value } else { value })
}

/// The pid a pid file names, when it is a pid `kill` can be asked about (TS process.kill throws for others).
fn pid_of(value: f64) -> Option<i32> {
    if value.is_finite() && value.fract() == 0.0 && value >= f64::from(i32::MIN) && value <= f64::from(i32::MAX) {
        // In range and integral: exact.
        #[allow(clippy::cast_possible_truncation)]
        Some(value as i32)
    } else {
        None
    }
}

/// This process's pid as a JS number.
fn own_pid() -> f64 {
    f64::from(std::process::id())
}

/// `fs.readFile(path, 'utf8')`.
async fn read_text(path: &std::path::Path) -> Result<String> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        // Node reads a directory without its path in the message.
        Err(error) if errno_code(&error) == Some("EISDIR") => Err(DhError::with_code(
            "EISDIR",
            "EISDIR: illegal operation on a directory, read",
        )),
        Err(error) => Err(DhError::io(&error, "open", &path.to_string_lossy())),
    }
}

/// A panic's message as the log shows it (TS logs `error.stack || error.message`).
fn panic_text(info: &std::panic::PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    let message = payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "Box<dyn Any>".to_owned());
    match info.location() {
        Some(location) => format!("panicked at {location}: {message}"),
        None => message,
    }
}

impl Daemon {
    /// TS: InkDaemon.start: config dir, pid-file check, crash recovery and migrations (see the module docs), socket
    /// (0600), pid file, signal handlers, the notes watcher, then the agent-version and merge-detection timers.
    pub async fn start(&self) -> Result<()> {
        storage::ensure_config_dir().await?;
        self.log(&format!("starting daemon pid={}", std::process::id())).await;
        self.assert_no_live_daemon_from_pid_file().await?;
        // TS ensureNodePtyReady (node-pty's spawn-helper permissions) has no counterpart: the PTY is native.
        // If this daemon is starting, any previously-running sessions belonged to a different daemon process and their
        // PTY handles are gone. Mark them exited as crash/restart recovery, not as normal frontend quit behavior.
        let stored = storage::mark_all_non_exited_sessions_exited().await?;
        {
            let mut worktrees = self.worktrees.borrow_mut();
            for record in &stored.worktrees {
                worktrees.records.insert(record.id.clone(), record.clone());
            }
        }
        // Notes live in files: ones older versions kept in state.json move there first (idempotent), then all are read.
        let notes = self.notes_tasks.borrow().notes.clone();
        let legacy_notes = match notes {
            Some(notes) => {
                let legacy: Vec<(String, Option<Value>)> = stored
                    .sessions
                    .iter()
                    .map(|session| (session.id.clone(), session.notes.clone().map(Value::String)))
                    .collect();
                let migrated = notes.migrate(&legacy).await?;
                notes.load().await?;
                migrated
            }
            None => false,
        };
        // Before the sessions project their notes, so they show the merged worktree notes.
        self.merge_worktree_session_notes(&stored.sessions).await?;
        // Before anything writes a task list (it would drop the old follow-up bookkeeping), and before the sessions
        // project their notes.
        self.migrate_follow_up_tasks(&stored.sessions).await;
        for session in stored.sessions {
            self.set_session(session);
        }
        if legacy_notes {
            self.persist().await?;
        }

        self.prepare_socket().await?;
        self.listen().await?;
        let socket = self.paths.socket.clone();
        tokio::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
            .await
            .map_err(|error| DhError::io(&error, "chmod", &socket.to_string_lossy()))?;
        self.write_pid_file().await?;
        self.setup_process_handlers();
        let notes = self.notes_tasks.borrow().notes.clone();
        if let Some(notes) = notes {
            notes.watch();
        }
        self.log(&format!("daemon ready socket={}", socket.to_string_lossy()))
            .await;
        // Latest agent releases: looked up in the background now, then again once they are LATEST_MAX_AGE_MS old.
        let daemon = self.clone();
        drop(spawn(async move { daemon.refresh_agent_versions(false).await }));
        let weak = self.downgrade();
        let timer = Interval::every(AGENT_VERSIONS_TICK, move || {
            if let Some(daemon) = weak.upgrade() {
                drop(spawn(async move { daemon.refresh_agent_versions(false).await }));
            }
        });
        self.agents.borrow_mut().timer = Some(timer);
        let detection = self.detect_merges(None);
        drop(spawn(detection));
        let weak = self.downgrade();
        let timer = Interval::every(MERGE_DETECT_INTERVAL, move || {
            if let Some(daemon) = weak.upgrade() {
                drop(spawn(daemon.detect_merges(None)));
            }
        });
        self.merge.borrow_mut().timer = Some(timer);
        Ok(())
    }

    /// TS: InkDaemon.log: appends `[<ISO time>] daemon <message>` to the daemon log; failures are ignored.
    pub async fn log(&self, message: &str) {
        use tokio::io::AsyncWriteExt;
        let time = jsstr::date::to_iso_string(jsstr::date::now_ms()).unwrap_or_default();
        let line = format!("[{time}] daemon {message}\n");
        // Logging must never keep the daemon from starting or shutting down.
        let Ok(mut file) = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.paths.log_file)
            .await
        else {
            return;
        };
        // A tokio file hands writes to a blocking thread: flushing waits for the line to be written, as appendFile's
        // promise does (callers rely on the line being there once this returns).
        if file.write_all(line.as_bytes()).await.is_ok() {
            let _ = file.flush().await;
        }
    }

    /// TS: InkDaemon.assertNoLiveDaemonFromPidFile: `daemon already appears to be running as pid <pid>` when the pid
    /// file names another live process (a missing file is fine).
    pub async fn assert_no_live_daemon_from_pid_file(&self) -> Result<()> {
        let raw = match read_text(&self.paths.pid_file).await {
            Ok(raw) => raw,
            Err(error) if error.is_not_found() => return Ok(()),
            Err(error) => return Err(error),
        };
        let Some(pid) = parse_int(&raw) else {
            return Ok(());
        };
        if pid > 0.0
            && pid != own_pid()
            && let Some(live) = pid_of(pid).filter(|&pid| is_process_alive(pid))
        {
            return Err(DhError::msg(format!(
                "daemon already appears to be running as pid {live}"
            )));
        }
        Ok(())
    }

    /// TS: InkDaemon.writePidFile (`<pid>\n`).
    pub async fn write_pid_file(&self) -> Result<()> {
        let path = &self.paths.pid_file;
        tokio::fs::write(path, format!("{}\n", std::process::id()))
            .await
            .map_err(|error| DhError::io(&error, "open", &path.to_string_lossy()))
    }

    /// TS: InkDaemon.removePidFile: only when it still names this process; failures are ignored.
    pub async fn remove_pid_file(&self) {
        let Ok(raw) = read_text(&self.paths.pid_file).await else {
            return;
        };
        if parse_int(&raw) != Some(own_pid()) {
            return;
        }
        // Ignore pid cleanup failures.
        let _ = tokio::fs::remove_file(&self.paths.pid_file).await;
    }

    /// TS: InkDaemon.setupProcessHandlers: SIGINT/SIGTERM shut down (exit 0), SIGHUP is logged and ignored; a panic
    /// in a daemon task is logged, cleaned up after and exits 1 (TS uncaughtException/unhandledRejection).
    pub fn setup_process_handlers(&self) {
        for (kind, name) in [
            (SignalKind::interrupt(), "SIGINT"),
            (SignalKind::terminate(), "SIGTERM"),
        ] {
            let Ok(mut signals) = signal(kind) else { continue };
            let weak = self.downgrade();
            drop(spawn(async move {
                while signals.recv().await.is_some() {
                    let Some(daemon) = weak.upgrade() else { return };
                    // Each signal shuts down on its own, as each TS listener call does (a second one exits at once).
                    drop(spawn(async move {
                        daemon.log(&format!("received {name}; shutting down")).await;
                        daemon.cleanup().await;
                        std::process::exit(0);
                    }));
                }
            }));
        }
        if let Ok(mut hangups) = signal(SignalKind::hangup()) {
            let weak = self.downgrade();
            drop(spawn(async move {
                while hangups.recv().await.is_some() {
                    let Some(daemon) = weak.upgrade() else { return };
                    daemon.log_soon("received SIGHUP; keeping daemon alive".to_owned());
                }
            }));
        }
        // A panic in a task (tokio catches it there and the LocalSet goes on) is what an uncaught exception is in TS.
        let (sender, mut panics) = mpsc::unbounded_channel::<String>();
        if let Ok(mut slot) = PANICS.lock() {
            *slot = Some(sender);
        }
        std::panic::set_hook(Box::new(|info| {
            let text = panic_text(info);
            if let Ok(slot) = PANICS.lock()
                && let Some(sender) = slot.as_ref()
            {
                let _ = sender.send(text);
            }
        }));
        let weak = self.downgrade();
        drop(spawn(async move {
            while let Some(text) = panics.recv().await {
                let Some(daemon) = weak.upgrade() else { return };
                drop(spawn(async move {
                    daemon.log(&format!("uncaught exception: {text}")).await;
                    daemon.cleanup().await;
                    std::process::exit(1);
                }));
            }
        }));
    }

    /// TS: InkDaemon.cleanup (once): stops the timers, the notes watcher and setup processes, closes the clients,
    /// stops the Changes watches and workspace workers, kills the session workers (SIGKILL after 500 ms), persists,
    /// closes the server and removes the socket and pid file.
    pub async fn cleanup(&self) {
        if std::mem::replace(&mut self.lifecycle.borrow_mut().shutting_down, true) {
            return;
        }
        let agents_timer = self.agents.borrow_mut().timer.take();
        if let Some(mut timer) = agents_timer {
            timer.clear();
        }
        let merge_timer = self.merge.borrow_mut().timer.take();
        if let Some(mut timer) = merge_timer {
            timer.clear();
        }
        let notes = self.notes_tasks.borrow().notes.clone();
        if let Some(notes) = notes {
            notes.close();
        }
        let setups: Vec<_> = self.launch.borrow().setup_processes.values().copied().collect();
        for setup in setups {
            if let Some(pid) = setup.pid.filter(|&pid| pid != 0) {
                let _ = deckhand_sys::proc::kill_group(pid, Signal::SIGTERM);
            }
        }
        self.log("cleanup start").await;
        let clients: Vec<_> = std::mem::take(&mut self.server.borrow_mut().clients)
            .into_values()
            .map(|client| client.handle)
            .collect();
        for client in clients {
            client.destroy();
        }

        let watches: Vec<String> = self.changes.borrow().watches.keys().cloned().collect();
        for key in watches {
            self.stop_changes_watch(&key);
        }
        // Every Terminal, Git and Dev pane lives in a workspace worker. Promise.all: the first failure ends the
        // cleanup, the other retirements go on.
        let keys: Vec<String> = self.workspaces.borrow().workspaces.keys().cloned().collect();
        let retiring: Vec<_> = keys
            .into_iter()
            .map(|key| {
                let daemon = self.clone();
                spawn(async move { daemon.retire_workspace(&key).await })
            })
            .collect();
        if futures::future::try_join_all(retiring).await.is_err() {
            return;
        }

        // Detach workers first so their exit events are ignored, then wait briefly for each to exit before SIGKILL;
        // process.exit follows cleanup, so the fallback must not be deferred.
        let workers: Vec<_> = std::mem::take(&mut self.launch.borrow_mut().workers)
            .into_iter()
            .collect();
        let stopping = workers.into_iter().map(|(session_id, worker)| {
            let pid_file = self.paths.worker_dir.join(format!("{session_id}.pid"));
            async move {
                let channel = worker.channel.clone();
                channel.reject_pending("daemon shutting down");
                let already_exited = channel.has_process_exited();
                let deadline = tokio::time::Instant::now() + CLEANUP_KILL_GRACE;
                if channel.is_connected() {
                    let mut kill = Map::new();
                    kill.insert("type".to_owned(), Value::String("kill".to_owned()));
                    kill.insert("requestId".to_owned(), Value::String(uuid::Uuid::new_v4().to_string()));
                    kill.insert("force".to_owned(), Value::Bool(true));
                    // Fall through to direct worker termination when the send fails.
                    let _ = channel.send_message(&kill);
                }
                if !already_exited {
                    tokio::select! {
                        () = channel.wait_process_exit() => {}
                        () = tokio::time::sleep_until(deadline) => channel.kill(Signal::SIGKILL),
                    }
                }
                let _ = tokio::fs::remove_file(&pid_file).await;
            }
        });
        futures::future::join_all(stopping).await;
        if self.persist().await.is_err() {
            return;
        }

        let listener = self.server.borrow_mut().listener.take();
        if let Some(listener) = listener {
            listener.abort();
        }
        // Ignore socket cleanup failures.
        let _ = tokio::fs::remove_file(&self.paths.socket).await;
        self.remove_pid_file().await;
        self.log("cleanup complete").await;
    }

    /// state.json as persist writes it now: the sessions newest first (as stored), the worktree records.
    fn persisted_state(&self) -> StateFile {
        let sessions: Vec<Ordered<SessionRecord>> = self.sessions.borrow().sessions.values().cloned().collect();
        let worktrees = self.worktrees.borrow().records.values().cloned().collect();
        StateFile {
            sessions: storage::sort_sessions_newest_first(&sessions)
                .iter()
                .map(persisted_session)
                .collect(),
            worktrees,
        }
    }

    /// TS: InkDaemon.persist: state.json `{sessions (newest first, persistedSession), worktrees}`, coalesced (see
    /// [`LifecycleState::persist`]).
    pub fn persist(&self) -> SharedTask<Result<()>> {
        let coalescer = {
            let mut lifecycle = self.lifecycle.borrow_mut();
            lifecycle
                .persist
                .get_or_insert_with(|| {
                    let weak = self.downgrade();
                    Coalescer::new(move || {
                        // The state is read when the write starts, so every caller before it shares it.
                        let state = weak.upgrade().map(|daemon| daemon.persisted_state());
                        async move {
                            match state {
                                Some(state) => storage::save_state(&state).await,
                                None => Ok(()),
                            }
                        }
                    })
                })
                .clone()
        };
        coalescer.request()
    }

    /// TS: handleRequest `shutdown`: only on the dev channel (`This control is restricted to the isolated dev
    /// daemon`); answers `{ok: true}`, then cleans up and exits 0 after 25 ms.
    pub fn handle_shutdown(&self) -> Result<OkResult> {
        if !paths::is_dev_channel() {
            return Err(DhError::msg("This control is restricted to the isolated dev daemon"));
        }
        let daemon = self.clone();
        Timer::after(SHUTDOWN_DELAY, move || {
            drop(spawn(async move {
                daemon.cleanup().await;
                std::process::exit(0);
            }));
        })
        .detach();
        Ok(OkResult::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::{hello_line, local, session, test_client, test_daemon};
    use crate::server::{Client, ClientSubscription};
    use serde_json::json;
    use std::path::Path;
    use std::rc::Rc;
    use std::sync::MutexGuard;

    /// Environment variables set (or removed) while this lives.
    struct EnvVar {
        _guard: MutexGuard<'static, ()>,
        restore: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl EnvVar {
        fn set(vars: &[(&'static str, Option<&std::ffi::OsStr>)]) -> EnvVar {
            let guard = crate::server::tests::env_lock();
            let mut restore = Vec::new();
            for &(name, value) in vars {
                restore.push((name, std::env::var_os(name)));
                // SAFETY: tests touching the environment hold server::tests::env_lock.
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(name, value),
                        None => std::env::remove_var(name),
                    }
                }
            }
            EnvVar { _guard: guard, restore }
        }

        /// DECKHAND_HOME = `dir`.
        fn home(dir: &Path) -> EnvVar {
            EnvVar::set(&[(paths::HOME_ENV, Some(dir.as_os_str()))])
        }
    }

    impl Drop for EnvVar {
        fn drop(&mut self) {
            for (name, value) in self.restore.drain(..).rev() {
                // SAFETY: as in EnvVar::set (the environment lock is still held).
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(name, value),
                        None => std::env::remove_var(name),
                    }
                }
            }
        }
    }

    /// A process that is alive until it is stopped.
    fn live_child() -> std::process::Child {
        std::process::Command::new("/bin/sleep").arg("30").spawn().unwrap()
    }

    /// Kills and reaps `child`; its pid.
    fn stop(mut child: std::process::Child) -> i32 {
        let pid = i32::try_from(child.id()).unwrap();
        let _ = child.kill();
        let _ = child.wait();
        pid
    }

    /// The daemon log's messages (each line checked to be `[<ISO time>] daemon <message>`).
    fn log_lines(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("daemon.log"))
            .unwrap_or_default()
            .lines()
            .map(|line| {
                let (time, rest) = line.split_once("] daemon ").unwrap();
                let time = time.strip_prefix('[').unwrap();
                assert!(jsstr::date::parse(time).is_some(), "{line}");
                assert_eq!(time.len(), "2026-01-01T00:00:00.000Z".len(), "{line}");
                rest.to_owned()
            })
            .collect()
    }

    fn insert_session(daemon: &Daemon, record: Ordered<SessionRecord>) {
        daemon.sessions.borrow_mut().sessions.insert(record.id.clone(), record);
    }

    #[test]
    fn persisted_session_drops_projected_notes_and_the_attention_reason_in_place() {
        let held: Ordered<SessionRecord> = serde_json::from_value(json!({
            "id": "s1",
            "title": "one",
            "notes": "my notes",
            "program": "claude",
            "command": "claude",
            "cwd": "/r",
            "repoRoot": "/r",
            "status": "exited",
            "attention": {"state": "needs-input", "reason": {"kind": "question", "text": "rm -rf?"}, "event": "Notification", "at": "2026-01-01T00:00:00.000Z", "future": 1},
            "createdAt": "2026-01-01T00:00:00.000Z",
            "notesFile": {"path": "/h/notes/sessions/s1.md", "revision": "abc"},
            "updatedAt": "2026-01-01T00:00:00.000Z",
            "sharedNotes": {"kind": "repo", "id": "/r", "path": "/h/notes/repos/x.md", "text": "t", "revision": "r"},
            "unknownKey": [1, 2],
        }))
        .unwrap();
        let stored = persisted_session(&held);
        assert_eq!(
            serde_json::to_string(&stored).unwrap(),
            serde_json::to_string(&json!({
                "id": "s1",
                "title": "one",
                "program": "claude",
                "command": "claude",
                "cwd": "/r",
                "repoRoot": "/r",
                "status": "exited",
                "attention": {"state": "needs-input", "event": "Notification", "at": "2026-01-01T00:00:00.000Z", "future": 1},
                "createdAt": "2026-01-01T00:00:00.000Z",
                "updatedAt": "2026-01-01T00:00:00.000Z",
                "unknownKey": [1, 2],
            }))
            .unwrap()
        );
        // The held record is untouched.
        assert_eq!(held.notes.as_deref(), Some("my notes"));
        assert!(held.attention.as_ref().unwrap().reason.is_some());
    }

    #[test]
    fn persisted_session_drops_the_projected_worktree_markers() {
        let held: Ordered<SessionRecord> = serde_json::from_value(json!({
            "id": "s1",
            "title": "one",
            "program": "claude",
            "command": "claude",
            "cwd": "/r/wt",
            "repoRoot": "/r",
            "worktree": {"mode": "managed", "path": "/r/wt", "branch": "b", "id": "w1", "mergedAt": "2026-01-02T00:00:00.000Z", "mergeMode": "merge", "deletedAt": "2026-01-03T00:00:00.000Z"},
            "status": "exited",
            "createdAt": "2026-01-01T00:00:00.000Z",
            "updatedAt": "2026-01-01T00:00:00.000Z",
        }))
        .unwrap();
        let stored = serde_json::to_value(persisted_session(&held)).unwrap();
        assert_eq!(
            stored["worktree"],
            json!({"mode": "managed", "path": "/r/wt", "branch": "b", "id": "w1"})
        );
    }

    #[test]
    fn process_liveness_is_kill_zero() {
        assert!(is_process_alive(i32::try_from(std::process::id()).unwrap()));
        let child = live_child();
        assert!(is_process_alive(i32::try_from(child.id()).unwrap()));
        let pid = stop(child);
        assert!(!is_process_alive(pid));
    }

    #[test]
    fn pid_files_are_read_with_parse_int() {
        assert_eq!(parse_int("123\n"), Some(123.0));
        assert_eq!(parse_int("  42abc"), Some(42.0));
        assert_eq!(parse_int("+7"), Some(7.0));
        assert_eq!(parse_int("-5"), Some(-5.0));
        assert_eq!(parse_int("1e3"), Some(1.0));
        assert_eq!(parse_int("0x10"), Some(0.0));
        assert_eq!(parse_int(""), None);
        assert_eq!(parse_int("abc"), None);
        assert_eq!(parse_int("-"), None);
        assert_eq!(pid_of(12.0), Some(12));
        assert_eq!(pid_of(1e12), None);
        assert_eq!(pid_of(f64::INFINITY), None);
    }

    #[test]
    fn log_appends_timestamped_daemon_lines_and_ignores_failures() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = test_daemon(dir.path());
        local(async {
            daemon.log("one").await;
            daemon.log("two words").await;
        });
        assert_eq!(log_lines(dir.path()), ["one", "two words"]);
        // An unwritable log is ignored.
        let missing = test_daemon(&dir.path().join("missing/dir"));
        local(missing.log("lost"));
    }

    #[test]
    fn a_logged_line_is_in_the_file_when_log_returns() {
        // As `await fs.appendFile(...)`: callers (and tests reading the log) rely on the line being written. A FIFO
        // as the log makes the write wait for its reader, so returning early would show.
        let dir = tempfile::tempdir().unwrap();
        let daemon = test_daemon(dir.path());
        nix::unistd::mkfifo(&daemon.paths.log_file, nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();
        let fifo = daemon.paths.log_file.clone();
        let reader = std::thread::spawn(move || {
            use std::io::Read;
            let mut file = std::fs::File::open(fifo).unwrap();
            // Nothing is drained for a while: a write larger than the pipe's buffer cannot finish before.
            std::thread::sleep(Duration::from_millis(300));
            let mut text = String::new();
            file.read_to_string(&mut text).unwrap();
            text.len()
        });
        let long = "x".repeat(256 * 1024);
        // Timed inside the runtime: dropping it waits for its blocking pool, which would hide an early return.
        let elapsed = local(async {
            let started = std::time::Instant::now();
            daemon.log(&long).await;
            started.elapsed()
        });
        assert!(
            elapsed >= Duration::from_millis(250),
            "log returned before its line was written"
        );
        assert!(reader.join().unwrap() > long.len());
    }

    #[test]
    fn the_pid_file_names_this_process_and_is_removed_only_while_it_does() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = test_daemon(dir.path());
        let pid_file = dir.path().join("daemon.pid");
        local(async {
            daemon.write_pid_file().await.unwrap();
            assert_eq!(
                std::fs::read_to_string(&pid_file).unwrap(),
                format!("{}\n", std::process::id())
            );
            // Our own pid is not another daemon.
            daemon.assert_no_live_daemon_from_pid_file().await.unwrap();
            daemon.remove_pid_file().await;
            assert!(!pid_file.exists());
            // Missing: nothing to do.
            daemon.remove_pid_file().await;
            std::fs::write(&pid_file, "1\n").unwrap();
            daemon.remove_pid_file().await;
            assert_eq!(std::fs::read_to_string(&pid_file).unwrap(), "1\n");
        });
    }

    #[test]
    fn a_live_daemon_named_by_the_pid_file_refuses_startup() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = test_daemon(dir.path());
        let pid_file = dir.path().join("daemon.pid");
        local(async {
            // No pid file.
            daemon.assert_no_live_daemon_from_pid_file().await.unwrap();
            for harmless in ["", "garbage", "0", "-4", "99999999999999"] {
                std::fs::write(&pid_file, harmless).unwrap();
                daemon.assert_no_live_daemon_from_pid_file().await.unwrap();
            }
            let child = live_child();
            std::fs::write(&pid_file, format!(" {}\n", child.id())).unwrap();
            let error = daemon.assert_no_live_daemon_from_pid_file().await.unwrap_err();
            assert_eq!(
                error.to_string(),
                format!("daemon already appears to be running as pid {}", child.id())
            );
            // A pid nobody uses any more.
            stop(child);
            daemon.assert_no_live_daemon_from_pid_file().await.unwrap();
            // Unreadable: the error is kept.
            std::fs::remove_file(&pid_file).unwrap();
            std::fs::create_dir(&pid_file).unwrap();
            let error = daemon.assert_no_live_daemon_from_pid_file().await.unwrap_err();
            assert_eq!(error.to_string(), "EISDIR: illegal operation on a directory, read");
        });
    }

    #[test]
    fn start_stops_at_a_live_daemons_pid_file() {
        let dir = tempfile::tempdir().unwrap();
        let _home = EnvVar::home(dir.path());
        let daemon = test_daemon(dir.path());
        let child = live_child();
        std::fs::write(dir.path().join("daemon.pid"), format!("{}\n", child.id())).unwrap();
        let error = local(daemon.start()).unwrap_err();
        let pid = stop(child);
        assert_eq!(
            error.to_string(),
            format!("daemon already appears to be running as pid {pid}")
        );
        assert_eq!(
            log_lines(dir.path()),
            [format!("starting daemon pid={}", std::process::id())]
        );
        // Nothing was recovered or opened.
        assert!(!dir.path().join("state.json").exists());
        assert!(!dir.path().join("daemon.sock").exists());
    }

    #[test]
    fn persist_writes_the_sessions_newest_first_without_projections_and_coalesces() {
        use deckhand_protocol::data::SessionStatus;
        let dir = tempfile::tempdir().unwrap();
        let _home = EnvVar::home(dir.path());
        let daemon = test_daemon(dir.path());
        let mut old = session("old", "/r", "/r");
        old.status = SessionStatus::Exited;
        old.notes = Some("projected".to_owned());
        let mut new = session("new", "/r", "/r");
        new.status = SessionStatus::Exited;
        new.created_at = "2026-02-01T00:00:00.000Z".to_owned();
        insert_session(&daemon, new);
        insert_session(&daemon, old);
        insert_session(&daemon, session("live", "/r", "/r"));
        let record: Ordered<deckhand_protocol::records::WorktreeRecord> = serde_json::from_value(json!({
            "id": "w1", "path": "/r/wt", "createdAt": "2026-01-01T00:00:00.000Z", "mergedAt": "2026-01-02T00:00:00.000Z"
        }))
        .unwrap();
        daemon.worktrees.borrow_mut().records.insert("w1".to_owned(), record);
        local(async {
            let first = daemon.persist();
            let second = daemon.persist();
            // Both callers arrived before the write started: they share it.
            assert!(daemon.lifecycle.borrow().persist.as_ref().unwrap().is_queued());
            // A session set before the write starts is in it.
            insert_session(&daemon, session("late", "/r", "/r"));
            first.await.unwrap();
            second.await.unwrap();
        });
        let text = std::fs::read_to_string(dir.path().join("state.json")).unwrap();
        let state: Value = serde_json::from_str(&text).unwrap();
        let ids: Vec<&str> = state["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|session| session["id"].as_str().unwrap())
            .collect();
        // Running first (in map order: the sort is stable), then by createdAt.
        assert_eq!(ids, ["live", "late", "old", "new"]);
        assert!(state["sessions"][2].get("notes").is_none());
        assert_eq!(state["worktrees"][0]["mergedAt"], "2026-01-02T00:00:00.000Z");
        // A request after the write is another write.
        daemon.sessions.borrow_mut().sessions.shift_remove("late");
        local(async { daemon.persist().await.unwrap() });
        let text = std::fs::read_to_string(dir.path().join("state.json")).unwrap();
        assert!(!text.contains("\"late\""));
        assert!(text.ends_with("}\n"));
    }

    #[test]
    fn cleanup_runs_once_closing_clients_and_removing_the_socket_and_pid_file() {
        let dir = tempfile::tempdir().unwrap();
        let _home = EnvVar::home(dir.path());
        let daemon = test_daemon(dir.path());
        let (client, mut rx) = test_client(1);
        daemon.server.borrow_mut().clients.insert(
            client.id,
            Client {
                handle: client.clone(),
                subscription: ClientSubscription::default(),
            },
        );
        insert_session(&daemon, session("s1", "/r", "/r"));
        std::fs::write(dir.path().join("daemon.sock"), "").unwrap();
        local(async {
            daemon.write_pid_file().await.unwrap();
            daemon.cleanup().await;
            assert!(daemon.lifecycle.borrow().shutting_down);
            assert!(client.is_destroyed());
            assert_eq!(rx.try_recv().unwrap(), "");
            assert!(daemon.server.borrow().clients.is_empty());
            assert!(!dir.path().join("daemon.sock").exists());
            assert!(!dir.path().join("daemon.pid").exists());
            let state = std::fs::read_to_string(dir.path().join("state.json")).unwrap();
            assert!(state.contains("\"s1\""));
            // Once only.
            daemon.cleanup().await;
        });
        assert_eq!(log_lines(dir.path()), ["cleanup start", "cleanup complete"]);
    }

    #[test]
    fn cleanup_keeps_a_pid_file_naming_another_process() {
        let dir = tempfile::tempdir().unwrap();
        let _home = EnvVar::home(dir.path());
        let daemon = test_daemon(dir.path());
        std::fs::write(dir.path().join("daemon.pid"), "1\n").unwrap();
        local(daemon.cleanup());
        assert_eq!(std::fs::read_to_string(dir.path().join("daemon.pid")).unwrap(), "1\n");
    }

    /// A session worker that exits on `kill` (`OBEY` set) or ignores it.
    const SH_WORKER: &str = r#"#!/bin/sh
printf '@HELLO@\n' >&3
: > "$KILLS.ready"
while IFS= read -r line <&3; do
  case "$line" in
    *'"type":"kill"'*) printf '%s\n' "$line" >> "$KILLS"; if [ -n "$OBEY" ]; then exit 0; fi ;;
  esac
done
"#;

    fn spawn_worker(dir: &Path, name: &str, obey: bool) -> Rc<crate::launch::WorkerRuntime> {
        use crate::worker_channel::{SpawnOptions, WorkerChannel, WorkerKind};
        use std::cell::{Cell, RefCell};
        let exe = dir.join(format!("worker-{name}.sh"));
        std::fs::write(&exe, SH_WORKER.replace("@HELLO@", &hello_line())).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut env = vec![(
            "KILLS".to_owned(),
            dir.join(format!("{name}.kills")).to_string_lossy().into_owned(),
        )];
        if obey {
            env.push(("OBEY".to_owned(), "1".to_owned()));
        }
        let (channel, events) = WorkerChannel::spawn(SpawnOptions {
            kind: WorkerKind::Session,
            exe,
            env,
            log_path: dir.join(format!("{name}.log")),
        })
        .unwrap();
        // The events are not looked at; the channel keeps reading.
        std::mem::forget(events);
        Rc::new(crate::launch::WorkerRuntime {
            channel,
            attached_agent: RefCell::new(None),
            delete_worktree_on_exit: Cell::new(false),
            delete_branch_on_exit: Cell::new(false),
            send_open_items_on_exit: Cell::new(false),
            hook_token: String::new(),
            launch_id: String::new(),
            allow_data_loss: Cell::new(false),
            screen: RefCell::new(None),
            draft: RefCell::new(None),
            last_hook_at: Cell::new(None),
            preview_styled: Cell::new(false),
        })
    }

    #[test]
    fn cleanup_kills_session_workers_failing_their_requests_and_sigkills_after_500_ms() {
        let dir = tempfile::tempdir().unwrap();
        let _home = EnvVar::home(dir.path());
        let daemon = test_daemon(dir.path());
        std::fs::create_dir(dir.path().join("workers")).unwrap();
        local(async {
            let obedient = spawn_worker(dir.path(), "a", true);
            let stubborn = spawn_worker(dir.path(), "b", false);
            for (id, worker) in [("a", &obedient), ("b", &stubborn)] {
                std::fs::write(dir.path().join(format!("workers/{id}.pid")), "1\n").unwrap();
                daemon
                    .launch
                    .borrow_mut()
                    .workers
                    .insert(id.to_owned(), Rc::clone(worker));
            }
            let mut wait = Map::new();
            wait.insert("type".to_owned(), Value::String("wait".to_owned()));
            let channel = stubborn.channel.clone();
            let pending = spawn(async move { channel.request::<Value>(wait).await });
            // The grace is measured from cleanup on, so both workers must be up first: on macOS a freshly written
            // script took ~320 ms to start even alone and over 500 ms in `nextest --workspace` (neither worker had run
            // its first line when the SIGKILL came, so no kill was recorded).
            let spawned = std::time::Instant::now();
            for id in ["a", "b"] {
                while !dir.path().join(format!("{id}.kills.ready")).exists() {
                    assert!(spawned.elapsed() < Duration::from_secs(20), "worker {id} never started");
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
            let started = std::time::Instant::now();
            daemon.cleanup().await;
            let took = started.elapsed();
            assert!(took >= Duration::from_millis(450), "{took:?}");
            assert!(took < Duration::from_millis(2000), "{took:?}");
            assert_eq!(pending.await.unwrap_err().to_string(), "daemon shutting down");
            assert!(daemon.launch.borrow().workers.is_empty());
            for worker in [&obedient, &stubborn] {
                tokio::time::timeout(Duration::from_secs(5), worker.channel.wait_process_exit())
                    .await
                    .unwrap();
            }
            for id in ["a", "b"] {
                assert!(!dir.path().join(format!("workers/{id}.pid")).exists());
                let kill: Value =
                    serde_json::from_str(&std::fs::read_to_string(dir.path().join(format!("{id}.kills"))).unwrap())
                        .unwrap();
                let keys: Vec<&String> = kill.as_object().unwrap().keys().collect();
                assert_eq!(keys, ["type", "requestId", "force"]);
                assert_eq!(kill["force"], true);
            }
        });
    }

    #[test]
    fn shutdown_is_restricted_to_the_dev_channel() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = test_daemon(dir.path());
        for channel in [None, Some("stable"), Some("DEV")] {
            let _env = EnvVar::set(&[(paths::CHANNEL_ENV, channel.map(std::ffi::OsStr::new))]);
            let error = local(async { daemon.handle_shutdown() }).unwrap_err();
            assert_eq!(
                error.to_string(),
                "This control is restricted to the isolated dev daemon"
            );
        }
        assert!(!daemon.lifecycle.borrow().shutting_down);
    }
}
