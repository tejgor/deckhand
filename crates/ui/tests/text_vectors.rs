//! Replays tests/data/text-vectors.json (recorded from Ink's string-width, cli-truncate and wrap-ansi) against
//! deckhand_ui::primitives.

use deckhand_ui::primitives::{Wrap, string_width, truncate, wrap_text};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Vectors {
    string_width: Vec<WidthCase>,
    truncate: Vec<TruncateCase>,
    wrap: Vec<WrapCase>,
}

#[derive(Deserialize)]
struct WidthCase {
    text: String,
    width: usize,
}

#[derive(Deserialize)]
struct TruncateCase {
    text: String,
    columns: usize,
    position: String,
    result: String,
}

#[derive(Deserialize)]
struct WrapCase {
    text: String,
    columns: usize,
    result: String,
}

fn vectors() -> Vectors {
    let raw = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/text-vectors.json"))
        .expect("text vectors");
    serde_json::from_str(&raw).expect("vectors parse")
}

#[test]
fn string_width_matches_ink() {
    let failures: Vec<String> = vectors()
        .string_width
        .iter()
        .filter(|case| string_width(&case.text) != case.width)
        .map(|case| format!("{:?}: want {} got {}", case.text, case.width, string_width(&case.text)))
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn truncation_matches_cli_truncate() {
    let failures: Vec<String> = vectors()
        .truncate
        .iter()
        .filter_map(|case| {
            let position = match case.position.as_str() {
                "start" => Wrap::TruncateStart,
                "middle" => Wrap::TruncateMiddle,
                _ => Wrap::TruncateEnd,
            };
            let got = truncate(&case.text, case.columns, position);
            (got != case.result).then(|| {
                format!(
                    "{:?} {} {}: want {:?} got {:?}",
                    case.text, case.columns, case.position, case.result, got
                )
            })
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn wrapping_matches_wrap_ansi() {
    let failures: Vec<String> = vectors()
        .wrap
        .iter()
        // wrap-ansi NFC-normalises first; primitives::wrap_text does not (see its docs).
        .filter(|case| !case.text.chars().any(|c| ('\u{300}'..='\u{36f}').contains(&c)))
        .filter_map(|case| {
            let got = wrap_text(&case.text, case.columns).join("\n");
            (got != case.result)
                .then(|| format!("{:?} {}: want {:?} got {:?}", case.text, case.columns, case.result, got))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
