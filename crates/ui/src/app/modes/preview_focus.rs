//! Port of app.tsx's preview-focus keys and mouse wheel (→ on a running session's Preview: scroll it, or send the
//! wheel to Claude, which scrolls its own view), and the changes-/notes-focus hand-off to their flows.

use deckhand_core::key::Input;

use crate::app::WheelDirection;
use crate::app::state::App;
use crate::keymap::Keymap;

/// Preview focus keys.
pub static KEYMAP: Keymap = &[];

/// TS: the preview-focus branch of the useTerminalInput handler.
pub fn handle_preview_focus(app: &mut App, input: &Input) {
    let _ = (app, input);
    todo!("A2")
}

/// TS: the preview-focus `stdin.on('data')` listener: wheel reports scaled by the sensitivity (accumulated).
pub fn on_mouse_data(app: &mut App, text: &str) {
    let _ = (app, text);
    todo!("A2")
}

/// TS `adjustScrollSensitivity` (+ / −): saved to the config.
pub fn adjust_scroll_sensitivity(app: &mut App, delta: f64) {
    let _ = (app, delta);
    todo!("A2")
}

/// TS `sendClaudeWheel`: Claude scrolls its own view; true when sent.
pub fn send_claude_wheel(app: &mut App, direction: WheelDirection, count: usize) -> bool {
    let _ = (app, direction, count);
    todo!("A2")
}

/// TS `scrollPreview`: the wheel to Claude, else the preview's scroll offset.
pub fn scroll_preview(app: &mut App, direction: WheelDirection, count: usize) {
    let _ = (app, direction, count);
    todo!("A2")
}

/// TS: the changes-focus branch: the Changes flow's keys.
pub fn handle_changes_focus(app: &mut App, input: &Input) {
    let _ = (app, input);
    todo!("A2")
}

/// TS: the notes-focus branch: the Notes flow's keys.
pub fn handle_notes_focus(app: &mut App, input: &Input) {
    let _ = (app, input);
    todo!("A2")
}
