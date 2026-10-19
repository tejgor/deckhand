//! Port of worktreeRecords.ts.
//!
//! Merge and deletion markers describe a linked worktree, which several sessions can share. They live in one
//! WorktreeRecord per worktree incarnation (state.json `worktrees`); each session in it stores only the record's ID
//! (`worktree.id`). The daemon projects the record's markers into the sessions it holds and sends
//! ([`project_worktree`]) and strips them again before saving ([`stored_session`]).
//!
//! Sessions and records here are `Ordered` because these functions build what state.json and the wire carry: the
//! projected `worktree` object is `{...withoutMarkers(worktree), ...markers}` (markers last, in marker order).

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use serde::Serialize;

use crate::jsstr::{date, path};
use deckhand_protocol::data::{
    SessionRecord, SessionWorktreeMode, SessionWorktreeRecord, WorktreeMarkers, WorktreeMergeMode, WorktreeMode,
};
use deckhand_protocol::records::WorktreeRecord;
use deckhand_protocol::{KeyOrder, Ordered};

/// TS: worktreeRecords.ts WORKTREE_MARKERS entries: one merge or deletion marker key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorktreeMarker {
    MergedAt,
    MergeMode,
    MergeTargetBranch,
    MergeSourceRef,
    MergeSourceSha,
    MergeMarkedManually,
    MergeDetected,
    DeletedAt,
}

impl WorktreeMarker {
    /// The JSON key (`mergedAt`, ...).
    pub fn as_str(self) -> &'static str {
        match self {
            WorktreeMarker::MergedAt => "mergedAt",
            WorktreeMarker::MergeMode => "mergeMode",
            WorktreeMarker::MergeTargetBranch => "mergeTargetBranch",
            WorktreeMarker::MergeSourceRef => "mergeSourceRef",
            WorktreeMarker::MergeSourceSha => "mergeSourceSha",
            WorktreeMarker::MergeMarkedManually => "mergeMarkedManually",
            WorktreeMarker::MergeDetected => "mergeDetected",
            WorktreeMarker::DeletedAt => "deletedAt",
        }
    }
}

/// TS: worktreeRecords.ts WORKTREE_MARKERS.
pub const WORKTREE_MARKERS: [WorktreeMarker; 8] = [
    WorktreeMarker::MergedAt,
    WorktreeMarker::MergeMode,
    WorktreeMarker::MergeTargetBranch,
    WorktreeMarker::MergeSourceRef,
    WorktreeMarker::MergeSourceSha,
    WorktreeMarker::MergeMarkedManually,
    WorktreeMarker::MergeDetected,
    WorktreeMarker::DeletedAt,
];

/// TS: worktreeRecords.ts MERGE_MARKERS (every marker but `deletedAt`).
pub const MERGE_MARKERS: [WorktreeMarker; 7] = [
    WorktreeMarker::MergedAt,
    WorktreeMarker::MergeMode,
    WorktreeMarker::MergeTargetBranch,
    WorktreeMarker::MergeSourceRef,
    WorktreeMarker::MergeSourceSha,
    WorktreeMarker::MergeMarkedManually,
    WorktreeMarker::MergeDetected,
];

/// The daemon's worktree records by ID (TS `ReadonlyMap<string, WorktreeRecord>`).
pub type WorktreeRecords = IndexMap<String, Ordered<WorktreeRecord>>;

/// A JS-truthy optional string (set and not empty).
fn truthy(value: Option<&str>) -> bool {
    value.is_some_and(|text| !text.is_empty())
}

/// The session's `worktree.id`, when truthy.
fn worktree_id(session: &SessionRecord) -> Option<&str> {
    session
        .worktree
        .as_ref()
        .and_then(|worktree| worktree.id.as_deref())
        .filter(|id| !id.is_empty())
}

/// TS: worktreeRecords.ts worktreeMarkers, for a session's `worktree` (records have `WorktreeRecord::markers`).
pub fn worktree_markers(source: &SessionWorktreeRecord) -> WorktreeMarkers {
    WorktreeMarkers {
        merged_at: source.merged_at.clone(),
        merge_mode: source.merge_mode,
        merge_target_branch: source.merge_target_branch.clone(),
        merge_source_ref: source.merge_source_ref.clone(),
        merge_source_sha: source.merge_source_sha.clone(),
        merge_marked_manually: source.merge_marked_manually,
        merge_detected: source.merge_detected,
        deleted_at: source.deleted_at.clone(),
    }
}

/// Sets `marker` on `target` to `source`'s value.
fn copy_marker(target: &mut SessionWorktreeRecord, source: &WorktreeMarkers, marker: WorktreeMarker) {
    match marker {
        WorktreeMarker::MergedAt => target.merged_at.clone_from(&source.merged_at),
        WorktreeMarker::MergeMode => target.merge_mode = source.merge_mode,
        WorktreeMarker::MergeTargetBranch => target.merge_target_branch.clone_from(&source.merge_target_branch),
        WorktreeMarker::MergeSourceRef => target.merge_source_ref.clone_from(&source.merge_source_ref),
        WorktreeMarker::MergeSourceSha => target.merge_source_sha.clone_from(&source.merge_source_sha),
        WorktreeMarker::MergeMarkedManually => target.merge_marked_manually = source.merge_marked_manually,
        WorktreeMarker::MergeDetected => target.merge_detected = source.merge_detected,
        WorktreeMarker::DeletedAt => target.deleted_at.clone_from(&source.deleted_at),
    }
}

/// Whether `marker` is set on `markers`.
fn has_marker(markers: &WorktreeMarkers, marker: WorktreeMarker) -> bool {
    match marker {
        WorktreeMarker::MergedAt => markers.merged_at.is_some(),
        WorktreeMarker::MergeMode => markers.merge_mode.is_some(),
        WorktreeMarker::MergeTargetBranch => markers.merge_target_branch.is_some(),
        WorktreeMarker::MergeSourceRef => markers.merge_source_ref.is_some(),
        WorktreeMarker::MergeSourceSha => markers.merge_source_sha.is_some(),
        WorktreeMarker::MergeMarkedManually => markers.merge_marked_manually.is_some(),
        WorktreeMarker::MergeDetected => markers.merge_detected.is_some(),
        WorktreeMarker::DeletedAt => markers.deleted_at.is_some(),
    }
}

/// TS: worktreeRecords.ts withoutMarkers (a copy without `keys`; callers pass `&WORKTREE_MARKERS` for the default).
pub fn without_markers(source: &SessionWorktreeRecord, keys: &[WorktreeMarker]) -> SessionWorktreeRecord {
    let mut copy = source.clone();
    let none = WorktreeMarkers::default();
    for &key in keys {
        copy_marker(&mut copy, &none, key);
    }
    copy
}

/// TS: worktreeRecords.ts ownWorktreePath (the resolved root of a managed/attached linked worktree).
pub fn own_worktree_path(worktree: Option<&SessionWorktreeRecord>) -> Option<String> {
    let worktree = worktree?;
    if worktree.mode == SessionWorktreeMode::None || worktree.is_main == Some(true) {
        return None;
    }
    worktree
        .path
        .as_deref()
        .filter(|root| !root.is_empty())
        .map(path::resolve)
}

/// TS: worktreeRecords.ts liveWorktreeRecord (the not-deleted record at `worktree_path`, compared lexically).
pub fn live_worktree_record<'a, I>(records: I, worktree_path: &str) -> Option<&'a WorktreeRecord>
where
    I: IntoIterator<Item = &'a WorktreeRecord>,
{
    let key = path::resolve(worktree_path);
    records
        .into_iter()
        .find(|record| !truthy(record.deleted_at.as_deref()) && path::resolve(&record.path) == key)
}

// ---------------------------------------------------------------------------------------------------------------
// Key order: TS builds these records by spreading, so where a key lands depends on the record's history.

/// The top-level keys of `value` in the order it serialises with under `order`.
fn object_keys<T: Serialize>(order: &KeyOrder, value: &T) -> Vec<(String, KeyOrder)> {
    if let KeyOrder::Object(keys) = order {
        return keys.clone();
    }
    match serde_json::to_value(value).map(|json| KeyOrder::of(&json)) {
        Ok(KeyOrder::Object(keys)) => keys,
        _ => Vec::new(),
    }
}

/// `session` (built from `old`) with `old`'s key order, edited: `top` edits the session's keys, `worktree` the keys
/// of its `worktree` object. A `worktree` the old session did not have is added last, as a JS assignment would.
fn reorder(
    session: SessionRecord,
    old: &Ordered<SessionRecord>,
    top: impl FnOnce(&mut Vec<(String, KeyOrder)>),
    worktree: impl FnOnce(&mut Vec<(String, KeyOrder)>),
) -> Ordered<SessionRecord> {
    let mut keys = object_keys(old.key_order(), &**old);
    top(&mut keys);
    if let Some(value) = session.worktree.as_ref() {
        let at = match keys.iter().position(|(key, _)| key == "worktree") {
            Some(at) => at,
            None => {
                keys.push(("worktree".into(), KeyOrder::Unrecorded));
                keys.len() - 1
            }
        };
        if let Some((_, child)) = keys.get_mut(at) {
            let mut inner = match old.worktree.as_ref() {
                Some(previous) => object_keys(child, previous),
                // `{...(undefined ?? {mode: 'none'}), id}`: the new object's own literal order.
                None => object_keys(&KeyOrder::Unrecorded, value),
            };
            worktree(&mut inner);
            *child = KeyOrder::Object(inner);
        }
    }
    Ordered::with_order(session, KeyOrder::Object(keys))
}

fn drop_keys(keys: &mut Vec<(String, KeyOrder)>, names: &[&str]) {
    keys.retain(|(key, _)| !names.contains(&key.as_str()));
}

/// Records `name` as assigned now: a new key goes last, a known one keeps its place.
fn push_key(keys: &mut Vec<(String, KeyOrder)>, name: &str) {
    if !keys.iter().any(|(key, _)| key == name) {
        keys.push((name.to_owned(), KeyOrder::Unrecorded));
    }
}

fn marker_names(markers: &[WorktreeMarker]) -> Vec<&'static str> {
    markers.iter().map(|marker| marker.as_str()).collect()
}

/// TS: worktreeRecords.ts projectWorktree (the session as the daemon holds and sends it).
pub fn project_worktree(session: &Ordered<SessionRecord>, records: &WorktreeRecords) -> Ordered<SessionRecord> {
    let Some(id) = worktree_id(session) else {
        return session.clone();
    };
    let markers = records.get(id).map(|record| record.markers()).unwrap_or_default();
    let mut projected = (**session).clone();
    if let Some(worktree) = projected.worktree.as_mut() {
        for marker in WORKTREE_MARKERS {
            copy_marker(worktree, &markers, marker);
        }
    }
    reorder(
        projected,
        session,
        |_| {},
        |keys| {
            drop_keys(keys, &marker_names(&WORKTREE_MARKERS));
            for marker in WORKTREE_MARKERS {
                if has_marker(&markers, marker) {
                    push_key(keys, marker.as_str());
                }
            }
        },
    )
}

/// TS: worktreeRecords.ts storedSession (the session as persisted: projected markers dropped).
pub fn stored_session(session: &Ordered<SessionRecord>) -> Ordered<SessionRecord> {
    if worktree_id(session).is_none() {
        return session.clone();
    }
    let mut stored = (**session).clone();
    if let Some(worktree) = stored.worktree.as_mut() {
        *worktree = without_markers(worktree, &WORKTREE_MARKERS);
    }
    reorder(
        stored,
        session,
        |_| {},
        |keys| drop_keys(keys, &marker_names(&WORKTREE_MARKERS)),
    )
}

/// TS: worktreeRecords.ts time (`Date.parse`, 0 for a missing or invalid date).
fn time(value: Option<&str>) -> i64 {
    date::parse(value.unwrap_or("")).unwrap_or(0)
}

/// TS: worktreeRecords.ts MergeSource: a session's merge marker, from its own worktree or (a sub-session) top-level.
struct MergeSource {
    merged_at: Option<String>,
    merge_mode: Option<WorktreeMergeMode>,
    merge_target_branch: Option<String>,
    merge_source_ref: Option<String>,
    merge_marked_manually: Option<bool>,
}

impl MergeSource {
    fn of(session: &SessionRecord) -> Self {
        match session
            .worktree
            .as_ref()
            .filter(|worktree| worktree.mode != SessionWorktreeMode::None)
        {
            Some(worktree) => MergeSource {
                merged_at: worktree.merged_at.clone(),
                merge_mode: worktree.merge_mode,
                merge_target_branch: worktree.merge_target_branch.clone(),
                merge_source_ref: worktree.merge_source_ref.clone(),
                merge_marked_manually: worktree.merge_marked_manually,
            },
            None => MergeSource {
                merged_at: session.merged_at.clone(),
                // Not a SessionRecord field: only an unknown key read back from state.json can carry it.
                merge_mode: session
                    .extra
                    .get("mergeMode")
                    .and_then(|value| serde_json::from_value(value.clone()).ok()),
                merge_target_branch: session.merge_target_branch.clone(),
                merge_source_ref: session.merge_source_ref.clone(),
                merge_marked_manually: session.merge_marked_manually,
            },
        }
    }
}

/// TS: worktreeRecords.ts SESSION_MERGE_FIELDS (the top-level merge marker keys of old sessions).
const SESSION_MERGE_FIELDS: [&str; 4] = ["mergedAt", "mergeTargetBranch", "mergeSourceRef", "mergeMarkedManually"];

fn clear_session_merge_fields(session: &mut SessionRecord) {
    session.merged_at = None;
    session.merge_target_branch = None;
    session.merge_source_ref = None;
    session.merge_marked_manually = None;
}

fn has_session_merge_fields(session: &SessionRecord) -> bool {
    session.merged_at.is_some()
        || session.merge_target_branch.is_some()
        || session.merge_source_ref.is_some()
        || session.merge_marked_manually.is_some()
}

/// TS: worktreeRecords.ts migrateWorktreeRecords result.
#[derive(Clone, Debug, PartialEq)]
pub struct WorktreeMigration {
    pub sessions: Vec<Ordered<SessionRecord>>,
    pub worktrees: Vec<Ordered<WorktreeRecord>>,
    pub changed: bool,
}

/// TS: worktreeRecords.ts migrateWorktreeRecords (`new_id`: TS `randomUUID` by default).
///
/// Lifts per-session worktree markers (state written before worktree records) into records, and makes the stored
/// state consistent: every referenced record exists, no session stores projected markers, unreferenced records are
/// dropped.
///
/// Legacy sessions are grouped by worktree root: their own linked worktree, or, for a session without one (mode
/// `none`, e.g. a sub-session), its launch checkout when that is a linked worktree some session owns. Within a root,
/// each recorded deletion (`worktree.deletedAt`) ends one incarnation: the session that deleted it belongs to it, any
/// other to the first incarnation deleted at or after its last launch (`agentStartedAt`, else `createdAt`), and
/// sessions launched after the last deletion share the live one. An incarnation takes the most recent merge marker of
/// its sessions (a sub-session's top-level `M` marker included).
pub fn migrate_worktree_records(
    sessions: Vec<Ordered<SessionRecord>>,
    worktrees: Vec<Ordered<WorktreeRecord>>,
    new_id: &mut dyn FnMut() -> String,
) -> WorktreeMigration {
    let given = worktrees.len();
    let mut records: WorktreeRecords = IndexMap::new();
    for record in worktrees {
        // Like `Map.set`, a known id keeps its place and takes the later record.
        records.insert(record.id.clone(), record);
    }
    let mut changed = records.len() != given;
    let mut linked: HashSet<String> = records.values().map(|record| path::resolve(&record.path)).collect();
    for session in &sessions {
        if let Some(root) = own_worktree_path(session.worktree.as_ref()) {
            linked.insert(root);
        }
    }
    let legacy_root = |session: &SessionRecord| -> Option<String> {
        if worktree_id(session).is_some() {
            return None;
        }
        let own = own_worktree_path(session.worktree.as_ref());
        if own.is_some()
            || session
                .worktree
                .as_ref()
                .is_some_and(|worktree| worktree.mode != SessionWorktreeMode::None)
        {
            return own;
        }
        if session
            .requested_worktree_mode
            .is_some_and(|mode| mode != WorktreeMode::None)
        {
            return None;
        }
        let launch = path::resolve(session.launch_worktree_root.as_deref().unwrap_or(&session.cwd));
        linked.contains(&launch).then_some(launch)
    };

    let mut groups: IndexMap<String, Vec<&SessionRecord>> = IndexMap::new();
    for session in &sessions {
        if let Some(worktree) = session.worktree.as_ref()
            && let Some(id) = worktree_id(session)
            && !records.contains_key(id)
        {
            // A reference without its record (edited state): rebuild it from what the session still has.
            let markers = worktree_markers(worktree);
            let record = WorktreeRecord {
                id: id.to_owned(),
                path: worktree
                    .path
                    .clone()
                    .or_else(|| session.launch_worktree_root.clone())
                    .unwrap_or_else(|| session.cwd.clone()),
                created_at: session.created_at.clone(),
                base_sha: None,
                merged_at: markers.merged_at,
                merge_mode: markers.merge_mode,
                merge_target_branch: markers.merge_target_branch,
                merge_source_ref: markers.merge_source_ref,
                merge_source_sha: markers.merge_source_sha,
                merge_marked_manually: markers.merge_marked_manually,
                merge_detected: markers.merge_detected,
                deleted_at: markers.deleted_at,
                merge_dismissed_tip: None,
                extra: IndexMap::new(),
            };
            records.insert(id.to_owned(), Ordered::new(record));
            changed = true;
        }
        if let Some(root) = legacy_root(session) {
            groups.entry(root).or_default().push(session);
        }
    }

    let mut assigned: HashMap<String, String> = HashMap::new();
    for (root, group) in &groups {
        let mut deletions: Vec<&str> = Vec::new();
        for session in group {
            if let Some(deleted_at) = session
                .worktree
                .as_ref()
                .and_then(|worktree| worktree.deleted_at.as_deref())
                .filter(|deleted_at| !deleted_at.is_empty())
                && !deletions.contains(&deleted_at)
            {
                deletions.push(deleted_at);
            }
        }
        deletions.sort_by_key(|deletion| time(Some(deletion)));
        let last_launch =
            |session: &SessionRecord| time(Some(&session.created_at)).max(time(session.agent_started_at.as_deref()));
        let mut incarnations: IndexMap<Option<String>, Vec<&SessionRecord>> = IndexMap::new();
        for &session in group {
            let deleted_at = match session
                .worktree
                .as_ref()
                .and_then(|worktree| worktree.deleted_at.clone())
            {
                Some(deleted_at) => Some(deleted_at),
                None => deletions
                    .iter()
                    .find(|deletion| last_launch(session) <= time(Some(deletion)))
                    .map(|deletion| (*deletion).to_owned()),
            };
            incarnations.entry(deleted_at).or_default().push(session);
        }
        for (deleted_at, members) in incarnations {
            let deleted = truthy(deleted_at.as_deref());
            let existing = if deleted {
                None
            } else {
                live_worktree_record(records.values().map(|record| &**record), root).map(|record| record.id.clone())
            };
            let record_id = match existing {
                Some(id) => id,
                None => {
                    let merges: Vec<MergeSource> = members.iter().map(|session| MergeSource::of(session)).collect();
                    // `.filter(mergedAt).sort(latest first)[0]`: the first of the latest (the sort is stable).
                    let mut latest: Option<&MergeSource> = None;
                    for source in merges.iter().filter(|source| truthy(source.merged_at.as_deref())) {
                        if latest.is_none_or(|best| time(source.merged_at.as_deref()) > time(best.merged_at.as_deref()))
                        {
                            latest = Some(source);
                        }
                    }
                    // The earliest createdAt (the first of the earliest, as the stable sort leaves it).
                    let mut created_at: Option<&str> = None;
                    for session in &members {
                        if created_at.is_none_or(|best| time(Some(&session.created_at)) < time(Some(best))) {
                            created_at = Some(&session.created_at);
                        }
                    }
                    let id = new_id();
                    let record = WorktreeRecord {
                        id: id.clone(),
                        path: root.clone(),
                        created_at: created_at.unwrap_or_default().to_owned(),
                        base_sha: None,
                        merged_at: latest.and_then(|source| source.merged_at.clone()),
                        merge_mode: latest.and_then(|source| source.merge_mode),
                        merge_target_branch: latest.and_then(|source| source.merge_target_branch.clone()),
                        merge_source_ref: latest.and_then(|source| source.merge_source_ref.clone()),
                        merge_source_sha: None,
                        merge_marked_manually: latest.and_then(|source| source.merge_marked_manually),
                        merge_detected: None,
                        deleted_at: deleted_at.clone().filter(|_| deleted),
                        merge_dismissed_tip: None,
                        extra: IndexMap::new(),
                    };
                    records.insert(id.clone(), Ordered::new(record));
                    id
                }
            };
            for session in members {
                assigned.insert(session.id.clone(), record_id.clone());
            }
        }
    }

    let migrated: Vec<Ordered<SessionRecord>> = sessions
        .iter()
        .map(|session| {
            if let Some(id) = assigned.get(&session.id).filter(|id| !id.is_empty()) {
                changed = true;
                let mut lifted = (**session).clone();
                let mut worktree = lifted
                    .worktree
                    .take()
                    .unwrap_or_else(|| SessionWorktreeRecord::new(SessionWorktreeMode::None));
                worktree.id = Some(id.clone());
                lifted.worktree = Some(worktree);
                // A sub-session's top-level marker now belongs to its worktree's record.
                clear_session_merge_fields(&mut lifted);
                let lifted = reorder(
                    lifted,
                    session,
                    |keys| drop_keys(keys, &SESSION_MERGE_FIELDS),
                    |keys| push_key(keys, "id"),
                );
                return stored_session(&lifted);
            }
            if worktree_id(session).is_some()
                && session.worktree.as_ref().is_some_and(|worktree| {
                    let markers = worktree_markers(worktree);
                    WORKTREE_MARKERS.iter().any(|&marker| has_marker(&markers, marker))
                })
            {
                changed = true;
            }
            stored_session(session)
        })
        .collect();
    let referenced: HashSet<&str> = migrated.iter().filter_map(|session| worktree_id(session)).collect();
    let before = records.len();
    records.retain(|id, _| referenced.contains(id.as_str()));
    if records.len() != before {
        changed = true;
    }
    WorktreeMigration {
        sessions: migrated,
        worktrees: records.into_values().collect(),
        changed,
    }
}

/// TS: worktreeRecords.ts migrateDoneMarkers result.
#[derive(Clone, Debug, PartialEq)]
pub struct DoneMigration {
    pub sessions: Vec<Ordered<SessionRecord>>,
    pub changed: bool,
}

/// TS: worktreeRecords.ts migrateDoneMarkers.
///
/// Before `D`, `M` doubled as "this task is done" for sessions without a worktree record (the main checkout). Such a
/// session's own merge marker (top-level, or under `worktree` when attached to the main worktree) becomes `doneAt`
/// (kept if already set) and is removed. Run after migrate_worktree_records, which has lifted the markers of sessions
/// in a linked worktree into their records; worktree records are untouched. Idempotent.
pub fn migrate_done_markers(sessions: Vec<Ordered<SessionRecord>>) -> DoneMigration {
    let mut changed = false;
    let migrated = sessions
        .into_iter()
        .map(|session| {
            if worktree_id(&session).is_some() {
                return session;
            }
            let nested = session
                .worktree
                .as_ref()
                .filter(|worktree| worktree.mode != SessionWorktreeMode::None);
            let merged_at = session
                .merged_at
                .clone()
                .or_else(|| nested.and_then(|worktree| worktree.merged_at.clone()));
            let leftovers = has_session_merge_fields(&session)
                || nested.is_some_and(|worktree| {
                    let markers = worktree_markers(worktree);
                    MERGE_MARKERS.iter().any(|&marker| has_marker(&markers, marker))
                });
            if !leftovers {
                return session;
            }
            changed = true;
            let is_nested = nested.is_some();
            let mut next = (*session).clone();
            clear_session_merge_fields(&mut next);
            if let Some(worktree) = next.worktree.as_mut().filter(|_| is_nested) {
                *worktree = without_markers(worktree, &MERGE_MARKERS);
            }
            let set_done = truthy(merged_at.as_deref()) && !truthy(next.done_at.as_deref());
            if set_done {
                next.done_at = merged_at;
            }
            reorder(
                next,
                &session,
                |keys| {
                    drop_keys(keys, &SESSION_MERGE_FIELDS);
                    if set_done {
                        push_key(keys, "doneAt");
                    }
                },
                |keys| {
                    if is_nested {
                        drop_keys(keys, &marker_names(&MERGE_MARKERS));
                    }
                },
            )
        })
        .collect();
    DoneMigration {
        sessions: migrated,
        changed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::workspace_key;
    use deckhand_parity::{Case, Outcome, Replay, replay, tags};
    use serde_json::{Value, json};

    fn at(minute: u32) -> String {
        format!("2026-01-01T00:{minute:02}:00.000Z")
    }

    /// tests/worktreeRecords.test.ts `session(id, created, worktree, extra)`.
    fn session(id: &str, created: u32, worktree: Value, extra: Value) -> Ordered<SessionRecord> {
        let cwd = worktree.get("path").cloned().unwrap_or(json!("/repo"));
        let mut value = json!({
            "id": id, "title": id, "program": "claude", "command": "claude", "cwd": cwd, "repoRoot": "/repo",
            "launchWorktreeRoot": "/repo", "worktree": worktree, "status": "exited", "createdAt": at(created),
            "updatedAt": at(created),
        });
        for (key, entry) in extra.as_object().cloned().unwrap_or_default() {
            value[key] = entry;
        }
        serde_json::from_value(value).unwrap()
    }

    /// tests/worktreeRecords.test.ts `managed(extra)`.
    fn managed(extra: Value) -> Value {
        let mut value = json!({"mode": "managed", "path": "/wt/feature", "branch": "feature", "isMain": false});
        for (key, entry) in extra.as_object().cloned().unwrap_or_default() {
            value[key] = entry;
        }
        value
    }

    fn attached(extra: Value) -> Value {
        let mut value = managed(extra);
        value["mode"] = json!("attached");
        value
    }

    fn legacy_sessions() -> Vec<Ordered<SessionRecord>> {
        vec![
            session(
                "a",
                1,
                managed(
                    json!({"mergedAt": at(10), "mergeMode": "squash", "mergeTargetBranch": "main", "mergeSourceRef": "feature", "deletedAt": at(30)}),
                ),
                json!({}),
            ),
            session(
                "b",
                2,
                attached(
                    json!({"mergedAt": at(20), "mergeTargetBranch": "main", "mergeSourceRef": "feature", "mergeMarkedManually": true}),
                ),
                json!({}),
            ),
            // A sub-session: no worktree of its own, launched in the linked worktree; its top-level M marker is lifted.
            session(
                "child",
                3,
                json!({"mode": "none"}),
                json!({"cwd": "/wt/feature", "launchWorktreeRoot": "/wt/feature", "requestedWorktreeMode": "none", "mergedAt": at(5), "mergeTargetBranch": "main", "mergeSourceRef": "feature", "mergeMarkedManually": true}),
            ),
            // Created before the deletion but last launched after it, in the new worktree at the same path.
            session("restarted", 4, attached(json!({})), json!({"agentStartedAt": at(45)})),
            session("new", 40, managed(json!({})), json!({})),
            session(
                "other",
                5,
                managed(
                    json!({"path": "/wt/other", "mergedAt": at(8), "mergeMode": "merge", "mergeTargetBranch": "main", "mergeSourceRef": "other"}),
                ),
                json!({}),
            ),
            session(
                "main-none",
                6,
                json!({"mode": "none"}),
                json!({"mergedAt": at(9), "mergeTargetBranch": "main", "mergeSourceRef": "main", "mergeMarkedManually": true}),
            ),
            session(
                "main-attached",
                7,
                json!({"mode": "attached", "path": "/repo", "isMain": true, "mergedAt": at(9), "mergeTargetBranch": "dev", "mergeSourceRef": "main", "mergeMarkedManually": true}),
                json!({}),
            ),
            session(
                "waiting",
                8,
                json!({"mode": "none"}),
                json!({"launchWorktreeRoot": "/wt/feature", "requestedWorktreeMode": "new"}),
            ),
        ]
    }

    fn counter() -> impl FnMut() -> String {
        let mut next = 0;
        move || {
            next += 1;
            format!("w{next}")
        }
    }

    fn find<'a>(sessions: &'a [Ordered<SessionRecord>], id: &str) -> &'a Ordered<SessionRecord> {
        sessions.iter().find(|item| item.id == id).unwrap()
    }

    /// A record as JSON without its id (`{...record, id: undefined}`).
    fn without_id(record: &Ordered<WorktreeRecord>) -> Value {
        let mut value = serde_json::to_value(record).unwrap();
        value.as_object_mut().unwrap().shift_remove("id");
        value
    }

    /// Runs a port body whose harness steps may fail (argument decoding).
    fn port(body: impl FnOnce() -> Result<Outcome, String>) -> Outcome {
        body().unwrap_or_else(|error| Outcome::throws(format!("harness: {error}")))
    }

    // covers: B-P2-03 migration lifts per-session worktree markers into one record per worktree incarnation
    #[test]
    fn migration_lifts_markers_into_records() {
        let mut new_id = counter();
        let WorktreeMigration {
            sessions,
            worktrees,
            changed,
        } = migrate_worktree_records(legacy_sessions(), Vec::new(), &mut new_id);
        assert!(changed);
        let record_of = |id: &str| {
            let reference = find(&sessions, id)
                .worktree
                .as_ref()
                .and_then(|worktree| worktree.id.clone());
            worktrees
                .iter()
                .find(|record| Some(&record.id) == reference.as_ref())
                .unwrap()
        };
        // The deleted incarnation: the session that deleted it, its sibling and the sub-session, with the latest merge
        // marker.
        let deleted = record_of("a");
        assert_eq!(record_of("b"), deleted);
        assert_eq!(record_of("child"), deleted);
        assert_eq!(
            without_id(deleted),
            json!({"path": "/wt/feature", "createdAt": at(1), "mergedAt": at(20), "mergeTargetBranch": "main", "mergeSourceRef": "feature", "mergeMarkedManually": true, "deletedAt": at(30)})
        );
        // The live incarnation at the same path: no inherited markers.
        let live = record_of("new");
        assert_ne!(live.id, deleted.id);
        assert_eq!(record_of("restarted"), live);
        assert_eq!(without_id(live), json!({"path": "/wt/feature", "createdAt": at(4)}));
        assert_eq!(
            without_id(record_of("other")),
            json!({"path": "/wt/other", "createdAt": at(5), "mergedAt": at(8), "mergeMode": "merge", "mergeTargetBranch": "main", "mergeSourceRef": "other"})
        );
        assert_eq!(worktrees.len(), 3);
        // Sessions keep no markers of their own once they reference a record; the sub-session's top-level marker moved.
        for id in ["a", "b", "child", "restarted", "new", "other"] {
            let item = find(&sessions, id);
            let worktree = serde_json::to_value(item.worktree.as_ref().unwrap()).unwrap();
            let marker_keys: Vec<&String> = worktree
                .as_object()
                .unwrap()
                .keys()
                .filter(|key| key.starts_with("merge") || *key == "deletedAt")
                .collect();
            assert!(marker_keys.is_empty(), "{id}");
            assert_eq!(item.merged_at, None, "{id}");
        }
        // Main-checkout sessions and a session still waiting for its worktree are untouched.
        let legacy = legacy_sessions();
        for id in ["main-none", "main-attached", "waiting"] {
            assert_eq!(find(&sessions, id), find(&legacy, id), "{id}");
        }

        // Projected, every session of an incarnation shows its markers, and the deleted one has no workspace.
        let records: WorktreeRecords = worktrees
            .iter()
            .map(|record| (record.id.clone(), record.clone()))
            .collect();
        let view = |id: &str| project_worktree(find(&sessions, id), &records);
        assert_eq!(view("b").worktree.as_ref().unwrap().merged_at, Some(at(20)));
        assert_eq!(view("child").worktree.as_ref().unwrap().deleted_at, Some(at(30)));
        assert_eq!(workspace_key(&view("b")), None);
        assert_eq!(workspace_key(&view("child")), None);
        assert_eq!(workspace_key(&view("restarted")).as_deref(), Some("/wt/feature"));
        assert_eq!(view("restarted").worktree.as_ref().unwrap().merged_at, None);
        assert_eq!(&stored_session(&view("b")), find(&sessions, "b"));

        // Migrating the migrated state changes nothing.
        let again = migrate_worktree_records(sessions.clone(), worktrees.clone(), &mut || "unused".to_owned());
        assert!(!again.changed);
        assert_eq!(again.sessions, sessions);
        assert_eq!(again.worktrees, worktrees);
    }

    // covers: B-P2-04 migration repairs stored state: stray markers dropped, missing records rebuilt, unreferenced records removed
    #[test]
    fn migration_repairs_stored_state() {
        let stray = session("s", 1, managed(json!({"id": "kept", "mergedAt": at(3)})), json!({}));
        let orphan = session(
            "o",
            2,
            managed(json!({"id": "lost", "path": "/wt/lost", "deletedAt": at(4)})),
            json!({}),
        );
        let records: Vec<Ordered<WorktreeRecord>> = serde_json::from_value(json!([
            {"id": "kept", "path": "/wt/feature", "createdAt": at(1)},
            {"id": "unused", "path": "/wt/gone", "createdAt": at(0)},
        ]))
        .unwrap();
        let mut random = || uuid::Uuid::new_v4().to_string();
        let WorktreeMigration {
            sessions,
            worktrees,
            changed,
        } = migrate_worktree_records(vec![stray, orphan], records, &mut random);
        assert!(changed);
        // The record is the only copy: it has no marker.
        assert_eq!(sessions[0].worktree.as_ref().unwrap().merged_at, None);
        let mut ids: Vec<&str> = worktrees.iter().map(|record| record.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["kept", "lost"]);
        assert_eq!(
            worktrees.iter().find(|record| record.id == "lost").unwrap().deleted_at,
            Some(at(4))
        );
        // Legacy sessions at a path with a live record join it.
        let mut list = sessions.clone();
        list.push(session("late", 9, attached(json!({})), json!({})));
        let joined = migrate_worktree_records(list, worktrees, &mut random);
        assert_eq!(
            joined.sessions.last().unwrap().worktree.as_ref().unwrap().id.as_deref(),
            Some("kept")
        );
    }

    // covers: B-P2-05 done migration: main-checkout merge markers become doneAt and go; worktree records and their sessions are untouched; idempotent
    #[test]
    fn done_migration() {
        // The pure half of the TS test; its state-file half (loadState / markAllNonExitedSessionsExited) runs in I1.
        let lifted = migrate_worktree_records(legacy_sessions(), Vec::new(), &mut counter());
        let DoneMigration { sessions, changed } = migrate_done_markers(lifted.sessions.clone());
        assert!(changed);
        // Top-level M markers (mode none in the main checkout) and a main-worktree attach's markers become done markers.
        let main_none = find(&sessions, "main-none");
        let main_attached = find(&sessions, "main-attached");
        assert_eq!(main_none.done_at, Some(at(9)));
        assert_eq!(main_attached.done_at, Some(at(9)));
        for item in [main_none, main_attached] {
            assert_eq!(item.merged_at, None);
            assert_eq!(item.merge_target_branch, None);
            assert_eq!(item.merge_source_ref, None);
            assert_eq!(item.merge_marked_manually, None);
        }
        assert_eq!(
            serde_json::to_value(main_attached.worktree.as_ref().unwrap()).unwrap(),
            json!({"mode": "attached", "path": "/repo", "isMain": true})
        );
        // Sessions of linked worktrees (their markers already in records) and everyone else: unchanged, never done.
        for item in lifted
            .sessions
            .iter()
            .filter(|item| !["main-none", "main-attached"].contains(&item.id.as_str()))
        {
            assert_eq!(find(&sessions, &item.id), item, "{}", item.id);
        }
        assert!(
            sessions
                .iter()
                .filter(|item| worktree_id(item).is_some())
                .all(|item| item.done_at.is_none())
        );
        // A session already done keeps its own time; a second run changes nothing.
        let kept = migrate_done_markers(vec![session(
            "both",
            1,
            json!({"mode": "none"}),
            json!({"mergedAt": at(5), "doneAt": at(7), "mergeMarkedManually": true}),
        )]);
        assert_eq!(kept.sessions[0].done_at, Some(at(7)));
        assert_eq!(kept.sessions[0].merged_at, None);
        let again = migrate_done_markers(sessions.clone());
        assert!(!again.changed);
        assert_eq!(again.sessions, sessions);
    }

    #[test]
    fn projected_markers_go_last_in_marker_order() {
        let stored = session(
            "a",
            1,
            json!({"mode": "managed", "path": "/wt", "mergedAt": "stale", "id": "w1", "branch": "b"}),
            json!({}),
        );
        let records: WorktreeRecords = [(
            "w1".to_owned(),
            serde_json::from_value(
                json!({"id": "w1", "path": "/wt", "createdAt": "x", "deletedAt": "d", "mergedAt": "m"}),
            )
            .unwrap(),
        )]
        .into_iter()
        .collect();
        let projected = serde_json::to_value(project_worktree(&stored, &records)).unwrap();
        let projected = serde_json::to_string(&projected["worktree"]).unwrap();
        assert_eq!(
            projected,
            r#"{"mode":"managed","path":"/wt","id":"w1","branch":"b","mergedAt":"m","deletedAt":"d"}"#
        );
    }

    /// fixtures/state/home-v45: the legacy records and markers a v45 daemon migrates on start (MIGRATIONS.md rows 4-12)
    /// end up as in home-v45.after-idle, apart from what later start-up steps change (baseSha, merge detection).
    #[test]
    fn legacy_home_v45_state_migrates_like_ts() {
        let read = |name: &str| -> Value {
            let file = deckhand_parity::repo_root()
                .join("fixtures/state")
                .join(name)
                .join("state.json");
            serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
        };
        let before = read("home-v45");
        let after = read("home-v45.after-idle");
        let sessions: Vec<Ordered<SessionRecord>> = serde_json::from_value(before["sessions"].clone()).unwrap();
        // Records without a string id and path never make it into typed state (TS drops them in this function).
        let worktrees: Vec<Ordered<WorktreeRecord>> = before["worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|record| serde_json::from_value(record.clone()).ok())
            .collect();
        assert_eq!(worktrees.len(), 5);
        // The fixture names the two new records by creation order: the live incarnation first.
        let mut ids = ["<uuid-2>", "<uuid-1>"].into_iter().map(str::to_owned);
        let lifted = migrate_worktree_records(sessions, worktrees, &mut || ids.next().unwrap());
        assert!(lifted.changed);
        let done = migrate_done_markers(lifted.sessions);
        assert!(done.changed);

        let mut expected_records: Vec<Value> = after["worktrees"].as_array().unwrap().clone();
        for record in &mut expected_records {
            let id = record["id"].as_str().unwrap().to_owned();
            if id.ends_with("00a") {
                record.as_object_mut().unwrap().shift_remove("baseSha");
            }
            if id.ends_with("00b") {
                *record = before["worktrees"][1].clone();
            }
        }
        let got_records: Vec<Value> = lifted
            .worktrees
            .iter()
            .map(|record| serde_json::to_value(record).unwrap())
            .collect();
        assert_eq!(
            serde_json::to_string(&got_records).unwrap(),
            serde_json::to_string(&expected_records).unwrap()
        );

        let fields = [
            "worktree",
            "doneAt",
            "mergedAt",
            "mergeTargetBranch",
            "mergeSourceRef",
            "mergeMarkedManually",
        ];
        for expected in after["sessions"].as_array().unwrap() {
            let id = expected["id"].as_str().unwrap();
            // deckhand_protocol's WorktreeLinks keeps no unknown keys yet (reported as a frozen change request), so
            // `worktree.links.futureLinksField` (MIGRATIONS.md row 19) is not checked here.
            let mut expected = expected.clone();
            if let Some(links) = expected.pointer_mut("/worktree/links").and_then(Value::as_object_mut) {
                links.shift_remove("futureLinksField");
            }
            let got = serde_json::to_value(done.sessions.iter().find(|item| item.id == id).unwrap()).unwrap();
            for field in fields {
                assert_eq!(
                    serde_json::to_string(&got.get(field)).unwrap(),
                    serde_json::to_string(&expected.get(field)).unwrap(),
                    "{id} {field}"
                );
            }
        }
    }

    // ---- parity replays ----------------------------------------------------------------------------------------

    /// A `worktree` argument: the functions only read markers / mode / path, so a TS literal without `mode` gets one.
    fn worktree_arg(case: &Case, index: usize) -> Result<(SessionWorktreeRecord, bool), String> {
        let mut value = tags::untag(case.arg(index));
        let had_mode = value.get("mode").is_some();
        if let Some(object) = value.as_object_mut().filter(|_| !had_mode) {
            object.insert("mode".into(), json!("none"));
        }
        Ok((
            serde_json::from_value(value).map_err(|error| error.to_string())?,
            had_mode,
        ))
    }

    fn undefined_or<T: Serialize>(value: Option<T>) -> Outcome {
        match value {
            Some(value) => Outcome::ret(value),
            None => Outcome::ret(tags::undefined()),
        }
    }

    fn marker(name: &str) -> Result<WorktreeMarker, String> {
        WORKTREE_MARKERS
            .into_iter()
            .find(|marker| marker.as_str() == name)
            .ok_or_else(|| format!("not a marker: {name}"))
    }

    #[test]
    fn worktree_markers_parity() {
        replay("P2", "worktreeRecords/worktreeMarkers", |case: &Case| {
            Ok::<_, String>(worktree_markers(&worktree_arg(case, 0)?.0))
        });
    }

    #[test]
    fn without_markers_parity() {
        replay("P2", "worktreeRecords/withoutMarkers", |case: &Case| {
            let (source, had_mode) = worktree_arg(case, 0)?;
            let keys: Vec<WorktreeMarker> = if case.arg_is_undefined(1) {
                WORKTREE_MARKERS.to_vec()
            } else {
                case.arg_as::<Vec<String>>(1)?
                    .iter()
                    .map(|name| marker(name))
                    .collect::<Result<_, _>>()?
            };
            let mut out = serde_json::to_value(without_markers(&source, &keys)).map_err(|error| error.to_string())?;
            if let Some(object) = out.as_object_mut().filter(|_| !had_mode) {
                object.shift_remove("mode");
            }
            Ok::<_, String>(out)
        });
    }

    #[test]
    fn own_worktree_path_parity() {
        replay("P2", "worktreeRecords/ownWorktreePath", |case: &Case| {
            port(|| {
                let worktree: Option<SessionWorktreeRecord> = case.arg_as(0)?;
                Ok(undefined_or(own_worktree_path(worktree.as_ref())))
            })
        });
    }

    #[test]
    fn live_worktree_record_parity() {
        replay("P2", "worktreeRecords/liveWorktreeRecord", |case: &Case| {
            port(|| {
                let records: Vec<Ordered<WorktreeRecord>> = case.arg_as(0)?;
                let found = live_worktree_record(records.iter().map(|record| &**record), case.arg_str(1)?);
                let found =
                    found.and_then(|record| records.iter().find(|candidate| std::ptr::eq(&***candidate, record)));
                Ok(undefined_or(found))
            })
        });
    }

    #[test]
    fn project_worktree_parity() {
        Replay::new("P2", "worktreeRecords/projectWorktree")
            .key_order(true)
            .run(|case: &Case| {
                let session: Ordered<SessionRecord> = case.arg_as(0)?;
                let records: WorktreeRecords = case.arg_as(1)?;
                Ok::<_, String>(project_worktree(&session, &records))
            })
            .assert_ok();
    }

    #[test]
    fn stored_session_parity() {
        Replay::new("P2", "worktreeRecords/storedSession")
            .key_order(true)
            .run(|case: &Case| {
                let session: Ordered<SessionRecord> = case.arg_as(0)?;
                Ok::<_, String>(stored_session(&session))
            })
            .assert_ok();
    }

    #[test]
    fn migrate_worktree_records_parity() {
        Replay::new("P2", "worktreeRecords/migrateWorktreeRecords")
            .key_order(true)
            .run(|case: &Case| {
                let sessions: Vec<Ordered<SessionRecord>> = case.arg_as(0)?;
                let worktrees: Vec<Ordered<WorktreeRecord>> = case.arg_as(1)?;
                if case.fn_name(2) != Some("seqId") {
                    return Err("expected the seqId id source".to_owned());
                }
                let result = migrate_worktree_records(sessions, worktrees, &mut counter());
                Ok(json!({"sessions": result.sessions, "worktrees": result.worktrees, "changed": result.changed}))
            })
            .assert_ok();
    }

    #[test]
    fn migrate_done_markers_parity() {
        Replay::new("P2", "worktreeRecords/migrateDoneMarkers")
            .key_order(true)
            .run(|case: &Case| {
                let sessions: Vec<Ordered<SessionRecord>> = case.arg_as(0)?;
                let result = migrate_done_markers(sessions);
                Ok::<_, String>(json!({"sessions": result.sessions, "changed": result.changed}))
            })
            .assert_ok();
    }
}
