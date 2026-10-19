//! Port of daemon.ts (hook events from `deckhand hook`).
//!
//! Agents report their lifecycle through hooks (`deckhand hook`, relayed as `agent-hook` requests authenticated by
//! the launch's token): the session's attention state (working, needs input, ended, ...), its conversation ID, and
//! desktop notifications. The worker's activity heuristic (`agent-status`) settles stale states. A session started
//! from a task gets the task typed (never sent) into its agent once the screen settles at its input (the draft).

use std::time::Duration;

use deckhand_core::error::Result;
use deckhand_protocol::data::{AgentActivityStatus, AgentSessionRef, SessionRecord};
use deckhand_protocol::wire::AgentHook;

use crate::daemon::Daemon;

/// TS: daemon.ts DRAFT_WAIT_MS: how long a task's draft waits for the agent to settle at its input.
pub const DRAFT_WAIT: Duration = Duration::from_millis(120_000);

/// TS: daemon.ts DRAFT_SETTLE_MS: the screen has settled once unchanged this long.
pub const DRAFT_SETTLE: Duration = Duration::from_millis(700);

/// TS: daemon.ts AGENT_PROMPT_PATTERN (`/.../im`): a menu or question on the agent's screen, where a draft is never
/// typed (folder-trust prompts, update offers).
pub const AGENT_PROMPT_PATTERN: &str = r"(?im)do you trust|trust (?:the files|the contents|this folder|this directory)|press enter to continue|^\s*[❯›>]\s*1\.\s";

/// The hooks' part of the daemon state (the per-launch hook state lives on the session's worker).
#[derive(Debug, Default)]
pub struct HooksState {}

/// TS: `AGENT_PROMPT_PATTERN.test(screen)`.
pub fn shows_agent_prompt(screen: &str) -> bool {
    let _ = screen;
    todo!("D5")
}

/// TS: handleRequest `input`'s draft check: the data, without escape sequences, contains a letter (`/\p{L}/u`).
pub fn types_letters(data: &str) -> bool {
    let _ = data;
    todo!("D5")
}

impl Daemon {
    /// TS: InkDaemon.handleAgentHook (`Stale or unauthorized lifecycle callback`): applies the hook's signal to the
    /// session's attention (stale and repeated signals are dropped), adopts a new conversation ID, and notifies when
    /// the session newly needs attention.
    pub async fn handle_agent_hook(&self, message: &AgentHook) -> Result<()> {
        let _ = message;
        todo!("D5")
    }

    /// TS: InkDaemon.markAnswered: input answering a prompt makes a session that needs input work again.
    pub async fn mark_answered(&self, session_id: &str) -> Result<()> {
        let _ = session_id;
        todo!("D5")
    }

    /// TS: InkDaemon.acceptedNativeRef: the conversation ID a hook reports, when the session should adopt it.
    pub fn accepted_native_ref(
        &self,
        session: &SessionRecord,
        native_ref: Option<&AgentSessionRef>,
        event: &str,
    ) -> Option<AgentSessionRef> {
        let _ = (session, native_ref, event);
        todo!("D5")
    }

    /// TS: InkDaemon.notify: a desktop notification `Deckhand: <title>` when enabled (osascript on macOS,
    /// notify-send elsewhere, 2 s timeout); failures are ignored.
    pub async fn notify(&self, session: &SessionRecord, message: &str) {
        let _ = (session, message);
        todo!("D5")
    }

    /// TS: InkDaemon.setWorkerAgentStatus: records the activity status (a working signal on an idle screen becomes
    /// unknown) and types a pending draft once idle.
    pub async fn set_worker_agent_status(&self, session_id: &str, agent_status: AgentActivityStatus) -> Result<()> {
        let _ = (session_id, agent_status);
        todo!("D5")
    }

    /// TS: InkDaemon.typeDraft: types the pending draft as one bracketed paste once the screen shows no prompt.
    pub async fn type_draft(&self, session_id: &str) -> Result<()> {
        let _ = session_id;
        todo!("D5")
    }

    /// TS: InkDaemon.dropDraft: forgets the draft (`save`: also clear the session's `startPrompt`); `reason` is logged
    /// when it was not typed.
    pub fn drop_draft(&self, session_id: &str, reason: Option<&str>, save: bool) {
        let _ = (session_id, reason, save);
        todo!("D5")
    }
}
