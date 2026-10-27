//! The daemon's handling of hook events from `deckhand hook`.
//!
//! Agents report their lifecycle through hooks (`deckhand hook`, relayed as `agent-hook` requests authenticated by
//! the launch's token): the session's attention state (working, needs input, ended, ...), its conversation ID, and
//! desktop notifications. The worker's activity heuristic (`agent-status`) settles stale states. A session started
//! from a task gets the task typed (never sent) into its agent once the screen settles at its input (the draft).

use std::rc::Rc;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use deckhand_core::agent_signals::{attention_message, needs_attention, normalize_hook};
use deckhand_core::agents::{agent_spec, same_agent_session_ref};
use deckhand_core::error::{DhError, Result};
use deckhand_core::jsstr::{date, js_trim};
use deckhand_protocol::Ordered;
use deckhand_protocol::data::{
    AgentActivityStatus, AgentSessionRef, AttentionReasonKind, AttentionState, SessionAttention, SessionRecord,
    SessionStatus, SubSessionKind,
};
use deckhand_protocol::wire::AgentHook;
use deckhand_sys::async_util::spawn;
use deckhand_sys::notify::{self, Delivery};
use deckhand_sys::storage::load_app_config;
use indexmap::IndexMap;
use regex::Regex;
use serde_json::{Map, Value};

use crate::daemon::Daemon;
use crate::launch::WorkerRuntime;

/// TS: daemon.ts DRAFT_WAIT_MS: how long a task's draft waits for the agent to settle at its input.
pub const DRAFT_WAIT: Duration = Duration::from_millis(120_000);

/// TS: daemon.ts DRAFT_SETTLE_MS: the screen has settled once unchanged this long.
pub const DRAFT_SETTLE: Duration = Duration::from_millis(700);

/// TS: daemon.ts AGENT_PROMPT_PATTERN (`/do you trust|trust (?:the files|the contents|this folder|this
/// directory)|press enter to continue|^\s*[❯›>]\s*1\.\s/im`): a menu or question on the agent's screen, where a draft
/// is never typed (folder-trust prompts, update offers).
///
/// Written for the regex crate with the JS semantics kept: `i` without `u` folds ASCII letters only (so `ſ` is no
/// `s`), `\s` is the JS whitespace set (with U+FEFF, without U+0085), and a multiline `^` also follows `\r`, U+2028
/// and U+2029 (taking that terminator into the match changes nothing for a test, as `\s*` would take it anyway).
pub const AGENT_PROMPT_PATTERN: &str = concat!(
    r"(?i-u:do you trust|trust (?:the files|the contents|this folder|this directory)|press enter to continue)",
    r"|(?:^|[\n\r\u{2028}\u{2029}])",
    r"[\t\n\x0B\x0C\r \u{A0}\u{1680}\u{2000}-\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}]*",
    r"[❯›>]",
    r"[\t\n\x0B\x0C\r \u{A0}\u{1680}\u{2000}-\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}]*",
    r"1\.",
    r"[\t\n\x0B\x0C\r \u{A0}\u{1680}\u{2000}-\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}]",
);

/// A blocked or unanswered permission is logged once per daemon, not on every notification.
static BLOCKED_LOGGED: AtomicBool = AtomicBool::new(false);
static NOT_ASKED_LOGGED: AtomicBool = AtomicBool::new(false);

static AGENT_PROMPT: LazyLock<Regex> = LazyLock::new(|| Regex::new(AGENT_PROMPT_PATTERN).expect("valid regex"));

/// `/\x1b\[[0-?]*[ -/]*[@-~]/g`: CSI sequences.
static CSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").expect("valid regex"));

/// `/\x1b./g`: any other escape (JS `.` stops at line terminators).
static ESCAPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b[^\n\r\u{2028}\u{2029}]").expect("valid regex"));

/// `/\p{L}/u`.
static LETTER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\p{L}").expect("valid regex"));

/// The hooks' part of the daemon state (the per-launch hook state lives on the session's worker).
#[derive(Debug, Default)]
pub struct HooksState {}

/// TS: `AGENT_PROMPT_PATTERN.test(screen)`.
pub fn shows_agent_prompt(screen: &str) -> bool {
    AGENT_PROMPT.is_match(screen)
}

/// TS: handleRequest `input`'s draft check: the data, without escape sequences, contains a letter (`/\p{L}/u`).
pub fn types_letters(data: &str) -> bool {
    let without_csi = CSI.replace_all(data, "");
    let plain = ESCAPE.replace_all(&without_csi, "");
    LETTER.is_match(&plain)
}

/// `new Date().toISOString()`.
fn now_iso() -> String {
    date::to_iso_string(date::now_ms()).unwrap_or_default()
}

/// A JS-truthy optional string (set and not empty).
fn truthy(value: Option<&str>) -> bool {
    value.is_some_and(|text| !text.is_empty())
}

/// `{state, event, at}`: an attention record without a reason.
fn plain_attention(state: AttentionState, event: &str, at: String) -> SessionAttention {
    SessionAttention {
        state,
        event: event.to_owned(),
        at,
        reason: None,
        extra: IndexMap::new(),
    }
}

impl Daemon {
    /// TS: `this.workers.get(id)`.
    fn session_worker(&self, session_id: &str) -> Option<Rc<WorkerRuntime>> {
        self.launch.borrow().workers.get(session_id).cloned()
    }

    /// TS: InkDaemon.handleAgentHook (`Stale or unauthorized lifecycle callback`): applies the hook's signal to the
    /// session's attention (stale and repeated signals are dropped), adopts a new conversation ID, and notifies when
    /// the session newly needs attention.
    pub async fn handle_agent_hook(&self, message: &AgentHook) -> Result<()> {
        let worker = self.session_worker(&message.session_id);
        let session = self.require_session(&message.session_id)?;
        let Some(worker) = worker.filter(|worker| {
            worker.hook_token == message.token
                && worker.launch_id == message.launch_id
                && session.launch_id.as_deref() == Some(message.launch_id.as_str())
                && session.status != SessionStatus::Exited
        }) else {
            return Err(DhError::msg("Stale or unauthorized lifecycle callback"));
        };
        let Some(signal) = normalize_hook(session.program, &message.payload)? else {
            return Ok(());
        };
        // Async hooks race each other: one that started before the last applied signal is stale.
        if let Some(sent_at) = message.sent_at {
            if worker.last_hook_at.get().is_some_and(|last| sent_at < last) {
                return Ok(());
            }
            worker.last_hook_at.set(Some(sent_at));
        }
        let previous = session.attention.clone();
        let previous_state = previous.as_ref().map(|previous| previous.state);
        if signal.idle == Some(true)
            && previous_state.is_some_and(|state| state != AttentionState::Working && state != AttentionState::Unknown)
        {
            return Ok(());
        }
        // A native subagent's tool use only shows that the prompt it raised was answered; otherwise the root's state stands.
        if signal.from_subagent == Some(true)
            && signal.state == AttentionState::Working
            && previous_state != Some(AttentionState::NeedsInput)
        {
            return Ok(());
        }
        let native_ref = self.accepted_native_ref(&session, signal.native_ref.as_ref(), &signal.event);
        if let (Some(native_ref), Some(current)) = (&native_ref, &session.agent_session_ref) {
            self.log_soon(format!(
                "{} moved to conversation {} (was {}, {})",
                session.id, native_ref.value, current.value, signal.event
            ));
        }
        let previous_reason = previous.as_ref().and_then(|previous| previous.reason.clone());
        // A notification's generic text ("Claude is waiting for your input") never replaces the reason it follows.
        let reason = match (&signal.reason, previous_reason) {
            (Some(reason), Some(previous_reason)) if reason.kind == AttentionReasonKind::Notice => {
                Some(previous_reason)
            }
            _ => signal.reason.clone(),
        };
        let mut attention = plain_attention(signal.state, &signal.event, now_iso());
        attention.reason.clone_from(&reason);
        let previous_text = previous
            .as_ref()
            .and_then(|previous| previous.reason.as_ref())
            .map(|reason| reason.text.as_str());
        let reason_changed = reason.as_ref().map(|reason| reason.text.as_str()) != previous_text;
        if native_ref.is_none() && previous_state == Some(signal.state) && !reason_changed {
            // Tool-use events repeat constantly without changing anything visible; keep them off disk and the wire.
            if let Some(previous) = &previous {
                attention.at.clone_from(&previous.at);
            }
            let mut held = session;
            held.touch("attention");
            held.attention = Some(attention);
            self.set_session(held);
            return Ok(());
        }
        let mut updated = session;
        if let Some(native_ref) = native_ref {
            updated.touch("agentSessionRef");
            updated.agent_session_ref = Some(native_ref);
        }
        updated.touch("attention");
        updated.attention = Some(attention);
        let updated = self.save_session(updated).await?;
        if needs_attention(Some(signal.state)) && (previous_state != Some(signal.state) || reason_changed) {
            let text = attention_message(signal.state, reason.as_ref());
            let daemon = self.clone();
            drop(spawn(async move { daemon.notify(&updated, &text).await }));
        }
        Ok(())
    }

    /// TS: InkDaemon.markAnswered: input answering a prompt makes a session that needs input work again.
    ///
    /// You answered what the agent asked (Enter, a digit or Esc typed into it while it needs input): it works again.
    /// Claude sends no hook until the tool you allowed ends, which can take minutes; hooks that started before the
    /// answer are stale. A settled screen makes it unknown, as for any working state.
    ///
    /// TS calls it without awaiting, so its part before the save's first await runs as the answer arrives: the
    /// answer's time and the working state are in place when this returns (before any hook queued behind the
    /// input), and the returned future only persists and broadcasts. Call it where the input is handled; spawn
    /// only the future.
    pub fn mark_answered(&self, session_id: &str) -> impl Future<Output = Result<()>> + 'static {
        let answered = self.answer(session_id);
        let daemon = self.clone();
        async move {
            let Some(session) = answered else {
                return Ok(());
            };
            // The rest of saveSession (sessions.rs `save_session`), whose synchronous part `answer` ran.
            daemon.persist().await?;
            daemon.broadcast_session_updated(&session);
            daemon.sync_change_watches();
            daemon.require_session(&session.id)?;
            Ok(())
        }
    }

    /// markAnswered up to saveSession's first await: the answered session, held as working.
    fn answer(&self, session_id: &str) -> Option<Ordered<SessionRecord>> {
        let (Some(mut session), Some(worker)) = (self.get_session(session_id), self.session_worker(session_id)) else {
            return None;
        };
        let needs_input =
            session.attention.as_ref().map(|attention| attention.state) == Some(AttentionState::NeedsInput);
        if session.status != SessionStatus::Running || !needs_input {
            return None;
        }
        worker
            .last_hook_at
            .set(Some(worker.last_hook_at.get().unwrap_or(0).max(date::now_ms())));
        session.touch("attention");
        session.attention = Some(plain_attention(AttentionState::Working, "Answered", now_iso()));
        self.set_session_in_workspace(session.clone());
        Some(session)
    }

    /// TS: InkDaemon.acceptedNativeRef: the conversation ID a hook reports, when the session should adopt it.
    pub fn accepted_native_ref(
        &self,
        session: &SessionRecord,
        native_ref: Option<&AgentSessionRef>,
        event: &str,
    ) -> Option<AgentSessionRef> {
        let native_ref = native_ref?;
        if same_agent_session_ref(Some(native_ref), session.agent_session_ref.as_ref()) {
            return None;
        }
        // Claude moves to a new conversation ID without exiting (accepting a plan with "clear context", /clear,
        // /resume, /branch), and s must resume the one on screen. Plan acceptance has sent no SessionStart, so any
        // root event's ID counts; a native subagent's never reaches here. Others: identity is established at
        // SessionStart, and later events may only fill in a missing ref.
        if !agent_spec(session.program).id_at_launch && event != "SessionStart" && session.agent_session_ref.is_some() {
            return None;
        }
        if session.sub_session_kind == Some(SubSessionKind::Forked) {
            // A fork (e.g. a Codex child, which reports its own ID, or a legacy /branch child) never adopts its
            // parent's identity.
            let parent = session
                .forked_from_session_id
                .as_deref()
                .filter(|id| !id.is_empty())
                .and_then(|id| self.get_session(id));
            let parent_value = parent
                .as_ref()
                .and_then(|parent| parent.agent_session_ref.as_ref())
                .map(|r| r.value.as_str());
            let forked_value = session.forked_from_agent_session_ref.as_ref().map(|r| r.value.as_str());
            if forked_value == Some(native_ref.value.as_str()) || parent_value == Some(native_ref.value.as_str()) {
                return None;
            }
        }
        Some(native_ref.clone())
    }

    /// TS: InkDaemon.notify: a desktop notification `Deckhand: <title>` when enabled (osascript on macOS,
    /// notify-send elsewhere, 2 s timeout); failures are ignored. The port posts from Deckhand.app on macOS so the
    /// notification shows Deckhand's icon (see `deckhand_sys::notify`), and logs, once per daemon, when macOS blocks
    /// it or still waits for an answer, which Settings and `deckhand doctor` then explain.
    pub async fn notify(&self, session: &SessionRecord, message: &str) {
        // Headless machines and denied notification permissions are normal.
        let enabled = load_app_config()
            .await
            .is_ok_and(|config| config.notifications == Some(true));
        if !enabled {
            return;
        }
        let title = format!("Deckhand: {}", session.title);
        let note = match notify::send(&title, message).await {
            Delivery::Sent => None,
            Delivery::Blocked => (!BLOCKED_LOGGED.swap(true, Ordering::Relaxed))
                .then(|| "notifications are blocked for Deckhand in System Settings › Notifications".to_owned()),
            Delivery::NotAsked => (!NOT_ASKED_LOGGED.swap(true, Ordering::Relaxed))
                .then(|| "notifications wait for an answer to macOS's permission prompt".to_owned()),
            Delivery::Fallback(reason) => Some(reason),
        };
        if let Some(note) = note {
            self.log(&note).await;
        }
    }

    /// TS: InkDaemon.setWorkerAgentStatus: records the activity status (a working signal on an idle screen becomes
    /// unknown) and types a pending draft once idle.
    pub async fn set_worker_agent_status(&self, session_id: &str, agent_status: AgentActivityStatus) -> Result<()> {
        let Some(session) = self.get_session(session_id) else {
            return Ok(());
        };
        if session.status == SessionStatus::Exited || session.agent_status == Some(agent_status) {
            return Ok(());
        }
        let now = now_iso();
        // A signal still saying working on a settled screen is stale: Claude reports no hook when Esc interrupts it.
        // Agents animate while they work, so the screen going idle means they stopped; the next hook says what's next.
        let stale_working = agent_status == AgentActivityStatus::Idle
            && session.attention.as_ref().map(|attention| attention.state) == Some(AttentionState::Working);
        let mut updated = session;
        updated.touch("agentStatus");
        updated.agent_status = Some(agent_status);
        updated.touch("agentStatusUpdatedAt");
        updated.agent_status_updated_at = Some(now.clone());
        if stale_working {
            updated.touch("attention");
            updated.attention = Some(plain_attention(AttentionState::Unknown, "ScreenIdle", now));
        }
        self.save_session(updated).await?;
        if agent_status == AgentActivityStatus::Idle {
            self.type_draft(session_id).await?;
        }
        Ok(())
    }

    /// TS: InkDaemon.typeDraft: types the pending draft as one bracketed paste once the screen shows no prompt.
    ///
    /// A session started from a task gets the task typed into its agent's input, not sent: once the agent's screen
    /// first settles (unchanged for DRAFT_SETTLE, or activity idle) with no menu or question on it, as one bracketed
    /// paste (so no character is taken as a shortcut and nothing submits). It waits through prompts (e.g. folder
    /// trust) for DRAFT_WAIT at most.
    pub async fn type_draft(&self, session_id: &str) -> Result<()> {
        let Some(worker) = self.session_worker(session_id) else {
            return Ok(());
        };
        let Some(text) = worker.draft.borrow().as_ref().map(|draft| draft.text.clone()) else {
            return Ok(());
        };
        if worker.channel.is_exited() {
            return Ok(());
        }
        let settled = worker
            .screen
            .borrow()
            .as_deref()
            .is_some_and(|screen| !js_trim(screen).is_empty() && !shows_agent_prompt(screen));
        if !settled {
            return Ok(());
        }
        self.drop_draft(session_id, None, true);
        let mut payload = Map::new();
        payload.insert("type".to_owned(), Value::String("input".to_owned()));
        payload.insert("target".to_owned(), Value::String("agent".to_owned()));
        payload.insert("data".to_owned(), Value::String(format!("\x1b[200~{text}\x1b[201~")));
        self.send_worker_event(session_id, payload);
        self.log(&format!("typed the task draft into {session_id}")).await;
        Ok(())
    }

    /// TS: InkDaemon.dropDraft: forgets the draft (`save`: also clear the session's `startPrompt`); `reason` is logged
    /// when it was not typed.
    pub fn drop_draft(&self, session_id: &str, reason: Option<&str>, save: bool) {
        let Some(worker) = self.session_worker(session_id) else {
            return;
        };
        let draft = worker.draft.borrow_mut().take();
        let Some(draft) = draft else {
            return;
        };
        // Dropping the timers clears them.
        drop(draft);
        if save
            && let Some(mut session) = self.get_session(session_id)
            && truthy(session.start_prompt.as_deref())
        {
            session.start_prompt = None;
            self.save_session_soon(session);
        }
        if let Some(reason) = reason {
            self.log_soon(format!("task draft for {session_id} not typed: {reason}"));
        }
    }

    /// TS `void this.saveSession(session).catch(() => {})`: the session is held at once (as the TS sets it before its
    /// first await), then persisted and broadcast in the background; failures are ignored.
    fn save_session_soon(&self, session: Ordered<SessionRecord>) {
        self.set_session_in_workspace(session.clone());
        let persisted = self.persist();
        let daemon = self.clone();
        drop(spawn(async move {
            if persisted.await.is_ok() {
                daemon.broadcast_session_updated(&session);
                daemon.sync_change_watches();
            }
        }));
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::launch::Draft;
    use crate::server::tests::{hello_line, local, session, test_client, test_daemon};
    use crate::server::{Client, ClientId, ClientSubscription};
    use crate::worker_channel::{ChannelEvents, ProcessExit, WorkerChannel, WorkerKind};
    use deckhand_sys::async_util::Timer;
    use serde_json::json;
    use std::cell::{Cell, RefCell};
    use std::path::Path;
    use std::sync::MutexGuard;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;
    use tokio::sync::mpsc;

    /// DECKHAND_HOME = `dir` while this lives.
    pub(crate) struct Home {
        _guard: MutexGuard<'static, ()>,
        previous: Option<std::ffi::OsString>,
    }

    impl Home {
        pub(crate) fn set(dir: &Path) -> Home {
            let guard = crate::server::tests::env_lock();
            let previous = std::env::var_os(deckhand_sys::paths::HOME_ENV);
            // SAFETY: tests touching the environment hold server::tests::env_lock.
            unsafe { std::env::set_var(deckhand_sys::paths::HOME_ENV, dir) };
            Home {
                _guard: guard,
                previous,
            }
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            // SAFETY: as in Home::set (the environment lock is still held).
            unsafe {
                match self.previous.take() {
                    Some(value) => std::env::set_var(deckhand_sys::paths::HOME_ENV, value),
                    None => std::env::remove_var(deckhand_sys::paths::HOME_ENV),
                }
            }
        }
    }

    /// The other end of a fake session worker's channel.
    struct FakeWorker {
        theirs: BufReader<UnixStream>,
        _events: ChannelEvents,
    }

    impl FakeWorker {
        /// The next message the daemon sent.
        async fn next(&mut self) -> Value {
            let mut line = String::new();
            self.theirs.read_line(&mut line).await.unwrap();
            serde_json::from_str(&line).unwrap()
        }
    }

    /// A daemon in `dir` without a notes store, with session `s` (running, launch `launch-1`) and its fake worker
    /// (token `token`).
    async fn setup(dir: &Path) -> (Daemon, Rc<WorkerRuntime>, FakeWorker) {
        let daemon = test_daemon(dir);
        daemon.notes_tasks.borrow_mut().notes = None;
        let mut record = session("s", "/r", "/r");
        record.launch_id = Some("launch-1".into());
        daemon.set_session(record);
        let (ours, theirs) = UnixStream::pair().unwrap();
        let (channel, events) =
            WorkerChannel::attach(WorkerKind::Session, None, ours, std::future::pending::<ProcessExit>());
        let mut theirs = BufReader::new(theirs);
        theirs
            .get_mut()
            .write_all(format!("{}\n", hello_line()).as_bytes())
            .await
            .unwrap();
        let worker = Rc::new(WorkerRuntime {
            channel,
            attached_agent: RefCell::new(None),
            delete_worktree_on_exit: Cell::new(false),
            delete_branch_on_exit: Cell::new(false),
            send_open_items_on_exit: Cell::new(false),
            hook_token: "token".into(),
            launch_id: "launch-1".into(),
            allow_data_loss: Cell::new(false),
            screen: RefCell::new(None),
            draft: RefCell::new(None),
            last_hook_at: Cell::new(None),
        });
        daemon
            .launch
            .borrow_mut()
            .workers
            .insert("s".into(), Rc::clone(&worker));
        let fake = FakeWorker {
            theirs,
            _events: events,
        };
        (daemon, worker, fake)
    }

    /// A client subscribed to `/r`.
    pub(crate) fn subscribe(daemon: &Daemon, id: u64) -> mpsc::UnboundedReceiver<String> {
        let (handle, rx) = test_client(id);
        daemon.server.borrow_mut().clients.insert(
            ClientId(id),
            Client {
                handle,
                subscription: ClientSubscription {
                    repo_root: Some("/r".into()),
                    ..ClientSubscription::default()
                },
            },
        );
        rx
    }

    pub(crate) fn received(rx: &mut mpsc::UnboundedReceiver<String>) -> Vec<Value> {
        let mut out = Vec::new();
        while let Ok(line) = rx.try_recv() {
            out.push(serde_json::from_str(&line).unwrap());
        }
        out
    }

    fn hook(payload: Value, sent_at: Option<i64>) -> AgentHook {
        AgentHook {
            request_id: "r".into(),
            session_id: "s".into(),
            launch_id: "launch-1".into(),
            token: "token".into(),
            payload,
            sent_at,
        }
    }

    fn attention(daemon: &Daemon) -> Option<SessionAttention> {
        daemon.get_session("s").unwrap().attention.clone()
    }

    fn state(daemon: &Daemon) -> Option<AttentionState> {
        attention(daemon).map(|attention| attention.state)
    }

    /// Lets spawned work (saves, logs) run.
    async fn settle() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    /// The daemon log in `dir` once it contains `needle` (tokio's file writes land in the background, so a line
    /// logged may reach the file a moment later), else as it is after 5 s.
    pub(crate) async fn log_containing(dir: &Path, needle: &str) -> String {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let log = std::fs::read_to_string(dir.join("daemon.log")).unwrap_or_default();
            if log.contains(needle) || std::time::Instant::now() > deadline {
                return log;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[test]
    fn the_prompt_pattern_matches_like_the_js_regex() {
        assert!(shows_agent_prompt("Do you trust the files in this folder?"));
        assert!(shows_agent_prompt("Claude Code may read files.\nTrust this folder"));
        assert!(shows_agent_prompt("Update available! PRESS ENTER TO CONTINUE"));
        assert!(shows_agent_prompt("Pick one:\n  ❯ 1. Yes\n    2. No"));
        assert!(shows_agent_prompt("› 1. Trust"));
        assert!(shows_agent_prompt("x\r> 1. a"));
        assert!(shows_agent_prompt("x\u{2028}❯ 1. a"));
        assert!(shows_agent_prompt("\u{FEFF}❯ 1.\u{3000}a"));
        assert!(!shows_agent_prompt(""));
        assert!(!shows_agent_prompt("> fix the login bug"));
        assert!(!shows_agent_prompt("text ❯ 1. not at a line start"));
        assert!(!shows_agent_prompt("❯ 1.x"));
        // JS `\s` has no U+0085; `i` without `u` folds ASCII only.
        assert!(!shows_agent_prompt("❯ 1.\u{85}"));
        assert!(!shows_agent_prompt("do you tru\u{17F}t"));
        assert!(!shows_agent_prompt("\u{85}❯ 1. a"));
    }

    #[test]
    fn typed_letters_ignore_escape_sequences() {
        assert!(types_letters("fix"));
        assert!(types_letters("é"));
        assert!(types_letters("日本"));
        assert!(types_letters("\x1b[200~text\x1b[201~"));
        assert!(!types_letters("\r"));
        assert!(!types_letters("12"));
        assert!(!types_letters("\x1b[A\x1b[1;5D"));
        assert!(!types_letters("\x1bO"));
        assert!(!types_letters("\x1bb"));
        // Only the character after a non-CSI escape goes (as in the TS, SS3's final `P` stays a letter).
        assert!(types_letters("\x1bOP"));
        assert!(!types_letters(" !?"));
        // `.` stops at a line terminator, so the letter after it stays.
        assert!(types_letters("\x1b\na"));
    }

    #[test]
    fn hooks_from_another_launch_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        local(async {
            let (daemon, _worker, _fake) = setup(dir.path()).await;
            let payload = json!({"hook_event_name": "UserPromptSubmit"});
            let mut wrong = hook(payload.clone(), None);
            wrong.token = "other".into();
            let error = daemon.handle_agent_hook(&wrong).await.unwrap_err();
            assert_eq!(error.to_string(), "Stale or unauthorized lifecycle callback");
            let mut wrong = hook(payload.clone(), None);
            wrong.launch_id = "launch-0".into();
            let error = daemon.handle_agent_hook(&wrong).await.unwrap_err();
            assert_eq!(error.to_string(), "Stale or unauthorized lifecycle callback");
            let mut missing = hook(payload.clone(), None);
            missing.session_id = "nope".into();
            let error = daemon.handle_agent_hook(&missing).await.unwrap_err();
            assert_eq!(error.to_string(), "Session does not exist");
            let error = daemon.handle_agent_hook(&hook(json!([]), None)).await.unwrap_err();
            assert_eq!(error.to_string(), "Hook input must be an object");
            // An event that says nothing about attention changes nothing.
            daemon
                .handle_agent_hook(&hook(json!({"hook_event_name": "SubagentStop"}), None))
                .await
                .unwrap();
            assert_eq!(state(&daemon), None);
            daemon.patch_session("s", |session| session.status = SessionStatus::Exited);
            let error = daemon.handle_agent_hook(&hook(payload, None)).await.unwrap_err();
            assert_eq!(error.to_string(), "Stale or unauthorized lifecycle callback");
        });
    }

    // Saving a session runs the workspace (D6) and Changes (D7) bookkeeping, which are not ported yet; these tests
    // take the paths that hold the session without saving it (the TS daemon suites cover the rest end to end).

    /// Sets session `s`'s attention (and agent ref) as if earlier hooks had.
    fn seed(daemon: &Daemon, state: AttentionState, reason: Option<&str>, agent_ref: Option<&str>) {
        daemon.patch_session("s", |session| {
            let mut attention = plain_attention(state, "Seed", "2026-01-01T00:00:00.000Z".into());
            attention.reason =
                reason.map(|text| serde_json::from_value(json!({"kind": "failure", "text": text})).unwrap());
            session.touch("attention");
            session.attention = Some(attention);
            session.agent_session_ref = agent_ref.map(|value| {
                serde_json::from_value(json!({"provider": "claude", "kind": "id", "value": value})).unwrap()
            });
        });
    }

    #[test]
    fn repeated_and_stale_hooks_stay_off_disk_and_the_wire() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        local(async {
            let (daemon, worker, _fake) = setup(dir.path()).await;
            let mut rx = subscribe(&daemon, 1);
            seed(&daemon, AttentionState::Working, None, Some("conv-1"));
            // The same state again: held with its time kept, never saved or broadcast.
            daemon
                .handle_agent_hook(&hook(
                    json!({"hook_event_name": "PostToolUse", "session_id": "conv-1"}),
                    Some(100),
                ))
                .await
                .unwrap();
            let repeated = attention(&daemon).unwrap();
            assert_eq!(repeated.state, AttentionState::Working);
            assert_eq!(repeated.event, "PostToolUse");
            assert_eq!(repeated.at, "2026-01-01T00:00:00.000Z");
            assert_eq!(worker.last_hook_at.get(), Some(100));
            // A hook that started before the last applied one is stale.
            daemon
                .handle_agent_hook(&hook(json!({"hook_event_name": "Stop"}), Some(99)))
                .await
                .unwrap();
            assert_eq!(state(&daemon), Some(AttentionState::Working));
            assert_eq!(worker.last_hook_at.get(), Some(100));
            // One without a start time is never stale.
            daemon
                .handle_agent_hook(&hook(json!({"hook_event_name": "UserPromptSubmit"}), None))
                .await
                .unwrap();
            assert_eq!(attention(&daemon).unwrap().event, "UserPromptSubmit");
            assert_eq!(worker.last_hook_at.get(), Some(100));
            settle().await;
            assert!(received(&mut rx).is_empty());
            assert!(!deckhand_sys::paths::state_path().exists());
        });
    }

    #[test]
    fn notices_keep_the_reason_they_follow_and_idle_prompts_never_hide_a_question() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        local(async {
            let (daemon, _worker, _fake) = setup(dir.path()).await;
            seed(&daemon, AttentionState::NeedsInput, Some("Allow Bash?"), None);
            // A notification's generic text never replaces the reason it follows (so nothing changed).
            daemon
                .handle_agent_hook(&hook(
                    json!({"hook_event_name": "Notification", "notification_type": "permission_prompt", "message": "Claude needs your permission"}),
                    None,
                ))
                .await
                .unwrap();
            let held = attention(&daemon).unwrap();
            assert_eq!(held.state, AttentionState::NeedsInput);
            assert_eq!(held.event, "Notification");
            assert_eq!(held.reason.unwrap().text, "Allow Bash?");
            // The idle prompt only ends a working or unknown state.
            daemon
                .handle_agent_hook(&hook(
                    json!({"hook_event_name": "Notification", "notification_type": "idle_prompt", "message": "Claude is waiting for your input"}),
                    None,
                ))
                .await
                .unwrap();
            assert_eq!(attention(&daemon).unwrap().event, "Notification");
            assert_eq!(state(&daemon), Some(AttentionState::NeedsInput));
            // A native subagent's tool use only counts as answering a question; otherwise the root's state stands.
            seed(&daemon, AttentionState::ResponseEnded, None, None);
            daemon
                .handle_agent_hook(&hook(json!({"hook_event_name": "PostToolUse", "agent_id": "a1"}), None))
                .await
                .unwrap();
            assert_eq!(attention(&daemon).unwrap().event, "Seed");
            for previous in [
                AttentionState::ResponseEnded,
                AttentionState::Failed,
                AttentionState::Limited,
            ] {
                seed(&daemon, previous, Some("request failed"), None);
                daemon
                    .handle_agent_hook(&hook(
                        json!({"hook_event_name": "Notification", "notification_type": "idle_prompt"}),
                        None,
                    ))
                    .await
                    .unwrap();
                assert_eq!(attention(&daemon).unwrap().event, "Seed");
            }
            assert!(!deckhand_sys::paths::state_path().exists());
        });
    }

    #[test]
    fn native_refs_are_adopted_only_when_the_session_should_move() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = test_daemon(dir.path());
        daemon.notes_tasks.borrow_mut().notes = None;
        let reference = |provider: &str, value: &str| -> AgentSessionRef {
            serde_json::from_value(json!({"provider": provider, "kind": "id", "value": value})).unwrap()
        };
        let mut claude = session("c", "/r", "/r");
        assert_eq!(daemon.accepted_native_ref(&claude, None, "Stop"), None);
        assert_eq!(
            daemon.accepted_native_ref(&claude, Some(&reference("claude", "a")), "Stop"),
            Some(reference("claude", "a"))
        );
        claude.agent_session_ref = Some(reference("claude", "a"));
        assert_eq!(
            daemon.accepted_native_ref(&claude, Some(&reference("claude", "a")), "Stop"),
            None
        );
        assert_eq!(
            daemon.accepted_native_ref(&claude, Some(&reference("claude", "b")), "Stop"),
            Some(reference("claude", "b"))
        );

        // Codex: identity comes at SessionStart; later events only fill in a missing ref.
        let mut codex = session("x", "/r", "/r");
        codex.program = deckhand_protocol::data::ProgramKey::Codex;
        assert_eq!(
            daemon.accepted_native_ref(&codex, Some(&reference("codex", "a")), "Stop"),
            Some(reference("codex", "a"))
        );
        codex.agent_session_ref = Some(reference("codex", "a"));
        assert_eq!(
            daemon.accepted_native_ref(&codex, Some(&reference("codex", "b")), "Stop"),
            None
        );
        assert_eq!(
            daemon.accepted_native_ref(&codex, Some(&reference("codex", "b")), "SessionStart"),
            Some(reference("codex", "b"))
        );

        // A fork never adopts its parent's identity.
        let mut parent = session("p", "/r", "/r");
        parent.agent_session_ref = Some(reference("codex", "parent"));
        daemon.set_session(parent);
        let mut fork = codex.clone();
        fork.agent_session_ref = None;
        fork.sub_session_kind = Some(SubSessionKind::Forked);
        fork.forked_from_session_id = Some("p".into());
        fork.forked_from_agent_session_ref = Some(reference("codex", "origin"));
        assert_eq!(
            daemon.accepted_native_ref(&fork, Some(&reference("codex", "parent")), "SessionStart"),
            None
        );
        assert_eq!(
            daemon.accepted_native_ref(&fork, Some(&reference("codex", "origin")), "SessionStart"),
            None
        );
        assert_eq!(
            daemon.accepted_native_ref(&fork, Some(&reference("codex", "child")), "SessionStart"),
            Some(reference("codex", "child"))
        );
    }

    #[test]
    fn only_a_running_session_that_needs_input_is_answered() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        local(async {
            let (daemon, worker, _fake) = setup(dir.path()).await;
            daemon.mark_answered("missing").await.unwrap();
            // Not waiting on you: nothing changes.
            daemon.mark_answered("s").await.unwrap();
            assert_eq!(state(&daemon), None);
            seed(&daemon, AttentionState::ResponseEnded, None, None);
            daemon.mark_answered("s").await.unwrap();
            assert_eq!(state(&daemon), Some(AttentionState::ResponseEnded));
            seed(&daemon, AttentionState::NeedsInput, None, None);
            daemon.patch_session("s", |session| session.status = SessionStatus::Starting);
            daemon.mark_answered("s").await.unwrap();
            assert_eq!(state(&daemon), Some(AttentionState::NeedsInput));
            assert_eq!(worker.last_hook_at.get(), None);
        });
    }

    #[test]
    fn an_answer_makes_hooks_that_started_before_it_stale_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        local(async {
            let (daemon, worker, _fake) = setup(dir.path()).await;
            seed(&daemon, AttentionState::NeedsInput, Some("Allow Bash?"), None);
            let before = date::now_ms();
            // TS starts markAnswered without awaiting it: its part up to the save runs as the answer arrives, before
            // any hook queued behind the input. Saving runs the workspace bookkeeping (D6), not ported yet, which
            // panics after the session is held; the state this test checks is in place by then.
            let answered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| daemon.mark_answered("s")));
            assert!(worker.last_hook_at.get().is_some_and(|last| last >= before));
            let held = attention(&daemon).unwrap();
            assert_eq!(held.state, AttentionState::Working);
            assert_eq!(held.event, "Answered");
            assert_eq!(held.reason, None);
            // A hook that started before the answer is stale (this Stop would end the response).
            daemon
                .handle_agent_hook(&hook(json!({"hook_event_name": "Stop"}), Some(before - 1000)))
                .await
                .unwrap();
            assert_eq!(attention(&daemon).unwrap().event, "Answered");
            assert!(worker.last_hook_at.get().is_some_and(|last| last >= before));
            drop(answered);
        });
    }

    #[test]
    fn an_answer_typed_as_input_is_recorded_before_the_input_handler_returns() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        local(async {
            let (daemon, worker, _fake) = setup(dir.path()).await;
            seed(&daemon, AttentionState::NeedsInput, Some("Allow Bash?"), None);
            let before = date::now_ms();
            // TS `input` calls markAnswered (unawaited) right there: an agent-hook request handled next, before any
            // background task runs, must already see the answer.
            daemon
                .handle_input(&deckhand_protocol::wire::Input {
                    session_id: "s".into(),
                    data: "\r".into(),
                })
                .unwrap();
            assert!(worker.last_hook_at.get().is_some_and(|last| last >= before));
            assert_eq!(attention(&daemon).unwrap().event, "Answered");
            daemon
                .handle_agent_hook(&hook(json!({"hook_event_name": "Stop"}), Some(before - 1000)))
                .await
                .unwrap();
            assert_eq!(attention(&daemon).unwrap().event, "Answered");
        });
    }

    #[test]
    fn activity_of_exited_sessions_and_unchanged_activity_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        local(async {
            let (daemon, _worker, _fake) = setup(dir.path()).await;
            daemon.patch_session("s", |session| {
                session.touch("agentStatus");
                session.agent_status = Some(AgentActivityStatus::Idle);
            });
            seed(&daemon, AttentionState::Working, None, None);
            daemon
                .set_worker_agent_status("s", AgentActivityStatus::Idle)
                .await
                .unwrap();
            assert_eq!(state(&daemon), Some(AttentionState::Working));
            assert_eq!(daemon.get_session("s").unwrap().agent_status_updated_at, None);
            daemon.patch_session("s", |session| session.status = SessionStatus::Exited);
            daemon
                .set_worker_agent_status("s", AgentActivityStatus::Active)
                .await
                .unwrap();
            assert_eq!(
                daemon.get_session("s").unwrap().agent_status,
                Some(AgentActivityStatus::Idle)
            );
            daemon
                .set_worker_agent_status("missing", AgentActivityStatus::Active)
                .await
                .unwrap();
        });
    }

    #[test]
    fn a_draft_is_typed_once_the_screen_shows_no_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        local(async {
            let (daemon, worker, mut fake) = setup(dir.path()).await;
            // No draft, or no screen yet: nothing happens.
            daemon.type_draft("s").await.unwrap();
            *worker.draft.borrow_mut() = Some(Draft {
                text: "Fix the bug".into(),
                timer: Timer::after(DRAFT_WAIT, || {}),
                settle: None,
            });
            daemon.type_draft("s").await.unwrap();
            assert!(worker.draft.borrow().is_some());
            // A prompt on screen: the draft waits.
            *worker.screen.borrow_mut() = Some("Do you trust the files in this folder?\n❯ 1. Yes".into());
            daemon.type_draft("s").await.unwrap();
            assert!(worker.draft.borrow().is_some());
            // A blank screen (JS trim: U+FEFF is blank too): still waiting.
            *worker.screen.borrow_mut() = Some(" \n\u{FEFF}".into());
            daemon.type_draft("s").await.unwrap();
            assert!(worker.draft.borrow().is_some());
            // Settled at the input: typed as one bracketed paste, never sent.
            *worker.screen.borrow_mut() = Some("╭───╮\n│ > │\n╰───╯".into());
            daemon.type_draft("s").await.unwrap();
            assert!(worker.draft.borrow().is_none());
            assert_eq!(
                fake.next().await.to_string(),
                r#"{"type":"input","target":"agent","data":"\u001b[200~Fix the bug\u001b[201~"}"#
            );
            let log = log_containing(dir.path(), "daemon typed the task draft into s\n").await;
            assert!(log.contains("daemon typed the task draft into s\n"));
            assert!(!log.contains("not typed"));
            // An exited worker types nothing.
            *worker.draft.borrow_mut() = Some(Draft {
                text: "Again".into(),
                timer: Timer::after(DRAFT_WAIT, || {}),
                settle: None,
            });
            worker.channel.mark_exited();
            daemon.type_draft("s").await.unwrap();
            assert!(worker.draft.borrow().is_some());
        });
    }

    #[test]
    fn dropped_drafts_clear_their_timers_and_log_why() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        local(async {
            let (daemon, worker, _fake) = setup(dir.path()).await;
            daemon.patch_session("s", |session| {
                session.touch("startPrompt");
                session.start_prompt = Some("Task".into());
            });
            let fired = Rc::new(Cell::new(false));
            let flag = Rc::clone(&fired);
            *worker.draft.borrow_mut() = Some(Draft {
                text: "Task".into(),
                timer: Timer::after(Duration::from_millis(10), move || flag.set(true)),
                settle: Some(Timer::after(Duration::from_millis(10), || {})),
            });
            daemon.drop_draft("s", Some("the agent exited"), false);
            assert!(worker.draft.borrow().is_none());
            // Not saved: the start prompt stays.
            assert_eq!(daemon.get_session("s").unwrap().start_prompt.as_deref(), Some("Task"));
            // Nothing to drop: nothing logged.
            daemon.drop_draft("s", Some("again"), true);
            daemon.drop_draft("missing", Some("gone"), true);
            settle().await;
            assert!(!fired.get());
            let log = log_containing(dir.path(), "daemon task draft for s not typed: the agent exited\n").await;
            assert!(log.contains("daemon task draft for s not typed: the agent exited\n"));
            assert!(!log.contains("again"));
            assert!(!log.contains("gone"));
        });
    }

    #[test]
    fn notifications_are_off_unless_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        local(async {
            let daemon = test_daemon(dir.path());
            // No config: returns without running anything.
            daemon.notify(&session("s", "/r", "/r"), "needs input").await;
        });
    }
}
