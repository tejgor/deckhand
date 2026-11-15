//! `O`'s prompt (enter-idea, not in the TS), one line in the footer with two fields, as the Tasks board's editor has a
//! title and a note: **Name** (optional, short) and **What do you want to do?** (the idea). `tab`/`shift+tab` switch
//! fields; enter starts a new orchestrator for the idea (`open-orchestrator`: titled by the name, else after the idea's
//! first line; the idea its first message; on a new line of its own); Esc cancels. One orchestrator per idea, so
//! every idea typed here starts another.

use deckhand_core::display::strip_terminal_controls;
use deckhand_core::jsstr::js_len;
use deckhand_core::key::Input;
use deckhand_protocol::data::SessionStatus;

use crate::app::state::{App, Mode};
use crate::keymap::{Binding, HelpRef, KeySpec, Keymap, NamedKey};
use crate::primitives::{Row, Seg, TextStyle, row_text, string_width};
use crate::theme::THEME;

/// The longest idea the prompt takes (UTF-16 units), as long as a task's details.
pub const MAX_IDEA: usize = deckhand_core::tasks::MAX_TASK_BODY;

/// The longest name (UTF-16 units): a session title's limit.
pub const MAX_NAME: usize = deckhand_core::plan::MAX_TITLE;

/// The name field's label.
pub const NAME_LABEL: &str = "Name:";

/// What an empty name field says.
pub const NAME_PLACEHOLDER: &str = "(optional, 2–4 words)";

/// What the prompt asks.
pub const QUESTION: &str = "What do you want to do?";

/// Between the two fields.
const SEPARATOR: &str = " · ";

/// The prompt's keys, after its fields.
const KEYS: &str = " • tab next field • enter start • esc cancel";

/// The text cursor, at the end of the field being typed in.
const CURSOR: &str = "▏";

/// The prompt's fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IdeaField {
    /// The orchestrator's name (and its line's). Where the prompt starts.
    #[default]
    Name,
    /// What you want to do: its first message.
    Idea,
}

/// What `O`'s prompt holds; kept after a failed start, so `O` offers it again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IdeaPrompt {
    pub name: String,
    pub idea: String,
    /// The field being typed in.
    pub field: IdeaField,
}

impl IdeaPrompt {
    /// The field being typed in.
    fn text_mut(&mut self) -> &mut String {
        match self.field {
            IdeaField::Name => &mut self.name,
            IdeaField::Idea => &mut self.idea,
        }
    }
}

/// enter-idea keys.
pub static KEYMAP: Keymap = &[
    Binding {
        keys: &[KeySpec::Named(NamedKey::Tab), KeySpec::Named(NamedKey::ShiftTab)],
        action: "switch between the name and the idea",
        help: HelpRef::Topic("Sessions"),
    },
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

/// `O`: the prompt opens in its name field, with what a failed start left in it.
pub fn open_prompt(app: &mut App) {
    if app.client().is_none() {
        app.set_error(Some("still connecting to daemon".to_owned()));
        return;
    }
    app.set_error(None);
    app.browse.idea.field = IdeaField::Name;
    app.set_mode(Mode::EnterIdea);
}

/// The end of `text` that fits in `room` columns, behind `…` when cut (where the typing is).
fn fit_end(text: &str, room: usize) -> String {
    if string_width(text) <= room {
        return text.to_owned();
    }
    let mut kept: Vec<char> = Vec::new();
    let mut used = 1;
    for c in text.chars().rev() {
        let w = string_width(&c.to_string());
        if used + w > room {
            break;
        }
        used += w;
        kept.push(c);
    }
    format!("…{}", kept.into_iter().rev().collect::<String>())
}

/// The start of `text` that fits in `room` columns, ended with `…` when cut.
fn fit_start(text: &str, room: usize) -> String {
    if string_width(text) <= room {
        return text.to_owned();
    }
    let mut kept = String::new();
    let mut used = 1;
    for c in text.chars() {
        let w = string_width(&c.to_string());
        if used + w > room {
            break;
        }
        used += w;
        kept.push(c);
    }
    format!("{kept}…")
}

/// The footer while the prompt is open, `width` wide: the name, the idea and the keys. The field being typed in has
/// the cursor and shows its end when it does not fit; the other shows its start. The name takes what the idea leaves,
/// but at least a third of the room (and 12 columns); the idea the rest.
pub fn prompt_row(prompt: &IdeaPrompt, width: usize) -> Row {
    let on_name = prompt.field == IdeaField::Name;
    let label = |text: &str, active: bool| {
        if active {
            Seg::new(text, TextStyle::fg(THEME.active).bold(true))
        } else {
            Seg::new(text, TextStyle::fg(THEME.muted))
        }
    };
    let value =
        |text: String, active: bool| Seg::new(text, TextStyle::fg(if active { THEME.active } else { THEME.muted }));
    let fixed = string_width(NAME_LABEL)
        + 1
        + string_width(SEPARATOR)
        + string_width(QUESTION)
        + 1
        + string_width(CURSOR)
        + string_width(KEYS)
        + 1;
    let room = width.saturating_sub(fixed);
    let fit = |text: &str, room: usize, active: bool| {
        if active {
            fit_end(text, room)
        } else {
            fit_start(text, room)
        }
    };
    let name_room = (room / 3)
        .max(12)
        .max(room.saturating_sub(string_width(&prompt.idea)))
        .min(room);
    let mut row = vec![label(NAME_LABEL, on_name), Seg::plain(" ")];
    let name_shown = if prompt.name.is_empty() {
        if on_name {
            row.push(value(CURSOR.to_owned(), true));
        }
        let placeholder = fit_start(NAME_PLACEHOLDER, name_room);
        row.push(Seg::new(placeholder.clone(), TextStyle::fg(THEME.muted)));
        placeholder
    } else {
        let shown = fit(&prompt.name, name_room, on_name);
        row.push(value(shown.clone(), on_name));
        if on_name {
            row.push(value(CURSOR.to_owned(), true));
        }
        shown
    };
    row.push(Seg::new(SEPARATOR, TextStyle::fg(THEME.muted)));
    row.push(label(QUESTION, !on_name));
    if !on_name || !prompt.idea.is_empty() {
        row.push(Seg::plain(" "));
        let idea_room = room.saturating_sub(string_width(&name_shown));
        row.push(value(fit(&prompt.idea, idea_room, !on_name), !on_name));
    }
    if !on_name {
        row.push(value(CURSOR.to_owned(), true));
    }
    row.push(Seg::new(KEYS, TextStyle::fg(THEME.hint)));
    row
}

/// [`prompt_row`]'s text.
pub fn prompt_line(prompt: &IdeaPrompt, width: usize) -> String {
    row_text(&prompt_row(prompt, width))
}

/// enter-idea's keys: text goes into the field being typed in (line breaks as spaces); `tab`/`shift+tab` switch
/// fields; `enter` starts it once the idea has text (from the name, with no idea yet, it moves to the idea); `esc`
/// cancels.
pub fn handle_enter_idea(app: &mut App, input: &Input) {
    let key = &input.key;
    if key.escape {
        app.browse.idea = IdeaPrompt::default();
        app.set_mode(Mode::Browse);
        return;
    }
    if key.tab {
        let prompt = &mut app.browse.idea;
        prompt.field = match prompt.field {
            IdeaField::Name => IdeaField::Idea,
            IdeaField::Idea => IdeaField::Name,
        };
        return;
    }
    if key.return_ {
        let idea = app.browse.idea.idea.trim().to_owned();
        if idea.is_empty() {
            app.browse.idea.field = IdeaField::Idea;
            return;
        }
        let name = app.browse.idea.name.trim().to_owned();
        app.set_mode(Mode::Browse);
        start_orchestrator(app, &idea, Some(name.as_str()).filter(|name| !name.is_empty()));
        return;
    }
    if key.backspace || key.delete {
        app.browse.idea.text_mut().pop();
        return;
    }
    if key.ctrl || key.meta {
        return;
    }
    // Typed on with its enter or tab in one read (not a paste, whose line breaks are text): the enter still submits,
    // the tab still switches fields.
    let pasted = input.input.contains("\u{1b}[200~");
    let (text, submit, switch) = match (input.input.strip_suffix('\r'), input.input.strip_suffix('\t')) {
        (Some(text), _) if !pasted => (text, true, false),
        (_, Some(text)) if !pasted => (text, false, true),
        _ => (input.input.as_str(), false, false),
    };
    let typed = strip_terminal_controls(&text.replace("\r\n", " ").replace(['\r', '\n', '\t'], " "));
    let (limit, too_long) = match app.browse.idea.field {
        IdeaField::Name => (MAX_NAME, Some(format!("A name is at most {MAX_NAME} characters"))),
        IdeaField::Idea => (MAX_IDEA, None),
    };
    let field = app.browse.idea.text_mut();
    let mut cut = false;
    for c in typed.chars() {
        if js_len(field) + c.len_utf16() > limit {
            cut = true;
            break;
        }
        field.push(c);
    }
    if cut && let Some(message) = too_long {
        app.set_error(Some(message));
    }
    if submit || switch {
        handle_enter_idea(
            app,
            &Input::key(
                "",
                deckhand_core::key::Key {
                    return_: submit,
                    tab: switch,
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

/// Starts a new orchestrator for `idea`, named `name` (else after the idea), and selects it, as `n` leaves a new
/// session (`enter` attaches once it runs). The prompt's text is kept until it has started, so a failure loses
/// nothing.
pub fn start_orchestrator(app: &mut App, idea: &str, name: Option<&str>) {
    let Some(client) = app.client() else {
        app.set_error(Some("still connecting to daemon".to_owned()));
        return;
    };
    let layout = app.layout();
    let reply = client.open_orchestrator(
        &app.props.cwd,
        idea,
        name,
        i64::from(layout.preview_cols),
        i64::from(layout.preview_rows),
    );
    app.set_error(None);
    app.set_busy(true);
    drop(app.dispatch().spawn(reply, |app: &mut App, result| {
        app.set_busy(false);
        match result {
            Ok(opened) => {
                app.browse.idea = IdeaPrompt::default();
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
