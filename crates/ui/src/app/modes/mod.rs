//! The app's modes: one module per mode's keys.

pub mod action_picker;
pub mod browse;
pub mod confirm;
pub mod create;
pub mod filter;
pub mod preview_focus;
pub mod review;
pub mod search;
pub mod workspace_info;

use deckhand_core::key::Input;

/// Not in the TS: the keys that move a list's selection while letters are text (a search, a session's name): ↑/↓,
/// Ctrl+P/Ctrl+N and Ctrl+K/Ctrl+J, as (up, down). Ctrl+J arrives as a bare line feed, which Ink reads as text with no
/// Ctrl, so one is taken as Ctrl+J here only; keys sent to the agent keep it.
pub fn typing_moves(input: &Input) -> (bool, bool) {
    let key = &input.key;
    let ctrl = |letters: [&str; 2]| key.ctrl && letters.contains(&input.input.as_str());
    (
        key.up_arrow || ctrl(["p", "k"]),
        key.down_arrow || ctrl(["n", "j"]) || (input.input == "\n" && !key.meta),
    )
}
