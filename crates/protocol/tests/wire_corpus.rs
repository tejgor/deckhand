//! Replays the wire corpus (fixtures/wire/corpus.jsonl, captured from the TypeScript test suite, plus
//! the hand-written fixtures/wire/extra.jsonl) and the persisted-record fixtures through the typed protocol: every
//! message must deserialise into its typed enum and serialise back to exactly the original JSON, key order included.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use deckhand_protocol::events::ResponseData;
use deckhand_protocol::order::Ordered;
use deckhand_protocol::records::{self, AppConfig, RawConfig, StateFile, UiState, UiStateFile};
use deckhand_protocol::worker_ipc::WorkerResponseData;
use deckhand_protocol::{Request, ServerMessage, WorkerCommand, WorkerMessage, WorkspaceWorkerCommand};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/wire")
}

struct Line {
    source: String,
    dir: String,
    worker: Option<String>,
    req: Option<String>,
    msg: Value,
}

fn read_lines(name: &str) -> Vec<Line> {
    let path = fixtures().join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            let value: Value =
                serde_json::from_str(line).unwrap_or_else(|error| panic!("{name}:{}: not JSON: {error}", index + 1));
            let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
            Line {
                source: format!("{name}:{}", index + 1),
                dir: text("dir").unwrap_or_else(|| panic!("{name}:{}: no dir", index + 1)),
                worker: text("worker"),
                req: text("req"),
                msg: value
                    .get("msg")
                    .cloned()
                    .unwrap_or_else(|| panic!("{name}:{}: no msg", index + 1)),
            }
        })
        .collect()
}

/// Deserialises `value` as `T` and checks that `T` serialises back to the same JSON text.
fn round_trip<T: DeserializeOwned + Serialize>(source: &str, what: &str, value: &Value) -> Result<T, String> {
    let original = serde_json::to_string(value).map_err(|error| error.to_string())?;
    let typed: T = serde_json::from_value(value.clone())
        .map_err(|error| format!("{source}: {what}: decode failed: {error}\n  {original}"))?;
    let written = serde_json::to_string(&typed).map_err(|error| format!("{source}: {what}: encode failed: {error}"))?;
    if written != original {
        return Err(format!(
            "{source}: {what}: round trip differs\n  original: {original}\n  typed:    {written}"
        ));
    }
    Ok(typed)
}

fn typed_data<T: Serialize>(source: &str, what: &str, data: &Value, decoded: Result<T, String>) -> Result<(), String> {
    let typed = decoded?;
    let original = serde_json::to_string(data).map_err(|error| error.to_string())?;
    let written = serde_json::to_string(&typed).map_err(|error| error.to_string())?;
    if written != original {
        return Err(format!(
            "{source}: {what} data round trip differs\n  original: {original}\n  typed:    {written}"
        ));
    }
    Ok(())
}

fn type_of(msg: &Value) -> String {
    msg.get("type").and_then(Value::as_str).unwrap_or("?").to_owned()
}

/// Checks one line; returns the coverage key it adds.
fn check(line: &Line, failures: &mut Vec<String>) -> Option<String> {
    let kind = type_of(&line.msg);
    let source = line.source.as_str();
    let result = match (line.dir.as_str(), line.worker.as_deref()) {
        // Requests come from many producers (client.ts, attach.ts, cli.ts, tests): their key order is the producer's,
        // so it is checked through Ordered; the typed enum must still hold every field.
        ("c2d", _) => round_trip::<Ordered<Request>>(source, "request", &line.msg).map(|_| ()),
        ("d2c", _) => round_trip::<ServerMessage>(source, "server message", &line.msg).and_then(|message| {
            match (&message, &line.req) {
                (ServerMessage::Response(response), Some(req)) if response.ok => match &response.data {
                    Some(data) => typed_data(
                        source,
                        req,
                        data,
                        ResponseData::decode(req, data.clone()).map_err(|error| format!("{source}: {req}: {error}")),
                    ),
                    None => Ok(()),
                },
                _ => Ok(()),
            }
        }),
        // The daemon sends `kill` both directly ({type, requestId, force}) and through requestChannel (ID last).
        ("d2w", Some("session")) if kind == "kill" => {
            round_trip::<Ordered<WorkerCommand>>(source, "worker command", &line.msg).map(|_| ())
        }
        ("d2w", Some("session")) => round_trip::<WorkerCommand>(source, "worker command", &line.msg).map(|_| ()),
        ("d2w", Some("workspace")) => {
            round_trip::<WorkspaceWorkerCommand>(source, "workspace command", &line.msg).map(|_| ())
        }
        ("w2d", Some(worker @ ("session" | "workspace"))) => {
            round_trip::<WorkerMessage>(source, "worker message", &line.msg).and_then(|message| {
                match (&message, &line.req) {
                    (WorkerMessage::Response(response), Some(req)) if response.ok => match &response.data {
                        Some(data) => typed_data(
                            source,
                            req,
                            data,
                            WorkerResponseData::decode(worker == "workspace", req, data.clone())
                                .map_err(|error| format!("{source}: {req}: {error}")),
                        ),
                        None => Ok(()),
                    },
                    _ => Ok(()),
                }
            })
        }
        (dir, worker) => Err(format!("{source}: unknown direction {dir} / worker {worker:?}")),
    };
    if let Err(error) = result {
        failures.push(error);
    }
    let worker = line.worker.as_deref().unwrap_or("");
    Some(match (&line.req, kind.as_str()) {
        (Some(req), "response") if line.msg.get("ok") == Some(&Value::Bool(true)) => {
            format!("{} {worker} response {req}", line.dir)
        }
        _ => format!("{} {worker} {kind}", line.dir),
    })
}

fn all_lines() -> Vec<Line> {
    let mut lines = read_lines("corpus.jsonl");
    lines.extend(read_lines("extra.jsonl"));
    lines
}

#[test]
fn every_corpus_message_round_trips_through_its_typed_enum() {
    let lines = all_lines();
    assert!(lines.len() > 100, "the corpus looks truncated ({} lines)", lines.len());
    let mut failures = Vec::new();
    for line in &lines {
        check(line, &mut failures);
    }
    assert!(
        failures.is_empty(),
        "{} of {} lines failed:\n{}",
        failures.len(),
        lines.len(),
        failures.join("\n")
    );
}

#[test]
fn the_corpus_covers_every_variant() {
    let mut failures = Vec::new();
    let covered: BTreeSet<String> = all_lines()
        .iter()
        .filter_map(|line| check(line, &mut failures))
        .collect();
    let mut expected = Vec::new();
    expected.extend(Request::TYPES.iter().map(|kind| format!("c2d  {kind}")));
    expected.extend(ServerMessage::TYPES.iter().map(|kind| format!("d2c  {kind}")));
    expected.extend(WorkerCommand::TYPES.iter().map(|kind| format!("d2w session {kind}")));
    expected.extend(
        WorkspaceWorkerCommand::TYPES
            .iter()
            .map(|kind| format!("d2w workspace {kind}")),
    );
    for kind in [
        "hello",
        "response",
        "running",
        "exit",
        "agent-status",
        "preview-updated",
        "output",
    ] {
        expected.push(format!("w2d session {kind}"));
    }
    for kind in [
        "hello",
        "response",
        "terminal-updated",
        "git-updated",
        "dev-updated",
        "action-updated",
        "output",
    ] {
        expected.push(format!("w2d workspace {kind}"));
    }
    // Every request the daemon answers with data, answered successfully.
    for kind in Request::TYPES {
        if ResponseData::decode(kind, Value::Null).is_err_and(|error| error.to_string().starts_with("no response data"))
        {
            continue;
        }
        expected.push(format!("d2c  response {kind}"));
    }
    for kind in ["start", "kill", "snapshot", "attach"] {
        expected.push(format!("w2d session response {kind}"));
    }
    for kind in [
        "start",
        "shutdown",
        "snapshot",
        "start-dev",
        "start-action",
        "attach",
        "stop-dev",
        "stop-action",
        "restart-terminal",
        "idle",
    ] {
        expected.push(format!("w2d workspace response {kind}"));
    }
    let missing: Vec<&String> = expected.iter().filter(|key| !covered.contains(*key)).collect();
    assert!(missing.is_empty(), "variants without a corpus line: {missing:#?}");
}

#[test]
fn state_json_round_trips_byte_for_byte() {
    let path = fixtures().join("records/state.json");
    let text = fs::read_to_string(&path).expect("records/state.json");
    let state: StateFile = serde_json::from_str(&text).expect("state.json decodes");
    assert!(!state.sessions.is_empty());
    assert_eq!(records::to_ts_file_text(&state).unwrap(), text);
}

#[test]
fn config_json_round_trips_and_normalizes_like_ts() {
    let text = fs::read_to_string(fixtures().join("records/config.json")).expect("records/config.json");
    let raw: RawConfig = serde_json::from_str(&text).expect("config.json decodes");
    assert_eq!(records::to_ts_file_text(&raw).unwrap(), text);
    let normalized =
        fs::read_to_string(fixtures().join("records/app-config.normalized.json")).expect("normalized config");
    assert_eq!(
        records::to_ts_json(&AppConfig::from_raw(&raw)).unwrap(),
        normalized.trim_end()
    );
    let config: AppConfig = serde_json::from_str(&normalized).expect("normalized config decodes");
    assert_eq!(records::to_ts_json(&config).unwrap(), normalized.trim_end());
}

#[test]
fn ui_state_json_round_trips_and_normalizes_like_ts() {
    let text = fs::read_to_string(fixtures().join("records/ui-state.json")).expect("records/ui-state.json");
    let file: UiStateFile = serde_json::from_str(&text).expect("ui-state.json decodes");
    assert_eq!(records::to_ts_file_text(&file).unwrap(), text);
    for (repo, entry) in &file {
        let state: UiState = serde_json::from_value(entry.clone()).unwrap_or_else(|error| panic!("{repo}: {error}"));
        assert_eq!(
            serde_json::to_string(&state).unwrap(),
            serde_json::to_string(entry).unwrap(),
            "{repo}"
        );
    }
    // normalizeUiState of hand-made inputs, as TS computed it.
    let cases = fs::read_to_string(fixtures().join("records/ui-state.normalize.jsonl")).expect("normalize cases");
    for (index, case) in cases.lines().enumerate() {
        let case: Value = serde_json::from_str(case).expect("case");
        let input = case.get("input").cloned().unwrap_or(Value::Null);
        let expected = serde_json::to_string(case.get("output").expect("output")).unwrap();
        assert_eq!(
            serde_json::to_string(&UiState::normalize(&input)).unwrap(),
            expected,
            "case {}",
            index + 1
        );
    }
}
