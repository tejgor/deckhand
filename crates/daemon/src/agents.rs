//! Port of daemon.ts (agent versions, updates).
//!
//! The installed and latest version of every agent (agentVersionCheck.ts), broadcast to subscribed clients when they
//! change; the latest releases are looked up in the background at start and then on a timer.

use std::time::Duration;

use deckhand_core::error::{DhError, Result};
use deckhand_protocol::data::{AgentUpdateResult, AgentVersions, ProgramKey};
use deckhand_protocol::events::{AgentVersionsUpdated, ServerMessage};
use deckhand_protocol::wire::AgentVersionsRequest;
use deckhand_sys::agent_version_check::AgentVersionChecker;
use deckhand_sys::async_util::{Interval, spawn};
use serde_json::Value;

use crate::daemon::Daemon;
use crate::server::send_message;

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

/// TS `String(value)` for a decoded JSON value (`undefined` when absent).
fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_owned(),
        Some(Value::Null) => "null".to_owned(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => match number.as_f64() {
            // JS prints integral doubles without a fraction (`1`, not `1.0`) below 1e21; the bound keeps the cast exact.
            #[allow(clippy::cast_possible_truncation)]
            Some(float) if !number.is_i64() && !number.is_u64() && float.fract() == 0.0 && float.abs() < 1e21 => {
                (float as i128).to_string()
            }
            _ => number.to_string(),
        },
        // Array.prototype.join: null items print as nothing.
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                item => js_string(Some(item)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_owned(),
    }
}

/// TS: updateAgent's guard (``if (!Object.hasOwn(AGENTS, program)) throw new Error(`Unknown agent ${String(program)}`)``)
/// for a raw `update-agent` request: the failure TS answers when `program` names no agent, else `None`.
///
/// [`Daemon::update_agent`] takes a [`ProgramKey`], so a request naming no agent never decodes into a call; the
/// request decoder (server.rs `parse_request`) answers such a request with this error rather than its decode failure
/// (`Invalid update-agent request: ...`), which TS clients do not expect (tests/agentUpdates.test.ts `/Unknown agent/`).
pub fn unknown_agent_error(request: &Value) -> Option<DhError> {
    if request.get("type").and_then(Value::as_str) != Some("update-agent") {
        return None;
    }
    // Object.hasOwn looks up the property key, i.e. String(program).
    let program = js_string(request.get("program"));
    if serde_json::from_value::<ProgramKey>(Value::String(program.clone())).is_ok() {
        return None;
    }
    Some(DhError::msg(format!("Unknown agent {program}")))
}

/// TS: `exitCode` in a template literal (`null` when the updater was killed or never started).
fn exit_code_text(exit_code: Option<i32>) -> String {
    exit_code.map_or_else(|| "null".to_owned(), |code| code.to_string())
}

impl Daemon {
    /// TS: `this.agentVersions`.
    pub fn agent_versions(&self) -> AgentVersionChecker {
        self.agents
            .borrow_mut()
            .checker
            .get_or_insert_with(|| AgentVersionChecker::new(None))
            .clone()
    }

    /// TS: InkDaemon.publishAgentVersions: every agent's versions, broadcast to subscribed clients when changed.
    pub async fn publish_agent_versions(&self) -> AgentVersions {
        let checker = self.agent_versions();
        let sessions = self.all_sessions();
        let versions = checker.snapshot(&sessions).await;
        let json = serde_json::to_string(&versions).unwrap_or_default();
        let changed = {
            let mut agents = self.agents.borrow_mut();
            if agents.published.as_deref() == Some(json.as_str()) {
                false
            } else {
                agents.published = Some(json);
                true
            }
        };
        if changed {
            let clients: Vec<_> = self.server.borrow().clients.values().cloned().collect();
            for client in clients {
                // TS `if (client.repoRoot)`: subscribed clients only.
                if client
                    .subscription
                    .repo_root
                    .as_deref()
                    .is_some_and(|root| !root.is_empty())
                {
                    let message = ServerMessage::AgentVersionsUpdated(Box::new(AgentVersionsUpdated {
                        versions: versions.clone(),
                    }));
                    send_message(&client.handle, &message);
                }
            }
        }
        versions
    }

    /// TS: InkDaemon.refreshAgentVersions: looks up the latest releases (`force`: even when fresh) and publishes;
    /// failures are logged (`agent version check failed: <message>`).
    ///
    /// Neither step fails in the port: agentVersionCheck.ts keeps a failed lookup as that agent's error, and the
    /// snapshot reads every failure into its entries, so the TS catch has nothing to log.
    pub async fn refresh_agent_versions(&self, force: bool) {
        self.agent_versions().refresh_latest(force).await;
        self.publish_agent_versions().await;
    }

    /// TS: InkDaemon.updateAgent: runs the agent's update command and reports the new versions (sessions keep
    /// running their version). TS `Unknown agent <program>` (a request naming no agent) is raised before a
    /// [`ProgramKey`] exists: see [`unknown_agent_error`].
    pub async fn update_agent(&self, program: ProgramKey) -> Result<AgentUpdateResult> {
        let daemon = self.downgrade();
        // Clients see the agent as updating as soon as the update started.
        let on_start: Box<dyn FnOnce()> = Box::new(move || {
            if let Some(daemon) = daemon.upgrade() {
                drop(spawn(async move {
                    daemon.publish_agent_versions().await;
                }));
            }
        });
        let outcome = self.agent_versions().update(program, Some(on_start)).await?;
        self.log(&format!(
            "agent update {}: exit {} ({} -> {})",
            outcome.command,
            exit_code_text(outcome.exit_code),
            outcome.before.as_deref().unwrap_or("?"),
            outcome.after.as_deref().unwrap_or("?"),
        ))
        .await;
        let versions = self.publish_agent_versions().await;
        Ok(AgentUpdateResult {
            program,
            ok: outcome.ok,
            exit_code: outcome.exit_code.map(i64::from),
            command: outcome.command,
            output: outcome.output,
            before: outcome.before,
            after: outcome.after,
            versions,
        })
    }

    /// TS: handleRequest `agent-versions` (`refresh`: look up the latest releases now).
    pub async fn handle_agent_versions(&self, request: &AgentVersionsRequest) -> Result<AgentVersions> {
        if request.refresh == Some(true) {
            self.agent_versions().refresh_latest(true).await;
        }
        Ok(self.publish_agent_versions().await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::tests::{Home, log_containing, received, subscribe};
    use crate::server::tests::{local, session, test_client, test_daemon};
    use crate::server::{Client, ClientId, ClientSubscription};
    use deckhand_protocol::data::SessionStatus;
    use serde_json::Value;
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    /// A fake agent (shell builtins only, so one process per run): prints the version in `<path>.version`; `update`
    /// (alone) writes `2.0.0` there, anything else fails.
    const FAKE_AGENT: &str = r#"#!/bin/sh
if [ "$1" = "--version" ]; then IFS= read -r version < "$0.version"; printf '%s\n' "$version"; exit 0; fi
if [ "$*" = "update" ]; then printf '2.0.0 (fake)\n' > "$0.version"; printf '  Updated\n'; exit 0; fi
exit 3
"#;

    /// Fake claude/pi/codex first on PATH and DECKHAND_AGENT_LATEST set while this lives (the caller holds [`Home`],
    /// which serialises environment changes).
    struct FakeAgents {
        path: Option<OsString>,
        latest: Option<OsString>,
    }

    impl FakeAgents {
        fn install(bin: &Path) -> FakeAgents {
            std::fs::create_dir_all(bin).unwrap();
            for name in ["claude", "pi", "codex"] {
                let file = bin.join(name);
                std::fs::write(&file, FAKE_AGENT).unwrap();
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
                std::fs::write(bin.join(format!("{name}.version")), "1.0.0 (fake)\n").unwrap();
            }
            let path = std::env::var_os("PATH");
            let latest = std::env::var_os("DECKHAND_AGENT_LATEST");
            let mut joined = OsString::from(bin);
            if let Some(path) = &path {
                joined.push(":");
                joined.push(path);
            }
            // SAFETY: the caller holds Home's environment lock.
            unsafe {
                std::env::set_var("PATH", joined);
                std::env::set_var("DECKHAND_AGENT_LATEST", r#"{"claude":"2.0.0","pi":"1.0.0"}"#);
            }
            FakeAgents { path, latest }
        }
    }

    impl Drop for FakeAgents {
        fn drop(&mut self) {
            // SAFETY: as in FakeAgents::install.
            unsafe {
                for (key, value) in [
                    ("PATH", self.path.take()),
                    ("DECKHAND_AGENT_LATEST", self.latest.take()),
                ] {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
    }

    fn versions_sent(lines: &[Value]) -> usize {
        lines
            .iter()
            .filter(|line| line["type"] == "agent-versions-updated")
            .count()
    }

    /// The fake agents' runs, one test after the other (run in parallel, their processes slow down the timing
    /// sensitive worker tests of lifecycle.rs).
    #[test]
    fn agent_versions_with_fake_agents() {
        versions_are_broadcast_to_subscribed_clients_only_when_they_change();
        updating_an_agent_reports_its_new_version_and_logs_the_outcome();
    }

    fn versions_are_broadcast_to_subscribed_clients_only_when_they_change() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let _agents = FakeAgents::install(&dir.path().join("bin"));
        local(async {
            let daemon = test_daemon(dir.path());
            daemon.notes_tasks.borrow_mut().notes = None;
            let mut running = session("s", "/r", "/r");
            running.agent_version = Some("0.9.0".into());
            daemon.set_session(running);
            let mut subscribed = subscribe(&daemon, 1);
            let (handle, mut unsubscribed) = test_client(2);
            daemon.server.borrow_mut().clients.insert(
                ClientId(2),
                Client {
                    handle,
                    subscription: ClientSubscription {
                        repo_root: Some(String::new()),
                        ..ClientSubscription::default()
                    },
                },
            );

            let versions = daemon.publish_agent_versions().await;
            assert_eq!(versions.claude.installed.as_deref(), Some("1.0.0"));
            assert_eq!(versions.claude.latest, None);
            assert_eq!(versions.claude.running, 1);
            assert_eq!(versions.claude.outdated, 1);
            assert_eq!(versions.pi.running, 0);
            let sent = received(&mut subscribed);
            assert_eq!(versions_sent(&sent), 1);
            assert_eq!(
                sent[0],
                serde_json::json!({"type": "agent-versions-updated", "versions": versions})
            );
            assert!(received(&mut unsubscribed).is_empty());
            // Unchanged: nothing is sent.
            daemon.publish_agent_versions().await;
            assert!(received(&mut subscribed).is_empty());

            // Not fresh yet: looked up (a program missing from the override is that agent's error).
            daemon.refresh_agent_versions(false).await;
            let versions = daemon.publish_agent_versions().await;
            assert_eq!(versions.claude.latest.as_deref(), Some("2.0.0"));
            assert_eq!(versions.pi.latest.as_deref(), Some("1.0.0"));
            assert_eq!(
                versions.codex.error.as_deref(),
                Some("latest unknown: not listed in DECKHAND_AGENT_LATEST")
            );
            assert_eq!(versions_sent(&received(&mut subscribed)), 1);

            // A session that exits changes the counts.
            daemon.patch_session("s", |session| session.status = SessionStatus::Exited);
            let request = AgentVersionsRequest {
                request_id: "r".into(),
                refresh: None,
            };
            let versions = daemon.handle_agent_versions(&request).await.unwrap();
            assert_eq!(versions.claude.running, 0);
            assert_eq!(versions_sent(&received(&mut subscribed)), 1);
            let refreshed = daemon
                .handle_agent_versions(&AgentVersionsRequest {
                    request_id: "r".into(),
                    refresh: Some(true),
                })
                .await
                .unwrap();
            assert_eq!(refreshed.claude.latest.as_deref(), Some("2.0.0"));
        });
    }

    fn updating_an_agent_reports_its_new_version_and_logs_the_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let _home = Home::set(dir.path());
        let _agents = FakeAgents::install(&dir.path().join("bin"));
        local(async {
            let daemon = test_daemon(dir.path());
            daemon.notes_tasks.borrow_mut().notes = None;
            let mut subscribed = subscribe(&daemon, 1);
            let result = daemon.update_agent(ProgramKey::Claude).await.unwrap();
            assert!(result.ok);
            assert_eq!(result.exit_code, Some(0));
            assert_eq!(result.command, "claude update");
            assert_eq!(result.output, "Updated");
            assert_eq!(result.before.as_deref(), Some("1.0.0"));
            assert_eq!(result.after.as_deref(), Some("2.0.0"));
            assert_eq!(result.versions.claude.installed.as_deref(), Some("2.0.0"));
            assert_eq!(result.versions.claude.updating, None);
            let json = serde_json::to_value(&result).unwrap();
            let keys: Vec<&String> = json.as_object().unwrap().keys().collect();
            assert_eq!(
                keys,
                [
                    "program", "ok", "exitCode", "command", "output", "before", "after", "versions"
                ]
            );
            let line = "daemon agent update claude update: exit 0 (1.0.0 -> 2.0.0)\n";
            assert!(log_containing(dir.path(), line).await.contains(line));
            // Clients see the new version (and, when its snapshot is taken before the quick fake update ends, the
            // update in progress).
            let sent = received(&mut subscribed);
            assert!(
                sent.iter()
                    .any(|line| line["versions"]["claude"]["installed"] == "2.0.0")
            );

            // pi's update command (`update --self`) fails here.
            let failed = daemon.update_agent(ProgramKey::Pi).await.unwrap();
            assert!(!failed.ok);
            assert_eq!(failed.exit_code, Some(3));
            assert_eq!(failed.command, "pi update --self");
            assert_eq!(failed.after.as_deref(), Some("1.0.0"));
            let line = "daemon agent update pi update --self: exit 3 (1.0.0 -> 1.0.0)\n";
            assert!(log_containing(dir.path(), line).await.contains(line));
        });
    }

    #[test]
    fn exit_codes_read_like_a_template_literal() {
        assert_eq!(exit_code_text(Some(0)), "0");
        assert_eq!(exit_code_text(Some(-1)), "-1");
        assert_eq!(exit_code_text(None), "null");
    }

    #[test]
    fn update_requests_naming_no_agent_fail_like_ts() {
        let request = |program: Option<Value>| {
            let mut request = serde_json::json!({"type": "update-agent", "requestId": "r"});
            if let Some(program) = program {
                request["program"] = program;
            }
            unknown_agent_error(&request).map(|error| error.to_string())
        };
        assert_eq!(request(Some("nope".into())).as_deref(), Some("Unknown agent nope"));
        assert_eq!(request(None).as_deref(), Some("Unknown agent undefined"));
        assert_eq!(request(Some(Value::Null)).as_deref(), Some("Unknown agent null"));
        assert_eq!(request(Some(serde_json::json!(7))).as_deref(), Some("Unknown agent 7"));
        assert_eq!(
            request(Some(serde_json::json!(7.0))).as_deref(),
            Some("Unknown agent 7")
        );
        assert_eq!(
            request(Some(serde_json::json!(1.5))).as_deref(),
            Some("Unknown agent 1.5")
        );
        assert_eq!(
            request(Some(serde_json::json!(false))).as_deref(),
            Some("Unknown agent false")
        );
        assert_eq!(
            request(Some(serde_json::json!({"a": 1}))).as_deref(),
            Some("Unknown agent [object Object]")
        );
        assert_eq!(
            request(Some(serde_json::json!(["x", null, 2]))).as_deref(),
            Some("Unknown agent x,,2")
        );
        // Every agent passes the guard, as do other requests.
        for program in ["claude", "pi", "codex"] {
            assert_eq!(request(Some(program.into())), None);
        }
        assert!(unknown_agent_error(&serde_json::json!({"type": "agent-versions", "program": "nope"})).is_none());
    }
}
