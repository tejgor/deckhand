//! Port of agentVersions.ts.
//!
//! Agent versions as pure data: parsing `--version` output, comparing, and what the UI says about them (the Agents
//! screen U, the sidebar's ↑ marker, the header hint). The daemon's I/O lives in `deckhand_sys::agent_version_check`.

use std::sync::LazyLock;

use indexmap::IndexMap;
use regex::Regex;
use serde::{Deserialize, Serialize};

use deckhand_protocol::data::{AgentVersionInfo, AgentVersions, ProgramKey, SessionRecord, SessionStatus};

/// TS: agentVersions.ts AGENT_PROGRAMS.
pub const AGENT_PROGRAMS: [ProgramKey; 3] = [ProgramKey::Claude, ProgramKey::Pi, ProgramKey::Codex];

/// TS: agentVersions.ts installedVersions result: `Partial<Record<ProgramKey, string>>`, in AGENT_PROGRAMS order.
pub type InstalledVersions = IndexMap<ProgramKey, String>;

static VERSION_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[^0-9.])v?([0-9]+)\.([0-9]+)\.([0-9]+)").expect("VERSION_PATTERN is a valid regex")
});

/// `String(Number(digits))`: leading zeros dropped, and digits past f64 precision written the way JS writes them.
fn number_text(digits: &str) -> String {
    let value: f64 = digits.parse().unwrap_or(f64::NAN);
    if !value.is_finite() {
        return "Infinity".to_owned();
    }
    if value < 1e21 {
        return format!("{value:.0}");
    }
    // `{:e}` is the shortest round-trip mantissa; JS writes a positive exponent with its sign.
    format!("{value:e}").replacen('e', "e+", 1)
}

/// TS: agentVersions.ts parseVersion (the first x.y.z, as numbers: `v01.2.3` → `1.2.3`).
///
/// `2.1.287 (Claude Code)`, `codex-cli 0.157.0`, `1.0.2`; suffixes such as `-alpha.1` are ignored.
pub fn parse_version(text: Option<&str>) -> Option<String> {
    let captures = VERSION_PATTERN.captures(text?)?;
    let part = |index: usize| {
        captures
            .get(index)
            .map_or_else(String::new, |found| number_text(found.as_str()))
    };
    Some(format!("{}.{}.{}", part(1), part(2), part(3)))
}

/// TS: agentVersions.ts compareVersions (negative: `left` is older; unparsable versions compare equal).
pub fn compare_versions(left: &str, right: &str) -> i64 {
    let (Some(a), Some(b)) = (parse_version(Some(left)), parse_version(Some(right))) else {
        return 0;
    };
    let numbers = |text: &str| -> Vec<f64> { text.split('.').map(|part| part.parse().unwrap_or(f64::NAN)).collect() };
    let (x, y) = (numbers(&a), numbers(&b));
    for (left, right) in x.iter().zip(&y) {
        if left != right {
            // Version parts are small integers in practice; a difference past i64 saturates.
            #[allow(clippy::cast_possible_truncation)]
            return (left - right) as i64;
        }
    }
    0
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

/// A TS truthy string (`undefined` and `''` are not).
fn present(value: Option<&String>) -> Option<&str> {
    value.map(String::as_str).filter(|text| !text.is_empty())
}

/// TS: agentVersions.ts agentUpdateStatus.
pub fn agent_update_status(info: Option<&AgentVersionInfo>) -> AgentUpdateStatus {
    let Some(installed) = info.and_then(|info| present(info.installed.as_ref())) else {
        return AgentUpdateStatus::NotInstalled;
    };
    let Some(latest) = info.and_then(|info| present(info.latest.as_ref())) else {
        return AgentUpdateStatus::LatestUnknown;
    };
    if compare_versions(installed, latest) < 0 {
        AgentUpdateStatus::UpdateAvailable
    } else {
        AgentUpdateStatus::UpToDate
    }
}

/// TS: agentVersions.ts sessionOutdated (a running session launched with an older version than `installed`: a
/// restart would pick it up).
pub fn session_outdated(session: &SessionRecord, installed: Option<&str>) -> bool {
    if session.status != SessionStatus::Running {
        return false;
    }
    match (
        present(session.agent_version.as_ref()),
        installed.filter(|text| !text.is_empty()),
    ) {
        (Some(agent_version), Some(installed)) => compare_versions(agent_version, installed) < 0,
        _ => false,
    }
}

fn version_info(versions: &AgentVersions, program: ProgramKey) -> &AgentVersionInfo {
    match program {
        ProgramKey::Claude => &versions.claude,
        ProgramKey::Pi => &versions.pi,
        ProgramKey::Codex => &versions.codex,
    }
}

/// TS: agentVersions.ts installedVersions (installed versions by agent, for the sidebar).
pub fn installed_versions(versions: Option<&AgentVersions>) -> InstalledVersions {
    let Some(versions) = versions else {
        return InstalledVersions::new();
    };
    AGENT_PROGRAMS
        .iter()
        .filter_map(|&program| {
            present(version_info(versions, program).installed.as_ref()).map(|installed| (program, installed.to_owned()))
        })
        .collect()
}

/// TS: agentVersions.ts updateHint: the app header's quiet hint (`codex update · U`, `2 agent updates · U`, or
/// nothing: all current, or latest unknown).
pub fn update_hint(versions: Option<&AgentVersions>) -> String {
    let available: Vec<ProgramKey> = AGENT_PROGRAMS
        .iter()
        .copied()
        .filter(|&program| {
            agent_update_status(versions.map(|versions| version_info(versions, program)))
                == AgentUpdateStatus::UpdateAvailable
        })
        .collect();
    match available.as_slice() {
        [] => String::new(),
        [program] => format!("{program} update · U"),
        _ => format!("{} agent updates · U", available.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{AGENTS, tests::session_from};
    use deckhand_parity::{Case, Outcome, replay, tags};
    use serde_json::{Value, json};

    const UNIT: &str = "P1";

    fn info_from(program: ProgramKey, fields: Value) -> AgentVersionInfo {
        let mut record = json!({"program": program, "running": 0, "outdated": 0});
        if let (Some(base), Value::Object(fields)) = (record.as_object_mut(), fields) {
            for (key, value) in fields {
                // `program` is not read; mutated fixtures hold invalid ones.
                if key != "program" && !value.is_null() {
                    base.insert(key, value);
                }
            }
        }
        serde_json::from_value(record).expect("a valid AgentVersionInfo")
    }

    /// AgentVersions from a fixture's partial record: a missing agent reads as one with nothing installed, as
    /// `versions?.[program]?.installed` does in TS.
    fn versions_from(value: &Value) -> Option<AgentVersions> {
        let value = tags::untag(value);
        let fields = value.as_object()?;
        let info =
            |program: ProgramKey| info_from(program, fields.get(program.as_str()).cloned().unwrap_or(Value::Null));
        Some(AgentVersions {
            claude: info(ProgramKey::Claude),
            pi: info(ProgramKey::Pi),
            codex: info(ProgramKey::Codex),
        })
    }

    fn versions(fields: Value) -> AgentVersions {
        versions_from(&fields).expect("versions")
    }

    fn optional<T: Serialize>(value: Option<T>) -> Outcome {
        match value {
            Some(value) => Outcome::ret(value),
            None => Outcome::ret(tags::undefined()),
        }
    }

    // covers: B-P1-03 agent versions: each agent's --version output parses to x.y.z; comparison is numeric and ignores suffixes
    #[test]
    fn parses_and_compares_versions() {
        assert_eq!(
            parse_version(Some("2.1.287 (Claude Code)\n")).as_deref(),
            Some("2.1.287")
        );
        assert_eq!(parse_version(Some("codex-cli 0.157.0")).as_deref(), Some("0.157.0"));
        assert_eq!(parse_version(Some("1.0.2")).as_deref(), Some("1.0.2"));
        assert_eq!(
            parse_version(Some("codex-cli 0.162.0-alpha.16")).as_deref(),
            Some("0.162.0")
        );
        assert_eq!(parse_version(Some("v20.11.1")).as_deref(), Some("20.11.1"));
        for text in [None, Some(""), Some("Claude Code"), Some("version 2.1")] {
            assert_eq!(parse_version(text), None, "{text:?}");
        }
        assert!(compare_versions("2.1.287", "2.1.290") < 0);
        assert!(compare_versions("0.160.1", "0.157.0") > 0);
        // Numbers, not strings.
        assert!(compare_versions("1.10.0", "1.9.9") > 0);
        assert_eq!(compare_versions("0.162.0-alpha.16", "0.162.0"), 0);
        assert_eq!(compare_versions("garbage", "1.0.0"), 0);
        // The per-agent table: npm package of the latest release and the agent's own update command.
        let table: Vec<(ProgramKey, Vec<&str>)> = AGENTS
            .iter()
            .map(|spec| {
                let mut row = vec![spec.version.npm_package];
                row.extend(spec.version.update_args);
                (spec.program, row)
            })
            .collect();
        assert_eq!(
            table,
            [
                (ProgramKey::Claude, vec!["@anthropic-ai/claude-code", "update"]),
                (
                    ProgramKey::Pi,
                    vec!["@earendil-works/pi-coding-agent", "update", "--self"]
                ),
                (ProgramKey::Codex, vec!["@openai/codex", "update"]),
            ]
        );
    }

    // covers: B-P1-04 agent versions: status per agent, outdated sessions (running only), the header hint
    #[test]
    fn status_outdated_sessions_and_hint() {
        let info = |fields: Value| info_from(ProgramKey::Claude, fields);
        use AgentUpdateStatus::*;
        assert_eq!(
            agent_update_status(Some(&info(json!({"installed": "2.1.287", "latest": "2.1.291"})))),
            UpdateAvailable
        );
        assert_eq!(
            agent_update_status(Some(&info(json!({"installed": "2.1.291", "latest": "2.1.291"})))),
            UpToDate
        );
        assert_eq!(
            agent_update_status(Some(&info(json!({"installed": "2.1.292", "latest": "2.1.291"})))),
            UpToDate
        );
        assert_eq!(
            agent_update_status(Some(&info(json!({"installed": "2.1.287"})))),
            LatestUnknown
        );
        assert_eq!(
            agent_update_status(Some(&info(json!({"latest": "2.1.291"})))),
            NotInstalled
        );
        assert_eq!(agent_update_status(None), NotInstalled);

        let outdated = |status: &str, version: Option<&str>, installed: Option<&str>| {
            session_outdated(
                &session_from(json!({"status": status, "agentVersion": version})),
                installed,
            )
        };
        assert!(outdated("running", Some("2.1.287"), Some("2.1.290")));
        assert!(!outdated("running", Some("2.1.290"), Some("2.1.290")));
        // Downgraded: a restart would not be newer.
        assert!(!outdated("running", Some("2.1.291"), Some("2.1.290")));
        // Its next s uses the installed one anyway.
        assert!(!outdated("exited", Some("2.1.287"), Some("2.1.290")));
        assert!(!outdated("starting", Some("2.1.287"), Some("2.1.290")));
        assert!(!outdated("running", None, Some("2.1.290")));
        assert!(!outdated("running", Some("2.1.287"), None));

        let hint = |fields: Value| update_hint(Some(&versions(fields)));
        assert_eq!(
            hint(
                json!({"codex": {"installed": "0.157.0", "latest": "0.160.1"}, "pi": {"installed": "1.0.4", "latest": "1.0.4"}})
            ),
            "codex update · U"
        );
        assert_eq!(
            hint(
                json!({"codex": {"installed": "0.157.0", "latest": "0.160.1"}, "pi": {"installed": "1.0.2", "latest": "1.0.4"}})
            ),
            "2 agent updates · U"
        );
        // Unknown or current.
        assert_eq!(
            hint(json!({"claude": {"installed": "2.1.287"}, "codex": {"installed": "0.160.1", "latest": "0.160.1"}})),
            ""
        );
        // Not installed is not an update.
        assert_eq!(hint(json!({"pi": {"latest": "1.0.4"}})), "");
        assert_eq!(update_hint(None), "");
        let installed = installed_versions(Some(&versions(
            json!({"claude": {"installed": "2.1.290"}, "pi": {"latest": "1.0.4"}}),
        )));
        assert_eq!(
            installed,
            InstalledVersions::from([(ProgramKey::Claude, "2.1.290".to_owned())])
        );
    }

    #[test]
    fn parse_version_parity() {
        replay(UNIT, "agentVersions/parseVersion", |case: &Case| {
            let text = case.arg(0).as_str();
            optional(parse_version(text))
        });
    }

    #[test]
    fn compare_versions_parity() {
        replay(
            UNIT,
            "agentVersions/compareVersions",
            |case: &Case| -> Result<i64, String> { Ok(compare_versions(case.arg_str(0)?, case.arg_str(1)?)) },
        );
    }

    #[test]
    fn agent_update_status_parity() {
        replay(UNIT, "agentVersions/agentUpdateStatus", |case: &Case| {
            let info = Some(tags::untag(case.arg(0)))
                .filter(|value| !value.is_null())
                .map(|value| info_from(ProgramKey::Claude, value));
            Outcome::ret(agent_update_status(info.as_ref()))
        });
    }

    #[test]
    fn session_outdated_parity() {
        replay(UNIT, "agentVersions/sessionOutdated", |case: &Case| {
            let mut partial = tags::untag(case.arg(0));
            // Only `running` matters: a status Rust cannot hold (undefined, `stopped`) reads as another non-running one.
            if let Some(fields) = partial.as_object_mut()
                && fields.get("status").and_then(Value::as_str) != Some("running")
            {
                fields.insert("status".into(), json!("exited"));
            }
            let installed = case.arg(1).as_str();
            Outcome::ret(session_outdated(&session_from(partial), installed))
        });
    }

    #[test]
    fn installed_versions_parity() {
        replay(UNIT, "agentVersions/installedVersions", |case: &Case| {
            Outcome::ret(installed_versions(versions_from(case.arg(0)).as_ref()))
        });
    }

    #[test]
    fn update_hint_parity() {
        replay(UNIT, "agentVersions/updateHint", |case: &Case| {
            Outcome::ret(update_hint(versions_from(case.arg(0)).as_ref()))
        });
    }
}
