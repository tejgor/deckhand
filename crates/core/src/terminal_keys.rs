//! Port of terminalKeys.ts.
//!
//! Ink labels ASCII DEL (the usual Backspace byte) and Kitty's Backspace codepoint 127 as `delete`. Forward Delete
//! is a distinct CSI 3~ sequence. Alt+Backspace arrives as ESC DEL (or ESC BS): a backspace with meta.

use crate::key::Key;

/// TS: terminalKeys.ts normalizeTerminalKey (`sequence`: the raw bytes Ink parsed `key` from).
pub fn normalize_terminal_key(key: Key, sequence: &str) -> Key {
    let _ = (key, sequence);
    todo!("P11")
}
