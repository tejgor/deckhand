//! `O`'s dialog (enter-idea, not in the TS), **New orchestrator**, drawn over the faded session pane as the other
//! dialogs are. Two fields, as the Tasks board's editor has a title and a note: **Name** (optional, one short line)
//! and **What do you want to do?** (the idea: several lines, wrapped). `tab`/`shift+tab` switch fields; `enter` in the
//! name goes on to the idea, and in the idea starts a new line, as in a note; `ctrl+s` starts a new orchestrator for
//! the idea (`open-orchestrator`: titled by the name, else after the idea's first line; the idea its first message;
//! on a new line of its own); `esc` closes it and keeps what was typed for the next `O`. One orchestrator per idea, so
//! every idea started here starts another.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_core::display::{HINT_SEPARATOR, HintPart, fit_hint};
use deckhand_core::jsstr::js_len;
use deckhand_core::key::Input;
use deckhand_core::text_editor::{
    EditOptions, EditorLine, EditorState, clean_inserted_text, edit_text, replace_range, wrapped_editor_lines,
};
use deckhand_protocol::data::SessionStatus;

use crate::app::state::{App, Mode};
use crate::keymap::{Binding, HelpRef, KeySpec, Keymap, NamedKey};
use crate::primitives::{
    Border, DialogRows, DialogView, Row, Seg, TextStyle, Wrap, dialog_max_height, dialog_width, draw_dialog, truncate,
    truncate_row,
};
use crate::theme::THEME;

/// The longest idea the dialog takes (UTF-16 units), as long as a task's details.
pub const MAX_IDEA: usize = deckhand_core::tasks::MAX_TASK_BODY;

/// The longest name (UTF-16 units): a session title's limit.
pub const MAX_NAME: usize = deckhand_core::plan::MAX_TITLE;

/// The dialog's title.
pub const TITLE: &str = "New orchestrator";

/// The name field's label.
pub const NAME_LABEL: &str = "Name";

/// What an empty name field says.
pub const NAME_PLACEHOLDER: &str = "optional · 2–4 words";

/// The idea field's label: what the dialog asks.
pub const QUESTION: &str = "What do you want to do?";

/// What an empty idea says.
pub const IDEA_PLACEHOLDER: &str = "a sentence or a paragraph: the orchestrator's first message";

/// What `ctrl+s` with no idea says.
pub const BLANK_IDEA: &str = "Say what you want to do first";

/// The dialog's width at most, border included (a paragraph reads well at about 75 columns).
const DIALOG_WIDTH: u16 = 80;

/// The idea's rows at most (fewer when the screen is short); a longer idea scrolls with its cursor.
const IDEA_ROWS: usize = 8;

/// The name's label column: the label and the room after it.
const NAME_COLUMN: usize = 6;

/// The idea's wrap width before the dialog was first drawn.
const DEFAULT_IDEA_WIDTH: usize = 40;

/// The dialog's fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IdeaField {
    /// The orchestrator's name (and its line's). Where the dialog starts.
    #[default]
    Name,
    /// What you want to do: its first message.
    Idea,
}

/// What `O`'s dialog holds: kept when it is closed with `esc` or a start fails, so `O` offers it again; cleared once
/// an orchestrator starts from it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IdeaPrompt {
    pub name: EditorState,
    pub idea: EditorState,
    /// The field being typed in.
    pub field: IdeaField,
    /// Shown above the keys: why the last key did nothing (no idea, a name too long) or why the start failed.
    pub message: Option<String>,
    /// Inside a bracketed paste (`[200~` … `[201~`): `enter` and `tab` are text.
    pub pasting: bool,
    /// The idea's wrap width as last drawn (`↑`/`↓` move by its rows).
    pub idea_width: usize,
}

impl IdeaPrompt {
    /// A prompt holding `name` and `idea`, the cursor at their ends, typing in `field`.
    pub fn with(name: &str, idea: &str, field: IdeaField) -> Self {
        let state = |text: &str| EditorState {
            text: text.to_owned(),
            cursor: text.len(),
            ..EditorState::default()
        };
        IdeaPrompt {
            name: state(name),
            idea: state(idea),
            field,
            ..IdeaPrompt::default()
        }
    }

    /// Whether neither field has anything in it.
    pub fn is_empty(&self) -> bool {
        self.name.text.is_empty() && self.idea.text.is_empty()
    }

    /// The field being typed in.
    fn state_mut(&mut self) -> &mut EditorState {
        match self.field {
            IdeaField::Name => &mut self.name,
            IdeaField::Idea => &mut self.idea,
        }
    }

    /// The other field.
    fn switch(&mut self) {
        self.field = match self.field {
            IdeaField::Name => IdeaField::Idea,
            IdeaField::Idea => IdeaField::Name,
        };
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
        action: "from the name: go to the idea; in the idea: a new line",
        help: HelpRef::Topic("Sessions"),
    },
    Binding {
        keys: &[KeySpec::Ctrl("s")],
        action: "start an orchestrator for the idea",
        help: HelpRef::Topic("Sessions"),
    },
    Binding {
        keys: &[KeySpec::Named(NamedKey::Escape)],
        action: "close (what was typed is kept for the next O)",
        help: HelpRef::Topic("Sessions"),
    },
    Binding {
        keys: &[
            KeySpec::Named(NamedKey::Backspace),
            KeySpec::Named(NamedKey::Delete),
            KeySpec::Named(NamedKey::Left),
            KeySpec::Named(NamedKey::Right),
            KeySpec::Named(NamedKey::Up),
            KeySpec::Named(NamedKey::Down),
        ],
        action: "edit the field",
        help: HelpRef::Hidden("text editing; the dialog's footer says how to start it and close it"),
    },
];

/// `O`: the dialog opens with what was left in it (where it was left), else in an empty name.
pub fn open_prompt(app: &mut App) {
    if app.client().is_none() {
        app.set_error(Some("still connecting to daemon".to_owned()));
        return;
    }
    app.set_error(None);
    let prompt = &mut app.browse.idea;
    prompt.pasting = false;
    if prompt.is_empty() {
        prompt.field = IdeaField::Name;
    }
    app.set_mode(Mode::EnterIdea);
}

/// The name's editing limits.
fn name_options() -> EditOptions {
    EditOptions {
        select_all: Some(false),
        max_chars: Some(MAX_NAME),
        limit_message: Some(format!("A name is at most {MAX_NAME} characters")),
        tab: Some(false),
        wrap_width: None,
        page_rows: None,
    }
}

/// The idea's editing limits, `width` its wrap width.
fn idea_options(width: usize) -> EditOptions {
    EditOptions {
        select_all: Some(false),
        max_chars: Some(MAX_IDEA),
        limit_message: Some(format!("An idea is at most {MAX_IDEA} characters")),
        tab: Some(false),
        wrap_width: Some(width),
        page_rows: Some(IDEA_ROWS),
    }
}

/// Inserts `text` (typed or pasted) at the field's cursor: in the name on one line (line breaks and tabs as spaces),
/// in the idea with its line breaks (tabs as two spaces). What does not fit the field's limit is cut, and the
/// dialog says so.
fn insert(prompt: &mut IdeaPrompt, text: &str) {
    let cleaned = clean_inserted_text(text);
    let (typed, options) = match prompt.field {
        IdeaField::Name => (cleaned.replace(['\n', '\t'], " "), name_options()),
        IdeaField::Idea => (cleaned.replace('\t', "  "), idea_options(prompt.idea_width.max(1))),
    };
    let limit = options.max_chars.unwrap_or(usize::MAX);
    let state = prompt.state_mut();
    let room = limit.saturating_sub(js_len(&state.text));
    let mut kept = String::new();
    let mut used = 0;
    let mut cut = false;
    for c in typed.chars() {
        if used + c.len_utf16() > room {
            cut = true;
            break;
        }
        used += c.len_utf16();
        kept.push(c);
    }
    if !kept.is_empty() {
        *state = replace_range(state, state.cursor, state.cursor, &kept, &options);
    }
    if cut {
        prompt.message = options.limit_message;
    }
}

/// enter-idea's keys: text goes into the field being typed in, at its cursor; `tab`/`shift+tab` switch fields;
/// `enter` goes from the name to the idea, and in the idea starts a new line; `ctrl+s` starts it once the idea has
/// text; `esc` closes it, keeping what was typed. In a bracketed paste, `enter` and `tab` are text.
pub fn handle_enter_idea(app: &mut App, input: &Input) {
    let key = &input.key;
    if input.input == "[200~" {
        app.browse.idea.pasting = true;
        return;
    }
    if input.input == "[201~" {
        app.browse.idea.pasting = false;
        return;
    }
    let prompt = &mut app.browse.idea;
    prompt.message = None;
    if prompt.pasting {
        if key.return_ {
            insert(prompt, "\n");
        } else if key.tab {
            insert(prompt, "\t");
        } else if !key.escape && !input.input.is_empty() {
            insert(prompt, &input.input);
        }
        return;
    }
    if key.escape {
        app.set_mode(Mode::Browse);
        return;
    }
    if key.ctrl && input.input == "s" {
        start(app);
        return;
    }
    if key.tab {
        prompt.switch();
        return;
    }
    if key.return_ && prompt.field == IdeaField::Name {
        prompt.field = IdeaField::Idea;
        return;
    }
    let special = key.return_
        || key.backspace
        || key.delete
        || key.left_arrow
        || key.right_arrow
        || key.up_arrow
        || key.down_arrow
        || key.home
        || key.end
        || key.page_up
        || key.page_down
        || key.ctrl
        || key.meta;
    if special {
        if prompt.field == IdeaField::Name && (key.up_arrow || key.down_arrow || key.page_up || key.page_down) {
            return;
        }
        let options = match prompt.field {
            IdeaField::Name => name_options(),
            IdeaField::Idea => idea_options(width_or_default(prompt.idea_width)),
        };
        let typed = if key.return_ { "" } else { input.input.as_str() };
        let state = prompt.state_mut();
        let next = edit_text(state, typed, key, &options);
        prompt.message = next.message.clone().filter(|message| !message.is_empty());
        *prompt.state_mut() = EditorState { message: None, ..next };
        return;
    }
    if input.input.is_empty() {
        return;
    }
    // Keys typed faster than they are read arrive as one text (a paste comes bracketed, above): its enters, tabs and
    // ctrl+s still act as those keys.
    let mut text = String::new();
    let mut chars = input.input.chars().peekable();
    while let Some(c) = chars.next() {
        if !matches!(c, '\r' | '\t' | '\u{13}') {
            text.push(c);
            continue;
        }
        let prompt = &mut app.browse.idea;
        insert(prompt, &std::mem::take(&mut text));
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                match prompt.field {
                    IdeaField::Name => prompt.field = IdeaField::Idea,
                    IdeaField::Idea => insert(prompt, "\n"),
                }
            }
            '\t' => prompt.switch(),
            _ => {
                start(app);
                return;
            }
        }
    }
    insert(&mut app.browse.idea, &text);
}

/// The idea's wrap width, or a guess before the dialog was drawn.
fn width_or_default(width: usize) -> usize {
    if width == 0 { DEFAULT_IDEA_WIDTH } else { width }
}

/// `ctrl+s`: starts an orchestrator for the idea (trimmed, its line breaks kept), named by the name when it has
/// text; with no idea, says so and goes to it.
fn start(app: &mut App) {
    let prompt = &mut app.browse.idea;
    let idea = prompt.idea.text.trim().to_owned();
    if idea.is_empty() {
        prompt.message = Some(BLANK_IDEA.to_owned());
        prompt.field = IdeaField::Idea;
        return;
    }
    let name = prompt.name.text.trim().to_owned();
    app.set_mode(Mode::Browse);
    start_orchestrator(app, &idea, Some(name.as_str()).filter(|name| !name.is_empty()));
}

/// The rows of the dialog's layout over `bounds`: roomy (a blank row between the parts) when the screen has the
/// height, else compact; and how many rows the idea shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Shape {
    roomy: bool,
    idea_rows: usize,
}

/// The rows a roomy dialog has besides the idea's: the title and a blank row, the name, a blank row, the question,
/// the message row and the keys.
const ROOMY_ROWS: usize = 7;

/// A compact dialog's other rows: the title, the name, the question and the keys (a message takes the keys' row).
const COMPACT_ROWS: usize = 4;

/// The dialog's shape over `bounds`, from the most rows it can have (a blank margin above and below in a roomy one).
fn shape(bounds: Rect) -> Shape {
    let inner = usize::from(dialog_max_height(bounds)).saturating_sub(2);
    if inner >= ROOMY_ROWS + 2 + 3 {
        Shape {
            roomy: true,
            idea_rows: (inner - ROOMY_ROWS - 2).min(IDEA_ROWS),
        }
    } else {
        Shape {
            roomy: false,
            idea_rows: inner.saturating_sub(COMPACT_ROWS).clamp(1, IDEA_ROWS),
        }
    }
}

/// A label, bold in the active colour on the field being typed in.
fn label(text: &str, active: bool) -> Seg {
    if active {
        Seg::new(text, TextStyle::fg(THEME.active).bold(true))
    } else {
        Seg::new(text, TextStyle::fg(THEME.muted))
    }
}

/// An editor row, the cursor's character inverted when `active` (as in the Tasks board's editor).
fn cursor_of(line: &EditorLine, active: bool) -> Vec<Seg> {
    let mut segs = vec![Seg::plain(line.before.clone())];
    match (&line.cursor, active) {
        (Some(cursor), true) => segs.push(Seg::new(cursor.clone(), TextStyle::default().inverse(true))),
        (cursor, _) => segs.push(Seg::plain(cursor.clone().unwrap_or_default())),
    }
    segs.push(Seg::plain(line.after.clone()));
    segs
}

/// An empty field's muted placeholder, the cursor on its first character when `active`.
fn placeholder(text: &str, active: bool) -> Vec<Seg> {
    let muted = TextStyle::fg(THEME.muted);
    let mut chars = text.chars();
    match chars.next() {
        Some(first) if active => vec![
            Seg::new(first.to_string(), muted.inverse(true)),
            Seg::new(chars.as_str(), muted),
        ],
        _ => vec![Seg::new(text, muted)],
    }
}

/// The idea's wrap width in a dialog `width` wide (border included): inside the padding and the indent.
fn idea_width_of(width: u16) -> usize {
    usize::from(width.min(DIALOG_WIDTH)).saturating_sub(6).max(4)
}

/// The dialog's rows at `width` (border included; laid out at most [`DIALOG_WIDTH`] wide, so it never grows as the
/// idea does): the title; the name on one row (the end being typed shows); the question and the idea's rows, scrolled
/// to its cursor; the message, if any, and the keys.
fn dialog_rows(prompt: &IdeaPrompt, shape: Shape, width: u16) -> DialogRows {
    let content = usize::from(width.min(DIALOG_WIDTH)).saturating_sub(4).max(1);
    let fit = |segs: Row| truncate_row(&segs, content, Wrap::TruncateEnd);
    let on_name = prompt.field == IdeaField::Name;
    let mut header = vec![fit(vec![Seg::new(TITLE, TextStyle::fg(THEME.accent).bold(true))])];
    if shape.roomy {
        header.push(Vec::new());
    }
    let name_width = content.saturating_sub(NAME_COLUMN).max(4);
    let mut name = vec![
        label(NAME_LABEL, on_name),
        Seg::plain(" ".repeat(NAME_COLUMN - NAME_LABEL.len())),
    ];
    if prompt.name.text.is_empty() {
        name.extend(placeholder(NAME_PLACEHOLDER, on_name));
    } else if on_name {
        let rows = wrapped_editor_lines(&prompt.name, name_width.saturating_sub(1).max(1));
        if let Some(line) = rows.lines.get(rows.cursor_row) {
            name.extend(cursor_of(line, true));
        }
    } else {
        name.push(Seg::plain(truncate(&prompt.name.text, name_width, Wrap::TruncateEnd)));
    }
    let mut body = vec![fit(name)];
    if shape.roomy {
        body.push(Vec::new());
    }
    body.push(fit(vec![label(QUESTION, !on_name)]));
    let idea = wrapped_editor_lines(&prompt.idea, idea_width_of(width));
    let start = (prompt_signed(idea.cursor_row) - prompt_signed(shape.idea_rows) + 1)
        .min(prompt_signed(idea.lines.len()) - prompt_signed(shape.idea_rows))
        .max(0);
    let start = usize::try_from(start).unwrap_or(0);
    for index in 0..shape.idea_rows {
        let at = start + index;
        let mut segs = vec![Seg::plain("  ")];
        if prompt.idea.text.is_empty() {
            if at == 0 {
                segs.extend(placeholder(IDEA_PLACEHOLDER, !on_name));
            }
        } else if let Some(line) = idea.lines.get(at) {
            segs.extend(cursor_of(line, !on_name && at == idea.cursor_row));
        }
        body.push(fit(segs));
    }
    // Narrow, the field keys go first: starting and cancelling stay in sight.
    let part = |text: &str, short: &str, drop: i32| HintPart {
        text: text.to_owned(),
        short: Some(short.to_owned()),
        drop: Some(drop),
    };
    let keys: Vec<HintPart> = if on_name {
        vec![
            part("enter/tab next field", "tab next", 1),
            "ctrl+s start".into(),
            "esc cancel".into(),
        ]
    } else {
        vec![
            part("enter new line", "enter new line", 1),
            part("tab next field", "tab next", 2),
            "ctrl+s start".into(),
            "esc cancel".into(),
        ]
    };
    let keys = fit(vec![Seg::new(
        fit_hint(&keys, content, HINT_SEPARATOR),
        TextStyle::fg(THEME.muted),
    )]);
    let message = prompt
        .message
        .as_deref()
        .map(|message| fit(vec![Seg::new(message, TextStyle::fg(THEME.error))]));
    let footer = match (shape.roomy, message) {
        (true, message) => vec![message.unwrap_or_default(), keys],
        (false, Some(message)) => vec![message],
        (false, None) => vec![keys],
    };
    DialogRows { header, body, footer }
}

/// `value` as a signed row count.
fn prompt_signed(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Draws the dialog over `bounds` (the session pane, faded behind it): at most [`DIALOG_WIDTH`] wide and as tall as
/// its shape, the same size whatever is typed. Returns the idea's wrap width, for `↑`/`↓`.
pub fn render_idea_dialog(prompt: &IdeaPrompt, buf: &mut Buffer, bounds: Rect) -> usize {
    let shape = shape(bounds);
    let width = DIALOG_WIDTH.min(dialog_width(bounds));
    let build = |width: u16| dialog_rows(prompt, shape, width);
    let view = DialogView {
        scroll: 0,
        floor_width: width,
        floor_height: 0,
    };
    let drawn = draw_dialog(buf, bounds, &Border::round(THEME.border_active), &build, view);
    idea_width_of(drawn.width)
}

/// The status line while a new orchestrator starts.
pub fn starting_text(title: &str) -> String {
    format!("◎ {title} is starting · enter attaches once it runs")
}

/// Starts a new orchestrator for `idea`, named `name` (else after the idea), and selects it, as `n` leaves a new
/// session (`enter` attaches once it runs). The dialog's text is kept until it has started, so a failure loses
/// nothing: `O` offers it again, saying why it failed.
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
            Err(error) => {
                app.browse.idea.message = Some(format!("Not started: {error}"));
                app.set_error(Some(error.to_string()));
            }
        }
    }));
}

#[cfg(test)]
mod tests {
    use deckhand_core::key::Key;
    use deckhand_protocol::data::{SessionRecord, SessionRole};
    use serde_json::json;

    use super::*;
    use crate::app::modes::browse::tests::{Daemon, apply, local, named, text, with_sessions};
    use crate::app::test_support::session;
    use crate::primitives::row_text;

    fn key(apply: impl FnOnce(&mut Key)) -> Input {
        let mut key = Key::NONE;
        apply(&mut key);
        named(key)
    }

    fn enter() -> Input {
        key(|key| key.return_ = true)
    }

    fn tab(shift: bool) -> Input {
        key(|key| {
            key.tab = true;
            key.shift = shift;
        })
    }

    fn esc() -> Input {
        key(|key| {
            key.escape = true;
            key.meta = true;
        })
    }

    fn ctrl_s() -> Input {
        Input::key(
            "s",
            Key {
                ctrl: true,
                ..Key::NONE
            },
        )
    }

    /// What the dialog holds: its name, its idea and the field typed in.
    fn fields(app: &App) -> (&str, &str, IdeaField) {
        let prompt = &app.browse.idea;
        (&prompt.name.text, &prompt.idea.text, prompt.field)
    }

    fn orchestrator(id: &str, title: &str, status: SessionStatus) -> SessionRecord {
        let mut orchestrator = session(id, title, status);
        orchestrator.role = Some(SessionRole::Orchestrator);
        orchestrator
    }

    /// `O`, then `name`, `tab`, `idea` typed, then `ctrl+s`.
    fn submit_idea(app: &mut App, name: &str, idea: &str) {
        app.handle_input(&text("O"));
        assert_eq!(app.mode, Mode::EnterIdea);
        app.handle_input(&text(name));
        app.handle_input(&tab(false));
        app.handle_input(&text(idea));
        app.handle_input(&ctrl_s());
    }

    /// The dialog drawn over `bounds`, and its rows.
    fn drawn(prompt: &IdeaPrompt, bounds: Rect) -> (Buffer, Vec<String>) {
        let mut buf = Buffer::empty(bounds);
        render_idea_dialog(prompt, &mut buf, bounds);
        let rows = (bounds.top()..bounds.bottom())
            .map(|y| {
                (bounds.left()..bounds.right())
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect();
        (buf, rows)
    }

    /// The characters drawn inverted (the cursor).
    fn inverse(buf: &Buffer) -> Vec<String> {
        let area = buf.area;
        (area.top()..area.bottom())
            .flat_map(|y| (area.left()..area.right()).map(move |x| (x, y)))
            .filter(|&(x, y)| buf[(x, y)].modifier.contains(ratatui::style::Modifier::REVERSED))
            .map(|(x, y)| buf[(x, y)].symbol().to_owned())
            .collect()
    }

    #[test]
    fn o_opens_the_dialog_in_the_name_and_ctrl_s_starts_a_new_orchestrator_for_the_idea() {
        local(async {
            let (mut app, mut rx) = with_sessions(1);
            let mut daemon = Daemon::connect(&mut app);
            // One already runs: O asks for a new idea all the same (one orchestrator per idea).
            app.data
                .repo_sessions
                .push(orchestrator("first", "Rework auth", SessionStatus::Running));
            app.handle_input(&text("O"));
            assert_eq!(app.mode, Mode::EnterIdea);
            assert!(app.mode.is_dialog());
            assert_eq!(fields(&app), ("", "", IdeaField::Name));
            app.handle_input(&text("Build speed"));
            // Enter in the name goes on to the idea.
            app.handle_input(&enter());
            assert_eq!(fields(&app), ("Build speed", "", IdeaField::Idea));
            app.handle_input(&text("Speed up the build."));
            // Enter in the idea is a new line, as in a note.
            app.handle_input(&enter());
            app.handle_input(&text("Cache the deps"));
            app.handle_input(&key(|key| key.backspace = true));
            app.handle_input(&text("s"));
            assert_eq!(
                fields(&app),
                ("Build speed", "Speed up the build.\nCache the deps", IdeaField::Idea)
            );
            app.handle_input(&ctrl_s());
            assert_eq!(app.mode, Mode::Browse);
            assert!(app.messages.busy);
            let request = daemon.next().await;
            assert_eq!(request["type"], "open-orchestrator");
            assert_eq!(request["cwd"], "/repo");
            assert_eq!(request["prompt"], "Speed up the build.\nCache the deps");
            assert_eq!(request["title"], "Build speed");
            let layout = app.layout();
            assert_eq!(request["cols"], layout.preview_cols);
            assert_eq!(request["rows"], layout.preview_rows);
            let started = serde_json::to_value(orchestrator("second", "Build speed", SessionStatus::Starting))
                .unwrap_or_else(|error| panic!("{error}"));
            daemon
                .ok(&request, json!({"session": started, "lineId": "ab12cd34"}))
                .await;
            apply(&mut app, &mut rx).await;
            assert!(!app.messages.busy);
            assert_eq!(app.browse.selected_id.as_deref(), Some("second"));
            assert!(app.data.repo_sessions.iter().any(|session| session.id == "second"));
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("◎ Build speed is starting · enter attaches once it runs")
            );
            // Started: the next O is empty again.
            assert_eq!(app.browse.idea, IdeaPrompt::default());
        });
    }

    #[test]
    fn tab_and_shift_tab_switch_fields_and_a_blank_name_sends_no_title() {
        local(async {
            let (mut app, _rx) = with_sessions(1);
            let mut daemon = Daemon::connect(&mut app);
            app.handle_input(&text("O"));
            app.handle_input(&tab(false));
            assert_eq!(app.browse.idea.field, IdeaField::Idea);
            app.handle_input(&text("Store session tokens encrypted"));
            app.handle_input(&tab(true));
            assert_eq!(app.browse.idea.field, IdeaField::Name);
            app.handle_input(&text("  "));
            // A tab read with the typing still switches; an enter read with it in the name moves to the idea.
            app.handle_input(&text(" \t"));
            assert_eq!(app.browse.idea.field, IdeaField::Idea);
            app.handle_input(&tab(true));
            app.handle_input(&text("\r"));
            assert_eq!(fields(&app), ("   ", "Store session tokens encrypted", IdeaField::Idea));
            // ctrl+s starts it from either field; the name has no text, so the daemon names it after the idea.
            app.handle_input(&tab(false));
            app.handle_input(&ctrl_s());
            assert_eq!(app.mode, Mode::Browse);
            let request = daemon.next().await;
            assert_eq!(request["prompt"], "Store session tokens encrypted");
            assert!(request.get("title").is_none(), "{request}");
        });
    }

    #[test]
    fn line_breaks_typed_or_pasted_stay_in_the_idea_and_become_spaces_in_the_name() {
        local(async {
            let (mut app, _rx) = with_sessions(1);
            let _daemon = Daemon::connect(&mut app);
            app.handle_input(&text("O"));
            // A bracketed paste: enter and tab inside it are text, and esc does not close the dialog.
            app.handle_input(&text("[200~"));
            app.handle_input(&text("Auth\r\ntokens"));
            app.handle_input(&enter());
            app.handle_input(&text("now"));
            app.handle_input(&text("[201~"));
            assert_eq!(fields(&app), ("Auth tokens now", "", IdeaField::Name));
            app.handle_input(&tab(false));
            app.handle_input(&text("[200~"));
            app.handle_input(&text("First line\rsecond line"));
            app.handle_input(&enter());
            app.handle_input(&tab(false));
            app.handle_input(&esc());
            app.handle_input(&text("indented\u{1b}[31m"));
            app.handle_input(&text("[201~"));
            assert_eq!(app.mode, Mode::EnterIdea);
            assert_eq!(
                fields(&app),
                (
                    "Auth tokens now",
                    "First line\nsecond line\n  indented",
                    IdeaField::Idea
                )
            );
            // After the paste, tab switches fields again.
            app.handle_input(&tab(false));
            assert_eq!(app.browse.idea.field, IdeaField::Name);
            // Typed with its enter in one read, in the idea: a new line.
            app.handle_input(&tab(false));
            app.handle_input(&text(" done\r"));
            assert_eq!(app.browse.idea.idea.text, "First line\nsecond line\n  indented done\n");
            assert_eq!(app.mode, Mode::EnterIdea);
        });
    }

    #[test]
    fn keys_typed_faster_than_they_are_read_still_act_as_keys() {
        local(async {
            let (mut app, _rx) = with_sessions(1);
            let mut daemon = Daemon::connect(&mut app);
            app.handle_input(&text("O"));
            // One read: the name, enter (to the idea), a line, enter (a new line), a line, tab and back, ctrl+s.
            app.handle_input(&text("Tokens\rRework auth\r\nencrypted\t\t\u{13}"));
            assert_eq!(app.mode, Mode::Browse);
            let request = daemon.next().await;
            assert_eq!(request["title"], "Tokens");
            assert_eq!(request["prompt"], "Rework auth\nencrypted");
        });
    }

    #[test]
    fn a_blank_idea_is_refused_in_the_dialog() {
        local(async {
            let (mut app, _rx) = with_sessions(1);
            let _daemon = Daemon::connect(&mut app);
            app.handle_input(&text("O"));
            app.handle_input(&text("Auth tokens"));
            app.handle_input(&tab(false));
            app.handle_input(&text("  "));
            app.handle_input(&enter());
            app.handle_input(&tab(false));
            app.handle_input(&ctrl_s());
            assert_eq!(app.mode, Mode::EnterIdea);
            assert!(!app.messages.busy);
            assert_eq!(app.browse.idea.field, IdeaField::Idea);
            assert_eq!(app.browse.idea.message.as_deref(), Some(BLANK_IDEA));
            let (_, rows) = drawn(&app.browse.idea, Rect::new(0, 0, 98, 31));
            assert!(rows.iter().any(|row| row.contains(BLANK_IDEA)), "{rows:#?}");
            // The next key clears it.
            app.handle_input(&text("x"));
            assert_eq!(app.browse.idea.message, None);
        });
    }

    #[test]
    fn esc_starts_nothing_and_keeps_the_draft_for_the_next_o() {
        local(async {
            let (mut app, _rx) = with_sessions(1);
            let _daemon = Daemon::connect(&mut app);
            app.handle_input(&text("O"));
            app.handle_input(&text("Auth"));
            app.handle_input(&tab(false));
            app.handle_input(&text("Half a thought"));
            app.handle_input(&esc());
            assert_eq!(app.mode, Mode::Browse);
            assert!(!app.messages.busy);
            // O offers it again, where it was left.
            app.handle_input(&text("O"));
            assert_eq!(app.mode, Mode::EnterIdea);
            assert_eq!(fields(&app), ("Auth", "Half a thought", IdeaField::Idea));
            // ctrl+c acts as esc in a field.
            assert!(app.typing_in_field());
        });
    }

    #[test]
    fn a_failed_start_keeps_both_fields_and_says_why() {
        local(async {
            let (mut app, mut rx) = with_sessions(1);
            let mut daemon = Daemon::connect(&mut app);
            submit_idea(&mut app, "Auth", "Rework auth");
            let request = daemon.next().await;
            daemon.fail(&request, "claude is not installed").await;
            apply(&mut app, &mut rx).await;
            assert!(!app.messages.busy);
            assert_eq!(app.messages.error.as_deref(), Some("claude is not installed"));
            app.handle_input(&text("O"));
            assert_eq!(app.mode, Mode::EnterIdea);
            assert_eq!(fields(&app), ("Auth", "Rework auth", IdeaField::Idea));
            assert_eq!(
                app.browse.idea.message.as_deref(),
                Some("Not started: claude is not installed")
            );
        });
    }

    #[test]
    fn a_name_is_at_most_64_characters() {
        local(async {
            let (mut app, _rx) = with_sessions(1);
            let _daemon = Daemon::connect(&mut app);
            app.handle_input(&text("O"));
            for _ in 0..70 {
                app.handle_input(&text("n"));
            }
            assert_eq!(app.browse.idea.name.text, "n".repeat(64));
            assert_eq!(
                app.browse.idea.message.as_deref(),
                Some("A name is at most 64 characters")
            );
            // A paste is cut at the limit too.
            app.browse.idea = IdeaPrompt::default();
            app.handle_input(&text(&"m".repeat(70)));
            assert_eq!(app.browse.idea.name.text, "m".repeat(64));
            // Backspace and the arrows edit the field typed in, at its cursor.
            app.handle_input(&tab(false));
            app.handle_input(&text("ac"));
            app.handle_input(&key(|key| key.left_arrow = true));
            app.handle_input(&text("b"));
            app.handle_input(&key(|key| key.right_arrow = true));
            app.handle_input(&key(|key| key.backspace = true));
            assert_eq!(
                (app.browse.idea.name.text.len(), app.browse.idea.idea.text.as_str()),
                (64, "ab")
            );
        });
    }

    #[test]
    fn the_dialog_shows_both_fields_its_keys_and_the_cursor_in_the_field_typed_in() {
        let bounds = Rect::new(0, 0, 98, 31);
        let (buf, rows) = drawn(&IdeaPrompt::default(), bounds);
        let texts: Vec<&str> = rows
            .iter()
            .map(|row| row.trim_matches([' ', '│']))
            .filter(|row| !row.is_empty())
            .collect();
        assert_eq!(
            texts,
            [
                "╭──────────────────────────────────────────────────────────────────────────────╮",
                "New orchestrator",
                "Name  optional · 2–4 words",
                "What do you want to do?",
                "a sentence or a paragraph: the orchestrator's first message",
                "enter/tab next field • ctrl+s start • esc cancel",
                "╰──────────────────────────────────────────────────────────────────────────────╯",
            ]
        );
        // The empty name's cursor is on its placeholder.
        assert_eq!(inverse(&buf), ["o"]);
        // In the idea, at its end.
        let prompt = IdeaPrompt::with("Auth tokens", "Rotate tokens", IdeaField::Idea);
        let (buf, rows) = drawn(&prompt, bounds);
        assert!(rows.iter().any(|row| row.contains("│ Name  Auth tokens ")), "{rows:#?}");
        assert!(
            rows.iter()
                .any(|row| row.contains("enter new line • tab next field • ctrl+s start • esc cancel"))
        );
        assert_eq!(inverse(&buf), [" "]);
        // A long idea scrolls to its cursor; the dialog keeps its size.
        let long = (1..=30).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
        let (_, long_rows) = drawn(&IdeaPrompt::with("", &long, IdeaField::Idea), bounds);
        let corners = |rows: &[String]| {
            let top = rows.iter().position(|row| row.contains('╭'));
            let bottom = rows.iter().position(|row| row.contains('╰'));
            (top, bottom, rows.first().and_then(|row| row.find('╭')))
        };
        assert_eq!(corners(&long_rows), corners(&rows));
        assert!(long_rows.iter().any(|row| row.contains("line 30")), "{long_rows:#?}");
        assert!(!long_rows.iter().any(|row| row.contains("line 1 ")), "{long_rows:#?}");
    }

    #[test]
    fn the_dialog_fits_the_smallest_screens() {
        // 80x24 (the right pane beside the default sidebar), and the smallest layout: 20 columns, 8 rows.
        for (bounds, roomy) in [(Rect::new(0, 0, 55, 20), true), (Rect::new(0, 0, 20, 8), false)] {
            assert_eq!(shape(bounds).roomy, roomy);
            let prompt = IdeaPrompt::with("Auth tokens", &"word ".repeat(80), IdeaField::Idea);
            let (_, rows) = drawn(&prompt, bounds);
            let all = rows.join("\n");
            assert!(all.contains("New orch"), "{all}");
            assert!(all.contains("Name"), "{all}");
            assert!(all.contains("What do"), "{all}");
            // Nothing in the dialog scrolls but the idea, with its cursor: no "more" markers on its border.
            assert!(!all.contains("more"), "{all}");
        }
        // A compact dialog's message takes the keys' row.
        let prompt = IdeaPrompt {
            message: Some(BLANK_IDEA.to_owned()),
            ..IdeaPrompt::default()
        };
        let (_, rows) = drawn(&prompt, Rect::new(0, 0, 40, 8));
        assert!(rows.iter().any(|row| row.contains("Say what you")), "{rows:#?}");
        assert!(!rows.iter().any(|row| row.contains("ctrl+s")), "{rows:#?}");
        assert_eq!(
            shape(Rect::new(0, 0, 98, 31)),
            Shape {
                roomy: true,
                idea_rows: IDEA_ROWS
            }
        );
        assert_eq!(
            row_text(&dialog_rows(&prompt, shape(Rect::new(0, 0, 98, 31)), 80).header[0]),
            TITLE
        );
    }
}
