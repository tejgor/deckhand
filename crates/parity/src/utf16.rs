//! UTF-16 (TS string index) <-> UTF-8 byte offset conversion, and the manifest `utf16_fields` rewrite.
//!
//! `utf16_fields = { "<fixture>" = { "<offset path>" = "<string path>" } }`: paths are dot-separated from the case
//! object (`args`, `argsAfter`, `ret`), segments are object keys, array indices or `*` (every element). Each
//! wildcard in the offset path binds a value that fills the string path's wildcards in order, so
//! `"ret.*.start" = "args.0"` and `"ret.lines.*.col" = "ret.lines.*.text"` both work.
//!
//! Offsets past the end extend linearly (`len16 + k` -> `len8 + k`), negative offsets pass through, and an offset
//! that splits a surrogate pair (or a byte offset off a char boundary) is an error, reported as a case failure.

use indexmap::IndexMap;
use serde_json::Value;

pub fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// Byte offset of UTF-16 offset `offset` in `text`.
pub fn utf16_to_byte(text: &str, offset: i64) -> Result<i64, String> {
    if offset < 0 {
        return Ok(offset);
    }
    let mut units = 0i64;
    for (byte, character) in text.char_indices() {
        if units == offset {
            return Ok(byte as i64);
        }
        units += character.len_utf16() as i64;
        if units > offset {
            return Err(format!(
                "UTF-16 offset {offset} splits the surrogate pair of {character:?} in {text:?}"
            ));
        }
    }
    Ok(text.len() as i64 + (offset - units))
}

/// UTF-16 offset of byte offset `offset` in `text`.
pub fn byte_to_utf16(text: &str, offset: i64) -> Result<i64, String> {
    if offset < 0 {
        return Ok(offset);
    }
    let Ok(at) = usize::try_from(offset) else {
        return Ok(offset);
    };
    if at > text.len() {
        return Ok(utf16_len(text) as i64 + (offset - text.len() as i64));
    }
    if !text.is_char_boundary(at) {
        return Err(format!("byte offset {offset} is not on a char boundary in {text:?}"));
    }
    Ok(utf16_len(&text[..at]) as i64)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Utf16ToByte,
    ByteToUtf16,
}

fn segments(path: &str) -> Vec<&str> {
    path.split('.').filter(|segment| !segment.is_empty()).collect()
}

/// Concrete paths matching `pattern` in `root`, each with the values its wildcards took.
fn expand(root: &Value, pattern: &[&str]) -> Vec<(Vec<String>, Vec<String>)> {
    let mut out = Vec::new();
    fn walk(
        value: &Value,
        rest: &[&str],
        path: &mut Vec<String>,
        bound: &mut Vec<String>,
        out: &mut Vec<(Vec<String>, Vec<String>)>,
    ) {
        let Some((head, tail)) = rest.split_first() else {
            out.push((path.clone(), bound.clone()));
            return;
        };
        let children: Vec<(String, &Value)> = match (*head, value) {
            ("*", Value::Array(items)) => items
                .iter()
                .enumerate()
                .map(|(index, item)| (index.to_string(), item))
                .collect(),
            ("*", Value::Object(object)) => object.iter().map(|(key, item)| (key.clone(), item)).collect(),
            (key, other) => lookup(other, key)
                .map(|item| vec![(key.to_string(), item)])
                .unwrap_or_default(),
        };
        for (key, child) in children {
            path.push(key.clone());
            if *head == "*" {
                bound.push(key);
            }
            walk(child, tail, path, bound, out);
            path.pop();
            if *head == "*" {
                bound.pop();
            }
        }
    }
    walk(root, pattern, &mut Vec::new(), &mut Vec::new(), &mut out);
    out
}

fn lookup<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Array(items) => key.parse::<usize>().ok().and_then(|index| items.get(index)),
        Value::Object(object) => object.get(key),
        _ => None,
    }
}

fn lookup_path<'a>(root: &'a Value, path: &[String]) -> Option<&'a Value> {
    path.iter().try_fold(root, |value, key| lookup(value, key))
}

fn lookup_path_mut<'a>(root: &'a mut Value, path: &[String]) -> Option<&'a mut Value> {
    path.iter().try_fold(root, |value, key| match value {
        Value::Array(items) => key.parse::<usize>().ok().and_then(move |index| items.get_mut(index)),
        Value::Object(object) => object.get_mut(key),
        _ => None,
    })
}

/// Rewrites the offset fields of one case object (`{"args":..,"ret":..}`) in place. Offset paths that do not exist
/// in this case (or hold a non-number) are skipped; a missing string is an error.
pub fn convert_case(case: &mut Value, fields: &IndexMap<String, String>, direction: Direction) -> Result<(), String> {
    for (offset_pattern, string_pattern) in fields {
        let matches = expand(case, &segments(offset_pattern));
        for (offset_path, bound) in matches {
            let Some(offset) = lookup_path(case, &offset_path).and_then(Value::as_i64) else {
                continue;
            };
            let mut wildcards = bound.iter();
            let mut string_path = Vec::new();
            for segment in segments(string_pattern) {
                if segment == "*" {
                    let value = wildcards.next().ok_or_else(|| {
                        format!("utf16_fields: {string_pattern:?} has more wildcards than {offset_pattern:?}")
                    })?;
                    string_path.push(value.clone());
                } else {
                    string_path.push(segment.to_string());
                }
            }
            let text = match lookup_path(case, &string_path) {
                Some(Value::String(text)) => text.clone(),
                Some(other) => {
                    return Err(format!(
                        "utf16_fields: {} is not a plain string ({other})",
                        string_path.join(".")
                    ));
                }
                None => {
                    return Err(format!(
                        "utf16_fields: string {} (for {}) is missing",
                        string_path.join("."),
                        offset_path.join(".")
                    ));
                }
            };
            let converted = match direction {
                Direction::Utf16ToByte => utf16_to_byte(&text, offset),
                Direction::ByteToUtf16 => byte_to_utf16(&text, offset),
            }
            .map_err(|error| format!("{}: {error}", offset_path.join(".")))?;
            if let Some(slot) = lookup_path_mut(case, &offset_path) {
                *slot = Value::from(converted);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn utf16_and_byte_offsets_convert_both_ways() {
        let text = "a😀é\nb";
        // UTF-16: a=0, 😀=1..3, é=3, \n=4, b=5, end=6; bytes: a=0, 😀=1..5, é=5..7, \n=7, b=8, end=9
        for (units, bytes) in [(0, 0), (1, 1), (3, 5), (4, 7), (5, 8), (6, 9), (8, 11), (-1, -1)] {
            assert_eq!(utf16_to_byte(text, units).unwrap(), bytes, "utf16 {units}");
            assert_eq!(byte_to_utf16(text, bytes).unwrap(), units, "byte {bytes}");
        }
        assert!(utf16_to_byte(text, 2).unwrap_err().contains("surrogate"));
        assert!(byte_to_utf16(text, 2).unwrap_err().contains("char boundary"));
        assert_eq!(utf16_len(text), 6);
    }

    #[test]
    fn convert_case_follows_paths_and_wildcards() {
        let mut case = json!({"args": [{"text": "😀ab", "cursor": 3}, 2],
            "ret": {"rows": [{"start": 0, "end": 2}, {"start": 2, "end": 4}], "lines": [{"text": "é", "col": 1}]}});
        let fields: IndexMap<String, String> = [
            ("args.0.cursor", "args.0.text"),
            ("args.1", "args.0.text"),
            ("ret.rows.*.start", "args.0.text"),
            ("ret.rows.*.end", "args.0.text"),
            ("ret.lines.*.col", "ret.lines.*.text"),
            ("ret.missing", "args.0.text"),
        ]
        .into_iter()
        .map(|(offset, text)| (offset.to_string(), text.to_string()))
        .collect();
        convert_case(&mut case, &fields, Direction::Utf16ToByte).unwrap();
        assert_eq!(
            case,
            json!({"args": [{"text": "😀ab", "cursor": 5}, 4],
            "ret": {"rows": [{"start": 0, "end": 4}, {"start": 4, "end": 6}], "lines": [{"text": "é", "col": 2}]}})
        );
        convert_case(&mut case, &fields, Direction::ByteToUtf16).unwrap();
        assert_eq!(case["args"][0]["cursor"], json!(3));
        assert_eq!(case["ret"]["lines"][0]["col"], json!(1));
    }

    #[test]
    fn convert_case_reports_split_pairs_and_missing_strings() {
        let fields: IndexMap<String, String> = IndexMap::from([("args.1".to_string(), "args.0".to_string())]);
        let error = convert_case(&mut json!({"args": ["😀", 1]}), &fields, Direction::Utf16ToByte).unwrap_err();
        assert!(error.starts_with("args.1: "), "{error}");
        let error = convert_case(&mut json!({"args": [null, 1]}), &fields, Direction::Utf16ToByte).unwrap_err();
        assert!(error.contains("not a plain string"), "{error}");
    }
}
