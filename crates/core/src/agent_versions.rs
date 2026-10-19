//! Port of agentVersions.ts.
//!
//! Agent versions as pure data: parsing `--version` output, comparing, and what the UI says about them (the Agents
//! screen U, the sidebar's ↑ marker, the header hint). The daemon's I/O lives in `deckhand_sys::agent_version_check`.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use deckhand_protocol::data::{AgentVersionInfo, AgentVersions, ProgramKey, SessionRecord};

/// TS: agentVersions.ts AGENT_PROGRAMS.
pub const AGENT_PROGRAMS: [ProgramKey; 3] = [ProgramKey::Claude, ProgramKey::Pi, ProgramKey::Codex];

/// TS: agentVersions.ts installedVersions result: `Partial<Record<ProgramKey, string>>`, in AGENT_PROGRAMS order.
pub type InstalledVersions = IndexMap<ProgramKey, String>;

/// TS: agentVersions.ts parseVersion (the first x.y.z, as numbers: `v01.2.3` → `1.2.3`).
pub fn parse_version(text: Option<&str>) -> Option<String> {
    let _ = text;
    todo!("P1")
}

/// TS: agentVersions.ts compareVersions (negative: `left` is older; unparsable versions compare equal).
pub fn compare_versions(left: &str, right: &str) -> i64 {
    let _ = (left, right);
    todo!("P1")
}

/// TS: agentVersions.ts AgentUpdateStatus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentUpdateStatus {
    #[serde(rename = "up to date")]
    UpToDate,
    #[serde(rename = "update available")]
    UpdateAvailable,
    #[serde(rename = "not installed")]
    NotInstalled,
    #[serde(rename = "latest unknown")]
    LatestUnknown,
}

impl AgentUpdateStatus {
    /// The TS string.
    pub fn as_str(self) -> &'static str {
        match self {
            AgentUpdateStatus::UpToDate => "up to date",
            AgentUpdateStatus::UpdateAvailable => "update available",
            AgentUpdateStatus::NotInstalled => "not installed",
            AgentUpdateStatus::LatestUnknown => "latest unknown",
        }
    }
}

/// TS: agentVersions.ts agentUpdateStatus.
pub fn agent_update_status(info: Option<&AgentVersionInfo>) -> AgentUpdateStatus {
    let _ = info;
    todo!("P1")
}

/// TS: agentVersions.ts sessionOutdated (a running session launched with an older version than `installed`).
pub fn session_outdated(session: &SessionRecord, installed: Option<&str>) -> bool {
    let _ = (session, installed);
    todo!("P1")
}

/// TS: agentVersions.ts installedVersions.
pub fn installed_versions(versions: Option<&AgentVersions>) -> InstalledVersions {
    let _ = versions;
    todo!("P1")
}

/// TS: agentVersions.ts updateHint (`codex update · U`, `2 agent updates · U`, or empty).
pub fn update_hint(versions: Option<&AgentVersions>) -> String {
    let _ = versions;
    todo!("P1")
}
