//! Port of sessionOrder.ts.
//!
//! Session lists: functions that take a list of sessions are generic over [`SessionLike`], so the daemon can pass
//! its `Ordered<SessionRecord>`s, the UI its `SessionRecord`s, and either can pass a `Vec<&_>` a previous call
//! returned. Functions that return part of a list return references into it (TS returns the same objects).

use std::cmp::Ordering;
use std::collections::HashSet;

use deckhand_protocol::Ordered;
use deckhand_protocol::data::SessionRecord;

/// Anything that is (or holds) a session record: `SessionRecord`, `Ordered<SessionRecord>`, or a reference to one.
pub trait SessionLike {
    fn session(&self) -> &SessionRecord;
}

impl SessionLike for SessionRecord {
    fn session(&self) -> &SessionRecord {
        self
    }
}

impl SessionLike for Ordered<SessionRecord> {
    fn session(&self) -> &SessionRecord {
        self
    }
}

impl<T: SessionLike + ?Sized> SessionLike for &T {
    fn session(&self) -> &SessionRecord {
        (**self).session()
    }
}

/// TS: sessionOrder.ts orderValue (a finite `sidebarOrder`, else 0).
pub fn order_value(session: &SessionRecord) -> i64 {
    let _ = session;
    todo!("P2")
}

/// TS: sessionOrder.ts compareSessionOrder (`sidebarOrder`, then `createdAt`, then `id`, both by localeCompare).
pub fn compare_session_order(a: &SessionRecord, b: &SessionRecord) -> Ordering {
    let _ = (a, b);
    todo!("P2")
}

/// TS: sessionOrder.ts sortSessionsForSidebar (parents, each followed by its children, depth first).
pub fn sort_sessions_for_sidebar<S: SessionLike>(sessions: &[S]) -> Vec<&S> {
    let _ = sessions;
    todo!("P2")
}

/// TS: sessionOrder.ts sessionDepth.
pub fn session_depth<S: SessionLike>(session: &SessionRecord, sessions: &[S]) -> usize {
    let _ = (session, sessions);
    todo!("P2")
}

/// TS: sessionOrder.ts sessionHasChildren.
pub fn session_has_children<S: SessionLike>(session_id: &str, sessions: &[S]) -> bool {
    let _ = (session_id, sessions);
    todo!("P2")
}

/// TS: sessionOrder.ts sessionDescendants (every descendant, cycles cut).
pub fn session_descendants<'a, S: SessionLike>(session_id: &str, sessions: &'a [S]) -> Vec<&'a S> {
    let _ = (session_id, sessions);
    todo!("P2")
}

/// TS: sessionOrder.ts countSessionDescendants.
pub fn count_session_descendants<S: SessionLike>(session_id: &str, sessions: &[S]) -> usize {
    let _ = (session_id, sessions);
    todo!("P2")
}

/// TS: sessionOrder.ts countHiddenSessionDescendants.
pub fn count_hidden_session_descendants<S: SessionLike>(
    session_id: &str,
    sessions: &[S],
    hidden_session_ids: &HashSet<String>,
) -> usize {
    let _ = (session_id, sessions, hidden_session_ids);
    todo!("P2")
}

/// TS: sessionOrder.ts filterCollapsedSessions (`hiddenSessionIds` defaults to none).
pub fn filter_collapsed_sessions<'a, S: SessionLike>(
    sessions: &'a [S],
    collapsed_session_ids: &HashSet<String>,
    hidden_session_ids: Option<&HashSet<String>>,
) -> Vec<&'a S> {
    let _ = (sessions, collapsed_session_ids, hidden_session_ids);
    todo!("P2")
}
