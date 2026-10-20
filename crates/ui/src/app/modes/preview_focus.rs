//! Port of app.tsx's preview-focus keys and mouse wheel (→ on a running session's Preview: scroll it, or send the
//! wheel to a fullscreen agent, which scrolls its own view), and the changes-/notes-focus hand-off to their flows.

use std::time::Instant;

use deckhand_core::key::Input;
use deckhand_core::wheel::WheelScaler;
use deckhand_protocol::data::{JsNumber, SessionStatus};
use deckhand_protocol::records::AppConfig;
use deckhand_sys::storage::patch_app_config;

use crate::app::state::{App, Mode};
use crate::app::{
    SCROLL_SENSITIVITY_STEP, WheelDirection, format_scroll_sensitivity, mouse_wheel_sequence,
    normalize_scroll_sensitivity, parse_mouse_wheel,
};
use crate::flow::Flow;
use crate::keymap::{Binding, HelpRef, KeySpec, Keymap, NamedKey};

/// Preview focus keys.
pub static KEYMAP: Keymap = &[
    Binding {
        keys: &[
            KeySpec::Named(NamedKey::Escape),
            KeySpec::Named(NamedKey::Left),
            KeySpec::Char("h"),
        ],
        action: "back to the session list",
        help: HelpRef::Topic("Panes"),
    },
    Binding {
        keys: &[KeySpec::Char("+"), KeySpec::Char("-")],
        action: "mouse-wheel speed (saved)",
        help: HelpRef::Topic("Panes"),
    },
    Binding {
        keys: &[KeySpec::Char("=")],
        action: "mouse-wheel speed up",
        help: HelpRef::Hidden("= is + without Shift"),
    },
    Binding {
        keys: &[
            KeySpec::Char("k"),
            KeySpec::Named(NamedKey::Up),
            KeySpec::Char("j"),
            KeySpec::Named(NamedKey::Down),
        ],
        action: "scroll",
        help: HelpRef::Topic("Panes"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::PageUp), KeySpec::Named(NamedKey::PageDown)],
        action: "scroll a page",
        help: HelpRef::Topic("Panes"),
    },
    Binding {
        keys: &[KeySpec::Char("g"), KeySpec::Char("G")],
        action: "top / bottom",
        help: HelpRef::Topic("Panes"),
    },
];

/// TS: the preview-focus branch of the useTerminalInput handler.
pub fn handle_preview_focus(app: &mut App, input: &Input) {
    let key = &input.key;
    let text = input.input.as_str();
    if key.escape || key.left_arrow || text == "h" {
        app.set_mode(Mode::Browse);
        return;
    }
    // Mouse-wheel speed (also used when attached), saved to the config.
    if text == "+" || text == "=" || text == "-" {
        adjust_scroll_sensitivity(
            app,
            if text == "-" {
                -SCROLL_SENSITIVITY_STEP
            } else {
                SCROLL_SENSITIVITY_STEP
            },
        );
        return;
    }
    if text == "k" || key.up_arrow {
        scroll_preview(app, WheelDirection::Up, 1);
        return;
    }
    if text == "j" || key.down_arrow {
        scroll_preview(app, WheelDirection::Down, 1);
        return;
    }
    let rows = i64::from(app.layout().preview_rows);
    if key.page_up || key.page_down {
        let direction = if key.page_up {
            WheelDirection::Up
        } else {
            WheelDirection::Down
        };
        scroll_preview(app, direction, usize::try_from((rows - 2).max(1)).unwrap_or(1));
        return;
    }
    if text == "g" {
        // A fullscreen agent scrolls its own view: Ctrl+Home is its transcript's start (not in the TS, which sent 12
        // wheel events).
        if !send_agent_scroll_input(app, CTRL_HOME) {
            let offset = app.preview_focus.scroll_offset;
            app.preview_focus.scroll_offset = app.panes.preview.max_scroll_offset.unwrap_or(offset + rows);
        }
        return;
    }
    if text == "G" && !send_agent_scroll_input(app, CTRL_END) {
        app.preview_focus.scroll_offset = 0;
    }
}

/// Ctrl+Home / Ctrl+End (xterm): the start of the transcript / the latest output (following it again) in Claude's,
/// Codex's and Pi's fullscreen views.
const CTRL_HOME: &str = "\x1b[1;5H";
const CTRL_END: &str = "\x1b[1;5F";

/// TS: the preview-focus `stdin.on('data')` listener: wheel reports scaled by the sensitivity ([`WheelScaler`], as
/// when attached).
pub fn on_mouse_data(app: &mut App, text: &str) {
    let Some(wheel) = parse_mouse_wheel(text) else {
        return;
    };
    let now = Instant::now();
    let sensitivity = app.preview_focus.scroll_sensitivity;
    let scaled: usize = (0..wheel.count)
        .map(|_| app.preview_focus.wheel.scale(sensitivity, wheel.direction, now))
        .sum();
    if scaled == 0 {
        return;
    }
    scroll_preview(app, wheel.direction, scaled);
}

/// JS `Math.round` (halves round up, towards +∞).
fn js_round(value: f64) -> f64 {
    let floor = value.floor();
    if value - floor >= 0.5 { floor + 1.0 } else { floor }
}

/// TS `adjustScrollSensitivity` (+ / −): saved to the config.
pub fn adjust_scroll_sensitivity(app: &mut App, delta: f64) {
    let current = app.preview_focus.scroll_sensitivity;
    let next = normalize_scroll_sensitivity(Some(js_round((current + delta) * 100.0) / 100.0));
    app.preview_focus.scroll_sensitivity = next;
    app.preview_focus.wheel = WheelScaler::default();
    app.set_status(Some(format!(
        "Scroll speed {} (saved)",
        format_scroll_sensitivity(next)
    )));
    let saved = patch_app_config(AppConfig {
        scroll_speed: Some(JsNumber(next)),
        ..AppConfig::default()
    });
    drop(app.dispatch().spawn(saved, |app: &mut App, result| {
        if let Err(error) = result {
            app.set_error(Some(error.to_string()));
        }
    }));
}

/// TS `sendClaudeWheel`, for every agent: a running agent whose preview has no scrollback (Claude, Pi and Codex all
/// draw fullscreen in the alternate screen, which keeps none) scrolls its own view; true when sent. An agent printing
/// inline (Pi `tuiMode: "regular"`, Codex `--no-alt-screen`) leaves its history in the preview's scrollback instead.
pub fn send_agent_wheel(app: &mut App, direction: WheelDirection, count: usize) -> bool {
    let layout = app.layout();
    let wheel = mouse_wheel_sequence(direction, layout.preview_cols, layout.preview_rows, count);
    send_agent_scroll_input(app, &wheel)
}

/// [`send_agent_wheel`] with any input that scrolls the agent's view.
fn send_agent_scroll_input(app: &mut App, data: &str) -> bool {
    let Some(client) = app.client() else {
        return false;
    };
    if app.panes.preview.max_scroll_offset.is_some_and(|max| max > 0) {
        return false;
    }
    let Some(session) = app
        .selected_session()
        .filter(|session| session.status == SessionStatus::Running)
    else {
        return false;
    };
    client.send_agent_input(&session.id, data);
    app.preview_focus.scroll_offset = 0;
    true
}

/// TS `scrollPreview`: the wheel to a fullscreen agent, else the preview's scroll offset.
pub fn scroll_preview(app: &mut App, direction: WheelDirection, count: usize) {
    if send_agent_wheel(app, direction, count) {
        return;
    }
    let count = i64::try_from(count).unwrap_or(i64::MAX);
    let offset = app.preview_focus.scroll_offset;
    app.preview_focus.scroll_offset = match direction {
        WheelDirection::Up => {
            let up = offset.saturating_add(count);
            app.panes.preview.max_scroll_offset.unwrap_or(up).min(up)
        }
        WheelDirection::Down => offset.saturating_sub(count).max(0),
    };
}

/// TS: the changes-focus branch: the Changes flow's keys.
pub fn handle_changes_focus(app: &mut App, input: &Input) {
    app.changes_flow_call(|flow, props, cx| flow.handle_input(props, input, cx));
}

/// TS: the notes-focus branch: the Notes flow's keys.
pub fn handle_notes_focus(app: &mut App, input: &Input) {
    app.notes_flow_call(|flow, props, cx| flow.handle_input(props, input, cx));
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use deckhand_core::key::Key;
    use deckhand_protocol::data::{ProgramKey, SessionStatus};
    use deckhand_sys::storage::load_app_config;

    use super::*;
    use crate::app::modes::browse::tests::{Daemon, apply, local, named, text, with_sessions};

    /// Tests that change `DECKHAND_HOME` hold this.
    static ENV: Mutex<()> = Mutex::new(());

    #[test]
    fn esc_left_and_h_step_back_out() {
        for input in [
            named(Key {
                escape: true,
                ..Key::NONE
            }),
            named(Key {
                left_arrow: true,
                ..Key::NONE
            }),
            text("h"),
        ] {
            let (mut app, _rx) = with_sessions(1);
            app.set_mode(Mode::PreviewFocus);
            handle_preview_focus(&mut app, &input);
            assert_eq!(app.mode, Mode::Browse);
        }
    }

    #[test]
    fn keys_scroll_the_preview_within_its_bounds() {
        let (mut app, _rx) = with_sessions(1);
        app.set_mode(Mode::PreviewFocus);
        app.panes.preview.max_scroll_offset = Some(3);
        for _ in 0..5 {
            handle_preview_focus(&mut app, &text("k"));
        }
        assert_eq!(app.preview_focus.scroll_offset, 3);
        handle_preview_focus(
            &mut app,
            &named(Key {
                down_arrow: true,
                ..Key::NONE
            }),
        );
        assert_eq!(app.preview_focus.scroll_offset, 2);
        handle_preview_focus(&mut app, &text("j"));
        handle_preview_focus(&mut app, &text("j"));
        handle_preview_focus(&mut app, &text("j"));
        assert_eq!(app.preview_focus.scroll_offset, 0);
        handle_preview_focus(&mut app, &text("g"));
        assert_eq!(app.preview_focus.scroll_offset, 3);
        handle_preview_focus(&mut app, &text("G"));
        assert_eq!(app.preview_focus.scroll_offset, 0);
        // Without a known maximum: up as far as asked; g a page up.
        app.panes.preview.max_scroll_offset = None;
        let rows = i64::from(app.layout().preview_rows);
        handle_preview_focus(
            &mut app,
            &named(Key {
                page_up: true,
                ..Key::NONE
            }),
        );
        assert_eq!(app.preview_focus.scroll_offset, rows - 2);
        handle_preview_focus(
            &mut app,
            &named(Key {
                page_down: true,
                ..Key::NONE
            }),
        );
        assert_eq!(app.preview_focus.scroll_offset, 0);
        handle_preview_focus(&mut app, &text("g"));
        assert_eq!(app.preview_focus.scroll_offset, rows);
        // Other keys do nothing.
        handle_preview_focus(&mut app, &text("x"));
        assert_eq!(app.preview_focus.scroll_offset, rows);
        assert_eq!(app.mode, Mode::PreviewFocus);
    }

    #[test]
    fn the_wheel_is_scaled_by_the_sensitivity() {
        let (mut app, _rx) = with_sessions(1);
        // The default: every event.
        let up = mouse_wheel_sequence(WheelDirection::Up, 10, 10, 1);
        on_mouse_data(&mut app, &mouse_wheel_sequence(WheelDirection::Up, 10, 10, 3));
        assert_eq!(app.preview_focus.scroll_offset, 3);
        app.preview_focus.scroll_offset = 0;
        app.preview_focus.scroll_sensitivity = 0.5;
        app.preview_focus.wheel = WheelScaler::default();
        // The first event of a gesture moves, then every other one.
        on_mouse_data(&mut app, &up);
        assert_eq!(app.preview_focus.scroll_offset, 1);
        on_mouse_data(&mut app, &up);
        assert_eq!(app.preview_focus.scroll_offset, 1);
        on_mouse_data(&mut app, &mouse_wheel_sequence(WheelDirection::Up, 10, 10, 4));
        assert_eq!(app.preview_focus.scroll_offset, 3);
        on_mouse_data(&mut app, &mouse_wheel_sequence(WheelDirection::Down, 10, 10, 6));
        assert_eq!(app.preview_focus.scroll_offset, 0);
        on_mouse_data(&mut app, "plain text");
        assert_eq!(app.preview_focus.scroll_offset, 0);
    }

    #[test]
    fn a_running_fullscreen_agent_gets_the_wheel_itself() {
        local(async {
            let (mut app, _rx) = with_sessions(1);
            let mut daemon = Daemon::connect(&mut app);
            app.panes.preview.max_scroll_offset = Some(0);
            app.preview_focus.scroll_offset = 2;
            app.preview_focus.scroll_sensitivity = 1.0;
            let layout = app.layout();
            on_mouse_data(&mut app, &mouse_wheel_sequence(WheelDirection::Up, 1, 1, 2));
            let message = daemon.next().await;
            assert_eq!(message["type"], "input");
            assert_eq!(message["sessionId"], "s0");
            assert_eq!(
                message["data"],
                mouse_wheel_sequence(WheelDirection::Up, layout.preview_cols, layout.preview_rows, 2)
            );
            assert_eq!(app.preview_focus.scroll_offset, 0);
            // g / G: the agent's own top and bottom keys.
            handle_preview_focus(&mut app, &text("g"));
            assert_eq!(daemon.next().await["data"], CTRL_HOME);
            handle_preview_focus(&mut app, &text("G"));
            assert_eq!(daemon.next().await["data"], CTRL_END);
            // Pi and Codex too.
            for program in [ProgramKey::Pi, ProgramKey::Codex] {
                app.data.repo_sessions[0].program = program;
                handle_preview_focus(&mut app, &text("k"));
                assert_eq!(
                    daemon.next().await["data"],
                    mouse_wheel_sequence(WheelDirection::Up, layout.preview_cols, layout.preview_rows, 1)
                );
            }
            // An agent printing inline left scrollback in the preview: that scrolls instead.
            app.panes.preview.max_scroll_offset = Some(3);
            assert!(!send_agent_wheel(&mut app, WheelDirection::Up, 1));
            handle_preview_focus(&mut app, &text("g"));
            assert_eq!(app.preview_focus.scroll_offset, 3);
            app.preview_focus.scroll_offset = 0;
            app.panes.preview.max_scroll_offset = None;
            // An exited session scrolls the preview instead.
            app.data.repo_sessions[0].status = SessionStatus::Exited;
            assert!(!send_agent_wheel(&mut app, WheelDirection::Up, 1));
            scroll_preview(&mut app, WheelDirection::Up, 1);
            assert_eq!(app.preview_focus.scroll_offset, 1);
        });
    }

    #[test]
    fn plus_and_minus_change_the_wheel_speed_and_save_it() {
        let _guard = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = tempfile::Builder::new()
            .prefix("deckhand-preview-")
            .tempdir()
            .unwrap_or_else(|error| panic!("{error}"));
        let previous = std::env::var_os(deckhand_sys::paths::HOME_ENV);
        // SAFETY: tests touching the environment hold ENV.
        unsafe { std::env::set_var(deckhand_sys::paths::HOME_ENV, home.path()) };
        local(async {
            let (mut app, mut rx) = with_sessions(1);
            app.preview_focus
                .wheel
                .scale(0.5, WheelDirection::Up, std::time::Instant::now());
            handle_preview_focus(&mut app, &text("+"));
            assert_eq!(app.preview_focus.scroll_sensitivity, 1.1);
            assert_eq!(app.preview_focus.wheel, WheelScaler::default());
            assert_eq!(app.messages.status_message.as_deref(), Some("Scroll speed 1.1 (saved)"));
            apply(&mut app, &mut rx).await;
            handle_preview_focus(&mut app, &text("="));
            handle_preview_focus(&mut app, &text("-"));
            handle_preview_focus(&mut app, &text("-"));
            assert_eq!(app.preview_focus.scroll_sensitivity, 1.0);
            for _ in 0..3 {
                apply(&mut app, &mut rx).await;
            }
            assert_eq!(app.messages.error, None);
            let config = load_app_config().await.unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(config.scroll_speed.map(|value| value.0), Some(1.0));
            assert_eq!(config.attach_scroll_sensitivity, None);
            for _ in 0..40 {
                adjust_scroll_sensitivity(&mut app, SCROLL_SENSITIVITY_STEP);
            }
            assert_eq!(app.preview_focus.scroll_sensitivity, 3.0);
            for _ in 0..40 {
                adjust_scroll_sensitivity(&mut app, -SCROLL_SENSITIVITY_STEP);
            }
            assert_eq!(app.preview_focus.scroll_sensitivity, 0.0);
            assert_eq!(app.messages.status_message.as_deref(), Some("Scroll speed 0 (saved)"));
        });
        // SAFETY: as above (ENV is still held).
        unsafe {
            match previous {
                Some(value) => std::env::set_var(deckhand_sys::paths::HOME_ENV, value),
                None => std::env::remove_var(deckhand_sys::paths::HOME_ENV),
            }
        }
    }

    #[test]
    fn js_round_rounds_halves_up() {
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(js_round(15.999999999999998), 16.0);
        assert_eq!(js_round(0.49999999999999994), 0.0);
    }
}
