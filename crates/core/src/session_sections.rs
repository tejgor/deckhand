//! Not in the TS: the sidebar's default view, sessions in sections by state (needs you, unread, working, idle,
//! exited). Needs you and Unread list the session waiting longest first, Idle the one that went idle last first;
//! Working and Exited keep your order (`sidebarOrder`, `J`/`K`). The time order is only how the view lists them: your
//! order stays as it is. A sub-session sits under its parent (in your order) when both are in the same section, and is
//! listed at the section's top level otherwise.

use std::collections::{HashMap, HashSet};

use crate::display::still_starting;
use crate::session_features::session_has_unread_reply;
use crate::session_order::{SessionLike, parent_of, visit};
use deckhand_protocol::data::{AgentActivityStatus, AttentionState, SessionRecord, SessionStatus};
/// Persisted in ui-state.json, so defined by deckhand-protocol.
pub use deckhand_protocol::records::SessionView;

/// A section of the sidebar's state view, in the order they are listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SessionSection {
    /// Running and blocked on you: a question or permission, a failure or a usage limit.
    NeedsYou,
    /// Running, its turn over and the reply unread.
    Unread,
    /// Starting, or the agent is busy.
    Working,
    /// Running and waiting for a prompt.
    Idle,
    /// Not running.
    Exited,
}

impl SessionSection {
    pub const ALL: [SessionSection; 5] = [
        SessionSection::NeedsYou,
        SessionSection::Unread,
        SessionSection::Working,
        SessionSection::Idle,
        SessionSection::Exited,
    ];

    /// How the section lists its sessions by time ([`arrived_at`]), or `None` for your order.
    pub fn time_order(self) -> Option<TimeOrder> {
        match self {
            SessionSection::NeedsYou | SessionSection::Unread => Some(TimeOrder::OldestFirst),
            SessionSection::Idle => Some(TimeOrder::NewestFirst),
            SessionSection::Working | SessionSection::Exited => None,
        }
    }

    /// The section header's name.
    pub fn label(self) -> &'static str {
        match self {
            SessionSection::NeedsYou => "Needs you",
            SessionSection::Unread => "Unread",
            SessionSection::Working => "Working",
            SessionSection::Idle => "Idle",
            SessionSection::Exited => "Exited",
        }
    }
}

/// How a section orders its sessions by when they arrived in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeOrder {
    /// The one waiting longest first, so a new arrival joins at the bottom (Needs you, Unread).
    OldestFirst,
    /// The latest first, so the session whose reply you just read is on top (Idle).
    NewestFirst,
}

/// When `session` arrived in `section`, as an ISO time (they compare as strings), for the sections listed by time:
/// when it asked for you or ended its turn, or for Idle the later of reading its reply and its agent going idle.
pub fn arrived_at(session: &SessionRecord, section: SessionSection) -> Option<&str> {
    match section {
        SessionSection::NeedsYou | SessionSection::Unread => {
            session.attention.as_ref().map(|attention| attention.at.as_str())
        }
        SessionSection::Idle => session
            .reply_read_at
            .as_deref()
            .max(session.agent_status_updated_at.as_deref()),
        SessionSection::Working | SessionSection::Exited => None,
    }
}

/// The section a session is listed in. A busy agent is working even with an earlier reply unread.
pub fn session_section(session: &SessionRecord) -> SessionSection {
    let running = session.status == SessionStatus::Running;
    let attention = session
        .attention
        .as_ref()
        .map(|attention| attention.state)
        .filter(|_| running);
    if matches!(
        attention,
        Some(AttentionState::NeedsInput | AttentionState::Failed | AttentionState::Limited)
    ) {
        return SessionSection::NeedsYou;
    }
    if session.status == SessionStatus::Starting
        || (running && session.agent_status == Some(AgentActivityStatus::Active))
    {
        return SessionSection::Working;
    }
    if !running {
        return SessionSection::Exited;
    }
    if session_has_unread_reply(session) {
        SessionSection::Unread
    } else if still_starting(session) {
        // Not in the TS: activity not known yet reads as starting.
        SessionSection::Working
    } else {
        SessionSection::Idle
    }
}

/// The sessions of one section, from a list in sidebar order.
pub fn in_section<S: SessionLike>(sessions: &[S], section: SessionSection) -> Vec<&S> {
    sessions
        .iter()
        .filter(|item| session_section(item.session()) == section)
        .collect()
}

/// `sessions` (in sidebar order) grouped by section. Inside each, a session whose parent is in the same section
/// follows it depth first; the others are in the section's [`TimeOrder`], else keep their place in the list (as do
/// sessions that arrived at the same time, or with no time known).
pub fn order_by_section<S: SessionLike>(sessions: &[S]) -> Vec<&S> {
    let mut ordered = Vec::with_capacity(sessions.len());
    for section in SessionSection::ALL {
        let members = in_section(sessions, section);
        let ids: HashSet<&str> = members.iter().map(|item| item.session().id.as_str()).collect();
        let mut children: HashMap<&str, Vec<&S>> = HashMap::new();
        let mut roots = Vec::new();
        for &item in &members {
            match parent_of(item.session()).filter(|parent| ids.contains(parent)) {
                Some(parent) => children.entry(parent).or_default().push(item),
                None => roots.push(item),
            }
        }
        if let Some(order) = section.time_order() {
            roots.sort_by(|a, b| {
                let (a, b) = (arrived_at(a.session(), section), arrived_at(b.session(), section));
                match order {
                    TimeOrder::OldestFirst => a.cmp(&b),
                    TimeOrder::NewestFirst => b.cmp(&a),
                }
            });
        }
        let mut visited: HashSet<&str> = HashSet::new();
        for root in roots {
            visit(root, &children, &mut visited, &mut ordered);
        }
        // Sessions only reachable through a parent cycle.
        for item in members {
            visit(item, &children, &mut visited, &mut ordered);
        }
    }
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_features::tests::partial_session;
    use serde_json::{Value, json};

    fn session(id: &str, fields: Value) -> SessionRecord {
        let mut base = json!({
            "id": id, "title": id, "program": "claude", "command": "claude", "cwd": "/repo", "repoRoot": "/repo",
            "worktree": {"mode": "none"}, "status": "running", "agentStatus": "idle",
            "createdAt": "2026-10-01T00:00:00.000Z", "updatedAt": "2026-10-01T00:00:00.000Z",
        });
        if let (Some(base), Some(fields)) = (base.as_object_mut(), fields.as_object()) {
            base.extend(fields.clone());
        }
        partial_session(&base).expect("a session literal")
    }

    #[test]
    fn each_state_has_its_section() {
        let at = "2026-10-01T00:00:00.000Z";
        let cases = [
            (
                json!({"attention": {"state": "needs-input", "event": "Stop", "at": at}}),
                SessionSection::NeedsYou,
            ),
            (
                json!({"attention": {"state": "limited", "event": "Stop", "at": at}}),
                SessionSection::NeedsYou,
            ),
            (
                json!({"attention": {"state": "response-ended", "event": "Stop", "at": at}}),
                SessionSection::Unread,
            ),
            (
                json!({"attention": {"state": "response-ended", "event": "Stop", "at": at}, "replyReadAt": "2026-10-02T00:00:00.000Z"}),
                SessionSection::Idle,
            ),
            (
                json!({"attention": {"state": "response-ended", "event": "Stop", "at": at}, "agentStatus": "active"}),
                SessionSection::Working,
            ),
            (json!({"agentStatus": "active"}), SessionSection::Working),
            (
                json!({"status": "starting", "agentStatus": null}),
                SessionSection::Working,
            ),
            (json!({}), SessionSection::Idle),
            // Running before its activity is known reads as starting, unless a reply is unread.
            (json!({"agentStatus": "unknown"}), SessionSection::Working),
            (json!({"agentStatus": null}), SessionSection::Working),
            (
                json!({"attention": {"state": "response-ended", "event": "Stop", "at": at}, "agentStatus": "unknown"}),
                SessionSection::Unread,
            ),
            (
                json!({"status": "exited", "attention": {"state": "needs-input", "event": "Stop", "at": at}}),
                SessionSection::Exited,
            ),
        ];
        for (fields, section) in cases {
            let record = session("s", fields.clone());
            let record = crate::session_features::with_reply_read(record);
            assert_eq!(session_section(&record), section, "{fields}");
        }
    }

    #[test]
    fn needs_you_and_unread_list_the_longest_waiting_first_and_idle_the_latest() {
        let attention = |state: &str, at: &str| json!({"state": state, "event": "Stop", "at": at});
        let day = |day: u32| format!("2026-10-{day:02}T00:00:00.000Z");
        let list = vec![
            session("ask-late", json!({"attention": attention("needs-input", &day(5))})),
            session("ask-early", json!({"attention": attention("failed", &day(2))})),
            session("reply-late", json!({"attention": attention("response-ended", &day(6))})),
            session(
                "reply-early",
                json!({"attention": attention("response-ended", &day(3))}),
            ),
            // Idle: when its reply was read, or its agent went idle, whichever came later.
            session(
                "read-early",
                json!({"attention": attention("response-ended", &day(1)), "replyReadAt": day(4), "agentStatusUpdatedAt": day(1)}),
            ),
            session(
                "idle-late",
                json!({"agentStatusUpdatedAt": day(7), "replyReadAt": day(2)}),
            ),
            session(
                "read-late",
                json!({"attention": attention("response-ended", &day(1)), "replyReadAt": day(8), "agentStatusUpdatedAt": day(1)}),
            ),
            // An idle session with no time known goes last.
            session("idle-unknown", json!({})),
            // Working and Exited keep your order.
            session(
                "work-b",
                json!({"agentStatus": "active", "agentStatusUpdatedAt": day(9)}),
            ),
            session(
                "work-a",
                json!({"agentStatus": "active", "agentStatusUpdatedAt": day(1)}),
            ),
            session("gone-b", json!({"status": "exited", "agentStatusUpdatedAt": day(1)})),
            session("gone-a", json!({"status": "exited", "agentStatusUpdatedAt": day(9)})),
        ];
        let list: Vec<SessionRecord> = list.into_iter().map(crate::session_features::with_reply_read).collect();
        let ids: Vec<&str> = order_by_section(&list)
            .into_iter()
            .map(|item| item.id.as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "ask-early",
                "ask-late",
                "reply-early",
                "reply-late",
                "work-b",
                "work-a",
                "read-late",
                "idle-late",
                "read-early",
                "idle-unknown",
                "gone-b",
                "gone-a",
            ]
        );
    }

    #[test]
    fn sections_keep_your_order_and_nest_only_within_a_section() {
        // p (idle) has c1 (working) and c2 (idle); g (idle) is c1's child; w (working) is a top-level session.
        let list = vec![
            session("p", json!({})),
            session("c1", json!({"parentSessionId": "p", "agentStatus": "active"})),
            session("g", json!({"parentSessionId": "c1"})),
            session("c2", json!({"parentSessionId": "p"})),
            session("w", json!({"agentStatus": "active"})),
            session("x", json!({"status": "exited"})),
        ];
        let ids: Vec<&str> = order_by_section(&list)
            .into_iter()
            .map(|item| item.id.as_str())
            .collect();
        // Working: c1 (its parent is idle), then w. Idle: p with c2 under it, then g (its parent works).
        assert_eq!(ids, ["c1", "w", "p", "c2", "g", "x"]);
    }
}
