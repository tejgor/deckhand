//! Messages between sessions in the UI (`deckhand send` / `deckhand reply`, kept by the daemon's
//! `crates/daemon/src/messages.rs`): the messages the daemon keeps for the repository's sessions (`list-messages`
//! after each `subscribe`, then `messages-updated`), and the messages view (`e`) of the selected session.
//!
//! - **The view** lists what the session sent and was sent, **newest first**: you open it to see what just happened
//!   (whether your message was answered, what waits for it), and the log holds up to 50 messages a session, so the
//!   latest must not be at the bottom of a long scroll. Each shows its direction (`→` sent, `←` received), the other
//!   end (`you` for a message you sent by hand or a reply to one), how long ago, what became of it, its ID and its
//!   replies' IDs, then its text: the selected one whole, the others cut to a few lines.
//! - **Replies to you.** A reply to a message you sent by hand has no target: it comes back to you (`returned`) and
//!   this view is where you read it, in the log of the session that answered.
//! - **Cancelling.** `x` twice cancels the selected message while it is held (`cancel-message`). Nothing is sent or
//!   answered from here: you type to agents directly.

use std::ops::Range;
use std::rc::Rc;

use indexmap::IndexMap;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_core::display::{HINT_SEPARATOR, HintPart, display_session_title, fit_hint};
use deckhand_core::jsstr::date::now_ms;
use deckhand_core::key::Input;
use deckhand_core::messages_model::{Direction, LogRow, held_counts, log_row, session_log};
use deckhand_protocol::data::{MessageRecord, MessageStatus};

use crate::app::state::{App, Mode};
use crate::keymap::{Binding, HelpRef, KeySpec, Keymap, NamedKey};
use crate::press_again::PressAgain;
use crate::primitives::{
    Border, DialogRows, DialogScroll, DialogView, Row, Seg, TextStyle, Wrap, dialog_box, dialog_content, dialog_height,
    dialog_width, draw_dialog, fit_row, space_between, truncate_row, wrap_text,
};
use crate::theme::THEME;

/// The messages view's keys.
pub static KEYMAP: Keymap = &[
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
        action: "select a message",
        help: HelpRef::Hidden("moving through a list is what its on-screen hints say"),
    },
    Binding {
        keys: &[KeySpec::Char("x")],
        action: "twice: cancel the selected message while it is held",
        help: HelpRef::Topic("Sessions"),
    },
    Binding {
        keys: &[
            KeySpec::Named(NamedKey::Escape),
            KeySpec::Char("h"),
            KeySpec::Named(NamedKey::Left),
        ],
        action: "close the messages",
        help: HelpRef::Hidden("the view's own hint says esc goes back"),
    },
    crate::keymap::SCROLL_DIALOG,
];

/// How many lines of a message's text show while it is not selected.
pub const PREVIEW_LINES: usize = 3;

/// The messages view's state.
#[derive(Debug, Default)]
pub struct MessagesViewState {
    /// The session whose messages it shows.
    pub session_id: Option<String>,
    /// The selected message, by ID (the newest when unset or gone).
    pub selected: Option<String>,
    /// The message whose cancelling waits for a second `x`.
    pub press_again: PressAgain<String>,
    /// The selection moved: the next frame scrolls it into view.
    pub follow: bool,
    /// The message being cancelled, until the daemon answers.
    pub cancelling: Option<String>,
}

/// Asks the daemon for the repository's messages (after each `subscribe`, and `R`); they replace the ones held.
pub fn load(app: &mut App) {
    let Some(client) = app.client() else {
        return;
    };
    let reply = client.list_messages(&app.props.repo_root);
    drop(app.dispatch().spawn(reply, move |app: &mut App, result| {
        // An answer on a connection since replaced says nothing about the current one.
        if !app.client().is_some_and(|current| Rc::ptr_eq(&current, &client)) {
            return;
        }
        match result {
            Ok(messages) => set_messages(app, messages),
            Err(error) => app.set_error(Some(format!("Could not load the messages: {error}"))),
        }
    }));
}

/// `list-messages`' answer: the messages held now. A message updated since the list was made (its
/// `messages-updated` came first) keeps the newer record.
pub fn set_messages(app: &mut App, messages: Vec<MessageRecord>) {
    let mut held = std::mem::take(&mut app.data.session_messages);
    app.data.session_messages = messages
        .into_iter()
        .map(|message| match held.shift_remove(&message.id) {
            Some(newer) if newer.updated_at > message.updated_at => (newer.id.clone(), newer),
            _ => (message.id.clone(), message),
        })
        .collect::<IndexMap<_, _>>();
}

/// `messages-updated`: each message replaces the one with its ID, or is added.
pub fn messages_updated(app: &mut App, messages: Vec<MessageRecord>) {
    for message in messages {
        app.data.session_messages.insert(message.id.clone(), message);
    }
}

/// `e`: the selected session's messages.
pub fn open(app: &mut App) {
    let Some(session) = app.selected_session() else {
        return;
    };
    app.messages_view = MessagesViewState {
        session_id: Some(session.id),
        ..MessagesViewState::default()
    };
    app.set_mode(Mode::Messages);
}

/// The shown session's log as rows, newest first, and the selected row's index.
pub fn shown_rows(app: &App) -> (Vec<LogRow>, usize) {
    let Some(session_id) = app.messages_view.session_id.as_deref() else {
        return (Vec::new(), 0);
    };
    let sessions = &app.data.repo_sessions;
    let title_of = |id: &str| {
        sessions
            .iter()
            .find(|session| session.id == id)
            .map(|session| display_session_title(session, sessions))
    };
    let now = now_ms();
    let rows: Vec<LogRow> = session_log(app.data.session_messages.values(), session_id)
        .into_iter()
        .map(|message| log_row(message, session_id, now, &title_of))
        .collect();
    let selected = app
        .messages_view
        .selected
        .as_deref()
        .and_then(|id| rows.iter().position(|row| row.id == id))
        .unwrap_or(0);
    (rows, selected)
}

/// The shown session's title (its ID's start when it is gone).
pub fn shown_title(app: &App) -> String {
    let id = app.messages_view.session_id.clone().unwrap_or_default();
    let sessions = &app.data.repo_sessions;
    sessions
        .iter()
        .find(|session| session.id == id)
        .map(|session| display_session_title(session, sessions))
        .unwrap_or_else(|| id.chars().take(8).collect())
}

/// The messages view's keys: `j`/`k` (`g`/`G`) select, `x` twice cancels a held message, `esc` goes back. J/K
/// scroll (in `App::handle_input`).
pub fn handle_messages(app: &mut App, input: &Input) {
    let key = &input.key;
    let text = input.input.as_str();
    let armed = app.messages_view.press_again.take();
    if key.escape || key.left_arrow || text == "h" {
        app.set_mode(Mode::Browse);
        return;
    }
    let (rows, index) = shown_rows(app);
    let last = rows.len().saturating_sub(1);
    let moved = if key.down_arrow || text == "j" {
        Some((index + 1).min(last))
    } else if key.up_arrow || text == "k" {
        Some(index.saturating_sub(1))
    } else if key.home || text == "g" {
        Some(0)
    } else if key.end || text == "G" {
        Some(last)
    } else {
        None
    };
    if let Some(next) = moved {
        app.messages_view.selected = rows.get(next).map(|row| row.id.clone());
        app.messages_view.follow = true;
        return;
    }
    if text != "x" {
        return;
    }
    let Some(row) = rows.get(index) else {
        return;
    };
    if !row.cancellable {
        app.set_status(Some(format!(
            "Message {} is {}: only a held message can be cancelled",
            row.id, row.status
        )));
        return;
    }
    if app.messages_view.cancelling.is_some() {
        return;
    }
    if armed.as_deref() != Some(row.id.as_str()) {
        app.messages_view.press_again.arm(row.id.clone());
        let way = match row.direction {
            Direction::Sent => "to",
            Direction::Received => "from",
        };
        app.set_status(Some(format!("x again cancels message {} {way} {}", row.id, row.party)));
        return;
    }
    cancel(app, row.id.clone());
}

/// Cancels held message `id` (`cancel-message`, as you: any held message).
fn cancel(app: &mut App, id: String) {
    let Some(client) = app.client() else {
        app.set_error(Some("still connecting to daemon".to_owned()));
        return;
    };
    app.messages_view.cancelling = Some(id.clone());
    let reply = client.cancel_message(&id, None);
    drop(app.dispatch().spawn(reply, move |app: &mut App, result| {
        if app.messages_view.cancelling.as_deref() == Some(id.as_str()) {
            app.messages_view.cancelling = None;
        }
        match result {
            Ok(message) => {
                app.set_status(Some(format!("Cancelled message {}: it will not be typed", message.id)));
                messages_updated(app, vec![message]);
            }
            Err(error) => app.set_error(Some(error.to_string())),
        }
    }));
}

/// How many messages wait for `session_id`.
pub fn held_for(app: &App, session_id: &str) -> usize {
    held_counts(app.data.session_messages.values())
        .get(session_id)
        .copied()
        .unwrap_or(0)
}

/// The view's props.
#[derive(Clone, Copy, Debug)]
pub struct MessagesDialogProps<'a> {
    /// The session's title.
    pub title: &'a str,
    /// The session's ID (for `deckhand send` when it has none).
    pub session_id: &'a str,
    /// Its messages, newest first.
    pub rows: &'a [LogRow],
    pub selected: usize,
    /// The message being cancelled.
    pub cancelling: Option<&'a str>,
}

/// The view's rows and where each message's rows are in its body.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MessagesLayout {
    pub rows: DialogRows,
    pub blocks: Vec<Range<usize>>,
}

fn status_color(status: MessageStatus) -> ratatui::style::Color {
    match status {
        MessageStatus::Held => THEME.accent,
        MessageStatus::Delivered => THEME.active,
        MessageStatus::Answered | MessageStatus::Returned => THEME.success,
        MessageStatus::Dropped => THEME.error,
        MessageStatus::Cancelled => THEME.muted,
    }
}

/// A message's first row: the cursor, its direction and other end, its kind, age and status; its ID flush right.
fn head_row(row: &LogRow, selected: bool, width: usize) -> Row {
    let muted = TextStyle::fg(THEME.muted);
    let mut left = vec![
        if selected {
            Seg::new("› ", TextStyle::fg(THEME.active).bold(true))
        } else {
            Seg::plain("  ")
        },
        Seg::new(
            format!("{} ", row.direction.arrow()),
            TextStyle::fg(match row.direction {
                Direction::Sent => THEME.accent_soft,
                Direction::Received => THEME.active,
            }),
        ),
        Seg::new(row.party.clone(), TextStyle::fg(THEME.accent_soft).bold(selected)),
    ];
    for part in row
        .kind
        .iter()
        .chain(std::iter::once(&row.age).filter(|age| !age.is_empty()))
    {
        left.push(Seg::new(format!(" · {part}"), muted));
    }
    left.push(Seg::new(" · ", muted));
    left.push(Seg::new(row.status.as_str(), TextStyle::fg(status_color(row.status))));
    let right = vec![Seg::new(format!("  {}", row.id), muted)];
    space_between(&left, &right, width)
}

/// The view `width` wide (border included): the session and how many messages wait for it, its messages newest
/// first, and the keys.
pub fn messages_dialog_rows(props: &MessagesDialogProps<'_>, width: u16) -> MessagesLayout {
    let content_width = usize::from(width).saturating_sub(4).max(1);
    let text_width = content_width.saturating_sub(4).max(1);
    let muted = TextStyle::fg(THEME.muted);
    let mut layout = MessagesLayout::default();
    let rows = &mut layout.rows;
    rows.header.push(truncate_row(
        &[
            Seg::new("Messages", TextStyle::fg(THEME.accent).bold(true)),
            Seg::new(format!(" · {}", props.title), TextStyle::fg(THEME.accent_soft)),
        ],
        content_width,
        Wrap::TruncateEnd,
    ));
    let held = props.rows.iter().filter(|row| row.cancellable).count();
    let summary = match (props.rows.len(), held) {
        (0, _) => String::new(),
        (1, _) => "1 message".to_owned(),
        (count, 0) => format!("{count} messages, newest first"),
        (count, held) => format!("{count} messages, newest first · {held} held until it is idle"),
    };
    if !summary.is_empty() {
        rows.header.push(truncate_row(
            &[Seg::new(summary, muted)],
            content_width,
            Wrap::TruncateEnd,
        ));
    }
    rows.header.push(Vec::new());
    if props.rows.is_empty() {
        let short: String = props.session_id.chars().take(8).collect();
        rows.body.extend(fit_row(
            &[Seg::new(
                format!(
                    "No messages yet. Sessions send them with deckhand send; so can you, from a shell: \
                     deckhand send {short} \"<message>\"."
                ),
                muted,
            )],
            content_width,
            Wrap::Wrap,
        ));
    }
    for (index, row) in props.rows.iter().enumerate() {
        let selected = index == props.selected;
        if index > 0 {
            rows.body.push(Vec::new());
        }
        let start = rows.body.len();
        rows.body.push(head_row(row, selected, content_width));
        let indent = |text: String, style: TextStyle| vec![Seg::plain("    "), Seg::new(text, style)];
        let lines = wrap_text(row.text.trim_end(), text_width);
        let shown = if selected {
            lines.len()
        } else {
            lines.len().min(PREVIEW_LINES)
        };
        let text_style = if row.status == MessageStatus::Cancelled {
            muted
        } else {
            TextStyle::default()
        };
        for line in &lines[..shown] {
            rows.body.push(indent(line.clone(), text_style));
        }
        if shown < lines.len() {
            let more = lines.len() - shown;
            rows.body.push(indent(
                format!("… {more} more {}", if more == 1 { "line" } else { "lines" }),
                muted,
            ));
        }
        if row.truncated && shown == lines.len() {
            rows.body
                .push(indent("… only its start was kept".to_owned(), muted.italic(true)));
        }
        if let Some(held) = row.held.as_deref() {
            for line in wrap_text(&format!("held: {held}"), text_width) {
                rows.body.push(indent(line, TextStyle::fg(THEME.accent)));
            }
        }
        if let Some(note) = row.note.as_deref() {
            for line in wrap_text(&format!("! {note}"), text_width) {
                rows.body.push(indent(line, TextStyle::fg(THEME.warn)));
            }
        }
        if !row.replies.is_empty() {
            let label = if row.replies.len() == 1 { "reply" } else { "replies" };
            rows.body
                .push(indent(format!("{label} {}", row.replies.join(", ")), muted));
        }
        layout.blocks.push(start..rows.body.len());
    }
    rows.footer.push(Vec::new());
    let hint = match props.cancelling {
        Some(id) => format!("Cancelling {id}…"),
        None => {
            let mut parts: Vec<HintPart> = vec!["j/k select".into()];
            if props.rows.get(props.selected).is_some_and(|row| row.cancellable) {
                parts.push("x x cancel it".into());
            }
            parts.push("esc back".into());
            fit_hint(&parts, content_width, HINT_SEPARATOR)
        }
    };
    rows.footer.push(vec![Seg::new(hint, muted)]);
    layout
}

/// The scroll that shows `block` (body rows) in `room` rows, moving `scroll` as little as it can; its first row wins
/// when it does not fit.
pub fn scroll_into_view(scroll: usize, block: &Range<usize>, room: usize) -> usize {
    if block.start < scroll {
        return block.start;
    }
    if block.end > scroll + room {
        return block.start.min(block.end.saturating_sub(room));
    }
    scroll
}

/// [`messages_dialog_rows`] drawn over `area`; with `follow`, scrolled to show the selected message first. Returns the
/// scroll drawn and how the body scrolls.
pub fn render_messages_dialog(
    props: &MessagesDialogProps<'_>,
    buf: &mut Buffer,
    area: Rect,
    view: DialogView,
    follow: bool,
) -> (usize, DialogScroll) {
    let mut scroll = view.scroll;
    if follow {
        // The rows are laid out at the widest first; the dialog is narrower only when that changes no row.
        let layout = messages_dialog_rows(props, dialog_width(area));
        let rows = &layout.rows;
        let height = dialog_height(area, rows.len(), view.floor_height);
        let content = dialog_content(dialog_box(area, dialog_width(area), height), rows.len());
        let room = usize::from(content.height).saturating_sub(rows.header.len() + rows.footer.len());
        if let Some(block) = layout.blocks.get(props.selected) {
            scroll = scroll_into_view(scroll, block, room.max(1));
        }
    }
    let build = |width| messages_dialog_rows(props, width).rows;
    let drawn = draw_dialog(
        buf,
        area,
        &Border::round(THEME.border_active),
        &build,
        DialogView { scroll, ..view },
    );
    (scroll.min(drawn.max), drawn)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use deckhand_core::key::Key;
    use deckhand_protocol::data::{HeldReason, HeldReasonKind, SessionStatus};

    use super::*;
    use crate::app::modes::browse::tests::{Daemon, apply, local, named, text};
    use crate::app::state::AppProps;
    use crate::app::test_support::{app, session};
    use crate::primitives::{row_text, string_width};

    fn message(id: &str, fields: Value) -> MessageRecord {
        let mut value = json!({
            "id": id, "kind": "message", "text": "Which token store does the login form read from?",
            "status": "delivered", "createdAt": "2026-11-14T12:00:00.000Z", "updatedAt": "2026-11-14T12:00:00.000Z",
        });
        if let (Some(map), Some(extra)) = (value.as_object_mut(), fields.as_object()) {
            map.extend(extra.clone());
        }
        serde_json::from_value(value).unwrap_or_else(|error| panic!("{error}"))
    }

    fn at(minute: u32) -> String {
        format!("2026-11-14T12:{minute:02}:00.000Z")
    }

    /// Login UI asked Token storage (answered by b2); you asked Token storage (answered by d4, returned to you),
    /// then sent it two more, held; and you sent Login UI one, cancelled.
    fn walkthrough() -> Vec<MessageRecord> {
        vec![
            message(
                "a1a1a1",
                json!({"fromSessionId": "login", "fromTitle": "Login UI", "toSessionId": "tokens",
                    "toTitle": "Token storage", "wantsAnswer": true, "status": "answered", "replies": ["b2b2b2"],
                    "createdAt": at(1)}),
            ),
            message(
                "b2b2b2",
                json!({"kind": "reply", "fromSessionId": "tokens", "fromTitle": "Token storage",
                    "toSessionId": "login", "toTitle": "Login UI", "inReplyTo": "a1a1a1", "status": "returned",
                    "text": "The keychain, through TokenStore::shared().", "createdAt": at(2)}),
            ),
            message(
                "c3c3c3",
                json!({"toSessionId": "tokens", "toTitle": "Token storage", "status": "answered",
                    "text": "Is the migration done?", "replies": ["d4d4d4"], "createdAt": at(3)}),
            ),
            message(
                "d4d4d4",
                json!({"kind": "reply", "fromSessionId": "tokens", "fromTitle": "Token storage",
                    "inReplyTo": "c3c3c3", "status": "returned",
                    "text": "Done: the old store is read once and deleted.\nThe tests cover both paths.",
                    "createdAt": at(4)}),
            ),
            message(
                "e5e5e5",
                json!({"toSessionId": "tokens", "toTitle": "Token storage", "status": "held",
                    "text": "Then rebase on main and run the e2e suite.", "createdAt": at(5)}),
            ),
            message(
                "f6f6f6",
                json!({"toSessionId": "tokens", "toTitle": "Token storage", "status": "held",
                    "text": "And tell Login UI when it is green.", "createdAt": at(6)}),
            ),
            message(
                "g7g7g7",
                json!({"toSessionId": "login", "toTitle": "Login UI", "status": "cancelled",
                    "text": "Hold off on the styling.", "createdAt": at(7)}),
            ),
        ]
    }

    fn with_messages() -> (
        App,
        tokio::sync::mpsc::UnboundedReceiver<crate::app::event_loop::Update<App>>,
    ) {
        let (mut app, rx) = app(AppProps::default());
        app.data.repo_sessions = vec![
            session("login", "Login UI", SessionStatus::Running),
            session("tokens", "Token storage", SessionStatus::Running),
        ];
        app.data.sessions_loaded = true;
        set_messages(&mut app, walkthrough());
        app.set_selected_id(Some("tokens".to_owned()));
        (app, rx)
    }

    fn json_of(message: &MessageRecord) -> Value {
        serde_json::to_value(message).unwrap_or_else(|error| panic!("{error}"))
    }

    fn ids(app: &App) -> Vec<String> {
        app.data.session_messages.keys().cloned().collect()
    }

    #[test]
    fn the_messages_are_listed_on_subscribe_and_follow_their_updates() {
        local(async {
            let (mut app, mut rx) = app(AppProps::default());
            let mut daemon = Daemon::connect(&mut app);
            load(&mut app);
            let request = daemon.next().await;
            assert_eq!(request["type"], "list-messages");
            assert_eq!(request["repoRoot"], "/repo");
            // An update that overtook the list keeps its newer record; the rest come from the list.
            let mut newer = walkthrough()[4].clone();
            newer.status = MessageStatus::Delivered;
            newer.updated_at = at(9);
            messages_updated(&mut app, vec![newer]);
            let list: Vec<Value> = walkthrough().iter().map(json_of).collect();
            daemon.ok(&request, Value::Array(list)).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(
                ids(&app),
                ["a1a1a1", "b2b2b2", "c3c3c3", "d4d4d4", "e5e5e5", "f6f6f6", "g7g7g7"]
            );
            assert_eq!(app.data.session_messages["e5e5e5"].status, MessageStatus::Delivered);
            // messages-updated replaces by ID and adds new ones.
            let mut answered = walkthrough()[5].clone();
            answered.status = MessageStatus::Delivered;
            messages_updated(
                &mut app,
                vec![answered, message("h8h8h8", json!({"toSessionId": "tokens"}))],
            );
            assert_eq!(app.data.session_messages.len(), 8);
            assert_eq!(app.data.session_messages["f6f6f6"].status, MessageStatus::Delivered);
            // A reconnect lists them again: what the daemon no longer keeps goes.
            load(&mut app);
            let request = daemon.next().await;
            daemon.ok(&request, json!([json_of(&walkthrough()[0])])).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(ids(&app), ["a1a1a1"]);
        });
    }

    #[test]
    fn the_handler_upserts_messages_updated() {
        local(async {
            let (mut app, mut rx) = with_messages();
            let handlers = crate::app::effects::live_client_handlers(&app.connection.dispatch.clone(), 0);
            let mut cancelled = walkthrough()[4].clone();
            cancelled.status = MessageStatus::Cancelled;
            if let Some(handler) = &handlers.on_messages_updated {
                handler(vec![cancelled]);
            }
            apply(&mut app, &mut rx).await;
            assert_eq!(app.data.session_messages["e5e5e5"].status, MessageStatus::Cancelled);
            assert_eq!(held_for(&app, "tokens"), 1);
        });
    }

    #[test]
    fn e_opens_the_selected_sessions_messages_newest_first_and_esc_goes_back() {
        let (mut app, _rx) = with_messages();
        crate::app::modes::browse::handle_browse(&mut app, &text("e"));
        assert_eq!(app.mode, Mode::Messages);
        let (rows, selected) = shown_rows(&app);
        let shown: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(shown, ["f6f6f6", "e5e5e5", "d4d4d4", "c3c3c3", "b2b2b2", "a1a1a1"]);
        assert_eq!(selected, 0);
        handle_messages(&mut app, &text("j"));
        handle_messages(&mut app, &text("j"));
        assert_eq!(shown_rows(&app).1, 2);
        assert!(app.messages_view.follow);
        handle_messages(&mut app, &text("G"));
        assert_eq!(shown_rows(&app).1, 5);
        handle_messages(&mut app, &text("j"));
        assert_eq!(shown_rows(&app).1, 5, "stays on the last");
        handle_messages(&mut app, &text("g"));
        assert_eq!(shown_rows(&app).1, 0);
        // The selection follows its message as others arrive.
        handle_messages(&mut app, &text("j"));
        messages_updated(
            &mut app,
            vec![message("z9z9z9", json!({"toSessionId": "tokens", "createdAt": at(8)}))],
        );
        assert_eq!(shown_rows(&app).0[shown_rows(&app).1].id, "e5e5e5");
        handle_messages(
            &mut app,
            &named(Key {
                escape: true,
                ..Key::NONE
            }),
        );
        assert_eq!(app.mode, Mode::Browse);
    }

    #[test]
    fn x_twice_cancels_a_held_message_and_only_a_held_one() {
        local(async {
            let (mut app, mut rx) = with_messages();
            let mut daemon = Daemon::connect(&mut app);
            open(&mut app);
            // f6 (held) is selected: the first x asks, a second cancels it.
            handle_messages(&mut app, &text("x"));
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("x again cancels message f6f6f6 from you")
            );
            handle_messages(&mut app, &text("x"));
            let request = daemon.next().await;
            assert_eq!(request["type"], "cancel-message");
            assert_eq!(request["messageId"], "f6f6f6");
            assert!(request.get("caller").is_none(), "cancelled as you, by hand");
            assert_eq!(app.messages_view.cancelling.as_deref(), Some("f6f6f6"));
            let mut cancelled = walkthrough()[5].clone();
            cancelled.status = MessageStatus::Cancelled;
            daemon.ok(&request, json_of(&cancelled)).await;
            apply(&mut app, &mut rx).await;
            assert_eq!(app.messages_view.cancelling, None);
            assert_eq!(app.data.session_messages["f6f6f6"].status, MessageStatus::Cancelled);
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Cancelled message f6f6f6: it will not be typed")
            );
            // Another key between the two presses cancels nothing.
            handle_messages(&mut app, &text("j"));
            handle_messages(&mut app, &text("x"));
            handle_messages(&mut app, &text("k"));
            handle_messages(&mut app, &text("j"));
            handle_messages(&mut app, &text("x"));
            assert_eq!(app.messages_view.cancelling, None);
            // A message no longer held can't be.
            handle_messages(&mut app, &text("j"));
            handle_messages(&mut app, &text("x"));
            assert_eq!(
                app.messages.status_message.as_deref(),
                Some("Message d4d4d4 is returned: only a held message can be cancelled")
            );
            // A refusal shows as an error.
            handle_messages(&mut app, &text("k"));
            handle_messages(&mut app, &text("x"));
            handle_messages(&mut app, &text("x"));
            let request = daemon.next().await;
            assert_eq!(request["messageId"], "e5e5e5");
            daemon
                .fail(
                    &request,
                    "Message e5e5e5 is delivered: only a held message can be cancelled",
                )
                .await;
            apply(&mut app, &mut rx).await;
            assert_eq!(
                app.messages.error.as_deref(),
                Some("Message e5e5e5 is delivered: only a held message can be cancelled")
            );
        });
    }

    fn texts(layout: &MessagesLayout) -> Vec<String> {
        let rows = &layout.rows;
        [&rows.header, &rows.body, &rows.footer]
            .into_iter()
            .flatten()
            .map(|row| row_text(row).trim_end().to_owned())
            .collect()
    }

    fn props<'a>(rows: &'a [LogRow], selected: usize) -> MessagesDialogProps<'a> {
        MessagesDialogProps {
            title: "Token storage",
            session_id: "tokens",
            rows,
            selected,
            cancelling: None,
        }
    }

    /// The walkthrough as Token storage's view shows it at 12:10.
    fn token_rows() -> Vec<LogRow> {
        let messages = walkthrough();
        let titles = |id: &str| match id {
            "login" => Some("Login UI".to_owned()),
            "tokens" => Some("Token storage".to_owned()),
            _ => None,
        };
        let now = deckhand_core::jsstr::date::parse(&at(10)).unwrap_or_default();
        session_log(&messages, "tokens")
            .into_iter()
            .map(|message| log_row(message, "tokens", now, &titles))
            .collect()
    }

    #[test]
    fn a_reply_to_your_message_is_readable_where_it_came_back() {
        let rows = token_rows();
        let lines = texts(&messages_dialog_rows(&props(&rows, 2), 80));
        let at = |text: &str| {
            lines
                .iter()
                .position(|line| line.starts_with(text))
                .unwrap_or_else(|| panic!("{text}: {lines:#?}"))
        };
        // Sent by Token storage to you, back to you: its whole text, under its head.
        let reply = at("› → you · reply to c3c3c3 · 6m · returned");
        assert!(lines[reply].ends_with("d4d4d4"), "{}", lines[reply]);
        assert_eq!(
            lines[reply + 1..reply + 3],
            [
                "    Done: the old store is read once and deleted.",
                "    The tests cover both paths."
            ]
        );
        // Your message it answers names the reply.
        let asked = at("  ← you · 7m · answered");
        assert_eq!(
            lines[asked + 1..asked + 3],
            ["    Is the migration done?", "    reply d4d4d4"]
        );
        assert!(lines[at("  ← Login UI · wants an answer · 9m · answered")].ends_with("a1a1a1"));
        assert_eq!(lines[0], "Messages · Token storage");
        assert_eq!(lines[1], "6 messages, newest first · 2 held until it is idle");
        assert_eq!(lines.last().map(String::as_str), Some("j/k select • esc back"));
    }

    #[test]
    fn long_text_is_cut_unless_selected_and_a_kept_start_says_so() {
        let mut rows = token_rows();
        rows[0].text = (1..=8)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        rows[0].truncated = true;
        rows[1].note = Some("Typed, but no turn started: it may still be in the input".to_owned());
        let lines = texts(&messages_dialog_rows(&props(&rows, 1), 80));
        assert_eq!(
            lines[4..8],
            ["    line 1", "    line 2", "    line 3", "    … 5 more lines"]
        );
        assert!(
            lines
                .iter()
                .any(|line| line == "    ! Typed, but no turn started: it may still be in the input")
        );
        let selected = texts(&messages_dialog_rows(&props(&rows, 0), 80));
        assert_eq!(selected[11], "    line 8");
        assert_eq!(selected[12], "    … only its start was kept");
        assert!(selected.last().is_some_and(|hint| hint.contains("x x cancel it")));
    }

    #[test]
    fn an_empty_log_says_how_messages_are_sent() {
        let lines = texts(&messages_dialog_rows(&props(&[], 0), 80));
        assert_eq!(lines[0], "Messages · Token storage");
        assert!(
            lines[2].starts_with("No messages yet. Sessions send them with deckhand send"),
            "{lines:#?}"
        );
    }

    #[test]
    fn the_selected_message_scrolls_into_view() {
        assert_eq!(scroll_into_view(0, &(2..5), 10), 0);
        assert_eq!(scroll_into_view(8, &(2..5), 10), 2);
        assert_eq!(scroll_into_view(0, &(12..15), 10), 5);
        // Taller than the room: its head first.
        assert_eq!(scroll_into_view(0, &(12..40), 10), 12);
    }

    fn drawn(props: &MessagesDialogProps<'_>, width: u16, height: u16, scroll: usize, follow: bool) -> Vec<String> {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        let view = DialogView {
            scroll,
            ..DialogView::default()
        };
        render_messages_dialog(props, &mut buf, area, view, follow);
        (0..area.height)
            .map(|y| {
                let mut row = String::new();
                let mut x = 0;
                while x < area.width {
                    let symbol = buf[(x, y)].symbol();
                    row.push_str(symbol);
                    x += u16::try_from(string_width(symbol).max(1)).unwrap_or(1);
                }
                row.trim_end().to_owned()
            })
            .collect()
    }

    #[test]
    fn the_view_draws_the_log_and_follows_the_selection() {
        let rows = token_rows();
        let frame = drawn(&props(&rows, 2), 98, 31, 0, false);
        assert_pane("68-messages", &frame);
        // The last message selected in a short pane: scrolled so it shows.
        let frame = drawn(&props(&rows, 5), 98, 16, 0, true);
        assert!(
            frame.iter().any(|row| row.contains("› ← Login UI · wants an answer")),
            "{frame:#?}"
        );
    }

    #[test]
    fn the_app_marks_waiting_messages_and_draws_the_view_over_the_pane() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let (mut app, _rx) = with_messages();
        let mut terminal = Terminal::new(TestBackend::new(130, 36)).unwrap_or_else(|error| panic!("{error}"));
        let mut screen = |app: &mut App| -> Vec<String> {
            terminal
                .draw(|frame| crate::app::layout::render_app(app, frame))
                .unwrap_or_else(|error| panic!("{error}"));
            let buffer = terminal.backend().buffer();
            (0..buffer.area.height)
                .map(|y| {
                    let mut row = String::new();
                    let mut x = 0;
                    while x < buffer.area.width {
                        let symbol = buffer[(x, y)].symbol();
                        row.push_str(symbol);
                        x += u16::try_from(string_width(symbol).max(1)).unwrap_or(1);
                    }
                    row
                })
                .collect()
        };
        let browse = screen(&mut app);
        let row = browse
            .iter()
            .find(|line| line.contains("Token storage"))
            .unwrap_or_else(|| panic!("{browse:#?}"));
        assert!(row.contains("✉2"), "{row}");
        assert!(
            browse.iter().any(|line| line.contains("✉ 2 waiting · last from you")),
            "{browse:#?}"
        );
        open(&mut app);
        let view = screen(&mut app);
        assert!(
            view.iter().any(|line| line.contains("Messages · Token storage")),
            "{view:#?}"
        );
        assert!(view.iter().any(|line| line.contains("› ← you · ")), "{view:#?}");
    }

    /// The whole app drawn `width` by `height`.
    fn app_screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap_or_else(|error| panic!("{error}"));
        terminal
            .draw(|frame| crate::app::layout::render_app(app, frame))
            .unwrap_or_else(|error| panic!("{error}"));
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                let mut row = String::new();
                let mut x = 0;
                while x < buffer.area.width {
                    let symbol = buffer[(x, y)].symbol();
                    row.push_str(symbol);
                    x += u16::try_from(string_width(symbol).max(1)).unwrap_or(1);
                }
                row
            })
            .collect()
    }

    #[test]
    fn the_details_and_the_view_say_why_the_next_held_message_waits() {
        let (mut app, _rx) = with_messages();
        let mut next = walkthrough()[4].clone();
        next.held_reason = Some(HeldReason {
            kind: HeldReasonKind::Stalled,
            until: None,
        });
        messages_updated(&mut app, vec![next]);
        let browse = app_screen(&mut app, 130, 36);
        assert!(
            browse.iter().any(|line| line.contains("✉ 2 waiting · last from you")),
            "{browse:#?}"
        );
        assert!(
            browse.iter().any(|line| line.contains("an earlier message was typed")),
            "{browse:#?}"
        );
        open(&mut app);
        let view = app_screen(&mut app, 130, 36);
        assert!(
            view.iter()
                .any(|line| line.contains("held: an earlier message was typed but no turn started")),
            "{view:#?}"
        );
        // Only under the next one: the one after it waits behind it.
        assert_eq!(
            view.iter().filter(|line| line.contains("held: ")).count(),
            1,
            "{view:#?}"
        );
    }

    fn assert_pane(name: &str, frame: &[String]) {
        deckhand_testkit::assert_snapshot(format!("fixtures/ui/panes/{name}.txt"), &frame.join("\n"));
    }
}
