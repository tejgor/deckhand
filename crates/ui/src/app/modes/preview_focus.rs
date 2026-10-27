//! Preview focus (→ on a running session's Preview): typing to the agent in place. Every key goes to the agent as
//! the terminal sent it, until Ctrl+] or Ctrl+Space; the mouse wheel scrolls the preview, or a fullscreen agent's
//! own view. Also the changes-/notes-focus hand-off to their flows.

use std::time::Instant;

use deckhand_client::attach::detach_position;
use deckhand_core::display::display_session_title;
use deckhand_core::key::{Input, KeyEventType};
use deckhand_core::session_features::session_has_unread_reply;
use deckhand_core::terminal_keys::application_cursor_key;
use deckhand_protocol::data::SessionStatus;

use crate::app::event_loop::is_mouse_report;
use crate::app::state::{App, Mode};
use crate::app::{WheelDirection, mouse_wheel_sequence, parse_mouse_wheel};
use crate::flow::Flow;
use crate::input::KeyEvent;
use crate::keymap::{Binding, HelpRef, KeySpec, Keymap};

/// Preview focus keys: everything else goes to the agent.
pub static KEYMAP: Keymap = &[Binding {
    keys: &[KeySpec::Ctrl("]")],
    action: "back to the session list",
    help: HelpRef::Topic("Panes"),
}];

/// Bracketed-paste markers, as the terminal wraps a paste while preview focus has bracketed paste on.
const PASTE_START: &str = "\x1b[200~";
const PASTE_END: &str = "\x1b[201~";

/// One key event in preview focus, before any of Deckhand's own key handling (Ctrl+C included goes to the agent):
/// sent to the agent as the terminal sent it, one `input` per event, so the daemon sees a lone Esc, Enter or digit
/// as an answer. Ctrl+] or Ctrl+Space (outside a paste) sends what came before it and leaves.
pub fn on_raw_key(app: &mut App, event: &KeyEvent) {
    let sequence = event.sequence.as_str();
    // The wheel arrives through `on_mouse_data`; clicks and motion are not forwarded.
    if is_mouse_report(sequence) || event.input.key.event_type == Some(KeyEventType::Release) {
        return;
    }
    let modes = app
        .panes
        .preview
        .screen
        .as_ref()
        .map(|screen| screen.modes)
        .unwrap_or_default();
    if sequence == PASTE_START || sequence == PASTE_END {
        app.preview_focus.in_paste = sequence == PASTE_START;
        if modes.bracketed_paste {
            send_to_agent(app, sequence);
        }
        return;
    }
    if !app.preview_focus.in_paste
        && let Some(at) = detach_position(sequence.as_bytes())
    {
        // Every detach key starts with an ASCII byte: `at` is a char boundary.
        let before = &sequence[..at];
        if !before.is_empty() {
            send_to_agent(app, before);
        }
        leave_focus(app, None);
        return;
    }
    let data = if modes.application_cursor_keys {
        application_cursor_key(sequence).unwrap_or(sequence)
    } else {
        sequence
    };
    send_to_agent(app, data);
}

/// Keys for the focused agent: the preview follows the latest output again.
fn send_to_agent(app: &mut App, data: &str) {
    let (Some(client), Some(session_id)) = (app.client(), app.preview_focus.session_id.clone()) else {
        return;
    };
    app.preview_focus.scroll_offset = 0;
    client.send_agent_input(&session_id, data);
}

/// Back to the session list, with `status` if given. A reply the agent finished while you typed to it was read, as
/// after detaching from it.
pub fn leave_focus(app: &mut App, status: Option<String>) {
    app.set_mode(Mode::Browse);
    app.preview_focus.in_paste = false;
    app.panes.preview.screen = None;
    if status.is_some() {
        app.set_status(status);
    }
    let Some(session_id) = app.preview_focus.session_id.take() else {
        return;
    };
    let unread = app
        .sessions()
        .iter()
        .any(|session| session.id == session_id && session_has_unread_reply(session));
    let Some(client) = app.client().filter(|_| unread) else {
        return;
    };
    let reply = client.set_reply_read(&session_id, true);
    drop(app.dispatch().spawn(reply, |app: &mut App, result| match result {
        Ok(updated) => app.upsert(updated),
        Err(error) => app.set_error(Some(error.to_string())),
    }));
}

/// Not in the TS: preview focus ends by itself when its session stops running, the selection moves to another
/// session, or the connection to the daemon goes (its error is shown already).
pub fn interactive_focus_effect(app: &mut App) {
    if app.mode != Mode::PreviewFocus {
        return;
    }
    if app.client().is_none() {
        leave_focus(app, None);
        return;
    }
    let focused = app.preview_focus.session_id.clone();
    let Some(session) = app
        .selected_session()
        .filter(|session| focused.as_deref() == Some(session.id.as_str()))
    else {
        leave_focus(app, None);
        return;
    };
    if session.status != SessionStatus::Running {
        let title = display_session_title(&session, &app.sessions());
        leave_focus(app, Some(format!("{title} exited; back to the session list")));
    }
}

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
pub(crate) mod tests {
    use std::sync::Mutex;

    use deckhand_core::wheel::WheelScaler;
    use deckhand_protocol::data::{ProgramKey, SessionAttention, SessionStatus};
    use deckhand_protocol::screen::{PreviewScreen, ScreenModes};
    use serde_json::json;

    use super::*;
    use crate::app::modes::browse::tests::{Daemon, local, with_sessions};

    /// Tests that change `DECKHAND_HOME` hold this.
    pub(crate) static ENV: Mutex<()> = Mutex::new(());

    /// An app typing to s0.
    fn focused() -> (
        App,
        tokio::sync::mpsc::UnboundedReceiver<crate::app::event_loop::Update<App>>,
    ) {
        let (mut app, rx) = with_sessions(1);
        app.set_mode(Mode::PreviewFocus);
        app.preview_focus.session_id = Some("s0".to_owned());
        (app, rx)
    }

    fn key(app: &mut App, sequence: &str) {
        on_raw_key(app, &KeyEvent::from_sequence(sequence.to_owned()));
    }

    fn with_modes(app: &mut App, modes: ScreenModes) {
        app.panes.preview.screen = Some(PreviewScreen {
            modes,
            ..PreviewScreen::default()
        });
    }

    fn reply_ended(app: &mut App) {
        app.data.repo_sessions[0].attention = Some(
            serde_json::from_value::<SessionAttention>(
                json!({"state": "response-ended", "event": "Stop", "at": "2026-01-01T00:00:00.000Z"}),
            )
            .unwrap_or_else(|error| panic!("{error}")),
        );
    }

    #[test]
    fn every_key_goes_to_the_agent_as_typed() {
        local(async {
            let (mut app, _rx) = focused();
            let mut daemon = Daemon::connect(&mut app);
            app.preview_focus.scroll_offset = 4;
            // Esc, h and j no longer step out or scroll; Ctrl+C does not quit.
            for sequence in ["\x1b", "h", "j", "1", "\x03", "\r", "hi", "\x1b[A", "\x7f"] {
                key(&mut app, sequence);
                let message = daemon.next().await;
                assert_eq!(message["type"], "input");
                assert_eq!(message["sessionId"], "s0");
                assert_eq!(message["data"], sequence);
            }
            assert_eq!(app.mode, Mode::PreviewFocus);
            assert!(app.connection.exit.is_none());
            assert_eq!(app.preview_focus.scroll_offset, 0, "typing follows the latest output");
        });
    }

    #[test]
    fn ctrl_bracket_and_ctrl_space_leave_after_sending_what_came_first() {
        local(async {
            let (mut app, _rx) = focused();
            let mut daemon = Daemon::connect(&mut app);
            with_modes(&mut app, ScreenModes::default());
            key(&mut app, "ab\x1d");
            assert_eq!(daemon.next().await["data"], "ab");
            assert_eq!(app.mode, Mode::Browse);
            assert!(app.panes.preview.screen.is_none());
            assert!(app.preview_focus.session_id.is_none());

            // Leaving marks a reply the agent finished meanwhile read, as detaching does.
            for detach in ["\x00", "\x1b[32;5u", "\x1b[27;5;32~"] {
                app.set_mode(Mode::PreviewFocus);
                app.preview_focus.session_id = Some("s0".to_owned());
                reply_ended(&mut app);
                key(&mut app, detach);
                assert_eq!(app.mode, Mode::Browse);
                let message = daemon.next().await;
                assert_eq!(message["type"], "set-reply-read", "{detach:?}: {message}");
                assert_eq!(message["sessionId"], "s0");
                assert_eq!(message["read"], true);
            }
        });
    }

    #[test]
    fn paste_markers_reach_only_an_agent_that_asked_for_them() {
        local(async {
            let (mut app, _rx) = focused();
            let mut daemon = Daemon::connect(&mut app);
            // Without bracketed paste the markers are dropped, and a NUL inside a paste is pasted, not a detach.
            key(&mut app, PASTE_START);
            key(&mut app, "x\x00y");
            key(&mut app, PASTE_END);
            key(&mut app, "z");
            assert_eq!(daemon.next().await["data"], "x\x00y");
            assert_eq!(daemon.next().await["data"], "z");
            assert_eq!(app.mode, Mode::PreviewFocus);
            with_modes(
                &mut app,
                ScreenModes {
                    bracketed_paste: true,
                    ..ScreenModes::default()
                },
            );
            for sequence in [PASTE_START, "multi\rline", PASTE_END] {
                key(&mut app, sequence);
                assert_eq!(daemon.next().await["data"], sequence);
            }
            assert!(!app.preview_focus.in_paste);
        });
    }

    #[test]
    fn cursor_keys_follow_the_agents_cursor_key_mode() {
        local(async {
            let (mut app, _rx) = focused();
            let mut daemon = Daemon::connect(&mut app);
            with_modes(
                &mut app,
                ScreenModes {
                    application_cursor_keys: true,
                    ..ScreenModes::default()
                },
            );
            key(&mut app, "\x1b[A");
            assert_eq!(daemon.next().await["data"], "\x1bOA");
            key(&mut app, "\x1b[1;5C");
            assert_eq!(daemon.next().await["data"], "\x1b[1;5C");
        });
    }

    #[test]
    fn mouse_reports_are_not_typed() {
        local(async {
            let (mut app, _rx) = focused();
            let mut daemon = Daemon::connect(&mut app);
            key(&mut app, "\x1b[<0;3;4M");
            key(&mut app, "\x1b[M !!");
            key(&mut app, "z");
            assert_eq!(daemon.next().await["data"], "z");
        });
    }

    #[test]
    fn focus_ends_by_itself_when_its_agent_cannot_take_keys() {
        local(async {
            let (mut app, _rx) = focused();
            let _daemon = Daemon::connect(&mut app);
            interactive_focus_effect(&mut app);
            assert_eq!(app.mode, Mode::PreviewFocus);
            app.data.repo_sessions[0].status = SessionStatus::Exited;
            interactive_focus_effect(&mut app);
            assert_eq!(app.mode, Mode::Browse);
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Session 0 exited; back to the session list")
            );

            // Another session selected: back quietly.
            let (mut app, _rx) = with_sessions(2);
            let _daemon = Daemon::connect(&mut app);
            app.set_mode(Mode::PreviewFocus);
            app.preview_focus.session_id = Some("s1".to_owned());
            interactive_focus_effect(&mut app);
            assert_eq!(app.mode, Mode::Browse);
            assert_eq!(app.messages.status_message, None);
        });
        // No daemon: back too.
        let (mut app, _rx) = focused();
        interactive_focus_effect(&mut app);
        assert_eq!(app.mode, Mode::Browse);
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
            // Pi and Codex too.
            for program in [ProgramKey::Pi, ProgramKey::Codex] {
                app.data.repo_sessions[0].program = program;
                scroll_preview(&mut app, WheelDirection::Up, 1);
                assert_eq!(
                    daemon.next().await["data"],
                    mouse_wheel_sequence(WheelDirection::Up, layout.preview_cols, layout.preview_rows, 1)
                );
            }
            // An agent printing inline left scrollback in the preview: that scrolls instead.
            app.panes.preview.max_scroll_offset = Some(3);
            assert!(!send_agent_wheel(&mut app, WheelDirection::Up, 1));
            scroll_preview(&mut app, WheelDirection::Up, 5);
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
}
