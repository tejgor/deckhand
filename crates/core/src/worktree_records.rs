//! Port of worktreeRecords.ts.
//!
//! Merge and deletion markers describe a linked worktree, which several sessions can share. They live in one
//! WorktreeRecord per worktree incarnation (state.json `worktrees`); each session in it stores only the record's ID
//! (`worktree.id`). The daemon projects the record's markers into the sessions it holds and sends
//! ([`project_worktree`]) and strips them again before saving ([`stored_session`]).
//!
//! Sessions and records here are `Ordered` because these functions build what state.json and the wire carry: the
//! projected `worktree` object is `{...withoutMarkers(worktree), ...markers}` (markers last, in marker order).

use indexmap::IndexMap;

use deckhand_protocol::Ordered;
use deckhand_protocol::data::{SessionRecord, SessionWorktreeRecord, WorktreeMarkers};
use deckhand_protocol::records::WorktreeRecord;

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

/// TS: worktreeRecords.ts worktreeMarkers, for a session's `worktree` (records have `WorktreeRecord::markers`).
pub fn worktree_markers(source: &SessionWorktreeRecord) -> WorktreeMarkers {
    let _ = source;
    todo!("P2")
}

/// TS: worktreeRecords.ts withoutMarkers (a copy without `keys`; callers pass `&WORKTREE_MARKERS` for the default).
pub fn without_markers(source: &SessionWorktreeRecord, keys: &[WorktreeMarker]) -> SessionWorktreeRecord {
    let _ = (source, keys);
    todo!("P2")
}

/// TS: worktreeRecords.ts ownWorktreePath (the resolved root of a managed/attached linked worktree).
pub fn own_worktree_path(worktree: Option<&SessionWorktreeRecord>) -> Option<String> {
    let _ = worktree;
    todo!("P2")
}

/// TS: worktreeRecords.ts liveWorktreeRecord (the not-deleted record at `worktree_path`, compared lexically).
pub fn live_worktree_record<'a, I>(records: I, worktree_path: &str) -> Option<&'a WorktreeRecord>
where
    I: IntoIterator<Item = &'a WorktreeRecord>,
{
    let _ = (records.into_iter(), worktree_path);
    todo!("P2")
}

/// TS: worktreeRecords.ts projectWorktree (the session as the daemon holds and sends it).
pub fn project_worktree(session: &Ordered<SessionRecord>, records: &WorktreeRecords) -> Ordered<SessionRecord> {
    let _ = (session, records);
    todo!("P2")
}

/// TS: worktreeRecords.ts storedSession (the session as persisted: projected markers dropped).
pub fn stored_session(session: &Ordered<SessionRecord>) -> Ordered<SessionRecord> {
    let _ = session;
    todo!("P2")
}

/// TS: worktreeRecords.ts migrateWorktreeRecords result.
#[derive(Clone, Debug, PartialEq)]
pub struct WorktreeMigration {
    pub sessions: Vec<Ordered<SessionRecord>>,
    pub worktrees: Vec<Ordered<WorktreeRecord>>,
    pub changed: bool,
}

/// TS: worktreeRecords.ts migrateWorktreeRecords (`new_id`: TS `randomUUID` by default).
pub fn migrate_worktree_records(
    sessions: Vec<Ordered<SessionRecord>>,
    worktrees: Vec<Ordered<WorktreeRecord>>,
    new_id: &mut dyn FnMut() -> String,
) -> WorktreeMigration {
    let _ = (sessions, worktrees, new_id);
    todo!("P2")
}

/// TS: worktreeRecords.ts migrateDoneMarkers result.
#[derive(Clone, Debug, PartialEq)]
pub struct DoneMigration {
    pub sessions: Vec<Ordered<SessionRecord>>,
    pub changed: bool,
}

/// TS: worktreeRecords.ts migrateDoneMarkers.
pub fn migrate_done_markers(sessions: Vec<Ordered<SessionRecord>>) -> DoneMigration {
    let _ = sessions;
    todo!("P2")
}
