//! `O`'s one-line prompt (enter-idea, not in the TS): "What do you want to do?". Enter starts a new orchestrator for
//! the idea (`open-orchestrator`: titled after its first line, the idea its first message, on a new line of its own);
//! Esc cancels. One orchestrator per idea, so every idea typed here starts another.

use deckhand_core::display::strip_terminal_controls;
use deckhand_core::jsstr::js_len;
use deckhand_core::key::Input;
use deckhand_protocol::data::SessionStatus;

use crate::app::state::{App, Mode};
use crate::keymap::{Binding, HelpRef, KeySpec, Keymap, NamedKey};
use crate::primitives::string_width;

/// The longest idea the prompt takes (UTF-16 units), as long as a task's details.
pub const MAX_IDEA: usize = deckhand_core::tasks::MAX_TASK_BODY;

/// What the prompt asks.
pub const QUESTION: &str = "What do you want to do?";

/// The prompt's keys, after its text.
const KEYS: &str = " • enter start • esc cancel";

/// enter-idea keys.
pub static KEYMAP: Keymap = &[
    Binding {
        keys: &[KeySpec::Named(NamedKey::Return)],
        action: "start an orchestrator for the idea",
        help: HelpRef::Topic("Sessions"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Escape)],
        action: "cancel",
        help: HelpRef::Topic("Sessions"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Backspace), KeySpec::Named(NamedKey::Delete)],
        action: "delete the last character",
        help: HelpRef::Hidden("text editing; the footer says enter starts it, esc cancels"),
    },
];

/// `O`: the prompt opens, with the idea a failed start left in it.
pub fn open_prompt(app: &mut App) {
    if app.client().is_none() {
        app.set_error(Some("still connecting to daemon".to_owned()));
        return;
    }
    app.set_error(None);
    app.set_mode(Mode::EnterIdea);
}

/// The footer while the prompt is open: the question, what is typed (its end, when it does not fit) and the keys.
pub fn prompt_line(idea: &str, width: usize) -> String {
    let head = format!("{QUESTION} ");
    let room = width.saturating_sub(string_width(&head) + string_width(KEYS) + 1);
    let shown = if string_width(idea) <= room {
        idea.to_owned()
    } else {
        // The end, where the typing is, behind `…`.
        let mut kept: Vec<char> = Vec::new();
        let mut used = 1;
        for c in idea.chars().rev() {
            let w = string_width(&c.to_string());
            if used + w > room {
                break;
            }
            used += w;
            kept.push(c);
        }
        format!("…{}", kept.into_iter().rev().collect::<String>())
    };
    format!("{head}{shown}▏{KEYS}")
}

/// enter-idea's keys: text goes into the idea (line breaks as spaces); `enter` starts it, `esc` cancels.
pub fn handle_enter_idea(app: &mut App, input: &Input) {
    let key = &input.key;
    if key.escape {
        app.browse.idea.clear();
        app.set_mode(Mode::Browse);
        return;
    }
    if key.return_ {
        let idea = app.browse.idea.trim().to_owned();
        if !idea.is_empty() {
            app.set_mode(Mode::Browse);
            start_orchestrator(app, &idea);
        }
        return;
    }
    if key.backspace || key.delete {
        app.browse.idea.pop();
        return;
    }
    if key.ctrl || key.meta {
        return;
    }
    // Typed on with its enter in one read (not a paste, whose line breaks are text): the enter still submits.
    let pasted = input.input.contains("\u{1b}[200~");
    let (text, submit) = match input.input.strip_suffix('\r') {
        Some(text) if !pasted => (text, true),
        _ => (input.input.as_str(), false),
    };
    let typed = strip_terminal_controls(&text.replace("\r\n", " ").replace(['\r', '\n', '\t'], " "));
    for c in typed.chars() {
        if js_len(&app.browse.idea) + c.len_utf16() > MAX_IDEA {
            break;
        }
        app.browse.idea.push(c);
    }
    if submit {
        handle_enter_idea(
            app,
            &Input::key(
                "",
                deckhand_core::key::Key {
                    return_: true,
                    ..deckhand_core::key::Key::NONE
                },
            ),
        );
    }
}

/// The status line while a new orchestrator starts.
pub fn starting_text(title: &str) -> String {
    format!("◎ {title} is starting · enter attaches once it runs")
}

/// Starts a new orchestrator for `idea` and selects it, as `n` leaves a new session (`enter` attaches once it runs).
/// The prompt's text is kept until it has started, so a failure loses nothing.
pub fn start_orchestrator(app: &mut App, idea: &str) {
    let Some(client) = app.client() else {
        app.set_error(Some("still connecting to daemon".to_owned()));
        return;
    };
    let layout = app.layout();
    let reply = client.open_orchestrator(
        &app.props.cwd,
        idea,
        i64::from(layout.preview_cols),
        i64::from(layout.preview_rows),
    );
    app.set_error(None);
    app.set_busy(true);
    drop(app.dispatch().spawn(reply, |app: &mut App, result| {
        app.set_busy(false);
        match result {
            Ok(opened) => {
                app.browse.idea.clear();
                let session = opened.session.into_inner();
                app.set_selected_id(Some(session.id.clone()));
                app.upsert(session.clone());
                if session.status != SessionStatus::Exited {
                    app.set_status(Some(starting_text(&session.title)));
                }
            }
            Err(error) => app.set_error(Some(error.to_string())),
        }
    }));
}
