//! Port of agents.ts.
//!
//! One table of what each agent can do and how Deckhand launches it. The daemon decides *which* launch a session
//! needs ([`relaunch_plan`]); this table only turns a plan into argv and reads the agent's exit screen.

use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use deckhand_protocol::data::{AgentSessionRef, ProgramKey, RestartMode, SessionRecord};

/// TS: agents.ts LaunchPlan: a new conversation, resuming one, or forking a parent's.
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum LaunchPlan {
    #[serde(rename = "new")]
    New {
        #[serde(rename = "ref")]
        reference: Option<AgentSessionRef>,
        name: String,
    },
    #[serde(rename = "resume")]
    Resume {
        #[serde(rename = "ref")]
        reference: AgentSessionRef,
    },
    #[serde(rename = "fork")]
    Fork {
        parent: AgentSessionRef,
        #[serde(rename = "ref")]
        reference: Option<AgentSessionRef>,
        name: String,
    },
}

/// TS: agents.ts AgentVersionSource.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentVersionSource {
    /// npm package whose `latest` dist-tag is the newest release.
    pub npm_package: &'static str,
    /// Arguments of the agent's own non-interactive update command.
    pub update_args: &'static [&'static str],
}

/// TS: agents.ts AgentSpec. The TS spec's functions (`args`, `exitRef`, ...) are methods dispatching on `program`;
/// the optional ones return `None`/`false` for agents without them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSpec {
    #[serde(skip)]
    pub program: ProgramKey,
    pub label: &'static str,
    /// Deckhand picks the conversation ID at launch; otherwise it is captured later.
    pub id_at_launch: bool,
    /// A parent's conversation can be forked into a sub-session.
    pub forks: bool,
    /// Forking by ID works from another directory, so a forked child may run in a different worktree.
    pub forks_across_directories: bool,
    pub version: AgentVersionSource,
}

impl AgentSpec {
    /// TS: agents.ts AgentSpec.args (the launch plan as argv).
    pub fn args(&self, plan: &LaunchPlan) -> Vec<String> {
        let _ = plan;
        todo!("P1")
    }

    /// TS: agents.ts AgentSpec.exitRef (the conversation the exit screen says to resume; Claude and Codex).
    pub fn exit_ref(&self, output: &str) -> Option<AgentSessionRef> {
        let _ = output;
        todo!("P1")
    }

    /// TS: agents.ts AgentSpec.missingConversation (the ID the agent has no saved conversation for; Claude).
    pub fn missing_conversation(&self, output: &str) -> Option<String> {
        let _ = output;
        todo!("P1")
    }

    /// TS: agents.ts AgentSpec.forkFailed (a fork found no parent conversation; Pi).
    pub fn fork_failed(&self, output: &str) -> bool {
        let _ = output;
        todo!("P1")
    }
}

/// TS: agents.ts AGENTS, in AGENT_PROGRAMS order (claude, pi, codex).
pub static AGENTS: [AgentSpec; 3] = [
    AgentSpec {
        program: ProgramKey::Claude,
        label: "Claude",
        id_at_launch: true,
        forks: true,
        forks_across_directories: true,
        version: AgentVersionSource {
            npm_package: "@anthropic-ai/claude-code",
            update_args: &["update"],
        },
    },
    AgentSpec {
        program: ProgramKey::Pi,
        label: "Pi",
        id_at_launch: true,
        forks: true,
        forks_across_directories: true,
        version: AgentVersionSource {
            npm_package: "@earendil-works/pi-coding-agent",
            update_args: &["update", "--self"],
        },
    },
    AgentSpec {
        program: ProgramKey::Codex,
        label: "Codex",
        id_at_launch: false,
        forks: true,
        forks_across_directories: false,
        version: AgentVersionSource {
            npm_package: "@openai/codex",
            update_args: &["update"],
        },
    },
];

/// TS: agents.ts agentSpec.
pub fn agent_spec(program: ProgramKey) -> &'static AgentSpec {
    let _ = program;
    todo!("P1")
}

/// TS: agents.ts newAgentRef (a fresh UUID ref for agents that take one at launch).
pub fn new_agent_ref(program: ProgramKey) -> Option<AgentSessionRef> {
    let _ = program;
    todo!("P1")
}

/// TS: agents.ts launchArgs.
pub fn launch_args(program: ProgramKey, plan: &LaunchPlan) -> Vec<String> {
    let _ = (program, plan);
    todo!("P1")
}

/// TS: agents.ts sameAgentSessionRef.
pub fn same_agent_session_ref(left: Option<&AgentSessionRef>, right: Option<&AgentSessionRef>) -> bool {
    let _ = (left, right);
    todo!("P1")
}

/// TS: agents.ts forksParentAgain (a forked child without a conversation of its own forks its parent again).
pub fn forks_parent_again(session: &SessionRecord, never_started: bool) -> bool {
    let _ = (session, never_started);
    todo!("P1")
}

/// TS: agents.ts relaunchPlan (`None`: nothing to resume).
pub fn relaunch_plan(
    session: &SessionRecord,
    mode: RestartMode,
    never_started: bool,
    name: &str,
) -> Option<LaunchPlan> {
    let _ = (session, mode, never_started, name);
    todo!("P1")
}
