//! Port of worktreesModel.ts.
//!
//! W, the worktree manager (the worktrees flow renders it): the repository's worktrees grouped by what to do with
//! them. Times are milliseconds since the epoch.

use serde::{Deserialize, Serialize};

use crate::session_order::SessionLike;
use deckhand_protocol::data::{WorktreeOverview, WorktreeOverviewEntry};

/// TS: worktreesModel.ts WorktreeGroup.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WorktreeGroup {
    /// Merged (or nothing beyond the default branch) and deleting it with its branch loses nothing.
    #[serde(rename = "ready")]
    Ready,
    /// Merged, but it has uncommitted files or commits of its own still.
    #[serde(rename = "leftovers")]
    Leftovers,
    #[serde(rename = "progress")]
    Progress,
    /// Not merged, nothing running and untouched for IDLE_DAYS.
    #[serde(rename = "idle")]
    Idle,
    /// Its directory is gone.
    #[serde(rename = "missing")]
    Missing,
    /// The main checkout (never deleted).
    #[serde(rename = "main")]
    Main,
}

/// TS: worktreesModel.ts WORKTREE_GROUPS.
pub const WORKTREE_GROUPS: [WorktreeGroup; 6] = [
    WorktreeGroup::Ready,
    WorktreeGroup::Leftovers,
    WorktreeGroup::Progress,
    WorktreeGroup::Idle,
    WorktreeGroup::Missing,
    WorktreeGroup::Main,
];

/// TS: worktreesModel.ts GROUP_TITLES.
pub const GROUP_TITLES: [(WorktreeGroup, &str); 6] = [
    (WorktreeGroup::Ready, "Merged · safe to delete"),
    (WorktreeGroup::Leftovers, "Merged · has leftovers"),
    (WorktreeGroup::Progress, "In progress"),
    (WorktreeGroup::Idle, "Idle"),
    (WorktreeGroup::Missing, "Missing"),
    (WorktreeGroup::Main, "Main checkout"),
];

/// TS: worktreesModel.ts GROUP_TITLES[group].
pub fn group_title(group: WorktreeGroup) -> &'static str {
    let _ = group;
    todo!("P4")
}

/// TS: worktreesModel.ts IDLE_DAYS.
pub const IDLE_DAYS: i64 = 14;

/// TS: worktreesModel.ts WorktreeRow.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind")]
pub enum WorktreeRow {
    #[serde(rename = "heading")]
    Heading { group: WorktreeGroup, count: usize },
    #[serde(rename = "worktree")]
    Worktree {
        group: WorktreeGroup,
        entry: Box<WorktreeOverviewEntry>,
    },
    #[serde(rename = "empty")]
    Empty { text: String },
}

/// TS: worktreesModel.ts worktreeName (its branch, else `<dir> (detached)`).
pub fn worktree_name(entry: &WorktreeOverviewEntry) -> String {
    let _ = entry;
    todo!("P4")
}

/// TS: worktreesModel.ts mergedText (how it got merged, or why it counts as merged).
pub fn merged_text(entry: &WorktreeOverviewEntry, default_branch: Option<&str>) -> Option<String> {
    let _ = (entry, default_branch);
    todo!("P4")
}

/// TS: worktreesModel.ts lastActivity (ms).
pub fn last_activity<S: SessionLike>(entry: &WorktreeOverviewEntry, sessions: &[S]) -> Option<i64> {
    let _ = (entry, sessions);
    todo!("P4")
}

/// TS: worktreesModel.ts worktreeGroup.
pub fn worktree_group<S: SessionLike>(
    entry: &WorktreeOverviewEntry,
    sessions: &[S],
    now_ms: i64,
    default_branch: Option<&str>,
) -> WorktreeGroup {
    let _ = (entry, sessions, now_ms, default_branch);
    todo!("P4")
}

/// TS: worktreesModel.ts worktreeRows.
pub fn worktree_rows<S: SessionLike>(
    overview: Option<&WorktreeOverview>,
    sessions: &[S],
    now_ms: i64,
) -> Vec<WorktreeRow> {
    let _ = (overview, sessions, now_ms);
    todo!("P4")
}

/// TS: worktreesModel.ts selectableWorktreeRow.
pub fn selectable_worktree_row(row: &WorktreeRow) -> bool {
    let _ = row;
    todo!("P4")
}

/// TS: worktreesModel.ts worktreeRowKey.
pub fn worktree_row_key(row: &WorktreeRow) -> Option<String> {
    let _ = row;
    todo!("P4")
}

/// TS: worktreesModel.ts deleteRefusal.
pub fn delete_refusal(entry: &WorktreeOverviewEntry) -> Option<&'static str> {
    let _ = entry;
    todo!("P4")
}

/// TS: worktreesModel.ts protectedBranch.
pub fn protected_branch(branch: &str, default_branch: Option<&str>) -> bool {
    let _ = (branch, default_branch);
    todo!("P4")
}

/// TS: worktreesModel.ts bulkTargets.
pub fn bulk_targets(rows: &[WorktreeRow]) -> Vec<&WorktreeOverviewEntry> {
    let _ = rows;
    todo!("P4")
}

/// TS: worktreesModel.ts worktreeTags.
pub fn worktree_tags<S: SessionLike>(entry: &WorktreeOverviewEntry, sessions: &[S], now_ms: i64) -> Vec<String> {
    let _ = (entry, sessions, now_ms);
    todo!("P4")
}
