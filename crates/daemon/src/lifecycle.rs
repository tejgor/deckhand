//! Port of daemon.ts (startup, recovery, shutdown, persist).
//!
//! Startup recovers from the previous daemon: sessions it left running are marked exited (their PTYs died with it),
//! notes kept in state.json by older versions move to their files, worktree sessions' own notes merge into their
//! worktree's note and follow-up tasks become note items; then the socket is opened. Shutdown (signal, `shutdown`
//! request, fatal error) stops every worker, persists once more and removes the socket and pid file.

use deckhand_core::error::Result;
use deckhand_protocol::Ordered;
use deckhand_protocol::data::{OkResult, SessionRecord};
use deckhand_sys::async_util::{Coalescer, SharedTask};

use crate::daemon::Daemon;

/// The lifecycle's part of the daemon state.
#[derive(Default)]
pub struct LifecycleState {
    /// TS `shuttingDown`.
    pub shutting_down: bool,
    /// TS `persistInFlight` / `persistQueued`: at most one state.json write in flight plus one queued, shared by
    /// every caller arriving before the queued one snapshots the sessions. Created by [`Daemon::new`].
    pub persist: Option<Coalescer<Result<()>>>,
}

/// TS: daemon.ts persistedSession: the session as state.json stores it: no projected markers or notes, and no
/// attention reason (what the agent asked may quote code; kept in memory only).
pub fn persisted_session(session: &Ordered<SessionRecord>) -> Ordered<SessionRecord> {
    let _ = session;
    todo!("D2")
}

/// TS: InkDaemon.isProcessAlive (`kill(pid, 0)` succeeds).
pub fn is_process_alive(pid: i32) -> bool {
    let _ = pid;
    todo!("D2")
}

impl Daemon {
    /// TS: InkDaemon.start: config dir, pid-file check, crash recovery and migrations (see the module docs), socket
    /// (0600), pid file, signal handlers, the notes watcher, then the agent-version and merge-detection timers.
    pub async fn start(&self) -> Result<()> {
        todo!("D2")
    }

    /// TS: InkDaemon.log: appends `[<ISO time>] daemon <message>` to the daemon log; failures are ignored.
    pub async fn log(&self, message: &str) {
        let _ = message;
        todo!("D2")
    }

    /// TS: InkDaemon.assertNoLiveDaemonFromPidFile: `daemon already appears to be running as pid <pid>` when the pid
    /// file names another live process (a missing file is fine).
    pub async fn assert_no_live_daemon_from_pid_file(&self) -> Result<()> {
        todo!("D2")
    }

    /// TS: InkDaemon.writePidFile (`<pid>\n`).
    pub async fn write_pid_file(&self) -> Result<()> {
        todo!("D2")
    }

    /// TS: InkDaemon.removePidFile: only when it still names this process; failures are ignored.
    pub async fn remove_pid_file(&self) {
        todo!("D2")
    }

    /// TS: InkDaemon.setupProcessHandlers: SIGINT/SIGTERM shut down (exit 0), SIGHUP is logged and ignored; a panic
    /// in a daemon task is logged, cleaned up after and exits 1 (TS uncaughtException/unhandledRejection).
    pub fn setup_process_handlers(&self) {
        todo!("D2")
    }

    /// TS: InkDaemon.cleanup (once): stops the timers, the notes watcher and setup processes, closes the clients,
    /// stops the Changes watches and workspace workers, kills the session workers (SIGKILL after 500 ms), persists,
    /// closes the server and removes the socket and pid file.
    pub async fn cleanup(&self) {
        todo!("D2")
    }

    /// TS: InkDaemon.persist: state.json `{sessions (newest first, persistedSession), worktrees}`, coalesced (see
    /// [`LifecycleState::persist`]).
    pub fn persist(&self) -> SharedTask<Result<()>> {
        todo!("D2")
    }

    /// TS: handleRequest `shutdown`: only on the dev channel (`This control is restricted to the isolated dev
    /// daemon`); answers `{ok: true}`, then cleans up and exits 0 after 25 ms.
    pub fn handle_shutdown(&self) -> Result<OkResult> {
        todo!("D2")
    }
}
