//! Port of worktreesModel.ts.
//!
//! W, the worktree manager (the worktrees flow renders it): the repository's worktrees grouped by what to do with
//! them. Times are milliseconds since the epoch.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::jsstr::{date, path};
use crate::session_order::SessionLike;
use crate::sidebar_model::format_age;
use crate::tasks_board::worktree_open_items;
use deckhand_protocol::data::{
    MergeDetected, WorktreeInUse, WorktreeMergeMode, WorktreeMissing, WorktreeOverview, WorktreeOverviewEntry,
};

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
    GROUP_TITLES
        .iter()
        .find(|(key, _)| *key == group)
        .map_or("", |(_, title)| title)
}

/// TS: worktreesModel.ts IDLE_DAYS.
pub const IDLE_DAYS: i64 = 14;
const DAY: i64 = 24 * 60 * 60_000;

/// The group's wire string (`ready`, `leftovers`, ...).
fn group_key(group: WorktreeGroup) -> &'static str {
    match group {
        WorktreeGroup::Ready => "ready",
        WorktreeGroup::Leftovers => "leftovers",
        WorktreeGroup::Progress => "progress",
        WorktreeGroup::Idle => "idle",
        WorktreeGroup::Missing => "missing",
        WorktreeGroup::Main => "main",
    }
}

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
    if entry.branch.is_empty() {
        format!("{} (detached)", path::basename(&entry.path))
    } else {
        entry.branch.clone()
    }
}

/// TS: worktreesModel.ts mergedText (how it got merged, or why it counts as merged).
pub fn merged_text(entry: &WorktreeOverviewEntry, default_branch: Option<&str>) -> Option<String> {
    if let Some(markers) = entry.markers.as_ref()
        && markers.merged_at.as_deref().is_some_and(|at| !at.is_empty())
    {
        if markers.merge_detected == Some(MergeDetected::Pr) {
            return Some("PR merged".to_owned());
        }
        if markers.merge_detected == Some(MergeDetected::Ancestor) {
            let into = markers
                .merge_target_branch
                .as_deref()
                .or(default_branch)
                .unwrap_or("the default branch");
            return Some(format!("merged into {into}"));
        }
        if markers.merge_marked_manually == Some(true) {
            return Some("marked merged (M)".to_owned());
        }
        let how = if markers.merge_mode == Some(WorktreeMergeMode::Squash) {
            "squash-merged"
        } else {
            "merged"
        };
        return Some(
            match markers
                .merge_target_branch
                .as_deref()
                .filter(|branch| !branch.is_empty())
            {
                Some(branch) => format!("{how} into {branch}"),
                None => how.to_owned(),
            },
        );
    }
    // A branch with no commits of its own is done only once nothing else is left in it either (a fresh branch with
    // uncommitted work is work in progress).
    if entry.ahead_of_default == Some(0)
        && !entry.head.is_empty()
        && entry.inspection.as_ref().is_some_and(|inspection| inspection.safe)
    {
        return Some(format!(
            "nothing beyond {}",
            default_branch.unwrap_or("the default branch")
        ));
    }
    None
}

/// TS: worktreesModel.ts lastActivity (ms).
///
/// The latest sign of life: its last commit, its record's creation, or a session's last update in it.
pub fn last_activity<S: SessionLike>(entry: &WorktreeOverviewEntry, sessions: &[S]) -> Option<i64> {
    let parse = |value: Option<&str>| value.filter(|value| !value.is_empty()).and_then(date::parse);
    let own = [
        parse(entry.last_commit_at.as_deref()),
        parse(entry.created_at.as_deref()),
    ];
    let updates = sessions
        .iter()
        .map(SessionLike::session)
        .filter(|session| entry.session_ids.contains(&session.id))
        .map(|session| parse(Some(&session.updated_at)));
    own.into_iter().chain(updates).flatten().max()
}

/// TS: worktreesModel.ts worktreeGroup.
pub fn worktree_group<S: SessionLike>(
    entry: &WorktreeOverviewEntry,
    sessions: &[S],
    now_ms: i64,
    default_branch: Option<&str>,
) -> WorktreeGroup {
    if entry.is_main {
        return WorktreeGroup::Main;
    }
    if entry.missing.is_some() {
        return WorktreeGroup::Missing;
    }
    if merged_text(entry, default_branch).is_some() {
        return if entry.inspection.as_ref().is_some_and(|inspection| inspection.safe) {
            WorktreeGroup::Ready
        } else {
            WorktreeGroup::Leftovers
        };
    }
    let active = last_activity(entry, sessions);
    if entry.running_ids.is_empty() && active.is_some_and(|active| now_ms.saturating_sub(active) > IDLE_DAYS * DAY) {
        return WorktreeGroup::Idle;
    }
    WorktreeGroup::Progress
}

/// TS: worktreesModel.ts worktreeRows.
///
/// The list: each group that has worktrees under its heading (most recently active first), the main checkout last.
pub fn worktree_rows<S: SessionLike>(
    overview: Option<&WorktreeOverview>,
    sessions: &[S],
    now_ms: i64,
) -> Vec<WorktreeRow> {
    let Some(overview) = overview else {
        return Vec::new();
    };
    let mut groups: HashMap<WorktreeGroup, Vec<&WorktreeOverviewEntry>> = HashMap::new();
    for entry in &overview.entries {
        let group = worktree_group(entry, sessions, now_ms, overview.default_branch.as_deref());
        groups.entry(group).or_default().push(entry);
    }
    let mut rows = Vec::new();
    for group in WORKTREE_GROUPS {
        let Some(entries) = groups.get(&group).filter(|entries| !entries.is_empty()) else {
            continue;
        };
        // Most recently active first (a stable sort, as Array.prototype.sort is).
        let mut sorted: Vec<(i64, &WorktreeOverviewEntry)> = entries
            .iter()
            .map(|entry| (last_activity(entry, sessions).unwrap_or(0), *entry))
            .collect();
        sorted.sort_by(|left, right| right.0.cmp(&left.0));
        rows.push(WorktreeRow::Heading {
            group,
            count: entries.len(),
        });
        rows.extend(sorted.into_iter().map(|(_, entry)| WorktreeRow::Worktree {
            group,
            entry: Box::new(entry.clone()),
        }));
    }
    if !overview.entries.iter().any(|entry| !entry.is_main) {
        rows.push(WorktreeRow::Empty {
            text: "No other worktrees: esc, then n and tab creates one with a new session".to_owned(),
        });
    }
    rows
}

/// TS: worktreesModel.ts selectableWorktreeRow.
///
/// Rows j/k stop on: worktrees, and the heading of the ready group (x there deletes them all).
pub fn selectable_worktree_row(row: &WorktreeRow) -> bool {
    matches!(
        row,
        WorktreeRow::Worktree { .. }
            | WorktreeRow::Heading {
                group: WorktreeGroup::Ready,
                ..
            }
    )
}

/// TS: worktreesModel.ts worktreeRowKey.
pub fn worktree_row_key(row: &WorktreeRow) -> Option<String> {
    match row {
        WorktreeRow::Worktree { entry, .. } => Some(format!("wt:{}", entry.path)),
        WorktreeRow::Heading { group, .. } => Some(format!("group:{}", group_key(*group))),
        WorktreeRow::Empty { .. } => None,
    }
}

/// TS: worktreesModel.ts deleteRefusal.
///
/// Why x cannot delete it at all (shown instead of a confirmation), if so.
pub fn delete_refusal(entry: &WorktreeOverviewEntry) -> Option<&'static str> {
    if entry.is_main {
        return Some("The main checkout is never deleted");
    }
    match entry.in_use {
        Some(WorktreeInUse::This) => {
            return Some("This Deckhand runs in that worktree; delete it from another checkout");
        }
        Some(WorktreeInUse::Other) => return Some("Another Deckhand is open in that worktree; quit it first"),
        None => {}
    }
    if entry.locked == Some(true) && entry.missing.is_none() {
        return Some("That worktree is locked (git worktree unlock it first)");
    }
    None
}

/// TS: worktreesModel.ts protectedBranch (branches never deleted with a worktree).
pub fn protected_branch(branch: &str, default_branch: Option<&str>) -> bool {
    branch == "main" || branch == "master" || Some(branch) == default_branch
}

/// TS: worktreesModel.ts bulkTargets.
///
/// The ready group's worktrees that x on its heading deletes (with their branches unless protected).
pub fn bulk_targets(rows: &[WorktreeRow]) -> Vec<&WorktreeOverviewEntry> {
    rows.iter()
        .filter_map(|row| match row {
            WorktreeRow::Worktree {
                group: WorktreeGroup::Ready,
                entry,
            } if delete_refusal(entry).is_none() => Some(&**entry),
            _ => None,
        })
        .collect()
}

/// TS: worktreesModel.ts worktreeTags.
///
/// What the row says on its right: sessions, uncommitted work, commits of its own, age. Short first-dropped last.
pub fn worktree_tags<S: SessionLike>(entry: &WorktreeOverviewEntry, sessions: &[S], now_ms: i64) -> Vec<String> {
    let mut tags = Vec::new();
    if let Some(missing) = entry.missing {
        let tag = if missing == WorktreeMissing::Prunable {
            "directory gone"
        } else {
            "not listed by Git"
        };
        tags.push(tag.to_owned());
    }
    if entry.error.as_deref().is_some_and(|error| !error.is_empty()) {
        tags.push("not checked".to_owned());
    }
    if let Some(inspection) = entry.inspection.as_ref() {
        let changed = inspection.dirty_files + inspection.untracked_files;
        if changed != 0 {
            tags.push(format!("{changed} changed"));
        }
        if inspection.ignored_files != 0 {
            tags.push(format!("{} ignored", inspection.ignored_files));
        }
    }
    if let Some(ahead) = entry.ahead_of_default.filter(|ahead| *ahead != 0) {
        tags.push(format!("{ahead} commit{}", if ahead == 1 { "" } else { "s" }));
    }
    // Open checklist items of its note: deleting it offers to send them to the backlog.
    let open = worktree_open_items(entry.record_id.as_deref(), sessions).len();
    if open != 0 {
        tags.push(format!("☐{open}"));
    }
    let running = entry.running_ids.len();
    let session_count = entry.session_ids.len();
    if running != 0 {
        tags.push(format!("{running} running"));
    } else if session_count != 0 {
        tags.push(format!(
            "{session_count} session{}",
            if session_count == 1 { "" } else { "s" }
        ));
    } else if !entry.is_main && entry.record_id.as_deref().is_none_or(str::is_empty) {
        tags.push("not from Deckhand".to_owned());
    }
    if let Some(active) = last_activity(entry, sessions) {
        tags.push(format_age(now_ms as f64 - active as f64));
    }
    tags
}

#[cfg(test)]
mod tests {
    use super::*;
    use deckhand_parity::{Case, replay, tags};
    use deckhand_protocol::data::SessionRecord;
    use serde_json::{Map, Value, json};

    const UNIT: &str = "P4";
    const DAY_MS: i64 = 24 * 60 * 60_000;

    fn now() -> i64 {
        date::parse("2026-10-09T12:00:00Z").unwrap_or_default()
    }

    fn iso(offset_ms: i64) -> String {
        date::to_iso_string(now() + offset_ms).unwrap_or_default()
    }

    /// `{...base, ...fields}`; a `null` field is `undefined` (removed).
    fn merged(mut base: Value, fields: Value) -> Value {
        if let (Some(base), Value::Object(fields)) = (base.as_object_mut(), fields) {
            for (key, value) in fields {
                if value.is_null() {
                    base.remove(&key);
                } else {
                    base.insert(key, value);
                }
            }
        }
        base
    }

    fn clean() -> Value {
        json!({"safe": true, "reasons": [], "dirtyFiles": 0, "untrackedFiles": 0, "ignoredFiles": 0})
    }

    /// The test's `entry(overrides)`.
    fn entry(overrides: Value) -> WorktreeOverviewEntry {
        let branch = overrides.get("branch").and_then(Value::as_str).unwrap_or("x");
        let base = json!({
            "path": format!("/w/{branch}"), "branch": "x", "head": "a".repeat(40), "isMain": false, "sessionIds": [],
            "runningIds": [], "lastCommitAt": iso(-DAY_MS), "inspection": clean(), "aheadOfDefault": 2,
        });
        serde_json::from_value(merged(base, overrides)).expect("an entry literal")
    }

    /// The test's `session(id, overrides)`.
    fn session(id: &str, overrides: Value) -> SessionRecord {
        let base = json!({
            "id": id, "title": id, "program": "claude", "command": "claude", "args": [], "cwd": "/w", "repoRoot": "/r",
            "status": "exited", "createdAt": iso(-30 * DAY_MS), "updatedAt": iso(-30 * DAY_MS),
        });
        serde_json::from_value(merged(base, overrides)).expect("a session literal")
    }

    fn with_inspection(fields: Value) -> Value {
        merged(clean(), fields)
    }

    const NONE: &[SessionRecord] = &[];

    // covers: B-P4-06 worktrees are grouped by what to do with them
    #[test]
    fn worktrees_are_grouped() {
        let now = now();
        // Merged by Deckhand and clean: safe; with uncommitted files: leftovers.
        let squashed = entry(json!({"markers": {"mergedAt": "t", "mergeMode": "squash", "mergeTargetBranch": "main"}}));
        assert_eq!(worktree_group(&squashed, NONE, now, None), WorktreeGroup::Ready);
        let leftovers = entry(json!({"markers": {"mergedAt": "t"},
            "inspection": with_inspection(json!({"safe": false, "dirtyFiles": 1, "reasons": ["1 modified/staged file(s)"]}))}));
        assert_eq!(worktree_group(&leftovers, NONE, now, None), WorktreeGroup::Leftovers);
        // Never checked (an error): not called safe.
        let unchecked = entry(json!({"markers": {"mergedAt": "t"}, "inspection": null, "error": "boom"}));
        assert_eq!(worktree_group(&unchecked, NONE, now, None), WorktreeGroup::Leftovers);
        // Nothing beyond the default branch counts as merged (a worktree Deckhand does not track included).
        assert_eq!(
            worktree_group(&entry(json!({"aheadOfDefault": 0})), NONE, now, None),
            WorktreeGroup::Ready
        );
        // …but a fresh branch with uncommitted work is in progress, not merged.
        let fresh = entry(json!({"aheadOfDefault": 0,
            "inspection": with_inspection(json!({"safe": false, "untrackedFiles": 1, "reasons": ["1 untracked file(s)"]}))}));
        assert_eq!(worktree_group(&fresh, NONE, now, None), WorktreeGroup::Progress);
        // Not merged: in progress, or idle once nothing ran for two weeks.
        assert_eq!(
            worktree_group(&entry(json!({})), NONE, now, None),
            WorktreeGroup::Progress
        );
        let old = entry(
            json!({"lastCommitAt": iso(-20 * DAY_MS), "sessionIds": ["s"], "recordId": "r",
            "createdAt": iso(-40 * DAY_MS)}),
        );
        assert_eq!(
            worktree_group(&old, &[session("s", json!({}))], now, None),
            WorktreeGroup::Idle
        );
        let running = WorktreeOverviewEntry {
            running_ids: vec!["s".into()],
            ..old.clone()
        };
        assert_eq!(
            worktree_group(&running, &[session("s", json!({"status": "running"}))], now, None),
            WorktreeGroup::Progress
        );
        // A session updated recently keeps it out of idle.
        assert_eq!(
            worktree_group(&old, &[session("s", json!({"updatedAt": iso(-DAY_MS)}))], now, None),
            WorktreeGroup::Progress
        );
        let gone = entry(json!({"missing": "prunable", "markers": {"mergedAt": "t"}}));
        assert_eq!(worktree_group(&gone, NONE, now, None), WorktreeGroup::Missing);
        let main = entry(json!({"isMain": true, "aheadOfDefault": 0}));
        assert_eq!(worktree_group(&main, NONE, now, None), WorktreeGroup::Main);
    }

    // covers: B-P4-07 merge labels say how it was merged
    #[test]
    fn merge_labels() {
        let text = |overrides: Value, default: Option<&str>| merged_text(&entry(overrides), default);
        assert_eq!(
            text(json!({"markers": {"mergedAt": "t", "mergeDetected": "pr"}}), None).as_deref(),
            Some("PR merged")
        );
        assert_eq!(
            text(
                json!({"markers": {"mergedAt": "t", "mergeDetected": "ancestor"}}),
                Some("main")
            )
            .as_deref(),
            Some("merged into main")
        );
        assert_eq!(
            text(json!({"markers": {"mergedAt": "t", "mergeMarkedManually": true}}), None).as_deref(),
            Some("marked merged (M)")
        );
        assert_eq!(
            text(
                json!({"markers": {"mergedAt": "t", "mergeMode": "squash", "mergeTargetBranch": "dev"}}),
                None
            )
            .as_deref(),
            Some("squash-merged into dev")
        );
        assert_eq!(
            text(json!({"aheadOfDefault": 0}), Some("main")).as_deref(),
            Some("nothing beyond main")
        );
        assert_eq!(
            text(json!({"aheadOfDefault": 0, "inspection": null}), Some("main")),
            None
        );
        assert_eq!(text(json!({}), None), None);
    }

    // covers: B-P4-08 rows: groups in order, the main checkout last, x on the ready heading deletes what may be deleted
    #[test]
    fn rows_groups_and_bulk_targets() {
        let now = now();
        let overview = WorktreeOverview {
            main_root: None,
            default_branch: Some("main".into()),
            entries: vec![
                entry(json!({"isMain": true, "branch": "main", "path": "/r", "inUse": "this"})),
                entry(json!({"branch": "wip"})),
                entry(json!({"branch": "done-a", "aheadOfDefault": 0, "lastCommitAt": iso(-3 * DAY_MS)})),
                entry(json!({"branch": "done-b", "markers": {"mergedAt": "t"}, "inUse": "other"})),
                entry(json!({"branch": "done-c", "markers": {"mergedAt": "t"}})),
            ],
            unchecked: None,
            checked_at: iso(0),
        };
        let rows = worktree_rows(Some(&overview), NONE, now);
        let shown: Vec<String> = rows
            .iter()
            .map(|row| match row {
                WorktreeRow::Heading { group, count } => format!("# {} {count}", group_key(*group)),
                WorktreeRow::Worktree { entry, .. } => entry.branch.clone(),
                WorktreeRow::Empty { text } => text.clone(),
            })
            .collect();
        assert_eq!(
            shown,
            [
                "# ready 3",
                "done-b",
                "done-c",
                "done-a",
                "# progress 1",
                "wip",
                "# main 1",
                "main"
            ]
        );
        // Another Deckhand open in one: never deleted from here.
        let targets: Vec<&str> = bulk_targets(&rows).iter().map(|entry| entry.branch.as_str()).collect();
        assert_eq!(targets, ["done-c", "done-a"]);
        assert!(delete_refusal(&overview.entries[3]).is_some_and(|text| text.contains("Another Deckhand")));
        assert!(delete_refusal(&overview.entries[0]).is_some_and(|text| text.contains("main checkout")));
        assert_eq!(delete_refusal(&overview.entries[1]), None);
        // Only the main checkout: a hint how to make one.
        let only_main = WorktreeOverview {
            entries: vec![overview.entries[0].clone()],
            ..overview.clone()
        };
        assert!(matches!(
            worktree_rows(Some(&only_main), NONE, now).last(),
            Some(WorktreeRow::Empty { .. })
        ));
    }

    // covers: B-P4-09 tags: changes, commits, sessions and age
    #[test]
    fn tags_changes_commits_sessions_age() {
        let now = now();
        let dirty = entry(json!({
            "inspection": with_inspection(json!({"safe": false, "dirtyFiles": 2, "untrackedFiles": 1, "ignoredFiles": 1})),
            "sessionIds": ["a", "b"], "runningIds": ["a"], "aheadOfDefault": 1,
        }));
        assert_eq!(
            worktree_tags(&dirty, NONE, now),
            ["3 changed", "1 ignored", "1 commit", "1 running", "1d"]
        );
        assert_eq!(
            worktree_tags(&entry(json!({"aheadOfDefault": 0})), NONE, now),
            ["not from Deckhand", "1d"]
        );
        assert_eq!(
            worktree_tags(
                &entry(json!({"recordId": "r", "sessionIds": ["a"], "aheadOfDefault": 0})),
                NONE,
                now
            ),
            ["1 session", "1d"]
        );
        let unlisted = entry(
            json!({"missing": "unlisted", "recordId": "r", "lastCommitAt": null, "inspection": null,
            "aheadOfDefault": null}),
        );
        assert_eq!(worktree_tags(&unlisted, NONE, now), ["not listed by Git"]);
    }

    #[test]
    fn constants_and_titles() {
        assert_eq!(IDLE_DAYS, 14);
        assert_eq!(group_title(WorktreeGroup::Ready), "Merged · safe to delete");
        assert_eq!(group_title(WorktreeGroup::Main), "Main checkout");
        let keys: Vec<Value> = WORKTREE_GROUPS
            .iter()
            .map(|group| serde_json::to_value(group).unwrap_or_default())
            .collect();
        assert_eq!(
            keys,
            ["ready", "leftovers", "progress", "idle", "missing", "main"].map(|key| json!(key))
        );
        for group in WORKTREE_GROUPS {
            assert_eq!(json!(group_key(group)), serde_json::to_value(group).unwrap_or_default());
        }
    }

    // ── Parity replays (fixtures/parity/worktreesModel) ──────────────────────────────────────────────────────

    /// Keep `key` only when it is one of `allowed`, else replace it with `fallback` (`None`: drop it). Recorded
    /// mutations write strings outside the TS unions; each fallback behaves as the stray value does in the TS.
    fn coerce(object: &mut Map<String, Value>, key: &str, allowed: &[&str], fallback: Option<&str>) {
        let Some(value) = object.get(key) else {
            return;
        };
        if value.as_str().is_some_and(|text| allowed.contains(&text)) {
            return;
        }
        match fallback {
            Some(fallback) => {
                object.insert(key.to_owned(), json!(fallback));
            }
            None => {
                object.remove(key);
            }
        }
    }

    /// A recorded entry as the port's type: deleted booleans are false, stray union strings coerced.
    fn entry_value(value: &Value) -> Value {
        let mut value = tags::untag(value);
        if let Some(object) = value.as_object_mut() {
            object.entry("isMain").or_insert(json!(false));
            // A stray non-empty `missing` is truthy and not 'prunable'; an empty one is falsy.
            if object
                .get("missing")
                .is_some_and(|missing| missing.as_str() == Some(""))
            {
                object.remove("missing");
            }
            coerce(object, "missing", &["prunable", "unlisted"], Some("unlisted"));
            coerce(object, "inUse", &["this", "other"], None);
            if let Some(markers) = object.get_mut("markers").and_then(Value::as_object_mut) {
                coerce(markers, "mergeMode", &["merge", "squash"], None);
                coerce(markers, "mergeDetected", &["ancestor", "pr"], None);
            }
            if let Some(inspection) = object.get_mut("inspection").and_then(Value::as_object_mut) {
                inspection.entry("safe").or_insert(json!(false));
            }
        }
        value
    }

    fn entry_arg(case: &Case, index: usize) -> Result<WorktreeOverviewEntry, String> {
        serde_json::from_value(entry_value(case.arg(index))).map_err(|error| error.to_string())
    }

    /// A recorded session list: unions this module never reads are coerced to a valid value.
    fn sessions_arg(case: &Case, index: usize) -> Result<Vec<SessionRecord>, String> {
        let value = tags::untag(case.arg(index));
        value
            .as_array()
            .ok_or("sessions: not an array")?
            .iter()
            .map(|session| {
                let mut session = session.clone();
                let object = session.as_object_mut().ok_or("session: not an object")?;
                coerce(object, "status", &["starting", "running", "exited"], Some("exited"));
                coerce(object, "program", &["claude", "codex", "pi"], Some("claude"));
                if let Some(worktree) = object.get_mut("worktree").and_then(Value::as_object_mut) {
                    coerce(worktree, "mode", &["managed", "external", "none"], Some("managed"));
                }
                if let Some(shared) = object.get_mut("sharedNotes").and_then(Value::as_object_mut) {
                    // Anything but 'worktree' fails workNote's kind check, as 'repo' does.
                    coerce(shared, "kind", &["worktree", "repo"], Some("repo"));
                }
                serde_json::from_value(session).map_err(|error| error.to_string())
            })
            .collect()
    }

    fn optional_str(case: &Case, index: usize) -> Result<Option<&str>, String> {
        if case.arg_is_undefined(index) {
            Ok(None)
        } else {
            case.arg_str(index).map(Some)
        }
    }

    fn or_undefined<T: Serialize>(value: Option<T>) -> Value {
        value.map_or_else(tags::undefined, |value| json!(value))
    }

    /// A recorded row.
    fn row_value(row: &Value) -> Result<WorktreeRow, String> {
        let group = || -> Result<WorktreeGroup, String> {
            serde_json::from_value(row.get("group").cloned().unwrap_or_default()).map_err(|error| error.to_string())
        };
        Ok(match row.get("kind").and_then(Value::as_str) {
            Some("heading") => WorktreeRow::Heading {
                group: group()?,
                count: serde_json::from_value(row.get("count").cloned().unwrap_or_default())
                    .map_err(|error| error.to_string())?,
            },
            Some("worktree") => WorktreeRow::Worktree {
                group: group()?,
                entry: Box::new(
                    serde_json::from_value(entry_value(row.get("entry").unwrap_or(&Value::Null)))
                        .map_err(|error| error.to_string())?,
                ),
            },
            Some("empty") => WorktreeRow::Empty {
                text: row.get("text").and_then(Value::as_str).unwrap_or_default().to_owned(),
            },
            other => return Err(format!("unknown row kind {other:?}")),
        })
    }

    /// A recorded row list, each entry's `path` replaced by its index into the returned recorded entries (so the
    /// entries a port returns are written back exactly as recorded).
    fn rows_arg(case: &Case, index: usize) -> Result<(Vec<WorktreeRow>, Vec<Value>), String> {
        let value = tags::untag(case.arg(index));
        let mut originals = Vec::new();
        let rows = value
            .as_array()
            .ok_or("rows: not an array")?
            .iter()
            .map(|recorded| {
                let mut row = row_value(recorded)?;
                if let WorktreeRow::Worktree { entry, .. } = &mut row {
                    originals.push(recorded.get("entry").cloned().unwrap_or_default());
                    entry.path = format!("{}", originals.len() - 1);
                }
                Ok(row)
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok((rows, originals))
    }

    #[test]
    fn worktree_name_parity() {
        replay(UNIT, "worktreesModel/worktreeName", |case: &Case| {
            entry_arg(case, 0).map(|entry| worktree_name(&entry))
        });
    }

    #[test]
    fn merged_text_parity() {
        replay(UNIT, "worktreesModel/mergedText", |case: &Case| -> Result<_, String> {
            Ok(or_undefined(merged_text(&entry_arg(case, 0)?, optional_str(case, 1)?)))
        });
    }

    #[test]
    fn last_activity_parity() {
        replay(
            UNIT,
            "worktreesModel/lastActivity",
            |case: &Case| -> Result<_, String> {
                Ok(or_undefined(last_activity(
                    &entry_arg(case, 0)?,
                    &sessions_arg(case, 1)?,
                )))
            },
        );
    }

    #[test]
    fn worktree_group_parity() {
        replay(
            UNIT,
            "worktreesModel/worktreeGroup",
            |case: &Case| -> Result<_, String> {
                Ok(worktree_group(
                    &entry_arg(case, 0)?,
                    &sessions_arg(case, 1)?,
                    case.arg_i64(2)?,
                    optional_str(case, 3)?,
                ))
            },
        );
    }

    #[test]
    fn worktree_rows_parity() {
        replay(
            UNIT,
            "worktreesModel/worktreeRows",
            |case: &Case| -> Result<_, String> {
                let original = tags::untag(case.arg(0));
                let overview: Option<WorktreeOverview> = if case.arg_is_undefined(0) {
                    None
                } else {
                    let mut value = original.clone();
                    let entries = value
                        .get_mut("entries")
                        .and_then(Value::as_array_mut)
                        .ok_or("entries: not an array")?;
                    for (index, entry) in entries.iter_mut().enumerate() {
                        *entry = entry_value(entry);
                        // The index rides in `path`'s place, so a row's entry is written back as recorded.
                        if let Some(object) = entry.as_object_mut() {
                            object.insert("path".into(), json!(format!("{index}")));
                        }
                    }
                    Some(serde_json::from_value(value).map_err(|error| error.to_string())?)
                };
                let originals = original
                    .get("entries")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let rows = worktree_rows(overview.as_ref(), &sessions_arg(case, 1)?, case.arg_i64(2)?);
                rows.into_iter()
                    .map(|row| match row {
                        WorktreeRow::Worktree { group, entry } => {
                            let index: usize = entry.path.parse().map_err(|_| "path index")?;
                            let entry = originals.get(index).cloned().ok_or("entry index")?;
                            Ok(json!({"kind": "worktree", "group": group, "entry": entry}))
                        }
                        other => serde_json::to_value(other).map_err(|error| error.to_string()),
                    })
                    .collect::<Result<Vec<Value>, String>>()
            },
        );
    }

    #[test]
    fn selectable_worktree_row_parity() {
        replay(UNIT, "worktreesModel/selectableWorktreeRow", |case: &Case| {
            row_value(&tags::untag(case.arg(0))).map(|row| selectable_worktree_row(&row))
        });
    }

    #[test]
    fn worktree_row_key_parity() {
        replay(UNIT, "worktreesModel/worktreeRowKey", |case: &Case| {
            row_value(&tags::untag(case.arg(0))).map(|row| or_undefined(worktree_row_key(&row)))
        });
    }

    #[test]
    fn delete_refusal_parity() {
        replay(
            UNIT,
            "worktreesModel/deleteRefusal",
            |case: &Case| -> Result<_, String> { Ok(or_undefined(delete_refusal(&entry_arg(case, 0)?))) },
        );
    }

    #[test]
    fn protected_branch_parity() {
        replay(
            UNIT,
            "worktreesModel/protectedBranch",
            |case: &Case| -> Result<_, String> { Ok(protected_branch(case.arg_str(0)?, optional_str(case, 1)?)) },
        );
    }

    #[test]
    fn bulk_targets_parity() {
        replay(UNIT, "worktreesModel/bulkTargets", |case: &Case| -> Result<_, String> {
            let (rows, originals) = rows_arg(case, 0)?;
            bulk_targets(&rows)
                .into_iter()
                .map(|entry| {
                    let index: usize = entry.path.parse().map_err(|_| "path index")?;
                    originals.get(index).cloned().ok_or_else(|| "entry index".to_owned())
                })
                .collect::<Result<Vec<Value>, String>>()
        });
    }

    #[test]
    fn worktree_tags_parity() {
        replay(
            UNIT,
            "worktreesModel/worktreeTags",
            |case: &Case| -> Result<_, String> {
                Ok(worktree_tags(
                    &entry_arg(case, 0)?,
                    &sessions_arg(case, 1)?,
                    case.arg_i64(2)?,
                ))
            },
        );
    }
}
