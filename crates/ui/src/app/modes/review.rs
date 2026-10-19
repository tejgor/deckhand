//! Port of app.tsx's review-project keys: the inline repository-config review. Enter trusts these exact bytes and
//! resumes the gated action; s resumes it with the override ignored (or cancels); Esc closes (back where it came
//! from). A config that changed since the review shows the new bytes instead.

use deckhand_core::key::Input;

use crate::app::state::App;
use crate::keymap::Keymap;

/// review-project keys.
pub static KEYMAP: Keymap = &[];

/// TS: the review-project branch of the useTerminalInput handler.
pub fn handle_review_project(app: &mut App, input: &Input) {
    let _ = (app, input);
    todo!("A5")
}

/// TS: the review's `close`: forget it, back to browse, then its `back`.
pub fn close_review(app: &mut App) {
    let _ = app;
    todo!("A5")
}
