//! Port of app.tsx's filter menu (f): a filter's key picks it, f goes back to active, j/k show each filter as they
//! move, Enter keeps the shown one and Esc restores the filter and selection the menu was opened on.

use deckhand_core::jsstr::js_len;
use deckhand_core::key::Input;
use deckhand_core::session_features::{SESSION_FILTERS, filter_for_key};
use deckhand_protocol::records::SessionFilter;

use crate::app::state::{App, FilterMenuBack, Mode};
use crate::keymap::{Binding, HelpRef, KeySpec, Keymap, NamedKey};

/// pick-filter keys.
pub static KEYMAP: Keymap = &[
    Binding {
        keys: &[
            KeySpec::Char("a"),
            KeySpec::Char("r"),
            KeySpec::Char("!"),
            KeySpec::Char("e"),
            KeySpec::Char("A"),
            KeySpec::Char("*"),
        ],
        action: "pick that filter",
        help: HelpRef::Hidden("the guide's Find & organize line for f lists every filter and its key in its text"),
    },
    Binding {
        keys: &[KeySpec::Char("f")],
        action: "back to the active filter",
        help: HelpRef::Topic("Find & organize"),
    },
    Binding {
        keys: &[
            KeySpec::Char("j"),
            KeySpec::Char("k"),
            KeySpec::Char("g"),
            KeySpec::Char("G"),
            KeySpec::Named(NamedKey::Up),
            KeySpec::Named(NamedKey::Down),
            KeySpec::Named(NamedKey::Home),
            KeySpec::Named(NamedKey::End),
        ],
        action: "show the previous, next, first or last filter",
        help: HelpRef::Hidden("moving through the menu is what its on-screen hints say"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Return)],
        action: "keep the shown filter",
        help: HelpRef::Hidden("the menu's own hint says enter keeps it"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Escape)],
        action: "restore the filter and selection the menu was opened on",
        help: HelpRef::Hidden("the menu's own hint says esc cancels"),
    },
];

/// TS: f in browse (`fr` / `ff` can arrive as one chunk when typed fast: the second key picks in the menu).
pub fn open_filter_menu(app: &mut App, input: &Input) {
    let back = FilterMenuBack {
        filter: app.browse.session_filter,
        selected_id: app.browse.selected_id.clone(),
    };
    app.filter = Some(back.clone());
    app.set_mode(Mode::PickFilter);
    // TS `input.length === 2 && input[0] === 'f'`: `input[1]` is then one UTF-16 unit, so one BMP character.
    if js_len(&input.input) == 2
        && let Some(second) = input.input.strip_prefix('f')
    {
        let picked = Input::key(second, input.key);
        pick_filter(app, &picked, Some(&back));
    }
}

/// TS `pickFilter`: one key in the menu (`back`: what Esc restores).
pub fn pick_filter(app: &mut App, input: &Input, back: Option<&FilterMenuBack>) {
    let key = &input.key;
    let text = input.input.as_str();
    // TS `close(filter?)`.
    let close = |app: &mut App, filter: Option<SessionFilter>| {
        if let Some(filter) = filter {
            app.browse.session_filter = filter;
        }
        app.filter = None;
        app.set_mode(Mode::Browse);
    };
    if key.escape {
        if let Some(back) = back {
            app.browse.session_filter = back.filter;
            app.set_selected_id(back.selected_id.clone());
        }
        close(app, None);
        return;
    }
    if key.return_ {
        close(app, None);
        return;
    }
    if text == "f" {
        close(app, Some(SessionFilter::Active));
        return;
    }
    if let Some(picked) = filter_for_key(text) {
        close(app, Some(picked));
        return;
    }
    let last = SESSION_FILTERS.len() - 1;
    let index = SESSION_FILTERS
        .iter()
        .position(|filter| *filter == app.browse.session_filter)
        .unwrap_or(0);
    let next = if text == "g" || key.home {
        0
    } else if text == "G" || key.end {
        last
    } else if text == "j" || key.down_arrow {
        last.min(index + 1)
    } else if text == "k" || key.up_arrow {
        index.saturating_sub(1)
    } else {
        index
    };
    app.browse.session_filter = SESSION_FILTERS[next];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::AppProps;
    use crate::app::test_support::{app, session};
    use deckhand_core::key::Key;
    use deckhand_protocol::data::SessionStatus;

    fn key(f: impl FnOnce(&mut Key)) -> Input {
        let mut key = Key::NONE;
        f(&mut key);
        Input::key("", key)
    }

    fn open(app: &mut App) {
        open_filter_menu(app, &Input::text("f"));
    }

    #[test]
    fn j_and_k_show_each_filter_and_stop_at_the_ends() {
        let (mut app, _rx) = app(AppProps::default());
        open(&mut app);
        assert_eq!(app.mode, Mode::PickFilter);
        let back = app.filter.clone();
        for expected in [
            SessionFilter::Running,
            SessionFilter::Attention,
            SessionFilter::Exited,
            SessionFilter::Archived,
            SessionFilter::All,
            SessionFilter::All,
        ] {
            pick_filter(&mut app, &Input::text("j"), back.as_ref());
            assert_eq!(app.browse.session_filter, expected);
        }
        pick_filter(&mut app, &Input::text("k"), back.as_ref());
        assert_eq!(app.browse.session_filter, SessionFilter::Archived);
        pick_filter(&mut app, &key(|key| key.home = true), back.as_ref());
        assert_eq!(app.browse.session_filter, SessionFilter::Active);
        pick_filter(&mut app, &key(|key| key.up_arrow = true), back.as_ref());
        assert_eq!(app.browse.session_filter, SessionFilter::Active);
        pick_filter(&mut app, &Input::text("G"), back.as_ref());
        assert_eq!(app.browse.session_filter, SessionFilter::All);
        pick_filter(&mut app, &Input::text("g"), back.as_ref());
        assert_eq!(app.browse.session_filter, SessionFilter::Active);
        pick_filter(&mut app, &key(|key| key.down_arrow = true), back.as_ref());
        assert_eq!(app.browse.session_filter, SessionFilter::Running);
        // Other keys leave the shown filter, and the menu open.
        pick_filter(&mut app, &Input::text("z"), back.as_ref());
        assert_eq!(app.browse.session_filter, SessionFilter::Running);
        assert_eq!(app.mode, Mode::PickFilter);
    }

    #[test]
    fn a_filters_key_picks_it_and_f_goes_back_to_active() {
        let (mut app, _rx) = app(AppProps::default());
        open(&mut app);
        let back = app.filter.clone();
        pick_filter(&mut app, &Input::text("A"), back.as_ref());
        assert_eq!(app.browse.session_filter, SessionFilter::Archived);
        assert_eq!((app.mode, app.filter.clone()), (Mode::Browse, None));
        open(&mut app);
        assert_eq!(
            app.filter,
            Some(FilterMenuBack {
                filter: SessionFilter::Archived,
                selected_id: None
            })
        );
        let back = app.filter.clone();
        pick_filter(&mut app, &Input::text("f"), back.as_ref());
        assert_eq!(app.browse.session_filter, SessionFilter::Active);
        assert_eq!(app.mode, Mode::Browse);
        for (text, filter) in [
            ("a", SessionFilter::Active),
            ("r", SessionFilter::Running),
            ("!", SessionFilter::Attention),
            ("e", SessionFilter::Exited),
            ("*", SessionFilter::All),
        ] {
            open(&mut app);
            pick_filter(&mut app, &Input::text(text), None);
            assert_eq!(app.browse.session_filter, filter, "{text}");
            assert_eq!(app.mode, Mode::Browse);
        }
    }

    #[test]
    fn enter_keeps_the_shown_filter_and_escape_restores_the_opening_state() {
        let (mut app, _rx) = app(AppProps::default());
        app.data.repo_sessions = vec![session("a", "Alpha", SessionStatus::Running)];
        app.set_selected_id(Some("a".to_owned()));
        app.browse.session_filter = SessionFilter::Running;
        open(&mut app);
        let back = app.filter.clone();
        pick_filter(&mut app, &Input::text("j"), back.as_ref());
        app.set_selected_id(Some("other".to_owned()));
        pick_filter(&mut app, &key(|key| key.escape = true), back.as_ref());
        assert_eq!(app.browse.session_filter, SessionFilter::Running);
        assert_eq!(app.browse.selected_id.as_deref(), Some("a"));
        assert_eq!((app.mode, app.filter.clone()), (Mode::Browse, None));
        open(&mut app);
        let back = app.filter.clone();
        pick_filter(&mut app, &Input::text("j"), back.as_ref());
        pick_filter(&mut app, &key(|key| key.return_ = true), back.as_ref());
        assert_eq!(app.browse.session_filter, SessionFilter::Attention);
        assert_eq!((app.mode, app.filter.clone()), (Mode::Browse, None));
        // Escape without a recorded way back only closes.
        open(&mut app);
        pick_filter(&mut app, &Input::text("j"), None);
        pick_filter(&mut app, &key(|key| key.escape = true), None);
        assert_eq!(app.browse.session_filter, SessionFilter::Exited);
        assert_eq!(app.mode, Mode::Browse);
    }

    #[test]
    fn fast_typed_pairs_pick_in_the_menu_at_once() {
        let (mut app, _rx) = app(AppProps::default());
        open_filter_menu(&mut app, &Input::text("fr"));
        assert_eq!(app.browse.session_filter, SessionFilter::Running);
        assert_eq!((app.mode, app.filter.clone()), (Mode::Browse, None));
        open_filter_menu(&mut app, &Input::text("ff"));
        assert_eq!(app.browse.session_filter, SessionFilter::Active);
        assert_eq!(app.mode, Mode::Browse);
        // A second key that picks nothing leaves the menu open; its j/k move from the current filter.
        open_filter_menu(&mut app, &Input::text("fj"));
        assert_eq!(app.mode, Mode::PickFilter);
        assert_eq!(app.browse.session_filter, SessionFilter::Running);
        assert_eq!(app.filter.map(|back| back.filter), Some(SessionFilter::Active));
    }
}
