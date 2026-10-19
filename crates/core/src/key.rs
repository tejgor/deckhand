//! Port of Ink Key (keypress) model shared by ui input parser and text_editor.
//!
//! Ink 6's `useInput(handler)` calls `handler(input, key)` once per keypress (or once per paste with the whole text):
//! `key` is the [`Key`] below, built in `node_modules/ink/build/hooks/use-input.js` from `parse-keypress`. The ui
//! crate's input parser produces [`Input`]s; the editor (`text_editor`), the terminal-key normaliser and the app's
//! key handlers consume them. TS code that takes `Partial<Key>` takes a `Key` built with `..Key::default()`.

use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// Ink `Key.eventType`: only reported under the kitty keyboard protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KeyEventType {
    #[serde(rename = "press")]
    Press,
    #[serde(rename = "repeat")]
    Repeat,
    #[serde(rename = "release")]
    Release,
}

/// Ink `Key`: which special key was pressed, and the modifiers. Field order and JSON names are Ink's.
#[skip_serializing_none]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Key {
    pub up_arrow: bool,
    pub down_arrow: bool,
    pub left_arrow: bool,
    pub right_arrow: bool,
    pub page_down: bool,
    pub page_up: bool,
    pub home: bool,
    pub end: bool,
    /// `key.return` (Enter).
    #[serde(rename = "return")]
    pub return_: bool,
    pub escape: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub tab: bool,
    pub backspace: bool,
    pub delete: bool,
    /// Meta/Alt; Ink also sets it for Escape itself and for Option-modified keys.
    pub meta: bool,
    /// `key.super` (Cmd/Win; kitty keyboard protocol only).
    #[serde(rename = "super")]
    pub super_: bool,
    pub hyper: bool,
    pub caps_lock: bool,
    pub num_lock: bool,
    pub event_type: Option<KeyEventType>,
}

impl Key {
    /// No key and no modifier (what `{}` as a `Partial<Key>` means).
    pub const NONE: Key = Key {
        up_arrow: false,
        down_arrow: false,
        left_arrow: false,
        right_arrow: false,
        page_down: false,
        page_up: false,
        home: false,
        end: false,
        return_: false,
        escape: false,
        ctrl: false,
        shift: false,
        tab: false,
        backspace: false,
        delete: false,
        meta: false,
        super_: false,
        hyper: false,
        caps_lock: false,
        num_lock: false,
        event_type: None,
    };
}

/// One `useInput` call: the text typed or pasted (`input`; empty for special keys) and the key.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Input {
    pub input: String,
    pub key: Key,
}

impl Input {
    /// Typed text without any special key.
    pub fn text(input: impl Into<String>) -> Self {
        Input {
            input: input.into(),
            key: Key::NONE,
        }
    }

    /// A special key (or a modified letter, with `input` its name, as Ink reports Ctrl+A as `a` with `ctrl`).
    pub fn key(input: impl Into<String>, key: Key) -> Self {
        Input {
            input: input.into(),
            key,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialises_with_inks_names_in_inks_order() {
        let key = Key {
            up_arrow: true,
            return_: true,
            super_: true,
            caps_lock: true,
            ..Key::default()
        };
        assert_eq!(
            serde_json::to_string(&key).unwrap(),
            "{\"upArrow\":true,\"downArrow\":false,\"leftArrow\":false,\"rightArrow\":false,\"pageDown\":false,\
             \"pageUp\":false,\"home\":false,\"end\":false,\"return\":true,\"escape\":false,\"ctrl\":false,\
             \"shift\":false,\"tab\":false,\"backspace\":false,\"delete\":false,\"meta\":false,\"super\":true,\
             \"hyper\":false,\"capsLock\":true,\"numLock\":false}"
        );
        let release = Key {
            event_type: Some(KeyEventType::Release),
            ..Key::NONE
        };
        assert!(
            serde_json::to_string(&release)
                .unwrap()
                .ends_with("\"eventType\":\"release\"}")
        );
    }

    #[test]
    fn partial_keys_deserialise_with_defaults() {
        let key: Key = serde_json::from_str("{\"ctrl\":true,\"return\":true}").unwrap();
        assert_eq!(
            key,
            Key {
                ctrl: true,
                return_: true,
                ..Key::NONE
            }
        );
        assert_eq!(serde_json::from_str::<Key>("{}").unwrap(), Key::NONE);
        assert_eq!(Key::default(), Key::NONE);
    }

    #[test]
    fn inputs_carry_any_text() {
        for text in ["a", "😀", "👍🏽", "日本語", "e\u{301}", "\u{1b}[200~paste\u{1b}[201~", ""] {
            let input = Input::text(text);
            let json = serde_json::to_string(&input).unwrap();
            assert_eq!(serde_json::from_str::<Input>(&json).unwrap(), input);
        }
        let ctrl_a = Input::key(
            "a",
            Key {
                ctrl: true,
                ..Key::NONE
            },
        );
        assert_eq!(ctrl_a.input, "a");
        assert!(ctrl_a.key.ctrl && !ctrl_a.key.meta);
        let parsed: Input = serde_json::from_str("{\"input\":\"😀\",\"key\":{\"shift\":true}}").unwrap();
        assert_eq!(
            parsed,
            Input::key(
                "😀",
                Key {
                    shift: true,
                    ..Key::NONE
                }
            )
        );
    }
}
