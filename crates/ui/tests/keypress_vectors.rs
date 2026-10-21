//! Replays tests/data/keypress-vectors.json (recorded from Ink 6): raw stdin bytes in
//! chunks, through deckhand_ui::input, must give Ink's events, each with the `input` and `key` useInput reports.

use deckhand_core::key::Key;
use deckhand_ui::input::StdinParser;
use serde::Deserialize;

#[derive(Deserialize)]
struct Case {
    name: String,
    chunks: Vec<Option<Vec<u8>>>,
    events: Vec<Event>,
}

#[derive(Deserialize, Debug, PartialEq)]
struct Event {
    sequence: String,
    input: String,
    key: Key,
}

#[test]
fn ink_keypress_vectors_replay() {
    let raw = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/keypress-vectors.json"))
        .expect("keypress vectors");
    let cases: Vec<Case> = serde_json::from_str(&raw).expect("vectors parse");
    assert!(cases.len() > 200, "expected the full vector set");
    let mut failures = Vec::new();
    for case in &cases {
        let mut parser = StdinParser::new();
        let mut events = Vec::new();
        for chunk in &case.chunks {
            match chunk {
                Some(bytes) => events.extend(parser.push(bytes).events),
                None => events.extend(parser.flush_pending_escape()),
            }
        }
        events.extend(parser.flush_pending_escape());
        let got: Vec<Event> = events
            .into_iter()
            .map(|event| Event {
                sequence: event.sequence,
                input: event.input.input,
                key: event.input.key,
            })
            .collect();
        if got != case.events {
            failures.push(format!("{}:\n  want {:?}\n  got  {:?}", case.name, case.events, got));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}
