//! Port of app.tsx's search keys (/): typed text filters the session list (at most 256 characters); Enter keeps
//! it, Esc clears it.

use deckhand_core::key::Input;

use crate::app::state::App;
use crate::keymap::Keymap;

/// TS: app.tsx the search query's limit.
pub const MAX_QUERY: usize = 256;

/// search keys.
pub static KEYMAP: Keymap = &[];

/// TS: the search branch of the useTerminalInput handler.
pub fn handle_search(app: &mut App, input: &Input) {
    let _ = (app, input);
    todo!("A5")
}
