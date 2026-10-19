//! Structural comparison of tagged values with a readable diff.
//!
//! Numbers compare by value (`1` == `1.0`), object key order is ignored unless asked for, and an object property
//! holding `{"$u":1}` counts as absent (JS `{a: undefined}` vs `{}` after the recorder's default `omit`).

use serde_json::Value;

use crate::tags::is_undefined;

const MAX_DIFFS: usize = 8;

/// `None` when equal, else up to eight `path: expected .., got ..` lines.
pub fn diff(expected: &Value, actual: &Value) -> Option<String> {
    diff_with(expected, actual, false)
}

pub fn diff_with(expected: &Value, actual: &Value, key_order: bool) -> Option<String> {
    let mut out = Vec::new();
    walk(expected, actual, "$", key_order, &mut out);
    if out.is_empty() { None } else { Some(out.join("\n")) }
}

/// Compact JSON, shortened to 200 chars.
pub fn show(value: &Value) -> String {
    let text = value.to_string();
    if text.chars().count() <= 200 {
        text
    } else {
        format!("{}…", text.chars().take(200).collect::<String>())
    }
}

fn walk(expected: &Value, actual: &Value, path: &str, key_order: bool, out: &mut Vec<String>) {
    if out.len() >= MAX_DIFFS {
        return;
    }
    match (expected, actual) {
        (Value::Number(left), Value::Number(right)) => {
            if left.as_f64() != right.as_f64() {
                out.push(format!("{path}: expected {left}, got {right}"));
            }
        }
        (Value::Array(left), Value::Array(right)) => {
            for (index, (left, right)) in left.iter().zip(right).enumerate() {
                walk(left, right, &format!("{path}[{index}]"), key_order, out);
            }
            if left.len() != right.len() {
                out.push(format!(
                    "{path}: expected {} items, got {} ({})",
                    left.len(),
                    right.len(),
                    show(actual)
                ));
            }
        }
        (Value::Object(left), Value::Object(right)) => {
            let defined = |object: &serde_json::Map<String, Value>| -> Vec<String> {
                object
                    .iter()
                    .filter(|(_, value)| !is_undefined(value))
                    .map(|(key, _)| key.clone())
                    .collect()
            };
            let (left_keys, right_keys) = (defined(left), defined(right));
            for key in &left_keys {
                let child = format!("{path}.{key}");
                match right.get(key).filter(|value| !is_undefined(value)) {
                    Some(value) => walk(&left[key], value, &child, key_order, out),
                    None => out.push(format!("{child}: missing (expected {})", show(&left[key]))),
                }
            }
            for key in right_keys.iter().filter(|key| !left_keys.contains(key)) {
                out.push(format!("{path}.{key}: unexpected (got {})", show(&right[key])));
            }
            let same_set = left_keys.len() == right_keys.len() && left_keys.iter().all(|key| right_keys.contains(key));
            if key_order && same_set && left_keys != right_keys {
                out.push(format!("{path}: key order expected {left_keys:?}, got {right_keys:?}"));
            }
        }
        _ => {
            if expected != actual {
                out.push(format!("{path}: expected {}, got {}", show(expected), show(actual)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn equal_values_have_no_diff() {
        assert_eq!(
            diff(&json!({"a": 1, "b": [1.0, "x"]}), &json!({"b": [1, "x"], "a": 1.0})),
            None
        );
        assert_eq!(diff(&json!({"a": 1, "u": {"$u": 1}}), &json!({"a": 1})), None);
    }

    #[test]
    fn differences_name_their_paths() {
        let text = diff(
            &json!({"text": "ab", "cursor": 2, "rows": [1]}),
            &json!({"text": "ab", "cursor": 3, "rows": [1, 2], "x": true}),
        )
        .unwrap();
        assert!(text.contains("$.cursor: expected 2, got 3"), "{text}");
        assert!(text.contains("$.rows: expected 1 items, got 2"), "{text}");
        assert!(text.contains("$.x: unexpected (got true)"), "{text}");
        assert!(
            diff(&json!({"$u": 1}), &json!(null))
                .unwrap()
                .contains("expected {\"$u\":1}, got null")
        );
    }

    #[test]
    fn key_order_is_checked_only_on_request() {
        let (left, right) = (json!({"a": 1, "b": 2}), json!({"b": 2, "a": 1}));
        assert_eq!(diff(&left, &right), None);
        assert!(diff_with(&left, &right, true).unwrap().contains("key order"));
    }
}
