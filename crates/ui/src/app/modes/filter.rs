//! Port of app.tsx's filter menu (f): a filter's key picks it, f goes back to active, j/k show each filter as they
//! move, Enter keeps the shown one and Esc restores the filter and selection the menu was opened on.

use deckhand_core::key::Input;

use crate::app::state::{App, FilterMenuBack};
use crate::keymap::Keymap;

/// pick-filter keys.
pub static KEYMAP: Keymap = &[];

/// TS: f in browse (`fr` / `ff` can arrive as one chunk when typed fast: the second key picks in the menu).
pub fn open_filter_menu(app: &mut App, input: &Input) {
    let _ = (app, input);
    todo!("A5")
}

/// TS `pickFilter`: one key in the menu (`back`: what Esc restores).
pub fn pick_filter(app: &mut App, input: &Input, back: Option<&FilterMenuBack>) {
    let _ = (app, input, back);
    todo!("A5")
}
