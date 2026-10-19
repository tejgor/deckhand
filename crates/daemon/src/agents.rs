//! Port of daemon.ts (agent versions, updates).
//!
//! The installed and latest version of every agent (agentVersionCheck.ts), broadcast to subscribed clients when they
//! change; the latest releases are looked up in the background at start and then on a timer.

use std::time::Duration;

use deckhand_core::error::Result;
use deckhand_protocol::data::{AgentUpdateResult, AgentVersions, ProgramKey};
use deckhand_protocol::wire::AgentVersionsRequest;
use deckhand_sys::agent_version_check::AgentVersionChecker;
use deckhand_sys::async_util::Interval;

use crate::daemon::Daemon;

/// TS: daemon.ts AGENT_VERSIONS_TICK_MS: how often the daemon asks whether the latest releases are due for a lookup
/// (they are every LATEST_MAX_AGE).
pub const AGENT_VERSIONS_TICK: Duration = Duration::from_millis(30 * 60_000);

/// The agents' part of the daemon state.
#[derive(Default)]
pub struct AgentsState {
    /// TS `agentVersions` (created by [`Daemon::new`]).
    pub checker: Option<AgentVersionChecker>,
    /// TS `publishedAgentVersions`: the last `agent-versions-updated` sent (JSON), so only changes are broadcast.
    pub published: Option<String>,
    /// TS `agentVersionsTimer`.
    pub timer: Option<Interval>,
}

impl Daemon {
    /// TS: `this.agentVersions`.
    pub fn agent_versions(&self) -> AgentVersionChecker {
        todo!("D5")
    }

    /// TS: InkDaemon.publishAgentVersions: every agent's versions, broadcast to subscribed clients when changed.
    pub async fn publish_agent_versions(&self) -> AgentVersions {
        todo!("D5")
    }

    /// TS: InkDaemon.refreshAgentVersions: looks up the latest releases (`force`: even when fresh) and publishes;
    /// failures are logged (`agent version check failed: <message>`).
    pub async fn refresh_agent_versions(&self, force: bool) {
        let _ = force;
        todo!("D5")
    }

    /// TS: InkDaemon.updateAgent: runs the agent's update command and reports the new versions (sessions keep
    /// running their version). TS `Unknown agent <program>` cannot happen: [`ProgramKey`] is closed.
    pub async fn update_agent(&self, program: ProgramKey) -> Result<AgentUpdateResult> {
        let _ = program;
        todo!("D5")
    }

    /// TS: handleRequest `agent-versions` (`refresh`: look up the latest releases now).
    pub async fn handle_agent_versions(&self, request: &AgentVersionsRequest) -> Result<AgentVersions> {
        let _ = request;
        todo!("D5")
    }
}
