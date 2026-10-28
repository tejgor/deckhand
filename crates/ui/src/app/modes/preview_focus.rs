//! Preview focus (→ on a running session's Preview): typing to the agent in place. Every key goes to the agent as
//! the terminal sent it, until Ctrl+] or Ctrl+Space; the mouse wheel scrolls the preview, or a fullscreen agent's
//! own view. Terminal focus (→ on the Terminal tab) types into the shell shown the same way. Also the changes-/
//! notes-focus hand-off to their flows.

use std::time::Instant;

use deckhand_client::attach::detach_position;
use deckhand_core::display::display_session_title;
use deckhand_core::jsstr::date::now_ms;
use deckhand_core::key::{Input, KeyEventType};
use deckhand_core::session_features::session_has_unread_reply;
use deckhand_core::terminal_keys::application_cursor_key;
use deckhand_protocol::data::{RightPaneTab, SessionStatus};
use deckhand_protocol::screen::ScreenModes;

use crate::app::event_loop::is_mouse_report;
use crate::app::state::{App, Mode};
use crate::app::{WheelDirection, mouse_wheel_sequence, parse_mouse_wheel};
use crate::flow::Flow;
use crate::input::KeyEvent;
use crate::keymap::{Binding, HelpRef, KeySpec, Keymap};
use crate::panes::terminal::{TerminalView, shells, shown_shell};

/// Preview and terminal focus keys: everything else goes to the agent or the shell.
pub static KEYMAP: Keymap = &[Binding {
    keys: &[KeySpec::Ctrl("]")],
    action: "back to the session list",
    help: HelpRef::Topic("Panes"),
}];

/// Bracketed-paste markers, as the terminal wraps a paste while preview focus has bracketed paste on.
const PASTE_START: &str = "\x1b[200~";
const PASTE_END: &str = "\x1b[201~";

/// One key event in preview or terminal focus, before any of Deckhand's own key handling (Ctrl+C included goes to
/// the agent or shell): sent as the terminal sent it, one `input` per event, so the daemon sees a lone Esc, Enter or
/// digit as an answer. Ctrl+] or Ctrl+Space (outside a paste) sends what came before it and leaves.
pub fn on_raw_key(app: &mut App, event: &KeyEvent) {
    let sequence = event.sequence.as_str();
    // The wheel arrives through `on_mouse_data`; clicks and motion are not forwarded.
    if is_mouse_report(sequence) || event.input.key.event_type == Some(KeyEventType::Release) {
        return;
    }
    let modes = typed_modes(app);
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
    // Not in the TS: the first Ctrl+C of each stint says where it went (to the agent, which a second one may quit)
    // and how to get back; it is still sent.
    if sequence == "\x03" && !app.preview_focus.ctrl_c_hinted {
        app.preview_focus.ctrl_c_hinted = true;
        let to = if app.mode == Mode::TerminalFocus {
            "the shell"
        } else {
            "the agent"
        };
        app.set_status(Some(format!("ctrl+c went to {to}; ctrl+] returns to the list")));
    }
    let data = if modes.application_cursor_keys {
        application_cursor_key(sequence).unwrap_or(sequence)
    } else {
        sequence
    };
    send_to_agent(app, data);
}

/// The input modes of what the keys go to (the agent's screen, or the shell's).
fn typed_modes(app: &App) -> ScreenModes {
    let screen = match app.preview_focus.shell {
        Some(id) if app.mode == Mode::TerminalFocus => shells(&app.panes.terminal)
            .into_iter()
            .find(|shell| shell.id == id)
            .and_then(|shell| shell.screen),
        _ => app.panes.preview.screen.as_ref(),
    };
    screen.map(|screen| screen.modes).unwrap_or_default()
}

/// Keys for the focused agent (the preview follows the latest output again) or shell.
fn send_to_agent(app: &mut App, data: &str) {
    let (Some(client), Some(session_id)) = (app.client(), app.preview_focus.session_id.clone()) else {
        return;
    };
    if app.mode == Mode::TerminalFocus {
        client.send_shell_input(&session_id, app.preview_focus.shell.unwrap_or(0), data);
        return;
    }
    app.preview_focus.scroll_offset = 0;
    client.send_agent_input(&session_id, data);
}

/// Back to the session list, with `status` if given. A reply the agent finished while you typed to it was read, as
/// after detaching from it.
pub fn leave_focus(app: &mut App, status: Option<String>) {
    let typed_to_agent = app.mode == Mode::PreviewFocus;
    app.set_mode(Mode::Browse);
    app.preview_focus.in_paste = false;
    app.preview_focus.ctrl_c_hinted = false;
    app.preview_focus.shell = None;
    app.panes.preview.screen = None;
    if status.is_some() {
        app.set_status(status);
    }
    let session_id = app.preview_focus.session_id.take();
    let Some(session_id) = session_id.filter(|_| typed_to_agent) else {
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

/// Not in the TS: after typing ends by itself, list keys wait for this long a pause, so the rest of a sentence being
/// typed doesn't run commands (Backspace archives, s / S restart).
pub const TYPING_ENDED_QUIET_MS: i64 = 750;

/// Typing ended by itself: back to the session list, holding keys until the typing stops.
fn end_focus(app: &mut App, status: Option<String>) {
    leave_focus(app, status);
    app.preview_focus.held_since = Some(now_ms());
}

/// Whether a key on the session list is held back: it came less than [`TYPING_ENDED_QUIET_MS`] after typing ended by
/// itself, or after the last key held. Ctrl+C is never held.
pub fn hold_key(app: &mut App, input: &Input, now: i64) -> bool {
    let Some(since) = app.preview_focus.held_since else {
        return false;
    };
    if input.key.ctrl && input.input == "c" {
        return false;
    }
    if now - since >= TYPING_ENDED_QUIET_MS {
        app.preview_focus.held_since = None;
        return false;
    }
    app.preview_focus.held_since = Some(now);
    true
}

/// Not in the TS: preview focus ends by itself when its session stops running, the selection moves to another
/// session, or the connection to the daemon goes (its error is shown already); terminal focus when its shell is gone
/// or no longer shown.
pub fn interactive_focus_effect(app: &mut App) {
    if !app.mode.types_into_pane() {
        return;
    }
    if app.client().is_none() {
        end_focus(app, None);
        return;
    }
    let focused = app.preview_focus.session_id.clone();
    let Some(session) = app
        .selected_session()
        .filter(|session| focused.as_deref() == Some(session.id.as_str()))
    else {
        end_focus(app, None);
        return;
    };
    if app.mode == Mode::TerminalFocus {
        shell_focus_effect(app);
        return;
    }
    if session.status != SessionStatus::Running {
        let title = display_session_title(&session, &app.sessions());
        end_focus(app, Some(format!("{title} exited; back to the session list")));
    }
}

/// [`interactive_focus_effect`] of terminal focus: its shell must still run and be the one shown.
fn shell_focus_effect(app: &mut App) {
    let id = app.preview_focus.shell.unwrap_or(0);
    let terminal = &app.panes.terminal;
    if app.browse.active_tab != RightPaneTab::Terminal || app.panes.terminal_view == TerminalView::Action {
        end_focus(app, None);
        return;
    }
    let mine = terminal.session_id == app.preview_focus.session_id;
    let running = shells(terminal).iter().any(|shell| shell.id == id && shell.live);
    if mine && (!running || shown_shell(terminal, app.panes.terminal_view) != id) {
        let status = if running {
            None
        } else {
            Some("The shell exited; back to the session list".to_owned())
        };
        end_focus(app, status);
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
    use deckhand_protocol::data::{ProgramKey, RightPaneTab, SessionAttention, SessionStatus};
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
            // The first Ctrl+C of the stint said where it went, once.
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("ctrl+c went to the agent; ctrl+] returns to the list")
            );
            app.set_status(None);
            key(&mut app, "\x03");
            assert_eq!(daemon.next().await["data"], "\x03");
            assert_eq!(app.messages.status_message, None);
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
            // The keys still being typed are held until a pause, then the list takes them; Ctrl+C always goes through.
            let since = app.preview_focus.held_since.unwrap_or_else(|| panic!("held"));
            let backspace = Input::key(
                "",
                deckhand_core::key::Key {
                    backspace: true,
                    ..deckhand_core::key::Key::NONE
                },
            );
            assert!(hold_key(&mut app, &backspace, since + 100));
            assert!(hold_key(&mut app, &Input::text("s"), since + 700));
            let ctrl_c = Input::key(
                "c",
                deckhand_core::key::Key {
                    ctrl: true,
                    ..deckhand_core::key::Key::NONE
                },
            );
            assert!(!hold_key(&mut app, &ctrl_c, since + 800));
            assert!(
                hold_key(&mut app, &Input::text("x"), since + 1300),
                "the pause counts from the last key"
            );
            assert!(!hold_key(
                &mut app,
                &Input::text("j"),
                since + 1300 + TYPING_ENDED_QUIET_MS
            ));
            assert_eq!(app.preview_focus.held_since, None);

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

    /// An app typing into shell 2 of s0's worktree, on the Terminal tab.
    fn typing_into_a_shell() -> (
        App,
        tokio::sync::mpsc::UnboundedReceiver<crate::app::event_loop::Update<App>>,
    ) {
        let (mut app, rx) = with_sessions(1);
        app.set_active_tab(RightPaneTab::Terminal);
        app.panes.terminal = serde_json::from_value(json!({
            "content": "$ ", "live": true, "sessionId": "s0", "workspace": "/repo",
            "shells": [
                {"id": 0, "content": "$ ", "live": true},
                {"id": 2, "content": "$ ", "live": true,
                 "screen": {"lines": [], "modes": {"bracketedPaste": true, "applicationCursorKeys": false, "mouseTracking": false}}},
            ],
        }))
        .unwrap_or_else(|error| panic!("{error}"));
        app.panes.terminal_view = TerminalView::Shell(2);
        app.set_mode(Mode::TerminalFocus);
        app.preview_focus.session_id = Some("s0".to_owned());
        app.preview_focus.shell = Some(2);
        (app, rx)
    }

    #[test]
    fn keys_typed_into_a_shell_go_to_it_and_leaving_reads_no_reply() {
        local(async {
            let (mut app, _rx) = typing_into_a_shell();
            let mut daemon = Daemon::connect(&mut app);
            reply_ended(&mut app);
            for sequence in ["\x1b", "ls\r", "\x03"] {
                key(&mut app, sequence);
                assert_eq!(
                    daemon.next().await,
                    json!({"type": "terminal-input", "sessionId": "s0", "data": sequence, "shell": 2})
                );
            }
            // The shell's own modes decide the paste markers.
            key(&mut app, PASTE_START);
            assert_eq!(daemon.next().await["data"], PASTE_START);
            key(&mut app, PASTE_END);
            daemon.next().await;
            key(&mut app, "x\x1d");
            assert_eq!(daemon.next().await["data"], "x");
            assert_eq!(app.mode, Mode::Browse);
            assert_eq!(app.preview_focus.shell, None);
            // Typing into a shell reads no reply: nothing more was sent.
            key(&mut app, "z");
            app.client()
                .unwrap_or_else(|| panic!("client"))
                .send_agent_input("s0", "probe");
            assert_eq!(daemon.next().await["data"], "probe");
        });
    }

    #[test]
    fn terminal_focus_ends_when_its_shell_goes_or_is_not_shown() {
        local(async {
            let (mut app, _rx) = typing_into_a_shell();
            let _daemon = Daemon::connect(&mut app);
            interactive_focus_effect(&mut app);
            assert_eq!(app.mode, Mode::TerminalFocus);
            app.panes.terminal.shells = None;
            interactive_focus_effect(&mut app);
            assert_eq!(app.mode, Mode::Browse);
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("The shell exited; back to the session list")
            );

            let (mut app, _rx) = typing_into_a_shell();
            let _daemon = Daemon::connect(&mut app);
            app.panes.terminal_view = TerminalView::Action;
            interactive_focus_effect(&mut app);
            assert_eq!(app.mode, Mode::Browse);
        });
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
