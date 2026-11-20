//! Messages between sessions as the UI shows them (`deckhand send` / `deckhand reply`; the daemon's
//! `crates/daemon/src/messages.rs` keeps them). Pure: which messages wait for a session (the sidebar's `✉N`), a
//! session's log (what it sent and was sent), the details block's line, why the next one waits, and the rows of the
//! messages view.
//!
//! A message without a sender came from you (by hand) or, for a notice, from Deckhand; one without a target is a reply
//! to a message you sent by hand, handed back to you (`returned`).

use std::collections::HashMap;

use deckhand_protocol::data::{HeldReason, HeldReasonKind, MessageKind, MessageRecord, MessageStatus};

use crate::jsstr::{date, js_len};
use crate::notes::fit_first;
use crate::sidebar_model::format_age;

/// The sidebar's and the details' marker for messages waiting to be typed into a session.
pub const MESSAGE_MARKER: &str = "✉";

/// How many messages are held for each session (waiting in its queue until it is idle at its input), by session ID.
pub fn held_counts<'a>(messages: impl IntoIterator<Item = &'a MessageRecord>) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for message in messages {
        if message.status == MessageStatus::Held
            && let Some(target) = message.to_session_id.as_ref()
        {
            *counts.entry(target.clone()).or_insert(0) += 1;
        }
    }
    counts
}

/// The messages `session_id` sent or was sent, newest first (by when they were sent; of two sent at once, the one
/// listed later first).
pub fn session_log<'a>(
    messages: impl IntoIterator<Item = &'a MessageRecord>,
    session_id: &str,
) -> Vec<&'a MessageRecord> {
    let mut log: Vec<&MessageRecord> = messages
        .into_iter()
        .filter(|message| {
            message.from_session_id.as_deref() == Some(session_id)
                || message.to_session_id.as_deref() == Some(session_id)
        })
        .collect();
    log.reverse();
    // ISO timestamps sort as text; the sort is stable, so ties keep the later one first.
    log.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    log
}

/// Which way a message went, seen from the session whose log it is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// The session sent it (`→`).
    Sent,
    /// The session was sent it (`←`).
    Received,
}

impl Direction {
    pub fn arrow(self) -> &'static str {
        match self {
            Direction::Sent => "→",
            Direction::Received => "←",
        }
    }
}

/// Which way `message` went for `session_id`.
pub fn direction(message: &MessageRecord, session_id: &str) -> Direction {
    if message.from_session_id.as_deref() == Some(session_id) {
        Direction::Sent
    } else {
        Direction::Received
    }
}

/// The other end of a message, seen from one session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Party {
    /// You: a message sent by hand, or a reply handed back to you.
    You,
    /// Deckhand's own notice (that a message was not delivered).
    Deckhand,
    /// Another session, with its title when the message was sent.
    Session { id: String, title: Option<String> },
}

/// The other end of `message` for `session_id`.
pub fn other_party(message: &MessageRecord, session_id: &str) -> Party {
    let (id, title) = match direction(message, session_id) {
        Direction::Sent => (&message.to_session_id, &message.to_title),
        Direction::Received => (&message.from_session_id, &message.from_title),
    };
    match id {
        Some(id) => Party::Session {
            id: id.clone(),
            title: title.clone().filter(|title| !title.is_empty()),
        },
        None if message.kind == MessageKind::Notice => Party::Deckhand,
        None => Party::You,
    }
}

/// A session's title now (`None` when it is gone), for naming a party.
pub type TitleOf<'a> = &'a dyn Fn(&str) -> Option<String>;

/// What a party is called: `you`, `Deckhand`, or the session's title now, else as it was when the message was sent,
/// else its short ID.
pub fn party_name(party: &Party, title_of: TitleOf<'_>) -> String {
    match party {
        Party::You => "you".to_owned(),
        Party::Deckhand => "Deckhand".to_owned(),
        Party::Session { id, title } => title_of(id)
            .filter(|title| !title.is_empty())
            .or_else(|| title.clone())
            .unwrap_or_else(|| id.chars().take(8).collect()),
    }
}

/// Why a held message waits, in words, at `now_ms` (the details, the messages view and the daemon's log say it so).
pub fn held_reason_text(reason: &HeldReason, now_ms: i64) -> String {
    let text = match reason.kind {
        HeldReasonKind::NotRunning => "the session is not running: delivered once it runs again",
        HeldReasonKind::Starting => "the session is starting",
        HeldReasonKind::Working => "the session is working on a turn",
        HeldReasonKind::Question => "the session waits on a question or permission",
        HeldReasonKind::Menu => "the session shows a menu",
        HeldReasonKind::Draft => "a task draft waits in its input: send or clear it",
        HeldReasonKind::UnsentText => match reason.until.as_deref().and_then(date::parse) {
            Some(until) => {
                return format!(
                    "unsent text in its input, delivers {}, or press enter",
                    time_left(until - now_ms)
                );
            }
            None => "unsent text still in its input: press enter or clear it",
        },
        HeldReasonKind::UnsentPaste => "a paste in its input that can't be checked: press enter or clear it",
        HeldReasonKind::Stalled => "an earlier message was typed but no turn started: press enter in the session",
    };
    text.to_owned()
}

/// `in 4 min` (rounded up), `in under a minute`, `in a moment`.
fn time_left(ms: i64) -> String {
    match ms {
        ..=0 => "in a moment".to_owned(),
        1..60_000 => "in under a minute".to_owned(),
        _ => format!("in {} min", (ms + 59_999) / 60_000),
    }
}

/// Why `session_id`'s queue waits: the reason its next held message carries.
pub fn held_reason<'a>(
    messages: impl IntoIterator<Item = &'a MessageRecord>,
    session_id: &str,
) -> Option<&'a HeldReason> {
    messages
        .into_iter()
        .filter(|message| message.status == MessageStatus::Held && message.to_session_id.as_deref() == Some(session_id))
        .find_map(|message| message.held_reason.as_ref())
}

/// A status in words, as the messages view shows it.
pub fn status_text(status: MessageStatus) -> &'static str {
    status.as_str()
}

/// How long ago `iso` was at `now_ms` (`3m`, `now`), empty when it does not parse.
fn age(iso: &str, now_ms: i64) -> String {
    date::parse(iso).map_or_else(String::new, |at| format_age(now_ms as f64 - at as f64))
}

/// One message in the messages view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogRow {
    pub id: String,
    pub direction: Direction,
    /// The other end: `you`, `Deckhand` or a session's title.
    pub party: String,
    /// What kind of message it is, when not a plain one: `reply to 7a3f01`, `notice`, `wants an answer`.
    pub kind: Option<String>,
    /// How long ago it was sent (`3m`).
    pub age: String,
    pub status: MessageStatus,
    /// The IDs of its replies, oldest first.
    pub replies: Vec<String>,
    /// Why it was dropped, or that no turn started after it was typed.
    pub note: Option<String>,
    /// Held, the next of its queue: why it waits ([`held_reason_text`]).
    pub held: Option<String>,
    pub text: String,
    /// Only its start is kept (it left the queue longer than that).
    pub truncated: bool,
    /// Held: `x` twice cancels it.
    pub cancellable: bool,
}

/// `message` as a row of `session_id`'s messages view at `now_ms`.
pub fn log_row(message: &MessageRecord, session_id: &str, now_ms: i64, title_of: TitleOf<'_>) -> LogRow {
    let kind = match message.kind {
        MessageKind::Reply => Some(match message.in_reply_to.as_deref().filter(|id| !id.is_empty()) {
            Some(id) => format!("reply to {id}"),
            None => "reply".to_owned(),
        }),
        MessageKind::Notice => Some("notice".to_owned()),
        MessageKind::Message if message.wants_answer == Some(true) => Some("wants an answer".to_owned()),
        MessageKind::Message => None,
    };
    LogRow {
        id: message.id.clone(),
        direction: direction(message, session_id),
        party: party_name(&other_party(message, session_id), title_of),
        kind,
        age: age(&message.created_at, now_ms),
        status: message.status,
        replies: message.replies.clone().unwrap_or_default(),
        note: message.note.clone().filter(|note| !note.trim().is_empty()),
        held: message
            .held_reason
            .as_ref()
            .filter(|_| message.status == MessageStatus::Held)
            .map(|reason| held_reason_text(reason, now_ms)),
        text: message.text.clone(),
        truncated: message.text_truncated == Some(true),
        cancellable: message.status == MessageStatus::Held,
    }
}

/// What the details block says about a session's messages: how many wait for it, and its latest message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessagesBrief {
    /// Messages held for it.
    pub held: usize,
    /// Why the next of them waits.
    pub held_reason: Option<HeldReason>,
    pub last_direction: Direction,
    /// The other end of its latest message.
    pub last_party: String,
    pub last_created_at: String,
    pub last_status: MessageStatus,
}

/// `session_id`'s brief, `None` when it has no messages (`messages` in any order).
pub fn messages_brief<'a>(
    messages: impl IntoIterator<Item = &'a MessageRecord>,
    session_id: &str,
    title_of: TitleOf<'_>,
) -> Option<MessagesBrief> {
    let log = session_log(messages, session_id);
    let last = log.first()?;
    let held = log
        .iter()
        .filter(|message| message.status == MessageStatus::Held && message.to_session_id.as_deref() == Some(session_id))
        .count();
    Some(MessagesBrief {
        held,
        held_reason: held_reason(log.iter().copied(), session_id).cloned(),
        last_direction: direction(last, session_id),
        last_party: party_name(&other_party(last, session_id), title_of),
        last_created_at: last.created_at.clone(),
        last_status: last.status,
    })
}

/// The details' line, `width` wide: `✉ 2 waiting · last from Login UI 3m (answered)`. Narrower, the status goes,
/// then the party (cut first), then the latest message altogether.
pub fn brief_text(brief: &MessagesBrief, now_ms: i64, width: usize) -> String {
    let head = match brief.held {
        0 => MESSAGE_MARKER.to_owned(),
        held => format!("{MESSAGE_MARKER} {held} waiting ·"),
    };
    let way = match brief.last_direction {
        Direction::Sent => "to",
        Direction::Received => "from",
    };
    let age = age(&brief.last_created_at, now_ms);
    let status = status_text(brief.last_status);
    let party = &brief.last_party;
    let full = format!("{head} last {way} {party} {age} ({status})");
    let no_status = format!("{head} last {way} {party} {age}");
    // The party cut to what is left once the rest fits, keeping at least a few of its characters.
    let fixed = js_len(&format!("{head} last {way}  {age}"));
    let cut_party = (width > fixed + 3).then(|| {
        format!(
            "{head} last {way} {} {age}",
            crate::display::truncate(party, width - fixed)
        )
    });
    let mut candidates = vec![full, no_status];
    candidates.extend(cut_party);
    candidates.push(match brief.held {
        0 => format!("{MESSAGE_MARKER} last {age}"),
        held => format!("{MESSAGE_MARKER} {held} waiting"),
    });
    fit_first(&candidates, width)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn message(id: &str, fields: Value) -> MessageRecord {
        let mut value = json!({
            "id": id, "kind": "message", "text": "Which token store?", "status": "delivered",
            "createdAt": "2026-11-14T12:00:00.000Z", "updatedAt": "2026-11-14T12:00:00.000Z",
        });
        if let (Some(map), Some(extra)) = (value.as_object_mut(), fields.as_object()) {
            map.extend(extra.clone());
        }
        serde_json::from_value(value).unwrap_or_else(|error| panic!("{error}"))
    }

    fn at(minute: u32) -> String {
        format!("2026-11-14T12:{minute:02}:00.000Z")
    }

    fn noon_plus(minutes: i64) -> i64 {
        date::parse("2026-11-14T12:00:00.000Z").unwrap_or_default() + minutes * 60_000
    }

    fn titles(id: &str) -> Option<String> {
        match id {
            "login" => Some("Login UI".to_owned()),
            "tokens" => Some("Token storage".to_owned()),
            _ => None,
        }
    }

    /// Login UI asked Token storage (answered), you sent Token storage two (one held), and Token storage answered you.
    fn walkthrough() -> Vec<MessageRecord> {
        vec![
            message(
                "a1",
                json!({"fromSessionId": "login", "fromTitle": "Login UI", "toSessionId": "tokens",
                    "toTitle": "Token storage", "wantsAnswer": true, "status": "answered", "replies": ["b2"],
                    "createdAt": at(1)}),
            ),
            message(
                "b2",
                json!({"kind": "reply", "fromSessionId": "tokens", "toSessionId": "login", "toTitle": "Login UI",
                    "inReplyTo": "a1", "status": "returned", "text": "The keychain.", "createdAt": at(2)}),
            ),
            message(
                "c3",
                json!({"toSessionId": "tokens", "toTitle": "Token storage", "status": "answered", "replies": ["d4"],
                    "createdAt": at(3)}),
            ),
            message(
                "d4",
                json!({"kind": "reply", "fromSessionId": "tokens", "fromTitle": "Token storage", "inReplyTo": "c3",
                    "status": "returned", "text": "Done.", "createdAt": at(4)}),
            ),
            message(
                "e5",
                json!({"toSessionId": "tokens", "toTitle": "Token storage", "status": "held", "createdAt": at(5)}),
            ),
        ]
    }

    #[test]
    fn held_messages_count_for_their_target_only() {
        let messages = walkthrough();
        let counts = held_counts(&messages);
        assert_eq!(counts.get("tokens"), Some(&1));
        assert_eq!(
            counts.get("login"),
            None,
            "the sender of a held message has none waiting"
        );
        let mut delivered = messages.clone();
        delivered[4].status = MessageStatus::Delivered;
        assert!(held_counts(&delivered).is_empty());
    }

    #[test]
    fn a_sessions_log_holds_what_it_sent_and_was_sent_newest_first() {
        let messages = walkthrough();
        let ids = |session: &str| -> Vec<String> {
            session_log(&messages, session)
                .into_iter()
                .map(|message| message.id.clone())
                .collect()
        };
        assert_eq!(ids("tokens"), ["e5", "d4", "c3", "b2", "a1"]);
        assert_eq!(ids("login"), ["b2", "a1"]);
        assert!(ids("nobody").is_empty());
        // Sent in the same millisecond: the one listed later shows first.
        let same = vec![
            message("x", json!({"toSessionId": "s"})),
            message("y", json!({"toSessionId": "s"})),
        ];
        let ids: Vec<&str> = session_log(&same, "s")
            .iter()
            .map(|message| message.id.as_str())
            .collect();
        assert_eq!(ids, ["y", "x"]);
    }

    #[test]
    fn parties_are_you_deckhand_or_the_sessions_title_now() {
        let messages = walkthrough();
        let name = |message: &MessageRecord, session: &str| party_name(&other_party(message, session), &titles);
        // A message sent by hand comes from you; the reply to it goes back to you.
        assert_eq!(name(&messages[2], "tokens"), "you");
        assert_eq!(name(&messages[3], "tokens"), "you");
        assert_eq!(direction(&messages[3], "tokens"), Direction::Sent);
        // Between sessions: each sees the other, by its title now.
        assert_eq!(name(&messages[0], "tokens"), "Login UI");
        assert_eq!(name(&messages[0], "login"), "Token storage");
        assert_eq!(direction(&messages[0], "login"), Direction::Sent);
        assert_eq!(direction(&messages[0], "tokens"), Direction::Received);
        // A session gone: its title as sent, else its short ID.
        let gone = message(
            "f6",
            json!({"fromSessionId": "0123456789ab", "fromTitle": "Old", "toSessionId": "s"}),
        );
        assert_eq!(name(&gone, "s"), "Old");
        let untitled = message("f7", json!({"fromSessionId": "0123456789ab", "toSessionId": "s"}));
        assert_eq!(name(&untitled, "s"), "01234567");
        let notice = message("f8", json!({"kind": "notice", "toSessionId": "s"}));
        assert_eq!(name(&notice, "s"), "Deckhand");
    }

    #[test]
    fn a_log_row_says_what_kind_of_message_and_what_became_of_it() {
        let messages = walkthrough();
        let row = |index: usize, session: &str| log_row(&messages[index], session, noon_plus(6), &titles);
        let asked = row(0, "tokens");
        assert_eq!(asked.direction, Direction::Received);
        assert_eq!(asked.party, "Login UI");
        assert_eq!(asked.kind.as_deref(), Some("wants an answer"));
        assert_eq!(asked.age, "5m");
        assert_eq!(asked.status, MessageStatus::Answered);
        assert_eq!(asked.replies, ["b2"]);
        assert!(!asked.cancellable);
        let returned = row(3, "tokens");
        assert_eq!(
            (returned.direction, returned.party.as_str(), returned.kind.as_deref()),
            (Direction::Sent, "you", Some("reply to c3"))
        );
        assert_eq!(returned.text, "Done.");
        let held = row(4, "tokens");
        assert_eq!(held.kind, None);
        assert!(held.cancellable);
        assert_eq!(held.age, "1m");
        let mut cut = messages[1].clone();
        cut.text_truncated = Some(true);
        cut.note = Some("Typed, but no turn started".to_owned());
        let cut = log_row(&cut, "login", noon_plus(6), &titles);
        assert!(cut.truncated);
        assert_eq!(cut.note.as_deref(), Some("Typed, but no turn started"));
        let notice = message(
            "n",
            json!({"kind": "notice", "toSessionId": "login", "inReplyTo": "a1"}),
        );
        assert_eq!(
            log_row(&notice, "login", noon_plus(6), &titles).kind.as_deref(),
            Some("notice")
        );
    }

    #[test]
    fn the_brief_counts_what_waits_and_names_the_latest_message() {
        let messages = walkthrough();
        let brief = messages_brief(&messages, "tokens", &titles).unwrap_or_else(|| panic!("a brief"));
        assert_eq!(brief.held, 1);
        assert_eq!(brief.last_party, "you");
        assert_eq!(brief.last_status, MessageStatus::Held);
        assert_eq!(
            brief_text(&brief, noon_plus(8), 60),
            "✉ 1 waiting · last from you 3m (held)"
        );
        let login = messages_brief(&messages, "login", &titles).unwrap_or_else(|| panic!("a brief"));
        assert_eq!(login.held, 0);
        assert_eq!(
            brief_text(&login, noon_plus(8), 60),
            "✉ last from Token storage 6m (returned)"
        );
        assert_eq!(messages_brief(&messages, "nobody", &titles), None);
    }

    #[test]
    fn a_held_message_says_why_it_waits() {
        let reason =
            |kind: HeldReasonKind, until: Option<String>| held_reason_text(&HeldReason { kind, until }, noon_plus(0));
        assert_eq!(
            reason(HeldReasonKind::UnsentText, Some(at(4))),
            "unsent text in its input, delivers in 4 min, or press enter"
        );
        assert_eq!(
            reason(HeldReasonKind::UnsentText, Some("2026-11-14T12:00:30.000Z".to_owned())),
            "unsent text in its input, delivers in under a minute, or press enter"
        );
        assert_eq!(
            reason(HeldReasonKind::UnsentText, None),
            "unsent text still in its input: press enter or clear it"
        );
        assert_eq!(
            reason(HeldReasonKind::Stalled, None),
            "an earlier message was typed but no turn started: press enter in the session"
        );
        assert_eq!(
            reason(HeldReasonKind::Working, None),
            "the session is working on a turn"
        );
        // Only the next held message carries it: the brief and its row say it, the others don't.
        let mut messages = walkthrough();
        messages[4].held_reason = Some(HeldReason {
            kind: HeldReasonKind::UnsentText,
            until: Some(at(9)),
        });
        let brief = messages_brief(&messages, "tokens", &titles).unwrap_or_else(|| panic!("a brief"));
        assert_eq!(
            brief.held_reason.map(|reason| reason.kind),
            Some(HeldReasonKind::UnsentText)
        );
        let row = log_row(&messages[4], "tokens", noon_plus(5), &titles);
        assert_eq!(
            row.held.as_deref(),
            Some("unsent text in its input, delivers in 4 min, or press enter")
        );
        assert_eq!(log_row(&messages[3], "tokens", noon_plus(5), &titles).held, None);
        // Delivered, it no longer waits.
        messages[4].status = MessageStatus::Delivered;
        assert_eq!(log_row(&messages[4], "tokens", noon_plus(5), &titles).held, None);
        assert_eq!(held_reason(&messages, "tokens"), None);
    }

    #[test]
    fn a_narrow_brief_drops_the_status_then_cuts_the_party_then_keeps_the_count() {
        let brief = MessagesBrief {
            held: 2,
            held_reason: None,
            last_direction: Direction::Received,
            last_party: "Login UI".to_owned(),
            last_created_at: at(0),
            last_status: MessageStatus::Answered,
        };
        let text = |width| brief_text(&brief, noon_plus(3), width);
        assert_eq!(text(60), "✉ 2 waiting · last from Login UI 3m (answered)");
        assert_eq!(text(40), "✉ 2 waiting · last from Login UI 3m");
        assert_eq!(text(32), "✉ 2 waiting · last from Logi… 3m");
        assert_eq!(text(20), "✉ 2 waiting");
        let none_waiting = MessagesBrief { held: 0, ..brief };
        assert_eq!(brief_text(&none_waiting, noon_plus(3), 12), "✉ last 3m");
    }
}
