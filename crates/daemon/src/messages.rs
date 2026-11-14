//! Messages between sessions (not in the TS): `deckhand send`, `deckhand reply` and `deckhand send --cancel`.
//!
//! Every message (an instruction or question, a reply, Deckhand's notice that one was dropped) goes through one queue
//! per target session, in the order sent. A message is **held** while its target is not idle at its input: it starts,
//! works on a turn, waits on a question or permission (never typed into a dialog), shows a menu, has a task draft
//! waiting, or you typed text into it that you have not sent (or typed in the last [`TYPING_QUIET`]): the draft's
//! guard, never typing over you. Once the target is idle, its screen settled ([`crate::hooks::DRAFT_SETTLE`]), the
//! next message is typed as one bracketed paste under its header and submitted with Enter. The next one waits for the
//! turn that one starts to end.
//!
//! **Turns.** For an agent that reports turn signals (a hook arrived during this launch: Claude, Codex with hooks), a
//! turn starts with a working signal and ends with response-ended (or failed, rate-limited, an interrupt or a stale
//! working signal). If no turn starts within [`TURN_START_WAIT`] of the Enter, the message is marked so (its `note`),
//! a waiting `send --wait` is told, and the queue stops until a turn starts or you press Enter in it: nothing is
//! typed again blindly. Without signals the screen's activity stands in: active starts the turn, idle ends it, and
//! no activity within the wait lets the queue go on.
//!
//! **Waiting** (`send --wait`): the request is answered with the first reply, which is then not typed into the
//! sender. It is answered early when the target waits on a question or permission, ends the message's turn without
//! replying (signals only), exits, or did not start a turn; on its timeout too. Replies after that are queued to the
//! sender as messages, so nothing is lost. A reply to a message sent by hand goes back to you (`returned`).
//!
//! **Permissions.** By hand (no caller): any session. From a session: the sessions of its tree, everything under the
//! same top-level session, never itself. A reply answers only a message sent to the caller.
//!
//! **Storage.** `messages.json` in the state directory, written atomically after each change (coalesced): the held
//! messages and the last [`LOG_PER_SESSION`] of each session. It is its own file, not part of `state.json`, so a
//! Deckhand that predates messages never reads or rewrites it, and records another version wrote that this one
//! cannot read are written back untouched. Held messages survive a daemon restart and are delivered once their target
//! runs again and is idle; a waiting `send --wait` does not (its connection ends with the daemon), so later replies
//! are queued to its sender.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use deckhand_core::error::{DhError, Result};
use deckhand_core::jsstr::date;
use deckhand_protocol::data::{
    AgentActivityStatus, AttentionState, MessageKind, MessageRecord, MessageStatus, SendMessageResult, SessionRecord,
    SessionStatus, WaitOutcome, WaitOutcomeKind,
};
use deckhand_protocol::events::{MessagesUpdated, ServerMessage};
use deckhand_protocol::wire::{CancelMessage, ListMessages, ReplyMessage, SendMessage};
use deckhand_sys::async_util::{Coalescer, SharedTask, Timer, spawn};
use deckhand_sys::storage;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::oneshot;

use crate::daemon::Daemon;
use crate::hooks::{DRAFT_SETTLE, agent_input, paste, shows_agent_prompt, types_letters};
use crate::launch::WorkerRuntime;
use crate::server::{ClientHandle, ClientSubscription, send_message};

/// The file in the state directory.
pub const MESSAGES_FILE: &str = "messages.json";

/// The most a message (or reply) may be, in bytes.
pub const MAX_MESSAGE_BYTES: usize = 20 * 1024;

/// How long `send --wait` waits for a reply when the request names no timeout.
pub const DEFAULT_WAIT: Duration = Duration::from_secs(9 * 60);

/// How many messages (sent or received, held ones included) are kept for each session.
pub const LOG_PER_SESSION: usize = 50;

/// How long after the Enter a turn must start (signals: a working signal; else: screen activity).
pub const TURN_START_WAIT: Duration = Duration::from_secs(10);

/// A message waits this long after the last key you typed into its target.
pub const TYPING_QUIET: Duration = Duration::from_secs(3);

/// Between the paste and the Enter that submits it (an Enter inside the paste would only add a line).
pub const SUBMIT_AFTER_PASTE: Duration = Duration::from_millis(150);

/// How much of a message's text is kept once it has left the queue (UTF-16 units).
pub const KEPT_TEXT_UNITS: usize = 2000;

/// How much of a dropped message a notice quotes.
const NOTICE_EXCERPT_UNITS: usize = 300;

/// How often a waiting `send --wait` checks that its command is still there.
const WAIT_POLL: Duration = Duration::from_millis(500);

/// From a session, a target outside its tree.
pub const NOT_IN_TREE: &str = "A session sends messages only to the sessions in its tree: everything under the same \
top-level session (see deckhand ls)";

/// A session naming itself.
pub const TO_ITSELF: &str = "A session cannot send a message to itself";

/// `reply` by hand.
pub const REPLY_BY_HAND: &str = "deckhand reply runs inside the Deckhand session the message was sent to";

/// A `send --wait` whose command is gone.
const WAITER_GONE: &str = "The waiting command went away";

/// A `send --wait` waiting for its message's first reply.
pub struct Waiter {
    /// The connection of the waiting command: a reply for a closed one is queued instead.
    pub client: ClientHandle,
    pub answer: oneshot::Sender<WaitOutcome>,
}

/// The messages' part of the daemon state.
pub struct MessagesState {
    /// Every kept message, oldest first (the queues are the held ones, per target, in this order).
    pub messages: Vec<MessageRecord>,
    /// Records another version wrote that this one cannot read: written back as they were.
    pub foreign: Vec<Value>,
    /// The file's other keys.
    pub file_extra: IndexMap<String, Value>,
    /// Waiting `send --wait`s, by message ID.
    pub waiters: HashMap<String, Waiter>,
    /// The coalesced write of `messages.json` (made by the first save).
    pub save: Option<Coalescer<Result<()>>>,
    /// [`TURN_START_WAIT`], shorter in tests.
    pub turn_start_wait: Duration,
}

impl Default for MessagesState {
    fn default() -> Self {
        MessagesState {
            messages: Vec::new(),
            foreign: Vec::new(),
            file_extra: IndexMap::new(),
            waiters: HashMap::new(),
            save: None,
            turn_start_wait: TURN_START_WAIT,
        }
    }
}

/// A session launch's part of the messages: what holds its queue. Lives on its worker, so a new launch starts afresh.
#[derive(Default)]
pub struct Inbox {
    /// A lifecycle hook arrived during this launch: its turns are known from signals.
    pub signals: Cell<bool>,
    /// You typed text into the agent (or a draft was typed) that has not been sent: Enter, ctrl+c or ctrl+u clear it.
    pub user_text: Cell<bool>,
    /// When you last typed into the agent.
    pub typed_at: Cell<Option<Instant>>,
    /// When its screen last changed.
    pub screen_at: Cell<Option<Instant>>,
    /// A message is being typed (between its paste and its Enter).
    pub delivering: Cell<bool>,
    /// The turn the last delivered message starts, until it ends.
    pub turn: RefCell<Option<MessageTurn>>,
    /// Looks at the queue again once a typing pause or a screen settle has passed.
    pub retry: RefCell<Option<Timer>>,
}

/// The turn a delivered message starts.
pub struct MessageTurn {
    pub message_id: String,
    pub started: bool,
    /// No turn started after the Enter (signals): the queue stops until one does or you press Enter.
    pub stalled: bool,
    /// [`TURN_START_WAIT`] after the Enter.
    pub timer: Option<Timer>,
}

/// Whether a target's queue can move.
#[derive(Debug, PartialEq, Eq)]
enum Hold {
    Ready,
    /// Look again after this (typing pause, screen settle).
    Retry(Duration),
    /// An event (activity, a signal, Enter, running) looks again.
    Wait,
}

/// `messages.json`.
#[derive(Default, Serialize, Deserialize)]
struct MessagesFile {
    #[serde(default)]
    messages: Vec<Value>,
    #[serde(flatten)]
    extra: IndexMap<String, Value>,
}

fn now_iso() -> String {
    date::to_iso_string(date::now_ms()).unwrap_or_default()
}

/// `text` cut to `units` UTF-16 units (never inside a character), and whether it was cut.
fn cut(text: &str, units: usize) -> (String, bool) {
    let mut used = 0;
    let mut end = text.len();
    for (at, character) in text.char_indices() {
        used += character.len_utf16();
        if used > units {
            end = at;
            break;
        }
    }
    (text[..end].to_owned(), end < text.len())
}

/// Keeps at most [`KEPT_TEXT_UNITS`] of a message that left the queue.
fn shorten(message: &mut MessageRecord) {
    let (kept, cut_short) = cut(&message.text, KEPT_TEXT_UNITS);
    if cut_short {
        message.text = kept;
        message.text_truncated = Some(true);
    }
}

/// A new short ID (6 hex digits) none of `taken` has.
fn new_id(taken: &HashSet<&str>) -> String {
    loop {
        let id: String = uuid::Uuid::new_v4().simple().to_string().chars().take(6).collect();
        if !taken
            .iter()
            .any(|other| other.starts_with(&id) || id.starts_with(*other))
        {
            return id;
        }
    }
}

/// The text of a message or reply: refused when blank or past [`MAX_MESSAGE_BYTES`].
fn checked_text(text: Option<&str>, what: &str) -> Result<String> {
    let text = text.unwrap_or_default();
    if text.trim().is_empty() {
        return Err(DhError::msg(format!("The {what} is empty")));
    }
    if text.len() > MAX_MESSAGE_BYTES {
        return Err(DhError::msg(format!(
            "The {what} is {} KB; at most {} KB fits. Put the details in your line's note (deckhand note add) and \
             point at it",
            text.len().div_ceil(1024),
            MAX_MESSAGE_BYTES / 1024
        )));
    }
    Ok(text.to_owned())
}

/// The bracketed line(s) above a message, as its receiver sees them (a notice is its own text).
pub fn header(message: &MessageRecord) -> Option<String> {
    let id = &message.id;
    let from = match (&message.from_session_id, &message.from_title) {
        (Some(_), Some(title)) => format!("from \"{title}\""),
        (Some(id), None) => format!("from session {id}"),
        (None, _) => "from you (the user)".to_owned(),
    };
    match message.kind {
        MessageKind::Notice => None,
        MessageKind::Reply => {
            let to = message.in_reply_to.as_deref().unwrap_or_default();
            Some(format!("[Reply {from} to your message {to}]"))
        }
        MessageKind::Message if message.wants_answer == Some(true) => Some(format!(
            "[Message {id} {from}, which wants an answer.\n Reply once you have it: deckhand reply {id} \"<answer>\"]"
        )),
        MessageKind::Message => Some(format!("[Message {id} {from} via deckhand]")),
    }
}

/// What is typed into the target: the header, then the text.
pub fn rendered(message: &MessageRecord) -> String {
    match header(message) {
        Some(header) => format!("{header}\n{}", message.text),
        None => message.text.clone(),
    }
}

/// Keeps the held messages and the last [`LOG_PER_SESSION`] of each session in `known` (sent or received); a
/// message of no known session goes.
pub fn prune(messages: &mut Vec<MessageRecord>, known: &HashSet<String>) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut keep = vec![false; messages.len()];
    for (at, message) in messages.iter().enumerate().rev() {
        let held = message.status == MessageStatus::Held;
        for session in [message.from_session_id.as_ref(), message.to_session_id.as_ref()]
            .into_iter()
            .flatten()
            .filter(|session| known.contains(*session))
        {
            let count = counts.entry(session.clone()).or_default();
            if *count < LOG_PER_SESSION || held {
                keep[at] = true;
            }
            *count += 1;
        }
    }
    let mut flags = keep.into_iter();
    messages.retain(|_| flags.next().unwrap_or(false));
}

/// What typed `data` does to the text in the agent's input: `Some(true)` leaves text there (letters, a paste),
/// `Some(false)` sends or clears it (Enter, ctrl+c, ctrl+u after the last letters), `None` neither.
pub fn typed_effect(data: &str) -> Option<bool> {
    if data.contains("\x1b[200~") {
        return Some(true);
    }
    match data.rfind(['\r', '\x03', '\x15']) {
        Some(at) => Some(types_letters(data.get(at + 1..).unwrap_or_default())),
        None => types_letters(data).then_some(true),
    }
}

impl Daemon {
    fn messages_path(&self) -> PathBuf {
        self.paths.config_dir.join(MESSAGES_FILE)
    }

    fn worker(&self, session_id: &str) -> Option<Rc<WorkerRuntime>> {
        self.launch.borrow().workers.get(session_id).cloned()
    }

    /// Reads `messages.json` at startup: held messages whose target is gone are dropped; the rest wait for their
    /// target to run again.
    pub async fn load_messages(&self) -> Result<()> {
        let path = self.messages_path();
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(DhError::io(&error, "open", &path.to_string_lossy())),
        };
        let file: MessagesFile = match serde_json::from_str(&text) {
            Ok(file) => file,
            Err(error) => {
                // Kept aside rather than overwritten by the next save.
                let aside = self.paths.config_dir.join(format!("{MESSAGES_FILE}.unreadable"));
                let _ = tokio::fs::rename(&path, &aside).await;
                self.log(&format!(
                    "{MESSAGES_FILE} is unreadable ({error}); moved to {} and starting empty",
                    aside.to_string_lossy()
                ))
                .await;
                return Ok(());
            }
        };
        let mut messages = Vec::new();
        let mut foreign = Vec::new();
        for value in file.messages {
            match serde_json::from_value::<MessageRecord>(value.clone()) {
                Ok(message) => messages.push(message),
                Err(_) => foreign.push(value),
            }
        }
        let gone: Vec<String> = messages
            .iter()
            .filter(|message| message.status == MessageStatus::Held)
            .filter(|message| {
                message
                    .to_session_id
                    .as_deref()
                    .is_none_or(|to| self.get_session(to).is_none())
            })
            .map(|message| message.id.clone())
            .collect();
        {
            let mut state = self.messages.borrow_mut();
            state.messages = messages;
            state.foreign = foreign;
            state.file_extra = file.extra;
        }
        for id in gone {
            self.update_message(&id, |message| {
                message.status = MessageStatus::Dropped;
                message.note = Some("Its session was removed before it was delivered".to_owned());
                shorten(message);
            });
        }
        let held = self
            .messages
            .borrow()
            .messages
            .iter()
            .filter(|m| m.status == MessageStatus::Held)
            .count();
        if held > 0 {
            self.log(&format!("{held} held message(s) wait for their sessions"))
                .await;
        }
        Ok(())
    }

    /// Writes `messages.json` (coalesced: callers before the queued write starts share it).
    pub fn save_messages(&self) -> SharedTask<Result<()>> {
        let coalescer = {
            let mut state = self.messages.borrow_mut();
            state
                .save
                .get_or_insert_with(|| {
                    let weak = self.downgrade();
                    Coalescer::new(move || {
                        let contents = weak.upgrade().map(|daemon| {
                            let state = daemon.messages.borrow();
                            let mut messages: Vec<Value> = state
                                .messages
                                .iter()
                                .filter_map(|message| serde_json::to_value(message).ok())
                                .collect();
                            messages.extend(state.foreign.iter().cloned());
                            let file = MessagesFile {
                                messages,
                                extra: state.file_extra.clone(),
                            };
                            (daemon.messages_path(), serde_json::to_string_pretty(&file))
                        });
                        async move {
                            match contents {
                                Some((path, Ok(text))) => {
                                    storage::write_atomic(&path, format!("{text}\n").as_bytes(), storage::FILE_MODE)
                                        .await
                                }
                                Some((_, Err(error))) => Err(DhError::msg(error.to_string())),
                                None => Ok(()),
                            }
                        }
                    })
                })
                .clone()
        };
        coalescer.request()
    }

    /// The kept message `id`.
    pub fn message(&self, id: &str) -> Option<MessageRecord> {
        self.messages
            .borrow()
            .messages
            .iter()
            .find(|message| message.id == id)
            .cloned()
    }

    /// Changes message `id` (its `updatedAt` too); the message as it is now.
    fn update_message(&self, id: &str, change: impl FnOnce(&mut MessageRecord)) -> Option<MessageRecord> {
        let mut state = self.messages.borrow_mut();
        let message = state.messages.iter_mut().find(|message| message.id == id)?;
        change(message);
        message.updated_at = now_iso();
        Some(message.clone())
    }

    /// After a change: prunes the log, writes the file (in the background) and tells the clients.
    fn publish_messages(&self, changed: &[MessageRecord]) {
        let known: HashSet<String> = self.sessions.borrow().sessions.keys().cloned().collect();
        prune(&mut self.messages.borrow_mut().messages, &known);
        let saved = self.save_messages();
        let daemon = self.clone();
        drop(spawn(async move {
            if let Err(error) = saved.await {
                daemon.log(&format!("saving {MESSAGES_FILE} failed: {error}")).await;
            }
        }));
        self.broadcast_messages(changed);
    }

    /// `messages-updated` to each client in scope of a changed message's sender or target.
    fn broadcast_messages(&self, changed: &[MessageRecord]) {
        if changed.is_empty() {
            return;
        }
        let clients: Vec<(ClientHandle, ClientSubscription)> = self
            .server
            .borrow()
            .clients
            .values()
            .map(|client| (client.handle.clone(), client.subscription.clone()))
            .collect();
        for (handle, subscription) in clients {
            let messages: Vec<MessageRecord> = changed
                .iter()
                .filter(|message| self.message_in_scope(message, &subscription))
                .cloned()
                .collect();
            if !messages.is_empty() {
                send_message(&handle, &ServerMessage::MessagesUpdated(MessagesUpdated { messages }));
            }
        }
    }

    fn message_in_scope(&self, message: &MessageRecord, subscription: &ClientSubscription) -> bool {
        [message.from_session_id.as_deref(), message.to_session_id.as_deref()]
            .into_iter()
            .flatten()
            .filter_map(|id| self.get_session(id))
            .any(|session| self.in_client_scope(&session, subscription))
    }

    /// The message a prefix names.
    fn find_message(&self, prefix: &str) -> Result<MessageRecord> {
        let prefix = prefix.trim().to_ascii_lowercase();
        if prefix.is_empty() {
            return Err(DhError::msg("Name a message by its ID (the one in its header)"));
        }
        let state = self.messages.borrow();
        let mut found = state.messages.iter().filter(|message| message.id.starts_with(&prefix));
        match (found.next(), found.next()) {
            (Some(message), None) => Ok(message.clone()),
            (Some(_), Some(_)) => Err(DhError::msg(format!(
                "{prefix} names several messages; give more of its ID"
            ))),
            (None, _) => Err(DhError::msg(format!("No message {prefix}"))),
        }
    }

    /// The top-level session above `session_id` (itself when it has no parent).
    fn tree_root(&self, session_id: &str) -> String {
        let mut current = session_id.to_owned();
        let mut seen = HashSet::new();
        while seen.insert(current.clone()) {
            match self
                .get_session(&current)
                .and_then(|session| session.parent_session_id.clone())
                .filter(|parent| self.get_session(parent).is_some())
            {
                Some(parent) => current = parent,
                None => break,
            }
        }
        current
    }

    fn taken_ids(&self) -> HashSet<String> {
        self.messages
            .borrow()
            .messages
            .iter()
            .map(|message| message.id.clone())
            .collect()
    }

    fn new_message(
        &self,
        kind: MessageKind,
        from: Option<&SessionRecord>,
        to: Option<&SessionRecord>,
        text: String,
    ) -> MessageRecord {
        let taken = self.taken_ids();
        let now = now_iso();
        MessageRecord {
            id: new_id(&taken.iter().map(String::as_str).collect()),
            kind,
            from_session_id: from.map(|session| session.id.clone()),
            from_title: from.map(|session| session.title.clone()),
            to_session_id: to.map(|session| session.id.clone()),
            to_title: to.map(|session| session.title.clone()),
            text,
            text_truncated: None,
            wants_answer: None,
            in_reply_to: None,
            replies: None,
            status: MessageStatus::Held,
            created_at: now.clone(),
            delivered_at: None,
            updated_at: now,
            note: None,
            extra: IndexMap::new(),
        }
    }

    /// `send-message`: queues the message; with `wait`, answers with how the wait ended.
    pub async fn handle_send_message(&self, client: &ClientHandle, request: &SendMessage) -> Result<SendMessageResult> {
        let caller = request
            .caller
            .as_ref()
            .map(|caller| self.authenticate_caller(caller))
            .transpose()?;
        let target = self.require_session(&request.session_id)?;
        if let Some(caller) = &caller {
            if caller.id == target.id {
                return Err(DhError::msg(TO_ITSELF));
            }
            if self.tree_root(&caller.id) != self.tree_root(&target.id) {
                return Err(DhError::msg(NOT_IN_TREE));
            }
        }
        let text = checked_text(request.text.as_deref(), "message")?;
        if target.status == SessionStatus::Exited {
            return Err(DhError::msg(format!(
                "\"{}\" has exited: nothing was sent",
                target.title
            )));
        }
        let wait = request.wait == Some(true);
        let mut message = self.new_message(MessageKind::Message, caller.as_deref(), Some(&target), text);
        if wait {
            message.wants_answer = Some(true);
        }
        let id = message.id.clone();
        self.messages.borrow_mut().messages.push(message.clone());
        let answered = wait.then(|| {
            let (answer, answered) = oneshot::channel();
            self.messages.borrow_mut().waiters.insert(
                id.clone(),
                Waiter {
                    client: client.clone(),
                    answer,
                },
            );
            answered
        });
        self.publish_messages(std::slice::from_ref(&message));
        let from = caller
            .as_ref()
            .map_or_else(|| "you".to_owned(), |caller| caller.id.clone());
        self.log(&format!("message {id} from {from} to {} queued", target.id))
            .await;
        // Already waiting on a question or permission: a waiting sender hears it at once.
        if wait {
            self.tell_waiters_waiting(&target.id);
        }
        self.pump_messages(&target.id);
        let outcome = match answered {
            Some(answered) => {
                let timeout = request
                    .timeout_ms
                    .map_or(DEFAULT_WAIT, |ms| Duration::from_millis(u64::try_from(ms).unwrap_or(0)));
                Some(self.wait_for_answer(client, &id, answered, timeout).await?)
            }
            None => None,
        };
        let message = self.message(&id).unwrap_or(message);
        Ok(SendMessageResult { message, outcome })
    }

    /// Waits for message `id`'s waiter to be answered, up to `timeout`; gives up when the command is gone.
    async fn wait_for_answer(
        &self,
        client: &ClientHandle,
        id: &str,
        mut answered: oneshot::Receiver<WaitOutcome>,
        timeout: Duration,
    ) -> Result<WaitOutcome> {
        let timed_out = || WaitOutcome {
            kind: WaitOutcomeKind::Timeout,
            reply: None,
            reason: None,
        };
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let poll = tokio::time::sleep_until(deadline.min(tokio::time::Instant::now() + WAIT_POLL));
            tokio::select! {
                outcome = &mut answered => {
                    return match outcome {
                        Ok(outcome) => Ok(outcome),
                        Err(_) if client.is_destroyed() => Err(DhError::msg(WAITER_GONE)),
                        Err(_) => Ok(timed_out()),
                    };
                }
                () = poll => {}
            }
            if client.is_destroyed() {
                self.messages.borrow_mut().waiters.remove(id);
                return Err(DhError::msg(WAITER_GONE));
            }
            if tokio::time::Instant::now() >= deadline {
                let removed = self.messages.borrow_mut().waiters.remove(id).is_some();
                if removed {
                    self.log_soon(format!("message {id}: the wait for a reply timed out"));
                    return Ok(timed_out());
                }
                // Answered as it timed out: the answer is in.
                return Ok(answered.try_recv().unwrap_or_else(|_| timed_out()));
            }
        }
    }

    /// Hands `outcome` to the `send --wait` waiting on message `id`; false when none is waiting (or its command is
    /// gone).
    fn resolve_wait(&self, id: &str, outcome: WaitOutcome) -> bool {
        let waiter = self.messages.borrow_mut().waiters.remove(id);
        let Some(waiter) = waiter else {
            return false;
        };
        !waiter.client.is_destroyed() && waiter.answer.send(outcome).is_ok()
    }

    fn outcome(kind: WaitOutcomeKind) -> WaitOutcome {
        WaitOutcome {
            kind,
            reply: None,
            reason: None,
        }
    }

    /// `reply-message`: answers a message sent to the caller. The reply goes to the sender's waiting `send --wait`,
    /// else into the sender's queue (by hand: back to you).
    pub async fn handle_reply_message(&self, request: &ReplyMessage) -> Result<MessageRecord> {
        let Some(caller) = &request.caller else {
            return Err(DhError::msg(REPLY_BY_HAND));
        };
        let caller = self.authenticate_caller(caller)?;
        let original = self.find_message(&request.message_id)?;
        if original.to_session_id.as_deref() != Some(caller.id.as_str()) {
            return Err(DhError::msg(format!(
                "Message {} was not sent to this session: deckhand reply answers only the messages sent to the session \
                 it runs in",
                original.id
            )));
        }
        if original.kind == MessageKind::Notice {
            return Err(DhError::msg(format!(
                "Message {} is a notice from Deckhand: there is nobody to answer",
                original.id
            )));
        }
        if matches!(
            original.status,
            MessageStatus::Held | MessageStatus::Cancelled | MessageStatus::Dropped
        ) {
            return Err(DhError::msg(format!(
                "Message {} has not reached this session",
                original.id
            )));
        }
        let text = checked_text(request.text.as_deref(), "answer")?;
        let sender = original.from_session_id.as_deref().and_then(|id| self.get_session(id));
        let mut reply = self.new_message(MessageKind::Reply, Some(&caller), sender.as_deref(), text);
        if sender.is_none()
            && let Some(id) = &original.from_session_id
        {
            reply.to_session_id = Some(id.clone());
            reply.to_title.clone_from(&original.from_title);
        }
        reply.in_reply_to = Some(original.id.clone());
        reply.status = MessageStatus::Returned;
        let returned = self.resolve_wait(
            &original.id,
            WaitOutcome {
                kind: WaitOutcomeKind::Replied,
                reply: Some(Box::new(reply.clone())),
                reason: None,
            },
        );
        if !returned && original.from_session_id.is_some() {
            match &sender {
                Some(sender) if sender.status != SessionStatus::Exited => reply.status = MessageStatus::Held,
                _ => {
                    return Err(DhError::msg(format!(
                        "\"{}\" has exited: the reply cannot reach it",
                        original.from_title.as_deref().unwrap_or("Its sender")
                    )));
                }
            }
        }
        self.messages.borrow_mut().messages.push(reply.clone());
        let reply_id = reply.id.clone();
        let original = self.update_message(&original.id, |message| {
            message.status = MessageStatus::Answered;
            message.replies.get_or_insert_with(Vec::new).push(reply_id.clone());
        });
        let mut changed = vec![reply.clone()];
        changed.extend(original);
        self.publish_messages(&changed);
        let to = match reply.status {
            MessageStatus::Returned if reply.to_session_id.is_some() => "the waiting command".to_owned(),
            MessageStatus::Returned => "you".to_owned(),
            _ => reply.to_session_id.clone().unwrap_or_default(),
        };
        self.log(&format!("reply {reply_id} from {} to {to}", caller.id)).await;
        if let Some(to) = reply
            .to_session_id
            .as_deref()
            .filter(|_| reply.status == MessageStatus::Held)
        {
            self.pump_messages(to);
        }
        Ok(self.message(&reply_id).unwrap_or(reply))
    }

    /// `cancel-message`: cancels a held message the caller sent (by hand: any).
    pub async fn handle_cancel_message(&self, request: &CancelMessage) -> Result<MessageRecord> {
        let caller = request
            .caller
            .as_ref()
            .map(|caller| self.authenticate_caller(caller))
            .transpose()?;
        let message = self.find_message(&request.message_id)?;
        if let Some(caller) = &caller
            && message.from_session_id.as_deref() != Some(caller.id.as_str())
        {
            return Err(DhError::msg(format!(
                "Message {} was not sent by this session",
                message.id
            )));
        }
        if message.status != MessageStatus::Held {
            return Err(DhError::msg(format!(
                "Message {} is {}: only a held message can be cancelled",
                message.id, message.status
            )));
        }
        let cancelled = self.update_message(&message.id, |message| {
            message.status = MessageStatus::Cancelled;
            shorten(message);
        });
        self.resolve_wait(&message.id, Self::outcome(WaitOutcomeKind::Cancelled));
        let cancelled = cancelled.unwrap_or(message);
        self.publish_messages(std::slice::from_ref(&cancelled));
        self.log(&format!("message {} cancelled", cancelled.id)).await;
        Ok(cancelled)
    }

    /// `list-messages`.
    pub fn handle_list_messages(&self, request: &ListMessages) -> Result<Vec<MessageRecord>> {
        let scope = request.repo_root.as_ref().map(|root| ClientSubscription {
            repo_root: Some(root.clone()),
            ..ClientSubscription::default()
        });
        let messages = self.messages.borrow().messages.clone();
        Ok(messages
            .into_iter()
            .filter(|message| {
                request.session_id.as_ref().is_none_or(|id| {
                    message.from_session_id.as_ref() == Some(id) || message.to_session_id.as_ref() == Some(id)
                })
            })
            .filter(|message| scope.as_ref().is_none_or(|scope| self.message_in_scope(message, scope)))
            .collect())
    }

    /// The held messages to `target`, in queue order.
    fn held_to(&self, target: &str) -> Vec<MessageRecord> {
        self.messages
            .borrow()
            .messages
            .iter()
            .filter(|message| message.status == MessageStatus::Held && message.to_session_id.as_deref() == Some(target))
            .cloned()
            .collect()
    }

    /// Whether `target`'s queue can move now.
    fn delivery_hold(&self, target: &str) -> Hold {
        let (Some(session), Some(worker)) = (self.get_session(target), self.worker(target)) else {
            return Hold::Wait;
        };
        if session.status != SessionStatus::Running || worker.channel.is_exited() {
            return Hold::Wait;
        }
        let inbox = &worker.inbox;
        if inbox.delivering.get() || inbox.turn.borrow().is_some() {
            return Hold::Wait;
        }
        let state = session.attention.as_ref().map(|attention| attention.state);
        if matches!(state, Some(AttentionState::NeedsInput | AttentionState::Working)) {
            return Hold::Wait;
        }
        if session.agent_status != Some(AgentActivityStatus::Idle)
            || worker.draft.borrow().is_some()
            || inbox.user_text.get()
        {
            return Hold::Wait;
        }
        if let Some(left) = inbox
            .typed_at
            .get()
            .and_then(|at| TYPING_QUIET.checked_sub(at.elapsed()))
            .filter(|left| !left.is_zero())
        {
            return Hold::Retry(left);
        }
        let at_input = worker
            .screen
            .borrow()
            .as_deref()
            .is_some_and(|screen| !screen.trim().is_empty() && !shows_agent_prompt(screen));
        if !at_input {
            return Hold::Wait;
        }
        if let Some(left) = inbox
            .screen_at
            .get()
            .and_then(|at| DRAFT_SETTLE.checked_sub(at.elapsed()))
            .filter(|left| !left.is_zero())
        {
            return Hold::Retry(left);
        }
        Hold::Ready
    }

    /// Delivers `target`'s next held message if it is idle at its input, or looks again later.
    pub fn pump_messages(&self, target: &str) {
        let Some(next) = self.held_to(target).into_iter().next() else {
            return;
        };
        match self.delivery_hold(target) {
            Hold::Ready => {
                if let Some(worker) = self.worker(target) {
                    self.deliver(target, &worker, next);
                }
            }
            Hold::Retry(after) => self.retry_messages(target, after),
            Hold::Wait => {}
        }
    }

    /// Looks at `target`'s queue again after `delay` (replacing an earlier wait).
    fn retry_messages(&self, target: &str, delay: Duration) {
        let Some(worker) = self.worker(target) else {
            return;
        };
        let weak = self.downgrade();
        let id = target.to_owned();
        let timer = Timer::after(delay, move || {
            if let Some(daemon) = weak.upgrade() {
                daemon.pump_messages(&id);
            }
        });
        *worker.inbox.retry.borrow_mut() = Some(timer);
    }

    /// Types `message` into `target` as one bracketed paste under its header, then Enter.
    fn deliver(&self, target: &str, worker: &Rc<WorkerRuntime>, message: MessageRecord) {
        let id = message.id.clone();
        worker.inbox.delivering.set(true);
        *worker.inbox.turn.borrow_mut() = Some(MessageTurn {
            message_id: id.clone(),
            started: false,
            stalled: false,
            timer: None,
        });
        let text = rendered(&message);
        let delivered = self.update_message(&id, |message| {
            message.status = MessageStatus::Delivered;
            message.delivered_at = Some(now_iso());
            shorten(message);
        });
        self.publish_messages(&delivered.into_iter().collect::<Vec<_>>());
        self.send_worker_event(target, agent_input(&paste(&text)));
        let daemon = self.clone();
        let worker = Rc::clone(worker);
        let target = target.to_owned();
        drop(spawn(async move {
            tokio::time::sleep(SUBMIT_AFTER_PASTE).await;
            if daemon.is_current_worker(&target, &worker) && !worker.channel.is_exited() {
                daemon.send_worker_event(&target, agent_input("\r"));
            }
            worker.inbox.delivering.set(false);
            let wait = daemon.messages.borrow().turn_start_wait;
            let weak = daemon.downgrade();
            let (session, message) = (target.clone(), id.clone());
            let timer = Timer::after(wait, move || {
                if let Some(daemon) = weak.upgrade() {
                    daemon.check_turn_started(&session, &message);
                }
            });
            if let Some(turn) = worker.inbox.turn.borrow_mut().as_mut()
                && turn.message_id == id
                && !turn.started
            {
                turn.timer = Some(timer);
            }
            daemon.log(&format!("delivered message {id} to {target}")).await;
        }));
    }

    /// [`TURN_START_WAIT`] after a delivery: with signals and no turn, the message is marked and the queue stops;
    /// without signals, the queue goes on.
    fn check_turn_started(&self, target: &str, message_id: &str) {
        let Some(worker) = self.worker(target) else {
            return;
        };
        let signals = worker.inbox.signals.get();
        {
            let mut turn = worker.inbox.turn.borrow_mut();
            match turn.as_mut() {
                Some(current) if current.message_id == message_id && !current.started => {
                    if signals {
                        current.stalled = true;
                        current.timer = None;
                    } else {
                        *turn = None;
                    }
                }
                _ => return,
            }
        }
        if !signals {
            self.pump_messages(target);
            return;
        }
        let noted = self.update_message(message_id, |message| {
            message.note =
                Some("Typed, but no turn started: it may still be in the input. Nothing was typed again".to_owned());
        });
        self.publish_messages(&noted.into_iter().collect::<Vec<_>>());
        self.resolve_wait(message_id, Self::outcome(WaitOutcomeKind::NotStarted));
        self.log_soon(format!(
            "message {message_id}: no turn started in {target} after it was typed; its queue waits"
        ));
    }

    /// A turn started in `target` (a working signal, or activity without signals).
    fn turn_started(&self, worker: &WorkerRuntime) {
        if let Some(turn) = worker.inbox.turn.borrow_mut().as_mut() {
            turn.started = true;
            turn.stalled = false;
            turn.timer = None;
        }
    }

    /// The turn a message started ended; `answered_by_signal`: it ended with response-ended (or failed, limited), so
    /// a sender still waiting learns its message ended without a reply.
    fn turn_ended(&self, worker: &WorkerRuntime, answered_by_signal: bool) {
        let ended = {
            let mut turn = worker.inbox.turn.borrow_mut();
            match turn.as_ref() {
                Some(current) if current.started => turn.take(),
                _ => None,
            }
        };
        if let Some(turn) = ended
            && answered_by_signal
        {
            self.resolve_wait(&turn.message_id, Self::outcome(WaitOutcomeKind::Ended));
        }
    }

    /// Senders waiting on messages to `target` (held or delivered, not answered) learn that it waits on a question
    /// or permission, when it does.
    fn tell_waiters_waiting(&self, target: &str) {
        let Some(session) = self.get_session(target) else {
            return;
        };
        let Some(attention) = session.attention.as_ref().filter(|attention| {
            session.status == SessionStatus::Running && attention.state == AttentionState::NeedsInput
        }) else {
            return;
        };
        let reason = attention.reason.as_ref().map(|reason| reason.text.clone());
        let waiting: Vec<String> = {
            let state = self.messages.borrow();
            state
                .messages
                .iter()
                .filter(|message| {
                    message.to_session_id.as_deref() == Some(target)
                        && matches!(message.status, MessageStatus::Held | MessageStatus::Delivered)
                        && state.waiters.contains_key(&message.id)
                })
                .map(|message| message.id.clone())
                .collect()
        };
        for id in waiting {
            self.resolve_wait(
                &id,
                WaitOutcome {
                    kind: WaitOutcomeKind::Waiting,
                    reply: None,
                    reason: reason.clone(),
                },
            );
        }
    }

    /// After a lifecycle hook: the session reports signals; a working signal starts a message's turn, an ended turn
    /// ends it, a question tells waiting senders; then the queue moves if it can.
    pub fn messages_on_signal(&self, target: &str) {
        if let Some(worker) = self.worker(target) {
            worker.inbox.signals.set(true);
        }
        self.messages_on_attention(target);
    }

    /// After the session's attention changed (a hook, an answer, a stale working signal).
    pub fn messages_on_attention(&self, target: &str) {
        let (Some(session), Some(worker)) = (self.get_session(target), self.worker(target)) else {
            return;
        };
        // An attention carried over a relaunch is the previous launch's: no turn of this one ended.
        let attention = session
            .attention
            .as_ref()
            .filter(|attention| attention.carried.is_none());
        match attention.map(|attention| attention.state) {
            Some(AttentionState::Working) => self.turn_started(&worker),
            Some(AttentionState::NeedsInput) => self.tell_waiters_waiting(target),
            Some(AttentionState::ResponseEnded | AttentionState::Failed | AttentionState::Limited) => {
                self.turn_ended(&worker, true);
            }
            Some(AttentionState::Unknown) | None => self.turn_ended(&worker, false),
        }
        self.pump_messages(target);
    }

    /// After the session's screen activity changed: without signals it starts and ends a message's turn.
    pub fn messages_on_activity(&self, target: &str, status: AgentActivityStatus) {
        let Some(worker) = self.worker(target) else {
            return;
        };
        if !worker.inbox.signals.get() {
            match status {
                AgentActivityStatus::Active => self.turn_started(&worker),
                AgentActivityStatus::Idle => self.turn_ended(&worker, false),
                _ => {}
            }
        }
        self.pump_messages(target);
    }

    /// After a screen update: a held message waits for the screen to settle.
    pub fn messages_on_screen(&self, target: &str, worker: &WorkerRuntime) {
        worker.inbox.screen_at.set(Some(Instant::now()));
        if !self.held_to(target).is_empty() {
            self.retry_messages(target, DRAFT_SETTLE);
        }
    }

    /// After you typed `data` into the agent: text left in its input holds the queue until sent or cleared, and every
    /// key holds it for [`TYPING_QUIET`].
    pub fn messages_on_input(&self, target: &str, data: &str) {
        let Some(worker) = self.worker(target) else {
            return;
        };
        worker.inbox.typed_at.set(Some(Instant::now()));
        match typed_effect(data) {
            Some(true) => worker.inbox.user_text.set(true),
            Some(false) => {
                worker.inbox.user_text.set(false);
                // You sent what was in the input: a stalled message is yours to deal with now.
                let mut turn = worker.inbox.turn.borrow_mut();
                if turn.as_ref().is_some_and(|turn| turn.stalled) {
                    *turn = None;
                }
            }
            None => {}
        }
        if !self.held_to(target).is_empty() {
            self.retry_messages(target, TYPING_QUIET);
        }
    }

    /// A task draft was typed into the agent's input: it holds the queue as your own text would.
    pub fn messages_on_draft_typed(&self, worker: &WorkerRuntime) {
        worker.inbox.user_text.set(true);
    }

    /// `target` exited (or was removed): its held messages are dropped, their senders told (a waiting `send --wait`,
    /// else a notice in the sender's queue), and senders waiting on its delivered messages learn it exited.
    pub fn messages_on_exit(&self, target: &str) {
        let title = self
            .get_session(target)
            .map_or_else(|| target.to_owned(), |session| session.title.clone());
        let affected: Vec<MessageRecord> = self
            .messages
            .borrow()
            .messages
            .iter()
            .filter(|message| {
                message.to_session_id.as_deref() == Some(target)
                    && matches!(message.status, MessageStatus::Held | MessageStatus::Delivered)
            })
            .cloned()
            .collect();
        let mut changed = Vec::new();
        let mut notices = Vec::new();
        for message in affected {
            let told = self.resolve_wait(&message.id, Self::outcome(WaitOutcomeKind::Exited));
            if message.status != MessageStatus::Held {
                continue;
            }
            let original = message.text.clone();
            if let Some(dropped) = self.update_message(&message.id, |message| {
                message.status = MessageStatus::Dropped;
                message.note = Some(format!("\"{title}\" exited before it was delivered"));
                shorten(message);
            }) {
                changed.push(dropped);
            }
            let sender = message
                .from_session_id
                .as_deref()
                .filter(|from| *from != target)
                .and_then(|from| self.get_session(from))
                .filter(|sender| sender.status != SessionStatus::Exited);
            if told || message.kind == MessageKind::Notice {
                continue;
            }
            if let Some(sender) = sender {
                let (excerpt, cut_short) = cut(&original, NOTICE_EXCERPT_UNITS);
                let ellipsis = if cut_short { "…" } else { "" };
                let text = format!(
                    "[Not delivered: \"{title}\" exited]\nYour message {}: {excerpt}{ellipsis}",
                    message.id
                );
                let mut notice = self.new_message(MessageKind::Notice, None, Some(&sender), text);
                notice.in_reply_to = Some(message.id.clone());
                self.messages.borrow_mut().messages.push(notice.clone());
                notices.push(notice);
            }
        }
        changed.extend(notices.iter().cloned());
        if changed.is_empty() {
            return;
        }
        self.publish_messages(&changed);
        self.log_soon(format!(
            "{target} exited: {} held message(s) dropped",
            changed.len() - notices.len()
        ));
        for notice in notices {
            if let Some(to) = notice.to_session_id.as_deref() {
                self.pump_messages(to);
            }
        }
    }
}

#[cfg(test)]
mod tests;
