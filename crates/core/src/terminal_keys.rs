//! Port of terminalKeys.ts.
//!
//! Ink labels ASCII DEL (the usual Backspace byte) and Kitty's Backspace codepoint 127 as `delete`. Forward Delete
//! is a distinct CSI 3~ sequence. Alt+Backspace arrives as ESC DEL (or ESC BS): a backspace with meta.

use std::sync::OnceLock;

use regex::Regex;

use crate::key::Key;

/// What a raw sequence means for the Backspace/Delete labels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backward {
    /// ESC DEL / ESC BS: Alt+Backspace.
    WithMeta,
    /// DEL, BS, or Kitty's Backspace (CSI 127 u / CSI 8 u, with optional modifiers and event type).
    Plain,
}

fn backward(sequence: &str) -> Option<Backward> {
    static KITTY: OnceLock<Regex> = OnceLock::new();
    if sequence == "\x1b\x7f" || sequence == "\x1b\x08" {
        return Some(Backward::WithMeta);
    }
    // `\d` in the TS regex (no `u` flag) is ASCII-only.
    let kitty = KITTY
        .get_or_init(|| Regex::new(r"^\x1b\[(?:8|127)(?:;[0-9]+(?::[123])?(?:;[0-9:]+)?)?u$").expect("valid regex"));
    (sequence == "\x7f" || sequence == "\x08" || kitty.is_match(sequence)).then_some(Backward::Plain)
}

/// TS: terminalKeys.ts normalizeTerminalKey (`sequence`: the raw bytes Ink parsed `key` from).
pub fn normalize_terminal_key(key: Key, sequence: &str) -> Key {
    match backward(sequence) {
        Some(Backward::WithMeta) => Key {
            backspace: true,
            delete: false,
            meta: true,
            ..key
        },
        Some(Backward::Plain) => Key {
            backspace: true,
            delete: false,
            ..key
        },
        None => key,
    }
}

#[cfg(test)]
mod tests {
    use deckhand_testkit::{Case, replay};
    use serde_json::{Value, json};

    use super::*;
    use crate::text_editor::{EditOptions, EditorState, edit_text};

    fn key(value: Value) -> Key {
        serde_json::from_value(value).expect("a Key")
    }

    fn st(text: &str, cursor: usize) -> EditorState {
        EditorState {
            text: text.to_owned(),
            cursor,
            ..EditorState::default()
        }
    }

    // covers: B-P11-09 terminal Backspace bytes and Kitty codepoint delete left, not right
    #[test]
    fn backspace_bytes_delete_left() {
        for sequence in ["\x08", "\x7f", "\x1b[127u", "\x1b[127;1u", "\x1b[127;1:2u", "\x1b[8;1u"] {
            let key = normalize_terminal_key(key(json!({"delete": true})), sequence);
            assert!(key.backspace, "{sequence:?}");
            assert!(!key.delete, "{sequence:?}");
            assert_eq!(
                edit_text(&st("abcd", 2), "", &key, &EditOptions::default()),
                st("acd", 1)
            );
        }
    }

    // covers: B-P11-10 real forward Delete retains its direction and modifiers
    #[test]
    fn forward_delete_retained() {
        for sequence in ["\x1b[3~", "\x1b[3;2~", "\x1b[3;1:1~"] {
            let key = normalize_terminal_key(key(json!({"delete": true, "shift": true})), sequence);
            assert!(key.delete, "{sequence:?}");
            assert!(key.shift, "{sequence:?}");
            assert_eq!(
                edit_text(&st("abcd", 2), "", &key, &EditOptions::default()),
                st("abd", 2)
            );
        }
    }

    #[test]
    fn normalize_terminal_key_parity() {
        replay(
            "terminalKeys/normalizeTerminalKey",
            |case: &Case| -> Result<Value, String> {
                let sequence = case.arg_str(1)?;
                let result = serde_json::to_value(normalize_terminal_key(case.arg_as(0)?, sequence))
                    .map_err(|error| error.to_string())?;
                // TS spreads the partial key: the keys it had, plus the ones the branch taken sets.
                let Value::Object(mut out) = case.arg(0).clone() else {
                    return Err(String::from("key is not an object"));
                };
                let set: &[&str] = match backward(sequence) {
                    Some(Backward::WithMeta) => &["backspace", "delete", "meta"],
                    Some(Backward::Plain) => &["backspace", "delete"],
                    None => &[],
                };
                let names: Vec<String> = out
                    .keys()
                    .cloned()
                    .chain(set.iter().map(|name| (*name).to_owned()))
                    .collect();
                for name in names {
                    let value = result
                        .get(&name)
                        .cloned()
                        .ok_or_else(|| format!("unknown key field {name}"))?;
                    out.insert(name, value);
                }
                Ok(Value::Object(out))
            },
        );
    }
}
