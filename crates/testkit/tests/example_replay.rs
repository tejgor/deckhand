//! Replays tests/fixtures/_example against tiny hand-written stand-ins, exercising the replay end to end (UTF-16
//! offsets, exact error text, mutation, tags, directory cases). Real tests call `deckhand_testkit::replay(...)`.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;

use deckhand_testkit::tags;
use deckhand_testkit::{Case, CaseSettings, DirOp, Outcome, Replay};
use serde_json::{Value, json};

fn fixtures() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn replay(fixture: &str) -> Replay {
    let settings = CaseSettings::load_path(&fixtures().join("cases.toml")).unwrap();
    // Some cases here fail on purpose; DECKHAND_BLESS must not rewrite them.
    Replay::with_settings(settings, fixture)
        .fixtures_root(fixtures())
        .bless(false)
}

// Stand-ins for a text editor's wordLeft/wordRight, on byte offsets (the fixtures' UTF-16 offsets are converted).
fn is_word(character: char) -> bool {
    character.is_alphabetic() || character.is_numeric() || character == '_'
}
fn word_left(text: &str, cursor: usize) -> usize {
    let before = |position: usize| {
        text[..position]
            .chars()
            .next_back()
            .map(|character| (position - character.len_utf8(), character))
    };
    let mut position = cursor;
    while let Some((previous, _)) = before(position).filter(|(_, character)| !is_word(*character)) {
        position = previous;
    }
    while let Some((previous, _)) = before(position).filter(|(_, character)| is_word(*character)) {
        position = previous;
    }
    position
}
fn word_right(text: &str, cursor: usize) -> usize {
    let after = |position: usize| {
        text[position..]
            .chars()
            .next()
            .map(|character| (position + character.len_utf8(), character))
    };
    let mut position = cursor;
    while let Some((next, _)) = after(position).filter(|(_, character)| !is_word(*character)) {
        position = next;
    }
    while let Some((next, _)) = after(position).filter(|(_, character)| is_word(*character)) {
        position = next;
    }
    position
}

#[test]
fn word_left_and_right_with_utf16_offsets() {
    for (fixture, port) in [
        ("_example/wordLeft", word_left as fn(&str, usize) -> usize),
        ("_example/wordRight", word_right),
    ] {
        let report = replay(fixture)
            .run(|case: &Case| -> Result<usize, String> { Ok(port(case.arg_str(0)?, case.arg_usize(1)?)) });
        report.assert_ok();
        assert!(report.cases > 100, "{}", report.summary());
    }
}

#[test]
fn scroll_top_for() {
    let report = replay("_example/scrollTopFor").run(|case: &Case| -> Result<i64, String> {
        let (previous_top, cursor_row, rows, line_count) =
            (case.arg_i64(0)?, case.arg_i64(1)?, case.arg_i64(2)?, case.arg_i64(3)?);
        let mut top = previous_top;
        if cursor_row < top {
            top = cursor_row;
        } else if cursor_row >= top + rows {
            top = cursor_row - rows + 1;
        }
        Ok(top.min((line_count - rows).max(0)).max(0))
    });
    report.assert_ok();
}

/// JS `${number}` for the values the example uses.
fn js_number(value: f64) -> String {
    if value.is_nan() {
        "NaN".into()
    } else if value.is_infinite() {
        if value > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        }
    } else if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

#[test]
fn exact_error_text() {
    let report = replay("_example/checkedWidth").run(|case: &Case| -> Outcome {
        let width = match case.arg_f64(0) {
            Ok(width) => width,
            Err(error) => return Outcome::throws(error),
        };
        // `!(width > 0)` is deliberate: NaN must take the error branch, as in TS.
        #[allow(clippy::neg_cmp_op_on_partial_ord)]
        if !(width > 0.0) {
            return Outcome::throws(format!("width must be positive: {}", js_number(width)));
        }
        Outcome::ret(tags::number(width.floor()))
    });
    report.assert_ok();
}

#[test]
fn argument_mutation() {
    let report = replay("_example/pushUnique").run(|case: &Case| -> Outcome {
        let (Ok(mut list), Ok(item)) = (case.arg_as::<Vec<String>>(0), case.arg_str(1)) else {
            return Outcome::throws("bad arguments");
        };
        let added = !list.iter().any(|existing| existing == item);
        if added {
            list.push(item.to_string());
        }
        Outcome::ret(added).with_args_after(vec![json!(list), json!(item)])
    });
    report.assert_ok();
}

#[test]
fn maps_sets_dates_and_undefined_in_ts_key_order() {
    let report = replay("_example/summarize")
        .key_order(true)
        .run(|case: &Case| -> Result<Value, String> {
            let words = case.arg_as::<Vec<String>>(0)?;
            let mut counts: Vec<(String, i64)> = Vec::new();
            for word in &words {
                match counts.iter_mut().find(|(key, _)| key == word) {
                    Some((_, count)) => *count += 1,
                    None => counts.push((word.clone(), 1)),
                }
            }
            let mut unique: Vec<&String> = Vec::new();
            for word in &words {
                if !unique.contains(&word) {
                    unique.push(word);
                }
            }
            let mut out = serde_json::Map::new();
            out.insert(
                "counts".into(),
                tags::map(counts.into_iter().map(|(word, count)| (json!(word), json!(count)))),
            );
            out.insert("unique".into(), tags::set(unique.into_iter().map(|word| json!(word))));
            if let Some(first) = words.first() {
                out.insert("first".into(), json!(first));
            }
            out.insert("at".into(), tags::date(Some("2026-01-01T00:00:00.000Z")));
            out.insert("missing".into(), json!([tags::undefined()]));
            Ok(Value::Object(out))
        });
    report.assert_ok();
}

#[test]
fn function_arguments_resolve_by_registry_name() {
    let report = replay("_example/applyTwice").run(|case: &Case| -> Result<Value, String> {
        let apply: fn(f64) -> f64 = match case.fn_name(0) {
            Some("double") => |value| value * 2.0,
            Some("negate") => |value| -value,
            other => return Err(format!("unknown function {other:?}")),
        };
        Ok(tags::number(apply(apply(case.arg_f64(1)?))))
    });
    report.assert_ok();
}

/// Stand-in for the example's writeSettings(root, settings).
fn write_settings(_root: &Path, op: &DirOp) -> Result<String, String> {
    assert_eq!(op.fn_name, "writeSettings");
    let root = op.arg_str(0)?;
    let mut settings = match op.arg(1) {
        Value::Object(settings) => settings.clone(),
        other => return Err(format!("bad settings {other}")),
    };
    if settings.get("name").and_then(Value::as_str).is_none_or(str::is_empty) {
        return Err("settings.name is required".into());
    }
    let directory = Path::new(root).join("config");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&directory)
        .map_err(|error| error.to_string())?;
    let file = directory.join("settings.json");
    settings.insert("workspace".into(), json!(format!("{root}/workspace")));
    settings.insert("savedAt".into(), json!("2026-01-01T00:00:00.000Z"));
    let text = serde_json::to_string_pretty(&settings).map_err(|error| error.to_string())? + "\n";
    let mut handle = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&file)
        .map_err(|error| error.to_string())?;
    handle.write_all(text.as_bytes()).map_err(|error| error.to_string())?;
    let _ = std::fs::remove_file(Path::new(root).join("stale.txt"));
    Ok(file.to_string_lossy().into_owned())
}

#[test]
fn directory_cases_compare_outcome_and_tree() {
    for fixture in ["_example/write-settings", "_example/write-settings-invalid"] {
        let report = replay(fixture).run_dir(write_settings);
        report.assert_ok();
    }
    // A wrong port is caught by the tree comparison.
    let report = replay("_example/write-settings").run_dir(|root: &Path, op: &DirOp| {
        let result = write_settings(root, op);
        std::fs::write(root.join("keep.json"), "{}\n").map_err(|error| error.to_string())?;
        result
    });
    assert!(!report.ok);
    assert!(
        report.failures[0]
            .diff
            .contains("tree: keep.json content differs: line 1"),
        "{}",
        report.summary()
    );
}
