//! Port of changesModel.ts.
//!
//! The Git tab's Changes view as pure data: VS Code-style groups built from porcelain-v2 status plus numstat, the
//! rows and selection the pane shows, and the diff preview's line classification and first changed line. Git I/O
//! lives in `deckhand_sys::changes_git`; the records it produces are `deckhand_protocol` types.
//!
//! [`StatusEntry`] (workspaceGit.ts) is defined here because grouping consumes it; `deckhand_sys::workspace_git`
//! produces it.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use deckhand_protocol::data::{ChangeCounts, ChangeEntry, ChangeGroup, ChangesRecord, StageMode};

/// TS: changesModel.ts CHANGE_GROUPS (VS Code's order).
pub const CHANGE_GROUPS: [ChangeGroup; 4] = [
    ChangeGroup::Conflicts,
    ChangeGroup::Staged,
    ChangeGroup::Unstaged,
    ChangeGroup::Untracked,
];

/// TS: changesModel.ts CHANGE_GROUP_TITLES.
pub const CHANGE_GROUP_TITLES: [(ChangeGroup, &str); 4] = [
    (ChangeGroup::Conflicts, "Merge Conflicts"),
    (ChangeGroup::Staged, "Staged Changes"),
    (ChangeGroup::Unstaged, "Changes"),
    (ChangeGroup::Untracked, "Untracked"),
];

/// TS: changesModel.ts CHANGE_GROUP_TITLES[group].
pub fn change_group_title(group: ChangeGroup) -> &'static str {
    let _ = group;
    todo!("P7")
}

/// TS: changesModel.ts MAX_CHANGES.
pub const MAX_CHANGES: usize = 2000;

/// TS: changesModel.ts LineCounts.
#[skip_serializing_none]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineCounts {
    pub additions: Option<i64>,
    pub deletions: Option<i64>,
    pub binary: Option<bool>,
}

impl LineCounts {
    /// An entry's line counts (TS passes the entry itself as its LineCounts).
    pub fn of(entry: &ChangeEntry) -> LineCounts {
        let _ = entry;
        todo!("P7")
    }
}

/// workspaceGit.ts StatusEntry kind: a porcelain-v2 record kind (`1` ordinary, `2` rename/copy, `u` unmerged, `?`
/// untracked).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StatusKind {
    #[serde(rename = "1")]
    Ordinary,
    #[serde(rename = "2")]
    Renamed,
    #[serde(rename = "u")]
    Unmerged,
    #[serde(rename = "?")]
    Untracked,
}

/// workspaceGit.ts StatusEntry (one porcelain-v2 record; repository-relative paths).
#[skip_serializing_none]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusEntry {
    pub kind: StatusKind,
    pub xy: String,
    pub path: String,
    pub orig_path: Option<String>,
}

/// TS: changesModel.ts emptyCounts.
pub fn empty_counts() -> ChangeCounts {
    todo!("P7")
}

/// TS: changesModel.ts emptyChanges (callers set the fields TS passes in afterwards).
pub fn empty_changes() -> ChangesRecord {
    todo!("P7")
}

/// TS: changesModel.ts parseNumstat (`git diff --numstat -z` by new path; the last record of a path wins).
pub fn parse_numstat(raw: &str) -> IndexMap<String, LineCounts> {
    let _ = raw;
    todo!("P7")
}

/// TS: changesModel.ts groupChanges `numstat` argument (per group's diff).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Numstats {
    pub staged: Option<IndexMap<String, LineCounts>>,
    pub unstaged: Option<IndexMap<String, LineCounts>>,
    pub untracked: Option<IndexMap<String, LineCounts>>,
}

/// TS: changesModel.ts groupChanges result (`Pick<ChangesRecord, 'entries' | 'counts' | 'omitted'>`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GroupedChanges {
    pub entries: Vec<ChangeEntry>,
    pub counts: ChangeCounts,
    pub omitted: ChangeCounts,
}

/// TS: changesModel.ts groupChanges (`cap` defaults to MAX_CHANGES).
pub fn group_changes(entries: &[StatusEntry], numstat: &Numstats, cap: usize) -> GroupedChanges {
    let _ = (entries, numstat, cap);
    todo!("P7")
}

/// TS: changesModel.ts changeKey (`<group>\0<path>`).
pub fn change_key(entry: &ChangeEntry) -> String {
    let _ = entry;
    todo!("P7")
}

/// TS: changesModel.ts findChange.
pub fn find_change<'a>(entries: &'a [ChangeEntry], group: ChangeGroup, file: &str) -> Option<&'a ChangeEntry> {
    let _ = (entries, group, file);
    todo!("P7")
}

/// TS: changesModel.ts totalChanges.
pub fn total_changes(counts: &ChangeCounts) -> i64 {
    let _ = counts;
    todo!("P7")
}

/// TS: changesModel.ts ChangeRow.
#[derive(Clone, Debug, PartialEq)]
pub enum ChangeRow {
    Header {
        group: ChangeGroup,
        count: i64,
    },
    /// `index`: the entry's index among the selectable entries (`record.entries`).
    Entry {
        entry: ChangeEntry,
        index: usize,
    },
    More {
        group: ChangeGroup,
        count: i64,
    },
}

/// TS: changesModel.ts changeRows.
pub fn change_rows(record: &ChangesRecord) -> Vec<ChangeRow> {
    let _ = record;
    todo!("P7")
}

/// TS: changesModel.ts reselect `previous` argument.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreviousSelection {
    pub key: Option<String>,
    pub index: usize,
    pub offset: Option<usize>,
}

/// TS: changesModel.ts reselect (TS -1, nothing to select: `None`).
pub fn reselect(entries: &[ChangeEntry], previous: &PreviousSelection) -> Option<usize> {
    let _ = (entries, previous);
    todo!("P7")
}

/// TS: changesModel.ts groupOffset (an entry's position within its group).
pub fn group_offset(entries: &[ChangeEntry], index: usize) -> usize {
    let _ = (entries, index);
    todo!("P7")
}

/// TS: changesModel.ts stageMode.
pub fn stage_mode(entry: &ChangeEntry) -> StageMode {
    let _ = entry;
    todo!("P7")
}

/// TS: changesModel.ts changeLabel result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeLabel {
    pub name: String,
    pub dir: String,
}

/// TS: changesModel.ts changeLabel (a rename reads `old → new`).
pub fn change_label(entry: &ChangeEntry) -> ChangeLabel {
    let _ = entry;
    todo!("P7")
}

/// TS: changesModel.ts lineCountsText (`+12 −3`, `bin`, or empty).
pub fn line_counts_text(counts: &LineCounts) -> String {
    let _ = counts;
    todo!("P7")
}

/// TS: changesModel.ts firstChangedLine (on the new side; 1 without a hunk).
pub fn first_changed_line(diff: &str) -> i64 {
    let _ = diff;
    todo!("P7")
}

/// TS: changesModel.ts DiffLineKind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DiffLineKind {
    #[serde(rename = "add")]
    Add,
    #[serde(rename = "del")]
    Del,
    #[serde(rename = "hunk")]
    Hunk,
    #[serde(rename = "meta")]
    Meta,
    #[serde(rename = "context")]
    Context,
}

/// TS: changesModel.ts DiffLine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub text: String,
}

/// TS: changesModel.ts classifyDiff.
pub fn classify_diff(text: &str) -> Vec<DiffLine> {
    let _ = text;
    todo!("P7")
}

/// TS: changesModel.ts untrackedDiff (a file's contents as an all-additions diff).
pub fn untracked_diff(content: &str) -> String {
    let _ = content;
    todo!("P7")
}
