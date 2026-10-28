//! Search keys (/): typed text filters the session list (at most 256 characters); Enter keeps it, Esc clears it.

use deckhand_core::display::strip_terminal_controls;
use deckhand_core::jsstr::js_len;
use deckhand_core::key::Input;

use crate::app::state::{App, Mode};
use crate::keymap::{Binding, HelpRef, KeySpec, Keymap, NamedKey};

/// TS: app.tsx the search query's limit.
pub const MAX_QUERY: usize = 256;

/// search keys.
pub static KEYMAP: Keymap = &[
    Binding {
        keys: &[KeySpec::Named(NamedKey::Escape)],
        action: "clear the search",
        help: HelpRef::Topic("Find & organize"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Return)],
        action: "keep the search",
        help: HelpRef::Topic("Find & organize"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Backspace), KeySpec::Named(NamedKey::Delete)],
        action: "delete the last character",
        help: HelpRef::Hidden("text editing; the footer says enter keeps it, esc clears it"),
    },
];

/// The first `limit` UTF-16 code units of `text`; a character that would be cut in half is dropped (Rust strings
/// cannot hold the lone surrogate TS `slice` would leave).
fn take_units(text: &str, limit: usize) -> String {
    let mut units = 0;
    let mut kept = String::new();
    for c in text.chars() {
        units += c.len_utf16();
        if units > limit {
            break;
        }
        kept.push(c);
    }
    kept
}

/// TS: the search branch of the useTerminalInput handler.
pub fn handle_search(app: &mut App, input: &Input) {
    let key = &input.key;
    if key.escape {
        app.browse.session_query.clear();
        app.set_mode(Mode::Browse);
        return;
    }
    if key.return_ {
        app.set_mode(Mode::Browse);
        return;
    }
    // Not in the TS: ↑/↓, or Ctrl+P / Ctrl+N on the home row, move the selection among the matches while typing.
    let (up, down) = (
        key.up_arrow || (key.ctrl && input.input == "p"),
        key.down_arrow || (key.ctrl && input.input == "n"),
    );
    if up || down {
        crate::app::modes::browse::move_selection(app, if down { 1 } else { -1 });
        return;
    }
    if key.backspace || key.delete {
        // TS `slice(0, -1)` drops one UTF-16 unit (half of an emoji); here the whole character goes.
        app.browse.session_query.pop();
    } else if !key.ctrl && !key.meta {
        let typed = strip_terminal_controls(&input.input);
        let query = format!("{}{typed}", app.browse.session_query);
        app.browse.session_query = if js_len(&query) > MAX_QUERY {
            take_units(&query, MAX_QUERY)
        } else {
            query
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::AppProps;
    use crate::app::test_support::{app, session};
    use deckhand_core::key::Key;
    use deckhand_protocol::data::SessionStatus;

    fn pressed(set: impl FnOnce(&mut Key)) -> Input {
        let mut key = Key::NONE;
        set(&mut key);
        Input::key("", key)
    }

    #[test]
    fn typing_filters_the_list_and_enter_keeps_it() {
        let (mut app, _rx) = app(AppProps::default());
        app.data.repo_sessions = vec![
            session("a", "Alpha", SessionStatus::Running),
            session("b", "Beta", SessionStatus::Running),
        ];
        app.set_mode(Mode::Search);
        handle_search(&mut app, &Input::text("be"));
        handle_search(&mut app, &Input::text("t"));
        assert_eq!(app.browse.session_query, "bet");
        let visible: Vec<String> = app.visible_sessions().into_iter().map(|session| session.id).collect();
        assert_eq!(visible, ["b"]);
        handle_search(&mut app, &pressed(|key| key.return_ = true));
        assert_eq!((app.mode, app.browse.session_query.as_str()), (Mode::Browse, "bet"));
    }

    #[test]
    fn arrows_and_ctrl_n_p_move_the_selection_while_typing() {
        let (mut app, _rx) = app(AppProps::default());
        app.data.repo_sessions = vec![
            session("a", "Alpha one", SessionStatus::Running),
            session("b", "Beta", SessionStatus::Running),
            session("c", "Alpha two", SessionStatus::Running),
        ];
        app.set_mode(Mode::Search);
        handle_search(&mut app, &Input::text("alpha"));
        app.set_selected_id(Some("a".to_owned()));
        handle_search(&mut app, &pressed(|key| key.down_arrow = true));
        assert_eq!(app.browse.selected_id.as_deref(), Some("c"));
        handle_search(
            &mut app,
            &Input::key(
                "p",
                Key {
                    ctrl: true,
                    ..Key::NONE
                },
            ),
        );
        assert_eq!(app.browse.selected_id.as_deref(), Some("a"));
        handle_search(
            &mut app,
            &Input::key(
                "n",
                Key {
                    ctrl: true,
                    ..Key::NONE
                },
            ),
        );
        assert_eq!(app.browse.selected_id.as_deref(), Some("c"));
        assert_eq!((app.mode, app.browse.session_query.as_str()), (Mode::Search, "alpha"));
    }

    #[test]
    fn escape_clears_the_query_and_backspace_trims_it() {
        let (mut app, _rx) = app(AppProps::default());
        app.set_mode(Mode::Search);
        handle_search(&mut app, &Input::text("abc"));
        handle_search(&mut app, &pressed(|key| key.backspace = true));
        assert_eq!(app.browse.session_query, "ab");
        handle_search(&mut app, &pressed(|key| key.delete = true));
        assert_eq!(app.browse.session_query, "a");
        handle_search(&mut app, &pressed(|key| key.backspace = true));
        handle_search(&mut app, &pressed(|key| key.backspace = true));
        assert_eq!(app.browse.session_query, "");
        handle_search(&mut app, &Input::text("zz"));
        handle_search(&mut app, &pressed(|key| key.escape = true));
        assert_eq!((app.mode, app.browse.session_query.as_str()), (Mode::Browse, ""));
    }

    #[test]
    fn control_and_meta_keys_do_not_type_and_terminal_controls_are_stripped() {
        let (mut app, _rx) = app(AppProps::default());
        app.set_mode(Mode::Search);
        handle_search(
            &mut app,
            &Input::key(
                "a",
                Key {
                    ctrl: true,
                    ..Key::NONE
                },
            ),
        );
        handle_search(
            &mut app,
            &Input::key(
                "b",
                Key {
                    meta: true,
                    ..Key::NONE
                },
            ),
        );
        assert_eq!(app.browse.session_query, "");
        handle_search(&mut app, &Input::text("x\u{1b}[31my\u{7}z"));
        assert_eq!(app.browse.session_query, "xyz");
        // Tab and arrows type nothing (their input is empty).
        handle_search(&mut app, &pressed(|key| key.tab = true));
        handle_search(&mut app, &pressed(|key| key.down_arrow = true));
        assert_eq!(app.browse.session_query, "xyz");
        assert_eq!(app.mode, Mode::Search);
    }

    #[test]
    fn the_query_stops_at_256_utf16_units() {
        let (mut app, _rx) = app(AppProps::default());
        app.set_mode(Mode::Search);
        handle_search(&mut app, &Input::text("a".repeat(250)));
        handle_search(&mut app, &Input::text("b".repeat(20)));
        assert_eq!(js_len(&app.browse.session_query), MAX_QUERY);
        assert!(app.browse.session_query.ends_with("bbbbbb") && !app.browse.session_query.ends_with("bbbbbbb"));
        handle_search(&mut app, &Input::text("c"));
        assert_eq!(js_len(&app.browse.session_query), MAX_QUERY);
        // Astral characters count two units; one that would be cut in half is dropped.
        app.browse.session_query = "a".repeat(255);
        handle_search(&mut app, &Input::text("😀"));
        assert_eq!(app.browse.session_query, "a".repeat(255));
        app.browse.session_query = "a".repeat(254);
        handle_search(&mut app, &Input::text("😀"));
        assert_eq!(js_len(&app.browse.session_query), MAX_QUERY);
        handle_search(&mut app, &pressed(|key| key.backspace = true));
        assert_eq!(app.browse.session_query, "a".repeat(254));
    }
}
