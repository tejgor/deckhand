//! Session lists: functions that take a list of sessions are generic over [`SessionLike`], so the daemon can pass
//! its `Ordered<SessionRecord>`s, the UI its `SessionRecord`s, and either can pass a `Vec<&_>` a previous call
//! returned. Functions that return part of a list return references into it (TS returns the same objects).

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use crate::jsstr::locale_compare;
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

/// A JS-truthy parent id (set and not empty).
pub(crate) fn parent_of(session: &SessionRecord) -> Option<&str> {
    session.parent_session_id.as_deref().filter(|id| !id.is_empty())
}

/// `new Map(sessions.map(session => [session.id, session]))`: the last session with an id wins.
fn by_id<S: SessionLike>(sessions: &[S]) -> HashMap<&str, &SessionRecord> {
    sessions
        .iter()
        .map(|item| (item.session().id.as_str(), item.session()))
        .collect()
}

/// TS: sessionOrder.ts orderValue (a finite `sidebarOrder`, else 0; fractions kept).
pub fn order_value(session: &SessionRecord) -> f64 {
    session
        .sidebar_order
        .map(|order| order.0)
        .filter(|order| order.is_finite())
        .unwrap_or(0.0)
}

/// The `orderValue(a) - orderValue(b)` of compareSessionOrder (finite operands: never NaN, 0 only when equal).
pub fn order_delta(a: &SessionRecord, b: &SessionRecord) -> f64 {
    order_value(a) - order_value(b)
}

/// TS: sessionOrder.ts compareSessionOrder (`sidebarOrder`, then `createdAt`, then `id`, both by localeCompare).
pub fn compare_session_order(a: &SessionRecord, b: &SessionRecord) -> Ordering {
    let delta = order_delta(a, b);
    let by_order = if delta > 0.0 {
        Ordering::Greater
    } else if delta < 0.0 {
        Ordering::Less
    } else {
        Ordering::Equal
    };
    by_order
        .then_with(|| locale_compare(&a.created_at, &b.created_at))
        .then_with(|| locale_compare(&a.id, &b.id))
}

/// TS sortSessionsForSidebar's `visit`: `start`, then its unvisited descendants depth first (pre-order), iteratively
/// so that a deep parent chain cannot overflow the stack.
pub(crate) fn visit<'a, S: SessionLike>(
    start: &'a S,
    children: &HashMap<&str, Vec<&'a S>>,
    visited: &mut HashSet<&'a str>,
    ordered: &mut Vec<&'a S>,
) {
    if !visited.insert(start.session().id.as_str()) {
        return;
    }
    ordered.push(start);
    let mut stack: Vec<(&'a str, usize)> = vec![(start.session().id.as_str(), 0)];
    while let Some((id, next)) = stack.last_mut() {
        let child = children.get(*id).and_then(|kids| kids.get(*next)).copied();
        *next += 1;
        match child {
            Some(child) => {
                if visited.insert(child.session().id.as_str()) {
                    ordered.push(child);
                    stack.push((child.session().id.as_str(), 0));
                }
            }
            None => {
                stack.pop();
            }
        }
    }
}

/// TS: sessionOrder.ts sortSessionsForSidebar (parents, each followed by its children, depth first).
pub fn sort_sessions_for_sidebar<S: SessionLike>(sessions: &[S]) -> Vec<&S> {
    let ids = by_id(sessions);
    let mut children: HashMap<&str, Vec<&S>> = HashMap::new();
    let mut roots: Vec<&S> = Vec::new();
    for item in sessions {
        match parent_of(item.session()).filter(|parent| ids.contains_key(parent)) {
            Some(parent) => children.entry(parent).or_default().push(item),
            None => roots.push(item),
        }
    }
    let compare = |a: &&S, b: &&S| compare_session_order(a.session(), b.session());
    roots.sort_by(compare);
    for siblings in children.values_mut() {
        siblings.sort_by(compare);
    }

    let mut ordered: Vec<&S> = Vec::with_capacity(sessions.len());
    let mut visited: HashSet<&str> = HashSet::new();
    for root in roots {
        visit(root, &children, &mut visited, &mut ordered);
    }
    // Sessions only reachable through a parent cycle.
    let mut all: Vec<&S> = sessions.iter().collect();
    all.sort_by(compare);
    for item in all {
        visit(item, &children, &mut visited, &mut ordered);
    }
    ordered
}

/// TS: sessionOrder.ts sessionDepth.
pub fn session_depth<S: SessionLike>(session: &SessionRecord, sessions: &[S]) -> usize {
    let ids = by_id(sessions);
    let mut depth = 0;
    let mut parent_id = parent_of(session);
    let mut seen: HashSet<&str> = HashSet::from([session.id.as_str()]);
    while let Some(parent) = parent_id.filter(|parent| ids.contains_key(parent) && !seen.contains(parent)) {
        seen.insert(parent);
        depth += 1;
        parent_id = ids.get(parent).and_then(|record| parent_of(record));
    }
    depth
}

/// TS: sessionOrder.ts sessionHasChildren.
pub fn session_has_children<S: SessionLike>(session_id: &str, sessions: &[S]) -> bool {
    sessions
        .iter()
        .any(|item| item.session().parent_session_id.as_deref() == Some(session_id))
}

/// TS: sessionOrder.ts childSessionMap.
fn child_session_map<S: SessionLike>(sessions: &[S]) -> HashMap<&str, Vec<&S>> {
    let mut children: HashMap<&str, Vec<&S>> = HashMap::new();
    for item in sessions {
        if let Some(parent) = parent_of(item.session()) {
            children.entry(parent).or_default().push(item);
        }
    }
    children
}

/// TS: sessionOrder.ts sessionDescendants (every descendant, cycles cut).
pub fn session_descendants<'a, S: SessionLike>(session_id: &str, sessions: &'a [S]) -> Vec<&'a S> {
    let children = child_session_map(sessions);
    let mut descendants: Vec<&'a S> = Vec::new();
    let mut stack: Vec<&'a S> = children.get(session_id).cloned().unwrap_or_default();
    let mut seen: HashSet<&str> = HashSet::from([session_id]);
    while let Some(child) = stack.pop() {
        let id = child.session().id.as_str();
        if !seen.insert(id) {
            continue;
        }
        descendants.push(child);
        stack.extend(children.get(id).into_iter().flatten().copied());
    }
    descendants
}

/// TS: sessionOrder.ts countSessionDescendants.
pub fn count_session_descendants<S: SessionLike>(session_id: &str, sessions: &[S]) -> usize {
    session_descendants(session_id, sessions).len()
}

/// TS: sessionOrder.ts countHiddenSessionDescendants.
pub fn count_hidden_session_descendants<S: SessionLike>(
    session_id: &str,
    sessions: &[S],
    hidden_session_ids: &HashSet<String>,
) -> usize {
    if hidden_session_ids.is_empty() {
        return 0;
    }
    session_descendants(session_id, sessions)
        .into_iter()
        .filter(|item| hidden_session_ids.contains(&item.session().id))
        .count()
}

/// TS: sessionOrder.ts filterCollapsedSessions (`hiddenSessionIds` defaults to none).
pub fn filter_collapsed_sessions<'a, S: SessionLike>(
    sessions: &'a [S],
    collapsed_session_ids: &HashSet<String>,
    hidden_session_ids: Option<&HashSet<String>>,
) -> Vec<&'a S> {
    let none = HashSet::new();
    let hidden = hidden_session_ids.unwrap_or(&none);
    if collapsed_session_ids.is_empty() && hidden.is_empty() {
        return sessions.iter().collect();
    }
    let ids = by_id(sessions);
    sessions
        .iter()
        .filter(|item| {
            let session = item.session();
            if hidden.contains(&session.id) {
                return false;
            }
            let mut parent_id = parent_of(session);
            let mut seen: HashSet<&str> = HashSet::from([session.id.as_str()]);
            while let Some(parent) = parent_id.filter(|parent| ids.contains_key(parent) && !seen.contains(parent)) {
                if collapsed_session_ids.contains(parent) || hidden.contains(parent) {
                    return false;
                }
                seen.insert(parent);
                parent_id = ids.get(parent).and_then(|record| parent_of(record));
            }
            true
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use deckhand_protocol::data::JsNumber;
    use deckhand_testkit::{Case, replay};

    fn sessions(case: &Case, index: usize) -> Result<Vec<Ordered<SessionRecord>>, String> {
        case.arg_as(index)
    }

    fn id_set(case: &Case, index: usize) -> Result<HashSet<String>, String> {
        case.arg_as::<Vec<String>>(index).map(|ids| ids.into_iter().collect())
    }

    #[test]
    fn compare_session_order_parity() {
        replay("sessionOrder/compareSessionOrder", |case: &Case| {
            let a: SessionRecord = case.arg_as(0)?;
            let b: SessionRecord = case.arg_as(1)?;
            // TS returns the raw `sidebarOrder` difference when it is not zero, else localeCompare's -1/0/1.
            let ordering = compare_session_order(&a, &b);
            let delta = order_delta(&a, &b);
            #[allow(clippy::cast_precision_loss)]
            Ok::<_, String>(JsNumber(if delta != 0.0 { delta } else { ordering as i64 as f64 }))
        });
    }

    #[test]
    fn sort_sessions_for_sidebar_parity() {
        replay("sessionOrder/sortSessionsForSidebar", |case: &Case| {
            let list = sessions(case, 0)?;
            Ok::<_, String>(
                sort_sessions_for_sidebar(&list)
                    .into_iter()
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        });
    }

    #[test]
    fn session_depth_parity() {
        replay("sessionOrder/sessionDepth", |case: &Case| {
            let session: SessionRecord = case.arg_as(0)?;
            Ok::<_, String>(session_depth(&session, &sessions(case, 1)?))
        });
    }

    #[test]
    fn session_has_children_parity() {
        replay("sessionOrder/sessionHasChildren", |case: &Case| {
            Ok::<_, String>(session_has_children(case.arg_str(0)?, &sessions(case, 1)?))
        });
    }

    #[test]
    fn session_descendants_parity() {
        replay("sessionOrder/sessionDescendants", |case: &Case| {
            let list = sessions(case, 1)?;
            Ok::<_, String>(
                session_descendants(case.arg_str(0)?, &list)
                    .into_iter()
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        });
    }

    #[test]
    fn count_session_descendants_parity() {
        replay("sessionOrder/countSessionDescendants", |case: &Case| {
            Ok::<_, String>(count_session_descendants(case.arg_str(0)?, &sessions(case, 1)?))
        });
    }

    #[test]
    fn count_hidden_session_descendants_parity() {
        replay("sessionOrder/countHiddenSessionDescendants", |case: &Case| {
            Ok::<_, String>(count_hidden_session_descendants(
                case.arg_str(0)?,
                &sessions(case, 1)?,
                &id_set(case, 2)?,
            ))
        });
    }

    #[test]
    fn filter_collapsed_sessions_parity() {
        replay("sessionOrder/filterCollapsedSessions", |case: &Case| {
            let list = sessions(case, 0)?;
            let collapsed = id_set(case, 1)?;
            let hidden = if case.arg_is_undefined(2) {
                None
            } else {
                Some(id_set(case, 2)?)
            };
            Ok::<_, String>(
                filter_collapsed_sessions(&list, &collapsed, hidden.as_ref())
                    .into_iter()
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        });
    }

    #[test]
    fn fractional_sidebar_orders_sort_and_round_trip() {
        let make = |id: &str, order: serde_json::Value| -> Ordered<SessionRecord> {
            serde_json::from_value(serde_json::json!({
                "id": id, "title": "t", "program": "claude", "command": "claude", "cwd": "/r", "repoRoot": "/r",
                "status": "running", "createdAt": "2026-01-01T00:00:00.000Z", "updatedAt": "2026-01-01T00:00:00.000Z",
                "sidebarOrder": order,
            }))
            .unwrap()
        };
        let list = vec![
            make("a", serde_json::json!(1.5)),
            make("b", serde_json::json!(1.25)),
            make("c", serde_json::json!(1)),
            make("d", serde_json::json!(-0.5)),
        ];
        let ids: Vec<&str> = sort_sessions_for_sidebar(&list)
            .into_iter()
            .map(|item| item.id.as_str())
            .collect();
        assert_eq!(ids, ["d", "c", "b", "a"]);
        assert_eq!(order_delta(&list[0], &list[1]), 0.25);
        let written: Vec<String> = list
            .iter()
            .map(|item| serde_json::to_value(item).unwrap()["sidebarOrder"].to_string())
            .collect();
        assert_eq!(written, ["1.5", "1.25", "1", "-0.5"]);
        // A non-finite order (only possible in memory) reads as 0, as Number.isFinite rejects it.
        let mut infinite = list[2].clone();
        infinite.sidebar_order = Some(JsNumber(f64::INFINITY));
        assert_eq!(order_value(&infinite), 0.0);
    }

    #[test]
    fn deep_parent_chain_does_not_overflow() {
        let make = |id: String, parent: Option<String>| -> SessionRecord {
            serde_json::from_value(serde_json::json!({
                "id": id, "title": "t", "program": "claude", "command": "claude", "cwd": "/r", "repoRoot": "/r",
                "status": "running", "createdAt": "2026-01-01T00:00:00.000Z", "updatedAt": "2026-01-01T00:00:00.000Z",
                "parentSessionId": parent,
            }))
            .unwrap()
        };
        let list: Vec<SessionRecord> = (0..200_000)
            .map(|index| make(format!("s{index}"), (index > 0).then(|| format!("s{}", index - 1))))
            .collect();
        let sorted = sort_sessions_for_sidebar(&list);
        assert_eq!(sorted.len(), list.len());
        assert_eq!(sorted.last().map(|item| item.id.as_str()), Some("s199999"));
    }
}
