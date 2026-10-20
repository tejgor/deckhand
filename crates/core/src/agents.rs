//! Port of agents.ts.
//!
//! One table of what each agent can do and how Deckhand launches it. The daemon decides *which* launch a session
//! needs ([`relaunch_plan`]); this table only turns a plan into argv and reads the agent's exit screen.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::agent_signals::codex_resume_from_output;
use crate::jsstr;
use deckhand_protocol::data::{
    AgentSessionRef, AgentSessionRefKind, ProgramKey, RestartMode, SessionRecord, SubSessionKind,
};

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

fn owned(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

impl AgentSpec {
    /// TS: agents.ts AgentSpec.args (the launch plan as argv).
    pub fn args(&self, plan: &LaunchPlan) -> Vec<String> {
        match self.program {
            // claude --session-id <uuid> --name <label> · --resume <uuid|legacy name> · --resume <parent>
            // --fork-session --session-id <child>. Never --session-id with a plain --resume: Claude rejects it unless
            // --fork-session is given, and refuses an ID in use. --resume also accepts a name, so legacy parents with a
            // name ref fork the same way.
            ProgramKey::Claude => match plan {
                LaunchPlan::Resume { reference } => owned(&["--resume", &reference.value]),
                LaunchPlan::Fork {
                    parent,
                    reference,
                    name,
                } => {
                    let mut args = owned(&["--resume", &parent.value, "--fork-session"]);
                    if let Some(reference) = reference {
                        args.extend(owned(&["--session-id", &reference.value]));
                    }
                    args.extend(owned(&["--name", name]));
                    args
                }
                LaunchPlan::New { reference, name } => match reference {
                    Some(reference) if reference.kind == AgentSessionRefKind::Id => {
                        owned(&["--session-id", &reference.value, "--name", name])
                    }
                    Some(reference) if reference.kind == AgentSessionRefKind::Name => {
                        owned(&["--name", &reference.value])
                    }
                    _ => Vec::new(),
                },
            },
            // pi --session-id <uuid> --name <label> · --session-id <uuid> (opens it, or creates it if absent) ·
            // --fork <parent> --session-id <child>. Legacy path refs use --session <path>. --fork copies the parent
            // before the TUI starts, from any directory.
            ProgramKey::Pi => {
                let reference = match plan {
                    LaunchPlan::Fork {
                        parent,
                        reference,
                        name,
                    } => {
                        let mut args = owned(&["--fork", &parent.value]);
                        if let Some(reference) = reference {
                            args.extend(owned(&["--session-id", &reference.value]));
                        }
                        args.extend(owned(&["--name", name]));
                        return args;
                    }
                    LaunchPlan::New { reference, .. } => reference.as_ref(),
                    LaunchPlan::Resume { reference } => Some(reference),
                };
                match (reference, plan) {
                    (Some(reference), _) if reference.kind == AgentSessionRefKind::Path => {
                        owned(&["--session", &reference.value])
                    }
                    (Some(reference), LaunchPlan::New { name, .. }) if reference.kind == AgentSessionRefKind::Id => {
                        owned(&["--session-id", &reference.value, "--name", name])
                    }
                    (Some(reference), _) if reference.kind == AgentSessionRefKind::Id => {
                        owned(&["--session-id", &reference.value])
                    }
                    _ => Vec::new(),
                }
            }
            // codex · codex resume <id> · codex fork <parent id>. Codex picks every ID itself (also a fork's): Deckhand
            // learns it from an authenticated SessionStart hook or the `codex resume <id>` exit hint.
            ProgramKey::Codex => match plan {
                LaunchPlan::Fork { parent, .. } => owned(&["fork", &parent.value]),
                LaunchPlan::Resume { reference } if reference.kind == AgentSessionRefKind::Id => {
                    owned(&["resume", &reference.value])
                }
                _ => Vec::new(),
            },
        }
    }

    /// Whether the TS spec has `exitRef` (Claude and Codex).
    pub fn has_exit_ref(&self) -> bool {
        matches!(self.program, ProgramKey::Claude | ProgramKey::Codex)
    }

    /// Whether the TS spec has `missingConversation` (Claude).
    pub fn has_missing_conversation(&self) -> bool {
        self.program == ProgramKey::Claude
    }

    /// Whether the TS spec has `forkFailed` (Pi).
    pub fn has_fork_failed(&self) -> bool {
        self.program == ProgramKey::Pi
    }

    /// TS: agents.ts AgentSpec.exitRef (the conversation the exit screen says to resume; Claude and Codex).
    pub fn exit_ref(&self, output: &str) -> Option<AgentSessionRef> {
        match self.program {
            ProgramKey::Claude => {
                let plain = plain(output);
                // Security: a ref becomes `--resume <value>` argv on restart, so a value that looks like a flag
                // (screen text from an untrusted repo) is skipped; the first safe hint wins.
                let value = CLAUDE_RESUME.captures_iter(&plain).find_map(|captures| {
                    captures
                        .get(1)
                        .or_else(|| captures.get(2))
                        .or_else(|| captures.get(3))
                        .map(|found| jsstr::js_trim(found.as_str()))
                        .filter(|value| !value.is_empty() && !value.starts_with('-'))
                })?;
                let kind = if UUID_PATTERN.is_match(value) {
                    AgentSessionRefKind::Id
                } else {
                    AgentSessionRefKind::Name
                };
                Some(AgentSessionRef {
                    provider: ProgramKey::Claude,
                    kind,
                    value: value.to_owned(),
                    extra: Default::default(),
                })
            }
            ProgramKey::Codex => codex_resume_from_output(output),
            ProgramKey::Pi => None,
        }
    }

    /// TS: agents.ts AgentSpec.missingConversation (the ID the agent has no saved conversation for; Claude).
    pub fn missing_conversation(&self, output: &str) -> Option<String> {
        if self.program != ProgramKey::Claude {
            return None;
        }
        let plain = plain(output);
        Some(MISSING_CONVERSATION.captures(&plain)?.get(1)?.as_str().to_owned())
    }

    /// TS: agents.ts AgentSpec.forkFailed (a fork found no parent conversation; Pi).
    pub fn fork_failed(&self, output: &str) -> bool {
        self.program == ProgramKey::Pi && output.contains("No session found matching")
    }
}

/// JS `\s` (WhiteSpace and LineTerminator) as a class body: Rust's Unicode `\s` differs on U+0085 and U+FEFF.
pub(crate) const JS_SPACE: &str =
    r"\t\n\x0B\x0C\r \u{A0}\u{1680}\u{2000}-\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}";

static ESCAPE_SEQUENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]").expect("ESCAPE_SEQUENCE is a valid regex"));
// The TS patterns are `/i`: only their ASCII literals fold (JS non-unicode case folding never maps a non-ASCII
// letter such as U+017F onto ASCII, as Rust's Unicode folding would).
static CLAUDE_RESUME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"(?:^|\n)[{JS_SPACE}]*(?i-u:claude)[{JS_SPACE}]+(?i-u:--resume)(?:=|[{JS_SPACE}]+)(?:"([^"]+)"|'([^']+)'|([^{JS_SPACE}]+))"#
    ))
    .expect("CLAUDE_RESUME is a valid regex")
});
static MISSING_CONVERSATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i-u:No conversation found with session ID:)[{JS_SPACE}]*([0-9a-fA-F-]{{36}})"
    ))
    .expect("MISSING_CONVERSATION is a valid regex")
});
static UUID_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")
        .expect("UUID_PATTERN is a valid regex")
});

/// The agent's output without CSI escape sequences.
fn plain(output: &str) -> Cow<'_, str> {
    ESCAPE_SEQUENCE.replace_all(output, "")
}

/// TS: agents.ts AGENTS, in AGENT_PROGRAMS order (claude, pi, codex).
pub static AGENTS: [AgentSpec; 3] = [
    // `claude --version` prints `2.1.287 (Claude Code)`. The native builds are published to npm under the same
    // numbers; `latest` (not `stable`) is the channel `claude update` follows by default.
    AgentSpec {
        program: ProgramKey::Claude,
        label: "Claude",
        id_at_launch: true,
        forks: true,
        // Verified: the child is stored under the child's cwd project.
        forks_across_directories: true,
        version: AgentVersionSource {
            npm_package: "@anthropic-ai/claude-code",
            update_args: &["update"],
        },
    },
    // `pi --version` prints `1.0.2`; `pi update --self` updates pi only (not its packages or model catalogs).
    AgentSpec {
        program: ProgramKey::Pi,
        label: "Pi",
        id_at_launch: true,
        forks: true,
        // --fork <id> falls back to a search of every project's sessions and writes the child into the cwd's project.
        forks_across_directories: true,
        version: AgentVersionSource {
            npm_package: "@earendil-works/pi-coding-agent",
            update_args: &["update", "--self"],
        },
    },
    // `codex --version` prints `codex-cli 0.157.0`.
    AgentSpec {
        program: ProgramKey::Codex,
        label: "Codex",
        id_at_launch: false,
        forks: true,
        // Codex finds a session by ID anywhere, but resuming/forking one recorded in another directory asks whether to
        // use the session's directory or the current one (tui cwd prompt), so the child could run outside its worktree.
        forks_across_directories: false,
        version: AgentVersionSource {
            npm_package: "@openai/codex",
            update_args: &["update"],
        },
    },
];

/// TS: agents.ts agentSpec.
pub fn agent_spec(program: ProgramKey) -> &'static AgentSpec {
    match program {
        ProgramKey::Claude => &AGENTS[0],
        ProgramKey::Pi => &AGENTS[1],
        ProgramKey::Codex => &AGENTS[2],
    }
}

/// TS: agents.ts newAgentRef (a fresh UUID ref for agents that take one at launch, none for agents that report
/// their own).
pub fn new_agent_ref(program: ProgramKey) -> Option<AgentSessionRef> {
    agent_spec(program).id_at_launch.then(|| AgentSessionRef {
        provider: program,
        kind: AgentSessionRefKind::Id,
        value: uuid::Uuid::new_v4().to_string(),
        extra: Default::default(),
    })
}

/// TS: agents.ts launchArgs.
pub fn launch_args(program: ProgramKey, plan: &LaunchPlan) -> Vec<String> {
    agent_spec(program).args(plan)
}

/// TS: agents.ts sameAgentSessionRef.
pub fn same_agent_session_ref(left: Option<&AgentSessionRef>, right: Option<&AgentSessionRef>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => {
            left.provider == right.provider && left.kind == right.kind && left.value == right.value
        }
        _ => false,
    }
}

/// TS: agents.ts forksParentAgain (a forked child without a conversation of its own forks its parent again).
///
/// It never launched, its fork failed (the exit handling then stores the parent's ref), it never reported its own ID
/// (Codex), or it is a legacy child that still holds the parent's ref.
pub fn forks_parent_again(session: &SessionRecord, never_started: bool) -> bool {
    let parent = match session.sub_session_kind {
        Some(SubSessionKind::Forked) => session.forked_from_agent_session_ref.as_ref(),
        _ => None,
    };
    parent.is_some()
        && (never_started
            || session.agent_session_ref.is_none()
            || same_agent_session_ref(session.agent_session_ref.as_ref(), parent))
}

/// TS: agents.ts relaunchPlan (`None`: nothing to resume; the caller refuses rather than starting fresh).
///
/// `Fresh` (S) starts a new conversation; otherwise a fork without its own conversation forks its parent again, a
/// session that never started launches its assigned conversation, and anything else resumes its own reference.
pub fn relaunch_plan(
    session: &SessionRecord,
    mode: RestartMode,
    never_started: bool,
    name: &str,
) -> Option<LaunchPlan> {
    let name = name.to_owned();
    if mode == RestartMode::Fresh {
        return Some(LaunchPlan::New {
            reference: new_agent_ref(session.program),
            name,
        });
    }
    if forks_parent_again(session, never_started)
        && let Some(parent) = session.forked_from_agent_session_ref.clone()
    {
        return Some(LaunchPlan::Fork {
            parent,
            reference: new_agent_ref(session.program),
            name,
        });
    }
    let reference = session.agent_session_ref.clone();
    if never_started {
        return Some(LaunchPlan::New {
            reference: reference.or_else(|| new_agent_ref(session.program)),
            name,
        });
    }
    reference.map(|reference| LaunchPlan::Resume { reference })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use deckhand_parity::{Case, Outcome, replay, tags};
    use serde_json::{Value, json};

    const UNIT: &str = "P1";

    /// A SessionRecord from the partial session a fixture or test passes (the TS functions take a `Pick<..>`): the
    /// fields they never read get placeholders.
    pub(crate) fn session_from(partial: Value) -> SessionRecord {
        let mut record = json!({
            "id": "s", "title": "s", "program": "claude", "command": "claude", "cwd": "/repo", "repoRoot": "/repo",
            "status": "running", "createdAt": "2026-01-01T00:00:00.000Z", "updatedAt": "2026-01-01T00:00:00.000Z",
        });
        if let (Some(base), Value::Object(fields)) = (record.as_object_mut(), partial) {
            for (key, value) in fields {
                if !value.is_null() {
                    base.insert(key, value);
                }
            }
        }
        serde_json::from_value(record).expect("a valid session")
    }

    fn id(provider: ProgramKey, value: &str) -> AgentSessionRef {
        AgentSessionRef {
            provider,
            kind: AgentSessionRefKind::Id,
            value: value.to_owned(),
            extra: Default::default(),
        }
    }

    fn optional<T: Serialize>(value: Option<T>) -> Outcome {
        match value {
            Some(value) => Outcome::ret(value),
            None => Outcome::ret(tags::undefined()),
        }
    }

    /// The fixtures record a freshly generated UUID (one the input does not hold) as `<new-uuid>`.
    fn scrub(plan: Option<LaunchPlan>, known: &[Option<&AgentSessionRef>]) -> Option<LaunchPlan> {
        let mut plan = plan?;
        if let LaunchPlan::New { reference, .. } | LaunchPlan::Fork { reference, .. } = &mut plan
            && let Some(reference) = reference
            && UUID_PATTERN.is_match(&reference.value)
            && !known.iter().flatten().any(|known| known.value == reference.value)
        {
            reference.value = "<new-uuid>".to_owned();
        }
        Some(plan)
    }

    // covers: B-P1-01 each agent builds its create, resume, fork and fresh argv from one table
    #[test]
    fn builds_argv_from_one_table() {
        let new = |reference: Option<AgentSessionRef>, name: &str| LaunchPlan::New {
            reference,
            name: name.to_owned(),
        };
        let resume = |reference: AgentSessionRef| LaunchPlan::Resume { reference };
        let fork = |parent: AgentSessionRef, reference: Option<AgentSessionRef>, name: &str| LaunchPlan::Fork {
            parent,
            reference,
            name: name.to_owned(),
        };
        let other = |provider, kind, value: &str| AgentSessionRef {
            provider,
            kind,
            value: value.to_owned(),
            extra: Default::default(),
        };
        use ProgramKey::{Claude, Codex, Pi};
        assert_eq!(
            launch_args(Claude, &new(Some(id(Claude, "c1")), "dh-a")),
            ["--session-id", "c1", "--name", "dh-a"]
        );
        assert_eq!(launch_args(Claude, &resume(id(Claude, "c1"))), ["--resume", "c1"]);
        assert_eq!(
            launch_args(Claude, &resume(other(Claude, AgentSessionRefKind::Name, "dh-old"))),
            ["--resume", "dh-old"]
        );
        assert_eq!(
            launch_args(Claude, &fork(id(Claude, "p"), Some(id(Claude, "c2")), "dh-b")),
            [
                "--resume",
                "p",
                "--fork-session",
                "--session-id",
                "c2",
                "--name",
                "dh-b"
            ]
        );
        assert_eq!(
            launch_args(Pi, &new(Some(id(Pi, "p1")), "dh-a")),
            ["--session-id", "p1", "--name", "dh-a"]
        );
        assert_eq!(launch_args(Pi, &resume(id(Pi, "p1"))), ["--session-id", "p1"]);
        assert_eq!(
            launch_args(Pi, &resume(other(Pi, AgentSessionRefKind::Path, "/s.jsonl"))),
            ["--session", "/s.jsonl"]
        );
        assert_eq!(
            launch_args(Pi, &fork(id(Pi, "p"), Some(id(Pi, "c")), "dh-b")),
            ["--fork", "p", "--session-id", "c", "--name", "dh-b"]
        );
        assert!(launch_args(Codex, &new(None, "dh-a")).is_empty());
        assert_eq!(launch_args(Codex, &resume(id(Codex, "x1"))), ["resume", "x1"]);
        assert_eq!(launch_args(Codex, &fork(id(Codex, "x0"), None, "dh-b")), ["fork", "x0"]);
        assert!(!agent_spec(Codex).forks_across_directories);
        assert!(agent_spec(Claude).forks_across_directories && agent_spec(Pi).forks_across_directories);
    }

    // covers: B-P1-02 relaunch resumes a fork's own conversation and forks the parent again only when it has none
    #[test]
    fn relaunch_forks_parent_only_without_own_conversation() {
        let parent = id(ProgramKey::Claude, "parent");
        let fork = |own: &AgentSessionRef| {
            session_from(json!({
                "program": "claude", "subSessionKind": "forked", "forkedFromAgentSessionRef": parent,
                "agentSessionRef": own,
            }))
        };
        let child = id(ProgramKey::Claude, "child");
        assert_eq!(
            relaunch_plan(&fork(&child), RestartMode::Resume, false, "dh-x"),
            Some(LaunchPlan::Resume {
                reference: child.clone()
            })
        );
        match relaunch_plan(&fork(&parent), RestartMode::Resume, false, "dh-x") {
            Some(LaunchPlan::Fork {
                reference: Some(reference),
                ..
            }) => assert_ne!(reference.value, "parent"),
            other => panic!("expected a fork with a new ref, got {other:?}"),
        }
        // Never launched.
        assert!(matches!(
            relaunch_plan(&fork(&child), RestartMode::Resume, true, "dh-x"),
            Some(LaunchPlan::Fork { .. })
        ));
        assert!(matches!(
            relaunch_plan(&fork(&child), RestartMode::Fresh, false, "dh-x"),
            Some(LaunchPlan::New { .. })
        ));
        let codex_fork = session_from(json!({
            "program": "codex", "subSessionKind": "forked", "forkedFromAgentSessionRef": id(ProgramKey::Codex, "x0"),
        }));
        assert_eq!(
            relaunch_plan(&codex_fork, RestartMode::Resume, false, "dh-x"),
            Some(LaunchPlan::Fork {
                parent: id(ProgramKey::Codex, "x0"),
                reference: None,
                name: "dh-x".to_owned()
            })
        );
        // Unknown ID: refused, never fresh.
        let codex = session_from(json!({"program": "codex"}));
        assert_eq!(relaunch_plan(&codex, RestartMode::Resume, false, "dh-x"), None);
    }

    #[test]
    fn agent_spec_parity() {
        replay(UNIT, "agents/agentSpec", |case: &Case| -> Result<Value, String> {
            let spec = agent_spec(case.arg_as(0)?);
            let mut value = serde_json::to_value(spec).map_err(|error| error.to_string())?;
            if let Some(object) = value.as_object_mut() {
                object.insert("hasExitRef".into(), spec.has_exit_ref().into());
                object.insert("hasMissingConversation".into(), spec.has_missing_conversation().into());
                object.insert("hasForkFailed".into(), spec.has_fork_failed().into());
            }
            Ok(value)
        });
    }

    #[test]
    fn new_agent_ref_parity() {
        replay(UNIT, "agents/newAgentRef", |case: &Case| {
            let Ok(program) = case.arg_as::<ProgramKey>(0) else {
                return Outcome::throws("parity: bad arguments");
            };
            optional(new_agent_ref(program).map(|mut reference| {
                if UUID_PATTERN.is_match(&reference.value) {
                    reference.value = "<new-uuid>".to_owned();
                }
                reference
            }))
        });
    }

    #[test]
    fn launch_args_parity() {
        replay(
            UNIT,
            "agents/launchArgs",
            |case: &Case| -> Result<Vec<String>, String> { Ok(launch_args(case.arg_as(0)?, &case.arg_as(1)?)) },
        );
    }

    #[test]
    fn same_agent_session_ref_parity() {
        replay(
            UNIT,
            "agents/sameAgentSessionRef",
            |case: &Case| -> Result<bool, String> {
                let left: Option<AgentSessionRef> = case.arg_as(0)?;
                let right: Option<AgentSessionRef> = case.arg_as(1)?;
                Ok(same_agent_session_ref(left.as_ref(), right.as_ref()))
            },
        );
    }

    #[test]
    fn forks_parent_again_parity() {
        replay(UNIT, "agents/forksParentAgain", |case: &Case| -> Result<bool, String> {
            Ok(forks_parent_again(
                &session_from(tags::untag(case.arg(0))),
                case.arg_bool(1)?,
            ))
        });
    }

    #[test]
    fn relaunch_plan_parity() {
        replay(UNIT, "agents/relaunchPlan", |case: &Case| {
            let session = session_from(tags::untag(case.arg(0)));
            let (Ok(mode), Ok(never_started), Ok(name)) = (case.arg_as(1), case.arg_bool(2), case.arg_str(3)) else {
                return Outcome::throws("parity: bad arguments");
            };
            let plan = relaunch_plan(&session, mode, never_started, name);
            optional(scrub(
                plan,
                &[
                    session.agent_session_ref.as_ref(),
                    session.forked_from_agent_session_ref.as_ref(),
                ],
            ))
        });
    }

    #[test]
    fn exit_ref_rejects_flag_like_values() {
        let spec = agent_spec(ProgramKey::Claude);
        let uuid = "0b6c3c1e-1111-4222-8333-944445555666";
        let hostile = format!("Read x\n  claude --resume --settings=/tmp/evil.json\nclaude --resume {uuid}\n");
        let found = spec.exit_ref(&hostile).expect("the real hint");
        assert_eq!(found.value, uuid);
        assert!(
            spec.exit_ref("claude --resume --dangerously-skip-permissions\n")
                .is_none()
        );
        assert!(spec.exit_ref("claude --resume \" -x\"\n").is_none());
    }

    #[test]
    fn exit_ref_parity() {
        replay(UNIT, "agents/exitRef", |case: &Case| {
            let (Ok(program), Ok(output)) = (case.arg_as::<ProgramKey>(0), case.arg_str(1)) else {
                return Outcome::throws("parity: bad arguments");
            };
            let spec = agent_spec(program);
            if !spec.has_exit_ref() {
                return Outcome::ret(tags::undefined());
            }
            optional(spec.exit_ref(output))
        });
    }

    #[test]
    fn missing_conversation_parity() {
        replay(UNIT, "agents/missingConversation", |case: &Case| {
            let (Ok(program), Ok(output)) = (case.arg_as::<ProgramKey>(0), case.arg_str(1)) else {
                return Outcome::throws("parity: bad arguments");
            };
            optional(agent_spec(program).missing_conversation(output))
        });
    }

    #[test]
    fn fork_failed_parity() {
        replay(UNIT, "agents/forkFailed", |case: &Case| {
            let (Ok(program), Ok(output)) = (case.arg_as::<ProgramKey>(0), case.arg_str(1)) else {
                return Outcome::throws("parity: bad arguments");
            };
            let spec = agent_spec(program);
            if !spec.has_fork_failed() {
                return Outcome::ret(tags::undefined());
            }
            Outcome::ret(spec.fork_failed(output))
        });
    }
}
